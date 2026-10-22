//! Each artifact's site host (`<id>.localhost:<port>` on a local daemon)
//! serves the current version at its root and Clax's own paths under
//! `/_clax/`; the public host serves no content.
use clax_testkit::TestServer;
use serde_json::{Value, json};

/// A new version of `id` from `index.html` alone; its number.
async fn republish(ts: &TestServer, id: &str, if_version: u32, files: Value) -> reqwest::Response {
    ts.post_json(
        &format!("/api/artifacts/{id}/versions"),
        json!({"if_version": if_version, "files": files}),
    )
    .await
}

/// Uploads a PNG asset to `id`; the upload's answer.
async fn upload(ts: &TestServer, id: &str) -> Value {
    let part = reqwest::multipart::Part::bytes(vec![137, 80, 78, 71])
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

/// Marks `id` as an artifact made before site hosts (migration 42's mark).
fn make_legacy(ts: &TestServer, id: &str) {
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute("UPDATE artifacts SET legacy_paths = 1 WHERE id = ?1", [id])
        .unwrap();
}

fn id_of(created: &Value) -> String {
    created["artifact"]["id"].as_str().unwrap().to_string()
}

async fn assert_site_404(ts: &TestServer, id: &str, path: &str) {
    let res = ts.site_get(id, path).await;
    assert_eq!(res.status(), 404, "{path}");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer", "{path}");
}

#[tokio::test]
async fn the_root_serves_the_current_version() {
    let ts = TestServer::spawn().await;
    let id = id_of(
        &ts.publish("S", &[("index.html", "<p>one"), ("a.css", "p{}")])
            .await,
    );
    let res = republish(
        &ts,
        &id,
        1,
        json!({"index.html": {"content": "<p>two", "encoding": "utf8"}, "a.css": {"content": "b{}", "encoding": "utf8"}}),
    )
    .await;
    assert_eq!(res.status(), 201);
    let res = ts.site_get(&id, "/").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    let etag = res.headers()["etag"].to_str().unwrap().to_string();
    let html = res.text().await.unwrap();
    assert!(html.contains("<p>two"), "{html}");
    assert!(html.contains("data-version=\"2\""), "{html}");
    // A supporting file of the current version is revalidated too: its
    // bytes change with the next version.
    let res = ts.site_get(&id, "/a.css").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert!(res.headers().contains_key("etag"));
    assert_eq!(res.text().await.unwrap(), "b{}");
    let res = ts
        .client
        .get(format!("{}/", ts.base))
        .header("host", ts.site_host(&id))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
    assert_site_404(&ts, &id, "/missing.html").await;
}

#[tokio::test]
async fn an_older_version_is_served_under_clax_v() {
    let ts = TestServer::spawn().await;
    let id = id_of(
        &ts.publish("S", &[("index.html", "<p>one"), ("a.css", "p{}")])
            .await,
    );
    let res = republish(
        &ts,
        &id,
        1,
        json!({"index.html": {"content": "<p>two", "encoding": "utf8"}}),
    )
    .await;
    assert_eq!(res.status(), 201);
    let res = ts.site_get(&id, "/_clax/v/1/").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert!(res.text().await.unwrap().contains("<p>one"));
    let res = ts.site_get(&id, "/_clax/v/1/a.css").await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(res.text().await.unwrap(), "p{}");
    assert!(
        ts.site_get(&id, "/_clax/v/2/")
            .await
            .text()
            .await
            .unwrap()
            .contains("<p>two")
    );
    assert_site_404(&ts, &id, "/_clax/v/3/").await;
}

#[tokio::test]
async fn a_published_path_under_clax_is_refused() {
    let ts = TestServer::spawn().await;
    for path in ["_clax/x.js", "_clax"] {
        let res = ts
            .post_json(
                "/api/artifacts",
                json!({"title": "S", "files": {
                    "index.html": {"content": "<p>", "encoding": "utf8"},
                    path: {"content": "x", "encoding": "utf8"},
                }}),
            )
            .await;
        assert_eq!(res.status(), 400, "{path}");
        let e: Value = res.json().await.unwrap();
        assert_eq!(e["error"]["code"], "invalid_path", "{path}");
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .contains("'_clax/…' is reserved"),
            "{e}"
        );
    }
    let id = id_of(&ts.publish("S", &[("index.html", "<p>")]).await);
    let res = republish(
        &ts,
        &id,
        1,
        json!({"index.html": {"content": "<p>", "encoding": "utf8"}, "_clax/x.js": {"content": "x", "encoding": "utf8"}}),
    )
    .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn a_new_artifact_owns_blob_paths() {
    let ts = TestServer::spawn().await;
    let id = id_of(
        &ts.publish("S", &[("index.html", "<p>"), ("_blob/a.txt", "mine")])
            .await,
    );
    let res = ts.site_get(&id, "/_blob/a.txt").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "mine");
    // Its assets are under /_clax/blob/, which its upload answers.
    let up = upload(&ts, &id).await;
    let url = up["url"].as_str().unwrap();
    assert_eq!(
        url,
        format!("/_clax/blob/{}", up["asset"]["id"].as_str().unwrap())
    );
    let res = ts.site_get(&id, url).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().to_vec(), vec![137, 80, 78, 71]);
}

