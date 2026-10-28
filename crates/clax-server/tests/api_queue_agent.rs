//! A queue's triage routes (spec 2026-10-27-clax-sdk-design §7.3, §7.4):
//! members and their agents list, claim, renew, release, reply to, resolve
//! and annotate reports; an agent writes only under its own live lease.

use clax_core::audit::AuditCtx;
use clax_core::queue::{LEASE_SECS, ReportersSee, Role};
use clax_core::working::ManualClock;
use clax_store::queues::{NewQueue, NewReport};
use clax_testkit::{FAKE_PNG, TestServer, element_anchor};
use serde_json::{Value, json};
use std::sync::Arc;

struct Q {
    ts: TestServer,
    clock: Arc<ManualClock>,
    qid: String,
    tid: String,
    aid: String,
}

async fn setup() -> Q {
    let clock = Arc::new(ManualClock::at("2026-10-27T12:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let qid = ts
        .store
        .create_queue(
            &AuditCtx::DAEMON,
            None,
            NewQueue {
                name: "App reports".into(),
                origins: vec!["https://app.example.com".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id;
    let r = ts
        .store
        .file_report_async(
            &AuditCtx::DAEMON,
            &qid,
            Some("emp-1"),
            NewReport {
                external_id: Some("invoice:4821".into()),
                url: "https://app.example.com/invoices/4821".into(),
                title: "Invoice 4821".into(),
                anchor: serde_json::from_value(element_anchor()).unwrap(),
                body: "The total is wrong".into(),
                author_name: "emp-1".into(),
                metadata: json!({"build": "1"}),
                revision: None,
                clip: Some(FAKE_PNG.to_vec()),
            },
        )
        .await
        .unwrap();
    Q {
        ts,
        clock,
        qid,
        tid: r.id,
        aid: r.artifact.id,
    }
}

impl Q {
    async fn session(&self, hsid: &str) -> String {
        self.ts.register_session("claude", hsid).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn report(&self, tail: &str) -> String {
        format!("/api/queues/{}/reports/{}{tail}", self.qid, self.tid)
    }

    /// The owner's agent of session `sid` (none: the channel marker alone).
    async fn agent(
        &self,
        sid: Option<&str>,
        method: &str,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut b = self.ts.authed(
            self.ts
                .client
                .request(method.parse().unwrap(), format!("{}{path}", self.ts.base)),
        );
        b = match sid {
            Some(s) => b.header("x-clax-session", s),
            None => b.header("x-clax-channel", "agent"),
        };
        let r = b.json(&body).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// A browser with `cookie`.
    async fn browser(&self, cookie: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .ts
            .client
            .request(method.parse().unwrap(), format!("{}{path}", self.ts.base))
            .header("cookie", format!("clax_viewer={cookie}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn two_sessions_race_a_claim_and_one_wins() {
    let q = setup().await;
    let (a, b) = (q.session("a").await, q.session("b").await);
    let claim = q.report("/claim");
    let (ra, rb) = tokio::join!(
        q.agent(Some(&a), "POST", &claim, json!({})),
        q.agent(Some(&b), "POST", &claim, json!({})),
    );
    let mut codes = [ra.0, rb.0];
    codes.sort();
    assert_eq!(codes, [200, 409], "{ra:?} {rb:?}");
    let lost = if ra.0 == 409 { &ra.1 } else { &rb.1 };
    assert_eq!(code(lost), "thread_taken", "{lost}");
    let won = if ra.0 == 200 { &ra.1 } else { &rb.1 };
    assert_eq!(won["fence"], 1);
    assert_eq!(won["report"]["state"], "claimed");
    let (s, v) = q.agent(Some(&a), "GET", &q.report(""), json!(null)).await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("claimed")));
}

#[tokio::test]
async fn an_expired_lease_is_reclaimed_and_the_old_holders_reply_is_lease_lost() {
    let q = setup().await;
    let (a, b) = (q.session("a").await, q.session("b").await);
    let (s, _) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!(s, 200);
    let (s, v) = q
        .agent(
            Some(&a),
            "POST",
            &q.report("/comments"),
            json!({"body": "on it"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["comment"]["author_kind"], "agent");

    q.clock.advance(LEASE_SECS + 1);
    let (s, v) = q
        .agent(Some(&b), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!((s, v["fence"].as_u64()), (200, Some(2)), "{v}");
    for (path, body) in [
        ("/comments", json!({"body": "late"})),
        ("/resolve", json!({})),
        ("/renew", json!({})),
        ("", json!({"metadata": {"x": 1}, "if_version": 1})),
    ] {
        let method = if path.is_empty() { "PATCH" } else { "POST" };
        let (s, v) = q.agent(Some(&a), method, &q.report(path), body).await;
        assert_eq!((s, code(&v)), (409, "lease_lost"), "{path}: {v}");
    }
    let (_, v) = q.agent(Some(&b), "GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["comments"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["report"]["status"], "open");
    let (s, v) = q
        .agent(Some(&b), "POST", &q.report("/resolve"), json!({}))
        .await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("resolved")), "{v}");
}

#[tokio::test]
async fn an_agent_that_never_claimed_or_released_is_claim_required() {
    let q = setup().await;
    let a = q.session("a").await;
    let writes = [
        ("POST", "/comments", json!({"body": "hi"})),
        ("POST", "/resolve", json!({})),
        ("PATCH", "", json!({"metadata": {"x": 1}, "if_version": 1})),
        (
            "PATCH",
            "/artifact",
            json!({"metadata": {"x": 1}, "if_version": 1}),
        ),
    ];
    for (m, path, body) in &writes {
        let (s, v) = q.agent(Some(&a), m, &q.report(path), body.clone()).await;
        assert_eq!((s, code(&v)), (409, "claim_required"), "{path}: {v}");
    }
    q.agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/release"), json!({}))
        .await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("open")), "{v}");
    let (s, v) = q
        .agent(
            Some(&a),
            "POST",
            &q.report("/comments"),
            json!({"body": "hi"}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "claim_required"), "{v}");
    let (_, v) = q.agent(Some(&a), "GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["comments"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_agent_without_a_session_is_no_session() {
    let q = setup().await;
    for (m, path, body) in [
        ("POST", "/claim", json!({})),
        ("POST", "/renew", json!({})),
        ("POST", "/release", json!({})),
        ("POST", "/comments", json!({"body": "hi"})),
        ("POST", "/resolve", json!({})),
    ] {
        let (s, v) = q.agent(None, m, &q.report(path), body).await;
        assert_eq!((s, code(&v)), (400, "no_session"), "{path}: {v}");
    }
}

#[tokio::test]
async fn a_person_replies_and_resolves_without_a_claim() {
    let q = setup().await;
    let ana = q.ts.person("Ana").await;
    q.ts.store
        .set_member(
            &AuditCtx::DAEMON,
            &q.qid,
            &ana.public_id,
            Some(Role::Triage),
        )
        .unwrap();
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/comments"),
            json!({"body": "Looking"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["comment"]["author_kind"], "viewer");
    assert_eq!(v["comment"]["author_name"], "Ana");
    let (s, v) = q
        .browser(&ana.cookie, "POST", &q.report("/resolve"), json!({}))
        .await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("resolved")), "{v}");
    let (s, v) = q
        .browser(&ana.cookie, "GET", &q.report("/clip"), json!(null))
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn working_on_a_claimed_report_renews_its_lease() {
    let q = setup().await;
    let (a, b) = (q.session("a").await, q.session("b").await);
    q.agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    q.clock.advance(LEASE_SECS - 100);
    let (s, v) = q
        .agent(
            Some(&a),
            "PUT",
            &format!("/api/sessions/{a}/working/{}", q.aid),
            json!({"thread_ids": [q.tid]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    q.clock.advance(200);
    let (s, v) = q
        .agent(Some(&b), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (409, "thread_taken"), "{v}");
    let (s, v) = q
        .agent(
            Some(&a),
            "POST",
            &q.report("/comments"),
            json!({"body": "still here"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn a_non_member_is_not_a_member() {
    let q = setup().await;
    let bob = q.ts.person("Bob").await;
    for path in [format!("/api/queues/{}/reports", q.qid), q.report("")] {
        let (s, v) = q.browser(&bob.cookie, "GET", &path, json!(null)).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{path}: {v}");
    }
    let (s, v) = q
        .browser(
            &bob.cookie,
            "POST",
            &q.report("/comments"),
            json!({"body": "x"}),
        )
        .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    let a = q.session("a").await;
    let (s, v) = q
        .agent(
            Some(&a),
            "GET",
            "/api/queues/01J0000000000000000000000A/reports",
            json!(null),
        )
        .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
}

#[tokio::test]
async fn the_owners_agent_lists_reports_by_state() {
    let q = setup().await;
    let a = q.session("a").await;
    let list = |state: &str| format!("/api/queues/{}/reports?state={state}", q.qid);
    let (s, v) = q.agent(Some(&a), "GET", &list("open"), json!(null)).await;
    assert_eq!(
        (s, v["reports"].as_array().map(Vec::len)),
        (200, Some(1)),
        "{v}"
    );
    q.agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    let (_, v) = q.agent(Some(&a), "GET", &list("open"), json!(null)).await;
    assert_eq!(v["reports"].as_array().map(Vec::len), Some(0));
    let (_, v) = q
        .agent(Some(&a), "GET", &list("claimed"), json!(null))
        .await;
    assert_eq!(v["reports"][0]["id"], json!(q.tid));
    let (s, v) = q.agent(Some(&a), "GET", &list("bogus"), json!(null)).await;
    assert_eq!((s, code(&v)), (400, "invalid_state"), "{v}");
}

#[tokio::test]
async fn a_stale_if_version_is_a_conflict() {
    let q = setup().await;
    let a = q.session("a").await;
    q.agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    let body = json!({"metadata": {"ticket": "T-1"}, "if_version": 1});
    let (s, v) = q
        .agent(Some(&a), "PATCH", &q.report(""), body.clone())
        .await;
    assert_eq!((s, v["metadata_version"].as_u64()), (200, Some(2)), "{v}");
    assert_eq!(v["report"]["metadata"], json!({"ticket": "T-1"}));
    let stale = json!({"metadata": {"ticket": "T-2"}, "if_version": 1});
    let (s, v) = q
        .agent(Some(&a), "PATCH", &q.report(""), stale.clone())
        .await;
    assert_eq!((s, code(&v)), (409, "conflict"), "{v}");
    let (s, v) = q
        .agent(Some(&a), "PATCH", &q.report("/artifact"), body)
        .await;
    assert_eq!((s, v["metadata_version"].as_u64()), (200, Some(2)), "{v}");
    let (s, v) = q
        .agent(Some(&a), "PATCH", &q.report("/artifact"), stale)
        .await;
    assert_eq!((s, code(&v)), (409, "conflict"), "{v}");
}
