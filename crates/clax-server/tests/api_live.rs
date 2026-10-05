//! Live pages over HTTP (spec 2026-10-05-chrome-overlay-design §7, §8, §9.2,
//! L10): lookup, a comment with its snapshot, the views, the publish
//! refusal, the snapshot policy, and hiding live pages from the LAN.
mod common;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use serde_json::{Value, json};
use std::net::{IpAddr, UdpSocket};

fn anchor() -> String {
    json!({"kind": "element", "selector": "main > button", "quote": "Save", "file": "index.html"})
        .to_string()
}

async fn post_thread_titled(
    ts: &TestServer,
    cookie: &str,
    url: &str,
    title: &str,
    snapshot: &str,
) -> reqwest::Response {
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", title.to_string())
        .text("anchor", anchor())
        .text("body", "The button overflows")
        .text("snapshot", snapshot.to_string())
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    ts.client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

async fn post_thread(
    ts: &TestServer,
    cookie: &str,
    url: &str,
    snapshot: &str,
) -> reqwest::Response {
    post_thread_titled(ts, cookie, url, "Settings", snapshot).await
}

/// A non-loopback IPv4 address of this machine (see `api_host.rs`).
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// The test daemon's base URL on a non-loopback interface address, when the
/// machine has one and the daemon listens on every interface.
fn lan_base(ts: &TestServer) -> Option<String> {
    let ip = non_loopback_ipv4()?;
    Some(format!("http://{ip}:{}", ts.addr.port()))
}

#[tokio::test]
async fn a_first_comment_creates_the_page_its_snapshot_and_the_thread_with_its_route() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let res = post_thread(
        &ts,
        &v.cookie,
        "http://localhost:5173/settings?tab=billing&utm_source=x",
        "<!doctype html><main><button>Save</button></main>",
    )
    .await;
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["page"]["page_url"], "http://localhost:5173/settings");
    assert_eq!(body["version"], 1);
    assert_eq!(body["thread"]["anchor"]["route"], "?tab=billing");
    assert_eq!(body["thread"]["has_clip"], true);
    assert_eq!(body["thread"]["comments"][0]["author_name"], "Alex");
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    assert_eq!(
        body["page"]["url"],
        format!("http://localhost:{}/a/{aid}", ts.addr.port())
    );
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["kind"], "live");
    assert_eq!(a["artifact"]["live"]["origin"], "http://localhost:5173");
    assert_eq!(a["artifact"]["live"]["path"], "/settings");
    assert_eq!(
        a["artifact"]["live"]["page_url"],
        "http://localhost:5173/settings"
    );
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    let listed = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == aid)
        .expect("the live page is listed on this machine");
    assert_eq!(listed["kind"], "live");
    assert_eq!(listed["live"]["path"], "/settings");
    let found: Value = ts
        .get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2Fsettings%23%2Fx")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(found["page"]["artifact_id"], aid);
    assert_eq!(found["page"]["current_version"], 1);
    assert_eq!(found["route"], "#/x");
    let none: Value = ts
        .get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2Fother")
        .await
        .json()
        .await
        .unwrap();
    assert!(none["page"].is_null(), "a lookup never creates");
    assert!(none["route"].is_null());
}

#[tokio::test]
async fn an_html_artifacts_view_says_its_kind_and_has_no_live_part() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["kind"], "html");
    assert!(a["artifact"].get("live").is_none_or(Value::is_null));
}

#[tokio::test]
async fn an_unchanged_snapshot_reuses_the_version() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let html = "<!doctype html><p>same</p>";
    let a: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", html)
        .await
        .json()
        .await
        .unwrap();
    let b: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", html)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["version"], 1);
    assert_eq!(b["version"], 1);
    let c: Value = post_thread(
        &ts,
        &v.cookie,
        "http://localhost:5173/",
        "<!doctype html><p>new</p>",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(c["version"], 2);
    assert_eq!(c["page"]["current_version"], 2);
}

