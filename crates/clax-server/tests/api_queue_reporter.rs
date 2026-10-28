//! A queue's service and reporter routes (spec 2026-10-27-clax-sdk-design
//! §5.3): signed requests of a service grant and of reporter tokens, the
//! reporter's origin, its view of the queue, CORS, and the refusal of a
//! queue credential everywhere else.

use clax_core::audit::AuditCtx;
use clax_core::queue::ReportersSee;
use clax_core::sig::{self, KeyPair, SignInput};
use clax_core::working::ManualClock;
use clax_store::queues::NewQueue;
use clax_testkit::TestServer;
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::Arc;

const ORIGIN: &str = "https://app.example";
const OTHER: &str = "https://evil.example";

/// The smallest PNG a clip check takes: its signature and a little more.
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(b"\0\0\0\rIHDR clip bytes");
    b
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// A daemon with one queue for [`ORIGIN`] whose reporters see their own
/// reports, and that queue's service grant.
struct Setup {
    ts: TestServer,
    clock: Arc<ManualClock>,
    qid: String,
    service_token: String,
    service_key: KeyPair,
}

impl Setup {
    async fn new() -> Setup {
        let clock = Arc::new(ManualClock::at("2026-10-27T10:00:00Z"));
        let ts = TestServer::spawn_with_clock(clock.clone()).await;
        let qid = queue(&ts, ReportersSee::Own);
        let (_, token, key) = ts
            .store
            .create_service_grant(&AuditCtx::DAEMON, &qid, "app backend")
            .unwrap();
        Setup {
            ts,
            clock,
            qid,
            service_token: token.expose().to_string(),
            service_key: key,
        }
    }

    fn host(&self) -> String {
        self.ts.base.strip_prefix("http://").unwrap().to_string()
    }

    fn now(&self) -> i64 {
        self.ts.store.now_utc().timestamp()
    }

    /// `Clax-Signature` of a request signed by `key` now.
    fn sign(&self, key: &KeyPair, method: &str, pq: &str, token: &str, body: &[u8]) -> String {
        signature(key, method, pq, &self.host(), self.now(), token, body)
    }

    /// Sends `method pq` with `headers` and `body`.
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

    /// A signed service request.
    async fn service(&self, pq: &str, body: &Value) -> reqwest::Response {
        let body = serde_json::to_vec(body).unwrap();
        let sig = self.sign(&self.service_key, "POST", pq, &self.service_token, &body);
        self.send(
            "POST",
            pq,
            &[
                (
                    "authorization",
                    format!("Clax-Queue {}", self.service_token),
                ),
                ("clax-signature", sig),
                ("content-type", "application/json".into()),
            ],
            body,
        )
        .await
    }

    /// Mints a reporter token for `reporter_id` on [`ORIGIN`].
    async fn reporter(&self, reporter_id: &str) -> Reporter {
        let key = KeyPair::generate();
        let res = self
            .service(
                &format!("/api/queues/{}/reporter-tokens", self.qid),
                &json!({"reporter_id": reporter_id, "origin": ORIGIN,
                        "public_key": sig::encode_public(&key.public())}),
            )
            .await;
        assert_eq!(res.status(), 200);
        let v: Value = res.json().await.unwrap();
        Reporter {
            token: v["token"].as_str().unwrap().to_string(),
            key,
        }
    }

    /// A reporter's signed request from `origin` (`None`: no `Origin`).
    async fn as_reporter(
        &self,
        r: &Reporter,
        origin: Option<&str>,
        method: &str,
        pq: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let sig = self.sign(&r.key, method, pq, &r.token, &body);
        let mut h = vec![
            ("authorization", format!("Clax-Reporter {}", r.token)),
            ("clax-signature", sig),
        ];
        if let Some(o) = origin {
            h.push(("origin", o.to_string()));
        }
        if let Some(ct) = content_type {
            h.push(("content-type", ct.to_string()));
        }
        self.send(method, pq, &h, body).await
    }

    fn reports(&self) -> String {
        format!("/api/queues/{}/reporter/reports", self.qid)
    }

