//! End-to-end tests of the Claude Code channel: a fake `claude` script runs
//! the built `clax mcp --agent claude` as its child, so the shim's parent
//! command line carries the launch flags under test, and the test drives the
//! shim over raw JSON-RPC lines.

use crate::common::clax_bin;
use clax_core::Home;
use clax_server::daemon::read_daemon_info;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// The harness variables a test run may inherit; every process here clears them.
const HARNESS_VARS: &[&str] = &[
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CLAX_SESSION_ID",
    "GROK_SESSION_ID",
    "GROK_HOOK_EVENT",
    "GROK_PLUGIN_ROOT",
    "GROK_HOME",
];

/// `cmd` with the harness environment cleared and scratch homes under `dir`.
fn scratch_env(cmd: &mut Command, dir: &Path) {
    for k in HARNESS_VARS {
        cmd.env_remove(k);
    }
    cmd.env("CLAX_HOME", dir.join("ax"))
        .env("HOME", dir)
        .env("CLAUDE_CONFIG_DIR", dir.join("claude-config"))
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
}

/// A stand-in for `claude`: `<dir>/claude <flags…>` runs `clax mcp --agent
/// claude` as its child, with stdin and stdout passed through, so the shim's
/// parent command line carries the flags. Driven over raw JSON-RPC lines.
struct FakeClaude {
    dir: tempfile::TempDir,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    pending: Vec<Value>,
    next_id: u64,
}

const FAKE: &str = "#!/bin/bash\n\"$CLAX_TEST_BIN\" --port 0 mcp --agent claude\n";

impl FakeClaude {
    fn launch(flags: &[&str]) -> FakeClaude {
        let dir = tempfile::tempdir().unwrap();
        let script = clax_fake_exe::install(&dir.path().join("claude"), FAKE);
        let mut cmd = Command::new(&script);
        cmd.args(flags).env("CLAX_TEST_BIN", clax_bin());
        FakeClaude::spawn(dir, cmd)
    }

    /// `clax --port 0 mcp --agent <agent>` run directly by the test.
    fn direct(agent: &str) -> FakeClaude {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(clax_bin());
        cmd.args(["--port", "0", "mcp", "--agent", agent]);
        FakeClaude::spawn(dir, cmd)
    }

