//! People, rights, narrowing, elevation, ending one agent and the
//! `*unconnected` switch (spec 2026-10-08-multi-agent-multi-person §6.4,
//! §7.2–§7.4, §9.6, §14, M17, M20): every change is made in a browser on a
//! same-origin request, applies at the agent's next request, and no agent
//! changes its own rights.

use crate::common::TestServer;
use clax_core::audit::AuditCtx;
use clax_core::store::approvals::{Scope, Via};
use clax_server::testing::TestClient;
use serde_json::{Value, json};

/// `(status, body)` of a response.
async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// The owner, the owner's agent, and Sam and Mia connected
/// (`clax:collaborator`), each with an agent.
struct People {
    ts: TestServer,
    sam: TestClient,
    mia: TestClient,
    sam_cookie: String,
    sam_public: String,
    mia_cookie: String,
    owner_session: String,
    sam_session: String,
    mia_session: String,
}

async fn people() -> People {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let sv = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let mv = ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let owner_session = id(&ts.register_session("claude", "owner-h1").await);
    let sam_session = id(&sam.register("claude", "sam-h1").await);
    let mia_session = id(&mia.register("codex", "mia-h1").await);
    People {
        ts,
        sam,
        mia,
        sam_cookie: format!("clax_viewer={}", sv.id),
        sam_public: sv.public_id,
        mia_cookie: format!("clax_viewer={}", mv.id),
        owner_session,
        sam_session,
        mia_session,
    }
}

impl People {
    fn handle(&self, sid: &str) -> String {
        self.ts
            .store
            .get_session(sid)
            .unwrap()
            .unwrap()
            .agent_handle
    }

    /// A request with `cookie` (a browser), or the daemon token alone.
    async fn call(&self, who: Who<'_>, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let ts = &self.ts;
        let mut r = ts
            .client
            .request(m.parse().unwrap(), format!("{}{path}", ts.base));
        r = match who {
            Who::Browser(c) => r.header("cookie", c),
            Who::Token => ts.authed(r),
            Who::OwnerAgent => ts.authed(r).header("x-clax-session", &self.owner_session),
        };
        if let Some(b) = body {
            r = r.json(&b);
        }
        json_of(r.send().await.unwrap()).await
    }

    async fn owner(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let c = self.ts.owner_cookie();
        self.call(Who::Browser(&c), m, path, body).await
    }

    async fn sams_browser(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call(Who::Browser(&self.sam_cookie), m, path, body)
            .await
    }

    async fn sam_agent(&self, sid: &str, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        json_of(self.sam.agent(m, path, Some(sid), body).await).await
    }

    /// Sam's agent `sid` publishes a version of `aid` over `if_version`.
    async fn sam_publishes(&self, sid: &str, aid: &str, if_version: u32) -> (u16, Value) {
        self.sam_agent(
            sid,
            "POST",
            &format!("/api/artifacts/{aid}/versions"),
            Some(json!({"if_version": if_version, "files": {
                "index.html": {"content": format!("<p>v{}</p>", if_version + 1), "encoding": "utf8"}}})),
        )
        .await
    }

    /// The audit events of `kind`, their bodies, oldest first.
    fn audited(&self, kind: &str) -> Vec<Value> {
        let db = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        let mut stmt = db
            .prepare("SELECT body FROM audit_events WHERE kind = ?1 ORDER BY seq")
            .unwrap();
        stmt.query_map([kind], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|b| serde_json::from_str(&b.unwrap()).unwrap())
            .collect()
    }

