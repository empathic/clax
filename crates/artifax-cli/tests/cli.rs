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

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().arg("stop").output();
        let info = std::fs::read_to_string(self.dir.path().join("ax/daemon.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        if let Some(pid) = info.and_then(|v| v["pid"].as_i64()) {
            // SAFETY: signal 0 probes, SIGTERM ends the leaked test daemon.
            unsafe {
                if libc::kill(pid as libc::pid_t, 0) == 0 {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }
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
fn write(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn publish_list_pin_open_delete_roundtrip() {
    let e = Env::new();
    let index = write(
        e.dir.path(),
        "site/index.html",
        "<title>Hello</title><p>v1</p>",
    );
    write(e.dir.path(), "site/app.js", "1");
    write(e.dir.path(), "site/img/logo.png", "not-really-png");
    let out = e
        .cmd()
        .args([
            "publish", "--json", "--port", "0", "--title", "Hello", "--dir",
        ])
        .arg(e.dir.path().join("site"))
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["version"], 1);
    assert!(v["url"].as_str().unwrap().ends_with(&format!("/a/{id}")));

    let list: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["list", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(list["artifacts"][0]["title"], "Hello");
    let files: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["list", "--json", "--files", &id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert!(files["files"]["img/logo.png"].is_object());

    write(e.dir.path(), "site/index.html", "<p>v2</p>");
    let v2: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["publish", "--json", "--id", &id])
            .arg(&index)
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(v2["version"], 2);

    e.cmd().args(["pin", &id]).assert().success();
    let list: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["list", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(list["artifacts"][0]["pinned"], true);
    e.cmd().args(["unpin", &id]).assert().success();

    let open: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["open", "--json", &id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert!(open["url"].as_str().unwrap().contains(&format!("/a/{id}")));
    let open2: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["open", "--json", open["url"].as_str().unwrap()])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(open, open2, "open accepts a URL too");

    e.cmd().args(["delete", &id]).assert().success();
    e.cmd()
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id).not());
    e.cmd()
        .args(["delete", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not_found"));
    e.stop();
}

#[test]
fn publish_rejects_missing_index_and_bad_id() {
    let e = Env::new();
    e.cmd()
        .args(["publish", "--port", "0"])
        .arg(e.dir.path().join("nope.html"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope.html"));
    e.cmd()
        .args(["open", "not-an-id"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid_id"));
    e.stop();
}

#[test]
fn doctor_runs_all_checks() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let out = e
        .cmd()
        .args(["doctor", "--json"])
        .assert()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let names: Vec<&str> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for n in [
        "home",
        "daemon",
        "daemon_json_mode",
        "db_integrity",
        "version_files",
        "ui",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    e.stop();
}

#[test]
fn publish_dir_skips_dotfiles_and_mirrors_on_update() {
    let e = Env::new();
    let index = write(e.dir.path(), "site/index.html", "<p>v1</p>");
    write(e.dir.path(), "site/a.js", "1");
    write(e.dir.path(), "site/.hidden", "secret");
    let site = e.dir.path().join("site");
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0", "--dir"])
        .arg(&site)
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let id = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let files = |e: &Env| -> serde_json::Value {
        serde_json::from_slice(
            &e.cmd()
                .args(["list", "--json", "--files", &id])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .unwrap()
    };
    let f = files(&e);
    assert!(f["files"]["a.js"].is_object());
    assert!(f["files"][".hidden"].is_null());

    std::fs::remove_file(site.join("a.js")).unwrap();
    e.cmd()
        .args(["publish", "--json", "--id", &id, "--dir"])
        .arg(&site)
        .arg(&index)
        .assert()
        .success();
    assert!(files(&e)["files"]["a.js"].is_null());
    e.stop();
}
