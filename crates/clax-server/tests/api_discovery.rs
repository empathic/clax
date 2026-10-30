//! `GET /.well-known/clax` describes the instance: its name, origin, API
//! root, site domain and, hosted, the paths of its device sign-in.

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
    assert!(v.get("device").is_none(), "{v}");
    assert!(v.get("site_domain").is_none(), "{v}");
    assert!(
        !v["sign_in"].as_array().unwrap().contains(&json!("device")),
        "{v}"
    );
}
