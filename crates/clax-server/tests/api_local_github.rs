//! GitHub sign-in on a local daemon (`[github]`, `clax_server::github`),
//! against the testkit's fake GitHub: an admin adds `@octo-cat` to a
//! queue, octo-cat signs in through the fake and becomes that member, told
//! once in their inbox, as no one more than that person; a forged state, a
//! host `[github]` does not name, a login no one added, the owner's linked
//! account, and a daemon without `[github]` are refused.

use clax_testkit::TestServer;
use clax_testkit::github::FakeGitHub;
use reqwest::Method;
use reqwest::header::{COOKIE, HOST, LOCATION, SET_COOKIE};
use serde_json::{Value, json};

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A request from the owner's browser.
async fn owner(ts: &TestServer, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut r = ts
        .client
        .request(m, format!("{}{path}", ts.base))
        .header(COOKIE, ts.owner_cookie());
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// A browser of someone else's: no owner session, no redirects followed.
fn stranger() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// A request from the browser holding `cookie`.
async fn as_cookie(ts: &TestServer, cookie: &str, m: Method, path: &str) -> (u16, Value) {
    json_of(
        stranger()
            .request(m, format!("{}{path}", ts.base))
            .header(COOKIE, cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
}

fn cookie_of(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers().get_all(SET_COOKIE).iter().find_map(|v| {
        let v = v.to_str().ok()?;
        let (k, rest) = v.split_once('=')?;
        (k == name).then(|| rest.split(';').next().unwrap_or("").to_string())
    })
}

fn local_host(ts: &TestServer) -> String {
    format!("localhost:{}", ts.addr.port())
}

/// `/auth/github` on `host`: its answer.
async fn start_on(ts: &TestServer, host: &str) -> reqwest::Response {
    stranger()
        .get(format!("{}/auth/github", ts.base))
        .header(HOST, host)
        .send()
        .await
        .unwrap()
}

/// Signs a new browser in through `fake` as account `id`: its
/// `clax_viewer` cookie (`clax_viewer=…`), or the callback's status and
/// page.
async fn sign_in(ts: &TestServer, fake: &FakeGitHub, id: i64) -> Result<String, (u16, String)> {
    let host = local_host(ts);
    let res = start_on(ts, &host).await;
    assert_eq!(res.status(), 303);
    let flow = format!(
        "clax_github={}",
        cookie_of(&res, "clax_github").expect("a flow cookie")
    );
    let authorize = res.headers()[LOCATION].to_str().unwrap().to_string();
    assert!(authorize.starts_with(&fake.base), "{authorize}");
    fake.sign_in_as(id);
    let back = stranger().get(&authorize).send().await.unwrap();
    assert_eq!(back.status(), 302);
    let to = url::Url::parse(back.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        to.authority(),
        host,
        "GitHub returns to the configured origin"
    );
    let res = stranger()
        .get(format!(
            "{}{}?{}",
            ts.base,
            to.path(),
            to.query().unwrap_or("")
        ))
        .header(HOST, &host)
        .header(COOKIE, &flow)
        .send()
        .await
        .unwrap();
    if res.status() != 303 {
        assert_eq!(cookie_of(&res, "clax_viewer"), None);
        return Err((res.status().as_u16(), res.text().await.unwrap()));
    }
    assert_eq!(res.headers()[LOCATION], "/");
    Ok(format!(
        "clax_viewer={}",
        cookie_of(&res, "clax_viewer").expect("a viewer cookie")
    ))
}

async fn make_queue(ts: &TestServer) -> String {
    let (s, v) = owner(
        ts,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Bugs", "origins": ["https://app.example.com"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["queue"]["id"].as_str().unwrap().to_string()
}

async fn with_github() -> (TestServer, FakeGitHub) {
    let fake = FakeGitHub::start("the-client", "the-client-secret").await;
    let f = fake.clone();
    let ts = TestServer::spawn_with(move |s| f.turn_on(s)).await;
    (ts, fake)
}

#[tokio::test]
async fn an_added_login_signs_in_with_github_and_becomes_that_member_and_no_more() {
    let (ts, fake) = with_github().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    fake.account(7, "octo-cat", Some("Octo Cat"));
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "@Octo-Cat", "role": "triage"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(fake.lookups(), ["octo-cat"], "asked once, lowercased");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["github_logins"], true, "{v}");
    assert_eq!(v["pending"][0]["github_login"], "octo-cat", "as GitHub spells it: {v}");
    assert_eq!(v["pending"][0]["legacy"], false, "{v}");

    let cat = sign_in(&ts, &fake, 7).await.unwrap();
    let q = &fake.authorized()[0];
    assert_eq!(q["client_id"], "the-client");
    assert_eq!(
        q["redirect_uri"],
        format!("http://{}/auth/github/callback", local_host(&ts))
    );

    // A member at the role, the invite gone.
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["pending"], json!([]), "{v}");
    let added = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "Octo Cat")
        .cloned()
        .unwrap_or_else(|| panic!("octo-cat is a member: {v}"));
    assert_eq!(
        (&added["role"], &added["owner"], &added["maker"]),
        (&json!("triage"), &json!(false), &json!(false))
    );
    let pid = added["public_id"].as_str().unwrap().to_string();

    // Their browser lists the queue and holds one access item.
    let (s, v) = as_cookie(&ts, &cat, Method::GET, "/api/queues").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["id"], json!(qid), "{v}");
    assert_eq!(v["queues"][0]["role"], "triage", "{v}");
    let inbox = "/api/inbox?kind=access&read=all";
    let (_, v) = as_cookie(&ts, &cat, Method::GET, inbox).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["items"][0]["access"]["queue_id"], json!(qid));

    // No more than that person: not the owner, no rights of their own, no
    // owner routes.
    let viewer = ts.store.viewer_by_public_id(&pid).unwrap().unwrap();
    assert!(viewer.person);
    assert_ne!(
        ts.store.owner().unwrap().map(|o| o.public_id),
        Some(pid.clone())
    );
    let (s, v) = as_cookie(&ts, &cat, Method::GET, "/api/people").await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(
        ts.store
            .github_user_of(&pid)
            .unwrap()
            .map(|u| (u.id, u.login)),
        Some((7, "octo-cat".to_string()))
    );

    // A later sign-in is the same person, told nothing again.
    let again = sign_in(&ts, &fake, 7).await.unwrap();
    assert_ne!(again, cat, "a new cookie");
    let (_, v) = as_cookie(&ts, &again, Method::GET, inbox).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(
        v["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["public_id"] == json!(pid))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_sign_in_is_refused_for_a_forged_state_another_host_and_a_login_no_one_added() {
    let (ts, fake) = with_github().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    fake.account(7, "octo-cat", None);
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "octo-cat", "role": "admin"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");

    // A state that is not the flow cookie's, or no flow cookie at all.
    let host = local_host(&ts);
    let res = start_on(&ts, &host).await;
    let flow = format!("clax_github={}", cookie_of(&res, "clax_github").unwrap());
    for cookie in [Some(flow.as_str()), None] {
        let mut r = stranger()
            .get(format!(
                "{}/auth/github/callback?code=code-7&state=forged",
                ts.base
            ))
            .header(HOST, &host);
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        let res = r.send().await.unwrap();
        assert_eq!(res.status(), 400);
        assert_eq!(cookie_of(&res, "clax_viewer"), None);
    }

    // A host `[github] origins` does not name starts nothing.
    let res = start_on(&ts, &format!("127.0.0.1:{}", ts.addr.port())).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        cookie_of(&res, "clax_github").unwrap_or_default(),
        "",
        "no flow is started"
    );
    assert_eq!(res.headers().get(LOCATION), None);

    // octo-dog was added by no one: no person is made, octo-cat still waits.
    fake.account(8, "octo-dog", None);
    let (s, page) = sign_in(&ts, &fake, 8).await.unwrap_err();
    assert_eq!(s, 403, "{page}");
    assert!(page.contains("Nobody here has added"), "{page}");
    assert!(ts.store.github_links().unwrap().is_empty());
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["pending"][0]["github_login"], "octo-cat", "{v}");
    assert_eq!(v["members"].as_array().unwrap().len(), 1, "only the maker");
}

