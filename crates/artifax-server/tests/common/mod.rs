#![allow(dead_code)]
use artifax_core::{EventBus, Home, Store};
use artifax_server::wrap_cache::WrapCache;
use artifax_server::{AppState, build_router};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub struct TestServer {
    pub base: String,
    pub token: String,
    pub home: Home,
    pub client: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl TestServer {
    pub async fn spawn() -> TestServer {
        Self::spawn_with(|_| {}).await
    }

    pub async fn spawn_with(f: impl FnOnce(&mut AppState)) -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = Arc::new(Store::open(&home).unwrap());
        let token = "test-token".to_string();
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
        };
        f(&mut state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
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
            base: format!("http://{addr}"),
            token,
            home,
            client: reqwest::Client::new(),
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
