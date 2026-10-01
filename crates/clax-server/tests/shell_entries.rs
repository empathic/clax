//! `/` serves the gallery entry and `/a/…` the artifact entry. Its own test
//! binary: the web UI override is process-wide.
#![cfg(debug_assertions)]

mod common;
use clax_server::routes::shell::set_web_dist;
use common::TestServer;

#[tokio::test]
async fn each_route_gets_its_own_entry() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("index.html"), "<p>gallery</p>").unwrap();
    std::fs::write(dist.path().join("artifact.html"), "<p>artifact</p>").unwrap();
    set_web_dist(dist.path().to_path_buf());
    let ts = TestServer::spawn().await;
    let body = |p: &'static str| {
        let ts = &ts;
        async move { ts.get(p).await.text().await.unwrap() }
    };
    assert_eq!(body("/").await, "<p>gallery</p>");
    assert_eq!(body("/a/7q3k9mzx2b4t").await, "<p>artifact</p>");
    assert_eq!(body("/a/7q3k9mzx2b4t/").await, "<p>artifact</p>");
    assert_eq!(
        body("/a/7q3k9mzx2b4t/v/2/docs/x.html").await,
        "<p>artifact</p>"
    );
    assert_eq!(body("/a/not-an-id").await, "<p>gallery</p>");
    let res = ts.get("/a/7q3k9mzx2b4t").await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert!(res.headers().contains_key("etag"));
}
