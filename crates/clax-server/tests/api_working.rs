use crate::common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    (ts, clock)
}

/// A claude session owning a one-version artifact with one sent thread.
async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "w1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>")
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn put(ts: &TestServer, sid: &str, aid: &str, body: Value) -> reqwest::Response {
    ts.authed(
        ts.client
            .put(format!("{}/api/sessions/{sid}/working/{aid}", ts.base)),
    )
    .json(&body)
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn setting_needs_the_token_and_reading_does_not() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    assert_eq!(
        ts.client
            .put(&url)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let res = put(
        &ts,
        &sid,
        &aid,
        json!({"thread_ids": [tid], "message": " Two\ncolumns "}),
    )
    .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["working"]["message"], "Two columns");
    assert_eq!(v["working"]["session_id"], sid.as_str());
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    let w = &public["working"][0];
    assert_eq!(w["harness"], "claude");
    assert!(w["agent"].as_str().unwrap().starts_with("a_"), "{w}");
    assert_eq!(w["thread_ids"], json!([tid]));
    assert!(!public.to_string().contains(&sid), "{public}");
    assert!(w.get("session_id").is_none());
    let mine = ts.get_authed(&format!("/api/sessions/{sid}/working")).await;
    assert_eq!(mine.status(), 200);
    assert_eq!(
        ts.get(&format!("/api/sessions/{sid}/working"))
            .await
            .status(),
        401
    );
}

#[tokio::test]
async fn artifact_views_carry_the_working_list_without_session_ids() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({"message": "Refactoring"})).await;
    let one: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(one["artifact"]["working"][0]["message"], "Refactoring");
    let all: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(all["artifacts"][0]["working"][0]["harness"], "claude");
    assert!(
        all["artifacts"][0]["working"][0]
            .get("session_id")
            .is_none()
    );
}

#[tokio::test]
async fn bad_requests_name_their_code() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let code = |r: Value| r["error"]["code"].as_str().unwrap().to_string();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": ["nope"]})).await;
    assert_eq!(r.status(), 400);
    assert_eq!(code(r.json().await.unwrap()), "invalid_args");
    let many: Vec<String> = (0..21).map(|_| clax_core::new_ulid()).collect();
    assert_eq!(
        code(
            put(&ts, &sid, &aid, json!({"thread_ids": many}))
                .await
                .json()
                .await
                .unwrap()
        ),
        "invalid_args"
    );
    let r = put(
        &ts,
        &sid,
        &aid,
        json!({"thread_ids": [clax_core::new_ulid()]}),
    )
    .await;
    assert_eq!(code(r.json().await.unwrap()), "unknown_thread");
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        ))
        .send()
        .await
        .unwrap();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": [tid]})).await;
    assert_eq!(code(r.json().await.unwrap()), "thread_not_open");
    let r = put(&ts, &sid, "7q3k9mzx2b4t", json!({})).await;
    assert_eq!(r.status(), 404);
    let r = put(&ts, &clax_core::new_ulid(), &aid, json!({})).await;
    assert_eq!(r.status(), 404);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(put(&ts, &sid, &aid, json!({})).await.json().await.unwrap()),
        "unknown_session"
    );
}

#[tokio::test]
async fn a_long_message_is_cut_and_flagged() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let v: Value = put(&ts, &sid, &aid, json!({"message": "m".repeat(300)}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["message_truncated"], true);
    assert_eq!(
        v["working"]["message"].as_str().unwrap().chars().count(),
        140
    );
}

#[tokio::test]
async fn delete_clears_one_record_or_its_threads() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    put(
        &ts,
        &sid,
        &aid,
        json!({"thread_ids": [tid], "message": "x"}),
    )
    .await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    let v: Value = ts
        .authed(ts.client.delete(format!("{url}?thread_ids={tid}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["cleared"], true);
    assert_eq!(v["working"], Value::Null, "its last thread went");
    put(&ts, &sid, &aid, json!({"message": "y"})).await;
    let v: Value = ts
        .authed(ts.client.delete(&url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["cleared"], true);
    let v: Value = ts
        .authed(ts.client.delete(&url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["cleared"], false);
}

#[tokio::test]
async fn renew_and_end_act_on_every_record_of_the_session() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({})).await;
    clock.advance(100);
    let v: Value = ts
        .post_json(&format!("/api/sessions/{sid}/working/renew"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["renewed"], 1);
    clock.advance(100);
    let w: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        w["working"].as_array().unwrap().len(),
        1,
        "renewed at 100 s, so alive at 200 s"
    );
    let v: Value = ts
        .post_json(&format!("/api/sessions/{sid}/working/end"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["cleared"], 1);
    let w: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["working"], json!([]));
}

#[tokio::test]
async fn expiry_is_hidden_at_once_and_announced_by_the_sweep() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    put(&ts, &sid, &aid, json!({"message": "x"})).await;
    let set = ev.next_named("working").await;
    assert_eq!(set["working"][0]["message"], "x");
    clock.advance(120);
    let w: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["working"], json!([]));
    clax_server::working::sweep_and_announce(&ts.store, &ts.working, &ts.events).await;
    let gone = ev.next_named("working").await;
    assert_eq!(
        gone,
        json!({"type": "working", "artifact_id": aid, "working": []})
    );
}

#[tokio::test]
async fn a_types_filter_drops_other_events() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events("?types=working").await;
    ts.thread(&aid, 1, "plain note").await;
    put(&ts, &sid, &aid, json!({})).await;
    let (name, data) = ev.next().await;
    assert_eq!(name, "working");
    assert_eq!(data["artifact_id"], aid.as_str());
}

#[tokio::test]
async fn the_roster_names_every_live_record_with_its_session_for_the_token_only() {
    let (ts, clock) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    put(
        &ts,
        &sid,
        &aid,
        json!({"thread_ids": [tid], "message": "Fixing"}),
    )
    .await;
    assert_eq!(ts.get("/api/working").await.status(), 401);
    let v: Value = ts.get_authed("/api/working").await.json().await.unwrap();
    let r = v["working"].as_array().unwrap();
    assert_eq!(r.len(), 1, "{v}");
    assert_eq!(r[0]["session_id"], sid.as_str());
    assert_eq!(r[0]["artifact_id"], aid.as_str());
    assert_eq!(r[0]["harness"], "claude");
    assert_eq!(r[0]["message"], "Fixing");
    assert_eq!(r[0]["thread_ids"], json!([tid]));
    assert!(r[0]["started_at"].is_string() && r[0]["expires_at"].is_string());
    clock.advance(121);
    let v: Value = ts.get_authed("/api/working").await.json().await.unwrap();
    assert_eq!(v["working"], json!([]), "a lapsed record leaves the roster");
}
