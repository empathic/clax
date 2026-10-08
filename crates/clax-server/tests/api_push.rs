use crate::common;
use clax_server::push::{CodexPush, CodexSource};
use common::TestServer;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A fake `codex` in `dir` that records `CODEX_HOME` (`codex_home.txt`),
/// then its arguments (`args.txt`, which appears complete, last), both
/// beside itself, sleeps, and exits with `exit`.
fn fake_codex(dir: &Path, exit: i32, sleep_s: u32) -> PathBuf {
    clax_fake_exe::install(
        &dir.join("codex"),
        &format!(
            "#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '%s' \"${{CODEX_HOME:-}}\" > \"$d/codex_home.txt\"\nprintf '%s\\n' \"$@\" > \"$d/args.txt.tmp\"\nmv \"$d/args.txt.tmp\" \"$d/args.txt\"\nsleep {sleep_s}\nexit {exit}\n"
        ),
    )
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
            ..Default::default()
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

/// The arguments the fake `codex` recorded, once it has run (within 5 s).
async fn ran(dir: &Path) -> String {
    for _ in 0..100 {
        if let Ok(a) = std::fs::read_to_string(dir.join("args.txt")) {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the fake codex never ran");
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
    // The claim marks the row delivered before `codex queue` runs, and its
    // `delivered` event is published at the claim.
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "delivered" {
            assert_eq!(e["thread_id"], tid);
            break;
        }
    }
    let s = eventually(&ts, &aid, tid, "delivered").await;
    assert_eq!(s["tier"], "queue");
    let args = ran(d.path()).await;
    assert!(args.starts_with("queue\n--thread\ncx-1\n--message\n[clax] 1 comment sent to you:\n[clax] Comment sent to you on \"Pushed\""), "{args}");
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
        json!({"tier": "queue", "available": true, "reason": null, "codex_home": "/tmp/cxh",
            "last_error": null, "last_error_at": null})
    );
    let push: Value = ts.get("/api/push").await.json().await.unwrap();
    assert_eq!(
        (
            push["codex"]["available"].clone(),
            push["codex"]["source"].clone()
        ),
        (json!(true), json!("env"))
    );
    assert!(
        push["codex"].get("bin").is_none(),
        "the daemon's codex path needs the token: {push}"
    );
    let authed: Value = ts.get_authed("/api/push").await.json().await.unwrap();
    assert_eq!(
        authed["codex"]["bin"],
        d.path().join("codex").to_string_lossy().as_ref()
    );
}

/// A non-zero `codex queue` exit never means the session exited (measured,
/// docs/contract.md): the rows go back to the in-band tiers, the session stays
/// live, and the failure is recorded on its push state.
#[tokio::test]
async fn non_zero_exit_releases_the_rows_and_keeps_the_session() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 1, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, Some("cx-2")).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let t = ts.thread(&aid, 1, "@agent anyone?").await;
    let tid = t["id"].as_str().unwrap();
    loop {
        let e = ev.next_named("feedback_state").await;
        assert_ne!(e["state"], "agent_ended", "{e}");
        if e["state"] == "sent" && e["tier"] == "stop_hook" {
            assert_eq!(e["thread_id"], tid);
            break;
        }
    }
    let s = state_of(&ts, &aid, tid).await;
    assert_eq!(
        (s["state"].as_str(), s["tier"].as_str()),
        (Some("sent"), Some("stop_hook")),
        "released, waiting on the in-band tiers"
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
    assert_eq!(sess["push"]["available"], true);
    assert_eq!(sess["push"]["last_error"], "codex queue exited with code 1");
    assert!(sess["push"]["last_error_at"].is_string());
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["watches"].as_array().unwrap().len(), 1, "the watch stays");
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
    assert_eq!(
        fb["feedback"][0]["thread_id"], tid,
        "the same session gets the row in-band"
    );
}

