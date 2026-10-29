//! The instance's own feedback integration (spec
//! 2026-10-29-feedback-loop-design §10): `POST /_app/feedback/token` mints
//! a reporter token through the public reporter-tokens route, for people
//! with the `feedback` flag; `GET /_app/feedback/config` says what the
//! shell mounts the overlay with; `GET /api/me/flags` lists the caller's
//! own flags.

use base64::Engine as _;
use clax_core::audit::AuditCtx;
use clax_core::queue::ReportersSee;
use clax_core::sig::{self, KeyPair, SignInput};
use clax_core::working::ManualClock;
use clax_store::queues::NewQueue;
use clax_store::roles::OWNER;
use clax_testkit::{TestServer, TestViewer};
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::{Arc, Mutex};

const ORIGIN: &str = "https://clax.test";

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// A daemon whose feedback integration files into a queue for [`ORIGIN`],
/// with that queue's ID and its service grant (for an external mint).
struct Setup {
    ts: TestServer,
    qid: String,
    grant_token: String,
    grant_key: KeyPair,
}

impl Setup {
    async fn new() -> Setup {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let made = Arc::new(Mutex::new(None));
        let out = made.clone();
        let ts = TestServer::spawn_with_clock_and(clock, move |s| {
            let qid = s
                .store
                .create_queue(
                    &AuditCtx::DAEMON,
                    None,
                    NewQueue {
                        name: "Clax feedback".into(),
                        origins: vec![ORIGIN.into()],
                        reporters_see: ReportersSee::Own,
                    },
                )
                .unwrap()
                .id;
            let (_, token, key) = s
                .store
                .create_service_grant(&AuditCtx::DAEMON, &qid, "clax.dev")
                .unwrap();
            let grant = json!({
                "token": token.expose(),
                "private_key_pkcs8": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(key.pkcs8().expose()),
            });
            let i =
                clax_server::integrations::feedback::integration(&qid, ORIGIN, &grant.to_string())
                    .unwrap();
            s.feedback = Some(Arc::new(i));
            *out.lock().unwrap() = Some((qid, token.expose().to_string(), key));
        })
        .await;
        let (qid, grant_token, grant_key) = made.lock().unwrap().take().unwrap();
        Setup {
            ts,
            qid,
            grant_token,
            grant_key,
        }
    }

    /// Sets person `person`'s (a person key) `feedback` flag.
    fn flag(&self, person: &str, on: bool) {
        self.ts
            .store
            .set_flag(&AuditCtx::DAEMON, person, "feedback", on)
            .unwrap();
    }

    /// Links a GitHub account `login` named `name` to person `public_id`,
    /// as a sign-in does.
    fn github(&self, public_id: &str, login: &str, name: Option<&str>) {
        let conn = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        conn.execute(
            "INSERT INTO github_identities (github_id, viewer_id, login, name, avatar_url, created_at, signed_in_at)
             SELECT 4242, id, ?2, ?3, NULL, '2026-10-29T10:00:00Z', '2026-10-29T10:00:00Z'
               FROM viewers WHERE public_id = ?1",
            rusqlite::params![public_id, login, name],
        )
        .unwrap();
    }

