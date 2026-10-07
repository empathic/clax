//! Live pages over HTTP (spec 2026-10-05-chrome-overlay-design §7, §8, §9.2,
//! L10): lookup, a comment with its snapshot, the views, the publish
//! refusal, the snapshot policy, and hiding live pages from the LAN (whose
//! requests arrive through [`TestServer::lan`], so they run on any machine).
use crate::common;
use clax_core::audit::AuditCtx;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use serde_json::{Value, json};

fn anchor() -> String {
    json!({"kind": "element", "selector": "main > button", "quote": "Save", "file": "index.html"})
        .to_string()
}

/// The fields of a comment on a live page, naming the threads in `pending`.
fn thread_form(
    url: &str,
    title: &str,
    snapshot: &str,
    pending: &[&str],
) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", title.to_string())
        .text("anchor", anchor())
        .text("body", "The button overflows")
        .text("pending", json!(pending).to_string())
        .text("snapshot", snapshot.to_string())
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        )
}

async fn post_thread_form(
    ts: &TestServer,
    cookie: &str,
    form: reqwest::multipart::Form,
) -> reqwest::Response {
    ts.client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

async fn post_thread_titled(
    ts: &TestServer,
    cookie: &str,
    url: &str,
    title: &str,
    snapshot: &str,
) -> reqwest::Response {
    post_thread_form(ts, cookie, thread_form(url, title, snapshot, &[])).await
}

async fn post_thread(
    ts: &TestServer,
    cookie: &str,
    url: &str,
    snapshot: &str,
) -> reqwest::Response {
    post_thread_titled(ts, cookie, url, "Settings", snapshot).await
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
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let html_id = html["artifact"]["id"].as_str().unwrap();
    let list: Value = lc
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
        let st = lc
            .get(format!("{lan}{path}"))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(st, 404, "{path}");
    }
    // The live-page routes themselves are not there for the LAN.
    let st = lc
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
    let st = lc
        .post(format!("{lan}/api/live/threads"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    // An HTML artifact stays visible from the LAN.
    let st = lc
        .get(format!("{lan}/api/artifacts/{html_id}"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
    // Loopback, and the token from the LAN, still see it.
    assert_eq!(ts.get(&format!("/api/artifacts/{aid}")).await.status(), 200);
    let st = ts
        .authed(lc.get(format!("{lan}/api/artifacts/{aid}")))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200);
    let list: Value = ts
        .authed(lc.get(format!("{lan}/api/artifacts")))
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

/// The next SSE event of `stream` other than keep-alive comments, as
/// (name, data), within 20 s; `ready` too when `with_ready`.
async fn next_block(
    stream: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    buf: &mut String,
    with_ready: bool,
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
            if block.starts_with(':') || (name == "ready" && !with_ready) {
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

/// The next SSE event of `stream` other than `ready` and comments.
async fn next_event(
    stream: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    buf: &mut String,
) -> (String, Value) {
    next_block(stream, buf, false).await
}

#[tokio::test]
async fn the_event_stream_carries_no_live_page_event_to_the_lan() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Alex")).await;
    let res = lc.get(format!("{lan}/api/events")).send().await.unwrap();
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

/// `aid` with its first character percent-encoded, as a path segment.
fn encoded(aid: &str) -> String {
    format!("%{:02X}{}", aid.as_bytes()[0], &aid[1..])
}

#[tokio::test]
async fn live_pages_stay_hidden_from_the_lan_whatever_form_the_request_takes() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let tid = body["thread"]["id"].as_str().unwrap();
    let enc = encoded(aid);
    let get = |path: String| {
        let req = lc.get(format!("{lan}{path}"));
        async move { req.send().await.unwrap().status() }
    };
    // Percent-encoded IDs, and the clip.
    for path in [
        format!("/api/artifacts/{enc}"),
        format!("/api/artifacts/{enc}/threads"),
        format!("/api/artifacts/{enc}/versions"),
        format!("/api/artifacts/{enc}/threads/{tid}/clip"),
        format!("/api/artifacts/{aid}/threads/{tid}/clip"),
        format!("/c/{enc}/v/1/"),
        format!("/c/{enc}/v/1/index.html"),
        format!("/a/{enc}"),
    ] {
        assert_eq!(get(path.clone()).await, 404, "{path}");
    }
    // The same forms answer on this machine, so the 404s above are the hiding.
    for path in [
        format!("/api/artifacts/{enc}"),
        format!("/api/artifacts/{aid}/threads/{tid}/clip"),
    ] {
        assert_eq!(ts.get(&path).await.status(), 200, "{path}");
    }
    // `?artifact=` of the list.
    let list: Value = lc
        .get(format!("{lan}/api/artifacts?artifact={aid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["artifacts"], json!([]));
    // The artifact host, sent to the LAN address.
    let st = lc
        .get(format!("{lan}/v/1/"))
        .header("host", format!("{aid}.localhost:{}", ts.addr.port()))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    // A stream opened from the LAN cannot subscribe to the page's topics.
    let res = lc.get(format!("{lan}/api/stream")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut stream = Box::pin(res.bytes_stream());
    let mut buf = String::new();
    let (name, ready) = next_block(&mut stream, &mut buf, true).await;
    assert_eq!(name, "ready");
    let sid = ready["stream"].as_str().unwrap();
    for topic in ["artifact", "working", "presence", "docs"] {
        let res = lc
            .post(format!("{lan}/api/stream/{sid}"))
            .json(&json!({"subscribe": [format!("{topic}:{aid}")]}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{topic}");
    }
}

#[tokio::test]
async fn a_lan_viewer_learns_nothing_of_live_pages_through_the_viewer_routes() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let tid = body["thread"]["id"].as_str().unwrap();
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let html_id = html["artifact"]["id"].as_str().unwrap();
    ts.thread_as(html_id, &v.cookie, "on the html page").await;
    let cookie = format!("clax_viewer={}", v.cookie);
    // On this machine, the viewer's attention names both.
    let near: Value = ts
        .client
        .get(format!("{}/api/viewers/me/attention", ts.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(near["artifacts"].get(aid).is_some(), "{near}");
    let far: Value = lc
        .get(format!("{lan}/api/viewers/me/attention"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(far["artifacts"].get(aid).is_none(), "{far}");
    assert!(far["artifacts"].get(html_id).is_some(), "{far}");
    let res = lc
        .get(format!("{lan}/api/viewers/me/attention?artifact={aid}"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let res = lc
        .get(format!("{lan}/api/viewers/me/seen?artifact={aid}"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    for (path, body) in [
        (
            "/api/viewers/me/seen",
            json!({"artifact_id": aid, "version": 1}),
        ),
        (
            "/api/viewers/me/looked",
            json!({"artifact_id": aid, "thread_ids": [tid]}),
        ),
        (
            "/api/viewers/me/presence",
            json!({"artifact_id": aid, "state": "here"}),
        ),
    ] {
        let res = lc
            .put(format!("{lan}{path}"))
            .header("cookie", &cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{path}");
    }
}

#[tokio::test]
async fn loopback_and_unspecified_addresses_on_the_daemons_port_are_its_own() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let port = ts.addr.port();
    for own in [
        format!("http://127.0.0.2:{port}/"),
        format!("http://127.1.2.3:{port}/x"),
        format!("http://0.0.0.0:{port}/"),
        format!("http://[0:0:0:0:0:0:0:1]:{port}/"),
        format!("http://[::]:{port}/"),
    ] {
        let res = post_thread(&ts, &v.cookie, &own, "<p>").await;
        assert_eq!(res.status(), 400, "{own}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "own_origin",
            "{own}"
        );
    }
    let other = format!("http://127.0.0.2:{}/", port.wrapping_add(1));
    assert_eq!(
        post_thread(&ts, &v.cookie, &other, "<p>").await.status(),
        201
    );
}

/// A live page with one thread, sent to a `claude` session watching it.
async fn sent_live_thread(ts: &TestServer) -> (String, String, String) {
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(
        ts,
        &v.cookie,
        "http://localhost:5173/",
        "<!doctype html><p>v1</p>",
    )
    .await
    .json()
    .await
    .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap().to_string();
    let tid = body["thread"]["id"].as_str().unwrap().to_string();
    let s = ts.register_session("claude", "live-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    ts.send_thread(&aid, &tid).await;
    (aid, tid, sid)
}

async fn agent_reply(
    ts: &TestServer,
    aid: &str,
    tid: &str,
    sid: &str,
    addressed: bool,
) -> reqwest::Response {
    ts.authed(ts.client.post(format!(
        "{}/api/artifacts/{aid}/threads/{tid}/comments",
        ts.base
    )))
    .header("x-clax-session", sid)
    .json(&json!({"body": "Fixed", "author_kind": "agent", "addressed": addressed}))
    .send()
    .await
    .unwrap()
}

async fn post_snapshot(
    ts: &TestServer,
    cookie: &str,
    html: &str,
    pending: &[&str],
) -> reqwest::Response {
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Home")
        .text("pending", json!(pending).to_string())
        .text("snapshot", html.to_string());
    post_snapshot_form(ts, cookie, form).await
}

async fn post_snapshot_form(
    ts: &TestServer,
    cookie: &str,
    form: reqwest::multipart::Form,
) -> reqwest::Response {
    ts.client
        .post(format!("{}/api/live/snapshots", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_addressed_reply_waits_for_the_next_snapshot_and_links_to_it() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    let v = ts.viewer(Some("Mia")).await;
    assert_eq!(
        post_snapshot(&ts, &v.cookie, "<p>x", &[&tid])
            .await
            .status(),
        409,
        "nothing pending yet"
    );
    let res = agent_reply(&ts, &aid, &tid, &sid, true).await;
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["addressed"], "pending");
    assert_eq!(body["thread"]["addressed_pending"]["harness"], "claude");
    let snap: Value = post_snapshot(&ts, &v.cookie, "<!doctype html><p>v1</p>", &[&tid])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        snap["version"], 2,
        "a snapshot after an address is a version even when identical"
    );
    assert_eq!(snap["linked"], json!([tid]));
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["addressed_in"], json!([2]));
    assert!(t["thread"]["addressed_pending"].is_null());
    assert_eq!(
        post_snapshot(&ts, &v.cookie, "<p>y", &[&tid])
            .await
            .status(),
        409,
        "the address was used up"
    );
}

#[tokio::test]
async fn a_comment_snapshot_also_links_pending_addresses() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    agent_reply(&ts, &aid, &tid, &sid, true).await;
    let v = ts.viewer(Some("Mia")).await;
    let form = thread_form(
        "http://localhost:5173/",
        "Settings",
        "<!doctype html><p>v2</p>",
        &[&tid],
    );
    let body: Value = post_thread_form(&ts, &v.cookie, form)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["version"], 2);
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["addressed_in"], json!([2]));
}

#[tokio::test]
async fn an_agent_resolve_on_a_live_page_is_pending_too() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"as": "agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        t["thread"]["addressed_in"],
        json!([]),
        "not linked to the current version"
    );
    assert_eq!(t["thread"]["addressed_pending"]["harness"], "claude");
}

#[tokio::test]
async fn addressed_is_refused_on_html_artifacts_and_for_viewers() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "h-1").await;
    let sid = s["id"].as_str().unwrap();
    let a = ts.publish_as(sid, "T", "<p>").await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let t = ts.thread(aid, 1, "x").await;
    let tid = t["id"].as_str().unwrap();
    ts.send_thread(aid, tid).await;
    let res = agent_reply(&ts, aid, tid, sid, true).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_args"
    );
    let v = ts.viewer(Some("Alex")).await;
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"body": "x", "addressed": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        got["thread"]["comments"].as_array().unwrap().len(),
        1,
        "nothing was written"
    );
    assert!(got["thread"]["addressed_pending"].is_null());
}

#[tokio::test]
async fn deleting_an_addressed_thread_drops_its_pending_address() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    assert_eq!(agent_reply(&ts, &aid, &tid, &sid, true).await.status(), 201);
    let res = ts
        .authed(ts.client.delete(format!(
            "{}/api/artifacts/{aid}/threads/{tid}?as=agent",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        ts.get(&format!("/api/artifacts/{aid}/threads/{tid}"))
            .await
            .status(),
        404
    );
    let v = ts.viewer(Some("Mia")).await;
    assert_eq!(
        post_snapshot(&ts, &v.cookie, "<p>x", &[&tid])
            .await
            .status(),
        409,
        "no address is left waiting"
    );
}

#[tokio::test]
async fn a_live_page_takes_no_asset_uploads() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
            .file_name("a.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/assets", ts.base)),
        )
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "live_page"
    );
    let list: Value = ts
        .get(&format!("/api/artifacts/{aid}/assets"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["assets"], json!([]));
}

#[tokio::test]
async fn a_live_pages_blobs_and_snapshots_route_are_hidden_from_the_lan() {
    let mut store = None;
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |s| {
        store = Some(s.store.clone());
    })
    .await;
    let store = store.unwrap();
    let (lc, lan) = ts.lan();
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>")
        .await
        .json()
        .await
        .unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    // The upload route refuses live pages; an asset written some other way
    // (an older daemon, a hand-edited store) stays hidden all the same.
    let asset = store
        .add_asset(
            &AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(aid).unwrap(),
            "image/png",
            FAKE_PNG,
        )
        .unwrap();
    let blob = format!("/_blob/{}", asset.id);
    let lan_get = |path: String| lc.get(format!("{lan}{path}")).send();
    assert_eq!(lan_get(blob.clone()).await.unwrap().status(), 404);
    assert_eq!(ts.get(&blob).await.status(), 200, "loopback sees it");
    let st = ts
        .authed(lc.get(format!("{lan}{blob}")))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 200, "the token from the LAN sees it");
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("pending", "[]")
        .text("snapshot", "<p>");
    let st = lc
        .post(format!("{lan}/api/live/snapshots"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 404);
    // An HTML artifact's blob stays visible from the LAN.
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap();
    let a = store
        .add_asset(
            &AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(hid).unwrap(),
            "image/png",
            FAKE_PNG,
        )
        .unwrap();
    assert_eq!(
        lan_get(format!("/_blob/{}", a.id)).await.unwrap().status(),
        200
    );
}

#[tokio::test]
async fn a_snapshot_links_only_the_pending_threads_it_names() {
    let ts = TestServer::spawn().await;
    let (aid, a, sid) = sent_live_thread(&ts).await;
    let v = ts.viewer(Some("Mia")).await;
    let body: Value = post_thread(
        &ts,
        &v.cookie,
        "http://localhost:5173/",
        "<!doctype html><p>v1</p>",
    )
    .await
    .json()
    .await
    .unwrap();
    let b = body["thread"]["id"].as_str().unwrap().to_string();
    let c = ts.thread(&aid, 1, "never addressed").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.send_thread(&aid, &b).await;
    assert_eq!(agent_reply(&ts, &aid, &a, &sid, true).await.status(), 201);
    assert_eq!(agent_reply(&ts, &aid, &b, &sid, true).await.status(), 201);
    // The caller saw only `a` (and `c`, not pending) when it serialized.
    let res = post_snapshot(&ts, &v.cookie, "<p>fixed a", &[&c, &a]).await;
    assert_eq!(res.status(), 200);
    let snap: Value = res.json().await.unwrap();
    assert_eq!(snap["version"], 2);
    assert_eq!(snap["linked"], json!([a]));
    let tb: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{b}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(tb["thread"]["addressed_in"], json!([]));
    assert_eq!(tb["thread"]["addressed_pending"]["harness"], "claude");
    // Naming none of the pending threads writes nothing.
    let res = post_snapshot(&ts, &v.cookie, "<p>x", &[&a, &c]).await;
    assert_eq!(res.status(), 409);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "nothing_pending"
    );
    let page: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        page["artifact"]["current_version"], 2,
        "no version for a 409"
    );
    // Two pending threads named together link oldest address first.
    let d = ts.thread(&aid, 1, "d").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.send_thread(&aid, &d).await;
    assert_eq!(agent_reply(&ts, &aid, &d, &sid, true).await.status(), 201);
    let snap: Value = post_snapshot(&ts, &v.cookie, "<p>fixed b, d", &[&d, &b])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(snap["version"], 3);
    assert_eq!(snap["linked"], json!([b, d]));
}

#[tokio::test]
async fn a_snapshot_refuses_fields_it_does_not_take() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    assert_eq!(agent_reply(&ts, &aid, &tid, &sid, true).await.status(), 201);
    let v = ts.viewer(Some("Mia")).await;
    let base = || {
        reqwest::multipart::Form::new()
            .text("url", "http://localhost:5173/")
            .text("pending", json!([tid]).to_string())
            .text("snapshot", "<p>")
    };
    for extra in ["anchor", "body", "clip", "bogus"] {
        let res = post_snapshot_form(&ts, &v.cookie, base().text(extra, "x")).await;
        assert_eq!(res.status(), 400, "{extra}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "invalid_args",
            "{extra}"
        );
    }
    let no_pending = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("snapshot", "<p>");
    let res = post_snapshot_form(&ts, &v.cookie, no_pending).await;
    assert_eq!(res.status(), 400, "pending is required");
    let bad = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("pending", "not json")
        .text("snapshot", "<p>");
    assert_eq!(post_snapshot_form(&ts, &v.cookie, bad).await.status(), 400);
    let twice = base().text("pending", "[]");
    assert_eq!(
        post_snapshot_form(&ts, &v.cookie, twice).await.status(),
        400,
        "a field given twice"
    );
    // Nothing above wrote a version or used the address.
    assert_eq!(
        post_snapshot(&ts, &v.cookie, "<p>", &[&tid]).await.status(),
        200
    );
}

#[tokio::test]
async fn a_comment_snapshot_links_only_the_pending_threads_it_names() {
    let ts = TestServer::spawn().await;
    let (aid, a, sid) = sent_live_thread(&ts).await;
    let b = ts.thread(&aid, 1, "b").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.send_thread(&aid, &b).await;
    assert_eq!(agent_reply(&ts, &aid, &a, &sid, true).await.status(), 201);
    assert_eq!(agent_reply(&ts, &aid, &b, &sid, true).await.status(), 201);
    let v = ts.viewer(Some("Mia")).await;
    // An identical snapshot makes no version, so links nothing.
    let form = thread_form(
        "http://localhost:5173/",
        "Settings",
        "<!doctype html><p>v1</p>",
        &[&a, &b],
    );
    let body: Value = post_thread_form(&ts, &v.cookie, form)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["version"], 1);
    let view = |tid: String| {
        let ts = &ts;
        let aid = aid.clone();
        async move {
            ts.get(&format!("/api/artifacts/{aid}/threads/{tid}"))
                .await
                .json::<Value>()
                .await
                .unwrap()["thread"]
                .clone()
        }
    };
    assert!(view(a.clone()).await["addressed_pending"].is_object());
    // A new version links the named thread only.
    let form = thread_form(
        "http://localhost:5173/",
        "Settings",
        "<!doctype html><p>fixed a</p>",
        &[&a],
    );
    let res = post_thread_form(&ts, &v.cookie, form).await;
    assert_eq!(res.status(), 201);
    assert_eq!(res.json::<Value>().await.unwrap()["version"], 2);
    let ta = view(a.clone()).await;
    assert_eq!(ta["addressed_in"], json!([2]));
    assert!(ta["addressed_pending"].is_null());
    let tb = view(b.clone()).await;
    assert_eq!(tb["addressed_in"], json!([]));
    assert_eq!(tb["addressed_pending"]["harness"], "claude");
}

#[tokio::test]
async fn a_comment_refuses_fields_it_does_not_take_and_needs_pending() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Mia")).await;
    let url = "http://localhost:5173/";
    let form = || thread_form(url, "Settings", "<p>", &[]);
    for extra in ["bogus", "version"] {
        let res = post_thread_form(&ts, &v.cookie, form().text(extra, "x")).await;
        assert_eq!(res.status(), 400, "{extra}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "invalid_args",
            "{extra}"
        );
    }
    let twice = form().text("pending", "[]");
    assert_eq!(post_thread_form(&ts, &v.cookie, twice).await.status(), 400);
    let no_pending = reqwest::multipart::Form::new()
        .text("url", url)
        .text("anchor", anchor())
        .text("body", "x")
        .text("snapshot", "<p>");
    let res = post_thread_form(&ts, &v.cookie, no_pending).await;
    assert_eq!(res.status(), 400, "pending is required");
    let bad = reqwest::multipart::Form::new()
        .text("url", url)
        .text("anchor", anchor())
        .text("body", "x")
        .text("pending", "{}")
        .text("snapshot", "<p>");
    assert_eq!(post_thread_form(&ts, &v.cookie, bad).await.status(), 400);
    let page: Value = ts
        .get(&format!("/api/live/pages?url={}", urlencode(url)))
        .await
        .json()
        .await
        .unwrap();
    assert!(page["page"].is_null(), "nothing was created");
}

