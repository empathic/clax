use crate::common;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn app_harness_registers_and_reuses_its_row() {
    let ts = TestServer::spawn().await;
    let a = ts.register_session("app", "install-1").await;
    assert_eq!(a["harness"], "app");
    let b = ts.register_session("app", "install-1").await;
    assert_eq!(a["id"], b["id"], "the install ID keys one live row");
}

#[tokio::test]
async fn unknown_harness_message_lists_app() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json("/api/sessions", json!({"harness": "zed", "cwd": "/w"}))
        .await;
    assert_eq!(res.status(), 400);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        v["error"]["message"],
        "harness must be one of claude, codex, grok, pi, app"
    );
}

#[tokio::test]
async fn sent_thread_reaches_the_app_session_by_its_wait_poll() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("app", "install-1").await;
    let sid = s["id"].as_str().unwrap();
    let p = ts.publish_as(sid, "Runs", "<p>runs</p>").await;
    let aid = p["artifact"]["id"].as_str().unwrap();
    let t = ts.thread(aid, 1, "make it blue").await;
    let tid = t["id"].as_str().unwrap();
    let sent = ts.send_thread(aid, tid).await;
    assert_eq!(
        sent["feedback_state"]["tier"], "wait",
        "a sent thread waits on the app's feedback poll: {sent}"
    );
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=wait&wait=5"))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["feedback"][0]["body"], "make it blue");
}

#[tokio::test]
async fn app_session_replies_and_resolves_as_the_agent() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("app", "install-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let p = ts.publish_as(&sid, "Runs", "<p>runs</p>").await;
    let aid = p["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "make it blue").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.send_thread(&aid, &tid).await;
    let reply = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "Done: blue now.", "author_kind": "agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 201, "{}", reply.text().await.unwrap());
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"as": "agent"}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["thread"]["status"], "resolved", "{v}");
    assert_eq!(v["thread"]["resolved_by"], "agent:app", "{v}");
    let last = v["thread"]["comments"].as_array().unwrap().last().unwrap();
    assert_eq!(last["body"], "Done: blue now.", "{v}");
    assert_eq!(last["author_kind"], "agent", "{v}");
    assert_eq!(last["via_harness"], "app", "{v}");
}

#[tokio::test]
async fn app_session_push_is_the_wait_tier() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("app", "install-1").await;
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{}", s["id"].as_str().unwrap()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["push"]["tier"], "wait");
    assert_eq!(v["push"]["available"], true);
    assert_eq!(v["push"]["reason"], Value::Null);
}
