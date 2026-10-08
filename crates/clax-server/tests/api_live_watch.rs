//! Scope watches over HTTP (spec 2026-10-05-chrome-overlay-design L2, §9.3):
//! a watch on a page URL covers that page and every live page below it, now
//! and as new ones are created, and its comments carry the live payload.
use crate::common;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use serde_json::{Value, json};

async fn live_watch(ts: &TestServer, sid: &str, url: &str) -> Value {
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

async fn comment(ts: &TestServer, cookie: &str, url: &str) -> Value {
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "P")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "body", "file": "index.html"}).to_string(),
        )
        .text("body", "Look")
        .text("pending", "[]")
        .text("snapshot", "<!doctype html><p>x")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json().await.unwrap()
}

async fn watches(ts: &TestServer, sid: &str) -> Vec<String> {
    let w: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/watches"))
        .await
        .json()
        .await
        .unwrap();
    w["watches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["artifact_id"].as_str().unwrap().to_string())
        .collect()
}

/// Ends session `sid` as the harness does.
async fn end_session(ts: &TestServer, sid: &str) {
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn scope_watch_covers_pages_created_later() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-1").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/").await;
    assert_eq!(w["live_watch"]["scope"], "http://localhost:5173/*");
    assert_eq!(w["live_watch"]["replies_armed"], true);
    assert_eq!(
        w["page"]["current_version"], 1,
        "the root page exists with its placeholder"
    );
    assert_eq!(w["covered"], json!([w["page"]["artifact_id"]]));
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    assert!(
        watches(&ts, sid).await.contains(&aid.to_string()),
        "the new page is watched"
    );
    let tid = c["thread"]["id"].as_str().unwrap();
    ts.send_thread(aid, tid).await;
    let fb: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=wait&wait=1"))
        .await
        .json()
        .await
        .unwrap();
    let text = fb["text"].as_str().unwrap();
    assert!(
        text.contains("live page http://localhost:5173/settings"),
        "{text}"
    );
    assert!(text.contains("Snapshot: "), "{text}");
    assert!(text.contains("(snapshot v1)"), "{text}");
    assert_eq!(
        fb["feedback"][0]["live"]["page_url"],
        "http://localhost:5173/settings"
    );
}

#[tokio::test]
async fn a_scope_on_a_path_covers_that_path_and_below_only() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-2").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/docs").await;
    assert_eq!(
        w["live_watch"]["scope"],
        "http://localhost:5173/docs and http://localhost:5173/docs/*"
    );
    let v = ts.viewer(Some("Alex")).await;
    let inside = comment(&ts, &v.cookie, "http://localhost:5173/docs/intro").await;
    let outside = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let sibling = comment(&ts, &v.cookie, "http://localhost:5173/docsx").await;
    let w = watches(&ts, sid).await;
    assert!(w.contains(&inside["page"]["artifact_id"].as_str().unwrap().to_string()));
    assert!(!w.contains(&outside["page"]["artifact_id"].as_str().unwrap().to_string()));
    assert!(!w.contains(&sibling["page"]["artifact_id"].as_str().unwrap().to_string()));
}

#[tokio::test]
async fn a_scope_watch_covers_the_pages_that_already_exist() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap().to_string();
    let s = ts.register_session("claude", "w-6").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/").await;
    let covered: Vec<String> = serde_json::from_value(w["covered"].clone()).unwrap();
    assert!(covered.contains(&aid), "{covered:?}");
    assert!(watches(&ts, sid).await.contains(&aid));
}

#[tokio::test]
async fn removing_a_scope_keeps_direct_watches() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-3").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/").await;
    let root = w["page"]["artifact_id"].as_str().unwrap().to_string();
    let v = ts.viewer(Some("Alex")).await;
    let other = comment(&ts, &v.cookie, "http://localhost:5173/x").await["page"]["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{other}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .authed(ts.client.delete(format!(
            "{}/api/sessions/{sid}/live-watches?url=http%3A%2F%2Flocalhost%3A5173%2F",
            ts.base
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let removed: Value = res.json().await.unwrap();
    assert_eq!(removed["removed"], json!([root]));
    assert_eq!(removed["page_url"], "http://localhost:5173/");
    let left = watches(&ts, sid).await;
    assert!(!left.contains(&root));
    assert!(left.contains(&other), "the direct watch stays");
}