#[tokio::test]
async fn missing_binary_and_timeouts_release_rows_for_the_other_tiers() {
    let slow = tempfile::tempdir().unwrap();
    for (bin, timeout, ran) in [
        (
            PathBuf::from("/nonexistent/codex"),
            Duration::from_secs(10),
            None,
        ),
        (
            fake_codex(slow.path(), 0, 5),
            Duration::from_secs(2),
            Some(slow.path().join("args.txt")),
        ),
    ] {
        let ts = server(Some(bin), timeout).await;
        let (sid, aid) = codex_owner(&ts, Some("cx-3")).await;
        let mut ev = ts.events(&format!("?artifact={aid}")).await;
        let t = ts.thread(&aid, 1, "@agent please").await;
        let tid = t["id"].as_str().unwrap();
        // Released and marked push-failed: waiting on the in-band tiers, not on Codex.
        loop {
            let e = ev.next_named("feedback_state").await;
            if e["state"] == "sent" && e["tier"] == "stop_hook" {
                break;
            }
        }
        let s = state_of(&ts, &aid, tid).await;
        assert_eq!(
            (s["state"].as_str(), s["tier"].as_str()),
            (Some("sent"), Some("stop_hook")),
            "released, waiting on the in-band tiers"
        );
        if let Some(args) = ran {
            assert!(args.exists(), "the slow fake ran before it timed out");
        }
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
        let err = sess["push"]["last_error"].as_str().unwrap_or_default();
        assert!(
            err.starts_with("codex queue timed out")
                || err.starts_with("codex queue could not run"),
            "{err}"
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
        json!({"codex": {"available": false, "source": "not_found", "reason": "codex is not on the daemon's PATH; native push disabled"}})
    );
    let authed: Value = bare.get_authed("/api/push").await.json().await.unwrap();
    assert_eq!(authed["codex"]["bin"], Value::Null);
    assert_eq!(authed["codex"]["source"], "not_found");
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

#[tokio::test]
async fn a_session_in_wait_for_feedback_gets_the_rows_in_band_not_by_queue() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, Some("cx-6")).await;
    let poll = ts
        .authed(
            ts.client
                .get(format!("{}/api/sessions/{sid}/feedback?wait=10", ts.base)),
        )
        .send();
    let poll = tokio::spawn(poll);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = ts.thread(&aid, 1, "@agent while you wait").await;
    let tid = t["id"].as_str().unwrap();
    let fb: Value = poll.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(fb["feedback"][0]["thread_id"], tid, "{fb}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !d.path().join("args.txt").exists(),
        "a waiting session is not queued to"
    );
    let s = state_of(&ts, &aid, tid).await;
    assert_eq!(
        (s["state"].as_str(), s["tier"].as_str()),
        (Some("acknowledged"), Some("wait")),
        "{s}"
    );
}

#[tokio::test]
async fn without_a_recorded_codex_home_codex_inherits_the_daemons() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "codex", "parent_pid": 4244, "harness_session_id": "cx-7", "cwd": "/w"}),
        )
        .await;
    let sid = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts.publish_as(&sid, "Inherited", "<h2>Goals</h2>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    ts.thread(&aid, 1, "@agent hi").await;
    loop {
        if ev.next_named("feedback_state").await["state"] == "delivered" {
            break;
        }
    }
    ran(d.path()).await;
    assert_eq!(
        std::fs::read_to_string(d.path().join("codex_home.txt")).unwrap(),
        std::env::var("CODEX_HOME").unwrap_or_default(),
        "no CODEX_HOME is set for the child; it inherits the daemon's"
    );
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["codex_home"], Value::Null);
}

#[tokio::test]
async fn codex_home_is_ignored_for_other_harnesses_and_kept_on_a_rejoin_without_it() {
    let ts = server(None, Duration::from_secs(10)).await;
    let join = |hsid: &'static str, harness: &'static str, home: Option<&'static str>| {
        let mut body = json!({"harness": harness, "parent_pid": 4245, "harness_session_id": hsid, "cwd": "/w"});
        if let Some(h) = home {
            body["codex_home"] = json!(h);
        }
        ts.post_json("/api/sessions/join", body)
    };
    let res = join("cl-1", "claude", Some("/tmp/nope")).await;
    let sid = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = clax_core::Store::open(&ts.home).unwrap();
    assert_eq!(store.codex_home(&sid).unwrap(), None, "not a Codex session");
    let res = join("cx-8", "codex", Some("/tmp/cxh8")).await;
    let sid = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let again = join("cx-8", "codex", None).await;
    assert_eq!(
        again.json::<Value>().await.unwrap()["session"]["id"],
        sid.as_str()
    );
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["codex_home"], "/tmp/cxh8");
}

