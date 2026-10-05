//! `clax native-host` against a real daemon on a scratch home.

use assert_cmd::Command;
use clax_core::extension::{extension_id_in_effect, extension_origin};
use serde_json::{Value, json};
use std::path::Path;

fn frame(v: &Value) -> Vec<u8> {
    let b = serde_json::to_vec(v).unwrap();
    let mut out = (b.len() as u32).to_ne_bytes().to_vec();
    out.extend(b);
    out
}

/// The one framed message in `stdout`; fails when it holds anything else.
fn only_message(stdout: &[u8]) -> Value {
    assert!(
        stdout.len() >= 4,
        "stdout holds a framed message: {stdout:?}"
    );
    let n = u32::from_ne_bytes(stdout[..4].try_into().unwrap()) as usize;
    assert_eq!(stdout.len(), 4 + n, "stdout holds exactly one message");
    serde_json::from_slice(&stdout[4..]).unwrap()
}

/// `clax` for the home `home`, with Chrome's sparse environment: no PATH and
/// no HOME beyond what the test sets.
fn clax(home: &Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env_clear()
        .env("CLAX_HOME", home)
        .env("CLAX_NO_OPEN", "1");
    c
}

/// The origin Chrome passes for the extension with this home's ID in effect.
fn our_origin(home: &Path) -> String {
    format!("{}/", extension_origin(&extension_id_in_effect(home)))
}

/// Stops the scratch home's daemon when the test ends, passing or not.
struct StopDaemon<'a>(&'a Path);
impl Drop for StopDaemon<'_> {
    fn drop(&mut self) {
        let _ = clax(self.0).arg("stop").output();
    }
}

#[test]
fn pairing_starts_a_daemon_and_returns_a_credential() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let _stop = StopDaemon(&home);
    // Any free port: config.toml cannot name port 0, so the flag does.
    let out = clax(&home)
        .args(["--port", "0", "native-host", &our_origin(&home)])
        .write_stdin(frame(
            &json!({"type": "pair", "v": 1, "extension_version": "0.0.0"}),
        ))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = only_message(&out.stdout);
    assert_eq!(v["type"], "paired", "{v}");
    assert_eq!(v["v"], 1, "{v}");
    let credential = v["credential"].as_str().unwrap();
    assert!(credential.starts_with("cxe_"), "{v}");
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("daemon.json")).unwrap()).unwrap();
    assert_eq!(
        v["daemon"],
        format!("http://localhost:{}", info["port"]),
        "{v}"
    );
    assert_eq!(v["clax_version"], env!("CARGO_PKG_VERSION"));
    assert!(v["viewer"]["public_id"].is_string(), "{v}");
    assert!(v["viewer"].get("cookie").is_none(), "{v}");
    let log = std::fs::read_to_string(home.join("logs/native-host.log")).unwrap();
    assert!(log.contains("paired"), "{log}");
    assert!(
        !log.contains(credential),
        "the log never holds the credential"
    );

    // A second pairing reuses the running daemon and mints another credential.
    let again = clax(&home)
        .args([
            "--port",
            "0",
            "native-host",
            &our_origin(&home),
            "--parent-window=0",
        ])
        .write_stdin(frame(&json!({"type": "pair", "v": 1})))
        .output()
        .unwrap();
    assert!(again.status.success());
    let w = only_message(&again.stdout);
    assert_eq!(w["daemon"], v["daemon"]);
    assert_eq!(w["viewer"]["public_id"], v["viewer"]["public_id"]);
    assert_ne!(w["credential"], v["credential"]);
}

#[test]
fn a_wrong_origin_gets_one_error_and_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = clax(dir.path())
        .args([
            "native-host",
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/",
        ])
        .write_stdin(frame(&json!({"type": "pair", "v": 1})))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = only_message(&out.stdout);
    assert_eq!(v["type"], "error");
    assert_eq!(v["code"], "wrong_origin");
    assert!(
        !dir.path().join("daemon.json").exists(),
        "no daemon was started"
    );
}

#[test]
fn a_missing_origin_is_a_wrong_origin() {
    let dir = tempfile::tempdir().unwrap();
    let out = clax(dir.path())
        .arg("native-host")
        .write_stdin(frame(&json!({"type": "pair", "v": 1})))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(only_message(&out.stdout)["code"], "wrong_origin");
}

#[test]
fn bad_messages_get_an_error_reply_and_start_no_daemon() {
    let dir = tempfile::tempdir().unwrap();
    for (input, code) in [
        (
            frame(&json!({"type": "pair", "v": 2})),
            "unsupported_version",
        ),
        (frame(&json!({"type": "hello", "v": 1})), "bad_request"),
        (b"\x05\x00".to_vec(), "bad_request"),
    ] {
        let out = clax(dir.path())
            .args(["native-host", &our_origin(dir.path())])
            .write_stdin(input)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(only_message(&out.stdout)["code"], code);
    }
    assert!(!dir.path().join("daemon.json").exists());
}

#[test]
fn no_home_is_one_error_reply() {
    let out = Command::cargo_bin("clax")
        .unwrap()
        .env_clear()
        .args([
            "native-host",
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/",
        ])
        .write_stdin(frame(&json!({"type": "pair", "v": 1})))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = only_message(&out.stdout);
    assert_eq!(v["type"], "error");
    assert_eq!(v["code"], "daemon_unavailable", "{v}");
}