    /// Files a report as `r` from [`ORIGIN`], with a clip.
    async fn file(&self, r: &Reporter, body: &str) -> reqwest::Response {
        let (mp, ct) = multipart(&[
            ("url", Part::Text("https://app.example/orders/7#items")),
            ("title", Part::Text("Order 7")),
            (
                "anchor",
                Part::Text(&clax_testkit::element_anchor().to_string()),
            ),
            ("body", Part::Text(body)),
            ("metadata", Part::Text(r#"{"order":7}"#)),
            ("clip", Part::Png(png())),
        ]);
        self.as_reporter(r, Some(ORIGIN), "POST", &self.reports(), Some(&ct), mp)
            .await
    }
}

struct Reporter {
    token: String,
    key: KeyPair,
}

fn queue(ts: &TestServer, see: ReportersSee) -> String {
    ts.store
        .create_queue(
            &AuditCtx::DAEMON,
            None,
            NewQueue {
                name: "Orders".into(),
                origins: vec![ORIGIN.into()],
                reporters_see: see,
            },
        )
        .unwrap()
        .id
}

/// `Clax-Signature` of `method pq` to `host` at `t` by `key`, with `token`
/// and `body` and no agent headers: what `web/sdk`'s `signedFetch` sends.
fn signature(
    key: &KeyPair,
    method: &str,
    pq: &str,
    host: &str,
    t: i64,
    token: &str,
    body: &[u8],
) -> String {
    let nonce = sig::new_nonce();
    key.sign(&SignInput {
        method,
        path_and_query: pq,
        host,
        t,
        nonce: &nonce,
        body_sha256: sha2::Sha256::digest(body).into(),
        token_sha256: Some(clax_core::secret::sha256(token)),
        agent_headers_sha256: sig::no_agent_headers_sha256(),
    })
    .unwrap()
    .to_string()
}

enum Part<'a> {
    Text(&'a str),
    Png(Vec<u8>),
}

/// A `multipart/form-data` body as `web/sdk`'s `multipart` builds it.
fn multipart(parts: &[(&str, Part<'_>)]) -> (Vec<u8>, String) {
    let boundary = "clax-testboundary";
    let mut b = Vec::new();
    for (name, p) in parts {
        match p {
            Part::Text(t) => {
                b.extend_from_slice(
                    format!(
                        "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n"
                    )
                    .as_bytes(),
                );
                b.extend_from_slice(t.as_bytes());
            }
            Part::Png(bytes) => {
                b.extend_from_slice(format!("--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"; filename=\"clip.png\"\r\ncontent-type: image/png\r\n\r\n").as_bytes());
                b.extend_from_slice(bytes);
            }
        }
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (b, format!("multipart/form-data; boundary={boundary}"))
}

async fn status_code(res: reqwest::Response) -> (u16, String) {
    let s = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or_default();
    (
        s,
        v["error"]["code"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test]
async fn mint_takes_a_good_signature_only() {
    let s = Setup::new().await;
    let pq = format!("/api/queues/{}/reporter-tokens", s.qid);
    let key = KeyPair::generate();
    let body = json!({"reporter_id": "emp-1", "origin": ORIGIN,
                      "public_key": sig::encode_public(&key.public())});
    let res = s.service(&pq, &body).await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert!(v["token"].as_str().unwrap().starts_with("cxr_"));
    assert!(v["expires_at"].as_str().unwrap() > "2026-10-27T10:14");

    let bytes = serde_json::to_vec(&body).unwrap();
    let auth = ("authorization", format!("Clax-Queue {}", s.service_token));
    let ct = ("content-type", "application/json".to_string());
    // Signed by another key.
    let forged = s.sign(&KeyPair::generate(), "POST", &pq, &s.service_token, &bytes);
    let res = s
        .send(
            "POST",
            &pq,
            &[auth.clone(), ("clax-signature", forged), ct.clone()],
            bytes.clone(),
        )
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    // Signed over another body.
    let other = s.sign(&s.service_key, "POST", &pq, &s.service_token, b"{}");
    let res = s
        .send(
            "POST",
            &pq,
            &[auth.clone(), ("clax-signature", other), ct.clone()],
            bytes.clone(),
        )
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    // No signature.
    let res = s
        .send("POST", &pq, &[auth.clone(), ct.clone()], bytes.clone())
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    // A good request sent twice: the second is a replay.
    let good = s.sign(&s.service_key, "POST", &pq, &s.service_token, &bytes);
    let h = [auth.clone(), ("clax-signature", good), ct.clone()];
    assert_eq!(s.send("POST", &pq, &h, bytes.clone()).await.status(), 200);
    let res = s.send("POST", &pq, &h, bytes.clone()).await;
    assert_eq!(status_code(res).await, (401, "replayed_request".into()));
    // Too far from the daemon's clock.
    let stale = signature(
        &s.service_key,
        "POST",
        &pq,
        &s.host(),
        s.now() - 200,
        &s.service_token,
        &bytes,
    );
    let res = s
        .send(
            "POST",
            &pq,
            &[auth.clone(), ("clax-signature", stale), ct.clone()],
            bytes.clone(),
        )
        .await;
    assert_eq!(status_code(res).await, (401, "stale_request".into()));
    // A browser's request.
    let sig = s.sign(&s.service_key, "POST", &pq, &s.service_token, &bytes);
    let res = s
        .send(
            "POST",
            &pq,
            &[
                auth.clone(),
                ("clax-signature", sig),
                ct.clone(),
                ("origin", ORIGIN.into()),
            ],
            bytes.clone(),
        )
        .await;
    assert_eq!(status_code(res).await, (403, "forbidden_origin".into()));
    // An origin the queue does not list.
    let res = s
        .service(
            &pq,
            &json!({"reporter_id": "emp-1", "origin": OTHER,
                    "public_key": sig::encode_public(&key.public())}),
        )
        .await;
    assert_eq!(status_code(res).await, (403, "wrong_origin".into()));
    // The grant's token on another queue's route.
    let q2 = queue(&s.ts, ReportersSee::Own);
    let res = s
        .service(&format!("/api/queues/{q2}/reporter-tokens"), &body)
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
}

#[tokio::test]
async fn a_reporter_files_with_a_clip_and_reads_it_back() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    let res = s.file(&r, "The total is wrong").await;
    assert_eq!(res.status(), 201);
    assert_eq!(
        res.headers()["access-control-allow-origin"]
            .to_str()
            .unwrap(),
        ORIGIN
    );
    let v: Value = res.json().await.unwrap();
    let report = &v["report"];
    let tid = report["id"].as_str().unwrap().to_string();
    assert_eq!(report["reporter_id"], "emp-1");
    assert_eq!(report["has_clip"], true);
    assert_eq!(report["metadata"], json!({"order": 7}));
    assert!(
        report["artifact"]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://app.example/orders/7")
    );
    assert_eq!(report["comments"][0]["body"], "The total is wrong");

    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["reports"].as_array().unwrap().len(), 1);
    assert_eq!(v["reports"][0]["id"], tid.as_str());
    assert_eq!(v["next"], Value::Null);

    let clip = format!("{}/{tid}/clip", s.reports());
    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &clip, None, vec![])
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.bytes().await.unwrap().to_vec(), png());

    let comments = format!("{}/{tid}/comments", s.reports());
    let body = serde_json::to_vec(&json!({"body": "It is 12, not 21"})).unwrap();
    let res = s
        .as_reporter(
            &r,
            Some(ORIGIN),
            "POST",
            &comments,
            Some("application/json"),
            body,
        )
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let all = v["report"]["comments"].as_array().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["body"], "It is 12, not 21");
    assert_eq!(all[1]["author_name"], "emp-1");

    // A body other than the one signed.
    let (mp, ct) = multipart(&[("body", Part::Text("x"))]);
    let sig = s.sign(&r.key, "POST", &s.reports(), &r.token, b"other");
    let res = s
        .send(
            "POST",
            &s.reports(),
            &[
                ("authorization", format!("Clax-Reporter {}", r.token)),
                ("clax-signature", sig),
                ("origin", ORIGIN.into()),
                ("content-type", ct),
            ],
            mp,
        )
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
}

#[tokio::test]
async fn a_report_s_fields_are_checked() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    let anchor = clax_testkit::element_anchor().to_string();
    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(16 * 1024));
    let cases: Vec<(Vec<(&str, Part<'_>)>, &str)> = vec![
        (
            vec![
                ("url", Part::Text("https://app.example/")),
                ("title", Part::Text("t")),
                ("anchor", Part::Text(&anchor)),
                ("body", Part::Text("b")),
                ("clip", Part::Png(b"GIF89a".to_vec())),
            ],
            "invalid_clip",
        ),
        (
            vec![
                ("url", Part::Text("https://app.example/")),
                ("title", Part::Text("t")),
                ("anchor", Part::Text(&anchor)),
                ("body", Part::Text("b")),
                ("metadata", Part::Text(&big)),
            ],
            "metadata_too_large",
        ),
        (
            vec![
                ("url", Part::Text("https://app.example/")),
                ("title", Part::Text("t")),
                ("anchor", Part::Text("{")),
                ("body", Part::Text("b")),
            ],
            "invalid_anchor",
        ),
        (
            vec![
                ("url", Part::Text("https://app.example/")),
                ("title", Part::Text("t")),
                ("anchor", Part::Text(&anchor)),
                ("body", Part::Text("b")),
                ("reporter_id", Part::Text("someone-else")),
            ],
            "invalid_args",
        ),
    ];
    for (parts, want) in cases {
        let (mp, ct) = multipart(&parts);
        let res = s
            .as_reporter(&r, Some(ORIGIN), "POST", &s.reports(), Some(&ct), mp)
            .await;
        assert_eq!(status_code(res).await, (400, want.to_string()));
    }
    // Metadata is measured as compact JSON, not as sent.
    let spaced = format!(
        "{{{}\"x\": \"{}\"}}",
        " ".repeat(20 * 1024),
        "a".repeat(1024)
    );
    let (mp, ct) = multipart(&[
        ("url", Part::Text("https://app.example/")),
        ("title", Part::Text("t")),
        ("anchor", Part::Text(&anchor)),
        ("body", Part::Text("b")),
        ("metadata", Part::Text(&spaced)),
    ]);
    let res = s
        .as_reporter(&r, Some(ORIGIN), "POST", &s.reports(), Some(&ct), mp)
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["report"]["metadata"]["x"].as_str().map(str::len), Some(1024));
}