    /// `method path` with `body`, in `who`'s browser (`None`: no
    /// credential).
    async fn as_browser(
        &self,
        who: Option<&TestViewer>,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut req = client().request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", self.ts.base),
        );
        if let Some(v) = who {
            req = req.header("cookie", format!("clax_viewer={}", v.cookie));
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    async fn mint_as(&self, who: &TestViewer, key: &KeyPair) -> (u16, Value) {
        self.as_browser(
            Some(who),
            "POST",
            "/_app/feedback/token",
            Some(json!({"public_key": sig::encode_public(&key.public())})),
        )
        .await
    }

    fn audit_kinds(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let mut stmt = conn
            .prepare("SELECT kind FROM audit_events ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn host(&self) -> String {
        self.ts.base.strip_prefix("http://").unwrap().to_string()
    }

    /// A signed request by `key` with `token` under `scheme`, now.
    async fn signed(
        &self,
        scheme: &str,
        token: &str,
        key: &KeyPair,
        pq: &str,
        origin: Option<&str>,
        content_type: &str,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let nonce = sig::new_nonce();
        let signature = key
            .sign(&SignInput {
                method: "POST",
                path_and_query: pq,
                host: &self.host(),
                t: self.ts.store.now_utc().timestamp(),
                nonce: &nonce,
                body_sha256: sha2::Sha256::digest(&body).into(),
                token_sha256: Some(clax_core::secret::sha256(token)),
                agent_headers_sha256: sig::no_agent_headers_sha256(),
            })
            .unwrap()
            .to_string();
        let mut req = client()
            .post(format!("{}{pq}", self.ts.base))
            .header("authorization", format!("{scheme} {token}"))
            .header("clax-signature", signature)
            .header("content-type", content_type);
        if let Some(o) = origin {
            req = req.header("origin", o);
        }
        req.body(body).send().await.unwrap()
    }

    /// Files a report with the reporter token `token` and its `key`, from
    /// [`ORIGIN`].
    async fn file(&self, token: &str, key: &KeyPair) -> Value {
        let boundary = "clax-testboundary";
        let mut b = Vec::new();
        let anchor = clax_testkit::element_anchor().to_string();
        for (name, value) in [
            ("url", "https://clax.test/a/abc"),
            ("title", "Gallery"),
            ("anchor", anchor.as_str()),
            ("body", "The button does nothing"),
        ] {
            b.extend_from_slice(
                format!("--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let res = self
            .signed(
                "Clax-Reporter",
                token,
                key,
                &format!("/api/queues/{}/reporter/reports", self.qid),
                Some(ORIGIN),
                &format!("multipart/form-data; boundary={boundary}"),
                b,
            )
            .await;
        assert_eq!(res.status(), 201);
        res.json().await.unwrap()
    }

    /// The queue's reports, as the owner (its maker) reads them.
    async fn triage_reports(&self) -> Value {
        self.ts
            .get_authed(&format!("/api/queues/{}/reports", self.qid))
            .await
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn token_needs_the_feedback_flag() {
    let s = Setup::new().await;
    let ana = s.ts.person("Ana").await;
    let key = KeyPair::generate();

    let (status, v) = s.mint_as(&ana, &key).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("no_flag")),
        "{v}"
    );
    let (status, v) = s
        .as_browser(Some(&ana), "GET", "/_app/feedback/config", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        v,
        json!({"enabled": false, "instance": null, "queue": null, "context": null})
    );

    // Without a credential, or as a viewer who is no person, none is minted.
    let (status, _) = s
        .as_browser(
            None,
            "POST",
            "/_app/feedback/token",
            Some(json!({"public_key": sig::encode_public(&key.public())})),
        )
        .await;
    assert_eq!(status, 401);
    let viewer = s.ts.viewer(Some("Guest")).await;
    let (status, v) = s.mint_as(&viewer, &key).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("not_a_person")),
        "{v}"
    );

    // A person's flag on: the config says so, with the context to attach.
    s.flag(&ana.public_id, true);
    let (status, v) = s
        .as_browser(Some(&ana), "GET", "/_app/feedback/config", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(v["enabled"], true, "{v}");
    assert_eq!(v["instance"], ORIGIN);
    assert_eq!(v["queue"], s.qid.as_str());
    assert_eq!(v["context"]["release"]["channel"], "dev");
    // The release is what `/healthz` reports.
    let health: Value = s.ts.get("/healthz").await.json().await.unwrap();
    assert_eq!(v["context"]["release"]["version"], health["version"]);
    assert_eq!(
        v["context"]["release"].get("commit"),
        health.get("commit").filter(|c| !c.is_null())
    );
    assert_eq!(
        v["context"]["release"].get("id"),
        health.get("build").filter(|c| !c.is_null())
    );
    assert_eq!(v["context"]["flags"]["feedback"], true);
    assert_eq!(v["context"]["flags"]["claxbot"], false);
    assert_eq!(
        v["context"]["app"]["repo"],
        "https://github.com/empathic/clax"
    );
    // Every release field is a string, as a report's context needs.
    for (k, f) in v["context"]["release"].as_object().unwrap() {
        assert!(f.is_string(), "release.{k}: {f}");
    }
    // A bad key is the public route's 400, passed on.
    let (status, v) = s
        .as_browser(
            Some(&ana),
            "POST",
            "/_app/feedback/token",
            Some(json!({"public_key": "not a key"})),
        )
        .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (400, Some("invalid_key")),
        "{v}"
    );
}

#[tokio::test]
async fn token_is_minted_through_the_public_route() {
    let s = Setup::new().await;
    let ana = s.ts.person("Ana").await;
    s.flag(&ana.public_id, true);
    s.github(&ana.public_id, "ana-gh", Some("Ana Lopez"));

    let before = s.audit_kinds();
    let key = KeyPair::generate();
    let (status, v) = s.mint_as(&ana, &key).await;
    assert_eq!(status, 200, "{v}");
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxr_"), "{v}");
    assert!(v["expires_at"].is_string(), "{v}");
    let in_process: Vec<String> = s.audit_kinds()[before.len()..].to_vec();

    // The token files a report as the person, with their GitHub profile.
    let filed = s.file(&token, &key).await;
    assert_eq!(filed["report"]["reporter_id"], ana.public_id.as_str());
    let v = s.triage_reports().await;
    let report = &v["reports"][0];
    assert_eq!(report["reporter_id"], ana.public_id.as_str(), "{v}");
    assert_eq!(
        report["reporter_profile"],
        json!({"name": "Ana Lopez", "github": "ana-gh"}),
        "{v}"
    );

    // An outside backend's mint over HTTP records what the in-process one did.
    let before = s.audit_kinds();
    let body = serde_json::to_vec(&json!({
        "reporter_id": "someone",
        "origin": ORIGIN,
        "public_key": sig::encode_public(&KeyPair::generate().public()),
    }))
    .unwrap();
    let res = s
        .signed(
            "Clax-Queue",
            &s.grant_token,
            &s.grant_key,
            &format!("/api/queues/{}/reporter-tokens", s.qid),
            None,
            "application/json",
            body,
        )
        .await;
    assert_eq!(res.status(), 200);
    let external: Vec<String> = s.audit_kinds()[before.len()..].to_vec();
    assert_eq!(in_process, external);
}

#[tokio::test]
async fn the_owner_files_as_their_public_id() {
    let s = Setup::new().await;
    s.flag(OWNER, true);
    let owner = s.ts.owner_public_id().await;
    let key = KeyPair::generate();
    let res = client()
        .post(format!("{}/_app/feedback/token", s.ts.base))
        .header("cookie", s.ts.owner_cookie())
        .json(&json!({"public_key": sig::encode_public(&key.public())}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let filed = s.file(v["token"].as_str().unwrap(), &key).await;
    assert_eq!(filed["report"]["reporter_id"], owner.as_str());
}

#[tokio::test]
async fn unconfigured_is_404() {
    let ts = TestServer::spawn().await;
    let ana = ts.person("Ana").await;
    ts.store
        .set_flag(&AuditCtx::DAEMON, &ana.public_id, "feedback", true)
        .unwrap();
    let res = client()
        .post(format!("{}/_app/feedback/token", ts.base))
        .header("cookie", format!("clax_viewer={}", ana.cookie))
        .json(&json!({"public_key": sig::encode_public(&KeyPair::generate().public())}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let v: Value = client()
        .get(format!("{}/_app/feedback/config", ts.base))
        .header("cookie", format!("clax_viewer={}", ana.cookie))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["enabled"], false, "{v}");
}

#[tokio::test]
async fn me_flags_lists_own_flags() {
    let ts = TestServer::spawn().await;
    let ana = ts.person("Ana").await;
    let get = |cookie: String| {
        let base = ts.base.clone();
        async move {
            let res = client()
                .get(format!("{base}/api/me/flags"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };
    let (status, v) = get(format!("clax_viewer={}", ana.cookie)).await;
    assert_eq!(status, 200);
    assert_eq!(
        v,
        json!({"flags": {"claxbot": false, "claxbot_unenforced": false, "feedback": false}})
    );
    ts.store
        .set_flag(&AuditCtx::DAEMON, &ana.public_id, "feedback", true)
        .unwrap();
    let (_, v) = get(format!("clax_viewer={}", ana.cookie)).await;
    assert_eq!(v["flags"]["feedback"], true, "{v}");
    // The owner's are the owner's defaults.
    let (status, v) = get(ts.owner_cookie()).await;
    assert_eq!(status, 200);
    assert_eq!(v["flags"]["claxbot"], true, "{v}");
    assert_eq!(v["flags"]["feedback"], false, "{v}");
    // Agents are refused.
    let res = client()
        .get(format!("{}/api/me/flags", ts.base))
        .header("authorization", format!("Bearer {}", ts.token))
        .header("x-clax-channel", "agent")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}
