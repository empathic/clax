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
    let (sam_cookie, mia_cookie) = (ts.browser_cookie(&sv.id), ts.browser_cookie(&mv.id));
    let owner_session = id(&ts.register_session("claude", "owner-h1").await);
    let sam_session = id(&sam.register("claude", "sam-h1").await);
    let mia_session = id(&mia.register("codex", "mia-h1").await);
    People {
        ts,
        sam,
        mia,
        sam_cookie: format!("clax_viewer={sam_cookie}"),
        sam_public: sv.public_id,
        mia_cookie: format!("clax_viewer={mia_cookie}"),
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

    /// A session's stored narrowing and elevations (their access arrays,
    /// joined), `None` for none.
    fn stored_rights(&self, sid: &str) -> (Option<String>, Option<String>) {
        let db = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db.query_row(
            "SELECT narrow_json, (SELECT group_concat(access_json) FROM session_elevations
               WHERE session_id = ?1) FROM sessions WHERE id = ?1",
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
    assert!(text.contains("1 comment from Sam on"), "{fb}");
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
        ("DELETE", format!("/api/people/{}", p.sam_public), None),
    ] {
        let (status, v) = p.sams_browser(m, &path, body).await;
        assert_eq!((status, code(&v)), (403, "forbidden"), "{m} {path}: {v}");
    }
    // Sam raises only on artifacts he created, never at every artifact:
    // not even his own agent.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{sam_h}/elevate"),
            Some(json!({"access": "clax:full"})),
        )
        .await;
    assert_eq!((status, code(&v)), (403, "not_creator"), "{v}");
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
        text.contains("1 comment from Lan (not connected) on"),
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

impl People {
    /// A session's stored harness session ID and parent PID.
    fn stored_row(&self, sid: &str) -> (Option<String>, Option<u32>) {
        let s = self.ts.store.get_session(sid).unwrap().unwrap();
        (s.harness_session_id, s.parent_pid)
    }
}

/// Spec §6.8 Matching: a proof never widens the match. An agent started
/// from another agent's shell (`claude -p`), holding its parent agent's
/// credential, joins or registers with its own harness session ID and
/// process: it gets a session of its own, and the parent's row, its
/// harness session ID and parent PID stay as they were.
#[tokio::test]
async fn a_nested_agent_proving_its_parents_session_gets_its_own() {
    let p = people().await;
    let parent = id(&p
        .sam
        .register_with(json!({"harness": "claude", "harness_session_id": "b-h",
                              "cwd": "/w", "parent_pid": 100}))
        .await);
    let before = p.stored_row(&parent);
    let nested_join = json!({"harness": "claude", "harness_session_id": "c-h",
                             "parent_pid": 300, "ancestor_pids": [250, 100]});
    let (status, v) = json_of(
        p.sam
            .agent(
                "POST",
                "/api/sessions/join",
                Some(&parent),
                Some(nested_join),
            )
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_ne!(v["session"]["id"], json!(parent), "{v}");
    assert!(v["session_secret"].as_str().is_some(), "{v}");
    assert_eq!(p.stored_row(&parent), before);
    let nested_reg = json!({"harness": "claude", "harness_session_id": "d-h",
                            "cwd": "/w", "parent_pid": 300});
    let (status, v) = json_of(
        p.sam
            .agent("POST", "/api/sessions", Some(&parent), Some(nested_reg))
            .await,
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_ne!(v["session"]["id"], json!(parent), "{v}");
    assert_eq!(p.stored_row(&parent), before, "the parent PID is kept");
    // A proof of another harness's session is no match either.
    let (status, v) = json_of(
        p.sam
            .agent(
                "POST",
                "/api/sessions",
                Some(&parent),
                Some(json!({"harness": "codex", "harness_session_id": "b-h", "cwd": "/w"})),
            )
            .await,
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_ne!(v["session"]["id"], json!(parent), "{v}");
    assert_eq!(v["session"]["harness"], "codex");
    // The parent itself, proving its own row from another process with the
    // same harness session ID, keeps its row and its first parent PID.
    let (status, v) = json_of(
        p.sam
            .agent(
                "POST",
                "/api/sessions",
                Some(&parent),
                Some(json!({"harness": "claude", "harness_session_id": "b-h",
                            "cwd": "/w", "parent_pid": 999})),
            )
            .await,
    )
    .await;
    assert_eq!(
        (status, v["session"]["id"].clone()),
        (201, json!(parent)),
        "{v}"
    );
    assert_eq!(p.stored_row(&parent), before);
}

/// Spec §6.8: every live narrowing binds a request naming no session, not
/// only one of them, whichever session carries the narrower one.
#[tokio::test]
async fn every_live_narrowing_binds_a_request_naming_no_session() {
    let p = people().await;
    let r =
        p.ts.post_json(
            "/api/artifacts",
            json!({"title": "Notes", "capabilities": {"db": {}},
                   "files": {"index.html": {"content": "<p>v1</p>", "encoding": "utf8"}}}),
        )
        .await;
    let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let doc = format!("/api/artifacts/{aid}/docs/notes/one");
    let write = json!({"data": {"text": "hi"}, "lww": true});
    let second = id(&p.sam.register("claude", "sam-h2").await);
    for (wide, narrow) in [(&p.sam_session, &second), (&second, &p.sam_session)] {
        for (sid, access) in [(wide, "clax:collaborator"), (narrow, "clax:reader")] {
            let (status, v) = p
                .sams_browser(
                    "PUT",
                    &format!("/api/agents/{}/narrow", p.handle(sid)),
                    Some(json!({"access": access})),
                )
                .await;
            assert_eq!(status, 200, "{v}");
        }
        let (status, v) = json_of(p.sam.agent("PUT", &doc, None, Some(write.clone())).await).await;
        assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
        let (status, v) = p.sam_agent(wide, "PUT", &doc, Some(write.clone())).await;
        assert_eq!(status, 200, "the wide one itself writes: {v}");
    }
}

// ------------------------------------------ artifact creators delegate (O11)

impl People {
    /// Mia's agent `sid` publishes a version of `aid` over `if_version`.
    async fn mia_publishes(&self, sid: &str, aid: &str, if_version: u32) -> (u16, Value) {
        json_of(
            self.mia
                .agent(
                    "POST",
                    &format!("/api/artifacts/{aid}/versions"),
                    Some(sid),
                    Some(json!({"if_version": if_version, "files": {
                        "index.html": {"content": format!("<p>m{if_version}</p>"), "encoding": "utf8"}}})),
                )
                .await,
        )
        .await
    }

    /// Sam's agent creates "Sam's Notes": its artifact ID.
    async fn sams_notes(&self) -> String {
        let (status, v) = self
            .sam_agent(
                &self.sam_session,
                "POST",
                "/api/artifacts",
                Some(json!({"title": "Sam's Notes",
                    "files": {"index.html": {"content": "<p>notes</p>", "encoding": "utf8"}}})),
            )
            .await;
        assert_eq!(status, 201, "{v}");
        v["artifact"]["id"].as_str().unwrap().to_string()
    }

    async fn mias_browser(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call(Who::Browser(&self.mia_cookie), m, path, body)
            .await
    }
}

/// The keys of object `v`, sorted.
fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    k.sort();
    k
}

fn update_at(aid: &str) -> Value {
    json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [aid]}])
}

