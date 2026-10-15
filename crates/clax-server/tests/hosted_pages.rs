//! A hosted daemon's pages (`clax_http::hosted`): `/setup` while setup is
//! open, the join and sign-in pages without a referrer, and the artifact
//! page's bootstrap for a guest of a page shared by link, served from a web
//! UI override (see `tests/web_dist.rs`).
#![cfg(debug_assertions)]

use crate::common::TestServer;
use clax_core::working::ManualClock;
use clax_server::routes::shell::set_web_dist;
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde_json::json;
use std::sync::Arc;

const ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/shell/artifact.html"
));

async fn dist() -> tokio::sync::MutexGuard<'static, ()> {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let guard = crate::web_dist_lock().await;
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("clax-hosted-pages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("artifact.html"), ENTRY).unwrap();
        std::fs::write(dir.join("index.html"), "<p>gallery</p>").unwrap();
        dir
    });
    set_web_dist(dir.clone());
    guard
}

#[tokio::test]
async fn the_sign_in_pages_load_without_a_referrer_and_setup_closes_once_done() {
    let _g = dist().await;
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some("s3cret")).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let r = ts.get("/setup").await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert_eq!(r.headers()["cache-control"], "no-store");
    for path in ["/join/K7QF-M2XD-9PNA", "/signin/cxl_x", "/signin"] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(
            r.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{path}"
        );
    }
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(ts.get("/setup").await.status(), 404);
}

#[tokio::test]
async fn a_guest_gets_a_shared_pages_bootstrap_and_no_other() {
    let _g = dist().await;
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some("s3cret")).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = r.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let shared = ts.publish("Shared", &[("index.html", "<h1>s</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let private = ts.publish("Private", &[("index.html", "<h1>p</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .put(format!("{}/api/hosted/artifacts/{shared}/link", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .json(&json!({"access": "commenter"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let page = ts.get(&format!("/a/{shared}")).await;
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(
        html.contains("id=\"clax-boot\"") && html.contains("Shared"),
        "{html}"
    );
    assert_eq!(ts.get(&format!("/a/{private}")).await.status(), 404);
    // The owner's browser gets every page, with the owner as its viewer.
    let r = ts
        .client
        .get(format!("{}/a/{private}", ts.base))
        .header(COOKIE, &owner)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("\"display_name\":\"Ada\""));
}

#[tokio::test]
async fn a_person_who_may_not_read_comments_gets_a_bootstrap_without_threads() {
    let _g = dist().await;
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some("s3cret")).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let cookie_of = |r: &reqwest::Response| {
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = cookie_of(&r);
    let plan = ts.publish("Plan", &[("index.html", "<h1>p</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", "secret words")
        .text("version", "1");
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{plan}/threads", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    // A person invited with `rights`, signed in in a browser of their own.
    let person = async |name: &str, rights: serde_json::Value| {
        let r = ts
            .client
            .post(format!("{}/api/people/invites", ts.base))
            .header(ORIGIN, &origin)
            .header(COOKIE, &owner)
            .json(&json!({"name": name, "rights": rights}))
            .send()
            .await
            .unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let r = ts
            .client
            .post(format!("{}/api/hosted/invites/{code}/accept", ts.base))
            .header(ORIGIN, &origin)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        cookie_of(&r)
    };
    let artifacts = json!({"type": "clax-artifacts", "actions": ["read"], "locations": [plan]});
    let sam = person("Sam", json!([artifacts])).await;
    let mia = person(
        "Mia",
        json!([artifacts, {"type": "clax-comments", "actions": ["read"], "locations": [plan]}]),
    )
    .await;
    let page = async |cookie: &str| {
        let r = ts
            .client
            .get(format!("{}/a/{plan}", ts.base))
            .header(COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.text().await.unwrap()
    };
    assert!(page(&mia).await.contains("secret words"));
    let html = page(&sam).await;
    assert!(html.contains("id=\"clax-boot\""), "{html}");
    assert!(html.contains("\"threads\":[]"), "{html}");
    assert!(!html.contains("secret words"), "{html}");
}
