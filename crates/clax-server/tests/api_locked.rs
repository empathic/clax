//! A daemon serving with `require_credential` (`clax serve --foreground
//! --require-credential`, the app's embedded daemon): nothing but `/healthz`,
//! the shell's static files and content addressed by artifact ID answers a
//! request that carries no credential, `GET /api/token` included.

use crate::common;
use common::TestServer;

async fn locked() -> TestServer {
    TestServer::spawn_with(|s| s.require_credential = true).await
}

/// `clax_key_<port>=<token>`: the cookie the app sets in its WebView.
fn key_cookie(ts: &TestServer) -> String {
    let host = ts.base.trim_start_matches("http://");
    format!("{}={}", clax_server::auth::key_cookie_name(host), ts.token)
}

async fn status(ts: &TestServer, path: &str, cookie: Option<&str>) -> u16 {
    let mut req = ts.client.get(format!("{}{path}", ts.base));
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    req.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn the_token_route_answers_no_one_without_a_credential() {
    let ts = locked().await;
    // What a browser's own token request carries, which any local program
    // can send.
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(body.get("token").is_none());
    // A wrong key cookie is no credential.
    let host = ts.base.trim_start_matches("http://");
    let wrong = format!("{}=nope", clax_server::auth::key_cookie_name(host));
    assert_eq!(status(&ts, "/api/token", Some(&wrong)).await, 401);
}

#[tokio::test]
async fn reads_are_refused_without_a_credential_and_served_with_one() {
    let ts = locked().await;
    let a = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let reads = [
        "/api/artifacts".to_string(),
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/versions"),
        format!("/api/artifacts/{aid}/versions/1/files/index.html"),
        format!("/api/artifacts/{aid}/docs:collections"),
        format!("/a/{aid}"),
        "/".to_string(),
    ];
    for path in &reads {
        assert_eq!(status(&ts, path, None).await, 401, "{path} unauthenticated");
        let authed = ts.get_authed(path).await.status();
        assert!(authed.is_success(), "{path} with the token: {authed}");
    }
    // The WebView's key cookie admits the shell page and its token request,
    // which hands the shell its cookies as before. (Documents ask the
    // handler's own credential, which the shell sends as the token.)
    let key = key_cookie(&ts);
    for path in reads.iter().filter(|p| !p.contains("/docs")) {
        let st = status(&ts, path, Some(&key)).await;
        assert!((200..300).contains(&st), "{path} with the key cookie: {st}");
    }
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("cookie", &key)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let cookies: Vec<String> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(cookies.iter().any(|c| c.starts_with("clax_owner_")));
    assert!(cookies.iter().any(|c| c.starts_with("clax_events_")));
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["token"], ts.token);
    // The owner cookie the token request set admits the API too.
    assert_eq!(
        status(&ts, "/api/artifacts", Some(&ts.owner_cookie())).await,
        200
    );
}

#[tokio::test]
async fn health_static_files_and_content_by_id_stay_open() {
    let ts = locked().await;
    let a = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(status(&ts, "/healthz", None).await, 200);
    assert_eq!(status(&ts, &format!("/c/{aid}/v/1/"), None).await, 200);
    // The artifact origin's content, rewritten to `/c/<id>/...`.
    let port = ts.addr.port();
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", format!("{aid}.localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn the_default_daemon_still_serves_loopback_without_a_credential() {
    let ts = TestServer::spawn().await;
    assert_eq!(status(&ts, "/api/token", None).await, 200);
    assert_eq!(status(&ts, "/api/artifacts", None).await, 200);
}
