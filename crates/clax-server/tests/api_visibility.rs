//! What agents see of threads (spec §7.5 thread rows, §8.1–§8.3, M10):
//! each field on its own, in reads and write responses alike, with the
//! placeholders of §8.3; a withheld clip is not fetchable; a person's
//! agent writes comments from a session only, as its person; people are
//! never filtered.

use crate::common::TestServer;
use crate::common::people::{
    BATCH_NOTE, CLIP, COMMENT, FILE_NAME, PREFIX, QUOTE, REPLY, SELECTOR, SUFFIX, World,
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
