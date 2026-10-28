//! `/` serves the gallery entry and `/a/…` the artifact entry, from a test
//! daemon serving a scratch web UI.
#![cfg(debug_assertions)]

use clax_testkit::TestServer;

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
    let ts = TestServer::spawn_with_web(dist.path()).await;
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

    // A lazy part of the bridge: read by the page beside it alone, and
    // immutable at its content-hashed name outside a debug build.
    let res = ts.get("/_clax/bridge/comment-abc.js").await;
    assert_eq!(res.status(), 200);
    assert!(!res.headers().contains_key("access-control-allow-origin"));
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

    // A part that is not there (an older bridge's, after an upgrade) is a
    // 404; a build manifest is never served.
    let gone = ts.get("/_clax/bridge/comment-old.js").await;
    assert_eq!(gone.status(), 404);
    assert!(!gone.headers().contains_key("access-control-allow-origin"));
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

/// The installed web app: the manifest and its icons under `/_clax/`, and
/// the service worker at `/sw.js` (scope `/`), each with its type, from
/// this machine and through a `[serve] trusted_hosts` name (`tailscale
/// serve`). The worker is revalidated on each use; none of them needs a
/// credential.
#[tokio::test]
async fn the_web_app_manifest_icons_and_service_worker_are_served() {
    const TRUSTED: &str = "phobos.tail556f5b.ts.net";
    let dist = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dist.path().join("_clax/app")).unwrap();
    std::fs::write(
        dist.path().join("_clax/app.webmanifest"),
        r#"{"name":"Clax","start_url":"/","display":"standalone"}"#,
    )
    .unwrap();
    std::fs::write(dist.path().join("_clax/sw.js"), "self.x = 1;").unwrap();
    std::fs::write(
        dist.path().join("_clax/app/apple-touch-icon.png"),
        b"\x89PNG\r\n\x1a\n",
    )
    .unwrap();
    let web = std::sync::Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let ts = TestServer::spawn_with(move |s| {
        s.web = web;
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
    })
    .await;
    for host in [None, Some(TRUSTED)] {
        let get = |p: &str| {
            let r = ts.client.get(format!("{}{p}", ts.base));
            match host {
                Some(h) => r.header("host", h),
                None => r,
            }
            .send()
        };
        let res = get("/_clax/app.webmanifest").await.unwrap();
        assert_eq!(res.status(), 200, "{host:?}");
        assert_eq!(
            res.headers()["content-type"],
            "application/manifest+json",
            "{host:?}"
        );
        let m: serde_json::Value = res.json().await.unwrap();
        assert_eq!(m["display"], "standalone");

        let res = get("/sw.js").await.unwrap();
        assert_eq!(res.status(), 200, "{host:?}");
        assert_eq!(res.headers()["content-type"], "text/javascript");
        assert_eq!(res.headers()["cache-control"], "no-cache");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        assert!(res.headers().contains_key("etag"));
        assert_eq!(res.text().await.unwrap(), "self.x = 1;");

        let res = get("/_clax/app/apple-touch-icon.png").await.unwrap();
        assert_eq!(res.status(), 200, "{host:?}");
        assert_eq!(res.headers()["content-type"], "image/png");
    }
    // Not built: 404, never another file.
    std::fs::remove_file(dist.path().join("_clax/sw.js")).unwrap();
    assert_eq!(ts.get("/sw.js").await.status(), 404);
}
