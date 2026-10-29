//! A hosted daemon's pages (`clax_server::hosted`): `/setup` while setup is
//! open, the join and sign-in pages without a referrer, and the artifact
//! page's bootstrap for a guest of a page shared by link, served from a
//! scratch web UI.
#![cfg(debug_assertions)]

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde_json::json;
use std::sync::Arc;

const ENTRY: &str = include_str!("../../../web/shell/artifact.html");

/// A scratch web UI: the artifact entry, a stand-in gallery and stand-in
/// legal documents.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.html"), ENTRY).unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>gallery</p>").unwrap();
    std::fs::write(dir.path().join("terms.html"), "<p>terms</p>").unwrap();
    std::fs::write(dir.path().join("privacy.html"), "<p>privacy</p>").unwrap();
    dir
}

/// A hosted test daemon whose setup takes `s3cret`, serving `dist`.
async fn spawn(clock: Arc<ManualClock>, dist: &tempfile::TempDir) -> TestServer {
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| s.web = web).await
}

#[tokio::test]
async fn the_sign_in_pages_load_without_a_referrer_and_setup_closes_once_done() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
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

/// A hosted test daemon serving `dist` and the quickstart release of
/// `clax_testkit::quickstart_tree("landing")`.
async fn spawn_with_quickstart(clock: Arc<ManualClock>, dist: &tempfile::TempDir) -> TestServer {
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let release =
        clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("landing"))
            .map(Arc::new);
    TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.quickstart = release;
    })
    .await
}

/// A client that reports redirects rather than following them.
fn unfollowed() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_signed_out_visitor_lands_on_the_bundled_quickstart() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn_with_quickstart(clock, &dist).await;
    let c = unfollowed();
    let r = c.get(format!("{}/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/start/");
    // The gallery's other pages still send a signed-out visitor to sign in.
    let r = c.get(format!("{}/inbox", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/signin");
    let r = c.get(format!("{}/start", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/start/");
    let r = c.get(format!("{}/start/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(r.headers()["content-security-policy"], "frame-ancestors 'none'");
    assert!(r.text().await.unwrap().contains("<p>landing</p>"));
    let r = c.get(format!("{}/start/style.css", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/css"));
    assert_eq!(r.text().await.unwrap(), "landing");
    let r = c.get(format!("{}/start/missing.css", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn the_landing_pages_reel_redirects_to_where_hosted_mode_serves_it() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let release =
        clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("landing"))
            .map(Arc::new);
    let reel = clax_http::hosted::Reel::new(
        "https://videos.example/r/downloads/default.mp4",
        Some("https://videos.example/r/thumbnails/thumbnail.jpg"),
    )
    .unwrap();
    let ts = TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.quickstart = release;
        s.hosted = s.hosted.take().map(|h| {
            Arc::new(
                Arc::try_unwrap(h)
                    .expect("hosted is not shared yet")
                    .with_reel(Some(reel)),
            )
        });
    })
    .await;
    let c = unfollowed();
    let r = c.get(format!("{}/start/reel.mp4", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], "https://videos.example/r/downloads/default.mp4");
    let r = c.get(format!("{}/start/reel-poster.jpg", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], "https://videos.example/r/thumbnails/thumbnail.jpg");
    let r = c.get(format!("{}/start/style.css", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn without_a_reel_elsewhere_the_landing_page_serves_none() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn_with_quickstart(clock, &dist).await;
    let c = unfollowed();
    for f in ["reel.mp4", "reel-poster.jpg"] {
        let r = c.get(format!("{}/start/{f}", ts.base)).send().await.unwrap();
        assert_eq!(r.status(), 404, "{f}");
    }
}

#[test]
fn a_reel_is_served_only_from_https_or_localhost() {
    use clax_http::hosted::Reel;
    assert!(Reel::new("https://videos.example/r.mp4", None).is_ok());
    assert!(Reel::new("http://localhost:8788/reel.mp4", Some("http://127.0.0.1:1/p.jpg")).is_ok());
    let e = Reel::new("https://videos.example/r.mp4", Some("http://videos.example/p.jpg")).unwrap_err();
    assert!(e.contains("http://videos.example/p.jpg"), "{e}");
    assert!(Reel::new("ftp://videos.example/r.mp4", None).is_err());
}

#[tokio::test]
async fn without_a_bundled_quickstart_a_signed_out_visitor_signs_in() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let c = unfollowed();
    let r = c.get(format!("{}/", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/signin");
    let r = c.get(format!("{}/start/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_guest_gets_a_shared_pages_bootstrap_and_no_other() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
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
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
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

#[tokio::test]
async fn the_bundled_documents_are_public_and_otherwise_absent() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock.clone(), &dist).await;
    for path in ["/terms", "/privacy"] {
        let r = ts.get(path).await;
        assert_ne!(r.status(), 200, "{path}");
    }
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let ts = TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.hosted = s.hosted.take().map(|h| {
            Arc::new(
                Arc::try_unwrap(h)
                    .expect("hosted is not shared yet")
                    .with_legal(Some(clax_core::config::Legal::Bundled)),
            )
        });
    })
    .await;
    for (path, body) in [("/terms", "<p>terms</p>"), ("/privacy", "<p>privacy</p>")] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(
            r.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{path}"
        );
        assert_eq!(r.text().await.unwrap(), body, "{path}");
    }
}
