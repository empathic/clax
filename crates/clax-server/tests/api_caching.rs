mod common;
use common::TestServer;

#[tokio::test]
async fn api_json_is_compressed_as_the_client_prefers_and_streams_never_are() {
    let ts = TestServer::spawn().await;
    for i in 0..12 {
        ts.publish(
            &format!("Artifact number {i} with a long enough title"),
            &[("index.html", "x")],
        )
        .await;
    }
    for (accept, want) in [("br", "br"), ("gzip", "gzip"), ("gzip, br", "br")] {
        let res = ts
            .client
            .get(format!("{}/api/artifacts", ts.base))
            .header("accept-encoding", accept)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["content-encoding"], want, "{accept}");
        assert!(
            res.headers()["vary"]
                .to_str()
                .unwrap()
                .contains("accept-encoding")
        );
    }
    // Without Accept-Encoding, plain.
    let res = ts.get("/api/artifacts").await;
    assert!(res.headers().get("content-encoding").is_none());
    // A small body is not worth it.
    let res = ts
        .client
        .get(format!("{}/api/viewers/me/attention", ts.base))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("content-encoding").is_none());
    // Event streams are never compressed.
    for path in ["/api/stream", "/api/events"] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("accept-encoding", "gzip, br")
            .send()
            .await
            .unwrap();
        assert_eq!(res.headers()["content-type"], "text/event-stream");
        assert!(res.headers().get("content-encoding").is_none(), "{path}");
    }
}

#[tokio::test]
async fn api_gets_carry_an_etag_and_answer_304_when_current() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let path = format!("{}/api/artifacts/{aid}/threads", ts.base);
    let res = ts.client.get(&path).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-cache");
    let tag = res.headers()["etag"].to_str().unwrap().to_string();
    let res = ts
        .client
        .get(&path)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
    assert_eq!(res.headers()["etag"], tag.as_str());
    assert!(res.bytes().await.unwrap().is_empty());
    ts.thread(aid, 1, "hello").await;
    let res = ts
        .client
        .get(&path)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "changed, so the old tag no longer matches"
    );
    assert_ne!(res.headers()["etag"], tag.as_str());
    // A route's own Cache-Control is kept.
    let v = ts.viewer(None).await;
    let res = ts
        .client
        .get(format!("{}/api/artifacts/{aid}", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["cache-control"], "private, no-cache");
    assert!(res.headers().contains_key("etag"));
}

#[tokio::test]
async fn hashed_shell_bundles_get_an_etag() {
    let ts = TestServer::spawn().await;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist/_clax/shell");
    // The largest bundle, well above the compression threshold.
    let Some(name) = std::fs::read_dir(dir).ok().and_then(|d| {
        d.filter_map(|e| {
            let e = e.ok()?;
            let n = e.file_name().into_string().ok()?;
            if !n.ends_with(".js") {
                return None;
            }
            Some((e.metadata().ok()?.len(), n))
        })
        .max()
        .map(|(_, n)| n)
    }) else {
        eprintln!("skipping: the web UI is not built");
        return;
    };
    let res = ts.get(&format!("/_clax/shell/{name}")).await;
    assert_eq!(res.status(), 200);
    // Immutable in a release build; a debug build reads from disk and revalidates.
    let want = if cfg!(debug_assertions) {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    assert_eq!(res.headers()["cache-control"], want);
    let tag = res.headers()["etag"].to_str().unwrap().to_string();
    let res = ts
        .client
        .get(format!("{}/_clax/shell/{name}", ts.base))
        .header("if-none-match", tag)
        .header("accept-encoding", "br")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
    let res = ts
        .client
        .get(format!("{}/_clax/shell/{name}", ts.base))
        .header("accept-encoding", "br")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-encoding"], "br");
}
