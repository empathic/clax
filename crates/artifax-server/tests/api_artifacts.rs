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

#[tokio::test]
async fn deleted_artifact_hides_versions_files_and_patch() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    for p in ["versions", "versions/1", "files"] {
        assert_eq!(
            ts.get(&format!("/api/artifacts/{id}/{p}")).await.status(),
            404,
            "{p}"
        );
    }
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/artifacts/{id}", ts.base))
                .json(&json!({"title": "x"})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn oversized_publish_is_413_body_too_large() {
    let ts = TestServer::spawn().await;
    let mut body = br#"{"files":{"index.html":{"content":""#.to_vec();
    body.resize(body.len() + 97 * 1024 * 1024, b'x');
    body.extend_from_slice(br#""}}}"#);
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts", ts.base))
                .header("content-type", "application/json")
                .body(reqwest::Body::from(body)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "body_too_large"
    );
}

#[tokio::test]
async fn bad_path_param_is_json_400() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/api/artifacts/7q3k9mzx2b4t/versions/abc").await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "invalid_path_param"
    );
}

#[tokio::test]
async fn patch_rejects_unknown_fields() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/artifacts/{id}", ts.base))
                .json(&json!({"capabilities": {}})),
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
async fn corrupt_row_is_left_out_of_the_list_and_named_on_lookup() {
    let ts = TestServer::spawn().await;
    let good = ts.publish("Good", &[("index.html", "g")]).await;
    let bad = ts.publish("Bad", &[("index.html", "b")]).await;
    let good_id = good["artifact"]["id"].as_str().unwrap().to_string();
    let bad_id = bad["artifact"]["id"].as_str().unwrap().to_string();
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute(
            "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
            [&bad_id],
        )
        .unwrap();

    let res = ts.get("/api/artifacts").await;
    assert_eq!(res.status(), 200);
    let list: serde_json::Value = res.json().await.unwrap();
    let ids: Vec<&str> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [good_id.as_str()]);

    let res = ts.get(&format!("/api/artifacts/{bad_id}")).await;
    assert_eq!(res.status(), 500);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "corrupt");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains(&bad_id), "{message}");
}

#[tokio::test]
async fn creating_an_artifact_requires_a_title_but_a_new_version_does_not() {
    let ts = TestServer::spawn().await;
    let page = json!({"index.html": {"content": "<title>Page</title>"}});
    for body in [
        json!({"files": page}),
        json!({"title": null, "files": page}),
        json!({"title": "  \n", "files": page}),
    ] {
        let res = ts.post_json("/api/artifacts", body.clone()).await;
        assert_eq!(res.status(), 400, "{body}");
        let err: serde_json::Value = res.json().await.unwrap();
        assert_eq!(err["error"]["code"], "invalid_args", "{body}");
        assert_eq!(
            err["error"]["message"],
            "title is required when creating an artifact"
        );
    }
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert!(list["artifacts"].as_array().unwrap().is_empty());

    let created = ts.publish("Titled", &[("index.html", "<p>1")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts
        .post_json(
            &format!("/api/artifacts/{id}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<p>2"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["artifact"]["title"], "Titled");
}

#[tokio::test]
async fn a_page_publish_is_announced_by_page() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish("Poll", &[("index.html", "<!doctype html><body>0</body>")])
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut events = ts.events(&format!("?artifact={aid}")).await;
    let body = json!({"if_version": 1, "files": {"index.html": {"content": "<!doctype html><body>1</body>", "encoding": "utf8"}}});
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-artifax-via", "page")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(
        events.next_named("version").await,
        json!({"type": "version", "artifact_id": aid, "n": 2, "by_page": true})
    );
    let body = json!({"if_version": 2, "files": {"index.html": {"content": "<p>agent</p>", "encoding": "utf8"}}});
    assert_eq!(
        ts.post_json(&format!("/api/artifacts/{aid}/versions"), body)
            .await
            .status(),
        201
    );
    assert_eq!(
        events.next_named("version").await,
        json!({"type": "version", "artifact_id": aid, "n": 3}),
        "by_page is omitted when false"
    );
}
