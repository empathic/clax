//! `/a/…` with the bootstrap block and the server-rendered frame. Its own test
//! binary: the web UI override is process-wide.
#![cfg(debug_assertions)]

mod common;
use clax_server::boot::FRAME_SANDBOX;
use clax_server::routes::shell::set_web_dist;
use common::TestServer;
use serde_json::{Value, json};

const ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/shell/artifact.html"
));
const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;

fn dist() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("clax-shell-boot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("artifact.html"), ENTRY).unwrap();
        std::fs::write(dir.join("index.html"), "<p>gallery</p>").unwrap();
        set_web_dist(dir);
    });
}

/// The bootstrap block's text and its parsed value.
fn boot(html: &str) -> (String, Value) {
    let start = html.find(OPEN).expect("a bootstrap block") + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    let text = html[start..end].to_string();
    let v = serde_json::from_str(&text).unwrap();
    (text, v)
}

async fn page(ts: &TestServer, path: &str, headers: &[(&str, String)]) -> reqwest::Response {
    let mut req = ts.client.get(format!("{}{path}", ts.base));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    req.send().await.unwrap()
}

async fn publish_version(ts: &TestServer, id: &str, html: &str) {
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/versions", ts.base)),
        )
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": html, "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());
}

/// The artifact API's answer less the session IDs the bootstrap leaves out.
fn without_sessions(mut api: Value) -> Value {
    api["artifact"]
        .as_object_mut()
        .unwrap()
        .remove("owner_session_id");
    for v in api["versions"].as_array_mut().unwrap() {
        v.as_object_mut().unwrap().remove("session_id");
    }
    api
}

