//! The extension gateway (spec 2026-10-05-chrome-overlay-design L5, L6,
//! §9.2, §9.5, §10 item 6): the extension's origin is admitted only with a
//! live credential, only to the allowlisted routes, only for live pages, and
//! acts there as the owner identity.

mod common;
use clax_core::extension::extension_origin;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};
use std::net::{IpAddr, UdpSocket};

fn origin(ts: &TestServer) -> String {
    extension_origin(&ts.extension_id())
}

async fn credential(ts: &TestServer) -> String {
    let v: Value = ts
        .authed(
            ts.client
                .post(format!("{}/api/extension/credentials", ts.base)),
        )
        .json(&json!({"extension_id": ts.extension_id()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["credential"].as_str().unwrap().to_string()
}

fn ext_at(
    ts: &TestServer,
    base: &str,
    m: Method,
    path: &str,
    cred: &str,
) -> reqwest::RequestBuilder {
    ts.client
        .request(m, format!("{base}{path}"))
        .header("origin", origin(ts))
        .header("sec-fetch-site", "cross-site")
        .header("authorization", format!("Clax-Extension {cred}"))
}

fn ext(ts: &TestServer, m: Method, path: &str, cred: &str) -> reqwest::RequestBuilder {
    ext_at(ts, &ts.base, m, path, cred)
}

async fn live_thread(ts: &TestServer, cred: &str) -> (String, String) {
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Home")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "body", "file": "index.html"}).to_string(),
        )
        .text("body", "From the extension")
        .text("pending", "[]")
        .text("snapshot", "<!doctype html><p>x")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    let res = ext(ts, Method::POST, "/api/live/threads", cred)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(
        res.headers()["access-control-allow-origin"],
        origin(ts).as_str()
    );
    let v: Value = res.json().await.unwrap();
    (
        v["page"]["artifact_id"].as_str().unwrap().into(),
        v["thread"]["id"].as_str().unwrap().into(),
    )
}

/// A non-loopback IPv4 address of this machine (see `api_host.rs`).
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

async fn code(res: reqwest::Response) -> String {
    let v: Value = res.json().await.unwrap();
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn preflights_are_answered_for_the_extension_and_allowed_routes_only() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/live/threads", ts.base))
        .header("origin", origin(&ts))
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let h = res.headers();
    assert_eq!(h["access-control-allow-origin"], origin(&ts).as_str());
    assert!(
        h["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    assert!(h["vary"].to_str().unwrap().contains("Origin"));
    assert!(
        h.get("access-control-allow-credentials").is_none(),
        "the extension sends no cookies (credentials: omit)"
    );
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/artifacts", ts.base))
        .header("origin", origin(&ts))
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // Another origin's preflight gets no CORS grant.
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/live/threads", ts.base))
        .header("origin", format!("chrome-extension://{}", "b".repeat(32)))
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn the_extension_acts_as_the_owner_on_live_pages() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let owner = ts.owner_viewer().await;
    let me: Value = ext(&ts, Method::GET, "/api/viewers/me", &cred)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        me["viewer"]["public_id"],
        owner.public_id.as_str(),
        "the extension is the owner viewer"
    );
    let res = ext(&ts, Method::PUT, "/api/viewers/me", &cred)
        .json(&json!({"display_name": "Alex"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.headers().get("set-cookie").is_none(),
        "the gateway never hands the extension a cookie"
    );
    assert_eq!(
        ts.owner_viewer().await.display_name.as_deref(),
        Some("Alex"),
        "the name is the owner's"
    );
    let (aid, tid) = live_thread(&ts, &cred).await;
    let t: Value = ext(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/threads/{tid}"),
        &cred,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(t["thread"]["comments"][0]["author_name"], "Alex");
    assert_eq!(
        t["thread"]["comments"][0]["author_public_id"],
        owner.public_id.as_str()
    );
    let looked = ext(&ts, Method::PUT, "/api/viewers/me/looked", &cred)
        .json(&json!({"artifact_id": aid, "thread_ids": [tid]}))
        .send()
        .await
        .unwrap();
    assert!(
        looked.status().is_success(),
        "looked-at marks are the owner's: {}",
        looked.status()
    );
    let here = ext(&ts, Method::PUT, "/api/viewers/me/presence", &cred)
        .json(&json!({"artifact_id": aid, "state": "here", "tab": "clax-ext:7"}))
        .send()
        .await
        .unwrap();
    assert!(here.status().is_success(), "presence: {}", here.status());
    for (m, path, body) in [
        (
            Method::POST,
            format!("/api/artifacts/{aid}/threads/{tid}/comments"),
            json!({"body": "more"}),
        ),
        (
            Method::POST,
            format!("/api/artifacts/{aid}/threads/{tid}/send"),
            json!({}),
        ),
        (
            Method::POST,
            format!("/api/artifacts/{aid}/threads/{tid}/resolve"),
            json!({}),
        ),
        (
            Method::POST,
            format!("/api/artifacts/{aid}/threads/{tid}/reopen"),
            json!({}),
        ),
        (
            Method::GET,
            format!("/api/artifacts/{aid}/threads/{tid}/clip"),
            Value::Null,
        ),
        (
            Method::GET,
            format!("/api/artifacts/{aid}/threads"),
            Value::Null,
        ),
        (
            Method::GET,
            format!("/api/artifacts/{aid}/working"),
            Value::Null,
        ),
        (
            Method::GET,
            format!("/api/artifacts/{aid}/presence"),
            Value::Null,
        ),
        (Method::GET, format!("/api/artifacts/{aid}"), Value::Null),
        (
            Method::GET,
            "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F".to_string(),
            Value::Null,
        ),
        (
            Method::DELETE,
            format!("/api/artifacts/{aid}/threads/{tid}"),
            Value::Null,
        ),
    ] {
        let mut r = ext(&ts, m.clone(), &path, &cred);
        if !body.is_null() {
            r = r.json(&body);
        }
        let res = r.send().await.unwrap();
        let st = res.status();
        assert!(st.is_success(), "{m} {path}: {st}");
        assert_eq!(
            res.headers()["access-control-allow-origin"],
            origin(&ts).as_str(),
            "{m} {path}"
        );
        assert!(res.headers().get("set-cookie").is_none(), "{m} {path}");
    }
}

#[tokio::test]
async fn everything_else_is_refused() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap();
    let (aid, tid) = live_thread(&ts, &cred).await;
    for (m, path, want) in [
        (Method::GET, "/api/artifacts".to_string(), 403),
        (Method::POST, "/api/artifacts".to_string(), 403),
        (Method::POST, format!("/api/artifacts/{aid}/versions"), 403),
        (Method::DELETE, format!("/api/artifacts/{aid}"), 403),
        (Method::GET, "/api/sessions".to_string(), 403),
        (Method::GET, "/api/token".to_string(), 403),
        (Method::GET, "/api/threads".to_string(), 403),
        (Method::GET, "/api/extension".to_string(), 403),
        (Method::POST, "/api/extension/credentials".to_string(), 403),
        (Method::GET, "/api/events".to_string(), 403),
        (Method::GET, "/api/viewers/me/attention".to_string(), 403),
        (Method::GET, format!("/api/artifacts/{aid}/docs"), 403),
        (Method::GET, format!("/c/{aid}/v/1/"), 403),
        (Method::GET, format!("/a/{aid}"), 403),
        (Method::GET, "/mcp".to_string(), 403),
        (Method::GET, "/api/live/pages/".to_string(), 403),
        (Method::GET, format!("/api/artifacts/{hid}"), 404),
        (Method::GET, format!("/api/artifacts/{hid}/threads"), 404),
        (
            Method::GET,
            format!("/api/artifacts/{hid}/threads/{tid}"),
            404,
        ),
        (Method::GET, "/api/artifacts/zzzzzzzzzzzz".to_string(), 404),
    ] {
        let res = ext(&ts, m.clone(), &path, &cred).send().await.unwrap();
        assert_eq!(res.status().as_u16(), want, "{m} {path}");
        assert_eq!(
            res.headers()["access-control-allow-origin"],
            origin(&ts).as_str(),
            "{m} {path}: a refusal is readable by the extension"
        );
    }
    // An artifact ID in a body names a live page too.
    for (path, body) in [
        (
            "/api/viewers/me/looked",
            json!({"artifact_id": hid, "thread_ids": []}),
        ),
        (
            "/api/viewers/me/presence",
            json!({"artifact_id": hid, "state": "here"}),
        ),
    ] {
        let st = ext(&ts, Method::PUT, path, &cred)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(st, 404, "{path}");
    }
    // A percent-encoded ID is the same artifact to the router and the gateway.
    let enc = format!("%{:02X}{}", hid.as_bytes()[0], &hid[1..]);
    let st = ext(&ts, Method::GET, &format!("/api/artifacts/{enc}"), &cred)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    let enc = format!("%{:02X}{}", aid.as_bytes()[0], &aid[1..]);
    let st = ext(&ts, Method::GET, &format!("/api/artifacts/{enc}"), &cred)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
}

#[tokio::test]
async fn credentials_and_origins_are_checked() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let path = "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F";
    let url = format!("{}{path}", ts.base);
    assert_eq!(
        ext(&ts, Method::GET, path, &cred)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let unknown = ext(
        &ts,
        Method::GET,
        path,
        "cxe_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(unknown.status(), 401);
    assert_eq!(
        unknown.headers()["access-control-allow-origin"],
        origin(&ts).as_str()
    );
    let missing = ts
        .client
        .get(&url)
        .header("origin", origin(&ts))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 401, "no credential");
    let malformed = ext(&ts, Method::GET, path, "not-a-credential")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 401);
    let bearer = ts
        .client
        .get(&url)
        .header("origin", origin(&ts))
        .header("authorization", format!("Bearer {}", ts.token))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bearer.status(),
        403,
        "the token is never accepted from the extension's origin"
    );
    for (o, why) in [
        (
            Some(format!("chrome-extension://{}", "b".repeat(32))),
            "another extension is a foreign origin",
        ),
        (Some("null".to_string()), "an opaque origin"),
        (Some("http://localhost:5173".to_string()), "a web page"),
        (None, "no origin at all"),
    ] {
        let mut r = ts
            .client
            .get(&url)
            .header("authorization", format!("Clax-Extension {cred}"));
        if let Some(o) = &o {
            r = r.header("origin", o);
        }
        let res = r.send().await.unwrap();
        assert_eq!(res.status(), 403, "{why}");
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "{why}"
        );
        assert_eq!(code(res).await, "forbidden_origin", "{why}");
    }
    // Another extension cannot use the viewer routes either.
    let other = ts
        .client
        .get(&url)
        .header("origin", format!("chrome-extension://{}", "b".repeat(32)))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 403);
    ts.authed(
        ts.client
            .delete(format!("{}/api/extension/credentials", ts.base)),
    )
    .send()
    .await
    .unwrap();
    let revoked = ext(&ts, Method::GET, path, &cred).send().await.unwrap();
    assert_eq!(revoked.status(), 401);
}

#[tokio::test]
async fn the_shell_keeps_its_own_origin_rules() {
    let ts = TestServer::spawn().await;
    let host = ts.base.trim_start_matches("http://").to_string();
    let path = "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F";
    let own = ts
        .client
        .get(format!("{}{path}", ts.base))
        .header("origin", format!("http://{host}"))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200);
    assert!(own.headers().get("access-control-allow-origin").is_none());
    let foreign = ts
        .client
        .get(format!("{}{path}", ts.base))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 403);
    assert_eq!(code(foreign).await, "forbidden_origin");
}

