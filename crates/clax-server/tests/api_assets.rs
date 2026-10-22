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
        "public, max-age=31536000, immutable"
    );
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
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
    let res = ts.site_get(&aid, &url).await;
    assert_eq!(res.status(), 404);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "not_found"
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
        let asset = ts.store.get_asset(&aid).unwrap().unwrap();
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

/// An asset is served under its own artifact only: another artifact's
/// prefix, or another artifact's origin, answers 404 as a missing asset does.
#[tokio::test]
async fn an_asset_is_served_only_under_its_own_artifact() {
    let ts = TestServer::spawn().await;
    let upload = |id: String| {
        let ts = &ts;
        async move {
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
            let body: serde_json::Value = res.json().await.unwrap();
            body["asset"]["id"].as_str().unwrap().to_string()
        }
    };
    let a = ts.publish("A", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.publish("B", &[("index.html", "<p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let asset = upload(a.clone()).await;
    assert_eq!(ts.get(&format!("/_blob/{a}/{asset}")).await.status(), 200);
    assert_eq!(ts.get(&format!("/_blob/{b}/{asset}")).await.status(), 404);
    let on = |host: String, path: String| {
        let ts = &ts;
        async move {
            ts.client
                .get(format!("{}{path}", ts.base))
                .header("host", host)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    let port = ts.addr.port();
    // On A's origin: by asset ID, or under A's prefix.
    assert_eq!(
        on(format!("{a}.localhost:{port}"), format!("/_blob/{asset}")).await,
        200
    );
    assert_eq!(
        on(
            format!("{a}.localhost:{port}"),
            format!("/_blob/{a}/{asset}")
        )
        .await,
        200
    );
    // On B's origin, A's asset is not there, whatever the path names.
    assert_eq!(
        on(format!("{b}.localhost:{port}"), format!("/_blob/{asset}")).await,
        404
    );
    assert_eq!(
        on(
            format!("{b}.localhost:{port}"),
            format!("/_blob/{a}/{asset}")
        )
        .await,
        404
    );
}
