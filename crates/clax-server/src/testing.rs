//! An in-process daemon on an ephemeral port, for integration tests
//! in this crate and in crates that call the daemon over HTTP.

use crate::daemon::{browser_host, probe_host};
use crate::wrap_cache::WrapCache;
use crate::{AppState, build_router};
use clax_core::{EventBus, Home, Store};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// The header that makes a test daemon take a request as arriving from
/// another machine on the LAN ([`TestServer::lan`]).
pub const LAN_HEADER: &str = "x-clax-test-lan";
/// The LAN peer a request with [`LAN_HEADER`] arrives from.
pub const LAN_PEER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 7);
/// The daemon's LAN interface address a request with [`LAN_HEADER`]
/// arrives on.
pub const LAN_LOCAL: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

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
        if let Some(c) = conn {
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(crate::auth::Conn {
                    peer: SocketAddr::new(IpAddr::V4(LAN_PEER), c.peer.port()),
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
    /// The server's presence registry.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The address the listener is bound to (may be unspecified, e.g. `0.0.0.0`).
    pub addr: SocketAddr,
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
        let home = Home::at(dir.path().join("ax"));
        let store = Arc::new(Store::open(&home).unwrap());
        let token = "test-token".to_string();
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let live_ids = Arc::new(crate::live::LiveIds::load(&store).unwrap());
        let ext_creds = Arc::new(crate::extension::Credentials::load(&store).unwrap());
        let mut state = AppState {
            store,
            home: home.clone(),
            token: token.clone(),
            events: EventBus::new(),
            started_at: Store::now(),
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
            working: Arc::new(clax_core::working::Working::new(Arc::new(
                clax_core::working::SystemClock,
            ))),
            presence: Arc::new(clax_core::presence::Presence::new(Arc::new(
                clax_core::working::SystemClock,
            ))),
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
            question_clock: Arc::new(clax_core::working::SystemClock),
            terminal_after_s: clax_core::config::TERMINAL_AFTER_S,
        };
        f(&mut state);
        state.stream.listen(&state.events);
        let events = state.events.clone();
        let store = state.store.clone();
        let working = state.working.clone();
        let presence = state.presence.clone();
        let extension_id = state.extension_id.clone();
        let app = build_router(state).layer(axum::middleware::from_fn(as_lan));
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
            presence,
            addr,
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