/// Spec §7.2, §7.4 (O11): the creator of an artifact raises another
/// person's agent on it from their browser; that agent may then act
/// there and nowhere else, and Sam's raise is his alone to remove.
#[tokio::test]
async fn a_creator_raises_another_persons_agent_on_their_artifact_only() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let qr =
        p.ts.publish("Quarterly Review", &[("index.html", "<p>v1</p>")])
            .await;
    let qr = qr["artifact"]["id"].as_str().unwrap().to_string();
    let mia_h = p.handle(&p.mia_session);
    // Mia's agent (a collaborator) may not publish Sam's artifact.
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // Sam may not raise it at every artifact, at `own`, at the owner's
    // artifact, nor to ask questions: none is an artifact he created.
    for bad in [
        json!("clax:full"),
        json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["*"]}]),
        json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["own"]}]),
        update_at(&qr),
        json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [notes, qr]}]),
        json!([{"type": "clax-ask", "actions": ["ask"]}]),
    ] {
        let (status, v) = p
            .sams_browser(
                "PUT",
                &format!("/api/agents/{mia_h}/elevate"),
                Some(json!({"access": bad})),
            )
            .await;
        assert_eq!((status, code(&v)), (403, "not_creator"), "{bad}: {v}");
    }
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
    // Sam raises it on his own artifact: it publishes there, and nowhere
    // else, and Mia's other agent stays as it was.
    let gen_before = p.ts.store.visibility_generation();
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/elevate"),
            Some(json!({"access": update_at(&notes)})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(p.ts.store.visibility_generation() > gen_before, "bumped");
    // Sam reads back his own raise of Mia's agent, and nothing more of it.
    assert_eq!(
        keys(&v["agent"]),
        ["elevation", "handle", "name", "person", "version"],
        "{v}"
    );
    assert_eq!(v["agent"]["handle"], json!(mia_h));
    assert_eq!(v["agent"]["elevation"], update_at(&notes));
    let rec = p.audited("agent.elevate");
    assert_eq!(rec.last().unwrap()["granter"], json!(p.sam_public));
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = p.mia_publishes(&p.mia_session, &qr, 1).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    let other = id(&p.mia.register("codex", "mia-h2").await);
    let (status, _) = p.mia_publishes(&other, &notes, 2).await;
    assert_eq!(status, 403);
    // Mia sees the raise and who gave it on her agent's card; she cannot
    // remove it (her DELETE removes only a raise of her own).
    let (_, me) = p.mias_browser("GET", "/api/people/me", None).await;
    let card = me["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == json!(mia_h))
        .unwrap()
        .clone();
    assert_eq!(card["elevations"][0]["granter"]["name"], json!("Sam"));
    let (status, v) = p
        .mias_browser("DELETE", &format!("/api/agents/{mia_h}/elevate"), None)
        .await;
    assert_eq!((status, code(&v)), (404, "not_raised"), "{v}");
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 2).await;
    assert_eq!(status, 201, "Sam's raise stays: {v}");
    // Sam lists his raises; Mia lists none; whether each created an
    // artifact, and what Sam may give there.
    let (status, v) = p
        .sams_browser(
            "GET",
            &format!("/api/people/me/raises?artifact={notes}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["creator"], json!(true));
    assert!(v["holds"].to_string().contains("update"), "{v}");
    // The agents on it a raise reaches: Sam's, which made it, and Mia's,
    // which published there.
    let raisable = v["raisable"].as_array().unwrap();
    assert!(raisable.contains(&json!(mia_h)), "{v}");
    assert!(raisable.contains(&json!(p.handle(&p.sam_session))), "{v}");
    assert_eq!(v["raises"].as_array().unwrap().len(), 1);
    assert_eq!(v["raises"][0]["agent"]["handle"], json!(mia_h));
    assert_eq!(v["raises"][0]["access"], update_at(&notes));
    let (_, v) = p
        .sams_browser("GET", &format!("/api/people/me/raises?artifact={qr}"), None)
        .await;
    assert_eq!(
        (v["creator"].clone(), v["holds"].clone()),
        (json!(false), Value::Null)
    );
    let (_, v) = p.mias_browser("GET", "/api/people/me/raises", None).await;
    assert_eq!(v["raises"], json!([]));
    // The raises listing is browser-only, like the rest of People.
    let (status, v) = p
        .call(Who::Token, "GET", "/api/people/me/raises", None)
        .await;
    assert_eq!((status, code(&v)), (403, "browser_required"), "{v}");
    let (status, _) = p
        .sam_agent(&p.sam_session, "GET", "/api/people/me/raises", None)
        .await;
    assert_eq!(status, 403);
    // Sam removes it: from its next request on, it is gone.
    let (status, _) = p
        .sams_browser("DELETE", &format!("/api/agents/{mia_h}/elevate"), None)
        .await;
    assert_eq!(status, 200);
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 3).await;
    assert_eq!(status, 403);
}

/// Spec §7.2 (O11): a creator's raise never gives more than the creator
/// may do on that artifact, when given and at every check after.
#[tokio::test]
async fn a_creators_raise_cannot_exceed_the_creators_own_rights() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let mia_h = p.handle(&p.mia_session);
    let delete_too =
        json!([{"type": "clax-artifacts", "actions": ["update", "delete"], "locations": [notes]}]);
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/elevate"),
            Some(json!({"access": delete_too})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    // The owner makes Sam a reader: he may no longer publish there, so his
    // raise gives Mia's agent nothing from its next request on, and he
    // cannot give it again.
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/elevate"),
            Some(json!({"access": update_at(&notes)})),
        )
        .await;
    assert_eq!((status, code(&v)), (403, "beyond_your_rights"), "{v}");
    let (_, v) = p
        .sams_browser(
            "GET",
            &format!("/api/people/me/raises?artifact={notes}"),
            None,
        )
        .await;
    assert!(!v["holds"].to_string().contains("update"), "{v}");
    // Sam a collaborator again: the raise he gave applies again.
    let (status, _) = p
        .owner(
            "PUT",
            &format!("/api/people/{}/rights", p.sam_public),
            Some(json!({"rights": "clax:collaborator"})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 201, "{v}");
}

/// Spec §14 (O11): deleting the artifact, or revoking its creator, drops
/// the creator's raises.
#[tokio::test]
async fn deleting_the_artifact_or_revoking_the_creator_drops_the_raise() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let second = p.sams_notes().await;
    let mia_h = p.handle(&p.mia_session);
    let both =
        json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [notes, second]}]);
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/elevate"),
            Some(json!({"access": both})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    // The first artifact deleted: the raise keeps the second only.
    let (status, v) = p
        .sam_agent(
            &p.sam_session,
            "DELETE",
            &format!("/api/artifacts/{notes}"),
            None,
        )
        .await;
    assert!(status < 300, "{v}");
    let (_, v) = p.mias_browser("GET", "/api/people/me", None).await;
    let card = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == json!(mia_h))
        .unwrap()
        .clone();
    assert_eq!(
        card["elevations"][0]["access"],
        update_at(&second),
        "{card}"
    );
    let dropped = p.audited("agent.elevate");
    assert_eq!(dropped.last().unwrap()["reason"], json!("artifact_deleted"));
    let (status, v) = p.mia_publishes(&p.mia_session, &second, 1).await;
    assert_eq!(status, 201, "{v}");
    // Sam revoked: his raise goes, and Mia's agent stays as it was.
    let (status, _) = p
        .owner("DELETE", &format!("/api/people/{}", p.sam_public), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
    assert_eq!(
        p.audited("agent.elevate").last().unwrap()["reason"],
        json!("person_revoked")
    );
    let (status, _) = p.mia_publishes(&p.mia_session, &second, 2).await;
    assert_eq!(status, 403);
    let s = p.ts.store.get_session(&p.mia_session).unwrap().unwrap();
    assert!(s.ended_at.is_none());
}

