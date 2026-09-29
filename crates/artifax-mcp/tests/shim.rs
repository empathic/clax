//! End-to-end tests of `artifax mcp`: the built binary is spawned as a stdio MCP
//! server, starts its own daemon in a temporary `ARTIFAX_HOME`, and is driven
//! with rmcp's client.

use artifax_core::Home;
use artifax_server::daemon::read_daemon_info;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The `artifax` binary, built once per test run. It belongs to `artifax-cli`,
/// so Cargo does not provide `CARGO_BIN_EXE_artifax` here; the tests build it
/// into the same target directory as this test executable.
fn artifax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = std::process::Command::new(env!("CARGO"));
        cmd.current_dir(&workspace).args([
            "build",
            "--quiet",
            "-p",
            "artifax-cli",
            "--bin",
            "artifax",
        ]);
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("run cargo build");
        assert!(status.success(), "cargo build -p artifax-cli failed");
        // Assumes Cargo's layout: the test executable lives in
        // <target>/<profile>/deps/, and `cargo build` (which honours
        // CARGO_TARGET_DIR like the enclosing `cargo test`) puts the binary in
        // <target>/<profile>/.
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("artifax");
        assert!(bin.exists(), "{} missing", bin.display());
        bin
    })
    .clone()
}

/// A running shim with its own `ARTIFAX_HOME`; dropping it stops the daemon the
/// shim started.
struct Shim {
    dir: tempfile::TempDir,
    client: Option<RunningService<RoleClient, ()>>,
}

impl Shim {
    async fn start(heartbeat_ms: Option<u64>) -> Shim {
        Shim::start_in(tempfile::tempdir().unwrap(), heartbeat_ms).await
    }

    /// Starts a shim whose `ARTIFAX_HOME` is `<dir>/ax`.
    async fn start_in(dir: tempfile::TempDir, heartbeat_ms: Option<u64>) -> Shim {
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let home = dir.path().join("ax");
        let work = dir.path().join("work");
        let cmd = tokio::process::Command::new(artifax_bin()).configure(|c| {
            c.args(["--port", "0", "mcp", "--agent", "claude"])
                .env("ARTIFAX_HOME", &home)
                .env("HOME", dir.path())
                .env("CLAUDE_CODE_SESSION_ID", "test-sess")
                .env("ARTIFAX_NO_OPEN", "1")
                .env("RUST_LOG", "error")
                .env_remove("ARTIFAX_SESSION_ID")
                .env_remove("CLAUDE_PROJECT_DIR")
                .current_dir(&work);
            if let Some(ms) = heartbeat_ms {
                c.args(["--heartbeat-interval-ms", &ms.to_string()]);
            }
        });
        let transport = TokioChildProcess::new(cmd).expect("spawn artifax mcp");
        let client = ().serve(transport).await.expect("MCP handshake");
        Shim {
            dir,
            client: Some(client),
        }
    }

    fn home(&self) -> Home {
        Home::at(self.dir.path().join("ax"))
    }

    fn client(&self) -> &RunningService<RoleClient, ()> {
        self.client.as_ref().unwrap()
    }

    /// `http://<host>:<port>` of the daemon the shim started.
    fn daemon_base(&self) -> String {
        let info = read_daemon_info(&self.home()).expect("daemon.json");
        format!(
            "http://{}:{}",
            artifax_server::daemon::probe_host(&info.bind),
            info.port
        )
    }

    /// GETs `path` from the daemon with its token.
    async fn get(&self, path: &str) -> Value {
        let token = read_daemon_info(&self.home()).expect("daemon.json").token;
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{}{path}", self.daemon_base()))
            .bearer_auth(token)
            .send()
            .await
            .expect("daemon answers")
            .json()
            .await
            .expect("JSON body")
    }

