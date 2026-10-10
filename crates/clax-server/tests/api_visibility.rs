//! What agents see of threads (spec §7.5 thread rows, §8.1–§8.3, M10):
//! each field on its own, in reads and write responses alike, with the
//! placeholders of §8.3; a withheld clip is not fetchable; a person's
//! agent writes comments from a session only, as its person; people are
//! never filtered.

use crate::common::TestServer;
use crate::common::people::{
    ASSET_BYTES, BATCH_NOTE, CAPABILITY, CARRIED_BYTES, CARRIED_NAME, CLIP, COMMENT, DESCRIPTION,
    FILE_BYTES, FILE_NAME, ICON, LABEL, NOTE, PREFIX, QUOTE, REPLY, SELECTOR, SUFFIX, TITLE,
    WORKING, World,
};
use clax_core::audit::AuditCtx;
use clax_core::store::approvals::{Scope, Via};
use clax_server::testing::{Call, FAKE_PNG, TestClient};
use serde_json::{Value, json};

/// Mia's canaries a thread view can carry.
const THREAD_CANARIES: &[&str] = &[
    COMMENT, REPLY, QUOTE, PREFIX, SUFFIX, SELECTOR, CLIP, BATCH_NOTE,
];

fn sam_public(w: &World) -> String {
    w.ts.store
        .viewer_of_grant(&w.sam.grant_id())
        .unwrap()
        .public_id
}

fn owner_public(ts: &TestServer) -> String {
    ts.store.owner_viewer(true).unwrap().public_id
}

/// `approver` (`None`: the owner) approves `author` everywhere.
fn approve(ts: &TestServer, approver: Option<&str>, author: &str) {
    ts.store
        .approve(
            &AuditCtx::DAEMON,
            approver,
            author,
            Scope::Everywhere,
            Via::Connect,
        )
        .unwrap();
}

fn no_canary(what: &str, text: &str) {
    for c in THREAD_CANARIES {
        assert!(!text.contains(c), "{what} leaked {c}: {text}");
    }
}

/// A multipart form of `fields` (name, bytes, file name): its content type
/// and body.
fn multipart(fields: &[(&str, &[u8], Option<&str>)]) -> (String, Vec<u8>) {
    let boundary = "clax-test-boundary-7f3a";
    let mut body = Vec::new();
    for (name, bytes, file) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(f) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: image/png\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn anchor_on(file: &str, quote: &str) -> String {
    json!({"kind": "element", "selector": "#goals", "quote": quote, "prefix": "", "suffix": "",
           "html_hash": "sha256:00", "rect": null, "custom_name": null, "file": file})
    .to_string()
}

/// A thread the owner starts by hand (the token alone) on version `n` of
/// `aid`, anchored on `file` with a clip of its own; its ID.
async fn owner_thread(ts: &TestServer, aid: &str, n: u32, file: &str, body: &str) -> String {
    let mut png = FAKE_PNG.to_vec();
    png.extend_from_slice(b"OWNERCLIP");
    let form = reqwest::multipart::Form::new()
        .text("anchor", anchor_on(file, "Quarterly goals"))
        .text("body", body.to_string())
        .text("version", n.to_string())
        .part(
            "clip",
            reqwest::multipart::Part::bytes(png)
                .file_name("clip.png")
                .mime_str("image/png")
                .unwrap(),
        );
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/threads", ts.base)),
        )
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    r.json::<Value>().await.unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// `method path` as the person's agent `c`, naming `session` (if any),
/// with a multipart form: status and JSON body.
async fn agent_form(
    c: &TestClient,
    path: &str,
    session: Option<&str>,
    fields: &[(&str, &[u8], Option<&str>)],
) -> (u16, Value) {
    let (ct, body) = multipart(fields);
    let mut headers = vec![("content-type", ct)];
    if let Some(s) = session {
        headers.push(("x-clax-session", s.to_string()));
    }
    let r = c
        .call(Call {
            method: "POST",
            path,
            authorization: Some(format!("Clax-Grant {}", c.access())),
            body: Some(body),
            headers,
            ..Default::default()
        })
        .await;
    (r.status().as_u16(), r.json().await.unwrap_or_default())
}

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or_default())
}

/// The owner's agent (the token with the agent-channel marker and a
/// session): `method path` with JSON `body`.
async fn owner_agent(w: &World, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let ts = &w.ts;
    let mut req = ts
        .authed(ts.client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", ts.base),
        ))
        .header("x-clax-channel", "agent")
        .header("x-clax-session", &w.owner_session);
    if let Some(b) = body {
        req = req.json(&b);
    }
    json_of(req.send().await.unwrap()).await
}