#[tokio::test]
async fn ending_the_session_ends_its_scope_watches() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-4").await;
    let sid = s["id"].as_str().unwrap().to_string();
    live_watch(&ts, &sid, "http://localhost:5173/").await;
    end_session(&ts, &sid).await;
    let s2 = ts.register_session("claude", "w-5").await;
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/y").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    assert!(
        !watches(&ts, s2["id"].as_str().unwrap())
            .await
            .contains(&aid.to_string())
    );
    assert!(!watches(&ts, &sid).await.contains(&aid.to_string()));
}

#[tokio::test]
async fn a_scope_watch_refuses_the_daemons_own_pages_and_unknown_sessions() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-7").await;
    let sid = s["id"].as_str().unwrap();
    let own = format!("http://localhost:{}/", ts.addr.port());
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": own}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "own_origin"
    );
    let res = ts
        .client
        .put(format!("{}/api/sessions/{sid}/live-watches", ts.base))
        .json(&json!({"url": "http://localhost:5173/"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "the token is required");
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/nope/live-watches", ts.base)),
        )
        .json(&json!({"url": "http://localhost:5173/"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
}

#[tokio::test]
async fn thread_views_carry_the_page_url_and_snapshot_path_of_a_live_page() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/settings?tab=b").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    let tid = c["thread"]["id"].as_str().unwrap();
    let t: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        t["thread"]["page_url"],
        "http://localhost:5173/settings?tab=b"
    );
    let snap = t["thread"]["snapshot_path"].as_str().unwrap();
    assert!(snap.ends_with("/versions/1/index.html"), "{snap}");
    let anon: Value = ts
        .client
        .get(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        anon["thread"]["snapshot_path"].is_null(),
        "only with the token"
    );
}

#[tokio::test]
async fn a_scope_keeps_its_trailing_slash_and_its_origin() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-8").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/docs/").await;
    assert_eq!(w["live_watch"]["scope"], "http://localhost:5173/docs/*");
    let v = ts.viewer(Some("Alex")).await;
    let aid = |c: Value| c["page"]["artifact_id"].as_str().unwrap().to_string();
    let below = aid(comment(&ts, &v.cookie, "http://localhost:5173/docs/a").await);
    let bare = aid(comment(&ts, &v.cookie, "http://localhost:5173/docs").await);
    let port = aid(comment(&ts, &v.cookie, "http://localhost:3000/docs/a").await);
    let scheme = aid(comment(&ts, &v.cookie, "https://localhost:5173/docs/a").await);
    let w = watches(&ts, sid).await;
    assert!(w.contains(&below));
    assert!(!w.contains(&bare), "/docs/ does not cover /docs");
    assert!(!w.contains(&port), "another port is another origin");
    assert!(!w.contains(&scheme), "another scheme is another origin");
}

#[cfg(unix)]
#[tokio::test]
async fn a_scope_watch_whose_page_cannot_be_made_is_not_kept() {
    use std::os::unix::fs::PermissionsExt;
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-9").await;
    let sid = s["id"].as_str().unwrap();
    let artifacts = ts.home.root().join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let mode = std::fs::metadata(&artifacts).unwrap().permissions();
    std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o555)).unwrap();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/live-watches", ts.base)),
        )
        .json(&json!({"url": "http://localhost:5173/"}))
        .send()
        .await
        .unwrap();
    std::fs::set_permissions(&artifacts, mode).unwrap();
    assert!(res.status().is_server_error(), "{}", res.status());
    // No scope watch was left behind: a page made later under it is not
    // watched.
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    assert!(!watches(&ts, sid).await.contains(&aid.to_string()));
}

#[tokio::test]
async fn a_scope_watch_for_an_unknown_session_makes_no_page() {
    let ts = TestServer::spawn().await;
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/nope/live-watches", ts.base)),
        )
        .json(&json!({"url": "http://localhost:5173/fresh"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let found: Value = ts
        .get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2Ffresh")
        .await
        .json()
        .await
        .unwrap();
    assert!(found["page"].is_null());
}
