use crate::common;
use common::TestServer;

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
    assert!(url.starts_with("/_blob/"));
    let res = ts.get(&url).await;
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
    let asset_id = body["asset"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.get(&url).await.status(), 404);
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
    let res = ts.get(&url).await;
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
        ts.get(&url).await.status(),
        200,
        "cross-artifact delete did nothing"
    );
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    let res = ts.get(&url).await;
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