#[tokio::test]
async fn a_page_title_is_stored_without_control_characters_collapsed_and_cut() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let long = format!("Settings\u{0}\u{7}  \n\t  {}", "x".repeat(300));
    let body: Value = post_thread_titled(&ts, &v.cookie, "http://localhost:5173/", &long, "<p>")
        .await
        .json()
        .await
        .unwrap();
    let title = body["page"]["title"].as_str().unwrap();
    assert!(title.starts_with("Settings x"), "{title:?}");
    assert!(!title.chars().any(char::is_control), "{title:?}");
    assert_eq!(title.chars().count(), 200);
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["title"], title);
    // A blank title falls back to the page's URL.
    let body: Value =
        post_thread_titled(&ts, &v.cookie, "http://localhost:5173/x", " \u{1b} ", "<p>")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(body["page"]["title"], "http://localhost:5173/x");
}

#[tokio::test]
async fn the_daemons_own_origin_and_other_schemes_are_refused() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let port = ts.addr.port();
    for own in [
        format!("http://localhost:{port}/a/7q3k9mzx2b4t"),
        format!("http://127.0.0.1:{port}/"),
        format!("http://[::1]:{port}/"),
        format!("http://7q3k9mzx2b4t.localhost:{port}/v/1/"),
    ] {
        let res = post_thread(&ts, &v.cookie, &own, "<p>").await;
        assert_eq!(res.status(), 400, "{own}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "own_origin",
            "{own}"
        );
        let res = ts
            .get(&format!("/api/live/pages?url={}", urlencode(&own)))
            .await;
        assert_eq!(res.status(), 400, "{own}");
    }
    // Another port on this machine is a page like any other.
    let other = format!("http://localhost:{}/", port.wrapping_add(1));
    assert_eq!(
        post_thread(&ts, &v.cookie, &other, "<p>").await.status(),
        201
    );
    let res = post_thread(&ts, &v.cookie, "file:///etc/passwd", "<p>").await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unsupported_url"
    );
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn a_comment_from_a_foreign_origin_is_refused() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("anchor", anchor())
        .text("body", "x")
        .text("snapshot", "<p>");
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .header("origin", "http://localhost:5173")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn a_live_page_cannot_be_published() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>x", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "live_page"
    );
}

#[tokio::test]
async fn snapshots_are_served_with_a_policy_that_runs_only_claxs_scripts() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(
        &ts,
        &v.cookie,
        "http://localhost:5173/",
        "<!doctype html><p>x</p>",
    )
    .await
    .json()
    .await
    .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let host = format!("localhost:{}", ts.addr.port());
    let policies = |res: &reqwest::Response| -> Vec<String> {
        res.headers()
            .get_all("content-security-policy")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    };
    for path in [format!("/c/{aid}/v/1/"), format!("/c/{aid}/v/1/index.html")] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{path}");
        let p = policies(&res);
        assert!(
            p.iter()
                .any(|p| p.contains(&format!("script-src http://{host}/_clax/"))
                    && p.contains("connect-src 'none'")),
            "{path}: {p:?}"
        );
        assert!(p.len() >= 2, "{path}: both policies apply: {p:?}");
    }
    // On the artifact's own host too.
    let ahost = format!("{aid}.localhost:{}", ts.addr.port());
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", &ahost)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        policies(&res)
            .iter()
            .any(|p| p.contains(&format!("script-src http://{ahost}/_clax/"))),
        "{:?}",
        policies(&res)
    );
    // An HTML artifact has no such policy.
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let res = ts
        .get(&format!(
            "/c/{}/v/1/",
            a["artifact"]["id"].as_str().unwrap()
        ))
        .await;
    assert!(!policies(&res).iter().any(|v| v.contains("script-src")));
}