/// The invite to @alice is the account that held "alice" when it was
/// added: alice renames to alice2, another account registers "alice" and
/// signs in first, and gets nothing (no person is even made); the original
/// account signs in as alice2 and is the member.
#[tokio::test]
async fn an_invite_binds_to_the_account_that_held_the_login_not_its_next_holder() {
    let (ts, fake) = with_github().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    fake.account(1, "Alice", Some("Alice A"));
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "@alice", "role": "triage"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["invite"]["github_login"], "Alice", "as GitHub spells it: {v}");

    fake.account(1, "alice2", Some("Alice A"));
    fake.account(2, "alice", Some("Not Alice"));
    let (s, page) = sign_in(&ts, &fake, 2).await.unwrap_err();
    assert_eq!(s, 403, "{page}");
    assert!(page.contains("Nobody here has added"), "{page}");
    assert!(ts.store.github_links().unwrap().is_empty(), "no one made");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["pending"].as_array().unwrap().len(), 1, "still waiting: {v}");

    let alice = sign_in(&ts, &fake, 1).await.unwrap();
    let (s, v) = as_cookie(&ts, &alice, Method::GET, "/api/queues").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["id"], json!(qid), "{v}");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["pending"], json!([]), "{v}");
    assert!(
        v["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "Alice A"),
        "{v}"
    );
}

