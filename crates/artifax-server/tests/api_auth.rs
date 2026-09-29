mod common;
use common::TestServer;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// A non-loopback IPv4 address of this machine: the source address the OS would
/// pick to reach a private-range destination. Connecting a UDP socket sends nothing.
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test]
async fn token_is_refused_to_non_loopback_peers() {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return;
    };
    let ts = TestServer::spawn_on(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |_| {}).await;
    // A local Host header isolates the peer-address check from the Host check.
    let res = ts
        .client
        .get(format!("http://{ip}:{}/api/token", ts.addr.port()))
        .header("host", "localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_loopback");
}

#[tokio::test]
async fn bearer_scheme_is_case_insensitive() {
    let ts = TestServer::spawn().await;
    for scheme in ["bearer", "BEARER", "BeArEr"] {
        let res = ts
            .client
            .post(format!("{}/api/artifacts", ts.base))
            .header("authorization", format!("{scheme} {}", ts.token))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        // Authorised, so the empty body reaches validation.
        assert_eq!(res.status(), 400, "{scheme}");
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["error"]["code"], "missing_index", "{scheme}");
    }
}

#[tokio::test]
async fn healthz_reports_version_and_allows_any_origin() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/healthz").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["version"], "test");
    assert!(body["pid"].as_u64().unwrap() > 0);
    let started_at = body["started_at"].as_str().expect("started_at is a string");
    chrono::DateTime::parse_from_rfc3339(started_at).expect("started_at is RFC 3339");
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
