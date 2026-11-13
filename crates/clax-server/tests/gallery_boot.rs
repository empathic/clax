//! The gallery pages with their bootstrap block (`clax_server::shell::gallery`),
//! from a local and a hosted test daemon serving a scratch web UI: each
//! field is what its route answers the same browser, and nothing more.
#![cfg(debug_assertions)]

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;
const MARK: &str = "<!--clax:boot-->";
const ENTRY: &str = "<!doctype html><head><title>Clax</title><!--clax:boot--></head><body><div id=\"app\"></div></body>";
const SECRET: &str = "the setup secret";

/// A scratch web UI: the gallery entry with its marker, and the artifact entry.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), ENTRY).unwrap();
    std::fs::write(
        dir.path().join("artifact.html"),
        include_str!("../../../web/shell/artifact.html"),
    )
    .unwrap();
    dir
}

fn web(dist: &tempfile::TempDir) -> Arc<dyn clax_core::embedded::FileTree> {
    Arc::new(clax_core::embedded::DiskTree::new(dist.path()))
}

/// The block in `html`, parsed; `None` when there is none. Panics on a
/// second block.
fn block(html: &str) -> Option<Value> {
    let start = html.find(OPEN)? + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    assert_eq!(html.matches(OPEN).count(), 1, "one block: {html}");
    let text = &html[start..end];
    for bad in ['<', '>', '&'] {
        assert!(!text.contains(bad), "escaped: {text}");
    }
    Some(serde_json::from_str(text).unwrap())
}

/// `GET path` with `cookie` (and the default client's headers) from the
/// local daemon's own host.
async fn local(ts: &TestServer, path: &str, cookie: &str) -> reqwest::Response {
    ts.client
        .get(format!("{}{path}", ts.base))
        .header("accept", "text/html")
        .header(COOKIE, cookie)
        .send()
        .await
        .unwrap()
}

/// `path`'s JSON body as the shell's own fetch gets it with `cookie`.
async fn fetched(ts: &TestServer, path: &str, cookie: &str) -> (u16, Value) {
    let r = ts
        .client
        .get(format!("{}{path}", ts.base))
        .header(COOKIE, cookie)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

/// Each field of `b` equals the route's answer to `cookie`'s fetch.
async fn assert_fields_are_the_routes(ts: &TestServer, b: &Value, cookie: &str, list: bool) {
    let mut fields = vec![
        ("viewer", "/api/viewers/me"),
        ("hosted", "/api/hosted/me"),
        ("feedback", "/_app/feedback/config"),
    ];
    if list {
        fields.push(("artifacts", "/api/artifacts"));
        fields.push(("attention", "/api/viewers/me/attention"));
    }
    for (name, path) in fields {
        let (status, body) = fetched(ts, path, cookie).await;
        assert_eq!(b["status"][name], json!(status), "{name}: {b}");
        let want = if status == 200 { body } else { Value::Null };
        assert_eq!(b[name], want, "{name}");
    }
    if !list {
        assert!(b.get("artifacts").is_none() && b.get("attention").is_none());
        assert!(b["status"].get("artifacts").is_none());
    }
}

#[tokio::test]
async fn a_local_viewer_gets_what_each_route_answers_it() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts.publish("Report", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.thread(&id, 1, "first").await;
    let cookie = format!("clax_viewer={}", ts.local_browser());
    let res = local(&ts, "/", &cookie).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "private, no-cache");
    assert_eq!(res.headers()["vary"], "Cookie");
    assert_eq!(res.headers()["x-frame-options"], "DENY");
    assert!(res.headers().get(SET_COOKIE).is_none(), "no viewer is made");
    let html = res.text().await.unwrap();
    assert!(!html.contains(MARK));
    let b = block(&html).expect("a block");
    assert_eq!(b["v"], 1);
    assert_eq!(b["kind"], "gallery");
    assert_fields_are_the_routes(&ts, &b, &cookie, true).await;
    assert_eq!(b["status"]["hosted"], 404, "a local daemon is not hosted");
    assert_eq!(b["hosted"], Value::Null);
    assert_eq!(b["artifacts"]["artifacts"][0]["id"], json!(id));
    assert!(
        !b["attention"]["artifacts"][&id]["open_in"]
            .as_array()
            .unwrap()
            .is_empty(),
        "its own thread: {b}"
    );
    assert_eq!(
        b["inbox"], false,
        "a viewer who is no person, not the owner"
    );
    assert!(!html.contains(&ts.token));
    assert!(!html.contains(&ts.local_browser()), "never the cookie");
}

