use clax_testkit::TestServer;
use serde_json::{Value, json};

async fn publish(ts: &TestServer, aid: &str, if_version: u32, files: Value) -> Value {
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .json(&json!({"if_version": if_version, "files": files}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    res.json().await.unwrap()
}

async fn restore(ts: &TestServer, aid: &str, body: Value) -> reqwest::Response {
    ts.authed(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/versions:restore", ts.base)),
    )
    .json(&body)
    .send()
    .await
    .unwrap()
}

async fn file(ts: &TestServer, aid: &str, n: u32, path: &str) -> reqwest::Response {
    ts.get(&format!("/api/artifacts/{aid}/versions/{n}/files/{path}"))
        .await
}

async fn runs(ts: &TestServer) -> String {
    let p = ts.publish("Runs", &[("index.html", "<p>one</p>")]).await;
    p["artifact"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn restore_republishes_the_old_files_as_a_new_version() {
    let ts = TestServer::spawn().await;
    let p = ts
        .publish(
            "Runs",
            &[("index.html", "<p>one</p>"), ("style.css", "p{}")],
        )
        .await;
    let aid = p["artifact"]["id"].as_str().unwrap().to_string();
    publish(
        &ts,
        &aid,
        1,
        json!({"index.html": {"content": "<p>two</p>"}, "app.js": {"content": "x()"}, "style.css": null}),
    )
    .await;
    let res = restore(&ts, &aid, json!({"version": 1, "if_version": 2})).await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["n"], 3);
    assert_eq!(v["version"]["label"], "Restored v1");
    assert_eq!(v["version"]["note"], "Restored version 1");
    assert_eq!(v["artifact"]["current_version"], 3);
    assert_eq!(v["url"], format!("/a/{aid}"));
    assert_eq!(v["note_truncated"], false);
    assert_eq!(
        file(&ts, &aid, 3, "index.html").await.text().await.unwrap(),
        "<p>one</p>"
    );
    assert_eq!(
        file(&ts, &aid, 3, "style.css").await.text().await.unwrap(),
        "p{}",
        "a file version 2 removed comes back"
    );
    assert_eq!(
        file(&ts, &aid, 3, "app.js").await.status(),
        404,
        "files not in v1 are gone"
    );
    let files: Vec<&String> = v["version"]["files"].as_object().unwrap().keys().collect();
    assert_eq!(files, ["index.html", "style.css"]);
    let v1: Value = ts
        .get(&format!("/api/artifacts/{aid}/versions/1"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["version"]["files"], v1["version"]["files"],
        "same bytes, same hashes"
    );
    assert_eq!(
        v["version"]["content_sha256"],
        v1["version"]["content_sha256"]
    );
}

#[tokio::test]
async fn restore_with_a_stale_if_version_is_a_conflict_and_writes_nothing() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    publish(
        &ts,
        &aid,
        1,
        json!({"index.html": {"content": "<p>two</p>"}}),
    )
    .await;
    let res = restore(&ts, &aid, json!({"version": 1, "if_version": 1})).await;
    assert_eq!(res.status(), 409);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "conflict");
    assert_eq!(v["error"]["current"], 2);
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["versions"].as_array().unwrap().len(), 2);
    assert_eq!(a["artifact"]["current_version"], 2);
}

#[tokio::test]
async fn restore_of_an_unknown_version_or_artifact_is_not_found() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    let res = restore(&ts, &aid, json!({"version": 9, "if_version": 1})).await;
    assert_eq!(res.status(), 404);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "not_found"
    );
    let res = restore(&ts, &aid, json!({"version": 0, "if_version": 1})).await;
    assert_eq!(res.status(), 404);
    let other = clax_core::ArtifactId::generate();
    let res = restore(&ts, other.as_str(), json!({"version": 1, "if_version": 1})).await;
    assert_eq!(res.status(), 404);
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["versions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn restore_needs_both_fields() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    for body in [json!({"if_version": 1}), json!({"version": 1}), json!({})] {
        let res = restore(&ts, &aid, body.clone()).await;
        assert_eq!(res.status(), 400, "{body}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "invalid_args",
            "{body}"
        );
    }
    let res = restore(&ts, &aid, json!({"version": "1", "if_version": 1})).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn restore_needs_the_token() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/versions:restore", ts.base))
        .json(&json!({"version": 1, "if_version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn restore_keeps_documents_and_capabilities() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Runs", "capabilities": {"db": {}}, "files": {"index.html": {"content": "<p>1</p>"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let put = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/docs/runs/r1", ts.base)),
        )
        .json(&json!({"data": {"km": 5}}))
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success(), "{}", put.text().await.unwrap());
    publish(&ts, &aid, 1, json!({"index.html": {"content": "<p>2</p>"}})).await;
    assert_eq!(
        restore(&ts, &aid, json!({"version": 1, "if_version": 2}))
            .await
            .status(),
        201
    );
    let doc: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/docs/runs/r1"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(doc["doc"]["data"]["km"], 5);
    assert_eq!(doc["doc"]["version"], 1, "the document is not rewritten");
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["capabilities"], json!({"db": {}}));
    assert_eq!(a["artifact"]["title"], "Runs");
}

#[tokio::test]
async fn restore_emits_a_version_event() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=version")).await;
    assert_eq!(
        restore(&ts, &aid, json!({"version": 1, "if_version": 1}))
            .await
            .status(),
        201
    );
    let e = ev.next_named("version").await;
    assert_eq!(e["artifact_id"], aid);
    assert_eq!(e["n"], 2);
}

