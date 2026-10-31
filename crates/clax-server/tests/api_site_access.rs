//! Who reads an artifact's site (`clax_http::site_access`): a site session
//! made by a one-use code from the shell (`POST
//! /api/artifacts/<id>/site-code`) or the public host's `/site-signin`,
//! checked against the holder's right on every request.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use futures::StreamExt;
use reqwest::header::{ACCEPT, COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";
/// The handshake state this file's browser holds ([`Site::redeem`]).
const STATE: &str = "this-browsers-state";

/// A hosted daemon on a stopped clock, whose sites are `<id>.localhost`.
struct Site {
    ts: TestServer,
    clock: Arc<ManualClock>,
    origin: String,
    /// Sends no redirect on.
    raw: reqwest::Client,
}

/// A browser: its clax.dev session cookie, if any.
#[derive(Clone, Default)]
struct Browser {
    cookie: Option<String>,
}

impl Site {
    async fn new() -> Site {
        let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
        let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        let raw = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Site {
            ts,
            clock,
            origin,
            raw,
        }
    }

    fn public_host(&self) -> String {
        format!("localhost:{}", self.ts.addr.port())
    }

    /// A request on the public host from `b`, as the shell makes it.
    fn public(&self, b: &Browser, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .raw
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header(HOST, self.public_host());
        if method != reqwest::Method::GET {
            r = r.header(ORIGIN, &self.origin);
        }
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        r
    }

    async fn send(
        &self,
        b: &Browser,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        let r = self
            .public(b, method, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    /// Signs `b` in through `path`, which answers with the session cookie.
    async fn sign_in(&self, b: &mut Browser, path: &str, body: Value) {
        let r = self
            .public(b, reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{path}");
        let c = r.headers()[SET_COOKIE].to_str().unwrap();
        b.cookie = Some(
            c.strip_prefix("clax_session=")
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string(),
        );
    }

    async fn owner(&self) -> Browser {
        let mut b = Browser::default();
        self.sign_in(
            &mut b,
            "/api/hosted/setup",
            json!({"secret": SECRET, "name": "Ada"}),
        )
        .await;
        b
    }

    /// A person with their own space alone, invited by `owner`; and their
    /// public ID.
    async fn member(&self, owner: &Browser, name: &str) -> (Browser, String) {
        let (s, v) = self
            .send(
                owner,
                reqwest::Method::POST,
                "/api/people/invites",
                json!({"name": name}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let mut b = Browser::default();
        self.sign_in(
            &mut b,
            &format!("/api/hosted/invites/{code}/accept"),
            json!({}),
        )
        .await;
        let (_, me) = self
            .send(&b, reqwest::Method::GET, "/api/hosted/me", Value::Null)
            .await;
        let pid = me["person"]["public_id"].as_str().unwrap().to_string();
        (b, pid)
    }

    async fn artifact(&self, title: &str) -> String {
        self.ts
            .publish(
                title,
                &[
                    ("index.html", &format!("<h1>{title}</h1>")),
                    ("a.css", "p{}"),
                ],
            )
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn site_host(&self, aid: &str) -> String {
        self.ts.site_host(aid)
    }

    /// `b`'s site code for `aid`, bound to [`STATE`] unless `body` names
    /// a state: the status, and the `src` it answers.
    async fn code(&self, b: &Browser, aid: &str, mut body: Value) -> (u16, String) {
        if body.get("state").is_none() {
            body["state"] = json!(STATE);
        }
        let (s, v) = self
            .send(
                b,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/site-code"),
                body,
            )
            .await;
        (s, v["src"].as_str().unwrap_or_default().to_string())
    }

    /// Follows `src` on `aid`'s site host with [`STATE`]'s cookie: the
    /// response, unfollowed.
    async fn redeem(&self, aid: &str, src: &str) -> reqwest::Response {
        self.redeem_as(aid, src, Some(STATE)).await
    }

    /// [`Site::redeem`] by a browser holding `state` (none: no state cookie).
    async fn redeem_as(&self, aid: &str, src: &str, state: Option<&str>) -> reqwest::Response {
        let path = &src[src.find("/_clax/session").expect("a session address")..];
        let mut r = self
            .raw
            .get(format!("{}{path}", self.ts.base))
            .header(HOST, self.site_host(aid));
        if let Some(st) = state {
            r = r.header(COOKIE, format!("clax_site_state={st}"));
        }
        r.send().await.unwrap()
    }

    /// The `Cookie` header of the site session `src` makes.
    async fn session(&self, aid: &str, src: &str) -> String {
        let r = self.redeem(aid, src).await;
        assert_eq!(r.status(), 302);
        let c = r.headers()[SET_COOKIE].to_str().unwrap();
        c.split(';').next().unwrap().to_string()
    }

    /// A subresource `GET` on `aid`'s site host with `cookie`.
    async fn on_site(&self, aid: &str, path: &str, cookie: Option<&str>) -> reqwest::Response {
        let mut r = self
            .raw
            .get(format!("{}{path}", self.ts.base))
            .header(HOST, self.site_host(aid));
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        r.send().await.unwrap()
    }
}

/// Whether `r` sets a site session's cookie.
fn sets_session(r: &reqwest::Response) -> bool {
    r.headers()
        .get_all(SET_COOKIE)
        .iter()
        .any(|c| c.to_str().unwrap().starts_with("clax_site="))
}

fn code_of(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_private_site_without_a_session_redirects_to_site_signin() {
    let site = Site::new().await;
    let aid = site.artifact("Plan").await;
    let r = site
        .raw
        .get(format!("{}/", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    let host = site.site_host(&aid).replace(':', "%3A");
    let to = r.headers()[LOCATION].to_str().unwrap().to_string();
    let prefix = format!("{}/site-signin?host={host}&next=%2F&state=", site.origin);
    assert!(to.starts_with(&prefix), "{to}");
    // The handshake's state, named in the address and held by the browser.
    let state = &to[prefix.len()..];
    let set = r.headers()[SET_COOKIE].to_str().unwrap();
    assert!(
        set.starts_with(&format!(
            "clax_site_state={state}; Path=/; Max-Age=600; HttpOnly"
        )),
        "{set}"
    );
    assert!(
        set.contains("SameSite=Lax") && !set.contains("Partitioned"),
        "{set}"
    );
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert_eq!(r.headers()["cache-control"], "private, no-store");
    let r = site
        .raw
        .get(format!("{}/a.css?x=1", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-dest", "document")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    assert!(
        r.headers()[LOCATION]
            .to_str()
            .unwrap()
            .contains("&next=%2Fa.css%3Fx%3D1&state="),
        "{:?}",
        r.headers()[LOCATION]
    );
}

#[tokio::test]
async fn a_private_sites_subresource_without_a_session_is_401() {
    let site = Site::new().await;
    let aid = site.artifact("Plan").await;
    for p in [
        "/",
        "/a.css",
        "/_clax/v/1/",
        "/_clax/blob/01J9Z3K4M5N6P7Q8R9S0T1V2W3",
        "/_blob/x",
    ] {
        let r = site.on_site(&aid, p, None).await;
        assert_eq!(r.status(), 401, "{p}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(code_of(&v), "site_session_required", "{p}");
    }
    // Health and the bridge's files are no one's.
    assert_eq!(
        site.on_site(&aid, "/_clax/healthz", None).await.status(),
        200
    );
    assert_eq!(site.on_site(&aid, "/_clax/nope", None).await.status(), 404);
}

#[tokio::test]
async fn the_shell_code_signs_the_frame_in_and_the_cookie_is_partitioned() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (s, src) = site
        .code(
            &owner,
            &aid,
            json!({"next": "/_clax/v/1/", "partitioned": true}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(
        src.starts_with(&format!(
            "http://{}/_clax/session?code=",
            site.site_host(&aid)
        )),
        "{src}"
    );
    assert!(src.ends_with("&next=%2F_clax%2Fv%2F1%2F"), "{src}");
    let r = site.redeem(&aid, &src).await;
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers()[LOCATION], "/_clax/v/1/");
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    let set = r.headers()[SET_COOKIE].to_str().unwrap().to_string();
    for attr in [
        "clax_site=",
        "HttpOnly",
        "SameSite=None",
        "Partitioned",
        "Secure",
        "Path=/",
    ] {
        assert!(set.contains(attr), "{attr}: {set}");
    }
    assert!(!set.contains("Domain"), "{set}");
    let cookie = set.split(';').next().unwrap();
    let r = site.on_site(&aid, "/", Some(cookie)).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("<h1>Plan</h1>"));
    // A top-level code's cookie is `SameSite=Lax`, unpartitioned.
    let (_, src) = site.code(&owner, &aid, json!({})).await;
    let r = site.redeem(&aid, &src).await;
    let set = r.headers()[SET_COOKIE].to_str().unwrap();
    assert!(
        set.contains("SameSite=Lax") && !set.contains("Partitioned"),
        "{set}"
    );
    // An agent's credential mints none; neither does no one.
    let (s, _) = site.code(&Browser::default(), &aid, json!({})).await;
    assert_eq!(s, 404, "the artifact is not shared by link");
}

#[tokio::test]
async fn a_code_is_single_use_and_expires_after_sixty_seconds() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (_, src) = site.code(&owner, &aid, json!({})).await;
    site.session(&aid, &src).await;
    let again = site.redeem(&aid, &src).await;
    assert_eq!(again.status(), 302);
    assert!(!sets_session(&again));
    assert!(
        again.headers()[LOCATION]
            .to_str()
            .unwrap()
            .starts_with(&format!("{}/site-signin?", site.origin))
    );
    let (_, late) = site.code(&owner, &aid, json!({})).await;
    site.clock.advance(61);
    let r = site.redeem(&aid, &late).await;
    assert!(!sets_session(&r));
    assert!(
        r.headers()[LOCATION]
            .to_str()
            .unwrap()
            .contains("/site-signin?")
    );
}

#[tokio::test]
async fn revoking_a_share_ends_the_site_session_on_the_next_request() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let (s, _) = site.code(&sam, &aid, json!({})).await;
    assert_eq!(s, 404, "not Sam's to read yet");
    let shares = format!("/api/hosted/artifacts/{aid}/shares");
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &shares,
            json!({"person": sam_pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let share = v["share"]["id"].as_str().unwrap().to_string();
    let (s, src) = site.code(&sam, &aid, json!({})).await;
    assert_eq!(s, 200);
    let cookie = site.session(&aid, &src).await;
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("{shares}/{share}"),
            Value::Null,
        )
        .await;
    assert!(s == 200 || s == 204, "{s}");
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 401);
    assert_eq!(
        site.on_site(&aid, "/a.css", Some(&cookie)).await.status(),
        401
    );
}

#[tokio::test]
async fn signing_out_ends_the_browsers_site_sessions() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (_, src) = site.code(&owner, &aid, json!({"partitioned": true})).await;
    let cookie = site.session(&aid, &src).await;
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::POST,
            "/api/hosted/signout",
            json!({}),
        )
        .await;
    assert!(s == 200 || s == 204, "{s}");
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 401);
}

#[tokio::test]
async fn a_link_viewer_reads_without_signing_in_until_the_link_is_turned_off() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let link = format!("/api/hosted/artifacts/{aid}/link");
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &link,
            json!({"access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let anon = Browser::default();
    let (s, src) = site.code(&anon, &aid, json!({"partitioned": true})).await;
    assert_eq!(s, 200);
    let cookie = site.session(&aid, &src).await;
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    // At the top level, `/site-signin` makes one with no sign-in too.
    let host = site.site_host(&aid);
    let r = site
        .public(
            &anon,
            reqwest::Method::GET,
            &format!("/site-signin?host={host}&next=%2Fa.css&state={STATE}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    let to = r.headers()[LOCATION].to_str().unwrap().to_string();
    let top = site.session(&aid, &to).await;
    assert_eq!(site.on_site(&aid, "/a.css", Some(&top)).await.status(), 200);
    site.send(&owner, reqwest::Method::PUT, &link, json!({"access": null}))
        .await;
    for c in [&cookie, &top] {
        assert_eq!(site.on_site(&aid, "/", Some(c)).await.status(), 401);
    }
}

#[tokio::test]
async fn a_site_session_is_for_one_artifact() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (a, b) = (site.artifact("A").await, site.artifact("B").await);
    let (_, src) = site.code(&owner, &a, json!({})).await;
    // A's code redeems on A's host alone.
    let r = site.redeem(&b, &src).await;
    assert!(!sets_session(&r));
    let (_, src) = site.code(&owner, &a, json!({})).await;
    let cookie = site.session(&a, &src).await;
    assert_eq!(site.on_site(&a, "/", Some(&cookie)).await.status(), 200);
    assert_eq!(site.on_site(&b, "/", Some(&cookie)).await.status(), 401);
}

#[tokio::test]
async fn a_session_expires_after_an_hour_unused_and_renews_by_use() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (_, src) = site.code(&owner, &aid, json!({})).await;
    let cookie = site.session(&aid, &src).await;
    for _ in 0..3 {
        site.clock.advance(3599);
        assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    }
    site.clock.advance(3600);
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 401);
}

#[tokio::test]
async fn next_never_leaves_the_site_host() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (_, src) = site
        .code(&owner, &aid, json!({"next": "https://evil.example/"}))
        .await;
    assert!(src.ends_with("&next=%2F"), "{src}");
    for evil in [
        "https%3A%2F%2Fevil.example%2F",
        "%2F%2Fevil.example",
        "%2F%5Cevil.example",
    ] {
        let (_, src) = site.code(&owner, &aid, json!({})).await;
        let src = format!("{}&next={evil}", src.split("&next=").next().unwrap());
        let r = site.redeem(&aid, &src).await;
        assert_eq!(r.headers()[LOCATION], "/", "{evil}");
    }
}

#[tokio::test]
async fn site_signin_signs_in_checks_the_right_and_sends_the_browser_back() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let host = site.site_host(&aid);
    let at = |b: &Browser, q: String| {
        let r = site.public(
            b,
            reqwest::Method::GET,
            &format!("/site-signin?{q}&state={STATE}"),
        );
        async move { r.send().await.unwrap() }
    };
    for q in [
        "host=nope.localhost&next=%2F".to_string(),
        format!("host={aid}.evil.example&next=%2F"),
        String::from("next=%2F"),
    ] {
        assert_eq!(at(&owner, q.clone()).await.status(), 404, "{q}");
    }
    let r = at(&Browser::default(), format!("host={host}&next=%2F")).await;
    assert_eq!(r.status(), 302);
    assert!(
        r.headers()[LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/signin?next="),
        "{:?}",
        r.headers()[LOCATION]
    );
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    let (sam, _) = site.member(&owner, "Sam").await;
    let r = at(&sam, format!("host={host}&next=%2F")).await;
    assert_eq!(r.status(), 302);
    assert_eq!(
        r.headers()[LOCATION],
        format!("/a/{aid}"),
        "no right: the page says so"
    );
    let r = at(&owner, format!("host={host}&next=%2Fa.css")).await;
    assert_eq!(r.status(), 302);
    let to = r.headers()[LOCATION].to_str().unwrap().to_string();
    assert!(
        to.starts_with(&format!("http://{host}/_clax/session?code=")),
        "{to}"
    );
    let red = site.redeem(&aid, &to).await;
    assert_eq!(red.headers()[LOCATION], "/a.css");
    let set = red.headers()[SET_COOKIE].to_str().unwrap();
    assert!(
        set.contains("SameSite=Lax") && !set.contains("Partitioned"),
        "{set}"
    );
}

#[tokio::test]
async fn a_local_daemons_owner_holds_the_site_and_no_one_else() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Doc", &[("index.html", "<p>doc")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(ts.site_get_anon(&aid, "/").await.status(), 401);
    assert_eq!(ts.site_get(&aid, "/").await.status(), 200);
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/site-code", ts.base))
        .header("cookie", format!("clax_viewer={}", ts.local_browser()))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "only the owner's credential mints a code");
}

#[tokio::test]
async fn a_frame_starts_its_handshake_with_a_state_the_shell_binds_its_code_to() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Doc", &[("index.html", "<p>doc")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raw = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // The frame's first page: a state in a partitioned cookie, handed to
    // the shell's origin alone by a script only it runs.
    let r = raw
        .get(format!(
            "{}/_clax/session/start?next=%2F_clax%2Fv%2F1%2F",
            ts.base
        ))
        .header(HOST, ts.site_host(&aid))
        .header("sec-fetch-dest", "iframe")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // A local daemon's shell, on any of its loopback names.
    let port = ts.addr.port();
    let shells = format!("http://localhost:{port} http://127.0.0.1:{port}");
    let shell_list = format!("[\"http://localhost:{port}\",\"http://127.0.0.1:{port}\"]");
    let csp = r.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.starts_with("default-src 'none'; script-src 'nonce-"),
        "{csp}"
    );
    assert!(
        csp.ends_with(&format!("; frame-ancestors {shells}")),
        "{csp}"
    );
    assert_eq!(r.headers()["cache-control"], "no-store");
    let set = r.headers()[SET_COOKIE].to_str().unwrap().to_string();
    assert!(
        set.contains("Path=/;") && set.contains("Partitioned") && set.contains("HttpOnly"),
        "{set}"
    );
    let state = set
        .strip_prefix("clax_site_state=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let html = r.text().await.unwrap();
    assert!(
        html.contains(&format!(
            "for(const o of {shell_list})parent.postMessage({{\"next\":\"/_clax/v/1/\",\"state\":\"{state}\",\"type\":\"clax:site-state\"}},o);"
        )),
        "{html}"
    );
    // Another tab's frame starting while this one signs in shares the
    // state the browser holds, so neither undoes the other's.
    let again = raw
        .get(format!("{}/_clax/session/start?next=%2F", ts.base))
        .header(HOST, ts.site_host(&aid))
        .header("sec-fetch-dest", "iframe")
        .header(COOKIE, format!("clax_site_state={state}"))
        .send()
        .await
        .unwrap();
    assert!(
        again.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with(&format!("clax_site_state={state};"))
    );
    // The shell asks for a code bound to it; the frame redeems it beside
    // its state cookie, which a frame's redemption keeps for those tabs.
    let code = |state: Option<&str>| {
        let mut body = json!({"next": "/_clax/v/1/", "partitioned": true});
        if let Some(st) = state {
            body["state"] = json!(st);
        }
        let req = ts
            .authed(
                ts.client
                    .post(format!("{}/api/artifacts/{aid}/site-code", ts.base)),
            )
            .json(&body);
        async move { req.send().await.unwrap() }
    };
    let r = code(None).await;
    assert_eq!(r.status(), 400, "a code needs a state");
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_args"
    );
    let src: Value = code(Some(&state)).await.json().await.unwrap();
    let src = src["src"].as_str().unwrap();
    let r = raw
        .get(format!(
            "{}{}",
            ts.base,
            &src[src.find("/_clax/session").unwrap()..]
        ))
        .header(HOST, ts.site_host(&aid))
        .header(COOKIE, format!("clax_site_state={state}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()[LOCATION], "/_clax/v/1/");
    let sets: Vec<String> = r
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        sets[0].starts_with("clax_site=") && sets[0].contains("Partitioned"),
        "{sets:?}"
    );
    assert_eq!(sets.len(), 1, "{sets:?}");
    let cookie = sets[0].split(';').next().unwrap();
    let r = raw
        .get(format!("{}/_clax/v/1/", ts.base))
        .header(HOST, ts.site_host(&aid))
        .header(COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn a_frame_without_the_sites_cookies_is_told_so_rather_than_sent_to_sign_in() {
    let ts = TestServer::spawn().await;
    let aid = ts
        .publish("Doc", &[("index.html", "<p>secret words")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raw = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let port = ts.addr.port();
    let shell_list = format!("[\"http://localhost:{port}\",\"http://127.0.0.1:{port}\"]");
    let get = |path: &str, framed: bool, cookie: Option<&str>| {
        let mut req = raw
            .get(format!("{}{path}", ts.base))
            .header(HOST, ts.site_host(&aid))
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", if framed { "iframe" } else { "document" });
        if let Some(c) = cookie {
            req = req.header(COOKIE, c);
        }
        async move { req.send().await.unwrap() }
    };
    // The redemption's redirect to the page, and a code redeemed, with no
    // cookie at all: the browser keeps the frame's cookies from the site.
    for path in [
        "/_clax/v/1/",
        "/_clax/session?code=nope&next=%2F_clax%2Fv%2F1%2F",
    ] {
        let r = get(path, true, None).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["cache-control"], "no-store", "{path}");
        assert!(r.headers().get(SET_COOKIE).is_none(), "{path}");
        let csp = r.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            csp.starts_with("default-src 'none'; script-src 'nonce-"),
            "{csp}"
        );
        assert!(
            csp.ends_with(&format!(
                "; frame-ancestors http://localhost:{port} http://127.0.0.1:{port}"
            )),
            "{csp}"
        );
        let html = r.text().await.unwrap();
        assert!(
            html.contains(&format!("const S={shell_list};"))
                && html
                    .contains("for(const o of S)parent.postMessage({type:\"clax:no-cookie\"},o);")
                && html.contains("document.requestStorageAccess"),
            "{html}"
        );
        assert!(!html.contains("secret words"), "{html}");
    }
    // A frame that holds the state cookie, and any top-level visit, still
    // go to sign in.
    for (framed, cookie) in [(true, Some("clax_site_state=s")), (false, None)] {
        let r = get("/_clax/v/1/", framed, cookie).await;
        assert_eq!(r.status(), 302, "{framed} {cookie:?}");
        assert!(
            r.headers()[LOCATION]
                .to_str()
                .unwrap()
                .contains("/site-signin?")
        );
    }
}

#[tokio::test]
async fn a_code_minted_for_one_browser_signs_no_other_in() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    // An attacker's code, minted for the attacker's own state, handed to a
    // victim's browser: no state cookie, or the victim's own, redeems it.
    for victim in [None, Some("the-victims-state")] {
        let (_, src) = site
            .code(&owner, &aid, json!({"state": "the-attackers-state"}))
            .await;
        let r = site.redeem_as(&aid, &src, victim).await;
        assert_eq!(r.status(), 302, "{victim:?}");
        let sets: Vec<String> = r
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert!(
            sets.iter().all(|c| !c.starts_with("clax_site=")),
            "{victim:?}: {sets:?}"
        );
        assert!(
            r.headers()[LOCATION]
                .to_str()
                .unwrap()
                .starts_with(&format!("{}/site-signin?", site.origin))
        );
    }
    // The top-level handshake: the site's redirect sets the state the
    // sign-in binds its code to.
    let r = site
        .raw
        .get(format!("{}/", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header(ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    let to = r.headers()[LOCATION].to_str().unwrap().to_string();
    let state = to.rsplit_once("&state=").unwrap().1.to_string();
    let r = site
        .public(
            &owner,
            reqwest::Method::GET,
            &to[to.find("/site-signin").unwrap()..],
        )
        .send()
        .await
        .unwrap();
    let src = r.headers()[LOCATION].to_str().unwrap().to_string();
    assert!(
        site.redeem_as(&aid, &src, Some("another"))
            .await
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .all(|c| !c.to_str().unwrap().starts_with("clax_site="))
    );
    let r = site
        .public(
            &owner,
            reqwest::Method::GET,
            &to[to.find("/site-signin").unwrap()..],
        )
        .send()
        .await
        .unwrap();
    let src = r.headers()[LOCATION].to_str().unwrap().to_string();
    let r = site.redeem_as(&aid, &src, Some(&state)).await;
    assert!(
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("clax_site=")
    );
    // /site-signin without a state mints nothing.
    let host = site.site_host(&aid);
    let r = site
        .public(
            &owner,
            reqwest::Method::GET,
            &format!("/site-signin?host={host}&next=%2F"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn removing_a_person_ends_their_site_session_on_the_next_request() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let shares = format!("/api/hosted/artifacts/{aid}/shares");
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &shares,
            json!({"person": sam_pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, src) = site.code(&sam, &aid, json!({})).await;
    let cookie = site.session(&aid, &src).await;
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/people/{sam_pid}"),
            Value::Null,
        )
        .await;
    assert!(s == 200 || s == 204, "{s} {v}");
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 401);
}

#[tokio::test]
async fn deleting_the_artifact_ends_every_site_session_the_owners_too() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let (_, src) = site.code(&owner, &aid, json!({})).await;
    let cookie = site.session(&aid, &src).await;
    assert_eq!(site.on_site(&aid, "/", Some(&cookie)).await.status(), 200);
    let r = site
        .ts
        .authed(
            site.ts
                .client
                .delete(format!("{}/api/artifacts/{aid}", site.ts.base)),
        )
        .header(HOST, site.public_host())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    for p in ["/", "/a.css", "/_clax/v/1/"] {
        assert_eq!(
            site.on_site(&aid, p, Some(&cookie)).await.status(),
            401,
            "{p}"
        );
    }
}

#[tokio::test]
async fn a_world_site_is_read_by_anyone_until_it_is_private_again() {
    let site = Site::new().await;
    let aid = site.artifact("Plan").await;
    let id = clax_core::ArtifactId::parse(&aid).unwrap();
    site.ts.store.set_site_open_to_world(&id, true).unwrap();
    let r = site.on_site(&aid, "/a.css", None).await;
    assert_eq!(r.status(), 200);
    // An open site's files are public, revalidated on every request.
    assert_eq!(r.headers()["cache-control"], "public, no-cache");
    let r = site.on_site(&aid, "/_clax/v/1/a.css", None).await;
    assert_eq!(r.headers()["cache-control"], "public, no-cache");
    site.ts.store.set_site_open_to_world(&id, false).unwrap();
    assert_eq!(site.on_site(&aid, "/a.css", None).await.status(), 401);
}

#[tokio::test]
async fn a_private_sites_responses_are_private_to_their_browser() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
            .file_name("a.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let up: Value = site
        .ts
        .authed(
            site.ts
                .client
                .post(format!("{}/api/artifacts/{aid}/assets", site.ts.base)),
        )
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let blob = up["url"].as_str().unwrap().to_string();
    let (_, src) = site.code(&owner, &aid, json!({})).await;
    let cookie = site.session(&aid, &src).await;
    for (path, cache) in [
        ("/", "private, no-cache"),
        ("/a.css", "private, no-cache"),
        ("/_clax/v/1/", "private, no-cache"),
        ("/_clax/v/1/a.css", "private, max-age=31536000, immutable"),
        (blob.as_str(), "private, max-age=31536000, immutable"),
    ] {
        let r = site.on_site(&aid, path, Some(&cookie)).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["cache-control"], cache, "{path}");
        assert!(
            r.headers().get("access-control-allow-origin").is_none(),
            "{path}"
        );
    }
}

impl Site {
    /// `b`'s `PATCH /api/artifacts/<aid> {site_access: v}`.
    async fn site_access(&self, b: &Browser, aid: &str, v: &str) -> (u16, Value) {
        self.send(
            b,
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"site_access": v}),
        )
        .await
    }

    /// A Clax DB artifact whose `data/guestbook` anyone reads and writes,
    /// with an uploaded asset: its ID and the asset's.
    async fn guestbook(&self) -> (String, String) {
        let r = self
            .ts
            .post_json(
                "/api/artifacts",
                json!({
                    "title": "Guestbook",
                    "capabilities": {"room": {}, "db": {"rules": [
                        {"path": "data/guestbook", "read": "public", "write": "public"}
                    ]}},
                    "files": {"index.html": {"content": "<h1>Guestbook</h1>", "encoding": "utf8"}}
                }),
            )
            .await;
        assert_eq!(r.status(), 201);
        let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let part = reqwest::multipart::Part::bytes(vec![137, 80, 78, 71])
            .file_name("logo.png")
            .mime_str("image/png")
            .unwrap();
        let r = self
            .ts
            .authed(
                self.ts
                    .client
                    .post(format!("{}/api/artifacts/{aid}/assets", self.ts.base))
                    .multipart(reqwest::multipart::Form::new().part("file", part)),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201);
        let asset = r.json::<Value>().await.unwrap()["asset"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        (aid, asset)
    }

    /// A signed-out visitor's room socket on `aid`'s host, from its own
    /// origin.
    async fn visitor_room(
        &self,
        aid: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!(
            "{}/_clax/api/room?peer=0123456789abcdef",
            self.ts.base.replacen("http://", "ws://", 1)
        )
        .into_client_request()
        .unwrap();
        let h = req.headers_mut();
        h.insert("host", self.site_host(aid).parse().unwrap());
        h.insert(
            "origin",
            format!("http://{}", self.site_host(aid)).parse().unwrap(),
        );
        tokio_tungstenite::connect_async(req)
            .await
            .expect("upgrade")
            .0
    }

    /// A signed-out visitor's runtime call on `aid`'s host, from its own
    /// origin.
    async fn visitor_api(
        &self,
        aid: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut r = self
            .raw
            .request(method, format!("{}/_clax/api/{path}", self.ts.base))
            .header(HOST, self.site_host(aid))
            .header(ORIGIN, format!("http://{}", self.site_host(aid)));
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send().await.unwrap()
    }
}

#[tokio::test]
async fn a_world_site_is_served_to_anyone_and_its_page_is_not() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, _) = site.guestbook().await;
    let (s, v) = site.site_access(&owner, &aid, "world").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["artifact"]["site_access"], "world");
    let (_, got) = site
        .send(
            &owner,
            reqwest::Method::GET,
            &format!("/api/hosted/artifacts/{aid}/sharing"),
            Value::Null,
        )
        .await;
    assert_eq!(got["site_access"], "world", "{got}");
    assert_eq!(got["site_url"], format!("http://{}/", site.site_host(&aid)));
    assert_eq!(v["site_url"], got["site_url"]);
    let r = site
        .raw
        .get(format!("{}/", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header(ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("clax_visitor=")
    );
    assert!(r.text().await.unwrap().contains("<h1>Guestbook</h1>"));
    // The artifact's page on the public host keeps its own access.
    let r = site
        .public(
            &Browser::default(),
            reqwest::Method::GET,
            &format!("/a/{aid}"),
        )
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 200);
    assert!(!r.text().await.unwrap().contains("Guestbook"));
    let (s, _) = site
        .send(
            &Browser::default(),
            reqwest::Method::GET,
            &format!("/api/artifacts/{aid}/threads"),
            Value::Null,
        )
        .await;
    assert!(s == 401 || s == 404, "{s}");
    // Only who manages it opens it: a person it is shared with does not.
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/shares"),
            json!({"person": sam_pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = site.site_access(&sam, &aid, "private").await;
    assert!(s == 403, "{s} {v}");
    let (s, v) = site.site_access(&owner, &aid, "everyone").await;
    assert_eq!((s, code_of(&v)), (400, "invalid_argument"));
}

#[tokio::test]
async fn a_world_sites_files_revalidate_on_every_request_and_only_navigations_set_the_cookie() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, asset) = site.guestbook().await;
    site.site_access(&owner, &aid, "world").await;
    let cc = |r: &reqwest::Response| {
        assert!(r.headers().get("access-control-allow-origin").is_none());
        r.headers()["cache-control"].to_str().unwrap().to_string()
    };
    for (p, want) in [
        ("/", "public, no-cache"),
        // A version's files and the assets revalidate too, never
        // `immutable`: closing the site holds on the next request.
        ("/_clax/v/1/", "public, no-cache"),
        (&format!("/_clax/blob/{asset}") as &str, "public, no-cache"),
    ] {
        let r = site.on_site(&aid, p, None).await;
        assert_eq!(r.status(), 200, "{p}");
        assert!(r.headers().get(SET_COOKIE).is_none(), "{p} sets no cookie");
        assert_eq!(cc(&r), want, "{p}");
    }
    // A navigation sets the cookie, privately.
    let r = site
        .raw
        .get(format!("{}/", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header(ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert!(r.headers().get(SET_COOKIE).is_some());
    assert_eq!(cc(&r), "private, no-store");
    // So does the runtime's first call; a forged cookie is replaced.
    let r = site
        .raw
        .get(format!("{}/_clax/api/whoami", site.ts.base))
        .header(HOST, site.site_host(&aid))
        .header(COOKIE, "clax_visitor=someone-elses.id")
        .send()
        .await
        .unwrap();
    assert!(
        r.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("clax_visitor=")
    );
    assert_eq!(cc(&r), "private, no-store");
}

#[tokio::test]
async fn closing_a_world_site_refuses_the_next_visitor_request() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, asset) = site.guestbook().await;
    site.site_access(&owner, &aid, "world").await;
    let blob = format!("/_clax/blob/{asset}");
    let doc = "docs/data/guestbook/entries/e1";
    assert_eq!(site.on_site(&aid, "/", None).await.status(), 200);
    assert_eq!(site.on_site(&aid, &blob, None).await.status(), 200);
    let r = site
        .visitor_api(
            &aid,
            reqwest::Method::PUT,
            doc,
            Some(json!({"data": {"hi": 1}})),
        )
        .await;
    assert_eq!(r.status(), 200);
    // A visitor's room socket, open while the site is.
    let mut ws = site.visitor_room(&aid).await;
    let welcome = ws.next().await.unwrap().unwrap();
    assert!(welcome.to_text().unwrap().contains("welcome"));
    let (s, v) = site.site_access(&owner, &aid, "private").await;
    assert_eq!((s, &v["artifact"]["site_access"]), (200, &json!("private")));
    let closed = loop {
        match ws.next().await.expect("a frame").expect("readable") {
            tokio_tungstenite::tungstenite::Message::Close(c) => break c.expect("a close frame"),
            _ => continue,
        }
    };
    assert_eq!(
        (u16::from(closed.code), closed.reason.as_str()),
        (4403, "revoked")
    );
    for p in ["/", "/index.html", blob.as_str()] {
        let r = site.on_site(&aid, p, None).await;
        assert!(
            r.status() == 401 || r.status() == 301,
            "{p}: {}",
            r.status()
        );
    }
    for m in [reqwest::Method::GET, reqwest::Method::PUT] {
        let body = (m == reqwest::Method::PUT).then(|| json!({"data": {}}));
        let r = site.visitor_api(&aid, m.clone(), doc, body).await;
        assert_eq!(r.status(), 401, "{m}");
    }
}

#[tokio::test]
async fn the_open_site_link_signs_the_owner_in_on_a_world_site() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, _) = site.guestbook().await;
    site.site_access(&owner, &aid, "world").await;
    // "Open site" goes through /site-signin, which makes the owner's site
    // session there too.
    let host = site.site_host(&aid);
    let r = site
        .public(
            &owner,
            reqwest::Method::GET,
            &format!("/site-signin?host={host}&next=%2F&state={STATE}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    let to = r.headers()[LOCATION].to_str().unwrap().to_string();
    let cookie = site.session(&aid, &to).await;
    let r = site
        .raw
        .get(format!("{}/_clax/api/whoami", site.ts.base))
        .header(HOST, &host)
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers().get(SET_COOKIE).is_none(),
        "a holder is no visitor"
    );
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        (v["kind"].as_str(), v["can_edit"].as_bool()),
        (Some("owner"), Some(true)),
        "{v}"
    );
    // Without it, the same site reads them as a visitor.
    let r = site
        .visitor_api(&aid, reqwest::Method::GET, "whoami", None)
        .await;
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["kind"], "visitor");
}

