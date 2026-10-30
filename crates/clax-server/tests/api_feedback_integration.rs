//! The instance's own feedback integration (spec
//! 2026-10-29-feedback-loop-design §10): `POST /_app/feedback/token` mints
//! a reporter token through the public reporter-tokens route, for every
//! signed-in person; `GET /_app/feedback/config` says what the shell
//! mounts the overlay with; `GET /api/me/flags` lists the caller's own
//! flags. A hosted instance configured with none makes its own queue and
//! grant, keeps them and makes them again when they are gone.

use base64::Engine as _;
use clax_core::audit::AuditCtx;
use clax_core::queue::ReportersSee;
use clax_core::sig::{self, KeyPair, SignInput};
use clax_core::working::ManualClock;
use clax_http::state::InstanceFeedback;
use clax_store::queues::NewQueue;
use clax_testkit::remotes::{Public, SECRET};
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
    grant_id: String,
    grant_token: String,
    grant_key: KeyPair,
    /// The daemon's state, as it was before it served (its stores and
    /// caches are the daemon's own).
    state: clax_http::state::AppState,
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
            let (service_grant, token, key) = s
                .store
                .create_service_grant(&AuditCtx::DAEMON, &qid, "clax.dev")
                .unwrap();
            let grant = json!({
                "token": token.expose(),
                "private_key": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(key.pkcs8().expose()),
            });
            let i = clax_server::integrations::feedback::integration(
                &qid,
                ORIGIN,
                ORIGIN,
                &grant.to_string(),
            )
            .unwrap();
            s.feedback = InstanceFeedback::Configured(Arc::new(i));
            *out.lock().unwrap() = Some((
                qid,
                service_grant.id,
                token.expose().to_string(),
                key,
                s.clone(),
            ));
        })
        .await;
        let (qid, grant_id, grant_token, grant_key, state) = made.lock().unwrap().take().unwrap();
        Setup {
            ts,
            qid,
            grant_id,
            grant_token,
            grant_key,
            state,
        }
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
async fn every_signed_in_person_reports_with_no_flag() {
    let s = Setup::new().await;
    let ana = s.ts.person("Ana").await;
    let key = KeyPair::generate();

    // A connected person no admin has touched mints a token.
    let (status, v) = s.mint_as(&ana, &key).await;
    assert_eq!(status, 200, "{v}");
    assert!(v["token"].as_str().unwrap().starts_with("cxr_"), "{v}");

    // Without a credential, or as a viewer who is no person, none is minted,
    // and the config is off.
    let (status, v) = s
        .as_browser(None, "GET", "/_app/feedback/config", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        v,
        json!({"enabled": false, "instance": null, "queue": null, "context": null})
    );
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

    let (status, v) = s
        .as_browser(Some(&viewer), "GET", "/_app/feedback/config", None)
        .await;
    assert_eq!((status, &v["enabled"]), (200, &json!(false)), "{v}");

    // The person's config says it is on, with the context to attach.
    let (status, v) = s
        .as_browser(Some(&ana), "GET", "/_app/feedback/config", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(v["enabled"], true, "{v}");
    assert_eq!(v["instance"], ORIGIN);
    assert_eq!(v["queue"], s.qid.as_str());
    // The overlay's `user`: the person's public ID, as the token's reporter ID.
    assert_eq!(v["user"], ana.public_id.as_str(), "{v}");
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
    assert_eq!(
        v["context"]["flags"],
        json!({"claxbot": false, "claxbot_unenforced": false}),
        "{v}"
    );
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
    s.github(&ana.public_id, "ana-gh", Some("Ana Lopez"));

    let before = s.audit_kinds();
    let key = KeyPair::generate();
    let (status, v) = s.mint_as(&ana, &key).await;
    assert_eq!(status, 200, "{v}");
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxr_"), "{v}");
    assert!(v["expires_at"].is_string(), "{v}");
    let in_process: Vec<String> = s.audit_kinds()[before.len()..].to_vec();
    // Minting is unaudited by design, in process as over HTTP.
    assert!(in_process.is_empty(), "{in_process:?}");

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
    assert!(external.is_empty(), "{external:?}");
}

#[tokio::test]
async fn the_owner_files_as_their_public_id() {
    let s = Setup::new().await;
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

/// A local daemon (not hosted) configured with none: the integration is
/// off, and no queue is made.
#[tokio::test]
async fn unconfigured_is_404() {
    let ts = TestServer::spawn().await;
    let ana = ts.person("Ana").await;
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
    assert!(ts.store.queues_of(None).unwrap().is_empty());
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
        json!({"flags": {"claxbot": false, "claxbot_unenforced": false}})
    );
    // The owner's are the owner's defaults.
    let (status, v) = get(ts.owner_cookie()).await;
    assert_eq!(status, 200);
    assert_eq!(
        v,
        json!({"flags": {"claxbot": true, "claxbot_unenforced": false}})
    );
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

#[tokio::test]
async fn the_in_process_mint_is_the_public_routes_request() {
    let s = Setup::new().await;
    let InstanceFeedback::Configured(i) = s.state.feedback.clone() else {
        panic!("the setup configures the integration");
    };
    let pk = sig::encode_public(&KeyPair::generate().public());
    let req = clax_server::integrations::feedback::mint_request(
        &i,
        s.ts.store.now_utc().timestamp(),
        "r1",
        &pk,
        None,
    )
    .unwrap();
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 1 << 16).await.unwrap();
    let again =
        axum::http::Request::from_parts(parts.clone(), axum::body::Body::from(body.clone()));
    let instance = clax_server::integrations::instance::Instance::new(s.state.clone());
    let minted = clax_server::integrations::feedback::mint(instance.router(), again)
        .await
        .unwrap();
    assert!(
        matches!(
            minted,
            clax_server::integrations::feedback::Minted::Token { .. }
        ),
        "{minted:?}"
    );
    // The same signed request sent over HTTP is a replay: the in-process
    // mint spent its nonce, as the public route spends every request's.
    let mut http = client().post(format!("{}{}", s.ts.base, parts.uri));
    for (k, v) in &parts.headers {
        http = http.header(k.as_str(), v.as_bytes());
    }
    let res = http.body(body.to_vec()).send().await.unwrap();
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (401, Some("replayed_request")),
        "{v}"
    );
}

