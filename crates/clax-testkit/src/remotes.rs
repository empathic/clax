//! A hosted daemon as a public Clax, and a local daemon signed in to it by
//! device code: what the server's tests of publishing to a public Clax and
//! of linked threads share.

use super::TestServer;
use clax_core::working::ManualClock;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

pub const SECRET: &str = "the setup secret";

pub async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

/// A hosted daemon and its owner's browser.
pub struct Public {
    pub ts: TestServer,
    pub clock: Arc<ManualClock>,
    pub origin: String,
    pub cookie: String,
}

impl Public {
    pub async fn new() -> Public {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        let res = ts
            .client
            .post(format!("{}/api/hosted/setup", ts.base))
            .header(HOST, format!("localhost:{}", ts.addr.port()))
            .header(ORIGIN, &origin)
            .json(&json!({"secret": SECRET, "name": "Ada"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let cookie = res.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .strip_prefix("clax_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        Public {
            ts,
            clock,
            origin,
            cookie,
        }
    }

    /// A request of the owner's browser at the public origin.
    pub async fn browser(&self, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let mut r = self
            .ts
            .client
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()))
            .header(COOKIE, format!("clax_session={}", self.cookie));
        if method != reqwest::Method::GET {
            r = r.header(ORIGIN, &self.origin).json(&body);
        }
        json_of(r.send().await.unwrap()).await
    }

    /// Invites a person named `name` and makes the browser theirs: its
    /// later requests are that person's. Their public ID.
    pub async fn become_person(&mut self, name: &str) -> String {
        let (s, v) = self
            .browser(
                reqwest::Method::POST,
                "/api/people/invites",
                json!({"name": name}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let res = self
            .ts
            .client
            .post(format!("{}/api/hosted/invites/{code}/accept", self.ts.base))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()))
            .header(ORIGIN, &self.origin)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        self.cookie = res.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .strip_prefix("clax_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (_, me) = self
            .browser(reqwest::Method::GET, "/api/hosted/me", json!(null))
            .await;
        me["person"]["public_id"].as_str().unwrap().to_string()
    }

    /// A request of the owner's CLI there (the daemon token).
    pub async fn cli(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut r = self
            .ts
            .client
            .request(method, format!("{}{path}", self.ts.base))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()))
            .bearer_auth(&self.ts.token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        json_of(r.send().await.unwrap()).await
    }
}

pub async fn local_req(
    ts: &TestServer,
    method: reqwest::Method,
    path: &str,
    body: Value,
    session: Option<&str>,
) -> (u16, Value) {
    let mut r = ts
        .authed(ts.client.request(method, format!("{}{path}", ts.base)))
        .json(&body);
    if let Some(s) = session {
        r = r.header("x-clax-session", s);
    }
    json_of(r.send().await.unwrap()).await
}

pub async fn post(ts: &TestServer, path: &str, body: Value) -> (u16, Value) {
    local_req(ts, reqwest::Method::POST, path, body, None).await
}

/// Signs `local` in to `public` by device code, the public owner
/// approving; the remote's view.
pub async fn connect(local: &TestServer, public: &Public) -> Value {
    connect_with(local, public, json!({"url": public.origin})).await
}

/// [`connect`] as the Clax app signs in: its sign-in names the kind
/// `clax-app`, so the remote's token is its person's device's, which may
/// manage the sharing of their artifacts there.
pub async fn connect_app(local: &TestServer, public: &Public) -> Value {
    connect_with(
        local,
        public,
        json!({"url": public.origin, "kind": "clax-app", "machine": "Clax on phone"}),
    )
    .await
}

/// [`connect`] with `body` as `POST /api/remotes`'s.
pub async fn connect_with(local: &TestServer, public: &Public, body: Value) -> Value {
    let (s, v) = post(local, "/api/remotes", body).await;
    assert_eq!(s, 201, "{v}");
    let uc = v["user_code"].as_str().unwrap().to_string();
    let id = v["remote"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["remote"]["status"], "pending");
    assert!(
        v["verification_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/device?code={uc}")),
        "{v}"
    );
    let (s, v) = post(local, &format!("/api/remotes/{id}/poll"), json!({})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("pending")), "{v}");
    // The approval page names this machine.
    let (s, peek) = public
        .browser(
            reqwest::Method::GET,
            &format!("/api/hosted/device/{uc}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{peek}");
    assert!(
        peek["client_name"]
            .as_str()
            .unwrap()
            .starts_with("Clax on "),
        "{peek}"
    );
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    public.clock.advance(5);
    let (s, v) = post(local, &format!("/api/remotes/{id}/poll"), json!({})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("connected")), "{v}");
    // The home holding the token is its user's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(local.state.home.root()), 0o700);
        assert_eq!(mode(&local.state.home.db_path()), 0o600);
    }
    v["remote"].clone()
}
