//! End-to-end tests of `clax mcp`: the built binary is spawned as a stdio MCP
//! server, starts its own daemon in a temporary `CLAX_HOME`, and is driven
//! with rmcp's client.

use clax_core::Home;
use clax_server::daemon::read_daemon_info;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The `clax` binary, built once per test run. It belongs to `clax-cli`,
/// so Cargo does not provide `CARGO_BIN_EXE_clax` here; the tests build it
/// into the same target directory as this test executable.
fn clax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = std::process::Command::new(env!("CARGO"));
        cmd.current_dir(&workspace)
            .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"]);
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("run cargo build");
        assert!(status.success(), "cargo build -p clax-cli failed");
        // Assumes Cargo's layout: the test executable lives in
        // <target>/<profile>/deps/, and `cargo build` (which honours
        // CARGO_TARGET_DIR like the enclosing `cargo test`) puts the binary in
        // <target>/<profile>/.
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("clax");
        assert!(bin.exists(), "{} missing", bin.display());
        bin
    })
    .clone()
}

/// A running shim with its own `CLAX_HOME`; dropping it stops the daemon the
/// shim started.
struct Shim {
    dir: tempfile::TempDir,
    client: Option<RunningService<RoleClient, ()>>,
}

impl Shim {
    async fn start(heartbeat_ms: Option<u64>) -> Shim {
        Shim::start_in(tempfile::tempdir().unwrap(), heartbeat_ms).await
    }

    /// Starts a Claude Code shim whose `CLAX_HOME` is `<dir>/ax`.
    async fn start_in(dir: tempfile::TempDir, heartbeat_ms: Option<u64>) -> Shim {
        Shim::spawn(
            dir,
            heartbeat_ms,
            "claude",
            ("CLAUDE_CODE_SESSION_ID", "test-sess"),
        )
        .await
    }

    /// Starts a Grok Build shim whose `GROK_SESSION_ID` is `session_id`.
    async fn start_grok(session_id: &str) -> Shim {
        Shim::spawn(
            tempfile::tempdir().unwrap(),
            None,
            "grok",
            ("GROK_SESSION_ID", session_id),
        )
        .await
    }

