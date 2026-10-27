//! With the home's `[serve] tailscale_identity` on, a request that came
//! through `tailscale serve` (a loopback peer naming a `[serve]
//! trusted_hosts` name) is the viewer its `Tailscale-User-Login` names, with
//! Tailscale's name. Anything else ignores the headers, and the token rules
//! are unchanged.
use clax_testkit::TestServer;
use serde_json::Value;
use std::net::{IpAddr, UdpSocket};

/// The `tailscale serve` name these tests trust.
const TRUSTED: &str = "phobos.tail556f5b.ts.net";

/// The headers `tailscale serve` adds for a tailnet user.
const ALEX: [(&str, &str); 2] = [
    ("tailscale-user-login", "alex@github"),
    ("tailscale-user-name", "Alex Kesling"),
];

/// A non-loopback IPv4 address of this machine (see `api_host.rs`).
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// A daemon trusting [`TRUSTED`], with `tailscale_identity` set to `on`.
async fn tailnet(on: bool) -> TestServer {
    TestServer::spawn_with(|s| {
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = on;
    })
    .await
}

/// `req` with `host` and the tailnet user's headers.
fn as_alex(req: reqwest::RequestBuilder, host: &str) -> reqwest::RequestBuilder {
    ALEX.iter()
        .fold(req.header("host", host), |r, (k, v)| r.header(*k, *v))
}

/// `GET /api/viewers/me` on `base` as Alex through `host`: the viewer, and
/// whether a viewer cookie was set.
async fn me(ts: &TestServer, base: &str, host: &str) -> (Value, bool) {
    let res = as_alex(ts.client.get(format!("{base}/api/viewers/me")), host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{host}");
    let cookie = res.headers().get("set-cookie").is_some();
    let body: Value = res.json().await.unwrap();
    (body["viewer"].clone(), cookie)
}

/// A thread on `aid` as Alex through `host`, from `origin`.
async fn comment(ts: &TestServer, aid: &str, host: &str, origin: &str) -> reqwest::Response {
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", "from the phone")
        .text("version", "1");
    as_alex(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads", ts.base)),
        host,
    )
    .header("origin", origin)
    .multipart(form)
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn a_tailnet_user_through_serve_is_the_viewer_its_login_names() {
    let ts = tailnet(true).await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let (v, cookie) = me(&ts, &ts.base, TRUSTED).await;
    assert_eq!(v["display_name"], "Alex Kesling");
    assert_eq!(v["tailscale_login"], "alex@github");
    assert!(v.get("owner").is_none() && v.get("person").is_none(), "{v}");
    assert!(!cookie, "no viewer cookie is minted for a tailnet user");
    let pid = v["public_id"].as_str().unwrap().to_string();
    // The same login is the same viewer, with or without the port.
    let (again, _) = me(&ts, &ts.base, &format!("{TRUSTED}:443")).await;
    assert_eq!(again["public_id"], pid.as_str());
    // A typed name never replaces Tailscale's.
    let res = as_alex(
        ts.client.put(format!("{}/api/viewers/me", ts.base)),
        TRUSTED,
    )
    .json(&serde_json::json!({"display_name": "Mallory"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 409);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "name_from_tailscale");
    let (after, _) = me(&ts, &ts.base, TRUSTED).await;
    assert_eq!(after["display_name"], "Alex Kesling");
    // A comment is Alex's, by Tailscale's name, with no cookie at all.
    let res = comment(&ts, aid, TRUSTED, &format!("https://{TRUSTED}")).await;
    assert_eq!(res.status(), 201);
    let t = res.json::<Value>().await.unwrap()["thread"].clone();
    assert_eq!(t["comments"][0]["author_public_id"], pid.as_str());
    assert_eq!(t["comments"][0]["author_name"], "Alex Kesling");
    // The People list marks the name as Tailscale's.
    let art: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    let people = art["artifact"]["participants"]["people"]
        .as_array()
        .unwrap();
    assert_eq!(people.len(), 1, "{people:?}");
    assert_eq!(people[0]["public_id"], pid.as_str());
    assert_eq!(people[0]["display_name"], "Alex Kesling");
    assert_eq!(people[0]["tailscale_login"], "alex@github");
    let looked: Value = ts
        .get(&format!("/api/viewers?ids={pid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(looked["viewers"][0]["tailscale_login"], "alex@github");
}

#[tokio::test]
async fn a_tailnet_user_gets_no_token_and_no_owners_power() {
    let ts = tailnet(true).await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let get = |path: String| {
        as_alex(ts.client.get(format!("{}{path}", ts.base)), TRUSTED)
            .header("sec-fetch-site", "same-origin")
            .send()
    };
    let res = get("/api/token".into()).await.unwrap();
    assert_eq!(res.status(), 403);
    assert!(res.headers().get("set-cookie").is_none(), "no owner cookie");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_loopback");
    // The session list needs the token.
    let res = get("/api/sessions".into()).await.unwrap();
    assert_eq!(res.status(), 401);
    // Publishing needs the token.
    let res = as_alex(
        ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        TRUSTED,
    )
    .header("origin", format!("https://{TRUSTED}"))
    .json(&serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "<h2>y</h2>", "encoding": "utf8"}}}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 401);
    let art: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(art["artifact"]["current_version"], 1, "{art}");
}

#[tokio::test]
async fn the_headers_are_ignored_on_a_host_that_is_not_trusted() {
    let ts = tailnet(true).await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let port = ts.addr.port();
    // A local process on this machine can send them to localhost.
    for host in [format!("localhost:{port}"), format!("127.0.0.1:{port}")] {
        let (v, cookie) = me(&ts, &ts.base, &host).await;
        assert!(v.get("tailscale_login").is_none(), "{host}: {v}");
        assert_eq!(v["display_name"], Value::Null, "{host}");
        assert!(cookie, "{host}: a cookie viewer, as without the headers");
        let res = comment(&ts, aid, &host, &format!("http://{host}")).await;
        assert_eq!(res.status(), 201, "{host}");
        let t = res.json::<Value>().await.unwrap()["thread"].clone();
        assert_eq!(t["comments"][0]["author_public_id"], Value::Null, "{host}");
        assert_eq!(t["comments"][0]["author_name"], "Viewer", "{host}");
    }
}

#[tokio::test]
async fn the_headers_are_ignored_with_the_setting_off() {
    let ts = tailnet(false).await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let (v, cookie) = me(&ts, &ts.base, TRUSTED).await;
    assert!(v.get("tailscale_login").is_none(), "{v}");
    assert_eq!(v["display_name"], Value::Null);
    assert!(cookie);
    let res = comment(&ts, aid, TRUSTED, &format!("https://{TRUSTED}")).await;
    assert_eq!(res.status(), 201);
    let t = res.json::<Value>().await.unwrap()["thread"].clone();
    assert_eq!(t["comments"][0]["author_public_id"], Value::Null);
    assert_eq!(t["comments"][0]["author_name"], "Viewer");
}

#[tokio::test]
async fn the_headers_are_ignored_from_a_peer_that_is_not_loopback() {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return;
    };
    // A LAN client that names the trusted host itself is not the proxy.
    let ts = TestServer::spawn_on(ip, |s| {
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = true;
    })
    .await;
    let lan = format!("http://{ip}:{}", ts.addr.port());
    let (v, cookie) = me(&ts, &lan, TRUSTED).await;
    assert!(v.get("tailscale_login").is_none(), "{v}");
    assert_eq!(v["display_name"], Value::Null);
    assert!(cookie, "a cookie viewer, as without the headers");
}
