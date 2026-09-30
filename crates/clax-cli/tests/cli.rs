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
        let mut c = Command::cargo_bin("clax").unwrap();
        c.env("CLAX_HOME", self.dir.path().join("ax"))
            .env("CLAX_CODEX_BIN", "")
            .env("HOME", self.dir.path());
        // Harness directories default under the temp HOME, never the real ones.
        for var in [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
            "CLAUDE_PLUGIN_ROOT",
            "PLUGIN_ROOT",
        ] {
            c.env_remove(var);
        }
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
fn serve_with_a_different_bind_than_the_running_daemon_fails() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    e.cmd()
        .args(["serve", "--port", "0", "--bind", "0.0.0.0"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("clax stop"));
    e.stop();
}

#[test]
fn concurrent_auto_starts_yield_one_daemon() {
    let e = Env::new();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let home = e.dir.path().join("ax");
            let hd = e.dir.path().to_path_buf();
            std::thread::spawn(move || {
                let mut c = Command::cargo_bin("clax").unwrap();
                c.env("CLAX_HOME", home)
                    .env("CLAX_CODEX_BIN", "")
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
        "corrupt_rows",
        "version_files",
        "stale_files",
        "assets",
        "ui",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    e.stop();
}

#[test]
fn publish_dir_skips_dotfiles_and_mirrors_on_update() {
    let e = Env::new();
    let index = write(
        e.dir.path(),
        "site/index.html",
        "<title>Site</title><p>v1</p>",
    );
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

#[test]
fn missing_clax_home_and_home_is_an_error() {
    Command::cargo_bin("clax")
        .unwrap()
        .env_remove("CLAX_HOME")
        .env_remove("HOME")
        .args(["status", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "neither CLAX_HOME nor HOME is set",
        ));
}

#[test]
fn doctor_names_corrupt_rows_and_still_checks_good_artifacts() {
    let e = Env::new();
    let home = clax_core::Home::at(e.dir.path().join("ax"));
    let store = clax_core::Store::open(&home).unwrap();
    let make = |title: &str| {
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": title,
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap();
        store.create_artifact(p, None).unwrap().0.id
    };
    let good = make("good");
    let bad = make("bad");
    drop(store);
    rusqlite::Connection::open(home.db_path())
        .unwrap()
        .execute(
            "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
            [&bad],
        )
        .unwrap();
    // Remove a file of the good artifact so version_files has something to find.
    std::fs::remove_file(
        home.version_dir(&clax_core::ArtifactId::parse(&good).unwrap(), 1)
            .join("index.html"),
    )
    .unwrap();

    let out = e.cmd().args(["doctor", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = |name: &str| {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no {name} check"))
            .clone()
    };
    let corrupt = check("corrupt_rows");
    assert_eq!(corrupt["ok"], false);
    assert_eq!(corrupt["detail"], format!("{bad}:capabilities_json"));
    let files = check("version_files");
    assert_eq!(files["ok"], false);
    assert_eq!(files["detail"], format!("missing {good}:index.html"));
}

fn make_artifact(store: &clax_core::Store, title: &str) -> clax_core::ArtifactId {
    let p = clax_core::publish::validate(
        serde_json::from_value(serde_json::json!({
            "title": title,
            "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        }))
        .unwrap(),
    )
    .unwrap();
    clax_core::ArtifactId::parse(&store.create_artifact(p, None).unwrap().0.id).unwrap()
}

fn doctor_check(e: &Env, args: &[&str], name: &str) -> serde_json::Value {
    let out = e
        .cmd()
        .arg("doctor")
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} check"))
        .clone()
}

#[test]
fn doctor_fix_clears_stale_files() {
    let e = Env::new();
    let home = clax_core::Home::at(e.dir.path().join("ax"));
    let store = clax_core::Store::open(&home).unwrap();
    let id = make_artifact(&store, "A");
    drop(store);
    let versions = home.artifact_dir(&id).join("versions");
    std::fs::create_dir_all(versions.join(".tmp-x")).unwrap();
    std::fs::create_dir_all(versions.join("99")).unwrap();
    rusqlite::Connection::open(home.db_path())
        .unwrap()
        .execute(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
             VALUES ('zzzzzzzzzzzz', 'z', 'x', 'x', 0, '1')",
            [],
        )
        .unwrap();

    let before = doctor_check(&e, &[], "stale_files");
    assert_eq!(before["ok"], false);
    let detail = before["detail"].as_str().unwrap();
    assert!(detail.contains(".tmp-x"), "{detail}");
    assert!(detail.contains("versions/99"), "{detail}");
    assert!(detail.contains("zzzzzzzzzzzz"), "{detail}");

    let fixed = doctor_check(&e, &["--fix"], "stale_files");
    assert_eq!(fixed["ok"], true, "{fixed}");
    assert!(!versions.join(".tmp-x").exists());
    assert!(!versions.join("99").exists());
    assert!(versions.join("1").exists(), "live version is kept");
    assert_eq!(doctor_check(&e, &[], "stale_files")["ok"], true);
}

#[test]
fn doctor_fix_clears_asset_problems() {
    let e = Env::new();
    let home = clax_core::Home::at(e.dir.path().join("ax"));
    let store = clax_core::Store::open(&home).unwrap();
    let live = make_artifact(&store, "live");
    let gone = make_artifact(&store, "gone");
    let missing = store.add_asset(&live, "image/png", &[1, 2, 3]).unwrap();
    let wrong = store.add_asset(&live, "image/png", &[1, 2, 3]).unwrap();
    let fine = store.add_asset(&live, "image/png", &[1]).unwrap();
    store.add_asset(&gone, "image/png", &[9]).unwrap();
    store.delete_artifact(&gone).unwrap();
    std::fs::remove_file(store.get_asset(&missing.id).unwrap().unwrap().1).unwrap();
    std::fs::write(
        store.get_asset(&wrong.id).unwrap().unwrap().1,
        b"longer than recorded",
    )
    .unwrap();
    let tmp = home.assets_dir(&live).join("X.png.tmp");
    std::fs::write(&tmp, b"x").unwrap();
    drop(store);

    let before = doctor_check(&e, &[], "assets");
    assert_eq!(before["ok"], false);
    let detail = before["detail"].as_str().unwrap();
    assert!(
        detail.contains(&format!("{live}:{}", missing.id)),
        "{detail}"
    );
    assert!(detail.contains(&format!("{live}:{}", wrong.id)), "{detail}");
    assert!(!detail.contains(&fine.id), "{detail}");
    assert!(detail.contains("X.png.tmp"), "{detail}");

    // Rows of the soft-deleted artifact are cleared; the live artifact's broken
    // rows are reported but never deleted, so the check still fails on them.
    let after = doctor_check(&e, &["--fix"], "assets");
    assert!(!tmp.exists());
    let detail = after["detail"].as_str().unwrap();
    assert!(!detail.contains("X.png.tmp"), "{detail}");
    assert!(detail.contains(&missing.id), "{detail}");
    let store = clax_core::Store::open(&home).unwrap();
    let rows = store.list_all_asset_rows().unwrap();
    assert!(rows.iter().all(|r| !r.artifact_deleted));
    assert_eq!(rows.len(), 3);

    // Once the live problems are repaired by hand the check passes.
    store.delete_asset(&missing.id).unwrap();
    store.delete_asset(&wrong.id).unwrap();
    drop(store);
    assert_eq!(doctor_check(&e, &[], "assets")["ok"], true);
}

#[test]
fn doctor_fix_deletes_corrupt_rows_of_deleted_artifacts_only() {
    let e = Env::new();
    let home = clax_core::Home::at(e.dir.path().join("ax"));
    let store = clax_core::Store::open(&home).unwrap();
    let live = make_artifact(&store, "live");
    let gone = make_artifact(&store, "gone");
    store.delete_artifact(&gone).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(home.db_path()).unwrap();
    db.execute(
        "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
        [gone.as_str()],
    )
    .unwrap();
    drop(db);

    let before = doctor_check(&e, &[], "corrupt_rows");
    assert_eq!(before["ok"], false);
    assert_eq!(before["detail"], format!("{gone}:capabilities_json"));
    assert_eq!(doctor_check(&e, &["--fix"], "corrupt_rows")["ok"], true);
    assert_eq!(doctor_check(&e, &[], "corrupt_rows")["ok"], true);

    rusqlite::Connection::open(home.db_path())
        .unwrap()
        .execute(
            "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
            [live.as_str()],
        )
        .unwrap();
    let still = doctor_check(&e, &["--fix"], "corrupt_rows");
    assert_eq!(still["ok"], false, "live rows are never deleted");
    assert_eq!(still["detail"], format!("{live}:capabilities_json"));
}

#[test]
fn usage_errors_exit_1_and_help_and_version_exit_0() {
    let e = Env::new();
    e.cmd()
        .arg("frobnicate")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Usage"));
    e.cmd().assert().failure().code(1);
    e.cmd()
        .args(["publish", "--if-version", "x", "a.html"])
        .assert()
        .failure()
        .code(1);
    e.cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("publish"));
    e.cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("clax"));
}

#[test]
fn file_spec_splits_only_when_the_source_exists() {
    let e = Env::new();
    let index = write(e.dir.path(), "index.html", "<title>Specs</title><p>x</p>");
    let odd = write(e.dir.path(), "a=b.js", "1");
    let src = write(e.dir.path(), "src.js", "2");
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0", "--file"])
        .arg(&odd)
        .arg("--file")
        .arg(format!("{}=lib/dest.js", src.display()))
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
    let files: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["list", "--json", "--files", &id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert!(files["files"]["a=b.js"].is_object(), "{files}");
    assert!(files["files"]["lib/dest.js"].is_object(), "{files}");
    assert!(files["files"]["src.js"].is_null(), "{files}");
    e.stop();
}

#[test]
fn doctor_lists_the_new_checks() {
    let e = Env::new();
    for n in ["stale_files", "assets"] {
        assert_eq!(doctor_check(&e, &[], n)["ok"], true, "{n}");
    }
}

#[test]
fn doctor_fix_refuses_while_a_daemon_is_live() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let versions = {
        let home = clax_core::Home::at(e.dir.path().join("ax"));
        let store = clax_core::Store::open(&home).unwrap();
        let id = make_artifact(&store, "A");
        home.artifact_dir(&id).join("versions")
    };
    std::fs::create_dir_all(versions.join(".tmp-x")).unwrap();
    e.cmd()
        .args(["doctor", "--fix"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "doctor --fix needs the daemon stopped; run `clax stop` first",
        ));
    assert!(versions.join(".tmp-x").exists(), "nothing was touched");
    e.stop();
    let out = e.cmd().args(["doctor", "--fix"]).output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(".tmp-x"),
        "the summary names the path"
    );
    assert!(!versions.join(".tmp-x").exists());
}

#[test]
fn publish_takes_a_new_artifacts_title_from_the_page_or_refuses() {
    let e = Env::new();
    let titled = write(
        e.dir.path(),
        "titled.html",
        "<title>From &lt;page&gt;</title><p>",
    );
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0"])
        .arg(&titled)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let id = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let list: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["list", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(list["artifacts"][0]["title"], "From <page>");

    let bare = write(e.dir.path(), "bare.html", "<p>no title");
    e.cmd()
        .args(["publish"])
        .arg(&bare)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--title").and(predicate::str::contains("<title>")));
    // An update needs no title.
    e.cmd()
        .args(["publish", "--id", &id])
        .arg(&bare)
        .assert()
        .success();
    e.stop();
}

#[test]
fn open_fails_when_the_opener_fails() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let page = write(e.dir.path(), "p.html", "<title>Open</title>");
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0"])
        .arg(&page)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let id = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let bin = e.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let fake = |code: u8| {
        for name in ["open", "xdg-open"] {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\nexit {code}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    };
    fake(1);
    e.cmd()
        .env("PATH", &path)
        .args(["open", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not open a browser"))
        .stderr(predicate::str::contains(format!("/a/{id}")));
    fake(0);
    e.cmd()
        .env("PATH", &path)
        .args(["open", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("/a/{id}")));
    e.stop();
}

#[test]
fn read_prints_the_read_tool_result() {
    let e = Env::new();
    let page = write(e.dir.path(), "r/index.html", "<title>Read</title><p>v1");
    write(e.dir.path(), "r/app.js", "console.log(1)");
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0", "--file"])
        .arg(e.dir.path().join("r/app.js"))
        .arg(&page)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pubd: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let id = pubd["id"].as_str().unwrap().to_string();
    write(e.dir.path(), "r/index.html", "<title>Read</title><p>v2");
    e.cmd()
        .args(["publish", "--id", &id])
        .arg(&page)
        .assert()
        .success();
    let json = |args: &[&str]| -> serde_json::Value {
        serde_json::from_slice(&e.cmd().args(args).assert().success().get_output().stdout).unwrap()
    };
    let r = json(&["read", "--json", &id]);
    assert_eq!(
        r,
        serde_json::json!({
            "artifact_id": id,
            "version": 2,
            "path": "index.html",
            "content_type": "text/html",
            "truncated": false,
            "size": 24,
            "content": "<title>Read</title><p>v2",
            "feedback": [],
        })
    );
    let r = json(&[
        "read",
        "--json",
        pubd["url"].as_str().unwrap(),
        "--version",
        "1",
        "--path",
        "app.js",
        "--max-bytes",
        "7",
    ]);
    assert_eq!(r["version"], 1);
    assert_eq!(r["path"], "app.js");
    assert_eq!(r["truncated"], true);
    assert_eq!(r["content"], "console");

    e.cmd()
        .args(["read", &id])
        .assert()
        .success()
        .stdout("<title>Read</title><p>v2");
    e.cmd()
        .args(["read", &id, "--path", "nope.css"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not_found"));
    e.stop();
}

#[test]
fn asset_upload_prints_the_asset_upload_tool_result() {
    let e = Env::new();
    let page = write(e.dir.path(), "index.html", "<title>Assets</title>");
    let out = e
        .cmd()
        .args(["publish", "--json", "--port", "0"])
        .arg(&page)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let id = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(e.dir.path().join("a.png"), [1u8, 2, 3]).unwrap();
    std::fs::write(e.dir.path().join("b.txt"), "hello").unwrap();
    // Relative paths resolve against the working directory.
    let out = e
        .cmd()
        .current_dir(e.dir.path())
        .args(["asset", "upload", "--json", &id, "a.png", "b.txt"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let r: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(r["feedback"], serde_json::json!([]));
    let assets = r["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2, "{r}");
    assert_eq!(assets[0]["content_type"], "image/png");
    assert_eq!(assets[0]["size"], 3);
    assert_eq!(assets[1]["content_type"], "text/plain");
    assert!(assets[0]["id"].is_string());
    let url = assets[0]["url"].as_str().unwrap();
    assert!(
        url.starts_with("http://localhost:") && url.contains("/_blob/"),
        "{url}"
    );

    let text = e
        .cmd()
        .current_dir(e.dir.path())
        .args(["asset", "upload", &id, "a.png"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(text).unwrap().contains("/_blob/"));
    e.cmd()
        .args(["asset", "upload", &id, "/no/such/file.png"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("file_unreadable"));
    e.stop();
}

#[test]
fn the_shim_serves_only_claude_and_codex() {
    let e = Env::new();
    e.cmd()
        .args(["mcp", "--agent", "pi"])
        .write_stdin("")
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid value 'pi'"));
    e.cmd()
        .args(["hook", "--agent", "pi", "session-start"])
        .write_stdin("")
        .assert()
        .success()
        .stderr(predicate::str::contains("invalid value 'pi'"));
}

#[test]
fn doctor_reports_codex_push_from_the_daemons_path() {
    let e = Env::new();
    let bin_dir = e.dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    std::fs::write(&codex, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // This test is about push: it lets the daemon look on its PATH.
    e.cmd()
        .env_remove("CLAX_CODEX_BIN")
        .env("PATH", &path)
        .args(["serve", "--port", "0"])
        .assert()
        .success();
    let push = doctor_check(&e, &["--agent", "codex"], "codex_push");
    assert_eq!(push["ok"], true);
    let detail = push["detail"].as_str().unwrap();
    assert!(
        detail.contains(&codex.display().to_string()) && detail.contains("found on PATH"),
        "{detail}"
    );
    let info: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(e.dir.path().join("ax/daemon.json")).unwrap(),
    )
    .unwrap();
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/api/sessions", info["port"]))
        .bearer_auth(info["token"].as_str().unwrap())
        .json(&serde_json::json!({"harness": "codex", "cwd": "/w", "pid": 1, "parent_pid": 2}))
        .send()
        .unwrap();
    let sessions = doctor_check(&e, &["--agent", "codex"], "codex_sessions");
    assert_eq!(sessions["ok"], false);
    let mcp = doctor_check(&e, &["--agent", "codex"], "mcp");
    assert_eq!(mcp["ok"], true, "{mcp}");
    let feedback = doctor_check(&e, &["--agent", "codex"], "feedback");
    assert_eq!(feedback["ok"], true, "{feedback}");
    assert!(
        feedback["detail"]
            .as_str()
            .unwrap()
            .contains("0 watch(es), 0 with replies armed; push off: "),
        "{feedback}"
    );
    assert!(
        sessions["detail"]
            .as_str()
            .unwrap()
            .contains("features.hooks = true")
    );
    e.stop();
}

/// Waits up to `timeout` for `fd` to reach EOF (every write end closed).
fn read_end_hits_eof(fd: libc::c_int, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `p` is one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut p, 1, left.as_millis() as libc::c_int) };
        if n <= 0 {
            return false;
        }
        let mut buf = [0u8; 64];
        // SAFETY: `buf` is a writable buffer of the stated length.
        let r = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if r == 0 {
            return true;
        }
        if r < 0 || std::time::Instant::now() >= deadline {
            return false;
        }
    }
}

/// How long a step that is immediate on an idle machine may take on a loaded one.
const LOADED_BOUND: std::time::Duration = std::time::Duration::from_secs(20);

/// Waits for a foreground daemon under `home` to write a daemon.json naming
/// its pid; false if it exits first or `timeout` passes.
fn wait_for_daemon_json(
    home: &std::path::Path,
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let pid = std::fs::read_to_string(home.join("daemon.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["pid"].as_u64());
        if pid == Some(u64::from(child.id())) {
            return true;
        }
        if child.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// A close-on-exec pipe, as std makes them.
fn cloexec_pipe() -> (libc::c_int, libc::c_int) {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` has room for the two descriptors pipe writes; fcntl only
    // sets flags on them.
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
    }
    (fds[0], fds[1])
}

/// Passes `fd` to the command's child as an extra inherited descriptor: the
/// state a concurrent fork catches a std pipe in on platforms without `pipe2`.
/// Only this child inherits it, so other tests' children cannot hold it.
fn inherit(cmd: &mut std::process::Command, fd: libc::c_int) {
    use std::os::unix::process::CommandExt;
    // SAFETY: fcntl is async-signal-safe and touches only the child's copy.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[test]
fn an_auto_started_daemon_does_not_hold_inherited_descriptors() {
    let e = Env::new();
    let (r, w) = cloexec_pipe();
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("clax"));
    cmd.env("CLAX_HOME", e.dir.path().join("ax"))
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", e.dir.path())
        .args(["status", "--start", "--json", "--port", "0"])
        .stdin(std::process::Stdio::null());
    inherit(&mut cmd, w);
    let out = cmd.output().unwrap();
    // SAFETY: `w` is this test's own write end, closed once.
    unsafe { libc::close(w) };
    assert!(out.status.success(), "{out:?}");
    // `status --start` returns once the daemon answers, so only the read is bounded.
    let eof = read_end_hits_eof(r, LOADED_BOUND);
    // SAFETY: `r` is this test's own read end, closed once.
    unsafe { libc::close(r) };
    e.stop();
    assert!(eof, "the daemon kept an inherited pipe write end open");
}

#[test]
fn a_foreground_daemon_closes_inherited_descriptors() {
    let e = Env::new();
    let (r, w) = cloexec_pipe();
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("clax"));
    cmd.env("CLAX_HOME", e.dir.path().join("ax"))
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", e.dir.path())
        .args(["serve", "--foreground", "--port", "0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    inherit(&mut cmd, w);
    let mut child = cmd.spawn().unwrap();
    // SAFETY: `w` is this test's own write end, closed once.
    unsafe { libc::close(w) };
    // The daemon closes inherited descriptors before it serves, so once it has
    // written daemon.json the pipe is already at EOF; the EOF bound only
    // covers the read itself on a loaded machine.
    let ready = wait_for_daemon_json(&e.dir.path().join("ax"), &mut child, LOADED_BOUND);
    let eof = ready && read_end_hits_eof(r, LOADED_BOUND);
    // SAFETY: `r` is this test's own read end, closed once.
    unsafe { libc::close(r) };
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(ready, "the daemon did not write daemon.json");
    assert!(alive, "the daemon kept running after closing descriptors");
    assert!(eof, "the daemon kept an inherited pipe write end open");
}

#[test]
fn every_hook_run_appends_a_line_to_hooks_log() {
    let e = Env::new();
    e.cmd()
        .args(["hook", "--agent", "codex", "stop"])
        .write_stdin("{}")
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("no clax daemon is running"));
    e.cmd()
        .args(["hook", "--agent", "pi", "session-start"])
        .write_stdin("")
        .assert()
        .success();
    let log = std::fs::read_to_string(e.dir.path().join("ax/logs/hooks.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    let bin = assert_cmd::cargo::cargo_bin("clax");
    assert!(
        lines[0].contains(&format!(
            " hook agent=codex event=stop bin={} duration_ms=",
            bin.display()
        )) && lines[0].ends_with(" exit=0 stderr=\"no clax daemon is running\""),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].contains(" hook agent=pi event=- ") && lines[1].contains("invalid value 'pi'"),
        "{}",
        lines[1]
    );
}

#[test]
fn doctor_agent_checks_each_layer_of_the_integration() {
    let e = Env::new();
    // A Codex cache copy whose manifest version matches this binary but whose
    // skill has no generated tools block: plugin passes, skill is stale.
    let root = ".codex/plugins/cache/clax/clax/0.2.0";
    write(
        e.dir.path(),
        &format!("{root}/.codex-plugin/plugin.json"),
        r#"{"name": "clax", "version": "0.2.0"}"#,
    );
    write(
        e.dir.path(),
        &format!("{root}/skills/clax/SKILL.md"),
        "# Clax\n\nThe tools are exposed as the `clax` MCP server (`publish`).\n",
    );
    e.cmd()
        .args(["hook", "--agent", "codex", "stop"])
        .write_stdin("{}")
        .assert()
        .success();
    let out = e
        .cmd()
        .args(["doctor", "--agent", "codex", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let checks = v["checks"].as_array().unwrap();
    let by_name = |n: &str| {
        checks
            .iter()
            .find(|c| c["name"] == n)
            .unwrap_or_else(|| panic!("no {n} check in {v}"))
            .clone()
    };
    for n in ["home", "daemon", "db_integrity", "codex_push"] {
        by_name(n);
    }
    let binary = by_name("binary");
    assert_eq!(binary["ok"], true);
    assert!(
        binary["detail"]
            .as_str()
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
    let plugin = by_name("plugin");
    assert_eq!(plugin["ok"], true, "{plugin}");
    assert!(
        plugin["detail"].as_str().unwrap().contains(root),
        "{plugin}"
    );
    let skill = by_name("skill");
    assert_eq!(skill["ok"], false);
    assert!(
        skill["detail"]
            .as_str()
            .unwrap()
            .starts_with("stale skill:")
            && skill["detail"]
                .as_str()
                .unwrap()
                .contains("codex plugin add clax@clax"),
        "{skill}"
    );
    let mcp = by_name("mcp");
    assert_eq!(mcp["ok"], false);
    assert!(
        mcp["detail"]
            .as_str()
            .unwrap()
            .contains("no daemon is running"),
        "{mcp}"
    );
    let hooks = by_name("hooks");
    assert_eq!(hooks["ok"], true);
    assert!(
        hooks["detail"]
            .as_str()
            .unwrap()
            .contains("agent=codex event=stop"),
        "{hooks}"
    );
    assert_eq!(by_name("feedback")["ok"], false);
    for n in ["name", "ok", "detail"] {
        assert!(checks.iter().all(|c| c.get(n).is_some()), "{n}");
    }

    // Every harness is accepted; the text form names each layer.
    for agent in ["claude", "pi"] {
        let out = e.cmd().args(["doctor", "--agent", agent]).output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        for n in ["binary", "plugin", "skill", "mcp", "hooks", "feedback"] {
            assert!(text.contains(&format!(" {n} ")), "{agent}: {n} in {text}");
        }
        assert!(!text.contains("codex_push"), "{text}");
    }
}

const NO_SESSION_NOTE: &str = "note: published without an agent session; comments on this page will wait until an agent session watches it";

#[test]
fn publish_says_when_no_agent_session_will_get_the_comments() {
    let e = Env::new();
    let index = write(e.dir.path(), "p/index.html", "<title>Solo</title><p>1");
    let out = e
        .cmd()
        .args(["publish", "--port", "0"])
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("published v1 at "), "{text}");
    assert_eq!(lines.get(1), Some(&NO_SESSION_NOTE), "{text}");
    let v: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["publish", "--port", "0", "--json"])
            .arg(&index)
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert!(v["session"].is_null(), "{v}");
    assert!(v.get("session").is_some(), "{v}");

    // An artifact an agent session owns: a CLI update keeps its session.
    let info: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(e.dir.path().join("ax/daemon.json")).unwrap(),
    )
    .unwrap();
    let http = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{}", info["port"]);
    let token = info["token"].as_str().unwrap();
    let session: serde_json::Value = http
        .post(format!("{base}/api/sessions"))
        .bearer_auth(token)
        .json(&serde_json::json!({"harness": "claude", "cwd": "/w"}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let sid = session["session"]["id"].as_str().unwrap();
    let created: serde_json::Value = http
        .post(format!("{base}/api/artifacts"))
        .bearer_auth(token)
        .header("X-Clax-Session", sid)
        .json(&serde_json::json!({"title": "Owned", "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let id = created["artifact"]["id"].as_str().unwrap();
    let out = e
        .cmd()
        .args(["publish", "--port", "0", "--id", id])
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(!text.contains("note:"), "{text}");
    let v: serde_json::Value = serde_json::from_slice(
        &e.cmd()
            .args(["publish", "--port", "0", "--json", "--id", id])
            .arg(&index)
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(v["session"], sid, "{v}");
    e.stop();
}