#[tokio::test]
async fn visible_comment_on_withheld_page_hides_quote_and_title() {
    let w = World::two_people().await;
    // Sam approved the owner at connect, not Mia.
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = &w.read.artifact;
    // The owner comments on Mia's page (version 1's index.html is hers).
    let tid = owner_thread(&w.ts, aid, 1, "index.html", "please tighten").await;
    let (status, v) = json_of(
        w.sam
            .get(
                Some(&w.sam_session),
                &format!("/api/artifacts/{aid}/threads/{tid}"),
            )
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let t = &v["thread"];
    assert_eq!(t["comments"][0]["body"], "please tighten");
    assert!(t["comments"][0].get("withheld").is_none(), "{t}");
    assert!(
        t["anchor"]["quote"].is_null() && t["anchor_withheld"] == true,
        "{t}"
    );
    assert!(
        t["anchor"]["selector"].is_null() && t["anchor"]["file"].is_null(),
        "{t}"
    );
    assert_eq!(t["clip_withheld"], true);
    assert_eq!(
        (t["has_clip"].clone(), t["clip_url"].clone()),
        (json!(false), Value::Null)
    );
    assert_eq!(t["withheld"]["authors"][0]["name"], "Mia");
    assert_eq!(
        t["withheld"]["authors"][0]["public_id"],
        json!(w.mia_public)
    );
    assert_eq!(t["withheld"]["count"], 0, "no comment is withheld: {t}");
    assert_eq!(t["withheld"]["approval"], "none");
    // The clip itself is not fetchable.
    let r = w
        .sam
        .get(
            Some(&w.sam_session),
            &format!("/api/artifacts/{aid}/threads/{tid}/clip"),
        )
        .await;
    assert_eq!(r.status(), 403);
    let text = r.text().await.unwrap();
    assert!(
        text.contains("\"withheld\"") && text.contains("Mia"),
        "{text}"
    );
    assert!(!text.contains("OWNERCLIP"), "{text}");
    // The listing too, and Mia's own thread there is all placeholders.
    let r = w
        .sam
        .get(
            Some(&w.sam_session),
            &format!("/api/artifacts/{aid}/threads?include_resolved=true"),
        )
        .await;
    let (status, list) = json_of(r).await;
    assert_eq!(status, 200);
    no_canary("the listing", &list.to_string());
    let mias = list["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(w.read.thread))
        .unwrap();
    assert_eq!(mias["comments"][0]["withheld"], true);
    assert_eq!(mias["comments"][0]["body"], Value::Null);
    assert_eq!(mias["comments"][0]["author_name"], "Mia", "names stay");
    assert_eq!(
        mias["withheld"]["count"], 2,
        "her comment and her reply: {mias}"
    );
    assert!(mias["sends"][0]["note"].is_null(), "{mias}");
    let resolved = list["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(w.read.resolved))
        .unwrap();
    assert!(resolved["resolved_by"].is_null() && resolved["resolved_by_name"].is_null());
    // The owner's agent lists every thread: the title and file names of
    // Mia's page are withheld, the owner's own comment is not.
    let (status, all) = owner_agent(&w, "GET", "/api/threads?include_resolved=true", None).await;
    assert_eq!(status, 200);
    let entry = all["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["artifact_id"] == json!(aid))
        .unwrap();
    assert!(entry["title"].is_null(), "{entry}");
    assert_eq!(entry["withheld"]["authors"][0]["name"], "Mia");
    assert_eq!(entry["files"], json!(["owner.html"]));
    assert_eq!(entry["files_withheld"], 3);
    assert!(!all.to_string().contains(FILE_NAME), "{all}");
    no_canary("the owner's agent's listing", &all.to_string());
    let mine = entry["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(tid))
        .unwrap();
    assert_eq!(mine["comments"][0]["body"], "please tighten");
}

#[tokio::test]
async fn write_routes_return_filtered_threads() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.scratch.artifact, &w.scratch.thread);
    let sid = &w.sam_session;
    let base = format!("/api/artifacts/{aid}/threads/{tid}");
    // A reply on a thread holding Mia's comment.
    let r = w
        .sam
        .agent(
            "POST",
            &format!("{base}/comments"),
            Some(sid),
            Some(json!({"body": "on it", "author_kind": "agent"})),
        )
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 201, "{v}");
    no_canary("a reply's answer", &v.to_string());
    assert_eq!(v["comment"]["body"], "on it");
    let comments = v["thread"]["comments"].as_array().unwrap();
    assert_eq!(comments[0]["body"], Value::Null);
    assert_eq!(comments[0]["withheld"], true);
    assert_eq!(comments.last().unwrap()["body"], "on it", "its own reply");
    assert_eq!(v["thread"]["withheld"]["authors"][0]["name"], "Mia");
    // Resolve and reopen likewise.
    for (path, verb) in [("resolve", "resolve"), ("reopen", "reopen")] {
        let r = w
            .sam
            .agent(
                "POST",
                &format!("{base}/{path}"),
                Some(sid),
                Some(json!({"as": "agent"})),
            )
            .await;
        let (status, v) = json_of(r).await;
        assert_eq!(status, 200, "{verb}: {v}");
        no_canary(verb, &v.to_string());
        assert_eq!(v["thread"]["comments"][0]["withheld"], true, "{verb}");
    }
    // A send's answer likewise.
    let (status, v) = json_of(
        w.sam
            .agent("POST", &format!("{base}/send"), Some(sid), None)
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    no_canary("a send", &v.to_string());
    assert_eq!(v["thread"]["comments"][0]["withheld"], true);
    // A batch: its own note is its own, and the thread it sends, on Mia's
    // page, keeps her anchor withheld.
    let fresh = owner_thread(&w.ts, aid, 1, "index.html", "the owner asks").await;
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/artifacts/{aid}/threads:send"),
            Some(sid),
            Some(json!({"thread_ids": [fresh], "note": "from Sam's agent"})),
        )
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 200, "{v}");
    no_canary("a batch", &v.to_string());
    assert_eq!(v["threads"][0]["sends"][0]["note"], "from Sam's agent");
    assert_eq!(v["threads"][0]["anchor_withheld"], true);
    assert_eq!(v["threads"][0]["clip_withheld"], true);
    // A new thread: its comment is Sam's, its anchor on Mia's file is hers.
    let (status, v) = agent_form(
        &w.sam,
        &format!("/api/artifacts/{aid}/threads"),
        Some(sid),
        &[
            (
                "anchor",
                anchor_on("index.html", "Quarterly goals").as_bytes(),
                None,
            ),
            ("body", b"a thread of Sam's agent", None),
            ("version", b"1", None),
        ],
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(
        v["thread"]["comments"][0]["body"],
        "a thread of Sam's agent"
    );
    assert_eq!(
        v["thread"]["comments"][0]["author_public_id"],
        json!(sam_public(&w))
    );
    assert_eq!(v["thread"]["anchor_withheld"], true);
    assert!(v["thread"]["anchor"]["quote"].is_null());
    no_canary("a new thread", &v.to_string());
}

#[tokio::test]
async fn owner_agent_sees_placeholder_for_page_written_comment() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.read.artifact, &w.read.thread);
    // Mia's page, running in the owner's browser, writes a comment.
    let r =
        w.ts.owner_browser()
            .post(
                &format!("/api/artifacts/{aid}/threads/{tid}/comments"),
                json!({"body": "PAGEWROTEthis", "via_page": true}),
            )
            .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    // The owner, by hand, writes another.
    let r =
        w.ts.post_json(
            &format!("/api/artifacts/{aid}/threads/{tid}/comments"),
            json!({"body": "the owner's own words"}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let (status, v) = owner_agent(
        &w,
        "GET",
        &format!("/api/artifacts/{aid}/threads/{tid}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let text = v.to_string();
    assert!(!text.contains("PAGEWROTEthis"), "{text}");
    no_canary("the owner's agent", &text);
    let comments = v["thread"]["comments"].as_array().unwrap();
    let page = comments.iter().find(|c| c["via_page"] == true).unwrap();
    assert_eq!(
        (page["body"].clone(), page["withheld"].clone()),
        (Value::Null, json!(true))
    );
    assert!(
        comments
            .iter()
            .any(|c| c["body"] == "the owner's own words"),
        "{comments:?}"
    );
    // Once the owner approves Mia, her page's words and her own reach the
    // owner's agents.
    approve(&w.ts, None, &w.mia_public);
    let (_, v) = owner_agent(
        &w,
        "GET",
        &format!("/api/artifacts/{aid}/threads/{tid}"),
        None,
    )
    .await;
    let text = v.to_string();
    for c in ["PAGEWROTEthis", COMMENT, REPLY, QUOTE] {
        assert!(text.contains(c), "{c}: {text}");
    }
    assert!(v["thread"].get("withheld").is_none(), "{v}");
}

#[tokio::test]
async fn people_are_never_filtered() {
    let w = World::two_people().await;
    let ts = &w.ts;
    let aid = &w.read.artifact;
    let list = format!("/api/artifacts/{aid}/threads?include_resolved=true");
    let clip = format!("/api/artifacts/{aid}/threads/{}/clip", w.read.thread);
    let sam_cookie = ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap().id;
    let lan = ts.viewer(Some("Lan")).await;
    let browsers: Vec<(&str, Option<String>)> = vec![
        ("Sam's browser", Some(format!("clax_viewer={sam_cookie}"))),
        ("a LAN viewer", Some(format!("clax_viewer={}", lan.cookie))),
        ("the owner's browser", Some(ts.owner_cookie())),
    ];
    for (who, cookie) in browsers {
        for (path, want) in [
            (&list, vec![COMMENT, REPLY, QUOTE, SELECTOR]),
            (&clip, vec![CLIP]),
        ] {
            let mut req = ts.client.get(format!("{}{path}", ts.base));
            if let Some(c) = &cookie {
                req = req.header("cookie", c);
            }
            let r = req.send().await.unwrap();
            assert_eq!(r.status(), 200, "{who} {path}");
            let body = String::from_utf8_lossy(&r.bytes().await.unwrap()).into_owned();
            for c in want {
                assert!(body.contains(c), "{who} {path}: {c}");
            }
        }
    }
    // The CLI by hand: the token alone.
    let body = ts.get_authed(&list).await.text().await.unwrap();
    assert!(body.contains(COMMENT) && body.contains(QUOTE));
    let all = ts
        .get_authed("/api/threads?include_resolved=true")
        .await
        .text()
        .await
        .unwrap();
    assert!(all.contains(crate::common::people::TITLE) && all.contains(FILE_NAME));
}

#[tokio::test]
async fn a_persons_agent_comments_from_a_session_as_its_person() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.scratch.artifact, &w.scratch.thread);
    let path = format!("/api/artifacts/{aid}/threads/{tid}/comments");
    // No session: refused, whatever the kind, before anything is written.
    for body in [
        json!({"body": "x", "author_kind": "agent"}),
        json!({"body": "x"}),
        json!({"body": "x", "author_kind": "viewer"}),
    ] {
        let (status, v) = json_of(w.sam.agent("POST", &path, None, Some(body.clone())).await).await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (400, json!("no_session")),
            "{body}: {v}"
        );
    }
    let (status, v) = agent_form(
        &w.sam,
        &format!("/api/artifacts/{aid}/threads"),
        None,
        &[
            ("anchor", anchor_on("index.html", "q").as_bytes(), None),
            ("body", b"x", None),
            ("version", b"1", None),
        ],
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("no_session")),
        "{v}"
    );
    // A page writes through a browser, never through a grant.
    let (status, v) = json_of(
        w.sam
            .agent(
                "POST",
                &path,
                Some(&w.sam_session),
                Some(json!({"body": "x", "via_page": true})),
            )
            .await,
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("invalid_via_page")),
        "{v}"
    );
    // From its session, a viewer comment is its person's words.
    let (status, v) = json_of(
        w.sam
            .agent(
                "POST",
                &path,
                Some(&w.sam_session),
                Some(json!({"body": "Sam's agent, as Sam"})),
            )
            .await,
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["comment"]["author_public_id"], json!(sam_public(&w)));
    assert_eq!(v["comment"]["author_name"], "Sam");
    // So it reaches the owner's agents only once the owner approves Sam,
    // not through the approval of LAN viewers.
    let get = format!("/api/artifacts/{aid}/threads/{tid}");
    let (_, v) = owner_agent(&w, "GET", &get, None).await;
    assert!(!v.to_string().contains("Sam's agent, as Sam"), "{v}");
    approve(&w.ts, None, &sam_public(&w));
    let (_, v) = owner_agent(&w, "GET", &get, None).await;
    assert!(v.to_string().contains("Sam's agent, as Sam"), "{v}");
    no_canary("after approving Sam", &v.to_string());
}

