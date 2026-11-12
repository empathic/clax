//! A binary that finds an older daemon replaces it on the same port, holding
//! the start lock; a newer daemon is kept.

use assert_cmd::Command;
use clax_testkit::{HANG, recv_within};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

struct Fake {
    port: u16,
    child: Arc<Mutex<Child>>,
    shutdown_seen: Arc<AtomicBool>,
}

/// A test's scratch home. Dropping it (when the test ends, passing or
/// panicking) runs `clax stop` there, so a daemon a failing test started is
/// not left running.
struct Scratch(tempfile::TempDir);

impl Scratch {
    fn new() -> Scratch {
        Scratch(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = clax(self.path()).arg("stop").output();
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let mut c = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// A stand-in daemon of `version`: its PID is a `sleep` child, it answers
/// `/healthz`, and on `POST /api/admin/shutdown` it closes its listener and
/// kills the child, as a real daemon exits.
fn fake_daemon(home: &std::path::Path, version: &str) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let child = Arc::new(Mutex::new(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap(),
    ));
    let pid = child.lock().unwrap().id();
    let seen = Arc::new(AtomicBool::new(false));
    let (c2, s2, v) = (child.clone(), seen.clone(), version.to_string());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = format!(r#"{{"version":"{v}","pid":{pid},"started_at":"s"}}"#);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            if req.starts_with("POST /api/admin/shutdown") {
                s2.store(true, Ordering::SeqCst);
                drop(s);
                break;
            }
        }
        // As a daemon's exit does, free the port before the PID is gone:
        // the replacement binds once the PID has exited, and a port still
        // held then would move it to the next one.
        drop(listener);
        let mut c = c2.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    });
    std::fs::create_dir_all(home).unwrap();
    let info = serde_json::json!({
        "port": port, "pid": pid, "token": "t", "started_at": "s",
        "bind": "127.0.0.1", "version": version,
    });
    std::fs::write(home.join("daemon.json"), info.to_string()).unwrap();
    Fake {
        port,
        child,
        shutdown_seen: seen,
    }
}

fn clax(dir: &std::path::Path) -> Command {
    let mut c = Command::new(crate::common::clax());
    c.env("HOME", dir)
        .env("CLAX_HOME", dir.join("ax"))
        .env_remove("CLAX_CONFIG_DIR")
        .env("CLAX_CODEX_BIN", "");
    c
}

fn daemon_json(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("ax/daemon.json")).unwrap()).unwrap()
}

