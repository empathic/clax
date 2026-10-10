//! An in-process daemon on an ephemeral port, for integration tests
//! in this crate and in crates that call the daemon over HTTP.

use crate::daemon::{browser_host, probe_host};
use crate::wrap_cache::WrapCache;
use crate::{AppState, build_router};
use clax_core::working::{Clock, ManualClock, SystemClock};
use clax_core::{EventBus, Home, Store};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// The header that makes a test daemon take a request as arriving from
/// another machine on the LAN ([`TestServer::lan`]).
pub const LAN_HEADER: &str = "x-clax-test-lan";
/// The LAN peer a request with [`LAN_HEADER`] arrives from.
pub const LAN_PEER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 7);
/// The header that names another peer address for a request with
/// [`LAN_HEADER`], so a test can speak from many LAN machines.
pub const PEER_HEADER: &str = "x-clax-test-peer";
/// The daemon's LAN interface address a request with [`LAN_HEADER`]
/// arrives on.
pub const LAN_LOCAL: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Writes fixture `sql` into `home`: runs it on a new database at the home's
/// database path, then writes each row of its `fixture_files (path,
/// content)` table to that path under the home root and drops the table.
/// A fixture is a whole database (schema, rows and `user_version`) with the
/// files its rows name (version files, clips, assets).
///
/// # Panics
/// When the home already has a database, the SQL fails, or a file path is
/// not a plain relative path.
pub fn load_fixture(home: &Home, sql: &str) {
    home.ensure_dirs().unwrap();
    assert!(
        !home.db_path().exists(),
        "{} exists",
        home.db_path().display()
    );
    let conn = rusqlite::Connection::open(home.db_path()).unwrap();
    conn.execute_batch(sql).unwrap();
    let files: Vec<(String, Vec<u8>)> = {
        let mut st = conn
            .prepare("SELECT path, content FROM fixture_files ORDER BY path")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    for (rel, bytes) in files {
        let rel = std::path::Path::new(&rel);
        assert!(
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "fixture file {} is not a plain relative path",
            rel.display()
        );
        let path = home.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    conn.execute_batch("DROP TABLE fixture_files").unwrap();
}

/// Rewrites the connection of a request carrying [`LAN_HEADER`] to one from
/// [`LAN_PEER`] to [`LAN_LOCAL`] (ports kept), so tests of the LAN rules run
/// on every machine, whatever interfaces it has.
async fn as_lan(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.headers().contains_key(LAN_HEADER) {
        let conn = req
            .extensions()
            .get::<axum::extract::ConnectInfo<crate::auth::Conn>>()
            .map(|c| c.0);
        let peer = req
            .headers()
            .get(PEER_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<IpAddr>().ok())
            .unwrap_or(IpAddr::V4(LAN_PEER));
        if let Some(c) = conn {
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(crate::auth::Conn {
                    peer: SocketAddr::new(peer, c.peer.port()),
                    local: SocketAddr::new(IpAddr::V4(LAN_LOCAL), c.local.port()),
                }));
        }
    }
    next.run(req).await
}

/// A running test daemon with its own temporary home and the fixed token `test-token`.
pub struct TestServer {
    pub base: String,
    pub token: String,
    pub home: Home,
    pub client: reqwest::Client,
    /// The server's event bus, for publishing events directly.
    pub events: EventBus,
    /// The server's store, for reading what a request left behind.
    pub store: Arc<Store>,
    /// The server's working registry.
    pub working: Arc<clax_core::working::Working>,
    /// The server's feedback context, for driving its background passes.
    pub feedback: crate::feedback::FeedbackCtx,
    /// The server's presence registry.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The address the listener is bound to (may be unspecified, e.g. `0.0.0.0`).
    pub addr: SocketAddr,
    /// The journal state the status route reports; no appender runs unless
    /// a test starts one with it.
    pub journal: Arc<crate::audit::JournalStatus>,
    /// The bounds on agents' unverified bodies.
    pub agent_gate: Arc<crate::agent::Gate>,
    extension_id: String,
    _dir: tempfile::TempDir,
}

impl TestServer {
    pub async fn spawn() -> TestServer {
        Self::spawn_with(|_| {}).await
    }

    pub async fn spawn_with(f: impl FnOnce(&mut AppState)) -> TestServer {
        Self::spawn_on(IpAddr::V4(Ipv4Addr::LOCALHOST), f).await
    }

    /// Like `spawn_with`, listening on `bind`. `base` targets `bind`, or the
    /// same-family loopback when `bind` is unspecified.
    pub async fn spawn_on(bind: IpAddr, f: impl FnOnce(&mut AppState)) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, Arc::new(SystemClock), f, false).await
    }

    /// [`TestServer::spawn_on`] with the shutdown route
    /// (`POST /api/admin/shutdown`) the daemon serves.
    pub async fn spawn_on_with_shutdown(bind: IpAddr) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, Arc::new(SystemClock), |_| {}, true).await
    }

    /// A test daemon listening on every interface (it may invite people)
    /// whose bounds on agents' unverified bodies wait and measure rates on
    /// the returned timer.
    pub async fn spawn_agent_timer() -> (TestServer, Arc<ManualTimer>) {
        Self::spawn_agent_timer_with_budget(crate::agent::BUDGET).await
    }

    /// [`TestServer::spawn_agent_timer`] with an overall budget of
    /// `budget` bytes of unverified bodies.
    pub async fn spawn_agent_timer_with_budget(budget: usize) -> (TestServer, Arc<ManualTimer>) {
        let timer = Arc::new(ManualTimer::default());
        let t = timer.clone();
        let ts = Self::spawn_on(IpAddr::V4(Ipv4Addr::UNSPECIFIED), move |s| {
            s.agent_gate = Arc::new(crate::agent::Gate::with_budget(t, budget));
        })
        .await;
        (ts, timer)
    }

    /// A test daemon whose store, working and presence registries and
    /// question clock all read `clock`.
    pub async fn spawn_with_clock(clock: Arc<ManualClock>) -> TestServer {
        Self::spawn_on_with_clock(IpAddr::V4(Ipv4Addr::LOCALHOST), clock).await
    }

    /// [`TestServer::spawn_with_clock`] listening on `bind` (`0.0.0.0` for
    /// one that may invite people: its invites name [`LAN_LOCAL`]).
    pub async fn spawn_on_with_clock(bind: IpAddr, clock: Arc<ManualClock>) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        Self::start(dir, bind, clock, |_| {}, false).await
    }

    /// A test daemon on a fresh copy of fixture `name` (a file of this
    /// crate's `tests/fixtures/`, loaded by [`load_fixture`]), its clock
    /// ([`TestServer::spawn_with_clock`]) stopped at `at` (RFC 3339). The
    /// store migrates the fixture's database on open, as a daemon does.
    ///
    /// # Panics
    /// When the fixture cannot be read or loaded, or `at` does not parse.
    pub async fn from_fixture_with_clock(name: &str, at: &str) -> TestServer {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let dir = tempfile::tempdir().unwrap();
        load_fixture(&Home::at(dir.path().join("ax")), &sql);
        let clock = Arc::new(ManualClock::at(at));
        Self::start(dir, IpAddr::V4(Ipv4Addr::LOCALHOST), clock, |_| {}, false).await
    }

    /// Serves the home `ax` under `dir` on `bind`, every time read from `clock`.
    async fn start(
        dir: tempfile::TempDir,
        bind: IpAddr,
        clock: Arc<dyn Clock>,
        f: impl FnOnce(&mut AppState),
        shutdown: bool,
    ) -> TestServer {
        let home = Home::at(dir.path().join("ax"));
        let store = Arc::new(Store::open_with_clock(&home, clock.clone()).unwrap());
        let token = "test-token".to_string();
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let live_ids = Arc::new(crate::live::LiveIds::load(&store).unwrap());
        let ext_creds =
            Arc::new(crate::extension::Credentials::load_with(&store, clock.clone()).unwrap());
        let started_at = store.now();
        let grant_auth = Arc::new(crate::grant_auth::GrantAuth::load(&store).unwrap());
        let vis_gen = crate::state::VisGen::install(&store);
        let mut state = AppState {
            store,
            home: home.clone(),
            token: token.clone(),
            events: EventBus::new(),
            started_at,
            version: "test",
            shutdown: tokio::sync::watch::channel(false).1,
            wrap_cache: Arc::new(WrapCache::new(crate::wrap_cache::DEFAULT_MAX_BYTES)),
            request_timeout: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(120),
            sse_keep_alive: Duration::from_secs(15),
            self_base: format!("http://{}:{port}", probe_host(&bind.to_string())),
            browser_base: format!("http://{}:{port}", browser_host(&bind.to_string())),
            feedback_waiters: Arc::new(Default::default()),
            followers: Arc::new(Default::default()),
            // Push off; tests about push set it with `spawn_with`.
            codex: Arc::new(Default::default()),
            working: Arc::new(clax_core::working::Working::new(clock.clone())),
            presence: Arc::new(clax_core::presence::Presence::new(clock.clone())),
            rooms: Arc::new(crate::room::Rooms::default()),
            // Sampling off; tests that sample set their own with `spawn_with`.
            sample: Arc::new(crate::sample::Sampler::disabled()),
            stream: crate::stream::Hub::new(live_ids.clone()),
            live_ids,
            ext_creds,
            extension_id: clax_core::extension::extension_id_in_effect(home.root()),
            questions: Arc::new(Default::default()),
            question_grace: Duration::from_secs(5),
            question_sleeper: Arc::new(crate::questions::TokioSleeper),
            question_clock: clock,
            terminal_after_s: clax_core::config::TERMINAL_AFTER_S,
            calibration: Arc::default(),
            audit_wake: crate::audit::AuditWake::new(),
            // No appender; tests of the journal start their own.
            journal: crate::audit::JournalStatus::off(None),
            exports: crate::routes::toolpath::Exports::new(),
            call_ids: Arc::new(crate::routes::sessions::CallIdWait::new(
                crate::routes::sessions::CALL_ID_GRACE,
                crate::routes::sessions::CALL_ID_CAP,
            )),
            bind,
            port,
            // Invites from a daemon on every interface name the address
            // `lan()` requests arrive on.
            lan_host: bind.is_unspecified().then_some(IpAddr::V4(LAN_LOCAL)),
            grant_auth,
            agent_gate: Arc::new(crate::agent::Gate::new(Arc::new(
                crate::agent::TokioTimer::default(),
            ))),
            vis_gen,
        };
        f(&mut state);
        state.audit_wake.install(&state.store);
        state.stream.listen(&state.events);
        crate::inbox::listen(&state);
        let events = state.events.clone();
        let store = state.store.clone();
        let working = state.working.clone();
        let feedback = state.feedback_ctx();
        let presence = state.presence.clone();
        let extension_id = state.extension_id.clone();
        let journal = state.journal.clone();
        let agent_gate = state.agent_gate.clone();
        let app = if shutdown {
            crate::build_router_with_shutdown(state, tokio::sync::watch::channel(false).0)
        } else {
            build_router(state)
        }
        .layer(axum::middleware::from_fn(as_lan));
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<crate::auth::Conn>(),
            )
            .await
            .unwrap();
        });
        TestServer {
            base: format!("http://{}:{port}", probe_host(&bind.to_string())),
            token,
            home,
            client: reqwest::Client::new(),
            events,
            store,
            working,
            feedback,
            presence,
            addr,
            journal,
            agent_gate,
            extension_id,
            _dir: dir,
        }
    }

    /// A client whose requests reach this daemon as from another machine on
    /// the LAN (peer [`LAN_PEER`], arriving on [`LAN_LOCAL`] with that
    /// address as their `Host`), and the base URL to send them to. A request
    /// may still set its own `Host`.
    pub fn lan(&self) -> (reqwest::Client, String) {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(LAN_HEADER, reqwest::header::HeaderValue::from_static("1"));
        h.insert(
            reqwest::header::HOST,
            format!("{LAN_LOCAL}:{}", self.addr.port()).parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .unwrap();
        (client, self.base.clone())
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            .unwrap()
    }

    /// A GET carrying the bearer token.
    pub async fn get_authed(&self, path: &str) -> reqwest::Response {
        self.authed(self.client.get(format!("{}{}", self.base, path)))
            .send()
            .await
            .unwrap()
    }

    pub fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    pub async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.authed(
            self.client
                .post(format!("{}{}", self.base, path))
                .json(&body),
        )
        .send()
        .await
        .unwrap()
    }

    /// Publishes a new artifact; `files` are (path, utf8 content).
    pub async fn publish(&self, title: &str, files: &[(&str, &str)]) -> serde_json::Value {
        let files: serde_json::Map<String, serde_json::Value> = files
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    serde_json::json!({"content": v, "encoding": "utf8"}),
                )
            })
            .collect();
        let res = self
            .post_json(
                "/api/artifacts",
                serde_json::json!({"title": title, "files": files}),
            )
            .await;
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
        res.json().await.unwrap()
    }
}

