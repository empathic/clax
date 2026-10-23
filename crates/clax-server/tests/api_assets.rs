use clax_testkit::TestServer;

#[tokio::test]
async fn upload_serve_list_delete() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let part = reqwest::multipart::Part::bytes(vec![137, 80, 78, 71])
        .file_name("logo.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/assets", ts.base))
                .multipart(form),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    let url = body["url"].as_str().unwrap().to_string();
    // An address on the artifact's site host; the public host's
    // `/_blob/<id>` only redirects there, for pages that name that.
    assert!(url.starts_with("/_clax/blob/"), "{url}");
    let asset_id = body["asset"]["id"].as_str().unwrap();
    let res = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}/_blob/{asset_id}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 301);
    assert_eq!(
        res.headers()["location"],
        format!(
            "http://{id}.localhost:{}/_clax/blob/{asset_id}",
            ts.addr.port()
        )
    );
    let res = ts.site_get(id, &url).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(
        res.headers()["cache-control"],
        "private, max-age=31536000, immutable"
    );
    assert!(
        res.headers().get("access-control-allow-origin").is_none(),
        "a site answer is its own origin's"
    );
    assert_eq!(res.headers()["content-security-policy"], "sandbox");
    assert_eq!(res.bytes().await.unwrap().to_vec(), vec![137, 80, 78, 71]);
    let list: serde_json::Value = ts
        .get(&format!("/api/artifacts/{id}/assets"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["assets"].as_array().unwrap().len(), 1);
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.site_get(id, &url).await.status(), 404);
}

#[tokio::test]
async fn unsupported_type_and_missing_field_are_400() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let part = reqwest::multipart::Part::bytes(vec![0])
        .file_name("x.exe")
        .mime_str("application/x-msdownload")
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
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "unsupported_type"
    );
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/assets", ts.base))
                .multipart(reqwest::multipart::Form::new().text("other", "x")),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "missing_file"
    );
}

#[tokio::test]
async fn blob_404s_after_artifact_delete_and_cross_artifact_delete_is_404() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let b = ts.publish("B", &[("index.html", "<p>")]).await;
    let bid = b["artifact"]["id"].as_str().unwrap().to_string();
    let part = reqwest::multipart::Part::bytes(vec![1])
        .file_name("x.png")
        .mime_str("image/png; charset=binary")
        .unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/assets", ts.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        )
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["asset"]["ext"], "png");
    let url = body["url"].as_str().unwrap().to_string();
    let asset_id = body["asset"]["id"].as_str().unwrap().to_string();
    let res = ts.site_get(&aid, &url).await;
    assert_eq!(res.headers()["content-length"], "1");
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{bid}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(
        ts.site_get(&aid, &url).await.status(),
        200,
        "cross-artifact delete did nothing"
    );
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    // No one reads a deleted artifact's site.
    let res = ts.site_get(&aid, &url).await;
    assert_eq!(res.status(), 401);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "site_session_required"
    );
}

#[tokio::test]
async fn non_multipart_upload_is_json_400() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/assets", ts.base))
                .header("content-type", "application/json")
                .body("{}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "invalid_multipart"
    );
}