#[tokio::test]
async fn an_agents_resolve_is_its_persons() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.scratch.artifact, &w.scratch.thread);
    let base = format!("/api/artifacts/{aid}/threads/{tid}");
    let r = w
        .sam
        .agent(
            "POST",
            &format!("{base}/resolve"),
            Some(&w.sam_session),
            Some(json!({"as": "agent"})),
        )
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["thread"]["resolved_by"], "agent:claude",
        "its own resolve"
    );
    let (_, v) = owner_agent(&w, "GET", &base, None).await;
    assert!(v["thread"]["resolved_by"].is_null(), "{v}");
    approve(&w.ts, None, &sam_public(&w));
    let (_, v) = owner_agent(&w, "GET", &base, None).await;
    assert_eq!(v["thread"]["resolved_by"], "agent:claude");
    // Without a session (the remote CLI), a resolve is the person's, as a
    // viewer.
    let other = format!("/api/artifacts/{aid}/threads/{}", w.scratch.resolved);
    let r = w
        .sam
        .agent("POST", &format!("{other}/reopen"), None, Some(json!({})))
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = w
        .sam
        .agent("POST", &format!("{other}/resolve"), None, Some(json!({})))
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["thread"]["resolved_by"],
        json!(format!("viewer:{}", sam_public(&w)))
    );
}

