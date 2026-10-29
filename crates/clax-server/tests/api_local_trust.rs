//! A local daemon trusts no one for reaching its port: owner power needs
//! the daemon token or an owner browser session a token holder opened for
//! a browser (a redeemed login code), and nothing a local process can forge
//! (`Sec-Fetch-Site`, `Origin`, `Host`, `X-Clax-Channel`) stands for either.

use clax_core::working::ManualClock;
use clax_testkit::{LAN_HEADER, LAN_LOCAL, PEER_HEADER, TestServer, element_anchor};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

/// A client that never follows a redirect, as a browser's address bar
/// answer is read here.
fn unfollowed() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// `/login?code=…&next=…` for `next`, made with the token.
async fn login_path(ts: &TestServer, next: &str) -> String {
    let res = ts
        .authed(ts.client.post(format!("{}/api/browser-login", ts.base)))
        .json(&json!({"next": next}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["expires_in"], 120);
    v["path"].as_str().unwrap().to_string()
}

/// The `name=value` of each cookie `res` sets.
fn set_cookies(res: &reqwest::Response) -> Vec<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").to_string())
        .collect()
}

/// The owner cookie among `cookies`.
fn owner_cookie_of(ts: &TestServer, cookies: &[String]) -> String {
    let name = clax_http::identity::owner_cookie_name(ts.base.trim_start_matches("http://"));
    cookies
        .iter()
        .find(|c| c.starts_with(&format!("{name}=")))
        .expect("an owner cookie")
        .clone()
}

async fn code_of(res: reqwest::Response) -> (u16, String) {
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    (
        status,
        v["error"]["code"].as_str().unwrap_or("").to_string(),
    )
}

/// The headers a local process can send to pass for the shell's own request.
fn forged(req: reqwest::RequestBuilder, ts: &TestServer) -> reqwest::RequestBuilder {
    req.header("sec-fetch-site", "same-origin")
        .header("origin", &ts.base)
        .header("x-clax-channel", "owner")
        .header(
            "clax-signature",
            "t=1, n=AAAAAAAAAAAAAAAAAAAAAA, s=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
}

#[tokio::test]
async fn the_token_is_refused_to_a_loopback_caller_without_a_session() {
    let ts = TestServer::spawn().await;
    let url = format!("{}/api/token", ts.base);
    for (what, req) in [
        ("plain", ts.client.get(&url)),
        (
            "same-origin",
            ts.client.get(&url).header("sec-fetch-site", "same-origin"),
        ),
        (
            "the daemon's origin",
            ts.client.get(&url).header("origin", &ts.base),
        ),
        ("every forgeable header", forged(ts.client.get(&url), &ts)),
    ] {
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 401, "{what}");
        assert!(set_cookies(&res).is_empty(), "{what}: no cookie is set");
        assert!(!res.text().await.unwrap().contains(&ts.token), "{what}");
    }
    // A browser of the owner's is served it, and nothing more is set.
    let res = ts
        .client
        .get(&url)
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(set_cookies(&res).is_empty());
    assert_eq!(res.json::<Value>().await.unwrap()["token"], ts.token);
}

