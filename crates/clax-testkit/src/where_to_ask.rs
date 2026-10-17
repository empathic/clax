//! Helpers for the tests of where an agent asks its person (spec
//! 2026-10-16-ask-where-attention-is-design): a daemon on a manual clock,
//! the session's where-to-ask reply, a typed prompt, an owner's tab, a
//! session with a page.

use crate::TestServer;
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
    let r = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"artifact_id": aid, "state": "here", "tab": tab, "focused": focused, "idle_ms": idle_ms}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
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
