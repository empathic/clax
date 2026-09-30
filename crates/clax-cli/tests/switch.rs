//! A binary that finds an older daemon replaces it on the same port, holding
//! the start lock; a newer daemon is kept.

use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Fake {
    port: u16,
    child: Arc<Mutex<Child>>,
    shutdown_seen: Arc<AtomicBool>,
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
                break; // drops the listener: the port is free again
            }
        }
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
    let mut c = Command::cargo_bin("clax").unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let exe = std::fs::canonicalize(env!("CARGO_BIN_EXE_clax")).unwrap();
    assert_eq!(now["exe"].as_str().unwrap(), exe.display().to_string());
    clax(dir.path()).arg("stop").assert().success();
}

#[test]
fn serve_keeps_a_newer_daemon() {
    let dir = tempfile::tempdir().unwrap();
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
    let mut c = fake.child.lock().unwrap();
    let _ = c.kill();
    let _ = c.wait();
}
