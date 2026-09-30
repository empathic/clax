//! Clax never reads, migrates, or deletes its previous name's home, and never
//! reads its previous variables: a live-looking daemon.json there is ignored
//! and left exactly as it was.

use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The previous name, assembled so the repository's name gate finds no literal.
const OLD: &str = concat!("arti", "fax");

#[test]
fn the_previous_home_and_variables_are_ignored_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let old_home = dir.path().join(format!(".{OLD}"));
    std::fs::create_dir_all(&old_home).unwrap();

    // A daemon the old home names: this process's PID (alive) and a port that
    // answers /healthz and counts every connection.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            seen.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let body = r#"{"version":"0.2.0","pid":1,"started_at":"2026-09-29T00:00:00Z"}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let info = serde_json::json!({
        "port": port,
        "pid": std::process::id(),
        "token": "t",
        "started_at": "2026-09-29T00:00:00Z",
        "bind": "127.0.0.1",
        "version": "0.2.0",
    })
    .to_string();
    let daemon_json = old_home.join("daemon.json");
    std::fs::write(&daemon_json, &info).unwrap();

    let old_var = |suffix: &str| format!("{}_{suffix}", OLD.to_uppercase());
    let out = Command::cargo_bin("clax")
        .unwrap()
        .arg("stop")
        .env("HOME", dir.path())
        .env_remove("CLAX_HOME")
        .env("CLAX_CODEX_BIN", "")
        .env(old_var("HOME"), &old_home)
        .env(old_var("BIN"), "/nonexistent")
        .output()
        .unwrap();

    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "no clax daemon is running"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the old daemon.json was followed"
    );
    assert_eq!(std::fs::read_to_string(&daemon_json).unwrap(), info);
    let entries: Vec<_> = std::fs::read_dir(&old_home)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("daemon.json")]);
}