#[tokio::test]
async fn a_whole_artifact_thread_on_anothers_page_withholds_no_anchor() {
    let w = World::two_people().await;
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = &w.read.artifact;
    let form = reqwest::multipart::Form::new()
        .text("anchor", json!({"kind": "artifact"}).to_string())
        .text("body", "overall, good")
        .text("version", "2");
    let r =
        w.ts.authed(
            w.ts.client
                .post(format!("{}/api/artifacts/{aid}/threads", w.ts.base)),
        )
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let tid = r.json::<Value>().await.unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = json_of(
        w.sam
            .get(
                Some(&w.sam_session),
                &format!("/api/artifacts/{aid}/threads/{tid}"),
            )
            .await,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let t = &v["thread"];
    assert!(
        t.get("anchor_withheld").is_none() && t.get("withheld").is_none(),
        "{t}"
    );
    assert_eq!(t["comments"][0]["body"], "overall, good");
}

#[tokio::test]
async fn a_withheld_threads_approval_state_is_the_readers_request() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.read.artifact, &w.read.thread);
    let sam = sam_public(&w);
    let get = format!("/api/artifacts/{aid}/threads/{tid}");
    let state = || async {
        let (_, v) = json_of(w.sam.get(Some(&w.sam_session), &get).await).await;
        v["thread"]["withheld"]["approval"].clone()
    };
    assert_eq!(state().await, "none");
    let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    // Another approver's request is not Sam's.
    db.execute(
        "INSERT INTO approval_requests (id, approver, author, artifact_id, thread_id, cause, status, created_at, updated_at)
         VALUES ('r0', NULL, ?1, ?2, ?3, 'routing', 'open', 'x', 'x')",
        rusqlite::params![w.mia_public, aid, tid],
    )
    .unwrap();
    assert_eq!(state().await, "none");
    db.execute(
        "INSERT INTO approval_requests (id, approver, author, artifact_id, thread_id, cause, status, created_at, updated_at)
         VALUES ('r1', ?1, ?2, ?3, ?4, 'ask', 'declined', 'x', 'x')",
        rusqlite::params![sam, w.mia_public, aid, tid],
    )
    .unwrap();
    assert_eq!(state().await, "declined");
    db.execute(
        "INSERT INTO approval_requests (id, approver, author, artifact_id, thread_id, cause, status, created_at, updated_at)
         VALUES ('r2', ?1, ?2, ?3, ?4, 'routing', 'open', 'x', 'x')",
        rusqlite::params![sam, w.mia_public, aid, tid],
    )
    .unwrap();
    assert_eq!(state().await, "requested");
}

#[tokio::test]
async fn a_persons_agent_reports_its_own_tool_calls() {
    let w = World::two_people().await;
    let sid = &w.sam_session;
    let call = |path: String, session: Option<String>, body: Value| {
        let sam = &w.sam;
        async move {
            let mut headers = Vec::new();
            if let Some(s) = session {
                headers.push(("x-clax-session", s));
            }
            sam.call(Call {
                method: "POST",
                path: &path,
                authorization: Some(format!("Clax-Grant {}", sam.access())),
                body: Some(serde_json::to_vec(&body).unwrap()),
                headers,
                ..Default::default()
            })
            .await
        }
    };
    // A call of Sam's agent that wrote a comment.
    let call_id = "01JBC0000000000000000000K1";
    let header = clax_core::audit::encode_call_header(&clax_core::audit::CallHeader {
        call_id: call_id.into(),
        tool: "comments_reply".into(),
        harness_tool: None,
        args_sha256: format!("sha256:{}", "ab".repeat(32)),
        started_at: "2026-10-10T10:00:00.000Z".into(),
        harness_call_id: None,
    })
    .unwrap();
    let r = w
        .sam
        .call(Call {
            method: "POST",
            path: &format!(
                "/api/artifacts/{}/threads/{}/comments",
                w.scratch.artifact, w.scratch.thread
            ),
            authorization: Some(format!("Clax-Grant {}", w.sam.access())),
            body: Some(
                serde_json::to_vec(&json!({"body": "done", "author_kind": "agent"})).unwrap(),
            ),
            headers: vec![("x-clax-session", sid.clone()), ("x-clax-call", header)],
            ..Default::default()
        })
        .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let report = json!({"call_id": call_id, "tool": "comments_reply",
        "args_sha256": format!("sha256:{}", "ab".repeat(32)),
        "started_at": "2026-10-10T10:00:00.000Z", "ended_at": "2026-10-10T10:00:00.400Z",
        "outcome": "ok"});
    let r = call(
        format!("/api/sessions/{sid}/tool-calls"),
        Some(sid.clone()),
        report.clone(),
    )
    .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 201, "{v}");
    let seq = v["seq"].as_i64().unwrap();
    let row = w.ts.store.events_after(seq - 1, 1).unwrap().remove(0);
    assert_eq!(row.kind, "tool.call");
    let body: Value = serde_json::from_str(&row.body).unwrap();
    assert_eq!(body["produced"].as_array().unwrap().len(), 1, "{body}");
    assert!(row.actor.contains(&w.sam.grant_id()), "{}", row.actor);
    // Another credential's session is not its to report for.
    let r = call(
        format!("/api/sessions/{}/tool-calls", w.owner_session),
        Some(sid.clone()),
        report.clone(),
    )
    .await;
    assert_eq!(json_of(r).await.1["error"]["code"], "foreign_session");
    // The sessionless report is the daemon's own `/mcp`'s.
    let r = call("/api/tool-calls".into(), Some(sid.clone()), report.clone()).await;
    assert_eq!(r.status(), 403);
    // Its harness's call ID too.
    let r = call(
        format!("/api/sessions/{sid}/tool-call-ids"),
        Some(sid.clone()),
        json!({"tool_use_id": "toolu_01", "tool_name": "mcp__clax__comments_reply",
               "args_sha256": format!("sha256:{}", "ab".repeat(32))}),
    )
    .await;
    let (status, v) = json_of(r).await;
    assert_eq!(status, 201, "{v}");
}

#[tokio::test]
async fn a_hook_question_never_falls_back_to_a_page_a_persons_agent_may_not_read() {
    let w = World::two_people().await;
    let sid = &w.sam_session;
    // Sam's session holds a watch on the live page (written directly: no
    // route gives a person's agent one).
    let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    db.execute(
        "INSERT INTO watches (session_id, artifact_id, replies_armed, created_at) VALUES (?1, ?2, 0, 'z')",
        rusqlite::params![sid, w.live],
    )
    .unwrap();
    drop(db);
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{sid}/questions"),
            Some(sid),
            Some(json!({"source": "hook", "tool_use_id": "toolu_q1", "questions": [{"question": "Which?",
                "header": "Pick", "multiSelect": false, "options": [{"label": "A", "description": "a"},
                {"label": "B", "description": "b"}]}]})),
        )
        .await;
    let (status, v) = json_of(r).await;
    assert!(status == 200 || status == 201, "{status}: {v}");
    assert!(!v.to_string().contains(&w.live), "{v}");
    assert!(v["question"]["artifact"].is_null(), "{v}");
}