#[tokio::test]
async fn a_credential_from_another_machine_is_refused() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("no LAN address; skipped");
        return;
    };
    let cred = credential(&ts).await;
    let lan = format!("http://{ip}:{}", ts.addr.port());
    let path = "/api/viewers/me";
    let res = ext_at(&ts, &lan, Method::GET, path, &cred)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let pre = ts
        .client
        .request(Method::OPTIONS, format!("{lan}{path}"))
        .header("origin", origin(&ts))
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();
    assert_eq!(pre.status(), 403);
}

#[tokio::test]
async fn the_extensions_stream_is_live_only() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let (aid, _) = live_thread(&ts, &cred).await;
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap().to_string();
    let res = ext(&ts, Method::GET, "/api/stream", &cred)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()["access-control-allow-origin"],
        origin(&ts).as_str()
    );
    let mut events = clax_server::testing::EventReader::from_response(res);
    let ready = events.next_named("ready").await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let sub = |topic: String| {
        ext(&ts, Method::POST, &format!("/api/stream/{sid}"), &cred)
            .json(&json!({"subscribe": [topic]}))
    };
    for t in [
        format!("artifact:{aid}"),
        format!("working:{aid}"),
        format!("presence:{aid}"),
    ] {
        assert_eq!(sub(t.clone()).send().await.unwrap().status(), 200, "{t}");
    }
    for t in [
        "gallery".to_string(),
        format!("artifact:{hid}"),
        format!("working:{hid}"),
        format!("docs:{aid}"),
    ] {
        let res = sub(t.clone()).send().await.unwrap();
        assert_eq!(res.status(), 403, "{t}");
        assert_eq!(code(res).await, "forbidden", "{t}");
    }
    // The owner's shell, holding the token, keeps every topic.
    let res = ts
        .authed(ts.client.get(format!("{}/api/stream", ts.base)))
        .send()
        .await
        .unwrap();
    let mut shell = clax_server::testing::EventReader::from_response(res);
    let ready = shell.next_named("ready").await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let st = ts
        .authed(ts.client.post(format!("{}/api/stream/{sid}", ts.base)))
        .json(&json!({"subscribe": ["gallery", format!("artifact:{hid}")]}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
}

/// Opens the extension's stream subscribed to `artifact:<aid>`, posts a
/// comment as the owner's shell and reads its event: the stream delivers.
async fn delivering_stream(
    ts: &TestServer,
    cred: &str,
    aid: &str,
    tid: &str,
) -> clax_server::testing::EventReader {
    let res = ext(ts, Method::GET, "/api/stream", cred)
        .send()
        .await
        .unwrap();
    let mut events = clax_server::testing::EventReader::from_response(res);
    let sid = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let st = ext(ts, Method::POST, &format!("/api/stream/{sid}"), cred)
        .json(&json!({"subscribe": [format!("artifact:{aid}")]}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
    owner_comment(ts, aid, tid, "before").await;
    let (name, _) = events.next().await;
    assert_ne!(name, "ready", "the comment reaches the stream");
    events
}

async fn owner_comment(ts: &TestServer, aid: &str, tid: &str, body: &str) {
    let st = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"body": body}))
        .send()
        .await
        .unwrap()
        .status();
    assert!(st.is_success(), "{st}");
}

