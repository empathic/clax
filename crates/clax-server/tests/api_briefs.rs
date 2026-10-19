//! `POST /api/artifacts:new` on a local daemon: an artifact started from a
//! brief in a person's browser (a placeholder page, the brief its first
//! thread), who may start one, and sending the brief to an agent that does
//! not watch the artifact yet (`POST …/threads/<tid>/send {to}`), which
//! gives the agent its watch as an `@name` does, asking that agent's person
//! first where they have not approved the brief's author.

use clax_core::audit::AuditCtx;
use clax_core::rights::Rights;
use clax_testkit::TestServer;
use serde_json::{Value, json};

const NEW: &str = "/api/artifacts:new";

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// `method <path>` with `body`, from the browser whose `Cookie` header is
/// `cookie` (none: no credential).
async fn call(
    ts: &TestServer,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(method.parse().unwrap(), format!("{}{path}", ts.base))
        .json(&body);
    if let Some(c) = cookie {
        r = r.header("cookie", c);
    }
    json_of(r.send().await.unwrap()).await
}

fn viewer_cookie(c: &str) -> String {
    format!("clax_viewer={c}")
}

fn brief(title: &str) -> Value {
    json!({"title": title, "brief": "A chart of Q3 revenue by region, bars sorted high to low"})
}

/// `GET /api/viewers/me`'s `may_create` for the browser holding `cookie`.
async fn may_create(ts: &TestServer, cookie: &str) -> Value {
    let (s, v) = call(ts, Some(cookie), "GET", "/api/viewers/me", Value::Null).await;
    assert_eq!(s, 200, "{v}");
    v["may_create"].clone()
}

async fn set_rights(ts: &TestServer, pid: &str, rights: Rights) {
    let (st, pid) = (ts.store.clone(), pid.to_string());
    tokio::task::spawn_blocking(move || {
        st.set_person_rights(&AuditCtx::DAEMON, &pid, Some(rights))
            .unwrap()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_owner_starts_an_artifact_from_a_brief_that_waits_for_an_agent() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_cookie();
    assert_eq!(may_create(&ts, &owner).await, json!(true));
    let (s, v) = call(&ts, Some(&owner), "POST", NEW, brief("  Q3 <revenue>  ")).await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["url"], json!(format!("/a/{aid}")));
    assert_eq!(v["artifact"]["title"], "Q3 <revenue>");
    assert_eq!(v["version"]["n"], 1);
    // The placeholder names the title, escaped, and says no agent built it.
    let page = ts
        .get(&format!("/api/artifacts/{aid}/versions/1/files/index.html"))
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("<title>Q3 &lt;revenue&gt;</title>"), "{page}");
    assert!(page.contains("hasn't built this page yet"), "{page}");
    // The brief is the first thread: on the whole artifact, the owner's
    // words, sent to no agent.
    let t = &v["thread"];
    assert_eq!(t["anchor"]["kind"], "artifact", "{t}");
    assert_eq!(
        t["comments"][0]["body"],
        "A chart of Q3 revenue by region, bars sorted high to low"
    );
    assert_eq!(t["comments"][0]["author_kind"], "viewer");
    assert_eq!(
        t["comments"][0]["author_public_id"],
        json!(ts.owner_public_id().await)
    );
    assert_eq!(t["sent_to_agent"], false, "{t}");
    let (_, threads) = json_of(ts.get(&format!("/api/artifacts/{aid}/threads")).await).await;
    assert_eq!(threads["threads"].as_array().unwrap().len(), 1, "{threads}");
    // No session made it, and none watches it.
    let (_, got) = json_of(ts.get_authed(&format!("/api/artifacts/{aid}")).await).await;
    assert_eq!(got["artifact"]["owner_session_id"], Value::Null, "{got}");
    // The token by hand starts one too.
    let (s, v) = json_of(ts.post_json(NEW, brief("By hand")).await).await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn a_title_and_a_brief_are_required_and_bounded() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_cookie();
    for (body, want) in [
        (json!({"title": " ", "brief": "x"}), "invalid_title"),
        (json!({"title": "a\nb", "brief": "x"}), "invalid_title"),
        (
            json!({"title": "x".repeat(201), "brief": "x"}),
            "invalid_title",
        ),
        (json!({"title": "Plan", "brief": " \n "}), "invalid_brief"),
        (
            json!({"title": "Plan", "brief": "x".repeat(10_001)}),
            "invalid_brief",
        ),
        (json!({"title": "Plan"}), "invalid_json"),
    ] {
        let (s, v) = call(&ts, Some(&owner), "POST", NEW, body.clone()).await;
        assert_eq!((s, code(&v)), (400, want), "{body}: {v}");
    }
    let (_, list) = json_of(ts.get_authed("/api/artifacts").await).await;
    assert_eq!(
        list["artifacts"],
        json!([]),
        "a refused brief makes nothing"
    );
}

