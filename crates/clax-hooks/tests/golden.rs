//! End-to-end tests of `clax hook`: the built binary is run with fixture
//! stdin against a daemon in a temporary `CLAX_HOME`.

use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The `clax` binary, built once per test run (see the MCP shim tests).
fn clax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = Command::new(env!("CARGO"));
        cmd.current_dir(&workspace)
            .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"]);
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        assert!(cmd.status().expect("run cargo build").success());
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("clax");
        assert!(bin.exists(), "{} missing", bin.display());
        bin
    })
    .clone()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

/// The harness environment a test inherits from the agent running it; every
/// `clax` run here starts without it.
const HARNESS_ENV: [&str; 9] = [
    "GROK_SESSION_ID",
    "GROK_HOOK_EVENT",
    "GROK_PLUGIN_ROOT",
    "GROK_HOME",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CLAX_SESSION_ID",
];

fn clax(home: &Path) -> Command {
    let mut c = Command::new(clax_bin());
    for var in HARNESS_ENV {
        c.env_remove(var);
    }
    c.env("CLAX_HOME", home)
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
    c
}

struct Ran {
    stdout: String,
    code: Option<i32>,
    elapsed: Duration,
}

fn hook(home: &Path, agent: &str, event: &str, stdin: &[u8]) -> Ran {
    hook_env(home, agent, event, stdin, &[])
}

fn hook_env(home: &Path, agent: &str, event: &str, stdin: &[u8], env: &[(&str, &str)]) -> Ran {
    let mut cmd = clax(home);
    cmd.args(["hook", "--agent", agent, event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let start = Instant::now();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    Ran {
        stdout: String::from_utf8(out.stdout).unwrap(),
        code: out.status.code(),
        elapsed: start.elapsed(),
    }
}

/// A daemon in a temp home; stopped on drop.
struct Daemon {
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let st = clax(&dir.path().join("ax"))
            .args(["--port", "0", "serve"])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
        Daemon { dir }
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    fn info(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.home().join("daemon.json")).unwrap()).unwrap()
    }
    fn sessions(&self, live: bool) -> Vec<Value> {
        let url = format!(
            "http://127.0.0.1:{}/api/sessions?live={live}",
            self.info()["port"]
        );
        let c = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let token = self.info()["token"].as_str().unwrap().to_string();
        let v: Value = c
            .get(url)
            .bearer_auth(token)
            .send()
            .unwrap()
            .json()
            .unwrap();
        v["sessions"].as_array().unwrap().clone()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = clax(&self.home())
            .arg("stop")
            .stdout(Stdio::null())
            .status();
    }
}