#[tokio::test]
async fn a_world_visitors_runtime_answers_are_never_public() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, _) = site.guestbook().await;
    let (s, v) = site.site_access(&owner, &aid, "world").await;
    assert_eq!(s, 200, "{v}");
    let put = site
        .visitor_api(
            &aid,
            reqwest::Method::PUT,
            "docs/data/guestbook/entries/hello",
            Some(json!({"data": {"hi": 1}})),
        )
        .await;
    assert_eq!(put.status(), 200);
    let cookie = put.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let read = site
        .raw
        .get(format!(
            "{}/_clax/api/docs/data/guestbook/entries/hello",
            site.ts.base
        ))
        .header(HOST, site.site_host(&aid))
        .header(ORIGIN, format!("http://{}", site.site_host(&aid)))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    let cache = read.headers()["cache-control"].to_str().unwrap();
    assert!(cache.starts_with("private"), "{cache}");
    // A visitor's page, by contrast, is public and varies by Cookie.
    let page = site.on_site(&aid, "/", Some(&cookie)).await;
    assert_eq!(page.headers()["cache-control"], "public, no-cache");
    assert_eq!(page.headers()["vary"], "Cookie");
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Site {
    /// An artifact that declares `room`, published by the owner's token.
    async fn room_artifact(&self, title: &str) -> String {
        let r = self
            .ts
            .post_json(
                "/api/artifacts",
                json!({
                    "title": title,
                    "capabilities": {"room": {}},
                    "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
                }),
            )
            .await;
        assert_eq!(r.status(), 201);
        r.json::<Value>().await.unwrap()["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A room socket under `peer` on `aid`'s site host from its own origin,
    /// with site cookie `cookie` (the `Cookie` header [`Site::session`]
    /// gives), its welcome read.
    async fn holder_room(&self, aid: &str, cookie: &str, peer: &str) -> Ws {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!(
            "{}/_clax/api/room?peer={peer}",
            self.ts.base.replacen("http://", "ws://", 1)
        )
        .into_client_request()
        .unwrap();
        let h = req.headers_mut();
        h.insert("host", self.site_host(aid).parse().unwrap());
        h.insert(
            "origin",
            format!("http://{}", self.site_host(aid)).parse().unwrap(),
        );
        h.insert("cookie", cookie.parse().unwrap());
        let mut ws = tokio_tungstenite::connect_async(req)
            .await
            .expect("upgrade")
            .0;
        let welcome = ws.next().await.unwrap().unwrap();
        assert!(welcome.to_text().unwrap().contains("welcome"), "{welcome}");
        ws
    }

    /// `b`'s share of `aid` with person `pid` as a viewer: its ID.
    async fn share(&self, b: &Browser, aid: &str, pid: &str) -> String {
        let (s, v) = self
            .send(
                b,
                reqwest::Method::PUT,
                &format!("/api/hosted/artifacts/{aid}/shares"),
                json!({"person": pid, "access": "viewer"}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["share"]["id"].as_str().unwrap().to_string()
    }

    /// A site session of `b` on `aid`: the `Cookie` header that carries it.
    async fn site_cookie(&self, b: &Browser, aid: &str) -> String {
        let (s, src) = self.code(b, aid, json!({})).await;
        assert_eq!(s, 200);
        self.session(aid, &src).await
    }
}

/// The close `ws` ends with, skipping the frames before it.
async fn closed(ws: &mut Ws) -> (u16, String) {
    loop {
        match ws.next().await.expect("a frame").expect("readable") {
            tokio_tungstenite::tungstenite::Message::Close(c) => {
                let c = c.expect("a close frame");
                return (u16::from(c.code), c.reason.to_string());
            }
            _ => continue,
        }
    }
}

/// That `ws` is still open: a join it sends is acked. The daemon acts on
/// every event published before a socket's message ahead of the message,
/// so an ack here means the changes made before it left the socket open.
async fn stays_open(ws: &mut Ws) {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    ws.send(Message::Text(
        json!({"t": "join", "id": 7, "room": "probe"})
            .to_string()
            .into(),
    ))
    .await
    .expect("sent");
    loop {
        match ws.next().await.expect("a frame").expect("readable") {
            Message::Text(t) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if v["t"] == "ack" && v["id"] == 7 {
                    return;
                }
            }
            Message::Close(c) => panic!("closed: {c:?}"),
            _ => continue,
        }
    }
}

const REVOKED: (u16, &str) = (4403, "revoked");

#[tokio::test]
async fn removing_a_share_closes_the_holders_room_socket_and_no_one_elses() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let share = site.share(&owner, &aid, &sam_pid).await;
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{aid}/shares/{share}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200);
    let (code, reason) = closed(&mut sams).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut owners).await;
}

