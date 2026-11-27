//! A token's queue rights (`clax-queues`): zero by default, granted per
//! token and per queue by anyone who holds them (`PUT
//! /api/people/grants/<gid>/elevate`), each action checked in one place
//! (`queue_agent::queue_right`), never beyond the token's person's
//! membership or its granter's, and revisited when its proof goes: the
//! next request is refused and the claim and watch it held end.

use clax_core::audit::AuditCtx;
use clax_core::queue::{ReportersSee, Role, State};
use clax_store::queues::{ExternalScope, NewQueue, NewReport, ReporterToken, Visibility};
use clax_testkit::{TestClient, TestServer, element_anchor};
use serde_json::{Value, json};

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// The owner's queue `q1` with one report, Sam and Mia connected and
/// members of it, Kim connected and not, and a session of Sam's agent.
struct R {
    ts: TestServer,
    q1: String,
    tid: String,
    sam: TestClient,
    sam_gid: String,
    sam_public: String,
    sam_cookie: String,
    mia_public: String,
    mia_cookie: String,
    kim_cookie: String,
    sid: String,
}

async fn queue(ts: &TestServer, name: &str) -> String {
    ts.store
        .create_queue(
            &AuditCtx::DAEMON,
            None,
            NewQueue {
                name: name.into(),
                origins: vec!["https://app.example.com".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
}

async fn file(ts: &TestServer, qid: &str, title: &str) -> String {
    let token = ReporterToken {
        queue_id: qid.to_string(),
        service_grant_id: "g1".into(),
        reporter_id: "emp-1".into(),
        origin: "https://app.example.com".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: None,
    };
    let (r, _) = ts
        .store
        .file_report_touched_async(
            &AuditCtx::DAEMON,
            qid,
            Some(&token),
            ExternalScope::Instance,
            NewReport {
                external_id: None,
                url: "https://app.example.com/invoices/1".into(),
                title: title.into(),
                anchor: serde_json::from_value(element_anchor()).unwrap(),
                body: "The total is wrong".into(),
                author_name: "emp-1".into(),
                metadata: json!({}),
                revision: None,
                clip: None,
                context: None,
                server: None,
                diagnostics: None,
            },
        )
        .await
        .unwrap();
    r.id
}

async fn setup() -> R {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let q1 = queue(&ts, "App reports").await;
    let tid = file(&ts, &q1, "Invoice").await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let kim = ts.connect_person("Kim", "clax:collaborator").await;
    let sv = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let mv = ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let kv = ts.store.viewer_of_grant(&kim.grant_id()).unwrap();
    for p in [&sv.public_id, &mv.public_id] {
        ts.store
            .set_member(&AuditCtx::DAEMON, &q1, p, Some(Role::Triage))
            .unwrap();
    }
    let sid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    R {
        q1,
        tid,
        sam_gid: sam.grant_id(),
        sam,
        sam_public: sv.public_id,
        sam_cookie: format!("clax_viewer={}", ts.browser_cookie(&sv.id)),
        mia_public: mv.public_id,
        mia_cookie: format!("clax_viewer={}", ts.browser_cookie(&mv.id)),
        kim_cookie: format!("clax_viewer={}", ts.browser_cookie(&kv.id)),
        sid,
        ts,
    }
}

fn access(actions: &[&str], queues: &[&str]) -> Value {
    json!([{"type": "clax-queues", "actions": actions, "locations": queues}])
}

impl R {
    fn report(&self, tail: &str) -> String {
        format!("/api/queues/{}/reports/{}{tail}", self.q1, self.tid)
    }

    /// Sam's agent, from its session.
    async fn agent(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let r = self.sam.agent(m, path, Some(&self.sid), body).await;
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// A browser with `cookie` (`None`: the owner's).
    async fn browser(
        &self,
        cookie: Option<&str>,
        m: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let owner = self.ts.owner_cookie();
        let mut r = self
            .ts
            .client
            .request(m.parse().unwrap(), format!("{}{path}", self.ts.base))
            .header("cookie", cookie.unwrap_or(&owner));
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// `who` grants Sam's token `access` (`null` removes theirs).
    async fn grant(&self, who: Option<&str>, access: Value) -> (u16, Value) {
        let path = format!("/api/people/grants/{}/elevate", self.sam_gid);
        if access.is_null() {
            self.browser(who, "DELETE", &path, None).await
        } else {
            self.browser(who, "PUT", &path, Some(json!({"access": access})))
                .await
        }
    }

    fn state(&self) -> State {
        self.ts
            .store
            .report(&self.q1, &self.tid, Visibility::All)
            .unwrap()
            .unwrap()
            .state
    }

    fn watched(&self) -> bool {
        self.ts
            .store
            .queue_watchers(&self.q1)
            .unwrap()
            .contains(&self.sid)
    }
}

/// Default deny, then each action's own gate: a token with nothing sees no
/// queue; with `read` it reads and watches and each other action is 403
/// `queue_not_granted` naming it, until that action is granted.
#[tokio::test]
async fn a_token_starts_with_no_queue_access_and_each_action_has_its_gate() {
    let r = setup().await;
    let list = format!("/api/queues/{}/reports", r.q1);
    // Default deny: a member's token sees nothing.
    let (s, v) = r.agent("GET", "/api/queues", None).await;
    assert_eq!((s, v["queues"].clone()), (200, json!([])), "{v}");
    for (m, p) in [
        ("GET", list.clone()),
        ("GET", r.report("")),
        ("POST", r.report("/claim")),
    ] {
        let (s, v) = r.agent(m, &p, Some(json!({}))).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{p}: {v}");
    }
    let (s, v) = r
        .agent("POST", &format!("/api/queues/{}/watch", r.q1), None)
        .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // Sam grants his own token read.
    let (s, v) = r
        .grant(Some(&r.sam_cookie), access(&["read"], &[&r.q1]))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grant"]["queues"][0]["actions"], json!(["read"]), "{v}");
    let (s, v) = r.agent("GET", &list, None).await;
    assert_eq!(
        (s, v["reports"].as_array().map(Vec::len)),
        (200, Some(1)),
        "{v}"
    );
    let (s, _) = r
        .agent("GET", &format!("{}/diagnostics", r.report("")), None)
        .await;
    assert_eq!(s, 200);
    let (s, v) = r
        .agent("POST", &format!("/api/queues/{}/watch", r.q1), None)
        .await;
    assert_eq!(s, 204, "{v}");
    for (action, m, p, body) in [
        ("claim", "POST", r.report("/claim"), json!({})),
        (
            "reply",
            "POST",
            r.report("/comments"),
            json!({"body": "Looking"}),
        ),
        ("fix", "POST", r.report("/fix"), json!({"summary": "Fixed"})),
        ("resolve", "POST", r.report("/resolve"), json!({})),
    ] {
        let (s, v) = r.agent(m, &p, Some(body)).await;
        assert_eq!((s, code(&v)), (403, "queue_not_granted"), "{action}: {v}");
        assert_eq!(
            v["error"]["right"],
            json!({"type": "clax-queues", "action": action}),
            "{v}"
        );
    }
    // Claim, then reply, then resolve, each once granted.
    r.grant(Some(&r.sam_cookie), access(&["claim"], &[&r.q1]))
        .await;
    let (s, v) = r.agent("POST", &r.report("/claim"), None).await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("claimed")), "{v}");
    let (s, v) = r
        .agent(
            "POST",
            &r.report("/comments"),
            Some(json!({"body": "Looking"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "queue_not_granted"), "{v}");
    r.grant(
        Some(&r.sam_cookie),
        access(&["claim", "reply", "resolve"], &[&r.q1]),
    )
    .await;
    let (s, v) = r
        .agent(
            "POST",
            &r.report("/comments"),
            Some(json!({"body": "Looking"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = r
        .agent("POST", &r.report("/fix"), Some(json!({"summary": "Fixed"})))
        .await;
    assert_eq!((s, code(&v)), (403, "queue_not_granted"), "{v}");
    let (s, v) = r.agent("POST", &r.report("/resolve"), None).await;
    assert_eq!((s, &v["report"]["state"]), (200, &json!("resolved")), "{v}");
    // Fix granted: the gate passes, and the report's state answers.
    r.grant(Some(&r.sam_cookie), access(&["fix"], &[&r.q1]))
        .await;
    let (s, v) = r
        .agent("POST", &r.report("/fix"), Some(json!({"summary": "Fixed"})))
        .await;
    assert_eq!(s, 409, "{v}");
    assert_ne!(code(&v), "queue_not_granted", "{v}");
    // Each change is in the audit journal, by its granter.
    let db = rusqlite::Connection::open(r.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let bodies: Vec<Value> = db
        .prepare("SELECT body FROM audit_events WHERE kind = 'grant.elevate' ORDER BY seq")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|b| serde_json::from_str(&b.unwrap()).unwrap())
        .collect();
    assert_eq!(bodies.len(), 4, "{bodies:?}");
    assert_eq!(bodies[0]["grant_id"], json!(r.sam_gid));
    assert_eq!(bodies[0]["granter"], json!(r.sam_public));
    assert_eq!(bodies[3]["access"][0]["actions"], json!(["fix", "read"]));
}

/// `queue_list` lists only the queues the token may read; `*` is every
/// queue the granter holds, and never one its person is not a member of,
/// whoever grants it.
#[tokio::test]
async fn queue_list_filters_by_read_and_a_grant_never_passes_membership() {
    let r = setup().await;
    let q2 = queue(&r.ts, "Second").await;
    let q3 = queue(&r.ts, "Not Sam's").await;
    r.ts.store
        .set_member(&AuditCtx::DAEMON, &q2, &r.sam_public, Some(Role::Triage))
        .unwrap();
    let ids = |v: &Value| -> Vec<String> {
        v["queues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap().to_string())
            .collect()
    };
    r.grant(Some(&r.sam_cookie), access(&["read"], &[&r.q1]))
        .await;
    let (_, v) = r.agent("GET", "/api/queues", None).await;
    assert_eq!(ids(&v), std::slice::from_ref(&r.q1));
    // Every queue Sam holds: q1 and q2, not q3.
    r.grant(Some(&r.sam_cookie), access(&["read"], &["*"]))
        .await;
    let (_, v) = r.agent("GET", "/api/queues", None).await;
    let mut got = ids(&v);
    got.sort();
    let mut want = vec![r.q1.clone(), q2.clone()];
    want.sort();
    assert_eq!(got, want);
    // Sam cannot name q3, which he does not hold.
    let (s, v) = r
        .grant(Some(&r.sam_cookie), access(&["read"], &[&q3]))
        .await;
    assert_eq!((s, code(&v)), (403, "beyond_your_rights"), "{v}");
    // The owner holds q3 and grants it to Sam's token: still not there,
    // as Sam is no member of it.
    let (s, v) = r.grant(None, access(&["read", "claim"], &[&q3])).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = r
        .agent("GET", &format!("/api/queues/{q3}/reports"), None)
        .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    // Other rights are not a token's grant.
    let (s, v) = r.grant(Some(&r.sam_cookie), json!(["clax:reader"])).await;
    assert_eq!((s, code(&v)), (400, "invalid_rights"), "{v}");
    // Nor is clax-queues a person's rights or a narrowing.
    let (s, v) = r
        .browser(
            Some(&r.sam_cookie),
            "PUT",
            &format!("/api/people/me/grants/{}/narrow", r.sam_gid),
            Some(json!({"access": access(&["read"], &[&r.q1])})),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_rights"), "{v}");
}

/// Another member grants Sam's token what they hold; a person who is not
/// a member cannot; the granter reads back their own grant alone; Sam
/// removes any granter's; two granters' grants unite, and revoking one
/// leaves what the other proves.
#[tokio::test]
async fn anyone_holding_a_right_grants_it_to_any_token_and_grants_unite() {
    let r = setup().await;
    let (s, v) = r
        .grant(Some(&r.kim_cookie), access(&["read"], &[&r.q1]))
        .await;
    assert_eq!((s, code(&v)), (403, "beyond_your_rights"), "{v}");
    let (s, v) = r
        .grant(Some(&r.mia_cookie), access(&["claim"], &[&r.q1]))
        .await;
    assert_eq!(s, 200, "{v}");
    let raises = v["grant"]["raises"].as_array().unwrap();
    assert_eq!(raises.len(), 1, "{v}");
    assert_eq!(raises[0]["granter"]["public_id"], json!(r.mia_public));
    assert!(
        v["grant"].get("queues").is_none(),
        "Mia does not read the token's rights: {v}"
    );
    // Sam's agent claims under Mia's grant.
    let (s, v) = r.agent("POST", &r.report("/claim"), None).await;
    assert_eq!(s, 200, "{v}");
    // The owner grants claim too: two grants that overlap.
    r.grant(None, access(&["claim"], &["*"])).await;
    // Mia's own removal leaves the owner's.
    let (s, v) = r.grant(Some(&r.mia_cookie), Value::Null).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(r.state(), State::Claimed);
    // Kim may not remove the owner's; Sam may, and then nothing is left.
    let path = format!("/api/people/grants/{}/elevate?granter=owner", r.sam_gid);
    let (s, v) = r.browser(Some(&r.kim_cookie), "DELETE", &path, None).await;
    assert_eq!((s, code(&v)), (404, "not_raised"), "{v}");
    let (s, v) = r.browser(Some(&r.sam_cookie), "DELETE", &path, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grant"]["raises"], json!([]), "{v}");
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    assert_eq!(r.state(), State::Open, "the claim ended with its right");
}

/// Grants Sam's token claim from Mia; Sam's agent claims and watches.
async fn claimed_under_mias_grant(r: &R) {
    let (s, v) = r
        .grant(Some(&r.mia_cookie), access(&["claim"], &[&r.q1]))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = r.agent("POST", &r.report("/claim"), None).await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = r
        .agent("POST", &format!("/api/queues/{}/watch", r.q1), None)
        .await;
    assert_eq!(s, 204);
    assert!(r.watched());
    assert_eq!(r.state(), State::Claimed);
}

/// The proof of a grant gone, in each of its ways: the next request is
/// refused, and the claim and the watch held under it end.
#[tokio::test]
async fn a_capability_is_revisited_when_its_proof_is_revoked() {
    // The granter revokes their grant.
    let r = setup().await;
    claimed_under_mias_grant(&r).await;
    r.grant(Some(&r.mia_cookie), Value::Null).await;
    assert_eq!(r.state(), State::Open);
    assert!(!r.watched());
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // The granter leaves the queue: what she gave goes with her right.
    let r = setup().await;
    claimed_under_mias_grant(&r).await;
    r.ts.store
        .set_member(&AuditCtx::DAEMON, &r.q1, &r.mia_public, None)
        .unwrap();
    assert_eq!(r.state(), State::Open);
    assert!(!r.watched());
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    // Back in the queue, her grant proves itself again: a new claim.
    r.ts.store
        .set_member(&AuditCtx::DAEMON, &r.q1, &r.mia_public, Some(Role::Triage))
        .unwrap();
    let (s, v) = r.agent("POST", &r.report("/claim"), None).await;
    assert_eq!(s, 200, "{v}");

    // The granter is revoked as a person.
    let r = setup().await;
    claimed_under_mias_grant(&r).await;
    r.ts.store
        .revoke_person(&AuditCtx::DAEMON, &r.mia_public)
        .unwrap();
    assert_eq!(r.state(), State::Open);
    assert!(!r.watched());
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // The token itself is revoked.
    let r = setup().await;
    claimed_under_mias_grant(&r).await;
    let (s, v) = r
        .browser(None, "DELETE", &format!("/api/grants/{}", r.sam_gid), None)
        .await;
    assert!(s < 300, "{s} {v}");
    assert_eq!(r.state(), State::Open);
    assert!(!r.watched());
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (401, "grant_revoked"), "{v}");

    // Narrowed to read alone, the claim ends but the watch stays.
    let r = setup().await;
    claimed_under_mias_grant(&r).await;
    r.grant(Some(&r.mia_cookie), access(&["read"], &[&r.q1]))
        .await;
    assert_eq!(r.state(), State::Open);
    assert!(r.watched());
    let (s, v) = r.agent("POST", &r.report("/renew"), None).await;
    assert_eq!((s, code(&v)), (403, "queue_not_granted"), "{v}");
}
