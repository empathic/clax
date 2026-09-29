//! An in-process daemon on an ephemeral port, for integration tests
//! in this crate and in crates that call the daemon over HTTP.

use crate::daemon::{browser_host, probe_host};
use crate::wrap_cache::WrapCache;
use crate::{AppState, build_router};
use artifax_core::{EventBus, Home, Store};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// A running test daemon with its own temporary home and the fixed token `test-token`.
pub struct TestServer {
    pub base: String,
    pub token: String,
    pub home: Home,
    pub client: reqwest::Client,
    /// The server's event bus, for publishing events directly.
    pub events: EventBus,
    /// The address the listener is bound to (may be unspecified, e.g. `0.0.0.0`).
    pub addr: SocketAddr,
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
        let mut state = AppState {
            store,
            home: home.clone(),
            token: token.clone(),
            events: EventBus::new(),
            started_at: Store::now(),
            version: "test",
            shutdown: tokio::sync::watch::channel(false).1,
            wrap_cache: Arc::new(WrapCache::new(64)),
            request_timeout: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(120),
            sse_keep_alive: Duration::from_secs(15),
            self_base: format!("http://{}:{port}", probe_host(&bind.to_string())),
            browser_base: format!("http://{}:{port}", browser_host(&bind.to_string())),
            feedback_waiters: Arc::new(Default::default()),
        };
        f(&mut state);
        let events = state.events.clone();
        let app = build_router(state);
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
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
            addr,
            _dir: dir,
        }
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
pub const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nartifax-test-clip";

/// An element anchor on `body > main > h2` quoting "Quarterly goals".
pub fn element_anchor() -> serde_json::Value {
    serde_json::json!({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null})
}

/// Reads Server-Sent Events from one `/api/events` response.
pub struct EventReader {
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + Send>>,
    buf: String,
}

impl EventReader {
    /// The next event other than `ready` and keep-alive comments, as (name, data), within 5 s.
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
                if name == "ready" {
                    continue;
                }
                return (
                    name,
                    serde_json::from_str(data).expect("event data is JSON"),
                );
            }
            let chunk = tokio::time::timeout(Duration::from_secs(5), self.stream.next())
                .await
                .expect("SSE chunk within 5 s")
                .expect("stream still open")
                .expect("chunk readable");
            self.buf
                .push_str(std::str::from_utf8(&chunk).expect("UTF-8 events"));
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
            .header("x-artifax-session", session_id)
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
        use futures::StreamExt;
        let res = self.get(&format!("/api/events{query}")).await;
        assert_eq!(res.status(), 200);
        let stream = res
            .bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));
        EventReader {
            stream: Box::pin(stream),
            buf: String::new(),
        }
    }
}
