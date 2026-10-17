//! A remote person's machine ([`TestClient`]) and a browser of the owner's
//! ([`OwnerBrowser`]).

use crate::*;
use clax_core::Store;
use std::net::IpAddr;
use std::sync::Arc;

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
    /// Send `X-Clax-Session` without the secret the client holds for that
    /// session, which is otherwise added (spec 2026-10-08 §6.8).
    pub bare_session: bool,
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
    /// Each session's secret, as its registration or join answered it
    /// (spec 2026-10-08 §6.8).
    secrets: std::sync::Mutex<std::collections::HashMap<String, String>>,
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
            secrets: Default::default(),
        }
    }

    /// The same client speaking from this machine (a loopback peer, as
    /// through a tunnel or a local proxy).
    pub fn on_loopback(mut self) -> TestClient {
        self.client = reqwest::Client::new();
        self
    }

    /// The same client reaching the daemon at its LAN address
    /// ([`LAN_LOCAL`] and the daemon's port) rather than the loopback
    /// address the test connects to: its requests name and sign that
    /// `Host`, as a machine on the LAN does.
    pub fn on_lan_address(mut self) -> TestClient {
        let port = self.host.rsplit_once(':').map(|(_, p)| p.to_string());
        self.host = format!("{LAN_LOCAL}:{}", port.unwrap_or_default());
        self
    }

    /// The `http://<host>` this client reaches the daemon at.
    pub fn reached_base(&self) -> String {
        format!("http://{}", self.host)
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
        let mut headers = c.sent_headers.unwrap_or(c.headers);
        // The secret of the session the request names, as an agent side
        // sends it (spec 2026-10-08 §6.8).
        let names = |k: &str| headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(k));
        if !c.bare_session && !names(clax_core::remote::SESSION_SECRET_HEADER) {
            let named = headers
                .iter()
                .find(|(h, _)| h.eq_ignore_ascii_case("x-clax-session"))
                .and_then(|(_, sid)| self.secret_of(sid));
            if let Some(secret) = named {
                headers.push((clax_core::remote::SESSION_SECRET_HEADER, secret));
            }
        }
        // JSON unless the call names its own type (a multipart form).
        if !body.is_empty()
            && !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            req = req.header("content-type", "application/json");
        }
        for (k, v) in headers {
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

    /// The secret of session `sid` this machine was answered, if any.
    pub fn secret_of(&self, sid: &str) -> Option<String> {
        self.secrets.lock().unwrap().get(sid).cloned()
    }

    /// Keeps `secret` as session `sid`'s.
    pub fn keep_secret(&self, sid: &str, secret: &str) {
        self.secrets
            .lock()
            .unwrap()
            .insert(sid.to_string(), secret.to_string());
    }

    /// Keeps the session secret a registration or join answer `v` carries.
    pub fn keep_session(&self, v: &serde_json::Value) {
        if let (Some(sid), Some(secret)) =
            (v["session"]["id"].as_str(), v["session_secret"].as_str())
        {
            self.keep_secret(sid, secret);
        }
    }

    /// A signed agent request with the access token (`Clax-Grant`),
    /// naming `session` in `X-Clax-Session` with the secret this machine
    /// holds for it, with JSON `body`.
    pub async fn agent(
        &self,
        method: &str,
        path: &str,
        session: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let secret = session.and_then(|s| self.secret_of(s));
        self.agent_proving(method, path, session, secret.as_deref(), body)
            .await
    }

    /// [`TestClient::agent`] proving `session` with `secret` (none:
    /// no `X-Clax-Session-Secret`).
    pub async fn agent_proving(
        &self,
        method: &str,
        path: &str,
        session: Option<&str>,
        secret: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let mut headers = Vec::new();
        if let Some(s) = session {
            headers.push(("x-clax-session", s.to_string()));
        }
        if let Some(s) = secret {
            headers.push((clax_core::remote::SESSION_SECRET_HEADER, s.to_string()));
        }
        let bare_session = secret.is_none();
        self.call(Call {
            method,
            path,
            authorization: Some(format!("Clax-Grant {}", self.access())),
            body: body.map(|b| serde_json::to_vec(&b).unwrap()),
            headers,
            bare_session,
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
        self.keep_session(&v);
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
        let mut headers = vec![("x-clax-session", sid.to_string())];
        if let Some(s) = self.secret_of(sid) {
            headers.push((clax_core::remote::SESSION_SECRET_HEADER, s));
        }
        self.call(Call {
            method,
            path,
            authorization: Some(format!("Clax-Grant {}", self.access())),
            headers,
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

    /// Whether `cookie` names a viewer.
    pub fn viewer_exists(&self, cookie: &str) -> bool {
        self.store.get_viewer(cookie).unwrap().is_some()
    }

    /// A browser cookie of viewer row `viewer_id`, as a browser link gives
    /// a person's browser one: a new secret naming it.
    pub fn browser_cookie(&self, viewer_id: &str) -> String {
        self.store
            .issue_viewer_cookie(viewer_id)
            .unwrap()
            .expose()
            .to_string()
    }
}