#[tokio::test]
async fn revoking_the_credential_ends_the_extensions_open_stream() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let (aid, tid) = live_thread(&ts, &cred).await;
    let mut events = delivering_stream(&ts, &cred, &aid, &tid).await;
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/extension/credentials", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    owner_comment(&ts, &aid, &tid, "after").await;
    assert_eq!(
        events.rest().await,
        Vec::<String>::new(),
        "the stream ends and delivers nothing more"
    );
}

#[tokio::test]
async fn an_expired_credential_ends_the_extensions_open_stream() {
    use clax_core::working::ManualClock;
    let clock = std::sync::Arc::new(ManualClock::at(&clax_core::Store::now()));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |st| {
        st.ext_creds = std::sync::Arc::new(
            clax_server::extension::Credentials::load_with(&st.store, c).unwrap(),
        );
    })
    .await;
    let cred = credential(&ts).await;
    let (aid, tid) = live_thread(&ts, &cred).await;
    let mut events = delivering_stream(&ts, &cred, &aid, &tid).await;
    clock.advance((clax_core::extension::CREDENTIAL_TTL_DAYS + 1) * 86_400);
    owner_comment(&ts, &aid, &tid, "after").await;
    assert_eq!(
        events.rest().await,
        Vec::<String>::new(),
        "the stream ends and delivers nothing more"
    );
}

