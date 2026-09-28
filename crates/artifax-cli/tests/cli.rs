use assert_cmd::Command;
use predicates::prelude::*;

struct Env {
    dir: tempfile::TempDir,
}
impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("artifax").unwrap();
        c.env("ARTIFAX_HOME", self.dir.path().join("ax"))
            .env("HOME", self.dir.path());
        c
    }
    fn stop(&self) {
        self.cmd().arg("stop").assert().success();
    }
}

#[test]
fn status_without_daemon_reports_not_running() {
    let e = Env::new();
    e.cmd()
        .args(["status", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"running\":false"));
}

#[test]
fn serve_starts_a_background_daemon_and_stop_ends_it() {
    let e = Env::new();
    let out = e
        .cmd()
        .args(["serve", "--json", "--port", "0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["running"], true);
    let port = v["port"].as_u64().unwrap();
    assert!(port > 0);
    let again: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["serve", "--json", "--port", "0"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        again["port"].as_u64().unwrap(),
        port,
        "second serve reuses the running daemon"
    );
    let st: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["status", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(st["running"], true);
    assert!(st["url"].as_str().unwrap().starts_with("http://localhost:"));
    e.stop();
    let st: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["status", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(st["running"], false);
}

#[test]
fn concurrent_auto_starts_yield_one_daemon() {
    let e = Env::new();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let home = e.dir.path().join("ax");
            let hd = e.dir.path().to_path_buf();
            std::thread::spawn(move || {
                let mut c = Command::cargo_bin("artifax").unwrap();
                c.env("ARTIFAX_HOME", home)
                    .env("HOME", hd)
                    .args(["status", "--start", "--json", "--port", "0"]);
                let out = c.assert().success().get_output().stdout.clone();
                serde_json::from_slice::<serde_json::Value>(&out).unwrap()["pid"]
                    .as_u64()
                    .unwrap()
            })
        })
        .collect();
    let pids: std::collections::HashSet<u64> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(pids.len(), 1, "all callers found the same daemon");
    e.stop();
}
