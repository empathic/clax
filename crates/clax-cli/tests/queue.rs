//! End-to-end tests of `clax queue` against a daemon in a temporary
//! `CLAX_HOME` on a free port: reports are filed into a queue the owner
//! administers through the store, as the reporter routes would file them.

use clax_core::audit::AuditCtx;
use clax_core::queue::ReportersSee;
use clax_store::queues::{ExternalScope, NewQueue, NewReport, ReporterToken};
use clax_testkit::{FAKE_PNG, element_anchor};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// The harness environment a test inherits from the agent running it; every
/// `clax` run here starts without it.
const HARNESS_ENV: [&str; 19] = [
    "CLAX_CHANNEL",
    "CLAX_QUEUE",
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_CI",
    "PI_CODING_AGENT",
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

/// A daemon in a temp home with one queue; stopped on drop.
struct Daemon {
    dir: tempfile::TempDir,
    queue: String,
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
        let mut d = Daemon {
            dir,
            queue: String::new(),
        };
        d.queue = d
            .store()
            .create_queue(
                &AuditCtx::DAEMON,
                None,
                NewQueue {
                    name: "App reports".into(),
                    origins: vec!["https://app.example.com".into()],
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
        d
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    fn store(&self) -> clax_store::Store {
        clax_store::Store::open(&clax_core::Home::at(self.home())).unwrap()
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

    /// Files a report into the queue as reporter `emp-1` would, with a
    /// context naming release 0.6.1, a clip and two diagnostics lines;
    /// returns its ID.
    fn file(&self, title: &str, body: &str) -> String {
        let token = ReporterToken {
            queue_id: self.queue.clone(),
            service_grant_id: "g1".into(),
            reporter_id: "emp-1".into(),
            origin: "https://app.example.com".into(),
            public_key: String::new(),
            expires_at: "2099-01-01T00:00:00.000Z".into(),
            profile: None,
        };
        let context = clax_core::report::parse_context(
            &json!({
                "v": 1,
                "captured_at": "2026-10-29T12:00:00Z",
                "release": {"version": "0.6.1", "commit": "abc123"},
                "flags": {"new_totals": true},
                "environment": {"url": "https://app.example.com/invoices/4821",
                    "viewport": {"w": 1280, "h": 800, "dpr": 2}},
                "capture": {"mode": "element", "method": "dom"},
            })
            .to_string(),
        )
        .unwrap();
        let diagnostics = clax_core::report::parse_diagnostics(
            concat!(
                r#"{"at":"2026-10-29T11:59:58Z","level":"error","kind":"console","text":"total mismatch"}"#,
                "\n",
                r#"{"at":"2026-10-29T11:59:59Z","level":"warn","kind":"request","text":"GET /api/invoices → 500 in 12 ms"}"#,
                "\n",
            )
            .as_bytes(),
            "2026-10-29T12:00:00Z",
        )
        .unwrap();
        self.store()
            .file_report(
                &AuditCtx::DAEMON,
                &self.queue,
                Some(&token),
                ExternalScope::Instance,
                NewReport {
                    external_id: Some(format!("invoice:{title}")),
                    url: "https://app.example.com/invoices/4821".into(),
                    title: title.into(),
                    anchor: serde_json::from_value(element_anchor()).unwrap(),
                    body: body.into(),
                    author_name: "emp-1".into(),
                    metadata: Value::Null,
                    revision: None,
                    clip: Some(FAKE_PNG.to_vec()),
                    context: Some(context),
                    server: None,
                    diagnostics: Some(diagnostics),
                },
            )
            .unwrap()
            .id
    }

    /// Registers a live Claude Code session; returns its ID.
    fn session(&self, hsid: &str) -> String {
        let v: Value = self
            .http()
            .post(format!("{}/api/sessions", self.base()))
            .bearer_auth(self.token())
            .json(&json!({"harness": "claude", "harness_session_id": hsid, "cwd": "/w"}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        v["session"]["id"].as_str().unwrap().to_string()
    }

    /// Whether a follower of session `sid` has polled its notices.
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

    fn run(&self, args: &[&str]) -> Output {
        clax(&self.home()).args(args).output().unwrap()
    }

    /// Runs `clax queue <args>` with `--queue` from `CLAX_QUEUE`, which must
    /// succeed; its stdout.
    fn queue(&self, args: &[&str]) -> String {
        let out = clax(&self.home())
            .env("CLAX_QUEUE", &self.queue)
            .arg("queue")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "clax queue {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// [`Daemon::queue`] with `--json`: its object.
    fn queue_json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.queue(&all);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
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

fn ids(v: &Value) -> Vec<&str> {
    v["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect()
}

#[test]
fn ls_and_show_json_carry_context() {
    let d = Daemon::start();
    let tid = d.file("Invoice 4821", "The total is wrong");

    let v = d.queue_json(&["ls"]);
    assert_eq!(v["queue"], d.queue.as_str());
    assert_eq!(ids(&v), [tid.as_str()]);
    let r = &v["reports"][0];
    assert_eq!(r["state"], "open");
    assert_eq!(r["context_summary"]["release"]["version"], "0.6.1", "{r}");
    assert_eq!(r["context_summary"]["mode"], "element");

    let text = d.queue(&["ls"]);
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
    assert_eq!(header, ["ID", "STATE", "RELEASE", "AGE", "NOTE"], "{text}");
    let row = lines.next().unwrap();
    assert!(row.starts_with(&tid), "{row}");
    assert!(
        row.contains("open") && row.contains("0.6.1") && row.contains("The total is wrong"),
        "{row}"
    );

    let v = d.queue_json(&["show", &tid]);
    assert_eq!(v["report"]["context"]["release"]["commit"], "abc123", "{v}");
    assert_eq!(v["report"]["context"]["flags"]["new_totals"], true);
    assert_eq!(v["diagnostics"]["capture"], 0);
    assert_eq!(v["diagnostics"]["lines"].as_array().unwrap().len(), 2);
    assert_eq!(v["clip"]["capture"], 0);
    assert!(v.get("feedback").is_none(), "{v}");
    let text = d.queue(&["show", &tid]);
    assert!(
        text.contains("release 0.6.1") && text.contains("The total is wrong"),
        "{text}"
    );

    let v = d.queue_json(&["diagnostics", &tid]);
    assert_eq!(v["capture"], 0);
    assert_eq!(v["lines"][0]["text"], "total mismatch", "{v}");
    let text = d.queue(&["diagnostics", &tid]);
    assert!(text.contains("error  console  total mismatch"), "{text}");

    // Without a queue, `ls` lists the queues.
    let out = d.run(&["queue", "ls", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["queues"][0]["id"], d.queue.as_str(), "{v}");
}

#[test]
fn claim_fix_then_ls_shows_fix_proposed() {
    let d = Daemon::start();
    let tid = d.file("Invoice 4821", "The total is wrong");
    let sid = d.session("fixer");

    // By hand, with no session, a claim is refused and says what to pass.
    let out = clax(&d.home())
        .args(["queue", "claim", &tid, "--queue", &d.queue])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no_session") && err.contains("--session"),
        "{err}"
    );

    let v = d.queue_json(&["claim", &tid, "--session", &sid]);
    assert_eq!(v["report"]["state"], "claimed", "{v}");
    assert_eq!(v["fence"], 1);
    let text = d.queue(&[
        "renew",
        &tid,
        "--agent",
        "claude",
        "--harness-session",
        "fixer",
    ]);
    assert!(text.starts_with(&format!("renewed {tid} until ")), "{text}");
    let v = d.queue_json(&["reply", &tid, "Looking at it", "--session", &sid]);
    assert_eq!(v["comment"]["author_kind"], "agent", "{v}");

    let v = d.queue_json(&[
        "fix",
        &tid,
        "--session",
        &sid,
        "--summary",
        "Totals now include tax",
        "--url",
        "https://github.com/acme/app/pull/7",
        "--commit",
        "def456",
        "--verify-url",
        "https://app.example.com/invoices/4821",
    ]);
    assert_eq!(v["report"]["state"], "fix_proposed", "{v}");
    assert_eq!(v["report"]["fixes"][0]["commit"], "def456");

    assert!(ids(&d.queue_json(&["ls"])).is_empty());
    let v = d.queue_json(&["ls", "--state", "fix_proposed"]);
    assert_eq!(ids(&v), [tid.as_str()]);
    assert_eq!(
        ids(&d.queue_json(&["ls", "--state", "all"])),
        [tid.as_str()]
    );
    let text = d.queue(&["ls", "--state", "fix_proposed"]);
    assert!(
        text.lines().nth(1).unwrap().contains("fix_proposed"),
        "{text}"
    );
}

#[test]
fn clip_writes_the_png() {
    let d = Daemon::start();
    let tid = d.file("Invoice 4821", "The total is wrong");
    let out_dir = tempfile::tempdir().unwrap();

    let path = out_dir.path().join("clip.png");
    let text = d.queue(&["clip", &tid, "-o", path.to_str().unwrap()]);
    assert!(text.contains("capture 0"), "{text}");
    assert_eq!(std::fs::read(&path).unwrap(), FAKE_PNG);

    // By default, `<report>.png` in the working directory.
    let out = clax(&d.home())
        .current_dir(out_dir.path())
        .args(["queue", "clip", &tid, "--queue", &d.queue, "--capture", "0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(out_dir.path().join(format!("{tid}.png"))).unwrap(),
        FAKE_PNG
    );

    // A capture that has no clip is refused.
    let out = d.run(&["queue", "clip", &tid, "--queue", &d.queue, "--capture", "1"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no_clip"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn resolve_with_reason() {
    let d = Daemon::start();
    let tid = d.file("Invoice 4821", "The total is wrong");
    let sid = d.session("fixer");
    d.queue(&["claim", &tid, "--session", &sid]);

    let out = d.run(&[
        "queue",
        "resolve",
        &tid,
        "--queue",
        &d.queue,
        "--session",
        &sid,
        "--reason",
        "nope",
    ]);
    assert!(!out.status.success(), "an unknown reason is a usage error");

    let v = d.queue_json(&[
        "resolve",
        &tid,
        "--session",
        &sid,
        "--reason",
        "cannot_reproduce",
    ]);
    assert_eq!(v["report"]["state"], "resolved", "{v}");
    assert_eq!(v["report"]["resolve_reason"], "cannot_reproduce");
    let v = d.queue_json(&["ls", "--state", "resolved"]);
    assert_eq!(ids(&v), [tid.as_str()]);
}

/// A running `clax queue follow` whose stdout lines arrive on a channel.
struct Follower {
    child: Child,
    lines: Receiver<String>,
}

impl Follower {
    fn spawn(d: &Daemon, args: &[&str]) -> Follower {
        let mut child = clax(&d.home())
            .args(["queue", "follow"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
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
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn follow_prints_each_filed_report_as_a_json_line() {
    let d = Daemon::start();
    let sid = d.session("fixer");
    let mut f = Follower::spawn(
        &d,
        &[&d.queue, "--session", &sid, "--once", "--poll-secs", "1"],
    );
    // The follower watches the queue before its first poll.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !d.following(&sid) {
        assert!(Instant::now() < deadline, "no follower of {sid} polled");
        std::thread::sleep(Duration::from_millis(50));
    }
    let tid = d.file("Invoice 4821", "The total is wrong");
    let line = f
        .lines
        .recv_timeout(Duration::from_secs(30))
        .expect("a line from the follower");
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["event"], "filed", "{v}");
    assert_eq!(v["queue"], d.queue.as_str());
    assert_eq!(v["report_id"], tid.as_str());
    assert_eq!(v["title"], "Invoice 4821");
    assert!(v["url"].as_str().unwrap().starts_with("http"), "{v}");
    assert!(v.get("reopened_by").is_none());
    let st = f.child.wait().unwrap();
    assert!(st.success(), "--once exits 0 after the line");
}
