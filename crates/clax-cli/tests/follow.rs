//! End-to-end tests of `clax feedback follow` against a daemon in a
//! temporary `CLAX_HOME`.

use clax_server::routes::feedback::MAX_WAIT_SECS;
use clax_testkit::{HANG, recv_within};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// A viewer cookie of a browser of this machine's with no name, its viewer
/// minted in the store of the home at `home` (reaching the daemon's port
/// alone mints none).
fn browser_cookie(home: &std::path::Path) -> String {
    let cookie = clax_core::secret::viewer_cookie().expose().to_string();
    clax_store::Store::open(&clax_core::Home::at(home.to_path_buf()))
        .unwrap()
        .mint_viewer(&cookie, true)
        .unwrap();
    format!("clax_viewer={cookie}")
}

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
    let mut c = Command::new(crate::common::clax());
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

    /// Registers a Claude Code session with harness session ID `hsid`;
    /// returns its ID.
    fn claude_session(&self, hsid: &str) -> String {
        let s: Value = self
            .http()
            .post(format!("{}/api/sessions", self.base()))
            .bearer_auth(self.token())
            .json(&json!({"harness": "claude", "harness_session_id": hsid, "cwd": "/tmp/project"}))
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
            .header("cookie", browser_cookie(&self.home()))
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

    /// Whether a `clax feedback follow` of `sid` has begun a notices poll:
    /// the session's push reports the follower (Grok's monitor, Claude
    /// Code's notice tier).
    fn following(&self, sid: &str) -> bool {
        let s: Value = self
            .http()
            .get(format!("{}/api/sessions/{sid}", self.base()))
            .bearer_auth(self.token())
            .send()
            .unwrap()
            .json()
            .unwrap();
        let push = &s["push"];
        push["tier"] == "notice" || (push["tier"] == "monitor" && push["available"] == true)
    }

    /// Waits until a follower of `sid` polls.
    fn wait_following(&self, sid: &str) {
        crate::common::wait_until(&format!("a follower of {sid} to poll"), || {
            self.following(sid).then_some(())
        });
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
    /// Its stdout's lines, then `None` once it closes.
    lines: Receiver<Option<String>>,
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
                if tx.send(Some(line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(None);
        });
        Follower { child, lines }
    }

    /// Its next line.
    fn line(&self) -> String {
        recv_within("a line from the follower", &self.lines)
            .expect("a line from the follower, not the end of its output")
    }

    /// Waits for the process to exit; its exit code.
    fn exit(&mut self) -> Option<i32> {
        crate::common::wait_until("the follower to exit", || {
            self.child.try_wait().unwrap().map(|st| st.code())
        })
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
    let line = f.line();
    assert_notice(&line, &t1);
    assert!(
        !line.contains("first"),
        "the line never carries the comment"
    );
    // Each next line is the next comment's: nothing else came between.
    let t2 = d.sent_thread(&aid, "second");
    assert_notice(&f.line(), &t2);
    let t3 = d.sent_thread(&aid, "third");
    assert_notice(&f.line(), &t3);
    assert_eq!(f.stderr(), "");
}

#[test]
fn it_finds_the_grok_session_from_the_environment() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    let aid = d.publish_as(&sid);
    let mut f = Follower::spawn(&d, &["--poll-secs", "2"], &[("GROK_SESSION_ID", "g1")]);
    let tid = d.sent_thread(&aid, "hello");
    assert_notice(&f.line(), &tid);
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
    d.wait_following(&sid);
    d.stop();
    d.serve();
    let sid = d.grok_session("g1");
    let aid = d.publish_as(&sid);
    let tid = d.sent_thread(&aid, "after the restart");
    assert_notice(&f.line(), &tid);
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
    d.wait_following(&sid);
    d.end(&sid);
    assert_eq!(f.exit(), Some(0));
    assert!(all_lines(&f).is_empty(), "printed nothing");
}

#[test]
fn it_exits_at_once_when_a_followed_clax_session_ends() {
    let d = Daemon::start();
    let sid = d.grok_session("g1");
    // A poll of the longest wait the daemon allows, past the hang guard:
    // an exit before then is the daemon ending the poll when the session
    // ends.
    assert!(Duration::from_secs(MAX_WAIT_SECS) > HANG);
    let poll = MAX_WAIT_SECS.to_string();
    let mut f = Follower::spawn(&d, &["--session", &sid, "--poll-secs", &poll], &[]);
    d.wait_following(&sid);
    d.end(&sid);
    assert_eq!(f.exit(), Some(0));
    assert!(all_lines(&f).is_empty(), "printed nothing");
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

const ONCE_C1: [&str; 7] = [
    "--once",
    "--agent",
    "claude",
    "--harness-session",
    "c1",
    "--poll-secs",
    "2",
];

/// Every line a follower that has exited printed, to the end of its
/// output.
fn all_lines(f: &Follower) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(line) = recv_within("the end of the follower's output", &f.lines) {
        out.push(line);
    }
    out
}

