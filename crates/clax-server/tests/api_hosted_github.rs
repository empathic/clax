//! GitHub sign-in on a hosted daemon (`clax_server::github`), against a
//! fake GitHub: the owner's login signs in as the owner, anyone else as a
//! person of their own, made at their first sign-in, whose rights reach
//! only what they made; a blocked account and a forged state are refused;
//! sign-ups are rate limited and members' artifacts capped.

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SECRET: &str = "the setup secret";

/// The fake GitHub's accounts by code (`code-<id>`), and the verifier each
/// token exchange sent.
#[derive(Clone, Default)]
struct Fake {
    users: Arc<Mutex<HashMap<i64, Value>>>,
    verifiers: Arc<Mutex<Vec<String>>>,
}

async fn access_token(
    State(f): State<Fake>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::Json<Value> {
    assert_eq!(
        form.get("client_id").map(String::as_str),
        Some("the-client")
    );
    assert_eq!(
        form.get("client_secret").map(String::as_str),
        Some("the-client-secret")
    );
    f.verifiers
        .lock()
        .unwrap()
        .push(form.get("code_verifier").cloned().unwrap_or_default());
    let code = form.get("code").cloned().unwrap_or_default();
    match code.strip_prefix("code-") {
        Some(id) => {
            axum::Json(json!({"access_token": format!("tok-{id}"), "token_type": "bearer"}))
        }
        None => axum::Json(json!({"error": "bad_verification_code"})),
    }
}

async fn user(State(f): State<Fake>, headers: HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(headers.contains_key("user-agent"));
    let id: Option<i64> = auth
        .strip_prefix("Bearer tok-")
        .and_then(|i| i.parse().ok());
    match id.and_then(|i| f.users.lock().unwrap().get(&i).cloned()) {
        Some(u) => axum::Json(u).into_response(),
        None => axum::http::StatusCode::UNAUTHORIZED.into_response(),
    }
}

struct Site {
    ts: TestServer,
    clock: Arc<ManualClock>,
    fake: Fake,
    http: reqwest::Client,
    host: String,
}

impl Site {
    async fn new(max_artifacts: u64) -> Site {
        let fake = Fake::default();
        let app = axum::Router::new()
            .route("/login/oauth/access_token", post(access_token))
            .route("/user", get(user))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gh = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let clock = Arc::new(ManualClock::at("2026-10-14T10:00:00Z"));
        let limits: Arc<dyn clax_core::working::Clock> = clock.clone();
        let ts = TestServer::spawn_hosted_with(clock.clone(), Some(SECRET), move |s| {
            let origin = format!("http://localhost:{}", s.port);
            let github = clax_http::github::GitHub::new(
                "the-client",
                "the-client-secret",
                Some("TheOwner"),
                Some(&gh),
                Some(&gh),
            );
            let h = clax_http::hosted::Hosted::new(&origin, Some(SECRET))
                .unwrap()
                .with_clock(limits)
                .with_github(github, max_artifacts);
            s.store.set_member_caps(Some(h.member_caps));
            s.hosted = Some(Arc::new(h));
        })
        .await;
        let host = format!("localhost:{}", ts.addr.port());
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Site {
            ts,
            clock,
            fake,
            http,
            host,
        }
    }

    fn account(&self, id: i64, login: &str, name: Option<&str>) {
        self.fake.users.lock().unwrap().insert(
            id,
            json!({"id": id, "login": login, "name": name, "avatar_url": format!("https://avatars.example/{id}")}),
        );
    }

    fn get(&self, path: &str, cookie: Option<&str>) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .get(format!("{}{path}", self.ts.base))
            .header(HOST, &self.host);
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        r
    }

    /// `/auth/github`'s redirect: the `state` it sends and the flow cookie.
    async fn start(&self) -> (String, String) {
        let res = self.get("/auth/github", None).send().await.unwrap();
        assert_eq!(res.status(), 303);
        let to = url::Url::parse(res.headers()[LOCATION].to_str().unwrap()).unwrap();
        assert!(to.path().ends_with("/login/oauth/authorize"));
        let q: HashMap<_, _> = to.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "the-client");
        assert_eq!(q["scope"], "read:user");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(
            q["redirect_uri"],
            format!("http://{}/auth/github/callback", self.host)
        );
        let cookie = cookie_of(&res, "clax_github").expect("a flow cookie");
        (q["state"].clone(), format!("clax_github={cookie}"))
    }

    /// Signs a new browser in as GitHub account `id`: its session cookie,
    /// or the refusal's status.
    async fn sign_in(&self, id: i64) -> Result<String, u16> {
        // Attempts come back with time: each sign-in is a minute apart.
        self.clock.advance(60);
        let (state, flow) = self.start().await;
        let res = self
            .get(
                &format!("/auth/github/callback?code=code-{id}&state={state}"),
                Some(&flow),
            )
            .send()
            .await
            .unwrap();
        if res.status() != 303 {
            return Err(res.status().as_u16());
        }
        assert_eq!(res.headers()[LOCATION], "/");
        Ok(format!(
            "clax_session={}",
            cookie_of(&res, "clax_session").expect("a session")
        ))
    }

    async fn me(&self, session: &str) -> Value {
        self.get("/api/hosted/me", Some(session))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn token(&self, session: &str) -> String {
        let v: Value = self
            .http
            .post(format!("{}/api/hosted/agent-tokens", self.ts.base))
            .header(HOST, &self.host)
            .header(ORIGIN, format!("http://{}", self.host))
            .header(COOKIE, session)
            .json(&json!({"label": "laptop"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["token"].as_str().unwrap().to_string()
    }

    /// Publishes a new artifact through the MCP adapter with agent token
    /// `token`: its ID, or the tool's error code.
    async fn publish(&self, token: &str, title: &str) -> Result<String, String> {
        let post = |body: Value, session: Option<&str>| {
            let mut r = self
                .http
                .post(format!("{}{}", self.ts.base, clax_testkit::mcp::ADAPTER))
                .header("accept", clax_testkit::mcp::ACCEPT)
                .header(HOST, &self.host)
                .bearer_auth(token)
                .json(&body);
            if let Some(s) = session {
                r = r.header("mcp-session-id", s);
            }
            r.send()
        };
        let res = post(clax_testkit::mcp::initialize(), None).await.unwrap();
        assert_eq!(res.status(), 200);
        let session = res.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "publish", "arguments": {"html": "<h1>x</h1>", "title": title}}});
        let text = post(body, Some(&session))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let line = text
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .unwrap_or(&text);
        let v: Value = serde_json::from_str(line).unwrap();
        let out: Value =
            serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap_or("{}"))
                .unwrap_or(json!({}));
        if v["result"]["isError"] == true {
            return Err(out["error"]["code"]
                .as_str()
                .unwrap_or("unknown")
                .to_string());
        }
        Ok(out["artifact_id"]
            .as_str()
            .or(out["artifact"]["id"].as_str())
            .unwrap_or_else(|| panic!("{out}"))
            .to_string())
    }
}