impl Daemon {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.info()["port"])
    }
    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }
    fn token(&self) -> String {
        self.info()["token"].as_str().unwrap().to_string()
    }

    /// Registers the session a shim would, then publishes as it (which
    /// watches the artifact, replies armed).
    fn session_with_artifact(&self, harness: &str, hsid: &str) -> (String, String) {
        self.registered_with_artifact(&serde_json::json!({"harness": harness, "harness_session_id": hsid, "cwd": "/tmp/project"}))
    }

    /// Registers a session with `body`, then publishes as it (which watches
    /// the artifact, replies armed).
    fn registered_with_artifact(&self, body: &Value) -> (String, String) {
        let s: Value = self
            .http()
            .post(format!("{}/api/sessions", self.base()))
            .bearer_auth(self.token())
            .json(body)
            .send()
            .unwrap()
            .json()
            .unwrap();
        let sid = s["session"]["id"].as_str().unwrap().to_string();
        let a: Value = self
            .http()
            .post(format!("{}/api/artifacts", self.base()))
            .bearer_auth(self.token())
            .header("x-clax-session", &sid)
            .json(&serde_json::json!({"title": "Hooked", "files": {"index.html": {"content": "<h2>Goals</h2>", "encoding": "utf8"}}}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        (sid, a["artifact"]["id"].as_str().unwrap().to_string())
    }

    /// The session's undelivered feedback, read without waiting (this
    /// delivers it).
    fn pending(&self, sid: &str) -> Vec<Value> {
        let v: Value = self
            .http()
            .get(format!(
                "{}/api/sessions/{sid}/feedback?tier=wait",
                self.base()
            ))
            .bearer_auth(self.token())
            .send()
            .unwrap()
            .json()
            .unwrap();
        v["feedback"].as_array().unwrap().clone()
    }

    fn session(&self, sid: &str) -> Value {
        let v: Value = self
            .http()
            .get(format!("{}/api/sessions/{sid}", self.base()))
            .bearer_auth(self.token())
            .send()
            .unwrap()
            .json()
            .unwrap();
        v["session"].clone()
    }

    /// A thread sent to the agent (its body mentions @agent).
    fn sent_thread(&self, aid: &str, body: &str) {
        let form = reqwest::blocking::multipart::Form::new()
            .text(
                "anchor",
                r#"{"kind":"element","selector":"body > h2","quote":"Goals"}"#,
            )
            .text("body", format!("@agent {body}"))
            .text("version", "1");
        let res = self
            .http()
            .post(format!("{}/api/artifacts/{aid}/threads", self.base()))
            .multipart(form)
            .send()
            .unwrap();
        assert_eq!(res.status(), 201);
    }
}

fn one_line_json(out: &str) -> Value {
    assert_eq!(out.lines().count(), 1, "{out:?}");
    serde_json::from_str(out).unwrap()
}

fn lifecycle(agent: &str, harness_session_id: &str) {
    let d = Daemon::start();
    let start = fixture(&format!("{agent}-session-start.json"));

    let r = hook(&d.home(), agent, "session-start", &start);
    assert_eq!(r.code, Some(0));
    let v = one_line_json(&r.stdout);
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let text = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let url = format!("http://localhost:{}/", d.info()["port"]);
    assert!(text.contains(&url), "{text} lacks {url}");

    let live = d.sessions(true);
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["harness"], agent);
    assert_eq!(live[0]["harness_session_id"], harness_session_id);
    assert_eq!(live[0]["parent_pid"], std::process::id());

    // Idempotent.
    let first_id = live[0]["id"].clone();
    let r = hook(&d.home(), agent, "session-start", &start);
    assert_eq!(r.code, Some(0));
    assert_eq!(d.sessions(true)[0]["id"], first_id);
    assert_eq!(d.sessions(true).len(), 1);

    let end = fixture(&format!("{agent}-session-end.json"));
    let r = hook(&d.home(), agent, "session-end", &end);
    assert_eq!(r.code, Some(0));
    assert_eq!(r.stdout, "");
    assert!(d.sessions(true).is_empty());
    let all = d.sessions(false);
    assert_eq!(all.len(), 1);
    assert!(all[0]["ended_at"].is_string(), "{all:?}");
}

#[test]
fn claude_session_lifecycle() {
    lifecycle("claude", "cc-hook-1");
}

#[test]
fn codex_session_lifecycle() {
    lifecycle("codex", "cx-hook-1");
}

#[test]
fn unusable_stdin_prints_nothing() {
    let d = Daemon::start();
    for name in ["malformed.json", "empty.json"] {
        for event in ["session-start", "session-end", "stop", "prompt"] {
            let r = hook(&d.home(), "claude", event, &fixture(name));
            assert_eq!(r.code, Some(0), "{name} {event}");
            assert_eq!(r.stdout, "", "{name} {event}");
            assert!(r.elapsed < Duration::from_secs(5), "{name} {event}");
        }
    }
    assert!(d.sessions(false).is_empty());
}

#[test]
fn no_daemon_prints_nothing_and_starts_none() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    for event in ["session-start", "session-end", "stop", "prompt"] {
        let r = hook(
            &home,
            "claude",
            event,
            &fixture("claude-session-start.json"),
        );
        assert_eq!(r.code, Some(0));
        assert_eq!(r.stdout, "");
    }
    // Each run gave up at once on finding no daemon, rather than waiting
    // out its deadline: its log line names that reason, not a timeout.
    let log = std::fs::read_to_string(home.join("logs/hooks.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 4, "{log}");
    for (line, event) in lines
        .iter()
        .zip(["session-start", "session-end", "stop", "prompt"])
    {
        assert!(line.contains(&format!("event={event} ")), "{line}");
        assert!(
            line.ends_with("stderr=\"no clax daemon is running\""),
            "{line}"
        );
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(!home.join("daemon.json").exists());
}

#[test]
fn hook_behind_a_wrapper_shell_joins_the_shim_row() {
    let d = Daemon::start();
    let c = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let info = d.info();
    let res = c
        .post(format!("http://127.0.0.1:{}/api/sessions", info["port"]))
        .bearer_auth(info["token"].as_str().unwrap())
        .json(&serde_json::json!({
            "harness": "codex",
            "cwd": "/tmp/project",
            "pid": 424242,
            "parent_pid": std::process::id(),
        }))
        .send()
        .unwrap();
    assert!(res.status().is_success());
    let shim_id = res.json::<Value>().unwrap()["session"]["id"].clone();

    // `; true` keeps the shell from exec-optimising, so the hook's parent is
    // the shell and the harness (this process) is its parent.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "'{}' hook --agent codex session-start; true",
            clax_bin().display()
        ))
        .env("CLAX_HOME", d.home())
        .env("CLAX_CODEX_BIN", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&fixture("codex-session-start.json"))
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Clax daemon")
    );

    let all = d.sessions(false);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["id"], shim_id);
    assert_eq!(all[0]["harness_session_id"], "cx-hook-1");
    assert_eq!(all[0]["pid"], 424242);
}

