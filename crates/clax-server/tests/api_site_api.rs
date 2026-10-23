//! The page runtime's backend on an artifact's site host
//! (`clax_server::routes::site_api`): `/_clax/api/…`, scoped by the host to
//! its artifact, as the site's caller.

use clax_core::ArtifactId;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::Method;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

/// A site API request on `aid`'s host from `cookie`, with `Origin` the
/// site's own unless `origin` names another (`Some("")`: none).
struct Call<'a> {
    ts: &'a TestServer,
    aid: &'a str,
    cookie: Option<&'a str>,
    origin: Option<&'a str>,
}

impl Call<'_> {
    fn site_origin(&self) -> String {
        format!("http://{}", self.ts.site_host(self.aid))
    }

    fn req(&self, m: Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .ts
            .client
            .request(m, format!("{}/_clax/api/{path}", self.ts.base))
            .header(HOST, self.ts.site_host(self.aid));
        match self.origin {
            Some("") => {}
            Some(o) => r = r.header(ORIGIN, o),
            None => r = r.header(ORIGIN, self.site_origin()),
        }
        if let Some(c) = self.cookie {
            r = r.header(COOKIE, c);
        }
        r
    }

    async fn send(&self, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.req(m, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let res = r.send().await.unwrap();
        let s = res.status().as_u16();
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "{path}"
        );
        (s, res.json().await.unwrap_or(Value::Null))
    }
}

fn on<'a>(ts: &'a TestServer, aid: &'a str, cookie: Option<&'a str>) -> Call<'a> {
    Call {
        ts,
        aid,
        cookie,
        origin: None,
    }
}

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Board", "capabilities": caps, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn open_to_world(ts: &TestServer, aid: &str) {
    ts.store
        .set_site_open_to_world(&ArtifactId::parse(aid).unwrap(), true)
        .unwrap();
}

