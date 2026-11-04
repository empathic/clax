//! A queue's installs (`routes/installs.rs`): made, listed, changed and
//! deleted by the queue's admin in the browser; their public config read
//! with no credential, its ETag and 304, and CORS for the install's own
//! origins alone.

use clax_testkit::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

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

const SITE: &str = "https://changelist.example.com";

/// A daemon, the owner's cookie, a queue and an install of it.
async fn world() -> (TestServer, String, String, String) {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let owner = ts.owner_cookie();
    let (s, v) = browser(
        &ts,
        &owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Changelist bugs", "origins": [SITE]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let qid = v["queue"]["id"].as_str().unwrap().to_string();
    let (s, v) = browser(
        &ts,
        &owner,
        Method::POST,
        &format!("/api/queues/{qid}/installs"),
        Some(json!({
            "name": " Changelist ",
            "origins": ["https://Changelist.Example.com/"],
            "config": {"product_name": "Changelist feedback", "queue_label": "Sends to"},
        })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let iid = v["install"]["id"].as_str().unwrap().to_string();
    (ts, owner, qid, iid)
}

#[tokio::test]
async fn an_install_is_made_listed_changed_and_deleted_in_the_browser() {
    let (ts, owner, qid, iid) = world().await;
    let (s, v) = browser(&ts, &owner, Method::GET, &format!("/api/queues/{qid}/installs"), None).await;
    assert_eq!(s, 200, "{v}");
    let i = &v["installs"][0];
    assert_eq!(i["id"], iid.as_str());
    assert_eq!(i["name"], "Changelist");
    assert_eq!(i["origins"], json!([SITE]));
    assert_eq!(i["config"]["product_name"], "Changelist feedback");
    assert_eq!(i["config"]["reports_button"], true);
    assert_eq!(i["config"]["picker"], true);

    let (s, v) = browser(
        &ts,
        &owner,
        Method::PATCH,
        &format!("/api/queues/{qid}/installs/{iid}"),
        Some(json!({"config": {"reports_button": false, "brand_color": "#0A7"}})),
    )
    .await;
    assert_eq!(s, 400, "a colour is #rrggbb: {v}");
    assert_eq!(v["error"]["code"], "invalid_install");

    let (s, v) = browser(
        &ts,
        &owner,
        Method::PATCH,
        &format!("/api/queues/{qid}/installs/{iid}"),
        Some(json!({"config": {"reports_button": false, "brand_color": "#00AA77"}})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["install"]["config"]["reports_button"], false);
    assert_eq!(v["install"]["config"]["brand_color"], "#00aa77");
    assert_eq!(v["install"]["config"]["product_name"], Value::Null);

    let (s, _) = browser(
        &ts,
        &owner,
        Method::DELETE,
        &format!("/api/queues/{qid}/installs/{iid}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = browser(&ts, &owner, Method::GET, &format!("/api/queues/{qid}/installs"), None).await;
    assert_eq!((s, v["installs"].clone()), (200, json!([])));
    let r = ts
        .client
        .get(format!("{}/api/installs/{iid}/config", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404, "a deleted install has no config");
}

/// The apps home declares (`~/.config/clax/apps.json`), before Clax's own
/// Service is live and with Changeling's bridge port and Service.
const APPS: &str = r#"{
  "clax": {"id": "app:clax", "service": "clax", "addresses": [
    {"origin": "http://127.0.0.1:7480", "prefix": "/", "network": "loopback"},
    {"origin": "https://phobos.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"},
    {"origin": "https://clax.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"}]},
  "basis": {"id": "app:basis", "addresses": [
    {"origin": "http://127.0.0.1:7484", "prefix": "/", "network": "loopback"},
    {"origin": "https://phobos.tail556f5b.ts.net", "prefix": "/fold", "network": "tailnet"}]},
  "changeling": {"id": "app:changeling", "service": "changeling", "addresses": [
    {"origin": "http://127.0.0.1:7700", "prefix": "/", "network": "loopback"},
    {"origin": "https://phobos.tail556f5b.ts.net:8444", "prefix": "/", "network": "tailnet"},
    {"origin": "https://changeling.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"}]}
}"#;

#[tokio::test]
async fn a_queue_and_its_install_follow_the_app_home_declares_with_no_hand_edit() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let owner = ts.owner_cookie();
    // The Changeling queue as it is stored today: its loopback origin alone.
    let (s, v) = browser(
        &ts,
        &owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Changeling", "origins": ["http://127.0.0.1:7700"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let qid = v["queue"]["id"].as_str().unwrap().to_string();
    let (s, v) = browser(
        &ts,
        &owner,
        Method::POST,
        &format!("/api/queues/{qid}/installs"),
        Some(json!({"name": "Changeling", "origins": ["http://127.0.0.1:7700"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let iid = v["install"]["id"].as_str().unwrap().to_string();
    let config = format!("{}/api/installs/{iid}/config", ts.base);
    let container = format!("{}/_clax/containers/{qid}/config.json", ts.base);
    let bridge = "https://phobos.tail556f5b.ts.net:8444";
    let allowed = |r: &reqwest::Response| {
        r.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_string())
    };

    // Nothing declared: the tailnet bridge is another site, and the config
    // names no origin of Clax's.
    let r = ts.client.get(&config).header("origin", bridge).send().await.unwrap();
    assert_eq!(allowed(&r), None);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["origin"], Value::Null);
    let r = ts.client.get(&container).header("origin", bridge).send().await.unwrap();
    assert_eq!(allowed(&r), None);

    // Home declares app:changeling: the same rows now admit every address.
    ts.store
        .set_apps(clax_core::apps::Apps::parse(APPS).unwrap());
    for o in [bridge, "https://changeling.tail556f5b.ts.net", "http://127.0.0.1:7700"] {
        let r = ts.client.get(&config).header("origin", o).send().await.unwrap();
        assert_eq!(allowed(&r).as_deref(), Some(o), "install config from {o}");
        let r = ts.client.get(&container).header("origin", o).send().await.unwrap();
        assert_eq!(allowed(&r).as_deref(), Some(o), "container from {o}");
        let r = ts
            .client
            .request(Method::OPTIONS, format!("{}/api/queues/{qid}/reporter/reports", ts.base))
            .header("origin", o)
            .header("access-control-request-method", "POST")
            .send()
            .await
            .unwrap();
        assert_eq!(allowed(&r).as_deref(), Some(o), "reporter preflight from {o}");
    }
    for o in ["https://clax.tail556f5b.ts.net", "https://phobos.tail556f5b.ts.net"] {
        let r = ts.client.get(&config).header("origin", o).send().await.unwrap();
        assert_eq!(allowed(&r), None, "{o} is not Changeling's");
    }
    // The pages are on Clax's own Service origin, whatever Host was asked.
    let r = ts
        .client
        .get(&config)
        .header("origin", bridge)
        .header("host", "evil.example.com")
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["origin"], "https://clax.tail556f5b.ts.net", "{v}");
    assert_eq!(v["queue"]["page"], format!("/queues/{qid}"));

    // A queue may name the app itself; the list keeps the reference.
    let (s, v) = browser(
        &ts,
        &owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"origins": ["app:changeling"]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queue"]["origins"], json!(["app:changeling"]));
    let r = ts.client.get(&container).header("origin", bridge).send().await.unwrap();
    assert_eq!(allowed(&r).as_deref(), Some(bridge));
    let (s, v) = browser(
        &ts,
        &owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"origins": ["app:Not A Name"]})),
    )
    .await;
    assert_eq!((s, v["error"]["code"].clone()), (400, json!("invalid_origin")));
}

#[tokio::test]
async fn the_config_is_public_with_an_etag_and_cors_for_the_installs_origins_alone() {
    let (ts, owner, qid, iid) = world().await;
    let url = format!("{}/api/installs/{iid}/config", ts.base);

    // From the install's own site, with no credential.
    let r = ts.client.get(&url).header("origin", SITE).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let h = r.headers().clone();
    assert_eq!(h["access-control-allow-origin"], SITE);
    assert_eq!(h["access-control-expose-headers"], "ETag");
    assert_eq!(h["cache-control"], "no-cache");
    let etag = h["etag"].to_str().unwrap().to_string();
    assert!(etag.starts_with('"') && etag.len() == 66, "{etag}");
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["install_id"], iid.as_str());
    assert_eq!(v["queue"]["id"], qid.as_str());
    assert_eq!(v["queue"]["name"], "Changelist bugs");
    assert_eq!(v["queue"]["reporters_see"], "own");
    assert_eq!(v["queue"]["page"], format!("/queues/{qid}"));
    assert_eq!(v["queue"]["reporter_page"], format!("/reporter/{qid}"));
    assert_eq!(v["config"]["queue_label"], "Sends to");
    assert!(!v.to_string().contains("origins"), "the config lists no origins");

    // Unchanged: 304, the same tag.
    let r = ts
        .client
        .get(&url)
        .header("origin", SITE)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 304);
    assert_eq!(r.headers()["etag"].to_str().unwrap(), etag);

    // Another site's page is not allowed to read it.
    let r = ts
        .client
        .get(&url)
        .header("origin", "https://evil.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert!(r.headers().get("access-control-allow-origin").is_none());

    // The preflight a conditional fetch sends.
    let r = ts
        .client
        .request(Method::OPTIONS, &url)
        .header("origin", SITE)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "if-none-match")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    assert_eq!(r.headers()["access-control-allow-origin"], SITE);
    assert_eq!(r.headers()["access-control-allow-headers"], "If-None-Match");

    // A change to the queue's name changes the config and its tag.
    let (s, _) = browser(
        &ts,
        &owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Changelist reports"})),
    )
    .await;
    assert_eq!(s, 200);
    let r = ts
        .client
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert_ne!(r.headers()["etag"].to_str().unwrap(), etag);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["queue"]["name"], "Changelist reports");

    let r = ts
        .client
        .get(format!("{}/api/installs/nope/config", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
}

#[tokio::test]
async fn only_the_queues_admin_changes_its_installs() {
    let (ts, _owner, qid, iid) = world().await;
    let sam_client = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam_client.grant_id()).unwrap();
    let sam = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    let (s, v) = browser(&ts, &sam, Method::GET, &format!("/api/queues/{qid}/installs"), None).await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"]["code"], "not_a_member");
    let (s, _) = browser(
        &ts,
        &sam,
        Method::DELETE,
        &format!("/api/queues/{qid}/installs/{iid}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
    let (s, _) = browser(
        &ts,
        &sam,
        Method::POST,
        &format!("/api/queues/{qid}/installs"),
        Some(json!({"name": "Mine"})),
    )
    .await;
    assert_eq!(s, 404);
}