    /// A session's stored narrowing and elevation.
    fn stored_rights(&self, sid: &str) -> (Option<String>, Option<String>) {
        let db = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db.query_row(
            "SELECT narrow_json, elevate_json FROM sessions WHERE id = ?1",
            [sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }
}

#[derive(Clone, Copy)]
enum Who<'a> {
    Browser(&'a str),
    Token,
    OwnerAgent,
}

/// Every change of who may do what, for Sam and his agent.
fn changes(p: &People) -> Vec<(&'static str, String, Option<Value>)> {
    let h = p.handle(&p.sam_session);
    let reader = json!("clax:reader");
    vec![
        (
            "PUT",
            format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": reader})),
        ),
        (
            "PUT",
            format!("/api/agents/{h}/elevate"),
            Some(json!({"access": "clax:full"})),
        ),
        ("DELETE", format!("/api/agents/{h}/elevate"), None),
        (
            "PUT",
            format!("/api/agents/{h}/narrow"),
            Some(json!({"access": reader})),
        ),
        (
            "PUT",
            format!("/api/people/me/grants/{}/narrow", p.sam.grant_id()),
            Some(json!({"access": reader})),
        ),
        ("DELETE", format!("/api/agents/{h}"), None),
        ("DELETE", format!("/api/people/{}", p.sam_public), None),
    ]
}

/// Nothing of Sam's changed: his rights, his grant, his agent.
fn nothing_changed(p: &People, gen_before: u64) {
    let st = &p.ts.store;
    assert_eq!(st.visibility_generation(), gen_before, "no bump");
    assert_eq!(
        st.person_rights(&p.sam_public).unwrap(),
        clax_core::rights::Rights::preset("clax:collaborator").unwrap()
    );
    assert_eq!(st.grant(&p.sam.grant_id()).unwrap().status, "approved");
    assert!(st.grant(&p.sam.grant_id()).unwrap().narrow.is_none());
    let s = st.get_session(&p.sam_session).unwrap().unwrap();
    assert!(s.ended_at.is_none());
    assert_eq!(p.stored_rights(&p.sam_session), (None, None));
}

#[tokio::test]
async fn rights_changes_need_a_browser() {
    let p = people().await;
    let before = p.ts.store.visibility_generation();
    for (m, path, body) in changes(&p) {
        // The daemon token alone: the CLI by hand, or an agent with shell
        // access calling curl.
        let (status, v) = p.call(Who::Token, m, &path, body.clone()).await;
        assert_eq!(
            (status, code(&v)),
            (403, "browser_required"),
            "token: {m} {path}: {v}"
        );
        // The owner's agent: the token naming its session.
        let (status, v) = p.call(Who::OwnerAgent, m, &path, body.clone()).await;
        assert_eq!(
            (status, code(&v)),
            (403, "not_for_agents"),
            "owner agent: {m} {path}: {v}"
        );
        // A page in the owner's browser (another origin) cannot either.
        let ts = &p.ts;
        let mut r = ts
            .client
            .request(m.parse().unwrap(), format!("{}{path}", ts.base))
            .header("cookie", ts.owner_cookie())
            .header("origin", "http://abcdefghijkm.localhost:7480");
        if let Some(b) = &body {
            r = r.json(b);
        }
        let (status, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!(
            (status, code(&v)),
            (403, "forbidden_origin"),
            "page: {m} {path}: {v}"
        );
    }
    nothing_changed(&p, before);
}

#[tokio::test]
async fn an_agent_cannot_change_its_own_rights() {
    let p = people().await;
    let before = p.ts.store.visibility_generation();
    for (m, path, body) in changes(&p) {
        let (status, v) = p.sam_agent(&p.sam_session, m, &path, body).await;
        assert_eq!(
            (status, code(&v)),
            (403, "forbidden"),
            "Sam's agent: {m} {path}: {v}"
        );
    }
    // Nor the owner's agent its own.
    let h = p.handle(&p.owner_session);
    for (m, path) in [
        ("PUT", format!("/api/agents/{h}/narrow")),
        ("PUT", format!("/api/agents/{h}/elevate")),
    ] {
        let (status, v) = p
            .call(
                Who::OwnerAgent,
                m,
                &path,
                Some(json!({"access": "clax:reader"})),
            )
            .await;
        assert_eq!((status, code(&v)), (403, "not_for_agents"), "{path}: {v}");
    }
    nothing_changed(&p, before);
}

#[tokio::test]
async fn elevation_exceeds_the_person_for_one_agent_only_until_its_session_ends() {
    let p = people().await;
    let a =
        p.ts.publish("Quarterly Review", &[("index.html", "<p>v1</p>")])
            .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    // The owner makes Sam a reader.
    let (status, v) = p
        .owner(
            "PUT",
            &format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["person"]["rights"],
        clax_core::rights::Rights::preset("clax:reader")
            .unwrap()
            .to_json()
    );
    let second = id(&p.sam.register("claude", "sam-h2").await);
    let (status, v) = p.sam_publishes(&p.sam_session, &aid, 1).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // The owner gives Sam's first agent publishing on this one artifact.
    let gen_before = p.ts.store.visibility_generation();
    let h = p.handle(&p.sam_session);
    let access = json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [aid]}]);
    let (status, v) = p
        .owner(
            "PUT",
            &format!("/api/agents/{h}/elevate"),
            Some(json!({"access": access})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["agent"]["elevate"], access, "{v}");
    assert!(v["agent"]["rights"].to_string().contains("update"), "{v}");
    assert!(p.ts.store.visibility_generation() > gen_before, "bumped");
    let rec = p.audited("agent.elevate");
    assert_eq!(rec.last().unwrap()["session"], json!(p.sam_session));
    assert_eq!(rec.last().unwrap()["access"], access);
    // That agent may now publish there; Sam's other agent may not.
    let (status, v) = p.sam_publishes(&p.sam_session, &aid, 1).await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = p.sam_publishes(&second, &aid, 2).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // Nor elsewhere: the elevation names one artifact.
    let b = p.ts.publish("Other", &[("index.html", "<p>v1</p>")]).await;
    let bid = b["artifact"]["id"].as_str().unwrap();
    let (status, _) = p.sam_publishes(&p.sam_session, bid, 1).await;
    assert_eq!(status, 403);
    // Removing it is the owner's too, and applies at the next request.
    let h2 = p.handle(&second);
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/agents/{h2}/elevate"),
            Some(json!({"access": access})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, v) = p.sam_publishes(&second, &aid, 2).await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = p
        .owner("DELETE", &format!("/api/agents/{h2}/elevate"), None)
        .await;
    assert_eq!((status, v["agent"]["elevate"].clone()), (200, Value::Null));
    let (status, _) = p.sam_publishes(&second, &aid, 3).await;
    assert_eq!(status, 403);
    // The first agent's session ends: its elevation goes with it, and the
    // same agent registering again starts from Sam's rights.
    let r = p
        .sam
        .agent(
            "PATCH",
            &format!("/api/sessions/{}", p.sam_session),
            Some(&p.sam_session),
            Some(json!({"ended": true})),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(p.stored_rights(&p.sam_session), (None, None));
    let again = id(&p.sam.register("claude", "sam-h1").await);
    assert_ne!(again, p.sam_session);
    let (status, _) = p.sam_publishes(&again, &aid, 3).await;
    assert_eq!(status, 403);
    // An agent of this machine is not elevated: the token is not limited.
    let ho = p.handle(&p.owner_session);
    let (status, v) = p
        .owner(
            "PUT",
            &format!("/api/agents/{ho}/elevate"),
            Some(json!({"access": access})),
        )
        .await;
    assert_eq!((status, code(&v)), (400, "token_agent"), "{v}");
}

#[tokio::test]
async fn narrowing_applies_at_the_next_request_and_cannot_widen() {
    let p = people().await;
    let r =
        p.ts.post_json(
            "/api/artifacts",
            json!({"title": "Notes", "capabilities": {"db": {}},
                   "files": {"index.html": {"content": "<p>v1</p>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let doc = format!("/api/artifacts/{aid}/docs/notes/one");
    let write = json!({"data": {"text": "hi"}});
    let (status, v) = p
        .sam_agent(&p.sam_session, "PUT", &doc, Some(write.clone()))
        .await;
    assert_eq!(status, 200, "{v}");
    let h = p.handle(&p.sam_session);
    // Wider than Sam's rights, for the agent or the whole machine: refused.
    for (path, what) in [
        (format!("/api/agents/{h}/narrow"), "agent"),
        (
            format!("/api/people/me/grants/{}/narrow", p.sam.grant_id()),
            "grant",
        ),
    ] {
        let (status, v) = p
            .sams_browser("PUT", &path, Some(json!({"access": "clax:full"})))
            .await;
        assert_eq!((status, code(&v)), (400, "not_narrower"), "{what}: {v}");
    }
    assert_eq!(p.stored_rights(&p.sam_session), (None, None));
    // Sam narrows his agent to reading: its next request is refused.
    let gen_before = p.ts.store.visibility_generation();
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(p.ts.store.visibility_generation() > gen_before, "bumped");
    assert_eq!(
        p.audited("agent.narrow").last().unwrap()["session"],
        json!(p.sam_session)
    );
    let (status, v) = p
        .sam_agent(&p.sam_session, "PUT", &doc, Some(write.clone()))
        .await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // Within the narrowing, still allowed: reading.
    let (status, _) = p.sam_agent(&p.sam_session, "GET", &doc, None).await;
    assert_eq!(status, 200);
    // Sam removes the narrowing: writing again.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{h}/narrow"),
            Some(json!({"access": null})),
        )
        .await;
    assert_eq!((status, v["agent"]["narrow"].clone()), (200, Value::Null));
    let (status, v) = p
        .sam_agent(
            &p.sam_session,
            "PUT",
            &format!("{doc}-2"),
            Some(write.clone()),
        )
        .await;
    assert_eq!(status, 200, "the narrowing removed: {v}");
    // The machine's narrowing reaches every agent of it.
    let second = id(&p.sam.register("claude", "sam-h2").await);
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/people/me/grants/{}/narrow", p.sam.grant_id()),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(p.audited("grant.narrow").len(), 1);
    for sid in [&p.sam_session, &second] {
        let (status, v) = p.sam_agent(sid, "PUT", &doc, Some(write.clone())).await;
        assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    }
    // Mia's agent is untouched.
    let (status, v) = json_of(
        p.mia
            .agent(
                "PUT",
                &format!("{doc}-3"),
                Some(&p.mia_session),
                Some(write.clone()),
            )
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
}

#[tokio::test]
async fn a_waiting_agent_takes_with_its_narrowed_rights() {
    let p = people().await;
    let a = p.ts.publish("Review", &[("index.html", "<p>v1</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let other = p.ts.publish("Other", &[("index.html", "<p>v1</p>")]).await;
    let other = other["artifact"]["id"].as_str().unwrap().to_string();
    let sid = p.sam_session.clone();
    let r = p
        .sam
        .agent(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    // Sam's agents may read the owner's words.
    p.ts.store
        .approve(
            &AuditCtx::DAEMON,
            Some(&p.sam_public),
            &p.ts.owner_public_id().await,
            Scope::Everywhere,
            Via::Connect,
        )
        .unwrap();
    // Sam's agent waits. The owner sends it a comment on the first
    // artifact, which wakes the wait; before its take, Sam narrows the agent
    // to the other artifact. The take then runs with the narrowed rights.
    let h = p.handle(&sid);
    let path = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
    let waiting = async { json_of(p.sam.agent("GET", &path, Some(&sid), None).await).await };
    let meanwhile = async {
        p.ts.wait_feedback_waiters(&sid, 1).await;
        p.ts.hold_feedback_at(&sid, true, "before_take").await;
        let t = p.ts.thread_as_owner(&aid, "please fix the chart").await;
        let tid = t["id"].as_str().unwrap().to_string();
        let (status, v) = p
            .owner(
                "POST",
                &format!("/api/artifacts/{aid}/threads/{tid}/send"),
                Some(json!({"to": h})),
            )
            .await;
        assert_eq!(status, 200, "{v}");
        p.ts.wait_held_feedback_polls(&sid, 1).await;
        let (status, v) = p
            .sams_browser(
                "PUT",
                &format!("/api/agents/{h}/narrow"),
                Some(json!({"access": [
                    {"type": "clax-artifacts", "actions": ["read"], "locations": [other]},
                    {"type": "clax-comments", "actions": ["read", "watch"], "locations": [other]}
                ]})),
            )
            .await;
        assert_eq!(status, 200, "{v}");
        p.ts.hold_feedback_at(&sid, false, "before_take").await;
        p.ts.wait_feedback_waiters(&sid, 1).await;
        p.ts.expire_feedback(&sid).await;
    };
    let ((status, v), ()) = tokio::join!(waiting, meanwhile);
    assert_eq!(status, 200, "{v}");
    assert!(!v.to_string().contains("please fix the chart"), "{v}");
    // Narrowing removed: its next take delivers it.
    let (status, _) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{h}/narrow"),
            Some(json!({"access": null})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, v) = p
        .sam_agent(
            &sid,
            "GET",
            &format!("/api/sessions/{sid}/feedback?tier=piggyback"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(v.to_string().contains("please fix the chart"), "{v}");
}

#[tokio::test]
async fn revoking_a_person_revokes_grants_and_invites_and_keeps_history() {
    let p = people().await;
    let a = p.ts.publish("Review", &[("index.html", "<p>v1</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    // Sam comments from his browser, and has an unused invite for a second
    // machine.
    let sam_viewer = p.sam_cookie.trim_start_matches("clax_viewer=").to_string();
    let t = p.ts.thread_as(&aid, &sam_viewer, "Sam's note").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let (status, v) = p
        .owner(
            "POST",
            "/api/people/invites",
            Some(json!({"viewer": p.sam_public})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let invite = v["invite"]["id"].as_str().unwrap().to_string();
    // Sam's agent works on the artifact and waits for feedback; the owner's
    // People listens.
    let sid = p.sam_session.clone();
    let r = p
        .sam
        .agent(
            "PUT",
            &format!("/api/sessions/{sid}/working/{aid}"),
            Some(&sid),
            Some(json!({"message": "drafting"})),
        )
        .await;
    assert_eq!(r.status(), 200);
    let working = || async {
        let (_, v) = p
            .owner("GET", &format!("/api/artifacts/{aid}/working"), None)
            .await;
        v["working"].as_array().map_or(0, Vec::len)
    };
    assert_eq!(working().await, 1);
    let mut people_topic = p.ts.stream_as_owner(&["people"]).await;
    let gen_before = p.ts.store.visibility_generation();
    let path = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
    let waiting = async { json_of(p.sam.agent("GET", &path, Some(&sid), None).await).await };
    let revoke = async {
        p.ts.wait_feedback_waiters(&sid, 1).await;
        p.owner("DELETE", &format!("/api/people/{}", p.sam_public), None)
            .await
    };
    // The revocation wakes the agent's wait: it returns, well before its
    // ten minutes.
    let (_, (status, v)) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(waiting, revoke)
    })
    .await
    .expect("the parked wait returns once its person is revoked");
    assert!(status == 200 || status == 401, "{status}: {v}");
    // Its working record ended, and People heard the grant end.
    assert_eq!(
        working().await,
        0,
        "the revoked agent's working record ended"
    );
    loop {
        let (name, ev) = people_topic.next().await;
        if name == "people" && ev["change"]["what"] == "grant_ended" {
            assert_eq!(ev["change"]["grant_id"], json!(p.sam.grant_id()), "{ev}");
            break;
        }
    }
    assert_eq!(v["ended_grants"], json!([p.sam.grant_id()]));
    assert_eq!(v["revoked_invites"], json!([invite]));
    assert!(v["person"]["revoked_at"].is_string(), "{v}");
    assert!(p.ts.store.visibility_generation() > gen_before, "bumped");
    assert_eq!(p.audited("person.revoke").len(), 1);
    // His grant, his session, his agent's next request.
    assert_eq!(
        p.ts.store.grant(&p.sam.grant_id()).unwrap().status,
        "revoked"
    );
    let s = p.ts.store.get_session(&p.sam_session).unwrap().unwrap();
    assert!(s.ended_at.is_some());
    let (status, v) = p
        .sam_agent(&p.sam_session, "GET", "/api/artifacts", None)
        .await;
    assert_eq!((status, code(&v)), (401, "grant_revoked"), "{v}");
    // His browser is a plain viewer's now.
    let (status, _) = p.sams_browser("GET", "/api/people/me", None).await;
    assert_eq!(status, 403);
    // History stays: his comment, his name, his row in People.
    let (_, v) = p
        .owner("GET", &format!("/api/artifacts/{aid}/threads/{tid}"), None)
        .await;
    let c = &v["thread"]["comments"][0];
    assert_eq!(
        (c["author_name"].clone(), c["author_public_id"].clone()),
        (json!("Sam"), json!(p.sam_public))
    );
    let (status, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(status, 200);
    let sam = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["public_id"] == json!(p.sam_public))
        .unwrap();
    assert!(sam["revoked_at"].is_string(), "{sam}");
    assert_eq!(sam["agents"], json!([]));
    // What his browser writes now stays his: withheld from the owner's
    // agents as Sam's, and never named as not connected (spec §6.4).
    let mine =
        p.ts.publish_as(&p.owner_session, "Mine", "<main><p>Mine</p></main>")
            .await;
    let mine = mine["artifact"]["id"].as_str().unwrap().to_string();
    let t = p.ts.thread_as(&mine, &sam_viewer, "Sam after").await;
    let later = t["id"].as_str().unwrap().to_string();
    let (status, v) = p
        .sams_browser(
            "POST",
            &format!("/api/artifacts/{mine}/threads/{later}/send"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let fb = owner_feedback(&p).await;
    let text = fb.to_string();
    assert!(!text.contains("Sam after"), "{fb}");
    assert!(text.contains("comments from Sam on"), "{fb}");
    assert!(!text.contains("(not connected)"), "{fb}");
    // Mia is untouched.
    assert_eq!(
        p.ts.store.grant(&p.mia.grant_id()).unwrap().status,
        "approved"
    );
}

#[tokio::test]
async fn people_routes_are_owner_or_self() {
    let p = people().await;
    let (sam_h, mia_h, owner_h) = (
        p.handle(&p.sam_session),
        p.handle(&p.mia_session),
        p.handle(&p.owner_session),
    );
    // The owner sees everyone.
    let (status, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(status, 200, "{v}");
    let names: Vec<&str> = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["display_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Sam", "Mia"]);
    assert_eq!(v["people"][0]["agents"][0]["handle"], json!(sam_h));
    assert_eq!(v["people"][0]["grants"][0]["id"], json!(p.sam.grant_id()));
    assert_eq!(v["unconnected"]["on"], json!(true));
    let (status, v) = p.owner("GET", "/api/people/me", None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["person"]["owner"], json!(true));
    let mine: Vec<&Value> = v["agents"].as_array().unwrap().iter().collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["handle"], json!(owner_h));
    // Sam sees himself only.
    let (status, v) = p.sams_browser("GET", "/api/people", None).await;
    assert_eq!((status, code(&v)), (403, "forbidden"), "{v}");
    let (status, v) = p.sams_browser("GET", "/api/people/me", None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["person"]["public_id"], json!(p.sam_public));
    assert_eq!(v["grants"].as_array().unwrap().len(), 1);
    assert_eq!(v["agents"].as_array().unwrap().len(), 1);
    assert!(!v.to_string().contains(&mia_h), "{v}");
    // Owner-only changes, from Sam's browser.
    for (m, path, body) in [
        (
            "PUT",
            format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": "clax:full"})),
        ),
        (
            "PUT",
            format!("/api/agents/{sam_h}/elevate"),
            Some(json!({"access": "clax:full"})),
        ),
        ("DELETE", format!("/api/people/{}", p.sam_public), None),
    ] {
        let (status, v) = p.sams_browser(m, &path, body).await;
        assert_eq!((status, code(&v)), (403, "forbidden"), "{m} {path}: {v}");
    }
    // Mia's agent and machine are not Sam's to narrow or end.
    for (m, path, body) in [
        (
            "PUT",
            format!("/api/agents/{mia_h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        ),
        (
            "PUT",
            format!("/api/people/me/grants/{}/narrow", p.mia.grant_id()),
            Some(json!({"access": "clax:reader"})),
        ),
        ("DELETE", format!("/api/agents/{mia_h}"), None),
    ] {
        let (status, v) = p.sams_browser(m, &path, body).await;
        assert_eq!(status, 404, "{m} {path}: {v}");
    }
    // Nor are they the owner's to narrow: narrowing is the person's.
    for path in [
        format!("/api/agents/{mia_h}/narrow"),
        format!("/api/people/me/grants/{}/narrow", p.mia.grant_id()),
    ] {
        let (status, v) = p
            .owner("PUT", &path, Some(json!({"access": "clax:reader"})))
            .await;
        assert_eq!(status, 404, "{path}: {v}");
    }
    assert!(
        p.ts.store
            .grant(&p.mia.grant_id())
            .unwrap()
            .narrow
            .is_none()
    );
    // The owner's own agent of this machine: the token's rights are not
    // limited, so it is not narrowed.
    let (status, v) = p
        .owner(
            "PUT",
            &format!("/api/agents/{owner_h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!((status, code(&v)), (400, "token_agent"), "{v}");
    // A LAN viewer has no People.
    let lan = p.ts.viewer(Some("Lan")).await;
    for path in ["/api/people", "/api/people/me"] {
        let c = format!("clax_viewer={}", lan.cookie);
        let (status, _) = p.call(Who::Browser(&c), "GET", path, None).await;
        assert_eq!(status, 403, "{path}");
    }
}

#[tokio::test]
async fn a_person_or_the_owner_ends_one_agent_with_its_cleanup() {
    let p = people().await;
    let (sam_h, mia_h) = (p.handle(&p.sam_session), p.handle(&p.mia_session));
    // Sam's agent asks him something and is narrowed; the owner elevated it.
    let r = p
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{}/questions", p.sam_session),
            Some(&p.sam_session),
            Some(json!({"source": "ask", "questions": [{"question": "Teal?",
                "header": "Colour", "options": [{"label": "Teal"}, {"label": "Amber"}]}]})),
        )
        .await;
    assert_eq!(r.status(), 201);
    let qid = r.json::<Value>().await.unwrap()["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{sam_h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/agents/{sam_h}/elevate"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    // Sam ends it from his browser.
    let (status, v) = p
        .sams_browser("DELETE", &format!("/api/agents/{sam_h}"), None)
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["agent"]["withdrawn_questions"], json!([qid]));
    let reason: String = rusqlite::Connection::open(p.ts.home.db_path())
        .unwrap()
        .query_row(
            "SELECT ended_reason FROM sessions WHERE id = ?1",
            [&p.sam_session],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reason, "person");
    assert_eq!(p.stored_rights(&p.sam_session), (None, None));
    let ends: Vec<Value> = p.audited("session.end");
    assert_eq!(ends.last().unwrap()["reason"], "person", "{ends:?}");
    // Gone: a second end finds nothing.
    let (status, _) = p
        .sams_browser("DELETE", &format!("/api/agents/{sam_h}"), None)
        .await;
    assert_eq!(status, 404);
    // The owner ends anyone's.
    let (status, v) = p
        .owner("DELETE", &format!("/api/agents/{mia_h}"), None)
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(
        p.ts.store
            .get_session(&p.mia_session)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
}

/// The owner's agent and an artifact it published, with a LAN viewer's
/// thread sent to the agents.
async fn lan_thread(p: &People) -> (String, String, clax_server::testing::TestViewer) {
    let a =
        p.ts.publish_as(&p.owner_session, "Plan", "<main><p>Plan</p></main>")
            .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let lan = p.ts.viewer(Some("Lan")).await;
    let t = p.ts.thread_as(&aid, &lan.cookie, "Lan says hi").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let c = format!("clax_viewer={}", lan.cookie);
    let (status, v) = p
        .call(
            Who::Browser(&c),
            "POST",
            &format!("/api/artifacts/{aid}/threads/{tid}/send"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    (aid, tid, lan)
}

/// What the owner's agent's next tool result carries.
async fn owner_feedback(p: &People) -> Value {
    let (status, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/sessions/{}/feedback?tier=piggyback", p.owner_session),
            None,
        )
        .await;
    assert_eq!(status, 200, "{v}");
    v
}

#[tokio::test]
async fn unconnected_switch_off_withholds_lan_viewers_from_owner_agents() {
    let p = people().await;
    let (status, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(status, 200);
    assert_eq!(v["unconnected"]["on"], json!(true), "{v}");
    let switch = v["unconnected"]["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Off: the owner's approval of `*unconnected` revoked.
    let (status, v) = p
        .owner("DELETE", &format!("/api/approvals/{switch}"), None)
        .await;
    assert_eq!(status, 200, "{v}");
    let (_, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(v["unconnected"], json!({"on": false, "approval_id": null}));
    let (aid, tid, _) = lan_thread(&p).await;
    let fb = owner_feedback(&p).await;
    let text = fb.to_string();
    assert!(!text.contains("Lan says hi"), "{fb}");
    assert!(
        text.contains("comments from Lan (not connected) on"),
        "{fb}"
    );
    // The owner's agent's thread read holds it back too.
    let (status, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/artifacts/{aid}/threads/{tid}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(v["thread"]["comments"][0]["body"], Value::Null, "{v}");
    // The owner's browser still reads it.
    let (_, v) = p
        .owner("GET", &format!("/api/artifacts/{aid}/threads/{tid}"), None)
        .await;
    assert_eq!(v["thread"]["comments"][0]["body"], "Lan says hi");
    // On again: delivered, its author marked.
    let (status, v) = p
        .owner(
            "POST",
            "/api/approvals",
            Some(json!({"author": "*unconnected", "scope": "everywhere"})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let (_, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(v["unconnected"]["on"], json!(true));
    let fb = owner_feedback(&p).await;
    assert!(fb.to_string().contains("Lan says hi"), "{fb}");
    assert_eq!(fb["feedback"][0]["author"], "Lan (not connected)", "{fb}");
}

#[tokio::test]
async fn unconnected_authors_are_marked_not_connected() {
    let p = people().await;
    let (aid, tid, lan) = lan_thread(&p).await;
    // Mia (a person, approved by the owner) and the owner comment too.
    let (status, _) = p
        .owner(
            "POST",
            "/api/approvals",
            Some(
                json!({"author": p.ts.store.viewer_of_grant(&p.mia.grant_id()).unwrap().public_id,
                         "scope": "everywhere"}),
            ),
        )
        .await;
    assert_eq!(status, 201);
    let mia_viewer = p.mia_cookie.trim_start_matches("clax_viewer=").to_string();
    p.ts.reply_as(&aid, &tid, &mia_viewer, "Mia agrees").await;
    let fb = owner_feedback(&p).await;
    let items = fb["feedback"].as_array().unwrap();
    let author = |body: &str| {
        items
            .iter()
            .find(|i| i["body"] == body)
            .unwrap_or_else(|| panic!("{body}: {fb}"))["author"]
            .clone()
    };
    assert_eq!(author("Lan says hi"), "Lan (not connected)");
    assert_eq!(author("Mia agrees"), "Mia");
    assert!(
        fb["text"]
            .as_str()
            .unwrap()
            .contains("Lan (not connected): \"Lan says hi\""),
        "{fb}"
    );
    // In thread views an agent reads, and the artifact's participants.
    let (_, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/artifacts/{aid}/threads/{tid}"),
            None,
        )
        .await;
    let names: Vec<&str> = v["thread"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["author_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Lan (not connected)", "Mia"], "{v}");
    let (_, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/artifacts/{aid}"),
            None,
        )
        .await;
    let people = &v["artifact"]["participants"]["people"];
    let lan_row = people
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["public_id"] == json!(lan.public_id))
        .unwrap();
    assert_eq!(lan_row["display_name"], "Lan (not connected)", "{v}");
    // Presence and the viewer lookup the owner's agents read.
    let c = format!("clax_viewer={}", lan.cookie);
    let (status, v) = p
        .call(
            Who::Browser(&c),
            "PUT",
            "/api/viewers/me/presence",
            Some(json!({"artifact_id": aid, "state": "here"})),
        )
        .await;
    assert!(status < 300, "{status}: {v}");
    let (status, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/artifacts/{aid}/presence"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let row = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["public_id"] == json!(lan.public_id))
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(row["display_name"], "Lan (not connected)", "{v}");
    let (status, v) = p
        .call(
            Who::OwnerAgent,
            "GET",
            &format!("/api/viewers?ids={}", lan.public_id),
            None,
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["viewers"][0]["display_name"], "Lan (not connected)",
        "{v}"
    );
    let (_, v) = p
        .owner("GET", &format!("/api/viewers?ids={}", lan.public_id), None)
        .await;
    assert_eq!(v["viewers"][0]["display_name"], "Lan", "{v}");
    // People are not filtered or marked: the owner's browser reads "Lan".
    let (_, v) = p
        .owner("GET", &format!("/api/artifacts/{aid}/threads/{tid}"), None)
        .await;
    assert_eq!(v["thread"]["comments"][0]["author_name"], "Lan");
    assert!(!v.to_string().contains("(not connected)"), "{v}");
}

#[tokio::test]
async fn the_extension_lists_the_owners_approval_requests_and_decides_nothing() {
    let p = people().await;
    let ext = p.ts.extension().await;
    let r = ext.get("/api/approvals/requests").await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["open"], json!(0), "{v}");
    let r = ext
        .post(
            "/api/approvals/requests/R/decide",
            json!({"scope": "everywhere"}),
        )
        .await;
    assert_eq!(r.status(), 403);
    let r = ext.get("/api/people").await;
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn the_people_page_is_the_gallery() {
    let ts = TestServer::spawn().await;
    let a = ts.get("/").await;
    let b = ts.get("/people").await;
    assert_eq!(a.status(), b.status());
    assert_eq!(a.text().await.unwrap(), b.text().await.unwrap());
}

impl People {
    /// The stored hash of session `sid`'s secret.
    fn stored_secret(&self, sid: &str) -> Option<String> {
        let db = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db.query_row(
            "SELECT secret_sha256 FROM sessions WHERE id = ?1",
            [sid],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Sam's agent `sid` publishes a version of `aid` over `if_version`,
    /// proving `sid` with `secret` (none: no secret header).
    async fn sam_publishes_proving(
        &self,
        sid: &str,
        secret: Option<&str>,
        aid: &str,
        if_version: u32,
    ) -> (u16, Value) {
        json_of(
            self.sam
                .agent_proving(
                    "POST",
                    &format!("/api/artifacts/{aid}/versions"),
                    Some(sid),
                    secret,
                    Some(json!({"if_version": if_version, "files": {
                        "index.html": {"content": "<p>sibling</p>", "encoding": "utf8"}}})),
                )
                .await,
        )
        .await
    }

    /// Sam's agent `sid` ends its session.
    async fn sam_ends(&self, sid: &str) {
        let r = self
            .sam
            .agent(
                "PATCH",
                &format!("/api/sessions/{sid}"),
                Some(sid),
                Some(json!({"ended": true})),
            )
            .await;
        assert_eq!(r.status(), 200);
    }
}

/// Spec §6.8 (O10): a sibling agent on the same machine holds the
/// machine's key but not another session's secret, so it cannot act with
/// that session's elevation, by its header or by its path.
#[tokio::test]
async fn a_sibling_cannot_act_as_an_elevated_session_without_its_secret() {
    let p = people().await;
    let a =
        p.ts.publish("Quarterly Review", &[("index.html", "<p>v1</p>")])
            .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    let second = id(&p.sam.register("claude", "sam-h2").await);
    let h = p.handle(&p.sam_session);
    let access = json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [aid]}]);
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/agents/{h}/elevate"),
            Some(json!({"access": access})),
        )
        .await;
    assert_eq!(status, 200);
    // The sibling finds the elevated session's ID in its machine's listing.
    let (status, v) = p
        .sam_agent(&second, "GET", "/api/sessions?live=true", None)
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(v.to_string().contains(&p.sam_session), "{v}");
    // Naming it without a secret, or with its own: refused.
    let own = p.sam.secret_of(&second).unwrap();
    for (what, secret) in [("no secret", None), ("its own secret", Some(own.as_str()))] {
        let (status, v) = p
            .sam_publishes_proving(&p.sam_session, secret, &aid, 1)
            .await;
        assert_eq!(
            (status, code(&v)),
            (401, "invalid_session_proof"),
            "{what}: {v}"
        );
    }
    // Naming it in the path while proving itself in the header: refused.
    let (status, v) = json_of(
        p.sam
            .agent(
                "POST",
                &format!("/api/sessions/{}/working/end", p.sam_session),
                Some(&second),
                Some(json!({})),
            )
            .await,
    )
    .await;
    assert_eq!((status, code(&v)), (401, "invalid_session_proof"), "{v}");
    // The secret twice, even the right one: refused.
    let right = p.sam.secret_of(&p.sam_session).unwrap();
    let r = p
        .sam
        .call(clax_server::testing::Call {
            method: "GET",
            path: "/api/artifacts",
            authorization: Some(format!("Clax-Grant {}", p.sam.access())),
            headers: vec![
                ("x-clax-session", p.sam_session.clone()),
                ("x-clax-session-secret", right.clone()),
                ("x-clax-session-secret", right.clone()),
            ],
            ..Default::default()
        })
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!((status, code(&v)), (401, "invalid_session_proof"), "{v}");
    let (_, v) = p
        .sam_agent(
            &p.sam_session,
            "GET",
            &format!("/api/artifacts/{aid}"),
            None,
        )
        .await;
    assert_eq!(v["versions"].as_array().unwrap().len(), 1, "{v}");
    // The elevated session itself, with its secret, publishes.
    let (status, v) = p
        .sam_publishes_proving(&p.sam_session, Some(&right), &aid, 1)
        .await;
    assert_eq!(status, 201, "{v}");
    // Another person's agent naming it is still another credential's.
    let (status, v) = json_of(
        p.mia
            .agent_proving(
                "GET",
                "/api/artifacts",
                Some(&p.sam_session),
                Some(&right),
                None,
            )
            .await,
    )
    .await;
    assert_eq!((status, code(&v)), (403, "foreign_session"), "{v}");
}

/// Spec §6.8, §7.2: a request naming no session is bound by every live
/// narrowing of its machine's sessions, so an agent narrowed to reading
/// cannot write by leaving its session out; the narrowing ends with it.
#[tokio::test]
async fn a_narrowed_agent_cannot_write_by_naming_no_session() {
    let p = people().await;
    let r =
        p.ts.post_json(
            "/api/artifacts",
            json!({"title": "Notes", "capabilities": {"db": {}},
                   "files": {"index.html": {"content": "<p>v1</p>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let doc = format!("/api/artifacts/{aid}/docs/notes/one");
    let write = json!({"data": {"text": "hi"}, "lww": true});
    // Before any narrowing, the machine writes without a session.
    let (status, v) = json_of(p.sam.agent("PUT", &doc, None, Some(write.clone())).await).await;
    assert_eq!(status, 200, "{v}");
    let second = id(&p.sam.register("claude", "sam-h2").await);
    let h = p.handle(&p.sam_session);
    let (status, _) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    // With its session, refused; without one, refused too.
    let (status, v) = p
        .sam_agent(&p.sam_session, "PUT", &doc, Some(write.clone()))
        .await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    let (status, v) = json_of(p.sam.agent("PUT", &doc, None, Some(write.clone())).await).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // Reading, within the narrowing, still works without a session.
    let (status, v) = json_of(p.sam.agent("GET", &doc, None, None).await).await;
    assert_eq!(status, 200, "{v}");
    // The sibling, proving its own session, keeps its own rights.
    let (status, v) = p.sam_agent(&second, "PUT", &doc, Some(write.clone())).await;
    assert_eq!(status, 200, "{v}");
    // Once the narrowed agent ends, its narrowing no longer binds the machine.
    p.sam_ends(&p.sam_session).await;
    let (status, v) = json_of(p.sam.agent("PUT", &doc, None, Some(write.clone())).await).await;
    assert_eq!(status, 200, "{v}");
}

/// Spec §6.8: the secret is answered once, kept as a hash, recorded only as
/// issued, cleared at the session's end; a leaked secret of an ended
/// session proves nothing, and registering again gets a new one. The
/// daemon token's sessions get none.
#[tokio::test]
async fn a_session_secret_is_issued_once_and_dies_with_its_session() {
    let p = people().await;
    let secret = p.sam.secret_of(&p.sam_session).unwrap();
    assert!(secret.starts_with("cxs_") && secret.len() == 47, "{secret}");
    let hash = clax_core::secret::sha256_hex(&secret);
    assert_eq!(p.stored_secret(&p.sam_session), Some(hash.clone()));
    let started = p.audited("session.start");
    let sams = started
        .iter()
        .find(|b| b["harness_session_id"] == "sam-h1")
        .unwrap();
    assert_eq!(sams["secret_issued"], json!(true), "{sams}");
    let owners = started
        .iter()
        .find(|b| b["harness_session_id"] == "owner-h1")
        .unwrap();
    assert!(owners.get("secret_issued").is_none(), "{owners}");
    // Neither the secret nor its hash is in the journal.
    let db = rusqlite::Connection::open(p.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let mut stmt = db.prepare("SELECT body FROM audit_events").unwrap();
    for body in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        let body = body.unwrap();
        assert!(!body.contains(&secret) && !body.contains(&hash), "{body}");
    }
    // The owner's agent registers with the token: no secret.
    assert_eq!(p.stored_secret(&p.owner_session), None);
    let r =
        p.ts.post_json(
            "/api/sessions",
            json!({"harness": "claude", "harness_session_id": "owner-h9", "cwd": "/w"}),
        )
        .await;
    let v: Value = r.json().await.unwrap();
    assert!(v.get("session_secret").is_none(), "{v}");
    // The session ends: its secret is cleared and proves nothing.
    p.sam_ends(&p.sam_session).await;
    assert_eq!(p.stored_secret(&p.sam_session), None);
    let (status, v) = p
        .sam_agent(&p.sam_session, "GET", "/api/artifacts", None)
        .await;
    assert_eq!((status, code(&v)), (401, "invalid_session_proof"), "{v}");
    // The same harness session registers again: a new row, a new secret.
    let again = id(&p.sam.register("claude", "sam-h1").await);
    assert_ne!(again, p.sam_session);
    let fresh = p.sam.secret_of(&again).unwrap();
    assert_ne!(fresh, secret);
    let (status, v) = json_of(
        p.sam
            .agent_proving("GET", "/api/artifacts", Some(&again), Some(&secret), None)
            .await,
    )
    .await;
    assert_eq!(
        (status, code(&v)),
        (401, "invalid_session_proof"),
        "the old secret: {v}"
    );
    // Revoking the machine ends its sessions and clears their secrets.
    let (status, v) = p
        .owner("DELETE", &format!("/api/grants/{}", p.sam.grant_id()), None)
        .await;
    assert!(status == 200 || status == 204, "{status}: {v}");
    assert_eq!(p.stored_secret(&again), None);
}

/// Spec §6.7, §6.8: registration and join refresh a live session only for
/// a request that proves it, so a sibling cannot take one over by its
/// harness session ID or its parent PID.
#[tokio::test]
async fn registering_over_a_live_session_needs_its_secret() {
    let p = people().await;
    let reg = json!({"harness": "claude", "harness_session_id": "sam-h1", "cwd": "/w"});
    let join = json!({"harness": "claude", "harness_session_id": "sam-h1", "parent_pid": 4242});
    for (path, body) in [("/api/sessions", &reg), ("/api/sessions/join", &join)] {
        let (status, v) = json_of(
            p.sam
                .agent_proving("POST", path, None, None, Some(body.clone()))
                .await,
        )
        .await;
        assert_eq!(
            (status, code(&v)),
            (401, "invalid_session_proof"),
            "{path}: {v}"
        );
    }
    // A match by parent PID is refused the same way.
    let pid_row = id(&p
        .sam
        .register_with(json!({"harness": "codex", "cwd": "/w", "parent_pid": 777}))
        .await);
    let by_pid = json!({"harness": "codex", "cwd": "/w", "parent_pid": 777});
    let (status, v) = json_of(
        p.sam
            .agent_proving("POST", "/api/sessions", None, None, Some(by_pid.clone()))
            .await,
    )
    .await;
    assert_eq!((status, code(&v)), (401, "invalid_session_proof"), "{v}");
    let (status, v) = json_of(
        p.sam
            .agent("POST", "/api/sessions", Some(&pid_row), Some(by_pid))
            .await,
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["session"]["id"], json!(pid_row), "{v}");
    // With the session's secret: the same row, and no new secret.
    for (path, body) in [("/api/sessions", reg), ("/api/sessions/join", join)] {
        let (status, v) = json_of(
            p.sam
                .agent("POST", path, Some(&p.sam_session), Some(body))
                .await,
        )
        .await;
        assert!(status == 200 || status == 201, "{path}: {v}");
        assert_eq!(v["session"]["id"], json!(p.sam_session), "{path}: {v}");
        assert!(v.get("session_secret").is_none(), "{path}: {v}");
    }
    // A join that makes a row answers its secret, which then proves it.
    let (status, v) = json_of(
        p.sam
            .agent(
                "POST",
                "/api/sessions/join",
                None,
                Some(json!({"harness": "claude", "harness_session_id": "sam-h7", "parent_pid": 4343})),
            )
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(v["session_secret"].as_str().unwrap().starts_with("cxs_"));
    p.sam.keep_session(&v);
    let made = v["session"]["id"].as_str().unwrap();
    let (status, v) = p.sam_agent(made, "GET", "/api/artifacts", None).await;
    assert_eq!(status, 200, "{v}");
}