/// Bytes that pass the daemon's PNG check (signature only); not a decodable image.
pub const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nclax-test-clip";

/// An element anchor on `body > main > h2` quoting "Quarterly goals".
pub fn element_anchor() -> serde_json::Value {
    serde_json::json!({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null})
}

/// Reads Server-Sent Events from one `/api/events` or `/api/stream` response.
pub struct EventReader {
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + Send>>,
    buf: String,
    /// Whether `ready` events are returned rather than skipped.
    keep_ready: bool,
}

impl EventReader {
    /// A reader of `res`'s body that returns every event, `ready` included
    /// (an `/api/stream` response opens with `ready`, naming the stream).
    pub fn from_response(res: reqwest::Response) -> EventReader {
        use futures::StreamExt;
        let stream = res
            .bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));
        EventReader {
            stream: Box::pin(stream),
            buf: String::new(),
            keep_ready: true,
        }
    }

    /// The next event other than keep-alive comments (and `ready`, unless
    /// made by [`EventReader::from_response`]), as (name, data), within 20 s.
    pub async fn next(&mut self) -> (String, serde_json::Value) {
        use futures::StreamExt;
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block = self.buf[..end].to_string();
                self.buf.drain(..end + 2);
                if block.starts_with(':') {
                    continue;
                }
                let name = block
                    .lines()
                    .find_map(|l| l.strip_prefix("event: "))
                    .unwrap_or("message")
                    .to_string();
                let data = block
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .unwrap_or("null");
                if name == "ready" && !self.keep_ready {
                    continue;
                }
                return (
                    name,
                    serde_json::from_str(data).expect("event data is JSON"),
                );
            }
            let chunk = tokio::time::timeout(Duration::from_secs(20), self.stream.next())
                .await
                .expect("SSE chunk within 20 s")
                .expect("stream still open")
                .expect("chunk readable");
            self.buf
                .push_str(std::str::from_utf8(&chunk).expect("UTF-8 events"));
        }
    }

    /// The names of the events (keep-alive comments aside) until the body
    /// ends; panics when it has not ended within 20 s.
    pub async fn rest(&mut self) -> Vec<String> {
        use futures::StreamExt;
        let mut names = vec![];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            while let Some(end) = self.buf.find("\n\n") {
                let block = self.buf[..end].to_string();
                self.buf.drain(..end + 2);
                if let Some(n) = block.lines().find_map(|l| l.strip_prefix("event: ")) {
                    names.push(n.to_string());
                }
            }
            let chunk = tokio::time::timeout_at(deadline, self.stream.next())
                .await
                .expect("the stream ends within 20 s");
            match chunk {
                Some(Ok(c)) => self
                    .buf
                    .push_str(std::str::from_utf8(&c).expect("UTF-8 events")),
                _ => return names,
            }
        }
    }

    /// Skips events until one named `name`; returns its data.
    pub async fn next_named(&mut self, name: &str) -> serde_json::Value {
        loop {
            let (n, d) = self.next().await;
            if n == name {
                return d;
            }
        }
    }
}

