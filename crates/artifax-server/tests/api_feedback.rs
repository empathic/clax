mod common;
use common::TestServer;
use serde_json::Value;
use std::time::{Duration, Instant};

async fn sent_thread(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "h1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<h2>x</h2>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "hello").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn poll(ts: &TestServer, sid: &str, query: &str) -> Value {
    let res = ts
        .authed(
            ts.client
                .get(format!("{}/api/sessions/{sid}/feedback{query}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

#[tokio::test]
async fn long_poll_wakes_within_100_ms_of_a_send() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = sent_thread(&ts).await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/feedback?wait=5", ts.base)),
    );
    let waiter = tokio::spawn(async move {
        let body: Value = req.send().await.unwrap().json().await.unwrap();
        (Instant::now(), body)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    ts.send_thread(&aid, &tid).await;
    let sent = Instant::now();
    let (answered, body) = waiter.await.unwrap();
    assert!(answered.saturating_duration_since(sent) < Duration::from_millis(100));
    assert_eq!(body["feedback"].as_array().unwrap().len(), 1);
    assert!(
        body["text"].as_str().unwrap().starts_with(
            "[artifax] 1 comment sent to you:\n[artifax] Comment sent to you on \"T\""
        )
    );
    assert_eq!(body["feedback"][0]["thread_id"], tid);
    let again = poll(&ts, &sid, "?tier=piggyback").await;
    assert!(
        again["feedback"].as_array().unwrap().is_empty(),
        "the wait tier acknowledged it"
    );
}

#[tokio::test]
async fn a_wait_with_nothing_returns_empty_after_the_wait() {
    let ts = TestServer::spawn().await;
    let (sid, _aid, _tid) = sent_thread(&ts).await;
    let started = Instant::now();
    let body = poll(&ts, &sid, "?wait=1").await;
    assert!(started.elapsed() >= Duration::from_millis(950));
    assert_eq!(body["feedback"], serde_json::json!([]));
    assert_eq!(body["text"], Value::Null);
    assert_eq!(body["waited_s"], 1);
}

/// A long-poll whose client goes away while it waits is dropped with its
/// connection and takes nothing, so the row stays for the next tier. This
/// covers only a drop during the wait: a drop that lands after the wake, while
/// the take runs on the blocking pool, still marks the rows delivered (and, for
/// the in-band `wait` tier, acknowledged), and they are not resent.
#[tokio::test]
async fn abandoned_long_poll_marks_nothing() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = sent_thread(&ts).await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/feedback?wait=5", ts.base)),
    );
    let waiter = tokio::spawn(async move { req.send().await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    waiter.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    ts.send_thread(&aid, &tid).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let body = poll(&ts, &sid, "?tier=piggyback").await;
    assert_eq!(
        body["feedback"].as_array().unwrap().len(),
        1,
        "the dropped request took nothing"
    );
    assert_eq!(body["feedback"][0]["resent"], false);
}

#[tokio::test]
async fn tiers_gate_by_arming_and_resends_can_be_excluded() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = sent_thread(&ts).await;
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
        )
        .json(&serde_json::json!({"replies_armed": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    ts.send_thread(&aid, &tid).await;
    assert!(
        poll(&ts, &sid, "?tier=stop_hook").await["feedback"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        poll(&ts, &sid, "?tier=prompt_hook&resends=false").await["feedback"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn ack_acknowledges_threads() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = sent_thread(&ts).await;
    ts.send_thread(&aid, &tid).await;
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/feedback/ack", ts.base)),
        )
        .json(&serde_json::json!({"thread_ids": [tid]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["acknowledged"], 1);
    assert!(
        poll(&ts, &sid, "?tier=piggyback").await["feedback"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn ack_by_comment_ids_leaves_later_comments_pending() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = sent_thread(&ts).await;
    ts.send_thread(&aid, &tid).await;
    let view: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    let first = view["thread"]["comments"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let later = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .json(&serde_json::json!({"body": "and the footer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(later.status(), 201);
    let ack = |body: Value| {
        ts.authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/feedback/ack", ts.base)),
        )
        .json(&body)
        .send()
    };
    let res = ack(serde_json::json!({"comment_ids": [first]}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.json::<Value>().await.unwrap()["acknowledged"], 1);
    let left = poll(&ts, &sid, "?tier=piggyback").await;
    let bodies: Vec<&str> = left["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["and the footer"]);
    let res = ack(serde_json::json!({})).await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_json"
    );
}

#[tokio::test]
async fn feedback_route_errors() {
    let ts = TestServer::spawn().await;
    let (sid, _aid, _tid) = sent_thread(&ts).await;
    assert_eq!(
        ts.get(&format!("/api/sessions/{sid}/feedback"))
            .await
            .status(),
        401
    );
    let bad = ts
        .authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/feedback?tier=pigeon",
            ts.base
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_tier"
    );
    let missing = ts
        .authed(
            ts.client
                .get(format!("{}/api/sessions/nope/feedback", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn long_poll_is_exempt_from_the_request_timeout() {
    let ts = TestServer::spawn_with(|s| s.request_timeout = Duration::from_millis(200)).await;
    let (sid, _aid, _tid) = sent_thread(&ts).await;
    let body = poll(&ts, &sid, "?wait=1").await;
    assert_eq!(body["waited_s"], 1);
}

#[tokio::test]
async fn an_ended_session_cannot_poll_or_ack() {
    let ts = TestServer::spawn().await;
    let (sid, _aid, tid) = sent_thread(&ts).await;
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&serde_json::json!({"ended": true}))
        .send()
        .await
        .unwrap();
    let started = Instant::now();
    let res = ts
        .authed(
            ts.client
                .get(format!("{}/api/sessions/{sid}/feedback?wait=5", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "answered at once"
    );
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/feedback/ack", ts.base)),
        )
        .json(&serde_json::json!({"thread_ids": [tid]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
    let missing = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/nope/feedback/ack", ts.base)),
        )
        .json(&serde_json::json!({"thread_ids": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}
