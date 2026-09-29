mod common;
use artifax_server::push::{CodexPush, CodexSource};
use common::TestServer;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A fake `codex` that records its arguments and `CODEX_HOME`, sleeps, and exits with `exit`.
fn fake_codex(dir: &Path, exit: i32, sleep_s: u32) -> PathBuf {
    let bin = dir.join("codex");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{args}'\nprintf '%s' \"${{CODEX_HOME:-}}\" > '{home}'\nsleep {sleep_s}\nexit {exit}\n",
            args = dir.join("args.txt").display(),
            home = dir.join("codex_home.txt").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

async fn server(bin: Option<PathBuf>, timeout: Duration) -> TestServer {
    let source = if bin.is_some() {
        CodexSource::Env
    } else {
        CodexSource::NotFound
    };
    TestServer::spawn_with(move |s| {
        s.codex = Arc::new(CodexPush {
            bin,
            timeout,
            source,
        })
    })
    .await
}

/// A Codex session as the SessionStart hook leaves it (Codex session ID and
/// CODEX_HOME known), owning and watching a new artifact; (sid, aid).
async fn codex_owner(ts: &TestServer, hsid: Option<&str>) -> (String, String) {
    let sid = match hsid {
        Some(h) => {
            let res = ts.post_json("/api/sessions/join", json!({"harness": "codex", "parent_pid": 4242, "harness_session_id": h, "cwd": "/w", "codex_home": "/tmp/cxh"})).await;
            assert_eq!(res.status(), 200);
            res.json::<Value>().await.unwrap()["session"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
        None => {
            let res = ts
                .post_json(
                    "/api/sessions",
                    json!({"harness": "codex", "cwd": "/w", "pid": 1, "parent_pid": 4243}),
                )
                .await;
            res.json::<Value>().await.unwrap()["session"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let a = ts.publish_as(&sid, "Pushed", "<h2>Goals</h2>").await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}

async fn state_of(ts: &TestServer, aid: &str, tid: &str) -> Value {
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    v["thread"]["feedback_state"].clone()
}

async fn eventually(ts: &TestServer, aid: &str, tid: &str, want: &str) -> Value {
    for _ in 0..100 {
        let s = state_of(ts, aid, tid).await;
        if s["state"] == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "state never became {want}: {}",
        state_of(ts, aid, tid).await
    );
}

#[tokio::test]
async fn exit_0_delivers_by_queue_with_the_payload_and_codex_home() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, Some("cx-1")).await;
    let before = ts.thread(&aid, 1, "plain").await;
    assert_eq!(before["feedback_state"], Value::Null);
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    let tid = t["id"].as_str().unwrap();
    // The claim marks the row delivered before `codex queue` runs; the
    // `delivered` event is published once it has exited 0.
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "delivered" {
            assert_eq!(e["thread_id"], tid);
            break;
        }
    }
    let s = eventually(&ts, &aid, tid, "delivered").await;
    assert_eq!(s["tier"], "queue");
    let args = std::fs::read_to_string(d.path().join("args.txt")).unwrap();
    assert!(args.starts_with("queue\n--thread\ncx-1\n--message\n[artifax] 1 comment sent to you:\n[artifax] Comment sent to you on \"Pushed\""), "{args}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("codex_home.txt")).unwrap(),
        "/tmp/cxh"
    );
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        sess["push"],
        json!({"tier": "queue", "available": true, "reason": null, "codex_home": "/tmp/cxh"})
    );
    let push: Value = ts.get("/api/push").await.json().await.unwrap();
    assert_eq!(
        (
            push["codex"]["available"].clone(),
            push["codex"]["source"].clone()
        ),
        (json!(true), json!("env"))
    );
}

#[tokio::test]
async fn non_zero_exit_ends_the_session_and_reports_agent_ended() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 1, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, Some("cx-2")).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let t = ts.thread(&aid, 1, "@agent anyone?").await;
    let tid = t["id"].as_str().unwrap();
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "agent_ended" {
            assert_eq!(e["thread_id"], tid);
            break;
        }
    }
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(sess["session"]["ended_at"].is_string());
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert!(w["watches"].as_array().unwrap().is_empty());
    let next = ts.register_session("claude", "after").await;
    let nsid = next["id"].as_str().unwrap();
    ts.authed(
        ts.client
            .put(format!("{}/api/sessions/{nsid}/watches/{aid}", ts.base)),
    )
    .send()
    .await
    .unwrap();
    let fb: Value = ts
        .authed(ts.client.get(format!(
            "{}/api/sessions/{nsid}/feedback?tier=piggyback",
            ts.base
        )))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        fb["feedback"][0]["thread_id"], tid,
        "the released row went to the next watcher"
    );
}

