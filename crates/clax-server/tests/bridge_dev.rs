//! A debug build reads the bridge from disk on every request (as `just dev`
//! rebuilds it under a running daemon): a rebuilt bridge gets a new URL in the
//! next page served, and the bridge is never immutable. The test serves the web
//! UI from a temporary copy ([`set_web_dist`]) and rewrites that, never the
//! real `web/dist`; it is its own test binary because the override is
//! process-wide.
#![cfg(debug_assertions)]

mod common;
use clax_server::routes::shell::{bridge_version, set_web_dist};
use common::TestServer;

#[tokio::test]
async fn a_rebuilt_bridge_gets_a_new_url_and_is_never_immutable() {
    let dist = tempfile::tempdir().unwrap();
    let real = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist"));
    std::fs::create_dir_all(dist.path().join("_clax")).unwrap();
    // A copy of the built bridge when there is one (read only), else a stand-in.
    let built =
        std::fs::read(real.join("_clax/bridge.js")).unwrap_or_else(|_| b"/* built */".to_vec());
    let path = dist.path().join("_clax/bridge.js");
    std::fs::write(&path, &built).unwrap();
    set_web_dist(dist.path().to_path_buf());
    std::fs::write(&path, "/* bridge A */").unwrap();

    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>i</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let page = |id: String| {
        let ts = &ts;
        async move { ts.get(&format!("/c/{id}/v/1/")).await.text().await.unwrap() }
    };
    let a = bridge_version();
    assert_eq!(a.len(), 12);
    assert!(
        page(id.to_string())
            .await
            .contains(&format!("bridge.js?v={a}\""))
    );
    let res = ts.get(&format!("/_clax/bridge.js?v={a}")).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.text().await.unwrap(), "/* bridge A */");

    std::fs::write(&path, "/* bridge B, rebuilt */").unwrap();
    let b = bridge_version();
    assert_ne!(a, b, "the version follows the bytes on disk");
    let html = page(id.to_string()).await;
    assert!(html.contains(&format!("bridge.js?v={b}\"")), "{html}");
    assert!(!html.contains(&a), "{html}");
    let res = ts.get(&format!("/_clax/bridge.js?v={b}")).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.text().await.unwrap(), "/* bridge B, rebuilt */");
}
