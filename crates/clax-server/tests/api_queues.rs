//! Report queues' administration (`routes/queues.rs`) and discovery
//! (`routes/discovery.rs`): queues made, listed, changed and deleted in a
//! browser; members and their roles; service grants shown once and their
//! revocation ending the reporter tokens they minted; another person's
//! queue out of reach; agents refused; `/.well-known/clax` on a local and a
//! hosted Clax.

use base64::Engine as _;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A request from the browser holding `cookie` (`name=value`).
async fn browser(
    ts: &TestServer,
    cookie: &str,
    m: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(m, format!("{}{path}", ts.base))
        .header("cookie", cookie);
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// A daemon listening beyond loopback (so it invites), Sam connected, and
/// the owner's and Sam's browser cookies.
struct World {
    ts: TestServer,
    owner: String,
    sam: String,
    sam_public: String,
    sam_client: clax_testkit::TestClient,
}

async fn world() -> World {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam_client = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam_client.grant_id()).unwrap();
    let sam = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    World {
        owner: ts.owner_cookie(),
        sam,
        sam_public: v.public_id,
        sam_client,
        ts,
    }
}

async fn make_queue(w: &World, cookie: &str, name: &str) -> String {
    let (s, v) = browser(
        &w.ts,
        cookie,
        Method::POST,
        "/api/queues",
        Some(json!({"name": name, "origins": ["https://App.Example.com/"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["queue"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_queue_is_made_listed_changed_and_deleted_in_the_browser() {
    let w = world().await;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "  Bugs ", "origins": ["https://App.Example.com/"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let q = &v["queue"];
    assert_eq!(q["name"], "Bugs");
    assert_eq!(q["origins"], json!(["https://app.example.com"]));
    assert_eq!(q["reporters_see"], "own");
    assert_eq!(q["role"], "admin");
    let qid = q["id"].as_str().unwrap().to_string();

    let (s, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"].as_array().unwrap().len(), 1);
    assert_eq!(v["queues"][0]["id"], qid.as_str());

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Reports", "reporters_see": "all"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queue"]["name"], "Reports");
    assert_eq!(v["queue"]["reporters_see"], "all");
    assert_eq!(v["queue"]["origins"], json!(["https://app.example.com"]));

    // Bad input is refused, naming what is wrong.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"origins": ["app.example.com"]})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_origin"), "{v}");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": " "})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_name"), "{v}");

    // A page of another origin cannot make one in the owner's name.
    let (s, v) = json_of(
        w.ts.client
            .post(format!("{}/api/queues", w.ts.base))
            .header("cookie", &w.owner)
            .header("origin", "https://evil.example")
            .json(&json!({"name": "Mine"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("/api/queues/{qid}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Back"})),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
}

#[tokio::test]
async fn members_hold_the_role_an_admin_gives_them() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    let members = format!("/api/queues/{qid}/members");
    let sams = format!("{members}/{}", w.sam_public);

    // Not yet a member: Sam reaches nothing of it.
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &members, None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // Triage: Sam lists it and changes nothing.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["members"],
        json!([{"public_id": w.sam_public, "name": "Sam", "role": "triage"}])
    );
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"][0]["id"], qid.as_str());
    assert_eq!(v["queues"][0]["role"], "triage");
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &members, None).await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");

    // Sam's agent lists the queues Sam is a member of.
    let (s, v) = json_of(w.sam_client.agent("GET", "/api/queues", None, None).await).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["role"], "triage");

    // Admin: Sam changes it.
    browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "admin"})),
    )
    .await;
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // Only connected people are added.
    let ann = w.ts.viewer(Some("Ann")).await;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("{members}/{}", ann.public_id),
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "owner"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_json"), "{v}");

    // Removed: Sam reaches nothing of it again.
    let (s, v) = browser(&w.ts, &w.owner, Method::DELETE, &sams, None).await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(v["members"], json!([]));
}

#[tokio::test]
async fn another_persons_queue_is_not_found() {
    let w = world().await;
    let qid = make_queue(&w, &w.sam, "Sam's bugs").await;
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"][0]["role"], "admin");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    for (m, path, body) in [
        (
            Method::PATCH,
            format!("/api/queues/{qid}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/api/queues/{qid}"), None),
        (Method::GET, format!("/api/queues/{qid}/members"), None),
        (
            Method::GET,
            format!("/api/queues/{qid}/service-grants"),
            None,
        ),
        (
            Method::POST,
            format!("/api/queues/{qid}/service-grants"),
            Some(json!({"name": "x"})),
        ),
    ] {
        let (s, v) = browser(&w.ts, &w.owner, m.clone(), &path, body).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{m} {path}: {v}");
    }
    // A viewer who is no connected person makes none.
    let ann = w.ts.viewer(Some("Ann")).await;
    let (s, v) = browser(
        &w.ts,
        &format!("clax_viewer={}", ann.cookie),
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Ann's"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_a_person"), "{v}");
}

#[tokio::test]
async fn agents_list_queues_and_administer_none() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    // The owner's agent lists the owner's queues.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.get(format!("{}/api/queues", w.ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["id"], qid.as_str());
    // … and makes none.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.post(format!("{}/api/queues", w.ts.base)))
            .header("x-clax-channel", "agent")
            .json(&json!({"name": "Agent's"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    // A person's agent is refused every admin route.
    let (s, v) = json_of(
        w.sam_client
            .agent("POST", "/api/queues", None, Some(json!({"name": "x"})))
            .await,
    )
    .await;
    assert_eq!(s, 403, "{v}");
    // The owner by hand administers.
    let (s, v) = json_of(
        w.ts.authed(
            w.ts.client
                .get(format!("{}/api/queues/{qid}/service-grants", w.ts.base)),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn a_service_grant_is_shown_once_and_its_revocation_ends_its_reporter_tokens() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    let grants = format!("/api/queues/{qid}/service-grants");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        &grants,
        Some(json!({"name": "Backend"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["grant"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["grant"]["name"], "Backend");
    assert!(v["grant"]["created_at"].is_string());
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxq_"), "{token}");
    let pkcs8 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(v["private_key"].as_str().unwrap())
        .unwrap();
    let key = clax_core::sig::KeyPair::from_pkcs8(&pkcs8).unwrap();

    // The grant is its token's, with the key's public half alone.
    let grant =
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(&token))
            .unwrap()
            .expect("the token names the grant");
    assert_eq!(grant.id, gid);
    assert_eq!(
        grant.public_key,
        clax_core::sig::encode_public(&key.public())
    );

    // Listed without its token or key.
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &grants, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grants"].as_array().unwrap().len(), 1);
    let listed = v["grants"][0].as_object().unwrap();
    let mut keys: Vec<&str> = listed.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["created_at", "id", "name"]);
    assert!(!v.to_string().contains(&token));

    // It mints a reporter token, which its revocation ends.
    let reporter_key = clax_core::sig::KeyPair::generate();
    let (rtoken, _) =
        w.ts.store
            .mint_reporter_token(
                &grant,
                "user-1",
                "https://app.example.com",
                &clax_core::sig::encode_public(&reporter_key.public()),
                None,
            )
            .unwrap();
    assert!(
        w.ts.store
            .reporter_by_token(&rtoken.sha256_hex())
            .unwrap()
            .is_some()
    );
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("{grants}/{gid}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        w.ts.store.reporter_by_token(&rtoken.sha256_hex()).unwrap(),
        None
    );
    assert_eq!(
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(&token))
            .unwrap(),
        None
    );
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &grants, None).await;
    assert_eq!(v["grants"], json!([]));
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("{grants}/{gid}"),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

fn assert_discovery(v: &Value, kind: &str) {
    assert_eq!(v["kind"], kind, "{v}");
    assert_eq!(v["contract_version"], "1");
    let sign_in = v["sign_in"].as_array().unwrap();
    assert!(sign_in.contains(&json!("invite")), "{v}");
    assert!(sign_in.contains(&json!("reporter_token")), "{v}");
    assert!(
        v["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("queues"))
    );
}

#[tokio::test]
async fn discovery_says_what_this_clax_is_without_a_credential() {
    let ts = TestServer::spawn().await;
    let (s, v) = json_of(
        ts.client
            .get(format!("{}/.well-known/clax", ts.base))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_discovery(&v, "local");

    let clock = Arc::new(ManualClock::at("2026-10-27T10:00:00Z"));
    let hosted = TestServer::spawn_hosted(clock, Some("the setup secret")).await;
    let (s, v) = json_of(
        hosted
            .client
            .get(format!("{}/.well-known/clax", hosted.base))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_discovery(&v, "hosted");
    assert!(
        v["sign_in"]
            .as_array()
            .unwrap()
            .contains(&json!("sign_in_link"))
    );
}