/// A new visitor's cookie on `aid`'s world-open site: what its first
/// response sets.
async fn visitor(ts: &TestServer, aid: &str) -> String {
    let res = on(ts, aid, None)
        .req(Method::GET, "whoami")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let set = res.headers()[SET_COOKIE].to_str().unwrap().to_string();
    for attr in [
        "clax_visitor=",
        "HttpOnly",
        "Path=/",
        "SameSite=Lax",
        "Max-Age=31536000",
    ] {
        assert!(set.contains(attr), "{attr}: {set}");
    }
    assert!(!set.contains("Domain"), "{set}");
    set.split(';').next().unwrap().to_string()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn the_api_reaches_only_the_hosts_artifact() {
    let ts = TestServer::spawn().await;
    let a = artifact(&ts, json!({"db": {}})).await;
    let b = artifact(&ts, json!({"db": {}})).await;
    let cookie = ts.site_cookie(&a).await;
    let api = on(&ts, &a, Some(&cookie));
    let (s, v) = api
        .send(
            Method::PUT,
            "docs/tasks/t1",
            Some(json!({"data": {"n": 1}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A body or query naming B changes nothing of B.
    let (s, _) = api
        .send(
            Method::PUT,
            "docs/tasks/t2",
            Some(json!({"data": {"n": 2}, "artifact_id": b})),
        )
        .await;
    assert_eq!(s, 400, "an unknown body field is refused");
    let (s, v) = api
        .send(
            Method::POST,
            "docs:batch",
            Some(
                json!({"writes": [{"op": "set", "path": format!("../{b}/tasks/t3"), "data": {}}]}),
            ),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = api
        .send(
            Method::PUT,
            &format!("docs/tasks/t4?aid={b}&artifact={b}"),
            Some(json!({"data": {"n": 4}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = api
        .send(
            Method::GET,
            &format!("docs?collection=tasks&artifact={b}"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["docs"].as_array().unwrap().len(), 2, "{v}");
    for p in ["tasks/t1", "tasks/t4"] {
        let res = ts.get_authed(&format!("/api/artifacts/{b}/docs/{p}")).await;
        assert_eq!(res.status(), 404, "{p} is A's alone");
        let res = ts.get_authed(&format!("/api/artifacts/{a}/docs/{p}")).await;
        assert_eq!(res.status(), 200, "{p}");
    }
    let (s, v) = api.send(Method::GET, "artifact", None).await;
    assert_eq!((s, v["artifact"]["id"].as_str()), (200, Some(a.as_str())));
    // No path reaches another artifact, nor anything but the runtime's.
    for p in [
        format!("artifact/{b}"),
        format!("../artifacts/{b}"),
        "threads".to_string(),
        format!("docs/../../{b}/docs/tasks/t1"),
    ] {
        let (s, _) = api.send(Method::GET, &p, None).await;
        assert!(s == 404 || s == 400, "{p}: {s}");
    }
    // A's session is nothing on B's host.
    let (s, v) = on(&ts, &b, Some(&cookie))
        .send(Method::GET, "docs/tasks/t1", None)
        .await;
    assert_eq!((s, code(&v)), (401, "site_session_required"));
}

#[tokio::test]
async fn a_visitor_reads_and_writes_only_what_public_rules_allow() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "data/guestbook", "read": "public", "write": "public"}]}}),
    )
    .await;
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/docs/tasks/t1", ts.base)),
        )
        .json(&json!({"data": {"secret": true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // Private: no visitor at all.
    let (s, v) = on(&ts, &aid, None)
        .send(Method::GET, "docs/tasks/t1", None)
        .await;
    assert_eq!((s, code(&v)), (401, "site_session_required"));
    open_to_world(&ts, &aid);
    let me = visitor(&ts, &aid).await;
    let api = on(&ts, &aid, Some(&me));
    let (s, v) = api
        .send(
            Method::PUT,
            "docs/data/guestbook/entries/g1",
            Some(json!({"data": {"hi": 1}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = api
        .send(Method::GET, "docs/data/guestbook/entries/g1", None)
        .await;
    assert_eq!((s, &v["doc"]["data"]), (200, &json!({"hi": 1})));
    let (s, _) = api
        .send(Method::PUT, "docs/tasks/t2", Some(json!({"data": {}})))
        .await;
    assert_eq!(s, 404, "the default rules grant a visitor nothing");
    let (s, _) = api.send(Method::GET, "docs/tasks/t1", None).await;
    assert_eq!(s, 404);
    // The artifact reads as itself, with what its page declares (from which
    // the bridge decides what `use()` resolves), and nothing of who works
    // on it.
    let (s, v) = api.send(Method::GET, "artifact", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["artifact"]["id"], json!(aid));
    assert_eq!(
        v["artifact"]["capabilities"]["db"]["rules"][0]["path"],
        json!("data/guestbook"),
        "{v}"
    );
    let mut keys: Vec<&String> = v["artifact"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["capabilities", "current_version", "id", "title"],
        "{v}"
    );
    // A visitor uploads nothing.
    let (s, _) = api.send(Method::POST, "assets", Some(json!({}))).await;
    assert!(s == 401 || s == 403 || s == 400, "{s}");
}

#[tokio::test]
async fn a_visitor_has_a_self_subtree() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "data/users/{self}", "read": "public", "write": "public"}]}}),
    )
    .await;
    open_to_world(&ts, &aid);
    let (one, two) = (visitor(&ts, &aid).await, visitor(&ts, &aid).await);
    assert_ne!(one, two);
    let id = |c: &str| {
        let api = on(&ts, &aid, None);
        let c = c.to_string();
        async move {
            let api = Call {
                cookie: Some(&c),
                ..api
            };
            let (s, v) = api.send(Method::GET, "whoami", None).await;
            assert_eq!((s, v["kind"].as_str()), (200, Some("visitor")), "{v}");
            v["id"].as_str().unwrap().to_string()
        }
    };
    let (id1, id2) = (id(&one).await, id(&two).await);
    assert_ne!(id1, id2);
    assert_eq!(id1, id(&one).await, "stable per browser");
    assert!(!one.contains(&id1), "the cookie holds no visitor ID");
    let mine = format!("docs/data/users/{id1}/note");
    let (s, v) = on(&ts, &aid, Some(&one))
        .send(Method::PUT, &mine, Some(json!({"data": {"n": 1}})))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = on(&ts, &aid, Some(&one))
        .send(Method::GET, &mine, None)
        .await;
    assert_eq!(s, 200);
    let (s, _) = on(&ts, &aid, Some(&two))
        .send(Method::GET, &mine, None)
        .await;
    assert_eq!(s, 404, "another visitor's subtree");
    let (s, _) = on(&ts, &aid, Some(&two))
        .send(Method::PUT, &mine, Some(json!({"data": {}})))
        .await;
    assert_eq!(s, 404);
    let theirs = format!("docs/data/users/{id2}/note");
    let (s, _) = on(&ts, &aid, Some(&two))
        .send(Method::PUT, &theirs, Some(json!({"data": {"n": 2}})))
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn a_patch_with_an_invalid_field_opens_nothing() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let patch = |body: Value| {
        ts.authed(
            ts.client
                .patch(format!("{}/api/artifacts/{aid}", ts.base))
                .json(&body),
        )
        .send()
    };
    for bad in [
        json!({"site_access": "world", "capabilities": {"db": {"rules": "no"}}}),
        json!({"site_access": "world", "title": ""}),
        json!({"site_access": "world", "nope": 1}),
    ] {
        let res = patch(bad.clone()).await.unwrap();
        assert_eq!(res.status(), 400, "{bad}");
        assert!(
            !ts.store
                .site_open_to_world(&ArtifactId::parse(&aid).unwrap())
                .unwrap(),
            "{bad}"
        );
    }
    let res = patch(json!({"site_access": "world", "title": "Board 2"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (&v["artifact"]["site_access"], &v["artifact"]["title"]),
        (&json!("world"), &json!("Board 2"))
    );
}

#[tokio::test]
async fn a_write_from_another_origin_is_refused() {
    let ts = TestServer::spawn().await;
    let a = artifact(&ts, json!({"db": {}})).await;
    let b = artifact(&ts, json!({"db": {}})).await;
    let cookie = ts.site_cookie(&a).await;
    let b_origin = format!("http://{}", ts.site_host(&b));
    let local = ts.base.clone();
    for origin in [
        "https://evil.example",
        b_origin.as_str(),
        local.as_str(),
        "",
        "null",
    ] {
        let api = Call {
            origin: Some(origin),
            ..on(&ts, &a, Some(&cookie))
        };
        let (s, v) = api
            .send(Method::PUT, "docs/tasks/t1", Some(json!({"data": {}})))
            .await;
        assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{origin:?}");
        if !origin.is_empty() {
            let (s, _) = api.send(Method::GET, "docs/tasks/t1", None).await;
            assert_eq!(s, 403, "a read from {origin}");
        }
    }
    // A read with no Origin from the page's own origin is a read.
    let api = Call {
        origin: Some(""),
        ..on(&ts, &a, Some(&cookie))
    };
    let (s, _) = api.send(Method::GET, "docs?collection=tasks", None).await;
    assert_eq!(s, 200);
    let res = api
        .req(Method::GET, "docs?collection=tasks")
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // A credential of its own is refused: the site's cookie is the caller.
    let res = on(&ts, &a, Some(&cookie))
        .req(Method::GET, "docs?collection=tasks")
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

const SECRET: &str = "the setup secret";

/// A hosted daemon's owner browser's and an invited person's site
/// cookies on `aid`, from site codes their sessions mint.
async fn hosted_cookie(ts: &TestServer, session: &str, aid: &str) -> Option<String> {
    let origin = format!("http://localhost:{}", ts.addr.port());
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/site-code", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(ORIGIN, origin)
        .header(COOKIE, format!("clax_session={session}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    if res.status() != 200 {
        return None;
    }
    let v: Value = res.json().await.unwrap();
    Some(ts.redeem_site_code(v["src"].as_str().unwrap()).await)
}

async fn signed_in(ts: &TestServer, path: &str, body: Value, cookie: Option<&str>) -> String {
    let origin = format!("http://localhost:{}", ts.addr.port());
    let mut r = ts
        .client
        .post(format!("{}{path}", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(ORIGIN, origin)
        .json(&body);
    if let Some(c) = cookie {
        r = r.header(COOKIE, format!("clax_session={c}"));
    }
    let res = r.send().await.unwrap();
    assert_eq!(res.status().as_u16() / 100, 2, "{path}");
    match res.headers().get(SET_COOKIE) {
        Some(c) => c
            .to_str()
            .unwrap()
            .strip_prefix("clax_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string(),
        None => res.json::<Value>().await.unwrap().to_string(),
    }
}

#[tokio::test]
async fn whoami_names_the_caller() {
    let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some(SECRET)).await;
    let owner = signed_in(
        &ts,
        "/api/hosted/setup",
        json!({"secret": SECRET, "name": "Ada"}),
        None,
    )
    .await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let c = hosted_cookie(&ts, &owner, &aid).await.unwrap();
    let (s, v) = on(&ts, &aid, Some(&c))
        .send(Method::GET, "whoami", None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["kind"], "owner", "{v}");
    assert_eq!(v["name"], "Ada");
    assert_eq!(v["can_edit"], true);
    assert_eq!(v["level"], "admin");
    // A person the artifact is shared with, as a viewer.
    let invite: Value = serde_json::from_str(
        &signed_in(
            &ts,
            "/api/people/invites",
            json!({"name": "Sam"}),
            Some(&owner),
        )
        .await,
    )
    .unwrap();
    let code = invite["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let sam = signed_in(
        &ts,
        &format!("/api/hosted/invites/{code}/accept"),
        json!({}),
        None,
    )
    .await;
    let me = ts
        .client
        .get(format!("{}/api/hosted/me", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={sam}"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let pid = me["person"]["public_id"].as_str().unwrap().to_string();
    let shared = ts
        .client
        .put(format!("{}/api/hosted/artifacts/{aid}/shares", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(ORIGIN, format!("http://localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={owner}"))
        .json(&json!({"person": pid, "access": "viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(shared.status(), 200);
    let c = hosted_cookie(&ts, &sam, &aid).await.unwrap();
    let (s, v) = on(&ts, &aid, Some(&c))
        .send(Method::GET, "whoami", None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        (
            v["kind"].as_str(),
            v["id"].as_str(),
            v["can_edit"].as_bool()
        ),
        (Some("person"), Some(pid.as_str()), Some(false)),
        "{v}"
    );
    // Anyone, once the site is open to the world.
    open_to_world(&ts, &aid);
    let (s, v) = on(&ts, &aid, None).send(Method::GET, "whoami", None).await;
    assert_eq!(s, 200);
    assert_eq!(
        (
            v["kind"].as_str(),
            v["level"].as_str(),
            v["can_edit"].as_bool()
        ),
        (Some("visitor"), Some("public"), Some(false)),
        "{v}"
    );
    assert!(v.get("name").is_none());
}

#[tokio::test]
async fn visitor_writes_are_rate_limited() {
    let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock.clone(), Some(SECRET)).await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "data/guestbook", "read": "public", "write": "public"}]}}),
    )
    .await;
    open_to_world(&ts, &aid);
    let me = visitor(&ts, &aid).await;
    let api = on(&ts, &aid, Some(&me));
    let write = |n: u32| {
        let api = &api;
        async move {
            api.send(
                Method::PUT,
                &format!("docs/data/guestbook/entries/g{n}"),
                Some(json!({"data": {"n": n}})),
            )
            .await
        }
    };
    for n in 0..60 {
        let (s, v) = write(n).await;
        assert_eq!(s, 200, "{n}: {v}");
    }
    let (s, v) = write(60).await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
    assert_eq!(v["error"]["retry_after_s"], 1);
    // Reads are not counted.
    let (s, _) = api
        .send(Method::GET, "docs/data/guestbook/entries/g1", None)
        .await;
    assert_eq!(s, 200);
    clock.advance(1);
    let (s, _) = write(61).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn the_room_opens_on_the_site_host_from_its_own_origin() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"room": {}})).await;
    open_to_world(&ts, &aid);
    let connect = |origin: String| {
        let mut req = format!(
            "{}/_clax/api/room?peer=0123456789abcdef",
            ts.base.replacen("http://", "ws://", 1)
        )
        .into_client_request()
        .unwrap();
        let h = req.headers_mut();
        h.insert("host", ts.site_host(&aid).parse().unwrap());
        h.insert("origin", origin.parse().unwrap());
        tokio_tungstenite::connect_async(req)
    };
    let (mut ws, _) = connect(format!("http://{}", ts.site_host(&aid)))
        .await
        .expect("upgrade");
    let hello = ws.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(hello.to_text().unwrap()).unwrap();
    assert_eq!(v, json!({"t": "welcome", "peer": "0123456789abcdef"}));
    match connect("https://evil.example".into()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => {
            assert_eq!(res.status().as_u16(), 403);
        }
        other => panic!("another origin's socket opened: {:?}", other.is_ok()),
    }
}

#[tokio::test]
async fn a_page_forges_no_tool_call_or_git_context_into_the_audit() {
    use clax_core::audit::{CallHeader, encode_call_header};
    use clax_core::gitctx::{GitContext, GitField, encode_header};
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    let cookie = ts.site_cookie(&aid).await;
    let call = CallHeader {
        call_id: "01JBC0000000000000000000AA".into(),
        tool: "publish".into(),
        harness_tool: None,
        args_sha256: format!("sha256:{}", "c6".repeat(32)),
        started_at: "2026-10-06T14:03:11.402Z".into(),
        harness_call_id: None,
    };
    let git = GitContext {
        repo_root: "/forged/repo".into(),
        remote: None,
        remote_url: None,
        branch: Some("forged-branch".into()),
        head: None,
        dirty: false,
        diff_sha256: None,
        diff_bytes: None,
        diff_truncated: false,
        diff_unavailable: false,
        untracked: 0,
        captured_at: "2026-10-06T14:03:11.512Z".into(),
    };
    let res = on(&ts, &aid, Some(&cookie))
        .req(Method::PUT, "docs/tasks/t1")
        .header("x-clax-call", encode_call_header(&call).unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git)).unwrap())
        .header("x-clax-channel", "agent")
        .json(&json!({"data": {"n": 1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let forged: i64 = c
        .query_row(
            "SELECT count(*) FROM audit_events
             WHERE call_id IS NOT NULL OR body LIKE '%forged%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(forged, 0);
    let wrote: i64 = c
        .query_row(
            "SELECT count(*) FROM audit_events WHERE artifact_id = ?1",
            [&aid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(wrote > 0, "the write is audited");
}

#[tokio::test]
async fn a_link_viewers_writes_are_counted_as_a_visitors() {
    let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some(SECRET)).await;
    let owner = signed_in(
        &ts,
        "/api/hosted/setup",
        json!({"secret": SECRET, "name": "Ada"}),
        None,
    )
    .await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "data/guestbook", "read": "public", "write": "public"}]}}),
    )
    .await;
    let public = format!("localhost:{}", ts.addr.port());
    let res = ts
        .client
        .put(format!("{}/api/hosted/artifacts/{aid}/link", ts.base))
        .header(HOST, &public)
        .header(ORIGIN, format!("http://{public}"))
        .header(COOKIE, format!("clax_session={owner}"))
        .json(&json!({"access": "viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A link viewer: a site code with no session.
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/site-code", ts.base))
        .header(HOST, &public)
        .header(ORIGIN, format!("http://{public}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let cookie = ts.redeem_site_code(v["src"].as_str().unwrap()).await;
    let api = on(&ts, &aid, Some(&cookie));
    let (s, v) = api.send(Method::GET, "whoami", None).await;
    assert_eq!((s, v["kind"].as_str()), (200, Some("link")), "{v}");
    for n in 0..60 {
        let (s, v) = api
            .send(
                Method::PUT,
                &format!("docs/data/guestbook/entries/g{n}"),
                Some(json!({"data": {}})),
            )
            .await;
        assert_eq!(s, 200, "{n}: {v}");
    }
    let (s, v) = api
        .send(
            Method::PUT,
            "docs/data/guestbook/entries/x",
            Some(json!({"data": {}})),
        )
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
}

/// An `image/png` upload through `api`: the status and the answer.
async fn upload(api: &Call<'_>) -> (u16, Value) {
    let part = reqwest::multipart::Part::bytes(clax_testkit::FAKE_PNG.to_vec())
        .file_name("logo.png")
        .mime_str("image/png")
        .unwrap();
    let res = api
        .req(Method::POST, "assets")
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    let s = res.status().as_u16();
    (s, res.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_owner_deletes_an_asset_of_the_hosts_artifact_alone() {
    let ts = TestServer::spawn().await;
    let a = artifact(&ts, json!({"assets": {}})).await;
    let b = artifact(&ts, json!({"assets": {}})).await;
    let (ca, cb) = (ts.site_cookie(&a).await, ts.site_cookie(&b).await);
    let (on_a, on_b) = (on(&ts, &a, Some(&ca)), on(&ts, &b, Some(&cb)));
    let (s, v) = upload(&on_a).await;
    assert_eq!(s, 201, "{v}");
    let mine = v["asset"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["url"], json!(format!("/_clax/blob/{mine}")));
    let (s, v) = upload(&on_b).await;
    assert_eq!(s, 201, "{v}");
    let theirs = v["asset"]["id"].as_str().unwrap().to_string();
    // B's asset is nothing on A's host.
    let (s, _) = on_a
        .send(Method::DELETE, &format!("assets/{theirs}"), None)
        .await;
    assert_eq!(s, 404);
    let (s, v) = on_b.send(Method::GET, "assets", None).await;
    assert_eq!((s, v["assets"].as_array().map(Vec::len)), (200, Some(1)));
    // Nor is any other path under it.
    for p in [
        format!("assets/{mine}/x"),
        format!("assets/{mine}%2F"),
        "assets/".to_string(),
    ] {
        let (s, _) = on_a.send(Method::DELETE, &p, None).await;
        assert!(s == 404 || s == 400, "{p}: {s}");
    }
    // Without the site's Origin a delete is refused.
    let (s, v) = Call {
        origin: Some(""),
        ..on(&ts, &a, Some(&ca))
    }
    .send(Method::DELETE, &format!("assets/{mine}"), None)
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"));
    let res = on_a
        .req(Method::DELETE, &format!("assets/{mine}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let (s, v) = on_a.send(Method::GET, "assets", None).await;
    assert_eq!((s, v["assets"].as_array().map(Vec::len)), (200, Some(0)));
}

/// An upload of `bytes` as `content_type` through `api`: the status and the answer.
async fn upload_bytes(api: &Call<'_>, content_type: &str, bytes: Vec<u8>) -> (u16, Value) {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("upload")
        .mime_str(content_type)
        .unwrap();
    let res = api
        .req(Method::POST, "assets")
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    let s = res.status().as_u16();
    (s, res.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_page_upload_meets_the_contracts_rules_on_the_server() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"assets": {}})).await;
    let cookie = ts.site_cookie(&aid).await;
    let api = on(&ts, &aid, Some(&cookie));
    // What the page's own checks refuse, the site refuses too: a page that
    // skips them stores nothing.
    let big_svg = [b"<svg/>".to_vec(), vec![b' '; 2 * 1024 * 1024]].concat();
    for (ct, body, want) in [
        (
            "image/png",
            b"  <html><script>alert(1)</script>".to_vec(),
            "unsupported_type",
        ),
        ("text/html", b"<p>hi</p>".to_vec(), "unsupported_type"),
        ("image/svg+xml", b"not an svg".to_vec(), "unsupported_type"),
        ("image/svg+xml", big_svg, "asset_too_large"),
        (
            "text/javascript",
            b"<script>x</script>".to_vec(),
            "unsupported_type",
        ),
        ("text/csv", vec![0xff, 0xfe, 0x41, 0x00], "invalid_request"),
    ] {
        let (s, v) = upload_bytes(&api, ct, body).await;
        assert_eq!((s, code(&v)), (400, want), "{ct}: {v}");
    }
    let (s, v) = api.send(Method::GET, "assets", None).await;
    assert_eq!((s, v["assets"].as_array().map(Vec::len)), (200, Some(0)));
    let svg = b"<?xml version=\"1.0\"?><svg/>".to_vec();
    let (s, v) = upload_bytes(&api, "image/svg+xml", svg).await;
    assert_eq!(s, 201, "{v}");
}

/// Sam, a person the hosted owner (session `owner`) shares `aid` with as
/// `access`: Sam's browser session.
async fn shared_with_sam(ts: &TestServer, owner: &str, aid: &str, access: &str) -> String {
    let invite: Value = serde_json::from_str(
        &signed_in(
            ts,
            "/api/people/invites",
            json!({"name": "Sam"}),
            Some(owner),
        )
        .await,
    )
    .unwrap();
    let code = invite["invite"]["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let sam = signed_in(
        ts,
        &format!("/api/hosted/invites/{code}/accept"),
        json!({}),
        None,
    )
    .await;
    let me = ts
        .client
        .get(format!("{}/api/hosted/me", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={sam}"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let pid = me["person"]["public_id"].as_str().unwrap().to_string();
    let shared = ts
        .client
        .put(format!("{}/api/hosted/artifacts/{aid}/shares", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(ORIGIN, format!("http://localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={owner}"))
        .json(&json!({"person": pid, "access": access}))
        .send()
        .await
        .unwrap();
    assert_eq!(shared.status(), 200);
    sam
}

#[tokio::test]
async fn only_the_owner_deletes_an_asset_through_the_site() {
    let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some(SECRET)).await;
    let owner = signed_in(
        &ts,
        "/api/hosted/setup",
        json!({"secret": SECRET, "name": "Ada"}),
        None,
    )
    .await;
    let aid = artifact(&ts, json!({"assets": {}})).await;
    let mine = hosted_cookie(&ts, &owner, &aid).await.unwrap();
    let (s, v) = upload(&on(&ts, &aid, Some(&mine))).await;
    assert_eq!(s, 201, "{v}");
    let path = format!("assets/{}", v["asset"]["id"].as_str().unwrap());
    // An editor of the artifact, signed in on its site.
    let sam = shared_with_sam(&ts, &owner, &aid, "editor").await;
    let theirs = hosted_cookie(&ts, &sam, &aid).await.unwrap();
    let (s, _) = on(&ts, &aid, Some(&theirs))
        .send(Method::DELETE, &path, None)
        .await;
    assert!(
        s == 401 || s == 403,
        "an editor deleted the owner's asset: {s}"
    );
    // Nor a visitor, once the site is open to the world.
    open_to_world(&ts, &aid);
    let anyone = visitor(&ts, &aid).await;
    let (s, _) = on(&ts, &aid, Some(&anyone))
        .send(Method::DELETE, &path, None)
        .await;
    assert!(s == 401 || s == 403, "a visitor deleted an asset: {s}");
    let (s, v) = on(&ts, &aid, Some(&mine))
        .send(Method::GET, "assets", None)
        .await;
    assert_eq!((s, v["assets"].as_array().map(Vec::len)), (200, Some(1)));
}

/// A GET of `path` through `api` with `If-None-Match: tag`: the status and the `ETag`.
async fn get_tagged(api: &Call<'_>, path: &str, tag: Option<&str>) -> (u16, Option<String>) {
    let mut r = api.req(Method::GET, path);
    if let Some(t) = tag {
        r = r.header("if-none-match", t);
    }
    let res = r.send().await.unwrap();
    let etag = res
        .headers()
        .get("etag")
        .map(|v| v.to_str().unwrap().to_string());
    (res.status().as_u16(), etag)
}

#[tokio::test]
async fn a_docs_read_answers_304_until_its_artifacts_documents_change() {
    let ts = TestServer::spawn().await;
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "data/guestbook", "read": "public", "write": "public"}]}}),
    )
    .await;
    let cookie = ts.site_cookie(&aid).await;
    let api = on(&ts, &aid, Some(&cookie));
    let doc = json!({"data": {"n": 1}});
    let (s, _) = api
        .send(Method::PUT, "docs/data/guestbook/e/a", Some(doc))
        .await;
    assert_eq!(s, 200);
    let list = "docs?collection=data/guestbook/e";
    for path in [
        "docs/data/guestbook/e/a",
        list,
        "docs/data/guestbook/e/none",
    ] {
        let (s, tag) = get_tagged(&api, path, None).await;
        assert!(s == 200 || s == 404, "{path}: {s}");
        let tag = tag.unwrap_or_else(|| panic!("{path} has no ETag"));
        let again = get_tagged(&api, path, Some(&tag)).await;
        assert_eq!(again, (304, Some(tag.clone())), "{path}");
        let other = get_tagged(&api, path, Some("W/\"other\"")).await;
        assert_eq!(other.0, s, "{path}");
    }
    let before = get_tagged(&api, list, None).await.1.unwrap();
    // Any write to the artifact's documents, even elsewhere, makes it stale.
    let (s, _) = api
        .send(Method::PUT, "docs/other/x", Some(json!({"data": {}})))
        .await;
    assert_eq!(s, 200);
    let (s, after) = get_tagged(&api, list, Some(&before)).await;
    assert_eq!(s, 200);
    assert_ne!(after.as_deref(), Some(before.as_str()));
    // Another caller's validator is never this one's: a visitor reads
    // other documents at the same sequence.
    open_to_world(&ts, &aid);
    let me = visitor(&ts, &aid).await;
    let theirs = on(&ts, &aid, Some(&me));
    let (s, tag) = get_tagged(&theirs, list, after.as_deref()).await;
    assert_eq!(s, 200);
    assert_ne!(tag, after);
    // A change to the declaration (its rules) makes it stale too.
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/artifacts/{aid}", ts.base))
                .json(&json!({"capabilities": {"db": {}}})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(get_tagged(&api, list, after.as_deref()).await.0, 200);
    // A write carries none.
    let res = api
        .req(Method::PUT, "docs/other/y")
        .json(&json!({"data": {}}))
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("etag").is_none());
}

/// `db.artifact(<source>)` on a site: another app's documents, read through
/// the site's `read/<source>/docs…` for its artifact alone, once the owner
/// granted it, by whoever may read the source; never by a visitor, and only
/// reads.
#[tokio::test]
async fn a_site_reads_another_apps_documents_once_granted_and_never_for_a_visitor() {
    let ts = TestServer::spawn().await;
    let b = artifact(&ts, json!({"db": {}})).await;
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{b}/docs/runs/r1", ts.base))
                .json(&json!({"data": {"km": 5}, "lww": true})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let a = artifact(&ts, json!({"db": {"read": [b]}})).await;
    let cookie = ts.site_cookie(&a).await;
    let api = on(&ts, &a, Some(&cookie));
    let doc = format!("read/{b}/docs/runs/r1");
    let (s, v) = api.send(Method::GET, &doc, None).await;
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{a}/db-grants/{b}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let (s, v) = api.send(Method::GET, &doc, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["doc"]["data"], json!({"km": 5}));
    let (s, v) = api
        .send(Method::GET, &format!("read/{b}/docs?collection=runs"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["docs"].as_array().unwrap().len(), 1, "{v}");
    // Reads alone, and only of a source A declares.
    let (s, _) = api
        .send(Method::PUT, &doc, Some(json!({"data": {"km": 6}})))
        .await;
    assert_eq!(s, 405);
    let c = artifact(&ts, json!({"db": {}})).await;
    let (s, v) = api
        .send(Method::GET, &format!("read/{c}/docs/runs/r1"), None)
        .await;
    assert_eq!(s, 403, "{v}");
    assert!(["not_declared", "not_granted"].contains(&code(&v)), "{v}");
    // A's site open to the world: its visitors read none of B's.
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/artifacts/{a}", ts.base))
                .json(&json!({"site_access": "world"})),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let anon = visitor(&ts, &a).await;
    let (s, v) = on(&ts, &a, Some(&anon)).send(Method::GET, &doc, None).await;
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
}