#[tokio::test]
async fn a_legacy_artifact_keeps_blob_reserved() {
    let ts = TestServer::spawn().await;
    let id = id_of(&ts.publish("S", &[("index.html", "<p>")]).await);
    make_legacy(&ts, &id);
    let up = upload(&ts, &id).await;
    let asset = up["asset"]["id"].as_str().unwrap();
    assert_eq!(up["url"], format!("/_blob/{asset}"));
    let res = ts.site_get(&id, &format!("/_blob/{asset}")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    let res = ts.site_get(&id, &format!("/_clax/blob/{asset}")).await;
    assert_eq!(res.status(), 200);
    let res = republish(
        &ts,
        &id,
        1,
        json!({"index.html": {"content": "<p>", "encoding": "utf8"}, "_blob/a.txt": {"content": "x", "encoding": "utf8"}}),
    )
    .await;
    assert_eq!(res.status(), 400);
    let e: Value = res.json().await.unwrap();
    assert_eq!(e["error"]["code"], "invalid_path");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("'_blob/…' is reserved"),
        "{e}"
    );
}

#[tokio::test]
async fn an_asset_is_served_only_on_its_own_artifacts_host() {
    let ts = TestServer::spawn().await;
    let a = id_of(&ts.publish("A", &[("index.html", "<p>a")]).await);
    let b = id_of(&ts.publish("B", &[("index.html", "<p>b")]).await);
    let up = upload(&ts, &b).await;
    let blob = format!("/_clax/blob/{}", up["asset"]["id"].as_str().unwrap());
    assert_site_404(&ts, &a, &blob).await;
    assert_eq!(ts.site_get(&b, &blob).await.status(), 200);
    // A legacy host's `/_blob/` is its own assets' too.
    make_legacy(&ts, &a);
    let legacy = format!("/_blob/{}", up["asset"]["id"].as_str().unwrap());
    assert_site_404(&ts, &a, &legacy).await;
}

#[tokio::test]
async fn an_unknown_clax_path_is_404() {
    let ts = TestServer::spawn().await;
    let id = id_of(&ts.publish("S", &[("index.html", "<p>")]).await);
    for path in [
        "/_clax/nope",
        "/_clax/v/x/",
        "/_clax/api/docs",
        "/_clax/blob/",
        "/_clax/draft/a/b/",
    ] {
        assert_site_404(&ts, &id, path).await;
    }
    // The bridge and the UI's files are served there, as on the public host.
    let res = ts.site_get(&id, "/_clax/bridge.js").await;
    assert_eq!(res.status(), 200);
    // An unknown artifact's host serves nothing.
    assert_site_404(&ts, "abcdefghjkmn", "/").await;
}

#[tokio::test]
async fn the_public_host_serves_no_content() {
    let ts = TestServer::spawn().await;
    let id = id_of(&ts.publish("S", &[("index.html", "<p>")]).await);
    for path in [
        format!("/c/{id}/v/1/"),
        format!("/c/{id}/v/1/index.html"),
        format!("/c/{id}/v/1"),
    ] {
        assert_eq!(ts.get(&path).await.status(), 404, "{path}");
    }
    // A shell with no site origins (on a host other than `localhost` or
    // `127.0.0.1`, as a LAN viewer's) still frames it on this host.
    let res = ts
        .client
        .get(format!("{}/c/{id}/v/1/", ts.base))
        .header("host", format!("[::1]:{}", ts.addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // Nor do the old site paths reach a version: `/v/1/` is the site's own.
    assert_site_404(&ts, &id, "/v/1/").await;
}

#[tokio::test]
async fn clax_healthz_answers_on_any_site_host() {
    let ts = TestServer::spawn().await;
    let want: Value = ts.get("/healthz").await.json().await.unwrap();
    // A well-formed ID no artifact has: the same answer, nothing of it.
    let res = ts.site_get("abcdefghjkmn", "/_clax/healthz").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    let got: Value = res.json().await.unwrap();
    assert!(got["version"].is_string(), "{got}");
    assert_eq!(got["version"], want["version"]);
    // The shell probes it from its own origin.
    let res = ts
        .client
        .get(format!("{}/_clax/healthz", ts.base))
        .header("host", ts.site_host("abcdefghjkmn"))
        .header("origin", ts.base.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    let id = id_of(&ts.publish("S", &[("index.html", "<p>")]).await);
    let known: Value = ts
        .site_get(&id, "/_clax/healthz")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        known.as_object().unwrap().keys().collect::<Vec<_>>(),
        got.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    // A malformed host is no site host: `/_clax/healthz` is the public
    // host's UI path there, as before.
    let res = ts
        .client
        .get(format!("{}/_clax/healthz", ts.base))
        .header("host", format!("ABCDEFGHJKMN.localhost:{}", ts.addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert!(res.headers().get("referrer-policy").is_none());
}
