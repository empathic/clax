use crate::common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    (
        TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await,
        clock,
    )
}

async fn setup(ts: &TestServer, harness: &str) -> (String, String, String) {
    let s = ts
        .register_session(harness, &format!("{harness}-auto"))
        .await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>")
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn working(ts: &TestServer, aid: &str) -> Vec<Value> {
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    v["working"].as_array().unwrap().clone()
}

async fn take(ts: &TestServer, sid: &str, tier: &str) -> Value {
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}"))
        .await
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn each_delivering_tier_marks_the_session_working_on_the_thread() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait", "inject"] {
        let (ts, _) = server().await;
        let (sid, aid, tid) = setup(&ts, if tier == "inject" { "pi" } else { "claude" }).await;
        assert!(working(&ts, &aid).await.is_empty(), "{tier}");
        let got = take(&ts, &sid, tier).await;
        assert_eq!(got["feedback"].as_array().unwrap().len(), 1, "{tier}");
        let w = working(&ts, &aid).await;
        assert_eq!(w.len(), 1, "{tier}");
        assert_eq!(w[0]["thread_ids"], json!([tid]), "{tier}");
    }
}

#[tokio::test]
async fn an_empty_hook_or_tool_take_renews_but_a_heartbeat_and_a_wait_poll_do_not() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"heartbeat": true}))
        .send()
        .await
        .unwrap();
    take(&ts, &sid, "wait").await;
    clock.advance(20);
    assert!(
        working(&ts, &aid).await.is_empty(),
        "neither the heartbeat nor the wait poll renewed"
    );
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    take(&ts, &sid, "stop_hook").await;
    clock.advance(100);
    assert_eq!(
        working(&ts, &aid).await.len(),
        1,
        "the empty stop_hook take renewed at 100 s"
    );
}

#[tokio::test]
async fn the_agent_reply_to_the_last_named_thread_clears() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "Done.", "author_kind": "agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(ev.next_named("working").await["working"], json!([]));
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn an_agent_resolve_and_a_viewer_resolve_take_the_thread_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.post(format!(
        "{}/api/artifacts/{aid}/threads/{tid}/resolve",
        ts.base
    )))
    .header("x-clax-session", &sid)
    .json(&json!({"as": "agent"}))
    .send()
    .await
    .unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let t2 = ts.thread(&aid, 1, "@agent again").await;
    take(&ts, &sid, "piggyback").await;
    assert_eq!(working(&ts, &aid).await.len(), 1);
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/resolve",
            ts.base,
            t2["id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn a_publish_by_the_session_clears_its_record_on_that_artifact() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn session_end_and_artifact_delete_clear() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events("?types=working").await;
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(ev.next_named("working").await["working"], json!([]));
}

#[tokio::test]
async fn a_thread_delete_takes_it_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let named = ts.viewer(Some("Alex")).await;
    let res = ts
        .client
        .delete(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base))
        .header("cookie", format!("clax_viewer={}", named.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(working(&ts, &aid).await.is_empty());
}