/// A home whose daemon.json names a server that answers `/healthz` and never
/// answers anything else.
fn stalled_daemon() -> tempfile::TempDir {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                if buf[..n].starts_with(b"GET /healthz") {
                    let body = r#"{"version":"0.0.0","pid":1,"started_at":"x"}"#;
                    let res = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(res.as_bytes());
                } else {
                    std::thread::sleep(Duration::from_secs(10));
                }
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("daemon.json"),
        serde_json::json!({
            "port": port,
            "pid": std::process::id(),
            "token": "t",
            "started_at": "2026-09-28T00:00:00Z",
            "bind": "127.0.0.1",
            "version": "0.0.0",
        })
        .to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn session_end_gives_up_within_codexs_three_second_cap() {
    let dir = stalled_daemon();
    let home = dir.path().join("ax");
    // The first run of a freshly built binary can be slow to start (macOS
    // checks it); keep that out of the timing.
    assert!(
        clax(&home)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
    let r = hook(
        &home,
        "codex",
        "session-end",
        &fixture("codex-session-end.json"),
    );
    assert_eq!(r.code, Some(0));
    assert_eq!(r.stdout, "");
    assert!(r.elapsed < Duration::from_millis(2900), "{:?}", r.elapsed);

    // Session start keeps its longer budget.
    let r = hook(
        &home,
        "codex",
        "session-start",
        &fixture("codex-session-start.json"),
    );
    assert_eq!(r.code, Some(0));
    assert!(
        r.elapsed >= Duration::from_millis(2900) && r.elapsed < Duration::from_millis(4500),
        "{:?}",
        r.elapsed
    );
}

fn stop_loop(agent: &str, hsid: &str) {
    let d = Daemon::start();
    let (_sid, aid) = d.session_with_artifact(agent, hsid);
    let stop = fixture(&format!("{agent}-stop.json"));
    let active = fixture(&format!("{agent}-stop-active.json"));

    let r = hook(&d.home(), agent, "stop", &stop);
    assert_eq!(
        (r.code, r.stdout.as_str()),
        (Some(0), ""),
        "nothing pending: allow the stop"
    );

    d.sent_thread(&aid, "make it two columns");
    let r = hook(&d.home(), agent, "stop", &stop);
    let v = one_line_json(&r.stdout);
    assert_eq!(v["decision"], "block");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason
            .starts_with("[clax] 1 comment sent to you:\n[clax] Comment sent to you on \"Hooked\""),
        "{reason}"
    );
    assert!(
        reason.contains("Viewer: \"@agent make it two columns\""),
        "{reason}"
    );
    assert!(r.elapsed < Duration::from_secs(5));

    let r = hook(&d.home(), agent, "stop", &active);
    assert_eq!(r.stdout, "", "stop_hook_active with nothing new: allow");

    d.sent_thread(&aid, "and the footer");
    let r = hook(&d.home(), agent, "stop", &active);
    assert_eq!(
        one_line_json(&r.stdout)["decision"],
        "block",
        "stop_hook_active with a new row: block once"
    );
    let r = hook(&d.home(), agent, "stop", &active);
    assert_eq!(r.stdout, "", "then allow");
}

#[test]
fn claude_stop_blocks_once_per_new_comment() {
    stop_loop("claude", "cc-hook-1");
}

#[test]
fn codex_stop_blocks_once_per_new_comment() {
    stop_loop("codex", "cx-hook-1");
}

#[test]
fn prompt_hook_adds_pending_feedback_even_without_armed_replies() {
    let d = Daemon::start();
    let (sid, aid) = d.session_with_artifact("claude", "cc-hook-1");
    let res = d
        .http()
        .put(format!("{}/api/sessions/{sid}/watches/{aid}", d.base()))
        .bearer_auth(d.token())
        .json(&serde_json::json!({"replies_armed": false}))
        .send()
        .unwrap();
    assert!(res.status().is_success());
    d.sent_thread(&aid, "tighten the spacing");
    let r = hook(&d.home(), "claude", "stop", &fixture("claude-stop.json"));
    assert_eq!(r.stdout, "", "unarmed: the Stop hook stays out of the way");
    let r = hook(
        &d.home(),
        "claude",
        "prompt",
        &fixture("claude-prompt.json"),
    );
    let v = one_line_json(&r.stdout);
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("tighten the spacing")
    );
    let r = hook(
        &d.home(),
        "claude",
        "prompt",
        &fixture("claude-prompt.json"),
    );
    assert_eq!(r.stdout, "", "delivered once");
}