/// A room socket opened on the public host with a hosted browser session
/// is checked again as a site holder's is: removing the share closes it,
/// signing the browser out closes the owner's.
#[tokio::test]
async fn a_public_host_room_socket_closes_when_its_session_stops_reading() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let share = site.share(&owner, &aid, &sam_pid).await;
    let public_room = |b: &Browser, peer: &str| {
        let mut req = format!(
            "{}/api/artifacts/{aid}/room?peer={peer}",
            site.ts.base.replacen("http://", "ws://", 1)
        )
        .into_client_request()
        .unwrap();
        let h = req.headers_mut();
        h.insert("host", site.public_host().parse().unwrap());
        h.insert("origin", site.origin.parse().unwrap());
        h.insert(
            "cookie",
            format!("clax_session={}", b.cookie.as_ref().unwrap())
                .parse()
                .unwrap(),
        );
        async move {
            let mut ws = tokio_tungstenite::connect_async(req)
                .await
                .expect("upgrade")
                .0;
            let welcome = ws.next().await.unwrap().unwrap();
            assert!(welcome.to_text().unwrap().contains("welcome"), "{welcome}");
            ws
        }
    };
    let mut sams = public_room(&sam, "000000000000000s").await;
    let mut owners = public_room(&owner, "000000000000000o").await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{aid}/shares/{share}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200);
    let (code, reason) = closed(&mut sams).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut owners).await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::POST,
            "/api/hosted/signout",
            json!({}),
        )
        .await;
    assert_eq!(s, 204);
    let (code, reason) = closed(&mut owners).await;
    assert_eq!((code, reason.as_str()), REVOKED);
}

