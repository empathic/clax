//! End-to-end tests of `artifax hook`: the built binary is run with fixture
//! stdin against a daemon in a temporary `ARTIFAX_HOME`.

use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The `artifax` binary, built once per test run (see the MCP shim tests).
fn artifax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = Command::new(env!("CARGO"));
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
        assert!(cmd.status().expect("run cargo build").success());
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("artifax");
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

fn artifax(home: &Path) -> Command {
    let mut c = Command::new(artifax_bin());
    c.env("ARTIFAX_HOME", home)
        .env("ARTIFAX_CODEX_BIN", "")
        .env("ARTIFAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
    c
}

struct Ran {
    stdout: String,
    code: Option<i32>,
    elapsed: Duration,
}

fn hook(home: &Path, agent: &str, event: &str, stdin: &[u8]) -> Ran {
    let mut child = artifax(home)
        .args(["hook", "--agent", agent, event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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
        let st = artifax(&dir.path().join("ax"))
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
        let _ = artifax(&self.home())
            .arg("stop")
            .stdout(Stdio::null())
            .status();
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
        for event in ["session-start", "session-end"] {
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
    for event in ["session-start", "session-end"] {
        let r = hook(
            &home,
            "claude",
            event,
            &fixture("claude-session-start.json"),
        );
        assert_eq!(r.code, Some(0));
        assert_eq!(r.stdout, "");
        assert!(r.elapsed < Duration::from_secs(5));
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
            artifax_bin().display()
        ))
        .env("ARTIFAX_HOME", d.home())
        .env("ARTIFAX_CODEX_BIN", "")
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
            .contains("Artifax daemon")
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
        artifax(&home)
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
