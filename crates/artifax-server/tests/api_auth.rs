mod common;
use common::TestServer;

#[tokio::test]
async fn healthz_reports_version_and_allows_any_origin() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/healthz").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["version"], "test");
    assert!(body["pid"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn token_is_served_to_loopback_peers() {
    let ts = TestServer::spawn().await;
    let body: serde_json::Value = ts.get("/api/token").await.json().await.unwrap();
    assert_eq!(body["token"], "test-token");
}

#[tokio::test]
async fn token_response_is_uncached_and_not_cors_enabled() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/api/token").await;
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert!(res.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn token_is_refused_for_non_local_host_header() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_loopback");
}

#[tokio::test]
async fn write_routes_require_bearer_token() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .post(format!("{}/api/artifacts", ts.base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    let res = ts
        .client
        .post(format!("{}/api/artifacts", ts.base))
        .bearer_auth("wrong")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}
