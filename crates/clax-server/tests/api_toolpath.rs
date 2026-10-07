//! The audit context each request resolves (spec
//! 2026-10-06-toolpath-audit-design §5.2, §6, §6.9), read back through the
//! test-only route `GET /api/_test/audit/ctx`; the appender's nudge; and
//! the events each request that changes history records (§6.1, §6.3).
use crate::common;
use clax_core::audit::{CallHeader, encode_call_header};
use clax_core::gitctx::{GitContext, GitField, encode_header};
use common::TestServer;
use serde_json::{Value, json};

const CTX: &str = "/api/_test/audit/ctx";

fn git() -> GitContext {
    GitContext {
        repo_root: "/Users/alex/work/app".into(),
        remote: Some("origin".into()),
        remote_url: Some("https://github.com/empathic/app.git".into()),
        branch: Some("feat/settings".into()),
        head: Some("9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0d".into()),
        dirty: false,
        diff_sha256: None,
        diff_bytes: None,
        diff_truncated: false,
        untracked: 0,
        captured_at: "2026-10-06T14:03:11.512Z".into(),
    }
}

fn call() -> CallHeader {
    CallHeader {
        call_id: "01JBC0000000000000000000AA".into(),
        tool: "publish".into(),
        harness_tool: Some("mcp__clax__publish".into()),
        args_sha256: format!("sha256:{}", "c6".repeat(32)),
        started_at: "2026-10-06T14:03:11.402Z".into(),
        harness_call_id: None,
    }
}

async fn ctx(req: reqwest::RequestBuilder) -> Value {
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

#[tokio::test]
async fn ctx_owner_cookie_is_owner_shell() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_public_id().await;
    let url = format!("{}{CTX}", ts.base);
    let v = ctx(ts.client.get(&url).header("cookie", ts.owner_cookie())).await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "shell");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // The token alone is the owner through the CLI; it may name its channel.
    let v = ctx(ts.authed(ts.client.get(&url))).await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "cli");
    // A browser of the owner's (where a page's own requests carry the
    // owner cookie too) is the owner through the shell, whatever channel,
    // session, git state or tool call it claims.
    let session = ts.register_session("claude", "hs-owner").await;
    let v = ctx(ts
        .client
        .get(&url)
        .header("cookie", ts.owner_cookie())
        .header("x-clax-via", "mcp")
        .header("x-clax-session", session["id"].as_str().unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "shell");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // The page publishing through the shell names `page`, which is no channel.
    let v = ctx(ts.authed(ts.client.get(&url)).header("x-clax-via", "page")).await;
    assert_eq!(v["via"], "cli");
}

