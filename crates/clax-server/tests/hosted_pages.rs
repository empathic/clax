//! A hosted daemon's pages (`clax_server::hosted`): `/setup` while setup is
//! open, the join and sign-in pages without a referrer, and the artifact
//! page's bootstrap for a guest of a page shared by link, served from a
//! scratch web UI.
#![cfg(debug_assertions)]

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde_json::json;
use std::sync::Arc;

const ENTRY: &str = include_str!("../../../web/shell/artifact.html");

/// A scratch web UI: the artifact entry, a stand-in gallery and stand-in
/// legal documents.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.html"), ENTRY).unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>gallery</p>").unwrap();
    std::fs::write(dir.path().join("terms.html"), "<p>terms</p>").unwrap();
    std::fs::write(dir.path().join("privacy.html"), "<p>privacy</p>").unwrap();
    dir
}

/// A hosted test daemon whose setup takes `s3cret`, serving `dist`.
async fn spawn(clock: Arc<ManualClock>, dist: &tempfile::TempDir) -> TestServer {
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| s.web = web).await
}

#[tokio::test]
async fn the_sign_in_pages_load_without_a_referrer_and_setup_closes_once_done() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let r = ts.get("/setup").await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert_eq!(r.headers()["cache-control"], "no-store");
    for path in ["/join/K7QF-M2XD-9PNA", "/signin/cxl_x", "/signin"] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(
            r.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{path}"
        );
    }
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(ts.get("/setup").await.status(), 404);
}

/// A hosted test daemon serving `dist` and the quickstart release of
/// `clax_testkit::quickstart_tree("landing")`.
async fn spawn_with_quickstart(clock: Arc<ManualClock>, dist: &tempfile::TempDir) -> TestServer {
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let release =
        clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("landing"))
            .map(Arc::new);
    TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.quickstart = release;
    })
    .await
}

