//! Approving an artifact's declaration (`crates/clax-server/src/routes/permissions.rs`):
//! what of a version's declaration is in force before and after its owner
//! approves it, who may approve, and what a publish answer, the artifact
//! view, the `net` and `sample` routes and a site's visitor see.

use clax_testkit::{EventReader, TestServer};
use reqwest::Method;
use reqwest::header::{COOKIE, ETAG, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};

const HOST_NAME: &str = "api.clax.invalid";

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// Creates an artifact declaring `caps`: the create answer.
async fn create(ts: &TestServer, caps: Value) -> Value {
    let (status, body) = send(authed(ts, Method::POST, "/api/artifacts").json(&json!({
        "title": "Weather",
        "capabilities": caps,
        "files": {"index.html": {"content": "<p>1</p>"}},
    })))
    .await;
    assert_eq!(status, 201, "{body}");
    body
}

async fn app(ts: &TestServer, caps: Value) -> String {
    create(ts, caps).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Publishes version `if_version + 1` of `aid` declaring `caps`.
async fn next(ts: &TestServer, aid: &str, if_version: u32, caps: Value) -> Value {
    let (status, body) = send(
        authed(ts, Method::POST, &format!("/api/artifacts/{aid}/versions")).json(&json!({
            "if_version": if_version,
            "capabilities": caps,
            "files": {"index.html": {"content": format!("<p>{}</p>", if_version + 1)}},
        })),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    body
}

/// The artifact view the token reads.
async fn view(ts: &TestServer, aid: &str) -> Value {
    let (status, body) = send(authed(ts, Method::GET, &format!("/api/artifacts/{aid}"))).await;
    assert_eq!(status, 200, "{body}");
    body["artifact"].clone()
}

/// A `PUT capability-approval` of `aid` with the token, before its body.
fn approval(ts: &TestServer, aid: &str) -> reqwest::RequestBuilder {
    authed(
        ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/capability-approval"),
    )
}

/// The `{version, declaration_sha256}` a `pending_capabilities` names.
fn shown(pending: &Value) -> Value {
    json!({"version": pending["version"], "declaration_sha256": pending["declaration_sha256"]})
}

fn is_sha256_hex(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

const OWNER_BROWSER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// `POST sample` as the owner's browser (the token and a viewer cookie).
async fn sample(ts: &TestServer, aid: &str) -> (u16, Value) {
    send(
        authed(ts, Method::POST, &format!("/api/artifacts/{aid}/sample"))
            .header(COOKIE, format!("clax_viewer={OWNER_BROWSER}"))
            .json(&json!({"input": "hi"})),
    )
    .await
}

#[tokio::test]
async fn a_new_declaration_is_not_in_force_until_its_owner_approves() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"net": {"hosts": [HOST_NAME]}, "db": {}})).await;
    let a = view(&ts, &aid).await;
    assert_eq!(a["capabilities_in_force"], json!({"db": {}}));
    assert_eq!(a["may_approve"], json!(true));
    let pending = a["pending_capabilities"].clone();
    assert!(is_sha256_hex(&pending["declaration_sha256"]), "{pending}");
    let mut without_sha = pending.clone();
    without_sha
        .as_object_mut()
        .unwrap()
        .remove("declaration_sha256");
    assert_eq!(
        without_sha,
        json!({"version": 1, "capabilities": ["net"], "hosts": [HOST_NAME]})
    );

    let (status, body) = send(
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{aid}/net:fetch"),
        )
        .json(&json!({"url": format!("https://{HOST_NAME}/v1")})),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");

    // Both of what was shown are required.
    let (status, body) = send(approval(&ts, &aid).json(&json!({"version": 1}))).await;
    assert_eq!((status, code(&body)), (400, "invalid_args"), "{body}");

    let (status, body) = send(approval(&ts, &aid).json(&shown(&pending))).await;
    assert_eq!(status, 200, "{body}");
    let declared = json!({"net": {"hosts": [HOST_NAME]}, "db": {}});
    assert_eq!(body["in_force"], declared);
    assert_eq!(body["approval"]["version"], json!(1));
    assert_eq!(body["approval"]["by"], json!("owner"));
    let a = view(&ts, &aid).await;
    assert_eq!(a["capabilities_in_force"], declared);
    assert_eq!(a["pending_capabilities"], Value::Null);
    // Approving granted the host that waited.
    let (status, body) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/net-grants"),
    ))
    .await;
    assert_eq!((status, &body["granted"]), (200, &json!(true)), "{body}");

    let (status, p) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/permissions"),
    ))
    .await;
    assert_eq!(status, 200, "{p}");
    assert_eq!(
        (&p["version"], &p["declared"], &p["in_force"], &p["pending"]),
        (&json!(1), &declared, &declared, &Value::Null)
    );
    assert_eq!(p["approved"]["capabilities"], declared);
    assert_eq!(p["net"]["declared"], json!([HOST_NAME]));
    assert_eq!(p["net"]["waiting"], json!([]));
    assert_eq!(p["net"]["granted"], json!(true));
    assert_eq!(p["sample"], json!({"granted": false, "granted_at": null}));
    assert_eq!(p["db_read"], json!([]));
}