#[tokio::test]
async fn a_revoked_grant_is_unavailable() {
    let s = Setup::new().await;
    let ana = s.ts.person("Ana").await;
    s.ts.store
        .revoke_service_grant(&AuditCtx::DAEMON, &s.qid, &s.grant_id)
        .unwrap();
    let (status, v) = s.mint_as(&ana, &KeyPair::generate()).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (503, Some("feedback_unavailable")),
        "{v}"
    );
    // Nor does it name the grant.
    assert!(!v.to_string().contains(&s.grant_token), "{v}");
}

#[tokio::test]
async fn only_the_shell_and_never_an_agent_mints() {
    let s = Setup::new().await;
    let ana = s.ts.person("Ana").await;
    let pk = sig::encode_public(&KeyPair::generate().public());
    let res = client()
        .post(format!("{}/_app/feedback/token", s.ts.base))
        .header("cookie", format!("clax_viewer={}", ana.cookie))
        .header("origin", "https://evil.example")
        .json(&json!({"public_key": pk}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "forbidden_origin", "{v}");
    for (method, path) in [
        ("POST", "/_app/feedback/token"),
        ("GET", "/_app/feedback/config"),
    ] {
        let mut req = client()
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{}{path}", s.ts.base),
            )
            .header("authorization", format!("Bearer {}", s.ts.token))
            .header("x-clax-channel", "agent");
        if method == "POST" {
            req = req.json(&json!({"public_key": pk}));
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 403, "{method} {path}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "not_for_agents", "{method} {path}: {v}");
    }
}

/// docs/feedback.md's Rust backend, as the guide prints it: an outside
/// backend's mint over HTTP, signed for the instance's host, for a page
/// origin of its own.
async fn mint_over_http(
    instance: &str,
    queue_id: &str,
    page_origin: &str,
    grant_json: &str,
    reporter_id: &str,
    public_key: &str,
    now_unix_seconds: i64,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    use clax_server::integrations::feedback::{integration, mint_request};
    let i = integration(queue_id, instance, page_origin, grant_json)?;
    let req = mint_request(&i, now_unix_seconds, reporter_id, public_key, None)?;
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 64 * 1024).await?;
    let answer: Value = reqwest::Client::new()
        .post(format!("{}{}", i.instance, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(answer)
}

#[tokio::test]
async fn the_guides_rust_backend_mints_over_http() {
    let s = Setup::new().await;
    let grant = json!({
        "token": s.grant_token,
        "private_key": base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(s.grant_key.pkcs8().expose()),
    });
    let browser = KeyPair::generate();
    // The daemon's clock: 2026-10-29T10:00:00Z.
    let now = 1_793_268_000;
    let v = mint_over_http(
        &s.ts.base,
        &s.qid,
        ORIGIN,
        &grant.to_string(),
        "u-42",
        &sig::encode_public(&browser.public()),
        now,
    )
    .await
    .unwrap();
    assert!(v["token"].as_str().unwrap().starts_with("cxr_"), "{v}");
    assert!(v["expires_at"].is_string(), "{v}");
}

/// The hosted daemon `ts` (setup secret [`SECRET`]) after its first
/// person's setup, in the owner's browser.
async fn set_up(ts: TestServer, clock: Arc<ManualClock>) -> Public {
    let origin = format!("http://localhost:{}", ts.addr.port());
    let res = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header("host", format!("localhost:{}", ts.addr.port()))
        .header("origin", &origin)
        .json(&json!({"secret": SECRET, "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let cookie = res.headers()["set-cookie"]
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

/// `p`'s browser at another daemon on the same store, `ts`: its own port
/// and public origin, the same session.
fn at(p: &Public, ts: TestServer) -> Public {
    Public {
        origin: format!("http://localhost:{}", ts.addr.port()),
        ts,
        clock: p.clock.clone(),
        cookie: p.cookie.clone(),
    }
}

/// The owner's queues, as the Queues page lists them.
async fn owners_queues(p: &Public) -> Vec<Value> {
    let (status, v) = p
        .browser(reqwest::Method::GET, "/api/queues", Value::Null)
        .await;
    assert_eq!(status, 200, "{v}");
    v["queues"].as_array().unwrap().clone()
}

/// A token for the browser of `p`, for a fresh key.
async fn mint_in(p: &Public) -> (u16, Value) {
    let pk = sig::encode_public(&KeyPair::generate().public());
    p.browser(
        reqwest::Method::POST,
        "/_app/feedback/token",
        json!({"public_key": pk}),
    )
    .await
}

/// clax.dev's path with nothing configured: the first config request of a
/// person once the owner exists makes the instance's own queue (the
/// owner's, for the public origin, reporters seeing their own) and one
/// service grant; later requests, other people's included, and a second
/// start on the same store reuse them; a revoked grant is replaced on the
/// same queue.
#[tokio::test]
async fn a_hosted_instance_with_no_configuration_makes_its_own_queue_once() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
    // No one signed in: refused, and nothing made.
    let res = ts
        .client
        .get(format!("{}/_app/feedback/config", ts.base))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (401, Some("sign_in_required")),
        "{v}"
    );
    assert!(ts.store.queues_of(None).unwrap().is_empty());

    let mut p = set_up(ts, clock.clone()).await;
    assert!(owners_queues(&p).await.is_empty());
    let (status, v) = p
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(status, 200);
    assert_eq!(v["enabled"], true, "{v}");
    assert_eq!(v["instance"], p.origin.as_str(), "{v}");
    let qid = v["queue"].as_str().unwrap().to_string();
    // The owner sees it on the Queues page as its admin.
    let queues = owners_queues(&p).await;
    assert_eq!(queues.len(), 1, "{queues:?}");
    assert_eq!(queues[0]["id"], qid.as_str());
    assert_eq!(queues[0]["name"], "clax-feedback");
    assert_eq!(queues[0]["role"], "admin");
    assert_eq!(queues[0]["origins"], json!([p.origin]));
    assert_eq!(queues[0]["reporters_see"], "own");
    let grants = p.ts.store.service_grants(&qid).unwrap();
    assert_eq!(grants.len(), 1);
    let (status, v) = mint_in(&p).await;
    assert_eq!(status, 200, "{v}");

    // Asked again, and by a connected person, the same queue and grant.
    let (_, v) = p
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(v["queue"], qid.as_str(), "{v}");
    let owner = p.cookie.clone();
    let ana = p.become_person("Ana").await;
    let (_, v) = p
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(
        (&v["enabled"], &v["queue"]),
        (&json!(true), &json!(qid)),
        "{v}"
    );
    assert_eq!(v["user"], ana.as_str(), "{v}");
    let (status, v) = mint_in(&p).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(p.ts.store.queues_of(None).unwrap().len(), 1);
    assert_eq!(p.ts.store.service_grants(&qid).unwrap(), grants);

    // A second start on the same store (another port, so another public
    // origin): the same queue and grant, the queue listing that origin.
    let ts2 =
        p.ts.reopened_hosted_with(clock.clone(), Some(SECRET), |_| {})
            .await;
    let mut p2 = at(&p, ts2);
    p2.cookie = owner;
    let (_, v) = p2
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(
        (&v["enabled"], &v["queue"]),
        (&json!(true), &json!(qid)),
        "{v}"
    );
    assert_eq!(v["instance"], p2.origin.as_str(), "{v}");
    let (status, v) = mint_in(&p2).await;
    assert_eq!(status, 200, "{v}");
    let queues = owners_queues(&p2).await;
    assert_eq!(queues.len(), 1, "{queues:?}");
    assert_eq!(queues[0]["origins"], json!([p.origin, p2.origin]));
    assert_eq!(p2.ts.store.service_grants(&qid).unwrap(), grants);

    // The grant revoked: a new one, on the same queue, and minting goes on.
    p2.ts
        .store
        .revoke_service_grant(&AuditCtx::DAEMON, &qid, &grants[0].id)
        .unwrap();
    let (status, v) = mint_in(&p2).await;
    assert_eq!(status, 200, "{v}");
    let now = p2.ts.store.service_grants(&qid).unwrap();
    assert_eq!(now.len(), 1);
    assert_ne!(now[0].id, grants[0].id);
    assert_eq!(owners_queues(&p2).await.len(), 1);

    // An agent is still refused.
    let res = p2
        .ts
        .client
        .get(format!("{}/_app/feedback/config", p2.ts.base))
        .header("authorization", format!("Bearer {}", p2.ts.token))
        .header("x-clax-channel", "agent")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

/// On a hosted instance an integration it was configured with wins: no
/// queue of its own is made.
#[tokio::test]
async fn a_hosted_instances_configured_integration_wins() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let made = Arc::new(Mutex::new(None));
    let out = made.clone();
    let ts = TestServer::spawn_hosted_with(clock.clone(), Some(SECRET), move |s| {
        let origin = format!("http://localhost:{}", s.port);
        let qid = s
            .store
            .create_queue(
                &AuditCtx::DAEMON,
                None,
                NewQueue {
                    name: "configured".into(),
                    origins: vec![origin.clone()],
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
        let (_, token, key) = s
            .store
            .create_service_grant(&AuditCtx::DAEMON, &qid, "configured")
            .unwrap();
        let grant = json!({
            "token": token.expose(),
            "private_key": base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(key.pkcs8().expose()),
        });
        let i = clax_server::integrations::feedback::integration(
            &qid,
            &origin,
            &origin,
            &grant.to_string(),
        )
        .unwrap();
        s.feedback = InstanceFeedback::Configured(Arc::new(i));
        *out.lock().unwrap() = Some(qid);
    })
    .await;
    let qid = made.lock().unwrap().take().unwrap();
    let p = set_up(ts, clock).await;
    let (_, v) = p
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(
        (&v["enabled"], &v["queue"]),
        (&json!(true), &json!(qid)),
        "{v}"
    );
    let (status, v) = mint_in(&p).await;
    assert_eq!(status, 200, "{v}");
    let queues = owners_queues(&p).await;
    assert_eq!(queues.len(), 1, "{queues:?}");
    assert_eq!(queues[0]["name"], "configured");
}

/// A hosted instance whose configuration did not check is off: it makes
/// no queue of its own.
#[tokio::test]
async fn a_hosted_instance_whose_configuration_failed_is_off() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with(clock.clone(), Some(SECRET), |s| {
        s.feedback = InstanceFeedback::Off;
    })
    .await;
    let p = set_up(ts, clock).await;
    let (_, v) = p
        .browser(reqwest::Method::GET, "/_app/feedback/config", Value::Null)
        .await;
    assert_eq!(v["enabled"], false, "{v}");
    let (status, _) = mint_in(&p).await;
    assert_eq!(status, 404);
    assert!(owners_queues(&p).await.is_empty());
}