#[tokio::test]
async fn a_reporter_token_holds_only_from_its_origin() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    for origin in [Some(OTHER), Some("https://app.example:8443"), None] {
        let res = s
            .as_reporter(&r, origin, "GET", &s.reports(), None, vec![])
            .await;
        assert_eq!(
            status_code(res).await,
            (403, "wrong_origin".into()),
            "{origin:?}"
        );
    }
    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(res.status(), 200);
    // An origin the queue no longer lists ends the tokens minted for it.
    s.ts.store
        .update_queue(
            &AuditCtx::DAEMON,
            &s.qid,
            clax_store::queues::QueuePatch {
                origins: Some(vec!["https://new.example".into()]),
                ..Default::default()
            },
        )
        .unwrap();
    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(status_code(res).await, (403, "wrong_origin".into()));
}

#[tokio::test]
async fn a_reporter_token_expires_and_ends_with_its_grant() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    s.clock.advance(901);
    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(status_code(res).await, (401, "token_expired".into()));

    let r = s.reporter("emp-1").await;
    let gid = s.ts.store.service_grants(&s.qid).unwrap()[0].id.clone();
    s.ts.store
        .revoke_service_grant(&AuditCtx::DAEMON, &s.qid, &gid)
        .unwrap();
    let res = s
        .as_reporter(&r, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
    let res = s
        .service(
            &format!("/api/queues/{}/reporter-tokens", s.qid),
            &json!({"reporter_id": "x", "origin": ORIGIN,
                    "public_key": sig::encode_public(&KeyPair::generate().public())}),
        )
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
}

