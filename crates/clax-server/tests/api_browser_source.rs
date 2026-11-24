//! The daemon's browser source's site pieces (`clax_server::browser_source`):
//! what a browser test's page is served is what the artifact's own site
//! serves.

use clax_bot::browser::BrowserSource;
use clax_server::browser_source::DaemonBrowser;
use clax_store::roles::OWNER;
use clax_testkit::{TestServer, within};
use serde_json::Value;

/// The pairs of `served` named `name`, whatever its case.
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .collect()
}

#[tokio::test]
async fn the_bridge_and_its_parts_are_served() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("B", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let b = DaemonBrowser::of(&ts.state);
    let bridge = b.clax_file("/_clax/bridge.js").expect("the bridge");
    let site = ts.site_get(id, "/_clax/bridge.js").await;
    assert_eq!(site.status(), 200);
    assert_eq!(
        header(&bridge.headers, "content-type"),
        vec![site.headers()["content-type"].to_str().unwrap()]
    );
    assert_eq!(bridge.status, 200);
    assert_eq!(bridge.body, site.bytes().await.unwrap().to_vec());
    let part = ts
        .web
        .paths()
        .into_iter()
        .filter_map(|p| p.strip_prefix("_clax/").map(str::to_string))
        .find(|p| p.starts_with("bridge/") && p.ends_with(".js"))
        .expect("the bridge has a lazy part");
    let url = format!("/_clax/{part}");
    let served = b.clax_file(&url).expect("the part");
    let site = ts.site_get(id, &url).await;
    assert_eq!(site.status(), 200);
    assert_eq!(served.body, site.bytes().await.unwrap().to_vec());
    // Nothing else of the web UI, and no way out of the bridge's files.
    for other in [
        "/_clax/transport.js",
        "/_clax/overlay.js",
        "/_clax/bridge/",
        "/_clax/bridge/../bridge.js",
        "/_clax/bridge/.vite/manifest.json",
        "/_clax/bridge/missing.js",
        "/bridge.js",
    ] {
        assert!(b.clax_file(other).is_none(), "{other}");
    }
    // A source with no site serves none.
    let bare = DaemonBrowser::new(ts.store.clone(), None);
    assert!(bare.clax_file("/_clax/bridge.js").is_none());
    assert!(bare.site_headers(id).is_empty());
}

#[tokio::test]
async fn site_headers_are_the_sites() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("H", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let ours = DaemonBrowser::of(&ts.state).site_headers(id);
    // A page of the site and a 404 of it carry the same isolation headers
    // and the same lockdown policy (the page's set by its route, the 404's
    // by the site layer).
    for path in ["/", "/missing.css"] {
        let res = ts.site_get(id, path).await;
        for name in [
            "content-security-policy",
            "referrer-policy",
            "x-content-type-options",
            "cross-origin-resource-policy",
            "strict-transport-security",
        ] {
            let site: Vec<&str> = res
                .headers()
                .get_all(name)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect();
            assert_eq!(header(&ours, name), site, "{name} of {path}");
        }
    }
    assert_eq!(header(&ours, "x-content-type-options"), vec!["nosniff"]);
    let csp = header(&ours, "content-security-policy").join(" ");
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    assert!(
        DaemonBrowser::of(&ts.state)
            .site_headers("not an ID")
            .is_empty()
    );
}

#[tokio::test]
async fn site_api_reads_as_the_person_and_refuses_another_artifact() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>a</p>")]).await;
    let a = a["artifact"]["id"].as_str().unwrap();
    let other = ts.publish("O", &[("index.html", "<p>o</p>")]).await;
    let other = other["artifact"]["id"].as_str().unwrap();
    let b = DaemonBrowser::of(&ts.state);
    let json = |body: &[u8]| serde_json::from_slice::<Value>(body).unwrap();

    // As the owner: what the owner's site session reads there.
    let who = within("whoami", b.site_api(OWNER, a, "/_clax/api/whoami")).await;
    assert_eq!(who.status, 200);
    let site = ts.site_get(a, "/_clax/api/whoami").await;
    assert_eq!(json(&who.body), site.json::<Value>().await.unwrap());
    assert_eq!(json(&who.body)["kind"], "owner");
    let art = within("artifact", b.site_api(OWNER, a, "/_clax/api/artifact")).await;
    assert_eq!(art.status, 200, "{}", String::from_utf8_lossy(&art.body));
    assert_eq!(json(&art.body)["artifact"]["id"], a);

    // Another artifact's documents, which `a` does not declare it reads.
    let read = within(
        "read",
        b.site_api(OWNER, a, &format!("/_clax/api/read/{other}/docs/notes/one")),
    )
    .await;
    assert_eq!(read.status, 403, "{}", String::from_utf8_lossy(&read.body));
    // The public host's routes are not the site API's.
    let public = within(
        "public",
        b.site_api(OWNER, a, &format!("/api/artifacts/{other}")),
    )
    .await;
    assert_eq!(public.status, 404);
    // Someone who may not read the artifact is placed as no one.
    let stranger = within(
        "stranger",
        b.site_api("01HZZZZZZZZZZZZZZZZZZZZZZZ", a, "/_clax/api/whoami"),
    )
    .await;
    assert_eq!(stranger.status, 401);
    assert_eq!(
        json(&stranger.body)["error"]["code"],
        "site_session_required"
    );
}
