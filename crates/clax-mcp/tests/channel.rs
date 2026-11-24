//! End-to-end tests of the Claude Code channel: a fake `claude` script runs
//! the built `clax mcp --agent claude` as its child, so the shim's parent
//! command line carries the launch flags under test, and the test drives the
//! shim over raw JSON-RPC lines.

use clax_core::Home;
use clax_core::daemon::read_daemon_info;
use clax_fake_exe::clax_bin;
use clax_testkit::{HANG, recv_within};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
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

/// What the shim or a follower (`FakeClaude::follow`) wrote, in the order
/// the test received it.
enum Out {
    /// A JSON-RPC message from the shim.
    Shim(Value),
    /// A line from the follower.
    Follower(String),
    /// The follower closed its stdout.
    FollowerEnd,
}

/// A stand-in for `claude`: `<dir>/claude <flags…>` runs `clax mcp --agent
/// claude` as its child, with stdin and stdout passed through, so the shim's
/// parent command line carries the flags. Driven over raw JSON-RPC lines.
struct FakeClaude {
    dir: tempfile::TempDir,
    child: Child,
    stdin: ChildStdin,
    /// The shim's and its followers' output, one channel, so a test can
    /// take whichever comes first.
    tx: Sender<Out>,
    rx: Receiver<Out>,
    /// What arrived while the test waited for something else.
    pending: Vec<Out>,
    next_id: u64,
}

const FAKE: &str = "#!/bin/bash\n\"$CLAX_TEST_BIN\" --port 0 mcp --agent claude\n";

impl FakeClaude {
    fn launch(flags: &[&str]) -> FakeClaude {
        let dir = tempfile::tempdir().unwrap();
        let script = clax_fake_exe::install(&dir.path().join("claude"), FAKE);
        let mut cmd = Command::new(&script);
        cmd.args(flags).env("CLAX_TEST_BIN", clax_bin(None));
        FakeClaude::spawn(dir, cmd)
    }

    /// `clax --port 0 mcp --agent <agent>` run directly by the test.
    fn direct(agent: &str) -> FakeClaude {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(clax_bin(None));
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
        let shim = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && shim.send(Out::Shim(v)).is_err()
                {
                    break;
                }
            }
        });
        FakeClaude {
            dir,
            child,
            stdin,
            tx,
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
        self.response(id)
    }

    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// The first output `wanted` picks, waited for under the HANG guard
    /// and naming `what` past it, keeping the rest for later takes.
    fn take(&mut self, what: &str, wanted: impl Fn(&Out) -> bool) -> Out {
        if let Some(i) = self.pending.iter().position(&wanted) {
            return self.pending.remove(i);
        }
        loop {
            let out = recv_within(what, &self.rx);
            if wanted(&out) {
                return out;
            }
            self.pending.push(out);
        }
    }

    /// The response to request `id`, keeping notifications that arrive
    /// meanwhile.
    fn response(&mut self, id: u64) -> Value {
        match self.take(
            &format!("the response to request {id}"),
            |o| matches!(o, Out::Shim(v) if v["id"] == id),
        ) {
            Out::Shim(v) => v,
            _ => unreachable!("take gives what it was asked for"),
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

    /// The next channel event's params, keeping other messages.
    fn next_event(&mut self) -> Value {
        match self.take("a channel event", is_event) {
            Out::Shim(v) => v["params"].clone(),
            _ => unreachable!("take gives what it was asked for"),
        }
    }

    /// The channel events already received, taken.
    fn events_so_far(&mut self) -> Vec<Value> {
        while let Ok(out) = self.rx.try_recv() {
            self.pending.push(out);
        }
        let (events, rest): (Vec<Out>, Vec<Out>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(is_event);
        self.pending = rest;
        events
            .into_iter()
            .map(|o| match o {
                Out::Shim(v) => v["params"].clone(),
                _ => unreachable!("partitioned on is_event"),
            })
            .collect()
    }

    /// Every line the follower printed until it closed its stdout: once it
    /// has exited, or been killed.
    fn follower_lines_to_the_end(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            match self.take("the end of the follower's output", |o| {
                matches!(o, Out::Follower(_) | Out::FollowerEnd)
            }) {
                Out::Follower(l) => lines.push(l),
                _ => return lines,
            }
        }
    }

    /// The ID of the shim's live session.
    fn session_id(&self) -> String {
        let live = self.get("/api/sessions?live=true");
        live["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["harness_session_id"] == "fake-sess")
            .and_then(|s| s["id"].as_str())
            .unwrap_or_else(|| panic!("{live}"))
            .to_string()
    }

    /// Returns once `ready` holds of the daemon's answer to `path`, asked
    /// again every 50 ms.
    #[expect(
        clippy::disallowed_methods,
        reason = "until_get polls the daemon under the HANG guard; nothing is decided by how long it takes"
    )]
    fn until_get(&self, path: &str, ready: impl Fn(&Value) -> bool) {
        let end = Instant::now() + HANG;
        loop {
            let v = self.get(path);
            if ready(&v) {
                return;
            }
            assert!(Instant::now() < end, "{path}: {v} (hang guard)");
            std::thread::sleep(Duration::from_millis(50));
        }
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
                clax_core::daemon::probe_host(&info.bind),
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
    /// lines among the shim's output ([`Out::Follower`], then
    /// [`Out::FollowerEnd`]). One follower at a time.
    fn follow(&self, poll_secs: u64) -> Child {
        let mut cmd = Command::new(clax_bin(None));
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
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if tx.send(Out::Follower(line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Out::FollowerEnd);
        });
        child
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        let mut stop = Command::new(clax_bin(None));
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
        .text("anchor", clax_testkit::element_anchor().to_string())
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

/// Whether `o` is a channel event from the shim.
fn is_event(o: &Out) -> bool {
    matches!(o, Out::Shim(v) if v["method"] == "notifications/claude/channel")
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
    // Launched without the channel: no notice will come, and the
    // instructions say so instead of describing notices.
    let i = r["result"]["instructions"].as_str().unwrap();
    assert!(!i.contains("<channel source="), "{i}");
    assert!(
        i.contains("not launched with the Clax channel") && i.contains("`wake`"),
        "{i}"
    );
    let mut c = FakeClaude::launch(FLAG);
    let r = c.initialize("2025-06-18");
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
    let events = [c.next_event()];
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
    // Announced once: the next event is the next comment's.
    let next = send_comment(&c, &aid, "and the footer");
    assert_eq!(c.next_event()["meta"]["thread_id"], next.as_str());
}

#[test]
fn a_channel_notice_delivers_nothing() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let tid = send_comment(&c, &aid, "please make it blue");
    assert_eq!(c.next_event()["meta"]["thread_id"], tid.as_str());
    // Over REST: any tool call would deliver the comment (tier 1).
    let sid = c.session_id();
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
    let mut follower = c.follow(2);
    // `--once`: one line, then it exits.
    let lines = c.follower_lines_to_the_end();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(&tid), "{lines:?}");
    let status = follower.wait().unwrap();
    assert!(status.success(), "{status:?}");
    // A response follows every event the shim sent before it: none.
    c.call("status", json!({}));
    assert_eq!(c.events_so_far(), Vec::<Value>::new());
}

