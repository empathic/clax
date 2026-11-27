//! End-to-end tests of `clax mcp`: the built binary is spawned as a stdio MCP
//! server, starts its own daemon in a temporary `CLAX_HOME`, and is driven
//! with rmcp's client.

use clax_core::Home;
use clax_core::daemon::read_daemon_info;
use clax_fake_exe::clax_bin;
use clax_testkit::{until, within};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::path::PathBuf;

/// A running shim with its own `CLAX_HOME`; dropping it stops the daemon the
/// shim started. The shim logs its warnings to `<dir>/shim.log`
/// ([`Shim::log`]), shown when a test fails.
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

    /// Starts an Antigravity shim, with `CLAX_SESSION_ID` set (it must not
    /// be used), against a daemon already serving its home, so the daemon
    /// can be asked about sessions before any call.
    async fn start_antigravity() -> Shim {
        let dir = Shim::serving_daemon();
        Shim::spawn(dir, None, "antigravity", ("CLAX_SESSION_ID", "from-env")).await
    }

    /// A directory whose home `<dir>/ax` a daemon serves.
    fn serving_daemon() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(clax_bin(None))
            .args(["--port", "0", "serve"])
            .env("CLAX_HOME", dir.path().join("ax"))
            .env("CLAX_CODEX_BIN", "")
            .env("HOME", dir.path())
            .output()
            .expect("run clax serve");
        assert!(out.status.success(), "{out:?}");
        dir
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
        let cmd = tokio::process::Command::new(clax_bin(None)).configure(|c| {
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
                "ANTIGRAVITY_AGENT",
                "ANTIGRAVITY_CONVERSATION_ID",
            ] {
                c.env_remove(var);
            }
            c.args(["--port", "0", "mcp", "--agent", agent])
                .env("CLAX_HOME", &home)
                .env("CLAX_CODEX_BIN", "")
                .env("HOME", dir.path())
                .env(session_var.0, session_var.1)
                .env("CLAX_NO_OPEN", "1")
                // Captures never race the real deadline on a loaded machine.
                .env(clax_mcp::git::DEADLINE_VAR, "20000")
                .env("RUST_LOG", "warn")
                .current_dir(&work);
            if let Some(ms) = heartbeat_ms {
                c.args(["--heartbeat-interval-ms", &ms.to_string()]);
            }
        });
        let log = std::fs::File::create(dir.path().join("shim.log")).unwrap();
        let (transport, _) = TokioChildProcess::builder(cmd)
            .stderr(log)
            .spawn()
            .expect("spawn clax mcp");
        let client = ().serve(transport).await.expect("MCP handshake");
        Shim {
            dir,
            client: Some(client),
        }
    }

    fn home(&self) -> Home {
        Home::at(self.dir.path().join("ax"))
    }

    /// What the shim has logged so far.
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("shim.log")).unwrap_or_default()
    }

    fn client(&self) -> &RunningService<RoleClient, ()> {
        self.client.as_ref().unwrap()
    }

    /// `http://<host>:<port>` of the daemon the shim started.
    fn daemon_base(&self) -> String {
        let info = read_daemon_info(&self.home()).expect("daemon.json");
        format!(
            "http://{}:{}",
            clax_core::daemon::probe_host(&info.bind),
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

    /// POSTs `body` to `path` on the daemon with its token.
    async fn post(&self, path: &str, body: Value) -> Value {
        let token = read_daemon_info(&self.home()).expect("daemon.json").token;
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{}{path}", self.daemon_base()))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("daemon answers")
            .json()
            .await
            .expect("JSON body")
    }

    /// Waits until session `sid` has `n` feedback polls of the kind
    /// `query` names (`parked=wait`, `held=true`; empty for every open
    /// `wait_for_feedback` poll).
    async fn until_waiters(&self, sid: &str, query: &str, n: u64) {
        let what = format!("{n} feedback polls ({query}) of {sid}");
        let path = format!("/api/_test/sessions/{sid}/feedback/waiters?until={n}&{query}");
        within(
            &what,
            until(&what, || async {
                (self.get(&path).await["count"] == n).then_some(())
            }),
        )
        .await;
    }

    /// Ends every open `wait_for_feedback` poll of session `sid` now, as if
    /// it had run out; how many that was.
    async fn expire(&self, sid: &str) -> Value {
        self.post(
            &format!("/api/_test/sessions/{sid}/feedback/expire"),
            json!({}),
        )
        .await["expired"]
            .clone()
    }

    async fn live_sessions(&self) -> Vec<Value> {
        self.get("/api/sessions?live=true").await["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The live sessions of harness `antigravity`.
    async fn antigravity_sessions(&self) -> Vec<Value> {
        let mut live = self.live_sessions().await;
        live.retain(|s| s["harness"] == "antigravity");
        live
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

    /// Calls tool `name` with `args` and the request `_meta` `meta`.
    async fn call_with_meta(&self, meta: Value, name: &'static str, args: Value) -> CallToolResult {
        let (Value::Object(meta), Value::Object(args)) = (meta, args) else {
            panic!("the meta and the tool arguments are objects")
        };
        let mut params = CallToolRequestParams::new(name).with_arguments(args);
        params.meta = Some(meta.into());
        self.client().call_tool(params).await.expect("tool call")
    }

    /// Sends a `wait_for_feedback` call with `args` from root conversation
    /// `root`, returning its handle, by which the test can cancel it.
    async fn start_wait(&self, root: &str, args: Value) -> RequestHandle<RoleClient> {
        let (Value::Object(meta), Value::Object(args)) = (from_conversation(root, root), args)
        else {
            panic!("the meta and the tool arguments are objects")
        };
        let mut params = CallToolRequestParams::new("wait_for_feedback").with_arguments(args);
        params.meta = Some(meta.into());
        self.client()
            .send_cancellable_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                PeerRequestOptions::no_options(),
            )
            .await
            .expect("the call is sent")
    }

    /// Runs `clax stop` against this shim's home and checks the daemon
    /// process is gone (the shim, its parent, must have reaped it).
    fn stop_daemon(&self) {
        let out = std::process::Command::new(clax_bin(None))
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

    /// [`Shim::finish`], keeping the shim's directory for the test to read.
    async fn finish_keeping_dir(mut self) -> tempfile::TempDir {
        if let Some(client) = self.client.take() {
            client.cancel().await.unwrap();
        }
        self.stop_daemon();
        std::mem::replace(&mut self.dir, tempfile::tempdir().unwrap())
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("the shim's log:\n{}", self.log());
        }
        let _ = std::process::Command::new(clax_bin(None))
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
            "ask",
            "asset_upload",
            "comments_handoff",
            "comments_read",
            "comments_reply",
            "comments_resolve",
            "connect_instance",
            "db_batch",
            "db_delete",
            "db_get",
            "db_list",
            "db_query",
            "db_set",
            "db_str_replace",
            "db_update",
            "delete",
            "draft",
            "fork",
            "git_remote",
            "list",
            "open",
            "pin",
            "publish",
            "publish_public",
            "queue_claim",
            "queue_fix",
            "queue_get",
            "queue_list",
            "queue_release",
            "queue_renew",
            "queue_reply",
            "queue_request",
            "queue_resolve",
            "queue_watch",
            "read",
            "status",
            "tag",
            "tags",
            "unpin",
            "wait_for_feedback",
            "watch",
            "where_to_ask",
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
    let seen = || async {
        shim.live_sessions().await[0]["last_seen_at"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let before = seen().await;
    let what = format!("last_seen_at past {before}");
    within(
        &what,
        until(&what, || async { (seen().await > before).then_some(()) }),
    )
    .await;
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
    let what = format!("session {id} ended");
    within(
        &what,
        until(&what, || async {
            let s = shim.get(&format!("/api/sessions/{id}")).await;
            (!s["session"]["ended_at"].is_null()).then_some(())
        }),
    )
    .await;
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
    // A heartbeat has found no daemon (it says so once).
    let what = "a heartbeat with no daemon";
    within(
        what,
        until(what, || async {
            shim.log().contains("heartbeat failed").then_some(())
        }),
    )
    .await;
    if let Some(info) = read_daemon_info(&shim.home()) {
        assert!(
            !clax_core::daemon::pid_alive(info.pid),
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

    assert_eq!(shim.client().list_all_tools().await.unwrap().len(), 43);
    let r = within("the tool's answer", shim.call("list", json!({}))).await;
    let (v, is_error) = body(&r);
    assert!(is_error, "{v}");
    assert_eq!(v["error"]["code"], "daemon_unreachable", "{v}");
    assert_eq!(
        v["error"]["log"],
        shim.home().log_path().to_string_lossy().as_ref()
    );
    // With no daemon, the shim still exits on stdin EOF, without waiting
    // out its time to end the session or send its reports.
    within("the shim's exit", shim.client.take().unwrap().cancel())
        .await
        .unwrap();
    let log = shim.log();
    assert!(!log.contains("ending the session timed out"), "{log}");
    assert!(!log.contains("reports still unsent"), "{log}");
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
    let out = std::process::Command::new(clax_bin(None))
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
    assert!(!clax_core::daemon::pid_alive(old_pid as u32));
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
    // A browser of this machine's with no name: its viewer minted in the
    // store (reaching the daemon's port alone mints none).
    let cookie = clax_core::secret::viewer_cookie().expose().to_string();
    clax_store::Store::open(&shim.home())
        .unwrap()
        .mint_viewer(&cookie, true)
        .unwrap();
    let thread = |body: &'static str| {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", body)
            .text("version", "1");
        http.post(format!("{base}/api/artifacts/{aid}/threads"))
            .header("cookie", format!("clax_viewer={cookie}"))
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
        trailing.contains("\nViewer (not connected): \"@agent two columns\"\n"),
        "{trailing}"
    );

    // The longest wait, past the hang guard: the comment ends it.
    let sid = shim.live_sessions().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let waiting = shim.call("wait_for_feedback", json!({"timeout_s": 600}));
    let sending = async {
        let what = "the wait in progress";
        until(what, || async {
            let v = shim
                .get(&format!("/api/_test/sessions/{sid}/feedback/waiters"))
                .await;
            (v["count"] == 1).then_some(())
        })
        .await;
        assert_eq!(thread("@agent and the footer").await.unwrap().status(), 201);
    };
    let (r, ()) = within("the wait woken by the comment", async {
        tokio::join!(waiting, sending)
    })
    .await;
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

/// The skill runs `push.follow_command` as given, so a session ID is one
/// shell word whatever it holds.
#[tokio::test]
async fn a_grok_status_gives_the_monitor_command_shell_quoted() {
    let shim = Shim::start_grok("g1'; touch pwned #").await;
    let v = ok(&shim.call("status", json!({})).await);
    let bin = v["binary"]["path"].as_str().unwrap().to_string();
    assert_eq!(
        v["push"]["follow_command"],
        format!(
            "'{}' feedback follow --agent grok --harness-session 'g1'\\''; touch pwned #'",
            bin.replace('\'', "'\\''")
        ),
        "{v}"
    );
    shim.finish().await;
}

/// The shim captures git state at registration, sends the call's identity
/// with each call, and reports its calls before it exits.
#[tokio::test]
async fn shim_records_registration_git_and_reports_calls_before_exit() {
    let shim = Shim::start(None).await;
    ok(&shim
        .call("publish", json!({"html": "<title>Shim</title><p>x"}))
        .await);
    let dir = shim.finish_keeping_dir().await;
    let home = Home::at(dir.path().join("ax"));
    let c = rusqlite::Connection::open(home.db_path()).unwrap();
    let rows: Vec<(String, String)> = c
        .prepare("SELECT kind, body FROM audit_events ORDER BY seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let body = |kind: &str| -> Value {
        let (_, b) = rows
            .iter()
            .find(|(k, _)| k == kind)
            .unwrap_or_else(|| panic!("no {kind} in {rows:?}"));
        serde_json::from_str(b).unwrap()
    };
    // The working directory is no repository.
    // A debug shim honours the test's long deadline; a release one keeps
    // the real 300 ms, which a loaded machine can pass.
    let start = body("session.start")["git_capture"].clone();
    if cfg!(debug_assertions) {
        assert_eq!(start, "not-a-repo");
    } else {
        assert!(start == "not-a-repo" || start == "timeout", "{start}");
    }
    assert_eq!(body("session.start")["via"], "mcp");
    let created = body("artifact.create");
    assert!(
        created["git_capture"] == "not-a-repo"
            || (!cfg!(debug_assertions) && created["git_capture"] == "timeout"),
        "{created}"
    );
    assert_eq!(created["call"]["tool"], "publish");
    let call = body("tool.call");
    assert_eq!(call["tool"], "publish");
    assert_eq!(call["call_id"], created["call"]["call_id"]);
    assert_eq!(call["outcome"], "ok");
}

/// The `_meta` of a call from conversation `conversation` under root
/// conversation `root`, as Antigravity sends it.
fn from_conversation(root: &str, conversation: &str) -> Value {
    json!({
        clax_core::antigravity::META_ROOT: root,
        clax_core::antigravity::META_CONVERSATION: conversation,
        "antigravity.google/artifacts_dir": format!("/brain/{conversation}"),
    })
}

/// Posts comment `body` on version 1 of artifact `aid` from a browser of
/// this machine's, as a viewer the store minted.
async fn comment(shim: &Shim, aid: &str, body: &'static str) {
    let cookie = clax_core::secret::viewer_cookie().expose().to_string();
    clax_store::Store::open(&shim.home())
        .unwrap()
        .mint_viewer(&cookie, true)
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", body)
        .text("version", "1");
    let res = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/api/artifacts/{aid}/threads",
            shim.daemon_base()
        ))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn an_antigravity_shim_registers_no_session_before_a_call() {
    let shim = Shim::start_antigravity().await;
    assert_eq!(shim.client().list_all_tools().await.unwrap().len(), 43);
    assert_eq!(shim.antigravity_sessions().await, Vec::<Value>::new());
    shim.finish().await;
}

#[tokio::test]
async fn two_root_conversations_get_two_sessions() {
    let shim = Shim::start_antigravity().await;
    let s1 = ok(&shim
        .call_with_meta(from_conversation("r1", "r1"), "status", json!({}))
        .await);
    let s2 = ok(&shim
        .call_with_meta(from_conversation("r2", "r2"), "status", json!({}))
        .await);
    assert_eq!(s1["session"]["harness_session_id"], "r1", "{s1}");
    assert_eq!(s2["session"]["harness_session_id"], "r2", "{s2}");
    let again = ok(&shim
        .call_with_meta(from_conversation("r1", "r1"), "status", json!({}))
        .await);
    assert_eq!(again["session"]["id"], s1["session"]["id"]);

    let mut live: Vec<(Value, Value)> = shim
        .antigravity_sessions()
        .await
        .into_iter()
        .map(|s| (s["harness_session_id"].clone(), s["id"].clone()))
        .collect();
    live.sort_by_key(|(h, _)| h.to_string());
    assert_eq!(
        live,
        [
            (json!("r1"), s1["session"]["id"].clone()),
            (json!("r2"), s2["session"]["id"].clone()),
        ]
    );
    shim.finish().await;
}

#[tokio::test]
async fn a_subagent_acts_in_the_root_session_and_is_handed_nothing() {
    let shim = Shim::start_antigravity().await;
    let p = ok(&shim
        .call_with_meta(
            from_conversation("r1", "r1"),
            "publish",
            json!({"html": "<main><h2>Goals</h2></main>", "title": "Loop"}),
        )
        .await);
    let aid = p["artifact_id"].as_str().unwrap();
    comment(&shim, aid, "@agent two columns").await;

    // `body` checks the result has one content block: no feedback.
    let listed = ok(&shim
        .call_with_meta(from_conversation("r1", "s1"), "list", json!({}))
        .await);
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1, "{listed}");
    let status = ok(&shim
        .call_with_meta(from_conversation("r1", "s1"), "status", json!({}))
        .await);
    assert_eq!(status["session"]["harness_session_id"], "r1", "{status}");
    let live = shim.antigravity_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");

    let r = shim
        .call_with_meta(from_conversation("r1", "r1"), "list", json!({}))
        .await;
    assert_eq!(r.content.len(), 2, "{r:?}");
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(v["feedback"][0]["body"], "@agent two columns", "{v}");
    shim.finish().await;
}

#[tokio::test]
async fn a_subagent_cannot_wait() {
    let shim = Shim::start_antigravity().await;
    let r = shim
        .call_with_meta(
            from_conversation("r1", "s1"),
            "wait_for_feedback",
            json!({"timeout_s": 1}),
        )
        .await;
    let (v, is_error) = body(&r);
    assert!(is_error, "{v}");
    assert_eq!(
        v["error"]["message"],
        "wait_for_feedback is for the main conversation: call it there, not from a subagent.",
        "{v}"
    );
    shim.finish().await;
}

#[tokio::test]
async fn a_call_with_no_conversation_registers_without_a_harness_session_id() {
    let shim = Shim::start_antigravity().await;
    let status = ok(&shim.call("status", json!({})).await);
    let live = shim.antigravity_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["harness_session_id"], Value::Null, "{live:?}");
    assert_eq!(live[0]["id"], status["session"]["id"]);
    shim.finish().await;
}

#[tokio::test]
async fn empty_or_non_string_conversation_keys_count_as_absent() {
    let shim = Shim::start_antigravity().await;
    let (root, conversation) = (
        clax_core::antigravity::META_ROOT,
        clax_core::antigravity::META_CONVERSATION,
    );
    let empty = ok(&shim
        .call_with_meta(json!({root: "", conversation: ""}), "status", json!({}))
        .await);
    let numbers = ok(&shim
        .call_with_meta(json!({root: 1, conversation: 2}), "status", json!({}))
        .await);
    let live = shim.antigravity_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["harness_session_id"], Value::Null, "{live:?}");
    assert_eq!(empty["session"]["id"], live[0]["id"]);
    assert_eq!(numbers["session"]["id"], live[0]["id"]);
    shim.finish().await;
}

#[tokio::test]
async fn every_conversation_is_ended_on_exit_and_its_hook_files_removed() {
    let mut shim = Shim::start_antigravity().await;
    let run = shim.home().root().join("run");
    let stamp = run.join("tool-hook/antigravity-r1");
    let ledger = run.join("wake-nudge/antigravity-r1.json");
    let beside = [run.join("x"), run.join("x.json")];
    for f in [&stamp, &ledger].into_iter().chain(&beside) {
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, "").unwrap();
    }
    let mut ids = Vec::new();
    for root in ["r1", "r2", "../x"] {
        let s = ok(&shim
            .call_with_meta(from_conversation(root, root), "status", json!({}))
            .await);
        ids.push(s["session"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(shim.antigravity_sessions().await.len(), 3);

    // Closes the shim's stdin and waits for it to exit.
    shim.client.take().unwrap().cancel().await.unwrap();
    for id in &ids {
        let s = shim.get(&format!("/api/sessions/{id}")).await;
        assert!(!s["session"]["ended_at"].is_null(), "{s}");
    }
    assert!(!stamp.exists() && !ledger.exists());
    for f in &beside {
        assert!(f.exists(), "{}", f.display());
    }
    shim.finish().await;
}

/// The result a `wait_for_feedback` call answered `handle` with.
async fn wait_result(handle: RequestHandle<RoleClient>) -> CallToolResult {
    match within("the wait's result", handle.await_response()).await {
        Ok(ServerResult::CallToolResult(r)) => r,
        other => panic!("not a tool result: {other:?}"),
    }
}

/// The session ID of root conversation `root`, registering it.
async fn session_of(shim: &Shim, root: &str) -> String {
    let s = ok(&shim
        .call_with_meta(from_conversation(root, root), "status", json!({}))
        .await);
    s["session"]["id"].as_str().unwrap().to_string()
}

/// Cancels the wait `handle` names and waits until its poll is gone.
async fn cancel_wait(shim: &Shim, sid: &str, handle: RequestHandle<RoleClient>) {
    handle.cancel(Some("test".into())).await.unwrap();
    shim.until_waiters(sid, "", 0).await;
}

#[tokio::test]
async fn an_unbounded_wait_outlasts_a_server_poll() {
    let shim = Shim::start_antigravity().await;
    let sid = session_of(&shim, "r1").await;
    let mut wait = shim.start_wait("r1", json!({})).await;
    shim.until_waiters(&sid, "parked=wait", 1).await;
    // Past its first take, the open poll never stops at `first_take`; the
    // next poll does.
    let hold = format!("/api/_test/sessions/{sid}/feedback/hold");
    shim.post(&hold, json!({"hold": true, "at": "first_take"}))
        .await;
    assert_eq!(shim.expire(&sid).await, 1);
    shim.until_waiters(&sid, "held=true", 1).await;
    assert!(wait.rx.try_recv().is_err(), "the wait answered");
    shim.post(&hold, json!({"hold": false})).await;
    cancel_wait(&shim, &sid, wait).await;
    shim.finish().await;
}

#[tokio::test]
async fn an_unbounded_wait_returns_the_comment() {
    let shim = Shim::start_antigravity().await;
    let p = ok(&shim
        .call_with_meta(
            from_conversation("r1", "r1"),
            "publish",
            json!({"html": "<main><h2>Goals</h2></main>", "title": "Loop"}),
        )
        .await);
    let sid = session_of(&shim, "r1").await;
    let wait = shim.start_wait("r1", json!({})).await;
    shim.until_waiters(&sid, "", 1).await;
    comment(
        &shim,
        p["artifact_id"].as_str().unwrap(),
        "@agent two columns",
    )
    .await;
    let r = wait_result(wait).await;
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(v["feedback"][0]["body"], "@agent two columns", "{v}");
    assert_eq!(v["call_again"], false, "{v}");
    shim.finish().await;
}

#[tokio::test]
async fn a_second_wait_while_one_is_open_returns_at_once() {
    let shim = Shim::start_antigravity().await;
    let sid = session_of(&shim, "r1").await;
    let wait = shim.start_wait("r1", json!({})).await;
    shim.until_waiters(&sid, "", 1).await;
    let second = ok(&shim
        .call_with_meta(
            from_conversation("r1", "r1"),
            "wait_for_feedback",
            json!({}),
        )
        .await);
    assert_eq!(second["already_waiting"], true, "{second}");
    assert_eq!(second["call_again"], false, "{second}");
    let open = shim
        .get(&format!("/api/_test/sessions/{sid}/feedback/waiters"))
        .await;
    assert_eq!(open["count"], 1, "{open}");
    cancel_wait(&shim, &sid, wait).await;
    shim.finish().await;
}

#[tokio::test]
async fn a_cancelled_wait_can_be_opened_again() {
    let shim = Shim::start_antigravity().await;
    let sid = session_of(&shim, "r1").await;
    let first = shim.start_wait("r1", json!({})).await;
    shim.until_waiters(&sid, "", 1).await;
    cancel_wait(&shim, &sid, first).await;

    let mut again = shim.start_wait("r1", json!({})).await;
    tokio::select! {
        r = &mut again.rx => panic!("the wait answered instead of waiting: {r:?}"),
        () = shim.until_waiters(&sid, "", 1) => {}
    }
    cancel_wait(&shim, &sid, again).await;
    shim.finish().await;
}

#[tokio::test]
async fn a_wait_with_a_timeout_is_bounded() {
    let shim = Shim::start_antigravity().await;
    let sid = session_of(&shim, "r1").await;
    let wait = shim.start_wait("r1", json!({"timeout_s": 5})).await;
    shim.until_waiters(&sid, "", 1).await;
    assert_eq!(shim.expire(&sid).await, 1);
    let v = ok(&wait_result(wait).await);
    assert_eq!(v["call_again"], true, "{v}");
    shim.finish().await;
}