/// Sam's rights narrowed to every comment action but `read`, and to
/// reading `readable` alone among artifacts (all of them when `None`).
fn narrow_sam(w: &World, readable: Option<&str>) {
    let at = readable.map_or_else(|| json!(["*"]), |a| json!([a]));
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": at},
        {"type": "clax-comments", "actions": ["write", "reply", "resolve", "send", "watch"], "locations": ["*"]},
        {"type": "clax-ask", "actions": ["ask"], "locations": ["*"]}
    ]))
    .unwrap();
    w.ts.store
        .set_person_rights(&AuditCtx::DAEMON, &sam_public(w), Some(rights))
        .unwrap();
}

#[tokio::test]
async fn every_thread_write_needs_comments_read() {
    let w = World::two_people().await;
    narrow_sam(&w, None);
    let (aid, tid) = (&w.scratch.artifact, &w.scratch.thread);
    let sid = Some(w.sam_session.as_str());
    let base = format!("/api/artifacts/{aid}/threads/{tid}");
    let writes: Vec<(&str, String, Option<Value>)> = vec![
        (
            "POST",
            format!("{base}/comments"),
            Some(json!({"body": "x", "author_kind": "agent"})),
        ),
        (
            "POST",
            format!("{base}/resolve"),
            Some(json!({"as": "agent"})),
        ),
        (
            "POST",
            format!("{base}/reopen"),
            Some(json!({"as": "agent"})),
        ),
        ("POST", format!("{base}/send"), None),
        (
            "POST",
            format!("/api/artifacts/{aid}/threads:send"),
            Some(json!({"thread_ids": [tid], "note": "n"})),
        ),
        ("DELETE", base.clone(), None),
    ];
    for (m, path, body) in writes {
        let (status, v) = json_of(w.sam.agent(m, &path, sid, body).await).await;
        assert_eq!(
            (
                status,
                v["error"]["code"].clone(),
                v["error"]["right"]["action"].clone()
            ),
            (403, json!("not_permitted"), json!("read")),
            "{m} {path}: {v}"
        );
    }
    // Nothing was written: the thread is there, still open.
    let t = w.ts.store.get_thread(tid).unwrap().expect("not deleted");
    assert_eq!(t.status, "open");
}

#[tokio::test]
async fn a_persons_agent_deletes_threads_only_from_a_session() {
    let w = World::two_people().await;
    let (aid, tid) = (&w.scratch.artifact, &w.scratch.resolved);
    let path = format!("/api/artifacts/{aid}/threads/{tid}");
    // Sam's agent, without a session, deleting a thread of Mia's.
    for q in ["", "?as=agent"] {
        let (status, v) = json_of(
            w.sam
                .agent("DELETE", &format!("{path}{q}"), None, None)
                .await,
        )
        .await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (400, json!("no_session")),
            "{q}: {v}"
        );
    }
    assert!(w.ts.store.get_thread(tid).unwrap().is_some(), "not deleted");
}

#[tokio::test]
async fn a_tool_call_report_names_no_artifact_the_agent_may_not_read() {
    let w = World::two_people().await;
    let sid = w.sam_session.clone();
    // Sam's agent may read the scratch artifact alone.
    narrow_sam(&w, Some(&w.scratch.artifact));
    let report = |call_id: &str, artifact: &str| {
        json!({"call_id": call_id, "tool": "read",
            "args_sha256": format!("sha256:{}", "cd".repeat(32)),
            "started_at": "2026-10-10T10:00:00.000Z", "ended_at": "2026-10-10T10:00:00.400Z",
            "outcome": "ok", "artifact_id": artifact})
    };
    for (call_id, artifact, kept) in [
        ("01JBC0000000000000000000M1", w.live.clone(), false),
        ("01JBC0000000000000000000M2", w.read.artifact.clone(), false),
        (
            "01JBC0000000000000000000M3",
            w.scratch.artifact.clone(),
            true,
        ),
    ] {
        let r = w
            .sam
            .agent(
                "POST",
                &format!("/api/sessions/{sid}/tool-calls"),
                Some(&sid),
                Some(report(call_id, &artifact)),
            )
            .await;
        let (status, v) = json_of(r).await;
        assert_eq!(status, 201, "{artifact}: {v}");
        let row =
            w.ts.store
                .events_after(v["seq"].as_i64().unwrap() - 1, 1)
                .unwrap()
                .remove(0);
        assert_eq!(row.kind, "tool.call");
        assert_eq!(
            row.ids.artifact.is_some(),
            kept,
            "{artifact}: {:?}",
            row.ids.artifact
        );
        assert!(!row.body.contains(&artifact) || kept, "{}", row.body);
    }
}

// Artifacts, versions, files, assets and working records (spec §7.5
// artifact rows, §8.2, §8.3, O9).

/// Mia's canaries an artifact, its versions, files, assets and working
/// records can carry.
const ARTIFACT_CANARIES: &[&str] = &[
    TITLE,
    DESCRIPTION,
    ICON,
    CAPABILITY,
    LABEL,
    NOTE,
    FILE_NAME,
    FILE_BYTES,
    CARRIED_NAME,
    CARRIED_BYTES,
    ASSET_BYTES,
    WORKING,
];

fn no_artifact_canary(what: &str, text: &str) {
    for c in ARTIFACT_CANARIES.iter().chain(THREAD_CANARIES) {
        assert!(!text.contains(c), "{what} leaked {c}: {text}");
    }
}

/// `method path` as Sam's agent from its session, with JSON `body`.
async fn sam(w: &World, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    json_of(w.sam.agent(method, path, Some(&w.sam_session), body).await).await
}

/// Sam's person's rights become `preset`.
fn sam_rights(w: &World, preset: &str) {
    w.ts.store
        .set_person_rights(
            &AuditCtx::DAEMON,
            &sam_public(w),
            Some(clax_core::rights::Rights::preset(preset).unwrap()),
        )
        .unwrap();
}

/// The entry for `aid` in a listing's `artifacts`.
fn entry<'a>(list: &'a Value, aid: &str) -> &'a Value {
    list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == json!(aid))
        .unwrap_or_else(|| panic!("{aid} is not listed: {list}"))
}