    fn spawn(dir: tempfile::TempDir, mut cmd: Command) -> FakeClaude {
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        cmd.current_dir(dir.path().join("work"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        scratch_env(&mut cmd, dir.path());
        // Claude Code sets the session ID in its servers' environment.
        if cmd.get_program().to_string_lossy().ends_with("/claude") {
            cmd.env("CLAUDE_CODE_SESSION_ID", "fake-sess");
        }
        let mut child = cmd.spawn().expect("spawn the shim");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        FakeClaude {
            dir,
            child,
            stdin,
            rx,
            pending: Vec::new(),
            next_id: 1,
        }
    }

    fn home(&self) -> Home {
        Home::at(self.dir.path().join("ax"))
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns its response, keeping notifications that
    /// arrive meanwhile.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        self.response(id, Duration::from_secs(30))
    }

    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn response(&mut self, id: u64, within: Duration) -> Value {
        if let Some(i) = self.pending.iter().position(|v| v["id"] == id) {
            return self.pending.remove(i);
        }
        let end = Instant::now() + within;
        loop {
            let v = self
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .expect("response");
            if v["id"] == id {
                return v;
            }
            self.pending.push(v);
        }
    }

    /// `initialize` with `version`, then `notifications/initialized`.
    fn initialize(&mut self, version: &str) -> Value {
        let r = self.request(
            "initialize",
            json!({"protocolVersion": version, "capabilities": {},
                "clientInfo": {"name": "fake-claude", "version": "0"}}),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }

    /// Every channel event received within `within`.
    fn channel_events(&mut self, within: Duration) -> Vec<Value> {
        let end = Instant::now() + within;
        while let Ok(v) = self
            .rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            self.pending.push(v);
        }
        let (events, rest): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|v| v["method"] == "notifications/claude/channel");
        self.pending = rest;
        events.into_iter().map(|v| v["params"].clone()).collect()
    }

    /// The JSON body of a `tools/call` response.
    fn result_body(r: &Value) -> Value {
        assert!(r["result"]["isError"] != true, "{r}");
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("text")).unwrap()
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}));
        FakeClaude::result_body(&r)
    }

    /// Publishes a page titled "Push" and returns its artifact ID.
    fn publish(&mut self) -> String {
        let p = self.call(
            "publish",
            json!({"html": "<!doctype html><title>Push</title><main><h2>Goals</h2></main>"}),
        );
        p["artifact_id"].as_str().expect("artifact_id").to_string()
    }

    /// `(base, token)` of the daemon the shim started.
    fn daemon(&self) -> (String, String) {
        let info = read_daemon_info(&self.home()).expect("daemon.json");
        (
            format!(
                "http://{}:{}",
                clax_server::daemon::probe_host(&info.bind),
                info.port
            ),
            info.token,
        )
    }

    /// GETs `path` from the daemon with its token.
    fn get(&self, path: &str) -> Value {
        let (base, token) = self.daemon();
        http()
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .send()
            .expect("daemon answers")
            .json()
            .expect("JSON body")
    }

    /// Starts `clax feedback follow --once` for this session, with its stdout
    /// lines sent to the returned receiver.
    fn follow(&self, poll_secs: u64) -> (Child, Receiver<String>) {
        let mut cmd = Command::new(clax_bin());
        cmd.args([
            "feedback",
            "follow",
            "--once",
            "--agent",
            "claude",
            "--harness-session",
            "fake-sess",
            "--poll-secs",
            &poll_secs.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        scratch_env(&mut cmd, self.dir.path());
        let mut child = cmd.spawn().expect("spawn clax feedback follow");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (child, rx)
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        let mut stop = Command::new(clax_bin());
        stop.arg("stop").stdout(Stdio::null()).stderr(Stdio::null());
        scratch_env(&mut stop, self.dir.path());
        let _ = stop.status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
}

/// Opens a thread on `aid`'s `index.html` over REST with `text`, sent to the
/// agent (an `@agent` mention), and returns the thread ID.
fn send_comment(c: &FakeClaude, aid: &str, text: &str) -> String {
    let (base, token) = c.daemon();
    let form = reqwest::blocking::multipart::Form::new()
        .text("anchor", clax_server::testing::element_anchor().to_string())
        .text("body", format!("@agent {text}"))
        .text("version", "1");
    let res = http()
        .post(format!("{base}/api/artifacts/{aid}/threads"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .expect("daemon answers");
    assert_eq!(res.status(), 201);
    let v: Value = res.json().unwrap();
    v["thread"]["id"].as_str().expect("thread ID").to_string()
}

/// Every line `rx` yields within `within`.
fn lines_within(rx: &Receiver<String>, within: Duration) -> Vec<String> {
    let end = Instant::now() + within;
    let mut out = Vec::new();
    while let Ok(l) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        out.push(l);
    }
    out
}

const FLAG: &[&str] = &[
    "--dangerously-load-development-channels",
    "plugin:clax@clax",
];

#[test]
fn declares_the_channel_and_never_permission_relay() {
    let mut c = FakeClaude::launch(&[]);
    let r = c.initialize("2025-06-18");
    assert_eq!(
        r["result"]["capabilities"]["experimental"],
        json!({"claude/channel": {}}),
        "{r}"
    );
    let i = r["result"]["instructions"].as_str().unwrap();
    assert!(
        i.contains("<channel source=\"plugin:clax:clax\"") && i.contains("comments_read"),
        "{i}"
    );
}

#[test]
fn a_2026_07_28_client_gets_a_channel_capable_revision() {
    let mut c = FakeClaude::launch(FLAG);
    let r = c.initialize("2026-07-28");
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25", "{r}");
    let mut d = FakeClaude::launch(FLAG);
    let r = d.request("server/discover", json!({}));
    if let Some(v) = r["result"]["supportedVersions"].as_array() {
        assert!(!v.iter().any(|x| x == "2026-07-28"), "{r}");
        assert!(v.iter().any(|x| x == "2025-11-25"), "{r}");
    } else {
        assert!(r.get("error").is_some(), "{r}");
    }
}

#[test]
fn the_codex_shim_declares_no_channel() {
    let mut c = FakeClaude::direct("codex");
    let r = c.initialize("2026-07-28");
    assert!(r.get("result").is_some(), "{r}");
    assert!(
        r["result"]["capabilities"].get("experimental").is_none(),
        "{r}"
    );
    let i = r["result"]["instructions"].as_str().unwrap_or_default();
    assert!(!i.contains("<channel"), "{i}");
}

#[test]
fn forwards_one_channel_event_per_comment_when_launched_with_the_channel() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let tid = send_comment(&c, &aid, "please make it blue");
    let events = c.channel_events(Duration::from_secs(5));
    assert_eq!(events.len(), 1, "{events:?}");
    let content = events[0]["content"].as_str().unwrap();
    assert!(
        content.starts_with("[clax] New comment on \"Push\"") && content.contains(&tid),
        "{content}"
    );
    assert!(
        !content.contains("make it blue"),
        "a notice never carries the comment"
    );
    assert_eq!(
        events[0]["meta"],
        json!({"artifact_id": aid, "thread_id": tid, "comment_id": events[0]["meta"]["comment_id"]})
    );
    assert!(
        events[0]["meta"]["comment_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    for k in events[0]["meta"].as_object().unwrap().keys() {
        assert!(
            k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
            "{k}"
        );
    }
    assert!(
        c.channel_events(Duration::from_secs(3)).is_empty(),
        "announced once"
    );
}

#[test]
fn a_channel_notice_delivers_nothing() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let tid = send_comment(&c, &aid, "please make it blue");
    assert_eq!(c.channel_events(Duration::from_secs(5)).len(), 1);
    // Over REST: any tool call would deliver the comment (tier 1).
    let live = c.get("/api/sessions?live=true");
    let sid = live["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["harness_session_id"] == "fake-sess")
        .and_then(|s| s["id"].as_str())
        .unwrap_or_else(|| panic!("{live}"))
        .to_string();
    let path = format!("/api/sessions/{sid}/feedback?tier=stop_hook&wait=0");
    let v = c.get(&path);
    let items = v["feedback"].as_array().expect("feedback");
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["thread_id"], tid.as_str(), "{v}");
    c.call("comments_read", json!({"url_or_id": aid, "thread_id": tid}));
    let v = c.get(&path);
    assert_eq!(v["feedback"], json!([]), "{v}");
}

#[test]
fn sends_nothing_without_the_launch_flag() {
    let mut c = FakeClaude::launch(&[]);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let tid = send_comment(&c, &aid, "please make it blue");
    assert!(c.channel_events(Duration::from_secs(4)).is_empty());
    let (mut follower, rx) = c.follow(2);
    let lines = lines_within(&rx, Duration::from_secs(5));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(&tid), "{lines:?}");
    let end = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(s) = follower.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < end, "follow --once did not exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status:?}");
}

