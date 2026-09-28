mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn publish_get_list_roundtrip() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish("Report", &[("index.html", "<p>v1</p>"), ("app.js", "1")])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 12);
    assert_eq!(created["version"]["n"], 1);
    assert_eq!(created["url"], format!("/a/{id}"));
    let got: serde_json::Value = ts
        .get(&format!("/api/artifacts/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["artifact"]["title"], "Report");
    assert_eq!(got["versions"].as_array().unwrap().len(), 1);
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
    let files: serde_json::Value = ts
        .get(&format!("/api/artifacts/{id}/files"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(files["files"]["app.js"]["content_type"], "text/javascript");
}

#[tokio::test]
async fn republish_requires_matching_if_version() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let body = |v: serde_json::Value| json!({"if_version": v, "files": {"index.html": {"content": "<p>v2</p>"}}});
    let res = ts
        .post_json(&format!("/api/artifacts/{id}/versions"), body(json!(5)))
        .await;
    assert_eq!(res.status(), 409);
    let err: serde_json::Value = res.json().await.unwrap();
    assert_eq!(err["error"]["code"], "conflict");
    assert_eq!(err["error"]["current"], 1);
    let res = ts
        .post_json(
            &format!("/api/artifacts/{id}/versions"),
            json!({"files": {"index.html": {"content": "x"}}}),
        )
        .await;
    assert_eq!(res.status(), 400);
    let res = ts
        .post_json(&format!("/api/artifacts/{id}/versions"), body(json!(1)))
        .await;
    assert_eq!(res.status(), 201);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["artifact"]["current_version"], 2);
    let res = ts.get(&format!("/api/artifacts/{id}/versions/2")).await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn validation_errors_are_400_with_codes() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"files": {"app.js": {"content": "1"}}}),
        )
        .await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "missing_index"
    );
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"files": {"index.html": {"content": "1"}, "../x": {"content": "1"}}}),
        )
        .await;
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "invalid_path"
    );
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts", ts.base))
                .header("content-type", "application/json")
                .body("{nope"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "invalid_json"
    );
}

#[tokio::test]
async fn patch_and_delete() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/artifacts/{id}", ts.base))
                .json(&json!({"pinned": true, "title": "New"})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let a: serde_json::Value = res.json().await.unwrap();
    assert_eq!(a["artifact"]["pinned"], true);
    assert_eq!(a["artifact"]["title"], "New");
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.get(&format!("/api/artifacts/{id}")).await.status(), 404);
    assert_eq!(ts.get("/api/artifacts/not-an-id").await.status(), 400);
}