#[tokio::test]
async fn the_pages_beside_the_gallery_get_the_headers_fields_alone() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let cookie = format!("clax_viewer={}", ts.local_browser());
    for path in [
        "/inbox",
        "/tags",
        "/queues",
        "/queues/q1",
        "/queues/q1/t1",
        "/settings",
        "/settings/you",
        "/people",
        "/admin",
        "/me",
        "/audit",
    ] {
        let res = local(&ts, path, &cookie).await;
        assert_eq!(res.status(), 200, "{path}");
        assert_eq!(
            res.headers()["cache-control"],
            "private, no-cache",
            "{path}"
        );
        let b = block(&res.text().await.unwrap()).expect(path);
        assert_fields_are_the_routes(&ts, &b, &cookie, false).await;
    }
}

#[tokio::test]
async fn another_viewer_gets_its_own_answers_and_none_of_the_firsts() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts.publish("Report", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The local browser's thread: its attention, not Bea's.
    ts.thread(&id, 1, "first").await;
    let bea = ts.viewer(Some("Bea")).await;
    let cookie = format!("clax_viewer={}", bea.cookie);
    let html = local(&ts, "/", &cookie).await.text().await.unwrap();
    let b = block(&html).unwrap();
    assert_fields_are_the_routes(&ts, &b, &cookie, true).await;
    assert_eq!(b["viewer"]["viewer"]["display_name"], "Bea");
    assert_eq!(b["viewer"]["viewer"]["public_id"], json!(bea.public_id));
    let opened = &b["attention"]["artifacts"][&id]["open_in"];
    assert!(
        opened.is_null() || opened.as_array().unwrap().is_empty(),
        "another viewer's open thread: {b}"
    );
    assert_eq!(b["feedback"]["enabled"], false, "a viewer files no reports");
    assert!(b["feedback"].get("user").is_none());
    assert_eq!(b["inbox"], false);
}

#[tokio::test]
async fn a_connected_persons_browser_has_an_inbox_and_a_revoked_ones_has_none() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let p = ts.person("Cy").await;
    let cookie = format!("clax_viewer={}", p.cookie);
    let b = block(&local(&ts, "/inbox", &cookie).await.text().await.unwrap()).unwrap();
    assert_eq!(b["viewer"]["viewer"]["person"], true, "{b}");
    assert_eq!(b["inbox"], true);
    let conn = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    conn.execute(
        "UPDATE viewers SET person_revoked_at = '2026-01-01T00:00:00Z' WHERE public_id = ?1",
        [&p.public_id],
    )
    .unwrap();
    let res = local(&ts, "/inbox", &cookie).await;
    assert_eq!(res.status(), 200);
    let b = block(&res.text().await.unwrap()).unwrap();
    assert_eq!(b["inbox"], false, "{b}");
}

#[tokio::test]
async fn a_local_owners_navigation_gets_no_block_and_a_proven_load_never_the_token() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    ts.publish("Secret plans", &[("index.html", "<p>x</p>")])
        .await;
    // A navigation: the owner cookie without its proof.
    let res = reqwest::Client::new()
        .get(format!("{}/", ts.base))
        .header("accept", "text/html")
        .header(COOKIE, ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "private, no-cache");
    let html = res.text().await.unwrap();
    assert!(block(&html).is_none(), "no block: {html}");
    assert!(!html.contains(MARK) && !html.contains("Secret plans"));
    // A load that names the proof (the in-process app's, or a test's).
    let html = local(&ts, "/", &ts.owner_cookie())
        .await
        .text()
        .await
        .unwrap();
    let b = block(&html).unwrap();
    assert_eq!(b["viewer"]["viewer"]["owner"], true, "{b}");
    assert_eq!(b["inbox"], true, "the owner's browser");
    assert_eq!(b["artifacts"]["artifacts"][0]["title"], "Secret plans");
    assert!(!html.contains(&ts.token), "the token never enters the page");
    assert!(b.get("token").is_none());
}