#[test]
fn codex_session_start_records_codex_home() {
    let d = Daemon::start();
    let r = hook_env(
        &d.home(),
        "codex",
        "session-start",
        &fixture("codex-session-start.json"),
        &[("CODEX_HOME", "/tmp/cxh-golden")],
    );
    assert_eq!(r.code, Some(0));
    let id = d.sessions(true)[0]["id"].as_str().unwrap().to_string();
    let v: Value = d
        .http()
        .get(format!("{}/api/sessions/{id}", d.base()))
        .bearer_auth(d.token())
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(v["push"]["codex_home"], "/tmp/cxh-golden");
    assert_eq!(
        v["push"]["reason"], "Codex push is off: CLAX_CODEX_BIN is set empty",
        "the harness disables push"
    );
}

/// A Grok session as its shim registers it: keyed on `GROK_SESSION_ID`,
/// with no cwd yet and a parent that is not the hook's.
fn grok_session_with_artifact(d: &Daemon) -> (String, String) {
    d.registered_with_artifact(&serde_json::json!({
        "harness": "grok",
        "harness_session_id": "019a-grok-1",
        "cwd": "",
        "pid": std::process::id(),
        "parent_pid": 1,
    }))
}

#[test]
fn grok_session_start_joins_by_session_id_and_prints_nothing() {
    let d = Daemon::start();
    let (sid, aid) = grok_session_with_artifact(&d);
    d.sent_thread(&aid, "make it two columns");
    let r = hook(
        &d.home(),
        "grok",
        "session-start",
        &fixture("grok-session-start.json"),
    );
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    let live = d.sessions(true);
    assert_eq!(live.len(), 1, "joined, not inserted: {live:?}");
    assert_eq!(live[0]["id"], sid.as_str());
    assert_eq!(d.session(&sid)["cwd"], "/tmp/project");
    assert_eq!(d.pending(&sid).len(), 1, "session start delivers nothing");
}

#[test]
fn grok_stop_blocks_once_at_the_end_of_a_turn() {
    let d = Daemon::start();
    let (_sid, aid) = grok_session_with_artifact(&d);
    d.sent_thread(&aid, "make it two columns");
    let r = hook(&d.home(), "grok", "stop", &fixture("grok-stop.json"));
    assert_eq!(r.code, Some(0));
    let v = one_line_json(&r.stdout);
    assert_eq!(v["decision"], "block");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("make it two columns"), "{reason}");
    let r = hook(&d.home(), "grok", "stop", &fixture("grok-stop-active.json"));
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
}

#[test]
fn grok_stop_at_session_end_does_nothing() {
    let d = Daemon::start();
    let (sid, aid) = grok_session_with_artifact(&d);
    d.sent_thread(&aid, "make it two columns");
    let r = hook(
        &d.home(),
        "grok",
        "stop",
        &fixture("grok-stop-shutdown.json"),
    );
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    assert_eq!(d.pending(&sid).len(), 1, "still undelivered");
}

#[test]
fn grok_session_end_ends_the_row_within_its_budget() {
    let d = Daemon::start();
    let (sid, _aid) = grok_session_with_artifact(&d);
    let end = fixture("grok-session-end.json");
    let r = hook(&d.home(), "grok", "session-end", &end);
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    assert!(r.elapsed < Duration::from_millis(1500), "{:?}", r.elapsed);
    assert!(d.sessions(true).is_empty());
    assert!(d.session(&sid)["ended_at"].is_string());

    let dir = tempfile::tempdir().unwrap();
    let r = hook(&dir.path().join("ax"), "grok", "session-end", &end);
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    assert!(r.elapsed < Duration::from_millis(1500), "{:?}", r.elapsed);
}