/// Spec §7.2 (O11): two granters' elevations of one agent stay separate:
/// each covers only its own locations, and each granter removes only theirs.
#[tokio::test]
async fn two_granters_elevations_stay_separate() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let qr =
        p.ts.publish("Quarterly Review", &[("index.html", "<p>v1</p>")])
            .await;
    let qr = qr["artifact"]["id"].as_str().unwrap().to_string();
    let mia_h = p.handle(&p.mia_session);
    let path = format!("/api/agents/{mia_h}/elevate");
    let (status, _) = p
        .sams_browser("PUT", &path, Some(json!({"access": update_at(&notes)})))
        .await;
    assert_eq!(status, 200);
    let (status, v) = p
        .owner("PUT", &path, Some(json!({"access": update_at(&qr)})))
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["agent"]["elevate"], update_at(&qr));
    assert_eq!(v["agent"]["elevations"].as_array().unwrap().len(), 2);
    assert_eq!(v["agent"]["elevations"][0]["granter"]["owner"], json!(true));
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 201);
    let (status, _) = p.mia_publishes(&p.mia_session, &qr, 1).await;
    assert_eq!(status, 201);
    // Sam's raise names only his artifact; a raise naming the owner's is
    // refused rather than merged with the owner's.
    let (status, v) = p
        .sams_browser("PUT", &path, Some(json!({"access": update_at(&qr)})))
        .await;
    assert_eq!((status, code(&v)), (403, "not_creator"), "{v}");
    // The owner removes theirs: Sam's stays.
    let (status, v) = p.owner("DELETE", &path, None).await;
    assert_eq!(status, 200);
    assert_eq!(v["agent"]["elevations"].as_array().unwrap().len(), 1);
    let (status, _) = p.mia_publishes(&p.mia_session, &qr, 2).await;
    assert_eq!(status, 403);
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 2).await;
    assert_eq!(status, 201);
    // The owner's People shows the agent with Sam's raise.
    let (_, v) = p.owner("GET", "/api/people", None).await;
    let mia = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["display_name"] == json!("Mia"))
        .unwrap()
        .clone();
    assert_eq!(
        mia["agents"][0]["elevations"][0]["granter"]["name"],
        json!("Sam")
    );
}