#[tokio::test]
async fn turning_the_link_off_closes_a_link_viewers_room_socket() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let link = format!("/api/hosted/artifacts/{aid}/link");
    let set = |access: Value| {
        site.send(
            &owner,
            reqwest::Method::PUT,
            &link,
            json!({"access": access}),
        )
    };
    let (s, v) = set(json!("viewer")).await;
    assert_eq!(s, 200, "{v}");
    let viewer_cookie = site.site_cookie(&Browser::default(), &aid).await;
    let mut viewers = site
        .holder_room(&aid, &viewer_cookie, "000000000000000l")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    // A change that keeps the link leaves the link viewer's socket open.
    assert_eq!(set(json!("commenter")).await.0, 200);
    stays_open(&mut viewers).await;
    assert_eq!(set(Value::Null).await.0, 200);
    let (code, reason) = closed(&mut viewers).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut owners).await;
}

#[tokio::test]
async fn signing_out_closes_the_room_sockets_of_the_browsers_site_sessions() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &sam_pid).await;
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::POST,
            "/api/hosted/signout",
            json!({}),
        )
        .await;
    assert_eq!(s, 204);
    let (code, reason) = closed(&mut owners).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut sams).await;
}

#[tokio::test]
async fn removing_a_person_closes_their_room_socket() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &sam_pid).await;
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/people/{sam_pid}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (code, reason) = closed(&mut sams).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut owners).await;
}