#[tokio::test]
async fn ctx_lan_viewer_has_public_id_and_name() {
    let ts = TestServer::spawn().await;
    let sam = ts.viewer(Some("Sam")).await;
    let (lan, base) = ts.lan();
    let url = format!("{base}{CTX}");
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={}", sam.cookie)))
    .await;
    assert_eq!(
        v["actor"],
        json!({"type": "viewer", "public_id": sam.public_id, "display_name": "Sam"})
    );
    assert_eq!(v["via"], "lan");
    // A viewer's claims of a channel, a session, git state or a tool call
    // are not believed.
    let session = ts.register_session("claude", "hs-1").await;
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={}", sam.cookie))
        .header("x-clax-via", "mcp")
        .header("x-clax-session", session["id"].as_str().unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(v["actor"]["type"], "viewer");
    assert_eq!(v["via"], "lan");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // A first-time viewer is recorded under the public ID they are given.
    let fresh = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={fresh}")))
    .await;
    assert_eq!(v["actor"]["type"], "viewer");
    let pid = v["actor"]["public_id"].as_str().unwrap().to_string();
    assert!(clax_core::is_public_id(&pid), "{v}");
    let again = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={fresh}")))
    .await;
    assert_eq!(again["actor"]["public_id"], pid.as_str());
    // No cookie: anonymous.
    let v = ctx(lan.get(&url)).await;
    assert_eq!(v["actor"], json!({"type": "anonymous"}));
    assert_eq!(v["via"], "lan");
}

#[tokio::test]
async fn ctx_session_header_is_agent_with_harness_ids() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "3f2c-hs").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let handle = session["agent_handle"].as_str().unwrap().to_string();
    let transcript = "/Users/alex/.claude/projects/app/3f2c-hs.jsonl";
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute(
            "UPDATE sessions SET transcript_path = ?1 WHERE id = ?2",
            rusqlite::params![transcript, sid],
        )
        .unwrap();
    let url = format!("{}{CTX}", ts.base);
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid)
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(
        v["actor"],
        json!({"type": "agent", "session_id": sid, "harness": "claude",
               "harness_session_id": "3f2c-hs", "agent_handle": handle,
               "transcript_path": transcript})
    );
    // A session without a channel header came through the MCP shim.
    assert_eq!(v["via"], "mcp");
    assert_eq!(v["git"], serde_json::to_value(git()).unwrap());
    assert_eq!(v["git_capture"], "ok");
    assert_eq!(v["call"], serde_json::to_value(call()).unwrap());
    // The agent side names its channel, and why it has no git context.
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid)
        .header("x-clax-via", "pi")
        .header(
            "x-clax-git",
            encode_header(&GitField::Capture("not-a-repo")).unwrap(),
        ))
    .await;
    assert_eq!(
        (v["actor"]["type"].as_str(), v["via"].as_str()),
        (Some("agent"), Some("pi"))
    );
    assert_eq!(
        (&v["git"], &v["git_capture"]),
        (&Value::Null, &json!("not-a-repo"))
    );
    // An ended session still names the agent that acted.
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid))
    .await;
    assert_eq!(v["actor"]["session_id"], sid.as_str());
    // `mcp` without a session is the sessionless `/mcp` route.
    let v = ctx(ts.authed(ts.client.get(&url)).header("x-clax-via", "mcp")).await;
    assert_eq!(v["actor"], json!({"type": "agent", "session_id": null}));
    assert_eq!(v["via"], "mcp");
    // `hook` or `pi` without a known session is the owner, on that channel.
    let owner = ts.owner_public_id().await;
    for via in ["hook", "pi"] {
        for session in [None, Some("01JB0000000000000000000000")] {
            let mut req = ts.authed(ts.client.get(&url)).header("x-clax-via", via);
            if let Some(s) = session {
                req = req.header("x-clax-session", s);
            }
            let v = ctx(req).await;
            assert_eq!(
                v["actor"],
                json!({"type": "owner", "public_id": owner}),
                "{via}"
            );
            assert_eq!(v["via"], via);
        }
    }
}

#[tokio::test]
async fn ctx_bad_headers_are_invalid_not_errors() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_public_id().await;
    let url = format!("{}{CTX}", ts.base);
    let bad = reqwest::header::HeaderValue::from_bytes(b"\xff\xfe").unwrap();
    let mut bad_call = call();
    bad_call.call_id = "nope".into();
    let bad_call = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&bad_call).unwrap())
    };
    for (git, call, via, session) in [
        (
            "!!not base64!!".parse().unwrap(),
            "!!".parse().unwrap(),
            "bogus".parse().unwrap(),
            "not-a-ulid".parse().unwrap(),
        ),
        (bad.clone(), bad.clone(), bad.clone(), bad.clone()),
        (
            "A".repeat(4096).parse().unwrap(),
            bad_call.parse().unwrap(),
            "daemon".parse().unwrap(),
            "01JB0000000000000000000000".parse().unwrap(),
        ),
    ] {
        let v = ctx(ts
            .authed(ts.client.get(&url))
            .header("x-clax-git", git)
            .header("x-clax-call", call)
            .header("x-clax-via", via)
            .header("x-clax-session", session))
        .await;
        assert_eq!(
            v["actor"],
            json!({"type": "owner", "public_id": owner}),
            "{v}"
        );
        assert_eq!(v["via"], "cli", "{v}");
        assert_eq!(
            (&v["git"], &v["git_capture"]),
            (&Value::Null, &json!("invalid")),
            "{v}"
        );
        assert_eq!(v["call"], Value::Null, "{v}");
    }
}