#[tokio::test]
async fn the_publish_answer_says_what_waits() {
    let ts = TestServer::spawn().await;
    let made = create(&ts, json!({"net": {"hosts": [HOST_NAME]}, "db": {}})).await;
    assert_eq!(
        made["pending_capabilities"]["capabilities"],
        json!(["net"]),
        "{made}"
    );
    let aid = made["artifact"]["id"].as_str().unwrap().to_string();
    ts.approve(&aid).await.expect("net waited");
    // A version adding a host says that host waits; one with nothing gated
    // added says nothing does.
    let v2 = next(
        &ts,
        &aid,
        1,
        json!({"net": {"hosts": [HOST_NAME, "b.clax.invalid"]}, "db": {}}),
    )
    .await;
    assert_eq!(v2["pending_capabilities"]["version"], json!(2), "{v2}");
    assert_eq!(v2["pending_capabilities"]["capabilities"], json!([]));
    assert_eq!(
        v2["pending_capabilities"]["hosts"],
        json!(["b.clax.invalid"])
    );
    let plain = create(&ts, json!({"db": {}})).await;
    assert_eq!(plain["pending_capabilities"], Value::Null, "{plain}");
}

#[tokio::test]
async fn approving_a_version_that_is_not_current_is_refused() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"sample": {}})).await;
    let shown_v1 = view(&ts, &aid).await["pending_capabilities"].clone();
    next(&ts, &aid, 1, json!({"sample": {}})).await;
    let (status, body) = send(approval(&ts, &aid).json(&shown(&shown_v1))).await;
    assert_eq!((status, code(&body)), (409, "stale_version"), "{body}");
    assert_eq!(body["error"]["current"], json!(2), "{body}");
    let a = view(&ts, &aid).await;
    assert_eq!(a["capabilities_in_force"], json!({}));
    assert_eq!(a["pending_capabilities"]["version"], json!(2));
    let (_, p) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/permissions"),
    ))
    .await;
    assert_eq!(p["approved"], Value::Null, "{p}");
}