/// Narrowing a person's rights takes away what their old rights let them
/// read: their room socket on its site closes. (On a hosted Clax an invite
/// makes a new person only; a person's rights change in People.)
#[tokio::test]
async fn narrowing_a_persons_rights_closes_their_room_socket() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let rights_at = format!("/api/people/{sam_pid}/rights");
    let set_rights = |rights: Value| {
        site.send(
            &owner,
            reqwest::Method::PUT,
            &rights_at,
            json!({"rights": rights}),
        )
    };
    let reads = json!({"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]});
    let (s, v) = set_rights(json!([reads])).await;
    assert_eq!(s, 200, "{v}");
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    let (s, v) = set_rights(json!([])).await;
    assert_eq!(s, 200, "{v}");
    let (code, reason) = closed(&mut sams).await;
    assert_eq!((code, reason.as_str()), REVOKED);
    stays_open(&mut owners).await;
}

/// On a hosted Clax a person reads a site only from a browser that redeemed
/// their invite (or signed in another way that keeps them a person), so
/// revoking that invite afterwards changes nothing: the invite is used, the
/// person stays one, and the room socket the revocation has checked again
/// stays open.
#[tokio::test]
async fn revoking_a_redeemed_invite_leaves_its_persons_room_socket_open() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &sam_pid).await;
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::GET,
            "/api/people/invites",
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let invites = v["invites"].as_array().unwrap();
    let sams_invite = invites
        .iter()
        .find(|i| i["person"]["public_id"] == sam_pid.as_str())
        .unwrap_or_else(|| panic!("{v}"));
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!(
                "/api/people/invites/{}",
                sams_invite["id"].as_str().unwrap()
            ),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["invite"]["used_at"].is_string(), "{v}");
    assert!(
        v["invite"]["revoked_at"].is_null(),
        "used, so unchanged: {v}"
    );
    stays_open(&mut sams).await;
}