fn cookie_of(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.strip_prefix(&format!("{name}=")))
        .map(|c| c.split(';').next().unwrap().to_string())
        .filter(|c| !c.is_empty())
}

#[tokio::test]
async fn github_signs_in_the_owner_and_makes_members_of_their_own_space() {
    let site = Site::new(2).await;
    // The instance's owner row, made by setup under a placeholder name.
    let setup: Value = site
        .http
        .post(format!("{}/api/hosted/setup", site.ts.base))
        .header(HOST, &site.host)
        .header(ORIGIN, format!("http://{}", site.host))
        .json(&json!({"secret": SECRET, "name": "Placeholder"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let owner_pid = setup["person"]["public_id"].as_str().unwrap().to_string();
    let me = site.me("").await;
    assert_eq!(me["github"], json!(true));

    // The owner's login (in another case) is the owner, named from GitHub.
    site.account(1, "theowner", Some("Alex K"));
    let owner = site.sign_in(1).await.unwrap();
    let me = site.me(&owner).await;
    assert_eq!(me["person"]["owner"], json!(true), "{me}");
    assert_eq!(me["person"]["public_id"], json!(owner_pid));
    assert_eq!(me["person"]["display_name"], json!("Alex K"));
    let owners = site
        .ts
        .publish("Owner's plan", &[("index.html", "<h1>p</h1>")])
        .await;
    let owners = owners["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(site.fake.verifiers.lock().unwrap()[0].len(), 43);

    // Anyone else is a new member, who publishes and sees only their own.
    site.account(2, "grace", Some("Grace Hopper"));
    let grace = site.sign_in(2).await.unwrap();
    let me = site.me(&grace).await;
    assert_eq!(me["person"]["owner"], json!(false));
    assert_eq!(me["person"]["display_name"], json!("Grace Hopper"));
    let grace_pid = me["person"]["public_id"].as_str().unwrap().to_string();
    let token = site.token(&grace).await;
    let graces = site.publish(&token, "Grace's notes").await.unwrap();
    let (s, _) = site
        .get(&format!("/api/artifacts/{owners}"), Some(&grace))
        .send()
        .await
        .map(|r| (r.status().as_u16(), ()))
        .unwrap();
    assert_eq!(s, 404);
    let list: Value = site
        .get("/api/artifacts", Some(&grace))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["id"].as_str())
        .collect();
    assert_eq!(ids, vec![graces.as_str()]);

    // Another member sees neither's.
    site.account(3, "bob", None);
    let bob = site.sign_in(3).await.unwrap();
    assert_eq!(site.me(&bob).await["person"]["display_name"], json!("bob"));
    let s = site
        .get(&format!("/api/artifacts/{graces}"), Some(&bob))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(s, 404);

    // Returning signs in as the same person.
    let again = site.sign_in(2).await.unwrap();
    assert_eq!(
        site.me(&again).await["person"]["public_id"],
        json!(grace_pid)
    );

    // A member's artifacts are capped; the owner's are not.
    site.publish(&token, "Second").await.unwrap();
    assert_eq!(
        site.publish(&token, "Third").await,
        Err("quota_exceeded".to_string())
    );
    // A brief started in the browser counts against the same cap, and
    // leaves nothing behind when refused.
    let res = site
        .http
        .post(format!("{}/api/artifacts:new", site.ts.base))
        .header(HOST, &site.host)
        .header(ORIGIN, format!("http://{}", site.host))
        .header(COOKIE, &grace)
        .json(&json!({"title": "Third", "brief": "A chart"}))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("quota_exceeded")),
        "{v}"
    );
    let list: Value = site
        .get("/api/artifacts", Some(&grace))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 2, "{list}");

    // People shows each person's GitHub login; revoking blocks the account.
    let people: Value = site
        .ts
        .authed(site.ts.client.get(format!("{}/api/people", site.ts.base)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let logins: Vec<&str> = people["people"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["github"]["login"].as_str())
        .collect();
    assert_eq!(logins, vec!["grace", "bob"]);
    let s = site
        .http
        .delete(format!("{}/api/people/{grace_pid}", site.ts.base))
        .header(HOST, &site.host)
        .header(ORIGIN, format!("http://{}", site.host))
        .header(COOKIE, &owner)
        .send()
        .await
        .unwrap()
        .status();
    assert!(s.is_success(), "{s}");
    assert_eq!(site.sign_in(2).await, Err(403));
    assert_eq!(site.me(&again).await["signed_in"], json!(false));

    // Someone who takes the owner's login later is not the owner.
    site.account(9, "TheOwner", None);
    assert_eq!(site.sign_in(9).await, Err(403));
}

#[tokio::test]
async fn a_forged_or_missing_state_signs_no_one_in_and_sign_ups_are_rate_limited() {
    let site = Site::new(200).await;
    site.account(5, "eve", None);
    let (state, flow) = site.start().await;
    for (q, cookie) in [
        (
            format!("code=code-5&state=not-{state}"),
            Some(flow.as_str()),
        ),
        (format!("code=code-5&state={state}"), None),
        ("code=code-5".to_string(), Some(flow.as_str())),
    ] {
        let res = site
            .get(&format!("/auth/github/callback?{q}"), cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{q}");
        assert!(cookie_of(&res, "clax_session").is_none());
        assert!(res.text().await.unwrap().contains("Sign in again"));
    }
    // A flow cookie past its ten minutes is refused too.
    let (state, flow) = site.start().await;
    site.clock.advance(601);
    let res = site
        .get(
            &format!("/auth/github/callback?code=code-5&state={state}"),
            Some(&flow),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    // Five new accounts from one address at once; the sixth waits.
    for id in 10..15 {
        site.account(id, &format!("user{id}"), None);
        // Within the sign-up limit's ten minutes per attempt.
        site.sign_in(id).await.unwrap();
    }
    site.account(15, "user15", None);
    assert_eq!(site.sign_in(15).await, Err(429));
    // A known account still signs in.
    site.sign_in(10).await.unwrap();
}
