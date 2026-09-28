mod common;
use common::TestServer;

#[tokio::test]
async fn serves_wrapped_index_and_files_with_caching_headers() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<title>R</title><p>hi</p>"),
                ("css/a.css", "p{color:red}"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts.get(&format!("/c/{id}/v/1/")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-store");
    let html = res.text().await.unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(&format!(
        "data-artifact=\"{id}\" data-version=\"1\" data-contract=\"0.2.61\""
    )));
    let res = ts.get(&format!("/c/{id}/v/1/css/a.css")).await;
    assert_eq!(res.headers()["content-type"], "text/css");
    assert_eq!(
        res.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert_eq!(res.text().await.unwrap(), "p{color:red}");
    assert_eq!(
        ts.get(&format!("/c/{id}/v/1/missing.js")).await.status(),
        404
    );
    assert_eq!(ts.get(&format!("/c/{id}/v/9/")).await.status(), 404);
    let res = ts
        .client
        .get(format!("{}/c/{id}/v/1", ts.base))
        .send()
        .await
        .unwrap();
    assert!(
        res.url().path().ends_with("/v/1/"),
        "redirected to trailing slash"
    );
}

#[tokio::test]
async fn artifact_host_routes_to_content_and_nothing_else() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("<p>hi</p>"));
    let res = ts
        .client
        .get(format!("{}/healthz", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .get(format!("{}/api/artifacts", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        404,
        "API is not reachable on an artifact origin"
    );
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", format!("{id}.localhost.attacker.com"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn main_origin_content_is_sandboxed_but_artifact_origin_is_not() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish("R", &[("index.html", "<p>hi</p>"), ("a.css", "p{}")])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let csp = "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";
    for path in [format!("/c/{id}/v/1/"), format!("/c/{id}/v/1/a.css")] {
        let res = ts.get(&path).await;
        assert_eq!(res.headers()["content-security-policy"], csp, "{path}");
    }
    let host = format!("{id}.localhost");
    for path in ["/v/1/", "/v/1/a.css"] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert!(
            res.headers().get("content-security-policy").is_none(),
            "{path}"
        );
    }
}

#[tokio::test]
async fn redirects_are_relative() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let c = no_redirect_client();
    let res = c
        .get(format!("{}/c/{id}/v/1", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 308);
    assert_eq!(res.headers()["location"], "1/");
    let res = c
        .get(format!("{}/v/1/index.html", ts.base))
        .header("host", format!("{id}.localhost"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 308);
    assert_eq!(res.headers()["location"], "./");
}

#[tokio::test]
async fn artifact_host_passes_blob_and_bridge_paths_to_router() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    for path in ["/_blob/does-not-exist", "/_artifax/x.js"] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{path}");
        // `/_artifax/*` has no route until the bridge is served, so only the blob
        // route is expected to answer with the JSON error body for now.
        if path.starts_with("/_blob/") {
            let v: serde_json::Value = res.json().await.expect(path);
            assert_eq!(v["error"]["code"], "not_found", "{path}");
        }
    }
}