#[tokio::test]
async fn a_host_the_api_refuses_or_another_sites_load_gets_no_block() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    ts.publish("Secret plans", &[("index.html", "<p>x</p>")])
        .await;
    let v = ts.viewer(Some("Ada")).await;
    let cookie = format!("clax_viewer={}", v.cookie);
    let port = ts.addr.port();
    for host in [
        format!("evil.example:{port}"),
        format!("localhost.evil.example:{port}"),
    ] {
        let res = ts
            .client
            .get(format!("{}/", ts.base))
            .header(HOST, &host)
            .header(COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{host}");
        let html = res.text().await.unwrap();
        assert!(block(&html).is_none(), "{host}");
        assert!(
            !html.contains("Ada") && !html.contains("Secret plans"),
            "{host}"
        );
    }
    for site in ["cross-site", "same-site"] {
        let res = ts
            .client
            .get(format!("{}/", ts.base))
            .header(COOKIE, &cookie)
            .header("sec-fetch-site", site)
            .header("sec-fetch-mode", "navigate")
            .send()
            .await
            .unwrap();
        let html = res.text().await.unwrap();
        assert!(block(&html).is_none(), "{site}");
        assert!(!html.contains("Ada"), "{site}");
    }
    // A load the user started (typed, bookmarked) or one of the shell's links.
    for site in ["none", "same-origin"] {
        let res = ts
            .client
            .get(format!("{}/", ts.base))
            .header(COOKIE, &cookie)
            .header("sec-fetch-site", site)
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .send()
            .await
            .unwrap();
        let b = block(&res.text().await.unwrap()).expect(site);
        assert_eq!(b["viewer"]["viewer"]["display_name"], "Ada", "{site}");
        assert_eq!(b["status"]["viewer"], 200, "{site}");
    }
}

#[tokio::test]
async fn hostile_titles_stay_data_and_the_etag_covers_the_block() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let title = "</script><!--clax:boot--><script>alert(1)</script>&amp;\u{2028}";
    ts.publish(title, &[("index.html", "<p>x</p>")]).await;
    let cookie = format!("clax_viewer={}", ts.local_browser());
    let res = local(&ts, "/", &cookie).await;
    let tag = res.headers()["etag"].to_str().unwrap().to_string();
    let html = res.text().await.unwrap();
    assert_eq!(html.matches("<script").count(), 1, "{html}");
    assert!(!html.contains(MARK));
    let b = block(&html).unwrap();
    assert_eq!(b["artifacts"]["artifacts"][0]["title"], title);
    let again = ts
        .client
        .get(format!("{}/", ts.base))
        .header(COOKIE, &cookie)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 304);
    assert_eq!(again.headers()["cache-control"], "private, no-cache");
    assert_eq!(again.headers()["vary"], "Cookie");
    // Another viewer's page is other bytes: never a 304 for these.
    let other = ts.viewer(None).await;
    let res = ts
        .client
        .get(format!("{}/", ts.base))
        .header(COOKIE, format!("clax_viewer={}", other.cookie))
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A change to what a route answers is a change to the page.
    ts.publish("Another", &[("index.html", "<p>y</p>")]).await;
    let after = ts
        .client
        .get(format!("{}/", ts.base))
        .header(COOKIE, &cookie)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 200);
}

#[tokio::test]
async fn a_slow_route_leaves_its_field_to_the_shell() {
    let dist = dist();
    let web = web(&dist);
    let ts = TestServer::spawn_with(move |s| {
        s.web = web;
        s.request_timeout = std::time::Duration::from_millis(200);
    })
    .await;
    let cookie = format!("clax_viewer={}", ts.local_browser());
    // Every route the page asks holds its answer past the API's timeout
    // (the test daemon's delay header goes with the page's own headers).
    let res = ts
        .client
        .get(format!("{}/", ts.base))
        .header(COOKIE, &cookie)
        .header("x-clax-test-delay-ms", "60000")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let b = block(&res.text().await.unwrap()).unwrap();
    for f in ["viewer", "feedback", "artifacts", "attention"] {
        assert_eq!(b[f], Value::Null, "{f}: {b}");
        assert!(b["status"].get(f).is_none(), "{f}: {b}");
    }
    assert_eq!(b["inbox"], Value::Null, "not known");
}

/// A hosted daemon on a stopped clock serving `dist`.
async fn hosted(dist: &tempfile::TempDir) -> TestServer {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let web = web(dist);
    TestServer::spawn_hosted_with(clock, Some(SECRET), move |s| s.web = web).await
}

/// A request to the hosted daemon from the browser holding `session`,
/// at the public host.
fn at(
    ts: &TestServer,
    method: reqwest::Method,
    path: &str,
    session: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut r = ts
        .client
        .request(method.clone(), format!("{}{path}", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()));
    if method != reqwest::Method::GET {
        r = r.header(ORIGIN, format!("http://localhost:{}", ts.addr.port()));
    }
    if let Some(c) = session {
        r = r.header(COOKIE, format!("clax_session={c}"));
    }
    r
}