#[test]
fn nudge_never_blocks() {
    let wake = clax_server::audit::AuditWake::new();
    let rx = wake.take_receiver().expect("the appender's receiver");
    assert!(wake.take_receiver().is_none(), "one appender");
    // Nobody drains the channel: every nudge past the first finds it full.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let w = wake.clone();
    std::thread::spawn(move || {
        for _ in 0..100_000 {
            w.nudge();
        }
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("100 000 nudges return without a reader");
    // They coalesce into one wake-up.
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
    // With the appender gone, a nudge still returns.
    drop(rx);
    wake.nudge();
}

/// A recorded event, read from the database: its kind, actor, ID columns
/// and body.
#[derive(Debug)]
struct Rec {
    kind: String,
    actor: Value,
    artifact_id: Option<String>,
    artifact2_id: Option<String>,
    session_id: Option<String>,
    call_id: Option<String>,
    origin: Option<String>,
    body: Value,
}

fn db(ts: &TestServer) -> rusqlite::Connection {
    rusqlite::Connection::open(ts.home.db_path()).unwrap()
}

/// The newest event's `seq` (0 when there is none).
fn last_seq(ts: &TestServer) -> i64 {
    db(ts)
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM audit_events", [], |r| {
            r.get(0)
        })
        .unwrap()
}

/// The events recorded after `seq`, oldest first.
fn events_since(ts: &TestServer, seq: i64) -> Vec<Rec> {
    let c = db(ts);
    let mut q = c
        .prepare(
            "SELECT kind, actor, artifact_id, artifact2_id, session_id, call_id, origin, body
             FROM audit_events WHERE seq > ?1 ORDER BY seq",
        )
        .unwrap();
    q.query_map([seq], |r| {
        Ok(Rec {
            kind: r.get(0)?,
            actor: serde_json::from_str(&r.get::<_, String>(1)?).unwrap(),
            artifact_id: r.get(2)?,
            artifact2_id: r.get(3)?,
            session_id: r.get(4)?,
            call_id: r.get(5)?,
            origin: r.get(6)?,
            body: serde_json::from_str(&r.get::<_, String>(7)?).unwrap(),
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// One request of [`every_mutation_records_one_event`]: its name, the
/// events it recorded, the kinds wanted, the actor type, and whether a tool
/// call is named.
type Row<'a> = (&'a str, Vec<Rec>, Vec<&'a str>, &'a str, bool);

fn kinds(rs: &[Rec]) -> Vec<&str> {
    rs.iter().map(|r| r.kind.as_str()).collect()
}

fn utf8(content: &str) -> Value {
    json!({"content": content, "encoding": "utf8"})
}

async fn status(req: reqwest::RequestBuilder) -> (u16, Value) {
    let res = req.send().await.unwrap();
    let code = res.status().as_u16();
    (code, res.json().await.unwrap_or(Value::Null))
}

fn png_form() -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(clax_server::testing::FAKE_PNG.to_vec())
        .mime_str("image/png")
        .unwrap();
    reqwest::multipart::Form::new().part("file", part)
}

/// The fields of a first comment on the live page `url`.
fn live_form(url: &str, snapshot: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "Settings")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "main", "quote": "Save", "file": "index.html"})
                .to_string(),
        )
        .text("body", "The button overflows")
        .text("pending", "[]")
        .text("snapshot", snapshot.to_string())
}

