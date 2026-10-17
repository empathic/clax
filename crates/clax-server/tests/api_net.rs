//! The `net` capability's routes (spec 2026-10-12-flutter-real-apps-design
//! C6): the owner's grants and the daemon's fetch, over a resolver and a
//! transport that never touch the network.

use clax_http::net::{Hop, HopResponse, Net, NetError, Resolver, Transport};
use clax_testkit::TestServer;
use futures::future::BoxFuture;
use reqwest::Method;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

struct Table;
impl Resolver for Table {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, std::io::Result<Vec<SocketAddr>>> {
        let ip = match host {
            "api.example.com" => "93.184.216.34",
            "intranet.example.com" => "10.0.0.7",
            _ => "93.184.216.99",
        };
        Box::pin(async move { Ok(vec![SocketAddr::new(ip.parse().unwrap(), port)]) })
    }
}

/// Answers every hop with its method and URL, and keeps the hops.
#[derive(Default)]
struct Echo(Mutex<Vec<Hop>>);
impl Transport for Echo {
    fn send(&self, hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>> {
        // A `.png` path answers bytes that are not UTF-8.
        let body = if hop.url.path().ends_with(".png") {
            vec![0x89, b'P', b'N', b'G', 0xff, 0x00]
        } else {
            format!("{} {}", hop.method, hop.url).into_bytes()
        };
        self.0.lock().unwrap().push(hop);
        Box::pin(async move {
            Ok(HopResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "text/plain".into()),
                    ("set-cookie".into(), "s=1".into()),
                ],
                body,
            })
        })
    }
}

async fn spawn() -> (TestServer, Arc<Echo>) {
    let echo = Arc::new(Echo::default());
    let t = echo.clone();
    let ts = TestServer::spawn_with(move |s| s.net = Arc::new(Net::new(Arc::new(Table), t))).await;
    (ts, echo)
}

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

async fn app(ts: &TestServer, caps: Value) -> String {
    let (status, body) = send(authed(ts, Method::POST, "/api/artifacts").json(
        &json!({"title": "Weather", "capabilities": caps, "files": {"index.html": {"content": "<p>x</p>"}}}),
    ))
    .await;
    assert_eq!(status, 201, "{body}");
    body["artifact"]["id"].as_str().unwrap().to_string()
}

async fn fetch(ts: &TestServer, aid: &str, body: Value) -> (u16, Value) {
    send(authed(ts, Method::POST, &format!("/api/artifacts/{aid}/net:fetch")).json(&body)).await
}