#[test]
fn a_comment_is_announced_once_across_channel_and_follow() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    for text in ["first", "second"] {
        let (mut follower, rx) = c.follow(1);
        std::thread::sleep(Duration::from_millis(500));
        send_comment(&c, &aid, text);
        let events = c.channel_events(Duration::from_secs(5));
        let lines = lines_within(&rx, Duration::from_millis(100));
        let _ = follower.kill();
        let _ = follower.wait();
        assert_eq!(
            events.len() + lines.len(),
            1,
            "{text}: {events:?} {lines:?}"
        );
    }
}

#[test]
fn no_channel_event_while_waiting_for_feedback() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let id = c.send_request(
        "tools/call",
        json!({"name": "wait_for_feedback", "arguments": {"timeout_s": 10}}),
    );
    std::thread::sleep(Duration::from_millis(500));
    send_comment(&c, &aid, "and the footer");
    let r = c.response(id, Duration::from_secs(5));
    let v = FakeClaude::result_body(&r);
    assert_eq!(v["feedback"].as_array().map(Vec::len), Some(1), "{v}");
    assert!(c.channel_events(Duration::from_secs(3)).is_empty());
}

#[test]
fn status_reports_the_channel_state() {
    let mut on = FakeClaude::launch(FLAG);
    on.initialize("2025-11-25");
    let s = on.call("status", json!({}));
    assert_eq!(s["push"]["tier"], "channel", "{s}");
    assert_eq!(s["push"]["channel"]["launch_flag"], "present");
    assert_eq!(s["push"]["channel"]["entry"], "plugin:clax@clax");
    assert_eq!(s["push"]["channel"]["registered"], Value::Null);
    assert!(s["push"].get("follow_command").is_none(), "{s}");

    let mut off = FakeClaude::launch(&[]);
    off.initialize("2025-11-25");
    let s = off.call("status", json!({}));
    assert_eq!(s["push"]["tier"], Value::Null, "{s}");
    assert_eq!(s["push"]["channel"]["launch_flag"], "absent");
    assert_eq!(
        s["push"]["channel"]["launch"],
        "claude --dangerously-load-development-channels plugin:clax@clax"
    );
    let cmd = s["push"]["follow_command"].as_str().unwrap();
    assert!(
        cmd.ends_with("feedback follow --once --agent claude --harness-session 'fake-sess'"),
        "{cmd}"
    );
}

