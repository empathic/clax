//! Who reads an artifact's site (`clax_http::site_access`): a site session
//! made by a one-use code from the shell (`POST
//! /api/artifacts/<id>/site-code`) or the public host's `/site-signin`,
//! checked against the holder's right on every request.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{ACCEPT, COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";

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

    /// `b`'s site code for `aid`: the status, and the `src` it answers.
    async fn code(&self, b: &Browser, aid: &str, body: Value) -> (u16, String) {
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

    /// Follows `src` on `aid`'s site host: the response, unfollowed.
    async fn redeem(&self, aid: &str, src: &str) -> reqwest::Response {
        let path = &src[src.find("/_clax/session").expect("a session address")..];
        self.raw
            .get(format!("{}{path}", self.ts.base))
            .header(HOST, self.site_host(aid))
            .send()
            .await
            .unwrap()
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
    assert_eq!(
        r.headers()[LOCATION],
        format!("{}/site-signin?host={host}&next=%2F", site.origin)
    );
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
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
            .ends_with("&next=%2Fa.css%3Fx%3D1"),
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
    assert!(again.headers().get(SET_COOKIE).is_none());
    assert!(
        again.headers()[LOCATION]
            .to_str()
            .unwrap()
            .starts_with(&format!("{}/site-signin?", site.origin))
    );
    let (_, late) = site.code(&owner, &aid, json!({})).await;
    site.clock.advance(61);
    let r = site.redeem(&aid, &late).await;
    assert!(r.headers().get(SET_COOKIE).is_none());
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
            &format!("/site-signin?host={host}&next=%2Fa.css"),
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
    assert!(r.headers().get(SET_COOKIE).is_none());
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
        let r = site.public(b, reqwest::Method::GET, &format!("/site-signin?{q}"));
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
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "only the owner's credential mints a code");
}