#[tokio::test]
async fn a_fetch_needs_the_owners_grant_and_ends_with_its_revocation() {
    let (ts, echo) = spawn().await;
    let aid = app(
        &ts,
        json!({"net": {"hosts": ["api.example.com", "intranet.example.com"]}}),
    )
    .await;
    let grants = format!("/api/artifacts/{aid}/net-grants");

    let (status, body) = send(authed(&ts, Method::GET, &grants)).await;
    assert_eq!(status, 200);
    assert_eq!(
        body["declared"],
        json!(["api.example.com", "intranet.example.com"])
    );
    assert_eq!(
        (body["grants"].clone(), body["granted"].clone()),
        (json!([]), json!(false))
    );

    let req = json!({"url": "https://api.example.com/v1?q=1", "method": "POST", "headers": {"cookie": "c=1", "x-key": "k"}, "body": "{}"});
    let (status, body) = fetch(&ts, &aid, req.clone()).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("not_granted")),
        "{body}"
    );
    assert!(echo.0.lock().unwrap().is_empty());

    // Only the hosts the owner was shown are granted; a PUT names them.
    let (status, body) = send(authed(&ts, Method::PUT, &grants)).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (400, Some("invalid_argument"))
    );
    let (status, body) =
        send(authed(&ts, Method::PUT, &grants).json(&json!({"hosts": ["intranet.example.com"]})))
            .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["granted"], json!(false));
    let (status, body) = fetch(&ts, &aid, req.clone()).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("not_granted"))
    );

    let (status, body) = send(authed(&ts, Method::PUT, &grants).json(
        &json!({"hosts": ["api.example.com", "intranet.example.com", "unseen.example.com"]}),
    ))
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["granted"], json!(true));
    assert_eq!(body["grants"].as_array().unwrap().len(), 2);

    let (status, body) = fetch(&ts, &aid, req.clone()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], 200);
    assert_eq!(body["url"], "https://api.example.com/v1?q=1");
    assert_eq!(body["body"], "POST https://api.example.com/v1?q=1");
    assert_eq!(body.get("body_base64"), None);
    assert_eq!(body["headers"], json!({"content-type": "text/plain"}));
    {
        let hops = echo.0.lock().unwrap();
        assert_eq!(
            hops[0].addrs,
            vec!["93.184.216.34:443".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(
            hops[0].headers,
            vec![
                ("x-key".to_string(), "k".to_string()),
                (
                    "user-agent".to_string(),
                    clax_http::net::USER_AGENT.to_string()
                )
            ]
        );
        assert_eq!(hops[0].body.as_deref(), Some(&b"{}"[..]));
    }

    // A body that is not UTF-8 comes as base64, byte for byte.
    let (status, body) = fetch(&ts, &aid, json!({"url": "https://api.example.com/t/1.png"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.get("body"), None);
    assert_eq!(body["body_base64"], "iVBOR/8A");

    let (status, body) = fetch(&ts, &aid, json!({"url": "https://intranet.example.com/"})).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("blocked_address")),
        "{body}"
    );

    let (status, body) = send(authed(&ts, Method::DELETE, &grants)).await;
    assert_eq!((status, body["revoked"].clone()), (200, json!(2)));
    let (status, body) = fetch(&ts, &aid, req).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("not_granted"))
    );
}