#[test]
fn the_shim_logs_its_channel_state() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let log = std::fs::read_to_string(c.home().hooks_log_path()).expect("hooks.log");
    let line = log
        .lines()
        .find(|l| l.contains(" channel agent=claude "))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        line.contains(" channel agent=claude launch_flag=present "),
        "{line}"
    );
    assert!(line.contains("entry=\"plugin:clax@clax\""), "{line}");
}

/// `server/discover` params declaring `version` in the request `_meta`, as a
/// 2026-07-28 client sends its opening probe.
fn discover_params(version: &str) -> Value {
    json!({"_meta": {
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "fake-claude", "version": "0"},
    }})
}

/// A refused discover probe, then `initialize`: the session runs on the
/// legacy lifecycle, so requests without `_meta` work.
fn falls_back_after_a_refused_discover(mut c: FakeClaude, probe: &str, supported: Value) {
    let r = c.request("server/discover", discover_params(probe));
    assert_eq!(r["error"]["code"], -32022, "{r}");
    assert_eq!(r["error"]["message"], "Unsupported protocol version", "{r}");
    assert_eq!(
        r["error"]["data"],
        json!({"requested": probe, "supported": supported}),
        "{r}"
    );
    let r = c.initialize("2025-11-25");
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25", "{r}");
    let r = c.request("tools/list", json!({}));
    let tools = r["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(tools.iter().any(|t| t["name"] == "status"), "{r}");
    let s = c.call("status", json!({}));
    assert!(s.is_object(), "{s}");
}

#[test]
fn a_refused_discover_probe_falls_back_to_initialize_with_the_channel() {
    falls_back_after_a_refused_discover(
        FakeClaude::launch(FLAG),
        "2026-07-28",
        json!(["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]),
    );
}

#[test]
fn a_refused_discover_probe_falls_back_to_initialize_without_the_channel() {
    falls_back_after_a_refused_discover(
        FakeClaude::direct("codex"),
        "2099-01-01",
        json!([
            "2024-11-05",
            "2025-03-26",
            "2025-06-18",
            "2025-11-25",
            "2026-07-28"
        ]),
    );
}

#[test]
fn channel_events_flow_after_a_discover_fallback() {
    let mut c = FakeClaude::launch(FLAG);
    let r = c.request("server/discover", discover_params("2026-07-28"));
    assert_eq!(r["error"]["code"], -32022, "{r}");
    c.initialize("2025-11-25");
    let aid = c.publish();
    let tid = send_comment(&c, &aid, "please make it blue");
    let events = c.channel_events(Duration::from_secs(5));
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["meta"]["thread_id"], tid.as_str(), "{events:?}");
}

#[test]
fn a_supported_discover_probe_opens_an_inline_session() {
    let mut c = FakeClaude::direct("codex");
    let r = c.request("server/discover", discover_params("2026-07-28"));
    let v = r["result"]["supportedVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(v.iter().any(|x| x == "2026-07-28"), "{r}");
    // The inline lifecycle needs `_meta` on every request.
    let r = c.request("tools/list", json!({}));
    assert!(r.get("error").is_some(), "{r}");
    let r = c.request("tools/list", discover_params("2026-07-28"));
    let tools = r["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(tools.iter().any(|t| t["name"] == "status"), "{r}");
}