#[tokio::test]
async fn a_declaration_changed_at_the_same_version_is_not_approved_unseen() {
    let ts = TestServer::spawn().await;
    // Declared by PATCH (an artifact whose clax.json does not declare it),
    // so the declaration changes without a new version.
    let aid = ts.publish("Weather", &[("index.html", "<p>1</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = send(
        authed(&ts, Method::PATCH, &format!("/api/artifacts/{aid}"))
            .json(&json!({"capabilities": {"net": {"hosts": [HOST_NAME]}}})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let seen = view(&ts, &aid).await["pending_capabilities"].clone();
    assert_eq!(seen["version"], json!(1), "{seen}");
    // A PATCH adds a host while the owner reviews: still version 1.
    let (status, body) = send(
        authed(&ts, Method::PATCH, &format!("/api/artifacts/{aid}"))
            .json(&json!({"capabilities": {"net": {"hosts": [HOST_NAME, "b.clax.invalid"]}}})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = send(approval(&ts, &aid).json(&shown(&seen))).await;
    assert_eq!((status, code(&body)), (409, "stale_version"), "{body}");
    assert_eq!(body["error"]["current"], json!(1), "{body}");
    let a = view(&ts, &aid).await;
    assert_eq!(a["capabilities_in_force"], json!({}));
    let now = &a["pending_capabilities"];
    assert_eq!(now["hosts"], json!([HOST_NAME, "b.clax.invalid"]));
    assert_ne!(now["declaration_sha256"], seen["declaration_sha256"]);
    let (_, g) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/net-grants"),
    ))
    .await;
    assert_eq!(g["grants"], json!([]), "nothing was granted: {g}");
}

#[tokio::test]
async fn no_agent_and_no_page_approves() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let aid = app(&ts, json!({"shell": {}})).await;
    let pending = view(&ts, &aid).await["pending_capabilities"].clone();
    let session = ts.register_session("claude", "perm-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let approve = format!("/api/artifacts/{aid}/capability-approval");
    let withdraw = format!("/api/artifacts/{aid}/capability-approval/shell");
    let read = format!("/api/artifacts/{aid}/permissions");

    // The owner's agent: the token with its session.
    for (m, path) in [
        (Method::PUT, &approve),
        (Method::DELETE, &withdraw),
        (Method::GET, &read),
    ] {
        let (status, body) = send(
            authed(&ts, m.clone(), path)
                .header("x-clax-session", &session)
                .json(&shown(&pending)),
        )
        .await;
        assert_eq!(
            (status, code(&body)),
            (403, "not_for_agents"),
            "{m} {path}: {body}"
        );
    }
    // A page, through the owner's shell.
    let (status, body) = send(
        approval(&ts, &aid)
            .header("x-clax-via", "page")
            .json(&shown(&pending)),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "not_for_pages"), "{body}");
    // A person's agent.
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sam_session = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = sam
        .agent("PUT", &approve, Some(&sam_session), Some(shown(&pending)))
        .await;
    assert_eq!(res.status(), 403, "{}", res.text().await.unwrap());
    // A person's browser at the owner's artifact: not theirs to approve.
    let v = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let (status, body) = send(
        ts.client
            .put(format!("{}{approve}", ts.base))
            .header(COOKIE, format!("clax_viewer={}", ts.browser_cookie(&v.id)))
            .json(&shown(&pending)),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "admin_only"), "{body}");
    // The agent channel's marker on the token.
    for (m, path) in [
        (Method::PUT, &approve),
        (Method::DELETE, &withdraw),
        (Method::GET, &read),
    ] {
        let (status, body) = send(
            authed(&ts, m.clone(), path)
                .header("x-clax-channel", "agent")
                .json(&shown(&pending)),
        )
        .await;
        assert_eq!(
            (status, code(&body)),
            (403, "not_for_agents"),
            "{m} {path}: {body}"
        );
    }
    // A request from another origin, approving or withdrawing.
    let (status, body) = send(
        approval(&ts, &aid)
            .header(ORIGIN, "http://elsewhere.example")
            .json(&shown(&pending)),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "forbidden_origin"), "{body}");
    let (status, body) =
        send(authed(&ts, Method::DELETE, &withdraw).header(ORIGIN, "http://elsewhere.example"))
            .await;
    assert_eq!((status, code(&body)), (403, "forbidden_origin"), "{body}");

    let a = view(&ts, &aid).await;
    assert_eq!(a["pending_capabilities"], pending, "nothing was approved");
    assert_eq!(a["capabilities_in_force"], json!({}));
    // An agent's view of the artifact offers no approval.
    let (status, body) = send(
        authed(&ts, Method::GET, &format!("/api/artifacts/{aid}"))
            .header("x-clax-session", &session),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["artifact"]["capabilities_in_force"], json!({}));
    assert_eq!(body["artifact"].get("may_approve"), None, "{body}");
    assert_eq!(body["artifact"].get("pending_capabilities"), None);

    // An unknown artifact is not found.
    let (status, _) = send(
        authed(
            &ts,
            Method::PUT,
            "/api/artifacts/7q3k9mzx2b4t/capability-approval",
        )
        .json(&json!({"version": 1, "declaration_sha256": "0".repeat(64)})),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn withdrawing_a_name_takes_it_out_of_force() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"sample": {}})).await;
    let (status, body) = sample(&ts, &aid).await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");
    ts.approve(&aid).await.expect("sample waited");
    let withdraw = format!("/api/artifacts/{aid}/capability-approval/sample");
    let (status, body) = send(authed(&ts, Method::DELETE, &withdraw)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"withdrawn": true, "in_force": {}}));
    // The grant approving made stays, and reaches nothing.
    let (_, g) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/sample-grant"),
    ))
    .await;
    assert_eq!(g["granted"], json!(true), "{g}");
    let (status, body) = sample(&ts, &aid).await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");
    let (status, body) = send(authed(
        &ts,
        Method::PUT,
        &format!("/api/artifacts/{aid}/sample-grant"),
    ))
    .await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");
    assert_eq!(
        view(&ts, &aid).await["pending_capabilities"]["capabilities"],
        json!(["sample"])
    );
    let (status, body) = send(authed(&ts, Method::DELETE, &withdraw)).await;
    assert_eq!((status, &body["withdrawn"]), (200, &json!(false)), "{body}");
    let (status, body) = send(authed(
        &ts,
        Method::DELETE,
        &format!("/api/artifacts/{aid}/capability-approval/db"),
    ))
    .await;
    assert_eq!((status, code(&body)), (400, "invalid_args"), "{body}");
}