/// A client that reports redirects rather than following them.
fn unfollowed() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_signed_out_visitor_lands_on_the_bundled_quickstart() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn_with_quickstart(clock, &dist).await;
    let c = unfollowed();
    let r = c.get(format!("{}/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/start/");
    // The gallery's other pages still send a signed-out visitor to sign in.
    let r = c.get(format!("{}/inbox", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/signin");
    let r = c.get(format!("{}/start", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/start/");
    // The directory is the shell's chrome (the gallery entry), which no page
    // may frame; the quickstart page in it may be framed by its own origin.
    let r = c.get(format!("{}/start/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-security-policy"],
        "frame-ancestors 'none'"
    );
    assert!(r.text().await.unwrap().contains("<p>gallery</p>"));
    let r = c
        .get(format!("{}/start/index.html", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(
        r.headers()["content-security-policy"],
        "frame-ancestors 'self'"
    );
    assert_eq!(r.headers()["x-frame-options"], "SAMEORIGIN");
    assert!(r.text().await.unwrap().contains("<p>landing</p>"));
    let r = c
        .get(format!("{}/start/style.css", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    assert_eq!(r.text().await.unwrap(), "landing");
    let r = c
        .get(format!("{}/start/missing.css", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn the_landing_pages_reel_redirects_to_where_hosted_mode_serves_it() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let release =
        clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("landing"))
            .map(Arc::new);
    let reel = clax_http::hosted::Reel::new(
        "https://videos.example/r/downloads/default.mp4",
        Some("https://videos.example/r/thumbnails/thumbnail.jpg"),
    )
    .unwrap();
    let ts = TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.quickstart = release;
        s.hosted = s.hosted.take().map(|h| {
            Arc::new(
                Arc::try_unwrap(h)
                    .expect("hosted is not shared yet")
                    .with_reel(Some(reel)),
            )
        });
    })
    .await;
    let c = unfollowed();
    let r = c
        .get(format!("{}/start/reel.mp4", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(
        r.headers()["location"],
        "https://videos.example/r/downloads/default.mp4"
    );
    let r = c
        .get(format!("{}/start/reel-poster.jpg", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(
        r.headers()["location"],
        "https://videos.example/r/thumbnails/thumbnail.jpg"
    );
    let r = c
        .get(format!("{}/start/style.css", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn without_a_reel_elsewhere_the_landing_page_serves_none() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn_with_quickstart(clock, &dist).await;
    let c = unfollowed();
    for f in ["reel.mp4", "reel-poster.jpg"] {
        let r = c
            .get(format!("{}/start/{f}", ts.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404, "{f}");
    }
}

#[test]
fn a_reel_is_served_only_from_https_or_localhost() {
    use clax_http::hosted::Reel;
    assert!(Reel::new("https://videos.example/r.mp4", None).is_ok());
    assert!(
        Reel::new(
            "http://localhost:8788/reel.mp4",
            Some("http://127.0.0.1:1/p.jpg")
        )
        .is_ok()
    );
    let e = Reel::new(
        "https://videos.example/r.mp4",
        Some("http://videos.example/p.jpg"),
    )
    .unwrap_err();
    assert!(e.contains("http://videos.example/p.jpg"), "{e}");
    assert!(Reel::new("ftp://videos.example/r.mp4", None).is_err());
}

#[tokio::test]
async fn without_a_bundled_quickstart_a_signed_out_visitor_signs_in() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let c = unfollowed();
    let r = c.get(format!("{}/", ts.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], "/signin");
    let r = c.get(format!("{}/start/", ts.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_guest_gets_a_shared_pages_bootstrap_and_no_other() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = r.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let shared = ts.publish("Shared", &[("index.html", "<h1>s</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let private = ts.publish("Private", &[("index.html", "<h1>p</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .put(format!("{}/api/hosted/artifacts/{shared}/link", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .json(&json!({"access": "commenter"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let page = ts.get(&format!("/a/{shared}")).await;
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(
        html.contains("id=\"clax-boot\"") && html.contains("Shared"),
        "{html}"
    );
    assert_eq!(ts.get(&format!("/a/{private}")).await.status(), 404);
    // The owner's browser gets every page, with the owner as its viewer.
    let r = ts
        .client
        .get(format!("{}/a/{private}", ts.base))
        .header(COOKIE, &owner)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("\"display_name\":\"Ada\""));
}

#[tokio::test]
async fn a_person_who_may_not_read_comments_gets_a_bootstrap_without_threads() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let cookie_of = |r: &reqwest::Response| {
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = cookie_of(&r);
    let plan = ts.publish("Plan", &[("index.html", "<h1>p</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", "secret words")
        .text("version", "1");
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{plan}/threads", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    // A person invited with `rights`, signed in in a browser of their own.
    let person = async |name: &str, rights: serde_json::Value| {
        let r = ts
            .client
            .post(format!("{}/api/people/invites", ts.base))
            .header(ORIGIN, &origin)
            .header(COOKIE, &owner)
            .json(&json!({"name": name, "rights": rights}))
            .send()
            .await
            .unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let r = ts
            .client
            .post(format!("{}/api/hosted/invites/{code}/accept", ts.base))
            .header(ORIGIN, &origin)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        cookie_of(&r)
    };
    let artifacts = json!({"type": "clax-artifacts", "actions": ["read"], "locations": [plan]});
    let sam = person("Sam", json!([artifacts])).await;
    let mia = person(
        "Mia",
        json!([artifacts, {"type": "clax-comments", "actions": ["read"], "locations": [plan]}]),
    )
    .await;
    let page = async |cookie: &str| {
        let r = ts
            .client
            .get(format!("{}/a/{plan}", ts.base))
            .header(COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.text().await.unwrap()
    };
    assert!(page(&mia).await.contains("secret words"));
    let html = page(&sam).await;
    assert!(html.contains("id=\"clax-boot\""), "{html}");
    assert!(html.contains("\"threads\":[]"), "{html}");
    assert!(!html.contains("secret words"), "{html}");
}

#[tokio::test]
async fn the_bundled_documents_are_public_and_otherwise_absent() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock.clone(), &dist).await;
    for path in ["/terms", "/privacy"] {
        let r = ts.get(path).await;
        assert_ne!(r.status(), 200, "{path}");
    }
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    let ts = TestServer::spawn_hosted_with(clock, Some("s3cret"), move |s| {
        s.web = web;
        s.hosted = s.hosted.take().map(|h| {
            Arc::new(
                Arc::try_unwrap(h)
                    .expect("hosted is not shared yet")
                    .with_legal(Some(clax_core::config::Legal::Bundled)),
            )
        });
    })
    .await;
    for (path, body) in [("/terms", "<p>terms</p>"), ("/privacy", "<p>privacy</p>")] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(
            r.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{path}"
        );
        assert_eq!(r.text().await.unwrap(), body, "{path}");
    }
}

/// The hint every 404 of an artifact carries on a hosted instance, for the
/// instance at `origin`.
fn not_found_hint(origin: &str) -> String {
    format!(
        "This may be a private page. If it is yours, sign in at {origin}/signin. Agents: connect this Clax instance with the connect_instance tool or `clax remote request <url>`."
    )
}

#[tokio::test]
async fn a_missing_and_a_private_artifact_404_alike() {
    let dist = dist();
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = spawn(clock, &dist).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let cookie_of = |r: &reqwest::Response| {
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = cookie_of(&r);
    let r = ts
        .client
        .post(format!("{}/api/people/invites", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .json(&json!({"name": "Mia"}))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    let code = v["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap();
    let r = ts
        .client
        .post(format!("{}/api/hosted/invites/{code}/accept", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    let mia = cookie_of(&r);
    // Ada's page, private to her, and Mia's, private to Mia.
    let adas = ts.publish("Ada's", &[("index.html", "<h1>a</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Mia's agent publishes through her token.
    let r = ts
        .client
        .post(format!("{}/api/hosted/agent-tokens", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &mia)
        .json(&json!({"label": "laptop"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let v: serde_json::Value = r.json().await.unwrap();
    let token = v["token"].as_str().unwrap().to_string();
    let mcp = |body: serde_json::Value, session: Option<&str>| {
        let mut r = ts
            .client
            .post(format!("{}{}", ts.base, clax_testkit::mcp::ADAPTER))
            .header("accept", clax_testkit::mcp::ACCEPT)
            .bearer_auth(&token)
            .json(&body);
        if let Some(s) = session {
            r = r.header("mcp-session-id", s);
        }
        r.send()
    };
    let r = mcp(clax_testkit::mcp::initialize(), None).await.unwrap();
    assert_eq!(r.status(), 200);
    let session = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
    let r = mcp(
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "publish", "arguments": {"html": "<h1>m</h1>", "title": "Mia's"}}}),
        Some(&session),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let mias = ts
        .store
        .list_artifacts()
        .unwrap()
        .into_iter()
        .find(|a| a.title == "Mia's")
        .unwrap()
        .id;
    // A well-formed ID nothing was made at: Ada's page's ID, changed.
    let last = if adas.ends_with('a') { "b" } else { "a" };
    let missing = format!("{}{last}", &adas[..adas.len() - 1]);
    let fetch = async |cookie: Option<&str>, html: bool, id: &str, tail: &str| {
        let mut req = ts.client.get(format!("{}/a/{id}{tail}", ts.base));
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        if html {
            req = req.header("accept", "text/html");
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), 404, "{cookie:?} {id}{tail} html={html}");
        let ct = r.headers()["content-type"].to_str().unwrap().to_string();
        (ct, r.text().await.unwrap())
    };
    // Each caller, with a page of another person's out of their reach.
    let callers = [
        (None, &adas),
        (Some(owner.as_str()), &mias),
        (Some(mia.as_str()), &adas),
    ];
    for (cookie, private) in callers {
        for tail in ["", "/", "/v/1", "/get", "/v/1/index.html"] {
            for html in [false, true] {
                let a = fetch(cookie, html, &missing, tail).await;
                let b = fetch(cookie, html, private, tail).await;
                let who = format!("{cookie:?} {tail} html={html}");
                // The ID is the only difference between the two requests.
                assert_eq!(a, b, "{who}");
                if html || tail == "/get" {
                    assert!(a.0.starts_with("text/html"), "{who}: {a:?}");
                    assert!(a.1.contains("Sign in"), "{who}: {}", a.1);
                    assert!(a.1.contains(&format!("{origin}/signin")), "{who}");
                    assert!(a.1.contains("connect_instance"), "{who}");
                } else {
                    assert!(a.0.starts_with("application/json"), "{who}: {a:?}");
                    let v: serde_json::Value = serde_json::from_str(&a.1).unwrap();
                    assert_eq!(v["error"]["code"], "not_found", "{who}");
                    assert_eq!(v["error"]["message"], "not found", "{who}");
                    assert_eq!(v["error"]["hint"], not_found_hint(&origin), "{who}");
                    assert_eq!(v["error"]["sign_in"], format!("{origin}/signin"));
                    assert_eq!(v["error"]["instance"], format!("{origin}/.well-known/clax"));
                }
            }
        }
    }
    // The hint names the instance's origin, never the request's Host.
    let r = ts
        .client
        .get(format!("{}/a/{missing}", ts.base))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    // Another host never reaches the artifact routes: the host gate's 403.
    assert_eq!(r.status(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "forbidden_host");
    assert!(!v.to_string().contains("evil.example"), "{v}");
}

#[tokio::test]
async fn a_site_host_answers_its_own_404_not_the_instances_hint() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with_site_domain(clock, Some("s3cret"), "clax.test").await;
    let port = ts.addr.port();
    let origin = format!("http://localhost:{port}");
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "s3cret", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let owner = r.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let aid = ts.publish("Site", &[("index.html", "<h1>s</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let host = format!("{aid}.clax.test:{port}");
    let raw = unfollowed();
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/site-code", ts.base))
        .header(ORIGIN, &origin)
        .header(COOKIE, &owner)
        .json(&json!({"partitioned": true, "state": "st"}))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    let src = v["src"].as_str().unwrap();
    let r = raw
        .get(format!(
            "{}{}",
            ts.base,
            &src[src.find("/_clax/").unwrap()..]
        ))
        .header("host", &host)
        .header(COOKIE, "clax_site_state=st")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    let site_cookie = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let on_site = raw
        .get(format!("{}/a/x", ts.base))
        .header("host", &host)
        .header(COOKIE, &site_cookie)
        .send()
        .await
        .unwrap();
    let status = on_site.status();
    let text = on_site.text().await.unwrap();
    assert_eq!(status, 404, "{text}");
    assert!(!text.contains("private page"), "{text}");
    // The same path on the instance's own host gets the hint.
    let last = if aid.ends_with('a') { "b" } else { "a" };
    let r = ts.get(&format!("/a/{}{last}", &aid[..aid.len() - 1])).await;
    assert_eq!(r.status(), 404);
    assert!(r.text().await.unwrap().contains("private page"));
}
