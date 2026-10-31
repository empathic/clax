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

/// The proof a login's page carries.
async fn proof_of(res: reqwest::Response) -> String {
    clax_testkit::login_page_proof(&res.text().await.unwrap())
}

/// `GET /api/token`'s status for a browser holding `cookie` that sends
/// `proof` (none: `None`), from a client that carries nothing else.
async fn token_status(ts: &TestServer, cookie: &str, proof: Option<&str>) -> u16 {
    let mut req = reqwest::Client::new()
        .get(format!("{}/api/token", ts.base))
        .header("cookie", cookie);
    if let Some(p) = proof {
        req = req.header(clax_http::local_trust::PROOF_HEADER, p);
    }
    req.send().await.unwrap().status().as_u16()
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
    // The login's page: it writes the session's proof into the origin's
    // storage and goes on to `next`.
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get("location").is_none());
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert_eq!(res.headers()["x-frame-options"], "DENY");
    let csp = res.headers()["content-security-policy"].to_str().unwrap();
    assert!(
        csp.starts_with("default-src 'none'; script-src 'nonce-"),
        "{csp}"
    );
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
    let page = res.text().await.unwrap();
    assert!(
        page.contains("data-next=\"/a/7q3k9mzx2b4t?thread=x\""),
        "{page}"
    );
    assert!(page.contains("data-key=\"clax.owner_proof."), "{page}");
    let proof = clax_testkit::login_page_proof(&page);
    assert!(proof.starts_with("cxp_"), "{proof}");
    assert!(!cookie.contains(&proof), "the proof is never the cookie");
    // The session with its proof is the owner's browser; its cookie alone,
    // or with another proof, is no credential.
    assert_eq!(token_status(&ts, &cookie, Some(&proof)).await, 200);
    assert_eq!(token_status(&ts, &cookie, None).await, 401);
    assert_eq!(
        token_status(&ts, &cookie, Some(&ts.owner_proof())).await,
        401
    );
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
    assert_eq!(res.status(), StatusCode::OK, "the code was not spent");
    // A browser that already holds a session gets a new one in its place
    // (its proof may be gone from the browser's storage); the cookie it
    // held ends nothing by itself (the page retires it with its proof).
    let path = login_path(&ts, "/").await;
    let res = browser
        .get(format!("{}{path}", ts.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let renewed = owner_cookie_of(&ts, &set_cookies(&res));
    assert_ne!(renewed, cookie);
    let renewed_proof = proof_of(res).await;
    assert_eq!(token_status(&ts, &cookie, Some(&proof)).await, 200);
    assert_eq!(token_status(&ts, &renewed, Some(&proof)).await, 401);
    assert_eq!(token_status(&ts, &renewed, Some(&renewed_proof)).await, 200);
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
    // The proof the browser keeps survives with it.
    let proof = ts.owner_proof();
    assert_eq!(token_status(&again, &cookie, Some(&proof)).await, 200);
    let hash = clax_core::secret::sha256_hex(&value);
    assert!(again.state.owner_sessions.end(&again.store, &hash).unwrap());
    assert_eq!(token_status(&again, &cookie, Some(&proof)).await, 401);
}

/// `DELETE /api/browser-login` (the token, by hand) ends every owner
/// browser session: each browser's cookie opens nothing after, a new
/// login code opens a new one. The owner's agents, a browser session
/// alone and no credential are refused.
#[tokio::test]
async fn the_owner_ends_every_browser_session_at_once() {
    let ts = TestServer::spawn().await;
    let first = ts.owner_cookie();
    let browser = unfollowed();
    let path = login_path(&ts, "/").await;
    let res = browser
        .get(format!("{}{path}", ts.base))
        .send()
        .await
        .unwrap();
    let second = owner_cookie_of(&ts, &set_cookies(&res));
    let second_proof = proof_of(res).await;
    let first_proof = ts.owner_proof();
    assert_eq!(token_status(&ts, &first, Some(&first_proof)).await, 200);
    assert_eq!(token_status(&ts, &second, Some(&second_proof)).await, 200);
    let end = || ts.client.delete(format!("{}/api/browser-login", ts.base));
    for (what, req, want) in [
        ("no credential", end(), (401, "")),
        (
            "a browser session alone",
            end().header("cookie", &first),
            (401, "unauthorized"),
        ),
        (
            "an agent of the owner's",
            ts.authed(end()).header("x-clax-channel", "agent"),
            (403, "not_for_agents"),
        ),
    ] {
        let (status, code) = code_of(req.send().await.unwrap()).await;
        assert_eq!(status, want.0, "{what}");
        if !want.1.is_empty() {
            assert_eq!(code, want.1, "{what}");
        }
    }
    assert_eq!(
        token_status(&ts, &first, Some(&first_proof)).await,
        200,
        "nothing ended yet"
    );
    let res = ts.authed(end()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["ended"], 2, "{v}");
    assert_eq!(token_status(&ts, &first, Some(&first_proof)).await, 401);
    assert_eq!(token_status(&ts, &second, Some(&second_proof)).await, 401);
    // A new login code opens a new session.
    let path = login_path(&ts, "/").await;
    let res = browser
        .get(format!("{}{path}", ts.base))
        .send()
        .await
        .unwrap();
    let cookie = owner_cookie_of(&ts, &set_cookies(&res));
    let proof = proof_of(res).await;
    assert_eq!(token_status(&ts, &cookie, Some(&proof)).await, 200);
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
            assert_eq!(
                code_of(res).await,
                (401, "no_credential".into()),
                "{site} {path}"
            );
        }
    }
    assert_eq!(viewers(), before, "no viewer was made");
    // A browser too old to send `Sec-Fetch-Site` still names the page's
    // origin: another one's is no credential either.
    for origin in ["http://localhost:3000", "null"] {
        let res = ts
            .client
            .get(format!("{}/api/token", ts.base))
            .header("cookie", ts.owner_cookie())
            .header("origin", origin)
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(res).await,
            (401, "no_credential".into()),
            "{origin}"
        );
    }
    let res = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", &ts.base)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the daemon's own origin");
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
            let res = with(
                ts.client
                    .get(format!("{}{path}?token=not-the-token", ts.base)),
            )
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
    assert_eq!(res.status(), StatusCode::OK);
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
    assert_eq!(res.status(), StatusCode::OK);
    let name = clax_http::identity::owner_cookie_name(&own);
    let cookie = set_cookies(&res)
        .into_iter()
        .find(|c| c.starts_with(&format!("{name}=")))
        .expect("an owner cookie for the LAN address");
    let proof = proof_of(res).await;
    let res = from_own(client.get(format!("{}/api/token", ts.base)))
        .header("cookie", &cookie)
        .header(clax_http::local_trust::PROOF_HEADER, &proof)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // Another machine replaying the cookie, its proof too, is no one.
    let res = client
        .get(format!("{}/api/token", ts.base))
        .header(LAN_HEADER, "1")
        .header("host", &own)
        .header("cookie", &cookie)
        .header(clax_http::local_trust::PROOF_HEADER, &proof)
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

