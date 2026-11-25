//! Byte ranges of a version's file and of an asset on the Worker's content
//! store (versions in the stand-in's repositories, assets in a bucket),
//! through the server's site routes as the Worker serves them.

use std::sync::Arc;

use clax_content_git::GitContent;
use clax_testkit::TestServer;
use serde_json::json;

use super::{clock, committer, stand_in};
use crate::repos::api::DoHost;
use crate::repos::assets::{BucketAssets, MemObjects};

/// `(status, Content-Range, body)` of `path` on `aid`'s site for `range`.
async fn ranged(ts: &TestServer, aid: &str, path: &str, range: &str) -> (u16, String, Vec<u8>) {
    let res = ts
        .client
        .get(format!("{}{path}", ts.base))
        .header("host", ts.site_host(aid))
        .header("cookie", ts.site_cookie(aid).await)
        .header("range", range)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let content_range = res
        .headers()
        .get("content-range")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, content_range, res.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn a_version_file_and_an_asset_answer_a_byte_range() {
    use base64::Engine;
    let content = GitContent::new(
        DoHost {
            transport: stand_in(clock()),
        },
        BucketAssets {
            objects: MemObjects::new(1000),
        },
        committer(),
        clock(),
    );
    let ts = TestServer::spawn_on_one_connection_with(Arc::new(content), |_| {}).await;
    let bytes: Vec<u8> = (0..600u32).map(|i| (i % 251) as u8).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "V", "files": {
                "index.html": {"content": "<video src=clip.mp4>", "encoding": "utf8"},
                "clip.mp4": {"content": b64, "encoding": "base64"},
            }}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let created: serde_json::Value = res.json().await.unwrap();
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    let up = ts
        .upload_asset(&id, "clip.mp4", "video/mp4", bytes.clone())
        .await;
    let url = up["url"].as_str().unwrap().to_string();
    for path in ["/clip.mp4", "/_clax/v/1/clip.mp4", url.as_str()] {
        assert_eq!(
            ranged(&ts, &id, path, "bytes=100-149").await,
            (206, "bytes 100-149/600".into(), bytes[100..150].to_vec()),
            "{path}"
        );
        assert_eq!(
            ranged(&ts, &id, path, "bytes=-5").await,
            (206, "bytes 595-599/600".into(), bytes[595..].to_vec()),
            "{path}"
        );
        assert_eq!(
            ranged(&ts, &id, path, "bytes=600-").await,
            (416, "bytes */600".into(), Vec::new()),
            "{path}"
        );
    }
}
