//! `GET /.well-known/clax` describes the instance: its name, origin, API
//! root, site domain, the path of its setup page for agents and, hosted,
//! the paths of its device sign-in. `GET /setup/feedback`, that page, is
//! Markdown with the instance's origin filled in, for any caller.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn discovery(ts: &TestServer) -> Value {
    let r = ts
        .client
        .get(format!("{}/.well-known/clax", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

#[tokio::test]
async fn discovery_names_a_hosted_instance_and_its_device_sign_in() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with_site_domain(clock.clone(), None, "sites.test").await;
    let v = discovery(&ts).await;
    assert_eq!(v["kind"], "hosted", "{v}");
    // The configured public origin (the harness reaches the daemon by
    // 127.0.0.1, its public origin is `localhost`).
    let port = ts.addr.port();
    assert_eq!(
        v["origin"],
        json!(format!("http://localhost:{port}")),
        "{v}"
    );
    assert_eq!(v["api"], "/api", "{v}");
    // No name set: the host.
    assert_eq!(v["name"], json!(format!("localhost:{port}")), "{v}");
    assert_eq!(v["site_domain"], "sites.test", "{v}");
    assert_eq!(v["setup"], json!({"feedback": "/setup/feedback"}), "{v}");
    assert!(
        v["sign_in"].as_array().unwrap().contains(&json!("device")),
        "{v}"
    );
    assert_eq!(
        v["device"],
        json!({
            "code": "/api/hosted/device/code",
            "approve": "/device",
            "token": "/api/hosted/device/token",
        }),
        "{v}"
    );

    let named = TestServer::spawn_hosted_with(clock, None, |s| {
        let h = Arc::try_unwrap(s.hosted.take().unwrap()).ok().unwrap();
        s.hosted = Some(Arc::new(h.with_name(Some("Studio Clax".into()))));
    })
    .await;
    let v = discovery(&named).await;
    assert_eq!(v["name"], "Studio Clax", "{v}");
    assert!(v.get("site_domain").is_none(), "{v}");
}

#[tokio::test]
async fn discovery_of_a_local_daemon_offers_no_device_sign_in() {
    let ts = TestServer::spawn().await;
    let v = discovery(&ts).await;
    assert_eq!(v["kind"], "local", "{v}");
    assert_eq!(v["api"], "/api", "{v}");
    let port = ts.addr.port();
    assert_eq!(
        v["origin"],
        json!(format!("http://localhost:{port}")),
        "{v}"
    );
    assert_eq!(v["name"], json!(format!("localhost:{port}")), "{v}");
    assert_eq!(v["setup"], json!({"feedback": "/setup/feedback"}), "{v}");
    assert!(v.get("device").is_none(), "{v}");
    assert!(v.get("site_domain").is_none(), "{v}");
    assert!(
        !v["sign_in"].as_array().unwrap().contains(&json!("device")),
        "{v}"
    );
}

/// `GET /setup/feedback` with no credential: the status, its
/// `Content-Type` and the page.
async fn setup_page(ts: &TestServer) -> (u16, String, String) {
    let r = ts
        .client
        .get(format!("{}/setup/feedback", ts.base))
        .header("host", format!("localhost:{}", ts.addr.port()))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let ty = r
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, ty, r.text().await.unwrap())
}

/// What the page must hold wherever it is served: the instance's origin
/// in its API calls and overlay, every placeholder filled, and no
/// credential of the instance's.
fn check_setup_page(page: &str, origin: &str, kind: &str, token: &str) {
    assert!(page.starts_with("# Set up Clax feedback"), "{page}");
    assert!(!page.contains("{{"), "a placeholder left: {page}");
    assert!(!page.contains("<!--"), "the editors' note left in: {page}");
    assert!(page.contains(&format!("(a `{kind}` Clax)")), "{page}");
    for call in [
        format!("--instance {origin}"),
        format!("{origin}/api/hosted/device/code"),
        format!("{origin}/api/queue-requests"),
        format!("instance: \"{origin}\""),
        format!("{origin}/mcp"),
    ] {
        assert!(page.contains(&call), "{call}: {page}");
    }
    assert!(!page.contains(token), "the daemon token: {page}");
    for prefix in ["cxq_", "cxa_", "cxr_"] {
        for (at, _) in page.match_indices(prefix) {
            let next = page[at + prefix.len()..].chars().next();
            assert!(
                !next.is_some_and(|c| c.is_ascii_alphanumeric()),
                "a credential-shaped {prefix} at {at}: {page}"
            );
        }
    }
}

#[tokio::test]
async fn the_setup_page_is_markdown_for_anyone_with_a_local_daemons_origin() {
    let ts = TestServer::spawn().await;
    let (status, ty, page) = setup_page(&ts).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(ty, "text/markdown; charset=utf-8");
    let origin = format!("http://localhost:{}", ts.addr.port());
    check_setup_page(&page, &origin, "local", &ts.token);
}

#[tokio::test]
async fn the_setup_page_of_a_hosted_instance_names_its_public_origin_for_anyone() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with(clock, None, |s| {
        let h = Arc::try_unwrap(s.hosted.take().unwrap()).ok().unwrap();
        s.hosted = Some(Arc::new(h.with_name(Some("Studio Clax".into()))));
    })
    .await;
    let (status, ty, page) = setup_page(&ts).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(ty, "text/markdown; charset=utf-8");
    let origin = format!("http://localhost:{}", ts.addr.port());
    check_setup_page(&page, &origin, "hosted", &ts.token);
    assert!(page.contains("**Studio Clax**"), "{page}");
}