    /// Starts `clax mcp --agent <agent>` with the inherited harness
    /// environment cleared and only `session_var` set.
    async fn spawn(
        dir: tempfile::TempDir,
        heartbeat_ms: Option<u64>,
        agent: &str,
        session_var: (&str, &str),
    ) -> Shim {
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let home = dir.path().join("ax");
        let work = dir.path().join("work");
        let cmd = tokio::process::Command::new(clax_bin()).configure(|c| {
            for var in [
                "GROK_SESSION_ID",
                "GROK_HOOK_EVENT",
                "GROK_PLUGIN_ROOT",
                "GROK_HOME",
                "CLAUDE_PID",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_PLUGIN_ROOT",
                "CLAUDE_PROJECT_DIR",
                "CLAX_SESSION_ID",
            ] {
                c.env_remove(var);
            }
            c.args(["--port", "0", "mcp", "--agent", agent])
                .env("CLAX_HOME", &home)
                .env("CLAX_CODEX_BIN", "")
                .env("HOME", dir.path())
                .env(session_var.0, session_var.1)
                .env("CLAX_NO_OPEN", "1")
                .env("RUST_LOG", "error")
                .current_dir(&work);
            if let Some(ms) = heartbeat_ms {
                c.args(["--heartbeat-interval-ms", &ms.to_string()]);
            }
        });
        let transport = TokioChildProcess::new(cmd).expect("spawn clax mcp");
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
            clax_server::daemon::probe_host(&info.bind),
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

    /// Runs `clax stop` against this shim's home and checks the daemon
    /// process is gone (the shim, its parent, must have reaped it).
    fn stop_daemon(&self) {
        let out = std::process::Command::new(clax_bin())
            .args(["--json", "stop"])
            .env("CLAX_HOME", self.dir.path().join("ax"))
            .env("CLAX_CODEX_BIN", "")
            .env("HOME", self.dir.path())
            .output()
            .expect("run clax stop");
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
        let _ = std::process::Command::new(clax_bin())
            .arg("stop")
            .env("CLAX_HOME", self.dir.path().join("ax"))
            .env("CLAX_CODEX_BIN", "")
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
            "comments_read",
            "comments_reply",
            "comments_resolve",
            "db_batch",
            "db_delete",
            "db_get",
            "db_list",
            "db_query",
            "db_set",
            "db_str_replace",
            "db_update",
            "delete",
            "list",
            "open",
            "pin",
            "publish",
            "read",
            "status",
            "unpin",
            "wait_for_feedback",
            "watch",
            "working"
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
            !clax_server::daemon::pid_alive(info.pid),
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
    // CLAX_HOME is a regular file, so the daemon cannot be started.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ax"), "not a directory").unwrap();
    let mut shim = Shim::start_in(dir, None).await;

    assert_eq!(shim.client().list_all_tools().await.unwrap().len(), 23);
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

#[tokio::test]
async fn an_older_daemon_is_replaced_on_the_same_bind() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let out = std::process::Command::new(clax_bin())
        .args(["--port", "0", "serve", "--bind", "0.0.0.0"])
        .env("CLAX_HOME", &home)
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", dir.path())
        .output()
        .expect("run clax serve");
    assert!(out.status.success(), "{out:?}");
    // Make the running daemon look older than the shim.
    let path = home.join("daemon.json");
    let mut info: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let old_pid = info["pid"].as_u64().unwrap();
    info["version"] = json!("0.0.1");
    std::fs::write(&path, info.to_string()).unwrap();

    let shim = Shim::start_in(dir, None).await;
    ok(&shim.call("status", json!({})).await);
    let now = read_daemon_info(&shim.home()).unwrap();
    assert_ne!(u64::from(now.pid), old_pid, "the older daemon was replaced");
    assert_eq!(now.bind, "0.0.0.0");
    assert!(!clax_server::daemon::pid_alive(old_pid as u32));
    shim.finish().await;
}

#[tokio::test]
async fn piggyback_and_wait_through_the_shim() {
    let shim = Shim::start(None).await;
    let p = ok(&shim
        .call(
            "publish",
            json!({"html": "<main><h2>Goals</h2></main>", "title": "Loop"}),
        )
        .await);
    let aid = p["artifact_id"].as_str().unwrap().to_string();
    let base = shim.daemon_base();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let thread = |body: &'static str| {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_server::testing::element_anchor().to_string())
            .text("body", body)
            .text("version", "1");
        http.post(format!("{base}/api/artifacts/{aid}/threads"))
            .multipart(form)
            .send()
    };
    assert_eq!(thread("@agent two columns").await.unwrap().status(), 201);
    let r = shim.call("list", json!({})).await;
    assert_eq!(r.content.len(), 2, "{r:?}");
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(v["feedback"][0]["body"], "@agent two columns");
    let trailing = &r.content[1].as_text().unwrap().text;
    assert!(
        trailing.starts_with(
            "---\n[clax] 1 comment sent to you:\n[clax] Comment sent to you on \"Loop\""
        ),
        "{trailing}"
    );
    assert!(
        trailing.contains("\nViewer: \"@agent two columns\"\n"),
        "{trailing}"
    );

    let waiting = shim.call("wait_for_feedback", json!({"timeout_s": 10}));
    let sending = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(thread("@agent and the footer").await.unwrap().status(), 201);
        Instant::now()
    };
    let (r, sent) = tokio::join!(waiting, sending);
    assert!(Instant::now().saturating_duration_since(sent) < Duration::from_secs(1));
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(v["feedback"][0]["body"], "@agent and the footer");
    assert_eq!(v["call_again"], false);
    let v = ok(&shim
        .call("wait_for_feedback", json!({"timeout_s": 1}))
        .await);
    assert_eq!(v["call_again"], true);
    shim.finish().await;
}

#[tokio::test]
async fn a_grok_shim_registers_by_its_session_id() {
    let shim = Shim::start_grok("019a-shim").await;
    let live = shim.live_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["harness"], "grok");
    assert_eq!(live[0]["harness_session_id"], "019a-shim");
    shim.finish().await;
}
