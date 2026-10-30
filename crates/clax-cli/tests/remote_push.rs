//! `clax remote add|list|remove` and `clax push`: this Clax signs in to a
//! public Clax (a hosted test daemon) by device code and publishes an
//! artifact's versions there.

use crate::common::Env;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::sync::Arc;

const SECRET: &str = "the setup secret";

/// The owner's session cookie on hosted daemon `ts`.
async fn owner_cookie(ts: &TestServer, origin: &str) -> String {
    let res = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header("host", format!("localhost:{}", ts.addr.port()))
        .header("origin", origin)
        .json(&json!({"secret": SECRET, "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .strip_prefix("clax_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn json_out(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(out)))
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_add_signs_in_by_device_code_and_push_publishes_every_version() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let public = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
    let origin = format!("http://localhost:{}", public.addr.port());
    let cookie = owner_cookie(&public, &origin).await;
    let e = Env::new();
    let page = e.dir.path().join("index.html");
    std::fs::write(&page, "<title>Plan</title><p>v1</p>").unwrap();
    let v = json_out(
        &e.cmd()
            .args(["publish", "--json", "--port", "0"])
            .arg(&page)
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let id = v["id"].as_str().unwrap().to_string();
    for n in 2..=3 {
        std::fs::write(&page, format!("<title>Plan</title><p>v{n}</p>")).unwrap();
        e.cmd()
            .args(["publish", "--json", "--id", &id])
            .arg(&page)
            .assert()
            .success();
    }
    let out = e.cmd().args(["push", &id]).assert().failure();
    assert!(
        String::from_utf8_lossy(&out.get_output().stderr).contains("no_remote"),
        "{out:?}"
    );
    // `remote add` prints the code and waits; the owner approves there.
    let mut add = std::process::Command::new(assert_cmd::cargo::cargo_bin("clax"));
    add.env("CLAX_HOME", e.dir.path().join("ax"))
        .env("CLAX_CODEX_BIN", "")
        .env("HOME", e.dir.path());
    for var in [
        "CLAX_CHANNEL",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "CODEX_CI",
        "PI_CODING_AGENT",
    ] {
        add.env_remove(var);
    }
    let mut child = add
        .args(["remote", "add", &origin, "--no-open", "--name", "pub"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    err.read_line(&mut first).unwrap();
    let code = first
        .split("approve code ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .unwrap_or_else(|| panic!("{first}"))
        .to_string();
    let res = public
        .client
        .post(format!("{}/api/hosted/device/{code}/approve", public.base))
        .header("host", format!("localhost:{}", public.addr.port()))
        .header("origin", &origin)
        .header("cookie", format!("clax_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let out = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("signed in to"),
        "{out:?}"
    );
    let list = String::from_utf8(
        e.cmd()
            .args(["remote", "list"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(list.contains("pub") && list.contains("as Ada"), "{list}");
    let v = json_out(
        &e.cmd()
            .args(["push", "--json", &id, "--to", "pub"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["pushed"].as_array().unwrap().len(), 3, "{v}");
    let rid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(v["url"], format!("{origin}/a/{rid}"));
    let rid_id = clax_core::ArtifactId::parse(&rid).unwrap();
    let versions = public.store.list_versions(&rid_id).unwrap();
    assert_eq!(versions.len(), 3);
    // Published by hand here, so by hand there: the person's, by no agent.
    assert!(
        versions.iter().all(|v| v.session_id.is_none()),
        "{versions:?}"
    );
    let a = public.store.get_artifact(&rid_id).unwrap().unwrap();
    assert_eq!(a.owner_session_id, None);
    // Under an agent harness, `clax push` asks no one: refused, pointed at
    // the tool that asks the owner.
    std::fs::write(&page, "<title>Plan</title><p>v4</p>").unwrap();
    e.cmd()
        .args(["publish", "--json", "--id", &id])
        .arg(&page)
        .assert()
        .success();
    let out = e
        .cmd()
        .env("CLAUDECODE", "1")
        .args(["push", &id])
        .assert()
        .failure();
    let err = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(
        err.contains("no_session") && err.contains("publish_public"),
        "{err}"
    );
    assert_eq!(public.store.list_versions(&rid_id).unwrap().len(), 3);
    e.cmd().args(["push", &id]).assert().success();
    let text = String::from_utf8(
        e.cmd()
            .args(["push", &id])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(text.starts_with("up to date: "), "{text}");
    // The link's comments: download only, two-way again, then unlinked.
    let v = json_out(
        &e.cmd()
            .args(["remote", "comments", "--json", &id, "download-only"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["download_only"], true, "{v}");
    let text = String::from_utf8(
        e.cmd()
            .args(["remote", "comments", &id, "two-way", "--remote", "pub"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(text.starts_with("two-way"), "{text}");
    e.cmd()
        .args(["remote", "comments", &id, "both"])
        .assert()
        .failure();
    let text = String::from_utf8(
        e.cmd()
            .args(["remote", "unlink", &id])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(text.starts_with("unlinked"), "{text}");
    let out = e.cmd().args(["remote", "unlink", &id]).assert().failure();
    assert!(
        String::from_utf8_lossy(&out.get_output().stderr).contains("no_link"),
        "{out:?}"
    );
    let grant = public.store.list_grants().unwrap();
    let gid = grant.iter().find(|g| g.bearer).unwrap().id.clone();
    let text = String::from_utf8(
        e.cmd()
            .args(["remote", "remove", "pub"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(text.contains("revoked its agent token there"), "{text}");
    // Revoked on the public Clax too.
    assert_ne!(public.store.grant(&gid).unwrap().status, "approved");
    let v = json_out(
        &e.cmd()
            .args(["remote", "list", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["remotes"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_adding_a_remote_is_pointed_at_remote_request() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let public = TestServer::spawn_hosted(clock, Some(SECRET)).await;
    let origin = format!("http://localhost:{}", public.addr.port());
    let e = Env::new();
    // Under a harness, adding is refused with one line naming what an
    // agent does instead.
    let out = e
        .cmd()
        .env("CLAUDECODE", "1")
        .args(["remote", "add", &origin, "--no-open"])
        .assert()
        .code(2);
    let line = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(
        line.contains("not agents") && line.contains("agents: use `clax remote request <url>`"),
        "{line}"
    );
    // Asking needs the agent's session.
    let out = e
        .cmd()
        .env("CLAUDECODE", "1")
        .args(["remote", "request", &origin, "--reason", "to read the plan"])
        .assert()
        .failure();
    let err = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(err.contains("session_required"), "{err}");
}
