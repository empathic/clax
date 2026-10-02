mod common;
use common::TestServer;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Registers a Grok session and returns its ID.
async fn grok_session(ts: &TestServer) -> String {
    let s = ts.register_session("grok", "g1").await;
    s["id"].as_str().unwrap().to_string()
}

/// Registers a Claude Code session and returns its ID.
async fn claude_session(ts: &TestServer) -> String {
    let s = ts.register_session("claude", "c1").await;
    s["id"].as_str().unwrap().to_string()
}

/// Publishes as `sid` (which watches with replies armed); returns the artifact ID.
async fn published(ts: &TestServer, sid: &str) -> String {
    let a = ts.publish_as(sid, "T", "<h2>x</h2>").await;
    a["artifact"]["id"].as_str().unwrap().to_string()
}

/// Opens a thread on `aid` and sends it to the agent; returns the thread ID.
async fn send_comment(ts: &TestServer, aid: &str) -> String {
    let t = ts.thread(aid, 1, "hello").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.send_thread(aid, &tid).await;
    tid
}

/// Publishes as `sid`, opens a thread and sends it to the agent; returns
/// (artifact ID, thread ID).
async fn sent_comment(ts: &TestServer, sid: &str) -> (String, String) {
    let aid = published(ts, sid).await;
    let tid = send_comment(ts, &aid).await;
    (aid, tid)
}

async fn notices(ts: &TestServer, sid: &str, wait: u64) -> Value {
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/notices?wait={wait}"))
        .await;
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

#[tokio::test]
async fn a_notice_points_at_the_comment_once_and_delivers_nothing() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let (aid, tid) = sent_comment(&ts, &sid).await;
    let v = notices(&ts, &sid, 0).await;
    assert_eq!(v["notices"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["notices"][0]["thread_id"], tid);
    assert_eq!(v["notices"][0]["artifact_id"], aid);
    let line = v["lines"][0].as_str().unwrap();
    assert!(
        line.starts_with("[clax] New comment on ") && line.contains(&aid) && !line.contains('\n'),
        "{line}"
    );
    assert!(
        !line.contains("hello"),
        "the line never carries the comment"
    );
    let again = notices(&ts, &sid, 0).await;
    assert!(again["notices"].as_array().unwrap().is_empty());
    // Still undelivered: the Stop hook's tier hands it over.
    let f: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(f["feedback"].as_array().unwrap().len(), 1, "{f}");
}

#[tokio::test]
async fn a_waiting_notices_poll_wakes_on_a_new_comment() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=10", ts.base)),
    );
    let waiter = tokio::spawn(async move {
        let body: Value = req.send().await.unwrap().json().await.unwrap();
        (Instant::now(), body)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tid = send_comment(&ts, &aid).await;
    let sent = Instant::now();
    let (answered, body) = waiter.await.unwrap();
    assert!(answered.saturating_duration_since(sent) < Duration::from_secs(2));
    assert_eq!(body["notices"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["notices"][0]["thread_id"], tid);
}

#[tokio::test]
async fn notices_stay_quiet_while_the_session_waits_for_feedback() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let wait = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/feedback?tier=wait&wait=5",
        ts.base
    )));
    let waiter = tokio::spawn(async move {
        let body: Value = wait.send().await.unwrap().json().await.unwrap();
        body
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let quiet = notices(&ts, &sid, 0).await;
    assert_eq!(quiet, json!({"notices": [], "lines": [], "waited_s": 0}));
    let tid = send_comment(&ts, &aid).await;
    let got = waiter.await.unwrap();
    assert_eq!(got["feedback"][0]["thread_id"], tid, "{got}");
    // Delivered by the wait: never announced afterwards.
    let after = notices(&ts, &sid, 0).await;
    assert!(after["notices"].as_array().unwrap().is_empty(), "{after}");
}

#[tokio::test]
async fn grok_push_reports_whether_a_follower_is_connected() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let push = |v: Value| v["push"].clone();
    let before: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(push(before.clone())["tier"], "monitor");
    assert_eq!(push(before.clone())["available"], false);
    assert!(
        push(before)["reason"]
            .as_str()
            .unwrap()
            .starts_with("no clax feedback follow is running")
    );
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=5", ts.base)),
    );
    let follower = tokio::spawn(async move { req.send().await.unwrap().status() });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let during: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(push(during.clone())["tier"], "monitor");
    assert_eq!(push(during.clone())["available"], true, "{during}");
    assert_eq!(push(during)["reason"], Value::Null);
    follower.abort();
}

#[tokio::test]
async fn notices_of_an_unknown_or_ended_session_are_errors() {
    let ts = TestServer::spawn().await;
    let missing = ts.get_authed("/api/sessions/nope/notices?wait=0").await;
    assert_eq!(missing.status(), 404);
    let sid = grok_session(&ts).await;
    let ended = ts
        .authed(
            ts.client
                .patch(format!("{}/api/sessions/{sid}", ts.base))
                .json(&json!({"ended": true})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 200);
    let started = Instant::now();
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/notices?wait=30"))
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "answered before any wait"
    );
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unknown_session", "{body}");
    let unauthed = ts.get(&format!("/api/sessions/{sid}/notices")).await;
    assert_eq!(unauthed.status(), 401);
}

#[tokio::test]
async fn claude_push_reports_a_notice_follower() {
    let ts = TestServer::spawn().await;
    let sid = claude_session(&ts).await;
    let push = |ts: &TestServer, sid: &str| {
        let path = format!("/api/sessions/{sid}");
        let req = ts.authed(ts.client.get(format!("{}{path}", ts.base)));
        async move { req.send().await.unwrap().json::<Value>().await.unwrap()["push"].clone() }
    };
    let idle = push(&ts, &sid).await;
    assert_eq!(idle["tier"], Value::Null);
    assert_eq!(idle["available"], false);
    assert!(
        idle["reason"]
            .as_str()
            .unwrap()
            .contains("--dangerously-load-development-channels plugin:clax@clax"),
        "{idle}"
    );
    // A notices poll in progress counts as a follower.
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=3", ts.base)),
    );
    let poll = tokio::spawn(async move { req.send().await.unwrap().status() });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let following = push(&ts, &sid).await;
    assert_eq!(
        following,
        json!({"tier": "notice", "available": true, "reason": null})
    );
    assert_eq!(poll.await.unwrap(), 200);
}
