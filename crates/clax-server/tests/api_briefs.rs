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
    assert_eq!((s, code(&v)), (401, "no_credential"), "{v}");
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
    // The owner's session with another origin is no credential at all
    // (`local_trust::session_counts`): the gate refuses it before any route.
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (401, "no_credential"), "{v}");
    let res = ts
        .client
        .post(format!("{}{NEW}", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("sec-fetch-site", "cross-site")
        .json(&brief("Nope"))
        .send()
        .await
        .unwrap();
    // Another site's fetch with the owner's session is no credential at
    // all: the gate refuses it before any route.
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (401, "no_credential"), "{v}");
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

/// `call` with extra request headers.
async fn call_with(
    ts: &TestServer,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(method.parse().unwrap(), format!("{}{path}", ts.base))
        .json(&body);
    if let Some(c) = cookie {
        r = r.header("cookie", c);
    }
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    json_of(r.send().await.unwrap()).await
}

/// The names `GET /api/agents?artifact=<aid>` lists to the browser holding
/// `cookie`.
async fn attachable(ts: &TestServer, cookie: &str, aid: &str) -> Vec<String> {
    let (s, v) = call(
        ts,
        Some(cookie),
        "GET",
        &format!("/api/agents?artifact={aid}"),
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let mut names: Vec<String> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        // The sessions alone: ClaxBot, built in, is listed where ClaxBot is set up.
        .filter(|a| a["builtin"] != true)
        .map(|a| a["name"].as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// A person connected with `preset`, whose rights are then `rights`, with a
/// live agent: their machine and the agent's session.
async fn narrowed(
    ts: &TestServer,
    name: &str,
    rights: Value,
) -> (clax_testkit::TestClient, String) {
    let c = ts.connect_person(name, "clax:collaborator").await;
    let sid = c.register("claude", &format!("{name}-brief")).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pid = ts.store.viewer_of_grant(&c.grant_id()).unwrap().public_id;
    set_rights(ts, &pid, Rights::parse(&rights).unwrap()).await;
    (c, sid)
}

#[tokio::test]
async fn the_dialog_lists_and_a_send_reaches_only_agents_that_may_read_and_watch() {
    let (ts, _sam, sam_sid, owner_sid, aid, tid) = briefed().await;
    let owner = ts.owner_cookie();
    // Lee's agent may read only another artifact; Kim's may read this one
    // but not watch it.
    let other = ts.publish("Other", &[("index.html", "<p>x")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_lee, lee_sid) = narrowed(
        &ts,
        "Lee",
        json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [other]}]),
    )
    .await;
    let (_kim, kim_sid) = narrowed(
        &ts,
        "Kim",
        json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read"], "locations": ["*"]},
        ]),
    )
    .await;
    let mut want = vec![name_of(&ts, &owner_sid), name_of(&ts, &sam_sid)];
    want.sort();
    assert_eq!(attachable(&ts, &owner, &aid).await, want);
    let send = format!("/api/artifacts/{aid}/threads/{tid}/send");
    // The send refuses each left out, and gives it no watch.
    for (sid, want) in [(&lee_sid, "agent_cannot_read"), (&kim_sid, "unknown_agent")] {
        let (s, v) = call(
            &ts,
            Some(&owner),
            "POST",
            &send,
            json!({"to": name_of(&ts, sid)}),
        )
        .await;
        assert_eq!((s, code(&v)), (400, want), "{v}");
        assert!(!watches(&ts, sid, &aid), "{want}: no watch");
    }
    // A guest lists no agents; a reader, who may not send, may name none.
    let gil = ts.viewer(Some("Gil")).await;
    let (s, v) = call(
        &ts,
        Some(&viewer_cookie(&gil.cookie)),
        "GET",
        &format!("/api/agents?artifact={aid}"),
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let rae = ts.person("Rae").await;
    set_rights(&ts, &rae.public_id, Rights::preset("clax:reader").unwrap()).await;
    assert_eq!(
        attachable(&ts, &viewer_cookie(&rae.cookie), &aid).await,
        Vec::<String>::new()
    );
    // A resolved thread is refused first, and gives no watch.
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/threads/{tid}/resolve"),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    let (s, v) = call(
        &ts,
        Some(&owner),
        "POST",
        &send,
        json!({"to": name_of(&ts, &owner_sid)}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "thread_resolved"), "{v}");
    assert!(!watches(&ts, &owner_sid, &aid));
}

