//! Draft artifacts over HTTP (spec 2026-10-12-flutter-app-round2-design
//! §5.1): `POST/GET /api/drafts`, `DELETE /api/drafts/<id>`, the document
//! copy, publishing to a draft, reads through its app's grants, and the
//! routes and streams that never show one.

use crate::common;
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_core::db::{Caller, Level};
use clax_core::store::docs::{BatchOp, BatchWrite};
use clax_server::testing::EventReader;
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

const READER: &str = "x-clax-reader";

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

/// A request with the token.
fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

/// Publishes an app declaring `caps`; its ID.
async fn app(ts: &TestServer, title: &str, caps: Value) -> String {
    let (s, v) = send(authed(ts, Method::POST, "/api/artifacts").json(&json!({
        "title": title,
        "capabilities": caps,
        "files": {"index.html": {"content": "<p>app</p>"}}
    })))
    .await;
    assert_eq!(s, 201, "{v}");
    v["artifact"]["id"].as_str().unwrap().to_string()
}

/// Writes `data` at `path` of `aid` with the token.
async fn seed(ts: &TestServer, aid: &str, path: &str, data: Value) -> Value {
    let (s, v) = send(
        authed(
            ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/{path}"),
        )
        .json(&json!({"data": data, "lww": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

/// `POST /api/drafts` with `body`; the status and answer.
async fn draft_raw(ts: &TestServer, body: Value) -> (u16, Value) {
    send(authed(ts, Method::POST, "/api/drafts").json(&body)).await
}

/// A draft of `of` (or of a new app) declaring `caps`; its `draft` object.
async fn draft(ts: &TestServer, of: Option<&str>, caps: Value) -> Value {
    let (s, v) = draft_raw(
        ts,
        json!({"of": of, "title": "Draft", "capabilities": caps, "files": {"index.html": "<p>draft</p>"}}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["draft"].clone()
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// Publishes a version of `aid` at `if_version` with the token.
async fn publish(ts: &TestServer, aid: &str, if_version: u32) -> Value {
    let (s, v) = send(
        authed(ts, Method::POST, &format!("/api/artifacts/{aid}/versions")).json(&json!({
            "if_version": if_version,
            "files": {"index.html": {"content": format!("<p>v{}</p>", if_version + 1)}}
        })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v
}

/// The artifact IDs `GET /api/artifacts<query>` lists, with the token.
async fn listed(ts: &TestServer, query: &str) -> Vec<String> {
    let (s, v) = send(authed(ts, Method::GET, &format!("/api/artifacts{query}"))).await;
    assert_eq!(s, 200, "{v}");
    v["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect()
}

/// Opens `/api/stream` with the token and subscribes it to `topics`.
async fn stream_with_token(ts: &TestServer, topics: &[&str]) -> EventReader {
    let res = authed(ts, Method::GET, "/api/stream").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = send(
        authed(ts, Method::POST, &format!("/api/stream/{sid}")).json(&json!({"subscribe": topics})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    events
}

/// The next event naming an artifact: its name and artifact ID.
async fn next_about(r: &mut EventReader) -> (String, String) {
    loop {
        let (name, data) = r.next().await;
        if let Some(aid) = data["artifact_id"].as_str() {
            return (name, aid.to_string());
        }
    }
}

#[tokio::test]
async fn draft_copies_docs_and_stays_hidden() {
    let ts = TestServer::spawn().await;
    let a = app(&ts, "Tracker", json!({"db": {}})).await;
    seed(&ts, &a, "notes/n1", json!({"t": "one"})).await;
    seed(&ts, &a, "runs/r1", json!({"t": "run"})).await;
    let viewer = ts.viewer(Some("V")).await;
    let private = format!(
        "/api/artifacts/{a}/docs/data/users/{}/pick",
        viewer.public_id
    );
    let (s, _) = send(
        ts.client
            .put(format!("{}{private}", ts.base))
            .header("cookie", format!("clax_viewer={}", viewer.cookie))
            .json(&json!({"data": {"n": 1}, "lww": true})),
    )
    .await;
    assert_eq!(s, 200);
    let (_, inbox_before) = send(authed(&ts, Method::GET, "/api/inbox")).await;
    let mut events = ts.events_with("", |r| r.bearer_auth(&ts.token)).await;

    let d = draft(&ts, Some(&a), json!({"db": {}})).await;
    assert_eq!(
        (
            d["of"].as_str(),
            d["version"].as_u64(),
            d["docs_copied"].as_u64(),
            d["docs_truncated"].as_bool()
        ),
        (Some(a.as_str()), Some(1), Some(2), Some(false)),
        "{d}"
    );
    let did = id(&d);
    let (s, v) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{did}/docs/notes/n1"),
    ))
    .await;
    assert_eq!(
        (
            s,
            v["doc"]["data"]["t"].as_str(),
            v["doc"]["version"].as_u64()
        ),
        (200, Some("one"), Some(1)),
        "{v}"
    );
    assert_eq!(listed(&ts, "").await, vec![a.clone()]);
    let (s, v) = send(authed(&ts, Method::GET, &format!("/api/artifacts/{did}"))).await;
    assert_eq!(
        s, 200,
        "a draft is read and rendered like any artifact: {v}"
    );

    publish(&ts, &did, 1).await;
    publish(&ts, &a, 1).await;
    assert_eq!(
        next_about(&mut events).await,
        ("version".to_string(), a.clone()),
        "the unfiltered stream skips the draft"
    );
    let (_, inbox_after) = send(authed(&ts, Method::GET, "/api/inbox")).await;
    assert_eq!(inbox_after["items"], inbox_before["items"]);

    let (s, v) = send(authed(&ts, Method::GET, "/api/drafts")).await;
    assert_eq!(s, 200);
    let drafts = v["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 1, "{v}");
    assert_eq!(
        (drafts[0]["id"].as_str(), drafts[0]["of"].as_str()),
        (Some(did.as_str()), Some(a.as_str()))
    );
    assert!(drafts[0]["created_at"].is_string());
}

#[tokio::test]
async fn a_new_apps_draft_copies_nothing() {
    let ts = TestServer::spawn().await;
    let d = draft(&ts, None, json!({})).await;
    assert_eq!(
        (
            d["of"].clone(),
            d["docs_copied"].as_u64(),
            d["docs_truncated"].as_bool()
        ),
        (Value::Null, Some(0), Some(false))
    );
    let (s, v) = draft_raw(
        &ts,
        json!({"title": "x", "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}}),
    )
    .await;
    assert_eq!(s, 201, "a publish's file objects are taken too: {v}");
    let (s, v) = draft_raw(
        &ts,
        json!({"of": id(&d), "title": "x", "files": {"index.html": "x"}}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_args")),
        "{v}"
    );
    let (s, _) = draft_raw(
        &ts,
        json!({"of": "zzzzzzzzzzzz", "title": "x", "files": {"index.html": "x"}}),
    )
    .await;
    assert_eq!(s, 404);
    let (s, v) = draft_raw(&ts, json!({"title": "x", "files": {}})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("missing_index"))
    );
}

#[tokio::test]
async fn draft_routes_need_the_token() {
    let ts = TestServer::spawn().await;
    let d = draft(&ts, None, json!({})).await;
    let url = |p: &str| format!("{}{p}", ts.base);
    for (m, p) in [
        (Method::POST, "/api/drafts".to_string()),
        (Method::GET, "/api/drafts".to_string()),
        (Method::DELETE, format!("/api/drafts/{}", id(&d))),
    ] {
        let (s, _) = send(ts.client.request(m.clone(), url(&p)).json(&json!({
            "title": "x", "files": {"index.html": "x"}
        })))
        .await;
        assert_eq!(s, 401, "{m} {p}");
    }
}

#[tokio::test]
async fn draft_writes_never_touch_the_app() {
    let ts = TestServer::spawn().await;
    let a = app(&ts, "Tracker", json!({"db": {}})).await;
    let before = seed(&ts, &a, "notes/n1", json!({"t": "app"})).await;
    let did = id(&draft(&ts, Some(&a), json!({"db": {}})).await);
    let (s, v) = send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{did}/docs/notes/n1"),
        )
        .json(&json!({"data": {"t": "draft"}, "if_version": 1})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    seed(&ts, &did, "notes/n2", json!({"t": "new"})).await;
    let (s, v) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{a}/docs/notes/n1"),
    ))
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["doc"], before["doc"], "the app's document is untouched");
    let (s, _) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{a}/docs/notes/n2"),
    ))
    .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn draft_copy_stops_at_cap() {
    let ts = TestServer::spawn().await;
    let a = app(&ts, "Big", json!({"db": {}})).await;
    let aid = ArtifactId::parse(&a).unwrap();
    let owner = Caller {
        level: Level::Owner,
        viewer: None,
    };
    let paths: Vec<String> = (0..2001).map(|i| format!("items/i{i:04}")).collect();
    for chunk in paths.chunks(50) {
        let writes = chunk
            .iter()
            .map(|p| BatchWrite {
                path: p.clone(),
                op: BatchOp::Set(json!({"n": 1})),
                if_version: None,
            })
            .collect();
        ts.store
            .doc_batch(&AuditCtx::DAEMON, &aid, writes, true, &owner)
            .unwrap();
    }
    let d = draft(&ts, Some(&a), json!({"db": {}})).await;
    assert_eq!(
        (d["docs_copied"].as_u64(), d["docs_truncated"].as_bool()),
        (Some(2000), Some(true))
    );
}

#[tokio::test]
async fn threads_on_a_draft_are_refused() {
    let ts = TestServer::spawn().await;
    let did = id(&draft(&ts, None, json!({})).await);
    let res = ts.create_thread(&did, 1, "@agent this draft", None).await;
    assert_eq!(res.status(), 403);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "draft", "{v}");
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{did}/threads", ts.base)),
        )
        .multipart(
            reqwest::multipart::Form::new()
                .text("anchor", clax_server::testing::element_anchor().to_string())
                .text("body", "x")
                .text("version", "1"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "the token too");
}

#[tokio::test]
async fn a_draft_is_published_without_a_session_and_never_watched() {
    let ts = TestServer::spawn().await;
    let did = id(&draft(&ts, None, json!({})).await);
    let v = publish(&ts, &did, 1).await;
    assert_eq!(v["version"]["n"], 2);
    let sess = ts.register_session("claude", "h-draft").await;
    let sid = sess["id"].as_str().unwrap();
    let (s, v) = send(
        authed(&ts, Method::POST, &format!("/api/artifacts/{did}/versions"))
            .header("x-clax-session", sid)
            .json(&json!({"if_version": 2, "files": {"index.html": {"content": "<p>3</p>"}}})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("draft")),
        "{v}"
    );
    let (s, v) = send(authed(
        &ts,
        Method::PUT,
        &format!("/api/sessions/{sid}/watches/{did}"),
    ))
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("draft")),
        "{v}"
    );
}

#[tokio::test]
async fn draft_reads_another_app_with_ofs_grant() {
    let ts = TestServer::spawn().await;
    let b = app(&ts, "Source", json!({"db": {}})).await;
    seed(&ts, &b, "notes/n1", json!({"t": "b"})).await;
    let caps = json!({"db": {"read": [b.clone()]}});
    let a = app(&ts, "Reader", caps.clone()).await;
    let did = id(&draft(&ts, Some(&a), caps).await);
    let read = || {
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{b}/docs/notes/n1"),
        )
        .header(READER, &did)
    };
    let (s, v) = send(read()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_granted")),
        "{v}"
    );
    let (s, _) = send(authed(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{a}/db-grants/{b}"),
    ))
    .await;
    assert!((200..300).contains(&s), "{s}");
    let (s, v) = send(read()).await;
    assert_eq!((s, v["doc"]["data"]["t"].as_str()), (200, Some("b")), "{v}");
    let (s, _) = send(authed(
        &ts,
        Method::DELETE,
        &format!("/api/artifacts/{a}/db-grants/{b}"),
    ))
    .await;
    assert!((200..300).contains(&s), "{s}");
    let (s, v) = send(read()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_granted")),
        "{v}"
    );
}

#[tokio::test]
async fn delete_and_sweep_remove_everything() {
    let ts = TestServer::spawn().await;
    let a = app(&ts, "Tracker", json!({"db": {}})).await;
    seed(&ts, &a, "notes/n1", json!({})).await;
    let did = id(&draft(&ts, Some(&a), json!({"db": {}})).await);
    let dir = ts.home.artifact_dir(&ArtifactId::parse(&did).unwrap());
    assert!(dir.exists());
    let (s, v) = send(authed(&ts, Method::DELETE, &format!("/api/drafts/{a}"))).await;
    assert_eq!(s, 404, "an app is no draft: {v}");
    let (s, v) = send(authed(&ts, Method::DELETE, &format!("/api/drafts/{did}"))).await;
    assert_eq!((s, v["deleted"].as_bool()), (200, Some(true)), "{v}");
    assert!(!dir.exists());
    for p in [
        format!("/api/artifacts/{did}"),
        format!("/api/artifacts/{did}/docs/notes/n1"),
    ] {
        assert_eq!(send(authed(&ts, Method::GET, &p)).await.0, 404, "{p}");
    }
    let (_, v) = send(authed(&ts, Method::GET, "/api/drafts")).await;
    assert_eq!(v["drafts"], json!([]));
    let (s, _) = send(authed(&ts, Method::DELETE, &format!("/api/drafts/{did}"))).await;
    assert_eq!(s, 404);
    let (s, v) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{a}/docs/notes/n1"),
    ))
    .await;
    assert_eq!(s, 200, "the app keeps its documents: {v}");

    let old = draft(&ts, Some(&a), json!({"db": {}})).await;
    let oid = ArtifactId::parse(&id(&old)).unwrap();
    let (_, v) = send(authed(&ts, Method::GET, "/api/drafts")).await;
    let created = v["drafts"][0]["created_at"].as_str().unwrap().to_string();
    let created = chrono::DateTime::parse_from_rfc3339(&created)
        .unwrap()
        .to_utc();
    let max_age = clax_core::store::drafts::DRAFT_MAX_AGE;
    let kept = ts
        .store
        .sweep_drafts(
            &AuditCtx::DAEMON,
            created + chrono::Duration::minutes(59),
            max_age,
        )
        .unwrap();
    assert!(kept.is_empty(), "59 minutes old is kept");
    let gone = ts
        .store
        .sweep_drafts(
            &AuditCtx::DAEMON,
            created + chrono::Duration::minutes(61),
            max_age,
        )
        .unwrap();
    assert_eq!(gone, vec![oid.to_string()]);
    assert!(!ts.home.artifact_dir(&oid).exists());
    assert_eq!(
        send(authed(&ts, Method::GET, &format!("/api/artifacts/{oid}")))
            .await
            .0,
        404
    );
}

/// Every route and stream that lists artifacts leaves drafts out; only a
/// subscriber naming the draft hears it.
#[tokio::test]
async fn hidden_artifacts_never_listed() {
    let ts = TestServer::spawn().await;
    let a = app(&ts, "Tracker", json!({"db": {}})).await;
    let mut all = ts.events_with("", |r| r.bearer_auth(&ts.token)).await;
    let mut gallery = ts.stream_as_owner(&["gallery"]).await;
    let did = id(&draft(&ts, Some(&a), json!({"db": {}})).await);
    let mut named = ts
        .events_with(&format!("?artifact={did}"), |r| r.bearer_auth(&ts.token))
        .await;
    let mut topic = stream_with_token(&ts, &[&format!("artifact:{did}")]).await;

    publish(&ts, &did, 1).await;
    seed(&ts, &did, "notes/n1", json!({})).await;
    publish(&ts, &a, 1).await;
    assert_eq!(next_about(&mut all).await, ("version".into(), a.clone()));
    assert_eq!(
        next_about(&mut gallery).await,
        ("version".into(), a.clone())
    );
    assert_eq!(
        next_about(&mut named).await,
        ("version".into(), did.clone())
    );
    assert_eq!(
        next_about(&mut topic).await,
        ("version".into(), did.clone())
    );

    assert_eq!(listed(&ts, "").await, vec![a.clone()]);
    assert!(listed(&ts, &format!("?artifact={did}")).await.is_empty());
    assert_eq!(
        listed(&ts, &format!("?artifact={a}")).await,
        vec![a.clone()]
    );
    let (s, v) = send(authed(&ts, Method::GET, "/api/threads")).await;
    assert_eq!(s, 200);
    assert!(!v.to_string().contains(&did), "{v}");
    let (s, v) = send(
        ts.client
            .get(format!("{}/api/viewers/me/attention", ts.base))
            .header("cookie", ts.owner_cookie()),
    )
    .await;
    assert_eq!(s, 200);
    assert!(!v.to_string().contains(&did), "{v}");
    let (s, v) = send(authed(&ts, Method::GET, "/api/inbox")).await;
    assert_eq!(s, 200);
    assert!(!v.to_string().contains(&did), "{v}");

    let mcp = |body: Value| {
        ts.client
            .post(format!("{}/mcp", ts.base))
            .header("accept", "application/json, text/event-stream")
            .bearer_auth(&ts.token)
            .json(&body)
    };
    let res = mcp(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "api-drafts-test", "version": "0"}
        }}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let session = res
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    res.text().await.unwrap();
    let with = |r: reqwest::RequestBuilder| match &session {
        Some(s) => r.header("mcp-session-id", s),
        None => r,
    };
    with(mcp(
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ))
    .send()
    .await
    .unwrap();
    let text = with(mcp(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "list", "arguments": {}}}),
    ))
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(text.contains(&a), "the list names the app: {text}");
    assert!(!text.contains(&did), "the list never names a draft: {text}");
}