    async fn live_sessions(&self) -> Vec<Value> {
        self.get("/api/sessions?live=true").await["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    async fn call(&self, name: &'static str, args: Value) -> CallToolResult {
        let Value::Object(args) = args else {
            panic!("tool arguments are an object")
        };
        self.client()
            .call_tool(CallToolRequestParams::new(name).with_arguments(args))
            .await
            .expect("tool call")
    }

    /// Runs `artifax stop` against this shim's home and checks the daemon
    /// process is gone (the shim, its parent, must have reaped it).
    fn stop_daemon(&self) {
        let out = std::process::Command::new(artifax_bin())
            .args(["--json", "stop"])
            .env("ARTIFAX_HOME", self.dir.path().join("ax"))
            .env("HOME", self.dir.path())
            .output()
            .expect("run artifax stop");
        assert!(out.status.success(), "{out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).expect("stop --json output");
        assert_eq!(v["stopped"], true, "{v}");
    }

    /// Closes the shim's stdin, waits for it to exit, then stops the daemon.
    async fn finish(mut self) {
        if let Some(client) = self.client.take() {
            client.cancel().await.unwrap();
        }
        self.stop_daemon();
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = std::process::Command::new(artifax_bin())
            .arg("stop")
            .env("ARTIFAX_HOME", self.dir.path().join("ax"))
            .env("HOME", self.dir.path())
            .output();
    }
}

/// The JSON object a tool result carries, and whether it is an error result.
fn body(r: &CallToolResult) -> (Value, bool) {
    assert_eq!(r.content.len(), 1, "{r:?}");
    let text = &r.content[0].as_text().expect("text content").text;
    let v: Value = serde_json::from_str(text).expect("result text is JSON");
    (v, r.is_error == Some(true))
}

fn ok(r: &CallToolResult) -> Value {
    let (v, is_error) = body(r);
    assert!(!is_error, "{v}");
    v
}

#[tokio::test]
async fn serves_the_tools_and_registers_the_harness_session() {
    let shim = Shim::start(None).await;

    let mut names: Vec<String> = shim
        .client()
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "asset_upload",
            "delete",
            "list",
            "open",
            "pin",
            "publish",
            "read",
            "status",
            "unpin"
        ]
    );

    let published = ok(&shim
        .call("publish", json!({"html": "<h1>hi</h1>", "title": "Hello"}))
        .await);
    assert_eq!(published["version"], 1);
    let page = ok(&shim
        .call("read", json!({"url_or_id": published["url"]}))
        .await);
    assert_eq!(page["content"], "<h1>hi</h1>");