#[tokio::test]
async fn no_persons_agent_reaches_a_live_page_by_a_send() {
    let (ts, _sam, sam_sid, _owner_sid, _aid, _tid) = briefed().await;
    let v = ts.viewer(Some("Alex")).await;
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Settings")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "main", "quote": "Save", "file": "index.html"})
                .to_string(),
        )
        .text("body", "The button overflows")
        .text("pending", "[]")
        .text("snapshot", "<p>a live page</p>");
    let r = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let (s, made) = json_of(r).await;
    assert_eq!(s, 201, "{made}");
    let hid = made["page"]["artifact_id"].as_str().unwrap().to_string();
    let htid = made["thread"]["id"].as_str().unwrap().to_string();
    // The owner names Sam's agent, which a send there never reaches.
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{hid}/threads/{htid}/send",
            ts.base
        )))
        .json(&json!({"to": name_of(&ts, &sam_sid)}))
        .send()
        .await
        .unwrap();
    let (s2, v2) = json_of(res).await;
    assert_eq!((s2, code(&v2)), (400, "unknown_agent"), "{v2}");
    assert!(!watches(&ts, &sam_sid, &hid));
    // The dialog's listing names the live page to the owner alone: to a
    // connected person it does not exist.
    let (s3, v3) = call(
        &ts,
        Some(&ts.owner_cookie()),
        "GET",
        &format!("/api/agents?artifact={hid}"),
        Value::Null,
    )
    .await;
    assert_eq!(s3, 200, "{v3}");
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let mia_viewer = ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let cookie = viewer_cookie(&ts.browser_cookie(&mia_viewer.id));
    let (s4, v4) = call(
        &ts,
        Some(&cookie),
        "GET",
        &format!("/api/agents?artifact={hid}"),
        Value::Null,
    )
    .await;
    assert_eq!((s4, code(&v4)), (404, "not_found"), "{v4}");
}

#[tokio::test]
async fn a_connected_person_attaches_another_persons_agent_who_approves_first() {
    let (ts, sam, sam_sid, _owner_sid, _aid, _tid) = briefed().await;
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let mia_viewer = ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let cookie = viewer_cookie(&ts.browser_cookie(&mia_viewer.id));
    let (s, v) = call(&ts, Some(&cookie), "POST", NEW, brief("Mia's plan")).await;
    assert_eq!(s, 201, "{v}");
    let (aid, tid) = (
        v["artifact"]["id"].as_str().unwrap().to_string(),
        v["thread"]["id"].as_str().unwrap().to_string(),
    );
    assert!(
        attachable(&ts, &cookie, &aid)
            .await
            .contains(&name_of(&ts, &sam_sid))
    );
    let (s, v) = call(
        &ts,
        Some(&cookie),
        "POST",
        &format!("/api/artifacts/{aid}/threads/{tid}/send"),
        json!({"to": name_of(&ts, &sam_sid)}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(watches(&ts, &sam_sid, &aid));
    assert!(
        v["thread"]["feedback_state"]
            .to_string()
            .contains("withheld"),
        "Sam approves first: {v}"
    );
    // An agent of another machine starts nothing from a brief.
    let r = sam
        .agent("POST", NEW, Some(&sam_sid), Some(brief("By Sam's agent")))
        .await;
    let (s, v) = json_of(r).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
}

#[tokio::test]
async fn a_repeat_with_its_key_answers_the_first_artifact() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_cookie();
    let key = [("idempotency-key", "k-0001")];
    let (s, first) = call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key).await;
    assert_eq!(s, 201, "{first}");
    let (s, again) = call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key).await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["artifact"]["id"], first["artifact"]["id"]);
    assert_eq!(again["thread"]["id"], first["thread"]["id"]);
    assert_eq!(again["url"], first["url"]);
    // Another key, or another person's same key, makes another.
    let (s, other) = call_with(
        &ts,
        Some(&owner),
        "POST",
        NEW,
        brief("Twice"),
        &[("idempotency-key", "k-0002")],
    )
    .await;
    assert_eq!(s, 201, "{other}");
    assert_ne!(other["artifact"]["id"], first["artifact"]["id"]);
    let rae = ts.person("Rae").await;
    let (s, hers) = call_with(
        &ts,
        Some(&viewer_cookie(&rae.cookie)),
        "POST",
        NEW,
        brief("Rae's"),
        &key,
    )
    .await;
    assert_eq!(s, 201, "{hers}");
    assert_ne!(hers["artifact"]["id"], first["artifact"]["id"]);
    let long = "k".repeat(129);
    let (s, v) = call_with(
        &ts,
        Some(&owner),
        "POST",
        NEW,
        brief("Bad"),
        &[("idempotency-key", long.as_str())],
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (_, list) = json_of(ts.get_authed("/api/artifacts").await).await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 3, "{list}");
    // Once its artifact is deleted, the key makes a new one, which says who
    // started it, and a repeat answers the new one.
    let first_id = first["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{first_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());
    let (s, renewed) = call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key).await;
    assert_eq!(s, 201, "{renewed}");
    assert_ne!(renewed["artifact"]["id"], first["artifact"]["id"]);
    assert!(renewed["artifact"]["brief"].is_object(), "{renewed}");
    let (s, again) = call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key).await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["artifact"]["id"], renewed["artifact"]["id"]);
}

