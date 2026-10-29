//! A daemon serving with `require_credential` (`clax serve --foreground
//! --require-credential`, the app's embedded daemon): nothing but `/healthz`,
//! the shell's static files and content addressed by artifact ID answers a
//! request that carries no credential, `GET /api/token` included.

use clax_testkit::TestServer;

async fn locked() -> TestServer {
    TestServer::spawn_with(|s| s.require_credential = true).await
}

/// `clax_key_<port>=<token>`: the cookie the app sets in its WebView.
fn key_cookie(ts: &TestServer) -> String {
    let host = ts.base.trim_start_matches("http://");
    format!("{}={}", clax_http::auth::key_cookie_name(host), ts.token)
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
    assert_eq!(body["error"]["code"], "no_credential");
    assert!(body.get("token").is_none());
    // A wrong key cookie is no credential.
    let host = ts.base.trim_start_matches("http://");
    let wrong = format!("{}=nope", clax_http::auth::key_cookie_name(host));
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
    // The WebView's key cookie, which stands for an owner browser session,
    // admits the shell page and its token request. (Documents ask the
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
    assert!(
        res.headers().get("set-cookie").is_none(),
        "it sets no cookie"
    );
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["token"], ts.token);
    // An owner browser session admits the API too.
    assert_eq!(
        status(&ts, "/api/artifacts", Some(&ts.owner_cookie())).await,
        200
    );
}

#[tokio::test]
async fn health_stays_open_and_a_sites_content_is_its_access_checks() {
    let ts = locked().await;
    let a = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(status(&ts, "/healthz", None).await, 200);
    // A site host's requests pass the credential gate: its access decides.
    let port = ts.addr.port();
    let res = ts
        .client
        .get(format!("{}/_clax/api/whoami", ts.base))
        .header("host", format!("{aid}.localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "site_session_required", "{body}");
    assert_eq!(ts.site_get(&aid, "/_clax/v/1/").await.status(), 200);
    assert_eq!(ts.site_get(&aid, "/").await.status(), 200);
}

#[tokio::test]
async fn the_default_daemon_serves_loopback_nothing_without_a_credential() {
    let ts = TestServer::spawn().await;
    assert_eq!(status(&ts, "/api/token", None).await, 401);
    assert_eq!(status(&ts, "/api/artifacts", None).await, 401);
    assert_eq!(status(&ts, "/healthz", None).await, 200);
}

/// Content addressed by artifact ID never names its artifact to the hosts it
/// loads from: every content response, on the daemon's host and on the
/// artifact's own origin, and every asset carry `Referrer-Policy: no-referrer`.
#[tokio::test]
async fn content_and_assets_send_no_referrer() {
    let ts = locked().await;
    let a = ts
        .publish(
            "Notes",
            &[
                ("index.html", "<p>hi</p>"),
                ("app.js", "1"),
                ("more.html", "<p>more</p>"),
            ],
        )
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let port = ts.addr.port();
    let origin = format!("{aid}.localhost:{port}");
    let part = reqwest::multipart::Part::bytes(vec![137, 80, 78, 71])
        .file_name("logo.png")
        .mime_str("image/png")
        .unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/assets", ts.base))
                .multipart(reqwest::multipart::Form::new().part("file", part)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let blob = res.json::<serde_json::Value>().await.unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    let host = ts.base.trim_start_matches("http://").to_string();
    let cookie = ts.site_cookie(&aid).await;
    let cases = [
        ("/".to_string(), origin.clone(), 200),
        ("/app.js".to_string(), origin.clone(), 200),
        ("/more.html".to_string(), origin.clone(), 200),
        ("/_clax/v/1/".to_string(), origin.clone(), 200),
        ("/_clax/v/1/app.js".to_string(), origin.clone(), 200),
        (blob.clone(), origin, 200),
        // The public host serves no asset: it redirects to the asset's site.
        (blob, host, 301),
    ];
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (path, h, want) in &cases {
        let res = client
            .get(format!("{}{path}", ts.base))
            .header("host", h)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), *want, "{path} on {h}");
        assert_eq!(
            res.headers()
                .get("referrer-policy")
                .and_then(|v| v.to_str().ok()),
            Some("no-referrer"),
            "{path} on {h}"
        );
    }
}