#[tokio::test]
async fn a_reporter_sees_only_its_own_reports_under_own() {
    let s = Setup::new().await;
    let a = s.reporter("emp-a").await;
    let b = s.reporter("emp-b").await;
    let v: Value = s.file(&a, "mine").await.json().await.unwrap();
    let tid = v["report"]["id"].as_str().unwrap().to_string();

    let v: Value = s
        .as_reporter(&b, Some(ORIGIN), "GET", &s.reports(), None, vec![])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["reports"], json!([]));
    let clip = format!("{}/{tid}/clip", s.reports());
    let res = s
        .as_reporter(&b, Some(ORIGIN), "GET", &clip, None, vec![])
        .await;
    assert_eq!(res.status(), 404);
    let comments = format!("{}/{tid}/comments", s.reports());
    let body = serde_json::to_vec(&json!({"body": "me too"})).unwrap();
    let res = s
        .as_reporter(
            &b,
            Some(ORIGIN),
            "POST",
            &comments,
            Some("application/json"),
            body,
        )
        .await;
    assert_eq!(res.status(), 404);
    // Its own reporter reads it.
    let res = s
        .as_reporter(&a, Some(ORIGIN), "GET", &clip, None, vec![])
        .await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn the_preflight_answers_only_listed_origins() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    let paths = [
        s.reports(),
        format!("{}/t1/comments", s.reports()),
        format!("{}/t1/clip", s.reports()),
    ];
    for pq in &paths {
        let res = s
            .send(
                "OPTIONS",
                pq,
                &[
                    ("origin", ORIGIN.into()),
                    ("access-control-request-method", "POST".into()),
                    (
                        "access-control-request-headers",
                        "authorization,clax-signature".into(),
                    ),
                ],
                vec![],
            )
            .await;
        assert_eq!(res.status(), 204, "{pq}");
        let h = res.headers();
        assert_eq!(h["access-control-allow-origin"], ORIGIN);
        assert_eq!(
            h["access-control-allow-headers"],
            "authorization, clax-signature, content-type"
        );
        assert!(h.get("access-control-allow-credentials").is_none());
        assert!(h.get_all("vary").iter().any(|v| v == "Origin"));

        let res = s
            .send(
                "OPTIONS",
                pq,
                &[
                    ("origin", OTHER.into()),
                    ("access-control-request-method", "POST".into()),
                ],
                vec![],
            )
            .await;
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "{pq}"
        );
        assert!(res.headers().get("access-control-allow-headers").is_none());
    }
    // A refusal from another origin names none either.
    let res = s
        .as_reporter(&r, Some(OTHER), "GET", &s.reports(), None, vec![])
        .await;
    assert_eq!(res.status(), 403);
    assert!(res.headers().get("access-control-allow-origin").is_none());
    // The service route is not for browsers.
    let res = s
        .send(
            "OPTIONS",
            &format!("/api/queues/{}/reporter-tokens", s.qid),
            &[("origin", ORIGIN.into())],
            vec![],
        )
        .await;
    assert!(res.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn every_queue_route_needs_its_own_credential() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    let mint = format!("/api/queues/{}/reporter-tokens", s.qid);
    let routes = [
        ("POST", mint.clone()),
        ("GET", s.reports()),
        ("POST", s.reports()),
        ("POST", format!("{}/t1/comments", s.reports())),
        ("GET", format!("{}/t1/clip", s.reports())),
    ];
    for (method, pq) in &routes {
        let reporter_route = !pq.ends_with("reporter-tokens");
        let mut origin = vec![];
        if reporter_route {
            origin.push(("origin", ORIGIN.to_string()));
        }
        // None at all.
        let res = s.send(method, pq, &origin, vec![]).await;
        assert_eq!(
            status_code(res).await,
            (401, "invalid_token".into()),
            "{method} {pq}"
        );
        // The daemon's own token, and an unknown token of the scheme.
        let wrong = if reporter_route {
            "Clax-Reporter cxr_unknown"
        } else {
            "Clax-Queue cxq_unknown"
        };
        for auth in [format!("Bearer {}", s.ts.token), wrong.to_string()] {
            let mut h = origin.clone();
            h.push(("authorization", auth.clone()));
            let res = s.send(method, pq, &h, vec![]).await;
            assert_eq!(
                status_code(res).await,
                (401, "invalid_token".into()),
                "{method} {pq} {auth}"
            );
        }
        // The other kind of queue credential, signed.
        let (auth, key, token) = if reporter_route {
            (
                format!("Clax-Queue {}", s.service_token),
                &s.service_key,
                &s.service_token,
            )
        } else {
            (format!("Clax-Reporter {}", r.token), &r.key, &r.token)
        };
        let mut h = origin.clone();
        h.push(("authorization", auth));
        h.push(("clax-signature", s.sign(key, method, pq, token, b"")));
        let res = s.send(method, pq, &h, vec![]).await;
        assert_eq!(
            status_code(res).await,
            (401, "invalid_token".into()),
            "{method} {pq}"
        );
    }
}