#[tokio::test]
async fn a_poll_the_client_abandoned_no_longer_holds_off_queue() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (sid, aid) = codex_owner(&ts, Some("cx-9")).await;
    let poll = tokio::spawn(
        ts.authed(
            ts.client
                .get(format!("{}/api/sessions/{sid}/feedback?wait=10", ts.base)),
        )
        .send(),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    poll.abort();
    let _ = poll.await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    ts.thread(&aid, 1, "@agent after the poll").await;
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "delivered" {
            assert_eq!(e["tier"], "queue");
            break;
        }
    }
    ran(d.path()).await;
}

/// A claim made by a dispatch that no `thread` event follows (here a watch
/// retargeting an untargeted row) is announced at claim time, not only when
/// `codex queue` settles.
#[tokio::test]
async fn a_queue_claim_is_announced_before_codex_queue_finishes() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 30)), Duration::from_secs(60)).await;
    let a = ts
        .publish("Unowned", &[("index.html", "<h2>Goals</h2>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent anyone?").await;
    let tid = t["id"].as_str().unwrap();
    assert_eq!(
        t["feedback_state"]["state"], "agent_ended",
        "no target yet: {t}"
    );
    let res = ts.post_json("/api/sessions/join", json!({"harness": "codex", "parent_pid": 4244, "harness_session_id": "cx-m1", "cwd": "/w"})).await;
    let sid = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let started = std::time::Instant::now();
    let put = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success());
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "delivered" {
            assert_eq!(
                (e["thread_id"].as_str(), e["tier"].as_str()),
                (Some(tid), Some("queue"))
            );
            break;
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "announced at the claim, before the 30 s fake exits ({:?})",
        started.elapsed()
    );
}

async fn working_on(ts: &TestServer, aid: &str) -> Vec<Value> {
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    v["working"].as_array().unwrap().clone()
}

#[tokio::test]
async fn a_queue_that_exits_0_marks_the_session_working() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (_sid, aid) = codex_owner(&ts, Some("cx-w1")).await;
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    let tid = t["id"].as_str().unwrap();
    ran(d.path()).await;
    for _ in 0..100 {
        let w = working_on(&ts, &aid).await;
        if !w.is_empty() {
            assert_eq!(w.len(), 1, "{w:?}");
            assert_eq!(w[0]["harness"], "codex");
            assert_eq!(w[0]["thread_ids"], json!([tid]));
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the queued session was never marked working");
}

#[tokio::test]
async fn a_failed_queue_marks_nothing() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 1, 0)), Duration::from_secs(10)).await;
    let (_sid, aid) = codex_owner(&ts, Some("cx-w2")).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let t = ts.thread(&aid, 1, "@agent anyone?").await;
    let tid = t["id"].as_str().unwrap();
    // The release is announced as the failed queue settles.
    loop {
        let e = ev.next_named("feedback_state").await;
        if e["state"] == "sent" && e["tier"] == "stop_hook" {
            assert_eq!(e["thread_id"], tid);
            break;
        }
    }
    assert!(working_on(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn a_batch_reaches_codex_as_one_queued_message_led_by_its_note() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (_sid, aid) = codex_owner(&ts, Some("cx-batch")).await;
    let a = ts.thread(&aid, 1, "one").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.thread(&aid, 1, "two").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .json(&json!({"thread_ids": [a, b], "note": "Both, please"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let args = ran(d.path()).await;
    assert!(args.starts_with("queue\n--thread\ncx-batch\n--message\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Pushed\", sent together by Viewer. Note: \"Both, please\"\n"), "{args}");
    assert_eq!(
        args.matches("[clax] Comment sent to you").count(),
        2,
        "one message holds both"
    );
}