#[test]
fn once_exits_after_the_first_poll_that_printed() {
    let d = Daemon::start();
    let sid = d.claude_session("c1");
    let aid = d.publish_as(&sid);
    let mut f = Follower::spawn(&d, &ONCE_C1, &[]);
    d.wait_following(&sid);
    let t1 = d.sent_thread(&aid, "first");
    let t2 = d.sent_thread(&aid, "second");
    assert_eq!(f.exit(), Some(0));
    let mut lines = all_lines(&f);
    assert!((1..=2).contains(&lines.len()), "{lines:?}");
    assert_eq!(f.stderr(), "");
    if lines.len() == 1 {
        let mut g = Follower::spawn(&d, &ONCE_C1, &[]);
        assert_eq!(g.exit(), Some(0));
        lines.extend(all_lines(&g));
        assert_eq!(g.stderr(), "");
    }
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(line.starts_with("[clax] New comment on "), "{line}");
    }
    assert!(lines.iter().any(|l| l.contains(&t1)), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains(&t2)), "{lines:?}");
}

#[test]
fn once_exits_cleanly_when_the_session_ends() {
    let d = Daemon::start();
    let sid = d.claude_session("c1");
    d.publish_as(&sid);
    let mut f = Follower::spawn(&d, &[&ONCE_C1[..], &["--grace-secs", "1"]].concat(), &[]);
    d.wait_following(&sid);
    d.end(&sid);
    assert_eq!(f.exit(), Some(0));
    assert!(all_lines(&f).is_empty(), "printed nothing");
}

#[test]
fn once_does_not_exit_on_an_empty_poll() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let polled = stand_in(&home);
    let mut child = clax(&home)
        .args([
            "feedback",
            "follow",
            "--once",
            "--session",
            "S1",
            "--poll-secs",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Each poll answers empty: a second poll is the follower still running
    // after an empty one.
    for n in ["a first", "a second"] {
        recv_within(&format!("{n} notices poll"), &polled);
    }
    assert!(child.try_wait().unwrap().is_none(), "still running");
    let _ = child.kill();
    let _ = child.wait();
}

/// A stand-in daemon in `home` whose notices route answers empty at once,
/// as the real one does while the session is inside `wait_for_feedback`;
/// every other route answers `{}`. Each notices poll arrives on the
/// channel. The binary has run once when it returns.
fn stand_in(home: &Path) -> Receiver<()> {
    use std::io::Write as _;
    use std::net::TcpListener;
    std::fs::create_dir_all(home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let info = json!({"port": port, "pid": std::process::id(), "token": "t",
        "started_at": "2026-10-08T00:00:00Z", "bind": "127.0.0.1", "version": "test"});
    std::fs::write(home.join("daemon.json"), info.to_string()).unwrap();
    let (polls, polled) = channel::<()>();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { return };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if conn.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head);
            let body = if line.contains("/notices") {
                if polls.send(()).is_err() {
                    return;
                }
                r#"{"notices":[],"lines":[],"waited_s":0}"#
            } else {
                "{}"
            };
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    // Run once first, unbounded, as the daemon-backed tests do through
    // `serve`: a new executable's first run can wait on the system's check.
    assert!(
        clax(home)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
    polled
}

/// `follow` keeps polling notices polls that answer empty at once. How
/// it paces them is a unit test's, on a clock it moves
/// (`follow_paces_polls_that_answer_empty_at_once` in
/// `clax_commands::commands::feedback`).
#[test]
fn it_keeps_polling_after_notices_polls_that_answer_empty_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let polled = stand_in(&home);
    let mut child = clax(&home)
        .args(["feedback", "follow", "--session", "S1", "--poll-secs", "50"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for n in ["a first", "a second", "a third"] {
        recv_within(&format!("{n} notices poll"), &polled);
    }
    assert!(child.try_wait().unwrap().is_none(), "still running");
    let _ = child.kill();
    let _ = child.wait();
}
