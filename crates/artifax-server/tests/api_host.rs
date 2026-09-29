//! Every `/api` route answers only to a `Host` that names this machine
//! literally, or to the address the daemon is bound to (DNS rebinding).
mod common;
use common::TestServer;
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// A non-loopback IPv4 address of this machine (see `api_auth.rs`).
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

async fn status_with_host(ts: &TestServer, base: &str, path: &str, host: &str) -> (u16, Value) {
    let res = ts
        .client
        .get(format!("{base}{path}"))
        .header("host", host)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body = res.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn api_routes_refuse_a_host_that_is_not_this_machine() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let port = ts.addr.port();
    for host in [
        format!("evil.example:{port}"),
        "evil.example".to_string(),
        format!("10.9.8.7:{port}"),
        format!("localhost.evil.example:{port}"),
        format!("127.0.0.1.nip.io:{port}"),
    ] {
        for path in [
            "/api/artifacts".to_string(),
            "/api/viewers/me".to_string(),
            "/api/push".to_string(),
            "/api/token".to_string(),
            format!("/api/artifacts/{aid}/threads"),
        ] {
            let (status, body) = status_with_host(&ts, &ts.base, &path, &host).await;
            assert_eq!(status, 403, "{host} {path}");
            assert_eq!(body["error"]["code"], "forbidden_host", "{host} {path}");
        }
    }
    // A rebound page: Host and Origin agree, and both name the attacker's domain.
    let form = reqwest::multipart::Form::new()
        .text(
            "anchor",
            artifax_server::testing::element_anchor().to_string(),
        )
        .text("body", "@agent run this")
        .text("version", "1");
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("host", format!("rebind.example:{port}"))
        .header("origin", format!("http://rebind.example:{port}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let listed: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        listed["threads"].as_array().unwrap().is_empty(),
        "nothing was written"
    );
}

#[tokio::test]
async fn local_hosts_are_admitted_with_or_without_the_port() {
    let ts = TestServer::spawn().await;
    let port = ts.addr.port();
    for host in [
        "localhost".to_string(),
        format!("localhost:{port}"),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{port}"),
        "[::1]".to_string(),
        format!("[::1]:{port}"),
    ] {
        let (status, _) = status_with_host(&ts, &ts.base, "/api/artifacts", &host).await;
        assert_eq!(status, 200, "{host}");
    }
    let (status, body) =
        status_with_host(&ts, &ts.base, "/api/token", &format!("localhost:{port}")).await;
    assert_eq!((status, body["token"].as_str()), (200, Some("test-token")));
}

#[tokio::test]
async fn content_and_the_shell_are_not_api_routes() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let port = ts.addr.port();
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", format!("{aid}.localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "artifact content host");
    let res = ts
        .client
        .get(format!("{}/a/{aid}", ts.base))
        .header("host", format!("localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "shell");
}

#[tokio::test]
async fn a_daemon_bound_to_a_lan_address_admits_exactly_that_host() {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return;
    };
    for bind in [ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED)] {
        let ts = TestServer::spawn_on(bind, |_| {}).await;
        let port = ts.addr.port();
        let lan = format!("http://{ip}:{port}");
        let (status, _) =
            status_with_host(&ts, &lan, "/api/artifacts", &format!("{ip}:{port}")).await;
        assert_eq!(status, 200, "bind {bind}: the bound LAN host");
        let (status, _) = status_with_host(
            &ts,
            &lan,
            "/api/artifacts",
            &format!("{ip}:{}", port.wrapping_add(1)),
        )
        .await;
        assert_eq!(status, 403, "bind {bind}: another port");
        let (status, _) =
            status_with_host(&ts, &lan, "/api/artifacts", &format!("10.9.8.7:{port}")).await;
        assert_eq!(status, 403, "bind {bind}: an IP that is not the bind");
        let (status, _) =
            status_with_host(&ts, &lan, "/api/artifacts", &format!("mymac.local:{port}")).await;
        assert_eq!(status, 403, "bind {bind}: a DNS name");
        let (status, body) =
            status_with_host(&ts, &lan, "/api/token", &format!("{ip}:{port}")).await;
        assert_eq!(
            (status, body["error"]["code"].as_str()),
            (403, Some("not_loopback")),
            "bind {bind}: the token stays loopback-only"
        );
    }
}