#[tokio::test]
async fn the_daemons_own_mcp_open_hands_the_opener_a_login_link() {
    use clax_mcp::tools::{Opener, PublishArgs, TargetArgs};
    use rmcp::handler::server::wrapper::Parameters;
    let dir = tempfile::tempdir().unwrap();
    let opener = clax_fake_exe::install(
        &dir.path().join("open"),
        clax_testkit::opener::RECORDING_OPENER,
    );
    let ts = TestServer::spawn().await;
    // The `/mcp` adapter's tool set, acting for a caller with the token.
    let tools = clax_server::routes::mcp_http::tools(&ts.state)
        .with_opener(Opener::Program(opener))
        .with_open_wait(std::time::Duration::from_secs(30))
        .for_caller(Some(format!("Bearer {}", ts.token).parse().unwrap()));
    let text = |r: rmcp::model::CallToolResult| -> Value {
        serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
    };
    let published = text(
        tools
            .publish(Parameters(PublishArgs {
                html: Some("<title>Open me</title>".into()),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    let id = published["artifact_id"].as_str().unwrap().to_string();
    let o = text(
        tools
            .open(Parameters(TargetArgs {
                url_or_id: id.clone(),
            }))
            .await
            .unwrap(),
    );
    assert_eq!(o["opened"], true, "{o}");
    let got = std::fs::read_to_string(dir.path().join("got")).unwrap();
    let link = clax_testkit::opener::recorded_url(&got);
    assert!(link.contains("/login?code=cxi_"), "{link}");
    assert!(link.ends_with(&format!("next=%2Fa%2F{id}")), "{link}");
}

/// A `/proc/net/tcp` row for the socket from `local` to `remote` owned by
/// `uid`, as the kernel prints it on this machine.
fn tcp_row(local: std::net::SocketAddr, remote: std::net::SocketAddr, uid: u32) -> String {
    let hex = |a: std::net::SocketAddr| match a.ip() {
        IpAddr::V4(ip) => format!("{:08X}:{:04X}", u32::from_ne_bytes(ip.octets()), a.port()),
        IpAddr::V6(_) => unreachable!("the test connects over IPv4"),
    };
    format!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: {} {} 01 00000000:00000000 00:00000000 00000000  {uid}        0 1 1\n",
        hex(local),
        hex(remote)
    )
}

/// The status line of `GET <path>` sent on a connection of its own, after
/// `before` is given the connection's two ends (the browser's, the
/// daemon's).
async fn redeem_on_own_socket(
    ts: &TestServer,
    path: &str,
    before: impl FnOnce(std::net::SocketAddr, std::net::SocketAddr) + Send + 'static,
) -> String {
    let addr = ts.addr;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        addr.port()
    );
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        before(s.local_addr().unwrap(), s.peer_addr().unwrap());
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out.lines().next().unwrap_or("").to_string()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_login_code_redeems_only_for_the_daemons_own_user() {
    let proc = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proc.path().join("self")).unwrap();
    std::fs::create_dir_all(proc.path().join("net")).unwrap();
    std::fs::write(
        proc.path().join("self/status"),
        "Name:\tclax\nUid:\t1000\t1000\t1000\t1000\n",
    )
    .unwrap();
    let root = proc.path().to_path_buf();
    let ts = TestServer::spawn_with(move |s| s.proc_root = Some(root)).await;
    let path = login_path(&ts, "/").await;
    // No row lists the connection: who opened it cannot be told, so it is
    // refused, and the error says how in.
    let line = redeem_on_own_socket(&ts, &path, |_, _| {}).await;
    assert!(line.contains(" 403 "), "{line}");
    let res = ts
        .client
        .get(format!("{}{path}", ts.base))
        .send()
        .await
        .unwrap();
    let (status, body): (u16, Value) = (res.status().as_u16(), res.json().await.unwrap());
    assert_eq!(status, 403);
    assert_eq!(body["error"]["code"], "not_this_user");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("clax login"),
        "{body}"
    );
    // Another user's socket is refused.
    let tcp = proc.path().join("net/tcp");
    let t = tcp.clone();
    let line = redeem_on_own_socket(&ts, &path, move |browser, daemon| {
        std::fs::write(&t, tcp_row(browser, daemon, 1001)).unwrap();
    })
    .await;
    assert!(line.contains(" 403 "), "{line}");
    // The daemon's user's is redeemed: the refusals spent nothing.
    let line = redeem_on_own_socket(&ts, &path, move |browser, daemon| {
        std::fs::write(&tcp, tcp_row(browser, daemon, 1000)).unwrap();
    })
    .await;
    assert!(line.contains(" 200 "), "{line}");
}

/// The owner cookie is sent to every port of the daemon's host name, so a
/// server on another port can capture it and replay it from this machine
/// with the daemon's `Host`. Without the proof the login's page wrote into
/// the daemon's origin (which no other port reads) it is no credential: no
/// token, no read, no write, no event stream. Only the realtime routes take
/// the proof in their address.
#[tokio::test]
async fn an_owner_cookie_replayed_without_its_proof_is_no_credential() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Roadmap", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let host = ts.base.trim_start_matches("http://").to_string();
    // What the other port's server holds: the cookies, and no proof.
    let replay = || reqwest::Client::new();
    for path in [
        "/api/token".to_string(),
        "/api/artifacts".to_string(),
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/threads"),
        "/api/viewers/me".to_string(),
        "/api/inbox".to_string(),
    ] {
        let res = replay()
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .header("cookie", ts.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()), "{path}");
    }
    let res = replay()
        .patch(format!("{}/api/artifacts/{aid}", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"title": "Taken"}))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    for path in ["/api/events", "/api/stream"] {
        let res = replay()
            .get(format!("{}{path}", ts.base))
            .header("cookie", ts.events_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(res).await, (401, "no_credential".into()), "{path}");
    }
    // A wrong proof is no proof.
    assert_eq!(
        token_status(&ts, &ts.owner_cookie(), Some("cxp_guessed")).await,
        401
    );
    // The proof in the address of a realtime route opens it, and in no
    // other address counts.
    let proof = ts.owner_proof();
    let res = replay()
        .get(format!("{}/api/events?owner_proof={proof}", ts.base))
        .header("cookie", ts.events_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    drop(res);
    let res = replay()
        .get(format!("{}/api/token?owner_proof={proof}", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    // The proof alone is nothing either.
    let res = replay()
        .get(format!("{}/api/token", ts.base))
        .header(clax_http::local_trust::PROOF_HEADER, &proof)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    // The browser holding both is the owner's.
    assert_eq!(
        token_status(&ts, &ts.owner_cookie(), Some(&proof)).await,
        200
    );
}

/// `clax logout` ends the event streams and sockets opened with an owner
/// browser session, not only the requests made after it.
#[tokio::test]
async fn logging_out_ends_the_streams_and_sockets_opened_with_a_session() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let ts = TestServer::spawn().await;
    let aid = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Room", "capabilities": {"room": {}}, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The owner's viewer row, made on first use, which a live socket needs.
    ts.owner_public_id().await;
    let proof = ts.owner_proof();
    let wait = std::time::Duration::from_secs(20);
    let mut streams = Vec::new();
    for path in ["/api/events", "/api/stream"] {
        let res = reqwest::Client::new()
            .get(format!("{}{path}", ts.base))
            .header("cookie", ts.events_cookie())
            .header(clax_http::local_trust::PROOF_HEADER, &proof)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{path}");
        let mut body = res.bytes_stream();
        let first = tokio::time::timeout(wait, body.next())
            .await
            .expect("the stream's ready within 20 s")
            .expect("open")
            .unwrap();
        assert!(
            std::str::from_utf8(&first).unwrap().contains("ready"),
            "{path}"
        );
        streams.push((path, body));
    }
    let ws = ts.base.replacen("http://", "ws://", 1);
    let mut sockets = Vec::new();
    for (what, url) in [
        (
            "room",
            format!("{ws}/api/artifacts/{aid}/room?peer=aaaaaaaaaaaaaaaa&owner_proof={proof}"),
        ),
        (
            "live",
            format!("{ws}/api/artifacts/{aid}/live?owner_proof={proof}"),
        ),
    ] {
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("cookie", ts.owner_cookie().parse().unwrap());
        let (mut s, _) = tokio_tungstenite::connect_async(req)
            .await
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let hello = tokio::time::timeout(wait, s.next())
            .await
            .expect("a welcome within 20 s")
            .expect("open")
            .unwrap();
        assert!(hello.to_text().unwrap().contains("welcome"), "{what}");
        sockets.push((what, s));
    }
    let res = ts
        .authed(ts.client.delete(format!("{}/api/browser-login", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    for (path, mut body) in streams {
        let ended = tokio::time::timeout(wait, async {
            while let Some(chunk) = body.next().await {
                if chunk.is_err() {
                    break;
                }
            }
        })
        .await;
        assert!(ended.is_ok(), "{path} ends at logout");
    }
    for (what, mut s) in sockets {
        let closed = tokio::time::timeout(wait, async {
            loop {
                match s.next().await {
                    Some(Ok(Message::Close(c))) => return c.map(|c| u16::from(c.code)),
                    Some(Ok(_)) => continue,
                    _ => return None,
                }
            }
        })
        .await
        .expect("closed within 20 s");
        assert_eq!(closed, Some(4403), "{what}");
    }
}

/// A browser's login, from a client that carries `cookie` (if any): its
/// owner cookie and proof.
async fn login_in(ts: &TestServer, cookie: Option<&str>) -> (String, String) {
    let path = login_path(ts, "/").await;
    let mut req = unfollowed().get(format!("{}{path}", ts.base));
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = owner_cookie_of(ts, &set_cookies(&res));
    (cookie, proof_of(res).await)
}

/// `POST /api/browser-login/retire` from a browser holding `cookie` that
/// presents `proofs`: the status and the answer.
async fn retire(ts: &TestServer, cookie: &str, proofs: &[&str]) -> (u16, Value) {
    let res = reqwest::Client::new()
        .post(format!("{}/api/browser-login/retire", ts.base))
        .header("cookie", cookie)
        .header("origin", &ts.base)
        .header("sec-fetch-site", "same-origin")
        .header(clax_http::local_trust::PROOF_HEADER, proofs.join(","))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

/// Each login opens a session; the login's page, presenting every proof
/// its browser's storage holds, ends the sessions that browser's earlier
/// logins opened, and leaves another browser's alone.
#[tokio::test]
async fn a_login_retires_the_sessions_its_browser_held() {
    let ts = TestServer::spawn().await;
    let (old, old_proof) = login_in(&ts, None).await;
    let (elsewhere, elsewhere_proof) = login_in(&ts, None).await;
    let (now, now_proof) = login_in(&ts, Some(&old)).await;
    // Until the page retires it, the session the browser held stays live.
    assert_eq!(token_status(&ts, &old, Some(&old_proof)).await, 200);
    // The cookie alone, or with no proof of its own, retires nothing.
    assert_eq!(retire(&ts, &now, &[]).await.0, 401);
    assert_eq!(retire(&ts, &now, &[&old_proof]).await.0, 401);
    let (status, v) = retire(&ts, &now, &[&now_proof, &old_proof]).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v, json!({"kept": 0, "ended": 1, "dropped": [1]}));
    assert_eq!(token_status(&ts, &old, Some(&old_proof)).await, 401);
    assert_eq!(token_status(&ts, &now, Some(&now_proof)).await, 200);
    assert_eq!(
        token_status(&ts, &elsewhere, Some(&elsewhere_proof)).await,
        200,
        "a session whose proof was not presented"
    );
    // The owner's agents may not.
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/browser-login/retire", ts.base)),
        )
        .header("x-clax-channel", "agent")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(res).await, (403, "not_for_agents".into()));
}

/// Two logins at once in one browser: whichever cookie it kept, the proofs
/// its storage holds prove it, and the retire ends the other login's
/// session, naming the proof that stays.
#[tokio::test]
async fn two_logins_at_once_leave_the_cookie_the_browser_kept_proven() {
    let ts = TestServer::spawn().await;
    let (a, a_proof) = login_in(&ts, None).await;
    let (b, b_proof) = login_in(&ts, None).await;
    // The browser applied A's cookie last but ran B's script last: it holds
    // A's cookie and both proofs, B's written first.
    let both = format!("{b_proof},{a_proof}");
    assert_eq!(token_status(&ts, &a, Some(&both)).await, 200);
    let (status, v) = retire(&ts, &a, &[&b_proof, &a_proof]).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v, json!({"kept": 1, "ended": 1, "dropped": [0]}));
    assert_eq!(token_status(&ts, &a, Some(&a_proof)).await, 200);
    assert_eq!(token_status(&ts, &b, Some(&b_proof)).await, 401);
}

/// A server on another port planted the owner's older live cookie C1 on a
/// longer path, so the browser sends it before the login's C2: the retire
/// keeps the session of the proof the login's page presents first, its
/// own, and ends the planted one, so every later `clax open` works.
#[tokio::test]
async fn a_cookie_planted_on_a_longer_path_is_retired_not_the_logins_own() {
    let ts = TestServer::spawn().await;
    let (c1, p1) = login_in(&ts, None).await;
    let (c2, p2) = login_in(&ts, Some(&c1)).await;
    let planted = format!("{c1}; {c2}");
    let (status, v) = retire(&ts, &planted, &[&p2, &p1]).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v, json!({"kept": 0, "ended": 1, "dropped": [1]}));
    assert_eq!(token_status(&ts, &c2, Some(&p2)).await, 200);
    assert_eq!(token_status(&ts, &c1, Some(&p1)).await, 401);
    // The browser's later requests, the planted cookie still sent first on
    // /api/, are the login's session.
    assert_eq!(token_status(&ts, &planted, Some(&p2)).await, 200);
}

