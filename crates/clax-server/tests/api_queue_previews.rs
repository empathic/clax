//! Fixes checked on preview deployments: a service grant limited to a
//! queue's preview pattern, a fix's ticket and the one thing it does, the
//! reporter's previews on its own origin, and a verdict given there.

use clax_core::audit::AuditCtx;
use clax_core::queue::{FIX_TICKET_SECS, ReportersSee};
use clax_core::sig::{self, KeyPair, SignInput};
use clax_core::working::ManualClock;
use clax_store::queues::NewQueue;
use clax_testkit::{TestServer, element_anchor};
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::Arc;

const ORIGIN: &str = "https://app.example";
const PATTERN: &str = "https://app-pr-*.fly.dev";
const PREVIEW: &str = "https://app-pr-538.fly.dev";

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// The error codes the SDK's `QueueErrorCode` lists (web/sdk/src/types.ts).
fn sdk_error_codes() -> Vec<String> {
    serde_json::from_str(include_str!("../../../web/sdk/test/queue-error-codes.json")).unwrap()
}

/// An answer's status and JSON; a refusal's code must be one the SDK
/// lists.
async fn answer(res: reqwest::Response) -> (u16, Value) {
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or_default();
    if let Some(code) = v["error"]["code"].as_str() {
        assert!(
            sdk_error_codes().iter().any(|c| c == code),
            "{code} is missing from web/sdk/test/queue-error-codes.json and the SDK's QueueErrorCode"
        );
    }
    (s, v)
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// A daemon with one queue for [`ORIGIN`] and [`PATTERN`], a report of
/// `emp-1`'s filed in it, and the owner's agent session.
struct Setup {
    ts: TestServer,
    clock: Arc<ManualClock>,
    qid: String,
    tid: String,
    sid: String,
}

/// A reporter token and its key.
struct Reporter {
    token: String,
    key: KeyPair,
    origin: &'static str,
}

impl Setup {
    async fn new() -> Setup {
        Setup::with_origins(vec![ORIGIN.into(), PATTERN.into()]).await
    }

    async fn with_origins(origins: Vec<String>) -> Setup {
        let clock = Arc::new(ManualClock::at("2026-11-24T10:00:00Z"));
        let ts = TestServer::spawn_with_clock(clock.clone()).await;
        let qid = ts
            .store
            .create_queue(
                &AuditCtx::DAEMON,
                None,
                NewQueue {
                    name: "App".into(),
                    origins,
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
        let sid = ts.register_session("claude", "fixer").await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let mut s = Setup {
            ts,
            clock,
            qid,
            tid: String::new(),
            sid,
        };
        let r = s.reporter_by(&s.prod_grant().await, "emp-1", ORIGIN).await;
        s.tid = s.file(&r).await;
        s
    }

    fn host(&self) -> String {
        self.ts.base.strip_prefix("http://").unwrap().to_string()
    }

    fn sign(&self, key: &KeyPair, method: &str, pq: &str, token: &str, body: &[u8]) -> String {
        let nonce = sig::new_nonce();
        key.sign(&SignInput {
            method,
            path_and_query: pq,
            host: &self.host(),
            t: self.ts.store.now_utc().timestamp(),
            nonce: &nonce,
            body_sha256: sha2::Sha256::digest(body).into(),
            token_sha256: Some(clax_core::secret::sha256(token)),
            agent_headers_sha256: sig::no_agent_headers_sha256(),
        })
        .unwrap()
        .to_string()
    }

    async fn send(
        &self,
        method: &str,
        pq: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut req = client().request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{pq}", self.ts.base),
        );
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        req.body(body).send().await.unwrap()
    }

    /// The owner by hand makes a service grant `{name, origins?}`.
    async fn grant(&self, body: Value) -> (u16, Value) {
        let res = self
            .ts
            .authed(client().post(format!(
                "{}/api/queues/{}/service-grants",
                self.ts.base, self.qid
            )))
            .json(&body)
            .send()
            .await
            .unwrap();
        answer(res).await
    }

    /// A grant of every origin: its token and key.
    async fn prod_grant(&self) -> (String, KeyPair) {
        let (s, v) = self.grant(json!({"name": "prod"})).await;
        assert_eq!(s, 201, "{v}");
        grant_of(&v)
    }

    /// Mints a reporter token with `grant` for `reporter_id` on `origin`.
    async fn mint(
        &self,
        grant: &(String, KeyPair),
        reporter_id: &str,
        origin: &str,
    ) -> (u16, Value, KeyPair) {
        let key = KeyPair::generate();
        let pq = format!("/api/queues/{}/reporter-tokens", self.qid);
        let body = serde_json::to_vec(&json!({"reporter_id": reporter_id, "origin": origin,
            "public_key": sig::encode_public(&key.public())}))
        .unwrap();
        let sig = self.sign(&grant.1, "POST", &pq, &grant.0, &body);
        let res = self
            .send(
                "POST",
                &pq,
                &[
                    ("authorization", format!("Clax-Queue {}", grant.0)),
                    ("clax-signature", sig),
                    ("content-type", "application/json".into()),
                ],
                body,
            )
            .await;
        let (s, v) = answer(res).await;
        (s, v, key)
    }

    async fn reporter_by(
        &self,
        grant: &(String, KeyPair),
        reporter_id: &str,
        origin: &'static str,
    ) -> Reporter {
        let (s, v, key) = self.mint(grant, reporter_id, origin).await;
        assert_eq!(s, 200, "{origin}: {v}");
        Reporter {
            token: v["token"].as_str().unwrap().to_string(),
            key,
            origin,
        }
    }

    /// A reporter's signed request from its own origin.
    async fn as_reporter(
        &self,
        r: &Reporter,
        method: &str,
        pq: &str,
        body: Vec<u8>,
        ct: Option<&str>,
    ) -> (u16, Value) {
        let sig = self.sign(&r.key, method, pq, &r.token, &body);
        let mut h = vec![
            ("authorization", format!("Clax-Reporter {}", r.token)),
            ("clax-signature", sig),
            ("origin", r.origin.to_string()),
        ];
        if let Some(ct) = ct {
            h.push(("content-type", ct.to_string()));
        }
        answer(self.send(method, pq, &h, body).await).await
    }

    /// Files a report as `r`: its ID.
    async fn file(&self, r: &Reporter) -> String {
        let boundary = "clax-testboundary";
        let mut b = Vec::new();
        for (name, value) in [
            ("url", format!("{ORIGIN}/orders/7")),
            ("title", "Order 7".to_string()),
            ("anchor", element_anchor().to_string()),
            ("body", "The date picker jumps to January".to_string()),
        ] {
            b.extend_from_slice(
                format!("--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let pq = format!("/api/queues/{}/reporter/reports", self.qid);
        let ct = format!("multipart/form-data; boundary={boundary}");
        let (s, v) = self.as_reporter(r, "POST", &pq, b, Some(&ct)).await;
        assert_eq!(s, 201, "{v}");
        v["report"]["id"].as_str().unwrap().to_string()
    }

    fn report(&self, tail: &str) -> String {
        format!("/api/queues/{}/reports/{}{tail}", self.qid, self.tid)
    }

    /// The owner's agent, from its session.
    async fn agent(&self, method: &str, pq: &str, body: Value) -> (u16, Value) {
        let res = self
            .ts
            .authed(
                self.ts
                    .client
                    .request(method.parse().unwrap(), format!("{}{pq}", self.ts.base)),
            )
            .header("x-clax-session", &self.sid)
            .json(&body)
            .send()
            .await
            .unwrap();
        answer(res).await
    }

    /// The agent claims the report and proposes a fix with `preview`.
    async fn fix(&self, preview: bool) -> (u16, Value) {
        let (s, v) = self.agent("POST", &self.report("/claim"), json!({})).await;
        assert_eq!(s, 200, "{v}");
        self.agent(
            "POST",
            &self.report("/fix"),
            json!({"summary": "Clamp the month", "url": "https://github.com/acme/app/pull/538",
                   "preview": preview}),
        )
        .await
    }

    /// The ticket of a fix with preview.
    async fn ticket(&self) -> Value {
        let (s, v) = self.fix(true).await;
        assert_eq!(s, 200, "{v}");
        v["ticket"].clone()
    }

    /// The preview's backend presents `ticket` at `pq` with `body`.
    async fn present(
        &self,
        ticket: &str,
        pq: &str,
        body: Value,
        extra: &[(&str, String)],
    ) -> (u16, Value) {
        let mut h = vec![
            ("authorization", format!("Clax-Fix {ticket}")),
            ("content-type", "application/json".to_string()),
        ];
        h.extend(extra.iter().cloned());
        answer(
            self.send("POST", pq, &h, serde_json::to_vec(&body).unwrap())
                .await,
        )
        .await
    }

    async fn attach(&self, t: &Value, body: Value) -> (u16, Value) {
        self.present(
            t["token"].as_str().unwrap(),
            t["attach_path"].as_str().unwrap(),
            body,
            &[],
        )
        .await
    }

    fn kinds(&self, kind: &str) -> Vec<(Value, Value)> {
        self.ts
            .store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                (
                    serde_json::from_str(&e.actor).unwrap(),
                    serde_json::from_str(&e.body).unwrap(),
                )
            })
            .collect()
    }
}

fn grant_of(v: &Value) -> (String, KeyPair) {
    use base64::Engine as _;
    let pkcs8 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(v["private_key"].as_str().unwrap())
        .unwrap();
    (
        v["token"].as_str().unwrap().to_string(),
        KeyPair::from_pkcs8(&pkcs8).unwrap(),
    )
}

#[tokio::test]
async fn a_grant_limited_to_the_preview_pattern_mints_for_previews_alone() {
    let s = Setup::new().await;
    let (st, v) = s
        .grant(json!({"name": "previews", "origins": [PATTERN]}))
        .await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["grant"]["origins"], json!([PATTERN]));
    let previews = grant_of(&v);
    let (st, v, _) = s.mint(&previews, "emp-1", PREVIEW).await;
    assert_eq!(st, 200, "{v}");
    let (st, v, _) = s.mint(&previews, "emp-1", ORIGIN).await;
    assert_eq!((st, code(&v)), (403, "wrong_origin"), "{v}");
    // The listing says which grant is limited.
    let res =
        s.ts.authed(client().get(format!("{}/api/queues/{}/service-grants", s.ts.base, s.qid)))
            .send()
            .await
            .unwrap();
    let (_, v) = answer(res).await;
    let origins: Vec<&Value> = v["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| &g["origins"])
        .collect();
    assert_eq!(origins, [&Value::Null, &json!([PATTERN])], "{v}");
    // Its origins are among the queue's.
    for bad in [
        json!([]),
        json!(["https://other-*.fly.dev"]),
        json!(["https://evil.example"]),
    ] {
        let (st, v) = s.grant(json!({"name": "x", "origins": bad})).await;
        assert_eq!((st, code(&v)), (400, "invalid_origin"), "{bad}: {v}");
    }
}

#[tokio::test]
async fn a_preview_fix_is_issued_a_ticket_shown_once() {
    let s = Setup::new().await;
    let t = s.ticket().await;
    let token = t["token"].as_str().unwrap();
    assert!(
        token.starts_with(&format!("cxf_{}.{}.1.", s.qid, s.tid)),
        "{t}"
    );
    assert_eq!(t["n"], 1);
    assert_eq!(t["origins"], json!([PATTERN]));
    assert_eq!(
        t["attach_path"],
        json!(format!(
            "/api/queues/{}/reports/{}/fixes/1/preview",
            s.qid, s.tid
        ))
    );
    assert_eq!(t["expires_at"], json!("2026-12-08T10:00:00.000Z"));
    // Never again: the report and the record name no token.
    let (_, v) = s.agent("GET", &s.report(""), json!(null)).await;
    assert!(!v.to_string().contains(token));
    let fixes = s.kinds("queue.fix");
    assert!(fixes[0].1["ticket_id"].is_string(), "{fixes:?}");
    assert!(!serde_json::to_string(&fixes).unwrap().contains(token));
}

#[tokio::test]
async fn a_queue_without_a_pattern_issues_no_ticket() {
    let s = Setup::with_origins(vec![ORIGIN.into()]).await;
    let (st, v) = s.fix(true).await;
    assert_eq!((st, code(&v)), (400, "no_preview_origins"), "{v}");
    let (st, v) = s.fix(false).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v.get("ticket"), None);
}

#[tokio::test]
async fn the_preview_attaches_checks_and_verifies_on_its_own_origin() {
    let s = Setup::new().await;
    let t = s.ticket().await;
    let (st, v) = s
        .attach(&t, json!({"origin": PREVIEW, "path": "/orders/7"}))
        .await;
    assert_eq!(st, 200, "{v}");
    let url = format!("{PREVIEW}/orders/7");
    assert_eq!(
        (&v["verify_url"], &v["changed"]),
        (&json!(url), &json!(true))
    );
    assert_eq!(v["report"]["fixes"][0]["verify_url"], json!(url));
    let notice = v["report"]["comments"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(notice["body"].as_str().unwrap().contains(&url), "{notice}");
    // Again, on a redeploy: no change.
    let (st, v) = s
        .attach(&t, json!({"origin": PREVIEW, "path": "/orders/7"}))
        .await;
    assert_eq!((st, &v["changed"]), (200, &json!(false)), "{v}");
    let rows = s.kinds("queue.preview");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0["type"], "fix_ticket");
    assert_eq!(rows[0].1["origin"], PREVIEW);
    assert_eq!(rows[0].1["issued_by"], "claude");
    assert_eq!(rows[0].1["for_person"], "owner");

    // The reporter, on the preview through the grant limited to it, finds
    // the fix there and nowhere else.
    let (_, v) = s
        .grant(json!({"name": "previews", "origins": [PATTERN]}))
        .await;
    let on_preview = s.reporter_by(&grant_of(&v), "emp-1", PREVIEW).await;
    let pq = format!("/api/queues/{}/reporter/previews", s.qid);
    let (st, v) = s.as_reporter(&on_preview, "GET", &pq, vec![], None).await;
    assert_eq!(st, 200, "{v}");
    let ids: Vec<&Value> = v["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["id"])
        .collect();
    assert_eq!(ids, [&json!(s.tid)], "{v}");
    assert_eq!(v["reports"][0]["reporter_profile"], Value::Null);
    let on_prod = s.reporter_by(&s.prod_grant().await, "emp-1", ORIGIN).await;
    let (_, v) = s.as_reporter(&on_prod, "GET", &pq, vec![], None).await;
    assert_eq!(v["reports"], json!([]), "{v}");
    let other = s.reporter_by(&s.prod_grant().await, "emp-2", PREVIEW).await;
    let (_, v) = s.as_reporter(&other, "GET", &pq, vec![], None).await;
    assert_eq!(v["reports"], json!([]), "another reporter's: {v}");

    // It's fixed, on the preview: verified, and where.
    let vq = format!("/api/queues/{}/reporter/reports/{}/verify", s.qid, s.tid);
    let (st, v) = s
        .as_reporter(
            &on_preview,
            "POST",
            &vq,
            b"{}".to_vec(),
            Some("application/json"),
        )
        .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["report"]["state"], "verified");
    assert_eq!(v["report"]["fixes"][0]["verified_origin"], PREVIEW);
    let (_, v) = s.agent("GET", &s.report(""), json!(null)).await;
    assert_eq!(
        v["report"]["fixes"][0]["verified_origin"], PREVIEW,
        "triage reads it"
    );
    assert_eq!(s.kinds("queue.verify")[0].1["origin"], PREVIEW);

    // The ticket ended with the verdict.
    let (st, v) = s.attach(&t, json!({"origin": PREVIEW})).await;
    assert_eq!((st, code(&v)), (401, "token_expired"), "{v}");
}

#[tokio::test]
async fn a_ticket_attaches_only_an_origin_its_patterns_match() {
    let s = Setup::new().await;
    let t = s.ticket().await;
    for (body, want) in [
        (json!({"origin": ORIGIN}), (403, "wrong_origin")),
        (
            json!({"origin": "https://app-pr-1.evil.fly.dev"}),
            (403, "wrong_origin"),
        ),
        (
            json!({"origin": "http://app-pr-538.fly.dev"}),
            (403, "wrong_origin"),
        ),
        (json!({"origin": PATTERN}), (400, "invalid_origin")),
        (
            json!({"origin": PREVIEW, "path": "//evil.example"}),
            (400, "invalid_args"),
        ),
        (
            json!({"origin": PREVIEW, "reporter_id": "emp-1"}),
            (400, "invalid_args"),
        ),
        (json!({}), (400, "invalid_args")),
    ] {
        let (st, v) = s.attach(&t, body.clone()).await;
        assert_eq!((st, code(&v)), want, "{body}: {v}");
    }
    assert!(s.kinds("queue.preview").is_empty());
}

#[tokio::test]
async fn a_ticket_does_nothing_but_attach_to_its_own_fix() {
    let s = Setup::new().await;
    let t = s.ticket().await;
    let token = t["token"].as_str().unwrap();
    let (q, r) = (&s.qid, &s.tid);
    // Another fix, report or queue's route; every other queue route; a
    // route of the daemon's own.
    for (method, pq) in [
        (
            "POST",
            format!("/api/queues/{q}/reports/{r}/fixes/2/preview"),
        ),
        (
            "POST",
            format!("/api/queues/{q}/reports/01JB9T00000000000000000001/fixes/1/preview"),
        ),
        (
            "POST",
            format!("/api/queues/01JBQQ00000000000000000001/reports/{r}/fixes/1/preview"),
        ),
        ("POST", format!("/api/queues/{q}/reporter-tokens")),
        ("GET", format!("/api/queues/{q}/reporter/reports")),
        ("GET", format!("/api/queues/{q}/reporter/previews")),
        ("GET", format!("/api/queues/{q}/reporter/badge")),
        (
            "POST",
            format!("/api/queues/{q}/reporter/reports/{r}/verify"),
        ),
        (
            "POST",
            format!("/api/queues/{q}/reporter/reports/{r}/comments"),
        ),
        ("GET", format!("/api/queues/{q}/reports/{r}")),
        ("POST", format!("/api/queues/{q}/reports/{r}/verify")),
        ("POST", format!("/api/queues/{q}/reports/{r}/fix")),
        ("GET", format!("/api/queues/{q}/service-grants")),
        ("GET", "/api/artifacts".to_string()),
        ("GET", "/api/status".to_string()),
    ] {
        let res = s
            .send(
                method,
                &pq,
                &[
                    ("authorization", format!("Clax-Fix {token}")),
                    ("content-type", "application/json".into()),
                ],
                serde_json::to_vec(&json!({"origin": PREVIEW, "body": "x"})).unwrap(),
            )
            .await;
        let (st, v) = answer(res).await;
        assert_eq!((st, code(&v)), (401, "invalid_token"), "{method} {pq}: {v}");
    }
    // From a browser: refused before anything.
    for h in [
        ("origin", PREVIEW.to_string()),
        ("cookie", "clax_viewer=x".to_string()),
    ] {
        let (st, v) = s
            .present(
                token,
                t["attach_path"].as_str().unwrap(),
                json!({"origin": PREVIEW}),
                std::slice::from_ref(&h),
            )
            .await;
        assert_eq!((st, code(&v)), (403, "forbidden_origin"), "{h:?}: {v}");
    }
    // A made-up ticket of the right shape.
    let forged = format!("cxf_{q}.{r}.1.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let (st, v) = s
        .present(
            &forged,
            t["attach_path"].as_str().unwrap(),
            json!({"origin": PREVIEW}),
            &[],
        )
        .await;
    assert_eq!((st, code(&v)), (401, "invalid_token"), "{v}");
    // Nothing changed on the report.
    let (_, v) = s.agent("GET", &s.report(""), json!(null)).await;
    assert_eq!(v["report"]["fixes"][0].get("verify_url"), None, "{v}");
    assert_eq!(v["report"]["state"], "fix_proposed");
}

#[tokio::test]
async fn a_ticket_ends_after_fourteen_days_and_at_a_rejection() {
    let s = Setup::new().await;
    let t = s.ticket().await;
    s.clock.advance(FIX_TICKET_SECS - 1);
    let (st, v) = s.attach(&t, json!({"origin": PREVIEW})).await;
    assert_eq!(st, 200, "{v}");
    s.clock.advance(1);
    let (st, v) = s
        .attach(&t, json!({"origin": "https://app-pr-539.fly.dev"}))
        .await;
    assert_eq!((st, code(&v)), (401, "token_expired"), "{v}");

    // A triage person's rejection ends the next ticket too.
    let (st, v) = s.agent("POST", &s.report("/claim"), json!({})).await;
    assert_eq!((st, code(&v)), (409, "not_claimable"), "{v}");
    let res =
        s.ts.authed(client().post(format!("{}{}", s.ts.base, s.report("/reopen"))))
            .json(&json!({"body": "Still jumps"}))
            .send()
            .await
            .unwrap();
    let (st, v) = answer(res).await;
    assert_eq!(st, 200, "{v}");
    let t2 = s.ticket().await;
    assert_eq!(t2["n"], 2);
    let res =
        s.ts.authed(client().post(format!("{}{}", s.ts.base, s.report("/reopen"))))
            .json(&json!({"body": "Still jumps, again"}))
            .send()
            .await
            .unwrap();
    assert_eq!(answer(res).await.0, 200);
    let (st, v) = s.attach(&t2, json!({"origin": PREVIEW})).await;
    assert_eq!((st, code(&v)), (401, "token_expired"), "{v}");
}