#[test]
fn serve_replaces_an_older_daemon_on_its_port() {
    let dir = Scratch::new();
    let fake = fake_daemon(&dir.path().join("ax"), "0.0.1");
    let out = clax(dir.path())
        .args(["serve", "--json", "--port", "0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        fake.shutdown_seen.load(Ordering::SeqCst),
        "the old daemon was asked to shut down"
    );
    let now = daemon_json(dir.path());
    assert_eq!(
        now["port"].as_u64().unwrap(),
        u64::from(fake.port),
        "same port"
    );
    assert_eq!(now["version"], env!("CARGO_PKG_VERSION"));
    let exe = std::fs::canonicalize(crate::common::clax()).unwrap();
    assert_eq!(now["exe"].as_str().unwrap(), exe.display().to_string());
    clax(dir.path()).arg("stop").assert().success();
}

#[test]
fn serve_keeps_a_newer_daemon() {
    let dir = Scratch::new();
    let fake = fake_daemon(&dir.path().join("ax"), "999.0.0");
    let out = clax(dir.path())
        .args(["serve", "--json", "--port", "0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!fake.shutdown_seen.load(Ordering::SeqCst));
    assert_eq!(daemon_json(dir.path())["version"], "999.0.0");
    drop(fake);
}

/// `clax serve` as a child process, for tests that act while it runs; its
/// standard error's lines arrive on `stderr`.
struct Serving {
    child: Child,
    stderr: Receiver<String>,
}

fn serve_child(dir: &std::path::Path) -> Serving {
    let mut child = std::process::Command::new(crate::common::clax())
        .args(["serve", "--json", "--port", "0"])
        .env("HOME", dir)
        .env("CLAX_HOME", dir.join("ax"))
        .env_remove("CLAX_CONFIG_DIR")
        .env("CLAX_CODEX_BIN", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let err = child.stderr.take().unwrap();
    let (tx, stderr) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    Serving { child, stderr }
}

/// Takes the home's start lock.
fn lock(dir: &std::path::Path) -> clax_core::daemon::DaemonLock {
    std::fs::create_dir_all(dir.join("ax")).unwrap();
    clax_core::daemon::DaemonLock::acquire(&clax_core::Home::at(dir.join("ax"))).unwrap()
}

/// What `clax serve` says from inside its wait for the start lock while
/// the lock's holder reports a daemon starting (`starting.json`).
const WAITING: &str = "waiting for the daemon's first start";

/// Returns once `s` is waiting for the start lock the test holds. The
/// holder reports a start under way, as a daemon starting under the lock
/// does (`starting.json`, naming this live process), so `serve` says
/// [`WAITING`] from inside its wait; the report is removed again before
/// this returns, so nothing after it sees a start under way.
fn wait_until_blocked_on_lock(dir: &std::path::Path, s: &mut Serving) {
    let starting = dir.join("ax/starting.json");
    let report = serde_json::json!({"pid": std::process::id(), "phase": "recording",
        "done": 0, "total": 1, "bytes": 0, "heartbeat_at": "t"});
    std::fs::write(&starting, report.to_string()).unwrap();
    while !recv_within("serve waiting for the start lock", &s.stderr).contains(WAITING) {}
    std::fs::remove_file(&starting).unwrap();
    assert!(
        s.child.try_wait().unwrap().is_none(),
        "serve waits for the lock"
    );
}

fn finish(s: Serving) -> serde_json::Value {
    let out = s.child.wait_with_output().unwrap();
    // Its standard error ends with the process.
    let stderr: Vec<String> = s.stderr.iter().collect();
    assert!(out.status.success(), "{}", stderr.join("\n"));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn serve_replaces_an_older_daemon_another_client_started_while_it_waited() {
    let dir = Scratch::new();
    let held = lock(dir.path());
    // No daemon yet: serve finds none and waits for the start lock.
    let mut serving = serve_child(dir.path());
    wait_until_blocked_on_lock(dir.path(), &mut serving);
    // An older binary wins the lock and starts its daemon.
    let fake = fake_daemon(&dir.path().join("ax"), "0.0.1");
    drop(held);
    let v = finish(serving);
    assert!(
        fake.shutdown_seen.load(Ordering::SeqCst),
        "the older daemon was replaced"
    );
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["port"].as_u64().unwrap(), u64::from(fake.port));
    clax(dir.path()).arg("stop").assert().success();
}

#[test]
fn replace_waits_for_the_start_lock_and_keeps_a_daemon_another_client_put_in_place() {
    let dir = Scratch::new();
    let old = fake_daemon(&dir.path().join("ax"), "0.0.1");
    let held = lock(dir.path());
    // serve finds the older daemon and waits for the lock to replace it.
    let mut serving = serve_child(dir.path());
    wait_until_blocked_on_lock(dir.path(), &mut serving);
    assert!(
        !old.shutdown_seen.load(Ordering::SeqCst),
        "nothing is stopped without the lock"
    );
    // Meanwhile another client replaced it with a newer daemon.
    let newer = fake_daemon(&dir.path().join("ax"), "999.0.0");
    drop(held);
    let v = finish(serving);
    assert_eq!(
        v["version"], "999.0.0",
        "the daemon found under the lock is used"
    );
    assert!(!old.shutdown_seen.load(Ordering::SeqCst));
    assert!(!newer.shutdown_seen.load(Ordering::SeqCst));
}

#[test]
fn serve_replaces_a_real_older_daemon_and_ends_its_event_streams() {
    let dir = Scratch::new();
    let home = dir.path().join("ax");
    let mut old = std::process::Command::new(crate::common::clax())
        .args([
            "serve",
            "--foreground",
            "--port",
            "0",
            "--report-version",
            "0.0.1",
        ])
        .env("HOME", dir.path())
        .env("CLAX_HOME", &home)
        .env("CLAX_CODEX_BIN", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let old_pid = old.id();
    let info = crate::common::wait_until("the old daemon to write daemon.json", || {
        let t = std::fs::read_to_string(home.join("daemon.json")).ok()?;
        let v = serde_json::from_str::<serde_json::Value>(&t).ok()?;
        (v["pid"].as_u64() == Some(u64::from(old_pid))).then_some(v)
    });
    assert_eq!(info["version"], "0.0.1");
    let port = info["port"].as_u64().unwrap() as u16;
    let mut sse = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let token = info["token"].as_str().unwrap().to_string();
    write!(
        sse,
        "GET /api/events?token={token} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"
    )
    .unwrap();
    sse.set_read_timeout(Some(HANG)).unwrap();
    let mut first = [0u8; 64];
    let n = sse.read(&mut first).unwrap();
    assert!(String::from_utf8_lossy(&first[..n]).starts_with("HTTP/1.1 200"));
    // Reap the old daemon as soon as it exits, as its real parent would.
    let reaped = std::thread::spawn(move || old.wait());

    let v = finish(serve_child(dir.path()));
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["port"].as_u64().unwrap(), u64::from(port), "same port");
    assert_ne!(v["pid"].as_u64().unwrap(), u64::from(old_pid));
    reaped.join().unwrap().unwrap();
    // The event stream ended rather than hanging.
    let mut rest = Vec::new();
    sse.read_to_end(&mut rest).expect("the event stream ends");
    let log = std::fs::read_to_string(home.join("logs/daemon.log")).unwrap();
    assert!(log.contains("replacing clax daemon v0.0.1"), "{log}");
    clax(dir.path()).arg("stop").assert().success();
}

/// Records, as a rolled-back upgrade does, that upgrading to this binary
/// failed just now.
#[expect(
    clippy::disallowed_methods,
    reason = "the CLI ages a failed upgrade on the system's clock, so it is recorded at the system's time"
)]
fn hold_this_binary(dir: &std::path::Path) {
    let exe = std::fs::canonicalize(crate::common::clax()).unwrap();
    let mtime = std::fs::metadata(&exe)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let logs = dir.join("ax/logs");
    std::fs::create_dir_all(&logs).unwrap();
    let rec = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exe": exe.display().to_string(),
        "mtime_ns": mtime.as_nanos().to_string(),
        "at": now.as_secs(),
        "from_version": "0.0.1",
        "reason": "the new clax daemon failed to start: it crashed",
    });
    std::fs::write(logs.join("failed-upgrade.json"), rec.to_string()).unwrap();
}