/// A draft's documents are the owner's alone: without the token no read,
/// list, write or subscription reaches them, whatever the draft's rules
/// say, even after a publish loosens them; and no app reads a draft.
#[tokio::test]
async fn a_drafts_documents_are_the_owners_alone() {
    let ts = TestServer::spawn().await;
    let rules = json!({"db": {"rules": [{"path": "secrets", "read": "owner", "write": "owner"}]}});
    let a = app(&ts, "Vault", rules.clone()).await;
    seed(&ts, &a, "secrets/s1", json!({"k": "hush"})).await;
    seed(&ts, &a, "notes/n1", json!({"t": "open"})).await;
    let loose = id(&draft(&ts, Some(&a), json!({"db": {}})).await);
    let strict = id(&draft(&ts, Some(&a), rules).await);
    let v = publish(&ts, &strict, 1).await;
    assert_eq!(v["version"]["n"], 2);
    let (s, v) = send(
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{strict}/versions"),
        )
        .json(&json!({
            "if_version": 2, "capabilities": {"db": {}},
            "files": {"index.html": {"content": "<p>loosened</p>"}}
        })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let viewer = ts.viewer(Some("Named")).await;
    let cookie = format!("clax_viewer={}", viewer.cookie);
    for did in [&loose, &strict] {
        for path in [
            format!("/api/artifacts/{did}/docs/secrets/s1"),
            format!("/api/artifacts/{did}/docs/notes/n1"),
            format!("/api/artifacts/{did}/docs?collection=secrets"),
            format!("/api/artifacts/{did}/docs?collection=notes"),
        ] {
            let anon = send(ts.client.get(format!("{}{path}", ts.base))).await;
            assert_eq!(anon.0, 404, "no credentials: {path} {}", anon.1);
            let named = send(
                ts.client
                    .get(format!("{}{path}", ts.base))
                    .header("cookie", &cookie),
            )
            .await;
            assert_eq!(named.0, 404, "a viewer: {path} {}", named.1);
        }
        let (s, _) = send(
            ts.client
                .put(format!("{}/api/artifacts/{did}/docs/notes/n9", ts.base))
                .header("cookie", &cookie)
                .json(&json!({"data": {}, "lww": true})),
        )
        .await;
        assert_eq!(s, 404, "a viewer writes nothing");
        let (s, v) = send(authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{did}/docs/secrets/s1"),
        ))
        .await;
        assert_eq!(
            (s, v["doc"]["data"]["k"].as_str()),
            (200, Some("hush")),
            "the token reads"
        );
        for topic in [format!("docs:{did}"), format!("artifact:{did}")] {
            assert_eq!(viewer.subscribe_status(&[&topic]).await, 404, "{topic}");
        }
    }
    // An events stream naming the draft, without the token, hears none of it.
    let mut anon = ts.events(&format!("?artifact={loose},{a}")).await;
    let mut named = ts
        .events_as(&format!("?artifact={loose},{a}"), Some(&viewer.cookie))
        .await;
    seed(&ts, &loose, "notes/n1", json!({"t": "changed"})).await;
    publish(&ts, &loose, 1).await;
    publish(&ts, &a, 1).await;
    assert_eq!(next_about(&mut anon).await, ("version".into(), a.clone()));
    assert_eq!(next_about(&mut named).await, ("version".into(), a.clone()));

    // No app reads a draft, even one declaring and granted the read.
    let reader = app(&ts, "Reader", json!({"db": {"read": [loose.clone()]}})).await;
    let (s, _) = send(authed(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{reader}/db-grants/{loose}"),
    ))
    .await;
    assert!((200..300).contains(&s), "{s}");
    let (s, v) = send(
        authed(
            &ts,
            Method::GET,
            &format!("/api/artifacts/{loose}/docs/notes/n1"),
        )
        .header(READER, &reader),
    )
    .await;
    assert_eq!(s, 404, "{v}");
}
