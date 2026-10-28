//! `[serve] trusted_host_frames`: a shell on a `[serve] trusted_hosts` name
//! (`tailscale serve`'s) frames each artifact from `/_clax/frame/<id>/…` of
//! that same host, sandboxed without `allow-same-origin`, and relays its
//! runtime's site calls; without the setting the frame stays on the
//! artifact's own `<id>.localhost` site, as on main.
#![cfg(debug_assertions)]

use clax_testkit::TestServer;
use serde_json::{Value, json};

/// The `tailscale serve` name these tests trust.
const TRUSTED: &str = "phobos.tail556f5b.ts.net";
/// The owner's tailnet login (`[serve] owner_tailscale_login`).
const OWNER: &str = "bdelanghe@github";
const ENTRY: &str = include_str!("../../../web/shell/artifact.html");
const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;
const SANDBOX: &str = "allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals";

/// A scratch web UI: the artifact entry, a stand-in gallery and a stand-in
/// bridge part.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.html"), ENTRY).unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>gallery</p>").unwrap();
    std::fs::create_dir_all(dir.path().join("_clax/bridge")).unwrap();
    std::fs::write(dir.path().join("_clax/bridge/origin-abc.js"), "export {}").unwrap();
    dir
}

/// A daemon trusting [`TRUSTED`], naming [`OWNER`] the owner's tailnet
/// login, with `trusted_host_frames` set to `frames`.
async fn daemon(dist: &tempfile::TempDir, frames: bool) -> TestServer {
    let web = std::sync::Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
    TestServer::spawn_with(move |s| {
        s.web = web;
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = true;
        s.owner_tailscale_login = Some(std::sync::Arc::from(OWNER));
        s.trusted_host_frames = frames;
    })
    .await
}