#[tokio::test]
async fn restore_is_attributed_to_a_live_session_and_refuses_an_unknown_one() {
    let ts = TestServer::spawn().await;
    let aid = runs(&ts).await;
    let sess = ts.register_session("app", "install-1").await;
    let sid = sess["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions:restore", ts.base)),
        )
        .header("x-clax-session", "nope")
        .json(&json!({"version": 1, "if_version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions:restore", ts.base)),
        )
        .header("x-clax-session", sid)
        .json(&json!({"version": 1, "if_version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["n"], 2);
    assert_eq!(v["version"]["session_id"], sid);
}

#[tokio::test]
async fn a_live_page_cannot_be_restored() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let anchor =
        json!({"kind": "element", "selector": "main", "quote": "Save", "file": "index.html"});
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Settings")
        .text("anchor", anchor.to_string())
        .text("body", "The button overflows")
        .text("pending", "[]")
        .text("snapshot", "<p>");
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());
    let body: Value = res.json().await.unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let res = restore(&ts, aid, json!({"version": 1, "if_version": 1})).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "live_page"
    );
    let a: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["current_version"], 1);
}

#[tokio::test]
async fn a_publish_after_a_restore_builds_on_the_restored_files() {
    let ts = TestServer::spawn().await;
    let p = ts
        .publish(
            "Runs",
            &[("index.html", "<p>one</p>"), ("style.css", "p{}")],
        )
        .await;
    let aid = p["artifact"]["id"].as_str().unwrap().to_string();
    publish(
        &ts,
        &aid,
        1,
        json!({"index.html": {"content": "<p>two</p>"}, "style.css": null, "app.js": {"content": "x()"}}),
    )
    .await;
    assert_eq!(
        restore(&ts, &aid, json!({"version": 1, "if_version": 2}))
            .await
            .status(),
        201
    );
    let v = publish(
        &ts,
        &aid,
        3,
        json!({"index.html": {"content": "<p>four</p>"}}),
    )
    .await;
    assert_eq!(v["version"]["n"], 4);
    let files: Vec<&String> = v["version"]["files"].as_object().unwrap().keys().collect();
    assert_eq!(files, ["index.html", "style.css"]);
    assert_eq!(
        file(&ts, &aid, 4, "index.html").await.text().await.unwrap(),
        "<p>four</p>"
    );
    assert_eq!(
        file(&ts, &aid, 4, "style.css").await.text().await.unwrap(),
        "p{}"
    );
    assert_eq!(file(&ts, &aid, 4, "app.js").await.status(), 404);
}