#[tokio::test]
async fn every_mutation_records_one_event() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-every").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let sam = ts.viewer(Some("Sam")).await;
    let call_header = encode_call_header(&call()).unwrap();
    let b = ts.base.clone();
    let agent = |r: reqwest::RequestBuilder| {
        ts.authed(r)
            .header("x-clax-session", &sid)
            .header("x-clax-call", &call_header)
    };
    let owner = |r: reqwest::RequestBuilder| ts.authed(r);
    let viewer =
        |r: reqwest::RequestBuilder| r.header("cookie", format!("clax_viewer={}", sam.cookie));

    // An artifact with a db, made by the agent under a tool call.
    let seq = last_seq(&ts);
    let (code, v) = status(
        agent(ts.client.post(format!("{b}/api/artifacts"))).json(&json!({
            "title": "Tracker",
            "capabilities": {"db": {}},
            "files": {"index.html": utf8("<main></main>")}
        })),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let mut table: Vec<Row> = vec![(
        "create",
        events_since(&ts, seq),
        vec!["artifact.create", "version.publish"],
        "agent",
        true,
    )];
    let a = format!("{b}/api/artifacts/{aid}");
    let doc = format!("{a}/docs/tasks/t1");
    let mine = format!("{a}/docs/data/users/{}/pick", sam.public_id);
    let steps: Vec<(&str, reqwest::RequestBuilder, Vec<&str>, &str, bool)> = vec![
        (
            "publish",
            owner(ts.client.post(format!("{a}/versions")))
                .json(&json!({"if_version": 1, "files": {"index.html": utf8("<main>2</main>")}})),
            vec!["version.publish"],
            "owner",
            false,
        ),
        (
            "pin",
            agent(ts.client.patch(&a)).json(&json!({"pinned": true})),
            vec!["artifact.update"],
            "agent",
            true,
        ),
        (
            "asset upload",
            owner(ts.client.post(format!("{a}/assets"))).multipart(png_form()),
            vec!["asset.upload"],
            "owner",
            false,
        ),
        (
            "doc set",
            owner(ts.client.put(&doc)).json(&json!({"data": {"title": "Ship"}})),
            vec!["doc.write"],
            "owner",
            false,
        ),
        (
            "doc update",
            owner(ts.client.patch(&doc)).json(&json!({"data": {"done": true}, "lww": true})),
            vec!["doc.write"],
            "owner",
            false,
        ),
        (
            "doc str_replace",
            owner(ts.client.post(format!("{a}/docs:str_replace"))).json(&json!({
                "path": "tasks/t1", "field": "title", "old_str": "Ship", "new_str": "Shipped", "lww": true
            })),
            vec!["doc.write"],
            "owner",
            false,
        ),
        (
            "doc batch",
            agent(ts.client.post(format!("{a}/docs:batch"))).json(&json!({
                "writes": [
                    {"path": "tasks/t2", "op": "set", "data": {"n": 2}},
                    {"path": "tasks/t3", "op": "set", "data": {"n": 3}}
                ]
            })),
            vec!["doc.write", "doc.write"],
            "agent",
            true,
        ),
        (
            "doc acquire with data",
            owner(ts.client.post(format!("{a}/docs:acquire")))
                .json(&json!({"path": "tasks/t4", "holder": "h1", "data": {"n": 4}})),
            vec!["doc.write"],
            "owner",
            false,
        ),
        (
            "doc acquire without data",
            owner(ts.client.post(format!("{a}/docs:acquire")))
                .json(&json!({"path": "tasks/t5", "holder": "h1"})),
            vec![],
            "owner",
            false,
        ),
        (
            "viewer's own doc",
            viewer(ts.client.put(&mine)).json(&json!({"data": {"pick": 1}})),
            vec!["doc.write"],
            "viewer",
            false,
        ),
        (
            "doc delete",
            owner(ts.client.delete(format!("{doc}?lww=true"))),
            vec!["doc.write"],
            "owner",
            false,
        ),
        (
            "delete of a missing doc",
            owner(ts.client.delete(format!("{doc}?lww=true"))),
            vec![],
            "owner",
            false,
        ),
        (
            "metadata edit",
            owner(ts.client.patch(&a)).json(&json!({
                "title": "Tracker", "description": "Tasks", "capabilities": {"db": {}}
            })),
            vec!["artifact.update"],
            "owner",
            false,
        ),
        (
            "empty metadata edit",
            owner(ts.client.patch(&a)).json(&json!({})),
            vec![],
            "owner",
            false,
        ),
        (
            "live page",
            viewer(ts.client.post(format!("{b}/api/live/threads")))
                .multipart(live_form("http://localhost:5173/settings", "<main>Save</main>")),
            vec!["artifact.create", "live.page", "live.snapshot"],
            "viewer",
            false,
        ),
    ];
    let mut live = None;
    for (name, req, want, actor, with_call) in steps {
        let seq = last_seq(&ts);
        let (code, v) = status(req).await;
        assert!((200..300).contains(&code), "{name}: {code} {v}");
        table.push((name, events_since(&ts, seq), want, actor, with_call));
        if name == "live page" {
            live = Some((
                v["page"]["artifact_id"].as_str().unwrap().to_string(),
                v["thread"]["id"].as_str().unwrap().to_string(),
            ));
        }
        if name == "asset upload" {
            let asset = v["asset"]["id"].as_str().unwrap().to_string();
            let seq = last_seq(&ts);
            let (code, v) = status(owner(ts.client.delete(format!("{a}/assets/{asset}")))).await;
            assert_eq!(code, 204, "{v}");
            table.push((
                "asset delete",
                events_since(&ts, seq),
                vec!["asset.delete"],
                "owner",
                false,
            ));
        }
    }
    // A snapshot the extension takes for an address waiting on the page.
    let (page, tid) = live.unwrap();
    let watch = format!("{b}/api/sessions/{sid}/watches/{page}");
    assert_eq!(status(owner(ts.client.put(&watch))).await.0, 200);
    ts.send_thread(&page, &tid).await;
    let reply = format!("{b}/api/artifacts/{page}/threads/{tid}/comments");
    let (code, v) = status(
        owner(ts.client.post(&reply))
            .header("x-clax-session", &sid)
            .json(&json!({"body": "Fixed", "author_kind": "agent", "addressed": true})),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let seq = last_seq(&ts);
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/settings")
        .text("title", "Settings")
        .text("pending", json!([tid]).to_string())
        .text("snapshot", "<main>Saved</main>");
    let (code, v) =
        status(viewer(ts.client.post(format!("{b}/api/live/snapshots"))).multipart(form)).await;
    assert_eq!(code, 200, "{v}");
    table.push((
        "live snapshot",
        events_since(&ts, seq),
        vec!["live.snapshot"],
        "viewer",
        false,
    ));
    // A scope watch on a page not yet seen makes it.
    let seq = last_seq(&ts);
    let (code, v) = status(
        agent(
            ts.client
                .put(format!("{b}/api/sessions/{sid}/live-watches")),
        )
        .json(&json!({"url": "http://localhost:5173/billing"})),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    table.push((
        "live watch",
        events_since(&ts, seq),
        vec!["artifact.create", "live.page", "live.snapshot"],
        "agent",
        true,
    ));

    let seq = last_seq(&ts);
    let (code, v) = status(owner(ts.client.delete(&a))).await;
    assert_eq!(code, 204, "{v}");
    table.push((
        "delete",
        events_since(&ts, seq),
        vec!["artifact.delete"],
        "owner",
        false,
    ));

    for (name, recs, want, actor, with_call) in &table {
        assert_eq!(kinds(recs), *want, "{name}");
        for r in recs {
            assert_eq!(r.actor["type"], *actor, "{name}: {r:?}");
            assert_eq!(r.call_id.is_some(), *with_call, "{name}: {r:?}");
            assert_eq!(r.body["call"].is_object(), *with_call, "{name}: {r:?}");
            assert!(r.artifact_id.is_some(), "{name}: {r:?}");
        }
    }
    let ops: Vec<&str> = table
        .iter()
        .flat_map(|(_, recs, ..)| recs)
        .filter(|r| r.kind == "doc.write")
        .map(|r| r.body["op"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops,
        [
            "set",
            "update",
            "str_replace",
            "set",
            "set",
            "acquire",
            "set",
            "delete"
        ]
    );
    let pin = &table.iter().find(|t| t.0 == "pin").unwrap().1[0];
    assert_eq!(pin.body["fields"], json!({"pinned": true}));
    let edit = &table.iter().find(|t| t.0 == "metadata edit").unwrap().1[0];
    assert_eq!(
        edit.body["fields"],
        json!({"title": "Tracker", "description": "Tasks", "capabilities": {"db": {}}})
    );
    let snap = &table.iter().find(|t| t.0 == "live snapshot").unwrap().1[0];
    assert_eq!(snap.body["addresses"], json!([tid]));
    assert_eq!(snap.body["path"], "/settings");
    let deleted = &table.last().unwrap().1[0];
    assert_eq!(
        (&deleted.body["title"], &deleted.body["current_version"]),
        (&json!("Tracker"), &json!(2))
    );
}

#[tokio::test]
async fn publish_records_file_hashes_and_addresses() {
    let ts = TestServer::spawn().await;
    let seq = last_seq(&ts);
    let v = ts
        .publish(
            "Report",
            &[("index.html", "<p>v1"), ("app.js", "let a = 1;")],
        )
        .await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let created = events_since(&ts, seq);
    assert_eq!(kinds(&created), ["artifact.create", "version.publish"]);
    assert_eq!(created[0].body["title"], "Report");
    assert_eq!(created[0].body["kind"], "html");
    let v1 = &created[1].body;
    let sha = |s: &str| format!("sha256:{}", clax_core::audit::sha256_hex(s.as_bytes()));
    assert_eq!(v1["n"], 1);
    assert_eq!(v1["files"]["app.js"]["sha256"], sha("let a = 1;"));
    assert_eq!(v1["files"]["app.js"]["size"], 10);
    assert_eq!(v1["files"]["index.html"]["sha256"], sha("<p>v1"));
    assert_eq!(v1["carried"], json!([]));
    assert_eq!(v1["addresses"], json!([]));
    assert_eq!(v1["by_page"], false);
    assert_eq!(v1["content_sha256"], v["version"]["content_sha256"]);

    let t = ts.thread(&aid, 1, "Make it bigger").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let seq = last_seq(&ts);
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "note": "bigger", "addresses": [tid],
                   "files": {"index.html": utf8("<p>v2")}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let pubd = events_since(&ts, seq);
    assert_eq!(kinds(&pubd), ["version.publish"]);
    let v2 = &pubd[0].body;
    assert_eq!(v2["n"], 2);
    assert_eq!(v2["note"], "bigger");
    assert_eq!(v2["carried"], json!(["app.js"]));
    assert_eq!(v2["files"]["app.js"]["sha256"], sha("let a = 1;"));
    assert_eq!(v2["files"]["index.html"]["sha256"], sha("<p>v2"));
    assert_eq!(v2["addresses"], json!([tid]));
    assert_eq!(pubd[0].artifact_id.as_deref(), Some(aid.as_str()));
    // The page publishing itself through the shell says so.
    let seq = last_seq(&ts);
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-via", "page")
        .json(&json!({"if_version": 2, "files": {"index.html": utf8("<p>v3")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(events_since(&ts, seq)[0].body["by_page"], true);
}

#[tokio::test]
async fn doc_write_records_hash_not_content() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Notes", "capabilities": {"db": {}},
                   "files": {"index.html": utf8("<main></main>")}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let secret = "the launch code is 0000";
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/docs/notes/n1", ts.base)),
        )
        .json(&json!({"data": {"text": secret}})),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["doc.write"]);
    let stored: String = db(&ts)
        .query_row(
            "SELECT json FROM docs WHERE artifact_id = ?1 AND path = 'notes/n1'",
            [&aid],
            |r| r.get(0),
        )
        .unwrap();
    let body = &recs[0].body;
    assert_eq!(
        body["sha256"],
        format!("sha256:{}", clax_core::audit::sha256_hex(stored.as_bytes()))
    );
    assert_eq!(
        (&body["collection"], &body["doc_id"], &body["op"]),
        (&json!("notes"), &json!("n1"), &json!("set"))
    );
    assert_eq!(body["version"], v["doc"]["version"]);
    let raw: String = db(&ts)
        .query_row(
            "SELECT body FROM audit_events WHERE kind = 'doc.write'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!raw.contains("launch code"), "{raw}");
    // A delete names no hash.
    let seq = last_seq(&ts);
    let (code, _) = status(ts.authed(ts.client.delete(format!(
        "{}/api/artifacts/{aid}/docs/notes/n1?lww=true",
        ts.base
    ))))
    .await;
    assert_eq!(code, 200);
    let del = events_since(&ts, seq);
    assert_eq!(
        (&del[0].body["op"], &del[0].body["sha256"]),
        (&json!("delete"), &Value::Null)
    );
}

#[tokio::test]
async fn live_snapshot_records_origin_and_path() {
    let ts = TestServer::spawn().await;
    let sam = ts.viewer(Some("Sam")).await;
    let seq = last_seq(&ts);
    let html = "<!doctype html><main><button>Save</button></main>";
    let (code, v) = status(
        ts.client
            .post(format!("{}/api/live/threads", ts.base))
            .header("cookie", format!("clax_viewer={}", sam.cookie))
            .multipart(live_form("http://localhost:5173/settings?tab=x", html)),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let aid = v["page"]["artifact_id"].as_str().unwrap();
    let recs = events_since(&ts, seq);
    assert_eq!(
        kinds(&recs),
        ["artifact.create", "live.page", "live.snapshot"]
    );
    let origin = "http://localhost:5173";
    assert_eq!(recs[0].body["kind"], "live");
    for r in &recs {
        assert_eq!(r.artifact_id.as_deref(), Some(aid));
        assert_eq!(r.actor["public_id"], sam.public_id.as_str());
    }
    for r in &recs[1..] {
        assert_eq!(r.origin.as_deref(), Some(origin), "{r:?}");
        assert_eq!(
            (&r.body["origin"], &r.body["path"]),
            (&json!(origin), &json!("/settings"))
        );
    }
    let snap = &recs[2].body;
    assert_eq!(snap["n"], 1);
    assert_eq!(
        snap["files"]["index.html"]["sha256"],
        format!("sha256:{}", clax_core::audit::sha256_hex(html.as_bytes()))
    );
    assert_eq!(
        snap["content_sha256"]
            .as_str()
            .map(|s| s.starts_with("sha256:")),
        Some(true)
    );
}

#[tokio::test]
async fn failed_publish_records_nothing() {
    let ts = TestServer::spawn().await;
    let v = ts.publish("Report", &[("index.html", "<p>v1")]).await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let seq = last_seq(&ts);
    // A stale pin, a missing artifact, and a body that does not validate.
    for (path, body, want) in [
        (
            format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 7, "files": {"index.html": utf8("<p>v2")}}),
            409,
        ),
        (
            "/api/artifacts/7q3k9mzx2b4t/versions".to_string(),
            json!({"if_version": 1, "files": {"index.html": utf8("<p>v2")}}),
            404,
        ),
        (
            "/api/artifacts".to_string(),
            json!({"title": "", "files": {"index.html": utf8("<p>")}}),
            400,
        ),
        (
            format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"../x": utf8("<p>")}}),
            400,
        ),
    ] {
        let res = ts.post_json(&path, body).await;
        assert_eq!(res.status().as_u16(), want, "{path}");
    }
    assert_eq!(last_seq(&ts), seq, "{:?}", events_since(&ts, seq));
}

#[tokio::test]
async fn event_under_call_carries_call_id() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-call").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let seq = last_seq(&ts);
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts", ts.base)))
        .header("x-clax-session", &sid)
        .header("x-clax-call", encode_call_header(&call()).unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .json(&json!({"title": "Report", "files": {"index.html": utf8("<p>")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["artifact.create", "version.publish"]);
    for r in &recs {
        assert_eq!(r.call_id.as_deref(), Some(call().call_id.as_str()));
        assert_eq!(r.body["call"]["tool"], "publish");
        assert_eq!(r.body["call"]["args_sha256"], call().args_sha256.as_str());
        assert_eq!(r.body["via"], "mcp");
        assert_eq!(r.body["git"]["head"], git().head.unwrap().as_str());
        assert_eq!(r.session_id.as_deref(), Some(sid.as_str()));
        assert_eq!(r.actor["session_id"], sid.as_str());
        assert!(r.artifact2_id.is_none());
    }
    // Without the header, no call.
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let seq = last_seq(&ts);
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": utf8("<p>2")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let recs = events_since(&ts, seq);
    assert_eq!(recs[0].call_id, None);
    assert!(recs[0].body.get("call").is_none(), "{:?}", recs[0].body);
}

#[tokio::test]
async fn a_claim_that_retires_the_cli_owner_resolves_its_records() {
    let ts = TestServer::spawn().await;
    // The CLI publishes before any browser: its owner row is made.
    let seq = last_seq(&ts);
    ts.publish("Report", &[("index.html", "<p>")]).await;
    let cli = events_since(&ts, seq)[0].actor["public_id"]
        .as_str()
        .unwrap()
        .to_string();
    // The owner's browser loads the shell: its viewer is claimed.
    let chrome = ts.viewer(None).await;
    let seq = last_seq(&ts);
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("sec-fetch-site", "same-origin")
        .header("cookie", format!("clax_viewer={}", chrome.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let claim = events_since(&ts, seq);
    assert_eq!(kinds(&claim), ["viewer.claim"]);
    assert_eq!(
        claim[0].body["from_public_id"],
        cli.as_str(),
        "the publish's actor resolves through the claim"
    );
    assert_eq!(claim[0].body["to_public_id"], chrome.public_id.as_str());
    assert_eq!(
        claim[0].actor,
        json!({"type": "owner", "public_id": chrome.public_id})
    );
    assert_eq!(claim[0].body["via"], "shell");
    assert_eq!(ts.owner_public_id().await, chrome.public_id);
}

#[tokio::test]
async fn a_refused_owner_write_writes_nothing() {
    let ts = TestServer::spawn().await;
    let viewers = || -> i64 {
        db(&ts)
            .query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))
            .unwrap()
    };
    let before = viewers();
    let seq = last_seq(&ts);
    let (lan, base) = ts.lan();
    let fresh = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    let body = json!({"origin": "http://localhost:5173", "pattern": "/a/*"});
    let res = lan
        .post(format!("{base}/api/live/rules"))
        .header("cookie", format!("clax_viewer={fresh}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "the LAN sees no live routes");
    // A first-time viewer on this machine is refused before its row is
    // made, and so is the owner's browser without the token, before the
    // owner row would be made.
    let res = ts
        .client
        .post(format!("{}/api/live/rules", ts.base))
        .header("cookie", format!("clax_viewer={fresh}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    for (method, path) in [
        (reqwest::Method::POST, "/api/live/rules".to_string()),
        (reqwest::Method::DELETE, "/api/live/rules/r1".to_string()),
        (
            reqwest::Method::POST,
            "/api/live/threads/01J9Z3K4M5N6P7Q8R9S0T1V2W3/move".to_string(),
        ),
    ] {
        let res = ts
            .client
            .request(method, format!("{}{path}", ts.base))
            .header("cookie", ts.owner_cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{path}");
    }
    assert_eq!(viewers(), before, "no viewer row was made");
    assert_eq!(last_seq(&ts), seq);
}
