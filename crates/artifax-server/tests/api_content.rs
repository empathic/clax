mod common;
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
    assert_eq!(res.headers()["cache-control"], "no-store");
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

#[tokio::test]
async fn main_origin_content_is_sandboxed_but_artifact_origin_is_not() {
    let ts = TestServer::spawn().await;
    let created = ts
        .publish("R", &[("index.html", "<p>hi</p>"), ("a.css", "p{}")])
        .await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let csp = "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";
    for path in [format!("/c/{id}/v/1/"), format!("/c/{id}/v/1/a.css")] {
        let res = ts.get(&path).await;
        assert_eq!(res.headers()["content-security-policy"], csp, "{path}");
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
        assert!(
            res.headers().get("content-security-policy").is_none(),
            "{path}"
        );
    }
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
    for path in ["/_blob/does-not-exist", "/_artifax/x.js"] {
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
            assert_eq!(res.headers()["cache-control"], "no-store");
        } else {
            assert_eq!(
                res.json::<serde_json::Value>().await.unwrap()["error"]["code"],
                "ui_not_built"
            );
        }
    }
    assert_eq!(ts.get("/_artifax/does-not-exist.js").await.status(), 404);
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
    let csp = "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";
    let index = ts.get(&format!("/c/{id}/v/1/")).await.text().await.unwrap();
    assert!(index.contains("data-file=\"index.html\""), "{index}");

    let res = ts.get(&format!("/c/{id}/v/1/about.html")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["content-security-policy"], csp);
    let html = res.text().await.unwrap();
    assert_eq!(html.matches("/_artifax/bridge.js").count(), 1, "{html}");
    assert!(html.starts_with("<!doctype html><html><head><title>About</title>"));
    assert!(html.contains(&format!(
        "<body><script src=\"/_artifax/bridge.js\" data-artifact=\"{id}\" data-version=\"1\" data-contract=\"0.2.61\" data-file=\"about.html\"></script><h2>About</h2>"
    )));

    let part = ts
        .get(&format!("/c/{id}/v/1/docs/part.htm"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        part.starts_with("<!doctype html><html><head>"),
        "a fragment gets the skeleton"
    );
    assert!(part.contains("data-file=\"docs/part.htm\"></script><p>part</p>"));

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
    assert!(res.headers().get("content-security-policy").is_none());
    let html = res.text().await.unwrap();
    assert_eq!(html.matches("/_artifax/bridge.js").count(), 1);
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
    let stale = "<!doctype html><html><body><script src=\"/_artifax/bridge.js\" data-artifact=\"7q3k9mzx2b4t\" data-version=\"1\" data-contract=\"0.2.61\" data-file=\"about.html\"></script><p>x</p></body></html>";
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
    assert_eq!(html.matches("/_artifax/bridge.js").count(), 1, "{html}");
    assert!(html.contains(&format!("data-artifact=\"{id}\"")));
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
    let aid = artifax_core::ArtifactId::parse(id).unwrap();
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
