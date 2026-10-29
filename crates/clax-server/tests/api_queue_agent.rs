//! A queue's triage routes (spec 2026-10-27-clax-sdk-design §7.3, §7.4):
//! members and their agents list, claim, renew, release, reply to, resolve
//! and annotate reports; an agent writes only under its own live lease.

use clax_core::audit::AuditCtx;
use clax_core::queue::{LEASE_SECS, ReportersSee, Role};
use clax_core::working::ManualClock;
use clax_store::queues::{ExternalScope, NewQueue, NewReport, ReporterToken};
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
    setup_on(std::net::Ipv4Addr::LOCALHOST.into()).await
}

/// [`setup`] on a daemon bound to `bind` (every interface, to invite people).
async fn setup_on(bind: std::net::IpAddr) -> Q {
    let clock = Arc::new(ManualClock::at("2026-10-27T12:00:00Z"));
    let ts = TestServer::spawn_on_with_clock(bind, clock.clone()).await;
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
    let token = ReporterToken {
        queue_id: qid.clone(),
        service_grant_id: "g1".into(),
        reporter_id: "emp-1".into(),
        origin: "https://app.example.com".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: None,
    };
    let r = ts
        .store
        .file_report_async(
            &AuditCtx::DAEMON,
            &qid,
            Some(&token),
            ExternalScope::Instance,
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
                context: None,
                server: None,
                diagnostics: None,
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
    for bad in ["bogus", "Open", "active,open"] {
        let (s, v) = q.agent(Some(&a), "GET", &list(bad), json!(null)).await;
        assert_eq!((s, code(&v)), (400, "invalid_state"), "{bad}: {v}");
    }
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

#[tokio::test]
async fn the_generic_thread_routes_refuse_a_report_and_leave_it_out_of_listings() {
    let q = setup().await;
    let a = q.session("a").await;
    let thread = |tail: &str| format!("/api/artifacts/{}/threads/{}{tail}", q.aid, q.tid);
    for (m, path, body) in [
        (
            "POST",
            thread("/comments"),
            json!({"body": "on it", "author_kind": "agent"}),
        ),
        ("POST", thread("/resolve"), json!({"as": "agent"})),
        ("GET", thread(""), json!(null)),
        ("GET", thread("/clip"), json!(null)),
        ("DELETE", thread("?as=agent"), json!(null)),
    ] {
        let (s, v) = q.agent(Some(&a), m, &path, body).await;
        assert_eq!((s, code(&v)), (409, "queue_report"), "{m} {path}: {v}");
        assert_eq!(v["error"]["queue_id"], json!(q.qid), "{v}");
        assert_eq!(
            v["error"]["message"],
            "this thread is a queue report: use the queue routes (queue_reply, queue_resolve)"
        );
    }
    // Claimed, it is still the queue's alone.
    let (s, _) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!(s, 200);
    let (s, v) = q
        .agent(
            Some(&a),
            "POST",
            &thread("/comments"),
            json!({"body": "on it", "author_kind": "agent"}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "queue_report"), "{v}");
    let (_, v) = q.agent(Some(&a), "GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["comments"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["report"]["status"], "open");

    let (s, v) = q
        .agent(
            Some(&a),
            "GET",
            &format!("/api/artifacts/{}/threads?include_resolved=true", q.aid),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["threads"], json!([]), "{v}");
    let (s, v) = q
        .agent(
            Some(&a),
            "GET",
            "/api/threads?include_resolved=true",
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        !v.to_string().contains(&q.tid),
        "the report is in no listing: {v}"
    );
}

#[tokio::test]
async fn a_persons_agent_triages_as_its_person_and_loses_its_lease_with_its_membership() {
    let q = setup_on("0.0.0.0".parse().unwrap()).await;
    let sam = q.ts.connect_person("Sam", "clax:collaborator").await;
    let public_id =
        q.ts.store
            .viewer_of_grant(&sam.grant_id())
            .unwrap()
            .public_id;
    let sid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let call = |m: &'static str, path: String, body: Option<Value>| {
        let sam = &sam;
        let sid = sid.clone();
        async move {
            let r = sam.agent(m, &path, Some(&sid), body).await;
            let s = r.status().as_u16();
            (s, r.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    // Not a member: the queue is not there.
    let (s, v) = call("GET", format!("/api/queues/{}/reports", q.qid), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    let (s, v) = call("POST", q.report("/claim"), Some(json!({}))).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // A triage member's agent claims and replies.
    q.ts.store
        .set_member(&AuditCtx::DAEMON, &q.qid, &public_id, Some(Role::Triage))
        .unwrap();
    let (s, v) = call("GET", format!("/api/queues/{}/reports", q.qid), None).await;
    assert_eq!(
        (s, v["reports"].as_array().map(Vec::len)),
        (200, Some(1)),
        "{v}"
    );
    let (s, v) = call("POST", q.report("/claim"), Some(json!({}))).await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("claimed")), "{v}");
    let (s, v) = call(
        "POST",
        q.report("/comments"),
        Some(json!({"body": "Looking"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");

    // Removed, it is a stranger again, and its lease is gone.
    q.ts.store
        .set_member(&AuditCtx::DAEMON, &q.qid, &public_id, None)
        .unwrap();
    for (m, path, body) in [
        ("GET", q.report(""), None),
        ("POST", q.report("/comments"), Some(json!({"body": "late"}))),
        ("POST", q.report("/renew"), Some(json!({}))),
    ] {
        let (s, v) = call(m, path.clone(), body).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{path}: {v}");
    }
    let r =
        q.ts.store
            .report(&q.qid, &q.tid, clax_store::queues::Visibility::All)
            .unwrap()
            .unwrap();
    assert_eq!(r.state, clax_core::queue::State::Open);
    assert!(q.ts.store.thread_owner(&q.tid).unwrap().is_none());
}

#[tokio::test]
async fn the_queue_tools_list_claim_reply_and_resolve_against_a_daemon() {
    let q = setup().await;
    let sid = q.session("mcp-1").await;
    let log = tempfile::tempdir().unwrap();
    let tools = clax_mcp::tools::ClaxTools::new(
        clax_mcp::client::DaemonClient::new(q.ts.base.clone(), q.ts.token.clone(), Some(sid)),
        q.ts.base.clone(),
        None,
        log.path().join("mcp.log"),
    );
    let call = |name: &'static str, args: Value| {
        let tools = tools.clone();
        async move {
            let r = tools
                .call_direct(
                    rmcp::model::CallToolRequestParams::new(name)
                        .with_arguments(args.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            let rmcp::model::CallToolResponse::Complete(r) = r else {
                panic!("{name} completes")
            };
            let r = serde_json::to_value(r).unwrap();
            assert_eq!(r["isError"], false, "{name}: {r}");
            let text = r["content"][0]["text"].as_str().unwrap().to_string();
            serde_json::from_str::<Value>(&text).unwrap()
        }
    };
    let v = call("queue_list", json!({})).await;
    assert_eq!(v["queues"][0]["id"], json!(q.qid), "{v}");
    let v = call("queue_list", json!({"queue": q.qid, "state": "open"})).await;
    assert_eq!(v["reports"][0]["id"], json!(q.tid), "{v}");
    let v = call("queue_claim", json!({"queue": q.qid, "report_id": q.tid})).await;
    assert_eq!(v["report"]["state"], "claimed", "{v}");
    let v = call(
        "queue_reply",
        json!({"queue": q.qid, "report_id": q.tid, "body": "Fixed in 4.2"}),
    )
    .await;
    assert_eq!(v["comment"]["body"], "Fixed in 4.2", "{v}");
    let v = call("queue_resolve", json!({"queue": q.qid, "report_id": q.tid})).await;
    assert_eq!(v["report"]["state"], "resolved", "{v}");
}

/// Files another report on queue `qid` as reporter `emp-1`, with
/// `diagnostics`.
async fn file_into(
    ts: &TestServer,
    qid: &str,
    diagnostics: Option<Vec<clax_core::report::DiagLine>>,
) -> clax_core::queue::Report {
    let token = ReporterToken {
        queue_id: qid.to_string(),
        service_grant_id: "g1".into(),
        reporter_id: "emp-1".into(),
        origin: "https://app.example.com".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: None,
    };
    ts.store
        .file_report_async(
            &AuditCtx::DAEMON,
            qid,
            Some(&token),
            ExternalScope::Instance,
            NewReport {
                external_id: None,
                url: "https://app.example.com/invoices/9".into(),
                title: "Invoice 9".into(),
                anchor: serde_json::from_value(element_anchor()).unwrap(),
                body: "Another".into(),
                author_name: "emp-1".into(),
                metadata: Value::Null,
                revision: None,
                clip: None,
                context: None,
                server: None,
                diagnostics,
            },
        )
        .await
        .unwrap()
}

impl Q {
    /// The owner by hand: the daemon token, no session, no agent marker.
    async fn owner(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .ts
            .authed(
                self.ts
                    .client
                    .request(method.parse().unwrap(), format!("{}{path}", self.ts.base)),
            )
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// A connected person `name`, a triage member of the queue.
    async fn triager(&self, name: &str) -> clax_testkit::TestViewer {
        let p = self.ts.person(name).await;
        self.ts
            .store
            .set_member(
                &AuditCtx::DAEMON,
                &self.qid,
                &p.public_id,
                Some(Role::Triage),
            )
            .unwrap();
        p
    }

    fn kinds(&self) -> Vec<(String, Value)> {
        self.ts
            .store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind.clone(), serde_json::from_str(&e.body).unwrap()))
            .collect()
    }
}

fn fix_body() -> Value {
    json!({"summary": "Rounded the total", "url": "https://github.com/acme/app/pull/12",
           "commit": "abc123", "release": "0.6.1", "verify_url": "https://app.example.com/invoices/4821",
           "verify_after": "release:0.6.1"})
}

#[tokio::test]
async fn fix_by_lease_holder_proposes_and_releases() {
    let q = setup().await;
    let a = q.session("a").await;
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let fence = v["fence"].as_u64().unwrap();
    let mut body = fix_body();
    body["fence"] = json!(fence);
    let (s, v) = q.agent(Some(&a), "POST", &q.report("/fix"), body).await;
    assert_eq!(s, 200, "{v}");
    let r = &v["report"];
    assert_eq!(r["state"], "fix_proposed", "{v}");
    assert_eq!(r["status"], "open");
    assert_eq!(r["claim"], Value::Null, "{v}");
    let fix = &r["fixes"][0];
    assert_eq!(
        (fix["n"].as_u64(), &fix["outcome"]),
        (Some(1), &json!("pending"))
    );
    assert_eq!(fix["url"], "https://github.com/acme/app/pull/12");
    assert_eq!(fix["verify_after"], "release:0.6.1");
    let last = r["comments"].as_array().unwrap().last().unwrap();
    assert_eq!(last["body"], "Rounded the total");
    assert_eq!(last["author_kind"], "agent");

    let kinds = q.kinds();
    let release = kinds
        .iter()
        .find(|(k, _)| k == "queue.release")
        .expect("a queue.release");
    assert_eq!(release.1["reason"], "fix_proposed", "{kinds:?}");
    let fixed = kinds
        .iter()
        .find(|(k, _)| k == "queue.fix")
        .expect("a queue.fix");
    assert_eq!(fixed.1["n"], 1);
    assert_eq!(fixed.1["commit"], "abc123");
    // Its lease is gone: it writes no more until the report is reopened.
    let (s, v) = q
        .agent(
            Some(&a),
            "POST",
            &q.report("/comments"),
            json!({"body": "x"}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "claim_required"), "{v}");
    // Nor claims it while the fix waits for its verdict.
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (409, "not_claimable"), "{v}");
}

#[tokio::test]
async fn fix_after_lease_lost_is_refused() {
    let q = setup().await;
    let (a, b) = (q.session("a").await, q.session("b").await);
    let (s, _) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!(s, 200);
    q.clock.advance(LEASE_SECS + 1);
    let (s, _) = q
        .agent(Some(&b), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!(s, 200);
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/fix"), fix_body())
        .await;
    assert_eq!((s, code(&v)), (409, "lease_lost"), "{v}");
    let (_, v) = q.agent(Some(&b), "GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["state"], "claimed", "{v}");
    assert!(v["report"].get("fixes").is_none(), "{v}");
    // Without a claim at all.
    let c = q.session("c").await;
    let (s, v) = q
        .agent(Some(&c), "POST", &q.report("/fix"), fix_body())
        .await;
    assert_eq!((s, code(&v)), (409, "thread_taken"), "{v}");
    let (s, v) = q.agent(None, "POST", &q.report("/fix"), fix_body()).await;
    assert_eq!((s, code(&v)), (400, "no_session"), "{v}");
}

#[tokio::test]
async fn fix_with_a_stale_fence_is_lease_lost() {
    let q = setup().await;
    let a = q.session("a").await;
    let (_, v) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    let first = v["fence"].as_u64().unwrap();
    q.agent(Some(&a), "POST", &q.report("/release"), json!({}))
        .await;
    let (_, v) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    let second = v["fence"].as_u64().unwrap();
    assert!(second > first, "{v}");
    let mut body = fix_body();
    body["fence"] = json!(first);
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/fix"), body.clone())
        .await;
    assert_eq!((s, code(&v)), (409, "lease_lost"), "{v}");
    let (_, v) = q.agent(Some(&a), "GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["state"], "claimed", "{v}");
    body["fence"] = json!(second);
    let (s, v) = q.agent(Some(&a), "POST", &q.report("/fix"), body).await;
    assert_eq!(
        (s, &v["report"]["state"]),
        (200, &json!("fix_proposed")),
        "{v}"
    );
}

#[tokio::test]
async fn claim_of_fix_proposed_is_not_claimable() {
    let q = setup().await;
    let ana = q.triager("Ana").await;
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/fix"),
            json!({"summary": " "}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_fix"), "{v}");
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/fix"),
            json!({"summary": "x", "bogus": 1}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_json"), "{v}");
    let (s, v) = q
        .browser(&ana.cookie, "POST", &q.report("/fix"), fix_body())
        .await;
    assert_eq!(
        (s, &v["report"]["state"]),
        (200, &json!("fix_proposed")),
        "{v}"
    );
    assert_eq!(v["report"]["fixes"][0]["proposer_name"], "Ana");
    let a = q.session("a").await;
    let (s, v) = q
        .agent(Some(&a), "POST", &q.report("/claim"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (409, "not_claimable"), "{v}");
    // A person's next fix supersedes the one waiting.
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/fix"),
            json!({"summary": "Second go"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let fixes = v["report"]["fixes"].as_array().unwrap();
    assert_eq!(fixes[0]["outcome"], "superseded", "{v}");
    assert_eq!(
        (fixes[1]["n"].as_u64(), &fixes[1]["outcome"]),
        (Some(2), &json!("pending"))
    );
}

#[tokio::test]
async fn agent_cannot_verify_or_reopen() {
    let q = setup().await;
    let ana = q.triager("Ana").await;
    q.browser(&ana.cookie, "POST", &q.report("/fix"), fix_body())
        .await;
    let a = q.session("a").await;
    for sid in [Some(a.as_str()), None] {
        for (path, body) in [
            ("/verify", json!({})),
            ("/reopen", json!({"body": "still broken"})),
        ] {
            let (s, v) = q.agent(sid, "POST", &q.report(path), body).await;
            assert_eq!((s, code(&v)), (403, "not_for_agents"), "{path}: {v}");
        }
    }
    let (_, v) = q.owner("GET", &q.report(""), json!(null)).await;
    assert_eq!(v["report"]["state"], "fix_proposed", "{v}");
    assert_eq!(v["report"]["fixes"][0]["outcome"], "pending", "{v}");
}

#[tokio::test]
async fn person_verifies_on_reporters_behalf() {
    let q = setup().await;
    let ana = q.triager("Ana").await;
    let bob = q.ts.person("Bob").await;
    let (s, v) = q
        .browser(&ana.cookie, "POST", &q.report("/verify"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (409, "not_fix_proposed"), "{v}");
    q.browser(&ana.cookie, "POST", &q.report("/fix"), fix_body())
        .await;
    let (s, v) = q
        .browser(&bob.cookie, "POST", &q.report("/verify"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/verify"),
            json!({"note": "Checked with emp-1 on a call"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let r = &v["report"];
    assert_eq!(r["state"], "verified", "{v}");
    assert_eq!(r["status"], "resolved");
    assert_eq!(r["resolve_reason"], "fixed");
    assert_eq!(r["fixes"][0]["outcome"], "verified");
    assert_eq!(
        r["fixes"][0]["verdict_note"],
        "Checked with emp-1 on a call"
    );
    let t = q.ts.store.get_thread(&q.tid).unwrap().unwrap();
    assert_eq!(
        t.resolved_by.as_deref(),
        Some(format!("viewer:{}", ana.public_id).as_str())
    );
    let (s, v) = q
        .browser(&ana.cookie, "POST", &q.report("/verify"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (409, "not_fix_proposed"), "{v}");

    // Still broken after all: reopened on the reporter's behalf.
    let (s, v) = q
        .browser(&ana.cookie, "POST", &q.report("/reopen"), json!({}))
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_json"), "{v}");
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/reopen"),
            json!({"body": "emp-1 sees it again"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let r = &v["report"];
    assert_eq!(r["state"], "open", "{v}");
    assert_eq!(r["reopen_count"], 1);
    assert!(r.get("resolve_reason").is_none(), "{v}");
    let last = r["comments"].as_array().unwrap().last().unwrap();
    assert_eq!(last["author_name"], "Ana");
    assert_eq!(last["capture"], 1);
    let kinds: Vec<String> = q.kinds().into_iter().map(|(k, _)| k).collect();
    for k in ["queue.fix", "queue.verify", "queue.reopen", "thread.reopen"] {
        assert!(kinds.iter().any(|x| x == k), "{k}: {kinds:?}");
    }
}

#[tokio::test]
async fn list_active_filters() {
    let q = setup().await;
    let claimed = file_into(&q.ts, &q.qid, None).await.id;
    let proposed = file_into(&q.ts, &q.qid, None).await.id;
    let verified = file_into(&q.ts, &q.qid, None).await.id;
    let resolved = file_into(&q.ts, &q.qid, None).await.id;
    let a = q.session("a").await;
    let at = |tid: &str, tail: &str| format!("/api/queues/{}/reports/{tid}{tail}", q.qid);
    q.agent(Some(&a), "POST", &at(&claimed, "/claim"), json!({}))
        .await;
    for tid in [&proposed, &verified] {
        let (s, v) = q.owner("POST", &at(tid, "/fix"), fix_body()).await;
        assert_eq!(s, 200, "{v}");
    }
    let (s, v) = q.owner("POST", &at(&verified, "/verify"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = q
        .owner(
            "POST",
            &at(&resolved, "/resolve"),
            json!({"reason": "wontfix"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let list = |state: &str| format!("/api/queues/{}/reports?state={state}", q.qid);
    let ids = |v: &Value| -> Vec<String> {
        v["reports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };
    for (state, want) in [
        ("active", vec![q.tid.clone(), claimed.clone()]),
        ("open", vec![q.tid.clone()]),
        ("claimed", vec![claimed.clone()]),
        ("fix_proposed", vec![proposed.clone()]),
        ("verified", vec![verified.clone()]),
        ("resolved", vec![resolved.clone()]),
    ] {
        let (s, v) = q.agent(Some(&a), "GET", &list(state), json!(null)).await;
        assert_eq!(s, 200, "{state}: {v}");
        assert_eq!(ids(&v), want, "{state}: {v}");
    }
    let (_, v) = q
        .agent(
            Some(&a),
            "GET",
            &format!("/api/queues/{}/reports", q.qid),
            json!(null),
        )
        .await;
    assert_eq!(ids(&v).len(), 5, "{v}");
}

#[tokio::test]
async fn resolve_takes_a_reason() {
    let q = setup().await;
    let other = file_into(&q.ts, &q.qid, None).await.id;
    let ana = q.triager("Ana").await;
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/resolve"),
            json!({"reason": "bogus"}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &q.report("/resolve"),
            json!({"reason": "duplicate"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["report"]["state"], "resolved");
    assert_eq!(v["report"]["resolve_reason"], "duplicate", "{v}");
    let resolve = q
        .kinds()
        .into_iter()
        .find(|(k, _)| k == "thread.resolve")
        .expect("a thread.resolve");
    assert_eq!(resolve.1["reason"], "duplicate", "{resolve:?}");
    // No reason: other.
    let (s, v) = q
        .browser(
            &ana.cookie,
            "POST",
            &format!("/api/queues/{}/reports/{other}/resolve", q.qid),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["report"]["resolve_reason"], "other", "{v}");
}

#[tokio::test]
async fn diagnostics_of_another_queue_is_not_a_member() {
    let q = setup().await;
    let ana = q.triager("Ana").await;
    let diag = clax_core::report::parse_diagnostics(
        br#"{"at":"2026-10-27T11:59:00Z","level":"error","kind":"error","text":"boom"}"#,
    )
    .unwrap();
    let mine = file_into(&q.ts, &q.qid, Some(diag.clone())).await.id;
    let (s, v) = q
        .browser(
            &ana.cookie,
            "GET",
            &format!("/api/queues/{}/reports/{mine}/diagnostics", q.qid),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["lines"][0]["text"], "boom", "{v}");
    // The setup's report was filed with none.
    let (_, v) = q
        .browser(&ana.cookie, "GET", &q.report("/diagnostics"), json!(null))
        .await;
    assert_eq!(v["lines"], json!([]), "{v}");

    let q2 =
        q.ts.store
            .create_queue(
                &AuditCtx::DAEMON,
                None,
                NewQueue {
                    name: "Other".into(),
                    origins: vec!["https://app.example.com".into()],
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
    let theirs = file_into(&q.ts, &q2, Some(diag)).await.id;
    for path in [
        format!("/api/queues/{q2}/reports/{theirs}/diagnostics"),
        format!("/api/queues/{q2}/reports/{theirs}/comments/c1/clip"),
    ] {
        let (s, v) = q.browser(&ana.cookie, "GET", &path, json!(null)).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{path}: {v}");
    }
    // Another queue's report through Ana's own queue is none of it.
    let (s, v) = q
        .browser(
            &ana.cookie,
            "GET",
            &format!("/api/queues/{}/reports/{theirs}/diagnostics", q.qid),
            json!(null),
        )
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}
