//! The audit context each request resolves (spec
//! 2026-10-06-toolpath-audit-design §5.2, §6, §6.9), read back through the
//! test-only route `GET /api/_test/audit/ctx`; the appender's nudge; and
//! the events each request that changes history records (§6.1, §6.2, §6.3).
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
        diff_unavailable: false,
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
        vec!["artifact.create", "version.publish", "watch.start"],
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
            vec![
                "artifact.create",
                "live.page",
                "live.snapshot",
                "thread.open",
                "comment.add",
            ],
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
        vec![
            "artifact.create",
            "live.page",
            "live.snapshot",
            "watch.start",
            "watch.start",
        ],
        "agent",
        true,
    ));

    // Threads, comments, sends and deliveries on the artifact, whose owner
    // is the agent's session: each request in turn, since later ones name
    // the threads earlier ones make.
    let threads = format!("{a}/threads");
    let thread_form = |body: &str| {
        reqwest::multipart::Form::new()
            .text("anchor", clax_server::testing::element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "2")
    };
    let (t1, t1_recs) = made(
        viewer(ts.client.post(&threads)).multipart(thread_form("Tighten")),
        &ts,
    )
    .await;
    table.push((
        "thread",
        t1_recs,
        vec!["thread.open", "comment.add"],
        "viewer",
        false,
    ));
    let (t2, t2_recs) = made(
        viewer(ts.client.post(&threads)).multipart(thread_form("@agent fix the header")),
        &ts,
    )
    .await;
    table.push((
        "thread mentioning @agent",
        t2_recs,
        vec!["thread.open", "comment.add", "thread.send"],
        "viewer",
        false,
    ));
    let t = |tid: &str, rest: &str| format!("{threads}/{tid}{rest}");
    let handle = session["agent_handle"].as_str().unwrap().to_string();
    // Armed, so the Pi injection tier hands rows over too.
    db(&ts)
        .execute(
            "UPDATE watches SET replies_armed = 1 WHERE session_id = ?1",
            [&sid],
        )
        .unwrap();
    let feedback = format!("{b}/api/sessions/{sid}/feedback");
    let thread_steps: Vec<(&str, reqwest::RequestBuilder, Vec<&str>, &str, bool)> = vec![
        (
            "viewer comment on a sent thread",
            viewer(ts.client.post(t(&t2, "/comments"))).json(&json!({"body": "and the footer"})),
            vec!["comment.add", "thread.send"],
            "viewer",
            false,
        ),
        (
            "send to one agent",
            owner(ts.client.post(t(&t2, "/send"))).json(&json!({"to": handle})),
            vec!["thread.send"],
            "owner",
            false,
        ),
        (
            "send that changes nothing",
            owner(ts.client.post(t(&t2, "/send"))).json(&json!({"to": handle})),
            vec![],
            "owner",
            false,
        ),
        (
            "batch send",
            owner(ts.client.post(format!("{a}/threads:send")))
                .json(&json!({"thread_ids": [t1], "note": "both"})),
            vec!["thread.send"],
            "owner",
            false,
        ),
        (
            "acknowledge",
            agent(ts.client.post(format!("{feedback}/ack"))).json(&json!({"thread_ids": [t1]})),
            vec!["feedback.delivered"],
            "agent",
            true,
        ),
        (
            "agent reply",
            agent(ts.client.post(t(&t2, "/comments")))
                .json(&json!({"body": "Fixed both", "author_kind": "agent"})),
            vec!["comment.add", "feedback.delivered", "feedback.delivered"],
            "agent",
            true,
        ),
        (
            "agent resolve",
            agent(ts.client.post(t(&t2, "/resolve"))).json(&json!({"as": "agent"})),
            vec!["thread.resolve"],
            "agent",
            true,
        ),
        (
            "resolve of a resolved thread",
            viewer(ts.client.post(t(&t2, "/resolve"))),
            vec![],
            "viewer",
            false,
        ),
        (
            "reopen",
            owner(ts.client.post(t(&t2, "/reopen"))),
            vec!["thread.reopen"],
            "owner",
            false,
        ),
        (
            "reopen of an open thread",
            owner(ts.client.post(t(&t2, "/reopen"))),
            vec![],
            "owner",
            false,
        ),
        (
            "viewer resolve",
            viewer(ts.client.post(t(&t1, "/resolve"))),
            vec!["thread.resolve"],
            "viewer",
            false,
        ),
        (
            "viewer comment reopening a sent thread",
            viewer(ts.client.post(t(&t1, "/comments"))).json(&json!({"body": "not yet"})),
            vec!["comment.add", "thread.reopen", "thread.send"],
            "viewer",
            false,
        ),
        (
            "feedback poll",
            agent(ts.client.get(format!("{feedback}?tier=wait&wait=0"))),
            vec!["feedback.delivered", "working.start"],
            "agent",
            true,
        ),
        (
            "viewer comment for the prompt hook",
            viewer(ts.client.post(t(&t1, "/comments"))).json(&json!({"body": "one more"})),
            vec!["comment.add", "thread.send"],
            "viewer",
            false,
        ),
        (
            "prompt hook poll",
            agent(ts.client.get(format!("{feedback}?tier=prompt_hook&wait=0")))
                .header("x-clax-via", "hook"),
            vec!["feedback.delivered"],
            "agent",
            true,
        ),
        (
            "viewer comment for Pi",
            viewer(ts.client.post(t(&t1, "/comments"))).json(&json!({"body": "last one"})),
            vec!["comment.add", "thread.send"],
            "viewer",
            false,
        ),
        (
            "inject poll",
            agent(ts.client.get(format!("{feedback}?tier=inject&wait=0")))
                .header("x-clax-via", "pi"),
            vec!["feedback.delivered"],
            "agent",
            true,
        ),
        (
            "thread delete",
            viewer(ts.client.delete(t(&t2, ""))),
            vec!["thread.delete"],
            "viewer",
            false,
        ),
    ];
    for (name, req, want, actor, with_call) in thread_steps {
        let seq = last_seq(&ts);
        let (code, v) = status(req).await;
        assert!((200..300).contains(&code), "{name}: {code} {v}");
        table.push((name, events_since(&ts, seq), want, actor, with_call));
    }

    // Watches, working records, rules, moves and sessions (§6.3–§6.5,
    // §6.8), in turn.
    let watch_a = format!("{b}/api/sessions/{sid}/watches/{aid}");
    let working_a = format!("{b}/api/sessions/{sid}/working/{aid}");
    let scope = format!("{b}/api/sessions/{sid}/live-watches");
    let rules = format!("{b}/api/live/rules");
    let join = json!({
        "harness": "claude", "parent_pid": 4242, "harness_session_id": "hs-every",
        "transcript_path": "/t/hs-every.jsonl"
    });
    // A second agent watching only the page the move takes a thread from.
    let watcher = ts.register_session("claude", "hs-watcher").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let later_steps: Vec<(&str, reqwest::RequestBuilder, Vec<&str>, &str, bool)> = vec![
        (
            "turn end",
            agent(
                ts.client
                    .post(format!("{b}/api/sessions/{sid}/working/end")),
            )
            .json(&json!({})),
            vec!["working.stop"],
            "agent",
            true,
        ),
        (
            "working",
            agent(ts.client.put(&working_a)).json(&json!({"message": "Tightening"})),
            vec!["working.start"],
            "agent",
            true,
        ),
        (
            "working update",
            agent(ts.client.put(&working_a)).json(&json!({"message": "Still tightening"})),
            vec![],
            "agent",
            true,
        ),
        (
            "working renew",
            agent(
                ts.client
                    .post(format!("{b}/api/sessions/{sid}/working/renew")),
            ),
            vec![],
            "agent",
            true,
        ),
        (
            "working clear",
            agent(ts.client.delete(&working_a)),
            vec!["working.stop"],
            "agent",
            true,
        ),
        (
            "working again",
            agent(ts.client.put(&working_a)).json(&json!({})),
            vec!["working.start"],
            "agent",
            true,
        ),
        (
            "turn end with a record",
            agent(
                ts.client
                    .post(format!("{b}/api/sessions/{sid}/working/end")),
            )
            .json(&json!({})),
            vec!["working.stop"],
            "agent",
            true,
        ),
        (
            "watch disarm",
            agent(ts.client.put(&watch_a)).json(&json!({"replies_armed": false})),
            vec!["watch.update"],
            "agent",
            true,
        ),
        (
            "watch unchanged",
            agent(ts.client.put(&watch_a)).json(&json!({"replies_armed": false})),
            vec![],
            "agent",
            true,
        ),
        (
            "unwatch",
            agent(ts.client.delete(&watch_a)),
            vec!["watch.stop"],
            "agent",
            true,
        ),
        (
            "unwatch of no watch",
            agent(ts.client.delete(&watch_a)),
            vec![],
            "agent",
            true,
        ),
        (
            "watch",
            agent(ts.client.put(&watch_a)),
            vec!["watch.start"],
            "agent",
            true,
        ),
        (
            "scope disarm",
            agent(ts.client.put(&scope))
                .json(&json!({"url": "http://localhost:5173/billing", "replies_armed": false})),
            vec!["watch.update", "watch.update"],
            "agent",
            true,
        ),
        (
            "watcher",
            owner(
                ts.client
                    .put(format!("{b}/api/sessions/{watcher}/watches/{page}")),
            ),
            vec!["watch.start"],
            "agent",
            false,
        ),
        (
            "move",
            owner(ts.client.post(format!("{b}/api/live/threads/{tid}/move")))
                .json(&json!({"page_url": "http://localhost:5173/billing"})),
            // Each version the thread names is copied to its new page.
            // The watcher of the source page follows the thread.
            vec![
                "live.snapshot",
                "live.snapshot",
                "live.snapshot",
                "thread.move",
                "watch.start",
            ],
            "owner",
            false,
        ),
        (
            "scope unwatch",
            agent(
                ts.client
                    .delete(format!("{scope}?url=http://localhost:5173/billing")),
            ),
            vec!["watch.stop", "watch.stop"],
            "agent",
            true,
        ),
        (
            "scope unwatch of no scope",
            agent(
                ts.client
                    .delete(format!("{scope}?url=http://localhost:5173/billing")),
            ),
            vec![],
            "agent",
            true,
        ),
        (
            "rule",
            owner(ts.client.post(&rules))
                .json(&json!({"origin": "http://localhost:5173", "pattern": "/none/:id"})),
            vec!["live.rule"],
            "owner",
            false,
        ),
        (
            "rule again",
            owner(ts.client.post(&rules))
                .json(&json!({"origin": "http://localhost:5173", "pattern": "/none/:id"})),
            vec![],
            "owner",
            false,
        ),
        (
            "join",
            agent(ts.client.post(format!("{b}/api/sessions/join"))).json(&join),
            vec!["session.join"],
            "agent",
            true,
        ),
        (
            "join again",
            agent(ts.client.post(format!("{b}/api/sessions/join"))).json(&join),
            vec![],
            "agent",
            true,
        ),
        (
            "heartbeat",
            agent(ts.client.patch(format!("{b}/api/sessions/{sid}")))
                .json(&json!({"heartbeat": true})),
            vec![],
            "agent",
            true,
        ),
        (
            "register",
            owner(ts.client.post(format!("{b}/api/sessions"))).json(&json!({
                "harness": "codex", "harness_session_id": "cx-every", "cwd": "/w", "pid": 77
            })),
            vec!["session.start"],
            "agent",
            false,
        ),
        (
            "register again",
            owner(ts.client.post(format!("{b}/api/sessions"))).json(&json!({
                "harness": "codex", "harness_session_id": "cx-every", "cwd": "/w", "pid": 77
            })),
            vec![],
            "agent",
            false,
        ),
    ];
    let mut rule_id = None;
    let mut codex = None;
    for (name, req, want, actor, with_call) in later_steps {
        let seq = last_seq(&ts);
        let (code, v) = status(req).await;
        assert!((200..300).contains(&code), "{name}: {code} {v}");
        table.push((name, events_since(&ts, seq), want, actor, with_call));
        if name == "rule" {
            rule_id = Some(v["rule"]["id"].as_str().unwrap().to_string());
        }
        if name == "register" {
            codex = Some(v["session"]["id"].as_str().unwrap().to_string());
        }
    }
    let seq = last_seq(&ts);
    let rule_id = rule_id.unwrap();
    let (code, v) = status(owner(ts.client.delete(format!("{rules}/{rule_id}")))).await;
    assert_eq!(code, 200, "{v}");
    table.push((
        "rule delete",
        events_since(&ts, seq),
        vec!["live.rule", "live.rule"],
        "owner",
        false,
    ));
    let codex = codex.unwrap();
    let seq = last_seq(&ts);
    let (code, v) = status(
        owner(ts.client.patch(format!("{b}/api/sessions/{codex}"))).json(&json!({"ended": true})),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    table.push((
        "session end",
        events_since(&ts, seq),
        vec!["session.end"],
        "agent",
        false,
    ));

    // A record ends with its artifact.
    assert_eq!(
        status(agent(ts.client.put(&working_a)).json(&json!({})))
            .await
            .0,
        200
    );
    let seq = last_seq(&ts);
    let (code, v) = status(owner(ts.client.delete(&a))).await;
    assert_eq!(code, 204, "{v}");
    table.push((
        "delete",
        events_since(&ts, seq),
        vec!["artifact.delete", "working.stop"],
        "owner",
        false,
    ));

    let wrong: Vec<String> = table
        .iter()
        .filter(|(_, recs, want, ..)| kinds(recs) != *want)
        .map(|(name, recs, want, ..)| format!("{name}: {:?}, want {want:?}", kinds(recs)))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    for (name, recs, _, actor, with_call) in &table {
        for r in recs {
            assert_eq!(r.actor["type"], *actor, "{name}: {r:?}");
            assert_eq!(r.call_id.is_some(), *with_call, "{name}: {r:?}");
            assert_eq!(r.body["call"].is_object(), *with_call, "{name}: {r:?}");
            // Sessions, rules and scope watches belong to no artifact.
            let install = r.kind.starts_with("session.")
                || r.kind == "live.rule"
                || r.body["target"] == "scope";
            assert_eq!(r.artifact_id.is_none(), install, "{name}: {r:?}");
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
    let row = |name: &str| &table.iter().find(|t| t.0 == name).unwrap().1;
    let gone = &row("delete")[1];
    assert_eq!(
        (&gone.body["reason"], gone.session_id.as_deref()),
        (&json!("deleted"), Some(sid.as_str()))
    );

    // Watches (§6.4): each start, update and stop names its target and
    // session.
    let disarm = &row("watch disarm")[0];
    assert_eq!(
        (&disarm.body["target"], &disarm.body["fields"]),
        (&json!("artifact"), &json!({"replies_armed": false}))
    );
    assert!(disarm.body.get("cause").is_none());
    assert_eq!(disarm.session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(disarm.artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(row("unwatch")[0].body["replies_armed"], false);
    let made = row("live watch");
    assert_eq!(
        kinds(made),
        [
            "artifact.create",
            "live.page",
            "live.snapshot",
            "watch.start",
            "watch.start"
        ]
    );
    let billing = made[0].artifact_id.clone().unwrap();
    assert_eq!(
        (
            &made[3].body["target"],
            &made[3].body["path"],
            &made[3].origin
        ),
        (
            &json!("scope"),
            &json!("/billing"),
            &Some("http://localhost:5173".to_string())
        )
    );
    assert_eq!(
        (
            &made[4].body["target"],
            &made[4].body["source"],
            made[4].artifact_id.as_deref()
        ),
        (&json!("page"), &json!("scope"), Some(billing.as_str()))
    );
    assert_eq!(made[4].body["cause"], "scope");
    assert!(made[3].body.get("cause").is_none());
    let disarmed = row("scope disarm");
    assert_eq!(
        (&disarmed[0].body["target"], &disarmed[0].body["fields"]),
        (&json!("scope"), &json!({"replies_armed": false}))
    );
    assert_eq!(
        (
            &disarmed[1].body["target"],
            &disarmed[1].body["fields"],
            &disarmed[1].body["cause"]
        ),
        (
            &json!("page"),
            &json!({"replies_armed": false}),
            &json!("scope")
        )
    );
    assert_eq!(disarmed[1].artifact_id.as_deref(), Some(billing.as_str()));
    let unwatched = row("scope unwatch");
    assert_eq!(
        (&unwatched[0].body["target"], &unwatched[1].body["target"]),
        (&json!("scope"), &json!("page"))
    );
    assert_eq!(unwatched[1].artifact_id.as_deref(), Some(billing.as_str()));

    // Working records (§6.5): heartbeats and updates record nothing.
    let started = &row("working")[0];
    assert_eq!(
        (&started.body["message"], &started.body["thread_ids"]),
        (&json!("Tightening"), &json!([]))
    );
    assert!(started.body["key"].is_string());
    let cleared = &row("working clear")[0];
    assert_eq!(cleared.body["key"], started.body["key"]);
    assert_eq!(cleared.body["reason"], "explicit");
    assert!(cleared.body["duration_ms"].as_i64().unwrap() >= 0);
    assert!(cleared.body.get("for_actor").is_none());
    assert_eq!(row("turn end with a record")[0].body["reason"], "explicit");

    // A move (§6.3): the source page and the target, both artifacts named.
    let moved = row("move");
    // The thread's two versions, then the page's own current one on top.
    let sources: Vec<(&Value, &Value)> = moved[..3]
        .iter()
        .map(|r| (&r.body["source"]["artifact_id"], &r.body["source"]["n"]))
        .collect();
    assert_eq!(
        sources,
        [
            (&json!(page), &json!(1)),
            (&json!(page), &json!(2)),
            (&json!(billing), &json!(1))
        ]
    );
    let mv = &moved[3];
    assert_eq!(mv.artifact_id.as_deref(), Some(page.as_str()));
    assert_eq!(mv.artifact2_id.as_deref(), Some(billing.as_str()));
    assert_eq!(mv.body["from_artifact_id"], page.as_str());
    assert_eq!(mv.body["to_artifact_id"], billing.as_str());
    assert_eq!(mv.body["move_kind"], "move");
    assert_eq!(mv.body["rule_id"], Value::Null);
    assert!(mv.body["move_id"].is_string());
    assert!(
        mv.body["to_url"]
            .as_str()
            .unwrap()
            .starts_with("http://localhost:5173/billing"),
        "{:?}",
        mv.body
    );
    // The watch the move carried, after it, naming it.
    let carried = &moved[4];
    assert_eq!(
        (
            carried.session_id.as_deref(),
            carried.artifact_id.as_deref(),
            &carried.body["cause"],
            &carried.body["move_id"],
            &carried.body["source"]
        ),
        (
            Some(watcher.as_str()),
            Some(billing.as_str()),
            &json!("move"),
            &mv.body["move_id"],
            &json!("direct")
        )
    );

    // Rules (§6.3).
    let set = &row("rule")[0];
    assert_eq!(
        (&set.body["op"], &set.body["pattern"], &set.body["rule_id"]),
        (&json!("set"), &json!("/none/:id"), &json!(rule_id))
    );
    assert_eq!(set.origin.as_deref(), Some("http://localhost:5173"));
    let ops: Vec<&Value> = row("rule delete").iter().map(|r| &r.body["op"]).collect();
    assert_eq!(ops, [&json!("delete"), &json!("drop")]);
    assert_eq!(row("rule delete")[1].body["rule_id"], rule_id.as_str());

    // Sessions (§6.8): steps of the install path, as the session's agent.
    let joined = &row("join")[0];
    assert_eq!(joined.body["transcript_path"], "/t/hs-every.jsonl");
    assert_eq!(joined.actor["transcript_path"], "/t/hs-every.jsonl");
    assert_eq!(joined.body["harness_session_id"], "hs-every");
    assert_eq!(joined.session_id.as_deref(), Some(sid.as_str()));
    let started = &row("register")[0];
    assert_eq!(
        (
            &started.body["harness"],
            &started.body["cwd"],
            &started.body["pid"]
        ),
        (&json!("codex"), &json!("/w"), &json!(77))
    );
    assert_eq!(started.actor["session_id"], codex.as_str());
    assert_eq!(started.body["via"], "mcp");
    let ended = &row("session end")[0];
    assert_eq!(ended.body["reason"], "explicit");
    assert_eq!(ended.session_id.as_deref(), Some(codex.as_str()));
    let opened = &row("thread")[0];
    assert_eq!(opened.body["version_n"], 2);
    assert_eq!(opened.body["anchor"]["selector"], "body > main > h2");
    assert_eq!(opened.body["anchor"]["quote"], "Quarterly goals");
    assert!(
        opened.body["anchor"].get("rect").is_none(),
        "{:?}",
        opened.body
    );
    assert_eq!(opened.body["has_clip"], false);
    assert_eq!(
        opened.body["first_comment_id"],
        row("thread")[1].body["comment_id"]
    );
    assert_eq!(row("thread")[1].body["body"], "Tighten");
    let resolved = &row("agent resolve");
    assert_eq!(resolved[0].body["resolved_by"], "agent:claude");
    assert_eq!(resolved[0].body["addressed_version"], 2);
    assert_eq!(
        row("send to one agent")[0].body["target"],
        json!({"session_id": sid, "agent_handle": handle})
    );
    for (name, via) in [("prompt hook poll", "hook"), ("inject poll", "pi")] {
        assert_eq!(row(name)[0].body["via"], via, "{name}");
    }
    let tiers: Vec<&str> = [
        "acknowledge",
        "agent reply",
        "feedback poll",
        "prompt hook poll",
        "inject poll",
    ]
    .iter()
    .flat_map(|n| row(n).iter())
    .filter(|r| r.kind == "feedback.delivered")
    .map(|r| r.body["tier"].as_str().unwrap())
    .collect();
    assert_eq!(
        tiers,
        [
            "piggyback",
            "piggyback",
            "piggyback",
            "wait",
            "prompt_hook",
            "inject"
        ]
    );
    for r in table.iter().flat_map(|(_, recs, ..)| recs) {
        if r.kind == "feedback.delivered" {
            assert_eq!(r.session_id.as_deref(), Some(sid.as_str()), "{r:?}");
        }
    }
    assert_eq!(row("thread delete")[0].body["moved"], false);
    let marked = &row("feedback poll")[1];
    assert_eq!(marked.body["thread_ids"], json!([t1]));
    assert_eq!(marked.session_id.as_deref(), Some(sid.as_str()));
}

/// The thread a request made, and the events it recorded.
async fn made(req: reqwest::RequestBuilder, ts: &TestServer) -> (String, Vec<Rec>) {
    let seq = last_seq(ts);
    let (code, v) = status(req).await;
    assert_eq!(code, 201, "{v}");
    (
        v["thread"]["id"].as_str().unwrap().to_string(),
        events_since(ts, seq),
    )
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
        [
            "artifact.create",
            "live.page",
            "live.snapshot",
            "thread.open",
            "comment.add"
        ]
    );
    let origin = "http://localhost:5173";
    assert_eq!(recs[0].body["kind"], "live");
    for r in &recs {
        assert_eq!(r.artifact_id.as_deref(), Some(aid));
        assert_eq!(r.actor["public_id"], sam.public_id.as_str());
    }
    assert_eq!(recs[3].body["anchor"]["route"], "?tab=x");
    for r in &recs[1..3] {
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
    // The publish watches the artifact for the session.
    assert_eq!(
        kinds(&recs),
        ["artifact.create", "version.publish", "watch.start"]
    );
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
        (reqwest::Method::POST, "/api/live/sites/join".to_string()),
        (reqwest::Method::POST, "/api/live/sites/split".to_string()),
        (reqwest::Method::POST, "/api/live/sites/answer".to_string()),
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

/// An artifact the session `sid` owns, at version 2, with a thread sent to
/// the agent: the artifact's and the thread's IDs.
async fn sent_thread(ts: &TestServer, sid: &str) -> (String, String) {
    let aid = ts
        .publish_as(sid, "Report", "<h2>Quarterly goals</h2>")
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .json(&json!({"if_version": 1, "files": {"index.html": utf8("<h2>Goals</h2>")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let tid = ts.thread(&aid, 2, "@agent tighten this").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    (aid, tid)
}

#[tokio::test]
async fn lan_reply_records_viewer_identity() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let session = ts.register_session("claude", "hs-lan").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let (aid, tid) = sent_thread(&ts, &sid).await;
    let sam = ts.viewer(Some("Sam")).await;
    let (lan, base) = ts.lan();
    let seq = last_seq(&ts);
    let res = lan
        .post(format!("{base}/api/artifacts/{aid}/threads/{tid}/comments"))
        .header("cookie", format!("clax_viewer={}", sam.cookie))
        .json(&json!({"body": "Still too long"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let recs = events_since(&ts, seq);
    // The thread was sent, so the comment is forwarded too.
    assert_eq!(kinds(&recs), ["comment.add", "thread.send"]);
    let viewer = json!({"type": "viewer", "public_id": sam.public_id, "display_name": "Sam"});
    for r in &recs {
        assert_eq!(r.actor, viewer, "{r:?}");
        assert_eq!(r.body["via"], "lan");
        assert_eq!(r.session_id, None, "a viewer has no session");
        assert!(r.body.get("git").is_none() && r.body.get("call").is_none());
    }
    let c = &recs[0].body;
    assert_eq!(c["comment_id"], v["comment"]["id"]);
    assert_eq!(c["body"], "Still too long");
    assert_eq!(
        (&c["author_kind"], &c["author_name"]),
        (&json!("viewer"), &json!("Sam"))
    );
    assert_eq!(
        (&c["via_harness"], &c["via_page"]),
        (&Value::Null, &json!(false))
    );
    // The thread names no one agent, so it goes to every live owner and
    // watcher.
    let send = &recs[1].body;
    assert_eq!(send["target"], "watchers");
    assert_eq!(send["thread_ids"], json!([tid]));
    assert_eq!(send["feedback_ids"].as_array().map(Vec::len), Some(1));
    assert!(send.get("batch_id").is_none(), "{send}");
}

#[tokio::test]
async fn agent_reply_records_session_git_and_call() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-reply").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let (aid, tid) = sent_thread(&ts, &sid).await;
    let plain = ts.thread(&aid, 2, "plain").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reply = |tid: &str, body: &str| {
        ts.authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .header("x-clax-call", encode_call_header(&call()).unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .json(&json!({"body": body, "author_kind": "agent"}))
    };
    // A thread with a clip says so.
    let seq = last_seq(&ts);
    let res = ts
        .create_thread(
            &aid,
            2,
            "see the clip",
            Some(clax_server::testing::FAKE_PNG),
        )
        .await;
    assert_eq!(res.status(), 201);
    let opened = events_since(&ts, seq);
    assert_eq!(opened[0].kind, "thread.open");
    assert_eq!(opened[0].body["has_clip"], true);
    // Refused or guided replies record nothing.
    let seq = last_seq(&ts);
    let (code, v) = status(reply(&plain, "Done")).await;
    assert_eq!(code, 200, "{v}");
    assert!(v["guidance"].is_string(), "{v}");
    assert_eq!(status(reply(&tid, "   ")).await.0, 400);
    assert_eq!(last_seq(&ts), seq, "{:?}", events_since(&ts, seq));

    let (code, v) = status(reply(&tid, "Tightened the copy")).await;
    assert_eq!(code, 201, "{v}");
    let recs = events_since(&ts, seq);
    // The reply, then the first delivery of the comment it answers.
    assert_eq!(kinds(&recs), ["comment.add", "feedback.delivered"]);
    for r in &recs {
        assert_eq!(r.actor["type"], "agent");
        assert_eq!(r.actor["session_id"], sid.as_str());
        assert_eq!(r.actor["harness_session_id"], "hs-reply");
        assert_eq!(r.session_id.as_deref(), Some(sid.as_str()));
        assert_eq!(r.call_id.as_deref(), Some(call().call_id.as_str()));
        assert_eq!(r.body["call"]["tool"], "publish");
        assert_eq!(r.body["via"], "mcp");
        assert_eq!(r.body["git"]["head"], git().head.unwrap().as_str());
        assert_eq!(r.body["git_capture"], "ok");
        assert_eq!(r.artifact_id.as_deref(), Some(aid.as_str()));
    }
    let c = &recs[0].body;
    assert_eq!(c["comment_id"], v["comment"]["id"]);
    assert_eq!(c["body"], "Tightened the copy");
    assert_eq!(
        (&c["author_kind"], &c["author_name"], &c["via_harness"]),
        (&json!("agent"), &json!("claude"), &json!("claude"))
    );
    assert_eq!(recs[1].body["tier"], "piggyback");
    let fid: String = db(&ts)
        .query_row(
            "SELECT id FROM feedback WHERE thread_id = ?1",
            [&tid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recs[1].body["feedback_id"], fid.as_str());
}

#[tokio::test]
async fn batch_send_is_one_event_listing_threads() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-batch").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let (aid, sent) = sent_thread(&ts, &sid).await;
    let id = |v: Value| v["id"].as_str().unwrap().to_string();
    let a = id(ts.thread(&aid, 2, "first").await);
    let b = id(ts.thread(&aid, 2, "second").await);
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .json(&json!({
                "thread_ids": [a, sent, b],
                "note": "both",
                "to": session["agent_handle"]
            })),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["unchanged"], json!([sent]));
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["thread.send"], "one event for the batch");
    let r = &recs[0];
    assert_eq!(r.artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(r.body["batch_id"], v["batch"]["id"]);
    assert_eq!(r.body["thread_ids"], json!([a, b]));
    assert_eq!(
        r.body["target"],
        json!({"session_id": sid, "agent_handle": session["agent_handle"]})
    );
    let rows: Vec<String> = {
        let c = db(&ts);
        let mut q = c
            .prepare("SELECT id FROM feedback WHERE batch_id = ?1 ORDER BY created_at, id")
            .unwrap();
        q.query_map([v["batch"]["id"].as_str().unwrap()], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(rows.len(), 2);
    let mut recorded: Vec<String> = serde_json::from_value(r.body["feedback_ids"].clone()).unwrap();
    recorded.sort();
    let mut rows = rows;
    rows.sort();
    assert_eq!(recorded, rows);
    // A batch with nothing to send records nothing.
    let seq = last_seq(&ts);
    let (code, _) = status(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .json(&json!({"thread_ids": [a, b]})),
    )
    .await;
    assert_eq!(code, 409);
    assert_eq!(last_seq(&ts), seq);
    // Sent to every live owner and watcher, the target is `watchers`.
    let c = id(ts.thread(&aid, 2, "third").await);
    let seq = last_seq(&ts);
    let (code, _) = status(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .json(&json!({"thread_ids": [c]})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(events_since(&ts, seq)[0].body["target"], "watchers");
}

#[tokio::test]
async fn delivery_retry_is_not_recorded() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-retry").await;
    let sid = session["id"].as_str().unwrap().to_string();
    sent_thread(&ts, &sid).await;
    let take = || async {
        let res = ts
            .authed(ts.client.get(format!(
                "{}/api/sessions/{sid}/feedback?tier=stop_hook&wait=0",
                ts.base
            )))
            .header("x-clax-via", "hook")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json::<Value>().await.unwrap()["feedback"].clone()
    };
    // Armed, so the Stop hook tier hands the row over.
    let watch = db(&ts)
        .execute(
            "UPDATE watches SET replies_armed = 1 WHERE session_id = ?1",
            [&sid],
        )
        .unwrap();
    assert_eq!(watch, 1);
    let seq = last_seq(&ts);
    let first = take().await;
    assert_eq!(first.as_array().map(Vec::len), Some(1), "{first}");
    let recs = events_since(&ts, seq);
    // The feedback marks the session working on the artifact.
    assert_eq!(kinds(&recs), ["feedback.delivered", "working.start"]);
    assert_eq!(recs[1].body["via"], "hook");
    assert_eq!(recs[1].session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(recs[0].body["tier"], "stop_hook");
    assert_eq!(recs[0].body["via"], "hook");
    assert_eq!(recs[0].actor["session_id"], sid.as_str());
    assert_eq!(recs[0].body["feedback_id"], first[0]["feedback_id"]);
    // Unacknowledged past the resend delay: handed over again, not recorded.
    db(&ts)
        .execute(
            "UPDATE feedback SET last_sent_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    let seq = last_seq(&ts);
    let again = take().await;
    assert_eq!(again[0]["resent"], true, "{again}");
    // Acknowledging a delivered row records nothing either.
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/feedback/ack", ts.base)),
        )
        .json(&json!({"comment_ids": [first[0]["comment_id"]]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(last_seq(&ts), seq, "{:?}", events_since(&ts, seq));
}

#[tokio::test]
async fn resolve_addressed_links_version() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-resolve").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let (aid, tid) = sent_thread(&ts, &sid).await;
    let resolve = |aid: &str, tid: &str| {
        ts.authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"as": "agent"}))
    };
    let seq = last_seq(&ts);
    assert_eq!(status(resolve(&aid, &tid)).await.0, 200);
    let recs = events_since(&ts, seq);
    // The link rides in the resolve: no separate `thread.addressed`.
    assert_eq!(kinds(&recs), ["feedback.delivered", "thread.resolve"]);
    assert_eq!(
        (
            &recs[1].body["resolved_by"],
            &recs[1].body["addressed_version"]
        ),
        (&json!("agent:claude"), &json!(2))
    );
    assert_eq!(recs[1].artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(recs[1].actor["session_id"], sid.as_str());
    // A thread a viewer already resolved, then resolved by the agent: the
    // link is the only change, recorded alone.
    let second = ts.thread(&aid, 2, "@agent and this").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{second}/resolve",
            ts.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let seq = last_seq(&ts);
    assert_eq!(status(resolve(&aid, &second)).await.0, 200);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["thread.addressed"]);
    assert_eq!(
        (&recs[0].body["version_n"], &recs[0].body["source"]),
        (&json!(2), &json!("resolve"))
    );
    let thread_col: Option<String> = db(&ts)
        .query_row(
            "SELECT thread_id FROM audit_events WHERE kind = 'thread.addressed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(thread_col.as_deref(), Some(second.as_str()));
    // Reopened and resolved again: already linked, so no second link.
    assert_eq!(
        status(ts.authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/reopen",
            ts.base
        ))))
        .await
        .0,
        200
    );
    let seq = last_seq(&ts);
    assert_eq!(status(resolve(&aid, &tid)).await.0, 200);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["thread.resolve"]);
    assert_eq!(recs[0].body["addressed_version"], Value::Null);
    // A viewer's resolve addresses nothing.
    let other = ts.thread(&aid, 2, "plain").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let seq = last_seq(&ts);
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{other}/resolve",
            ts.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["thread.resolve"]);
    assert_eq!(
        (
            &recs[0].body["resolved_by"],
            &recs[0].body["addressed_version"]
        ),
        (&json!("viewer:anonymous"), &Value::Null)
    );
    // On a live page the address waits for the next snapshot, which lists it.
    let sam = ts.viewer(Some("Sam")).await;
    let (code, v) = status(
        ts.client
            .post(format!("{}/api/live/threads", ts.base))
            .header("cookie", format!("clax_viewer={}", sam.cookie))
            .multipart(live_form(
                "http://localhost:5173/settings",
                "<main>Save</main>",
            )),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let page = v["page"]["artifact_id"].as_str().unwrap().to_string();
    let live = v["thread"]["id"].as_str().unwrap().to_string();
    let watch = format!("{}/api/sessions/{sid}/watches/{page}", ts.base);
    assert_eq!(status(ts.authed(ts.client.put(&watch))).await.0, 200);
    ts.send_thread(&page, &live).await;
    let seq = last_seq(&ts);
    assert_eq!(status(resolve(&page, &live)).await.0, 200);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["feedback.delivered", "thread.resolve"]);
    assert_eq!(recs[1].body["addressed_version"], Value::Null);
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/settings")
        .text("title", "Settings")
        .text("pending", json!([live]).to_string())
        .text("snapshot", "<main>Saved</main>");
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.client
            .post(format!("{}/api/live/snapshots", ts.base))
            .header("cookie", format!("clax_viewer={}", sam.cookie))
            .multipart(form),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(
        kinds(&recs),
        ["live.snapshot"],
        "the link rides in the snapshot"
    );
    assert_eq!(recs[0].body["addresses"], json!([live]));
}

#[tokio::test]
async fn a_refused_thread_request_writes_nothing() {
    let ts = TestServer::spawn().await;
    let v = ts
        .publish("Report", &[("index.html", "<h2>Quarterly goals</h2>")])
        .await;
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let tid = ts.thread(&aid, 1, "hi").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let viewers = || -> i64 {
        db(&ts)
            .query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))
            .unwrap()
    };
    let before = viewers();
    let seq = last_seq(&ts);
    let url = format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base);
    // A first-time viewer has no name: refused before its row is made.
    let fresh = "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    for req in [
        ts.client.post(format!("{url}/reopen")),
        ts.client.delete(&url),
    ] {
        let res = req.header("cookie", fresh).send().await.unwrap();
        assert_eq!(res.status(), 403);
    }
    // An agent's delete without the token is unauthorised, as before.
    let res = ts
        .client
        .delete(format!("{url}?as=agent"))
        .header("cookie", fresh)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // So is any thread request refused before it acts, the agent's
    // without the token included.
    let threads = format!("{}/api/artifacts/{aid}/threads", ts.base);
    for (req, want) in [
        (
            ts.client
                .post(format!("{url}/reopen"))
                .json(&json!({"as": "agent"})),
            401,
        ),
        (
            ts.client
                .post(format!("{url}/resolve"))
                .json(&json!({"as": "agent"})),
            401,
        ),
        (
            ts.client
                .post(format!("{url}/comments"))
                .json(&json!({"body": "hi", "author_kind": "agent"})),
            401,
        ),
        (
            ts.client
                .post(format!("{url}/comments"))
                .json(&json!({"body": "hi", "author_kind": "robot"})),
            400,
        ),
        (ts.client.post(format!("{url}/send")).body("{not json"), 400),
        (
            ts.client
                .post(format!("{threads}:send"))
                .json(&json!({"thread_ids": "nope"})),
            400,
        ),
        (
            ts.client
                .post(&threads)
                .multipart(reqwest::multipart::Form::new().text("body", "no anchor")),
            400,
        ),
    ] {
        let res = req.header("cookie", fresh).send().await.unwrap();
        assert_eq!(res.status(), want, "{:?}", res.url());
    }
    assert_eq!(viewers(), before, "no viewer row was made");
    assert_eq!(last_seq(&ts), seq);
    // Accepted by the access checks and refused by the store (an empty
    // comment): the requester's viewer row may be made, but no event.
    let res = ts
        .client
        .post(format!("{url}/comments"))
        .header("cookie", fresh)
        .json(&json!({"body": "   "}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        viewers(),
        before + 1,
        "the requester's row, as ensure_viewer makes it"
    );
    assert_eq!(last_seq(&ts), seq, "{:?}", events_since(&ts, seq));
}

/// A server whose working registry reads `clock`.
async fn server_at(clock: std::sync::Arc<clax_core::working::ManualClock>) -> TestServer {
    TestServer::spawn_with(move |s| {
        s.working = std::sync::Arc::new(clax_core::working::Working::new(clock))
    })
    .await
}

#[tokio::test]
async fn ttl_expiry_records_system_stop_with_for_actor() {
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-07T10:00:00Z"));
    let ts = server_at(clock.clone()).await;
    let session = ts.register_session("claude", "hs-ttl").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let aid = ts.publish_as(&sid, "T", "<main></main>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let put = |aid: &str| {
        ts.authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/working/{aid}", ts.base)),
        )
        .json(&json!({"message": "Working"}))
    };
    assert_eq!(status(put(&aid)).await.0, 200);
    let started = events_since(&ts, 0)
        .into_iter()
        .find(|r| r.kind == "working.start")
        .unwrap();
    // The sweep records a lapsed record's end as Clax's, for the agent.
    clock.advance(30);
    let seq = last_seq(&ts);
    clax_server::working::sweep_and_announce(&ts.store, &ts.working, &ts.events).await;
    assert_eq!(last_seq(&ts), seq, "nothing lapsed yet");
    clock.advance(130);
    clax_server::working::sweep_and_announce(&ts.store, &ts.working, &ts.events).await;
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["working.stop"]);
    let stop = &recs[0];
    assert_eq!(stop.actor, json!({"type": "system", "reason": "ttl"}));
    assert_eq!(stop.body["via"], "daemon");
    assert_eq!(stop.body["reason"], "ttl");
    assert_eq!(stop.body["key"], started.body["key"]);
    // It ended when it lapsed, 120 s after its last renewal.
    assert_eq!(stop.body["duration_ms"], 120_000);
    assert_eq!(stop.body["for_actor"]["type"], "agent");
    assert_eq!(stop.body["for_actor"]["session_id"], sid.as_str());
    assert_eq!(stop.body["for_actor"]["harness_session_id"], "hs-ttl");
    assert_eq!(
        (stop.session_id.as_deref(), stop.artifact_id.as_deref()),
        (Some(sid.as_str()), Some(aid.as_str()))
    );
    // A lapsed record a later change finds before the sweep is recorded the
    // same way, before that change's own events.
    assert_eq!(status(put(&aid)).await.0, 200);
    clock.advance(500);
    let seq = last_seq(&ts);
    let other = ts.publish_as(&sid, "U", "<main></main>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(status(put(&other)).await.0, 200);
    let recs: Vec<Rec> = events_since(&ts, seq)
        .into_iter()
        .filter(|r| r.kind.starts_with("working."))
        .collect();
    assert_eq!(kinds(&recs), ["working.stop", "working.start"]);
    assert_eq!(recs[0].actor["reason"], "ttl");
    assert_eq!(recs[0].body["duration_ms"], 120_000);
    assert_eq!(recs[0].artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(recs[1].actor["type"], "agent");
}

#[tokio::test]
async fn heartbeat_records_nothing() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-beat").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let aid = ts.publish_as(&sid, "T", "<main></main>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.base.clone();
    assert_eq!(
        status(
            ts.authed(
                ts.client
                    .put(format!("{b}/api/sessions/{sid}/working/{aid}"))
            )
            .json(&json!({}))
        )
        .await
        .0,
        200
    );
    let seq = last_seq(&ts);
    for _ in 0..3 {
        let (code, v) = status(
            ts.authed(ts.client.patch(format!("{b}/api/sessions/{sid}")))
                .json(&json!({"heartbeat": true})),
        )
        .await;
        assert_eq!(code, 200, "{v}");
        let (code, v) = status(
            ts.authed(
                ts.client
                    .post(format!("{b}/api/sessions/{sid}/working/renew")),
            ),
        )
        .await;
        assert_eq!((code, &v["renewed"]), (200, &json!(1)), "{v}");
        // A shim registering again, and a hook joining again, change
        // nothing the records hold.
        let (code, v) = status(
            ts.authed(ts.client.post(format!("{b}/api/sessions")))
                .json(&json!({"harness": "claude", "harness_session_id": "hs-beat", "cwd": "/w"})),
        )
        .await;
        assert_eq!((code, &v["session"]["id"]), (201, &json!(sid)), "{v}");
        let (code, v) = status(
            ts.authed(ts.client.post(format!("{b}/api/sessions/join")))
                .json(
                    &json!({"harness": "claude", "parent_pid": 1, "harness_session_id": "hs-beat"}),
                ),
        )
        .await;
        assert_eq!((code, &v["session"]["id"]), (200, &json!(sid)), "{v}");
    }
    assert_eq!(last_seq(&ts), seq, "{:?}", events_since(&ts, seq));
}

#[tokio::test]
async fn join_records_the_transcript_path_for_each_harness() {
    let ts = TestServer::spawn().await;
    let b = ts.base.clone();
    for harness in ["claude", "codex", "grok"] {
        // A hook that runs before the shim makes the row; the shim adopts it.
        let seq = last_seq(&ts);
        let transcript = format!("/t/{harness}.jsonl");
        let (code, v) = status(
            ts.authed(ts.client.post(format!("{b}/api/sessions/join")))
                .header("x-clax-via", "hook")
                .json(&json!({
                    "harness": harness, "parent_pid": 9000, "harness_session_id": format!("{harness}-1"),
                    "cwd": "/w", "transcript_path": transcript
                })),
        )
        .await;
        assert_eq!(code, 200, "{v}");
        let sid = v["session"]["id"].as_str().unwrap().to_string();
        let recs = events_since(&ts, seq);
        assert_eq!(kinds(&recs), ["session.start"], "{harness}");
        let r = &recs[0];
        assert_eq!(r.body["transcript_path"], transcript.as_str());
        assert_eq!(r.body["harness"], harness);
        assert_eq!(r.body["via"], "hook");
        assert_eq!(r.actor["session_id"], sid.as_str());
        assert_eq!(r.actor["transcript_path"], transcript.as_str());
        assert!(r.artifact_id.is_none(), "a session is never a path");
        // A resumed harness session names a new transcript: a join.
        let seq = last_seq(&ts);
        let resumed = format!("/t/{harness}-resumed.jsonl");
        let (code, v) = status(
            ts.authed(ts.client.post(format!("{b}/api/sessions/join")))
                .json(&json!({
                    "harness": harness, "parent_pid": 9000, "harness_session_id": format!("{harness}-1"),
                    "transcript_path": resumed
                })),
        )
        .await;
        assert_eq!(code, 200, "{v}");
        let recs = events_since(&ts, seq);
        assert_eq!(kinds(&recs), ["session.join"], "{harness}");
        assert_eq!(recs[0].body["transcript_path"], resumed.as_str());
        assert_eq!(
            ts.store
                .session_actor(&sid)
                .unwrap()
                .unwrap()
                .transcript_path,
            Some(resumed)
        );
    }
    // A transcript path with control characters is refused, and records
    // nothing.
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{b}/api/sessions/join")))
            .json(&json!({
                "harness": "claude", "parent_pid": 9001, "harness_session_id": "bad",
                "transcript_path": "/t/a\nb"
            })),
    )
    .await;
    assert_eq!(code, 400, "{v}");
    assert_eq!(last_seq(&ts), seq);
    // Pi names its session file when it registers.
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{b}/api/sessions")))
            .header("x-clax-via", "pi")
            .json(&json!({
                "harness": "pi", "harness_session_id": "pi-1", "cwd": "/w",
                "transcript_path": "/s/pi-1.jsonl"
            })),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["session.start"]);
    assert_eq!(recs[0].body["transcript_path"], "/s/pi-1.jsonl");
    assert_eq!(recs[0].body["via"], "pi");
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{b}/api/sessions")))
            .json(&json!({
                "harness": "pi", "harness_session_id": "pi-2", "cwd": "/w",
                "transcript_path": "x".repeat(4097)
            })),
    )
    .await;
    assert_eq!(
        (code, &v["error"]["code"]),
        (400, &json!("invalid_session"))
    );
    assert_eq!(last_seq(&ts), seq);
}

#[tokio::test]
async fn merge_records_each_moved_thread_with_both_artifacts() {
    let ts = TestServer::spawn().await;
    let sam = ts.viewer(Some("Sam")).await;
    let b = ts.base.clone();
    let mut pages = Vec::new();
    for path in ["/users/1", "/users/2"] {
        let (code, v) = status(
            ts.client
                .post(format!("{b}/api/live/threads"))
                .header("cookie", format!("clax_viewer={}", sam.cookie))
                .multipart(live_form(
                    &format!("http://localhost:5173{path}"),
                    "<main>Save</main>",
                )),
        )
        .await;
        assert_eq!(code, 201, "{v}");
        pages.push((
            v["page"]["artifact_id"].as_str().unwrap().to_string(),
            v["thread"]["id"].as_str().unwrap().to_string(),
        ));
    }
    let moves = |recs: &[Rec]| -> Vec<(String, String, String, String)> {
        recs.iter()
            .filter(|r| r.kind == "thread.move")
            .map(|r| {
                assert_eq!(r.body["from_artifact_id"], json!(r.artifact_id));
                assert_eq!(r.body["to_artifact_id"], json!(r.artifact2_id));
                assert_eq!(r.origin.as_deref(), Some("http://localhost:5173"));
                (
                    r.artifact_id.clone().unwrap(),
                    r.artifact2_id.clone().unwrap(),
                    r.body["move_kind"].as_str().unwrap().to_string(),
                    r.body["rule_id"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };

    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{b}/api/live/rules")))
            .json(&json!({"origin": "http://localhost:5173", "pattern": "/users/:id"})),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    assert_eq!(v["moved"].as_array().unwrap().len(), 2, "{v}");
    let rule = v["rule"]["id"].as_str().unwrap().to_string();
    let canon = v["page"]["artifact_id"].as_str().unwrap().to_string();
    let recs = events_since(&ts, seq);
    assert_eq!(recs[0].kind, "live.rule");
    assert_eq!(recs[0].body["op"], "set");
    for r in &recs {
        assert_eq!(r.actor["type"], "owner", "{r:?}");
    }
    let mut merged = moves(&recs);
    merged.sort();
    let mut want: Vec<_> = pages
        .iter()
        .map(|(page, _)| {
            (
                page.clone(),
                canon.clone(),
                "merge".to_string(),
                rule.clone(),
            )
        })
        .collect();
    want.sort();
    assert_eq!(merged, want);
    let threads: Vec<&str> = recs
        .iter()
        .filter(|r| r.kind == "thread.move")
        .map(|r| r.body["move_id"].as_str().unwrap())
        .collect();
    assert_eq!(threads.len(), 2);
    assert_ne!(threads[0], threads[1]);

    // Deleting the rule moves each thread back to its own page.
    let seq = last_seq(&ts);
    let (code, v) = status(ts.authed(ts.client.delete(format!("{b}/api/live/rules/{rule}")))).await;
    assert_eq!(code, 200, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(recs[0].kind, "live.rule");
    assert_eq!(recs[0].body["op"], "delete");
    let mut back = moves(&recs);
    back.sort();
    let mut want: Vec<_> = pages
        .iter()
        .map(|(page, _)| {
            (
                canon.clone(),
                page.clone(),
                "unmerge".to_string(),
                rule.clone(),
            )
        })
        .collect();
    want.sort();
    assert_eq!(back, want);
    // A rule already gone: refused, nothing recorded.
    let seq = last_seq(&ts);
    let (code, _) = status(ts.authed(ts.client.delete(format!("{b}/api/live/rules/{rule}")))).await;
    assert_eq!(code, 404);
    assert_eq!(last_seq(&ts), seq);
}

#[tokio::test]
async fn working_ends_name_their_cause() {
    let ts = TestServer::spawn().await;
    let sam = ts.viewer(Some("Sam")).await;
    let session = ts.register_session("claude", "hs-cause").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let aid = ts
        .publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>")
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.base.clone();
    let threads: Vec<String> = {
        let mut v = Vec::new();
        for i in 0..4 {
            let t = ts.thread(&aid, 1, &format!("t{i}")).await;
            let tid = t["id"].as_str().unwrap().to_string();
            ts.send_thread(&aid, &tid).await;
            v.push(tid);
        }
        v
    };
    let work = |tids: Vec<&str>| {
        ts.authed(
            ts.client
                .put(format!("{b}/api/sessions/{sid}/working/{aid}")),
        )
        .json(&json!({"thread_ids": tids}))
    };
    let viewer =
        |r: reqwest::RequestBuilder| r.header("cookie", format!("clax_viewer={}", sam.cookie));
    let agent = |r: reqwest::RequestBuilder| ts.authed(r).header("x-clax-session", &sid);
    let t = |tid: &str, rest: &str| format!("{b}/api/artifacts/{aid}/threads/{tid}{rest}");
    // Each step makes a record naming one thread, then ends that thread.
    let steps: Vec<(reqwest::RequestBuilder, &str, &str, &str)> = vec![
        (
            viewer(ts.client.post(t(&threads[0], "/resolve"))),
            "thread.resolve",
            "resolved",
            "viewer",
        ),
        (
            viewer(ts.client.delete(t(&threads[1], ""))),
            "thread.delete",
            "deleted",
            "viewer",
        ),
        (
            agent(ts.client.post(t(&threads[2], "/resolve"))).json(&json!({"as": "agent"})),
            "thread.resolve",
            "resolved",
            "agent",
        ),
        (
            agent(ts.client.post(t(&threads[3], "/comments")))
                .json(&json!({"body": "Done", "author_kind": "agent"})),
            "comment.add",
            "explicit",
            "agent",
        ),
    ];
    for (i, (req, kind, reason, actor)) in steps.into_iter().enumerate() {
        assert_eq!(status(work(vec![&threads[i]])).await.0, 200);
        let seq = last_seq(&ts);
        let (code, v) = status(req).await;
        assert!((200..300).contains(&code), "{kind}: {code} {v}");
        let recs = events_since(&ts, seq);
        assert!(recs.iter().any(|r| r.kind == kind), "{:?}", kinds(&recs));
        let stop = recs.iter().find(|r| r.kind == "working.stop").unwrap();
        assert_eq!(stop.body["reason"], reason, "{kind}");
        assert_eq!(stop.actor["type"], actor, "{kind}");
        assert!(stop.body.get("for_actor").is_none());
    }

    // The session ends with a record live: its end, then the record's.
    assert_eq!(status(work(vec![])).await.0, 200);
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.patch(format!("{b}/api/sessions/{sid}")))
            .json(&json!({"ended": true})),
    )
    .await;
    assert_eq!(code, 200, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["session.end", "working.stop"]);
    assert_eq!(recs[1].body["reason"], "session_end");
    assert_eq!(recs[1].actor["session_id"], sid.as_str());

    // The reaper ends an idle session's records as Clax's, for the agent.
    let idle = ts.register_session("claude", "hs-idle").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other = ts.publish_as(&idle, "U", "<main></main>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        status(
            ts.authed(
                ts.client
                    .put(format!("{b}/api/sessions/{idle}/working/{other}"))
            )
            .json(&json!({}))
        )
        .await
        .0,
        200
    );
    db(&ts)
        .execute(
            "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
            [&idle],
        )
        .unwrap();
    let seq = last_seq(&ts);
    let (feedback, store) = (ts.feedback.clone(), ts.store.clone());
    let reaped = tokio::task::spawn_blocking(move || {
        clax_server::daemon::reap_idle(
            &feedback,
            &store,
            std::time::Duration::from_secs(300),
            &|_| false,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reaped.ended, std::slice::from_ref(&idle));
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["session.end", "working.stop"]);
    for r in &recs {
        assert_eq!(r.actor, json!({"type": "system", "reason": "ttl"}));
        assert_eq!(r.body["for_actor"]["session_id"], idle.as_str());
    }
    assert_eq!(
        (&recs[0].body["reason"], &recs[1].body["reason"]),
        (&json!("ttl"), &json!("session_end"))
    );
}

// --- export and status (spec §8.3) ---

fn viewer_count(ts: &TestServer) -> i64 {
    db(ts)
        .query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn lan_viewer_cannot_export() {
    let ts = TestServer::spawn().await;
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let viewer = ts.viewer(Some("Ana")).await;
    let (viewers, seq) = (viewer_count(&ts), last_seq(&ts));
    let (lan, base) = ts.lan();
    for path in ["/api/toolpath/export", "/api/toolpath/status"] {
        let url = format!("{base}{path}");
        let refused = [
            lan.get(&url)
                .header("cookie", format!("clax_viewer={}", viewer.cookie)),
            lan.get(&url),
            // A viewer on this machine, a first-time viewer, and no
            // credentials at all are not the owner either.
            ts.client
                .get(&url)
                .header("cookie", format!("clax_viewer={}", viewer.cookie)),
            ts.client
                .get(&url)
                .header("cookie", "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3"),
            ts.client.get(&url),
        ];
        for (i, req) in refused.into_iter().enumerate() {
            let res = req.send().await.unwrap();
            assert_eq!(res.status(), 403, "{path} request {i}");
            let body: Value = res.json().await.unwrap();
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }
    assert_eq!((viewer_count(&ts), last_seq(&ts)), (viewers, seq));
    // The owner's cookies count only for what they were made for: the
    // owner cookie not from another machine, the events cookie not here;
    // and the extension gateway does not reach the history.
    let cred: Value = ts
        .authed(
            ts.client
                .post(format!("{}/api/extension/credentials", ts.base)),
        )
        .json(&json!({"extension_id": ts.extension_id()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cred = cred["credential"].as_str().unwrap().to_string();
    let (viewers, seq) = (viewer_count(&ts), last_seq(&ts));
    let host = ts.base.trim_start_matches("http://");
    let events = format!(
        "{}={}",
        clax_server::auth::events_cookie_name(host),
        clax_server::auth::events_cookie_value(&ts.token)
    );
    for path in ["/api/toolpath/export", "/api/toolpath/status"] {
        let refused = [
            lan.get(format!("{base}{path}"))
                .header("cookie", ts.owner_cookie()),
            ts.client
                .get(format!("{}{path}", ts.base))
                .header("cookie", events.clone()),
            ts.client
                .get(format!("{}{path}", ts.base))
                .header(
                    "origin",
                    clax_core::extension::extension_origin(&ts.extension_id()),
                )
                .header("sec-fetch-site", "cross-site")
                .header("authorization", format!("Clax-Extension {cred}")),
        ];
        for (i, req) in refused.into_iter().enumerate() {
            let res = req.send().await.unwrap();
            assert!(
                matches!(res.status().as_u16(), 403 | 404),
                "{path} owner-like request {i}: {}",
                res.status()
            );
        }
    }
    // Refused before anything is made.
    assert_eq!((viewer_count(&ts), last_seq(&ts)), (viewers, seq));
    // The token and the owner cookie may.
    let url = format!("{}/api/toolpath/export", ts.base);
    for req in [
        ts.authed(ts.client.get(&url)),
        ts.client.get(&url).header("cookie", ts.owner_cookie()),
    ] {
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["content-type"], "application/json");
        assert_eq!(res.headers()["cache-control"], "no-store");
        let doc: Value = res.json().await.unwrap();
        assert_eq!(doc["paths"][0]["meta"]["title"], "Notes");
    }
}

#[tokio::test]
async fn export_route_selects_streams_and_refuses_before_writing() {
    let ts = TestServer::spawn().await;
    // Enough history for several chunks of the stream.
    let mut ids = Vec::new();
    for i in 0..40 {
        let a = ts
            .publish(
                &format!("Page {i} {}", "x".repeat(200)),
                &[("index.html", "<h1>p</h1>")],
            )
            .await;
        ids.push(a["artifact"]["id"].as_str().unwrap().to_string());
    }
    let get = |q: String| {
        let req = ts.authed(
            ts.client
                .get(format!("{}/api/toolpath/export?{q}", ts.base)),
        );
        async move { req.send().await.unwrap() }
    };
    let all = get("pretty=true".into()).await;
    assert_eq!(all.status(), 200);
    let text = all.text().await.unwrap();
    assert!(text.len() > 3 * 64 * 1024, "{} bytes", text.len());
    let doc: Value = serde_json::from_str(&text).unwrap();
    let artifact_paths = doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["meta"]["clax"]["projection"] == "artifact")
        .count();
    assert_eq!(artifact_paths, ids.len());
    // The same database and arguments give the same bytes.
    assert_eq!(get("pretty=true".into()).await.text().await.unwrap(), text);
    // One artifact, with view refs under the browser base.
    let one: Value = get(format!("artifact={}&no_text=true", ids[3]))
        .await
        .json()
        .await
        .unwrap();
    let paths = one["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0]["path"]["id"], format!("clax-artifact-{}", ids[3]));
    assert_eq!(
        paths[0]["meta"]["refs"][0]["href"],
        format!("{}/a/{}", ts.base.replace("127.0.0.1", "localhost"), ids[3])
    );
    assert_eq!(one["meta"]["clax"]["redaction"], json!(["no-text"]));
    assert!(!one.to_string().contains("Page 3"));
    // One artifact as JSONL.
    let res = get(format!("artifact={}&format=jsonl", ids[0])).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "application/x-ndjson");
    let lines = res.text().await.unwrap();
    assert!(lines.starts_with("{\"PathOpen\""));
    assert!(lines.ends_with("{\"PathClose\":{}}\n"));
    // Refusals are statuses, before any byte.
    for (q, code) in [
        ("format=jsonl".to_string(), "jsonl_needs_one_path"),
        ("shape=tree".into(), "invalid_parameter"),
        (
            "since=2026-10-01&since=2026-10-02".into(),
            "invalid_parameter",
        ),
        ("no_text=yes".into(), "invalid_parameter"),
        ("pretty=1".into(), "invalid_parameter"),
        ("artefact=x".into(), "unknown_parameter"),
        ("artifact=zzzzzzzzzzzz".into(), "unknown_artifact"),
        ("by_session=nobody".into(), "unknown_session"),
        ("live=http://localhost:5173/x".into(), "unknown_live_page"),
        ("since=tuesday".into(), "invalid_time"),
    ] {
        let res = get(q.clone()).await;
        assert_eq!(res.status(), 400, "{q}");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["error"]["code"], code, "{q}");
    }
}

#[tokio::test]
async fn toolpath_status_reports_the_table() {
    let ts = TestServer::spawn().await;
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let res = ts.get_authed("/api/toolpath/status").await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["newest_seq"], last_seq(&ts));
    assert_eq!(v["journal"], false);
    assert!(v["dir"].as_str().unwrap().ends_with("toolpath/journal"));
    for k in ["segment", "cursor", "lag_ms", "last_error"] {
        assert_eq!(v[k], Value::Null, "{k}");
    }
}

#[tokio::test]
async fn one_export_runs_at_a_time() {
    let mut permits = None;
    let ts = TestServer::spawn_with(|st| permits = Some(st.exports.permits())).await;
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let (viewers, seq) = (viewer_count(&ts), last_seq(&ts));
    // An export holds the one permit while it runs.
    let held = permits.unwrap().try_acquire_owned().unwrap();
    let res = ts.get_authed("/api/toolpath/export").await;
    assert_eq!(res.status(), 503);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "export_busy");
    assert_eq!((viewer_count(&ts), last_seq(&ts)), (viewers, seq));
    drop(held);
    assert_eq!(ts.get_authed("/api/toolpath/export").await.status(), 200);
}

// --- the journal appender (spec §7) -----------------------------------------

use clax_core::toolpath::segment::{MemFs, SegmentConfig, SegmentWriter};
use clax_server::audit::{Appender, JournalStart, JournalStatus, start_journal};
use std::sync::Arc;

const JOURNAL: &str = "/home/toolpath/journal";

fn seg_config(ts: &TestServer) -> SegmentConfig {
    SegmentConfig::new(ts.store.install_id().unwrap(), "test", "0000000")
}

fn manual_clock() -> Arc<clax_core::working::ManualClock> {
    Arc::new(clax_core::working::ManualClock::at("2026-10-06T12:00:00Z"))
}

/// The journal every recorded event makes, written in one go.
fn journal_of(ts: &TestServer) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let fs = MemFs::new();
    let mut w = SegmentWriter::open_or_recover(
        JOURNAL,
        seg_config(ts),
        manual_clock(),
        Box::new(fs.clone()),
    )
    .unwrap();
    w.append_batch(&ts.store.events_after(0, 100_000).unwrap())
        .unwrap();
    fs.state().files.clone()
}

fn appender(
    ts: &TestServer,
    fs: &MemFs,
    clock: Arc<clax_core::working::ManualClock>,
    status: Arc<JournalStatus>,
) -> Appender {
    Appender::new(
        ts.store.clone(),
        JOURNAL,
        seg_config(ts),
        clock,
        Box::new(fs.clone()),
        status,
    )
}

#[tokio::test]
async fn journal_off_still_records_table_and_catches_up() {
    let ts = TestServer::spawn().await;
    // Off: the table is written, the journal is not.
    let off = start_journal(
        JournalStart {
            store: ts.store.clone(),
            dir: ts.home.root().join("toolpath/journal"),
            wake: clax_server::audit::AuditWake::new(),
            status: ts.journal.clone(),
            version: "test".into(),
        },
        Ok(clax_core::config::ToolpathConfig {
            journal: false,
            ..Default::default()
        }),
    );
    assert!(off.is_none());
    for i in 0..3 {
        ts.publish(&format!("Page {i}"), &[("index.html", "<h1>p</h1>")])
            .await;
    }
    let newest = last_seq(&ts);
    assert!(newest >= 3);
    assert!(!ts.home.root().join("toolpath").exists());
    let v = ts
        .get_authed("/api/toolpath/status")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        (v["journal"].clone(), v["cursor"].clone()),
        (json!(false), Value::Null)
    );
    // On again: the journal catches up with everything recorded meanwhile,
    // as one journal written from the start would hold it.
    let fs = MemFs::new();
    let mut a = appender(&ts, &fs, manual_clock(), ts.journal.clone());
    a.drain_now().unwrap();
    assert_eq!(fs.state().files, journal_of(&ts));
    let v = ts
        .get_authed("/api/toolpath/status")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(v["journal"], true);
    assert_eq!(v["cursor"], newest);
    assert_eq!(v["lag_ms"], 0);
    assert_eq!(v["last_error"], Value::Null);
    // The segment is named by the first event's day.
    let names: Vec<String> = fs
        .paths()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1);
    assert_eq!(v["segment"].as_str().unwrap(), names[0]);
    assert_eq!(v["warning"], Value::Null);
}

#[tokio::test]
async fn eio_backs_off_and_retries_same_batch() {
    let ts = TestServer::spawn().await;
    for i in 0..3 {
        ts.publish(&format!("Page {i}"), &[("index.html", "<h1>p</h1>")])
            .await;
    }
    let fs = MemFs::new();
    let clock = manual_clock();
    let mut a = appender(&ts, &fs, clock.clone(), ts.journal.clone());
    // A disk that fails mid-write: the events stay recorded, the journal
    // falls behind, and status says why.
    fs.fail(&["append"], 100, 5);
    fs.state().partial = Some(100);
    let e = a.drain_now().unwrap_err();
    assert!(e.contains("os error 5"), "{e}");
    let v = ts
        .get_authed("/api/toolpath/status")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert!(
        v["last_error"].as_str().unwrap().contains("os error 5"),
        "{v}"
    );
    assert_eq!(v["cursor"], 0);
    assert!(v["lag_ms"].as_i64().is_some());
    // Retries back off from 1 s, doubling to 60 s, and write nothing while
    // they wait.
    let attempts = |fs: &MemFs| fs.state().appends;
    let mut waits = Vec::new();
    for _ in 0..8 {
        let before = attempts(&fs);
        let mut waited = 0;
        loop {
            clock.advance(1);
            waited += 1;
            let _ = a.drain_now();
            if attempts(&fs) > before {
                break;
            }
        }
        waits.push(waited);
    }
    assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
    // Once the disk writes again, the same events are written once, byte
    // for byte as an unbroken journal holds them.
    fs.state().fail_count = 0;
    clock.advance(60);
    ts.publish("After", &[("index.html", "<h1>a</h1>")]).await;
    a.drain_now().unwrap();
    assert_eq!(fs.state().files, journal_of(&ts));
    let v = ts
        .get_authed("/api/toolpath/status")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(v["last_error"], Value::Null);
    assert_eq!(v["cursor"], last_seq(&ts));
    assert_eq!(v["lag_ms"], 0);
    // A full disk, a refused directory: the same.
    for (op, errno) in [("append", 28), ("create_dir_all", 13)] {
        let fs = MemFs::new();
        let mut a = appender(&ts, &fs, clock.clone(), JournalStatus::off(None));
        fs.fail(&[op], 1, errno);
        assert!(
            a.drain_now()
                .unwrap_err()
                .contains(&format!("os error {errno}"))
        );
        clock.advance(1);
        a.drain_now().unwrap();
        assert_eq!(fs.state().files, journal_of(&ts));
    }
}

#[tokio::test]
async fn the_journal_thread_follows_the_config_and_closes_at_stop() {
    let ts = TestServer::spawn().await;
    let dir = ts.home.root().join("toolpath/journal");
    let start = |wake: Arc<clax_server::audit::AuditWake>| JournalStart {
        store: ts.store.clone(),
        dir: dir.clone(),
        wake,
        status: ts.journal.clone(),
        version: "test".into(),
    };
    // An invalid [toolpath] leaves the journal off and says why.
    let bad = start_journal(
        start(clax_server::audit::AuditWake::new()),
        Err(clax_core::CoreError::invalid(
            "bad_config",
            "[toolpath] other: unknown field",
        )),
    );
    assert!(bad.is_none());
    let v = ts
        .get_authed("/api/toolpath/status")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(v["journal"], false);
    assert!(
        v["last_error"].as_str().unwrap().contains("[toolpath]"),
        "{v}"
    );
    // On: the thread recovers and writes; stopping it catches up, syncs and
    // closes the segment.
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let wake = clax_server::audit::AuditWake::new();
    let j = start_journal(
        start(wake.clone()),
        Ok(clax_core::config::ToolpathConfig::default()),
    )
    .expect("the journal starts");
    ts.publish("More", &[("index.html", "<h1>m</h1>")]).await;
    j.stop();
    let state = ts.journal.get();
    assert_eq!(state.cursor, Some(last_seq(&ts)));
    // Found by name wherever its day put it.
    let name = state.segment.unwrap();
    let file = walk(&dir)
        .into_iter()
        .find(|p| p.file_name().unwrap().to_string_lossy() == name)
        .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("{\"PathOpen\":"));
    assert!(text.ends_with(&format!(
        "{{\"Head\":{{\"step_id\":\"e{:012}\"}}}}\n{{\"PathClose\":{{}}}}\n",
        last_seq(&ts)
    )));
    assert_eq!(
        text.lines().filter(|l| l.starts_with("{\"Step\":")).count() as i64,
        ts.store.events_after(0, 100_000).unwrap().len() as i64
    );
}

/// Every file under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn drains_do_not_sync_and_a_quiet_second_does() {
    let ts = TestServer::spawn().await;
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let fs = MemFs::new();
    let clock = manual_clock();
    let mut a = appender(&ts, &fs, clock.clone(), JournalStatus::off(None));
    let syncs = |fs: &MemFs| fs.state().syncs.values().sum::<u32>();
    // Nudge-driven drains write without syncing, however many there are...
    for i in 0..5 {
        ts.publish(&format!("P{i}"), &[("index.html", "<h1>p</h1>")])
            .await;
        a.drain_now().unwrap();
    }
    assert_eq!(syncs(&fs), 0);
    // ...until bytes have waited 5 s,
    clock.advance(5);
    ts.publish("Late", &[("index.html", "<h1>l</h1>")]).await;
    a.drain_now().unwrap();
    assert_eq!(syncs(&fs), 1);
    // and a quiet second syncs at once.
    ts.publish("Quiet", &[("index.html", "<h1>q</h1>")]).await;
    a.drain_now().unwrap();
    assert_eq!(syncs(&fs), 1);
    a.quiet_tick().unwrap();
    assert_eq!(syncs(&fs), 2);
    a.quiet_tick().unwrap();
    assert_eq!(syncs(&fs), 2, "nothing unsynced");
}