/// A comment on `url` for pick `pick`, mentioning the agent.
fn picked_form(url: &str, pick: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "Settings")
        .text("anchor", anchor())
        .text("body", "@agent the button overflows")
        .text("pending", "[]")
        .text("pick_id", pick.to_string())
        .text("snapshot", "<!doctype html><p>v1</p>")
}

async fn thread_count(ts: &TestServer, aid: &str) -> usize {
    let list: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    list["threads"].as_array().unwrap().len()
}

#[tokio::test]
async fn a_retried_comment_with_its_pick_id_makes_and_sends_one_thread() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let pick = "0123456789abcdef0123456789abcdef";
    let first = post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/", pick)).await;
    assert_eq!(first.status(), 201);
    let first: Value = first.json().await.unwrap();
    let aid = first["page"]["artifact_id"].as_str().unwrap().to_string();
    let tid = first["thread"]["id"].as_str().unwrap().to_string();
    let sent_at = first["thread"]["sent_to_agent"].clone();

    let again = post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/", pick)).await;
    assert_eq!(again.status(), 200, "a repeat answers the thread it made");
    let again: Value = again.json().await.unwrap();
    assert_eq!(again["thread"]["id"], tid);
    assert_eq!(again["page"]["artifact_id"], aid);
    assert_eq!(again["version"], first["version"]);
    assert_eq!(again["thread"]["sent_to_agent"], sent_at);
    assert_eq!(again["thread"]["comments"].as_array().unwrap().len(), 1);
    assert_eq!(thread_count(&ts, &aid).await, 1, "no second thread");

    // The same pick on another page, and another pick on this one, are new.
    let other =
        post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/x", pick)).await;
    assert_eq!(other.status(), 201);
    let fresh = post_thread_form(
        &ts,
        &v.cookie,
        picked_form("http://localhost:5173/", &"f".repeat(32)),
    )
    .await;
    assert_eq!(fresh.status(), 201);
    assert_eq!(thread_count(&ts, &aid).await, 2);
}

#[tokio::test]
async fn a_repeated_pick_whose_thread_was_deleted_makes_a_new_one() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let pick = "00000000000000000000000000000001";
    let first: Value =
        post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/", pick))
            .await
            .json()
            .await
            .unwrap();
    let aid = first["page"]["artifact_id"].as_str().unwrap();
    let tid = first["thread"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let again = post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/", pick)).await;
    assert_eq!(again.status(), 201);
    let again: Value = again.json().await.unwrap();
    assert_ne!(again["thread"]["id"], tid);
}

#[tokio::test]
async fn a_pick_id_must_be_32_lowercase_hex_digits() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(None).await;
    for bad in ["short", &"A".repeat(32), &"g".repeat(32), &"a".repeat(33)] {
        let res =
            post_thread_form(&ts, &v.cookie, picked_form("http://localhost:5173/", bad)).await;
        assert_eq!(res.status(), 400, "{bad}");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_args");
    }
}