#[tokio::test]
async fn refusals_before_any_request() {
    let (ts, echo) = spawn().await;
    let aid = app(&ts, json!({"net": {"hosts": ["api.example.com"]}})).await;
    let (status, _) = send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/net-grants"),
        )
        .json(&json!({"hosts": ["api.example.com"]})),
    )
    .await;
    assert_eq!(status, 200);
    for (body, want) in [
        (
            json!({"url": "https://other.example.com/"}),
            (403, "not_declared"),
        ),
        (
            json!({"url": "http://api.example.com/"}),
            (400, "https_only"),
        ),
        (
            json!({"url": "https://api.example.com/", "method": "CONNECT"}),
            (400, "invalid_argument"),
        ),
        (json!({"nope": 1}), (400, "invalid_argument")),
    ] {
        let (status, got) = fetch(&ts, &aid, body.clone()).await;
        assert_eq!(
            (status, got["error"]["code"].as_str().unwrap_or_default()),
            want,
            "{body}: {got}"
        );
    }
    let plain = app(&ts, json!({"db": {}})).await;
    let (status, got) = fetch(&ts, &plain, json!({"url": "https://api.example.com/"})).await;
    assert_eq!(
        (status, got["error"]["code"].as_str()),
        (403, Some("not_declared"))
    );
    let (status, got) = send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{plain}/net-grants"),
        )
        .json(&json!({"hosts": ["api.example.com"]})),
    )
    .await;
    assert_eq!(
        (status, got["error"]["code"].as_str()),
        (400, Some("not_declared"))
    );
    let (status, _) = fetch(
        &ts,
        "zzzzzzzzzzzz",
        json!({"url": "https://api.example.com/"}),
    )
    .await;
    assert_eq!(status, 404);

    // Without the token nothing is fetched or granted.
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/net:fetch", ts.base))
        .json(&json!({"url": "https://api.example.com/"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = ts
        .client
        .put(format!("{}/api/artifacts/{aid}/net-grants", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert!(echo.0.lock().unwrap().is_empty());

    let (status, got) = send(authed(&ts, Method::POST, "/api/artifacts").json(
        &json!({"title": "x", "capabilities": {"net": {"hosts": ["127.0.0.1"]}}, "files": {"index.html": {"content": "<p>x</p>"}}}),
    ))
    .await;
    assert_eq!(
        (status, got["error"]["code"].as_str()),
        (400, Some("invalid_capabilities"))
    );
}

/// `POST /api/drafts` of `of` declaring `caps`; the draft's ID.
async fn draft(ts: &TestServer, of: Option<&str>, caps: Value) -> String {
    let (status, body) = send(authed(ts, Method::POST, "/api/drafts").json(
        &json!({"of": of, "title": "Draft", "capabilities": caps, "files": {"index.html": "<p>d</p>"}}),
    ))
    .await;
    assert_eq!(status, 201, "{body}");
    body["draft"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_draft_fetches_under_its_live_apps_grants() {
    let (ts, echo) = spawn().await;
    let aid = app(
        &ts,
        json!({"net": {"hosts": ["api.example.com", "tiles.example.com"]}}),
    )
    .await;
    let grants = format!("/api/artifacts/{aid}/net-grants");
    let caps = json!({"net": {"hosts": ["api.example.com", "new.example.com"]}});
    let did = draft(&ts, Some(&aid), caps.clone()).await;
    let fresh = draft(&ts, None, caps).await;
    let code = |r: (u16, Value)| {
        (
            r.0,
            r.1["error"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };
    let get = |h: &str| json!({"url": format!("https://{h}/v1")});

    assert_eq!(
        code(fetch(&ts, &did, get("api.example.com")).await),
        (403, "not_granted".into()),
        "nothing granted yet"
    );
    let (status, _) = send(
        authed(&ts, Method::PUT, &grants)
            .json(&json!({"hosts": ["api.example.com", "tiles.example.com"]})),
    )
    .await;
    assert_eq!(status, 200);

    let (status, body) = fetch(&ts, &did, get("api.example.com")).await;
    assert_eq!(
        (status, body["body"].as_str()),
        (200, Some("GET https://api.example.com/v1")),
        "{body}"
    );
    assert_eq!(
        code(fetch(&ts, &did, get("tiles.example.com")).await),
        (403, "not_declared".into()),
        "the draft does not declare it"
    );
    assert_eq!(
        code(fetch(&ts, &did, get("new.example.com")).await),
        (403, "not_granted".into()),
        "the app has no grant for it"
    );
    assert_eq!(
        code(fetch(&ts, &fresh, get("api.example.com")).await),
        (403, "not_granted".into()),
        "a new app's draft has no live app"
    );

    // A draft never takes a grant of its own.
    let (status, body) = send(
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{did}/net-grants"),
        )
        .json(&json!({"hosts": ["new.example.com"]})),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("draft"))
    );

    let (status, _) = send(authed(&ts, Method::DELETE, &grants)).await;
    assert_eq!(status, 200);
    assert_eq!(
        code(fetch(&ts, &did, get("api.example.com")).await),
        (403, "not_granted".into()),
        "the app's revocation reaches the draft"
    );
    assert_eq!(echo.0.lock().unwrap().len(), 1, "one fetch went out");

    // A draft made offline (a bundle update's check) reaches nothing, even
    // a host its app is granted.
    let (status, _) =
        send(authed(&ts, Method::PUT, &grants).json(&json!({"hosts": ["api.example.com"]}))).await;
    assert_eq!(status, 200);
    let (status, body) = send(authed(&ts, Method::POST, "/api/drafts").json(&json!({
        "of": aid, "title": "Check", "inherit_net": false,
        "capabilities": {"net": {"hosts": ["api.example.com"]}},
        "files": {"index.html": "<p>d</p>"}
    })))
    .await;
    assert_eq!(status, 201, "{body}");
    let offline = body["draft"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        code(fetch(&ts, &offline, get("api.example.com")).await),
        (403, "not_granted".into())
    );
    let (status, _) = fetch(&ts, &did, get("api.example.com")).await;
    assert_eq!(status, 200, "the agent's draft still inherits");
}
