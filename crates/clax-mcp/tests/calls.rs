//! Tool-call records (spec 2026-10-06-toolpath-audit-design §6.7, §9.3):
//! each MCP call's ID, argument hash and git state on the requests it
//! makes, and its background report, through a real MCP connection.

use clax_core::model::Session;
use clax_mcp::git::GitCapture;
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::TestServer;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `tools` served over an in-process MCP connection, and a client of it.
async fn connect(tools: ClaxTools) -> RunningService<RoleClient, ()> {
    let (client, server) = tokio::io::duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server);
    tokio::spawn(async move {
        if let Ok(running) = tools.serve((sr, sw)).await {
            let _ = running.waiting().await;
        }
    });
    let (cr, cw) = tokio::io::split(client);
    ().serve((cr, cw)).await.expect("MCP handshake")
}

async fn call(c: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(name.to_string());
    p.arguments = args.as_object().cloned();
    tokio::time::timeout(Duration::from_secs(30), c.call_tool(p))
        .await
        .expect("the tool answers")
        .expect("a tool result")
}

fn result_json(r: &CallToolResult) -> Value {
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

fn git_bin() -> PathBuf {
    clax_core::gitctx::find_git(std::env::var_os("PATH").as_deref()).expect("git on PATH")
}

/// A one-commit git repository, canonical path.
fn repo(dir: &Path) -> PathBuf {
    let root = dir.join("app");
    std::fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new(git_bin())
            .arg("-C")
            .arg(&root)
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-q", "-m", "one"]);
    root.canonicalize().unwrap()
}

/// Registers a Claude Code session working in `cwd`.
async fn session(ts: &TestServer, cwd: &Path) -> Session {
    let res = ts
        .post_json(
            "/api/sessions",
            json!({"harness": "claude", "harness_session_id": "hs-calls", "cwd": cwd}),
        )
        .await;
    assert_eq!(res.status(), 201);
    serde_json::from_value(res.json::<Value>().await.unwrap()["session"].clone()).unwrap()
}

/// The tools of a shim registered as `s`, capturing git with the real git.
fn shim_tools(ts: &TestServer, s: &Session) -> ClaxTools {
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone()))
            .with_via(clax_mcp::client::VIA),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s.clone()),
        ts.home.log_path(),
    )
    .with_git(GitCapture::with(Some(git_bin()), Duration::from_secs(20)))
}

#[derive(Debug)]
struct Rec {
    seq: i64,
    kind: String,
    actor: Value,
    artifact_id: Option<String>,
    call_id: Option<String>,
    body: Value,
}