/// A site room socket in use renews its site session, so it outlives the
/// session's hour counted from its opening: an unrelated access change
/// past that hour leaves it open, while one opened as long ago and unused
/// since closes; the holder losing the right still closes it.
#[tokio::test]
async fn a_site_room_socket_in_use_outlives_its_sessions_first_hour() {
    use clax_store::site_sessions::{SITE_SESSION_RENEW_SECS, SITE_SESSION_TTL_SECS};
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let (mia, _) = site.member(&owner, "Mia").await;
    let share = site.share(&owner, &aid, &sam_pid).await;
    let sam_cookie = site.site_cookie(&sam, &aid).await;
    let mut sams = site
        .holder_room(&aid, &sam_cookie, "000000000000000s")
        .await;
    let owner_cookie = site.site_cookie(&owner, &aid).await;
    let mut owners = site
        .holder_room(&aid, &owner_cookie, "000000000000000o")
        .await;
    // Sam's page sends messages through the hour; the owner's sends none.
    for _ in 0..(SITE_SESSION_TTL_SECS / SITE_SESSION_RENEW_SECS) {
        site.clock.advance(SITE_SESSION_RENEW_SECS);
        stays_open(&mut sams).await;
    }
    site.clock.advance(SITE_SESSION_RENEW_SECS);
    // Someone else signing out asks every holder's socket to be checked.
    let (s, _) = site
        .send(
            &mia,
            reqwest::Method::POST,
            "/api/hosted/signout",
            json!({}),
        )
        .await;
    assert_eq!(s, 204);
    stays_open(&mut sams).await;
    let (code, reason) = closed(&mut owners).await;
    assert_eq!(
        (code, reason.as_str()),
        REVOKED,
        "the unused session expired"
    );
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{aid}/shares/{share}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200);
    let (code, reason) = closed(&mut sams).await;
    assert_eq!((code, reason.as_str()), REVOKED);
}

