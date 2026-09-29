mod common;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn publishing_watches_armed_and_republishing_keeps_the_arming() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "h1").await;
    let sid = s["id"].as_str().unwrap();
    let a = ts.publish_as(sid, "T", "<p>1</p>").await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["watches"][0]["artifact_id"], aid);
    assert_eq!(w["watches"][0]["replies_armed"], true);
    ts.authed(
        ts.client
            .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
    )
    .json(&json!({"replies_armed": false}))
    .send()
    .await
    .unwrap();
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-artifax-session", sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["watches"][0]["replies_armed"], false);
}

#[tokio::test]
async fn watch_routes_need_the_token_and_delete_removes() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("codex", "c1").await;
    let sid = s["id"].as_str().unwrap();
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let url = format!("{}/api/sessions/{sid}/watches/{aid}", ts.base);
    assert_eq!(ts.client.put(&url).send().await.unwrap().status(), 401);
    let w: Value = ts
        .authed(ts.client.put(&url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        w["watch"]["replies_armed"], true,
        "an empty body arms replies"
    );
    assert_eq!(
        ts.authed(ts.client.delete(&url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        ts.get(&format!("/api/sessions/{sid}/watches"))
            .await
            .status(),
        401,
        "session reads need the token"
    );
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert!(w["watches"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn untargeted_feedback_goes_to_the_next_publisher_and_session_end_releases() {
    let ts = TestServer::spawn().await;
    let s1 = ts.register_session("claude", "one").await;
    let sid1 = s1["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid1, "T", "<p>1</p>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{sid1}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid1}/watches"))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        w["watches"].as_array().unwrap().is_empty(),
        "ending a session drops its watches"
    );
    let t = ts.thread(&aid, 1, "@agent anyone?").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let fs = ev.next_named("feedback_state").await;
    assert_eq!(
        (fs["thread_id"].as_str(), fs["state"].as_str()),
        (Some(tid.as_str()), Some("agent_ended"))
    );
    let s2 = ts.register_session("claude", "two").await;
    let sid2 = s2["id"].as_str().unwrap().to_string();
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-artifax-session", &sid2)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(ev.next_named("feedback_state").await["state"], "sent");
    let fb: Value = ts
        .authed(ts.client.get(format!(
            "{}/api/sessions/{sid2}/feedback?tier=piggyback",
            ts.base
        )))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fb["feedback"][0]["thread_id"], tid);
}
