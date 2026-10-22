//! `/a/…` with the bootstrap block and the server-rendered frame, from a test
//! daemon serving a scratch web UI.
#![cfg(debug_assertions)]

use clax_server::shell::boot::FRAME_SANDBOX;
use clax_testkit::TestServer;
use serde_json::{Value, json};

const ENTRY: &str = include_str!("../../../web/shell/artifact.html");
const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;

/// A scratch web UI: the artifact entry and a stand-in gallery.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.html"), ENTRY).unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>gallery</p>").unwrap();
    dir
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
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
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
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let s = ts.register_session("claude", "boot-session").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let id = ts.publish_as(&sid, "Owned", "<p>x</p>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let api: Value = ts
        .get_authed(&format!("/api/artifacts/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        api["artifact"]["owner_session_id"], sid,
        "the API names the owner session"
    );
    let res = ts
        .create_thread(&id, 1, "with a clip", Some(clax_testkit::FAKE_PNG))
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
    let agents = &b["artifact"]["artifact"]["participants"]["agents"];
    assert_eq!(agents[0]["harness"], "claude", "{text}");
    assert!(clax_core::is_agent_handle(
        agents[0]["handle"].as_str().unwrap()
    ));
    assert!(b.get("attention").is_none(), "no viewer, no attention");
    assert_eq!(b["threads"][0]["clip_path"], Value::Null);
    assert!(b["threads"][0]["clip_url"].is_string());
    assert!(!html.contains(&ts.token));
}

#[tokio::test]
async fn names_the_cookies_viewer_only() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
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
    let b = boot(&html).1;
    assert_eq!(b["viewer"], me["viewer"]);
    assert!(b["artifact"]["artifact"]["participants"].is_object());
    assert_eq!(
        b["artifact"]["artifact"]["may_tag"],
        json!(false),
        "a guest changes no artifact's tags"
    );
    assert_eq!(
        b["artifact"]["artifact"]["may_resolve"],
        json!(true),
        "on a local daemon everyone resolves"
    );
    assert_eq!(
        b["attention"]["open_in"],
        json!([]),
        "the cookie's viewer gets its attention"
    );
    assert!(b["attention"]["looked"].is_object());
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
    let b = boot(&unknown.text().await.unwrap()).1;
    assert_eq!(b["viewer"], Value::Null);
    assert!(
        b.get("attention").is_none(),
        "an unknown cookie gets no attention"
    );
}