/// A script of Mia's, carried forward under the owner's index.html, uploads
/// from the owner's browser: the upload carries Mia (spec
/// 2026-10-08-multi-agent-multi-person §8.2, M11).
#[tokio::test]
async fn a_pages_upload_carries_every_author_of_the_page() {
    use clax_core::visibility::{Author, authors_of_asset};
    let ts = TestServer::spawn().await;
    let (invite, _) = ts
        .store
        .create_invite(
            &clax_core::audit::AuditCtx::DAEMON,
            clax_store::people::InvitePerson::New("Mia".into()),
            None,
        )
        .unwrap();
    let mia = invite.person;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<script src=app.js></script>"),
                ("app.js", "MIA"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    // Version 1 is Mia's, as her own publish writes it.
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE versions SET person = ?2, file_authors_json = ?3 WHERE artifact_id = ?1 AND n = 1",
        rusqlite::params![
            id,
            mia,
            serde_json::json!({"index.html": [mia], "app.js": [mia]}).to_string()
        ],
    )
    .unwrap();
    drop(db);
    // The owner's agent rewrites index.html; app.js is carried.
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/versions", ts.base))
                .json(&serde_json::json!({"if_version": 1, "files": {
                    "index.html": {"content": "<h1>x</h1><script src=app.js></script>", "encoding": "utf8"}}})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    let upload = |page: bool| {
        let part = reqwest::multipart::Part::bytes(b"MIA".to_vec())
            .file_name("notes.txt")
            .mime_str("text/plain")
            .unwrap();
        let mut req = ts.authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/assets", ts.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        );
        if page {
            req = req.header("x-clax-via", "page");
        }
        req.send()
    };
    let authors = |res: serde_json::Value| {
        let aid = res["asset"]["id"].as_str().unwrap().to_string();
        let asset = ts.store.find_asset(&aid).unwrap().unwrap();
        authors_of_asset(&asset)
    };
    let res = upload(true).await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(
        authors(res.json().await.unwrap()),
        vec![Author::Owner, Author::Person(mia.clone())],
        "the page's upload carries Mia's script"
    );
    let res = upload(false).await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(
        authors(res.json().await.unwrap()),
        vec![Author::Owner],
        "the owner's agent's upload is the owner's"
    );
}

/// A fork's page keeps loading the assets it names from the fork's own
/// site, under the same addresses: the fork copies them, so neither the
/// source's deletion of one nor of itself breaks the fork's.
#[tokio::test]
async fn a_forks_page_loads_its_assets_from_its_own_site() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    let bytes = vec![137, 80, 78, 71, 1, 2, 3];
    let body = ts
        .upload_asset(&id, "logo.png", "image/png", bytes.clone())
        .await;
    let url = body["url"].as_str().unwrap().to_string();
    let asset_id = body["asset"]["id"].as_str().unwrap().to_string();
    let html = format!("<img src=\"{url}\">");
    let res = ts
        .post_json(
            &format!("/api/artifacts/{id}/versions"),
            serde_json::json!({"if_version": 1,
                   "files": {"index.html": {"content": html, "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let res = ts
        .post_json(&format!("/api/artifacts/{id}/fork"), serde_json::json!({}))
        .await;
    assert_eq!(res.status(), 201);
    let fid = res.json::<serde_json::Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let page = ts.site_get(&fid, "/").await.text().await.unwrap();
    assert!(
        page.contains(&html),
        "the fork's page is unrewritten: {page}"
    );
    let fetch = |art: String| {
        let (ts, url) = (&ts, url.clone());
        async move {
            let res = ts.site_get(&art, &url).await;
            (res.status().as_u16(), res.bytes().await.unwrap().to_vec())
        }
    };
    assert_eq!(fetch(fid.clone()).await, (200, bytes.clone()));
    let list: serde_json::Value = ts
        .get(&format!("/api/artifacts/{fid}/assets"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["assets"][0]["id"], asset_id.as_str());
    // The source's deletion of the asset, then of itself, leaves the fork's.
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(fetch(id.clone()).await.0, 404);
    assert_eq!(fetch(fid.clone()).await, (200, bytes.clone()));
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(fetch(fid.clone()).await, (200, bytes));
}

/// `/_blob/<artifact ID>/<asset ID>`, the address pages were given for their
/// own assets, redirects to the asset on that artifact's site, and answers
/// 404 for another artifact's asset; a site serves its own artifact's
/// assets only.
#[tokio::test]
async fn an_asset_is_served_only_under_its_own_artifact() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.publish("B", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let body = ts
        .upload_asset(&a, "logo.png", "image/png", vec![137, 80, 78, 71])
        .await;
    let asset = body["asset"]["id"].as_str().unwrap().to_string();
    let no_follow = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = no_follow
        .get(format!("{}/_blob/{a}/{asset}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 301);
    assert_eq!(
        res.headers()["location"],
        format!("http://{a}.localhost:{}/_clax/blob/{asset}", ts.addr.port())
    );
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    let res = no_follow
        .get(format!("{}/_blob/{b}/{asset}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let blob = format!("/_clax/blob/{asset}");
    assert_eq!(ts.site_get(&a, &blob).await.status(), 200);
    // On B's site, A's asset is not there.
    assert_eq!(ts.site_get(&b, &blob).await.status(), 404);
}

/// On an artifact's site, `/_blob/<its ID>/<asset ID>`, the address a local
/// daemon gave its pages (and their documents) for their own assets before
/// each artifact had a site, serves the asset there, legacy artifact or
/// not; another artifact's ID there names no asset.
#[tokio::test]
async fn a_site_serves_its_own_assets_at_their_earlier_scoped_address() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.publish("B", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let bytes = vec![137, 80, 78, 71, 9];
    let asset = ts
        .upload_asset(&a, "logo.png", "image/png", bytes.clone())
        .await["asset"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b_asset = ts
        .upload_asset(&b, "b.png", "image/png", vec![137, 80, 78, 71])
        .await["asset"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let check = |legacy: bool| {
        let (ts, a, b, asset, b_asset, bytes) = (&ts, &a, &b, &asset, &b_asset, &bytes);
        async move {
            let res = ts.site_get(a, &format!("/_blob/{a}/{asset}")).await;
            assert_eq!(res.status(), 200, "legacy {legacy}");
            assert_eq!(res.headers()["content-type"], "image/png");
            assert_eq!(res.headers()["content-security-policy"], "sandbox");
            assert_eq!(&res.bytes().await.unwrap().to_vec(), bytes);
            // Another artifact's ID, with its asset or this one's: nothing.
            for path in [
                format!("/_blob/{b}/{b_asset}"),
                format!("/_blob/{b}/{asset}"),
            ] {
                let res = ts.site_get(a, &path).await;
                assert_eq!(res.status(), 404, "legacy {legacy}: {path}");
            }
            // This artifact's ID with another's asset: nothing.
            let res = ts.site_get(a, &format!("/_blob/{a}/{b_asset}")).await;
            assert_eq!(res.status(), 404, "legacy {legacy}");
        }
    };
    check(false).await;
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute("UPDATE artifacts SET legacy_paths = 1 WHERE id = ?1", [&a])
        .unwrap();
    check(true).await;
    // A legacy artifact's pages also name its assets as `/_blob/<asset ID>`.
    let res = ts.site_get(&a, &format!("/_blob/{asset}")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().to_vec(), bytes);
}