#[tokio::test]
async fn a_panicking_appender_says_the_journal_stopped() {
    let ts = TestServer::spawn().await;
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let fs = MemFs::new();
    fs.state().panic_on = Some("append");
    let status = JournalStatus::off(None);
    let a = appender(&ts, &fs, manual_clock(), status.clone());
    assert!(status.get().journal);
    let (_tx, rx) = std::sync::mpsc::sync_channel(1);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let t = std::thread::spawn(move || a.run(rx, stop));
    assert!(t.join().is_err(), "it panicked");
    let s = status.get();
    assert!(!s.journal);
    assert_eq!(
        s.last_error.as_deref(),
        Some("the journal appender stopped unexpectedly")
    );
}

/// A file system whose appends wait until the test opens the gate.
struct GatedFs {
    inner: MemFs,
    gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl clax_core::toolpath::segment::JournalFs for GatedFs {
    fn create_dir_all(&mut self, d: &std::path::Path) -> std::io::Result<()> {
        self.inner.create_dir_all(d)
    }
    fn list(&mut self, d: &std::path::Path) -> std::io::Result<Vec<String>> {
        self.inner.list(d)
    }
    fn len(&mut self, p: &std::path::Path) -> std::io::Result<Option<u64>> {
        self.inner.len(p)
    }
    fn read_at(&mut self, p: &std::path::Path, o: u64, b: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read_at(p, o, b)
    }
    fn create(&mut self, p: &std::path::Path) -> std::io::Result<()> {
        self.inner.create(p)
    }
    fn append(&mut self, p: &std::path::Path, d: &[u8]) -> std::io::Result<()> {
        let (open, cv) = &*self.gate;
        let mut g = open.lock().unwrap();
        while !*g {
            g = cv.wait(g).unwrap();
        }
        drop(g);
        self.inner.append(p, d)
    }
    fn sync_data(&mut self, p: &std::path::Path) -> std::io::Result<()> {
        self.inner.sync_data(p)
    }
    fn set_len(&mut self, p: &std::path::Path, l: u64) -> std::io::Result<()> {
        self.inner.set_len(p, l)
    }
    fn rename(&mut self, f: &std::path::Path, t: &std::path::Path) -> std::io::Result<()> {
        self.inner.rename(f, t)
    }
    fn remove(&mut self, p: &std::path::Path) -> std::io::Result<()> {
        self.inner.remove(p)
    }
}

#[tokio::test]
async fn recording_never_waits_on_a_wedged_journal() {
    let mut wake = None;
    let ts = TestServer::spawn_with(|st| wake = Some(st.audit_wake.clone())).await;
    let wake = wake.unwrap();
    ts.publish("First", &[("index.html", "<h1>f</h1>")]).await;
    let fs = MemFs::new();
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let a = Appender::new(
        ts.store.clone(),
        JOURNAL,
        seg_config(&ts),
        manual_clock(),
        Box::new(GatedFs {
            inner: fs.clone(),
            gate: gate.clone(),
        }),
        JournalStatus::off(None),
    );
    let rx = wake.take_receiver().unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = stop.clone();
    let t = std::thread::spawn(move || a.run(rx, thread_stop));
    // The appender is stuck in its first write; every publish still
    // returns, each nudging a full channel.
    let started = std::time::Instant::now();
    for i in 0..20 {
        ts.publish(&format!("Page {i}"), &[("index.html", "<h1>p</h1>")])
            .await;
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(
        fs.state().files.values().all(|f| f.is_empty()),
        "nothing written yet"
    );
    // Released, it catches up with all of them.
    {
        let (open, cv) = &*gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    wake.nudge();
    t.join().unwrap();
    let mut want = journal_of(&ts);
    // The stop closed the segment the reference leaves open.
    let path = want.keys().next().unwrap().clone();
    want.get_mut(&path).unwrap().extend_from_slice(
        format!(
            "{{\"Head\":{{\"step_id\":\"e{:012}\"}}}}\n{{\"PathClose\":{{}}}}\n",
            last_seq(&ts)
        )
        .as_bytes(),
    );
    let got = fs.state().files.clone();
    assert_eq!(got.get(&path), want.get(&path));
}

#[test]
fn a_wedged_appender_does_not_hold_the_daemons_exit() {
    // The daemon's own runtime, dropped at the end as `clax serve` drops
    // it: a blocking-pool thread would hold the drop until the appender
    // returned, and a wedged one never does.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut wake = None;
    let ts = rt.block_on(TestServer::spawn_with(|st| {
        wake = Some(st.audit_wake.clone())
    }));
    let wake = wake.unwrap();
    rt.block_on(ts.publish("Notes", &[("index.html", "<h1>n</h1>")]));
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let a = Appender::new(
        ts.store.clone(),
        JOURNAL,
        seg_config(&ts),
        manual_clock(),
        Box::new(GatedFs {
            inner: MemFs::new(),
            gate: gate.clone(),
        }),
        JournalStatus::off(None),
    );
    let rx = wake.take_receiver().unwrap();
    let h = clax_server::audit::spawn_appender(a, rx, wake.clone()).unwrap();
    // Its first write never returns; the stop gives up at its limit.
    let started = std::time::Instant::now();
    let stopped = rt.block_on(h.stop_within(std::time::Duration::from_millis(300)));
    assert!(!stopped, "a wedged appender cannot close its segment");
    drop(ts);
    drop(rt);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the stop and the runtime's drop took {:?}",
        started.elapsed()
    );
    // Released, the left-behind thread finishes on its own.
    let (open, cv) = &*gate;
    *open.lock().unwrap() = true;
    cv.notify_all();
}

#[tokio::test]
async fn a_responsive_appender_stops_within_its_limit() {
    let mut wake = None;
    let ts = TestServer::spawn_with(|st| wake = Some(st.audit_wake.clone())).await;
    let wake = wake.unwrap();
    ts.publish("Notes", &[("index.html", "<h1>n</h1>")]).await;
    let fs = MemFs::new();
    let a = appender(&ts, &fs, manual_clock(), JournalStatus::off(None));
    let rx = wake.take_receiver().unwrap();
    let h = clax_server::audit::spawn_appender(a, rx, wake.clone()).unwrap();
    assert!(h.stop_within(std::time::Duration::from_secs(10)).await);
    let text = String::from_utf8(fs.state().files.values().next().unwrap().clone()).unwrap();
    assert!(text.ends_with("{\"PathClose\":{}}\n"));
}

/// The report of [`call`] once it ended.
fn call_report() -> Value {
    let c = call();
    json!({
        "call_id": c.call_id,
        "tool": c.tool,
        "harness_tool": c.harness_tool,
        "args_sha256": c.args_sha256,
        "started_at": c.started_at,
        "ended_at": "2026-10-06T14:03:11.913Z",
        "outcome": "ok",
    })
}

#[tokio::test]
async fn tool_calls_route_records_the_call_with_what_it_produced() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-calls").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let b = ts.base.clone();
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{b}/api/artifacts")))
            .header("x-clax-via", "mcp")
            .header("x-clax-session", &sid)
            .header("x-clax-call", encode_call_header(&call()).unwrap())
            .json(&json!({"title": "Called", "files": {"index.html": utf8("<p>x")}})),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let made: Vec<i64> = {
        let c = db(&ts);
        let mut q = c
            .prepare("SELECT seq FROM audit_events WHERE seq > ?1 ORDER BY seq")
            .unwrap();
        q.query_map([seq], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(!made.is_empty());
    let url = format!("{b}/api/sessions/{sid}/tool-calls");
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(&url))
            .header("x-clax-via", "mcp")
            .json(&call_report()),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    assert_eq!(v["recorded"], true);
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["tool.call"]);
    let r = &recs[0];
    assert_eq!(r.actor["type"], "agent");
    assert_eq!(r.actor["session_id"], sid.as_str());
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(r.call_id.as_deref(), Some(call().call_id.as_str()));
    assert_eq!(r.artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(r.body["via"], "mcp");
    assert_eq!(r.body["produced"], json!(made));
    assert_eq!(r.body["outcome"], "ok");
    assert_eq!(r.body["harness_tool"], "mcp__clax__publish");
    assert!(r.body.get("call").is_none(), "{}", r.body);

    // Reported again (a retried POST): recorded once.
    let seq = last_seq(&ts);
    let (code, v) = status(ts.authed(ts.client.post(&url)).json(&call_report())).await;
    assert_eq!((code, &v["recorded"]), (200, &json!(false)), "{v}");
    assert!(events_since(&ts, seq).is_empty());
}

#[tokio::test]
async fn tool_calls_route_refuses_bad_reports_and_strangers() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "hs-bad").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let url = format!("{}/api/sessions/{sid}/tool-calls", ts.base);
    let seq = last_seq(&ts);
    for (field, bad) in [
        ("call_id", json!("nope")),
        ("tool", json!("mcp__clax__publish")),
        ("args_sha256", json!("sha256:xyz")),
        ("ended_at", json!("later")),
        ("ended_at", json!("2026-10-06T14:03:11.401Z")),
        ("outcome", json!("maybe")),
        ("artifact_id", json!("a b")),
    ] {
        let mut body = call_report();
        body[field] = bad;
        let (code, v) = status(ts.authed(ts.client.post(&url)).json(&body)).await;
        assert_eq!(code, 400, "{field}: {v}");
    }
    let (code, _) = status(
        ts.authed(
            ts.client
                .post(format!("{}/api/sessions/nope/tool-calls", ts.base)),
        )
        .json(&call_report()),
    )
    .await;
    assert_eq!(code, 400);
    // A session that never existed records nothing.
    let (code, v) = status(
        ts.authed(ts.client.post(format!(
            "{}/api/sessions/01JBC0000000000000000000ZZ/tool-calls",
            ts.base
        )))
        .json(&call_report()),
    )
    .await;
    assert_eq!(
        (code, &v["error"]["code"]),
        (404, &json!("unknown_session")),
        "{v}"
    );
    // Without the token: refused, whatever the cookie.
    let (code, _) = status(
        ts.client
            .post(&url)
            .header("cookie", ts.owner_cookie())
            .json(&call_report()),
    )
    .await;
    assert_eq!(code, 401);
    let (code, _) = status(
        ts.client
            .post(format!("{}/api/tool-calls", ts.base))
            .json(&call_report()),
    )
    .await;
    assert_eq!(code, 401);
    assert!(events_since(&ts, seq).is_empty());
}

#[tokio::test]
async fn sessionless_tool_call_is_the_mcp_client() {
    let ts = TestServer::spawn().await;
    let seq = last_seq(&ts);
    let (code, v) = status(
        ts.authed(ts.client.post(format!("{}/api/tool-calls", ts.base)))
            .header("x-clax-via", "mcp")
            .json(&call_report()),
    )
    .await;
    assert_eq!(code, 201, "{v}");
    let recs = events_since(&ts, seq);
    assert_eq!(kinds(&recs), ["tool.call"]);
    assert_eq!(recs[0].actor, json!({"type": "agent", "session_id": null}));
    assert_eq!(recs[0].session_id, None);
    assert_eq!(recs[0].artifact_id, None);
    assert_eq!(recs[0].body["produced"], json!([]));
}
