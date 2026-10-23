//! A fork's assets on the Worker's content store (versions in the
//! stand-in's repositories, assets in a bucket), through the server's
//! routes as the Worker serves them: the fork copies the assets its page
//! names, under the same IDs, and its site serves them whatever the source
//! does after.

use std::sync::Arc;

use clax_content_git::GitContent;
use clax_testkit::TestServer;
use serde_json::{Value, json};

use super::{clock, committer, stand_in};
use crate::repos::api::DoHost;
use crate::repos::assets::{BucketAssets, MemObjects};

/// `(status, bytes)` of `path` on the site of artifact `aid`.
async fn site(ts: &TestServer, aid: &str, path: &str) -> (u16, Vec<u8>) {
    let res = ts.site_get(aid, path).await;
    (res.status().as_u16(), res.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn a_forks_page_loads_its_assets_from_its_own_site() {
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
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    let bytes = vec![137, 80, 78, 71, 9, 8, 7];
    let up = ts
        .upload_asset(&id, "logo.png", "image/png", bytes.clone())
        .await;
    let url = up["url"].as_str().unwrap().to_string();
    let asset_id = up["asset"]["id"].as_str().unwrap().to_string();
    let html = format!("<img src=\"{url}\">");
    let res = ts
        .post_json(
            &format!("/api/artifacts/{id}/versions"),
            json!({"if_version": 1,
                   "files": {"index.html": {"content": html, "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let res = ts
        .post_json(&format!("/api/artifacts/{id}/fork"), json!({}))
        .await;
    assert_eq!(res.status(), 201);
    let fid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, page) = site(&ts, &fid, "/").await;
    let page = String::from_utf8(page).unwrap();
    assert_eq!(status, 200);
    assert!(
        page.contains(&html),
        "the fork's page is unrewritten: {page}"
    );
    assert_eq!(site(&ts, &fid, &url).await, (200, bytes.clone()));
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(site(&ts, &id, &url).await.0, 404);
    assert_eq!(site(&ts, &fid, &url).await, (200, bytes.clone()));
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(site(&ts, &fid, &url).await, (200, bytes));
}
