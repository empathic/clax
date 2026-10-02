//! End-to-end tests of `clax feedback follow` against a daemon in a
//! temporary `CLAX_HOME`.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// The harness environment a test inherits from the agent running it; every
/// `clax` run here starts without it.
const HARNESS_ENV: [&str; 12] = [
    "GROK_SESSION_ID",
    "GROK_HOOK_EVENT",
    "GROK_PLUGIN_ROOT",
    "GROK_HOME",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "PI_CODING_AGENT_DIR",
    "CLAX_SESSION_ID",
];

fn clax(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_clax"));
    for var in HARNESS_ENV {
        c.env_remove(var);
    }
    c.env("CLAX_HOME", home)
        .env("HOME", home.parent().unwrap())
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
    c
}

/// A daemon in a temp home; stopped on drop.
struct Daemon {
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let d = Daemon {
            dir: tempfile::tempdir().unwrap(),
        };
        d.serve();
        d
    }
    fn serve(&self) {
        let st = clax(&self.home())
            .args(["--port", "0", "serve"])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
    }
    fn stop(&self) {
        let st = clax(&self.home())
            .arg("stop")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    fn info(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.home().join("daemon.json")).unwrap()).unwrap()
    }
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.info()["port"])
    }
    fn token(&self) -> String {
        self.info()["token"].as_str().unwrap().to_string()
    }
    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }

    /// Registers a Grok session with harness session ID `hsid`; returns its ID.
    fn grok_session(&self, hsid: &str) -> String {
        let s: Value = self
            .http()
            .post(format!("{}/api/sessions", self.base()))
            .bearer_auth(self.token())
            .json(&json!({"harness": "grok", "harness_session_id": hsid, "cwd": "/tmp/project"}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        s["session"]["id"].as_str().unwrap().to_string()
    }

    /// Publishes as `sid`, which watches the artifact with replies armed;
    /// returns the artifact ID.
    fn publish_as(&self, sid: &str) -> String {
        let a: Value = self
            .http()
            .post(format!("{}/api/artifacts", self.base()))
            .bearer_auth(self.token())
            .header("x-clax-session", sid)
            .json(&json!({"title": "Followed", "files": {"index.html": {"content": "<h2>Goals</h2>", "encoding": "utf8"}}}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        a["artifact"]["id"].as_str().unwrap().to_string()
    }

    /// Opens a thread sent to the agent (its body mentions @agent); returns
    /// its ID.
    fn sent_thread(&self, aid: &str, body: &str) -> String {
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
        let t: Value = res.json().unwrap();
        t["thread"]["id"]
            .as_str()
            .or(t["id"].as_str())
            .unwrap()
            .to_string()
    }

    fn end(&self, sid: &str) {
        let res = self
            .http()
            .patch(format!("{}/api/sessions/{sid}", self.base()))
            .bearer_auth(self.token())
            .json(&json!({"ended": true}))
            .send()
            .unwrap();
        assert_eq!(res.status(), 200);
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

/// A running `clax feedback follow` whose stdout lines arrive on a channel.
struct Follower {
    child: Child,
    lines: Receiver<String>,
}

impl Follower {
    fn spawn(d: &Daemon, args: &[&str], env: &[(&str, &str)]) -> Follower {
        let mut cmd = clax(&d.home());
        cmd.args(["feedback", "follow"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Follower { child, lines }
    }

    fn line(&self, within: Duration) -> String {
        self.lines
            .recv_timeout(within)
            .expect("a line from the follower")
    }

    fn quiet_for(&self, d: Duration) {
        match self.lines.recv_timeout(d) {
            Err(RecvTimeoutError::Timeout) => {}
            other => panic!("unexpected output: {other:?}"),
        }
    }

    /// Waits up to `within` for the process to exit; its exit code.
    fn exit(&mut self, within: Duration) -> Option<i32> {
        let start = Instant::now();
        while start.elapsed() < within {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the follower did not exit within {within:?}");
    }

    fn stderr(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut s = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_notice(line: &str, tid: &str) {
    assert!(line.starts_with("[clax] New comment on "), "{line}");
    assert!(line.contains(tid), "{line}");
}

#[test]
fn it_prints_one_line_per_new_comment_and_nothing_else() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    let aid = d.publish_as(&sid);
    let mut f = Follower::spawn(
        &d,
        &[
            "--agent",
            "grok",
            "--harness-session",
            "g1",
            "--poll-secs",
            "2",
        ],
        &[],
    );
    let t1 = d.sent_thread(&aid, "first");
    let line = f.line(Duration::from_secs(5));
    assert_notice(&line, &t1);
    assert!(
        !line.contains("first"),
        "the line never carries the comment"
    );
    f.quiet_for(Duration::from_millis(500));
    let t2 = d.sent_thread(&aid, "second");
    assert_notice(&f.line(Duration::from_secs(5)), &t2);
    f.quiet_for(Duration::from_secs(3));
    assert_eq!(f.stderr(), "");
}

#[test]
fn it_finds_the_grok_session_from_the_environment() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    let aid = d.publish_as(&sid);
    let mut f = Follower::spawn(&d, &["--poll-secs", "2"], &[("GROK_SESSION_ID", "g1")]);
    let tid = d.sent_thread(&aid, "hello");
    assert_notice(&f.line(Duration::from_secs(5)), &tid);
    assert_eq!(f.stderr(), "");
}

#[test]
fn it_survives_a_daemon_restart() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    d.publish_as(&sid);
    let f = Follower::spawn(
        &d,
        &[
            "--agent",
            "grok",
            "--harness-session",
            "g1",
            "--poll-secs",
            "2",
        ],
        &[],
    );
    std::thread::sleep(Duration::from_millis(500));
    d.stop();
    d.serve();
    let sid = d.grok_session("g1");
    let aid = d.publish_as(&sid);
    let tid = d.sent_thread(&aid, "after the restart");
    assert_notice(&f.line(Duration::from_secs(10)), &tid);
}

#[test]
fn it_exits_cleanly_when_the_session_ends() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    d.publish_as(&sid);
    let mut f = Follower::spawn(
        &d,
        &[
            "--agent",
            "grok",
            "--harness-session",
            "g1",
            "--poll-secs",
            "2",
            "--grace-secs",
            "1",
        ],
        &[],
    );
    std::thread::sleep(Duration::from_millis(500));
    d.end(&sid);
    assert_eq!(f.exit(Duration::from_secs(5)), Some(0));
    assert!(f.lines.try_recv().is_err(), "printed nothing");
}

#[test]
fn it_exits_at_once_when_a_followed_clax_session_ends() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    let mut f = Follower::spawn(&d, &["--session", &sid], &[]);
    std::thread::sleep(Duration::from_millis(500));
    let ended = Instant::now();
    d.end(&sid);
    assert_eq!(f.exit(Duration::from_secs(3)), Some(0));
    assert!(ended.elapsed() < Duration::from_secs(3));
    assert!(f.lines.try_recv().is_err(), "printed nothing");
}

#[test]
fn without_a_session_it_is_a_usage_error() {
    let d = Daemon::start();
    let out = clax(&d.home())
        .args(["feedback", "follow"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8(out.stderr).unwrap();
    for name in [
        "--session",
        "--agent",
        "--harness-session",
        "GROK_SESSION_ID",
    ] {
        assert!(err.contains(name), "{err}");
    }
}
