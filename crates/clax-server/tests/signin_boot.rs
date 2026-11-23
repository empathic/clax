//! A hosted daemon's sign-in pages and landing page with their bootstrap
//! block (`clax_server::shell::signin`), from a hosted test daemon serving
//! a scratch web UI: each field is what its route answers the same
//! browser, a field the page would not read is left out, and `/signin?next=`
//! sends a signed-in browser on from the server.
#![cfg(debug_assertions)]

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;
const ENTRY: &str = "<!doctype html><head><title>Clax</title><!--clax:boot--></head><body><div id=\"app\"></div></body>";
const SECRET: &str = "the setup secret";

/// A hosted daemon on a stopped clock serving a scratch web UI (the
/// gallery entry with its marker) and a quickstart release.
async fn hosted() -> (TestServer, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), ENTRY).unwrap();
    let web: Arc<dyn clax_core::embedded::FileTree> =
        Arc::new(clax_core::embedded::DiskTree::new(dir.path()));
    let release =
        clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("landing"))
            .map(Arc::new);
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with(clock, Some(SECRET), move |s| {
        s.web = web;
        s.quickstart = release;
    })
    .await;
    (ts, dir)
}

/// A request to the daemon at its public host, from the browser holding `session`.
fn at(
    ts: &TestServer,
    method: reqwest::Method,
    path: &str,
    session: Option<&str>,
) -> reqwest::RequestBuilder {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut r = client
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

/// The block in `html`, parsed; `None` when there is none.
fn block(html: &str) -> Option<Value> {
    let start = html.find(OPEN)? + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    assert_eq!(html.matches(OPEN).count(), 1, "one block: {html}");
    Some(serde_json::from_str(&html[start..end]).unwrap())
}

/// `path`'s page for `session`: its block, checked for the headers a page of one browser's carries.
async fn page(ts: &TestServer, path: &str, session: Option<&str>) -> Value {
    let res = at(ts, reqwest::Method::GET, path, session)
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{path}");
    assert_eq!(res.headers()["cache-control"], "no-store", "{path}");
    assert_eq!(res.headers()["vary"], "Cookie", "{path}");
    block(&res.text().await.unwrap()).expect("a block")
}

/// `path`'s status and JSON body as the page's own fetch gets them.
async fn fetched(ts: &TestServer, path: &str, session: Option<&str>) -> (u16, Value) {
    let r = at(ts, reqwest::Method::GET, path, session)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

/// Each field `fields` names equals its route's answer to the same browser;
/// every other field is null with no status.
async fn fields_are_the_routes(
    ts: &TestServer,
    b: &Value,
    session: Option<&str>,
    fields: &[(&str, &str)],
) {
    assert_eq!(b["v"], 1);
    assert_eq!(b["kind"], "signin");
    for name in ["hosted", "public", "invite", "link", "device"] {
        match fields.iter().find(|(n, _)| *n == name) {
            Some((_, path)) => {
                let (status, body) = fetched(ts, path, session).await;
                assert_eq!(b["status"][name], status, "{name}: {b}");
                assert_eq!(
                    b[name],
                    if status == 200 { body } else { Value::Null },
                    "{name}"
                );
            }
            None => {
                assert!(b["status"].get(name).is_none(), "{name} left out: {b}");
                assert_eq!(b[name], Value::Null, "{name}");
            }
        }
    }
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

async fn owner(ts: &TestServer) -> String {
    signed_in(
        at(ts, reqwest::Method::POST, "/api/hosted/setup", None)
            .json(&json!({"secret": SECRET, "name": "Ada"}))
            .send()
            .await
            .unwrap(),
    )
    .await
}

/// The code at the end of `url`.
fn code_of(url: &Value) -> String {
    url.as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn each_sign_in_page_carries_what_its_routes_answer_its_browser() {
    let (ts, _dir) = hosted().await;
    // Setup is open: the setup page, signed out.
    let b = page(&ts, "/setup", None).await;
    fields_are_the_routes(&ts, &b, None, &[("hosted", "/api/hosted/me")]).await;
    assert_eq!(b["hosted"]["setup_open"], true);

    let ada = owner(&ts).await;
    let b = page(&ts, "/signin", None).await;
    fields_are_the_routes(
        &ts,
        &b,
        None,
        &[
            ("hosted", "/api/hosted/me"),
            ("public", "/api/hosted/public"),
        ],
    )
    .await;
    assert_eq!(b["hosted"]["signed_in"], false);
    let b = page(&ts, "/signin", Some(&ada)).await;
    fields_are_the_routes(
        &ts,
        &b,
        Some(&ada),
        &[
            ("hosted", "/api/hosted/me"),
            ("public", "/api/hosted/public"),
        ],
    )
    .await;
    assert_eq!(b["hosted"]["person"]["display_name"], "Ada");

    let link: Value = at(
        &ts,
        reqwest::Method::POST,
        "/api/hosted/signin-links",
        Some(&ada),
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let code = code_of(&link["url"]);
    let path = format!("/api/hosted/signin-links/{code}");
    let b = page(&ts, &format!("/signin/{code}"), None).await;
    fields_are_the_routes(
        &ts,
        &b,
        None,
        &[("hosted", "/api/hosted/me"), ("link", &path)],
    )
    .await;
    assert_eq!(b["link"]["name"], "Ada");

    let invite: Value = at(
        &ts,
        reqwest::Method::POST,
        "/api/people/invites",
        Some(&ada),
    )
    .json(&json!({"name": "Mia"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let code = code_of(&invite["invite"]["url"]);
    let path = format!("/api/hosted/invites/{code}");
    let b = page(&ts, &format!("/join/{code}"), None).await;
    fields_are_the_routes(
        &ts,
        &b,
        None,
        &[("hosted", "/api/hosted/me"), ("invite", &path)],
    )
    .await;
    assert_eq!(b["invite"]["name"], "Mia");

    let device: Value = at(&ts, reqwest::Method::POST, "/api/hosted/device/code", None)
        .json(&json!({"client_name": "Pixel"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user = device["user_code"].as_str().unwrap().to_string();
    let path = format!("/api/hosted/device/{user}");
    let b = page(&ts, &format!("/device?code={user}"), Some(&ada)).await;
    fields_are_the_routes(
        &ts,
        &b,
        Some(&ada),
        &[("hosted", "/api/hosted/me"), ("device", &path)],
    )
    .await;
    assert_eq!(b["device"]["client_name"], "Pixel");
    // Signed out, the page looks no code up: nor does its block.
    let b = page(&ts, &format!("/device?code={user}"), None).await;
    fields_are_the_routes(&ts, &b, None, &[("hosted", "/api/hosted/me")]).await;

    let b = page(&ts, "/start/", Some(&ada)).await;
    fields_are_the_routes(&ts, &b, Some(&ada), &[("hosted", "/api/hosted/me")]).await;
    assert_eq!(b["hosted"]["signed_in"], true);
}

#[tokio::test]
async fn a_spent_or_unknown_code_is_the_routes_refusal_and_an_odd_one_is_left_to_the_page() {
    let (ts, _dir) = hosted().await;
    let _ada = owner(&ts).await;
    let b = page(&ts, "/join/nope", None).await;
    fields_are_the_routes(
        &ts,
        &b,
        None,
        &[
            ("hosted", "/api/hosted/me"),
            ("invite", "/api/hosted/invites/nope"),
        ],
    )
    .await;
    assert_eq!(b["status"]["invite"], 404);
    let b = page(&ts, "/signin/nope", None).await;
    assert_eq!(b["status"]["link"], 404);
    assert_eq!(b["link"], Value::Null);
    // A code the page would encode before asking is the page's to ask.
    let b = page(&ts, "/join/a%20b", None).await;
    fields_are_the_routes(&ts, &b, None, &[("hosted", "/api/hosted/me")]).await;
}

#[tokio::test]
async fn a_signed_in_browser_sent_to_sign_in_with_next_is_redirected_there() {
    let (ts, _dir) = hosted().await;
    let ada = owner(&ts).await;
    let res = at(
        &ts,
        reqwest::Method::GET,
        "/signin?next=%2Ftags",
        Some(&ada),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 303);
    assert_eq!(res.headers()[LOCATION], "/tags");
    assert_eq!(res.headers()["cache-control"], "no-store");
    // Signed out, or a next the page would not follow, and it is the page.
    for (path, session) in [
        ("/signin?next=%2Ftags", None),
        (
            "/signin?next=https%3A%2F%2Fevil.example%2F",
            Some(ada.as_str()),
        ),
        ("/signin?next=%2F%2Fevil.example", Some(ada.as_str())),
        ("/signin", Some(ada.as_str())),
    ] {
        let res = at(&ts, reqwest::Method::GET, path, session)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{path} {session:?}");
    }
}

#[tokio::test]
async fn an_invite_page_loaded_again_takes_no_more_sign_in_attempts_and_guessing_codes_is_still_held()
 {
    let (ts, _dir) = hosted().await;
    let ada = owner(&ts).await;
    let invite: Value = at(
        &ts,
        reqwest::Method::POST,
        "/api/people/invites",
        Some(&ada),
    )
    .json(&json!({"name": "Mia"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let code = code_of(&invite["invite"]["url"]);
    // Reloaded, or fetched by a link's preview: one attempt in all.
    for n in 0..15 {
        let b = page(&ts, &format!("/join/{code}"), None).await;
        assert_eq!(b["status"]["invite"], 200, "load {n}: {b}");
        assert_eq!(b["invite"]["name"], "Mia");
    }
    // Each other code still costs one (10 per client): nine more answer,
    // the tenth is refused, left to the page with no status.
    for n in 0..9 {
        let b = page(&ts, &format!("/join/guess{n}"), None).await;
        assert_eq!(b["status"]["invite"], 404, "guess {n}: {b}");
    }
    let b = page(&ts, "/join/guess9", None).await;
    assert!(b["status"].get("invite").is_none(), "{b}");
    let (status, _) = fetched(&ts, "/api/hosted/invites/guess10", None).await;
    assert_eq!(status, 429);
    // The invite already looked up still answers its page.
    let b = page(&ts, &format!("/join/{code}"), None).await;
    assert_eq!(b["status"]["invite"], 200, "{b}");
}
