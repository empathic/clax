//! Hosted mode's sign-in (`clax_http::hosted`): the first person's
//! setup, invites redeemed in the browser, one-use sign-in links, sign out,
//! the session cookie's flags, CSRF on every change, agent tokens, and
//! guests on pages shared by link; and the local daemon serving none of it.

use crate::common::TestServer;
use clax_core::working::ManualClock;
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

    /// Sam, invited by `owner` and joined in a new browser.
    async fn person(&self, owner: &Browser, name: &str) -> Browser {
        let (s, v) = self
            .post(owner, "/api/people/invites", json!({"name": name}))
            .await;
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
    let viewer = site
        .ts
        .store
        .upsert_viewer(&clax_core::new_ulid(), Some("Lan"))
        .unwrap();
    for c in [owner_cookie, format!("clax_viewer={}", viewer.id)] {
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
    // A member reads the gallery.
    site.artifact().await;
    let (s, v) = site.get(&sam, "/api/artifacts").await;
    assert_eq!((s, v["artifacts"].as_array().map(Vec::len)), (200, Some(1)));
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
    let mcp = format!("{}/mcp", site.origin);
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
            assert_eq!((s, code(&v)), (401, "unauthorized"), "{p} [{auth}]: {v}");
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
        json!({}),
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