#[tokio::test]
async fn a_connected_person_starts_their_own_and_a_guest_or_a_reader_may_not() {
    let ts = TestServer::spawn().await;
    let rae = ts.person("Rae").await;
    let cookie = viewer_cookie(&rae.cookie);
    assert_eq!(may_create(&ts, &cookie).await, json!(true));
    let (s, v) = call(&ts, Some(&cookie), "POST", NEW, brief("Rae's chart")).await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (st, a, pid) = (ts.store.clone(), aid.clone(), rae.public_id.clone());
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&a, Some(&pid)))
            .await
            .unwrap()
            .unwrap(),
        "the artifact is Rae's"
    );
    assert_eq!(
        v["thread"]["comments"][0]["author_public_id"],
        json!(rae.public_id)
    );
    assert_eq!(v["thread"]["comments"][0]["author_name"], "Rae");
    // A reader creates nothing, and is offered no New.
    set_rights(&ts, &rae.public_id, Rights::preset("clax:reader").unwrap()).await;
    assert_eq!(may_create(&ts, &cookie).await, json!(false));
    let (s, v) = call(&ts, Some(&cookie), "POST", NEW, brief("Nope")).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // Rights that create but read only artifacts named by ID would make an
    // artifact she could not open.
    let named = json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [aid]},
        {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
        {"type": "clax-comments", "actions": ["write"], "locations": ["*"]},
    ]);
    set_rights(&ts, &rae.public_id, Rights::parse(&named).unwrap()).await;
    let (s, v) = call(&ts, Some(&cookie), "POST", NEW, brief("Nope")).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // A guest: a named viewer who is no connected person.
    let gil = ts.viewer(Some("Gil")).await;
    let gil = viewer_cookie(&gil.cookie);
    assert_eq!(may_create(&ts, &gil).await, json!(false));
    let (s, v) = call(&ts, Some(&gil), "POST", NEW, brief("Nope")).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // No credential.
    let (s, v) = call(&ts, None, "POST", NEW, brief("Nope")).await;
    assert_eq!((s, code(&v)), (401, "unauthorized"), "{v}");
    // Another site's page in the owner's browser.
    let res = ts
        .client
        .post(format!("{}{NEW}", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", "https://evil.example")
        .json(&brief("Nope"))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    let res = ts
        .client
        .post(format!("{}{NEW}", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("sec-fetch-site", "cross-site")
        .json(&brief("Nope"))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    // The owner's agents publish instead.
    let sid = ts.register_session("claude", "brief-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(ts.client.post(format!("{}{NEW}", ts.base)))
        .header("x-clax-session", &sid)
        .json(&brief("Nope"))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    let (_, list) = json_of(ts.get_authed("/api/artifacts").await).await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1, "{list}");
}

/// A daemon reachable from the LAN with Sam connected (a collaborator, with
/// a live agent), an owner's live agent, and an artifact the owner started
/// from a brief: `(ts, sam, sam's session, owner's session, aid, tid)`.
async fn briefed() -> (
    TestServer,
    clax_testkit::TestClient,
    String,
    String,
    String,
    String,
) {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sam_sid = sam.register("claude", "sam-brief").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner_sid = ts.register_session("claude", "owner-brief").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = call(&ts, Some(&ts.owner_cookie()), "POST", NEW, brief("Revenue")).await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let tid = v["thread"]["id"].as_str().unwrap().to_string();
    (ts, sam, sam_sid, owner_sid, aid, tid)
}

fn handle_of(ts: &TestServer, sid: &str) -> String {
    ts.store.get_session(sid).unwrap().unwrap().agent_handle
}

fn name_of(ts: &TestServer, sid: &str) -> String {
    ts.store.get_session(sid).unwrap().unwrap().name.unwrap()
}

/// Whether session `sid` watches `aid`.
fn watches(ts: &TestServer, sid: &str, aid: &str) -> bool {
    ts.store
        .list_watches(sid)
        .unwrap()
        .iter()
        .any(|w| w.artifact_id == aid)
}

#[tokio::test]
async fn sending_the_brief_to_the_owners_agent_gives_it_a_watch_and_delivers_the_brief() {
    let (ts, _sam, _sam_sid, owner_sid, aid, tid) = briefed().await;
    assert!(!watches(&ts, &owner_sid, &aid));
    let send = format!("/api/artifacts/{aid}/threads/{tid}/send");
    let to = json!({"to": handle_of(&ts, &owner_sid)});
    // A guest cannot give an agent a watch by naming it: only a watcher.
    let gil = ts.viewer(Some("Gil")).await;
    let (s, v) = call(
        &ts,
        Some(&viewer_cookie(&gil.cookie)),
        "POST",
        &send,
        to.clone(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "unknown_agent"), "{v}");
    // Nor can an agent channel: agents hand threads off.
    let res = ts
        .authed(ts.client.post(format!("{}{send}", ts.base)))
        .header("x-clax-session", &owner_sid)
        .json(&to)
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (400, "unknown_agent"), "{v}");
    assert!(!watches(&ts, &owner_sid, &aid));
    // The owner's browser picks the agent.
    let (s, v) = call(&ts, Some(&ts.owner_cookie()), "POST", &send, to).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["thread"]["sent_to_agent"], true, "{v}");
    assert!(watches(&ts, &owner_sid, &aid), "the pick gives it a watch");
    let res = ts
        .get_authed(&format!("/api/sessions/{owner_sid}/feedback"))
        .await;
    let (s, fb) = json_of(res).await;
    assert_eq!(s, 200, "{fb}");
    assert!(
        fb.to_string()
            .contains("A chart of Q3 revenue by region, bars sorted high to low"),
        "the agent receives the brief: {fb}"
    );
}

#[tokio::test]
async fn sending_the_brief_to_another_persons_agent_asks_that_person_first() {
    let (ts, sam, sam_sid, _owner_sid, aid, tid) = briefed().await;
    let send = format!("/api/artifacts/{aid}/threads/{tid}/send");
    // By name, as the gallery lists another person's agent.
    let (s, v) = call(
        &ts,
        Some(&ts.owner_cookie()),
        "POST",
        &send,
        json!({"to": name_of(&ts, &sam_sid)}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(watches(&ts, &sam_sid, &aid), "Sam's agent watches it");
    assert!(
        v["thread"]["feedback_state"]
            .to_string()
            .contains("withheld"),
        "Sam approves first: {v}"
    );
    // Sam's agent is told the thread waits, never its words.
    let r = sam
        .agent(
            "GET",
            &format!("/api/sessions/{sam_sid}/feedback"),
            Some(&sam_sid),
            None,
        )
        .await;
    let (s, fb) = json_of(r).await;
    assert_eq!(s, 200, "{fb}");
    assert!(!fb.to_string().contains("Q3 revenue"), "{fb}");
    // An unknown name gives nothing a watch.
    let (s, v) = call(
        &ts,
        Some(&ts.owner_cookie()),
        "POST",
        &send,
        json!({"to": "nobody-here"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "unknown_agent"), "{v}");
}