fn assert_text_withheld(what: &str, a: &Value) {
    for k in ["title", "description", "icon", "capabilities"] {
        assert!(a[k].is_null(), "{what}: {k}: {a}");
    }
    assert_eq!(a["withheld"]["authors"][0]["name"], "Mia", "{what}: {a}");
}

#[tokio::test]
async fn titles_are_withheld_in_list_and_read() {
    let w = World::two_people().await;
    let aid = &w.read.artifact;
    // Sam's agent makes an artifact of its own.
    let (status, v) = sam(
        &w,
        "POST",
        "/api/artifacts",
        Some(json!({"title": "Sam's plan", "description": "Sam's words",
                    "files": {"index.html": {"content": "<p>Sam</p>", "encoding": "utf8"}}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["artifact"]["title"], "Sam's plan");
    assert_eq!(v["version"]["files"]["index.html"]["size"], 10);
    let own = v["artifact"]["id"].as_str().unwrap().to_string();
    // Its list, its read, and a list of one: Mia's text is withheld, its
    // own is not.
    let (status, list) = sam(&w, "GET", "/api/artifacts", None).await;
    assert_eq!(status, 200, "{list}");
    no_artifact_canary("Sam's agent's list", &list.to_string());
    assert_text_withheld("the list", entry(&list, aid));
    assert_eq!(entry(&list, &own)["title"], "Sam's plan");
    assert!(entry(&list, &own).get("withheld").is_none());
    let (status, one) = sam(&w, "GET", &format!("/api/artifacts?artifact={aid}"), None).await;
    assert_eq!(status, 200, "{one}");
    no_artifact_canary("a list of one", &one.to_string());
    assert_text_withheld("a list of one", entry(&one, aid));
    let (status, v) = sam(&w, "GET", &format!("/api/artifacts/{aid}"), None).await;
    assert_eq!(status, 200, "{v}");
    no_artifact_canary("Sam's agent's read", &v.to_string());
    assert_text_withheld("the read", &v["artifact"]);
    // The owner's agent: Mia's titles are withheld from it too (O9).
    let (status, list) = owner_agent(&w, "GET", "/api/artifacts", None).await;
    assert_eq!(status, 200);
    no_artifact_canary("the owner's agent's list", &list.to_string());
    assert_text_withheld("the owner's list", entry(&list, aid));
    let (_, v) = owner_agent(&w, "GET", &format!("/api/artifacts/{aid}"), None).await;
    assert_text_withheld("the owner's read", &v["artifact"]);
    // Sam's agent changes the title: that field is its person's now, the
    // others stay Mia's.
    sam_rights(&w, "clax:full");
    let (status, v) = sam(
        &w,
        "PATCH",
        &format!("/api/artifacts/{aid}"),
        Some(json!({"title": "Renamed by Sam"})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["artifact"]["title"], "Renamed by Sam");
    assert!(v["artifact"]["description"].is_null(), "{v}");
    assert_eq!(v["artifact"]["withheld"]["authors"][0]["name"], "Mia");
    no_artifact_canary("a PATCH's answer", &v.to_string());
    // Once Sam approves Mia, her words reach his agents.
    approve(&w.ts, Some(&sam_public(&w)), &w.mia_public);
    let (_, v) = sam(&w, "GET", &format!("/api/artifacts/{aid}"), None).await;
    assert_eq!(v["artifact"]["description"], DESCRIPTION);
    assert!(v["artifact"].get("withheld").is_none(), "{v}");
}

#[tokio::test]
async fn list_leaves_out_unreadable_artifacts() {
    let w = World::two_people().await;
    let owners = w.ts.any_artifact().await;
    // Sam's agent may read the scratch artifact alone.
    narrow_sam(&w, Some(&w.scratch.artifact));
    let (status, list) = sam(&w, "GET", "/api/artifacts", None).await;
    assert_eq!(status, 200, "{list}");
    let ids: Vec<&str> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [w.scratch.artifact.as_str()], "{list}");
    // Naming one it may not read answers as naming none at all.
    let (_, one) = sam(
        &w,
        "GET",
        &format!("/api/artifacts?artifact={}", w.read.artifact),
        None,
    )
    .await;
    let (_, none) = sam(&w, "GET", "/api/artifacts?artifact=7q3k9mzx2b4t", None).await;
    assert_eq!(one, none);
    assert_eq!(one, json!({"artifacts": []}));
    let (status, v) = sam(&w, "GET", &format!("/api/artifacts/{owners}"), None).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (403, json!("not_permitted"))
    );
    // A live page is never listed, whatever the rights.
    sam_rights(&w, "clax:full");
    let (_, list) = sam(&w, "GET", "/api/artifacts", None).await;
    assert!(!list.to_string().contains(&w.live), "{list}");
    let (_, one) = sam(
        &w,
        "GET",
        &format!("/api/artifacts?artifact={}", w.live),
        None,
    )
    .await;
    assert_eq!(one, json!({"artifacts": []}));
    // The working roster likewise leaves out what it may not read.
    narrow_sam(&w, Some(&w.scratch.artifact));
    let (status, v) = sam(&w, "GET", "/api/working", None).await;
    assert_eq!(status, 200, "{v}");
    assert!(!v.to_string().contains(&w.read.artifact), "{v}");
    no_artifact_canary("the roster", &v.to_string());
}

#[tokio::test]
async fn version_files_and_notes_are_withheld() {
    let w = World::two_people().await;
    // Sam approved the owner, not Mia.
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = &w.read.artifact;
    for (who, get) in [("Sam's agent", true), ("the owner's agent", false)] {
        let call = |path: String| {
            let w = &w;
            async move {
                if get {
                    sam(w, "GET", &path, None).await
                } else {
                    owner_agent(w, "GET", &path, None).await
                }
            }
        };
        let (status, v) = call(format!("/api/artifacts/{aid}/versions")).await;
        assert_eq!(status, 200, "{who}: {v}");
        no_artifact_canary(who, &v.to_string());
        let vs = v["versions"].as_array().unwrap();
        // Version 1 is Mia's: label, note and every file.
        assert!(
            vs[0]["label"].is_null() && vs[0]["note"].is_null(),
            "{who}: {v}"
        );
        assert_eq!(vs[0]["files"], json!({}), "{who}");
        assert_eq!(vs[0]["files_withheld"], 3, "{who}");
        assert!(vs[0]["content_sha256"].is_null(), "{who}: {v}");
        // Version 2 is the owner's agent's, carrying Mia's files.
        assert_eq!(vs[1]["label"], "the owner's agent's", "{who}");
        let files: Vec<&String> = vs[1]["files"].as_object().unwrap().keys().collect();
        assert_eq!(files, ["owner.html"], "{who}");
        assert_eq!(vs[1]["files_withheld"], 3, "{who}");
        let (status, one) = call(format!("/api/artifacts/{aid}/versions/1")).await;
        assert_eq!(status, 200, "{who}: {one}");
        assert_eq!(one["version"], vs[0], "{who}");
        let (status, v) = call(format!("/api/artifacts/{aid}")).await;
        assert_eq!(status, 200);
        assert_eq!(v["versions"][1], vs[1], "{who}");
        let (status, f) = call(format!("/api/artifacts/{aid}/files")).await;
        assert_eq!(status, 200, "{who}: {f}");
        let files: Vec<&String> = f["files"].as_object().unwrap().keys().collect();
        assert_eq!(files, ["owner.html"], "{who}: {f}");
        assert_eq!(f["files_withheld"], 3, "{who}");
        // Its content: a withheld file and one that is not there answer
        // alike, so the route tells nothing of a guessed name.
        let (status, _) = call(format!("/api/artifacts/{aid}/versions/2/files/owner.html")).await;
        assert_eq!(status, 200, "{who}");
        let mut answers = Vec::new();
        for path in [CARRIED_NAME, FILE_NAME, "index.html", "not-there.css"] {
            let (status, v) = call(format!("/api/artifacts/{aid}/versions/2/files/{path}")).await;
            assert_eq!(
                (status, v["error"]["code"].clone()),
                (403, json!("withheld")),
                "{who} {path}: {v}"
            );
            no_artifact_canary(who, &v.to_string());
            answers.push(v);
        }
        assert!(answers.windows(2).all(|p| p[0] == p[1]), "{answers:?}");
        assert_eq!(answers[0]["error"]["authors"][0]["name"], "Mia");
    }
    // Where nothing is withheld, a missing file is just missing.
    let mine = w.ts.any_artifact().await;
    let (status, _) = sam(
        &w,
        "GET",
        &format!("/api/artifacts/{mine}/versions/1/files/not-there.css"),
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn embedded_working_messages_are_filtered() {
    let w = World::two_people().await;
    let aid = &w.read.artifact;
    // Sam's agent works on the same artifact, with a message of its own.
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/working/{aid}", w.sam_session),
        Some(json!({"message": "Sam's agent at work"})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let check = |what: &str, records: &Value| {
        let records = records
            .as_array()
            .unwrap_or_else(|| panic!("{what}: {records}"));
        assert_eq!(records.len(), 2, "{what}: {records:?}");
        let mias = records
            .iter()
            .find(|r| r["message"] != "Sam's agent at work")
            .unwrap();
        assert_eq!(
            (mias["message"].clone(), mias["withheld"].clone()),
            (Value::Null, json!(true)),
            "{what}"
        );
        assert!(
            records
                .iter()
                .any(|r| r["message"] == "Sam's agent at work"),
            "{what}: {records:?}"
        );
    };
    let (_, v) = sam(&w, "GET", &format!("/api/artifacts/{aid}"), None).await;
    check("the read", &v["artifact"]["working"]);
    let (_, v) = sam(&w, "GET", "/api/artifacts", None).await;
    check("the list", &entry(&v, aid)["working"]);
    let (_, v) = sam(&w, "GET", &format!("/api/artifacts/{aid}/working"), None).await;
    check("the artifact's working", &v["working"]);
    let (_, v) = sam(&w, "GET", "/api/working", None).await;
    no_artifact_canary("the roster", &v.to_string());
    let on: Vec<Value> = v["working"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["artifact_id"] == json!(aid))
        .cloned()
        .collect();
    check("the roster", &json!(on));
    // Another credential's session is not named to a person's agent.
    let mias = on.iter().find(|r| r["withheld"] == true).unwrap();
    assert!(mias["session_id"].is_null(), "{mias}");
    let mine = on.iter().find(|r| r.get("withheld").is_none()).unwrap();
    assert_eq!(mine["session_id"], json!(w.sam_session));
    // The owner's agent: Mia's and Sam's words are both withheld from it.
    let (_, v) = owner_agent(&w, "GET", &format!("/api/artifacts/{aid}/working"), None).await;
    no_artifact_canary("the owner's agent", &v.to_string());
    assert!(!v.to_string().contains("Sam's agent at work"), "{v}");
}

#[tokio::test]
async fn publish_conflict_summary_is_filtered() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    sam_rights(&w, "clax:full");
    let file = |c: &str| json!({"content": c, "encoding": "utf8"});
    // A stale publish: the conflict, and the read a client summarises it
    // from, carry nothing of Mia's.
    let (status, v) = sam(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 1, "files": {"index.html": file("<p>Sam</p>")}})),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (409, json!("conflict")),
        "{v}"
    );
    no_artifact_canary("a conflict", &v.to_string());
    let (_, v) = sam(&w, "GET", &format!("/api/artifacts/{aid}"), None).await;
    no_artifact_canary("the read after a conflict", &v.to_string());
    let (_, v) = owner_agent(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 1, "files": {"index.html": file("<p>x</p>")}})),
    )
    .await;
    assert_eq!(v["error"]["code"], "conflict");
    no_artifact_canary("the owner's agent's conflict", &v.to_string());
    // A publish over Mia's version: Sam's agent's own file is its own;
    // Mia's carried files keep her authors and stay withheld.
    let (status, v) = sam(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 2, "label": "Sam's", "files": {
            "index.html": file("<p>Sam's page</p>"), "sam.css": file("p {}")}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    no_artifact_canary("a publish's answer", &v.to_string());
    assert_eq!(v["version"]["label"], "Sam's");
    let files: Vec<&String> = v["version"]["files"].as_object().unwrap().keys().collect();
    assert_eq!(files, ["index.html", "sam.css"], "{v}");
    assert_eq!(v["version"]["files_withheld"], 3, "{v}");
    assert_text_withheld("a publish's artifact", &v["artifact"]);
    let id = clax_core::ArtifactId::parse(aid).unwrap();
    let v3 = w.ts.store.get_version(&id, 3).unwrap().unwrap();
    assert_eq!(
        clax_core::visibility::authors_of_file(&v3, CARRIED_NAME),
        [clax_core::visibility::Author::Person(w.mia_public.clone())]
    );
    assert_eq!(
        clax_core::visibility::authors_of_file(&v3, "sam.css"),
        [clax_core::visibility::Author::Person(sam_public(&w))]
    );
}