#[tokio::test]
async fn the_same_keyed_brief_sent_twice_at_once_makes_one_artifact() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_cookie();
    let key = [("idempotency-key", "k-race")];
    let (a, b) = tokio::join!(
        call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key),
        call_with(&ts, Some(&owner), "POST", NEW, brief("Once"), &key),
    );
    let mut statuses = [a.0, b.0];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 201], "{} {}", a.1, b.1);
    assert_eq!(a.1["artifact"]["id"], b.1["artifact"]["id"]);
    assert_eq!(a.1["thread"]["id"], b.1["thread"]["id"]);
    let (_, list) = json_of(ts.get_authed("/api/artifacts").await).await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1, "{list}");
}

#[tokio::test]
async fn an_artifact_started_from_a_brief_names_who_started_it_to_people() {
    let ts = TestServer::spawn().await;
    let rae = ts.person("Rae").await;
    let cookie = viewer_cookie(&rae.cookie);
    // A title may join letters and emoji (ZWNJ, ZWJ).
    let (s, v) = call(&ts, Some(&cookie), "POST", NEW, brief("می\u{200c}خواهم")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["artifact"]["brief"], json!({"by": "Rae"}));
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (_, got) = call(
        &ts,
        Some(&cookie),
        "GET",
        &format!("/api/artifacts/{aid}"),
        Value::Null,
    )
    .await;
    assert_eq!(got["artifact"]["brief"], json!({"by": "Rae"}), "{got}");
    let (_, list) = call(
        &ts,
        Some(&ts.owner_cookie()),
        "GET",
        "/api/artifacts",
        Value::Null,
    )
    .await;
    assert_eq!(
        list["artifacts"][0]["brief"],
        json!({"by": "Rae"}),
        "{list}"
    );
    let page = ts.get(&format!("/a/{aid}")).await.text().await.unwrap();
    assert!(
        page.contains("\"brief\":{\"by\":\"Rae\"}"),
        "the page's bootstrap"
    );
    // Agents are not told.
    let sid = ts.register_session("claude", "brief-by").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(ts.client.get(format!("{}/api/artifacts/{aid}", ts.base)))
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    let (_, got) = json_of(res).await;
    assert!(got["artifact"].get("brief").is_none(), "{got}");
    // An artifact an agent published names none.
    let p = ts.publish("Plain", &[("index.html", "<p>x")]).await;
    let pid = p["artifact"]["id"].as_str().unwrap();
    let (_, got) = call(
        &ts,
        Some(&cookie),
        "GET",
        &format!("/api/artifacts/{pid}"),
        Value::Null,
    )
    .await;
    assert!(got["artifact"].get("brief").is_none(), "{got}");
}