/// A login GitHub does not know (or an organization's), a login that is
/// not one, and GitHub not answering: each is refused and nothing waits;
/// only a well-formed login is asked of GitHub.
#[tokio::test]
async fn a_login_github_does_not_answer_for_adds_nothing() {
    let (ts, fake) = with_github().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    let add = |login: &str| json!({"github_login": login, "role": "triage"});
    fake.organization(40, "acme");
    for login in ["no-such-person", "acme"] {
        let (s, v) = owner(&ts, Method::PUT, &members, Some(add(login))).await;
        assert_eq!((s, code(&v)), (400, "unknown_github_login"), "{login}: {v}");
    }
    let (s, v) = owner(&ts, Method::PUT, &members, Some(add("../user"))).await;
    assert_eq!((s, code(&v)), (400, "invalid_login"), "{v}");
    assert_eq!(fake.lookups(), ["no-such-person", "acme"], "nothing else asked");
    fake.account(7, "octo-cat", None);
    fake.set_unavailable(true);
    let (s, v) = owner(&ts, Method::PUT, &members, Some(add("octo-cat"))).await;
    assert_eq!((s, code(&v)), (503, "github_unavailable"), "{v}");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(v["pending"], json!([]), "nothing kept: {v}");
    fake.set_unavailable(false);
    // A spent rate limit is GitHub not answering too, and is not asked
    // again without credentials.
    fake.set_rate_limited(true);
    let (s, v) = owner(&ts, Method::PUT, &members, Some(add("octo-cat"))).await;
    assert_eq!((s, code(&v)), (503, "github_unavailable"), "{v}");
    assert_eq!(fake.anonymous_lookups(), 0);
    fake.set_rate_limited(false);
    let (s, v) = owner(&ts, Method::PUT, &members, Some(add("octo-cat"))).await;
    assert_eq!(s, 201, "{v}");
}

/// GitHub refusing the OAuth App's credentials on `GET /users` does not
/// stop adding by login: the public endpoint is asked again without them.
#[tokio::test]
async fn a_lookup_github_refuses_the_apps_credentials_for_is_asked_again_without_them() {
    let (ts, fake) = with_github().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    fake.account(7, "Octo-Cat", None);
    fake.set_refuses_app(true);
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "octo-cat", "role": "triage"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["invite"]["github_login"], "Octo-Cat", "{v}");
    assert_eq!(fake.anonymous_lookups(), 1);
    // Unknown is still unknown when asked without credentials.
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "no-one-x", "role": "triage"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "unknown_github_login"), "{v}");
}

#[tokio::test]
async fn the_owners_github_account_never_signs_in_as_the_owner_on_a_local_clax() {
    let (ts, fake) = with_github().await;
    // A home once hosted: the owner's row is linked to a GitHub account.
    let user = clax_store::github::GitHubUser {
        id: 9,
        login: "the-owner".into(),
        name: None,
        avatar_url: None,
    };
    ts.store
        .github_sign_in(&clax_core::audit::AuditCtx::DAEMON, &user, true)
        .unwrap();
    let owner_pid = ts.store.owner().unwrap().unwrap().public_id;
    fake.account(9, "the-owner", None);
    let (s, page) = sign_in(&ts, &fake, 9).await.unwrap_err();
    assert_eq!(s, 403, "{page}");
    assert!(page.contains("owner"), "{page}");
    // Still linked to the owner, and no person made for it.
    assert_eq!(
        ts.store.github_user_of(&owner_pid).unwrap().map(|u| u.id),
        Some(9)
    );
    assert_eq!(ts.store.github_links().unwrap().len(), 1);
}

#[tokio::test]
async fn without_github_sign_in_no_login_is_added_and_the_routes_are_not_found() {
    let ts = TestServer::spawn().await;
    let qid = make_queue(&ts).await;
    let members = format!("/api/queues/{qid}/members");
    let (s, v) = owner(
        &ts,
        Method::PUT,
        &members,
        Some(json!({"github_login": "octo-cat", "role": "triage"})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "github_sign_in_off"), "{v}");
    let (_, v) = owner(&ts, Method::GET, &members, None).await;
    assert_eq!(
        (&v["pending"], &v["github_logins"]),
        (&json!([]), &json!(false)),
        "{v}"
    );
    let host = local_host(&ts);
    assert_eq!(start_on(&ts, &host).await.status(), 404);
    let res = stranger()
        .get(format!(
            "{}/auth/github/callback?code=code-1&state=s",
            ts.base
        ))
        .header(HOST, &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}