#[tokio::test]
async fn hostile_titles_and_comments_stay_data() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
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
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
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
    // A sandboxed frame loads the artifact's site origin too.
    assert!(sandbox.contains(&format!(r#"<iframe class="frame" title="artifact content" src="http://{id}.localhost:{port}/_clax/v/2/" allow="clipboard-write; fullscreen" sandbox="{FRAME_SANDBOX}"></iframe>"#)), "{sandbox}");
    assert_eq!(
        boot(&sandbox).1["frame"],
        json!({"mode": "sandbox", "src": format!("http://{id}.localhost:{port}/_clax/v/2/")})
    );
    let sub = get(
        format!("/a/{id}"),
        vec![("cookie", "clax_frame=subdomain".into())],
    )
    .await;
    assert!(sub.contains(&format!(r#"<iframe class="frame" title="artifact content" src="http://{id}.localhost:{port}/_clax/v/2/" allow="clipboard-write; fullscreen"></iframe>"#)), "{sub}");
    assert_eq!(
        boot(&sub).1["frame"],
        json!({"mode": "subdomain", "src": format!("http://{id}.localhost:{port}/_clax/v/2/")})
    );
    // A loopback host not named `localhost` (as on a LAN view: no artifact
    // origins) is always sandboxed, whatever the cookie says.
    let v6 = get(
        format!("/a/{id}"),
        vec![
            ("host", format!("[::1]:{port}")),
            ("cookie", "clax_frame=subdomain".into()),
        ],
    )
    .await;
    assert!(
        v6.contains(&format!(r#"src="/c/{id}/v/2/""#))
            && v6.contains(&format!(r#"sandbox="{FRAME_SANDBOX}""#)),
        "{v6}"
    );
    let pinned = get(
        format!("/a/{id}/v/1/docs/a%20b.html"),
        vec![("cookie", "clax_frame=sandbox".into())],
    )
    .await;
    assert!(
        pinned.contains(&format!(
            r#"src="http://{id}.localhost:{port}/_clax/v/1/docs/a%20b.html""#
        )),
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
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts.publish("E", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let first = page(&ts, &format!("/a/{id}"), &[]).await;
    assert_eq!(first.headers()["cache-control"], "private, no-cache");
    assert_eq!(first.headers()["vary"], "Cookie");
    let tag = first.headers()["etag"].to_str().unwrap().to_string();
    let again = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag.clone())]).await;
    assert_eq!(again.status(), 304);
    assert_eq!(again.headers()["vary"], "Cookie");
    assert_eq!(again.headers()["cache-control"], "private, no-cache");
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
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
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

#[tokio::test]
async fn a_host_the_api_refuses_gets_no_data() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts
        .publish("Secret plans", &[("index.html", "<p>x</p>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.thread(&id, 1, "a private remark").await;
    let v = ts.viewer(Some("Ada")).await;
    let port = ts.addr.port();
    let cookie = format!("clax_viewer={}; clax_frame=sandbox", v.cookie);
    for host in [
        format!("evil.example:{port}"),
        "evil.example:7480".to_string(),
        format!("192.168.1.5:{port}"),
        format!("localhost.evil.example:{port}"),
    ] {
        let api = page(
            &ts,
            &format!("/api/artifacts/{id}/threads"),
            &[("host", host.clone())],
        )
        .await;
        assert_eq!(api.status(), 403, "{host}: the API refuses this host");
        let res = page(
            &ts,
            &format!("/a/{id}"),
            &[("host", host.clone()), ("cookie", cookie.clone())],
        )
        .await;
        assert_eq!(res.status(), 200, "{host}");
        let html = res.text().await.unwrap();
        assert!(!html.contains(OPEN), "{host}: {html}");
        assert!(!html.contains("<iframe"), "{host}");
        assert!(
            !html.contains("a private remark") && !html.contains("Ada"),
            "{host}"
        );
        assert!(
            !html.contains("Secret plans") && html.contains("<h1>Clax</h1>"),
            "{host}"
        );
    }
    for host in [
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        ts.addr.to_string(),
    ] {
        let html = page(
            &ts,
            &format!("/a/{id}"),
            &[("host", host.clone()), ("cookie", cookie.clone())],
        )
        .await
        .text()
        .await
        .unwrap();
        let (_, b) = boot(&html);
        assert_eq!(
            b["threads"][0]["comments"][0]["body"], "a private remark",
            "{host}"
        );
        assert_eq!(b["viewer"]["display_name"], "Ada", "{host}");
        assert!(html.contains("<iframe"), "{host}");
    }
}

#[tokio::test]
async fn no_page_may_frame_the_shell() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts.publish("F", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let unframeable = |res: &reqwest::Response, what: &str| {
        assert_eq!(
            res.headers()["content-security-policy"],
            "frame-ancestors 'none'",
            "{what}"
        );
        assert_eq!(res.headers()["x-frame-options"], "DENY", "{what}");
    };
    for path in [
        "/".to_string(),
        format!("/a/{id}"),
        format!("/a/{id}/v/1/"),
        "/a/not-an-id".to_string(),
    ] {
        let res = page(&ts, &path, &[]).await;
        assert_eq!(res.status(), 200, "{path}");
        unframeable(&res, &path);
    }
    // The bare entry (an artifact the store does not have) and a revalidation
    // keep the headers too.
    unframeable(&page(&ts, "/a/7q3k9mzx2b4t", &[]).await, "bare entry");
    let first = page(&ts, &format!("/a/{id}"), &[]).await;
    let tag = first.headers()["etag"].to_str().unwrap().to_string();
    let again = page(&ts, &format!("/a/{id}"), &[("if-none-match", tag)]).await;
    assert_eq!(again.status(), 304);
    unframeable(&again, "304");
}

/// The owner's browser is marked as the owner's, in the boot payload as in
/// `GET /api/viewers/me` (its agents are "Your agents", spec
/// 2026-10-08-multi-agent-multi-person §10).
#[tokio::test]
async fn boot_marks_the_owners_viewer() {
    let dist = dist();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let id = ts.publish("V", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cookie = ts.owner_cookie();
    let me: Value = page(&ts, "/api/viewers/me", &[("cookie", cookie.clone())])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(me["viewer"]["owner"], true, "{me}");
    let html = page(&ts, &format!("/a/{id}"), &[("cookie", cookie)])
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(boot(&html).1["viewer"], me["viewer"]);
}

/// The `content` attribute of the `clax-embed` meta, unescaped, and where
/// the meta starts.
fn embed_meta(html: &str) -> Option<(usize, Value)> {
    let open = r#"<meta name="clax-embed" content=""#;
    let start = html.find(open)?;
    let rest = &html[start + open.len()..];
    let raw = &rest[..rest.find('"').unwrap()];
    let text = raw
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    Some((start, serde_json::from_str(&text).unwrap()))
}

#[tokio::test]
async fn an_embedded_core_marks_every_shell_document_and_frames_content_on_its_origin() {
    let dist = dist();
    std::fs::create_dir(dist.path().join("_clax")).unwrap();
    std::fs::write(dist.path().join("_clax/transport.js"), "/* transport */").unwrap();
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let embedded = {
        let web = std::sync::Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
        TestServer::spawn_with(move |s| {
            s.web = web;
            s.embed = Some(clax_http::state::EmbedOrigins {
                shell: "clax://app".into(),
                content: "clax://{id}.c".into(),
            });
        })
        .await
    };
    let created = embedded
        .publish("Report", &[("index.html", "<p>x</p>")])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();

    let html = page(&embedded, &format!("/a/{id}?app=1"), &[])
        .await
        .text()
        .await
        .unwrap();
    let (at, meta) = embed_meta(&html).expect("the clax-embed meta");
    assert_eq!(
        meta,
        json!({"transport": "host", "shell": "clax://app", "content": "clax://{id}.c"})
    );
    assert!(at < html.find("<script").unwrap(), "before any script");
    let (_, b) = boot(&html);
    assert_eq!(b["frame"]["mode"], "subdomain");
    let src = b["frame"]["src"].as_str().unwrap();
    assert!(src.starts_with(&format!("clax://{id}.c/v/1/")), "{src}");
    assert!(
        html.contains(&format!("src=\"clax://{id}.c/v/1/\"")),
        "{html}"
    );

    let gallery = page(&embedded, "/", &[]).await.text().await.unwrap();
    assert!(embed_meta(&gallery).is_some(), "{gallery}");

    // The app's transport is a classic script right after the meta, so it
    // runs before any of the shell's own, named by the version of its bytes
    // so that a document never runs a cached transport of another build.
    let v = clax_server::shell::transport_version(&*embedded.web);
    assert_eq!(v.len(), 12, "{v}");
    let tag = format!(r#"<script src="/_clax/transport.js?v={v}"></script>"#);
    for doc in [&html, &gallery] {
        let tag = tag.as_str();
        let (meta_at, _) = embed_meta(doc).unwrap();
        let script_at = doc.find(tag).expect("the transport's script");
        assert_eq!(doc.matches(tag).count(), 1, "{doc}");
        assert!(meta_at < script_at, "{doc}");
        assert_eq!(
            doc.find("<script"),
            Some(script_at),
            "first of the scripts: {doc}"
        );
    }

    // The content frame's bridge takes orders from the app's shell, which
    // its own origin does not name.
    let port = embedded.addr.port();
    let content = page(
        &embedded,
        "/v/1/",
        &[("host", format!("{id}.localhost:{port}"))],
    )
    .await
    .text()
    .await
    .unwrap();
    assert!(content.contains(r#"data-origin="clax://app""#), "{content}");

    let plain = ts.publish("Report", &[("index.html", "<p>x</p>")]).await;
    let pid = plain["artifact"]["id"].as_str().unwrap().to_string();
    let port = ts.addr.port();
    let content = page(&ts, "/v/1/", &[("host", format!("{pid}.localhost:{port}"))])
        .await
        .text()
        .await
        .unwrap();
    assert!(content.contains("<p>x</p>"), "{content}");
    assert!(!content.contains("data-origin"), "{content}");
    let html = page(&ts, &format!("/a/{pid}?app=1"), &[])
        .await
        .text()
        .await
        .unwrap();
    assert!(embed_meta(&html).is_none());
    assert!(!html.contains("clax-embed"));
    assert!(!html.contains("/_clax/transport.js"));

    // The versioned URL is immutable in a release build (a debug build reads
    // the file from disk, where it can change under the same daemon); any
    // other URL of it is revalidated against its ETag.
    let current = page(&embedded, &format!("/_clax/transport.js?v={v}"), &[]).await;
    assert_eq!(current.status(), 200);
    let expected = if cfg!(debug_assertions) {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    assert_eq!(current.headers()["cache-control"], expected);
    assert_eq!(current.headers()["content-type"], "text/javascript");
    for stale in ["/_clax/transport.js", "/_clax/transport.js?v=000000000000"] {
        let res = page(&embedded, stale, &[]).await;
        assert_eq!(res.headers()["cache-control"], "no-cache", "{stale}");
        let etag = res.headers()["etag"].to_str().unwrap().to_string();
        let again = page(&embedded, stale, &[("if-none-match", etag)]).await;
        assert_eq!(again.status(), 304, "{stale}");
    }
}
