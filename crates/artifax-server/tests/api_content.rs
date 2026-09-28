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