    let sessions = shim.live_sessions().await;
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let s = &sessions[0];
    assert_eq!(s["harness"], "claude");
    assert_eq!(s["harness_session_id"], "test-sess");
    assert_eq!(s["parent_pid"], std::process::id());
    let work = shim.dir.path().join("work").canonicalize().unwrap();
    assert_eq!(
        PathBuf::from(s["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        work
    );

    // The publish is attributed to the shim's session.
    let got = shim
        .get(&format!(
            "/api/artifacts/{}",
            published["artifact_id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(got["artifact"]["owner_session_id"], s["id"], "{got}");
    let status = ok(&shim.call("status", json!({})).await);
    assert_eq!(status["session"]["id"], s["id"]);
    shim.finish().await;
}

#[tokio::test]
async fn heartbeats_advance_last_seen_at() {
    let shim = Shim::start(Some(200)).await;
    let before = shim.live_sessions().await[0]["last_seen_at"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after = shim.live_sessions().await[0]["last_seen_at"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        after > before,
        "last_seen_at {before} did not advance ({after})"
    );
    shim.finish().await;
}

#[tokio::test]
async fn closing_stdin_ends_the_session() {
    let mut shim = Shim::start(None).await;
    let id = shim.live_sessions().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Closes the shim's stdin and waits for it to exit.
    shim.client.take().unwrap().cancel().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = shim.get(&format!("/api/sessions/{id}")).await;
        if !s["session"]["ended_at"].is_null() {
            break;
        }
        assert!(Instant::now() < deadline, "session not ended: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(shim.live_sessions().await.is_empty());
    shim.finish().await;
}

#[tokio::test]
async fn a_restarted_daemon_is_found_again_with_the_same_session() {
    let shim = Shim::start(None).await;
    let before = ok(&shim.call("status", json!({})).await);
    let old_pid = read_daemon_info(&shim.home()).unwrap().pid;
    shim.stop_daemon();

    // The next call finds no daemon, starts a replacement (a new port and token),
    // registers the session there again, and succeeds.
    let published = ok(&shim
        .call(
            "publish",
            json!({"html": "<title>again</title><p>again</p>"}),
        )
        .await);
    let after = ok(&shim.call("status", json!({})).await);
    assert_ne!(read_daemon_info(&shim.home()).unwrap().pid, old_pid);
    assert_eq!(after["session"]["id"], before["session"]["id"]);
    assert_ne!(after["daemon_url"], before["daemon_url"]);
    assert!(
        published["url"]
            .as_str()
            .unwrap()
            .starts_with(after["daemon_url"].as_str().unwrap()),
        "{published}"
    );
    let listed = ok(&shim.call("list", json!({"scope": "mine"})).await);
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1, "{listed}");
    shim.finish().await;
}

#[tokio::test]
async fn heartbeats_never_start_a_daemon_but_tool_calls_do() {
    let shim = Shim::start(Some(200)).await;
    let before = ok(&shim.call("status", json!({})).await);
    shim.stop_daemon();
    tokio::time::sleep(Duration::from_millis(700)).await;
    if let Some(info) = read_daemon_info(&shim.home()) {
        assert!(
            !artifax_server::daemon::pid_alive(info.pid),
            "a heartbeat started daemon {info:?}"
        );
    }
    let after = ok(&shim.call("status", json!({})).await);
    assert!(read_daemon_info(&shim.home()).is_some());
    assert_eq!(after["session"]["id"], before["session"]["id"]);
    shim.finish().await;
}

#[tokio::test]
async fn a_daemon_that_cannot_start_is_reported_as_unreachable() {
    // ARTIFAX_HOME is a regular file, so the daemon cannot be started.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ax"), "not a directory").unwrap();
    let mut shim = Shim::start_in(dir, None).await;

    assert_eq!(shim.client().list_all_tools().await.unwrap().len(), 9);
    let r = tokio::time::timeout(Duration::from_secs(10), shim.call("list", json!({})))
        .await
        .expect("the tool answers within 10s instead of hanging");
    let (v, is_error) = body(&r);
    assert!(is_error, "{v}");
    assert_eq!(v["error"]["code"], "daemon_unreachable", "{v}");
    assert_eq!(
        v["error"]["log"],
        shim.home().log_path().to_string_lossy().as_ref()
    );
    // With no daemon, the shim still exits promptly on stdin EOF.
    let t = Instant::now();
    shim.client.take().unwrap().cancel().await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

#[tokio::test]
async fn a_session_ended_under_the_shim_is_replaced_on_the_next_publish() {
    let shim = Shim::start(None).await;
    let before = shim.live_sessions().await;
    assert_eq!(before.len(), 1, "{before:?}");
    let old_id = before[0]["id"].as_str().unwrap().to_string();
    let token = read_daemon_info(&shim.home()).unwrap().token;
    let res = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .patch(format!("{}/api/sessions/{old_id}", shim.daemon_base()))
        .bearer_auth(token)
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(shim.live_sessions().await.is_empty());

    let published = ok(&shim
        .call("publish", json!({"html": "<p>after", "title": "After"}))
        .await);
    let live = shim.live_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_ne!(live[0]["id"], old_id.as_str());
    assert_eq!(live[0]["harness_session_id"], "test-sess");
    let got = shim
        .get(&format!(
            "/api/artifacts/{}",
            published["artifact_id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(got["artifact"]["owner_session_id"], live[0]["id"]);
    let status = ok(&shim.call("status", json!({})).await);
    assert_eq!(status["session"]["id"], live[0]["id"]);
    shim.finish().await;
}