/// The retire names, for the page to remove, the presented proofs that
/// prove no live session after it (those it ended, and those of sessions
/// ended before, by `clax logout`), and no other.
#[tokio::test]
async fn the_retire_names_the_dead_proofs_it_was_shown() {
    let ts = TestServer::spawn().await;
    let (old, old_proof) = login_in(&ts, None).await;
    let res = ts
        .authed(ts.client.delete(format!("{}/api/browser-login", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let (_, before) = login_in(&ts, None).await;
    let (now, now_proof) = login_in(&ts, Some(&old)).await;
    let (status, v) = retire(&ts, &now, &[&now_proof, &old_proof, &before]).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v, json!({"kept": 0, "ended": 1, "dropped": [1, 2]}));
}

/// A thread's clip is read with the proof like any other API route: an
/// image request (the cookie alone) is refused, so the shell fetches it.
#[tokio::test]
async fn a_clip_is_read_only_with_the_proof() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Clipped", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .create_thread(&aid, 1, "This part.", Some(clax_testkit::FAKE_PNG))
        .await;
    assert_eq!(res.status(), 201);
    let url = res.json::<Value>().await.unwrap()["thread"]["clip_url"]
        .as_str()
        .unwrap()
        .to_string();
    let get = |proof: Option<String>| {
        let mut req = reqwest::Client::new()
            .get(format!("{}{url}", ts.base))
            .header("cookie", ts.owner_cookie())
            .header(
                "accept",
                "image/avif,image/webp,image/png,image/*,*/*;q=0.8",
            )
            .header("sec-fetch-dest", "image");
        if let Some(p) = proof {
            req = req.header(clax_http::local_trust::PROOF_HEADER, p);
        }
        req.send()
    };
    let res = get(None).await.unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    let res = get(Some(ts.owner_proof())).await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().as_ref(), clax_testkit::FAKE_PNG);
}