#[tokio::test]
async fn missing_binary_and_timeouts_release_rows_for_the_other_tiers() {
    let slow = tempfile::tempdir().unwrap();
    for (bin, timeout) in [
        (PathBuf::from("/nonexistent/codex"), Duration::from_secs(10)),
        (fake_codex(slow.path(), 0, 5), Duration::from_millis(300)),
    ] {
        let ts = server(Some(bin), timeout).await;
        let (sid, aid) = codex_owner(&ts, Some("cx-3")).await;
        let t = ts.thread(&aid, 1, "@agent please").await;
        let tid = t["id"].as_str().unwrap();
        tokio::time::sleep(timeout + Duration::from_millis(500)).await;
        let s = state_of(&ts, &aid, tid).await;
        assert_eq!(
            (s["state"].as_str(), s["tier"].as_str()),
            (Some("sent"), Some("queue")),
            "released, still waiting"
        );
        let sess: Value = ts
            .get_authed(&format!("/api/sessions/{sid}"))
            .await
            .json()
            .await
            .unwrap();
        assert!(
            sess["session"]["ended_at"].is_null(),
            "the session stays live"
        );
        let fb: Value = ts
            .authed(ts.client.get(format!(
                "{}/api/sessions/{sid}/feedback?tier=piggyback",
                ts.base
            )))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(fb["feedback"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn no_push_without_a_session_id_or_armed_replies_or_codex() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, None).await;
    let t = ts.thread(&aid, 1, "@agent hello").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !d.path().join("args.txt").exists(),
        "no Codex session ID, no queue"
    );
    assert_eq!(
        state_of(&ts, &aid, t["id"].as_str().unwrap()).await["tier"],
        "stop_hook"
    );
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        sess["push"]["reason"],
        "Codex session ID unknown, native push disabled"
    );

    let (sid2, aid2) = codex_owner(&ts, Some("cx-4")).await;
    ts.authed(
        ts.client
            .put(format!("{}/api/sessions/{sid2}/watches/{aid2}", ts.base)),
    )
    .json(&json!({"replies_armed": false}))
    .send()
    .await
    .unwrap();
    ts.thread(&aid2, 1, "@agent hello").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !d.path().join("args.txt").exists(),
        "replies not armed, no queue"
    );

    let bare = server(None, Duration::from_secs(10)).await;
    let (sid3, _) = codex_owner(&bare, Some("cx-5")).await;
    let sess: Value = bare
        .get_authed(&format!("/api/sessions/{sid3}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        sess["push"]["reason"],
        "codex is not on the daemon's PATH; native push disabled"
    );
    let push: Value = bare.get("/api/push").await.json().await.unwrap();
    assert_eq!(
        push,
        json!({"codex": {"available": false, "bin": null, "source": "not_found"}})
    );
    let claude = bare.register_session("claude", "c").await;
    let sess: Value = bare
        .get_authed(&format!("/api/sessions/{}", claude["id"].as_str().unwrap()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["tier"], Value::Null);
    let pi = bare.register_session("pi", "p").await;
    let sess: Value = bare
        .get_authed(&format!("/api/sessions/{}", pi["id"].as_str().unwrap()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["tier"], "inject");
}