#[tokio::test]
async fn a_persons_agent_writes_artifacts_from_a_session_and_never_as_a_page() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    sam_rights(&w, "clax:full");
    let publish = json!({"if_version": 2, "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}});
    let create =
        json!({"title": "t", "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}});
    let writes: Vec<(&str, String, Option<Value>)> = vec![
        ("POST", "/api/artifacts".into(), Some(create)),
        (
            "POST",
            format!("/api/artifacts/{aid}/versions"),
            Some(publish),
        ),
        (
            "PATCH",
            format!("/api/artifacts/{aid}"),
            Some(json!({"title": "t"})),
        ),
    ];
    for (m, path, body) in &writes {
        let (status, v) = json_of(w.sam.agent(m, path, None, body.clone()).await).await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (400, json!("no_session")),
            "{m} {path}: {v}"
        );
    }
    let (status, v) = agent_form(
        &w.sam,
        &format!("/api/artifacts/{aid}/assets"),
        None,
        &[("file", FAKE_PNG, Some("a.png"))],
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("no_session")),
        "{v}"
    );
    // The page marker is a browser's: refused from a grant.
    for (m, path, body) in &writes[1..] {
        let r = w
            .sam
            .call(Call {
                method: m,
                path,
                authorization: Some(format!("Clax-Grant {}", w.sam.access())),
                body: body.as_ref().map(|b| serde_json::to_vec(b).unwrap()),
                headers: vec![
                    ("x-clax-session", w.sam_session.clone()),
                    ("x-clax-via", "page".into()),
                ],
                ..Default::default()
            })
            .await;
        let (status, v) = json_of(r).await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (400, json!("invalid_via_page")),
            "{m} {path}: {v}"
        );
    }
    let (ct, body) = multipart(&[("file", FAKE_PNG, Some("a.png"))]);
    let r = w
        .sam
        .call(Call {
            method: "POST",
            path: &format!("/api/artifacts/{aid}/assets"),
            authorization: Some(format!("Clax-Grant {}", w.sam.access())),
            body: Some(body),
            headers: vec![
                ("content-type", ct),
                ("x-clax-session", w.sam_session.clone()),
                ("x-clax-via", "page".into()),
            ],
            ..Default::default()
        })
        .await;
    let (status, v) = json_of(r).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("invalid_via_page")),
        "{v}"
    );
    // Nothing was written.
    let a =
        w.ts.store
            .get_artifact(&clax_core::ArtifactId::parse(aid).unwrap())
            .unwrap()
            .unwrap();
    assert_eq!((a.current_version, a.title.as_str()), (2, TITLE));
}