#[tokio::test]
async fn a_login_code_opens_one_browser_session_once() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let browser = unfollowed();
    let path = login_path(&ts, "/a/7q3k9mzx2b4t?thread=x").await;
    assert!(path.starts_with("/login?code=cxi_"), "{path}");
    let res = browser
        .get(format!("{}{path}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers()["location"], "/a/7q3k9mzx2b4t?thread=x");
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    let set: Vec<String> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let port = ts.addr.port();
    for (name, path, same_site) in [
        (format!("clax_owner_{port}"), "/", "Lax"),
        (format!("clax_events_{port}"), "/api/events", "Strict"),
        (format!("clax_events_{port}"), "/api/stream", "Strict"),
    ] {
        assert!(
            set.iter().any(|c| c.starts_with(&format!("{name}=cxb_"))
                && c.contains(&format!("Path={path};"))
                && c.contains("HttpOnly")
                && c.contains(&format!("Max-Age={}", 400 * 24 * 3600))
                && c.ends_with(&format!("SameSite={same_site}"))),
            "{name} {path}: {set:?}"
        );
    }
    let cookie = owner_cookie_of(&ts, &set_cookies(&res));
    // The session is the owner's browser.
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A second redemption is gone, and opens nothing.
    let again = browser
        .get(format!("{}{path}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::GONE);
    assert!(set_cookies(&again).is_empty());
    // A code past its two minutes is gone too.
    let late = login_path(&ts, "/").await;
    clock.advance(120);
    let res = browser
        .get(format!("{}{late}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::GONE);
    assert!(set_cookies(&res).is_empty());
    // A next that is not a path of this daemon: refused where it is made...
    for next in [
        "https://evil.example/",
        "//evil.example/",
        "/\\evil.example",
    ] {
        let res = ts
            .authed(ts.client.post(format!("{}/api/browser-login", ts.base)))
            .json(&json!({"next": next}))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (400, "invalid_next".into()), "{next}");
    }
    // ...and where it is redeemed, before the code is spent.
    let fresh = login_path(&ts, "/").await;
    let code = fresh
        .trim_start_matches("/login?code=")
        .split('&')
        .next()
        .unwrap()
        .to_string();
    let res = browser
        .get(format!(
            "{}/login?code={code}&next=https://evil.example/",
            ts.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookies(&res).is_empty());
    let res = browser
        .get(format!("{}{fresh}", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::SEE_OTHER,
        "the code was not spent"
    );
    // A browser that already holds a session keeps it: no new one is set.
    let path = login_path(&ts, "/").await;
    let res = browser
        .get(format!("{}{path}", ts.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(set_cookies(&res).is_empty());
    // Only the token makes a code.
    for req in [
        ts.client.post(format!("{}/api/browser-login", ts.base)),
        forged(
            ts.client.post(format!("{}/api/browser-login", ts.base)),
            &ts,
        )
        .header("cookie", &cookie),
    ] {
        let res = req.json(&json!({"next": "/"})).send().await.unwrap();
        assert_eq!(res.status(), 401);
    }
}

#[tokio::test]
async fn an_owner_session_survives_a_restart_and_ends_when_revoked() {
    let ts = TestServer::spawn().await;
    let value = ts.owner_cookie().split_once('=').unwrap().1.to_string();
    let again = ts.reopened().await;
    let cookie = format!(
        "{}={value}",
        clax_http::identity::owner_cookie_name(again.base.trim_start_matches("http://"))
    );
    let token = |ts: &TestServer, cookie: String| {
        let req = ts
            .client
            .get(format!("{}/api/token", ts.base))
            .header("cookie", cookie);
        async move { req.send().await.unwrap().status() }
    };
    assert_eq!(token(&again, cookie.clone()).await, 200);
    let hash = clax_core::secret::sha256_hex(&value);
    assert!(again.state.owner_sessions.end(&again.store, &hash).unwrap());
    assert_eq!(token(&again, cookie).await, 401);
}

#[tokio::test]
async fn an_owner_session_fetched_by_another_site_is_no_credential() {
    let ts = TestServer::spawn().await;
    let viewers = || {
        let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
        c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = viewers();
    // An artifact's `<id>.localhost` page or another localhost port is the
    // same site, so the browser sends the Lax owner cookie with its fetches.
    for site in ["same-site", "cross-site"] {
        for path in ["/api/viewers/me", "/api/artifacts", "/api/token"] {
            let res = ts
                .client
                .get(format!("{}{path}", ts.base))
                .header("cookie", ts.owner_cookie())
                .header("sec-fetch-site", site)
                .send()
                .await
                .unwrap();
            assert!(set_cookies(&res).is_empty(), "{site} {path}");
            assert_eq!(code_of(res).await, (401, "no_credential".into()), "{site} {path}");
        }
    }
    assert_eq!(viewers(), before, "no viewer was made");
    // The shell's own fetches, the same cookie, are the owner's.
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn owner_cookies_are_not_derived_from_the_token() {
    let ts = TestServer::spawn().await;
    let host = ts.base.trim_start_matches("http://");
    // The cookies earlier builds computed from the token, under their labels.
    let derived = |label: &str| {
        use sha2::{Digest, Sha256};
        let d = Sha256::new()
            .chain_update(label.as_bytes())
            .chain_update(ts.token.as_bytes())
            .finalize();
        d.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let owner = format!(
        "{}={}",
        clax_http::identity::owner_cookie_name(host),
        derived("clax owner cookie\n")
    );
    let events = format!(
        "{}={}",
        clax_http::auth::events_cookie_name(host),
        derived("clax events cookie\n")
    );
    for cookie in [&owner, &events, &format!("{owner}; {events}")] {
        for path in ["/api/token", "/api/viewers/me", "/api/stream", "/api/inbox"] {
            let res = forged(ts.client.get(format!("{}{path}", ts.base)), &ts)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 401, "{cookie} {path}");
        }
    }
    // The session's own value is no hash of the token.
    let value = ts.owner_cookie().split_once('=').unwrap().1.to_string();
    assert!(value.starts_with("cxb_"));
    assert_ne!(value, derived("clax owner cookie\n"));
}

#[tokio::test]
async fn a_caller_without_a_credential_cannot_comment_or_mint_a_viewer() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Roadmap", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let ada = ts.viewer(Some("Ada")).await;
    let tid = ts.thread_as(&aid, &ada.cookie, "a thread").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let viewers = || {
        let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
        c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = viewers();
    for headers in [false, true] {
        let with = |req: reqwest::RequestBuilder| if headers { forged(req, &ts) } else { req };
        // No viewer is minted.
        let res = with(ts.client.get(format!("{}/api/viewers/me", ts.base)))
            .send()
            .await
            .unwrap();
        assert!(set_cookies(&res).is_empty());
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        // No comment is written.
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", "from nowhere")
            .text("version", "1");
        let res = with(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/threads", ts.base)),
        )
        .multipart(form)
        .send()
        .await
        .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        let res = with(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        )))
        .json(&json!({"body": "from nowhere"}))
        .send()
        .await
        .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        // Nothing is sent to an agent.
        let res = with(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/send",
            ts.base
        )))
        .send()
        .await
        .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        // Nor read.
        let res = with(ts.client.get(format!("{}/api/artifacts", ts.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        // A cookie the daemon never minted names no one.
        let res = with(ts.client.get(format!("{}/api/viewers/me", ts.base)))
            .header(
                "cookie",
                "clax_viewer=cxv_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()));
        // Nor does a bearer that is not the token.
        let res = with(ts.client.get(format!("{}/api/artifacts", ts.base)))
            .bearer_auth("not-the-token")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        // Nor a `?token=` that is not the token, where a route takes one.
        for path in ["/api/events", &format!("/api/artifacts/{aid}/room")] {
            let res = with(ts.client.get(format!("{}{path}?token=not-the-token", ts.base)))
                .send()
                .await
                .unwrap();
            assert_eq!(code_of(res).await, (401, "no_credential".into()), "{path}");
        }
    }
    assert_eq!(viewers(), before, "no viewer was made");
    let threads: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    let t = &threads["threads"][0];
    assert_eq!(threads["threads"].as_array().unwrap().len(), 1);
    assert_eq!(t["comments"].as_array().unwrap().len(), 1);
    assert_eq!(t["sent_to_agent"], false);
    // The open paths stay open.
    for path in ["/healthz", "/.well-known/clax"] {
        assert_eq!(ts.get(path).await.status(), 200, "{path}");
    }
}

#[tokio::test]
async fn a_planted_viewer_is_claimed_only_at_login() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Roadmap", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A viewer cookie planted in the browser (a page on another port, a
    // local process), with a comment of its viewer's.
    let planted = ts.viewer(Some("Mallory")).await;
    ts.thread_as(&aid, &planted.cookie, "planted").await;
    // The shell's token request with it, as a browser of the owner's,
    // claims nothing.
    let res = forged(ts.client.get(format!("{}/api/token", ts.base)), &ts)
        .header(
            "cookie",
            format!("{}; clax_viewer={}", ts.owner_cookie(), planted.cookie),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(set_cookies(&res).is_empty());
    assert!(ts.store.get_viewer(&planted.cookie).unwrap().is_some());
    // The owner holds a browser's identity already, so a claim merges.
    let owner = ts.owner_public_id().await;
    // Only the browser redeeming a code the token holder made claims it,
    // and the merged viewer's cookie names no one after.
    let path = login_path(&ts, "/").await;
    let res = unfollowed()
        .get(format!("{}{path}", ts.base))
        .header("cookie", format!("clax_viewer={}", planted.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(
        set_cookies(&res).iter().any(|c| c == "clax_viewer="),
        "the viewer cookie is cleared"
    );
    assert!(ts.store.get_viewer(&planted.cookie).unwrap().is_none());
    let res = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", format!("clax_viewer={}", planted.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    let threads: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        threads["threads"][0]["comments"][0]["author_public_id"], owner,
        "its words are the owner's now"
    );
}

#[tokio::test]
async fn browser_only_decisions_refuse_the_bearer_token() {
    let ts = TestServer::spawn().await;
    const GID: &str = "01J9ZQ3W5X6Y7Z8A9B0C1D2E3F";
    const PID: &str = "u_0123456789abcdef012345";
    let decisions = |cred: &dyn Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder| {
        let base = &ts.base;
        vec![
            (
                "approving a connection",
                cred(forged(
                    ts.client.post(format!("{base}/api/grants/{GID}/approve")),
                    &ts,
                ))
                .json(&json!({"user_code": "ABCD-EFGH", "name": "Sam"})),
            ),
            (
                "setting a person's rights",
                cred(forged(
                    ts.client.put(format!("{base}/api/people/{PID}/rights")),
                    &ts,
                ))
                .json(&json!({"rights": {}})),
            ),
            (
                "revoking a person",
                cred(forged(
                    ts.client.delete(format!("{base}/api/people/{PID}")),
                    &ts,
                )),
            ),
        ]
    };
    let token = |r: reqwest::RequestBuilder| r.bearer_auth(&ts.token);
    for (what, req) in decisions(&token) {
        let res = req.send().await.unwrap();
        assert_eq!(
            code_of(res).await,
            (403, "browser_required".into()),
            "{what}"
        );
    }
    // An agent's session marks it the agent, whatever it claims.
    let agent =
        |r: reqwest::RequestBuilder| r.bearer_auth(&ts.token).header("x-clax-channel", "agent");
    for (what, req) in decisions(&agent) {
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 403, "{what}");
    }
    // The owner's browser session gets past the gate (to the decision's own answer).
    let cookie = ts.owner_cookie();
    let browser = |r: reqwest::RequestBuilder| r.header("cookie", cookie.clone());
    for (what, req) in decisions(&browser) {
        let (status, code) = code_of(req.send().await.unwrap()).await;
        assert!(
            code != "browser_required" && status != 401,
            "{what}: {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_lan_address_peer_is_held_to_the_loopback_rule() {
    let ts = TestServer::spawn_on(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |_| {}).await;
    let port = ts.addr.port();
    let own = format!("{LAN_LOCAL}:{port}");
    // This machine reaching its own LAN address: peer and local alike.
    let from_own = |req: reqwest::RequestBuilder| {
        req.header(LAN_HEADER, "1")
            .header(PEER_HEADER, LAN_LOCAL.to_string())
            .header("host", &own)
    };
    let client = unfollowed();
    for path in ["/api/token", "/api/artifacts", "/api/viewers/me"] {
        let res = forged(from_own(client.get(format!("{}{path}", ts.base))), &ts)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()), "{path}");
    }
    // A login link opened there makes the browser the owner's there.
    let path = login_path(&ts, "/").await;
    let res = from_own(client.get(format!("{}{path}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let name = clax_http::identity::owner_cookie_name(&own);
    let cookie = set_cookies(&res)
        .into_iter()
        .find(|c| c.starts_with(&format!("{name}=")))
        .expect("an owner cookie for the LAN address");
    let res = from_own(client.get(format!("{}/api/token", ts.base)))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // Another machine replaying the cookie is no one.
    let res = client
        .get(format!("{}/api/token", ts.base))
        .header(LAN_HEADER, "1")
        .header("host", &own)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // Nor is another machine with a `Clax-Signature` and no agent
    // credential anyone.
    for path in ["/api/viewers/me", "/api/artifacts"] {
        let res = forged(
            client
                .get(format!("{}{path}", ts.base))
                .header(LAN_HEADER, "1")
                .header("host", &own),
            &ts,
        )
        .send()
        .await
        .unwrap();
        assert!(set_cookies(&res).is_empty(), "{path}");
        assert_eq!(res.status(), 403, "{path}");
    }
    // Nor does another machine redeem a code.
    let path = login_path(&ts, "/").await;
    let res = client
        .get(format!("{}{path}", ts.base))
        .header(LAN_HEADER, "1")
        .header("host", &own)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn the_extension_gateway_refuses_a_forged_origin_without_a_credential() {
    let ts = TestServer::spawn().await;
    let origin = clax_core::extension::extension_origin(&ts.extension_id());
    for (method, path) in [
        ("GET", "/api/live/pages"),
        ("GET", "/api/inbox"),
        ("POST", "/api/extension/credentials"),
    ] {
        for auth in [None, Some("Clax-Extension cxe_forged")] {
            let mut req = ts
                .client
                .request(method.parse().unwrap(), format!("{}{path}", ts.base))
                .header("origin", &origin)
                .header("sec-fetch-site", "none");
            if let Some(a) = auth {
                req = req.header("authorization", a);
            }
            if method == "POST" {
                req = req.json(&json!({"extension_id": ts.extension_id()}));
            }
            let res = req.send().await.unwrap();
            let status = res.status().as_u16();
            let body = res.text().await.unwrap();
            assert!(
                status == 401 || status == 403,
                "{method} {path} {auth:?}: {status} {body}"
            );
        }
    }
    // The MCP endpoint and the event streams, with every forgeable header,
    // in the extension's name or the daemon's.
    for (method, path) in [
        ("POST", "/mcp"),
        ("GET", "/api/stream"),
        ("GET", "/api/events"),
    ] {
        for from in [origin.clone(), ts.base.clone()] {
            let res = forged(
                ts.client
                    .request(method.parse().unwrap(), format!("{}{path}", ts.base)),
                &ts,
            )
            .header("origin", &from)
            .send()
            .await
            .unwrap();
            let status = res.status().as_u16();
            assert!(
                status == 401 || status == 403,
                "{method} {path} {from}: {status}"
            );
        }
    }
}

#[tokio::test]
async fn a_cold_browser_gets_the_open_from_your_terminal_page() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Roadmap", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let html = "text/html,application/xhtml+xml";
    for path in ["/".to_string(), format!("/a/{aid}"), "/inbox".to_string()] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("accept", html)
            .header("sec-fetch-site", "none")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "{path}");
        assert!(
            res.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html"),
            "{path}"
        );
        let page = res.text().await.unwrap();
        assert!(page.contains("Open Clax from your terminal"), "{path}");
        assert!(
            page.contains("<code>clax open</code>") && page.contains("<code>clax login</code>")
        );
        assert!(!page.contains("Roadmap"), "{path}: nothing of the daemon's");
    }
    // With a session the shell is served.
    let res = ts
        .client
        .get(format!("{}/a/{aid}", ts.base))
        .header("accept", html)
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        !res.text()
            .await
            .unwrap()
            .contains("Open Clax from your terminal")
    );
}