#[tokio::test]
async fn a_queue_credential_holds_on_no_other_route() {
    let s = Setup::new().await;
    let r = s.reporter("emp-1").await;
    let aid = s.ts.publish("Page", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let q = &s.qid;
    let routes = [
        ("GET", "/api/artifacts".to_string()),
        ("GET", format!("/api/artifacts/{aid}")),
        ("POST", "/api/artifacts".to_string()),
        ("GET", "/api/token".to_string()),
        ("GET", "/api/status".to_string()),
        ("GET", "/api/queues".to_string()),
        ("POST", "/api/queues".to_string()),
        ("GET", format!("/api/queues/{q}")),
        ("GET", format!("/api/queues/{q}/members")),
        ("GET", format!("/api/queues/{q}/service-grants")),
        ("POST", format!("/api/queues/{q}/service-grants")),
        ("GET", format!("/api/queues/{q}/reports")),
        ("POST", format!("/api/queues/{q}/reports/t1/claim")),
        ("GET", format!("/api/queues/{q}/reports/t1/clip")),
        ("GET", "/api/events".to_string()),
        ("GET", "/api/nothing-here".to_string()),
        ("GET", "/".to_string()),
    ];
    for (method, pq) in &routes {
        let creds = [
            (
                format!("Clax-Reporter {}", r.token),
                s.sign(&r.key, method, pq, &r.token, b""),
            ),
            (
                format!("Clax-Queue {}", s.service_token),
                s.sign(&s.service_key, method, pq, &s.service_token, b""),
            ),
        ];
        for (auth, sig) in creds {
            let res = s
                .send(
                    method,
                    pq,
                    &[
                        ("authorization", auth.clone()),
                        ("clax-signature", sig),
                        ("origin", ORIGIN.into()),
                    ],
                    vec![],
                )
                .await;
            assert_eq!(
                status_code(res).await,
                (401, "invalid_token".into()),
                "{method} {pq} {auth}"
            );
        }
    }
    // On a site's host too.
    let res = client()
        .get(format!("{}/", s.ts.base))
        .header("host", s.ts.site_host(&aid))
        .header("authorization", format!("Clax-Reporter {}", r.token))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
}

/// A request signed as `web/sdk` signs it: the inputs and signature that
/// `clax_core::sig`'s `sdk_vector_is_fixed` pins (and `web/sdk`'s own
/// vector test), at the daemon. The daemon verifies it (the route then
/// refuses its JSON body as a report's multipart); one changed byte of the
/// signature does not verify.
#[tokio::test]
async fn a_request_signed_by_the_sdk_vector_verifies() {
    let clock = Arc::new(ManualClock::at("2025-10-09T08:53:20Z"));
    let ts = TestServer::spawn_with_clock(clock).await;
    assert_eq!(ts.store.now_utc().timestamp(), 1_760_000_000);
    let made = queue(&ts, ReportersSee::Own);
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    db.execute("UPDATE queues SET id = 'q1' WHERE id = ?1", [&made])
        .unwrap();
    let (grant, _, _) = ts
        .store
        .create_service_grant(&AuditCtx::DAEMON, "q1", "app")
        .unwrap();
    db.execute(
        "INSERT INTO queue_reporter_tokens (token_sha256, queue_id, service_grant_id,
            reporter_id, origin, public_key, expires_at)
         VALUES (?1, 'q1', ?2, 'emp-1', ?3, 'IVL40Zt5HSRFMkLhXy6rbLfP-ntqXtMAl5YOBpiB2xI',
            '2099-01-01T00:00:00.000Z')",
        rusqlite::params![
            clax_core::secret::sha256_hex("cxr_vector"),
            grant.id,
            ORIGIN
        ],
    )
    .unwrap();
    let pinned = "t=1760000000, n=AAECAwQFBgcICQoLDA0ODw, \
         s=8DwcAbZkGf5Qhq47ATzagkKAuIr-T4h6vKczyevVcv1UchPiD_ba5rmjr8HyInFyYULBDia1-PZesVfaOyxtBA";
    let send = |sig: String| {
        client()
            .post(format!(
                "{}/api/queues/q1/reporter/reports?cursor=c1",
                ts.base
            ))
            .header("host", "clax.example:8443")
            .header("authorization", "Clax-Reporter cxr_vector")
            .header("clax-signature", sig)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(r#"{"body":"hi"}"#)
            .send()
    };
    let tampered = pinned.replace("s=8Dw", "s=9Dw");
    let res = send(tampered).await.unwrap();
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    let res = send(pinned.to_string()).await.unwrap();
    assert_eq!(status_code(res).await, (400, "invalid_multipart".into()));
}
