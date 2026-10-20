//! Hosted mode's sign-in (`clax_http::hosted`): the first person's
//! setup, invites redeemed in the browser, one-use sign-in links, sign out,
//! the session cookie's flags, CSRF on every change, agent tokens, and
//! guests on pages shared by link; and the local daemon serving none of it.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A hosted daemon on a stopped clock, and the browser-side helpers.
struct Site {
    ts: TestServer,
    clock: Arc<ManualClock>,
    origin: String,
}

/// One browser: the session cookie it holds, if any.
#[derive(Clone, Default)]
struct Browser {
    cookie: Option<String>,
}

impl Site {
    async fn new() -> Site {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        Site { ts, clock, origin }
    }

    /// [`Site::new`] with the public gallery on.
    async fn with_public_gallery() -> Site {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted_with(clock.clone(), Some(SECRET), |s| {
            s.hosted = s.hosted.take().map(|h| {
                Arc::new(
                    Arc::try_unwrap(h)
                        .expect("hosted is not shared yet")
                        .with_public_gallery(true),
                )
            });
        })
        .await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        Site { ts, clock, origin }
    }

    /// [`Site::new`] showing `legal`'s Terms of Service and Privacy Policy.
    async fn with_legal(legal: clax_core::config::Legal) -> Site {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted_with(clock.clone(), Some(SECRET), |s| {
            s.hosted = s.hosted.take().map(|h| {
                Arc::new(
                    Arc::try_unwrap(h)
                        .expect("hosted is not shared yet")
                        .with_legal(Some(legal)),
                )
            });
        })
        .await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        Site { ts, clock, origin }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.ts.base)
    }

    /// A request from `b`, as the shell at the public origin makes it.
    fn req(&self, b: &Browser, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .ts
            .client
            .request(method.clone(), self.url(path))
            .header(HOST, format!("localhost:{}", self.ts.addr.port()));
        if method != reqwest::Method::GET {
            r = r.header(ORIGIN, &self.origin);
        }
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        r
    }

    async fn get(&self, b: &Browser, path: &str) -> (u16, Value) {
        json_of(
            self.req(b, reqwest::Method::GET, path)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    async fn send(
        &self,
        b: &Browser,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        json_of(self.req(b, method, path).json(&body).send().await.unwrap()).await
    }

    async fn post(&self, b: &Browser, path: &str, body: Value) -> (u16, Value) {
        self.send(b, reqwest::Method::POST, path, body).await
    }

    /// Signs `b` in through a route answering with the session cookie.
    async fn sign_in(&self, b: &mut Browser, path: &str, body: Value) -> Value {
        let res = self
            .req(b, reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        let cookie = res
            .headers()
            .get(SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string());
        let (status, v) = json_of(res).await;
        assert_eq!(status, 200, "{path}: {v}");
        let cookie = cookie.expect("a session cookie");
        b.cookie = Some(
            cookie
                .strip_prefix("clax_session=")
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string(),
        );
        v
    }

    /// Signs `b` in as a guest of the name `body` gives.
    async fn sign_in_put(&self, b: &mut Browser, body: Value) -> Value {
        let res = self
            .req(b, reqwest::Method::PUT, "/api/viewers/me")
            .json(&body)
            .send()
            .await
            .unwrap();
        let cookie = res
            .headers()
            .get(SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string());
        let (status, v) = json_of(res).await;
        assert_eq!(status, 200, "{v}");
        b.cookie = cookie.and_then(|c| {
            c.strip_prefix("clax_session=")
                .map(|c| c.split(';').next().unwrap().to_string())
        });
        v
    }

    /// The owner, signed in through setup.
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

    /// A collaborator (`clax:collaborator`, every artifact), invited by
    /// `owner` and joined in a new browser.
    async fn person(&self, owner: &Browser, name: &str) -> Browser {
        self.invited(owner, json!({"name": name, "rights": "clax:collaborator"}))
            .await
    }

    /// A person invited by `owner` with the default rights (their own
    /// space alone), joined in a new browser.
    async fn member(&self, owner: &Browser, name: &str) -> Browser {
        self.invited(owner, json!({"name": name})).await
    }

    async fn invited(&self, owner: &Browser, body: Value) -> Browser {
        let (s, v) = self.post(owner, "/api/people/invites", body).await;
        assert_eq!(s, 201, "{v}");
        let url = v["invite"]["url"].as_str().unwrap();
        let code = url.rsplit('/').next().unwrap();
        let mut b = Browser::default();
        self.sign_in(
            &mut b,
            &format!("/api/hosted/invites/{code}/accept"),
            json!({}),
        )
        .await;
        b
    }

    /// An artifact the owner's agent published.
    async fn artifact(&self) -> String {
        self.ts
            .publish("Plan", &[("index.html", "<h1>Plan</h1>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A thread `b` starts on `aid`, as the shell does.
    async fn thread(&self, b: &Browser, aid: &str, body: &str) -> (u16, Value) {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        json_of(
            self.req(
                b,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/threads"),
            )
            .multipart(form)
            .send()
            .await
            .unwrap(),
        )
        .await
    }
}

#[tokio::test]
async fn a_local_daemon_serves_none_of_it() {
    let ts = TestServer::spawn().await;
    for path in [
        "/api/hosted/me",
        "/setup",
        "/join/K7QF-M2XD-9PNA",
        "/signin",
    ] {
        assert_eq!(ts.get(path).await.status(), 404, "{path}");
    }
    let r = ts
        .post_json("/api/hosted/setup", json!({"secret": "x", "name": "Ada"}))
        .await;
    assert_eq!(r.status(), 404);
    // This machine's browser still gets the token.
    assert_eq!(ts.get("/api/token").await.status(), 200);
}

#[tokio::test]
async fn setup_makes_the_owner_once_with_the_secret_and_sets_the_cookie() {
    let site = Site::new().await;
    let anon = Browser::default();
    let (s, me) = site.get(&anon, "/api/hosted/me").await;
    assert_eq!(
        (s, me["setup_open"].as_bool(), me["signed_in"].as_bool()),
        (200, Some(true), Some(false))
    );
    assert_eq!(me["origin"], json!(site.origin));
    for wrong in [
        json!({"secret": "nope", "name": "Ada"}),
        json!({"secret": "", "name": "Ada"}),
    ] {
        let (s, v) = site.post(&anon, "/api/hosted/setup", wrong).await;
        assert_eq!((s, code(&v)), (404, "not_found"));
    }
    let (s, _) = site
        .post(&anon, "/api/hosted/setup", json!({"name": "Ada"}))
        .await;
    assert_eq!(s, 400);
    let res = site
        .req(&anon, reqwest::Method::POST, "/api/hosted/setup")
        .json(&json!({"secret": SECRET, "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let cookie = res.headers()[SET_COOKIE].to_str().unwrap().to_string();
    // On http://localhost the cookie goes without Secure; on https it has it
    // (hosted::tests::the_cookie_is_http_only_lax_and_secure_on_https).
    assert!(cookie.starts_with("clax_session=cxs_"), "{cookie}");
    assert!(
        cookie.ends_with("; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax"),
        "{cookie}"
    );
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (
            v["person"]["display_name"].as_str(),
            v["person"]["owner"].as_bool()
        ),
        (Some("Ada"), Some(true))
    );
    // Once only, and the page is gone.
    let (s, _) = site
        .post(
            &anon,
            "/api/hosted/setup",
            json!({"secret": SECRET, "name": "Eve"}),
        )
        .await;
    assert_eq!(s, 404);
    let owner = Browser {
        cookie: Some(
            cookie
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("clax_session=")
                .into(),
        ),
    };
    let (_, me) = site.get(&owner, "/api/hosted/me").await;
    assert_eq!(
        (me["setup_open"].as_bool(), me["person"]["owner"].as_bool()),
        (Some(false), Some(true))
    );
    // The owner's browser is the owner: People is theirs.
    let (s, v) = site.get(&owner, "/api/people/me").await;
    assert_eq!((s, v["person"]["owner"].as_bool()), (200, Some(true)));
}

#[tokio::test]
async fn setup_stays_closed_without_a_secret() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, None).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let r = ts
        .client
        .post(format!("{}/api/hosted/setup", ts.base))
        .header(ORIGIN, &origin)
        .json(&json!({"secret": "", "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(
        ts.get("/api/hosted/me")
            .await
            .json::<Value>()
            .await
            .unwrap()["setup_open"],
        json!(false)
    );
}

#[tokio::test]
async fn nothing_is_from_this_machine_and_only_the_public_host_answers() {
    let site = Site::new().await;
    let anon = Browser::default();
    // The token is never handed out (the owner's browser gets a shell
    // token instead), nor the browser link of `clax connect`.
    let (s, v) = site.get(&anon, "/api/token").await;
    assert_eq!((s, code(&v)), (403, "not_owner"));
    let (s, _) = site.get(&anon, "/connect/b/abc").await;
    assert_eq!(s, 404);
    // The owner cookie of a local daemon, and a LAN viewer's cookie, count
    // for nothing: the request is anonymous, and the gallery not theirs.
    let owner_cookie = site.ts.owner_cookie();
    let lan = clax_core::secret::viewer_cookie();
    site.ts
        .store
        .upsert_viewer(lan.expose(), Some("Lan"))
        .unwrap();
    for c in [owner_cookie, format!("clax_viewer={}", lan.expose())] {
        let r = site
            .ts
            .client
            .get(site.url("/api/artifacts"))
            .header(COOKIE, c.clone())
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!((s, code(&v)), (401, "sign_in_required"), "{c}");
    }
    // Another host is refused before any route.
    let r = site
        .ts
        .client
        .get(site.url("/api/hosted/me"))
        .header(HOST, "evil.example")
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!((s, code(&v)), (403, "forbidden_host"));
    // The CLI on the machine still speaks with the token.
    assert_eq!(site.ts.get_authed("/api/artifacts").await.status(), 200);
}

#[tokio::test]
async fn every_browser_change_must_come_from_the_public_origin() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let path = format!("/api/artifacts/{aid}/threads");
    let form = || {
        reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", "hello")
            .text("version", "1")
    };
    let cookie = format!("clax_session={}", owner.cookie.as_deref().unwrap());
    for origin in [
        None,
        Some("http://evil.example"),
        Some("null"),
        Some("https://localhost"),
    ] {
        let mut r = site
            .ts
            .client
            .post(site.url(&path))
            .header(COOKIE, &cookie)
            .multipart(form());
        if let Some(o) = origin {
            r = r.header(ORIGIN, o);
        }
        let (s, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{origin:?}");
    }
    for path in [
        "/api/hosted/signout",
        "/api/hosted/signin-links",
        "/api/hosted/agent-tokens",
    ] {
        let r = site
            .ts
            .client
            .post(site.url(path))
            .header(COOKIE, &cookie)
            .json(&json!({}));
        let (s, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{path}");
    }
    let (s, v) = site.thread(&owner, &aid, "hello").await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn an_invite_joins_once_in_the_browser_as_a_person() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (s, v) = site
        .post(&owner, "/api/people/invites", json!({"name": "Sam"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let url = v["invite"]["url"].as_str().unwrap().to_string();
    assert_eq!(v["invite"]["command"].as_str(), Some(url.as_str()));
    assert!(url.starts_with(&format!("{}/join/", site.origin)), "{url}");
    let code_ = url.rsplit('/').next().unwrap().to_string();
    let anon = Browser::default();
    let (s, peek) = site
        .get(&anon, &format!("/api/hosted/invites/{code_}"))
        .await;
    assert_eq!((s, peek["name"].as_str()), (200, Some("Sam")));
    // Only the owner invites: a person or no one may not.
    let (s, _) = site
        .post(&anon, "/api/people/invites", json!({"name": "Wes"}))
        .await;
    assert_eq!(s, 401);
    let mut sam = Browser::default();
    let v = site
        .sign_in(
            &mut sam,
            &format!("/api/hosted/invites/{code_}/accept"),
            json!({}),
        )
        .await;
    assert_eq!(
        (
            v["person"]["display_name"].as_str(),
            v["person"]["owner"].as_bool()
        ),
        (Some("Sam"), Some(false))
    );
    let (s, v) = site
        .post(
            &anon,
            &format!("/api/hosted/invites/{code_}/accept"),
            json!({}),
        )
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"));
    let (s, _) = site
        .get(&anon, &format!("/api/hosted/invites/{code_}"))
        .await;
    assert_eq!(s, 404);
    let (s, me) = site.get(&sam, "/api/people/me").await;
    assert_eq!(
        (
            s,
            me["person"]["owner"].as_bool(),
            me["person"]["display_name"].as_str()
        ),
        (200, Some(false), Some("Sam"))
    );
    let (s, v) = site
        .post(&sam, "/api/people/invites", json!({"name": "Wes"}))
        .await;
    assert_eq!((s, code(&v)), (403, "browser_required"));
    // An invite without rights gives Sam their own space: the owner's
    // artifact is not in Sam's gallery until it is shared with Sam.
    site.artifact().await;
    let (s, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!((s, v["artifacts"].as_array().map(Vec::len)), (200, Some(0)));
    // The owner revokes Sam: Sam's browser is signed out.
    let pid = me["person"]["public_id"].as_str().unwrap();
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/people/{pid}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn a_signin_link_signs_in_another_browser_once_within_ten_minutes() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let anon = Browser::default();
    let (s, v) = site
        .post(&anon, "/api/hosted/signin-links", json!({}))
        .await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"));
    let (s, v) = site.post(&sam, "/api/hosted/signin-links", json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["expires_at"], "2026-10-12T10:10:00.000Z");
    let url = v["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("{}/signin/cxl_", site.origin)),
        "{url}"
    );
    let code_ = url.rsplit('/').next().unwrap().to_string();
    let accept = format!("/api/hosted/signin-links/{code_}/accept");
    let mut phone = Browser::default();
    let v = site.sign_in(&mut phone, &accept, json!({})).await;
    assert_eq!(v["person"]["display_name"].as_str(), Some("Sam"));
    let (s, me) = site.get(&phone, "/api/hosted/me").await;
    assert_eq!(
        (s, me["person"]["display_name"].as_str()),
        (200, Some("Sam"))
    );
    let (s, v) = site.post(&anon, &accept, json!({})).await;
    assert_eq!((s, code(&v)), (404, "not_found"));
    // Ten minutes after it is made, a link no longer signs anyone in.
    let (_, v) = site
        .post(&owner, "/api/hosted/signin-links", json!({}))
        .await;
    let late = v["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    site.clock.advance(10 * 60);
    let (s, _) = site
        .post(
            &anon,
            &format!("/api/hosted/signin-links/{late}/accept"),
            json!({}),
        )
        .await;
    assert_eq!(s, 404);
    // A session lasts thirty days from sign-in.
    site.clock.advance(30 * 24 * 3600 - 10 * 60 - 1);
    assert_eq!(site.get(&phone, "/api/artifacts").await.0, 200);
    site.clock.advance(1);
    assert_eq!(site.get(&phone, "/api/artifacts").await.0, 401);
}

#[tokio::test]
async fn a_sign_in_link_says_whose_account_it_is_and_replaces_a_session_only_when_confirmed() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let (_, v) = site.post(&sam, "/api/hosted/signin-links", json!({})).await;
    let code_ = v["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let peek = format!("/api/hosted/signin-links/{code_}");
    let accept = format!("{peek}/accept");
    // The peek names the account and spends nothing, for anyone.
    for _ in 0..2 {
        let (s, v) = site.get(&Browser::default(), &peek).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["name"].as_str(), Some("Sam"));
        assert_eq!(v["owner"], json!(false));
        assert_eq!(v["expires_at"], "2026-10-12T10:10:00.000Z");
        assert!(v["public_id"].as_str().is_some_and(|p| !p.is_empty()));
    }
    let (s, v) = site
        .get(&Browser::default(), "/api/hosted/signin-links/cxl_nope")
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"));
    // The owner's browser is not signed in as Sam by the link alone.
    let (s, v) = site.post(&owner, &accept, json!({})).await;
    assert_eq!((s, code(&v)), (409, "session_exists"), "{v}");
    assert_eq!(v["error"]["current"]["owner"], json!(true));
    let (s, me) = site.get(&owner, "/api/hosted/me").await;
    assert_eq!((s, me["person"]["owner"].clone()), (200, json!(true)));
    let (s, v) = site.get(&Browser::default(), &peek).await;
    assert_eq!(s, 200, "the refused redemption spent nothing: {v}");
    let (s, v) = site.post(&owner, &accept, json!({"nope": 1})).await;
    assert_eq!((s, code(&v)), (400, "invalid_json"), "{v}");
    // Sam's own browser redeems Sam's link without confirming.
    let mut again = sam.clone();
    let v = site.sign_in(&mut again, &accept, json!({})).await;
    assert_eq!(v["person"]["display_name"].as_str(), Some("Sam"));
    // Confirmed, the redemption replaces the session.
    let (_, v) = site
        .post(&again, "/api/hosted/signin-links", json!({}))
        .await;
    let code2 = v["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let mut swapped = owner.clone();
    let v = site
        .sign_in(
            &mut swapped,
            &format!("/api/hosted/signin-links/{code2}/accept"),
            json!({"replace": true}),
        )
        .await;
    assert_eq!(v["person"]["display_name"].as_str(), Some("Sam"));
    assert_eq!(
        site.get(&owner, "/api/hosted/me").await.1["signed_in"],
        json!(false)
    );
}

#[tokio::test]
async fn an_invite_replaces_a_browsers_session_only_when_confirmed() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (s, v) = site
        .post(&owner, "/api/people/invites", json!({"name": "Rae"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let code_ = v["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let accept = format!("/api/hosted/invites/{code_}/accept");
    let (s, v) = site.post(&owner, &accept, json!({})).await;
    assert_eq!((s, code(&v)), (409, "session_exists"), "{v}");
    let (s, v) = site
        .get(&Browser::default(), &format!("/api/hosted/invites/{code_}"))
        .await;
    assert_eq!(
        (s, v["name"].as_str()),
        (200, Some("Rae")),
        "the invite is still live"
    );
    let mut b = owner.clone();
    let v = site
        .sign_in(&mut b, &accept, json!({"replace": true}))
        .await;
    assert_eq!(v["person"]["display_name"].as_str(), Some("Rae"));
}

#[tokio::test]
async fn signing_out_ends_the_session_and_clears_the_cookie() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let res = site
        .req(&owner, reqwest::Method::POST, "/api/hosted/signout")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(
        res.headers()[SET_COOKIE],
        "clax_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
    );
    let (_, me) = site.get(&owner, "/api/hosted/me").await;
    assert_eq!(me["signed_in"].as_bool(), Some(false));
    let (s, _) = site.get(&owner, "/api/people/me").await;
    assert_eq!(s, 401);
    // Signing out without a session is still 204.
    let (s, _) = site
        .post(&Browser::default(), "/api/hosted/signout", json!({}))
        .await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn a_member_mints_agent_tokens_with_their_rights_and_revokes_them() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let (s, v) = site
        .post(
            &sam,
            "/api/hosted/agent-tokens",
            json!({"label": "sam-laptop"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let token = v["token"].as_str().unwrap();
    assert!(token.starts_with("cxa_"));
    let mcp = format!("{}/mcp/json", site.origin);
    assert_eq!(v["mcp_url"], json!(mcp));
    assert_eq!(
        v["claude_command"].as_str().unwrap(),
        format!(
            "claude mcp add --transport http clax {mcp} --header \"Authorization: Bearer {token}\""
        )
    );
    assert!(
        v["codex_config"]
            .as_str()
            .unwrap()
            .contains(&format!("url = \"{mcp}\""))
    );
    let gid = v["id"].as_str().unwrap().to_string();
    // The token's grant has Sam's rights.
    assert_eq!(
        site.ts.store.bearer_grant(token).unwrap().as_deref(),
        Some(gid.as_str())
    );
    let gc = site.ts.store.grant_caller(&gid, None).unwrap();
    let (_, me) = site.get(&sam, "/api/people/me").await;
    assert_eq!(gc.person.as_deref(), me["person"]["public_id"].as_str());
    assert_eq!(gc.rights.to_json(), me["person"]["rights"]);
    // It lists without its token, and People shows it as a bearer machine.
    let (s, list) = site.get(&sam, "/api/hosted/agent-tokens").await;
    assert_eq!(
        (s, list["tokens"][0]["label"].as_str()),
        (200, Some("sam-laptop"))
    );
    assert!(!list.to_string().contains(token));
    assert_eq!(me["grants"][0]["bearer"], json!(true));
    assert_eq!(me["grants"][0]["client"]["key"], Value::Null);
    // Not the owner's to revoke here, nor anyone else's.
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/agent-tokens/{gid}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/hosted/agent-tokens/{gid}"),
            json!(null),
        )
        .await;
    assert_eq!((s, v["token"]["status"].as_str()), (200, Some("revoked")));
    assert_eq!(site.ts.store.bearer_grant(token).unwrap(), None);
    // A browser route: the daemon token, or no session, may not mint.
    let r = site
        .ts
        .authed(site.ts.client.post(site.url("/api/hosted/agent-tokens")))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!((s, code(&v)), (403, "browser_required"));
    let (s, _) = site
        .post(&Browser::default(), "/api/hosted/agent-tokens", json!({}))
        .await;
    assert_eq!(s, 401);
    // The owner mints too.
    let (s, _) = site
        .post(&owner, "/api/hosted/agent-tokens", json!({}))
        .await;
    assert_eq!(s, 201);
}

#[tokio::test]
async fn a_page_shared_by_link_takes_guests_up_to_commenting() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let aid = site.artifact().await;
    let other = site.artifact().await;
    let anon = Browser::default();
    // Unshared: nothing of it, and one answer whether or not it exists.
    let (s, v) = site.get(&anon, &format!("/api/artifacts/{aid}")).await;
    assert_eq!((s, code(&v)), (404, "not_found"));
    let (s, _) = site.get(&anon, "/api/artifacts/zzzzzzzzzzzz").await;
    assert_eq!(s, 404);
    // Only the owner or the page's maker shares it.
    let link = format!("/api/hosted/artifacts/{aid}/link");
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &link,
            json!({"access": "commenter"}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"));
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &link,
            json!({"access": "editor"}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"));
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &link,
            json!({"access": "commenter"}),
        )
        .await;
    assert_eq!(
        (s, v["url"].as_str()),
        (200, Some(format!("{}/a/{aid}", site.origin).as_str()))
    );
    let (_, links) = site.get(&owner, "/api/hosted/links").await;
    assert_eq!(links["links"][0]["access"], "commenter");
    // A link is its page's capability: a person is listed only the links
    // they could have made (of pages they made), not the owner's.
    let (_, links) = site.get(&sam, "/api/hosted/links").await;
    assert_eq!(links["links"], json!([]));
    // Anyone with the link reads it, and nothing else.
    for path in [
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts?artifact={aid}"),
        format!("/api/artifacts/{aid}/versions"),
        format!("/api/artifacts/{aid}/threads"),
    ] {
        assert_eq!(site.get(&anon, &path).await.0, 200, "{path}");
    }
    for path in [
        "/api/artifacts".to_string(),
        format!("/api/artifacts/{other}"),
        "/api/people/me".into(),
        "/api/events".into(),
    ] {
        assert!(
            matches!(site.get(&anon, &path).await.0, 401 | 404),
            "{path}"
        );
    }
    let r = site
        .req(&anon, reqwest::Method::GET, "/")
        .send()
        .await
        .unwrap();
    assert_eq!(r.url().path(), "/signin");
    // Commenting takes a name first; naming makes a guest.
    let (s, v) = site.thread(&anon, &aid, "hi").await;
    assert_eq!((s, code(&v)), (401, "guest_name_required"));
    // No session: a viewer of no public ID and no name, and none minted.
    let (s, v) = site.get(&anon, "/api/viewers/me").await;
    assert_eq!(
        (s, v["viewer"].clone()),
        (
            200,
            json!({"public_id": "", "display_name": null, "created_at": ""})
        )
    );
    let (s, v) = site
        .send(
            &anon,
            reqwest::Method::PUT,
            "/api/viewers/me",
            json!({"display_name": "Ada"}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "name_taken"));
    let mut mia = Browser::default();
    let res = site
        .req(&mia, reqwest::Method::PUT, "/api/viewers/me")
        .json(&json!({"display_name": "Mia"}))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()[SET_COOKIE].to_str().unwrap().to_string();
    let (s, v) = json_of(res).await;
    assert_eq!(
        (s, v["viewer"]["display_name"].as_str()),
        (200, Some("Mia"))
    );
    mia.cookie = Some(
        cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("clax_session=")
            .into(),
    );
    let (_, me) = site.get(&mia, "/api/hosted/me").await;
    assert_eq!(me["person"]["guest"], json!(true));
    let (s, v) = site.thread(&mia, &aid, "a guest's comment").await;
    assert_eq!(s, 201, "{v}");
    let tid = v["thread"]["id"].as_str().unwrap().to_string();
    assert!(v.to_string().contains("Mia"), "{v}");
    let (s, _) = site
        .post(
            &mia,
            &format!("/api/artifacts/{aid}/threads/{tid}/comments"),
            json!({"body": "more"}),
        )
        .await;
    assert_eq!(s, 201);
    // A guest is capped at commenting: no resolving, renaming, publishing,
    // and nothing of another page or of members.
    let (s, v) = site
        .post(
            &mia,
            &format!("/api/artifacts/{aid}/threads/{tid}/resolve"),
            json!({}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"));
    let (s, _) = site
        .send(
            &mia,
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"title": "x"}),
        )
        .await;
    assert_eq!(s, 401);
    let (s, _) = site.thread(&mia, &other, "elsewhere").await;
    assert_eq!(s, 404);
    for path in ["/api/hosted/agent-tokens", "/api/hosted/signin-links"] {
        let (s, v) = site.post(&mia, path, json!({})).await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{path}");
    }
    let (s, _) = site.get(&mia, "/api/artifacts").await;
    assert_eq!(s, 401);
    // A guest renames within the guests' rules.
    let (s, v) = site
        .send(
            &mia,
            reqwest::Method::PUT,
            "/api/viewers/me",
            json!({"display_name": "Sam"}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "name_taken"));
    let (s, v) = site
        .send(
            &mia,
            reqwest::Method::PUT,
            "/api/viewers/me",
            json!({"display_name": "Mia R"}),
        )
        .await;
    assert_eq!(
        (s, v["viewer"]["display_name"].as_str()),
        (200, Some("Mia R"))
    );
    // A view-only link reads but takes no comment.
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &link,
            json!({"access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = site.thread(&mia, &aid, "no").await;
    assert_eq!((s, code(&v)), (403, "not_permitted"));
    let (s, v) = site.thread(&anon, &aid, "no").await;
    assert_eq!((s, code(&v)), (403, "not_permitted"));
    assert_eq!(
        site.get(&mia, &format!("/api/artifacts/{aid}/threads"))
            .await
            .0,
        200
    );
    // Unshared again: gone for guests.
    let (s, _) = site
        .send(&owner, reqwest::Method::PUT, &link, json!({"access": null}))
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        site.get(&mia, &format!("/api/artifacts/{aid}")).await.0,
        404
    );
}

#[tokio::test]
async fn a_guests_stream_holds_only_shared_pages_topics() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let other = site.artifact().await;
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{aid}/link"),
        json!({"access": "viewer"}),
    )
    .await;
    let anon = Browser::default();
    let res = site
        .req(&anon, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut events = clax_testkit::EventReader::from_response(res);
    let (name, ready) = events.next().await;
    assert_eq!(name, "ready");
    let sid = ready["stream"].as_str().unwrap().to_string();
    let sub = |topics: Vec<String>| json!({"subscribe": topics});
    let (s, v) = site
        .post(
            &anon,
            &format!("/api/stream/{sid}"),
            sub(vec![format!("artifact:{aid}")]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    for t in [
        "gallery".to_string(),
        format!("artifact:{other}"),
        "people".into(),
        format!("docs:{aid}"),
    ] {
        let (s, _) = site
            .post(&anon, &format!("/api/stream/{sid}"), sub(vec![t.clone()]))
            .await;
        assert!(matches!(s, 403 | 404), "{t}: {s}");
    }
}

#[tokio::test]
async fn content_is_served_only_at_its_key_and_an_id_alone_reads_nothing() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let anon = Browser::default();
    let port = site.ts.addr.port();
    let status = |b: &Browser, p: String, host: Option<String>| {
        let mut r = site
            .ts
            .client
            .get(site.url(&p))
            .header(HOST, host.unwrap_or(format!("localhost:{port}")));
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        async move { r.send().await.unwrap() }
    };
    let content_host = Some(format!("{aid}.localhost:{port}"));
    // By its ID alone, nothing: not on the public host, not on its content
    // host, not with a member's session, not the API's raw file.
    for b in [&anon, &owner] {
        for p in [
            format!("/c/{aid}/v/1/"),
            format!("/c/{aid}/v/1/index.html"),
            format!("/c/{aid}/v/1"),
            format!("/c/{aid}/v/1/k/00000000000000000000000000000000/"),
        ] {
            assert_eq!(status(b, p.clone(), None).await.status(), 404, "{p}");
        }
        let r = status(b, "/v/1/".into(), content_host.clone()).await;
        assert_eq!(r.status(), 404);
    }
    let (s, v) = site
        .get(
            &anon,
            &format!("/api/artifacts/{aid}/versions/1/files/index.html"),
        )
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"));
    // Who may read the artifact is handed its versions' keys.
    let (s, v) = site.get(&owner, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(s, 200, "{v}");
    let key = v["versions"][0]["content_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(key.len(), 32);
    let (_, vs) = site
        .get(&owner, &format!("/api/artifacts/{aid}/versions"))
        .await;
    assert_eq!(vs["versions"][0]["content_key"], json!(key));
    // At the key, the content, with no referrer (its URL is the key).
    let r = status(&anon, format!("/c/{aid}/v/1/k/{key}/"), None).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert!(r.text().await.unwrap().contains("<h1>Plan</h1>"));
    let r = status(&anon, format!("/v/1/k/{key}/"), content_host.clone()).await;
    assert_eq!(r.status(), 200);
    let r = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(site.url(&format!("/c/{aid}/v/1/k/{key}")))
        .header(HOST, format!("localhost:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 308);
    assert_eq!(r.headers()["location"], format!("{key}/"));
    // Another version's address, a version in another form, or a key cut
    // short reads nothing.
    let (s, _) = site
        .ts
        .authed(site.ts.client.post(site.url(&format!("/api/artifacts/{aid}/versions"))))
        .header(HOST, format!("localhost:{port}"))
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<h1>Two</h1>", "encoding": "utf8"}}}))
        .send()
        .await
        .map(|r| (r.status().as_u16(), ()))
        .unwrap();
    assert_eq!(s, 201);
    for p in [
        format!("/c/{aid}/v/2/k/{key}/"),
        format!("/c/{aid}/v/01/k/{key}/"),
        format!("/c/{aid}/v/1/k/{}/", &key[..31]),
        format!("/c/{aid}/v/1/k/{}/", key.to_uppercase()),
    ] {
        assert_eq!(status(&anon, p.clone(), None).await.status(), 404, "{p}");
    }
    // The artifact page frames the latest version at its key.
    let page = site
        .ts
        .client
        .get(site.url(&format!("/a/{aid}")))
        .header(HOST, format!("localhost:{port}"))
        .header(
            COOKIE,
            format!(
                "clax_session={}; clax_frame=sandbox",
                owner.cookie.as_deref().unwrap()
            ),
        )
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (_, v2) = site
        .get(&owner, &format!("/api/artifacts/{aid}/versions/2"))
        .await;
    let key2 = v2["version"]["content_key"].as_str().unwrap();
    assert_ne!(key2, key);
    assert!(
        page.contains(&format!("src=\"/c/{aid}/v/2/k/{key2}/\"")),
        "{page}"
    );
    assert!(page.contains(&format!("\"content_key\":\"{key2}\"")));
    // An asset: by its ID nothing, at its keyed URL its bytes.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
            .file_name("a.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let r = site
        .ts
        .authed(
            site.ts
                .client
                .post(site.url(&format!("/api/artifacts/{aid}/assets"))),
        )
        .header(HOST, format!("localhost:{port}"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let (s, up) = json_of(r).await;
    assert_eq!(s, 201, "{up}");
    let asset = up["asset"]["id"].as_str().unwrap().to_string();
    let url = up["url"].as_str().unwrap().to_string();
    assert!(url.starts_with(&format!("/_blob/{asset}?k=")), "{url}");
    let (_, listed) = site
        .get(&owner, &format!("/api/artifacts/{aid}/assets"))
        .await;
    assert_eq!(listed["assets"][0]["url"], json!(url));
    for p in [
        format!("/_blob/{asset}"),
        format!("/_blob/{asset}?k=0"),
        format!("{url}0"),
    ] {
        assert_eq!(status(&anon, p.clone(), None).await.status(), 404, "{p}");
    }
    let r = status(&anon, url.clone(), None).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert_eq!(status(&anon, url, content_host).await.status(), 200);
    assert_eq!(
        site.req(&anon, reqwest::Method::GET, "/healthz")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn an_authorization_that_is_no_credential_reaches_nothing() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let (s, _) = site.thread(&owner, &aid, "private words").await;
    assert_eq!(s, 201);
    let paths = [
        "/".to_string(),
        "/api/artifacts".to_string(),
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/threads"),
        format!("/api/artifacts/{aid}/versions/1/files/index.html"),
    ];
    for p in &paths {
        for auth in ["Bearer junk", "Basic eDp5", "Bearer cxa_unknown", "x"] {
            let r = site
                .req(&Browser::default(), reqwest::Method::GET, p)
                .header("authorization", auth)
                .send()
                .await
                .unwrap();
            let (s, v) = json_of(r).await;
            // An agent token no grant holds is refused as an agent
            // credential.
            let want = if auth.starts_with("Bearer cxa_") {
                "invalid_token"
            } else {
                "unauthorized"
            };
            assert_eq!((s, code(&v)), (401, want), "{p} [{auth}]: {v}");
        }
        // Beside a member's session cookie too: it never borrows the session.
        let r = site
            .req(&owner, reqwest::Method::GET, p)
            .header("authorization", "Bearer junk")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
    // An unverifiable agent credential is refused as before.
    let r = site
        .req(&Browser::default(), reqwest::Method::GET, "/api/artifacts")
        .header("authorization", "Clax-Grant x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // The daemon token still speaks for the owner.
    assert_eq!(site.ts.get_authed("/api/artifacts").await.status(), 200);
}

#[tokio::test]
async fn the_owners_browser_holds_a_shell_token_that_stands_for_the_daemon_token() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let aid = site.artifact().await;
    // Only the owner's browser gets a token: its session's shell token.
    let (s, v) = site.get(&owner, "/api/token").await;
    assert_eq!(s, 200, "{v}");
    let shell = v["token"].as_str().unwrap().to_string();
    assert!(shell.starts_with("cxo_"), "{shell}");
    assert_ne!(shell, site.ts.token);
    for b in [&sam, &Browser::default()] {
        let (s, v) = site.get(b, "/api/token").await;
        assert_eq!((s, code(&v)), (403, "not_owner"));
    }
    let with = |b: &Browser, m: reqwest::Method, p: &str, t: &str| {
        site.req(b, m, p)
            .header("authorization", format!("Bearer {t}"))
    };
    // The owner's surfaces that take the token answer the owner's browser.
    for p in [
        "/api/questions",
        "/api/inbox",
        "/api/people",
        "/api/threads",
    ] {
        let r = with(&owner, reqwest::Method::GET, p, &shell)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{p}");
    }
    let r = with(
        &owner,
        reqwest::Method::PATCH,
        &format!("/api/artifacts/{aid}"),
        &shell,
    )
    .json(&json!({"title": "Renamed", "pinned": true}))
    .send()
    .await
    .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["artifact"]["title"].as_str(),
            v["artifact"]["pinned"].as_bool()
        ),
        (Some("Renamed"), Some(true))
    );
    // Without the owner's session cookie beside it, or beside another
    // browser's, it is no credential; nor is a change from another origin.
    for b in [&Browser::default(), &sam] {
        let r = with(b, reqwest::Method::GET, "/api/questions", &shell)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let mut other = Browser::default();
    let (_, link) = site
        .post(&owner, "/api/hosted/signin-links", json!({}))
        .await;
    let code_ = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    site.sign_in(
        &mut other,
        &format!("/api/hosted/signin-links/{code_}/accept"),
        json!({}),
    )
    .await;
    let r = with(&other, reqwest::Method::GET, "/api/questions", &shell)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = site
        .ts
        .client
        .delete(site.url(&format!("/api/artifacts/{aid}")))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .header(
            COOKIE,
            format!("clax_session={}", owner.cookie.as_deref().unwrap()),
        )
        .header("authorization", format!("Bearer {shell}"))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"));
    // The session's cookie alone still is no token.
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/artifacts/{aid}"),
            json!({}),
        )
        .await;
    assert_eq!(s, 401);
    let r = with(
        &owner,
        reqwest::Method::DELETE,
        &format!("/api/artifacts/{aid}"),
        &shell,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 204);
    // Signing out ends it.
    let (s, _) = site.post(&owner, "/api/hosted/signout", json!({})).await;
    assert_eq!(s, 204);
    let r = with(&owner, reqwest::Method::GET, "/api/questions", &shell)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

impl Site {
    /// A person invited with `rights` and joined in a new browser, with
    /// their public ID.
    async fn person_with(&self, owner: &Browser, name: &str, rights: Value) -> (Browser, String) {
        let (s, v) = self
            .post(
                owner,
                "/api/people/invites",
                json!({"name": name, "rights": rights}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let url = v["invite"]["url"].as_str().unwrap();
        let code = url.rsplit('/').next().unwrap();
        let mut b = Browser::default();
        let me = self
            .sign_in(
                &mut b,
                &format!("/api/hosted/invites/{code}/accept"),
                json!({}),
            )
            .await;
        let pid = me["person"]["public_id"].as_str().unwrap().to_string();
        (b, pid)
    }

    async fn titled(&self, title: &str) -> String {
        self.ts
            .publish(title, &[("index.html", "<h1>x</h1>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn listed(v: &Value) -> Vec<String> {
    let mut ids: Vec<String> = v["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_persons_rights_limit_their_browser_and_the_owner_sees_everything() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read and comment on Budget alone; Mia holds the default.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
                {"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": [budget]},
            ]),
        )
        .await;
    let mia = site.person(&owner, "Mia").await;
    let mut both = vec![plan.clone(), budget.clone()];
    both.sort();
    // Listings filter.
    let (_, v) = site.get(&owner, "/api/artifacts").await;
    assert_eq!(listed(&v), both);
    let (_, v) = site.get(&mia, "/api/artifacts").await;
    assert_eq!(listed(&v), both);
    let (_, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(listed(&v), vec![budget.clone()]);
    let (_, v) = site
        .get(&sam, &format!("/api/artifacts?artifact={plan}"))
        .await;
    assert_eq!(listed(&v), Vec::<String>::new());
    // Routes refuse what Sam may not read as if it were not there.
    for p in [
        format!("/a/{plan}"),
        format!("/api/artifacts/{plan}"),
        format!("/api/artifacts/{plan}/versions"),
        format!("/api/artifacts/{plan}/versions/1/files/index.html"),
        format!("/api/artifacts/{plan}/threads"),
        format!("/api/artifacts/{plan}/assets"),
        format!("/api/viewers/me/attention?artifact={plan}"),
    ] {
        let (s, v) = site.get(&sam, &p).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{p}: {v}");
        let (s, _) = site.get(&mia, &p).await;
        assert_eq!(s, 200, "Mia: {p}");
    }
    let (s, _) = site.thread(&sam, &plan, "let me in").await;
    assert_eq!(s, 404);
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            "/api/viewers/me/seen",
            json!({"artifact_id": plan, "version": 1}),
        )
        .await;
    assert_eq!(s, 404);
    // Within its rights, the route's own action is checked too: Sam
    // comments on Budget, and may not resolve there.
    let (s, t) = site.thread(&sam, &budget, "on budget").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap();
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/artifacts/{budget}/threads/{tid}/resolve"),
            json!({}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    let (s, _) = site
        .post(
            &mia,
            &format!("/api/artifacts/{budget}/threads/{tid}/resolve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200);
    // The stream: no topic of Plan's, and the gallery's (every artifact's
    // titles) left out; the daemon-wide event stream is the owner's.
    let res = site
        .req(&sam, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    let mut events = clax_testkit::EventReader::from_response(res);
    let (_, ready) = events.next().await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let (s, _) = site
        .post(
            &sam,
            &format!("/api/stream/{sid}"),
            json!({"subscribe": [format!("artifact:{plan}")]}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/stream/{sid}"),
            json!({"subscribe": ["gallery", format!("artifact:{budget}")]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["topics"], json!([format!("artifact:{budget}")]));
    let (s, _) = site.get(&sam, "/api/events").await;
    assert_eq!(s, 403);
    // A page shared by link opens to Sam as to anyone with its link, at
    // its access; the listing still holds what Sam's rights read.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{plan}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    let (s, _) = site.get(&sam, &format!("/api/artifacts/{plan}")).await;
    assert_eq!(s, 200);
    let (s, _) = site.thread(&sam, &plan, "by link").await;
    assert_eq!(s, 201);
    let (_, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(listed(&v), vec![budget.clone()]);
}

#[tokio::test]
async fn setup_sign_in_and_guests_are_limited_per_client_on_the_clock() {
    let site = Site::new().await;
    let anon = Browser::default();
    // Setup: five guesses, then 429 until two minutes give one back.
    for _ in 0..5 {
        let (s, _) = site
            .post(
                &anon,
                "/api/hosted/setup",
                json!({"secret": "guess", "name": "Eve"}),
            )
            .await;
        assert_eq!(s, 404);
    }
    let (s, v) = site
        .post(
            &anon,
            "/api/hosted/setup",
            json!({"secret": SECRET, "name": "Ada"}),
        )
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
    assert_eq!(v["error"]["retry_after_s"], json!(120));
    // A forged CF-Connecting-IP is not trusted without Cloudflare in front.
    let r = site
        .req(&anon, reqwest::Method::POST, "/api/hosted/setup")
        .header("cf-connecting-ip", "198.51.100.7")
        .json(&json!({"secret": SECRET, "name": "Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    site.clock.advance(120);
    let mut owner = Browser::default();
    site.sign_in(
        &mut owner,
        "/api/hosted/setup",
        json!({"secret": SECRET, "name": "Ada"}),
    )
    .await;
    // Codes: ten looks or redemptions of invites and sign-in links, then 429.
    for i in 0..10 {
        let p = if i % 2 == 0 {
            "/api/hosted/invites/AAAA-BBBB-CCCC".to_string()
        } else {
            "/api/hosted/signin-links/cxl_nothing/accept".to_string()
        };
        let (s, _) = if i % 2 == 0 {
            site.get(&anon, &p).await
        } else {
            site.post(&anon, &p, json!({})).await
        };
        assert_eq!(s, 404, "{p}");
    }
    let (s, v) = site
        .post(
            &anon,
            "/api/hosted/invites/AAAA-BBBB-CCCC/accept",
            json!({}),
        )
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"));
    site.clock.advance(30);
    let (s, _) = site.get(&anon, "/api/hosted/invites/AAAA-BBBB-CCCC").await;
    assert_eq!(s, 404);
    // Guests: five names, then 429.
    let aid = site.artifact().await;
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{aid}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    for i in 0..5 {
        let (s, v) = site
            .send(
                &anon,
                reqwest::Method::PUT,
                "/api/viewers/me",
                json!({"display_name": format!("Guest {i}")}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    let (s, v) = site
        .send(
            &anon,
            reqwest::Method::PUT,
            "/api/viewers/me",
            json!({"display_name": "Guest 6"}),
        )
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"));
}

#[tokio::test]
async fn sign_ins_and_sign_outs_are_in_the_audit_journal() {
    let site = Site::new().await;
    let from = site.ts.store.newest_seq().unwrap();
    let mut owner = site.owner().await;
    let _sam = site.person(&owner, "Sam").await;
    let (_, link) = site
        .post(&owner, "/api/hosted/signin-links", json!({}))
        .await;
    let c = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let mut phone = Browser::default();
    site.sign_in(
        &mut phone,
        &format!("/api/hosted/signin-links/{c}/accept"),
        json!({}),
    )
    .await;
    let aid = site.artifact().await;
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{aid}/link"),
        json!({"access": "viewer"}),
    )
    .await;
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Mia"}))
        .await;
    let (s, _) = site.post(&phone, "/api/hosted/signout", json!({})).await;
    assert_eq!(s, 204);
    let (s, _) = site
        .post(
            &owner,
            "/api/hosted/agent-tokens",
            json!({"label": "laptop"}),
        )
        .await;
    assert_eq!(s, 201);
    owner.cookie = None;
    let rows = site.ts.store.events_after(from, 100).unwrap();
    let starts: Vec<(String, String, String)> = rows
        .iter()
        .filter(|r| r.kind == "session.start")
        .map(|r| {
            let body: Value = serde_json::from_str(&r.body).unwrap();
            let actor: Value = serde_json::from_str(&r.actor).unwrap();
            assert_eq!(body["harness"], "browser");
            assert!(r.ids.session.as_deref().unwrap().starts_with("browser-"));
            (
                body["how"].as_str().unwrap().to_string(),
                actor["type"].as_str().unwrap().to_string(),
                actor["display_name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        starts,
        [
            ("setup".into(), "owner".into(), "".into()),
            ("invite".into(), "viewer".into(), "Sam".into()),
            ("link".into(), "owner".into(), "".into()),
            ("guest".into(), "viewer".into(), "Mia".into()),
        ]
    );
    let ends: Vec<&clax_store::audit::AuditRow> =
        rows.iter().filter(|r| r.kind == "session.end").collect();
    assert_eq!(ends.len(), 1);
    assert!(ends[0].body.contains("sign_out"));
    let link_start = rows
        .iter()
        .find(|r| r.kind == "session.start" && r.body.contains("\"link\""))
        .unwrap();
    assert_eq!(ends[0].ids.session, link_start.ids.session);
    let shared = rows
        .iter()
        .find(|r| r.kind == "artifact.update" && r.ids.artifact.as_deref() == Some(aid.as_str()))
        .expect("sharing by link is recorded");
    assert!(
        shared.body.contains("\"link_access\":\"viewer\""),
        "{}",
        shared.body
    );
    assert!(shared.actor.contains("\"owner\""), "{}", shared.actor);
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    assert!(
        kinds.contains(&"grant.approve") && kinds.contains(&"grant.token"),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn signing_in_anew_ends_the_browsers_previous_session() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{aid}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    // A guest's browser joins by invite: the guest's session ends.
    let mut b = Browser::default();
    site.sign_in_put(&mut b, json!({"display_name": "Mia"}))
        .await;
    let guest = b.clone();
    let (_, v) = site
        .post(&owner, "/api/people/invites", json!({"name": "Mia Lane"}))
        .await;
    let code_ = v["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    site.sign_in(
        &mut b,
        &format!("/api/hosted/invites/{code_}/accept"),
        json!({"replace": true}),
    )
    .await;
    let (_, me) = site.get(&guest, "/api/hosted/me").await;
    assert_eq!(me["signed_in"], json!(false));
    let (_, me) = site.get(&b, "/api/hosted/me").await;
    assert_eq!(me["person"]["display_name"], json!("Mia Lane"));
}

#[tokio::test]
async fn two_redemptions_of_one_code_at_once_sign_in_one_browser() {
    let site = Site::new().await;
    let owner = site.owner().await;
    for _ in 0..4 {
        // The attempts of the round before come back.
        site.clock.advance(300);
        let (_, link) = site
            .post(&owner, "/api/hosted/signin-links", json!({}))
            .await;
        let c = link["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let (_, inv) = site
            .post(
                &owner,
                "/api/people/invites",
                json!({"name": format!("P{}", c.len() + rand_suffix())}),
            )
            .await;
        let i = inv["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        for p in [
            format!("/api/hosted/signin-links/{c}/accept"),
            format!("/api/hosted/invites/{i}/accept"),
        ] {
            let anon = Browser::default();
            let (a, b) = tokio::join!(
                site.post(&anon, &p, json!({})),
                site.post(&anon, &p, json!({}))
            );
            let mut got = [a.0, b.0];
            got.sort();
            assert_eq!(got, [200, 404], "{p}");
        }
    }
}

fn rand_suffix() -> usize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[tokio::test]
async fn seats_hold_behind_a_persons_rights_and_a_spectators_snapshot_carries_keys() {
    let site = Site::new().await;
    site.ts.seats.set_limits(clax_core::seats::SeatLimits {
        people: 1,
        guests: 1,
        agents: 20,
        idle_ms: 180_000,
    });
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read Budget alone; Mia and Kim hold the default.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
                {"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": [budget]},
            ]),
        )
        .await;
    let mia = site.person(&owner, "Mia").await;
    let kim = site.person(&owner, "Kim").await;
    // Mia's stream takes Plan's one seat.
    let res = site
        .req(&mia, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    let mut events = clax_testkit::EventReader::from_response(res);
    let (_, ready) = events.next().await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let live: Vec<String> = ["artifact", "presence", "working"]
        .iter()
        .map(|t| format!("{t}:{plan}"))
        .collect();
    let (s, v) = site
        .post(
            &mia,
            &format!("/api/stream/{sid}"),
            json!({"subscribe": live}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // Kim, beyond the seats, is a spectator: refused the write.
    let (s, v) = site.thread(&kim, &plan, "let me write").await;
    assert_eq!(
        (s, code(&v)),
        (403, clax_core::seats::SPECTATOR_CODE),
        "{v}"
    );
    // Sam, who may not read Plan, finds no such artifact before any seat.
    let (s, v) = site.thread(&sam, &plan, "let me in").await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = site
        .get(&sam, &format!("/api/artifacts/{plan}/snapshot"))
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    // The spectator's snapshot hands each version its content key, as the
    // artifact's own view does.
    let (_, a) = site.get(&mia, &format!("/api/artifacts/{plan}")).await;
    let key = a["versions"][0]["content_key"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, snap) = site
        .get(&kim, &format!("/api/artifacts/{plan}/snapshot"))
        .await;
    assert_eq!(s, 200, "{snap}");
    assert_eq!(snap["versions"][0]["content_key"], json!(key));
    // A guest of a page shared by link reads its snapshot too.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{plan}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Gil"}))
        .await;
    let (s, snap) = site
        .get(&guest, &format!("/api/artifacts/{plan}/snapshot"))
        .await;
    assert_eq!(s, 200, "{snap}");
    assert_eq!(snap["versions"][0]["content_key"], json!(key));
}

/// A WebSocket to `aid`'s live socket from `b`, as the shell opens it: its
/// first frame, or the status that refused the upgrade.
async fn live_socket(site: &Site, b: &Browser, aid: &str) -> Result<Value, u16> {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let url = format!(
        "{}/api/artifacts/{aid}/live",
        site.ts.base.replacen("http://", "ws://", 1)
    );
    let mut req = url.into_client_request().unwrap();
    let h = req.headers_mut();
    h.insert(
        "host",
        format!("localhost:{}", site.ts.addr.port())
            .parse()
            .unwrap(),
    );
    h.insert("origin", site.origin.parse().unwrap());
    if let Some(c) = &b.cookie {
        h.insert("cookie", format!("clax_session={c}").parse().unwrap());
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((mut ws, _)) => loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
                .await
                .expect("a frame within 5 s")
                .expect("open")
                .expect("readable");
            if let Message::Text(t) = m {
                return Ok(serde_json::from_str(t.as_str()).unwrap());
            }
        },
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
        Err(e) => panic!("{e}"),
    }
}

#[tokio::test]
async fn drafts_and_live_layers_reach_signed_in_people_who_may_read_the_page() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read Budget alone; Mia holds the default.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
            ]),
        )
        .await;
    let mia = site.person(&owner, "Mia").await;
    let sid = site.ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let draft = |html: &'static str| {
        let (ts, sid, plan) = (&site.ts, sid.clone(), plan.clone());
        async move {
            let r = ts
                .authed(
                    ts.client
                        .put(format!("{}/api/sessions/{sid}/drafts/{plan}", ts.base)),
                )
                .json(&json!({"files": {"index.html": {"content": html, "encoding": "utf8"}}}))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        }
    };
    draft("<h1>x in May</h1>").await;
    let drafts = format!("/api/artifacts/{plan}/drafts");
    let (s, v) = site.get(&owner, &drafts).await;
    assert_eq!(s, 200, "{v}");
    let d = v["drafts"][0].clone();
    let (s, v) = site.get(&mia, &drafts).await;
    assert_eq!(
        (s, v["drafts"][0]["token"].clone()),
        (200, d["token"].clone())
    );
    // A person who may not read the page finds no such artifact, before
    // any draft or seat.
    let (s, v) = site.get(&sam, &drafts).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    assert_eq!(live_socket(&site, &sam, &plan).await, Err(404));
    assert_eq!(
        live_socket(&site, &mia, &plan).await.unwrap()["t"],
        "welcome"
    );
    // The draft URL's token is its capability, as a version's content key
    // is: the frame carries no cookie, and an ID alone reads nothing.
    let anon = Browser::default();
    let url = format!(
        "/c/{plan}/draft/{}/{}/",
        d["agent"].as_str().unwrap(),
        d["token"].as_str().unwrap()
    );
    let r = site
        .req(&anon, reqwest::Method::GET, &url)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert!(r.text().await.unwrap().contains("in May"));
    let wrong = format!(
        "/c/{plan}/draft/{}/AAAAAAAAAAAAAAAAAAAAAA/",
        d["agent"].as_str().unwrap()
    );
    assert_eq!(site.get(&anon, &wrong).await.0, 404);
    // Shared by link: a guest sees the drafts and the live layers; a
    // visitor who gave no name sees neither.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{plan}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    let (s, v) = site.get(&anon, &drafts).await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    assert_eq!(live_socket(&site, &anon, &plan).await, Err(401));
    let mut gil = Browser::default();
    site.sign_in_put(&mut gil, json!({"display_name": "Gil"}))
        .await;
    let (s, v) = site.get(&gil, &drafts).await;
    assert_eq!(
        (s, v["drafts"][0]["token"].clone()),
        (200, d["token"].clone())
    );
    assert_eq!(
        live_socket(&site, &gil, &plan).await.unwrap()["t"],
        "welcome"
    );
    let (s, _) = site
        .get(&gil, &format!("/api/artifacts/{budget}/drafts"))
        .await;
    assert_eq!(s, 404);
    // The guest's stream carries the drafts on the page's working topic.
    let res = site
        .req(&gil, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    let mut events = clax_testkit::EventReader::from_response(res);
    let (_, ready) = events.next().await;
    let stream = ready["stream"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &gil,
            &format!("/api/stream/{stream}"),
            json!({"subscribe": [format!("working:{plan}")]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    draft("<h1>x in June</h1>").await;
    let e = events.next_named("drafts").await;
    assert_eq!(e["artifact_id"], json!(plan), "{e}");
    assert_eq!(e["drafts"].as_array().map(Vec::len), Some(1), "{e}");
}

/// An agent's MCP client on the adapter with an agent token: the session
/// it was answered with, if any.
struct TokenAgent<'a> {
    site: &'a Site,
    token: String,
    session: Option<String>,
}

impl TokenAgent<'_> {
    /// The session ID its `Mcp-Session-Id` carries (`<session ID>.<secret>`).
    fn sid(&self) -> String {
        let header = self.session.as_deref().expect("an Mcp-Session-Id");
        header
            .split_once('.')
            .expect("a session secret")
            .0
            .to_string()
    }

    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.site.ts.base, clax_testkit::mcp::ADAPTER);
        let mut r = self
            .site
            .ts
            .client
            .request(method, url)
            .header("accept", clax_testkit::mcp::ACCEPT)
            .header(HOST, format!("localhost:{}", self.site.ts.addr.port()))
            .bearer_auth(&self.token);
        if let Some(s) = &self.session {
            r = r.header("mcp-session-id", s);
        }
        r
    }

    /// `initialize` as the MCP client `client`: the status; the session it
    /// is answered with is kept.
    async fn initialize(&mut self, client: &str) -> u16 {
        let mut body = clax_testkit::mcp::initialize();
        body["params"]["clientInfo"]["name"] = json!(client);
        let res = self
            .request(reqwest::Method::POST)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = res.status().as_u16();
        self.session = res
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        status
    }

    /// Calls `tool`: the HTTP status and the tool's result.
    async fn call(&self, tool: &str, args: Value) -> (u16, Value) {
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": tool, "arguments": args}});
        let res = self
            .request(reqwest::Method::POST)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap();
        let v: Value = serde_json::from_str(&text).unwrap_or(json!({"text": text}));
        (status, v["result"].clone())
    }
}

/// A tool result's error code (`structuredContent.error.code`), or "" for
/// a success.
fn tool_code(r: &Value) -> String {
    if r["isError"] != true {
        return String::new();
    }
    tool_json(r)["error"]["code"]
        .as_str()
        .unwrap_or("unknown")
        .to_string()
}

/// The JSON a tool result's text carries.
fn tool_json(r: &Value) -> Value {
    let text = r["content"][0]["text"].as_str().unwrap_or("{}");
    serde_json::from_str(text).unwrap_or(json!({}))
}

#[tokio::test]
async fn an_agent_token_acts_through_the_mcp_adapter_within_its_persons_rights() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read, comment on and watch Budget alone.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
                {"type": "clax-comments", "actions": ["read", "write", "reply", "watch"], "locations": [budget]},
            ]),
        )
        .await;
    let (s, v) = site
        .post(
            &sam,
            "/api/hosted/agent-tokens",
            json!({"label": "sam-laptop"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["id"].as_str().unwrap().to_string();
    let mut agent = TokenAgent {
        site: &site,
        token: v["token"].as_str().unwrap().to_string(),
        session: None,
    };
    // `initialize` registers a session for the token's grant, named after
    // the client.
    assert_eq!(agent.initialize("summary-writer").await, 200);
    let sid = agent.sid();
    let session = site
        .ts
        .store
        .get_session(&sid)
        .unwrap()
        .expect("registered");
    assert_eq!(session.grant_id.as_deref(), Some(gid.as_str()));
    assert_eq!(session.name.as_deref(), Some("summary-writer"));
    // Within Sam's rights: Budget is there (its file withheld until Sam
    // approves its author, as from any of Sam's agents), Plan is not.
    let (s, r) = agent.call("read", json!({"url_or_id": budget})).await;
    assert_eq!((s, tool_code(&r).as_str()), (200, "withheld"), "{r}");
    let (_, r) = agent.call("read", json!({"url_or_id": plan})).await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    let (_, r) = agent.call("list", json!({})).await;
    let listed = tool_json(&r)["artifacts"].as_array().unwrap().len();
    assert_eq!(listed, 1, "{r}");
    // Sam may not publish, so neither may the agent: not a new version of
    // Budget, nor a new artifact.
    let (_, r) = agent
        .call(
            "publish",
            json!({"id": budget, "html": "<h1>mine</h1>", "if_version": 1}),
        )
        .await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    let (_, r) = agent
        .call("publish", json!({"html": "<h1>new</h1>", "title": "New"}))
        .await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    assert_eq!(site.ts.store.list_artifacts().unwrap().len(), 2);
    // No file of the daemon's machine is read for it.
    let (_, r) = agent
        .call(
            "publish",
            json!({"file_path": "/etc/hosts", "title": "Hosts"}),
        )
        .await;
    assert_eq!(tool_code(&r), "invalid_args", "{r}");
    // Its session is its own: `working` shows on Budget.
    let (_, r) = agent
        .call(
            "working",
            json!({"url_or_id": budget, "message": "reading the budget"}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    // `DELETE` ends the session: its ID names nothing after.
    let res = agent.request(reqwest::Method::DELETE).send().await.unwrap();
    assert_eq!(res.status(), 204);
    let (s, _) = agent.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(s, 404);
    // A revoked token reaches nothing.
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/hosted/agent-tokens/{gid}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200);
    agent.session = None;
    assert_eq!(agent.initialize("summary-writer").await, 401);
}

impl Site {
    /// An agent token Sam mints, as Sam's MCP client holds it.
    async fn token_agent(&self, sam: &Browser, label: &str) -> TokenAgent<'_> {
        let (s, v) = self
            .post(sam, "/api/hosted/agent-tokens", json!({"label": label}))
            .await;
        assert_eq!(s, 201, "{v}");
        TokenAgent {
            site: self,
            token: v["token"].as_str().unwrap().to_string(),
            session: None,
        }
    }

    /// One reaper pass, as the daemon's.
    async fn reap(&self) -> Vec<String> {
        let (feedback, store) = (self.ts.feedback.clone(), self.ts.store.clone());
        tokio::task::spawn_blocking(move || {
            clax_server::serve::reap_idle(
                &feedback,
                &store,
                std::time::Duration::from_secs(clax_store::Store::SESSION_IDLE_SECS),
                &|_| panic!("a remote session is never probed"),
            )
        })
        .await
        .unwrap()
        .unwrap()
        .ended
    }

    fn live(&self, sid: &str) -> bool {
        self.ts
            .store
            .get_session(sid)
            .unwrap()
            .is_some_and(|s| s.ended_at.is_none())
    }
}

#[tokio::test]
async fn an_agent_tokens_sessions_are_its_own_end_when_left_and_are_capped() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [budget]}]),
        )
        .await;
    let mut a = site.token_agent(&sam, "laptop").await;
    let mut b = site.token_agent(&sam, "desktop").await;
    assert_eq!(a.initialize("summary-writer").await, 200);
    let header = a.session.clone().unwrap();
    let sid = a.sid();
    // The header carries the session's own secret (spec 2026-10-08 §6.9):
    // another MCP client of the same token naming the session by its ID
    // alone, or with another secret, is refused as for no session.
    let secret = header.split_once('.').unwrap().1;
    assert!(secret.starts_with("cxs_"), "{header}");
    let mut sibling = TokenAgent {
        site: &site,
        token: a.token.clone(),
        session: Some(sid.clone()),
    };
    let (s, _) = sibling.call("list", json!({})).await;
    assert_eq!(s, 404);
    sibling.session = Some(format!("{sid}.cxs_{}", "A".repeat(43)));
    let (s, _) = sibling.call("list", json!({})).await;
    assert_eq!(s, 404);
    let res = sibling
        .request(reqwest::Method::DELETE)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert!(site.live(&sid));
    // Another token's request naming the session: 404, as for none, and its
    // DELETE ends nothing.
    b.session = Some(header.clone());
    let (s, _) = b.call("list", json!({})).await;
    assert_eq!(s, 404);
    let res = b.request(reqwest::Method::DELETE).send().await.unwrap();
    assert_eq!(res.status(), 404);
    assert!(site.live(&sid));
    // Nor does an ID that is no session's, or one shaped as a path.
    for bad in ["01ZZZZZZZZZZZZZZZZZZZZZZZZ", "../artifacts", "a/b"] {
        b.session = Some(bad.to_string());
        let (s, _) = b.call("list", json!({})).await;
        assert_eq!(s, 404, "{bad}");
    }
    // Each request marks the session seen: in use, the reaper leaves it.
    for _ in 0..3 {
        site.clock.advance(20 * 60);
        let (s, r) = a.call("list", json!({})).await;
        assert_eq!((s, tool_code(&r).as_str()), (200, ""), "{r}");
        assert!(site.reap().await.is_empty());
    }
    // Left without a DELETE, it ends once unseen for the remote idle time,
    // and its ID then names nothing.
    site.clock
        .advance(clax_store::Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
    assert_eq!(site.reap().await, vec![sid.clone()]);
    let (s, _) = a.call("list", json!({})).await;
    assert_eq!(s, 404);
    // A token holds at most BEARER_SESSION_CAP live sessions: each
    // initialize beyond it ends the least recently seen.
    let cap = clax_store::Store::BEARER_SESSION_CAP;
    let mut sids = Vec::new();
    for _ in 0..cap + 2 {
        site.clock.advance(1);
        assert_eq!(a.initialize("summary-writer").await, 200);
        sids.push(a.sid());
    }
    let live: Vec<bool> = sids.iter().map(|s| site.live(s)).collect();
    assert_eq!(live.iter().filter(|l| **l).count(), cap, "{live:?}");
    assert!(
        !live[0] && !live[1] && live[2..].iter().all(|l| *l),
        "{live:?}"
    );
    // The other token's are its own.
    assert_eq!(b.initialize("other").await, 200);
    assert!(sids[2..].iter().all(|s| site.live(s)));
}

#[tokio::test]
async fn an_agent_token_on_the_api_reaches_only_a_grants_routes() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, sam_id) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [budget]}]),
        )
        .await;
    let (s, v) = site
        .post(&sam, "/api/hosted/agent-tokens", json!({"label": "laptop"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let (token, gid) = (
        v["token"].as_str().unwrap().to_string(),
        v["id"].as_str().unwrap().to_string(),
    );
    let mut agent = TokenAgent {
        site: &site,
        token: token.clone(),
        session: None,
    };
    assert_eq!(agent.initialize("summary-writer").await, 200);
    let sid = agent.sid();
    let handle = site
        .ts
        .store
        .get_session(&sid)
        .unwrap()
        .unwrap()
        .agent_handle;
    let api = |method: reqwest::Method, path: &str| {
        site.ts
            .client
            .request(method, site.url(path))
            .header(HOST, format!("localhost:{}", site.ts.addr.port()))
            .bearer_auth(&token)
    };
    // A grant's route, within Sam's rights: Budget alone.
    let (s, v) = json_of(
        api(reqwest::Method::GET, "/api/artifacts")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["artifacts"].as_array().unwrap().len(), 1, "{v}");
    // The owner's, the browser's and the People routes: none, whatever the
    // body. An agent token mints no token, approves no author and shares
    // nothing.
    use reqwest::Method as M;
    let refused = [
        (M::GET, "/api/token".to_string(), json!(null)),
        (M::GET, "/api/people".to_string(), json!(null)),
        (M::GET, "/api/people/me".to_string(), json!(null)),
        (M::GET, "/api/people/invites".to_string(), json!(null)),
        (
            M::POST,
            "/api/people/invites".to_string(),
            json!({"name": "Eve", "rights": "clax:collaborator"}),
        ),
        (
            M::PUT,
            format!("/api/people/{sam_id}/rights"),
            json!({"rights": "clax:collaborator"}),
        ),
        (M::DELETE, format!("/api/people/{sam_id}"), json!(null)),
        (
            M::PUT,
            format!("/api/people/me/grants/{gid}/narrow"),
            json!({"rights": []}),
        ),
        (M::GET, "/api/grants".to_string(), json!(null)),
        (M::POST, format!("/api/grants/{gid}/approve"), json!({})),
        (M::DELETE, format!("/api/grants/{gid}"), json!(null)),
        (
            M::POST,
            "/api/approvals".to_string(),
            json!({"author": "*unconnected", "scope": "*"}),
        ),
        (
            M::POST,
            "/api/approvals/requests/r1/decide".to_string(),
            json!({"allow": true}),
        ),
        (
            M::PUT,
            format!("/api/agents/{handle}"),
            json!({"name": "renamed"}),
        ),
        (
            M::PUT,
            format!("/api/agents/{handle}/elevate"),
            json!({"rights": []}),
        ),
        (M::DELETE, format!("/api/agents/{handle}"), json!(null)),
        (M::GET, "/api/inbox".to_string(), json!(null)),
        (M::GET, "/api/viewers/me".to_string(), json!(null)),
        (M::GET, "/api/toolpath/export".to_string(), json!(null)),
        (M::GET, "/api/hosted/agent-tokens".to_string(), json!(null)),
        (
            M::POST,
            "/api/hosted/agent-tokens".to_string(),
            json!({"label": "more"}),
        ),
        (
            M::DELETE,
            format!("/api/hosted/agent-tokens/{gid}"),
            json!(null),
        ),
        (M::POST, "/api/hosted/signin-links".to_string(), json!({})),
        (M::GET, "/api/hosted/links".to_string(), json!(null)),
        (
            M::PUT,
            format!("/api/hosted/artifacts/{budget}/link"),
            json!({"access": "comment"}),
        ),
        (M::POST, "/api/hosted/signout".to_string(), json!({})),
        // Hosted mode's own routes are an agent's no more than any other
        // route outside a grant's: refused before their handlers run, even
        // the ones open to anyone.
        (M::GET, "/api/hosted/me".to_string(), json!(null)),
        (M::GET, "/api/hosted/public".to_string(), json!(null)),
    ];
    for (m, path, body) in refused {
        let mut r = api(m.clone(), &path);
        if !body.is_null() {
            r = r.json(&body);
        }
        let (s, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!(s, 403, "{m} {path}: {v}");
        assert!(
            ["forbidden", "browser_required", "not_owner"].contains(&code(&v)),
            "{m} {path}: {v}"
        );
    }
    // Nothing changed: one token, Sam's rights, no link, no approval.
    let (_, v) = site.get(&sam, "/api/hosted/agent-tokens").await;
    assert_eq!(v["tokens"].as_array().map(Vec::len), Some(1), "{v}");
    assert!(site.ts.store.link_access(&budget).unwrap().is_none());
    // Beside a browser's marks the token is refused outright: an Origin or
    // a cookie is 403 `forbidden_origin`, so a page cannot spend it.
    for (k, val) in [
        (ORIGIN.as_str(), site.origin.clone()),
        (COOKIE.as_str(), "clax_session=x".to_string()),
    ] {
        let (s, v) = json_of(
            api(M::GET, "/api/artifacts")
                .header(k, val)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{k}: {v}");
    }
    // The one hosted route an agent token reaches: it revokes itself.
    let r = api(M::DELETE, "/api/hosted/agent-token")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let (s, v) = json_of(api(M::GET, "/api/artifacts").send().await.unwrap()).await;
    assert_eq!(s, 401, "{v}");
}

/// A POST written by hand on its own connection with agent token `token`,
/// declaring a `len`-byte body and sending none of it, so the test holds
/// the body's read open. The status and error code it is answered with
/// come from [`held_response`].
async fn held_body(site: &Site, path: &str, token: &str, len: usize) -> tokio::net::TcpStream {
    use tokio::io::AsyncWriteExt;
    let port = site.ts.addr.port();
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nhost: localhost:{port}\r\nconnection: close\r\n\
         authorization: Bearer {token}\r\naccept: {}\r\n\
         content-type: application/json\r\ncontent-length: {len}\r\n\r\n",
        clax_testkit::mcp::ACCEPT
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream
}

/// The status and error code `stream` ([`held_body`]) is answered with.
async fn held_response(mut stream: tokio::net::TcpStream) -> (u16, String) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut buf),
    )
    .await
    .expect("a response within 10 s");
    let text = String::from_utf8_lossy(&buf);
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    (status, code(&v).to_string())
}

#[tokio::test]
async fn an_agent_tokens_bodies_are_bounded_like_a_signed_grants() {
    use std::time::Duration;
    const MIB: usize = 1024 * 1024;
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let (ts, timer) = TestServer::spawn_hosted_agent_timer(clock.clone(), Some(SECRET)).await;
    let origin = format!("http://localhost:{}", ts.addr.port());
    let site = Site { ts, clock, origin };
    let owner = site.owner().await;
    let mint = |label: &'static str| {
        let site = &site;
        let owner = &owner;
        async move {
            let (s, v) = site
                .post(owner, "/api/hosted/agent-tokens", json!({"label": label}))
                .await;
            assert_eq!(s, 201, "{v}");
            v["token"].as_str().unwrap().to_string()
        }
    };
    let (token, other) = (mint("laptop").await, mint("desktop").await);
    let api = "/api/artifacts";
    let adapter = clax_testkit::mcp::ADAPTER;
    let r0 = site.ts.agent_gate.state().read;

    // One large body of a token at a time, on the API and the adapter
    // alike: the others wait in its queue of 4, and a sixth is refused at
    // once. Another token's is not held up by them.
    let holder = held_body(&site, api, &token, 2 * MIB).await;
    site.ts
        .wait_gate("the holder", |g| g.held == 1 && g.reserved == 2 * MIB)
        .await;
    let mut queued = Vec::new();
    for i in 1..=4 {
        let path = if i % 2 == 0 { adapter } else { api };
        queued.push(held_body(&site, path, &token, 2 * MIB).await);
        site.ts.wait_gate("queued", |g| g.queued == i).await;
    }
    let sixth = held_body(&site, adapter, &token, 2 * MIB).await;
    assert_eq!(held_response(sixth).await, (429, "limit_reached".into()));
    let theirs = held_body(&site, api, &other, 2 * MIB).await;
    site.ts
        .wait_gate("the other token's", |g| g.held == 2 && g.queued == 4)
        .await;
    // The holder's whole read ends 12 s in (10 s and 1 s per MiB): 408,
    // and the first in its queue takes the permit.
    timer.advance(Duration::from_secs(12));
    assert_eq!(held_response(holder).await, (408, "timeout".into()));
    drop((queued, theirs));
    site.ts
        .wait_gate("everything back", |g| {
            g.held == 0 && g.reserved == 0 && g.queued == 0 && g.small == 0
        })
        .await;
    assert_eq!(site.ts.agent_gate.state().read, r0, "nothing was sent");

    // Past the adapter's limit (96 MiB): 413 before any of it is read.
    let big = held_body(&site, adapter, &token, 97 * MIB).await;
    assert_eq!(held_response(big).await.0, 413);

    // Two publishes over 1 MiB at once through the adapter under the one
    // token, each read through the gate by the adapter and again by the
    // API it calls in process (each read waits for the one holding the
    // token's permit): both land.
    let mut a = TokenAgent {
        site: &site,
        token: token.clone(),
        session: None,
    };
    let mut b = TokenAgent {
        site: &site,
        token,
        session: None,
    };
    assert_eq!(a.initialize("one").await, 200);
    assert_eq!(b.initialize("two").await, 200);
    let html = |w: &str| format!("<h1>{w}</h1><p>{}</p>", "x".repeat(3 * MIB / 2));
    let (ra, rb) = tokio::join!(
        a.call("publish", json!({"html": html("A"), "title": "A"})),
        b.call("publish", json!({"html": html("B"), "title": "B"}))
    );
    for (s, r) in [ra, rb] {
        assert_eq!((s, tool_code(&r).as_str()), (200, ""), "{r}");
    }
    let read = site.ts.agent_gate.state().read - r0;
    assert!(read > (6 * MIB) as u64, "{read}");
}

#[tokio::test]
async fn a_member_forks_what_they_may_read_into_their_own_artifact() {
    let site = Site::new().await;
    site.ts.seats.set_limits(clax_core::seats::SeatLimits {
        people: 1,
        guests: 1,
        agents: 20,
        idle_ms: 180_000,
    });
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read Budget alone, and create nothing; Mia and Kim hold the
    // default, which creates.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [budget]}]),
        )
        .await;
    let (mia, mia_id) = site
        .person_with(&owner, "Mia", json!(["clax:collaborator"]))
        .await;
    let kim = site.person(&owner, "Kim").await;
    let fork = |aid: &str| format!("/api/artifacts/{aid}/fork");
    // No read: as if there were no such artifact. No create: refused.
    let (s, v) = site.post(&sam, &fork(&plan), json!({})).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = site.post(&sam, &fork(&budget), json!({})).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // Mia's stream takes Plan's one seat: Kim, beyond it, is a spectator.
    let res = site
        .req(&mia, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    let mut events = clax_testkit::EventReader::from_response(res);
    let (_, ready) = events.next().await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &mia,
            &format!("/api/stream/{sid}"),
            json!({"subscribe": [format!("artifact:{plan}")]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A spectator reads and forks (spec §7).
    let (s, v) = site.post(&kim, &fork(&plan), json!({})).await;
    assert_eq!(s, 201, "{v}");
    // Mia forks version 1: the fork is hers, and names where it came from.
    let (s, v) = site.post(&mia, &fork(&plan), json!({"version": 1})).await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    assert_ne!(fid, plan);
    assert_eq!(v["url"], json!(format!("/a/{fid}")));
    assert_eq!(v["version"]["n"], json!(1));
    assert_eq!(
        v["artifact"]["forked_from"],
        json!({"artifact_id": plan, "version": 1, "title": "Plan"})
    );
    assert!(v["version"]["content_key"].is_string(), "{v}");
    let (s, got) = site.get(&mia, &format!("/api/artifacts/{fid}")).await;
    assert_eq!(s, 200, "{got}");
    assert_eq!(got["artifact"]["forked_from"]["title"], json!("Plan"));
    let (s, people) = site.get(&owner, "/api/artifacts").await;
    assert_eq!(s, 200);
    assert!(listed(&people).contains(&fid));
    let st = site.ts.store.clone();
    let (fork_id, mia_id2) = (fid.clone(), mia_id.clone());
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&fork_id, Some(&mia_id2)))
            .await
            .unwrap()
            .unwrap()
    );
    // The source has no threads to give, and the fork has none.
    let (_, threads) = site
        .get(&mia, &format!("/api/artifacts/{fid}/threads"))
        .await;
    assert_eq!(threads["threads"], json!([]), "{threads}");
    // A version the source lacks.
    let (s, v) = site.post(&mia, &fork(&budget), json!({"version": 9})).await;
    assert_eq!((s, code(&v)), (400, "unknown_version"), "{v}");
    // A guest of a page shared by link may comment there, never fork.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{budget}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Gil"}))
        .await;
    let (s, v) = site.post(&guest, &fork(&budget), json!({})).await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    // The owner forks too; a source since deleted keeps its ID, not its title.
    let (s, v) = site.post(&owner, &fork(&budget), json!({})).await;
    assert_eq!(s, 201, "{v}");
    let oid = v["artifact"]["id"].as_str().unwrap().to_string();
    let res = site
        .ts
        .authed(
            site.ts
                .client
                .delete(site.url(&format!("/api/artifacts/{budget}"))),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let (_, got) = site.get(&owner, &format!("/api/artifacts/{oid}")).await;
    assert_eq!(
        got["artifact"]["forked_from"],
        json!({"artifact_id": budget, "version": 1, "title": null})
    );
}

#[tokio::test]
async fn a_forks_source_is_named_only_to_who_may_read_the_source() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let secret = site.titled("Secret plan").await;
    let (s, v) = site
        .post(&owner, &format!("/api/artifacts/{secret}/fork"), json!({}))
        .await;
    assert_eq!(s, 201, "{v}");
    let fid = v["artifact"]["id"].as_str().unwrap().to_string();
    let res = site
        .ts
        .authed(
            site.ts
                .client
                .patch(site.url(&format!("/api/artifacts/{fid}")))
                .json(&json!({"title": "Mine"})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // The fork is shared by link; its source is not.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{fid}/link"),
        json!({"access": "viewer"}),
    )
    .await;
    let anon = Browser::default();
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Gil"}))
        .await;
    let kim = site.person(&owner, "Kim").await;
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [fid]}]),
        )
        .await;
    let unnamed = json!({"artifact_id": secret, "version": 1, "title": null});
    for (who, b) in [("anonymous", &anon), ("a guest", &guest), ("Sam", &sam)] {
        let (s, got) = site.get(b, &format!("/api/artifacts/{fid}")).await;
        assert_eq!(s, 200, "{who}: {got}");
        assert_eq!(got["artifact"]["forked_from"], unnamed, "{who}");
        let page = site
            .req(b, reqwest::Method::GET, &format!("/a/{fid}"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(page.contains(&fid), "{who}: {page}");
        assert!(
            !page.contains("Secret plan"),
            "{who}: the page names the source"
        );
    }
    // A member who reads the source, by their rights or its link, sees it.
    let (_, got) = site.get(&kim, &format!("/api/artifacts/{fid}")).await;
    assert_eq!(
        got["artifact"]["forked_from"]["title"],
        json!("Secret plan")
    );
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{secret}/link"),
        json!({"access": "viewer"}),
    )
    .await;
    for (who, b) in [("anonymous", &anon), ("Sam", &sam)] {
        let (_, got) = site.get(b, &format!("/api/artifacts/{fid}")).await;
        assert_eq!(
            got["artifact"]["forked_from"]["title"],
            json!("Secret plan"),
            "{who}"
        );
    }
}

#[tokio::test]
async fn an_agent_token_forks_through_the_mcp_adapter_within_its_persons_rights() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // Sam may read Budget alone, and create nothing; Mia holds the default,
    // which creates.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [budget]}]),
        )
        .await;
    let (mia, mia_id) = site
        .person_with(&owner, "Mia", json!(["clax:collaborator"]))
        .await;
    let mut sams = site.token_agent(&sam, "sam-laptop").await;
    assert_eq!(sams.initialize("summary-writer").await, 200);
    // No read, as for the agent's `read`: refused. No create: refused.
    let (s, r) = sams.call("fork", json!({"url_or_id": plan})).await;
    assert_eq!((s, tool_code(&r).as_str()), (200, "not_permitted"), "{r}");
    let (s, r) = sams.call("fork", json!({"url_or_id": budget})).await;
    assert_eq!((s, tool_code(&r).as_str()), (200, "not_permitted"), "{r}");
    // Mia's agent forks version 1 of Plan: the fork is Mia's, made from the
    // agent's session, which watches it.
    let mut mias = site.token_agent(&mia, "mia-laptop").await;
    assert_eq!(mias.initialize("summary-writer").await, 200);
    let (s, r) = mias
        .call("fork", json!({"url_or_id": format!("/a/{plan}/v/1")}))
        .await;
    assert_eq!((s, tool_code(&r).as_str()), (200, ""), "{r}");
    let out = tool_json(&r);
    let fid = out["artifact_id"]
        .as_str()
        .expect("artifact_id")
        .to_string();
    assert_ne!(fid, plan);
    // The owner's title is withheld from Mia's agents until Mia approves
    // the owner as an author, as for its `read`.
    assert_eq!(
        out["forked_from"],
        json!({"artifact_id": plan, "version": 1, "title": null}),
        "{out}"
    );
    let st = site.ts.store.clone();
    let (fork_id, who) = (fid.clone(), mia_id.clone());
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&fork_id, Some(&who)))
            .await
            .unwrap()
            .unwrap()
    );
    let sid = mias.sid();
    let st = site.ts.store.clone();
    let watched = tokio::task::spawn_blocking(move || st.list_watches(&sid))
        .await
        .unwrap()
        .unwrap();
    assert!(watched.iter().any(|w| w.artifact_id == fid), "{watched:?}");
}

#[tokio::test]
async fn with_a_content_origin_content_is_served_there_alone_and_its_pages_name_the_shell() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with_content(clock.clone(), Some(SECRET)).await;
    let port = ts.addr.port();
    let origin = format!("http://localhost:{port}");
    let site = Site { ts, clock, origin };
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let (s, v) = site.get(&owner, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(s, 200, "{v}");
    let content = format!("http://[::1]:{port}");
    assert_eq!(v["content_origin"], json!(content));
    let key = v["versions"][0]["content_key"]
        .as_str()
        .unwrap()
        .to_string();
    let at = |host: String, path: String| {
        site.ts
            .client
            .get(site.url(&path))
            .header(HOST, host)
            .send()
    };
    let public = format!("localhost:{port}");
    let content_host = format!("[::1]:{port}");
    let keyed = format!("/c/{aid}/v/1/k/{key}/");
    // On the content origin, at its key, with the sandbox; the page's bridge
    // talks to the public origin.
    let r = at(content_host.clone(), keyed.clone()).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let html = r.text().await.unwrap();
    assert!(
        html.contains(&format!("data-origin=\"{}\"", site.origin)),
        "{html}"
    );
    // Never on the public origin.
    assert_eq!(
        at(public.clone(), keyed.clone()).await.unwrap().status(),
        404
    );
    // The content origin serves no API, shell or sign-in.
    for p in ["/api/artifacts", "/", "/setup", &format!("/a/{aid}")] {
        let r = at(content_host.clone(), p.to_string()).await.unwrap();
        assert_eq!(r.status(), 404, "{p}");
    }
    assert_eq!(
        at(content_host.clone(), "/healthz".into())
            .await
            .unwrap()
            .status(),
        200
    );
    // The shell's first load frames the page there, sandboxed.
    let r = site
        .req(&owner, reqwest::Method::GET, &format!("/a/{aid}"))
        .send()
        .await
        .unwrap();
    let page = r.text().await.unwrap();
    assert!(
        page.contains(&format!("src=\"{content}{keyed}\"")),
        "{page}"
    );
    // An asset's URL names the content origin.
    let (s, v) = site
        .get(&owner, &format!("/api/artifacts/{aid}/assets"))
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        clax_http::hosted::Hosted::new(&site.origin, None)
            .unwrap()
            .with_content_origin(&site.origin)
            .is_none(),
        "the public origin is no content origin"
    );
}

#[tokio::test]
async fn a_kept_socket_reopens_only_while_its_session_is_live() {
    use clax_http::identity::SocketCredentials;
    use clax_server::routes::{live_layers, room};
    let site = Site::new().await;
    let owner = site.owner().await;
    let res = site
        .ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Room", "capabilities": {"room": {}}, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let s = &site.ts.state;
    let held = SocketCredentials {
        session: Some(clax_core::secret::sha256_hex(
            owner.cookie.as_deref().unwrap(),
        )),
        ..SocketCredentials::default()
    };
    let peer = "0123456789abcdef";
    assert!(matches!(
        room::reopen(s, &aid, peer, &[], &held).await,
        Some(Ok(_))
    ));
    // Neither the token nor a session opened it: nothing reopens it.
    let nobody = SocketCredentials::default();
    assert!(room::reopen(s, &aid, peer, &[], &nobody).await.is_none());
    assert!(live_layers::reopen(s, &aid, &nobody).await.is_none());
    // The session ends while the object sleeps: its socket stays closed.
    let res = site
        .req(&owner, reqwest::Method::POST, "/api/hosted/signout")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(room::reopen(s, &aid, peer, &[], &held).await.is_none());
    assert!(live_layers::reopen(s, &aid, &held).await.is_none());
}

impl Site {
    /// Sam, who may read, comment on, send and watch `budget`, signed in;
    /// with his public ID and an agent token's MCP client `client` that
    /// watches `budget`, its handle.
    async fn sam_with_agent(
        &self,
        owner: &Browser,
        budget: &str,
        client: &str,
    ) -> (Browser, String, TokenAgent<'_>, String) {
        let (sam, sam_id) = self
            .person_with(
                owner,
                "Sam",
                json!([
                    {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
                    {"type": "clax-comments", "actions": ["read", "write", "reply", "send", "watch"], "locations": [budget]},
                ]),
            )
            .await;
        let mut agent = self.token_agent(&sam, "laptop").await;
        assert_eq!(agent.initialize(client).await, 200);
        let (_, r) = agent.call("watch", json!({"url_or_id": budget})).await;
        assert_eq!(tool_code(&r), "", "{r}");
        let handle = self
            .ts
            .store
            .get_session(&agent.sid())
            .unwrap()
            .unwrap()
            .agent_handle;
        (sam, sam_id, agent, handle)
    }
}

#[tokio::test]
async fn a_member_approves_an_author_for_their_agent_from_their_inbox() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let plan = site.titled("Plan").await;
    let (sam, sam_id, agent, handle) = site.sam_with_agent(&owner, &budget, "summary-writer").await;
    // Mia may read Plan alone.
    let (mia, _) = site
        .person_with(
            &owner,
            "Mia",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [plan]}]),
        )
        .await;
    let (s, t) = site.thread(&owner, &budget, "please total the rows").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    // The Send menu names the token's agent by its client's name, as Sam's.
    let (s, a) = site.get(&owner, &format!("/api/artifacts/{budget}")).await;
    assert_eq!(s, 200, "{a}");
    let p = &a["artifact"]["participants"];
    let named = p["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == json!(handle))
        .cloned()
        .unwrap_or_else(|| panic!("the agent among {p}"));
    assert_eq!(named["name"], "summary-writer", "{named}");
    assert_eq!(named["person"]["public_id"], json!(sam_id), "{named}");
    // Who approves first, asked through the hosted gate: Sam, for the
    // owner's thread, from the owner's browser and from Sam's.
    let firsts = format!("/api/artifacts/{budget}/approves-first?threads={tid}");
    for b in [&owner, &sam] {
        let (s, v) = site.get(b, &firsts).await;
        assert_eq!(
            (s, v["approves_first"].clone()),
            (200, json!([sam_id])),
            "{v}"
        );
    }
    // Mia may not read Budget: it is not there for her.
    let (s, _) = site.get(&mia, &firsts).await;
    assert_eq!(s, 404);
    // The owner sends it to Sam's agent: Sam is asked.
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{budget}/threads/{tid}/send"),
            json!({"to": handle}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // Before Sam decides, the agent reads the comment withheld.
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": budget, "thread_id": tid}),
        )
        .await;
    assert!(!r.to_string().contains("please total the rows"), "{r}");
    // Sam's hosted browser has its inbox, with the approval card.
    let (s, inbox) = site.get(&sam, "/api/inbox").await;
    assert_eq!(s, 200, "{inbox}");
    let item = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "approval")
        .cloned()
        .unwrap_or_else(|| panic!("an approval card in {inbox}"));
    let card = &item["approval"];
    assert_eq!(card["agent"]["handle"], json!(handle), "{card}");
    assert_eq!(card["agent"]["name"], "summary-writer", "{card}");
    assert_eq!(
        card["content"]["comments"][0]["body"], "please total the rows",
        "{card}"
    );
    let rid = card["id"].as_str().unwrap().to_string();
    // The owner's inbox holds none of Sam's.
    let (_, owners) = site.get(&owner, "/api/inbox").await;
    assert!(!owners.to_string().contains(&rid), "{owners}");
    // Neither Mia nor the agent decides it.
    let decide = format!("/api/approvals/requests/{rid}/decide");
    let (s, _) = site.post(&mia, &decide, json!({"scope": "thread"})).await;
    assert!(s == 403 || s == 404, "{s}");
    // Sam approves the owner for this thread.
    let (s, v) = site.post(&sam, &decide, json!({"scope": "thread"})).await;
    assert_eq!(s, 200, "{v}");
    // The agent receives the comment.
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": budget, "thread_id": tid}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    assert!(r.to_string().contains("please total the rows"), "{r}");
}

#[tokio::test]
async fn guests_reach_no_inbox_approval_or_approves_first_route() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{budget}/link"),
            json!({"access": "commenter"}),
        )
        .await;
    assert_eq!(s, 200);
    let (_, t) = site.thread(&owner, &budget, "a note").await;
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Gil"}))
        .await;
    // The guest reads the shared page...
    let (s, _) = site.get(&guest, &format!("/api/artifacts/{budget}")).await;
    assert_eq!(s, 200);
    // ...and none of the person routes.
    for (m, path) in [
        (reqwest::Method::GET, "/api/inbox".to_string()),
        (reqwest::Method::GET, "/api/inbox/summary".to_string()),
        (reqwest::Method::GET, "/api/approvals".to_string()),
        (reqwest::Method::GET, "/api/approvals/requests".to_string()),
        (
            reqwest::Method::POST,
            "/api/approvals/requests/01ZZZZZZZZZZZZZZZZZZZZZZZZ/decide".to_string(),
        ),
        (
            reqwest::Method::GET,
            format!("/api/artifacts/{budget}/approves-first?threads={tid}"),
        ),
        (reqwest::Method::GET, "/api/people/me".to_string()),
        (reqwest::Method::GET, "/api/agents".to_string()),
    ] {
        for b in [&guest, &Browser::default()] {
            let (s, v) = site
                .send(b, m.clone(), &path, json!({"scope": "thread"}))
                .await;
            assert_eq!((s, code(&v)), (401, "sign_in_required"), "{m} {path}: {v}");
        }
    }
}

#[tokio::test]
async fn a_member_narrows_and_ends_one_token_agent_session_from_people() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _, mut a, a_handle) = site.sam_with_agent(&owner, &budget, "writer-a").await;
    // A second MCP client of the same token: a session of its own.
    let mut b = TokenAgent {
        site: &site,
        token: a.token.clone(),
        session: None,
    };
    assert_eq!(b.initialize("writer-b").await, 200);
    assert_ne!(a.sid(), b.sid());
    let b_handle = site
        .ts
        .store
        .get_session(&b.sid())
        .unwrap()
        .unwrap()
        .agent_handle;
    // People lists the token as Sam's machine and each session as an agent
    // of it, by its client's name.
    let (s, me) = site.get(&sam, "/api/people/me").await;
    assert_eq!(s, 200, "{me}");
    let grants = me["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1, "{me}");
    let gid = grants[0]["id"].clone();
    let agents = me["agents"].as_array().unwrap();
    let named = |h: &str| agents.iter().find(|x| x["handle"] == json!(h)).cloned();
    let (pa, pb) = (named(&a_handle).unwrap(), named(&b_handle).unwrap());
    assert_eq!(
        (pa["name"].as_str(), pa["grant_id"].clone()),
        (Some("writer-a"), gid.clone())
    );
    assert_eq!(
        (pb["name"].as_str(), pb["grant_id"].clone()),
        (Some("writer-b"), gid)
    );
    // Sam narrows writer-a to comments alone: it no longer reads Budget,
    // while writer-b still does.
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &format!("/api/agents/{a_handle}/narrow"),
            json!({"access": [{"type": "clax-comments", "actions": ["read"], "locations": [budget]}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, r) = a.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    let (_, r) = b.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "withheld", "{r}");
    // A request of the token that names no session is held to every live
    // narrowing of its sessions (spec 2026-10-08 §6.8).
    let mut bare = TokenAgent {
        site: &site,
        token: a.token.clone(),
        session: None,
    };
    let (_, r) = bare.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    // Sam ends writer-a: its session refuses from then on, and a new
    // initialize starts a session of its own, not narrowed.
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/agents/{a_handle}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = a.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(s, 404);
    let (_, r) = bare.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "withheld", "{r}");
    assert_eq!(a.initialize("writer-a").await, 200);
    let (_, r) = a.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "withheld", "{r}");
    bare.session = None;
    // The owner, too, sees the token's agents under Sam in People.
    let (_, people) = site.get(&owner, "/api/people").await;
    assert!(people.to_string().contains(&b_handle), "{people}");
}

// Device sign-in (`/api/hosted/device/…`): another machine's Clax signs in
// as a person and receives an agent token of theirs.

impl Site {
    /// A request of another machine's Clax: no cookie, no `Origin`.
    async fn machine(&self, path: &str, body: Value) -> (u16, Value) {
        json_of(
            self.ts
                .client
                .post(self.url(path))
                .header(HOST, format!("localhost:{}", self.ts.addr.port()))
                .json(&body)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    async fn start_device(&self, name: &str) -> Value {
        let (s, v) = self
            .machine("/api/hosted/device/code", json!({"client_name": name}))
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn poll_device(&self, device_code: &str) -> (u16, Value) {
        self.machine(
            "/api/hosted/device/token",
            json!({"device_code": device_code}),
        )
        .await
    }
}

#[tokio::test]
async fn a_person_approves_a_machines_device_sign_in_and_it_gets_their_agent_token_once() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.person(&owner, "Sam").await;
    let v = site.start_device("sams-laptop").await;
    let (dc, uc) = (
        v["device_code"].as_str().unwrap().to_string(),
        v["user_code"].as_str().unwrap().to_string(),
    );
    assert!(dc.starts_with("cxd_"), "{v}");
    assert_eq!(v["verification_uri"], format!("{}/device", site.origin));
    assert_eq!(
        v["verification_uri_complete"],
        format!("{}/device?code={uc}", site.origin)
    );
    assert_eq!(
        (v["expires_in"].as_i64(), v["interval"].as_i64()),
        (Some(600), Some(5))
    );
    // The approval page is the shell's, open to anyone; its API is a
    // member's.
    let page = site
        .req(&Browser::default(), reqwest::Method::GET, "/device")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let (s, v) = site
        .get(&Browser::default(), &format!("/api/hosted/device/{uc}"))
        .await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    let (s, v) = site.get(&sam, &format!("/api/hosted/device/{uc}")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["client_name"], "sams-laptop");
    assert!(
        v["peer_ip"].as_str().is_some_and(|ip| !ip.is_empty()),
        "{v}"
    );
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!((s, code(&v)), (400, "authorization_pending"), "{v}");
    // Approving is a browser's same-origin change.
    let (s, v) = json_of(
        site.ts
            .client
            .post(site.url(&format!("/api/hosted/device/{uc}/approve")))
            .header(HOST, format!("localhost:{}", site.ts.addr.port()))
            .header(
                COOKIE,
                format!("clax_session={}", sam.cookie.clone().unwrap()),
            )
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    let (s, v) = site
        .post(&sam, &format!("/api/hosted/device/{uc}/approve"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let gid = v["token"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["token"]["label"], "sams-laptop");
    // Listed on Sam's People page at once.
    let (_, list) = site.get(&sam, "/api/hosted/agent-tokens").await;
    assert_eq!(list["tokens"][0]["id"], gid.as_str(), "{list}");
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!((s, code(&v)), (400, "slow_down"), "{v}");
    site.clock.advance(5);
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grant_id"], gid.as_str());
    assert_eq!(v["person"]["display_name"], "Sam");
    assert_eq!(v["origin"], site.origin.as_str());
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxa_"));
    // The token is Sam's agent's.
    let ok = site
        .ts
        .client
        .get(site.url("/api/artifacts"))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    // Once.
    site.clock.advance(5);
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!((s, code(&v)), (400, "expired_token"), "{v}");
    // Revoked on People, it stops working.
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/hosted/agent-tokens/{gid}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200);
    let gone = site
        .ts
        .client
        .get(site.url("/api/artifacts"))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 401);
}

#[tokio::test]
async fn a_device_sign_in_is_refused_when_denied_expired_or_unknown() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let v = site.start_device("box").await;
    let (dc, uc) = (
        v["device_code"].as_str().unwrap().to_string(),
        v["user_code"].as_str().unwrap().to_string(),
    );
    let (s, _) = site
        .post(&owner, &format!("/api/hosted/device/{uc}/deny"), json!({}))
        .await;
    assert_eq!(s, 204);
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!((s, code(&v)), (400, "access_denied"), "{v}");
    // Expiry, on the daemon's clock.
    let v = site.start_device("box").await;
    let (dc, uc) = (
        v["device_code"].as_str().unwrap().to_string(),
        v["user_code"].as_str().unwrap().to_string(),
    );
    site.clock.advance(600);
    let (s, _) = site.get(&owner, &format!("/api/hosted/device/{uc}")).await;
    assert_eq!(s, 404);
    let (s, v) = site.poll_device(&dc).await;
    assert_eq!((s, code(&v)), (400, "expired_token"), "{v}");
    let (s, v) = site.poll_device("cxd_unknown").await;
    assert_eq!((s, code(&v)), (400, "expired_token"), "{v}");
    let (s, _) = site.get(&owner, "/api/hosted/device/ABCD-1234").await;
    assert_eq!(s, 404);
    // A guest approves nothing.
    let mut guest = Browser::default();
    let aid = site.artifact().await;
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/link"),
            json!({"access": "commenter"}),
        )
        .await;
    assert_eq!(s, 200);
    site.sign_in_put(&mut guest, json!({"display_name": "Gus"}))
        .await;
    let v = site.start_device("box").await;
    let uc = v["user_code"].as_str().unwrap();
    let (s, v) = site
        .post(
            &guest,
            &format!("/api/hosted/device/{uc}/approve"),
            json!({}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
}

#[tokio::test]
async fn starting_device_sign_ins_is_rate_limited_per_client() {
    let site = Site::new().await;
    for _ in 0..5 {
        site.start_device("box").await;
    }
    let (s, v) = site
        .machine("/api/hosted/device/code", json!({"client_name": "box"}))
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
    site.clock.advance(60);
    site.start_device("box").await;
}

#[tokio::test]
async fn a_local_daemon_has_no_device_sign_in() {
    let ts = TestServer::spawn().await;
    let r = ts
        .client
        .post(format!("{}/api/hosted/device/code", ts.base))
        .json(&json!({"client_name": "box"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_token_client_initializing_again_keeps_the_narrowing_of_its_name() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _, a, a_handle) = site.sam_with_agent(&owner, &budget, "writer-a").await;
    let client = |session| TokenAgent {
        site: &site,
        token: a.token.clone(),
        session,
    };
    let mut b = client(None);
    assert_eq!(b.initialize("writer-b").await, 200);
    // Sam narrows writer-a to comments alone.
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &format!("/api/agents/{a_handle}/narrow"),
            json!({"access": [{"type": "clax-comments", "actions": ["read"], "locations": [budget]}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // writer-a initializes again while its narrowed session stands: the new
    // session starts within that narrowing, recorded as inherited.
    let mut a2 = client(None);
    assert_eq!(a2.initialize("writer-a").await, 200);
    assert_ne!(a2.sid(), a.sid());
    let (_, r) = a2.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    let db = rusqlite::Connection::open(site.ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let inherited: Vec<Value> = db
        .prepare("SELECT body FROM audit_events WHERE kind = 'agent.narrow' ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|b| serde_json::from_str(&b.unwrap()).unwrap())
        .filter(|b: &Value| b["session"] == json!(a2.sid()))
        .collect();
    assert_eq!(inherited.len(), 1, "{inherited:?}");
    assert_eq!(inherited[0]["inherited"], json!(true), "{inherited:?}");
    // Another client name of the token inherits nothing.
    let mut b2 = client(None);
    assert_eq!(b2.initialize("writer-b").await, 200);
    let (_, r) = b2.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "withheld", "{r}");
    // The token's live sessions fill its cap, the narrowed ones least
    // recently seen: the next writer-a still starts narrowed, and the cap
    // then ends the oldest.
    let mut fillers = Vec::new();
    for n in 0..clax_store::Store::BEARER_SESSION_CAP - 4 {
        let mut f = client(None);
        assert_eq!(f.initialize(&format!("filler-{n}")).await, 200);
        fillers.push(f);
    }
    let mut a3 = client(None);
    assert_eq!(a3.initialize("writer-a").await, 200);
    let (_, r) = a3.call("read", json!({"url_or_id": budget})).await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    let (s, _) = client(a.session.clone())
        .call("read", json!({"url_or_id": budget}))
        .await;
    assert_eq!(s, 404, "the least recently seen session ended");
    drop((b, fillers));
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `aid`'s live socket from `b`, past its welcome frame.
async fn live_ws(site: &Site, b: &Browser, aid: &str) -> Ws {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let url = format!(
        "{}/api/artifacts/{aid}/live",
        site.ts.base.replacen("http://", "ws://", 1)
    );
    let mut req = url.into_client_request().unwrap();
    let h = req.headers_mut();
    h.insert(
        "host",
        format!("localhost:{}", site.ts.addr.port())
            .parse()
            .unwrap(),
    );
    h.insert("origin", site.origin.parse().unwrap());
    if let Some(c) = &b.cookie {
        h.insert("cookie", format!("clax_session={c}").parse().unwrap());
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) if t.as_str().contains("\"welcome\"") => return ws,
            _ => {}
        }
    }
}

/// The next live batch on `ws` that names `person`'s sender.
async fn batch_naming(ws: &mut Ws, person: &str) -> Value {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
            .await
            .expect("a batch within 10 s")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            let names = v["senders"]
                .as_array()
                .is_some_and(|s| s.iter().any(|x| x["person_id"] == json!(person)));
            if names {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn a_person_who_may_no_longer_read_comments_gets_inbox_items_without_comment_text() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sid = site.ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = site.ts.publish_as(&sid, "Plan", "<h1>Plan</h1>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reads = json!({"type": "clax-artifacts", "actions": ["read"], "locations": [plan]});
    let (mia, mia_id) = site
        .person_with(
            &owner,
            "Mia",
            json!([reads, {"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": [plan]}]),
        )
        .await;
    let (s, t) = site.thread(&mia, &plan, "secret question").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{plan}/threads/{tid}/send"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    site.ts
        .reply_as_agent(&sid, &plan, &tid, "secret answer")
        .await;
    let reply = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap_or_else(|| panic!("items: {v}"))
            .iter()
            .find(|i| i["kind"] == "reply")
            .cloned()
            .unwrap_or_else(|| panic!("a reply item: {v}"))
    };
    let (s, v) = site.get(&mia, "/api/inbox").await;
    assert_eq!(s, 200, "{v}");
    let item = reply(&v);
    assert_eq!(item["reply"]["body"], "secret answer", "{item}");
    let (s, v) = site.get(&mia, "/api/inbox?q=secret").await;
    assert_eq!((s, v["total"].as_u64()), (200, Some(1)), "{v}");
    assert!(item["thread"]["summary"].is_string(), "{item}");
    // The owner takes Mia's comment read at Plan away.
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/people/{mia_id}/rights"),
            json!({"rights": [reads]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site.get(&mia, "/api/inbox").await;
    assert_eq!(s, 200, "{v}");
    let item = reply(&v);
    assert_eq!(item["thread"]["id"], tid.as_str(), "{item}");
    assert_eq!(item["reply"]["body"], Value::Null, "{item}");
    assert_eq!(item["thread"]["summary"], Value::Null, "{item}");
    assert!(!v.to_string().contains("secret"), "{v}");
    let (s, v) = site.get(&mia, "/api/inbox/summary").await;
    assert_eq!(s, 200, "{v}");
    assert!(!v.to_string().contains("secret"), "{v}");
    // A search does not match the text withheld from her, so it cannot be
    // probed word by word; marking a search's items read marks none.
    let (s, v) = site.get(&mia, "/api/inbox?q=secret").await;
    assert_eq!((s, v["total"].as_u64()), (200, Some(0)), "{v}");
    assert_eq!(v["items"], json!([]), "{v}");
    let (s, v) = site.get(&mia, "/api/inbox?q=answer&kind=reply").await;
    assert_eq!((s, v["total"].as_u64()), (200, Some(0)), "{v}");
}

#[tokio::test]
async fn a_person_who_may_not_read_comments_gets_no_comment_text() {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    // Sam reads Plan and not its comments; Mia reads both.
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [plan]}]),
        )
        .await;
    let (mia, mia_id) = site
        .person_with(
            &owner,
            "Mia",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [plan]},
                {"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": [plan]},
            ]),
        )
        .await;
    let (s, t) = site.thread(&owner, &plan, "secret words").await;
    assert_eq!(s, 201, "{t}");
    let (s, v) = site
        .get(&sam, &format!("/api/artifacts/{plan}/threads"))
        .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // The snapshot: Mia's carries the thread, Sam's (the same build) none.
    // (The page's first-load data: tests/hosted_pages.rs.)
    let snapshot = format!("/api/artifacts/{plan}/snapshot");
    let (s, snap) = site.get(&mia, &snapshot).await;
    assert_eq!(s, 200, "{snap}");
    assert!(snap.to_string().contains("secret words"), "{snap}");
    let (s, snap) = site.get(&sam, &snapshot).await;
    assert_eq!(s, 200, "{snap}");
    assert_eq!(snap["threads"], json!([]), "{snap}");
    assert!(!snap.to_string().contains("secret words"), "{snap}");
    assert_eq!(snap["versions"][0]["n"], json!(1), "the rest is there");
    // The stream's artifact topic: versions reach Sam, thread events not.
    let open = |b: &Browser| {
        let r = site.req(b, reqwest::Method::GET, "/api/stream");
        async move {
            let res = r.send().await.unwrap();
            let mut events = clax_testkit::EventReader::from_response(res);
            let (_, ready) = events.next().await;
            (events, ready["stream"].as_str().unwrap().to_string())
        }
    };
    let (mut sam_events, sam_stream) = open(&sam).await;
    let (mut mia_events, mia_stream) = open(&mia).await;
    for (b, sid) in [(&sam, &sam_stream), (&mia, &mia_stream)] {
        let (s, v) = site
            .post(
                b,
                &format!("/api/stream/{sid}"),
                json!({"subscribe": [format!("artifact:{plan}")]}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["topics"], json!([format!("artifact:{plan}")]), "{v}");
    }
    let (s, t) = site.thread(&owner, &plan, "more secret words").await;
    assert_eq!(s, 201, "{t}");
    let r = site
        .ts
        .authed(
            site.ts
                .client
                .post(format!("{}/api/artifacts/{plan}/versions", site.ts.base)),
        )
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<h1>two</h1>", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let e = mia_events.next_named("thread").await;
    assert!(e.to_string().contains("more secret words"), "{e}");
    let (name, e) = sam_events.next().await;
    assert_eq!(name, "version", "Sam's first event is the version: {e}");
    // The live socket: Mia's draft reaches the owner, not Sam, who gets her
    // pointer alone.
    let mut sam_ws = live_ws(&site, &sam, &plan).await;
    let mut owner_ws = live_ws(&site, &owner, &plan).await;
    let mut mia_ws = live_ws(&site, &mia, &plan).await;
    for m in [
        json!({"t": "draft", "thread_id": null, "text": "draft words"}),
        json!({"t": "cursor", "version": 1, "file": "index.html",
               "anchor": {"path": [0], "offset": -1, "x": 0.5, "y": 0.5}, "selection": null}),
    ] {
        mia_ws
            .send(Message::Text(m.to_string().into()))
            .await
            .unwrap();
    }
    let b = batch_naming(&mut sam_ws, &mia_id).await;
    let s = b["senders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["person_id"] == json!(mia_id))
        .unwrap()
        .clone();
    assert!(s.get("draft").is_none() && s.get("typing").is_none(), "{b}");
    loop {
        let b = batch_naming(&mut owner_ws, &mia_id).await;
        if b.to_string().contains("draft words") {
            break;
        }
    }
    // Sam's own draft is relayed to no one: the owner sees his pointer only.
    let (_, sam_me) = site.get(&sam, "/api/viewers/me").await;
    let sam_id = sam_me["viewer"]["public_id"].as_str().unwrap().to_string();
    for m in [
        json!({"t": "draft", "thread_id": null, "text": "sams words"}),
        json!({"t": "cursor", "version": 1, "file": "index.html",
               "anchor": {"path": [0], "offset": -1, "x": 0.2, "y": 0.2}, "selection": null}),
    ] {
        sam_ws
            .send(Message::Text(m.to_string().into()))
            .await
            .unwrap();
    }
    let b = batch_naming(&mut owner_ws, &sam_id).await;
    assert!(!b.to_string().contains("sams words"), "{b}");
}

#[tokio::test]
async fn a_browser_holds_at_most_its_cap_of_streams_and_sockets() {
    use clax_http::socket_caps::MAX_PER_SESSION;
    use futures::SinkExt;
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [plan]}]),
        )
        .await;
    let stream = |b: &Browser| site.req(b, reqwest::Method::GET, "/api/stream").send();
    // Its streams and its live socket count together.
    let mut held = Vec::new();
    for _ in 0..MAX_PER_SESSION - 1 {
        let r = stream(&sam).await.unwrap();
        assert_eq!(r.status(), 200);
        held.push(r);
    }
    let mut ws = live_ws(&site, &sam, &plan).await;
    let (s, v) = json_of(stream(&sam).await.unwrap()).await;
    assert_eq!((s, code(&v)), (429, "too_many_streams"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("this browser holds 32"),
        "{v}"
    );
    assert_eq!(live_socket(&site, &sam, &plan).await, Err(429));
    // Another browser is not held to Sam's.
    let r = stream(&owner).await.unwrap();
    assert_eq!(r.status(), 200);
    // A closed connection gives its place back.
    ws.send(tokio_tungstenite::tungstenite::Message::Close(None))
        .await
        .unwrap();
    drop(ws);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let r = loop {
        let r = stream(&sam).await.unwrap();
        if r.status() == 200 {
            break r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the closed socket's place within 10 s"
        );
        tokio::task::yield_now().await;
    };
    held.push(r);
    assert_eq!(held.len(), MAX_PER_SESSION);
}

impl Site {
    async fn put(&self, b: &Browser, path: &str, body: Value) -> (u16, Value) {
        self.send(b, reqwest::Method::PUT, path, body).await
    }

    /// The public ID `b` is signed in as.
    async fn pid(&self, b: &Browser) -> String {
        let (_, v) = self.get(b, "/api/hosted/me").await;
        v["person"]["public_id"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn an_artifact_shared_with_a_person_opens_to_them_at_its_access_alone() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    // An invite without rights gives a person their own space alone.
    let sam = site.member(&owner, "Sam").await;
    let sam_pid = site.pid(&sam).await;
    let (_, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(listed(&v), Vec::<String>::new());
    let sharing = format!("/api/hosted/artifacts/{plan}/sharing");
    let shares = format!("/api/hosted/artifacts/{plan}/shares");
    let (s, _) = site.get(&sam, &sharing).await;
    assert_eq!(s, 404, "an artifact Sam may not read is not there");
    let (s, v) = site.get(&owner, &sharing).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (&v["can_share"], &v["general"], &v["people"]),
        (&json!(true), &json!("restricted"), &json!([]))
    );
    assert!(
        v["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["person"] == json!(sam_pid))
    );
    // Can view.
    let (s, v) = site
        .put(
            &owner,
            &shares,
            json!({"person": sam_pid, "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let share_id = v["share"]["id"].as_str().unwrap().to_string();
    let (_, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(
        listed(&v),
        vec![plan.clone()],
        "the sharee's gallery holds it, and only it"
    );
    for p in [format!("/a/{plan}"), format!("/api/artifacts/{plan}")] {
        assert_eq!(site.get(&sam, &p).await.0, 200, "{p}");
    }
    assert_eq!(
        site.get(&sam, &format!("/api/artifacts/{budget}")).await.0,
        404
    );
    let (s, v) = site.get(&sam, &sharing).await;
    assert_eq!((s, &v["can_share"]), (200, &json!(false)), "{v}");
    assert!(v.get("people").is_none(), "a non-sharer sees no one: {v}");
    let (s, _) = site.thread(&sam, &plan, "a viewer").await;
    assert_eq!(s, 403, "a viewer does not comment");
    // Can comment.
    site.put(
        &owner,
        &shares,
        json!({"person": sam_pid, "access": "commenter"}),
    )
    .await;
    let (s, t) = site.thread(&sam, &plan, "a commenter").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let resolve = format!("/api/artifacts/{plan}/threads/{tid}/resolve");
    let (s, v) = site.post(&sam, &resolve, json!({})).await;
    assert_eq!(
        (s, code(&v)),
        (403, "not_permitted"),
        "a commenter does not resolve"
    );
    let mut agent = site.token_agent(&sam, "sam-laptop").await;
    assert_eq!(agent.initialize("writer").await, 200);
    let edit = json!({"id": plan, "html": "<h1>Sam's</h1>", "if_version": 1});
    let (_, r) = agent.call("publish", edit.clone()).await;
    assert_eq!(
        tool_code(&r),
        "not_permitted",
        "a commenter does not publish: {r}"
    );
    // Can edit: resolve, publish through their agent; never share, change
    // general access or delete.
    let (s, v) = site
        .put(
            &owner,
            &shares,
            json!({"person": sam_pid, "access": "editor"}),
        )
        .await;
    assert_eq!(
        (s, &v["share"]["id"]),
        (200, &json!(share_id)),
        "one share, changed"
    );
    assert_eq!(site.post(&sam, &resolve, json!({})).await.0, 200);
    let (_, r) = agent.call("publish", edit).await;
    assert_eq!(tool_code(&r), "", "an editor's agent publishes: {r}");
    // Edit changes contents alone: the title, not the pin or capabilities.
    let (_, r) = agent
        .call(
            "publish",
            json!({"id": plan, "html": "<h1>Sam's</h1>", "title": "Sam's plan", "capabilities": {}, "if_version": 2}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "the capabilities as they are pass: {r}");
    let (_, r) = agent
        .call(
            "publish",
            json!({"id": plan, "html": "<h1>db</h1>", "capabilities": {"db": {}}, "if_version": 3}),
        )
        .await;
    assert_eq!(tool_code(&r), "admin_only", "{r}");
    let (_, r) = agent.call("pin", json!({"url_or_id": plan})).await;
    assert_eq!(tool_code(&r), "admin_only", "{r}");
    let (_, r) = agent.call("unpin", json!({"url_or_id": plan})).await;
    assert_eq!(tool_code(&r), "", "unpinned already, no change: {r}");
    let plan_id = clax_core::ArtifactId::parse(&plan).unwrap();
    let a = site.ts.store.get_artifact(&plan_id).unwrap().unwrap();
    assert_eq!(
        (
            a.title.as_str(),
            a.pinned,
            &a.capabilities,
            a.current_version
        ),
        ("Sam's plan", false, &json!({}), 3)
    );
    let (_, r) = agent
        .call(
            "publish",
            json!({"id": budget, "html": "<h1>no</h1>", "if_version": 1}),
        )
        .await;
    assert_eq!(
        tool_code(&r),
        "not_permitted",
        "not at another artifact: {r}"
    );
    let (s, v) = site
        .put(
            &sam,
            &shares,
            json!({"github_login": "friend", "access": "viewer"}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, _) = site
        .put(
            &sam,
            &format!("/api/hosted/artifacts/{plan}/link"),
            json!({"access": "viewer"}),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{plan}/shares/{share_id}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::DELETE,
            &format!("/api/artifacts/{plan}"),
            json!(null),
        )
        .await;
    assert!(s >= 400, "an editor does not delete: {s}");
    assert!(
        site.ts
            .store
            .get_artifact(&clax_core::ArtifactId::parse(&plan).unwrap())
            .unwrap()
            .is_some()
    );
    // Bad bodies; and every change needs the public origin.
    for body in [
        json!({"access": "viewer"}),
        json!({"person": sam_pid, "github_login": "x", "access": "viewer"}),
        json!({"person": sam_pid, "access": "owner"}),
    ] {
        let (s, v) = site.put(&owner, &shares, body.clone()).await;
        assert_eq!((s, code(&v)), (400, "invalid_args"), "{body}");
    }
    let (s, v) = site
        .put(
            &owner,
            &shares,
            json!({"github_login": "-bad", "access": "viewer"}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_login"));
    let res = site
        .ts
        .client
        .put(site.url(&shares))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .header(
            COOKIE,
            format!("clax_session={}", owner.cookie.as_deref().unwrap()),
        )
        .json(&json!({"github_login": "octo", "access": "viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "no Origin, no change");
    // A login no one signed in with waits.
    let (s, v) = site
        .put(
            &owner,
            &shares,
            json!({"github_login": "@Octo", "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (&v["share"]["github_login"], &v["share"]["pending"]),
        (&json!("octo"), &json!(true))
    );
    // Removed, the artifact is gone from Sam's view.
    let (s, _) = site
        .send(
            &owner,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{plan}/shares/{share_id}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        site.get(&sam, &format!("/api/artifacts/{plan}")).await.0,
        404
    );
    let (_, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!(listed(&v), Vec::<String>::new());
    // The maker shares their own artifact; Mia, shared nothing, sees none
    // of the others.
    let (_, r) = agent
        .call(
            "publish",
            json!({"html": "<h1>Sam's own</h1>", "title": "Notes"}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    let notes = site
        .ts
        .store
        .list_artifacts()
        .unwrap()
        .into_iter()
        .find(|a| a.title == "Notes")
        .unwrap()
        .id;
    let mia = site.member(&owner, "Mia").await;
    let mia_pid = site.pid(&mia).await;
    let (_, v) = site.get(&mia, "/api/artifacts").await;
    assert_eq!(listed(&v), Vec::<String>::new());
    // An artifact Mia may not read is not there to her sharing routes
    // either: 404, as from its own routes, never 403.
    let (s, v) = site
        .put(
            &mia,
            &shares,
            json!({"github_login": "friend", "access": "viewer"}),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = site
        .put(
            &mia,
            &format!("/api/hosted/artifacts/{plan}/link"),
            json!({"access": "viewer"}),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = site
        .send(
            &mia,
            reqwest::Method::DELETE,
            &format!("/api/hosted/artifacts/{plan}/shares/{share_id}"),
            json!(null),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = site
        .get(&sam, &format!("/api/hosted/artifacts/{notes}/sharing"))
        .await;
    assert_eq!(
        (s, &v["can_share"], &v["suggestions"]),
        (200, &json!(true), &json!([])),
        "{v}"
    );
    let (s, _) = site
        .put(
            &sam,
            &format!("/api/hosted/artifacts/{notes}/shares"),
            json!({"person": mia_pid, "access": "commenter"}),
        )
        .await;
    assert_eq!(s, 200);
    let (_, v) = site.get(&mia, "/api/artifacts").await;
    assert_eq!(listed(&v), vec![notes.clone()]);
}

#[tokio::test]
async fn the_public_gallery_lists_public_artifacts_alone_to_anyone() {
    let site = Site::with_public_gallery().await;
    let owner = site.owner().await;
    let anon = Browser::default();
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    let _secret = site.titled("Secret").await;
    let link = |aid: &str| format!("/api/hosted/artifacts/{aid}/link");
    let (s, v) = site.get(&anon, "/api/hosted/public").await;
    assert_eq!((s, v), (200, json!({"artifacts": []})));
    let (s, v) = site
        .put(
            &owner,
            &link(&plan),
            json!({"access": "viewer", "listed": true}),
        )
        .await;
    assert_eq!((s, &v["listed"]), (200, &json!(true)), "{v}");
    site.put(&owner, &link(&budget), json!({"access": "commenter"}))
        .await;
    let (_, v) = site.get(&anon, "/api/hosted/public").await;
    let arts = v["artifacts"].as_array().unwrap();
    assert_eq!(arts.len(), 1, "{v}");
    assert_eq!(arts[0]["artifact_id"], json!(plan));
    assert_eq!(arts[0]["title"], json!("Plan"));
    assert!(
        arts[0]["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/a/{plan}"))
    );
    assert_eq!(
        arts[0].as_object().unwrap().len(),
        3,
        "its ID, title and URL alone"
    );
    let (_, v) = site
        .get(&owner, &format!("/api/hosted/artifacts/{plan}/sharing"))
        .await;
    assert_eq!(
        (&v["general"], &v["link_access"]),
        (&json!("public"), &json!("viewer"))
    );
    let (_, v) = site
        .get(&owner, &format!("/api/hosted/artifacts/{budget}/sharing"))
        .await;
    assert_eq!(v["general"], json!("link"));
    // A signed-out visitor opens it through its link.
    assert_eq!(
        site.get(&anon, &format!("/api/artifacts/{plan}")).await.0,
        200
    );
    // Only a page shared by link is listed; stopping unlists it.
    let (s, v) = site
        .put(
            &owner,
            &link(&plan),
            json!({"access": null, "listed": true}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"));
    let (_, v) = site
        .put(&owner, &link(&plan), json!({"access": "commenter"}))
        .await;
    assert_eq!(
        v["listed"],
        json!(true),
        "changing the access keeps the listing"
    );
    site.put(&owner, &link(&plan), json!({"access": null}))
        .await;
    let (_, v) = site.get(&anon, "/api/hosted/public").await;
    assert_eq!(v, json!({"artifacts": []}));
    // Someone other than the owner or the maker may not list it.
    let sam = site.member(&owner, "Sam").await;
    let (s, _) = site
        .put(
            &sam,
            &link(&budget),
            json!({"access": "viewer", "listed": true}),
        )
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn with_the_public_gallery_off_no_one_lists_a_page() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let anon = Browser::default();
    let plan = site.titled("Plan").await;
    let link = format!("/api/hosted/artifacts/{plan}/link");
    let (s, v) = site
        .put(&owner, &link, json!({"access": "viewer", "listed": true}))
        .await;
    assert_eq!((s, code(&v)), (403, "public_gallery_off"), "{v}");
    let (s, v) = site
        .put(&owner, &link, json!({"access": "viewer", "listed": false}))
        .await;
    assert_eq!((s, &v["listed"]), (200, &json!(false)), "{v}");
    let (_, v) = site
        .get(&owner, &format!("/api/hosted/artifacts/{plan}/sharing"))
        .await;
    assert_eq!(
        (&v["general"], &v["public_gallery"]),
        (&json!("link"), &json!(false)),
        "{v}"
    );
    let (s, v) = site.get(&anon, "/api/hosted/public").await;
    assert_eq!((s, v), (200, json!({"artifacts": []})));
    // A signed-out visitor still opens it through its link.
    assert_eq!(
        site.get(&anon, &format!("/api/artifacts/{plan}")).await.0,
        200
    );
}

impl Site {
    /// `b`'s `/api/stream`, as the shell opens it, subscribed to `topics`.
    async fn stream(&self, b: &Browser, topics: &[&str]) -> clax_testkit::EventReader {
        let res = self
            .req(b, reqwest::Method::GET, "/api/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let mut events = clax_testkit::EventReader::from_response(res);
        let sid = events.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let (s, v) = self
            .post(
                b,
                &format!("/api/stream/{sid}"),
                json!({"subscribe": topics}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        events
    }

    /// The IDs of `b`'s open questions (`GET /api/questions`).
    async fn open_questions(&self, b: &Browser) -> Vec<String> {
        let (s, v) = self.get(b, "/api/questions").await;
        assert_eq!(s, 200, "{v}");
        v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap().to_string())
            .collect()
    }
}

/// The `questions` argument of an `ask` with two options.
fn ask_which(question: &str) -> Value {
    json!([{"question": question, "header": "Pick",
        "options": [{"label": "A"}, {"label": "B"}]}])
}

/// The question view of the next `question` event on `stream`, which must
/// come while `ask` (an `ask` tool call) is still waiting; a call that
/// returns first fails the test with its result.
async fn asked_while_waiting<F>(
    ask: &mut std::pin::Pin<&mut F>,
    stream: &mut clax_testkit::EventReader,
) -> Value
where
    F: std::future::Future<Output = (u16, Value)>,
{
    tokio::select! {
        r = ask.as_mut() => panic!("ask returned before its question was announced: {r:?}"),
        d = stream.next_named("question") => d["question"].clone(),
    }
}

/// Sam's agent token asks over the MCP adapter; Kim, another member,
/// watches their own questions throughout. Both have a member's default
/// rights (their own space), which let an agent ask its own person. The question is Sam's
/// alone: it reaches Sam's list and stream, never Kim's, and Sam's answer
/// is what the blocked `ask` returns. Kim's own agent's question is the
/// first `question` event Kim's stream carries, so nothing of Sam's came
/// before it.
#[tokio::test]
async fn an_agent_tokens_ask_reaches_its_person_alone_and_returns_their_answer() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.member(&owner, "Sam").await;
    let kim = site.member(&owner, "Kim").await;
    let mut sam_stream = site.stream(&sam, &["questions"]).await;
    let mut kim_stream = site.stream(&kim, &["questions"]).await;
    let mut agent = site.token_agent(&sam, "sam-laptop").await;
    assert_eq!(agent.initialize("planner").await, 200);
    let ask = agent.call(
        "ask",
        json!({"questions": ask_which("Which plan?"), "timeout_s": 30}),
    );
    tokio::pin!(ask);
    let q = asked_while_waiting(&mut ask, &mut sam_stream).await;
    let qid = q["id"].as_str().unwrap().to_string();
    assert_eq!(q["status"], "open", "{q}");
    assert!(q["artifact"].is_null(), "{q}");
    // The ask is held before Sam answers, so the answer is what it returns
    // rather than what a later poll picks up.
    site.ts.wait_question_waiters(&qid, 1).await;
    assert_eq!(site.open_questions(&sam).await, vec![qid.clone()]);
    assert_eq!(site.open_questions(&kim).await, Vec::<String>::new());
    // Kim neither reads nor answers it.
    let (s, _) = site.get(&kim, &format!("/api/questions/{qid}")).await;
    assert_eq!(s, 404);
    let (s, _) = site
        .post(
            &kim,
            &format!("/api/questions/{qid}/answer"),
            json!({"answers": [{"selected": ["B"]}]}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/questions/{qid}/answer"),
            json!({"answers": [{"selected": ["A"]}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["question"]["status"], "answered", "{v}");
    let (s, r) = ask.await;
    assert_eq!((s, tool_code(&r).as_str()), (200, ""), "{r}");
    let out = tool_json(&r);
    assert_eq!(
        (&out["question_id"], &out["status"]),
        (&json!(qid), &json!("answered")),
        "{out}"
    );
    assert_eq!(out["reply"][0]["question"], "Which plan?", "{out}");
    assert_eq!(out["reply"][0]["selected"], json!(["A"]), "{out}");
    // Sam's stream tells of the answer too.
    let q = sam_stream.next_named("question").await["question"].clone();
    assert_eq!((&q["id"], &q["status"]), (&json!(qid), &json!("answered")));
    assert_eq!(site.open_questions(&sam).await, Vec::<String>::new());
    // Kim's own agent asks, and Kim declines: the first question Kim's
    // stream carries is that one.
    let mut kims = site.token_agent(&kim, "kim-laptop").await;
    assert_eq!(kims.initialize("planner").await, 200);
    let ask = kims.call(
        "ask",
        json!({"questions": ask_which("Which budget?"), "timeout_s": 30}),
    );
    tokio::pin!(ask);
    let q = asked_while_waiting(&mut ask, &mut kim_stream).await;
    let kid = q["id"].as_str().unwrap().to_string();
    assert_ne!(kid, qid, "Sam's question reached Kim's stream");
    site.ts.wait_question_waiters(&kid, 1).await;
    assert_eq!(site.open_questions(&kim).await, vec![kid.clone()]);
    let (s, v) = site
        .post(&kim, &format!("/api/questions/{kid}/decline"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, r) = ask.await;
    let out = tool_json(&r);
    assert_eq!(
        (&out["question_id"], &out["status"], &out["reply"]),
        (&json!(kid), &json!("declined"), &Value::Null),
        "{out}"
    );
}

/// An agent token's `ask` about an artifact its person may read names that
/// artifact in the question; about one they may not read it is refused and
/// nothing is asked.
#[tokio::test]
async fn an_agent_tokens_ask_about_an_artifact_names_it_within_its_persons_rights() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let budget = site.titled("Budget").await;
    let (sam, _) = site
        .person_with(
            &owner,
            "Sam",
            json!([
                {"type": "clax-artifacts", "actions": ["read"], "locations": [budget]},
                {"type": "clax-ask", "actions": ["ask"]},
            ]),
        )
        .await;
    let mut agent = site.token_agent(&sam, "sam-laptop").await;
    assert_eq!(agent.initialize("planner").await, 200);
    let (_, r) = agent
        .call(
            "ask",
            json!({"questions": ask_which("Which plan?"), "url_or_id": plan, "timeout_s": 30}),
        )
        .await;
    assert_eq!(tool_code(&r), "not_permitted", "{r}");
    assert_eq!(site.open_questions(&sam).await, Vec::<String>::new());
    let mut sam_stream = site.stream(&sam, &["questions"]).await;
    let ask = agent.call(
        "ask",
        json!({"questions": ask_which("Which line?"), "url_or_id": budget, "timeout_s": 30}),
    );
    tokio::pin!(ask);
    let q = asked_while_waiting(&mut ask, &mut sam_stream).await;
    assert_eq!(q["artifact"]["id"], budget, "{q}");
    assert_eq!(q["artifact"]["title"], "Budget", "{q}");
    let qid = q["id"].as_str().unwrap().to_string();
    site.ts.wait_question_waiters(&qid, 1).await;
    let (s, v) = site.get(&sam, "/api/questions").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["questions"][0]["id"], json!(qid), "{v}");
    assert_eq!(v["questions"][0]["artifact"]["id"], budget, "{v}");
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/questions/{qid}/answer"),
            json!({"answers": [{"text": "the second line"}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, r) = ask.await;
    assert_eq!((s, tool_code(&r).as_str()), (200, ""), "{r}");
    let out = tool_json(&r);
    assert_eq!(
        (&out["question_id"], &out["status"]),
        (&json!(qid), &json!("answered")),
        "{out}"
    );
    assert_eq!(out["reply"][0]["question"], "Which line?", "{out}");
    assert_eq!(out["reply"][0]["text"], "the second line", "{out}");
}

/// What an agent token's agent is handed links the hosted Clax at its
/// public origin, the address its person's browser opens, never the
/// address the daemon listens on.
#[tokio::test]
async fn an_agent_tokens_feedback_links_the_public_origin() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _, agent, handle) = site.sam_with_agent(&owner, &budget, "summary-writer").await;
    let (s, t) = site.thread(&owner, &budget, "please total the rows").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{budget}/threads/{tid}/send"),
            json!({"to": handle}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // Sam lets the owner's words reach his agent.
    let (_, inbox) = site.get(&sam, "/api/inbox").await;
    let rid = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "approval")
        .map(|i| i["approval"]["id"].as_str().unwrap().to_string())
        .unwrap_or_else(|| panic!("an approval card in {inbox}"));
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/approvals/requests/{rid}/decide"),
            json!({"scope": "thread"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, r) = agent
        .call(
            "wait_for_feedback",
            json!({"url_or_id": budget, "timeout_s": 5}),
        )
        .await;
    let text = r.to_string();
    assert!(text.contains("please total the rows"), "{text}");
    assert!(
        text.contains(&format!("{}/a/{budget}", site.origin)),
        "{text}"
    );
    assert!(!text.contains(&site.ts.base), "{text}");
}

impl Site {
    /// A thread `b` starts on `aid` with clip `png`, as the shell does: its ID.
    async fn thread_with_clip(&self, b: &Browser, aid: &str, body: &str, png: &[u8]) -> String {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1")
            .part(
                "clip",
                reqwest::multipart::Part::bytes(png.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        let (s, t) = json_of(
            self.req(
                b,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/threads"),
            )
            .multipart(form)
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(s, 201, "{t}");
        t["thread"]["id"].as_str().unwrap().to_string()
    }

    /// Sam's browser approves the owner's content for his agents.
    async fn approve_owner(&self, owner: &Browser, sam: &Browser) {
        let (_, me) = self.get(owner, "/api/viewers/me").await;
        let owner_id = me["viewer"]["public_id"].as_str().unwrap().to_string();
        let (s, v) = self
            .post(
                sam,
                "/api/approvals",
                json!({"author": owner_id, "scope": "everywhere"}),
            )
            .await;
        assert!(s == 201 || s == 200, "{v}");
    }
}

/// The image blocks of a tool result.
fn images(r: &Value) -> Vec<Value> {
    r["content"]
        .as_array()
        .map(|c| c.iter().filter(|b| b["type"] == "image").cloned().collect())
        .unwrap_or_default()
}

/// A PNG signature and an `IHDR` chunk of `w` by `h` pixels.
fn png_of(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend(b"clax-test-clip");
    b
}

#[tokio::test]
async fn an_agent_tokens_agent_reads_a_clip_by_link_in_listings_and_as_an_image_in_its_thread() {
    use base64::Engine as _;
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _, agent, handle) = site.sam_with_agent(&owner, &budget, "clip-reader").await;
    site.approve_owner(&owner, &sam).await;
    let png = png_of(640, 480);
    let tid = site
        .thread_with_clip(&sam, &budget, "the totals overflow", &png)
        .await;
    // A listing names the clip by its keyed address and a note; it attaches
    // no image, and names neither a path nor a clip reference.
    let (_, r) = agent
        .call("comments_read", json!({"url_or_id": budget}))
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    assert!(images(&r).is_empty(), "{r}");
    let listed = tool_json(&r);
    let t = &listed["threads"][0];
    assert_eq!(t["thread_id"], json!(tid), "{listed}");
    assert_eq!(t["clip_path"], Value::Null, "{t}");
    let url = t["clip"]["url"].as_str().expect("a clip url").to_string();
    assert!(
        url.starts_with(&format!("{}/_clip/{budget}/{tid}?k=", site.origin)),
        "{url}"
    );
    assert_eq!(t["clip"]["note"], clax_core::clips::LINK_NOTE, "{t}");
    assert!(!r.to_string().contains("clax-clip:"), "{r}");
    // The address serves the PNG with no credential, cached as immutable;
    // a wrong key, or none, reads nothing.
    let res = site.ts.client.get(&url).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    let cache = res.headers()["cache-control"].to_str().unwrap().to_string();
    assert!(cache.contains("immutable"), "{cache}");
    assert_eq!(res.bytes().await.unwrap().as_ref(), png.as_slice());
    let (bare, _) = url.split_once("?k=").unwrap();
    for wrong in [format!("{bare}?k=00"), bare.to_string()] {
        let res = site.ts.client.get(&wrong).send().await.unwrap();
        assert_eq!(res.status(), 404, "{wrong}");
    }
    // Reading the one thread attaches the clip as an image.
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": budget, "thread_id": tid}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    let imgs = images(&r);
    assert_eq!(imgs.len(), 1, "{r}");
    assert_eq!(imgs[0]["mimeType"], "image/png");
    assert_eq!(
        imgs[0]["data"],
        json!(base64::engine::general_purpose::STANDARD.encode(&png))
    );
    let one = tool_json(&r);
    assert_eq!(one["threads"][0]["clip"]["url"], json!(url), "{one}");
    assert_eq!(
        one["threads"][0]["clip"]["note"],
        clax_core::clips::ATTACHED_NOTE
    );
    // Sent to the agent, the comment's payload names the clip's address,
    // never a reference it cannot resolve.
    let (s, v) = site
        .post(
            &sam,
            &format!("/api/artifacts/{budget}/threads/{tid}/send"),
            json!({"to": handle}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, r) = agent
        .call(
            "wait_for_feedback",
            json!({"url_or_id": budget, "timeout_s": 5}),
        )
        .await;
    let text = r.to_string();
    assert!(text.contains("the totals overflow"), "{r}");
    assert!(!text.contains("clax-clip:"), "{r}");
    let out = tool_json(&r);
    let item = &out["feedback"][0];
    assert_eq!(item["clip_path"], Value::Null, "{item}");
    assert_eq!(item["clip"]["url"], json!(url), "{item}");
    assert!(
        text.contains(&format!("Clip: {url} (")),
        "the payload's Clip line names the address: {r}"
    );
}

#[tokio::test]
async fn an_oversized_clip_is_noted_and_not_attached() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let (sam, _, agent, _) = site.sam_with_agent(&owner, &budget, "clip-reader").await;
    site.approve_owner(&owner, &sam).await;
    let side = clax_core::clips::INLINE_MAX_SIDE + 1;
    let tid = site
        .thread_with_clip(&sam, &budget, "too wide", &png_of(side, 10))
        .await;
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": budget, "thread_id": tid}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    assert!(images(&r).is_empty(), "{r}");
    let t = &tool_json(&r)["threads"][0];
    assert!(t["clip"]["url"].is_string(), "{t}");
    let note = t["clip"]["note"].as_str().unwrap();
    assert!(note.contains("not attached"), "{note}");
    assert!(note.contains(&format!("{side}x10 pixels")), "{note}");
}

#[tokio::test]
async fn a_clip_withheld_from_an_agent_or_on_an_artifact_it_may_not_read_reaches_it_not() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let budget = site.titled("Budget").await;
    let plan = site.titled("Plan").await;
    let (sam, _, agent, _) = site.sam_with_agent(&owner, &budget, "clip-reader").await;
    // Sam has not approved the owner: the owner's clip is withheld from his
    // agent, so no address names it and nothing is attached.
    let tid = site
        .thread_with_clip(&owner, &budget, "owner's note", &png_of(8, 8))
        .await;
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": budget, "thread_id": tid}),
        )
        .await;
    assert_eq!(tool_code(&r), "", "{r}");
    assert!(images(&r).is_empty(), "{r}");
    let t = &tool_json(&r)["threads"][0];
    assert_eq!(t["clip_withheld"], true, "{t}");
    assert!(t.get("clip").is_none(), "{t}");
    assert!(!r.to_string().contains("/_clip/"), "{r}");
    // Plan is not Sam's to read: neither its threads nor their clips reach
    // his agent.
    let ptid = site
        .thread_with_clip(&owner, &plan, "plan note", &png_of(8, 8))
        .await;
    let (_, r) = agent
        .call(
            "comments_read",
            json!({"url_or_id": plan, "thread_id": ptid}),
        )
        .await;
    assert_ne!(tool_code(&r), "", "{r}");
    assert!(images(&r).is_empty(), "{r}");
    assert!(!r.to_string().contains("/_clip/"), "{r}");
    let _ = sam;
}

#[tokio::test]
async fn a_local_daemons_agent_reads_its_clip_by_path_as_before() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Doc", &[("index.html", "<h1>x</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .create_thread(&aid, 1, "tighten", Some(clax_testkit::FAKE_PNG))
        .await;
    assert_eq!(res.status(), 201);
    let t: Value = res.json().await.unwrap();
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "comments_read", "arguments": {"url_or_id": aid, "thread_id": tid}}});
    let res = clax_testkit::mcp::mcp_post(&ts, clax_testkit::mcp::ADAPTER, &call)
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    let r = clax_testkit::mcp::rpc_message(res).await["result"].clone();
    assert_eq!(tool_code(&r), "", "{r}");
    assert!(images(&r).is_empty(), "{r}");
    let t = &tool_json(&r)["threads"][0];
    let path = t["clip_path"].as_str().expect("the clip's file");
    assert!(std::path::Path::new(path).is_file(), "{path}");
    assert!(t.get("clip").is_none(), "{t}");
    // A local daemon serves no keyed clip address.
    let res = ts
        .client
        .get(format!("{}/_clip/{aid}/{tid}?k=00", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

/// `GET path` on the hosted test server with `Host: host`.
async fn get_at(site: &Site, host: &str, path: &str) -> reqwest::Response {
    site.ts
        .client
        .get(site.url(path))
        .header(HOST, host)
        .send()
        .await
        .unwrap()
}

/// The path and query of `url`, after its origin.
fn path_of(url: &str) -> String {
    let rest = url.split_once("://").unwrap().1;
    rest[rest.find('/').unwrap()..].to_string()
}

#[tokio::test]
async fn a_clip_address_is_private_to_caches_and_ends_with_its_thread_or_artifact() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let host = format!("localhost:{}", site.ts.addr.port());
    for gone in ["thread", "artifact"] {
        let aid = site.titled("Budget").await;
        let tid = site
            .thread_with_clip(&owner, &aid, "a note", &png_of(8, 8))
            .await;
        let url = clax_http::hosted::clip_link(&site.ts.state, &aid, &tid).unwrap();
        let r = get_at(&site, &host, &path_of(&url)).await;
        assert_eq!(r.status(), 200, "{gone}");
        assert_eq!(
            r.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        let s = if gone == "thread" {
            let path = format!("/api/artifacts/{aid}/threads/{tid}");
            site.send(&owner, reqwest::Method::DELETE, &path, json!(null))
                .await
                .0
        } else {
            let url = format!("{}/api/artifacts/{aid}", site.ts.base);
            let req = site
                .ts
                .authed(site.ts.client.delete(url))
                .header(HOST, &host);
            req.send().await.unwrap().status().as_u16()
        };
        assert!(s == 200 || s == 204, "{gone}: {s}");
        let r = get_at(&site, &host, &path_of(&url)).await;
        assert_eq!(r.status(), 404, "after its {gone} is deleted");
    }
}

#[tokio::test]
async fn with_a_content_origin_a_clip_is_served_there_alone() {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with_content(clock.clone(), Some(SECRET)).await;
    let port = ts.addr.port();
    let origin = format!("http://localhost:{port}");
    let site = Site { ts, clock, origin };
    let owner = site.owner().await;
    let aid = site.titled("Budget").await;
    let png = png_of(8, 8);
    let tid = site.thread_with_clip(&owner, &aid, "a note", &png).await;
    let url = clax_http::hosted::clip_link(&site.ts.state, &aid, &tid).unwrap();
    assert!(
        url.starts_with(&format!("http://[::1]:{port}/_clip/")),
        "{url}"
    );
    let r = get_at(&site, &format!("[::1]:{port}"), &path_of(&url)).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap().as_ref(), png.as_slice());
    let r = get_at(&site, &format!("localhost:{port}"), &path_of(&url)).await;
    assert_eq!(r.status(), 404, "never on the public origin");
}

#[tokio::test]
async fn a_live_pages_clip_is_named_by_no_address() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.titled("Budget").await;
    let tid = site
        .thread_with_clip(&owner, &aid, "a note", &png_of(8, 8))
        .await;
    let read = || async {
        let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "comments_read", "arguments": {"url_or_id": aid, "thread_id": tid}}});
        let res = clax_testkit::mcp::mcp_post(&site.ts, clax_testkit::mcp::ADAPTER, &call)
            .header(HOST, format!("localhost:{}", site.ts.addr.port()))
            .bearer_auth(&site.ts.token)
            .send()
            .await
            .unwrap();
        let r = clax_testkit::mcp::rpc_message(res).await["result"].clone();
        assert_eq!(tool_code(&r), "", "{r}");
        tool_json(&r)["threads"][0].clone()
    };
    // The owner's agent on the hosted machine is named the address...
    assert!(read().await["clip"]["url"].is_string());
    // ...but not once the artifact is a live page, whose clip the address
    // does not serve to a caller with no credential.
    site.ts.state.live_ids.insert(&aid, "http://localhost:5173");
    let t = read().await;
    assert!(t.get("clip").is_none(), "{t}");
}

#[tokio::test]
async fn a_live_page_is_hidden_from_visitors_though_they_connect_from_loopback() {
    // The test's client connects from 127.0.0.1, as every visitor does
    // through a proxy on the hosted daemon's own machine.
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/link"),
            json!({"access": "commenter"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let anon = Browser::default();
    let (s, v) = site.get(&anon, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(s, 200, "{v}");
    site.ts.state.live_ids.insert(&aid, "http://localhost:5173");
    let (s, v) = site.get(&anon, &format!("/api/artifacts/{aid}")).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

#[tokio::test]
async fn an_instance_without_documents_shows_none_and_takes_no_agreement() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let (s, v) = site.get(&owner, "/api/hosted/me").await;
    assert_eq!((s, &v["legal"]), (200, &Value::Null), "{v}");
    let (s, v) = site
        .post(&owner, "/api/hosted/legal/agree", json!({"version": "1"}))
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

#[tokio::test]
async fn signing_in_under_the_notice_agrees_and_the_prompt_records_the_rest() {
    let site = Site::with_legal(clax_core::config::Legal::Own {
        terms_url: "https://example.org/terms".into(),
        privacy_url: "https://example.org/privacy".into(),
        version: "3".into(),
    })
    .await;
    let (s, v) = site.get(&Browser::default(), "/api/hosted/me").await;
    assert_eq!(s, 200);
    assert_eq!(
        v["legal"],
        json!({"terms_url": "https://example.org/terms", "privacy_url": "https://example.org/privacy", "version": "3", "agreed": null})
    );
    // Setup, an invite and a sign-in link each sign in under the notice.
    let owner = site.owner().await;
    let bo = site.member(&owner, "Bo").await;
    let (s, v) = site.post(&bo, "/api/hosted/signin-links", json!({})).await;
    assert_eq!(s, 200, "{v}");
    let code_of = v["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let mut laptop = Browser::default();
    site.sign_in(
        &mut laptop,
        &format!("/api/hosted/signin-links/{code_of}/accept"),
        json!({}),
    )
    .await;
    for b in [&owner, &bo, &laptop] {
        let (_, v) = site.get(b, "/api/hosted/me").await;
        assert_eq!(v["legal"]["agreed"], true, "{v}");
    }
    // A person with no agreement (one who joined before the documents were
    // shown) is asked, and agreeing to an older text is refused.
    let conn = rusqlite::Connection::open(site.ts.home.db_path()).unwrap();
    conn.execute(
        "DELETE FROM legal_agreements WHERE viewer_id = (SELECT id FROM viewers WHERE display_name = 'Bo')",
        [],
    )
    .unwrap();
    let (_, v) = site.get(&bo, "/api/hosted/me").await;
    assert_eq!(v["legal"]["agreed"], false, "{v}");
    let (s, v) = site
        .post(&bo, "/api/hosted/legal/agree", json!({"version": "2"}))
        .await;
    assert_eq!((s, code(&v)), (409, "legal_changed"), "{v}");
    let (s, _) = site
        .post(&bo, "/api/hosted/legal/agree", json!({"version": "3"}))
        .await;
    assert_eq!(s, 204);
    let (_, v) = site.get(&bo, "/api/hosted/me").await;
    assert_eq!(v["legal"]["agreed"], true, "{v}");
    let via: String = conn
        .query_row(
            "SELECT via FROM legal_agreements WHERE viewer_id = (SELECT id FROM viewers WHERE display_name = 'Bo')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(via, "prompt");
    // No session agrees to nothing.
    let (s, _) = site
        .post(
            &Browser::default(),
            "/api/hosted/legal/agree",
            json!({"version": "3"}),
        )
        .await;
    assert_ne!(s, 204);
}

#[tokio::test]
async fn people_start_artifacts_from_briefs_and_guests_may_not() {
    let site = Site::new().await;
    site.ts.seats.set_limits(clax_core::seats::SeatLimits {
        people: 1,
        guests: 1,
        agents: 20,
        idle_ms: 180_000,
    });
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let new = "/api/artifacts:new";
    let brief = |title: &str| json!({"title": title, "brief": "A one-page roadmap for Q4"});
    let may_create = async |b: &Browser| {
        let (s, v) = site.get(b, "/api/viewers/me").await;
        assert_eq!(s, 200, "{v}");
        (v["may_create"].clone(), v["viewer"]["public_id"].clone())
    };
    // The owner's browser.
    assert_eq!(may_create(&owner).await.0, json!(true));
    let (s, v) = site.post(&owner, new, brief("Roadmap")).await;
    assert_eq!(s, 201, "{v}");
    assert!(v["version"]["content_key"].is_string(), "{v}");
    assert_eq!(v["thread"]["anchor"]["kind"], "artifact");
    // A person holding the default rights (their own space) starts one
    // that is theirs, and reads it.
    let mia = site.member(&owner, "Mia").await;
    let (may, mia_id) = may_create(&mia).await;
    assert_eq!(may, json!(true));
    let (s, v) = site.post(&mia, new, brief("Mia's roadmap")).await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["thread"]["comments"][0]["author_public_id"], mia_id);
    let (st, a, pid) = (
        site.ts.store.clone(),
        aid.clone(),
        mia_id.as_str().unwrap().to_string(),
    );
    assert!(
        tokio::task::spawn_blocking(move || st.artifact_created_by(&a, Some(&pid)))
            .await
            .unwrap()
            .unwrap(),
        "the artifact is Mia's"
    );
    let (s, got) = site.get(&mia, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(s, 200, "{got}");
    let (s, threads) = site
        .get(&mia, &format!("/api/artifacts/{aid}/threads"))
        .await;
    assert_eq!(s, 200, "{threads}");
    assert_eq!(threads["threads"].as_array().unwrap().len(), 1);
    // A reader may not, and is offered no New.
    let (sam, _) = site
        .person_with(&owner, "Sam", json!(["clax:reader"]))
        .await;
    assert_eq!(may_create(&sam).await.0, json!(false));
    let (s, v) = site.post(&sam, new, brief("Nope")).await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // Lee's stream takes Plan's one seat, so Kim is a spectator there; a
    // new artifact is Kim's own, whose seat is hers: she starts it and
    // sends its brief to an agent.
    let lee = site.person(&owner, "Lee").await;
    let res = site
        .req(&lee, reqwest::Method::GET, "/api/stream")
        .send()
        .await
        .unwrap();
    let mut events = clax_testkit::EventReader::from_response(res);
    let (_, ready) = events.next().await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &lee,
            &format!("/api/stream/{sid}"),
            json!({"subscribe": [format!("artifact:{plan}")]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let kim = site.person(&owner, "Kim").await;
    let (s, v) = site.thread(&kim, &plan, "on Plan").await;
    assert_eq!((s, code(&v)), (403, "spectator"), "{v}");
    let (s, v) = site.post(&kim, new, brief("Kim's roadmap")).await;
    assert_eq!(s, 201, "{v}");
    let (kaid, ktid) = (
        v["artifact"]["id"].as_str().unwrap().to_string(),
        v["thread"]["id"].as_str().unwrap().to_string(),
    );
    let agent = site.ts.register_session("claude", "hosted-brief").await;
    let (s, v) = site
        .post(
            &kim,
            &format!("/api/artifacts/{kaid}/threads/{ktid}/send"),
            json!({"to": agent["name"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let watching = site
        .ts
        .store
        .list_watches(agent["id"].as_str().unwrap())
        .unwrap();
    assert!(
        watching.iter().any(|w| w.artifact_id == kaid),
        "{watching:?}"
    );
    // A guest of a page shared by link may comment there, never start one.
    site.send(
        &owner,
        reqwest::Method::PUT,
        &format!("/api/hosted/artifacts/{plan}/link"),
        json!({"access": "commenter"}),
    )
    .await;
    let mut guest = Browser::default();
    site.sign_in_put(&mut guest, json!({"display_name": "Gil"}))
        .await;
    let (s, v) = site.post(&guest, new, brief("Nope")).await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    let (s, v) = site.post(&Browser::default(), new, brief("Nope")).await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    // The owner's session, from another site's page.
    let cookie = format!("clax_session={}", owner.cookie.as_deref().unwrap());
    for origin in [None, Some("http://evil.example")] {
        let mut r = site
            .ts
            .client
            .post(site.url(new))
            .header(COOKIE, &cookie)
            .json(&brief("Nope"));
        if let Some(o) = origin {
            r = r.header(ORIGIN, o);
        }
        let (s, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{origin:?}");
    }
}

#[tokio::test]
async fn a_member_who_may_not_send_attaches_no_agent_and_agent_tokens_start_nothing() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let new = "/api/artifacts:new";
    let brief = json!({"title": "Nia's notes", "brief": "Notes from the launch review"});
    // Nia creates in her own space and comments there, but sends nothing.
    let (nia, _) = site
        .person_with(
            &owner,
            "Nia",
            json!([
                {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
                {"type": "clax-artifacts", "actions": ["read"], "locations": ["own"]},
                {"type": "clax-comments", "actions": ["read", "write"], "locations": ["own"]},
            ]),
        )
        .await;
    let (s, v) = site.post(&nia, new, brief.clone()).await;
    assert_eq!(s, 201, "{v}");
    let (aid, tid) = (
        v["artifact"]["id"].as_str().unwrap().to_string(),
        v["thread"]["id"].as_str().unwrap().to_string(),
    );
    let agent = site.ts.register_session("claude", "hosted-nia").await;
    let sid = agent["id"].as_str().unwrap().to_string();
    let (s, v) = site.get(&nia, &format!("/api/agents?artifact={aid}")).await;
    assert_eq!((s, v["agents"].clone()), (200, json!([])), "{v}");
    let (s, v) = site
        .post(
            &nia,
            &format!("/api/artifacts/{aid}/threads/{tid}/send"),
            json!({"to": agent["name"]}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    let watching = site.ts.store.list_watches(&sid).unwrap();
    assert!(
        watching.iter().all(|w| w.artifact_id != aid),
        "{watching:?}"
    );
    // The owner, who may send anywhere, may name it there.
    let (s, v) = site
        .get(&owner, &format!("/api/agents?artifact={aid}"))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["agents"].as_array().unwrap().len(), 1, "{v}");
    // An agent token of a member who creates starts nothing from a brief.
    let mia = site.member(&owner, "Mia").await;
    let (s, t) = site
        .post(
            &mia,
            "/api/hosted/agent-tokens",
            json!({"label": "mia-laptop"}),
        )
        .await;
    assert_eq!(s, 201, "{t}");
    let token = t["token"].as_str().unwrap();
    let res = site
        .ts
        .client
        .post(site.url(new))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .bearer_auth(token)
        .json(&brief)
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
}

#[tokio::test]
async fn a_persons_inbox_groups_hold_their_own_items_and_no_withheld_text() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sid = site.ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut pages = Vec::new();
    for title in ["Plan", "Notes"] {
        pages.push(
            site.ts.publish_as(&sid, title, "<h1>Page</h1>").await["artifact"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let (plan, notes) = (pages[0].clone(), pages[1].clone());
    let reads = json!({"type": "clax-artifacts", "actions": ["read"], "locations": [plan, notes]});
    let comments = |at: Value| json!({"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": at});
    let (mia, mia_id) = site
        .person_with(
            &owner,
            "Mia",
            json!([reads, comments(json!([plan, notes]))]),
        )
        .await;
    // Mia asks on each page and the agent answers with a secret.
    for at in [&plan, &notes] {
        let (s, t) = site.thread(&mia, at, "a question").await;
        assert_eq!(s, 201, "{t}");
        let tid = t["thread"]["id"].as_str().unwrap().to_string();
        let (s, v) = site
            .post(
                &owner,
                &format!("/api/artifacts/{at}/threads/{tid}/send"),
                json!({}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        site.ts
            .reply_as_agent(&sid, at, &tid, "secret answer")
            .await;
    }
    let keys = |v: &Value| -> Vec<(String, u64)> {
        v["groups"]
            .as_array()
            .unwrap_or_else(|| panic!("groups: {v}"))
            .iter()
            .map(|g| {
                (
                    g["artifact_id"].as_str().unwrap_or("none").to_string(),
                    g["total"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    // Her groups count her items alone, not the owner's on the same pages.
    let (s, v) = site.get(&mia, "/api/inbox?group=artifact").await;
    assert_eq!(s, 200, "{v}");
    let (_, flat) = site.get(&mia, "/api/inbox").await;
    let mine = flat["items"].as_array().unwrap();
    assert!(!keys(&v).is_empty(), "{v}");
    for (aid, total) in keys(&v) {
        let n = mine
            .iter()
            .filter(|i| i["artifact"]["id"] == aid.as_str())
            .count() as u64;
        assert_eq!(total, n, "{aid}: {v}");
    }
    // The owner's items on the same pages (their publications) are not in hers.
    let ids = |v: &Value| -> Vec<String> {
        v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["items"].as_array().unwrap().iter())
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (_, owners) = site.get(&owner, "/api/inbox?group=artifact").await;
    let theirs = ids(&owners);
    assert!(!theirs.is_empty(), "{owners}");
    assert!(ids(&v).iter().all(|i| !theirs.contains(i)), "{v} {owners}");
    let (s, v) = site.get(&mia, "/api/inbox?group=artifact&q=secret").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(keys(&v), vec![(notes.clone(), 1), (plan.clone(), 1)], "{v}");
    // The owner takes Mia's comment read at Plan away: a search neither
    // finds nor counts Plan's group by the text withheld from her.
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/people/{mia_id}/rights"),
            json!({"rights": [reads, comments(json!([notes]))]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site.get(&mia, "/api/inbox?group=artifact&q=secret").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(keys(&v), vec![(notes.clone(), 1)], "{v}");
    assert_eq!(v["total"], 1, "{v}");
}

#[tokio::test]
async fn an_artifacts_answer_with_a_signed_in_viewers_attention_is_private() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact().await;
    let mia = site.person(&owner, "Mia").await;
    for b in [&owner, &mia] {
        let res = site
            .req(b, reqwest::Method::GET, &format!("/api/artifacts/{aid}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["cache-control"], "private, no-cache");
        assert_eq!(res.headers()["vary"], "Cookie");
        let v: Value = res.json().await.unwrap();
        assert!(v.get("attention").is_some(), "{v}");
    }
}

/// The names of a list of tags.
fn tag_names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn members_apply_the_owners_tags_to_their_own_artifacts_alone() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let plan = site.titled("Plan").await;
    let tags = |aid: &str| format!("/api/artifacts/{aid}/tags");
    // The owner's browser makes tags, from the public origin alone.
    let (s, v) = site
        .post(
            &owner,
            "/api/tags",
            json!({"name": "draft", "colour": "orange"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let res = site
        .ts
        .client
        .post(site.url("/api/tags"))
        .header(HOST, format!("localhost:{}", site.ts.addr.port()))
        .header(ORIGIN, "http://evil.example")
        .header(
            COOKIE,
            format!("clax_session={}", owner.cookie.clone().unwrap()),
        )
        .json(&json!({"name": "csrf"}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    // A member makes none, sees every tag, and applies them to their own.
    let sam = site.member(&owner, "Sam").await;
    let sam_pid = site.pid(&sam).await;
    let (s, v) = site.post(&sam, "/api/tags", json!({"name": "mine"})).await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, v) = site.get(&sam, "/api/tags").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(tag_names(&v["tags"]), ["draft"]);
    assert_eq!(v["tags"][0]["count"], 0);
    let (s, v) = site
        .post(
            &sam,
            "/api/artifacts:new",
            json!({"title": "Sam's", "brief": "a page"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mine = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &tags(&mine),
            json!({"tags": ["DRAFT"]}),
        )
        .await;
    assert_eq!(
        (s, tag_names(&v["tags"])),
        (200, vec!["draft".to_string()]),
        "{v}"
    );
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &tags(&mine),
            json!({"tags": ["nope"]}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "unknown_tag"), "{v}");
    let (_, v) = site.get(&sam, "/api/artifacts?tag=draft").await;
    assert_eq!(listed(&v), vec![mine.clone()]);
    let (_, v) = site.get(&sam, "/api/tags").await;
    assert_eq!(v["tags"][0]["count"], 1, "{v}");
    // Another's artifact: not there until shared, and an editor's share
    // does not let them tag it.
    let (s, _) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &tags(&plan),
            json!({"tags": ["draft"]}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = site
        .put(
            &owner,
            &format!("/api/hosted/artifacts/{plan}/shares"),
            json!({"person": sam_pid, "access": "editor"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site
        .send(
            &sam,
            reqwest::Method::PUT,
            &tags(&plan),
            json!({"tags": ["draft"]}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    // The owner tags any artifact, and the member sees the tags there.
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &tags(&plan),
            json!({"tags": ["draft"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = site.get(&sam, &format!("/api/artifacts/{plan}")).await;
    assert_eq!(tag_names(&v["artifact"]["tags"]), ["draft"]);
    // Signed out: no list, no change.
    let anon = Browser::default();
    let (s, v) = site.get(&anon, "/api/tags").await;
    assert_eq!((s, code(&v)), (401, "sign_in_required"), "{v}");
    let (s, _) = site
        .send(
            &anon,
            reqwest::Method::PUT,
            &tags(&plan),
            json!({"tags": []}),
        )
        .await;
    assert!(s == 401 || s == 403, "{s}");
}

#[tokio::test]
async fn a_tag_filter_matches_no_item_whose_text_is_withheld() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sid = site.ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = site.ts.publish_as(&sid, "Plan", "<h1>Plan</h1>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reads = json!({"type": "clax-artifacts", "actions": ["read"], "locations": [plan]});
    let (mia, mia_id) = site
        .person_with(
            &owner,
            "Mia",
            json!([reads, {"type": "clax-comments", "actions": ["read", "write", "reply"], "locations": [plan]}]),
        )
        .await;
    let (s, t) = site.thread(&mia, &plan, "a question").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{plan}/threads/{tid}/send"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    site.ts.reply_as_agent(&sid, &plan, &tid, "an answer").await;
    let (s, v) = site
        .post(&owner, "/api/tags", json!({"name": "layoffs"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/artifacts/{plan}/tags"),
            json!({"tags": ["layoffs"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let about_plan = |v: &Value, key: &str| {
        v[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key}: {v}"))
            .iter()
            .filter(|i| i["artifact"]["id"] == json!(plan))
            .count()
    };
    // While she reads its comments, the filter finds her items about it.
    let (s, v) = site.get(&mia, "/api/inbox?tag=layoffs").await;
    assert_eq!(s, 200, "{v}");
    assert!(about_plan(&v, "items") > 0, "{v}");
    // Once its comments are withheld from her, no tag filter matches them:
    // it would tell her which tags the page carries now.
    let (s, v) = site
        .send(
            &owner,
            reqwest::Method::PUT,
            &format!("/api/people/{mia_id}/rights"),
            json!({"rights": [reads]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site.get(&mia, "/api/inbox").await;
    assert_eq!(s, 200, "{v}");
    assert!(about_plan(&v, "items") > 0, "the items stay: {v}");
    for path in [
        "/api/inbox?tag=layoffs",
        "/api/inbox?tag=layoffs&read=unread",
        "/api/inbox?tag=nope",
    ] {
        let (s, v) = site.get(&mia, path).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["items"], json!([]), "{path}: {v}");
        assert_eq!(v["total"], json!(0), "{path}: {v}");
    }
    let (s, v) = site
        .get(&mia, "/api/inbox?group=artifact&tag=layoffs")
        .await;
    assert_eq!((s, &v["groups"]), (200, &json!([])), "{v}");
    let (s, v) = site.get(&mia, "/api/inbox/summary?tag=layoffs").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(about_plan(&v, "latest"), 0, "{v}");
    assert_eq!(v["questions"], json!([]), "{v}");
    let (s, v) = site
        .post(
            &mia,
            "/api/inbox/read",
            json!({"all": true, "filter": {"tag": "layoffs"}}),
        )
        .await;
    assert_eq!((s, &v["marked"]), (200, &json!(0)), "{v}");
}
