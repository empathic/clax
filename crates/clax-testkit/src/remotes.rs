//! A hosted daemon as a public Clax, and a local daemon signed in to it by
//! device code: what the server's tests of publishing to a public Clax and
//! of linked threads share.

use super::TestServer;
use axum::response::IntoResponse;
use clax_core::working::ManualClock;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

pub const SECRET: &str = "the setup secret";

/// The page [`Public::publish`] publishes there.
pub const PAGE: &str = "<h1>Their plan</h1>";

/// `s` as a query value (`application/x-www-form-urlencoded`).
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

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
        Public::set_up(ts, clock).await
    }

    /// [`Public::new`] serving each artifact's site under `domain`
    /// (`<id>.<domain>:<its port>`), which its discovery names.
    pub async fn with_site_domain(domain: &str) -> Public {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts =
            TestServer::spawn_hosted_with_site_domain(clock.clone(), Some(SECRET), domain).await;
        Public::set_up(ts, clock).await
    }

    /// `ts` set up by its owner, Ada, whose browser this is.
    async fn set_up(ts: TestServer, clock: Arc<ManualClock>) -> Public {
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

    /// Revokes there the agent token `local` holds for remote `rid`, as
    /// the token's holder may (`DELETE /api/hosted/agent-token`); `local`
    /// still holds it.
    pub async fn revoke(&self, local: &TestServer, rid: &str) {
        let token = local.store.remote_token(rid).unwrap().unwrap();
        let res = self
            .ts
            .client
            .delete(format!("{}/api/hosted/agent-token", self.ts.base))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()))
            .bearer_auth(token.expose())
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }

    /// An artifact titled `Theirs` of [`PAGE`] that the owner's CLI
    /// publishes there (private there): its ID.
    pub async fn publish(&self) -> String {
        let (s, v) = self
            .cli(
                reqwest::Method::POST,
                "/api/artifacts",
                Some(json!({"title": "Theirs",
                    "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["artifact"]["id"].as_str().unwrap().to_string()
    }

    /// A thread on the whole of version 1 of `raid` there, saying `body`,
    /// from the browser whose session cookie is `cookie`: the thread.
    pub async fn browser_thread(&self, raid: &str, cookie: &str, body: &str) -> Value {
        let res = self
            .ts
            .client
            .post(format!("{}/api/artifacts/{raid}/threads", self.ts.base))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()))
            .header(COOKIE, format!("clax_session={cookie}"))
            .header(ORIGIN, &self.origin)
            .multipart(
                reqwest::multipart::Form::new()
                    .text("anchor", json!({"kind": "artifact"}).to_string())
                    .text("body", body.to_string())
                    .text("version", "1"),
            )
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(res).await;
        assert_eq!(s, 201, "{v}");
        v["thread"].clone()
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

/// [`post`] as the owner by hand who confirms in their browser what the
/// daemon holds for a confirmation while an agent is live
/// ([`TestServer::confirmed`]).
pub async fn owner_post(ts: &TestServer, path: &str, body: Value) -> (u16, Value) {
    json_of(ts.confirmed(reqwest::Method::POST, path, body).await).await
}

/// Signs `local` in to `public` by device code, the public owner
/// approving; the remote's view.
pub async fn connect(local: &TestServer, public: &Public) -> Value {
    connect_with(local, public, json!({"url": public.origin}), None).await
}

/// [`connect`] as the Clax app signs in: its sign-in names the kind
/// `clax-app`, so the remote's token is its person's device's, which may
/// manage the sharing of their artifacts there.
pub async fn connect_app(local: &TestServer, public: &Public) -> Value {
    connect_with(
        local,
        public,
        json!({"url": public.origin, "kind": "clax-app", "machine": "Clax on phone"}),
        None,
    )
    .await
}

/// [`connect`] with `body` as `POST /api/remotes`'s. With `return_to`, the
/// absolute address a `return_to` path in `body` is to become: the request
/// comes to that address's host, and the approval page names the address
/// as the one to send the person back to.
pub async fn connect_with(
    local: &TestServer,
    public: &Public,
    body: Value,
    return_to: Option<&str>,
) -> Value {
    let req = |confirm: Option<&str>| {
        let mut req = local
            .authed(local.client.post(format!("{}/api/remotes", local.base)))
            .json(&body);
        if let Some(to) = return_to {
            let host = to
                .strip_prefix("http://")
                .and_then(|r| r.split('/').next())
                .expect("an http address");
            req = req.header(HOST, host);
        }
        if let Some(id) = confirm {
            req = req.header(clax_core::confirm::CONFIRMATION_HEADER, id);
        }
        req
    };
    let (mut s, mut v) = json_of(req(None).send().await.unwrap()).await;
    // While an agent is live, the owner confirms in their browser first.
    if s == 202 && v["status"] == "confirmation_pending" {
        let id = v["confirmation_id"].as_str().unwrap().to_string();
        local.confirm(&id, true).await;
        (s, v) = json_of(req(Some(&id)).send().await.unwrap()).await;
    }
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["return_to_kept"], json!(return_to.is_some()), "{v}");
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
    assert_eq!(peek["return_to"], json!(return_to), "{peek}");
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

/// A web server that is not a Clax until told it is: `GET
/// /.well-known/clax` answers 404, then, after [`NotYetClax::become_clax`],
/// a hosted Clax's discovery named `Late`. It counts the discovery
/// requests it got.
pub struct NotYetClax {
    pub origin: String,
    clax: Arc<std::sync::atomic::AtomicBool>,
    asked: Arc<std::sync::atomic::AtomicUsize>,
    serving: tokio::task::JoinHandle<()>,
}

impl NotYetClax {
    pub async fn start() -> NotYetClax {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let clax = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (c, a, o) = (clax.clone(), asked.clone(), origin.clone());
        let app = axum::Router::new().route(
            "/.well-known/clax",
            axum::routing::get(move || {
                let (c, a, o) = (c.clone(), a.clone(), o.clone());
                async move {
                    a.fetch_add(1, Ordering::SeqCst);
                    if c.load(Ordering::SeqCst) {
                        axum::Json(json!({"kind": "hosted", "contract_version": "1",
                            "name": "Late", "origin": o, "api": "/api",
                            "sign_in": ["device"], "capabilities": []}))
                        .into_response()
                    } else {
                        reqwest::StatusCode::NOT_FOUND.into_response()
                    }
                }
            }),
        );
        let serving = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        NotYetClax {
            origin,
            clax,
            asked,
            serving,
        }
    }

    /// From now on its discovery says it is a Clax.
    pub fn become_clax(&self) {
        self.clax.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The discovery requests it got.
    pub fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for NotYetClax {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// A local port nothing listens on: one the system handed out and took
/// back.
pub fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