fn events(ts: &TestServer) -> Vec<Rec> {
    let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let mut q = c
        .prepare(
            "SELECT seq, kind, actor, artifact_id, call_id, body FROM audit_events ORDER BY seq",
        )
        .unwrap();
    q.query_map([], |r| {
        Ok(Rec {
            seq: r.get(0)?,
            kind: r.get(1)?,
            actor: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
            artifact_id: r.get(3)?,
            call_id: r.get(4)?,
            body: serde_json::from_str(&r.get::<_, String>(5)?).unwrap(),
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn tool_calls(ts: &TestServer) -> Vec<Rec> {
    events(ts)
        .into_iter()
        .filter(|r| r.kind == "tool.call")
        .collect()
}

#[tokio::test]
async fn read_only_tool_records_tool_call() {
    let ts = TestServer::spawn().await;
    let d = tempfile::tempdir().unwrap();
    let s = session(&ts, &repo(d.path())).await;
    let tools = shim_tools(&ts, &s);
    let c = connect(tools.clone()).await;
    let r = call(&c, "list", json!({"limit": 5})).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    tools.settle_reports().await;
    let calls = tool_calls(&ts);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let t = &calls[0];
    assert_eq!(t.body["tool"], "list");
    assert_eq!(t.body["outcome"], "ok");
    assert_eq!(t.body["produced"], json!([]));
    assert_eq!(
        t.body["args_sha256"],
        clax_core::toolpath::args::args_sha256(&json!({"limit": 5}))
    );
    assert_eq!(t.actor["session_id"], s.id.as_str());
    assert_eq!(t.body["via"], "mcp");
    assert_eq!(t.call_id.as_deref(), t.body["call_id"].as_str());
    // A read-only call captures no git state.
    assert!(t.body.get("git").is_none() && t.body.get("git_capture").is_none());
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn publish_tool_call_lists_produced_seq() {
    let ts = TestServer::spawn().await;
    let d = tempfile::tempdir().unwrap();
    let root = repo(d.path());
    std::fs::write(root.join("a.txt"), "edited\n").unwrap();
    let s = session(&ts, &root).await;
    let tools = shim_tools(&ts, &s);
    let c = connect(tools.clone()).await;
    let before = events(&ts).last().map_or(0, |r| r.seq);
    let r = call(
        &c,
        "publish",
        json!({"html": "<title>Called</title><p>x", "note": "first"}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let aid = result_json(&r)["artifact_id"].as_str().unwrap().to_string();
    tools.settle_reports().await;
    let all = events(&ts);
    let calls: Vec<&Rec> = all.iter().filter(|r| r.kind == "tool.call").collect();
    assert_eq!(calls.len(), 1);
    let call_id = calls[0].call_id.clone().unwrap();
    let made: Vec<&Rec> = all
        .iter()
        .filter(|r| r.seq > before && r.kind != "tool.call")
        .collect();
    assert!(made.iter().any(|r| r.kind == "version.publish"), "{made:?}");
    for r in &made {
        // Every event of the call carries the call and the git state.
        assert_eq!(r.call_id.as_deref(), Some(call_id.as_str()), "{r:?}");
        assert_eq!(r.body["call"]["call_id"], call_id.as_str());
        assert_eq!(r.body["call"]["tool"], "publish");
        assert_eq!(r.body["git_capture"], "ok", "{r:?}");
        assert_eq!(r.body["git"]["repo_root"], root.to_str().unwrap());
        assert_eq!(r.body["git"]["dirty"], true);
        assert_eq!(r.body["via"], "mcp");
        assert_eq!(r.actor["session_id"], s.id.as_str());
    }
    let seqs: Vec<i64> = made.iter().map(|r| r.seq).collect();
    assert_eq!(calls[0].body["produced"], json!(seqs));
    assert_eq!(calls[0].artifact_id.as_deref(), Some(aid.as_str()));
    assert_eq!(calls[0].body["outcome"], "ok");
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn an_error_result_is_recorded_as_an_error_on_the_named_artifact() {
    let ts = TestServer::spawn().await;
    let d = tempfile::tempdir().unwrap();
    let s = session(&ts, &repo(d.path())).await;
    let a = ts.publish("Doc", &[("index.html", "<p>x")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let tools = shim_tools(&ts, &s);
    let c = connect(tools.clone()).await;
    let r = call(
        &c,
        "read",
        json!({"url_or_id": aid, "path": "missing.html"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    tools.settle_reports().await;
    let calls = tool_calls(&ts);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].body["outcome"], "error");
    assert_eq!(calls[0].artifact_id.as_deref(), Some(aid.as_str()));
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn shim_and_daemon_mcp_send_via_mcp() {
    let ts = TestServer::spawn().await;
    // The shim: the session's agent, on mcp.
    let d = tempfile::tempdir().unwrap();
    let s = session(&ts, &repo(d.path())).await;
    let c = connect(shim_tools(&ts, &s)).await;
    let r = call(&c, "publish", json!({"html": "<title>Shim</title>"})).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let create = events(&ts)
        .into_iter()
        .rfind(|r| r.kind == "artifact.create")
        .unwrap();
    assert_eq!(create.actor["type"], "agent");
    assert_eq!(create.actor["session_id"], s.id.as_str());
    assert_eq!(create.body["via"], "mcp");
    c.cancel().await.unwrap();

    // The daemon's own /mcp client: no session, so the sessionless agent,
    // on mcp, with no working directory to capture.
    let daemon = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None).with_via(clax_mcp::client::VIA),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    );
    let c = connect(daemon.clone()).await;
    let r = call(&c, "publish", json!({"html": "<title>Daemon</title>"})).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    daemon.settle_reports().await;
    let all = events(&ts);
    let create = all.iter().rfind(|r| r.kind == "artifact.create").unwrap();
    assert_eq!(create.actor, json!({"type": "agent", "session_id": null}));
    assert_eq!(create.body["via"], "mcp");
    assert_eq!(create.body["git_capture"], "no-cwd");
    let t = all.iter().rfind(|r| r.kind == "tool.call").unwrap();
    assert_eq!(t.actor, json!({"type": "agent", "session_id": null}));
    assert_eq!(t.body["tool"], "publish");
    assert!(t.body["produced"].as_array().unwrap().len() >= 2);
    c.cancel().await.unwrap();
}

/// Each request a fake daemon saw: `"<method> <path>"` and its `x-clax-*`
/// headers.
type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

/// A daemon that answers `list` and `pin`, keeps each request's
/// `x-clax-*` headers, and holds every tool-call report until released.
struct FakeDaemon {
    base: String,
    seen: Seen,
    reports: tokio::sync::mpsc::UnboundedReceiver<Value>,
    release: Arc<tokio::sync::Semaphore>,
}

async fn fake_daemon() -> FakeDaemon {
    use axum::routing::{get, patch, post};
    let seen: Seen = Arc::default();
    let (tx, reports) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let keep = {
        let seen = seen.clone();
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let seen = seen.clone();
            async move {
                let headers = req
                    .headers()
                    .iter()
                    .filter(|(k, _)| k.as_str().starts_with("x-clax-"))
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
                    .collect();
                seen.lock()
                    .unwrap()
                    .push((format!("{} {}", req.method(), req.uri().path()), headers));
                next.run(req).await
            }
        }
    };
    let gate = release.clone();
    let app = axum::Router::new()
        .route(
            "/api/artifacts",
            get(|| async { axum::Json(json!({"artifacts": []})) }),
        )
        .route(
            "/api/artifacts/{id}",
            patch(|| async { axum::Json(json!({"artifact": {"pinned": true}})) }),
        )
        .route(
            "/api/tool-calls",
            post(move |axum::Json(v): axum::Json<Value>| {
                let tx = tx.clone();
                let gate = gate.clone();
                async move {
                    let _ = tx.send(v);
                    let _permit = gate.acquire().await;
                    (
                        axum::http::StatusCode::CREATED,
                        axum::Json(json!({"recorded": true, "seq": 1})),
                    )
                }
            }),
        )
        .layer(axum::middleware::from_fn(keep));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    FakeDaemon {
        base,
        seen,
        reports,
        release,
    }
}

fn fake_tools(f: &FakeDaemon) -> ClaxTools {
    ClaxTools::new(
        DaemonClient::new(f.base.clone(), "tok".into(), None).with_via(clax_mcp::client::VIA),
        "http://localhost:1".into(),
        None,
        PathBuf::from("/nonexistent/daemon.log"),
    )
}

#[tokio::test]
async fn tool_call_post_does_not_delay_result() {
    let mut f = fake_daemon().await;
    let tools = fake_tools(&f);
    let c = connect(tools.clone()).await;
    // The report is held at the daemon, yet the result comes back.
    let r = call(&c, "list", json!({})).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let report = tokio::time::timeout(Duration::from_secs(30), f.reports.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report["tool"], "list");
    assert_eq!(report["outcome"], "ok");
    assert_eq!(
        report["args_sha256"],
        "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
    // A second call answers too while the first report is still held.
    let r = call(&c, "list", json!({"limit": 1})).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    f.release.add_permits(2);
    tokio::time::timeout(Duration::from_secs(30), tools.settle_reports())
        .await
        .unwrap();
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn read_only_tools_send_no_git_header() {
    let f = fake_daemon().await;
    f.release.add_permits(100);
    let c = connect(fake_tools(&f)).await;
    call(&c, "list", json!({})).await;
    call(&c, "pin", json!({"url_or_id": "k3m9q2w8x1ab"})).await;
    let seen = f.seen.lock().unwrap().clone();
    let req = |what: &str| {
        seen.iter()
            .find(|(r, _)| r == what)
            .unwrap_or_else(|| panic!("no {what} in {seen:?}"))
            .1
            .clone()
    };
    let header = |h: &[(String, String)], name: &str| {
        h.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    };
    let list = req("GET /api/artifacts");
    assert_eq!(header(&list, "x-clax-via").as_deref(), Some("mcp"));
    let list_call =
        clax_core::audit::decode_call_header(&header(&list, "x-clax-call").unwrap()).unwrap();
    assert_eq!(list_call.tool, "list");
    assert_eq!(header(&list, "x-clax-git"), None);
    let pin = req("PATCH /api/artifacts/k3m9q2w8x1ab");
    let pin_call =
        clax_core::audit::decode_call_header(&header(&pin, "x-clax-call").unwrap()).unwrap();
    assert_eq!(pin_call.tool, "pin");
    assert_ne!(pin_call.call_id, list_call.call_id);
    assert_eq!(
        clax_core::gitctx::decode_header(&header(&pin, "x-clax-git").unwrap()),
        clax_core::gitctx::GitField::Capture("no-cwd")
    );
    // Reports carry no call header of their own.
    let reports: Vec<_> = seen
        .iter()
        .filter(|(r, _)| r == "POST /api/tool-calls")
        .collect();
    for (_, h) in reports {
        assert_eq!(header(h, "x-clax-call"), None);
        assert_eq!(header(h, "x-clax-via").as_deref(), Some("mcp"));
    }
    c.cancel().await.unwrap();
}