/// The session cookie a sign-in route's answer sets.
async fn signed_in(res: reqwest::Response) -> String {
    assert_eq!(res.status(), 200);
    let c = res.headers()[SET_COOKIE].to_str().unwrap().to_string();
    c.strip_prefix("clax_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// The owner's session, through setup, and a member's (their own space
/// alone), through an invite.
async fn owner_and_member(ts: &TestServer) -> (String, String) {
    let owner = signed_in(
        at(ts, reqwest::Method::POST, "/api/hosted/setup", None)
            .json(&json!({"secret": SECRET, "name": "Ada"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let invite: Value = at(
        ts,
        reqwest::Method::POST,
        "/api/people/invites",
        Some(&owner),
    )
    .json(&json!({"name": "Mia"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let code = invite["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let member = signed_in(
        at(
            ts,
            reqwest::Method::POST,
            &format!("/api/hosted/invites/{code}/accept"),
            None,
        )
        .json(&json!({}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    (owner, member)
}

/// `path`'s block for the browser holding `session`, and the answers its
/// routes give that browser's own fetches, field by field.
async fn hosted_block(ts: &TestServer, path: &str, session: &str) -> (Value, String) {
    let res = at(ts, reqwest::Method::GET, path, Some(session))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{path}");
    assert_eq!(res.headers()["cache-control"], "private, no-cache");
    assert_eq!(res.headers()["vary"], "Cookie");
    let html = res.text().await.unwrap();
    (block(&html).expect("a block"), html)
}

async fn hosted_fields(ts: &TestServer, b: &Value, session: &str) {
    for (name, path) in [
        ("viewer", "/api/viewers/me"),
        ("hosted", "/api/hosted/me"),
        ("feedback", "/_app/feedback/config"),
        ("artifacts", "/api/artifacts"),
        ("attention", "/api/viewers/me/attention"),
    ] {
        let r = at(ts, reqwest::Method::GET, path, Some(session))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let body: Value = r.json().await.unwrap_or(Value::Null);
        assert_eq!(b["status"][name], json!(status), "{name}: {b}");
        assert_eq!(
            b[name],
            if status == 200 { body } else { Value::Null },
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_hosted_member_gets_their_reach_and_the_owner_theirs() {
    let dist = dist();
    let ts = hosted(&dist).await;
    let (owner, member) = owner_and_member(&ts).await;
    let id = ts
        .publish("The owner's plans", &[("index.html", "<p>x</p>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (b, html) = hosted_block(&ts, "/", &owner).await;
    hosted_fields(&ts, &b, &owner).await;
    assert_eq!(b["hosted"]["signed_in"], true);
    assert_eq!(b["status"]["hosted"], 200);
    assert_eq!(b["inbox"], true, "the owner's browser");
    assert_eq!(b["artifacts"]["artifacts"][0]["id"], json!(id));
    assert!(!html.contains(&ts.token) && !html.contains(&owner));
    assert!(!html.contains("cxo_"), "no shell token");

    let (b, html) = hosted_block(&ts, "/", &member).await;
    hosted_fields(&ts, &b, &member).await;
    assert_eq!(b["hosted"]["signed_in"], true, "{b}");
    assert_eq!(b["viewer"]["viewer"]["display_name"], "Mia", "{b}");
    assert!(
        b["artifacts"]["artifacts"].as_array().unwrap().is_empty(),
        "the owner's artifact is out of a member's reach: {b}"
    );
    assert!(!html.contains("The owner's plans") && !html.contains(&id));
    assert!(!html.contains(&member), "never the session cookie");
    assert_eq!(b["inbox"], true, "a connected person's browser");

    let (b, _) = hosted_block(&ts, "/settings/you", &member).await;
    assert!(b.get("artifacts").is_none());
}

#[tokio::test]
async fn a_hosted_visitor_with_no_session_is_sent_to_sign_in_with_no_block() {
    let dist = dist();
    let ts = hosted(&dist).await;
    owner_and_member(&ts).await;
    let unfollowed = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = unfollowed
        .get(format!("{}/", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert!(res.status().is_redirection(), "{}", res.status());
    assert!(!res.text().await.unwrap().contains(OPEN));
    // The sign-in pages show the bare entry.
    let res = at(&ts, reqwest::Method::GET, "/signin", None)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    assert!(block(&html).is_none() && !html.contains(MARK));
}