#[tokio::test]
async fn approving_tells_open_views() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"shell": {}})).await;
    let res = ts.get(&format!("/api/events?artifact={aid}")).await;
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    assert_eq!(events.next().await.0, "ready");
    ts.approve(&aid).await.expect("shell waited");
    let ev = events.next_named("permissions").await;
    assert_eq!(ev, json!({"type": "permissions", "artifact_id": aid}));
}

#[tokio::test]
async fn a_visitor_reads_the_declaration_in_force() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"net": {"hosts": [HOST_NAME]}, "db": {}})).await;
    ts.store
        .set_site_open_to_world(&clax_core::ArtifactId::parse(&aid).unwrap(), true)
        .unwrap();
    let site = ts.site_host(&aid);
    let at = |path: &str, cookie: Option<&str>| {
        let mut r = ts
            .client
            .get(format!("{}/_clax/api/{path}", ts.base))
            .header(HOST, &site)
            .header(ORIGIN, format!("http://{site}"));
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        r
    };
    let res = at("whoami", None).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let cookie = res.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let (status, v) = send(at("artifact", Some(&cookie))).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["artifact"]["capabilities"], json!({"db": {}}), "{v}");
    ts.approve(&aid).await.expect("net waited");
    let (_, v) = send(at("artifact", Some(&cookie))).await;
    assert_eq!(
        v["artifact"]["capabilities"],
        json!({"net": {"hosts": [HOST_NAME]}, "db": {}})
    );
}

#[tokio::test]
async fn an_apps_draft_is_held_to_its_apps_approval_and_never_approved_itself() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"net": {"hosts": [HOST_NAME]}})).await;
    ts.approve(&aid).await.expect("net waited");
    let (status, body) = send(authed(&ts, Method::POST, "/api/drafts").json(&json!({
        "of": aid, "title": "Draft",
        "capabilities": {"net": {"hosts": [HOST_NAME, "b.clax.invalid"]}},
        "files": {"index.html": "<p>d</p>"},
    })))
    .await;
    assert_eq!(status, 201, "{body}");
    let did = body["draft"]["id"].as_str().unwrap().to_string();
    let (status, body) = send(
        approval(&ts, &did).json(&json!({"version": 1, "declaration_sha256": "0".repeat(64)})),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "draft"), "{body}");
    let (status, body) = send(
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{did}/net:fetch"),
        )
        .json(&json!({"url": "https://b.clax.invalid/v1"})),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");
}