#[tokio::test]
async fn embeds_what_the_api_answers_and_never_the_token() {
    dist();
    let ts = TestServer::spawn().await;
    let created = ts.publish("Report", &[("index.html", "<p>x</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    ts.thread(&id, 1, "first").await;
    let res = page(&ts, &format!("/a/{id}"), &[]).await;
    assert!(
        res.headers().get("set-cookie").is_none(),
        "no viewer is created"
    );
    let html = res.text().await.unwrap();
    let (_, b) = boot(&html);
    let api: Value = ts
        .get(&format!("/api/artifacts/{id}"))
        .await
        .json()
        .await
        .unwrap();
    let threads: Value = ts
        .get(&format!(
            "/api/artifacts/{id}/threads?include_resolved=true&limit=200"
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(b["v"], 1);
    assert_eq!(b["artifact"], without_sessions(api));
    assert_eq!(b["threads"], threads["threads"]);
    assert_eq!(b["viewer"], Value::Null);
    assert_eq!(b["frame"], Value::Null);
    assert!(!html.contains(&ts.token));
    assert!(html.contains("<h1>Report</h1>"));
    assert!(!html.contains("<!--clax:boot-->") && !html.contains("<!--clax:frame-->"));
}

#[tokio::test]
async fn names_no_session_and_no_clip_path() {
    dist();
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "boot-session").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let id = ts.publish_as(&sid, "Owned", "<p>x</p>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let api: Value = ts
        .get(&format!("/api/artifacts/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        api["artifact"]["owner_session_id"], sid,
        "the API names the owner session"
    );
    let res = ts
        .create_thread(&id, 1, "with a clip", Some(clax_server::testing::FAKE_PNG))
        .await;
    assert_eq!(res.status(), 201);
    // The token holder is answered with the clip's path on disk; the page is not.
    let authed: Value = ts
        .get_authed(&format!("/api/artifacts/{id}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert!(authed["threads"][0]["clip_path"].is_string());
    let html = page(
        &ts,
        &format!("/a/{id}"),
        &[("authorization", format!("Bearer {}", ts.token))],
    )
    .await
    .text()
    .await
    .unwrap();
    let (text, b) = boot(&html);
    assert!(!text.contains(&sid), "{text}");
    assert!(!text.contains("session_id"), "{text}");
    assert_eq!(b["threads"][0]["clip_path"], Value::Null);
    assert!(b["threads"][0]["clip_url"].is_string());
    assert!(!html.contains(&ts.token));
}

#[tokio::test]
async fn names_the_cookies_viewer_only() {
    dist();
    let ts = TestServer::spawn().await;
    let id = ts.publish("V", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = ts.viewer(Some("Ada")).await;
    let cookie = format!("clax_viewer={}", v.cookie);
    let html = page(&ts, &format!("/a/{id}"), &[("cookie", cookie.clone())])
        .await
        .text()
        .await
        .unwrap();
    let me: Value = page(&ts, "/api/viewers/me", &[("cookie", cookie)])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(boot(&html).1["viewer"], me["viewer"]);
    assert!(
        !html.contains(&v.cookie),
        "the viewer cookie never enters the page"
    );
    let unknown = page(
        &ts,
        &format!("/a/{id}"),
        &[("cookie", "clax_viewer=01J00000000000000000000000".into())],
    )
    .await;
    assert!(unknown.headers().get("set-cookie").is_none());
    assert_eq!(
        boot(&unknown.text().await.unwrap()).1["viewer"],
        Value::Null
    );
}

#[tokio::test]
async fn hostile_titles_and_comments_stay_data() {
    dist();
    let ts = TestServer::spawn().await;
    let title = "</script><b>x</b><!--clax:frame--><img src=x onerror=\"alert(1)\">";
    let id = ts.publish(title, &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let body = "</script><!--<script>\u{2028}\u{2029}&</SCRIPT ><img src=x onerror=alert(1)>";
    ts.thread(&id, 1, body).await;
    let html = page(
        &ts,
        &format!("/a/{id}"),
        &[("cookie", "clax_frame=sandbox".into())],
    )
    .await
    .text()
    .await
    .unwrap();
    let (text, b) = boot(&html);
    for bad in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
        assert!(!text.contains(bad), "{bad:?} in {text}");
    }
    assert_eq!(b["threads"][0]["comments"][0]["body"], body);
    assert_eq!(b["artifact"]["artifact"]["title"], title);
    assert!(html.contains(
        "<h1>&lt;/script&gt;&lt;b&gt;x&lt;/b&gt;&lt;!--clax:frame--&gt;&lt;img src=x onerror=&quot;alert(1)&quot;&gt;</h1>"
    ));
    // Exactly the entry's own scripts plus the one block, and one frame.
    assert_eq!(
        html.matches("<script").count(),
        ENTRY.matches("<script").count() + 1
    );
    assert_eq!(html.matches("<iframe").count(), 1);
    assert!(!html.contains("<img"));
}

#[tokio::test]
async fn renders_the_frame_only_when_the_mode_is_known() {
    dist();
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "F",
            &[("index.html", "<p>x</p>"), ("docs/a b.html", "<p>y</p>")],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    publish_version(&ts, &id, "<p>v2</p>").await;
    let port = ts.addr.port();
    let get = |path: String, headers: Vec<(&'static str, String)>| {
        let ts = &ts;
        async move { page(ts, &path, &headers).await.text().await.unwrap() }
    };
    assert!(!get(format!("/a/{id}"), vec![]).await.contains("<iframe"));
    for odd in [
        "clax_frame=",
        "clax_frame=Subdomain",
        "clax_frame=subdomain1",
        "xclax_frame=subdomain",
    ] {
        let html = get(format!("/a/{id}"), vec![("cookie", odd.into())]).await;
        assert!(!html.contains("<iframe"), "{odd}");
        assert_eq!(boot(&html).1["frame"], Value::Null, "{odd}");
    }
    let sandbox = get(
        format!("/a/{id}"),
        vec![("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert!(sandbox.contains(&format!(r#"<iframe class="frame" title="artifact content" src="/c/{id}/v/2/" allow="clipboard-write; fullscreen" sandbox="{FRAME_SANDBOX}"></iframe>"#)), "{sandbox}");
    assert_eq!(
        boot(&sandbox).1["frame"],
        json!({"mode": "sandbox", "src": format!("/c/{id}/v/2/")})
    );
    let sub = get(
        format!("/a/{id}"),
        vec![("cookie", "clax_frame=subdomain".into())],
    )
    .await;
    assert!(sub.contains(&format!(r#"<iframe class="frame" title="artifact content" src="http://{id}.localhost:{port}/v/2/" allow="clipboard-write; fullscreen"></iframe>"#)), "{sub}");
    assert_eq!(
        boot(&sub).1["frame"],
        json!({"mode": "subdomain", "src": format!("http://{id}.localhost:{port}/v/2/")})
    );
    let lan = get(
        format!("/a/{id}"),
        vec![
            ("host", format!("192.168.1.5:{port}")),
            ("cookie", "clax_frame=subdomain".into()),
        ],
    )
    .await;
    assert!(
        lan.contains(&format!(r#"src="/c/{id}/v/2/""#))
            && lan.contains(&format!(r#"sandbox="{FRAME_SANDBOX}""#)),
        "{lan}"
    );
    let pinned = get(
        format!("/a/{id}/v/1/docs/a%20b.html"),
        vec![("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert!(
        pinned.contains(&format!(r#"src="/c/{id}/v/1/docs/a%20b.html""#)),
        "{pinned}"
    );
    let missing = get(
        format!("/a/{id}/nope.html"),
        vec![("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert!(!missing.contains("<iframe"));
    let no_version = get(
        format!("/a/{id}/v/9"),
        vec![("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert!(!no_version.contains("<iframe"));
    let res = page(
        &ts,
        &format!("/a/{id}"),
        &[("host", format!("{id}.localhost:{port}"))],
    )
    .await;
    assert_eq!(res.status(), 404, "never served on an artifact origin");
}

#[tokio::test]
async fn the_etag_covers_the_injected_bytes() {
    dist();
    let ts = TestServer::spawn().await;
    let id = ts.publish("E", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let first = page(&ts, &format!("/a/{id}"), &[]).await;
    assert_eq!(first.headers()["cache-control"], "no-cache");
    assert_eq!(first.headers()["vary"], "Cookie");
    let tag = first.headers()["etag"].to_str().unwrap().to_string();
    let again = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag.clone())]).await;
    assert_eq!(again.status(), 304);
    assert_eq!(again.headers()["vary"], "Cookie");
    ts.thread(&id, 1, "new").await;
    let after = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag.clone())]).await;
    assert_eq!(after.status(), 200);
    let after_tag = after.headers()["etag"].to_str().unwrap().to_string();
    assert_ne!(after_tag, tag);
    // Another viewer, or another frame mode, never gets a 304 for these bytes.
    let v = ts.viewer(None).await;
    let as_viewer = page(
        &ts,
        &format!("/a/{id}"),
        &[
            ("cookie", format!("clax_viewer={}", v.cookie)),
            ("if-none-match", after_tag.clone()),
        ],
    )
    .await;
    assert_eq!(as_viewer.status(), 200);
    assert_ne!(as_viewer.headers()["etag"].to_str().unwrap(), after_tag);
    let framed = page(
        &ts,
        &format!("/a/{id}"),
        &[
            ("cookie", "clax_frame=sandbox".into()),
            ("if-none-match", after_tag.clone()),
        ],
    )
    .await;
    assert_eq!(framed.status(), 200);
    assert_eq!(framed.headers()["vary"], "Cookie");
}

#[tokio::test]
async fn an_unknown_artifact_gets_the_bare_entry() {
    dist();
    let ts = TestServer::spawn().await;
    let res = page(
        &ts,
        "/a/7q3k9mzx2b4t",
        &[("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    assert!(!html.contains(OPEN) && !html.contains("<!--clax:boot-->"));
    assert!(!html.contains("<iframe") && !html.contains("<!--clax:frame-->"));
    assert!(html.contains("<h1>Clax</h1>"));
}
