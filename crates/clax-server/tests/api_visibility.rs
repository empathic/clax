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
    // The reply alone: a person's agent's reply on the owner's thread
    // takes nothing (O8).
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
    // The owner's agent works on the live page: on its roster, never on a
    // person's agent's.
    w.ts.put_working(
        &w.owner_session,
        &w.live,
        json!({"message": "on the live page"}),
    )
    .await;
    let (_, v) = sam(&w, "GET", "/api/working", None).await;
    assert!(!v.to_string().contains(&w.live), "{v}");
    let (_, v) = owner_agent(&w, "GET", "/api/working", None).await;
    assert!(v.to_string().contains(&w.live), "{v}");
    // The working roster likewise leaves out what it may not read.
    narrow_sam(&w, Some(&w.scratch.artifact));
    let (status, v) = sam(&w, "GET", "/api/working", None).await;
    assert_eq!(status, 200, "{v}");
    assert!(!v.to_string().contains(&w.read.artifact), "{v}");
    no_artifact_canary("the roster", &v.to_string());
    // A working record whose session is gone names no one: its message is
    // withheld.
    let gone = w.ts.register_session("claude", "gone-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    w.ts.put_working(
        &gone,
        &w.scratch.artifact,
        json!({"message": "GONESESSIONwords"}),
    )
    .await;
    let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    db.execute("DELETE FROM sessions WHERE id = ?1", [&gone])
        .unwrap();
    drop(db);
    let (_, v) = sam(
        &w,
        "GET",
        &format!("/api/artifacts/{}/working", w.scratch.artifact),
        None,
    )
    .await;
    assert!(!v.to_string().contains("GONESESSIONwords"), "{v}");
    // Read at `own`: what its person made, listed and on its roster.
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": ["own"]},
        {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
        {"type": "clax-comments", "actions": ["watch"], "locations": ["*"]}
    ]))
    .unwrap();
    w.ts.store
        .set_person_rights(&AuditCtx::DAEMON, &sam_public(&w), Some(rights))
        .unwrap();
    let (status, v) = sam(
        &w,
        "POST",
        "/api/artifacts",
        Some(json!({"title": "Sam's own",
                    "files": {"index.html": {"content": "<p>s</p>", "encoding": "utf8"}}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let own = v["artifact"]["id"].as_str().unwrap().to_string();
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/working/{own}", w.sam_session),
        Some(json!({"message": "on my own"})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (_, list) = sam(&w, "GET", "/api/artifacts", None).await;
    let ids: Vec<&str> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [own.as_str()], "{list}");
    let (_, v) = sam(&w, "GET", "/api/working", None).await;
    let on: Vec<&Value> = v["working"].as_array().unwrap().iter().collect();
    assert_eq!(on.len(), 1, "{v}");
    assert_eq!(on[0]["artifact_id"], json!(own));
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
    // Deletes too come from a session.
    for path in [
        format!("/api/artifacts/{aid}/assets/{}", w.scratch.asset),
        format!("/api/artifacts/{aid}"),
    ] {
        let (status, v) = json_of(w.sam.agent("DELETE", &path, None, None).await).await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (400, json!("no_session")),
            "DELETE {path}: {v}"
        );
    }
    // Nothing was written.
    let a =
        w.ts.store
            .get_artifact(&clax_core::ArtifactId::parse(aid).unwrap())
            .unwrap()
            .unwrap();
    assert_eq!((a.current_version, a.title.as_str()), (2, TITLE));
    assert!(w.ts.store.get_asset(&w.scratch.asset).unwrap().is_some());
}

/// An artifact Mia's agent makes from its session (its `owner_session_id`
/// is Mia's), with withheld files `MIAXname.js` and a directory `MIAXdir`.
async fn mia_agent_artifact(w: &World) -> String {
    let file = |c: &str| json!({"content": c, "encoding": "utf8"});
    let (status, v) = json_of(
        w.mia
            .agent(
                "POST",
                "/api/artifacts",
                Some(&w.mia_session),
                Some(
                    json!({"title": TITLE, "description": DESCRIPTION, "files": {
                        "index.html": file("<p>Mia</p>"),
                        "MIAXname.js": file("/* m */"),
                        "MIAXdir/MIAXnested.css": file("/* m */"),
                    }}),
                ),
            )
            .await,
    )
    .await;
    assert_eq!(status, 201, "{v}");
    v["artifact"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn write_answers_name_no_other_credentials_session() {
    let w = World::two_people().await;
    let aid = mia_agent_artifact(&w).await;
    let stored =
        w.ts.store
            .get_artifact(&clax_core::ArtifactId::parse(&aid).unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(
        stored.owner_session_id.as_deref(),
        Some(w.mia_session.as_str())
    );
    sam_rights(&w, "clax:full");
    let file = json!({"content": "<p>Sam</p>", "encoding": "utf8"});
    for (m, path, body) in [
        (
            "PATCH",
            format!("/api/artifacts/{aid}"),
            json!({"pinned": true}),
        ),
        (
            "PATCH",
            format!("/api/artifacts/{aid}"),
            json!({"title": "Sam's"}),
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"index.html": file}}),
        ),
    ] {
        let r = w
            .sam
            .agent(m, &path, Some(&w.sam_session), Some(body))
            .await;
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        assert!(status < 300, "{m} {path}: {status} {text}");
        assert!(!text.contains(&w.mia_session), "{m} {path}: {text}");
        assert!(!text.contains("owner_session_id"), "{m} {path}: {text}");
        no_artifact_canary(&format!("{m} {path}"), &text);
    }
}

#[tokio::test]
async fn a_persons_agents_publish_leaves_untargeted_feedback_untargeted() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    // The owner by hand: an artifact no session watches, and a thread sent
    // to no agent in particular.
    let aid = w.ts.any_artifact().await;
    let tid = w.ts.thread(&aid, 1, "for any agent").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    w.ts.send_thread(&aid, &tid).await;
    let untargeted = |w: &World| -> i64 {
        let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db.query_row(
            "SELECT COUNT(*) FROM feedback WHERE thread_id = ?1 AND target_session_id IS NULL",
            [&tid],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(untargeted(&w), 1);
    let file = json!({"content": "<p>Sam</p>", "encoding": "utf8"});
    let (status, v) = sam(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 1, "files": {"index.html": file}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(
        untargeted(&w),
        1,
        "Sam's agent took the owner's agents' feedback"
    );
    // The owner's agent's publish takes it, as before.
    let (status, v) = owner_agent(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 2, "files": {"index.html": file}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(untargeted(&w), 0);
}

#[tokio::test]
async fn a_publish_never_tells_whether_a_guessed_name_is_withheld() {
    let w = World::two_people().await;
    let aid = mia_agent_artifact(&w).await;
    sam_rights(&w, "clax:full");
    let file = |c: &str| json!({"content": c, "encoding": "utf8"});
    // An address of no thread rolls each publish back, after the paths.
    let bogus = "01JZZZZZZZZZZZZZZZZZZZZZZZ";
    for (hit, miss) in [
        ("miaxname.js", "zzzname.js"),
        ("MIAXname.js/x", "zzz.js/x"),
        ("MIAXdir", "zzzdir"),
    ] {
        let mut answers = Vec::new();
        for guess in [hit, miss] {
            let (status, v) = sam(
                &w,
                "POST",
                &format!("/api/artifacts/{aid}/versions"),
                Some(json!({"if_version": 1, "addresses": [bogus],
                            "files": {"index.html": file("<p>s</p>"), guess: file("x")}})),
            )
            .await;
            answers.push((status, v));
        }
        assert_eq!(answers[0], answers[1], "{hit} against {miss}");
        assert_eq!(
            answers[0].1["error"]["code"], "unknown_thread",
            "{answers:?}"
        );
    }
    // A put over a withheld path replaces it, as a remove would.
    let (status, v) = sam(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 1, "files": {"index.html": file("<p>s</p>"), "miaxname.js": file("x")}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let v2 =
        w.ts.store
            .get_version(&clax_core::ArtifactId::parse(&aid).unwrap(), 2)
            .unwrap()
            .unwrap();
    let files: Vec<&String> = v2.files.keys().collect();
    assert_eq!(
        files,
        ["MIAXdir/MIAXnested.css", "index.html", "miaxname.js"],
        "{v}"
    );
    // A collision among paths the publisher sees is refused as before.
    let (status, v) = sam(
        &w,
        "POST",
        &format!("/api/artifacts/{aid}/versions"),
        Some(json!({"if_version": 2, "files": {"index.html": file("<p>s</p>"), "MiaxName.js": file("y")}})),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("invalid_path")),
        "{v}"
    );
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

// Documents (spec §7.5 docs row, §8.2 document authors, §8.3 document).

/// Mia's document in the world's artifacts, and its ID.
const MIA_DOC: &str = "notes/MIACANARYdocpath";
const MIA_DOC_ID: &str = "MIACANARYdocpath";

fn doc_url(aid: &str, path: &str) -> String {
    format!("/api/artifacts/{aid}/docs/{path}")
}

fn no_doc_field(what: &str, text: &str) {
    assert!(
        !text.contains(crate::common::people::DOC_FIELD),
        "{what} leaked Mia's field: {text}"
    );
}

/// The document at `path` of `aid` as stored: its data and version, and
/// its authors' tokens.
fn stored_doc(w: &World, aid: &str, path: &str) -> (Option<(Value, u64)>, Vec<String>) {
    let id = clax_core::ArtifactId::parse(aid).unwrap();
    let doc =
        w.ts.store
            .doc_get(
                &id,
                path,
                &clax_core::db::Caller::new(clax_core::db::Level::Owner, None),
            )
            .unwrap()
            .map(|d| (d.data, d.version));
    let authors =
        w.ts.store
            .doc_authors(&id, path)
            .unwrap()
            .iter()
            .map(|a| a.token().unwrap().to_string())
            .collect();
    (doc, authors)
}

/// The placeholder of a withheld document read by its path (§8.3).
fn assert_doc_withheld(what: &str, doc: &Value, id: &str, by: &[&str]) {
    let names: Vec<&str> = doc["withheld"]["authors"]
        .as_array()
        .unwrap_or_else(|| panic!("{what}: not withheld: {doc}"))
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, by, "{what}: {doc}");
    assert_eq!(doc["doc_id"], json!(id), "{what}: {doc}");
    assert!(doc["version"].is_u64(), "{what}: {doc}");
    assert!(doc.get("data").is_none(), "{what}: {doc}");
    no_doc_field(what, &doc.to_string());
}

#[tokio::test]
async fn a_withheld_document_read_by_its_path_is_a_placeholder() {
    let w = World::two_people().await;
    let aid = &w.read.artifact;
    let (_, version) = stored_doc(&w, aid, MIA_DOC).0.unwrap();
    let (status, v) = sam(&w, "GET", &doc_url(aid, MIA_DOC), None).await;
    assert_eq!(status, 200, "{v}");
    assert_doc_withheld("Sam's agent", &v["doc"], MIA_DOC_ID, &["Mia"]);
    assert_eq!(v["doc"]["version"], json!(version));
    assert_eq!(
        v["doc"]["withheld"]["authors"][0]["public_id"],
        json!(w.mia_public)
    );
    let (status, v) = owner_agent(&w, "GET", &doc_url(aid, MIA_DOC), None).await;
    assert_eq!(status, 200, "{v}");
    assert_doc_withheld("the owner's agent", &v["doc"], MIA_DOC_ID, &["Mia"]);
    // A missing document is missing.
    let (status, v) = sam(&w, "GET", &doc_url(aid, "notes/none"), None).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (404, json!("not_found"))
    );
    // The owner by hand and, once Sam approves Mia, Sam's agent read it.
    let r = w.ts.get_authed(&doc_url(aid, MIA_DOC)).await;
    assert!(
        r.text()
            .await
            .unwrap()
            .contains(crate::common::people::DOC_FIELD)
    );
    approve(&w.ts, Some(&sam_public(&w)), &w.mia_public);
    let (_, v) = sam(&w, "GET", &doc_url(aid, MIA_DOC), None).await;
    assert_eq!(
        v["doc"]["data"]["text"],
        json!(crate::common::people::DOC_FIELD),
        "{v}"
    );
}

#[tokio::test]
async fn withheld_documents_are_counted_not_listed() {
    let w = World::two_people().await;
    let aid = &w.read.artifact;
    let (status, v) = sam(
        &w,
        "PUT",
        &doc_url(aid, "notes/sam1"),
        Some(json!({"data": {"text": "zzz"}})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let list = |q: &str| format!("/api/artifacts/{aid}/docs?collection=notes{q}");
    let enc = |s: &str| {
        s.replace('%', "%25")
            .replace(' ', "%20")
            .replace('"', "%22")
            .replace('[', "%5B")
            .replace(']', "%5D")
            .replace(',', "%2C")
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace('=', "%3D")
            .replace('!', "%21")
    };
    let queries = [
        String::new(),
        format!("&where={}", enc(r#"[["text",">","A"]]"#)),
        format!("&where={}", enc(r#"[["text","<","N"]]"#)),
        format!(
            "&where={}",
            enc(&format!(
                r#"[["text","==","{}"]]"#,
                crate::common::people::DOC_FIELD
            ))
        ),
        "&order_by=text".into(),
        "&order_by=text&direction=desc&limit=1".into(),
        "&limit=1".into(),
    ];
    for q in &queries {
        let (status, v) = sam(&w, "GET", &list(q), None).await;
        assert_eq!(status, 200, "{q}: {v}");
        let paths: Vec<&str> = v["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect();
        assert!(paths.iter().all(|p| *p == "notes/sam1"), "{q}: {v}");
        assert_eq!(v["withheld_count"], json!(2), "{q}: {v}");
        assert_eq!(v["next_cursor"], Value::Null, "{q}: {v}");
        assert!(!v.to_string().contains("MIACANARY"), "{q}: {v}");
    }
    // The owner's agent has approved neither person: Mia's two and Sam's.
    for q in &queries {
        let (status, v) = owner_agent(&w, "GET", &list(q), None).await;
        assert_eq!(status, 200, "{q}: {v}");
        assert_eq!(
            (v["docs"].clone(), v["withheld_count"].clone()),
            (json!([]), json!(3)),
            "{q}: {v}"
        );
    }
    // The owner by hand lists all three, with no count.
    let v: Value = w.ts.get_authed(&list("")).await.json().await.unwrap();
    assert_eq!(v["docs"].as_array().unwrap().len(), 3, "{v}");
    assert!(v.get("withheld_count").is_none(), "{v}");
}

#[tokio::test]
async fn content_dependent_writes_on_withheld_documents_are_refused() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let before = stored_doc(&w, aid, MIA_DOC);
    let version = before.0.as_ref().unwrap().1;
    let url = format!("/api/artifacts/{aid}/docs:str_replace");
    let mut answers = Vec::new();
    for old in ["MIACANARY", "not in it", "M"] {
        for extra in [
            json!({}),
            json!({"replace_all": true}),
            json!({"if_version": version}),
            json!({"if_version": version + 100}),
            json!({"lww": true}),
        ] {
            let mut body =
                json!({"path": MIA_DOC, "field": "text", "old_str": old, "new_str": "x"});
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            for who in ["sam", "owner"] {
                let (status, v) = if who == "sam" {
                    sam(&w, "POST", &url, Some(body.clone())).await
                } else {
                    owner_agent(&w, "POST", &url, Some(body.clone())).await
                };
                assert_eq!(
                    (status, v["error"]["code"].clone()),
                    (403, json!("withheld")),
                    "{who} {body}: {v}"
                );
                assert_eq!(v["error"]["authors"][0]["name"], json!("Mia"), "{v}");
                no_doc_field(who, &v.to_string());
                answers.push((who, v));
            }
        }
    }
    // Whether or not the text would match, the answer is the same.
    for (who, v) in &answers {
        let first = &answers.iter().find(|(w, _)| w == who).unwrap().1;
        assert_eq!(v, first, "{who}");
    }
    assert_eq!(stored_doc(&w, aid, MIA_DOC), before, "nothing was written");
    // A missing document is missing; Sam's own is his to edit.
    let missing = json!({"path": "notes/none", "field": "text", "old_str": "a", "new_str": "b"});
    let (status, v) = sam(&w, "POST", &url, Some(missing)).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (404, json!("not_found")),
        "{v}"
    );
    sam(
        &w,
        "PUT",
        &doc_url(aid, "notes/sam"),
        Some(json!({"data": {"text": "Sam's"}})),
    )
    .await;
    let mine = json!({"path": "notes/sam", "field": "text", "old_str": "Sam", "new_str": "S", "lww": true});
    let (status, v) = sam(&w, "POST", &url, Some(mine)).await;
    assert_eq!(
        (status, v["doc"]["data"]["text"].clone()),
        (200, json!("S's")),
        "{v}"
    );
}

#[tokio::test]
async fn patch_returns_filtered_documents() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let url = doc_url(aid, MIA_DOC);
    // A merge into Mia's document depends on what it holds: refused. A
    // blind replace is allowed, answers its placeholder, and Sam joins her
    // as an author.
    let (status, v) = sam(
        &w,
        "PATCH",
        &url,
        Some(json!({"data": {"sam": "hi"}, "lww": true})),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (403, json!("withheld")),
        "{v}"
    );
    assert_eq!(stored_doc(&w, aid, MIA_DOC).1, vec![w.mia_public.clone()]);
    let (status, v) = sam(
        &w,
        "PUT",
        &url,
        Some(json!({"data": {"sam": "all mine"}, "lww": true})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_doc_withheld("PUT", &v["doc"], MIA_DOC_ID, &["Mia"]);
    assert_eq!(v["created"], json!(false));
    let sam_id = sam_public(&w);
    assert_eq!(
        stored_doc(&w, aid, MIA_DOC).1,
        vec![w.mia_public.clone(), sam_id.clone()]
    );
    // Rewriting it never made it Sam's: still withheld from him, and now
    // from Mia's agent too until she approves Sam.
    let (_, v) = sam(&w, "GET", &url, None).await;
    assert_doc_withheld("Sam's read", &v["doc"], MIA_DOC_ID, &["Mia"]);
    let (_, v) = json_of(w.mia.agent("GET", &url, None, None).await).await;
    assert_doc_withheld("Mia's read", &v["doc"], MIA_DOC_ID, &["Sam"]);
    approve(&w.ts, Some(&w.mia_public), &sam_id);
    let (_, v) = json_of(w.mia.agent("GET", &url, None, None).await).await;
    assert_eq!(v["doc"]["data"], json!({"sam": "all mine"}), "{v}");
    // The owner's agent's PUT answers its placeholder too; its PATCH is
    // refused.
    let (status, v) = owner_agent(
        &w,
        "PATCH",
        &url,
        Some(json!({"data": {"o": 1}, "lww": true})),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (403, json!("withheld")),
        "{v}"
    );
    let (status, v) = owner_agent(
        &w,
        "PUT",
        &url,
        Some(json!({"data": {"o": 1}, "lww": true})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_doc_withheld("the owner's PUT", &v["doc"], MIA_DOC_ID, &["Mia", "Sam"]);
    // Deleting it and writing anew starts Sam's own document, which he
    // merges into.
    let (status, v) = sam(&w, "DELETE", &format!("{url}?lww=true"), None).await;
    assert_eq!((status, v), (200, json!({"deleted": true})));
    let (status, v) = sam(&w, "PUT", &url, Some(json!({"data": {"n": 1}}))).await;
    assert_eq!(
        (status, v["doc"]["data"].clone(), v["created"].clone()),
        (200, json!({"n": 1}), json!(true)),
        "{v}"
    );
    let (_, v) = sam(
        &w,
        "PATCH",
        &url,
        Some(json!({"data": {"m": 2}, "lww": true})),
    )
    .await;
    assert_eq!(v["doc"]["data"], json!({"n": 1, "m": 2}), "{v}");
}

#[tokio::test]
async fn batch_results_are_filtered() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let url = format!("/api/artifacts/{aid}/docs:batch");
    let body = json!({"lww": true, "writes": [
        {"op": "set", "path": "notes/sam2", "data": {"a": 1}},
        {"op": "set", "path": MIA_DOC, "data": {"b": 2}},
        {"op": "delete", "path": "notes/none"},
    ]});
    for (who, (status, v)) in [
        ("sam", sam(&w, "POST", &url, Some(body.clone())).await),
        (
            "owner",
            owner_agent(&w, "POST", &url, Some(body.clone())).await,
        ),
    ] {
        assert_eq!(status, 200, "{who}: {v}");
        no_doc_field(who, &v.to_string());
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 3, "{v}");
        for r in results {
            let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["deleted", "op", "path", "version"], "{who}: {r}");
        }
    }
    let (_, authors) = stored_doc(&w, aid, MIA_DOC);
    assert_eq!(
        authors,
        vec![w.mia_public.clone(), sam_public(&w), "*owner".to_string()]
    );
    // A failing batch names the caller's own operation and path only.
    let bad = json!({"writes": [
        {"op": "set", "path": MIA_DOC, "data": {"c": 3}, "if_version": 1},
    ]});
    let (status, v) = sam(&w, "POST", &url, Some(bad)).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (409, json!("conflict")),
        "{v}"
    );
    no_doc_field("conflict", &v.to_string());
    // A merge into it is refused at its place, before its pin, and nothing
    // in the batch lands.
    let merge = json!({"lww": true, "writes": [
        {"op": "set", "path": "notes/sam3", "data": {"a": 1}},
        {"op": "update", "path": MIA_DOC, "data": {"c": 3}, "if_version": 1},
    ]});
    let (status, v) = sam(&w, "POST", &url, Some(merge)).await;
    assert_eq!(
        (
            status,
            v["error"]["code"].clone(),
            v["error"]["op"].clone(),
            v["error"]["path"].clone()
        ),
        (403, json!("withheld"), json!(1), json!(MIA_DOC)),
        "{v}"
    );
    assert_eq!(v["error"]["authors"][0]["name"], "Mia", "{v}");
    assert_eq!(stored_doc(&w, aid, "notes/sam3").0, None);
}

#[tokio::test]
async fn a_persons_agent_never_writes_documents_as_a_page() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let before = stored_doc(&w, aid, MIA_DOC);
    let writes: Vec<(&str, String, Value)> = vec![
        ("PUT", doc_url(aid, "notes/page"), json!({"data": {"a": 1}})),
        (
            "PATCH",
            doc_url(aid, MIA_DOC),
            json!({"data": {"a": 1}, "lww": true}),
        ),
        (
            "DELETE",
            format!("{}?lww=true", doc_url(aid, MIA_DOC)),
            Value::Null,
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/docs:batch"),
            json!({"writes": [{"op": "set", "path": "notes/page", "data": {"a": 1}}]}),
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/docs:str_replace"),
            json!({"path": "notes/page", "field": "a", "old_str": "x", "new_str": "y"}),
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/docs:acquire"),
            json!({"path": "notes/page", "holder": "h", "data": {"a": 1}}),
        ),
    ];
    for session in [Some(w.sam_session.clone()), None] {
        for (m, path, body) in &writes {
            let mut headers = vec![("x-clax-via", "page".to_string())];
            if let Some(s) = &session {
                headers.push(("x-clax-session", s.clone()));
            }
            let r = w
                .sam
                .call(Call {
                    method: m,
                    path,
                    authorization: Some(format!("Clax-Grant {}", w.sam.access())),
                    body: (!body.is_null()).then(|| serde_json::to_vec(body).unwrap()),
                    headers,
                    ..Default::default()
                })
                .await;
            let (status, v) = json_of(r).await;
            assert_eq!(
                (status, v["error"]["code"].clone()),
                (400, json!("invalid_via_page")),
                "{m} {path} ({session:?}): {v}"
            );
        }
    }
    // Nothing was written: no document, no lease, Mia's untouched.
    assert_eq!(stored_doc(&w, aid, MIA_DOC), before);
    assert_eq!(stored_doc(&w, aid, "notes/page").0, None);
    let leases: i64 = rusqlite::Connection::open(w.ts.home.db_path())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM leases", [], |r| r.get(0))
        .unwrap();
    assert_eq!(leases, 0);
}

/// A merge's outcome depends on the stored body (its 256 KiB check, which
/// fields a delete marker removes), so on a withheld document every merge
/// is refused before it is evaluated: the same request answers the same
/// whatever the document holds, and writes nothing.
#[tokio::test]
async fn a_merge_never_tells_what_a_withheld_document_holds() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    for (p, flag) in [("notes/t", true), ("notes/f", false)] {
        let (s, v) = json_of(
            w.mia
                .agent(
                    "PUT",
                    &doc_url(aid, p),
                    None,
                    Some(json!({"data": {"flag": flag, "text": "hello"}})),
                )
                .await,
        )
        .await;
        assert_eq!(s, 200, "{v}");
    }
    // A pad that fits beside {flag: true} and not beside {flag: false}.
    let base = serde_json::to_vec(&json!({"flag": true, "text": "hello", "pad": ""}))
        .unwrap()
        .len();
    let pad = "a".repeat(clax_core::store::docs::MAX_DOC_BYTES - base);
    let before: Vec<_> = ["notes/t", "notes/f"]
        .iter()
        .map(|p| stored_doc(&w, aid, p))
        .collect();
    let requests: Vec<(&str, String, Value)> = vec![
        (
            "PATCH",
            String::new(),
            json!({"data": {"pad": pad}, "lww": true}),
        ),
        (
            "PATCH",
            String::new(),
            json!({"data": {"flag": {"__delete__": true}, "pad": pad}, "lww": true}),
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/docs:batch"),
            json!({"lww": true, "writes": [{"op": "update", "path": "{p}", "data": {"pad": pad}}]}),
        ),
        (
            "POST",
            format!("/api/artifacts/{aid}/docs:acquire"),
            json!({"path": "{p}", "holder": "h", "data": {"pad": pad}}),
        ),
    ];
    for (m, url, body) in &requests {
        let mut answers = Vec::new();
        for p in ["notes/t", "notes/f"] {
            let url = if url.is_empty() {
                doc_url(aid, p)
            } else {
                url.clone()
            };
            let body: Value = serde_json::from_str(&body.to_string().replace("{p}", p)).unwrap();
            let (status, mut v) = sam(&w, m, &url, Some(body)).await;
            assert_eq!(
                (status, v["error"]["code"].clone()),
                (403, json!("withheld")),
                "{m} {url}: {v}"
            );
            // The batch's error names the caller's own path.
            if let Some(e) = v["error"].as_object_mut() {
                e.remove("path");
                e.remove("message");
            }
            answers.push(v);
        }
        assert_eq!(answers[0], answers[1], "{m} {url}");
    }
    let after: Vec<_> = ["notes/t", "notes/f"]
        .iter()
        .map(|p| stored_doc(&w, aid, p))
        .collect();
    assert_eq!(after, before, "nothing was written");
}

/// A lease on a withheld document is busy whatever holder a person's
/// agent guesses: it never confirms Mia's holder or takes her lease over.
#[tokio::test]
async fn a_lease_on_a_withheld_document_never_confirms_its_holder() {
    let w = World::two_people().await;
    let aid = &w.scratch.artifact;
    let url = format!("/api/artifacts/{aid}/docs:acquire");
    let (s, mine) = json_of(
        w.mia
            .agent(
                "POST",
                &url,
                None,
                Some(json!({"path": MIA_DOC, "holder": "mia-laptop", "ttl_ms": 600000})),
            )
            .await,
    )
    .await;
    assert_eq!((s, mine["acquired"].clone()), (200, json!(true)), "{mine}");
    let (_, miss) = sam(
        &w,
        "POST",
        &url,
        Some(json!({"path": MIA_DOC, "holder": "guess"})),
    )
    .await;
    let (_, hit) = sam(
        &w,
        "POST",
        &url,
        Some(json!({"path": MIA_DOC, "holder": "mia-laptop"})),
    )
    .await;
    assert_eq!(miss, hit);
    assert_eq!(
        (hit["acquired"].clone(), hit["expires_at"].clone()),
        (json!(false), mine["expires_at"].clone()),
        "{hit}"
    );
}

// Feedback: every tier, notices and long-polls (spec §8.3, §8.4, M16, M17).

/// Every canary of Mia's.
fn no_canary_at_all(what: &str, text: &str) {
    for c in crate::common::people::CANARIES {
        assert!(!text.contains(c), "{what} leaked {c}: {text}");
    }
}

/// The placeholder of Mia's thread `tid` on her artifact, whose title the
/// reader may not see either.
fn mias_placeholder(tid: &str) -> String {
    format!(
        "comments from Mia on \"an artifact by Mia\" (thread {tid}) are withheld until your person approves them."
    )
}

/// `text` with JSON's escaped quotes as quotes: a client's output that
/// carries the feedback text inside a JSON string.
fn unescaped(text: &str) -> String {
    text.replace("\\\"", "\"")
}

/// The owner approves Mia everywhere; the approval's ID.
fn owner_approves_mia(w: &World) -> String {
    w.ts.store
        .approve(
            &AuditCtx::DAEMON,
            None,
            &w.mia_public,
            Scope::Everywhere,
            Via::Shell,
        )
        .unwrap()
        .id
}

/// The owner's agent's feedback poll: `query` after `?`.
async fn owner_poll(w: &World, query: &str) -> Value {
    let (status, v) = owner_agent(
        w,
        "GET",
        &format!("/api/sessions/{}/feedback?{query}", w.owner_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v
}

#[tokio::test]
async fn hook_tiers_are_filtered_for_owner_agents() {
    use crate::common::clients::RealClients;
    use crate::common::people::OWNER_ASKS;
    let w = World::two_people().await;
    let tid = w.owner_thread_on_mias_page().await;
    let clients = RealClients::new(&w);
    let follow = clients.follow().await;
    no_canary_at_all("clax feedback follow", &follow.text);
    assert!(
        follow
            .text
            .contains("New comment on \"an artifact by Mia\" (http://"),
        "{}",
        follow.text
    );
    assert!(follow.text.contains(&tid), "{}", follow.text);
    let hooks = clients.hooks().await;
    let mut all = String::new();
    for h in &hooks {
        no_canary_at_all(&h.channel, &h.text);
        all.push_str(&h.text);
    }
    // The owner's comment, each field of Mia's withheld; Mia's comments
    // held, told once.
    let all = unescaped(&all);
    assert_eq!(all.matches(OWNER_ASKS).count(), 1, "{all}");
    for want in [
        "on \"an artifact by Mia\"",
        "Anchored on: (withheld: page by Mia)",
        "Clip: withheld",
    ] {
        assert!(all.contains(want), "{want}: {all}");
    }
    assert_eq!(
        all.matches(&mias_placeholder(&w.read.thread)).count(),
        1,
        "{all}"
    );
}

#[tokio::test]
async fn mcp_tools_are_filtered_for_the_owner() {
    use crate::common::clients::RealClients;
    let w = World::two_people().await;
    let clients = RealClients::new(&w);
    let mcp = clients.mcp_http().await;
    no_canary_at_all("/mcp", &mcp.text);
    for want in ["anchor_withheld", "clip_withheld"] {
        assert!(mcp.text.contains(want), "{want}: {}", mcp.text);
    }
    // The shim of the owner's agent's session: its first tool call hands
    // over the placeholder of Mia's held comments, once.
    let shim = clients.shim().await;
    no_canary_at_all("the MCP shim", &shim.text);
    assert_eq!(
        unescaped(&shim.text)
            .matches(&mias_placeholder(&w.read.thread))
            .count(),
        1,
        "{}",
        shim.text
    );
}

#[tokio::test]
async fn plugin_command_context_is_filtered_for_the_owner() {
    use crate::common::clients::RealClients;
    let w = World::two_people().await;
    let outs = RealClients::new(&w).slash_command_context().await;
    assert!(outs.len() >= 8, "{}", outs.len());
    let mut withheld = false;
    for o in &outs {
        no_canary_at_all(&o.channel, &o.text);
        withheld |= o.text.contains("withheld");
    }
    assert!(withheld, "{outs:?}");
}

#[tokio::test]
async fn a_bump_between_take_and_wait_is_not_missed() {
    let w = World::two_people().await;
    // Told once already (Mia's thread on each seeded artifact): a take
    // finds nothing more to hand over.
    let v = owner_poll(&w, "tier=piggyback").await;
    assert_eq!(v["withheld"].as_array().map(Vec::len), Some(2), "{v}");
    let sid = w.owner_session.clone();
    w.ts.hold_feedback(&sid, true).await;
    let poll = owner_poll(&w, "tier=wait&wait=600");
    let control = async {
        // The poll took (nothing) and stops before it waits: the
        // approval lands in that gap.
        w.ts.wait_held_feedback_polls(&sid, 1).await;
        owner_approves_mia(&w);
        w.ts.hold_feedback(&sid, false).await;
        // A poll that missed the bump would park until its deadline.
        let v: Value =
            w.ts.get_authed(&format!(
                "/api/_test/feedback/{sid}/waiters?parked=wait&until=0"
            ))
            .await
            .json()
            .await
            .unwrap();
        if v["count"] != 0 {
            w.ts.expire_feedback(&sid).await;
        }
        v["count"].clone()
    };
    let (v, parked) = tokio::join!(poll, control);
    assert_eq!(parked, 0, "the poll missed the approval: {v}");
    let text = v["text"].as_str().unwrap_or_default();
    assert!(text.contains(COMMENT) && text.contains(REPLY), "{v}");
}

#[tokio::test]
async fn approval_wakes_a_waiting_poll() {
    let w = World::two_people().await;
    let v = owner_poll(&w, "tier=piggyback").await;
    assert!(v["feedback"].as_array().unwrap().is_empty(), "{v}");
    let sid = w.owner_session.clone();
    let poll = owner_poll(&w, "tier=wait&wait=600");
    let control = async {
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        owner_approves_mia(&w);
    };
    let (v, ()) = tokio::join!(poll, control);
    let bodies: Vec<&str> = v["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, [COMMENT, REPLY, COMMENT, REPLY], "{v}");
    assert!(v.get("withheld").is_none(), "{v}");
}

#[tokio::test]
async fn revocation_during_wait_for_feedback_holds_rows() {
    let w = World::two_people().await;
    let id = owner_approves_mia(&w);
    // Approved: Mia's comments on both seeded artifacts are delivered.
    let v = owner_poll(&w, "tier=piggyback").await;
    assert_eq!(v["feedback"].as_array().unwrap().len(), 4, "{v}");
    let sid = w.owner_session.clone();
    let late = "Mia's late words";
    let poll = owner_poll(&w, "tier=wait&wait=600");
    let control = async {
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        w.ts.store
            .revoke_approval(&AuditCtx::DAEMON, &id, None)
            .unwrap();
        // The revocation woke the poll; it took nothing and waits again.
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        w.mia_comments(&w.read.artifact, &w.read.thread, late).await;
    };
    let (v, ()) = tokio::join!(poll, control);
    assert!(v["feedback"].as_array().unwrap().is_empty(), "{v}");
    let text = v["text"].as_str().unwrap();
    assert!(!text.contains(late), "{text}");
    assert!(
        text.contains(&format!(
            "[clax] 1 comment from Mia on \"an artifact by Mia\" (thread {}) is withheld until your person approves it.",
            w.read.thread
        )),
        "{text}"
    );
    assert_eq!(v["withheld"][0]["count"], 1, "{v}");
    // Held, not dropped: the thread waits on the owner's approval.
    let state =
        w.ts.store
            .feedback_state(&w.read.thread, false)
            .unwrap()
            .unwrap();
    assert_eq!(state.state, clax_core::FeedbackPhase::Withheld);
}

#[tokio::test]
async fn withheld_matrix_tiers() {
    use crate::common::clients::RealClients;
    use crate::common::people::OWNER_ASKS;
    // Each in-band tier, through its real client: no canary before the
    // approval and Mia's placeholder exactly once; her words after it.
    for tier in ["piggyback", "stop", "prompt", "wait", "inject"] {
        let w = World::two_people().await;
        let first = unescaped(&tier_output(&w, tier).await);
        no_canary_at_all(tier, &first);
        assert_eq!(
            first.matches(&mias_placeholder(&w.read.thread)).count(),
            1,
            "{tier}: {first}"
        );
        let second = unescaped(&tier_output(&w, tier).await);
        no_canary_at_all(tier, &second);
        assert!(!second.contains("are withheld until"), "{tier}: {second}");
        owner_approves_mia(&w);
        w.mia_comments(&w.read.artifact, &w.read.thread, "Mia, approved")
            .await;
        let after = tier_output(&w, tier).await;
        assert!(after.contains("Mia, approved"), "{tier}: {after}");
        assert!(after.contains(COMMENT), "{tier}: {after}");
    }
    // Notices through `clax feedback follow`: Mia's comment is announced
    // only once approved; the owner's is announced under a withheld title.
    let w = World::two_people().await;
    w.owner_thread_on_mias_page().await;
    let clients = RealClients::new(&w);
    let n = clients.follow().await.text;
    no_canary_at_all("follow", &n);
    assert!(!n.contains(&w.read.thread), "{n}");
    owner_approves_mia(&w);
    w.mia_comments(&w.read.artifact, &w.read.thread, "Mia, approved")
        .await;
    let n = clients.follow().await.text;
    assert!(n.contains(&w.read.thread) && n.contains(TITLE), "{n}");
    assert!(!n.contains(OWNER_ASKS), "a notice carries no comment: {n}");
}

/// What the owner's agent is handed through `tier`'s client: the MCP
/// shim's tool calls (`piggyback`), the hook binary (`stop`, `prompt`), or
/// the poll `wait_for_feedback` and Pi's injection loop make (`wait`,
/// `inject`).
async fn tier_output(w: &World, tier: &str) -> String {
    use crate::common::clients::RealClients;
    match tier {
        "piggyback" => RealClients::new(w).shim().await.text,
        "stop" | "prompt" => RealClients::new(w).hook(tier).await.text,
        _ => owner_poll(w, &format!("tier={tier}&wait=0"))
            .await
            .to_string(),
    }
}

#[tokio::test]
async fn the_codex_queue_sends_only_filtered_visible_rows() {
    use crate::common::people::OWNER_ASKS;
    let dir = tempfile::tempdir().unwrap();
    // Each run writes its arguments into a FIFO, which the test reads:
    // reading waits for a run, with no clock.
    let fifo = dir.path().join("args.fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let bin = clax_fake_exe::install(
        &dir.path().join("codex"),
        "#!/bin/sh\nd=\"$(dirname \"$0\")\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$d/args.fifo\"\nexit 0\n",
    );
    let w = World::two_people_with(move |s| {
        s.codex = std::sync::Arc::new(clax_server::push::CodexPush {
            bin: Some(bin),
            timeout: std::time::Duration::from_secs(10),
            source: clax_server::push::CodexSource::Env,
            ..Default::default()
        })
    })
    .await;
    let ts = &w.ts;
    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "codex", "parent_pid": 4242, "harness_session_id": "cx-1", "cwd": "/w"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let cx = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = &w.read.artifact;
    let r = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{cx}/watches/{aid}", ts.base)),
        )
        .header("x-clax-session", &cx)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let queued = || {
        let fifo = fifo.clone();
        async move {
            let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo));
            tokio::time::timeout(std::time::Duration::from_secs(60), read)
                .await
                .expect("codex queue ran")
                .unwrap()
                .unwrap()
        }
    };
    // Mia's comment is held: nothing queued. The owner's comment on her
    // page is queued, each field of hers withheld.
    w.mia_comments(aid, &w.read.thread, "Mia, unapproved").await;
    w.owner_thread_on_mias_page().await;
    let text = queued().await;
    no_canary_at_all("codex queue", &text);
    assert!(!text.contains("Mia, unapproved"), "{text}");
    assert!(text.contains(OWNER_ASKS), "{text}");
    assert!(
        text.contains("Anchored on: (withheld: page by Mia)"),
        "{text}"
    );
    assert!(
        !text.contains("are withheld until"),
        "queue tells of none: {text}"
    );
    owner_approves_mia(&w);
    w.mia_comments(aid, &w.read.thread, "Mia, approved").await;
    let text = queued().await;
    assert!(
        text.contains("Mia, approved") && text.contains("Mia, unapproved"),
        "{text}"
    );
}

/// Spec §11 "Display": an agent's reply reads `<name> (agent, via
/// <harness>)` to its own person's agents and `<name> (<person>'s agent,
/// via <harness>)` to other people's; people's own views keep the stored
/// author name.
#[tokio::test]
async fn an_agent_reply_names_the_agent_its_person_and_its_harness() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    let aid = w.ts.any_artifact().await;
    let mut tids = Vec::new();
    for body in ["for the owner's agent", "for Sam's agent"] {
        let tid = owner_thread(&w.ts, &aid, 1, "index.html", body).await;
        w.ts.send_thread(&aid, &tid).await;
        tids.push(tid);
    }
    let reply = json!({"body": "done", "author_kind": "agent"});
    let path = |tid: &str| format!("/api/artifacts/{aid}/threads/{tid}/comments");
    let (status, mine) = owner_agent(&w, "POST", &path(&tids[0]), Some(reply.clone())).await;
    assert_eq!(status, 201, "{mine}");
    let (status, sams) = sam(&w, "POST", &path(&tids[1]), Some(reply)).await;
    assert_eq!(status, 201, "{sams}");
    let name = |sid: &str| w.ts.store.get_session(sid).unwrap().unwrap().name.unwrap();
    let (owner_agent_name, sam_agent_name) = (name(&w.owner_session), name(&w.sam_session));
    let owner_name = w.ts.store.agent_person(None).unwrap().name;
    assert_eq!(
        mine["comment"]["author_name"],
        format!("{owner_agent_name} (agent, via claude)"),
        "{mine}"
    );
    assert_eq!(
        sams["comment"]["author_name"],
        format!("{sam_agent_name} (agent, via claude)"),
        "{sams}"
    );
    let replies = |v: &Value| -> Vec<String> {
        tids.iter()
            .map(|tid| {
                let t = v["threads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["id"] == json!(tid))
                    .unwrap_or_else(|| panic!("{tid}: {v}"));
                t["comments"][1]["author_name"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    let list = format!("/api/artifacts/{aid}/threads");
    let (status, v) = owner_agent(&w, "GET", &list, None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        replies(&v),
        [
            format!("{owner_agent_name} (agent, via claude)"),
            format!("{sam_agent_name} (Sam's agent, via claude)"),
        ]
    );
    let (status, v) = sam(&w, "GET", &list, None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        replies(&v),
        [
            format!("{owner_agent_name} ({owner_name}'s agent, via claude)"),
            format!("{sam_agent_name} (agent, via claude)"),
        ]
    );
    // Agents read the rendered line, with no `agent` object.
    assert!(
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["comments"][1].get("agent").is_none()),
        "{v}"
    );
    // People's views keep the stored name, with the agent's name and
    // person for the shell to render the line by its reader.
    let v: Value = w.ts.owner_browser().get(&list).await.json().await.unwrap();
    assert_eq!(replies(&v), ["claude", "claude"]);
    let agent = |tid: &str| {
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == json!(tid))
            .unwrap()["comments"][1]["agent"]
            .clone()
    };
    assert_eq!(
        agent(&tids[0]),
        json!({"name": owner_agent_name, "person": null, "person_name": owner_name})
    );
    assert_eq!(
        agent(&tids[1]),
        json!({"name": sam_agent_name, "person": sam_public(&w), "person_name": "Sam"})
    );
}

/// Spec §7.5: feedback on an artifact a person's agent may no longer read
/// is held, and each in-band tier tells the agent once that it no longer
/// has access to the page, with nothing of the page or its comments.
#[tokio::test]
async fn a_persons_agent_that_may_no_longer_read_a_page_is_told_once_on_every_tier() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait", "inject"] {
        let w = World::two_people().await;
        sam_rights(&w, "clax:full");
        let aid = w.read.artifact.clone();
        let (status, v) = sam(
            &w,
            "PUT",
            &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
            None,
        )
        .await;
        assert_eq!(status, 200, "{v}");
        w.mia_comments(&aid, &w.read.thread, "Mia to Sam").await;
        // Sam's agents may now read another artifact only.
        let rights = clax_core::rights::Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": [w.scratch.artifact]},
            {"type": "clax-comments", "actions": ["read", "reply", "watch"], "locations": ["*"]},
            {"type": "clax-ask", "actions": ["ask"], "locations": ["*"]}
        ]))
        .unwrap();
        w.ts.store
            .set_person_rights(&AuditCtx::DAEMON, &sam_public(&w), Some(rights))
            .unwrap();
        let poll = || async {
            let (status, v) = sam(
                &w,
                "GET",
                &format!(
                    "/api/sessions/{}/feedback?tier={tier}&wait=0",
                    w.sam_session
                ),
                None,
            )
            .await;
            assert_eq!(status, 200, "{tier}: {v}");
            v
        };
        let v = poll().await;
        let text = v.to_string();
        no_canary_at_all(tier, &text);
        assert!(!text.contains("Mia to Sam"), "{tier}: {text}");
        assert_eq!(v["feedback"], json!([]), "{tier}: {v}");
        assert_eq!(
            v["text"],
            json!(format!(
                "[clax] 1 comment in thread {} is held: you no longer have access to this page.",
                w.read.thread
            )),
            "{tier}: {v}"
        );
        assert_eq!(
            v["withheld"],
            json!([{"thread_id": w.read.thread, "artifact_id": aid, "count": 1,
                    "asked": false, "no_access": true}]),
            "{tier}: {v}"
        );
        // Told once.
        let v = poll().await;
        assert_eq!(v["text"], Value::Null, "{tier}: {v}");
        assert!(v.get("withheld").is_none(), "{tier}: {v}");
        // The owner reads the hold; Mia, its author, reads it as sent.
        let state = |v: Value| v["thread"]["feedback_state"]["state"].clone();
        let one = format!("/api/artifacts/{aid}/threads/{}", w.read.thread);
        let owner: Value = w.ts.owner_browser().get(&one).await.json().await.unwrap();
        assert_eq!(state(owner), json!("no_access"), "{tier}");
        let r = w.mia_browser(w.ts.client.get(format!("{}{one}", w.ts.base)));
        let mia: Value = r.send().await.unwrap().json().await.unwrap();
        assert_eq!(state(mia), json!("sent"), "{tier}");
        // Readable again: Mia's words are withheld, and told so; approved,
        // they are delivered.
        sam_rights(&w, "clax:full");
        let v = poll().await;
        assert!(
            v["text"].as_str().unwrap_or_default().contains(&format!(
                "1 comment from Mia on \"an artifact by Mia\" (thread {}) is withheld",
                w.read.thread
            )),
            "{tier}: {v}"
        );
        let owner: Value = w.ts.owner_browser().get(&one).await.json().await.unwrap();
        assert_eq!(state(owner), json!("withheld"), "{tier}");
        approve(&w.ts, Some(&sam_public(&w)), &w.mia_public);
        let v = poll().await;
        assert!(v.to_string().contains("Mia to Sam"), "{tier}: {v}");
    }
}

#[tokio::test]
async fn a_persons_agent_gets_placeholders_and_never_takes_untargeted_feedback_by_watching() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    let aid = w.read.artifact.clone();
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    w.mia_comments(&aid, &w.read.thread, "Mia to Sam").await;
    let (status, v) = sam(
        &w,
        "GET",
        &format!("/api/sessions/{}/feedback?tier=piggyback", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let text = v.to_string();
    no_canary_at_all("Sam's poll", &text);
    assert!(!text.contains("Mia to Sam"), "{text}");
    assert_eq!(v["withheld"][0]["count"], 1, "{v}");
    assert_eq!(v["withheld"][0]["title"], "an artifact by Mia", "{v}");
    let (status, v) = sam(
        &w,
        "GET",
        &format!("/api/sessions/{}/notices", w.sam_session),
        None,
    )
    .await;
    assert_eq!((status, v["notices"].clone()), (200, json!([])), "{v}");
    // The owner's untargeted feedback stays where it is when Sam's agent
    // watches, as when it publishes.
    let other = w.ts.any_artifact().await;
    let tid = w.ts.thread(&other, 1, "for any agent").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    w.ts.send_thread(&other, &tid).await;
    let untargeted = || -> i64 {
        let db = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db.query_row(
            "SELECT COUNT(*) FROM feedback WHERE thread_id = ?1 AND target_session_id IS NULL",
            [&tid],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(untargeted(), 1);
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{other}", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        untargeted(),
        1,
        "Sam's agent took the owner's agents' feedback"
    );
    let (status, v) = owner_agent(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{other}", w.owner_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(untargeted(), 0);
}

#[tokio::test]
async fn no_doc_event_reaches_a_waiting_agent() {
    use crate::common::people::DOC_FIELD;
    let w = World::two_people().await;
    // Mia's held comments told; the wait finds nothing more.
    owner_poll(&w, "tier=piggyback").await;
    let sid = w.owner_session.clone();
    let poll = owner_poll(&w, "tier=wait&wait=600");
    let control = async {
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        let r = w
            .mia
            .agent(
                "PUT",
                &format!(
                    "/api/artifacts/{}/docs/notes/MIACANARYwhilewaiting",
                    w.read.artifact
                ),
                Some(&w.mia_session),
                Some(json!({"data": {"text": DOC_FIELD}})),
            )
            .await;
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        w.ts.expire_feedback(&sid).await;
    };
    let (v, ()) = tokio::join!(poll, control);
    let text = v.to_string();
    no_canary_at_all("the wait", &text);
    assert!(!text.contains("MIACANARYwhilewaiting"), "{text}");
    // The event channels refuse agents outright.
    for path in ["/api/events", "/api/stream"] {
        let (status, v) = owner_agent(&w, "GET", path, None).await;
        assert_eq!(
            (status, v["error"]["code"].clone()),
            (403, json!("not_for_agents")),
            "{path}"
        );
        let r = w.sam.get(Some(&w.sam_session), path).await;
        assert_eq!(r.status(), 403, "{path}");
    }
}

#[tokio::test]
async fn rights_narrowed_during_a_wait_hold_rows_on_artifacts_no_longer_readable_and_tell_once() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    // Sam approved the owner: the owner's words reach Sam's agent.
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = w.ts.any_artifact().await;
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let sid = w.sam_session.clone();
    let (status, v) = sam(
        &w,
        "GET",
        &format!("/api/sessions/{sid}/feedback?tier=wait&wait=0"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let path = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
    let poll = sam(&w, "GET", &path, None);
    let control = async {
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        // Sam may read another artifact only: the generation moves.
        narrow_sam(&w, Some(&w.read.artifact));
        w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        let tid = owner_thread(&w.ts, &aid, 1, "index.html", "the owner, to Sam's agent").await;
        w.ts.send_thread(&aid, &tid).await;
        // The send woke the poll; it held the row and told of it.
        tid
    };
    let ((status, v), tid) = tokio::join!(poll, control);
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["feedback"], json!([]), "{v}");
    assert!(!v.to_string().contains("the owner, to Sam's agent"), "{v}");
    assert_eq!(
        v["text"],
        json!(format!(
            "[clax] 1 comment in thread {tid} is held: you no longer have access to this page."
        )),
        "{v}"
    );
    // Readable again: the row is delivered.
    narrow_sam(&w, None);
    let (_, v) = sam(
        &w,
        "GET",
        &format!("/api/sessions/{sid}/feedback?tier=piggyback"),
        None,
    )
    .await;
    assert_eq!(v["feedback"][0]["body"], "the owner, to Sam's agent", "{v}");
}

#[tokio::test]
async fn rights_narrowed_before_a_polls_first_take_hold_its_rows() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = w.ts.any_artifact().await;
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let sid = w.sam_session.clone();
    w.ts.hold_feedback_at(&sid, true, "first_take").await;
    let path = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
    let poll = sam(&w, "GET", &path, None);
    let control = async {
        // The poll authenticated with Sam's full rights; it narrows before
        // the poll reads the generation, so no later bump tells it.
        w.ts.wait_held_feedback_polls(&sid, 1).await;
        narrow_sam(&w, Some(&w.read.artifact));
        let tid = owner_thread(&w.ts, &aid, 1, "index.html", "the owner, to Sam's agent").await;
        w.ts.send_thread(&aid, &tid).await;
        w.ts.hold_feedback_at(&sid, false, "first_take").await;
        // Held and told; a poll on the old rights would have answered
        // with the row.
    };
    let ((status, v), ()) = tokio::join!(poll, control);
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["feedback"], json!([]), "{v}");
    assert!(!v.to_string().contains("the owner, to Sam's agent"), "{v}");
    assert_eq!(v["withheld"][0]["no_access"], true, "{v}");
}

#[tokio::test]
async fn a_row_between_take_and_wait_is_not_missed() {
    use crate::common::people::OWNER_ASKS;
    let w = World::two_people().await;
    // Told once already: a take finds nothing more to hand over.
    owner_poll(&w, "tier=piggyback").await;
    let sid = w.owner_session.clone();
    w.ts.hold_feedback(&sid, true).await;
    let poll = owner_poll(&w, "tier=wait&wait=600");
    let control = async {
        // The poll took nothing and stops before it waits: a row the
        // owner's agent may read lands in that gap.
        w.ts.wait_held_feedback_polls(&sid, 1).await;
        w.owner_thread_on_mias_page().await;
        w.ts.hold_feedback(&sid, false).await;
        let v: Value =
            w.ts.get_authed(&format!(
                "/api/_test/feedback/{sid}/waiters?parked=wait&until=0"
            ))
            .await
            .json()
            .await
            .unwrap();
        if v["count"] != 0 {
            w.ts.expire_feedback(&sid).await;
        }
        v["count"].clone()
    };
    let (v, parked) = tokio::join!(poll, control);
    assert_eq!(parked, 0, "the poll missed the row: {v}");
    assert!(
        v["text"].as_str().unwrap_or_default().contains(OWNER_ASKS),
        "{v}"
    );
}

#[tokio::test]
async fn a_revocation_between_view_and_take_withholds_what_follows_it() {
    let w = World::two_people().await;
    let id = owner_approves_mia(&w);
    owner_poll(&w, "tier=piggyback").await;
    let sid = w.owner_session.clone();
    let late = "Mia, after the revocation";
    w.ts.hold_feedback_at(&sid, true, "before_take").await;
    let poll = owner_poll(&w, "tier=piggyback");
    let control = async {
        // The poll built its view with Mia approved and stops before the
        // take: the revocation and Mia's next comment land in that gap.
        w.ts.wait_held_feedback_polls(&sid, 1).await;
        w.ts.store
            .revoke_approval(&AuditCtx::DAEMON, &id, None)
            .unwrap();
        w.mia_comments(&w.read.artifact, &w.read.thread, late).await;
        w.ts.hold_feedback_at(&sid, false, "before_take").await;
    };
    let (v, ()) = tokio::join!(poll, control);
    assert!(!v.to_string().contains(late), "{v}");
    assert_eq!(v["feedback"], json!([]), "{v}");
    assert_eq!(v["withheld"][0]["count"], 1, "{v}");
}

#[tokio::test]
async fn an_acknowledgement_needs_read_rights_on_the_artifact() {
    let w = World::two_people().await;
    sam_rights(&w, "clax:full");
    approve(&w.ts, Some(&sam_public(&w)), &owner_public(&w.ts));
    let aid = w.ts.any_artifact().await;
    let (status, v) = sam(
        &w,
        "PUT",
        &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let tid = owner_thread(&w.ts, &aid, 1, "index.html", "for Sam's agent").await;
    w.ts.send_thread(&aid, &tid).await;
    narrow_sam(&w, Some(&w.read.artifact));
    let ack = format!("/api/sessions/{}/feedback/ack", w.sam_session);
    let (status, v) = sam(&w, "POST", &ack, Some(json!({"thread_ids": [tid]}))).await;
    assert_eq!((status, v["acknowledged"].clone()), (200, json!(0)), "{v}");
    let rows = w.ts.store.feedback_rows(&tid).unwrap();
    let mine = rows
        .iter()
        .find(|f| f.target_session_id.as_deref() == Some(&w.sam_session))
        .unwrap();
    assert!(mine.delivered_at.is_none() && mine.acknowledged_at.is_none());
    narrow_sam(&w, None);
    let (status, v) = sam(&w, "POST", &ack, Some(json!({"thread_ids": [tid]}))).await;
    assert_eq!((status, v["acknowledged"].clone()), (200, json!(1)), "{v}");
}