#[tokio::test]
async fn on_a_local_daemon_a_person_does_not_approve_even_their_own_page() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = sam
        .agent(
            "POST",
            "/api/artifacts",
            Some(&sid),
            Some(json!({"title": "Sam's", "capabilities": {"sample": {}, "net": {"hosts": [HOST_NAME]}},
                "files": {"index.html": {"content": "<p>s</p>", "encoding": "utf8"}}})),
        )
        .await;
    assert_eq!(res.status(), 201);
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pending = view(&ts, &aid).await["pending_capabilities"].clone();
    let v = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let sams = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    let (status, body) = send(
        ts.client
            .get(format!("{}/api/artifacts/{aid}", ts.base))
            .header(COOKIE, &sams),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["artifact"].get("may_approve"), None, "{body}");
    assert_eq!(body["artifact"].get("pending_capabilities"), None);
    let (status, body) = send(
        ts.client
            .put(format!(
                "{}/api/artifacts/{aid}/capability-approval",
                ts.base
            ))
            .header(COOKIE, &sams)
            .json(&shown(&pending)),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "admin_only"), "{body}");
    assert_eq!(
        body["error"]["message"],
        "on a local Clax only the owner approves a page's capabilities"
    );
    let a = view(&ts, &aid).await;
    assert_eq!(a["pending_capabilities"], pending, "nothing was approved");
    assert_eq!(a["capabilities_in_force"], json!({}));
    let id = clax_core::ArtifactId::parse(&aid).unwrap();
    assert!(ts.store.net_access(&id).unwrap().grants.is_empty());
    assert_eq!(ts.store.sample_grant(&id).unwrap(), None);
    // The owner approves it, and that grants what waited.
    ts.approve(&aid).await.expect("it waited");
    assert!(ts.store.net_access(&id).unwrap().allows(HOST_NAME));
    assert!(ts.store.sample_grant(&id).unwrap().is_some());
}

#[tokio::test]
async fn withdrawing_net_refuses_its_fetches() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"net": {"hosts": [HOST_NAME]}})).await;
    ts.approve(&aid).await.expect("net waited");
    let (status, body) = send(authed(
        &ts,
        Method::DELETE,
        &format!("/api/artifacts/{aid}/capability-approval/net"),
    ))
    .await;
    assert_eq!((status, &body["withdrawn"]), (200, &json!(true)), "{body}");
    let (status, body) = send(
        authed(
            &ts,
            Method::POST,
            &format!("/api/artifacts/{aid}/net:fetch"),
        )
        .json(&json!({"url": format!("https://{HOST_NAME}/v1")})),
    )
    .await;
    assert_eq!((status, code(&body)), (403, "not_approved"), "{body}");
}

#[tokio::test]
async fn a_patch_that_narrows_the_approval_tells_open_views() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Weather", &[("index.html", "<p>1</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let patch = |caps: Value| {
        authed(&ts, Method::PATCH, &format!("/api/artifacts/{aid}"))
            .json(&json!({"capabilities": caps}))
    };
    let (status, body) = send(patch(
        json!({"net": {"hosts": [HOST_NAME, "b.clax.invalid"]}}),
    ))
    .await;
    assert_eq!(status, 200, "{body}");
    ts.approve(&aid).await.expect("net waited");
    let res = ts.get(&format!("/api/events?artifact={aid}")).await;
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    assert_eq!(events.next().await.0, "ready");
    let (status, body) = send(patch(json!({"net": {"hosts": [HOST_NAME]}}))).await;
    assert_eq!(status, 200, "{body}");
    let ev = events.next_named("permissions").await;
    assert_eq!(ev, json!({"type": "permissions", "artifact_id": aid}));
}

#[tokio::test]
async fn a_site_holder_reads_the_declaration_in_force() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"camera": {}, "db": {}})).await;
    let cookie = ts.site_cookie(&aid).await;
    let site = ts.site_host(&aid);
    let read = || {
        ts.client
            .get(format!("{}/_clax/api/artifact", ts.base))
            .header(HOST, &site)
            .header(ORIGIN, format!("http://{site}"))
            .header(COOKIE, &cookie)
    };
    let (status, v) = send(read()).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["artifact"]["capabilities"], json!({"db": {}}), "{v}");
    ts.approve(&aid).await.expect("camera waited");
    let (_, v) = send(read()).await;
    assert_eq!(
        v["artifact"]["capabilities"],
        json!({"camera": {}, "db": {}}),
        "{v}"
    );
}

#[tokio::test]
async fn a_site_holders_head_of_the_artifact_answers_without_the_full_bodys_validators() {
    let ts = TestServer::spawn().await;
    let aid = app(&ts, json!({"camera": {}, "db": {}})).await;
    let cookie = ts.site_cookie(&aid).await;
    let site = ts.site_host(&aid);
    let res = ts
        .client
        .head(format!("{}/_clax/api/artifact", ts.base))
        .header(HOST, &site)
        .header(ORIGIN, format!("http://{site}"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers().get(ETAG), None);
    assert!(res.bytes().await.unwrap().is_empty());
}
