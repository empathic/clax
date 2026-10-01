mod common;
use clax_core::wrap::bridge_tag_for;
use clax_server::routes::shell::bridge_version;
use common::TestServer;

#[tokio::test]
async fn serves_wrapped_index_and_files_with_caching_headers() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<title>R</title><p>hi</p>"),
                ("css/a.css", "p{color:red}"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts.get(&format!("/c/{id}/v/1/")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-cache");
    let html = res.text().await.unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(&format!(
        "data-artifact=\"{id}\" data-version=\"1\" data-contract=\"0.2.61\""
    )));
    let res = ts.get(&format!("/c/{id}/v/1/css/a.css")).await;
    assert_eq!(res.headers()["content-type"], "text/css");
    assert_eq!(
        res.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    assert_eq!(res.text().await.unwrap(), "p{color:red}");
    assert_eq!(
        ts.get(&format!("/c/{id}/v/1/missing.js")).await.status(),
        404
    );
    assert_eq!(ts.get(&format!("/c/{id}/v/9/")).await.status(), 404);
    let res = ts
        .client
        .get(format!("{}/c/{id}/v/1", ts.base))
        .send()
        .await
        .unwrap();
    assert!(
        res.url().path().ends_with("/v/1/"),
        "redirected to trailing slash"
    );
}

#[tokio::test]
async fn artifact_host_routes_to_content_and_nothing_else() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("<p>hi</p>"));
    let res = ts
        .client
        .get(format!("{}/healthz", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .get(format!("{}/api/artifacts", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        404,
        "API is not reachable on an artifact origin"
    );
    let res = ts
        .client
        .get(format!("{}/v/1/", ts.base))
        .header("host", format!("{id}.localhost.attacker.com"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// The policy of content on the main origin: sandboxed. It names no
/// `frame-ancestors`, which would refuse a page framed by another page of its
/// artifact (a sandboxed parent's origin is opaque).
const SANDBOXED: &str =
    "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";

#[tokio::test]
async fn main_origin_content_is_sandboxed_but_artifact_origin_is_not() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish("R", &[("index.html", "<p>hi</p>"), ("a.css", "p{}")])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    for path in [format!("/c/{id}/v/1/"), format!("/c/{id}/v/1/a.css")] {
        let res = ts.get(&path).await;
        assert_eq!(
            res.headers()["content-security-policy"],
            SANDBOXED,
            "{path}"
        );
    }
    let host = format!("{id}.localhost");
    for path in ["/v/1/", "/v/1/a.css"] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let csp = res.headers()["content-security-policy"].to_str().unwrap();
        assert!(!csp.contains("sandbox"), "{path}: {csp}");
    }
}

#[tokio::test]
async fn only_the_shell_and_the_artifact_frame_content_on_an_artifact_origin() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[("index.html", "<p>hi</p>"), ("about.html", "<p>a</p>")],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    // On an artifact origin the shell is at localhost or 127.0.0.1 on the
    // same port; the artifact's own pages may frame each other.
    for (host, port) in [
        (format!("{id}.localhost:4321"), ":4321"),
        (format!("{id}.localhost"), ""),
    ] {
        for path in ["/v/1/", "/v/1/about.html"] {
            let res = ts
                .client
                .get(format!("{}{path}", ts.base))
                .header("host", &host)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            assert_eq!(
                res.headers()["content-security-policy"],
                format!("frame-ancestors 'self' http://localhost{port} http://127.0.0.1{port}")
                    .as_str(),
                "{host}{path}"
            );
        }
    }
    // A revalidation keeps the policy.
    let host = format!("{id}.localhost:4321");
    let get = |etag: Option<String>| {
        let mut req = ts
            .client
            .get(format!("{}/v/1/", ts.base))
            .header("host", &host);
        if let Some(e) = etag {
            req = req.header("if-none-match", e);
        }
        req.send()
    };
    let first = get(None).await.unwrap();
    let etag = first.headers()["etag"].to_str().unwrap().to_string();
    let again = get(Some(etag)).await.unwrap();
    assert_eq!(again.status(), 304);
    assert_eq!(
        again.headers()["content-security-policy"],
        "frame-ancestors 'self' http://localhost:4321 http://127.0.0.1:4321"
    );
}

#[tokio::test]
async fn redirects_are_relative() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let c = no_redirect_client();
    let res = c
        .get(format!("{}/c/{id}/v/1", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 308);
    assert_eq!(res.headers()["location"], "1/");
    let res = c
        .get(format!("{}/v/1/index.html", ts.base))
        .header("host", format!("{id}.localhost"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 308);
    assert_eq!(res.headers()["location"], "./");
}

#[tokio::test]
async fn artifact_host_passes_blob_and_bridge_paths_to_router() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    for path in ["/_blob/does-not-exist", "/_clax/x.js"] {
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .header("host", &host)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{path}");
        let v: serde_json::Value = res.json().await.expect(path);
        assert_eq!(v["error"]["code"], "not_found", "{path}");
    }
}

#[tokio::test]
async fn shell_routes_serve_ui_or_explain_missing_build() {
    let ts = TestServer::spawn().await;
    for path in ["/", "/a/7q3k9mzx2b4t", "/a/7q3k9mzx2b4t/v/2"] {
        let res = ts.get(path).await;
        let status = res.status().as_u16();
        assert!(status == 200 || status == 503, "{path} -> {status}");
        if status == 200 {
            assert!(
                res.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("text/html")
            );
            // The artifact page carries one viewer's data: no shared cache.
            let want = if path == "/" {
                "no-cache"
            } else {
                "private, no-cache"
            };
            assert_eq!(res.headers()["cache-control"], want, "{path}");
        } else {
            assert_eq!(
                res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
                "ui_not_built"
            );
        }
    }
    assert_eq!(ts.get("/_clax/does-not-exist.js").await.status(), 404);
}

#[tokio::test]
async fn every_page_path_under_an_artifact_serves_the_artifact_entry() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[("index.html", "<p>i</p>"), ("docs/about.html", "<p>a</p>")],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let shell = ts.get(&format!("/a/{id}")).await;
    let (status, body) = (shell.status(), shell.bytes().await.unwrap());
    for path in [
        format!("/a/{id}/about.html"),
        format!("/a/{id}/docs/about.html"),
        format!("/a/{id}/v/1/docs/about.html"),
        format!("/a/{id}/v/1/"),
        format!("/a/{id}/v/x.html"),
    ] {
        let res = ts.get(&path).await;
        assert_eq!(res.status(), status, "{path}");
        assert_eq!(
            res.bytes().await.unwrap(),
            body,
            "{path} is the artifact entry"
        );
    }
    // Content, the API, and the shell's own assets keep their routes.
    let res = ts.get(&format!("/c/{id}/v/1/docs/about.html")).await;
    assert!(
        res.text()
            .await
            .unwrap()
            .contains("data-file=\"docs/about.html\"")
    );
    assert_eq!(ts.get(&format!("/api/artifacts/{id}")).await.status(), 200);
    assert_eq!(ts.get("/_clax/does-not-exist.js").await.status(), 404);
    // An artifact origin serves content only.
    let res = ts
        .client
        .get(format!("{}/a/{id}/about.html", ts.base))
        .header("host", format!("{id}.localhost"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

const ABOUT: &str = "<!doctype html><html><head><title>About</title></head><body><h2>About</h2><a href=\"index.html\">back</a></body></html>";

#[tokio::test]
async fn supporting_html_files_are_wrapped_like_the_index_and_others_are_not() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<a href=\"about.html\">about</a>"),
                ("about.html", ABOUT),
                ("docs/part.htm", "<p>part</p>"),
                ("a.css", "p{}"),
                ("data.json", "{\"x\":1}"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let index = ts.get(&format!("/c/{id}/v/1/")).await.text().await.unwrap();
    assert!(index.contains("data-file=\"index.html\""), "{index}");

    let res = ts.get(&format!("/c/{id}/v/1/about.html")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.headers()["content-security-policy"], SANDBOXED);
    let html = res.text().await.unwrap();
    assert_eq!(html.matches("/_clax/bridge.js").count(), 1, "{html}");
    // The bridge goes right after the doctype, before any page script.
    let tag = bridge_tag_for(id, 1, "0.2.61", "about.html", &bridge_version());
    assert!(
        html.starts_with(&format!(
            "<!doctype html>{tag}<html><head><title>About</title>"
        )),
        "{html}"
    );

    let part = ts
        .get(&format!("/c/{id}/v/1/docs/part.htm"))
        .await
        .text()
        .await
        .unwrap();
    // A fragment gets the skeleton, with the bridge first in its <head>.
    let tag = bridge_tag_for(id, 1, "0.2.61", "docs/part.htm", &bridge_version());
    assert!(
        part.starts_with(&format!(
            "<!doctype html><html><head>{tag}<meta charset=utf8>"
        )),
        "{part}"
    );
    assert!(part.contains("<body><p>part</p></body>"), "{part}");

    for (path, ct, body) in [
        ("a.css", "text/css", "p{}"),
        ("data.json", "application/json", "{\"x\":1}"),
    ] {
        let res = ts.get(&format!("/c/{id}/v/1/{path}")).await;
        assert_eq!(res.headers()["content-type"], ct, "{path}");
        assert_eq!(
            res.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(res.text().await.unwrap(), body, "{path} untouched");
    }

    let host = format!("{id}.localhost");
    let res = ts
        .client
        .get(format!("{}/v/1/about.html", ts.base))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()["content-security-policy"],
        "frame-ancestors 'self' http://localhost http://127.0.0.1"
    );
    let html = res.text().await.unwrap();
    assert_eq!(html.matches("/_clax/bridge.js").count(), 1);
    assert!(html.contains("data-file=\"about.html\""));

    let raw = ts
        .get_authed(&format!("/api/artifacts/{id}/versions/1/files/about.html"))
        .await;
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.text().await.unwrap(), ABOUT, "raw bytes stay unwrapped");
}

#[tokio::test]
async fn a_supporting_page_carrying_a_bridge_tag_is_served_with_one() {
    let ts = TestServer::spawn().await;
    let stale = "<!doctype html><html><body><script src=\"/_clax/bridge.js\" data-artifact=\"7q3k9mzx2b4t\" data-version=\"1\" data-contract=\"0.2.61\" data-file=\"about.html\"></script><p>x</p></body></html>";
    let created = ts
        .publish("R", &[("index.html", "<p>i</p>"), ("about.html", stale)])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let html = ts
        .get(&format!("/c/{id}/v/1/about.html"))
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(html.matches("/_clax/bridge.js").count(), 1, "{html}");
    assert!(html.contains(&format!("data-artifact=\"{id}\"")));
}

/// `GET path`, on `host` when given, with `If-None-Match: etag` when given.
async fn get_on(
    ts: &TestServer,
    host: Option<&str>,
    path: &str,
    etag: Option<&str>,
) -> reqwest::Response {
    let mut req = ts.client.get(format!("{}{path}", ts.base));
    if let Some(h) = host {
        req = req.header("host", h);
    }
    if let Some(e) = etag {
        req = req.header("if-none-match", e);
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn every_html_page_is_revalidated_and_answers_not_modified() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<p>i</p>"),
                ("about.html", "<!doctype html><body><p>a</p></body>"),
                ("a.css", "p{}"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    for (on, path) in [
        (None, format!("/c/{id}/v/1/")),
        (None, format!("/c/{id}/v/1/about.html")),
        (Some(host.as_str()), "/v/1/".to_string()),
        (Some(host.as_str()), "/v/1/about.html".to_string()),
    ] {
        let res = get_on(&ts, on, &path, None).await;
        assert_eq!(res.status(), 200, "{path}");
        assert_eq!(res.headers()["cache-control"], "no-cache", "{path}");
        let etag = res.headers()["etag"].to_str().unwrap().to_string();
        assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
        let again = get_on(&ts, on, &path, Some(&etag)).await;
        assert_eq!(again.status(), 304, "{path}");
        assert_eq!(again.headers()["cache-control"], "no-cache", "{path}");
        assert_eq!(again.headers()["etag"], etag.as_str(), "{path}");
        assert!(again.bytes().await.unwrap().is_empty());
        let other = get_on(&ts, on, &path, Some("\"elsewhere\"")).await;
        assert_eq!(other.status(), 200, "{path}");
    }
    let css = get_on(&ts, None, &format!("/c/{id}/v/1/a.css"), None).await;
    assert_eq!(
        css.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let shell = ts.get("/").await;
    if shell.status() == 200 {
        assert_eq!(shell.headers()["cache-control"], "no-cache");
        let etag = shell.headers()["etag"].to_str().unwrap().to_string();
        assert_eq!(get_on(&ts, None, "/", Some(&etag)).await.status(), 304);
    }
}

#[tokio::test]
async fn the_bridge_is_named_by_version_and_only_the_versioned_url_is_immutable_in_release() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>i</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let v = bridge_version();
    let v = v.as_str();
    let html = ts.get(&format!("/c/{id}/v/1/")).await.text().await.unwrap();
    assert!(
        html.contains(&bridge_tag_for(id, 1, "0.2.61", "index.html", v)),
        "{html}"
    );
    let bare = ts.get("/_clax/bridge.js").await;
    if bare.status() == 404 {
        assert!(v.is_empty(), "no bridge built, no version");
        assert!(html.contains("<script src=\"/_clax/bridge.js\" "));
        return;
    }
    assert_eq!(v.len(), 12, "{v}");
    assert!(html.contains(&format!("<script src=\"/_clax/bridge.js?v={v}\" ")));
    assert_eq!(bare.headers()["cache-control"], "no-cache");
    let host = format!("{id}.localhost");
    for on in [None, Some(host.as_str())] {
        let res = get_on(&ts, on, &format!("/_clax/bridge.js?v={v}"), None).await;
        assert_eq!(res.status(), 200);
        // A debug build reads the bridge from disk, where it can change under
        // the same daemon: it is never immutable there.
        let expected = if cfg!(debug_assertions) {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        };
        assert_eq!(res.headers()["cache-control"], expected);
        assert!(res.headers().contains_key("etag"));
        assert_eq!(res.headers()["content-type"], "text/javascript");
        let stale = get_on(&ts, on, "/_clax/bridge.js?v=000000000000", None).await;
        assert_eq!(
            stale.headers()["cache-control"],
            "no-cache",
            "another version's URL"
        );
    }
}

#[tokio::test]
async fn wrapped_pages_are_cached_per_file_until_the_artifact_is_deleted() {
    let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot = cache.clone();
    let ts = TestServer::spawn_with(move |s| {
        *slot.lock().unwrap() = Some(s.wrap_cache.clone());
    })
    .await;
    let cache = cache.lock().unwrap().clone().unwrap();
    let created = ts
        .publish(
            "R",
            &[
                ("index.html", "<p>index</p>"),
                ("about.html", "<p>about</p>"),
            ],
        )
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let get = |p: &'static str| {
        let url = format!("/c/{id}/v/1/{p}");
        let ts = &ts;
        async move { ts.get(&url).await.text().await.unwrap() }
    };
    assert!(get("").await.contains("<p>index</p>"));
    assert!(!cache.contains(id, 1, "about.html"));
    assert!(get("about.html").await.contains("<p>about</p>"));
    assert!(cache.contains(id, 1, "index.html") && cache.contains(id, 1, "about.html"));
    // Versions are immutable: a second serve comes from the cache, not the disk.
    let aid = clax_core::ArtifactId::parse(id).unwrap();
    let disk = ts
        .home
        .version_dir(&aid, 1)
        .join("files")
        .join("about.html");
    std::fs::write(&disk, "<p>changed</p>").unwrap();
    let again = get("about.html").await;
    assert!(again.contains("<p>about</p>") && !again.contains("changed"));
    assert!(
        get("").await.contains("<p>index</p>"),
        "each file keeps its own wrap"
    );
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(!cache.contains(id, 1, "index.html") && !cache.contains(id, 1, "about.html"));
    assert_eq!(
        ts.get(&format!("/c/{id}/v/1/about.html")).await.status(),
        404
    );
}

#[tokio::test]
async fn an_html_file_that_is_not_utf8_is_served_as_stored_and_remembered() {
    let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot = cache.clone();
    let ts = TestServer::spawn_with(move |s| {
        *slot.lock().unwrap() = Some(s.wrap_cache.clone());
    })
    .await;
    let cache = cache.lock().unwrap().clone().unwrap();
    let res = ts
        .post_json(
            "/api/artifacts",
            serde_json::json!({"title": "L", "files": {
                "index.html": {"content": "<p>i</p>", "encoding": "utf8"},
                "latin1.html": {"content": "PHA+Y2Fm6TwvcD4=", "encoding": "base64"}
            }}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let id = res.json::<serde_json::Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..2 {
        let res = ts.get(&format!("/c/{id}/v/1/latin1.html")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(
            res.headers()["cache-control"],
            "no-cache",
            "HTML is revalidated, wrapped or not"
        );
        let etag = res.headers()["etag"].to_str().unwrap().to_string();
        let again = ts
            .client
            .get(format!("{}/c/{id}/v/1/latin1.html", ts.base))
            .header("if-none-match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 304);
        assert_eq!(again.headers()["cache-control"], "no-cache");
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"<p>caf\xe9</p>");
        assert!(
            cache.contains(&id, 1, "latin1.html"),
            "the result is remembered, so the file is not read to find out again"
        );
    }
}

#[tokio::test]
async fn the_runtime_contract_type_definitions_are_served_as_text() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/_clax/contract/0.2.61/db.d.ts").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/plain; charset=utf-8");
    let text = res.text().await.unwrap();
    assert_eq!(text, include_str!("../../../web/contract/0.2.61/db.d.ts"));
    // Clax's additions sit beside the unchanged files.
    let res = ts.get("/_clax/contract/clax-extensions.d.ts").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        res.text().await.unwrap(),
        include_str!("../../../web/contract/clax-extensions.d.ts")
    );
    for missing in [
        "/_clax/contract/0.2.61/clax-extensions.d.ts",
        "/_clax/contract/clax-extensions",
        "/_clax/contract/0.2.61/nope.d.ts",
        "/_clax/contract/0.2.61/",
        "/_clax/contract/0.2.60/db.d.ts",
    ] {
        assert_eq!(ts.get(missing).await.status(), 404, "{missing}");
    }
    // An artifact's own origin reaches it too, like the other `/_clax` assets.
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = get_on(
        &ts,
        Some(&format!("{id}.localhost")),
        "/_clax/contract/0.2.61/user.d.ts",
        None,
    )
    .await;
    assert_eq!(res.status(), 200);
}