impl TestServer {
    /// Registers a live session; returns the session object.
    pub async fn register_session(&self, harness: &str, hsid: &str) -> serde_json::Value {
        let res = self
            .post_json(
                "/api/sessions",
                serde_json::json!({"harness": harness, "harness_session_id": hsid, "cwd": "/w"}),
            )
            .await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["session"].clone()
    }

    /// Creates an artifact attributed to `session_id` (so the session owns and watches it).
    pub async fn publish_as(&self, session_id: &str, title: &str, html: &str) -> serde_json::Value {
        let res = self
            .authed(self.client.post(format!("{}/api/artifacts", self.base)))
            .header("x-clax-session", session_id)
            .json(&serde_json::json!({"title": title, "files": {"index.html": {"content": html, "encoding": "utf8"}}}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json().await.unwrap()
    }

    /// `POST /api/artifacts/<aid>/threads` as a browser does, with [`element_anchor`].
    pub async fn create_thread(
        &self,
        aid: &str,
        version: u32,
        body: &str,
        clip: Option<&[u8]>,
    ) -> reqwest::Response {
        let mut form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", version.to_string());
        if let Some(bytes) = clip {
            form = form.part(
                "clip",
                reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        }
        self.client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    /// Creates a thread without a clip; returns the thread view.
    pub async fn thread(&self, aid: &str, version: u32, body: &str) -> serde_json::Value {
        let res = self.create_thread(aid, version, body, None).await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Presses "Send to agent"; returns the thread view.
    pub async fn send_thread(&self, aid: &str, tid: &str) -> serde_json::Value {
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                self.base
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Opens `/api/events<query>` and returns a reader past nothing yet.
    pub async fn events(&self, query: &str) -> EventReader {
        self.events_as(query, None).await
    }

    /// Like [`TestServer::events`], as the viewer whose cookie value is `cookie`.
    pub async fn events_as(&self, query: &str, cookie: Option<&str>) -> EventReader {
        self.events_with(query, |r| match cookie {
            Some(c) => r.header("cookie", format!("clax_viewer={c}")),
            None => r,
        })
        .await
    }

    /// Opens `/api/events<query>` with the request shaped by `build` (headers
    /// such as a cookie or the token; `query` may carry `&token=`).
    pub async fn events_with(
        &self,
        query: &str,
        build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> EventReader {
        use futures::StreamExt;
        let res = build(self.client.get(format!("{}/api/events{query}", self.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let stream = res
            .bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));
        EventReader {
            stream: Box::pin(stream),
            buf: String::new(),
            keep_ready: false,
        }
    }
}

/// A browser viewer created through `GET /api/viewers/me`: its cookie value
/// (send it as `Cookie: clax_viewer=<cookie>`) and its public ID.
pub struct TestViewer {
    pub cookie: String,
    pub public_id: String,
    base: String,
    client: reqwest::Client,
}

impl TestViewer {
    /// Opens `/api/stream` with this viewer's cookie and subscribes it to
    /// `topics`; the status of the subscription request.
    pub async fn subscribe_status(&self, topics: &[&str]) -> u16 {
        let cookie = format!("clax_viewer={}", self.cookie);
        let res = self
            .client
            .get(format!("{}/api/stream", self.base))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut events = EventReader::from_response(res);
        let id = events.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        self.client
            .post(format!("{}/api/stream/{id}", self.base))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "subscribe": topics }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

impl TestViewer {
    /// `PUT /api/viewers/me/looked`: this viewer looked at `tids` of `aid`;
    /// asserts a success.
    pub async fn look(&self, aid: &str, tids: &[&str]) {
        let res = self
            .client
            .put(format!("{}/api/viewers/me/looked", self.base))
            .header("cookie", format!("clax_viewer={}", self.cookie))
            .json(&serde_json::json!({"artifact_id": aid, "thread_ids": tids}))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }
}

impl TestServer {
    /// Creates a viewer, named `name` when given.
    pub async fn viewer(&self, name: Option<&str>) -> TestViewer {
        let res = self.get("/api/viewers/me").await;
        let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
        let cookie = set
            .split(';')
            .next()
            .and_then(|kv| kv.strip_prefix("clax_viewer="))
            .expect("clax_viewer cookie")
            .to_string();
        let mut v: serde_json::Value = res.json().await.unwrap();
        if let Some(n) = name {
            let res = self
                .client
                .put(format!("{}/api/viewers/me", self.base))
                .header("cookie", format!("clax_viewer={cookie}"))
                .json(&serde_json::json!({"display_name": n}))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            v = res.json().await.unwrap();
        }
        TestViewer {
            cookie,
            public_id: v["viewer"]["public_id"].as_str().unwrap().to_string(),
            base: self.base.clone(),
            client: self.client.clone(),
        }
    }

    /// The owner identity's public ID, made as a browser of the owner's
    /// makes it when there is none yet.
    pub async fn owner_public_id(&self) -> String {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/api/viewers/me", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["viewer"]["public_id"].as_str().unwrap().to_string()
    }

    /// The owner viewer, as a browser of the owner's sees it (made as a
    /// browser's when there is none yet).
    pub async fn owner_viewer(&self) -> clax_core::model::Viewer {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/api/viewers/me", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        serde_json::from_value(v["viewer"].clone()).unwrap()
    }

    /// The extension ID in effect for this server's home (spec
    /// 2026-10-05-chrome-overlay-design L15).
    pub fn extension_id(&self) -> String {
        self.extension_id.clone()
    }

    /// The owner cookie a browser of the owner's holds (`name=value`), as the
    /// shell's token request sets it.
    pub fn owner_cookie(&self) -> String {
        let host = self.base.trim_start_matches("http://");
        format!(
            "{}={}",
            crate::identity::owner_cookie_name(host),
            crate::identity::owner_cookie_value(&self.token)
        )
    }

    /// Creates a thread as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn thread_as(&self, aid: &str, cookie: &str, body: &str) -> serde_json::Value {
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self
            .client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Replies on `tid` as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn reply_as(
        &self,
        aid: &str,
        tid: &str,
        cookie: &str,
        body: &str,
    ) -> serde_json::Value {
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.base
            ))
            .header("cookie", format!("clax_viewer={cookie}"))
            .json(&serde_json::json!({"body": body}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }
}

/// A paired extension's requests ([`TestServer::extension`]): from its
/// origin, with its credential, as the extension's service worker sends
/// them.
pub struct TestExtension {
    base: String,
    client: reqwest::Client,
    origin: String,
    credential: String,
}

impl TestExtension {
    fn request(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(m, format!("{}{path}", self.base))
            .header("origin", &self.origin)
            .header("sec-fetch-site", "cross-site")
            .header(
                "authorization",
                format!("Clax-Extension {}", self.credential),
            )
    }

    /// `GET <path>` as the extension.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    /// `POST <path>` with JSON `body` as the extension.
    pub async fn post(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
}

impl TestServer {
    /// `POST /api/sessions/<sid>/questions` with `body`; the response body,
    /// after asserting a 200 or 201.
    pub async fn ask(&self, sid: &str, body: serde_json::Value) -> serde_json::Value {
        let res = self
            .post_json(&format!("/api/sessions/{sid}/questions"), body)
            .await;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap();
        assert!(status == 200 || status == 201, "{status}: {v}");
        v
    }

    /// Answers question `qid` as the owner through the shell; asserts 200.
    pub async fn answer_question(&self, qid: &str, body: serde_json::Value) {
        let res = self.answer_question_raw(qid, body).await;
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// Answers question `qid` as the owner through the shell: `POST
    /// /api/questions/<qid>/answer` with the owner cookie.
    pub async fn answer_question_raw(
        &self,
        qid: &str,
        body: serde_json::Value,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api/questions/{qid}/answer", self.base))
            .header("cookie", self.owner_cookie())
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// Opens `/api/stream` as a browser of the owner's (the owner cookie)
    /// and subscribes it to `topics`; a reader past nothing yet. While the
    /// reader is held, or for the hub's grace after, an owner surface is
    /// open.
    pub async fn stream_as_owner(&self, topics: &[&str]) -> EventReader {
        let res = self
            .client
            .get(format!("{}/api/stream", self.base))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut events = EventReader::from_response(res);
        let id = events.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let res = self
            .client
            .post(format!("{}/api/stream/{id}", self.base))
            .header("cookie", self.owner_cookie())
            .json(&serde_json::json!({ "subscribe": topics }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
        events
    }

    /// How many `wait_for_feedback` polls of session `sid` are in progress.
    pub async fn feedback_waiters(&self, sid: &str) -> u64 {
        let v: serde_json::Value = self
            .get_authed(&format!("/api/_test/sessions/{sid}/feedback/waiters"))
            .await
            .json()
            .await
            .unwrap();
        v["count"].as_u64().expect("a count")
    }

    /// Returns once `n` `wait_for_feedback` polls of session `sid` are in
    /// progress; panics when the daemon gives up waiting (5 s).
    pub async fn wait_feedback_waiters(&self, sid: &str, n: u64) {
        let v: serde_json::Value = self
            .get_authed(&format!(
                "/api/_test/sessions/{sid}/feedback/waiters?until={n}"
            ))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(v["count"], n, "{n} feedback polls of {sid} within 5 s");
    }

    /// Returns once `n` feedback polls of session `sid` with tier `tier`
    /// are parked waiting to be woken; panics when the daemon gives up
    /// waiting (5 s).
    pub async fn wait_parked_feedback_polls(&self, sid: &str, tier: &str, n: u64) {
        let v: serde_json::Value = self
            .get_authed(&format!(
                "/api/_test/sessions/{sid}/feedback/waiters?parked={tier}&until={n}"
            ))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(v["count"], n, "{n} parked {tier} polls of {sid} within 5 s");
    }

    /// A paired extension: a fresh credential for this server's extension
    /// ID, whose requests come from the extension's origin.
    pub async fn extension(&self) -> TestExtension {
        let v: serde_json::Value = self
            .authed(
                self.client
                    .post(format!("{}/api/extension/credentials", self.base)),
            )
            .json(&serde_json::json!({"extension_id": self.extension_id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        TestExtension {
            base: self.base.clone(),
            client: self.client.clone(),
            origin: clax_core::extension::extension_origin(&self.extension_id),
            credential: v["credential"].as_str().expect("a credential").to_string(),
        }
    }

    /// Question `qid`'s stored status.
    pub async fn question_status(&self, qid: &str) -> String {
        self.store
            .question(qid)
            .unwrap()
            .expect("the question exists")
            .status
            .as_str()
            .to_string()
    }

    /// Ends session `sid` as its harness does; asserts 200.
    pub async fn end_session(&self, sid: &str) {
        let res = self
            .authed(
                self.client
                    .patch(format!("{}/api/sessions/{sid}", self.base))
                    .json(&serde_json::json!({"ended": true})),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    /// How many polls hold question `qid` now.
    pub async fn question_waiters(&self, qid: &str) -> u64 {
        let v: serde_json::Value = self
            .get_authed(&format!("/api/_test/questions/{qid}/waiters"))
            .await
            .json()
            .await
            .unwrap();
        v["count"].as_u64().expect("a count")
    }

    /// Returns once `n` polls hold question `qid`; panics when the daemon
    /// gives up waiting (5 s).
    pub async fn wait_question_waiters(&self, qid: &str, n: u64) {
        let v: serde_json::Value = self
            .get_authed(&format!("/api/_test/questions/{qid}/waiters?until={n}"))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(v["count"], n, "{n} polls of {qid} within 5 s");
    }
}

impl TestServer {
    /// A thread the owner leaves (in a browser of the owner's) on version 1
    /// of `aid` and sends to the agent; returns the thread view.
    pub async fn thread_as_owner(&self, aid: &str, body: &str) -> serde_json::Value {
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self
            .client
            .post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .header("cookie", self.owner_cookie())
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        let t: serde_json::Value = res.json().await.unwrap();
        let tid = t["thread"]["id"].as_str().unwrap();
        let res = self
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                self.base
            ))
            .header("cookie", self.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// An agent reply of session `sid` on thread `tid` of `aid`, through the
    /// comments route; returns the comment.
    pub async fn reply_as_agent(
        &self,
        sid: &str,
        aid: &str,
        tid: &str,
        body: &str,
    ) -> serde_json::Value {
        let res = self
            .authed(self.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.base
            )))
            .header("x-clax-session", sid)
            .json(&serde_json::json!({"body": body, "author_kind": "agent"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
        res.json::<serde_json::Value>().await.unwrap()["comment"].clone()
    }

    /// `PUT /api/viewers/me/looked` as a browser of the owner's; asserts a
    /// success.
    pub async fn look_as_owner(&self, aid: &str, tids: &[&str]) {
        let res = self
            .client
            .put(format!("{}/api/viewers/me/looked", self.base))
            .header("cookie", self.owner_cookie())
            .json(&serde_json::json!({"artifact_id": aid, "thread_ids": tids}))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }

    /// `PUT /api/sessions/<sid>/working/<aid>` with `body`; asserts 200.
    pub async fn put_working(&self, sid: &str, aid: &str, body: serde_json::Value) {
        let res = self
            .authed(
                self.client
                    .put(format!("{}/api/sessions/{sid}/working/{aid}", self.base)),
            )
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// `DELETE /api/sessions/<sid>/working/<aid>`: the agent says it is
    /// done; asserts 200.
    pub async fn delete_working(&self, sid: &str, aid: &str) {
        let res = self
            .authed(
                self.client
                    .delete(format!("{}/api/sessions/{sid}/working/{aid}", self.base)),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    }

    /// Moves the working clock `secs` forward and sweeps (the debug skew
    /// route); asserts 200.
    pub async fn skew_working(&self, secs: i64) {
        let res = self
            .post_json("/api/_test/working/skew", serde_json::json!({"secs": secs}))
            .await;
        assert_eq!(res.status(), 200);
    }

    /// Deletes thread `tid` of `aid` as the owner (the token); asserts a
    /// success.
    pub async fn delete_thread_as_owner(&self, aid: &str, tid: &str) {
        let res = self
            .authed(
                self.client
                    .delete(format!("{}/api/artifacts/{aid}/threads/{tid}", self.base)),
            )
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }
}

/// A [`crate::questions::Sleeper`] whose sleeps end only when the test
/// fires them, by duration ([`ManualSleeper::fire`]).
#[derive(Default)]
pub struct ManualSleeper {
    pending: std::sync::Mutex<Vec<(Duration, tokio::sync::oneshot::Sender<()>)>>,
    changed: tokio::sync::Notify,
}

impl crate::questions::Sleeper for ManualSleeper {
    fn sleep(&self, d: Duration) -> futures::future::BoxFuture<'static, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().push((d, tx));
        self.changed.notify_waiters();
        Box::pin(async move {
            // A dropped sleeper never fires.
            if rx.await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

impl ManualSleeper {
    /// How many sleeps of `d` are waiting (a sleep whose future was
    /// dropped is not).
    pub fn waiting(&self, d: Duration) -> usize {
        let mut p = self.pending.lock().unwrap();
        p.retain(|(_, tx)| !tx.is_closed());
        p.iter().filter(|(x, _)| *x == d).count()
    }

    /// Returns once `n` sleeps of `d` are waiting; panics after 5 s.
    pub async fn wait_for(&self, d: Duration, n: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.waiting(d) == n {
                return;
            }
            tokio::time::timeout_at(deadline, changed)
                .await
                .unwrap_or_else(|_| {
                    panic!("{n} sleeps of {d:?} within 5 s, have {}", self.waiting(d))
                });
        }
    }

    /// Ends every waiting sleep of `d`; how many.
    pub fn fire(&self, d: Duration) -> usize {
        let mut p = self.pending.lock().unwrap();
        let (due, keep): (Vec<_>, Vec<_>) = p.drain(..).partition(|(x, _)| *x == d);
        *p = keep;
        due.into_iter()
            .filter_map(|(_, tx)| tx.send(()).ok())
            .count()
    }
}

/// A [`crate::agent::Timer`] whose time moves only when the test advances
/// it ([`ManualTimer::advance`]).
#[derive(Default)]
pub struct ManualTimer {
    state: std::sync::Mutex<TimerState>,
}

/// The time a [`ManualTimer`] reads, and each pending sleep's deadline.
type TimerState = (Duration, Vec<(Duration, tokio::sync::oneshot::Sender<()>)>);

impl crate::agent::Timer for ManualTimer {
    fn now(&self) -> Duration {
        self.state.lock().unwrap().0
    }

    fn sleep_until(&self, at: Duration) -> futures::future::BoxFuture<'static, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut st = self.state.lock().unwrap();
            if at <= st.0 {
                let _ = tx.send(());
            } else {
                st.1.push((at, tx));
            }
        }
        Box::pin(async move {
            if rx.await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

impl ManualTimer {
    /// Moves time on by `d`, ending every sleep due by then; how many.
    pub fn advance(&self, d: Duration) -> usize {
        let mut st = self.state.lock().unwrap();
        st.0 += d;
        let now = st.0;
        let (due, keep): (Vec<_>, Vec<_>) = st.1.drain(..).partition(|(at, _)| *at <= now);
        st.1 = keep;
        due.into_iter()
            .filter_map(|(_, tx)| tx.send(()).ok())
            .count()
    }
}

impl TestServer {
    /// Returns once the agent gate's state satisfies `f`; panics after 5 s.
    pub async fn wait_gate(
        &self,
        what: &str,
        f: impl Fn(crate::agent::GateState) -> bool,
    ) -> crate::agent::GateState {
        self.agent_gate
            .wait_until(f, Duration::from_secs(5))
            .await
            .unwrap_or_else(|| panic!("{what} within 5 s: {:?}", self.agent_gate.state()))
    }

    /// `GET /api/_test/agent/gate`: what the agent gate holds.
    pub async fn gate(&self) -> serde_json::Value {
        let res = self.get_authed("/api/_test/agent/gate").await;
        assert_eq!(res.status(), 200);
        res.json().await.unwrap()
    }

    /// Registers a live session with parent PID `pid`; the session.
    pub async fn register_session_pid(&self, harness: &str, hsid: &str, pid: u32) -> String {
        let res = self
            .post_json(
                "/api/sessions",
                serde_json::json!({"harness": harness, "harness_session_id": hsid,
                                   "cwd": "/w", "parent_pid": pid}),
            )
            .await;
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The ID of an artifact the owner published, made for the asking.
    pub async fn any_artifact(&self) -> String {
        self.publish("any", &[("index.html", "<p>any</p>")]).await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

impl TestServer {
    /// A test daemon whose question polls and graces run on the returned
    /// clock (the grace is 5 s; a poll of `wait=n` sleeps `n` s).
    pub async fn spawn_question_clock() -> (TestServer, Arc<ManualSleeper>) {
        let clock = Arc::new(ManualSleeper::default());
        let c = clock.clone();
        let ts = Self::spawn_with(move |s| s.question_sleeper = c).await;
        (ts, clock)
    }
}

/// A browser of the owner's ([`TestServer::owner_browser`]): requests with
/// the owner cookie, from this machine.
pub struct OwnerBrowser {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl OwnerBrowser {
    fn request(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(m, format!("{}{path}", self.base))
            .header("cookie", &self.cookie)
    }

    /// `GET <path>`.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    /// `POST <path>` with JSON `body`.
    pub async fn post(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// `DELETE <path>`.
    pub async fn delete(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::DELETE, path)
            .send()
            .await
            .unwrap()
    }
}

/// One signed request of a [`TestClient`], as [`TestClient::call`] sends
/// it; the defaults sign it correctly with the client's key.
#[derive(Default)]
pub struct Call<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// `Authorization`, whole (`GNAP cxc_…`).
    pub authorization: Option<String>,
    /// The token the signature covers; by default the one in
    /// `authorization`.
    pub signed_token: Option<String>,
    pub body: Option<Vec<u8>>,
    /// Sign with this key instead of the client's.
    pub key: Option<&'a clax_core::sig::KeyPair>,
    /// Send no `Clax-Signature`.
    pub unsigned: bool,
    /// Flip a bit of the signature.
    pub corrupt: bool,
    /// Seconds added to `t`.
    pub skew: i64,
    /// The nonce; a fresh one by default.
    pub nonce: Option<String>,
    /// More headers, signed when they are agent headers.
    pub headers: Vec<(&'static str, String)>,
    /// Send these headers instead of `headers`, which are still the ones
    /// signed.
    pub sent_headers: Option<Vec<(&'static str, String)>>,
    /// Send another method than the one signed.
    pub sent_method: Option<&'a str>,
    /// Send to another path and query than the one signed.
    pub sent_path: Option<&'a str>,
    /// Send another `Host` than the one signed.
    pub sent_host: Option<String>,
    /// Send another body than the one signed.
    pub sent_body: Option<Vec<u8>>,
}

/// The tokens a [`TestClient`] holds.
#[derive(Clone, Debug, Default)]
pub struct Held {
    pub grant_id: Option<String>,
    pub access: Option<String>,
    pub manage: Option<String>,
    pub continue_token: Option<String>,
    /// The last grant response that issued tokens.
    pub issued: Option<serde_json::Value>,
}

/// Another machine's Clax ([`TestServer::connect_person`]): an Ed25519 key
/// pair that signs every request (spec §6.6), sent from the LAN
/// ([`TestServer::lan`]) to this machine's loopback address, which it
/// names as its `Host`.
pub struct TestClient {
    base: String,
    host: String,
    client: reqwest::Client,
    store: Arc<Store>,
    pub key: clax_core::sig::KeyPair,
    held: std::sync::Mutex<Held>,
    /// The LAN address it speaks from ([`LAN_PEER`] unless set).
    peer: Option<IpAddr>,
}

impl TestClient {
    /// A client of `ts` with a new key pair and no grant yet.
    pub fn new(ts: &TestServer) -> TestClient {
        Self::with_key(ts, clax_core::sig::KeyPair::generate())
    }

    /// A client of `ts` with key pair `key` and no grant yet.
    pub fn with_key(ts: &TestServer, key: clax_core::sig::KeyPair) -> TestClient {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(LAN_HEADER, reqwest::header::HeaderValue::from_static("1"));
        TestClient {
            base: ts.base.clone(),
            host: ts.base.trim_start_matches("http://").to_string(),
            client: reqwest::Client::builder()
                .default_headers(h)
                .build()
                .unwrap(),
            store: ts.store.clone(),
            key,
            held: Default::default(),
            peer: None,
        }
    }

    /// The same client speaking from this machine (a loopback peer, as
    /// through a tunnel or a local proxy).
    pub fn on_loopback(mut self) -> TestClient {
        self.client = reqwest::Client::new();
        self
    }

    /// The same client speaking from LAN address `ip`.
    pub fn from_peer(mut self, ip: IpAddr) -> TestClient {
        self.peer = Some(ip);
        self
    }

    /// The tokens held now.
    pub fn held(&self) -> Held {
        self.held.lock().unwrap().clone()
    }

    /// The grant's ID; panics without one.
    pub fn grant_id(&self) -> String {
        self.held().grant_id.expect("a grant")
    }

    /// Sends `c`, signed as it says.
    pub async fn call(&self, c: Call<'_>) -> reqwest::Response {
        use clax_core::sig::{SignInput, new_nonce};
        use sha2::Digest;
        let body = c.body.unwrap_or_default();
        let signed_token = c.signed_token.or_else(|| {
            c.authorization
                .as_deref()
                .and_then(|a| a.split_once(' '))
                .map(|(_, t)| t.to_string())
        });
        let method =
            reqwest::Method::from_bytes(c.sent_method.unwrap_or(c.method).as_bytes()).unwrap();
        let mut req = self
            .client
            .request(
                method,
                format!("{}{}", self.base, c.sent_path.unwrap_or(c.path)),
            )
            .header("host", c.sent_host.as_deref().unwrap_or(&self.host));
        if let Some(ip) = self.peer {
            req = req.header(PEER_HEADER, ip.to_string());
        }
        if !c.unsigned {
            let nonce = c.nonce.unwrap_or_else(new_nonce);
            let input = SignInput {
                method: c.method,
                path_and_query: c.path,
                host: &self.host,
                t: self.store.now_utc().timestamp() + c.skew,
                nonce: &nonce,
                body_sha256: sha2::Sha256::digest(&body).into(),
                token_sha256: signed_token.as_deref().map(clax_core::secret::sha256),
                agent_headers_sha256: clax_core::sig::agent_headers_sha256(
                    &clax_core::sig::agent_header_values(
                        c.headers.iter().map(|(k, v)| (*k, v.as_bytes())),
                    )
                    .expect("each agent header once"),
                ),
            };
            let mut h = c.key.unwrap_or(&self.key).sign(&input).unwrap();
            if c.corrupt {
                h.sig[0] ^= 1;
            }
            req = req.header("clax-signature", h.to_string());
        }
        if let Some(a) = c.authorization {
            req = req.header("authorization", a);
        }
        let body = c.sent_body.unwrap_or(body);
        if !body.is_empty() {
            req = req.header("content-type", "application/json");
        }
        for (k, v) in c.sent_headers.unwrap_or(c.headers) {
            req = req.header(k, v);
        }
        req.body(body).send().await.unwrap()
    }

    /// `body` with `client` (this key, named `test-machine`) when it has
    /// none, and `interact.start: ["user_code"]` when it has neither that nor
    /// an invite.
    pub fn grant_body(&self, mut body: serde_json::Value) -> serde_json::Value {
        if body.get("client").is_none() {
            body["client"] = serde_json::json!({
                "key": {"proof": "clax-ed25519", "jwk": {"kty": "OKP", "crv": "Ed25519",
                        "x": clax_core::sig::encode_public(&self.key.public())}},
                "display": {"name": "test-machine"},
            });
        }
        if body.get("interact").is_none() && body.pointer("/clax/invite").is_none() {
            body["interact"] = serde_json::json!({"start": ["user_code"]});
        }
        body
    }

    /// `POST /api/grants` with [`TestClient::grant_body`] of `body`,
    /// signed; keeps the tokens a success hands out.
    pub async fn grant_request_raw(&self, body: serde_json::Value) -> reqwest::Response {
        let body = serde_json::to_vec(&self.grant_body(body)).unwrap();
        self.call(Call {
            method: "POST",
            path: "/api/grants",
            body: Some(body),
            ..Default::default()
        })
        .await
    }

    /// [`TestClient::grant_request_raw`], asserting a 200 or 201; the
    /// response, whose tokens the client keeps.
    pub async fn grant_request(&self, body: serde_json::Value) -> serde_json::Value {
        let res = self.grant_request_raw(body).await;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap();
        assert!(status == 200 || status == 201, "{status}: {v}");
        self.keep(&v);
        v
    }

    /// Keeps the grant ID and tokens grant response `v` hands out.
    pub fn keep(&self, v: &serde_json::Value) {
        let mut h = self.held.lock().unwrap();
        let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).map(str::to_string);
        if let Some(uri) = s("/continue/uri") {
            h.grant_id = uri
                .strip_prefix("/api/grants/")
                .and_then(|r| r.strip_suffix("/continue"))
                .map(str::to_string);
        }
        if let Some(t) = s("/continue/access_token/value") {
            h.continue_token = Some(t);
        }
        if let Some(t) = s("/access_token/value") {
            h.access = Some(t);
            h.manage = s("/access_token/manage/access_token/value");
            h.issued = Some(v.clone());
        }
    }

    /// A `GNAP` call with `token` on `path`, signed.
    pub async fn gnap(&self, method: &str, path: &str, token: &str) -> reqwest::Response {
        self.call(Call {
            method,
            path,
            authorization: Some(format!("GNAP {token}")),
            ..Default::default()
        })
        .await
    }

    /// `POST /api/grants/<gid>/continue` with the continuation token: the
    /// status and body; the tokens an approval hands out are kept.
    pub async fn poll(&self) -> (u16, serde_json::Value) {
        let h = self.held();
        let res = self
            .gnap(
                "POST",
                &format!("/api/grants/{}/continue", self.grant_id()),
                h.continue_token.as_deref().expect("a continuation token"),
            )
            .await;
        let status = res.status().as_u16();
        let v: serde_json::Value = res.json().await.unwrap_or_default();
        if status == 200 {
            self.keep(&v);
        }
        (status, v)
    }

    /// `DELETE /api/grants/<gid>/continue` with the continuation token.
    pub async fn disconnect(&self) -> reqwest::Response {
        let h = self.held();
        self.gnap(
            "DELETE",
            &format!("/api/grants/{}/continue", self.grant_id()),
            h.continue_token.as_deref().expect("a continuation token"),
        )
        .await
    }

    /// The access token held; panics without one.
    pub fn access(&self) -> String {
        self.held().access.expect("an access token")
    }

    /// The base URL and `Host` its requests use.
    pub fn target(&self) -> (&str, &str) {
        (&self.base, &self.host)
    }

    /// `Clax-Signature` for `method path` with `body` and no agent
    /// headers, signed now with the access token and this key (`corrupt`
    /// flips a bit of it).
    pub fn signature(&self, method: &str, path: &str, body: &[u8], corrupt: bool) -> String {
        use sha2::Digest;
        let nonce = clax_core::sig::new_nonce();
        let mut h = self
            .key
            .sign(&clax_core::sig::SignInput {
                method,
                path_and_query: path,
                host: &self.host,
                t: self.store.now_utc().timestamp(),
                nonce: &nonce,
                body_sha256: sha2::Sha256::digest(body).into(),
                token_sha256: Some(clax_core::secret::sha256(&self.access())),
                agent_headers_sha256: clax_core::sig::no_agent_headers_sha256(),
            })
            .unwrap();
        if corrupt {
            h.sig[0] ^= 1;
        }
        h.to_string()
    }

    /// A signed agent request with the access token (`Clax-Grant`),
    /// naming `session` in `X-Clax-Session`, with JSON `body`.
    pub async fn agent(
        &self,
        method: &str,
        path: &str,
        session: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let mut headers = Vec::new();
        if let Some(s) = session {
            headers.push(("x-clax-session", s.to_string()));
        }
        self.call(Call {
            method,
            path,
            authorization: Some(format!("Clax-Grant {}", self.access())),
            body: body.map(|b| serde_json::to_vec(&b).unwrap()),
            headers,
            ..Default::default()
        })
        .await
    }

    /// A signed `GET` with the access token.
    pub async fn get(&self, session: Option<&str>, path: &str) -> reqwest::Response {
        self.agent("GET", path, session, None).await
    }

    /// Registers a session of this machine's (`POST /api/sessions`); the
    /// session, asserting a 201.
    pub async fn register_with(&self, body: serde_json::Value) -> serde_json::Value {
        let res = self.agent("POST", "/api/sessions", None, Some(body)).await;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(status, 201, "{v}");
        v["session"].clone()
    }

    /// [`TestClient::register_with`] for `harness` and harness session ID
    /// `hsid`.
    pub async fn register(&self, harness: &str, hsid: &str) -> serde_json::Value {
        self.register_with(serde_json::json!({"harness": harness,
                                              "harness_session_id": hsid, "cwd": "/w"}))
            .await
    }

    /// [`TestClient::register`] with parent PID `pid`.
    pub async fn register_pid(&self, harness: &str, hsid: &str, pid: u32) -> serde_json::Value {
        self.register_with(
            serde_json::json!({"harness": harness, "harness_session_id": hsid,
                                              "cwd": "/w", "parent_pid": pid}),
        )
        .await
    }

    /// `method path` with the access token, a fresh `t` and nonce, naming
    /// `sid`, and a signature that does not verify.
    pub async fn call_with_bad_signature(
        &self,
        sid: &str,
        method: &str,
        path: &str,
    ) -> reqwest::Response {
        self.call(Call {
            method,
            path,
            authorization: Some(format!("Clax-Grant {}", self.access())),
            headers: vec![("x-clax-session", sid.to_string())],
            corrupt: true,
            ..Default::default()
        })
        .await
    }

    /// `DELETE /api/grants/<gid>/token` with the manage token.
    pub async fn revoke_token(&self) -> reqwest::Response {
        let h = self.held();
        self.gnap(
            "DELETE",
            &format!("/api/grants/{}/token", self.grant_id()),
            h.manage.as_deref().expect("a manage token"),
        )
        .await
    }
}

impl TestServer {
    /// A browser of the owner's on this machine.
    pub fn owner_browser(&self) -> OwnerBrowser {
        OwnerBrowser {
            base: self.base.clone(),
            client: self.client.clone(),
            cookie: self.owner_cookie(),
        }
    }

    /// The `clax connect` command of a new invite for a new person named
    /// `name`, made in the owner's browser (the daemon must listen beyond
    /// loopback).
    pub async fn invite_command(&self, name: &str) -> String {
        self.invite_with(serde_json::json!({ "name": name })).await
    }

    /// The command of an invite made with `body`; asserts a 201.
    pub async fn invite_with(&self, body: serde_json::Value) -> String {
        let res = self.owner_browser().post("/api/people/invites", body).await;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(status, 201, "{v}");
        v["invite"]["command"].as_str().unwrap().to_string()
    }

    /// A new person `name` with rights `preset`, connected by invite: their
    /// machine, holding its tokens.
    pub async fn connect_person(&self, name: &str, preset: &str) -> TestClient {
        let cmd = self
            .invite_with(serde_json::json!({"name": name, "rights": preset}))
            .await;
        let code = cmd.rsplit_once("/i/").expect("an invite command").1;
        let c = TestClient::new(self);
        c.grant_request(serde_json::json!({
            "access_token": {"access": [preset]},
            "clax": {"invite": code},
        }))
        .await;
        c
    }

    /// Whether a viewer row `id` exists.
    pub fn viewer_exists(&self, id: &str) -> bool {
        self.store.get_viewer(id).unwrap().is_some()
    }
}