/// A site's room socket as a Durable Object holds it
/// (`clax_server::routes::room::RoomSocket`, reopened after a wake by
/// `room::reopen` from the site credentials its attachment keeps): it
/// reopens as its holder while they read the artifact, and asks to be
/// checked again when who may read changes, after which it neither stays
/// nor reopens.
#[tokio::test]
async fn a_kept_site_room_socket_reopens_as_its_holder_while_they_read() {
    use clax_http::identity::SocketCredentials;
    use clax_http::site_access::SiteCredentials;
    use clax_server::routes::room::{self, RoomStep};
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.room_artifact("Board").await;
    let (sam, sam_pid) = site.member(&owner, "Sam").await;
    let share = site.share(&owner, &aid, &sam_pid).await;
    let kept = |cookie: &str| {
        let value = cookie.split_once('=').unwrap().1;
        SiteCredentials::Session(clax_core::secret::sha256_hex(value))
    };
    let sams = kept(&site.site_cookie(&sam, &aid).await);
    let owners = kept(&site.site_cookie(&owner, &aid).await);
    let s = &site.ts.state;
    // What the site API's room route keeps beside them: no hosted session
    // and no viewer cookie (the owner's would carry the token's tag).
    let none = SocketCredentials::default();
    let Some(Ok((mut sam_room, first))) =
        room::reopen(s, &aid, "000000000000000s", &[], &none, Some(&sams)).await
    else {
        panic!("Sam's socket reopens");
    };
    assert!(first[0].contains("welcome"), "{first:?}");
    assert_eq!(sam_room.site(), Some(&sams));
    let Some(Ok((mut owner_room, _))) =
        room::reopen(s, &aid, "000000000000000o", &[], &none, Some(&owners)).await
    else {
        panic!("the owner's socket reopens");
    };
    // A session that is not one, or another artifact's, reopens nothing.
    let other = site.room_artifact("Other").await;
    let forged = SiteCredentials::Session("0".repeat(64));
    for (a, c) in [(&aid, &forged), (&other, &sams)] {
        assert!(
            room::reopen(s, a, "000000000000000f", &[], &none, Some(c))
                .await
                .is_none()
        );
    }
    let (st, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{aid}/shares/{share}"),
            Value::Null,
        )
        .await;
    assert_eq!(st, 200);
    let recheck = |steps: Vec<RoomStep>| {
        steps.into_iter().find_map(|s| match s {
            RoomStep::Recheck(c) => Some(c),
            _ => None,
        })
    };
    let sam_check = recheck(sam_room.drain()).expect("Sam's socket asks to be checked");
    assert!(!sam_check.stays(s).await, "Sam no longer reads it");
    let owner_check = recheck(owner_room.drain()).expect("the owner's socket asks too");
    assert!(owner_check.stays(s).await, "the owner still reads it");
    assert!(
        room::reopen(s, &aid, "000000000000000s", &[], &none, Some(&sams))
            .await
            .is_none(),
        "after a wake Sam's socket stays closed"
    );
}

#[tokio::test]
async fn a_kept_visitor_room_socket_reopens_while_the_site_is_open_to_the_world() {
    use clax_http::identity::SocketCredentials;
    use clax_http::site_access::SiteCredentials;
    use clax_server::routes::room::{self, RoomStep};
    let site = Site::new().await;
    let owner = site.owner().await;
    let (aid, _) = site.guestbook().await;
    site.site_access(&owner, &aid, "world").await;
    let me: Value = site
        .visitor_api(&aid, reqwest::Method::GET, "whoami", None)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(me["kind"], "visitor", "{me}");
    let kept = SiteCredentials::Visitor(me["id"].as_str().unwrap().to_string());
    let s = &site.ts.state;
    let none = SocketCredentials::default();
    let Some(Ok((mut socket, _))) =
        room::reopen(s, &aid, "000000000000000v", &[], &none, Some(&kept)).await
    else {
        panic!("the visitor's socket reopens");
    };
    // Who may read changing elsewhere asks nothing of a visitor's socket.
    let (sam, _) = site.member(&owner, "Sam").await;
    let (st, _) = site
        .send(
            &sam,
            reqwest::Method::POST,
            "/api/hosted/signout",
            json!({}),
        )
        .await;
    assert_eq!(st, 204);
    assert!(
        !socket
            .drain()
            .iter()
            .any(|s| matches!(s, RoomStep::Recheck(_) | RoomStep::Close(..)))
    );
    site.site_access(&owner, &aid, "private").await;
    assert!(socket.drain().contains(&RoomStep::Close(4403, "revoked")));
    assert!(
        room::reopen(s, &aid, "000000000000000v", &[], &none, Some(&kept))
            .await
            .is_none(),
        "closed to the world, it reopens no visitor's socket"
    );
}