#[test]
fn serve_status_and_doctor_say_when_a_failed_upgrade_keeps_an_older_daemon() {
    let dir = Scratch::new();
    let fake = fake_daemon(&dir.path().join("ax"), "0.0.1");
    hold_this_binary(dir.path());
    let out = clax(dir.path())
        .args(["serve", "--json", "--port", "0"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        !fake.shutdown_seen.load(Ordering::SeqCst),
        "the held upgrade is not tried"
    );
    for s in [
        "keeping clax daemon v0.0.1",
        &format!("to v{}", env!("CARGO_PKG_VERSION")),
        "not tried again until",
        "`clax stop`",
    ] {
        assert!(stderr.contains(s), "{s} in {stderr}");
    }
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["version"], "0.0.1");
    let held = &j["upgrade_held"];
    assert_eq!(held["version"], env!("CARGO_PKG_VERSION"), "{j}");
    assert_eq!(held["from_version"], "0.0.1");
    assert!(
        held["reason"].as_str().unwrap().contains("it crashed"),
        "{j}"
    );
    for k in ["exe", "failed_at", "until", "advice"] {
        assert!(held[k].is_string(), "{k} in {j}");
    }

    let out = clax(dir.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["upgrade_held"]["reason"], held["reason"], "{j}");
    let out = clax(dir.path()).arg("status").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("upgrade held: keeping clax daemon v0.0.1"),
        "{text}"
    );
    assert!(
        text.contains("why: the new clax daemon failed to start"),
        "{text}"
    );

    let out = clax(dir.path())
        .args(["doctor", "--agent", "claude", "--json"])
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let upgrade = j["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "upgrade")
        .unwrap_or_else(|| panic!("no upgrade check in {j}"))
        .clone();
    assert_eq!(upgrade["ok"], false, "{upgrade}");
    assert!(
        upgrade["detail"]
            .as_str()
            .unwrap()
            .contains("why: the new clax daemon failed"),
        "{upgrade}"
    );
}