/// Spec §9.5 (O12, "Your content only"): the owner sees on a person's card
/// that person's live approvals of the owner's content, and nothing else
/// of theirs; no person reads another's approvals.
#[tokio::test]
async fn the_owner_sees_exactly_a_persons_approvals_of_the_owners_content() {
    let p = people().await;
    let owner = p.ts.owner_public_id().await;
    let mia_public =
        p.ts.store
            .viewer_of_grant(&p.mia.grant_id())
            .unwrap()
            .public_id;
    let notes = p.sams_notes().await;
    // Sam approves the owner's content on one artifact and Mia's
    // everywhere; Mia approves the owner's everywhere.
    let (status, v) = p
        .sams_browser(
            "POST",
            "/api/approvals",
            Some(json!({"author": owner, "scope": "artifact", "artifact_id": notes})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = p
        .sams_browser(
            "POST",
            "/api/approvals",
            Some(json!({"author": mia_public, "scope": "everywhere"})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let (_, sams) = p.sams_browser("GET", "/api/approvals", None).await;
    let sams = sams["approvals"].as_array().unwrap().clone();
    let (status, v) = p
        .mias_browser(
            "POST",
            "/api/approvals",
            Some(json!({"author": owner, "scope": "everywhere"})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = p.owner("GET", "/api/people", None).await;
    assert_eq!(status, 200, "{v}");
    let card = |name: &str| {
        v["people"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["display_name"] == json!(name))
            .unwrap()
            .clone()
    };
    let sam = card("Sam")["approvals_of_yours"].clone();
    assert_eq!(sam.as_array().unwrap().len(), 1, "{sam}");
    assert_eq!(sam[0]["author"]["public_id"], json!(owner));
    assert_eq!(sam[0]["approver"], json!(p.sam_public));
    assert_eq!(sam[0]["scope"], json!("artifact"));
    assert_eq!(sam[0]["artifact_title"], json!("Sam's Notes"));
    let mia = card("Mia")["approvals_of_yours"].clone();
    assert_eq!(mia.as_array().unwrap().len(), 1, "{mia}");
    assert_eq!(mia[0]["scope"], json!("everywhere"));
    // Sam's approval of Mia's content is not in the owner's view at all.
    let of_mia = sams
        .iter()
        .find(|a| a["author"]["public_id"] == json!(mia_public))
        .unwrap();
    assert!(
        !v.to_string().contains(of_mia["id"].as_str().unwrap()),
        "{v}"
    );
    // Revoked, it leaves the card.
    let (status, _) = p
        .sams_browser(
            "DELETE",
            &format!("/api/approvals/{}", sam[0]["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert!(status < 300);
    let (_, v) = p.owner("GET", "/api/people", None).await;
    let sam = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["display_name"] == json!("Sam"))
        .unwrap()["approvals_of_yours"]
        .clone();
    assert_eq!(sam, json!([]));
    // A person reads only their own approvals: Mia's list holds none of
    // Sam's, and People is not hers to read.
    let (_, mine) = p.mias_browser("GET", "/api/approvals", None).await;
    assert!(
        mine["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["approver"] == json!(mia_public)),
        "{mine}"
    );
    let (status, _) = p.mias_browser("GET", "/api/people", None).await;
    assert_eq!(status, 403);
    let (_, theirs) = json_of(
        p.mia
            .agent("GET", "/api/approvals", Some(&p.mia_session), None)
            .await,
    )
    .await;
    assert!(
        !theirs.to_string().contains(of_mia["id"].as_str().unwrap()),
        "{theirs}"
    );
}

/// Spec §7.4, §18: raising or removing a raise never shows a person another
/// person's agent beyond its name and their own raise of it: not its
/// narrowing, other granters' raises, rights or machine. Removing a raise
/// one never gave is 404 and recorded nowhere.
#[tokio::test]
async fn a_raise_answers_a_person_with_their_own_raise_alone() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let qr =
        p.ts.publish("Quarterly Review", &[("index.html", "<p>v1</p>")])
            .await;
    let qr = qr["artifact"]["id"].as_str().unwrap().to_string();
    let (mia_h, sam_h) = (p.handle(&p.mia_session), p.handle(&p.sam_session));
    let path = format!("/api/agents/{mia_h}/elevate");
    // Mia narrows her agent; the owner raises it.
    let (status, _) = p
        .mias_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/narrow"),
            Some(json!({"access": "clax:reader"})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = p
        .owner("PUT", &path, Some(json!({"access": update_at(&qr)})))
        .await;
    assert_eq!(status, 200);
    let leaks = |v: &Value| {
        let t = v.to_string();
        ["narrow", "rights", "elevations", "grant_id", "harness", &qr]
            .iter()
            .filter(|k| t.contains(**k))
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
    };
    // Sam, who never raised it, removes nothing: 404, no audit row, and
    // nothing of the agent in the answer.
    let before = p.audited("agent.elevate").len();
    let (status, v) = p.sams_browser("DELETE", &path, None).await;
    assert_eq!((status, code(&v)), (404, "not_raised"), "{v}");
    assert_eq!(leaks(&v), Vec::<String>::new(), "{v}");
    assert_eq!(p.audited("agent.elevate").len(), before);
    // Sam raises it on his artifact: his own raise comes back, nothing else.
    let (status, v) = p
        .sams_browser("PUT", &path, Some(json!({"access": update_at(&notes)})))
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        keys(&v["agent"]),
        ["elevation", "handle", "name", "person", "version"],
        "{v}"
    );
    assert_eq!(leaks(&v), Vec::<String>::new(), "{v}");
    // Removing it answers the same way, with no raise left.
    let (status, v) = p.sams_browser("DELETE", &path, None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["agent"]["elevation"], Value::Null);
    assert_eq!(leaks(&v), Vec::<String>::new(), "{v}");
    // An empty raise is refused, from a person and from the owner.
    for who in ["sam", "owner"] {
        let body = Some(json!({"access": []}));
        let (status, v) = if who == "sam" {
            p.sams_browser("PUT", &path, body).await
        } else {
            p.owner("PUT", &path, body).await
        };
        assert_eq!((status, code(&v)), (400, "invalid_rights"), "{who}: {v}");
    }
    // Raising his own agent, Sam sees what his People shows of it.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{sam_h}/elevate"),
            Some(json!({"access": update_at(&notes)})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(v["agent"]["rights"].is_array(), "{v}");
    assert_eq!(
        v["agent"]["elevations"][0]["granter"]["public_id"],
        json!(p.sam_public)
    );
}

/// Spec §7.4: `raisable` and `holds` are the artifact's creator's alone:
/// another person on it, and the owner on a person's artifact, read neither.
#[tokio::test]
async fn only_the_creator_reads_what_a_raise_could_give_and_reach() {
    let p = people().await;
    let notes = p.sams_notes().await;
    // Mia's agent watches it, so it is on the artifact.
    let r = p
        .mia
        .agent(
            "PUT",
            &format!("/api/sessions/{}/watches/{notes}", p.mia_session),
            Some(&p.mia_session),
            None,
        )
        .await;
    assert!(r.status().is_success(), "{}", r.status());
    let q = format!("/api/people/me/raises?artifact={notes}");
    let (_, v) = p.sams_browser("GET", &q, None).await;
    assert!(
        v["raisable"]
            .as_array()
            .unwrap()
            .contains(&json!(p.handle(&p.mia_session))),
        "{v}"
    );
    for (who, v) in [
        ("Mia", p.mias_browser("GET", &q, None).await.1),
        ("the owner", p.owner("GET", &q, None).await.1),
    ] {
        assert_eq!(
            (
                v["creator"].clone(),
                v["holds"].clone(),
                v["raisable"].clone()
            ),
            (json!(false), Value::Null, json!([])),
            "{who}: {v}"
        );
    }
}

// ------------------------------- who removes a raise; narrowing caps raises

impl People {
    /// The actor and body of each `agent.elevate`, oldest first.
    fn elevate_rows(&self) -> Vec<(Value, Value)> {
        let db = rusqlite::Connection::open(self.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        let mut stmt = db
            .prepare(
                "SELECT actor, body FROM audit_events WHERE kind = 'agent.elevate' ORDER BY seq",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(|x| {
                let (a, b) = x.unwrap();
                (
                    serde_json::from_str(&a).unwrap(),
                    serde_json::from_str(&b).unwrap(),
                )
            })
            .collect()
    }
}

/// Spec §7.4 (the owner's decision "Both may remove"): the agent's person
/// and the owner each remove a creator's raise of it, recorded with who
/// removed it; the creator may raise it again; a third person may not, and
/// learns nothing.
#[tokio::test]
async fn the_agents_person_and_the_owner_remove_a_creators_raise_and_no_one_else() {
    let p = people().await;
    let lee = p.ts.connect_person("Lee", "clax:collaborator").await;
    let lee_cookie = format!(
        "clax_viewer={}",
        p.ts.browser_cookie(&p.ts.store.viewer_of_grant(&lee.grant_id()).unwrap().id)
    );
    let notes = p.sams_notes().await;
    let mia_h = p.handle(&p.mia_session);
    let mia_public =
        p.ts.store
            .viewer_of_grant(&p.mia.grant_id())
            .unwrap()
            .public_id;
    let path = format!("/api/agents/{mia_h}/elevate");
    let sams = format!("{path}?granter={}", p.sam_public);
    let raise = || p.sams_browser("PUT", &path, Some(json!({"access": update_at(&notes)})));
    assert_eq!(raise().await.0, 200);
    // Lee, neither its person nor the owner nor the granter: 404, as for a
    // raise that is not there, nothing recorded, nothing of the agent shown.
    let before = p.elevate_rows().len();
    let (status, v) = p
        .call(Who::Browser(&lee_cookie), "DELETE", &sams, None)
        .await;
    assert_eq!((status, code(&v)), (404, "not_raised"), "{v}");
    assert!(
        !v.to_string().contains(&mia_h) && !v.to_string().contains("rights"),
        "{v}"
    );
    assert_eq!(p.elevate_rows().len(), before);
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 201, "Sam's raise stands");
    // Mia removes it from her agent: recorded as hers, and it is gone.
    let (status, v) = p.mias_browser("DELETE", &sams, None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["agent"]["elevations"],
        json!([]),
        "her own agent's whole view: {v}"
    );
    let (actor, body) = p.elevate_rows().last().unwrap().clone();
    assert_eq!(body["granter"], json!(p.sam_public));
    assert_eq!(body["reason"], json!("removed_by_person"));
    assert!(actor.to_string().contains(&mia_public), "{actor}");
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 2).await;
    assert_eq!(status, 403);
    // Removed again: nothing there, 404.
    let (status, v) = p.mias_browser("DELETE", &sams, None).await;
    assert_eq!((status, code(&v)), (404, "not_raised"), "{v}");
    // Sam raises it again; the owner removes it, recorded as the owner's.
    assert_eq!(raise().await.0, 200);
    let (status, v) = p.owner("DELETE", &sams, None).await;
    assert_eq!(status, 200, "{v}");
    let (actor, body) = p.elevate_rows().last().unwrap().clone();
    assert_eq!(body["reason"], json!("removed_by_owner"));
    assert_eq!(actor["type"], json!("owner"), "{actor}");
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
    // Mia may remove the owner's raise of her agent too.
    assert_eq!(
        p.owner("PUT", &path, Some(json!({"access": update_at(&notes)})))
            .await
            .0,
        200
    );
    let (status, _) = p
        .mias_browser("DELETE", &format!("{path}?granter=owner"), None)
        .await;
    assert_eq!(status, 200);
    let (actor, body) = p.elevate_rows().last().unwrap().clone();
    assert_eq!(
        (body["granter"].clone(), body["reason"].clone()),
        (Value::Null, json!("removed_by_person"))
    );
    assert!(actor.to_string().contains(&mia_public), "{actor}");
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
}

/// Spec §7.2 (the owner's decision "Narrowing caps raises"): the agent's
/// person's narrowing is the last word: read-only, a raised agent publishes
/// nothing, not even where it was raised; without it, the raise applies
/// again.
#[tokio::test]
async fn a_persons_narrowing_caps_a_creators_raise() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let mia_h = p.handle(&p.mia_session);
    let (status, _) = p
        .sams_browser(
            "PUT",
            &format!("/api/agents/{mia_h}/elevate"),
            Some(json!({"access": update_at(&notes)})),
        )
        .await;
    assert_eq!(status, 200);
    let narrow = format!("/api/agents/{mia_h}/narrow");
    let (status, v) = p
        .mias_browser("PUT", &narrow, Some(json!({"access": "clax:reader"})))
        .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!((status, code(&v)), (403, "not_permitted"), "{v}");
    // A narrowing that leaves publishing on leaves the raise: Mia turns
    // only deleting off.
    let no_delete = json!([
        {"type": "clax-artifacts", "actions": ["read", "create"], "locations": ["*"]},
        {"type": "clax-artifacts", "actions": ["update"], "locations": ["own"]},
        {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": ["*"]},
        {"type": "clax-db", "actions": ["read", "write"], "level": "interact", "locations": ["*"]},
        {"type": "clax-ask", "actions": ["ask"]}]);
    let (status, v) = p
        .mias_browser("PUT", &narrow, Some(json!({"access": no_delete})))
        .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 201, "{v}");
    // Read-only again, then no narrowing: the raise is back.
    assert_eq!(
        p.mias_browser("PUT", &narrow, Some(json!({"access": "clax:reader"})))
            .await
            .0,
        200
    );
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 2).await;
    assert_eq!(status, 403);
    assert_eq!(
        p.mias_browser("PUT", &narrow, Some(json!({"access": null})))
            .await
            .0,
        200
    );
    let (status, v) = p.mia_publishes(&p.mia_session, &notes, 2).await;
    assert_eq!(status, 201, "{v}");
}

/// Spec §7.4: a creator's change names the raise it was built on
/// (`if_version`), so a stale panel never brings back a raise that the
/// agent's person or the owner removed: Mia removes Sam's raise, the owner
/// removes his next one, and each time Sam's panel, still showing the old
/// raise, unchecks one box: refused 409 `conflict` with the raise as it
/// stands, nothing written or recorded.
#[tokio::test]
async fn a_stale_creators_change_never_restores_a_removed_raise() {
    let p = people().await;
    let notes = p.sams_notes().await;
    let mia_h = p.handle(&p.mia_session);
    let path = format!("/api/agents/{mia_h}/elevate");
    let sams = format!("{path}?granter={}", p.sam_public);
    let both =
        json!([{"type": "clax-artifacts", "actions": ["update", "delete"], "locations": [notes]}]);
    // Sam's panel believed there was no raise: it raises, and reads its version.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": both, "if_version": null})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let v1 = v["agent"]["version"].as_str().unwrap().to_string();
    // The same panel, built on no raise, cannot raise over the one now there.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": update_at(&notes), "if_version": null})),
        )
        .await;
    assert_eq!((status, code(&v)), (409, "conflict"), "{v}");
    assert_eq!(v["error"]["raise"]["version"], json!(v1), "{v}");
    // Mia removes it; Sam's stale panel (built on v1) unchecks Delete.
    assert_eq!(p.mias_browser("DELETE", &sams, None).await.0, 200);
    let before = p.audited("agent.elevate").len();
    let generation = p.ts.store.visibility_generation();
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": update_at(&notes), "if_version": v1})),
        )
        .await;
    assert_eq!((status, code(&v)), (409, "conflict"), "{v}");
    assert_eq!(
        v["error"]["raise"],
        Value::Null,
        "the raise as it stands: none"
    );
    assert_eq!(p.audited("agent.elevate").len(), before, "nothing recorded");
    assert_eq!(
        p.ts.store.visibility_generation(),
        generation,
        "nothing changed"
    );
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
    let (status, _) = p.mia_publishes(&p.mia_session, &notes, 1).await;
    assert_eq!(status, 403, "still removed");
    // Sam, having looked again (no raise), raises deliberately; the owner
    // removes it; his stale panel's next change is refused the same way.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": both, "if_version": null})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let v2 = v["agent"]["version"].as_str().unwrap().to_string();
    assert_ne!(v2, v1, "a new raise is a new version");
    // His panel, now current, changes it: a new version.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": update_at(&notes), "if_version": v2})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let v3 = v["agent"]["version"].as_str().unwrap().to_string();
    assert_ne!(v3, v2);
    assert_eq!(p.owner("DELETE", &sams, None).await.0, 200);
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": both, "if_version": v3})),
        )
        .await;
    assert_eq!((status, code(&v)), (409, "conflict"), "{v}");
    assert_eq!(p.stored_rights(&p.mia_session), (None, None));
    // The raises listing carries each raise's version for the panel.
    let (status, v) = p
        .sams_browser(
            "PUT",
            &path,
            Some(json!({"access": update_at(&notes), "if_version": null})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let (_, list) = p.sams_browser("GET", "/api/people/me/raises", None).await;
    assert_eq!(
        list["raises"][0]["version"], v["agent"]["version"],
        "{list}"
    );
}

/// A person's agent reaches the daemon at its LAN address; every comment
/// handed to it, by every tier and by the follower's notices, links there,
/// never to the daemon's loopback address, which on the person's machine is
/// their own.
#[tokio::test]
async fn a_persons_agent_gets_links_on_the_address_it_reached() {
    let p = people().await;
    let lee =
        p.ts.connect_person("Lee", "clax:collaborator")
            .await
            .on_lan_address();
    let lan_base = lee.reached_base();
    assert!(lan_base.starts_with("http://192.0.2.1:"), "{lan_base}");
    let sid = id(&lee.register("claude", "lee-h1").await);
    let lee_public =
        p.ts.store
            .viewer_of_grant(&lee.grant_id())
            .unwrap()
            .public_id;
    p.ts.store
        .approve(
            &AuditCtx::DAEMON,
            Some(&lee_public),
            &p.ts.owner_public_id().await,
            Scope::Everywhere,
            Via::Connect,
        )
        .unwrap();
    let a = p.ts.publish("Review", &[("index.html", "<p>v1</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let r = lee
        .agent(
            "PUT",
            &format!("/api/sessions/{sid}/watches/{aid}"),
            Some(&sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    let h = p.ts.store.get_session(&sid).unwrap().unwrap().agent_handle;
    let lan_url = format!("{lan_base}/a/{aid}");
    let loopback = p.ts.base.clone();
    let send = |body: &'static str| {
        let (p, aid, h) = (&p, aid.clone(), h.clone());
        async move {
            let t = p.ts.thread_as_owner(&aid, body).await;
            let tid = t["id"].as_str().unwrap().to_string();
            let (status, v) = p
                .owner(
                    "POST",
                    &format!("/api/artifacts/{aid}/threads/{tid}/send"),
                    Some(json!({"to": h})),
                )
                .await;
            assert_eq!(status, 200, "{v}");
        }
    };
    // The follower's notice (tier 5's announcement) first: a take would
    // hand the row over before it is announced.
    send("a notice").await;
    let (status, v) = json_of(
        lee.agent(
            "GET",
            &format!("/api/sessions/{sid}/notices?wait=0"),
            Some(&sid),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(!v["notices"].as_array().unwrap().is_empty(), "{v}");
    for n in v["notices"].as_array().unwrap() {
        assert_eq!(n["url"], json!(lan_url), "{v}");
    }
    assert!(!v.to_string().contains(&loopback), "{v}");
    for tier in ["wait", "piggyback", "stop_hook", "prompt_hook", "inject"] {
        send("please fix the chart").await;
        let (status, v) = json_of(
            lee.agent(
                "GET",
                &format!("/api/sessions/{sid}/feedback?tier={tier}"),
                Some(&sid),
                None,
            )
            .await,
        )
        .await;
        assert_eq!(status, 200, "{tier}: {v}");
        let items = v["feedback"].as_array().unwrap();
        assert!(!items.is_empty(), "{tier}: {v}");
        for i in items {
            assert_eq!(i["url"], json!(lan_url), "{tier}: {v}");
        }
        assert!(
            v["text"].as_str().unwrap().contains(&lan_url),
            "{tier}: {v}"
        );
        assert!(!v.to_string().contains(&loopback), "{tier}: {v}");
    }
}

/// `id` (a ULID) moved by `by`: a value minted beside it.
fn ulid_beside(id: &str, by: i128) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut n: u128 = id.bytes().fold(0u128, |n, b| {
        n * 32 + ALPHABET.iter().position(|&a| a == b).unwrap() as u128
    });
    n = (n as i128 + by) as u128;
    let mut out = vec![b'0'; 26];
    for c in out.iter_mut().rev() {
        *c = ALPHABET[(n % 32) as usize];
        n /= 32;
    }
    String::from_utf8(out).unwrap()
}

/// `GET /api/people/me` from the LAN with `cookie` (none: no cookie).
async fn people_me_from_lan(ts: &TestServer, cookie: Option<&str>) -> (u16, Value) {
    let (lan, base) = ts.lan();
    let mut r = lan.get(format!("{base}/api/people/me"));
    if let Some(c) = cookie {
        r = r.header("cookie", format!("clax_viewer={c}"));
    }
    json_of(r.send().await.unwrap()).await
}

/// Spec §21.4: a person's browser cookie is a random secret, never a row
/// ID: guessing a neighbour of a row ID the owner sees (an invite's) or
/// presenting the person's row ID grants nothing, the answer to a wrong
/// cookie is the same as to none, and the daemon keeps only the cookie's
/// hash.
#[tokio::test]
async fn a_persons_cookie_is_a_secret_that_no_row_id_reaches() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let res = ts
        .owner_browser()
        .post("/api/people/invites", json!({"name": "Eve"}))
        .await;
    let (status, inv) = json_of(res).await;
    assert_eq!(status, 201, "{inv}");
    let invite_id = inv["invite"]["id"].as_str().unwrap().to_string();
    let code = inv["invite"]["command"]
        .as_str()
        .unwrap()
        .rsplit_once("/i/")
        .unwrap()
        .1
        .to_string();
    let eve = TestClient::new(&ts);
    let r = eve
        .grant_request(json!({"access_token": {"access": ["clax:collaborator"]},
                              "clax": {"invite": code}}))
        .await;
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let (lan, _) = ts.lan();
    let res = lan.get(link).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
    let cookie = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("clax_viewer=")
        .unwrap()
        .to_string();
    let person = ts.store.viewer_of_grant(&eve.grant_id()).unwrap();
    // A secret of its own: `cxv_` and 32 random bytes, no row ID in it.
    assert!(cookie.starts_with("cxv_") && cookie.len() == 47, "{cookie}");
    for id in [&person.id, &invite_id] {
        assert!(!cookie.contains(id.as_str()), "{cookie} holds {id}");
    }
    let (status, me) = people_me_from_lan(&ts, Some(&cookie)).await;
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["person"]["public_id"], json!(person.public_id), "{me}");
    // Nothing near a row ID names Eve: her own row ID, the invite's, and
    // the values minted beside either.
    let none = people_me_from_lan(&ts, None).await;
    assert_eq!(none.0, 403);
    let mut guesses = vec![person.id.clone(), invite_id.clone()];
    for by in [-2, -1, 1, 2] {
        guesses.push(ulid_beside(&invite_id, by));
        guesses.push(ulid_beside(&person.id, by));
    }
    for g in &guesses {
        let got = people_me_from_lan(&ts, Some(g)).await;
        assert_eq!(got, none, "{g} must answer as no cookie does");
        assert!(
            ts.store.get_viewer(g).unwrap().is_none_or(|v| !v.person),
            "{g} names a person"
        );
    }
    // The daemon keeps the cookie's SHA-256, never the cookie.
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let hashes: Vec<String> = db
        .prepare("SELECT sha256 FROM viewer_cookies WHERE viewer_id = ?1")
        .unwrap()
        .query_map([&person.id], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(hashes, [clax_core::secret::sha256_hex(&cookie)]);
    let dump: String = db
        .query_row(
            "SELECT group_concat(sha256 || viewer_id || created_at) FROM viewer_cookies",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!dump.contains(&cookie));
}

/// Every cookie `GET /api/viewers/me` mints for a new browser is a fresh
/// secret, independent of the viewer row it names, and of the cookie
/// minted before it.
#[tokio::test]
async fn an_anonymous_viewers_cookie_is_a_secret_not_its_row_id() {
    let ts = TestServer::spawn().await;
    let a = ts.viewer(None).await;
    let b = ts.viewer(None).await;
    for v in [&a, &b] {
        assert!(
            v.cookie.starts_with("cxv_") && v.cookie.len() == 47,
            "{}",
            v.cookie
        );
        let row = ts.store.get_viewer(&v.cookie).unwrap().unwrap();
        assert_ne!(row.id, v.cookie);
        assert!(
            ts.store.get_viewer(&row.id).unwrap().is_none(),
            "a row ID names no viewer"
        );
    }
    assert_ne!(a.cookie[4..20], b.cookie[4..20]);
}

/// A viewer from before migration 24 holds its row ID as its cookie. It
/// keeps working until its next `GET /api/viewers/me`, which replaces it
/// with a fresh secret naming the same viewer; and a viewer made a person
/// never carries a person's authority on such a cookie.
#[tokio::test]
async fn a_legacy_viewer_cookie_is_replaced_and_never_names_a_person() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    // As a home migrated from main holds it: the row ID's hash, legacy.
    let legacy = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    let lan = ts.store.upsert_viewer(legacy, Some("Lan")).unwrap();
    assert_eq!(
        ts.store.get_viewer(legacy).unwrap().unwrap().public_id,
        lan.public_id
    );
    let (client, base) = ts.lan();
    let res = client
        .get(format!("{base}/api/viewers/me"))
        .header("cookie", format!("clax_viewer={legacy}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
    let fresh = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("clax_viewer=")
        .unwrap()
        .to_string();
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        v["viewer"]["public_id"],
        json!(lan.public_id),
        "the same viewer"
    );
    assert!(fresh.starts_with("cxv_"), "{fresh}");
    assert!(
        ts.store.get_viewer(legacy).unwrap().is_none(),
        "the old cookie names no one"
    );
    assert_eq!(
        ts.store.get_viewer(&fresh).unwrap().unwrap().public_id,
        lan.public_id
    );
    // Another legacy viewer, never re-issued, is made a person: its old
    // cookie names no one from then on.
    let other = "01J9Z3K4M5N6P7Q8R9S0T1V2W4";
    let o = ts.store.upsert_viewer(other, Some("Old")).unwrap();
    ts.store
        .create_invite(
            &clax_core::audit::AuditCtx::DAEMON,
            clax_core::store::people::InvitePerson::Viewer(o.public_id.clone()),
            None,
        )
        .unwrap();
    assert!(ts.store.get_viewer(other).unwrap().is_none());
    let (status, _) = people_me_from_lan(&ts, Some(other)).await;
    assert_eq!(status, 403);
}