#[tokio::test]
async fn a_credential_for_another_extension_id_is_refused() {
    let other = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let o = other.clone();
    let ts = TestServer::spawn_with(move |st| {
        let m = st.store.mint_extension_credential(&"p".repeat(32)).unwrap();
        *o.lock().unwrap() = m.credential;
        st.ext_creds =
            std::sync::Arc::new(clax_server::extension::Credentials::load(&st.store).unwrap());
    })
    .await;
    let cred = other.lock().unwrap().clone();
    let res = ext(&ts, Method::GET, "/api/viewers/me", &cred)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn the_extension_and_the_shell_change_only_their_own_streams() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let (aid, _) = live_thread(&ts, &cred).await;
    // The owner's shell without the token: the same caller as the extension.
    let res = ts
        .client
        .get(format!("{}/api/stream", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    let mut shell = clax_server::testing::EventReader::from_response(res);
    let shell_sid = shell.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ext(&ts, Method::GET, "/api/stream", &cred)
        .send()
        .await
        .unwrap();
    let mut mine = clax_server::testing::EventReader::from_response(res);
    let ext_sid = mine.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let topic = json!({"subscribe": [format!("artifact:{aid}")]});
    let res = ext(
        &ts,
        Method::POST,
        &format!("/api/stream/{shell_sid}"),
        &cred,
    )
    .json(&topic)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(code(res).await, "unknown_stream");
    let res = ts
        .client
        .post(format!("{}/api/stream/{ext_sid}", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&topic)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(code(res).await, "unknown_stream");
    // Each changes its own.
    for (sid, r) in [
        (
            &ext_sid,
            ext(&ts, Method::POST, &format!("/api/stream/{ext_sid}"), &cred),
        ),
        (
            &shell_sid,
            ts.client
                .post(format!("{}/api/stream/{shell_sid}", ts.base))
                .header("cookie", ts.owner_cookie()),
        ),
    ] {
        assert_eq!(r.json(&topic).send().await.unwrap().status(), 200, "{sid}");
    }
}
