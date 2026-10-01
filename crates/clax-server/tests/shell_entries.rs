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
    std::fs::create_dir_all(dist.path().join("_clax/bridge")).unwrap();
    std::fs::write(
        dist.path().join("_clax/bridge/comment-abc.js"),
        "export {};",
    )
    .unwrap();
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
    assert_eq!(ts.get("/").await.headers()["cache-control"], "no-cache");
    // No other page may frame either entry.
    for p in ["/", "/a/not-an-id", "/a/7q3k9mzx2b4t"] {
        let res = ts.get(p).await;
        assert_eq!(
            res.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{p}"
        );
        assert_eq!(res.headers()["x-frame-options"], "DENY", "{p}");
    }
    let res = ts.get("/a/7q3k9mzx2b4t").await;
    assert_eq!(res.headers()["cache-control"], "private, no-cache");
    assert!(res.headers().contains_key("etag"));

    // A lazy part of the bridge: readable from an opaque-origin sandbox, and
    // immutable at its content-hashed name outside a debug build.
    let res = ts.get("/_clax/bridge/comment-abc.js").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    assert_eq!(res.headers()["content-type"], "text/javascript");
    let cc = if cfg!(debug_assertions) {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    assert_eq!(res.headers()["cache-control"], cc);
    let etag = res.headers()["etag"].to_str().unwrap().to_string();
    assert_eq!(res.text().await.unwrap(), "export {};");
    let again = ts
        .client
        .get(format!("{}/_clax/bridge/comment-abc.js", ts.base))
        .header("if-none-match", etag)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 304);
    assert_eq!(again.headers()["access-control-allow-origin"], "*");

    // A part that is not there (an older bridge's, after an upgrade) is a
    // 404 a sandboxed page can read; a build manifest is never served.
    let gone = ts.get("/_clax/bridge/comment-old.js").await;
    assert_eq!(gone.status(), 404);
    assert_eq!(gone.headers()["access-control-allow-origin"], "*");
    std::fs::create_dir_all(dist.path().join("_clax/bridge/.vite")).unwrap();
    std::fs::write(dist.path().join("_clax/bridge/.vite/manifest.json"), "{}").unwrap();
    assert_eq!(
        ts.get("/_clax/bridge/.vite/manifest.json").await.status(),
        404
    );
    std::fs::write(dist.path().join("_clax/.hidden.js"), "x").unwrap();
    let res = ts.get("/_clax/.hidden.js").await;
    assert_eq!(res.status(), 404);
    assert!(!res.headers().contains_key("access-control-allow-origin"));
}