/// The Get in Clax page of a local daemon: a page load with the owner
/// cookie and no proof (every navigation of the owner's browser) gets the
/// bare page, which names nothing of the artifact and loads the app's page
/// with the proof; the same request with the proof gets the app's page.
#[tokio::test]
async fn the_get_page_is_bare_for_the_cookie_alone() {
    let ts = TestServer::spawn().await;
    let aid = ts
        .publish("Secret Calculator", &[("index.html", "<p>hi</p>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/versions/1"))
        .await
        .json()
        .await
        .unwrap();
    let content = v["version"]["content_sha256"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/versions/1/bundle", ts.base)),
        )
        .header("content-type", "application/vnd.empathic.clax+zip")
        .header("clax-bundle-lineage", "l_abcdefghijklmnopqrstuvwxyz")
        .header("clax-bundle-version", "3")
        .header("clax-bundle-content-sha256", content)
        .body(b"PK\x03\x04 a bundle".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let load = |path: String, proof: Option<String>| {
        let mut req = reqwest::Client::new()
            .get(format!("{}{path}", ts.base))
            .header("accept", "text/html")
            .header("cookie", ts.owner_cookie());
        if let Some(p) = proof {
            req = req.header(clax_http::local_trust::PROOF_HEADER, p);
        }
        req.send()
    };
    for path in [format!("/a/{aid}/get"), "/a/7q3k9mzx2b4t/get".to_string()] {
        let res = load(path.clone(), None).await.unwrap();
        assert_eq!(res.status(), 200, "{path}");
        let csp = res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            csp.contains("script-src 'nonce-") && csp.contains("connect-src 'self'"),
            "{csp}"
        );
        let html = res.text().await.unwrap();
        assert!(!html.contains("Secret Calculator"), "{html}");
        assert!(!html.contains("/bundle"), "{html}");
        assert!(html.contains("x-clax-owner-proof"), "{html}");
        assert!(html.contains("<h1>Get in Clax</h1>"), "{html}");
    }
    let res = load(format!("/a/{aid}/get"), Some(ts.owner_proof()))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    assert!(html.contains("Secret Calculator"), "{html}");
    assert!(
        html.contains(&format!("/api/artifacts/{aid}/versions/1/bundle")),
        "{html}"
    );
    // Its Download .clax answers the proof alone, as the bare page's
    // script fetches it.
    let bundle = format!("/api/artifacts/{aid}/versions/1/bundle");
    let res = load(bundle.clone(), None).await.unwrap();
    assert_eq!(code_of(res).await, (401, "no_credential".into()));
    let res = load(bundle, Some(ts.owner_proof())).await.unwrap();
    assert_eq!(res.status(), 200);
}