#[tokio::test]
async fn asset_names_and_bytes_are_withheld() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    sam_rights(&w, "clax:full");
    // Sam's agent uploads an asset of its own.
    let (status, v) = agent_form(
        &w.sam,
        &format!("/api/artifacts/{aid}/assets"),
        Some(&w.sam_session),
        &[("file", FAKE_PNG, Some("a.png"))],
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["asset"]["content_type"], "image/png");
    assert!(v["asset"].get("withheld").is_none(), "{v}");
    let own = v["asset"]["id"].as_str().unwrap().to_string();
    for (who, (status, v)) in [
        (
            "Sam's agent",
            sam(&w, "GET", &format!("/api/artifacts/{aid}/assets"), None).await,
        ),
        (
            "the owner's agent",
            owner_agent(&w, "GET", &format!("/api/artifacts/{aid}/assets"), None).await,
        ),
    ] {
        assert_eq!(status, 200, "{who}: {v}");
        no_artifact_canary(who, &v.to_string());
        let assets = v["assets"].as_array().unwrap();
        let mias = assets
            .iter()
            .find(|a| a["id"] == json!(w.scratch.asset))
            .unwrap();
        assert_eq!(mias["withheld"], true, "{who}: {mias}");
        for k in ["name", "content_type", "size", "ext"] {
            assert!(mias[k].is_null(), "{who}: {k}: {mias}");
        }
        assert_eq!(mias["artifact_id"], json!(aid));
        if who == "Sam's agent" {
            let mine = assets.iter().find(|a| a["id"] == json!(own)).unwrap();
            assert_eq!(mine["content_type"], "image/png", "{mine}");
        }
    }
    // The bytes are a browser's to fetch, never an agent's.
    let r = w
        .sam
        .get(Some(&w.sam_session), &format!("/_blob/{}", w.scratch.asset))
        .await;
    assert_eq!(r.status(), 403);
    let (status, v) = owner_agent(&w, "GET", &format!("/_blob/{}", w.scratch.asset), None).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (403, json!("not_for_agents"))
    );
}

#[tokio::test]
async fn creating_a_thread_never_tells_whether_a_withheld_file_exists() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let make = |file: &str| {
        let w = &w;
        let anchor = anchor_on(file, "q");
        async move {
            agent_form(
                &w.sam,
                &format!("/api/artifacts/{aid}/threads"),
                Some(&w.sam_session),
                &[
                    ("anchor", anchor.as_bytes(), None),
                    ("body", b"a guess", None),
                    ("version", b"1", None),
                ],
            )
            .await
        }
    };
    let before =
        w.ts.store
            .list_threads(
                &clax_core::ArtifactId::parse(aid).unwrap(),
                true,
                None,
                1000,
            )
            .unwrap()
            .0
            .len();
    for file in [FILE_NAME, CARRIED_NAME, "not-there.html"] {
        let (status, v) = make(file).await;
        assert_eq!(
            (
                status,
                v["error"]["code"].clone(),
                v["error"]["message"].clone()
            ),
            (
                400,
                json!("invalid_anchor"),
                json!(format!("version 1 of artifact {aid} has no file {file}"))
            ),
            "{file}: {v}"
        );
    }
    let after =
        w.ts.store
            .list_threads(
                &clax_core::ArtifactId::parse(aid).unwrap(),
                true,
                None,
                1000,
            )
            .unwrap()
            .0
            .len();
    assert_eq!(before, after, "no thread was written");
    // Every version has its index.html: a thread on it tells nothing.
    let (status, v) = make("index.html").await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["thread"]["anchor_withheld"], true);
}