// The reach rule on a hosted Clax: the owner is a person with extra powers,
// never universal access, so another person's site opens to them only as
// the artifacts shared with them, or open by their link, do.

impl Site {
    /// An artifact person `b` starts in their browser (the gallery's New):
    /// theirs, beyond the owner's reach.
    async fn made_by(&self, b: &Browser, title: &str) -> String {
        let (s, v) = self
            .send(
                b,
                reqwest::Method::POST,
                "/api/artifacts:new",
                json!({"title": title, "brief": "x"}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["artifact"]["id"].as_str().unwrap().to_string()
    }

    /// `b`'s public ID.
    async fn pid(&self, b: &Browser) -> String {
        let (_, me) = self
            .send(b, reqwest::Method::GET, "/api/hosted/me", Value::Null)
            .await;
        me["person"]["public_id"].as_str().unwrap().to_string()
    }

    /// A site API request on `aid`'s host from its own origin, with site
    /// cookie `cookie`.
    async fn site_api(
        &self,
        aid: &str,
        cookie: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut r = self
            .raw
            .request(method, format!("{}/_clax/api/{path}", self.ts.base))
            .header(HOST, self.site_host(aid))
            .header(ORIGIN, format!("http://{}", self.site_host(aid)))
            .header(COOKIE, cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let res = r.send().await.unwrap();
        let s = res.status().as_u16();
        (s, res.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn the_owner_signs_in_to_no_site_of_another_persons_page() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (mia, _) = site.member(&owner, "Mia").await;
    let mias = site.made_by(&mia, "Mia's plan").await;
    // Mia signs in to her own site.
    let (s, _) = site.code(&mia, &mias, json!({})).await;
    assert_eq!(s, 200);
    // The owner's browser and the owner's token mint no code for it.
    let (s, _) = site.code(&owner, &mias, json!({})).await;
    assert_eq!(s, 404);
    let r = site
        .ts
        .authed(
            site.ts
                .client
                .post(format!("{}/api/artifacts/{mias}/site-code", site.ts.base)),
        )
        .header(HOST, site.public_host())
        .header(ORIGIN, &site.origin)
        .json(&json!({"state": STATE}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    // A top-level visit is sent to the page, which is not there for them.
    let host = site.site_host(&mias);
    let r = site
        .public(
            &owner,
            reqwest::Method::GET,
            &format!("/site-signin?host={host}&next=%2F&state={STATE}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers()[LOCATION], format!("/a/{mias}"));
    let r = site
        .public(&owner, reqwest::Method::GET, &format!("/a/{mias}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    // And the site itself serves the owner nothing.
    assert_eq!(site.on_site(&mias, "/", None).await.status(), 401);
}

#[tokio::test]
async fn a_page_shared_with_the_owner_opens_on_its_site_at_the_shares_level_alone() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let owner_pid = site.pid(&owner).await;
    let (mia, _) = site.member(&owner, "Mia").await;
    let mias = site.made_by(&mia, "Mia's plan").await;
    let shares = format!("/api/hosted/artifacts/{mias}/shares");
    let (s, v) = site
        .send(
            &mia,
            reqwest::Method::PUT,
            &shares,
            json!({"person": owner_pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let share = v["share"]["id"].as_str().unwrap().to_string();
    let cookie = site.site_cookie(&owner, &mias).await;
    assert_eq!(site.on_site(&mias, "/", Some(&cookie)).await.status(), 200);
    // The owner reads as a viewer of Mia's page, never as its admin.
    let (s, v) = site
        .site_api(&mias, &cookie, reqwest::Method::GET, "whoami", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["kind"], "owner", "{v}");
    assert_ne!(v["level"], "admin", "{v}");
    assert_eq!(v["can_edit"], false, "{v}");
    // A viewer uploads nothing to it.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
            .file_name("a.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let r = site
        .raw
        .post(format!("{}/_clax/api/assets", site.ts.base))
        .header(HOST, site.site_host(&mias))
        .header(ORIGIN, format!("http://{}", site.site_host(&mias)))
        .header(COOKIE, &cookie)
        .multipart(form)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    assert!(
        status == 403 || status == 404,
        "upload: {status} {}",
        r.text().await.unwrap()
    );
    let (_, assets) = site
        .send(
            &mia,
            reqwest::Method::GET,
            &format!("/api/artifacts/{mias}/assets"),
            Value::Null,
        )
        .await;
    assert_eq!(assets["assets"], json!([]), "{assets}");
    // Once Mia ends the share, the owner's next request is refused.
    let (s, _) = site
        .send(
            &mia,
            reqwest::Method::DELETE,
            &format!("{shares}/{share}"),
            Value::Null,
        )
        .await;
    assert!(s == 200 || s == 204, "{s}");
    assert_eq!(site.on_site(&mias, "/", Some(&cookie)).await.status(), 401);
}

#[tokio::test]
async fn only_its_maker_opens_a_persons_site_to_the_world_and_revoking_them_takes_it_down() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let owner_pid = site.pid(&owner).await;
    let (mia, mia_pid) = site.member(&owner, "Mia").await;
    let mias = site.made_by(&mia, "Mia's plan").await;
    // Out of the owner's reach, it is not there for them to open.
    let (s, v) = site.site_access(&owner, &mias, "world").await;
    assert_eq!(s, 404, "{v}");
    // Shared with the owner to edit, it is still Mia's to manage.
    let (s, v) = site
        .send(
            &mia,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{mias}/shares"),
            json!({"person": owner_pid, "access": "editor"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site.site_access(&owner, &mias, "world").await;
    assert_eq!((s, code_of(&v)), (403, "admin_only"), "{v}");
    assert_eq!(site.on_site(&mias, "/", None).await.status(), 401);
    // Mia opens it, and anyone reads it: her choice, no leak.
    let (s, v) = site.site_access(&mia, &mias, "world").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(site.on_site(&mias, "/", None).await.status(), 200);
    // Revoking Mia takes her pages down, her world site too, until she is
    // reinstated.
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/people/{mia_pid}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(site.on_site(&mias, "/", None).await.status(), 401);
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::POST,
            &format!("/api/people/{mia_pid}/reinstate"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(site.on_site(&mias, "/", None).await.status(), 200);
}

#[tokio::test]
async fn revoking_a_person_closes_their_world_sites_visitor_sockets() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (mia, mia_pid) = site.member(&owner, "Mia").await;
    let board = site.room_artifact("Board").await;
    let (mias, _) = site
        .ts
        .store
        .fork_artifact_async(
            &clax_core::audit::AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(&board).unwrap(),
            None,
            Some(&mia_pid),
            None,
        )
        .await
        .unwrap();
    let mias = mias.id;
    let (s, v) = site.site_access(&mia, &mias, "world").await;
    assert_eq!(s, 200, "{v}");
    let mut visitor = site.visitor_room(&mias).await;
    let welcome = visitor.next().await.unwrap().unwrap();
    assert!(welcome.to_text().unwrap().contains("welcome"), "{welcome}");
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/people/{mia_pid}"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (code, reason) = closed(&mut visitor).await;
    assert_eq!((code, reason.as_str()), REVOKED);
}