/// A fleet-page-like artifact: it declares `db` and keeps one `fold`
/// document.
async fn fold_page(ts: &TestServer) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Fleet", "capabilities": {"db": {}}, "files": {"index.html": {"content": "<!doctype html><h1>Fleet</h1><ul id=folds></ul><script src=app.js></script>", "encoding": "utf8"}, "app.js": {"content": "1", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let aid = res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/docs/fold/root", ts.base)),
        )
        .json(&json!({"data": {"title": "Fleet", "links": ["a", "b"]}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    aid
}

/// `GET path` through `tailscale serve` on [`TRUSTED`] as `login`, as a
/// browser fetching from `site` (`Sec-Fetch-Site`).
async fn get(ts: &TestServer, path: &str, login: &str, site: &str) -> reqwest::Response {
    ts.client
        .get(format!("{}{path}", ts.base))
        .header("host", TRUSTED)
        .header("tailscale-user-login", login)
        .header("sec-fetch-site", site)
        .send()
        .await
        .unwrap()
}

/// The bootstrap block of `html`.
fn boot(html: &str) -> Value {
    let start = html.find(OPEN).expect("a bootstrap block") + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}

/// The `src` of the server-rendered frame in `html`, and its whole tag.
fn frame(html: &str) -> (String, String) {
    let start = html.find("<iframe class=\"frame\"").expect("a frame");
    let tag = &html[start..start + html[start..].find('>').unwrap()];
    let src = tag.split(" src=\"").nth(1).unwrap();
    (src[..src.find('"').unwrap()].to_string(), tag.to_string())
}

#[tokio::test]
async fn on_a_trusted_host_the_frame_is_a_sandboxed_path_whose_db_reads_through_the_shell() {
    let dist = dist();
    let ts = daemon(&dist, true).await;
    let aid = fold_page(&ts).await;
    // The shell, through tailscale serve, as the owner's tailnet login.
    let res = get(&ts, &format!("/a/{aid}"), OWNER, "none").await;
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    assert_eq!(boot(&html)["site"]["path"], "/_clax/frame/", "{html}");
    let (src, tag) = frame(&html);
    assert_eq!(src, format!("/_clax/frame/{aid}/_clax/v/1/"));
    assert!(tag.contains(&format!(" sandbox=\"{SANDBOX}\"")), "{tag}");
    assert!(!tag.contains("allow-same-origin"), "{tag}");
    // The API names the same site.
    let res = ts
        .client
        .get(format!("{}/api/artifacts/{aid}", ts.base))
        .header("host", TRUSTED)
        .header("tailscale-user-login", OWNER)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.json::<Value>().await.unwrap()["site"]["path"],
        "/_clax/frame/"
    );

    // The frame's page: served (the shell navigates it: same-origin), its
    // own policy sandboxing it without allow-same-origin, its bridge naming
    // this host's shell and relaying every call through it.
    let res = get(&ts, &src, OWNER, "same-origin").await;
    assert_eq!(res.status(), 200);
    let csp: Vec<String> = res
        .headers()
        .get_all("content-security-policy")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        csp.iter().any(|p| p == &format!("sandbox {SANDBOX}")),
        "{csp:?}"
    );
    assert!(
        csp.iter().all(|p| !p.contains("allow-same-origin")),
        "{csp:?}"
    );
    assert_eq!(
        res.headers()["cross-origin-resource-policy"],
        "cross-origin"
    );
    let page = res.text().await.unwrap();
    assert!(page.contains("<h1>Fleet</h1>"), "{page}");
    assert!(
        page.contains(&format!(
            "data-origin=\"https://{TRUSTED} http://{TRUSTED}\" data-api=\"frame\""
        )),
        "{page}"
    );
    // A file of the page, as its opaque origin loads it (cross-site).
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/v/1/app.js"),
        OWNER,
        "cross-site",
    )
    .await;
    assert_eq!(res.status(), 200);
    // A bridge part, which the opaque frame imports with CORS.
    let res = get(&ts, "/_clax/bridge/origin-abc.js", OWNER, "cross-site").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");

    // db reads, as the shell's relay (`site.fetch`) asks them: same-origin.
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/api/docs/fold/root"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_eq!(res.status(), 200);
    let v = res.json::<Value>().await.unwrap();
    assert_eq!(v["doc"]["data"]["title"], "Fleet", "{v}");
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/api/docs?collection=fold"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_eq!(res.status(), 200);
    let v = res.json::<Value>().await.unwrap();
    assert_eq!(v["docs"][0]["data"]["links"], json!(["a", "b"]), "{v}");
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/api/whoami"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_eq!(res.json::<Value>().await.unwrap()["kind"], "owner");
    // A write through the relay, with the shell's Origin.
    let res = ts
        .client
        .put(format!(
            "{}/_clax/frame/{aid}/_clax/api/docs/fold/root",
            ts.base
        ))
        .header("host", TRUSTED)
        .header("tailscale-user-login", OWNER)
        .header("sec-fetch-site", "same-origin")
        .header("origin", format!("https://{TRUSTED}"))
        .json(&json!({"data": {"title": "Fleet 2"}, "if_version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());

    // Never: the site API asked by the opaque frame itself, or another
    // site (cross-site), another login, or no login.
    for (login, site) in [
        (OWNER, "cross-site"),
        (OWNER, "same-site"),
        ("eve@github", "same-origin"),
        ("", "same-origin"),
    ] {
        let res = get(
            &ts,
            &format!("/_clax/frame/{aid}/_clax/api/docs/fold/root"),
            login,
            site,
        )
        .await;
        assert_eq!(res.status(), 403, "{login} {site}");
    }
    for login in ["eve@github", ""] {
        let res = get(&ts, &src, login, "same-origin").await;
        assert_eq!(res.status(), 403, "{login}");
    }
    // The frame path names a site of its own artifact alone: the public
    // host's API is not under it.
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/api/token"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_ne!(res.status(), 200);
}

#[tokio::test]
async fn without_the_setting_a_trusted_host_frames_as_main_does() {
    let dist = dist();
    let ts = daemon(&dist, false).await;
    let aid = fold_page(&ts).await;
    let res = get(&ts, &format!("/a/{aid}"), OWNER, "none").await;
    let html = res.text().await.unwrap();
    let b = boot(&html);
    assert!(b["site"].get("path").is_none(), "{}", b["site"]);
    assert_eq!(b["site"]["domain"], "localhost");
    let (src, tag) = frame(&html);
    assert!(
        src.starts_with(&format!("http://{aid}.localhost/_clax/session/start?next=")),
        "{src}"
    );
    assert!(!tag.contains("sandbox"), "{tag}");
    // The frame path is no site: the public host's 404.
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/v/1/"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_eq!(res.status(), 404);
    let res = get(
        &ts,
        &format!("/_clax/frame/{aid}/_clax/api/docs/fold/root"),
        OWNER,
        "same-origin",
    )
    .await;
    assert_eq!(res.status(), 404);
    // Nor does a host that is not trusted get path frames with it on.
    let on = daemon(&dist, true).await;
    let aid = fold_page(&on).await;
    let port = on.addr.port();
    let res = on
        .client
        .get(format!("{}/a/{aid}", on.base))
        .header("host", format!("localhost:{port}"))
        .send()
        .await
        .unwrap();
    let html = res.text().await.unwrap();
    assert!(boot(&html)["site"].get("path").is_none());
    assert!(!frame(&html).1.contains("sandbox"));
}
