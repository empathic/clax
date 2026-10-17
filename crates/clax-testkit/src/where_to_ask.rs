//! Helpers for the tests of where an agent asks its person (spec
//! 2026-10-16-ask-where-attention-is-design): a daemon on a manual clock,
//! the session's where-to-ask reply, a typed prompt, an owner's tab, a
//! session with a page.

use crate::{EventReader, TestClient, TestServer};
use clax_core::working::ManualClock;
use serde_json::{Value, json};
use std::sync::Arc;

/// A test daemon whose every clock reads one manual clock, at
/// 2026-10-16T10:00:00Z.
pub async fn clocked() -> (Arc<ManualClock>, TestServer) {
    let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
    (c.clone(), TestServer::spawn_with_clock(c).await)
}

/// `GET /api/sessions/<sid>/where-to-ask` with the owner's token: the whole
/// reply, `{where_to_ask, signals}`.
pub async fn where_to_ask_reply(ts: &TestServer, sid: &str) -> Value {
    let r = ts
        .get_authed(&format!("/api/sessions/{sid}/where-to-ask"))
        .await;
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()
}

/// `POST /api/sessions/<sid>/terminal`: the person typed a prompt now.
pub async fn typed(ts: &TestServer, sid: &str) -> Value {
    let r = ts
        .post_json(&format!("/api/sessions/{sid}/terminal"), json!({}))
        .await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

/// A tab of the owner's on `aid` reports here, focused or not, with its
/// last input `idle_ms` before the report.
pub async fn owner_tab(ts: &TestServer, aid: &str, tab: &str, focused: bool, idle_ms: u64) {
    // The owner's viewer row exists from here on, as a browser of theirs makes it.
    ts.owner_public_id().await;
    tab_as(ts, &ts.owner_cookie(), aid, tab, focused, idle_ms).await;
}

/// [`owner_tab`] for the browser holding `cookie` (a `Cookie` header's
/// value).
pub async fn tab_as(
    ts: &TestServer,
    cookie: &str,
    aid: &str,
    tab: &str,
    focused: bool,
    idle_ms: u64,
) {
    let r = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", cookie)
        .json(&json!({"artifact_id": aid, "state": "here", "tab": tab, "focused": focused, "idle_ms": idle_ms}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

/// The `Cookie` header of a browser of the person whose agent is `p`.
pub fn browser_of(ts: &TestServer, p: &TestClient) -> String {
    let v = ts.store.viewer_of_grant(&p.grant_id()).unwrap();
    format!("clax_viewer={}", ts.browser_cookie(&v.id))
}

/// Opens `/api/stream` as the browser holding `cookie` and subscribes it
/// to `topics`, as [`TestServer::stream_as_owner`] does for the owner.
pub async fn stream_as(ts: &TestServer, cookie: &str, topics: &[&str]) -> EventReader {
    let res = ts
        .client
        .get(format!("{}/api/stream", ts.base))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!("{}/api/stream/{id}", ts.base))
        .header("cookie", cookie)
        .json(&json!({ "subscribe": topics }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    events
}

/// A live claude session of the person whose agent is `p`, which has
/// published a page: `(sid, aid)`.
pub async fn person_paged(p: &TestClient, hsid: &str) -> (String, String) {
    let sid = p.register("claude", hsid).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = p
        .agent(
            "POST",
            "/api/artifacts",
            Some(&sid),
            Some(json!({"title": "Sam's board", "files": {"index.html": {"content": "<main><h2>Goals</h2></main>", "encoding": "utf8"}}})),
        )
        .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let a: Value = r.json().await.unwrap();
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}

/// `GET /api/sessions/<sid>/where-to-ask` as the session's agent `p`.
pub async fn where_to_ask_as(p: &TestClient, sid: &str) -> Value {
    let r = p
        .agent(
            "GET",
            &format!("/api/sessions/{sid}/where-to-ask"),
            Some(sid),
            None,
        )
        .await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

/// `POST /api/sessions/<sid>/terminal` as the session's agent `p`.
pub async fn typed_as(p: &TestClient, sid: &str) {
    let r = p
        .agent(
            "POST",
            &format!("/api/sessions/{sid}/terminal"),
            Some(sid),
            Some(json!({})),
        )
        .await;
    assert_eq!(r.status(), 200);
}

/// A clocked daemon that may invite people ([`clocked`] on `0.0.0.0`) with
/// Sam connected (`clax:full`).
pub async fn clocked_with_sam() -> (Arc<ManualClock>, TestServer, TestClient) {
    let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), c.clone()).await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    (c, ts, sam)
}

/// A live claude session that has published a page: `(sid, aid)`.
pub async fn paged(ts: &TestServer, hsid: &str) -> (String, String) {
    let sid = ts.register_session("claude", hsid).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts
        .publish_as(&sid, "Board", "<main><h2>Goals</h2></main>")
        .await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}
