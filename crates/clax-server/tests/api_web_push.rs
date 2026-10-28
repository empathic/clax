//! Web Push to the owner's devices: the subscription routes answer the
//! owner's browser alone (never a tailnet viewer through `tailscale serve`,
//! a LAN viewer, the token alone or an agent), and an item that waits on the owner, such as
//! an agent's question, is pushed to every subscribed device through its
//! push service (here a local one standing in for Apple's).
use clax_core::webpush::{Message, Receiver};
use clax_testkit::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// The `tailscale serve` name these tests trust.
const TRUSTED: &str = "phobos.tail556f5b.ts.net";

/// How a test request is made as one caller.
type As<'a> = Box<dyn Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + 'a>;

/// One request a push service took: its headers and body.
struct Pushed {
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

/// A push service on a loopback port: every POST to `/push/<device>` is sent
/// on the channel and answered with the status `answer` holds.
async fn push_service() -> (String, mpsc::UnboundedReceiver<Pushed>, Arc<AtomicU16>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let answer = Arc::new(AtomicU16::new(201));
    let status = answer.clone();
    let app = axum::Router::new().route(
        "/push/{device}",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let tx = tx.clone();
                let status = status.clone();
                async move {
                    let _ = tx.send(Pushed {
                        headers,
                        body: body.to_vec(),
                    });
                    axum::http::StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, rx, answer)
}

/// The next push, within a generous bound (the daemon sends it from a task).
async fn next(rx: &mut mpsc::UnboundedReceiver<Pushed>) -> Pushed {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("a push within 30 s")
        .expect("the push service runs")
}

/// `req` from a browser of the owner's on this machine (its owner cookie).
fn as_owner(ts: &TestServer, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header("cookie", ts.owner_cookie())
}

/// `{endpoint, keys}` as `PushSubscription.toJSON()` writes it, for
/// `receiver` at `endpoint`.
fn subscription(endpoint: &str, receiver: &Receiver) -> Value {
    let (p256dh, auth) = receiver.keys();
    json!({"endpoint": endpoint, "expirationTime": null, "keys": {"p256dh": p256dh, "auth": auth}})
}

async fn subscribe(ts: &TestServer, body: &Value) -> reqwest::Response {
    as_owner(
        ts,
        ts.client
            .post(format!("{}/api/web-push/subscriptions", ts.base))
            .json(body),
    )
    .send()
    .await
    .unwrap()
}

/// How many devices the owner has subscribed.
async fn devices(ts: &TestServer) -> u64 {
    let res = as_owner(ts, ts.client.get(format!("{}/api/web-push", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json::<Value>().await.unwrap()["devices"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn the_owners_browser_gets_the_key_and_subscribes_a_device() {
    let ts = TestServer::spawn().await;
    let res = as_owner(&ts, ts.client.get(format!("{}/api/web-push", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let key = v["public_key"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 87, "65 bytes, base64url: {key}");
    assert_eq!(v["devices"], 0);
    // The key is made once, an owner-only file of the home.
    let again: Value = as_owner(&ts, ts.client.get(format!("{}/api/web-push", ts.base)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["public_key"], key.as_str());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(ts.home.push_vapid_key())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let (base, mut rx, _) = push_service().await;
    let device = Receiver::new();
    let endpoint = format!("{base}/push/phone");
    let res = subscribe(&ts, &subscription(&endpoint, &device)).await;
    assert_eq!(res.status(), 201);
    assert_eq!(res.json::<Value>().await.unwrap()["devices"], 1);
    // The device hears at once that it is on.
    let p = next(&mut rx).await;
    assert_eq!(p.headers["content-encoding"], "aes128gcm");
    let auth = p.headers["authorization"].to_str().unwrap();
    assert!(
        auth.starts_with("vapid t=") && auth.ends_with(&format!(", k={key}")),
        "{auth}"
    );
    let m: Message = serde_json::from_slice(&device.open(&p.body).unwrap()).unwrap();
    assert_eq!(m.title, "Clax notifications are on for this device");
    assert_eq!(m.url, "/inbox");

    // A subscription that is not one: 400.
    for bad in [
        json!({"endpoint": "http://push.example/x", "keys": {"p256dh": "BAAA", "auth": "AAAA"}}),
        subscription("ftp://push.example/x", &Receiver::new()),
        json!({"endpoint": endpoint}),
    ] {
        let res = subscribe(&ts, &bad).await;
        assert_eq!(res.status(), 400, "{bad}");
    }
    assert_eq!(devices(&ts).await, 1);

    // Forgotten on request.
    let res = as_owner(
        &ts,
        ts.client
            .delete(format!("{}/api/web-push/subscriptions", ts.base))
            .json(&json!({"endpoint": endpoint})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap(),
        json!({"removed": true, "devices": 0})
    );
}

#[tokio::test]
async fn no_one_but_the_owners_browser_reaches_the_subscription_routes() {
    let ts = TestServer::spawn_with(|s| {
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = true;
    })
    .await;
    let body = subscription("https://web.push.apple.com/abc", &Receiver::new());
    // (status of GET /api/web-push, status of POST, error code) for each caller.
    let from = |login: &'static str, origin: String| {
        move |r: reqwest::RequestBuilder| {
            r.header("host", TRUSTED)
                .header("tailscale-user-login", login)
                .header("tailscale-user-name", "Someone")
                .header("origin", origin.clone())
        }
    };
    let tailnet = |login: &'static str| from(login, format!("https://{TRUSTED}"));
    let cases: Vec<(&str, As<'_>, u16, &str)> = vec![
        ("anonymous", Box::new(|r| r), 403, "forbidden"),
        (
            "a tailnet viewer",
            Box::new(tailnet("alex@github")),
            403,
            "forbidden",
        ),
        (
            "the owner's tailnet login, a viewer here",
            Box::new(tailnet("bdelanghe@github")),
            403,
            "forbidden",
        ),
        (
            "the owner's token alone",
            Box::new(|r: reqwest::RequestBuilder| r.bearer_auth(ts.token.clone())),
            403,
            "browser_required",
        ),
        (
            "the owner's agent",
            Box::new(|r: reqwest::RequestBuilder| {
                r.bearer_auth(ts.token.clone())
                    .header("x-clax-channel", "agent")
            }),
            403,
            "not_for_agents",
        ),
    ];
    for (who, with, status, code) in &cases {
        let get = with(ts.client.get(format!("{}/api/web-push", ts.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), *status, "{who}: GET");
        let post = with(
            ts.client
                .post(format!("{}/api/web-push/subscriptions", ts.base))
                .json(&body),
        )
        .send()
        .await
        .unwrap();
        assert_eq!(post.status(), *status, "{who}: POST");
        let v: Value = post.json().await.unwrap();
        assert_eq!(v["error"]["code"], *code, "{who}: {v}");
    }
    // A LAN viewer.
    let (lan, base) = ts.lan();
    let res = lan
        .post(format!("{base}/api/web-push/subscriptions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert!(
        ts.home.push_subscriptions().metadata().is_err(),
        "nothing kept"
    );
    // The owner's browser on this machine is, from the shell's origin only.
    let res = subscribe(&ts, &body).await;
    assert_eq!(res.status(), 201);
    let res = as_owner(
        &ts,
        ts.client
            .post(format!("{}/api/web-push/subscriptions", ts.base))
            .json(&body),
    )
    .header("origin", "https://evil.example")
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "forbidden_origin"
    );
}

#[tokio::test]
async fn an_agents_question_is_pushed_to_the_owners_devices() {
    let ts = TestServer::spawn().await;
    let (base, mut rx, answer) = push_service().await;
    let device = Receiver::new();
    let res = subscribe(&ts, &subscription(&format!("{base}/push/phone"), &device)).await;
    assert_eq!(res.status(), 201);
    let confirmation = next(&mut rx).await;
    assert!(!confirmation.body.is_empty());

    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let q = ts
        .ask(
            &sid,
            json!({"source": "ask", "questions": [{"question": "Ship the secret plan?", "header": "Pick",
                "options": [{"label": "A"}, {"label": "B"}]}]}),
        )
        .await;
    let qid = q["question"]["id"].as_str().unwrap();
    let p = next(&mut rx).await;
    assert_eq!(p.headers["content-encoding"], "aes128gcm");
    assert_eq!(p.headers["ttl"], "86400");
    let plain = device.open(&p.body).unwrap();
    let m: Message = serde_json::from_slice(&plain).unwrap();
    assert_eq!(m.title, "claude asked you a question");
    assert_eq!(m.url, format!("/inbox?q={qid}"));
    assert_eq!(m.tag, format!("clax-inbox-{qid}"));
    // A one-line title and a link: none of the question's text.
    assert!(!String::from_utf8_lossy(&plain).contains("secret"));

    // A push service that says the device is gone: it is forgotten.
    answer.store(410, Ordering::SeqCst);
    ts.ask(
        &sid,
        json!({"source": "ask", "questions": [{"question": "Again?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]}),
    )
    .await;
    next(&mut rx).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while devices(&ts).await != 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the gone device is forgotten"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn nothing_is_pushed_without_a_subscribed_device_or_for_what_does_not_wait() {
    let ts = TestServer::spawn().await;
    // No device: a question makes no push file and no key.
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.ask(
        &sid,
        json!({"source": "ask", "questions": [{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]}),
    )
    .await;
    assert!(ts.home.push_vapid_key().metadata().is_err());

    let (base, mut rx, _) = push_service().await;
    let device = Receiver::new();
    subscribe(&ts, &subscription(&format!("{base}/push/phone"), &device)).await;
    next(&mut rx).await;
    // A new version of an agent's page is an inbox item that waits on no one.
    let a = ts.publish_as(&sid, "Doc", "<p>1</p>").await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base))
                .header("x-clax-session", &sid)
                .json(&json!({"if_version": 1, "note": "Two",
                    "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}})),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    // Then a question: the next push is the question's, not the version's.
    let q = ts
        .ask(
            &sid,
            json!({"source": "ask", "questions": [{"question": "Now?", "header": "Pick",
                "options": [{"label": "A"}, {"label": "B"}]}]}),
        )
        .await;
    let p = next(&mut rx).await;
    let m: Message = serde_json::from_slice(&device.open(&p.body).unwrap()).unwrap();
    assert_eq!(
        m.url,
        format!("/inbox?q={}", q["question"]["id"].as_str().unwrap())
    );
}

#[tokio::test]
async fn an_agents_reply_on_the_owners_thread_is_pushed_with_its_threads_url() {
    let ts = TestServer::spawn().await;
    let (base, mut rx, _) = push_service().await;
    let device = Receiver::new();
    subscribe(&ts, &subscription(&format!("{base}/push/phone"), &device)).await;
    next(&mut rx).await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts.publish_as(&sid, "Board", "<h1>Board</h1>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread_as_owner(&aid, "make it blue").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.reply_as_agent(&sid, &aid, &tid, "Done: blue").await;
    // The page's first version is no one's to act on; the reply is the owner's.
    let p = next(&mut rx).await;
    let plain = device.open(&p.body).unwrap();
    let m: Message = serde_json::from_slice(&plain).unwrap();
    assert_eq!(m.title, "claude replied to you");
    assert_eq!(m.url, format!("/a/{aid}?thread={tid}"));
    assert_eq!(m.tag, format!("clax-inbox-{aid}"));
    assert!(
        !String::from_utf8_lossy(&plain).contains("blue"),
        "no reply text"
    );
}

/// A daemon trusting [`TRUSTED`] that names tailnet viewers when
/// `identity`, whose `[serve] owner_tailscale_login` is `owner`.
async fn tailnet_owned_by(identity: bool, owner: Option<&str>) -> TestServer {
    let owner = owner.map(str::to_string);
    TestServer::spawn_with(move |s| {
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = identity;
        s.owner_tailscale_login = owner.map(Arc::from);
    })
    .await
}

/// `req` as `tailscale serve` passes on a request of tailnet user `login`
/// through `host`, which the browser says `site` made, from `origin`.
fn through_serve(
    req: reqwest::RequestBuilder,
    host: &str,
    login: &str,
    site: &str,
    origin: Option<&str>,
) -> reqwest::RequestBuilder {
    let req = req
        .header("host", host)
        .header("tailscale-user-login", login)
        .header("tailscale-user-name", "Robert DeLanghe")
        .header("sec-fetch-site", site);
    match origin {
        Some(o) => req.header("origin", o),
        None => req,
    }
}

/// The statuses and error codes of GET /api/web-push, POST and DELETE
/// /api/web-push/subscriptions (`body`'s endpoint) made with `with`.
async fn web_push_calls(
    ts: &TestServer,
    body: &Value,
    with: &dyn Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Vec<(&'static str, u16, Value)> {
    let calls = [
        ("GET", ts.client.get(format!("{}/api/web-push", ts.base))),
        (
            "POST",
            ts.client
                .post(format!("{}/api/web-push/subscriptions", ts.base))
                .json(body),
        ),
        (
            "DELETE",
            ts.client
                .delete(format!("{}/api/web-push/subscriptions", ts.base))
                .json(&json!({"endpoint": body["endpoint"]})),
        ),
    ];
    let mut out = Vec::new();
    for (method, req) in calls {
        let res = with(req).send().await.unwrap();
        let status = res.status().as_u16();
        out.push((method, status, res.json().await.unwrap_or(Value::Null)));
    }
    out
}

#[tokio::test]
async fn the_owners_tailnet_login_through_serve_subscribes_a_device() {
    let ts = tailnet_owned_by(true, Some("bdelanghe@github")).await;
    let (base, mut rx, _) = push_service().await;
    let device = Receiver::new();
    let endpoint = format!("{base}/push/phone");
    let body = subscription(&endpoint, &device);
    let origin = format!("https://{TRUSTED}");
    let owner = |r| {
        through_serve(
            r,
            TRUSTED,
            "bdelanghe@github",
            "same-origin",
            Some(origin.as_str()),
        )
    };

    // The key, from the shell's own origin through the trusted host.
    let res = owner(ts.client.get(format!("{}/api/web-push", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["public_key"].as_str().unwrap().len(), 87, "{v}");
    assert_eq!(v["devices"], 0);
    // The same with the port, and a GET without Origin (as fetch sends one).
    let res = through_serve(
        ts.client.get(format!("{}/api/web-push", ts.base)),
        &format!("{TRUSTED}:443"),
        "bdelanghe@github",
        "same-origin",
        None,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Subscribing: 201, and the device hears at once.
    let res = owner(
        ts.client
            .post(format!("{}/api/web-push/subscriptions", ts.base))
            .json(&body),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.json::<Value>().await.unwrap()["devices"], 1);
    let p = next(&mut rx).await;
    let m: Message = serde_json::from_slice(&device.open(&p.body).unwrap()).unwrap();
    assert_eq!(m.title, "Clax notifications are on for this device");

    // Anyone else is refused all three routes, and the device stays.
    let port = ts.addr.port();
    let localhost = format!("localhost:{port}");
    let local_origin = format!("http://localhost:{port}");
    type With<'a> = Box<dyn Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + 'a>;
    let refused: Vec<(&str, With<'_>, &str)> = vec![
        (
            "another tailnet login",
            Box::new(|r| through_serve(r, TRUSTED, "eve@github", "same-origin", Some(&origin))),
            "forbidden",
        ),
        (
            "the owner's login in another case",
            Box::new(|r| {
                through_serve(r, TRUSTED, "BDELANGHE@github", "same-origin", Some(&origin))
            }),
            "forbidden",
        ),
        (
            "a host the daemon does not trust",
            Box::new(|r| {
                through_serve(
                    r,
                    "evil.example",
                    "bdelanghe@github",
                    "same-origin",
                    Some("https://evil.example"),
                )
            }),
            "forbidden_host",
        ),
        (
            "the owner's login sent to localhost by a local process",
            Box::new(|r| {
                through_serve(
                    r,
                    &localhost,
                    "bdelanghe@github",
                    "same-origin",
                    Some(&local_origin),
                )
            }),
            "forbidden",
        ),
        (
            "a cross-site request of the owner's device",
            Box::new(|r| through_serve(r, TRUSTED, "bdelanghe@github", "cross-site", None)),
            "forbidden_origin",
        ),
        (
            "a same-site request of the owner's device",
            Box::new(|r| through_serve(r, TRUSTED, "bdelanghe@github", "same-site", None)),
            "forbidden_origin",
        ),
        (
            "a cross-site page's request with its own Origin",
            Box::new(|r| {
                through_serve(
                    r,
                    TRUSTED,
                    "bdelanghe@github",
                    "cross-site",
                    Some("https://evil.example"),
                )
            }),
            "forbidden_origin",
        ),
    ];
    for (who, with, code) in &refused {
        for (method, status, v) in web_push_calls(&ts, &body, with.as_ref()).await {
            assert_eq!(status, 403, "{who}: {method}: {v}");
            assert_eq!(v["error"]["code"], *code, "{who}: {method}: {v}");
        }
    }
    let res = owner(ts.client.get(format!("{}/api/web-push", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["devices"], 1);

    // Forgotten on the owner's request.
    let res = owner(
        ts.client
            .delete(format!("{}/api/web-push/subscriptions", ts.base))
            .json(&json!({"endpoint": endpoint})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap(),
        json!({"removed": true, "devices": 0})
    );
}

#[tokio::test]
async fn without_the_setting_the_owners_tailnet_login_reaches_no_push_route() {
    let body = subscription("https://web.push.apple.com/abc", &Receiver::new());
    let origin = format!("https://{TRUSTED}");
    for (identity, owner) in [
        (true, None),
        (true, Some("someone-else@github")),
        (false, Some("bdelanghe@github")),
    ] {
        let ts = tailnet_owned_by(identity, owner).await;
        let with = |r| through_serve(r, TRUSTED, "bdelanghe@github", "same-origin", Some(&origin));
        for (method, status, v) in web_push_calls(&ts, &body, &with).await {
            assert_eq!(status, 403, "{identity} {owner:?}: {method}: {v}");
            assert_eq!(
                v["error"]["code"], "forbidden",
                "{identity} {owner:?}: {method}: {v}"
            );
        }
        assert!(
            ts.home.push_subscriptions().metadata().is_err(),
            "{identity} {owner:?}: nothing kept"
        );
        assert!(
            ts.home.push_vapid_key().metadata().is_err(),
            "{identity} {owner:?}: no key made"
        );
    }
}