#[tokio::test]
async fn live_pages_are_hidden_from_lan_callers_without_the_token() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let Some(lan) = lan_base(&ts) else {
        eprintln!("no LAN address; skipped");
        return;
    };
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let html_id = html["artifact"]["id"].as_str().unwrap();
    let list: Value = ts
        .client
        .get(format!("{lan}/api/artifacts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&Value> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["id"])
        .collect();
    assert!(!ids.contains(&&json!(aid)), "{ids:?}");
    assert!(ids.contains(&&json!(html_id)), "{ids:?}");
    for path in [
        format!("/api/artifacts/{aid}"),
        format!("/api/artifacts/{aid}/threads"),
        format!("/c/{aid}/v/1/"),
        format!("/a/{aid}"),
    ] {
        let st = ts
            .client
            .get(format!("{lan}{path}"))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(st, 404, "{path}");
    }
    // The live-page routes themselves are not there for the LAN.
    let st = ts
        .client
        .get(format!(
            "{lan}/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F"
        ))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("anchor", anchor())
        .text("body", "x")
        .text("snapshot", "<p>");
    let st = ts
        .client
        .post(format!("{lan}/api/live/threads"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    // An HTML artifact stays visible from the LAN.
    let st = ts
        .client
        .get(format!("{lan}/api/artifacts/{html_id}"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
    // Loopback, and the token from the LAN, still see it.
    assert_eq!(ts.get(&format!("/api/artifacts/{aid}")).await.status(), 200);
    let st = ts
        .authed(ts.client.get(format!("{lan}/api/artifacts/{aid}")))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
    let list: Value = ts
        .authed(ts.client.get(format!("{lan}/api/artifacts")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == aid)
    );
}

#[tokio::test]
async fn a_deleted_live_page_is_made_afresh_by_the_next_comment() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap().to_string();
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let found: Value = ts
        .get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F")
        .await
        .json()
        .await
        .unwrap();
    assert!(found["page"].is_null());
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    assert_ne!(body["page"]["artifact_id"], aid.as_str());
}

/// The next SSE event block of `stream` (not `ready`, not a comment) as
/// (name, data), within 20 s.
async fn next_event(
    stream: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    buf: &mut String,
) -> (String, Value) {
    use futures::StreamExt;
    loop {
        if let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf.drain(..end + 2);
            let name = block
                .lines()
                .find_map(|l| l.strip_prefix("event: "))
                .unwrap_or("message")
                .to_string();
            if block.starts_with(':') || name == "ready" {
                continue;
            }
            let data = block
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .unwrap_or("null");
            return (name, serde_json::from_str(data).unwrap());
        }
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(20), stream.next())
            .await
            .expect("an event within 20 s")
            .expect("stream open")
            .unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

#[tokio::test]
async fn the_event_stream_carries_no_live_page_event_to_the_lan() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let Some(lan) = lan_base(&ts) else {
        eprintln!("no LAN address; skipped");
        return;
    };
    let v = ts.viewer(Some("Alex")).await;
    let res = ts
        .client
        .get(format!("{lan}/api/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut far = Box::pin(res.bytes_stream());
    let res = ts
        .client
        .get(format!("{}/api/events", ts.base))
        .send()
        .await
        .unwrap();
    let mut near = Box::pin(res.bytes_stream());
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    // A marker after the live page's events.
    let marker = ts.publish("Marker", &[("index.html", "<p>")]).await;
    let marker_id = marker["artifact"]["id"].as_str().unwrap();
    let (mut far_buf, mut near_buf) = (String::new(), String::new());
    loop {
        let (name, data) = next_event(&mut far, &mut far_buf).await;
        assert_ne!(data["artifact_id"], aid, "{name}: {data}");
        if data["artifact_id"] == marker_id {
            break;
        }
    }
    let (_, data) = next_event(&mut near, &mut near_buf).await;
    assert_eq!(
        data["artifact_id"], aid,
        "this machine sees the live page's events"
    );
}