/// Who announced a comment first.
enum Announced {
    Channel(Value),
    Follower(String),
}

#[test]
fn a_comment_is_announced_once_across_channel_and_follow() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let sid = c.session_id();
    for text in ["first", "second"] {
        let mut follower = c.follow(1);
        // Both poll: the session's push is the follower's.
        c.until_get(&format!("/api/sessions/{sid}"), |v| {
            v["push"]["tier"] == "notice"
        });
        let tid = send_comment(&c, &aid, text);
        let first = match c.take(&format!("{text}: announced"), |o| {
            is_event(o) || matches!(o, Out::Follower(_))
        }) {
            Out::Shim(v) => Announced::Channel(v["params"].clone()),
            Out::Follower(l) => Announced::Follower(l),
            Out::FollowerEnd => unreachable!("take gives what it was asked for"),
        };
        match first {
            Announced::Channel(e) => {
                assert_eq!(e["meta"]["thread_id"], tid.as_str(), "{text}: {e}");
                let _ = follower.kill();
                let lines = c.follower_lines_to_the_end();
                assert!(!lines.iter().any(|l| l.contains(&tid)), "{text}: {lines:?}");
            }
            Announced::Follower(l) => {
                assert!(l.contains(&tid), "{text}: {l}");
                assert_eq!(
                    c.follower_lines_to_the_end(),
                    Vec::<String>::new(),
                    "{text}"
                );
            }
        }
        let _ = follower.wait();
    }
    // Neither was announced again on the channel: the next event is a new
    // comment's.
    let last = send_comment(&c, &aid, "third");
    assert_eq!(c.next_event()["meta"]["thread_id"], last.as_str());
}

#[test]
fn no_channel_event_while_waiting_for_feedback() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let aid = c.publish();
    let sid = c.session_id();
    // The longest wait: the comment ends it.
    let id = c.send_request(
        "tools/call",
        json!({"name": "wait_for_feedback", "arguments": {"timeout_s": 600}}),
    );
    c.until_get(
        &format!("/api/_test/sessions/{sid}/feedback/waiters"),
        |v| v["count"] == 1,
    );
    let tid = send_comment(&c, &aid, "and the footer");
    let r = c.response(id);
    let v = FakeClaude::result_body(&r);
    assert_eq!(v["feedback"].as_array().map(Vec::len), Some(1), "{v}");
    assert!(v.to_string().contains(&tid), "{v}");
    // The channel took no notice of it: the next event is the next
    // comment's.
    let next = send_comment(&c, &aid, "and the header");
    assert_eq!(c.next_event()["meta"]["thread_id"], next.as_str());
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
    let event = c.next_event();
    assert_eq!(event["meta"]["thread_id"], tid.as_str(), "{event}");
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
