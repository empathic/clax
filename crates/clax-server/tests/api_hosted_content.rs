//! The server on a store whose content I/O needs an executor, as a
//! Durable Object's does: one connection ([`clax_store::Store::open_on`])
//! with [`Deferred`] content. The handlers that read or write content await
//! the store's `async` twins, so they work there; the synchronous methods
//! fail with `WouldBlock`. And on the daemon's pool, concurrent writers of
//! one artifact's content wait for each other rather than answer busy.

use crate::common;
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_testkit::{Deferred, FAKE_PNG};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

fn would_block<T: std::fmt::Debug>(r: clax_core::Result<T>) {
    let e = r.unwrap_err();
    assert!(
        matches!(e, clax_core::CoreError::WouldBlock { .. }),
        "{e:?}"
    );
}

async fn upload(ts: &TestServer, id: &str) -> Value {
    let part = reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
        .file_name("logo.png")
        .mime_str("image/png")
        .unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/assets", ts.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json().await.unwrap()
}

#[tokio::test]
async fn content_routes_await_a_content_store_that_needs_an_executor() {
    let ts = TestServer::spawn_on_one_connection(Arc::new(Deferred::new())).await;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<p>one</p>"),
                ("about.html", "<p>about</p>"),
                ("app.js", "let a = 1;"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    let aid = ArtifactId::parse(&id).unwrap();
    let res = ts
        .post_json(
            &format!("/api/artifacts/{id}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<p>two</p>"}}}),
        )
        .await;
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());

    // Pages wrapped, files raw, and the API's raw read.
    let page = ts.get(&format!("/c/{id}/v/2/")).await.text().await.unwrap();
    assert!(page.contains("<p>two</p>"), "{page}");
    let about = ts.get(&format!("/c/{id}/v/2/about.html")).await;
    assert!(about.text().await.unwrap().contains("<p>about</p>"));
    let js = ts.get(&format!("/c/{id}/v/2/app.js")).await;
    assert_eq!(js.text().await.unwrap(), "let a = 1;");
    let raw = ts
        .get_authed(&format!("/api/artifacts/{id}/versions/1/files/index.html"))
        .await;
    assert_eq!(raw.text().await.unwrap(), "<p>one</p>");

    // An asset's upload, bytes and delete.
    let asset = upload(&ts, &id).await;
    let blob = asset["url"].as_str().unwrap().to_string();
    let res = ts.get(&blob).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().as_ref(), FAKE_PNG);
    let asset_id = asset["asset"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.get(&blob).await.status(), 404);

    // A thread with a clip, its clip, and its delete.
    let res = ts
        .create_thread(&id, 2, "Tighten this.", Some(FAKE_PNG))
        .await;
    assert_eq!(res.status(), 201);
    let t: Value = res.json().await.unwrap();
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        t["thread"]["clip_path"],
        Value::Null,
        "no file on this machine"
    );
    let clip = ts.get(t["thread"]["clip_url"].as_str().unwrap()).await;
    assert_eq!(clip.bytes().await.unwrap().as_ref(), FAKE_PNG);
    // Content I/O inside a synchronous method fails rather than wait.
    would_block(ts.store.read_clip(&aid, &tid));
    would_block(ts.store.read_file(&aid, 2, "index.html"));
    let p = clax_core::publish::validate(
        serde_json::from_value(json!({
            "if_version": 2,
            "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}
        }))
        .unwrap(),
    )
    .unwrap();
    would_block(ts.store.publish_version(&AuditCtx::DAEMON, &aid, p, None));
    let named = ts.viewer(Some("Sam")).await;
    let res = ts
        .client
        .delete(format!("{}/api/artifacts/{id}/threads/{tid}", ts.base))
        .header("cookie", format!("clax_viewer={}", named.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    assert_eq!(
        ts.store.read_clip_async(&aid, &tid).await.unwrap(),
        None,
        "the clip went with its thread"
    );

    // The artifact's delete removes its content.
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(
        ts.store
            .content()
            .read_file(&aid, 1, "index.html")
            .await
            .unwrap(),
        None
    );
}

/// On the pool, publishes and clipped thread creations of one artifact at
/// once each wait for its content lock: none is refused as busy (503) or
/// fails; a publish that loses the race to the same version is a conflict.
#[tokio::test]
async fn concurrent_publishes_and_clipped_threads_of_one_artifact_wait_for_each_other() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>0</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    let publishes = (0..12).map(|i| {
        let (ts, id) = (&ts, &id);
        async move {
            // Each publishes on the version it reads, so some race.
            let a: Value = ts
                .get(&format!("/api/artifacts/{id}"))
                .await
                .json()
                .await
                .unwrap();
            let n = a["artifact"]["current_version"].as_u64().unwrap();
            ts.post_json(
                &format!("/api/artifacts/{id}/versions"),
                json!({"if_version": n, "files": {"index.html": {"content": format!("<p>{i}</p>")}}}),
            )
            .await
            .status()
            .as_u16()
        }
    });
    let threads = (0..12).map(|i| {
        let (ts, id) = (&ts, &id);
        async move {
            ts.create_thread(id, 1, &format!("Comment {i}"), Some(FAKE_PNG))
                .await
                .status()
                .as_u16()
        }
    });
    let (published, made) = tokio::join!(
        futures::future::join_all(publishes),
        futures::future::join_all(threads)
    );
    assert!(
        published.iter().all(|s| *s == 201 || *s == 409),
        "{published:?}"
    );
    assert!(published.contains(&201), "{published:?}");
    assert!(made.iter().all(|s| *s == 201), "{made:?}");
    let list: Value = ts
        .get_authed(&format!("/api/artifacts/{id}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["threads"].as_array().unwrap().len(), 12);
}
