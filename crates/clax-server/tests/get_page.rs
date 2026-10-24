//! The Get in Clax page (`GET /a/<id>/get`) of a hosted Clax and the app
//! links beside it (`/.well-known/apple-app-site-association`,
//! `/.well-known/assetlinks.json`): what the page offers, who may see it,
//! and that its path is reserved from the shell's file paths.

use clax_core::config::{AndroidApp, ClaxApp};
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";
const MIME: &str = "application/vnd.empathic.clax+zip";
const LINEAGE: &str = "l_abcdefghijklmnopqrstuvwxyz";
/// A well-formed artifact ID no artifact has.
const NEVER: &str = "7q3k9mzx2b4t";
const TEAM: &str = "ABCDE12345";
/// The Clax app's bundle ID (`apps/clax_app/macos/Runner/Configs/AppInfo.xcconfig`,
/// `bazel/flutter/ios.bzl`) and Android application ID
/// (`apps/clax_app/android/app/build.gradle.kts`).
const APP_ID: &str = "dev.empathic.clax";

fn fingerprint() -> String {
    ["5A"; 32].join(":")
}

/// App links for both platforms and both store pages.
fn configured() -> ClaxApp {
    ClaxApp {
        apple_team_id: Some(TEAM.into()),
        android: Some(AndroidApp {
            package: APP_ID.into(),
            sha256: fingerprint(),
        }),
        ios_store: Some("https://apps.apple.com/app/clax/id6700000000".into()),
        android_store: Some(
            "https://play.google.com/store/apps/details?id=dev.empathic.clax".into(),
        ),
    }
}

/// A scratch web UI: the artifact entry and a stand-in gallery.
fn dist() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("artifact.html"),
        include_str!("../../../web/shell/artifact.html"),
    )
    .unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>gallery</p>").unwrap();
    dir
}

/// A hosted daemon on a stopped clock, showing the bundled legal
/// documents, with `app` as its Clax app settings.
struct Site {
    ts: TestServer,
    origin: String,
    _dist: tempfile::TempDir,
}

#[derive(Clone, Default)]
struct Browser {
    cookie: Option<String>,
}

fn cookie_of(res: &reqwest::Response) -> Option<String> {
    res.headers().get(SET_COOKIE).and_then(|v| {
        v.to_str()
            .unwrap()
            .strip_prefix("clax_session=")
            .map(|c| c.split(';').next().unwrap().to_string())
    })
}

impl Site {
    async fn new(app: ClaxApp) -> Site {
        let dist = dist();
        let web: Arc<dyn clax_core::embedded::FileTree> =
            Arc::new(clax_core::embedded::DiskTree::new(dist.path()));
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted_with(clock, Some(SECRET), move |s| {
            s.web = web;
            s.clax_app = Arc::new(app);
            s.hosted = s.hosted.take().map(|h| {
                Arc::new(
                    Arc::try_unwrap(h)
                        .expect("hosted is not shared yet")
                        .with_legal(Some(clax_core::config::Legal::Bundled)),
                )
            });
        })
        .await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        Site {
            ts,
            origin,
            _dist: dist,
        }
    }

    fn host(&self) -> String {
        format!("localhost:{}", self.ts.addr.port())
    }

    fn req(&self, b: &Browser, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .ts
            .client
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header(HOST, self.host());
        if method != reqwest::Method::GET {
            r = r.header(ORIGIN, &self.origin);
        }
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        r
    }

    /// `GET path` from `b`: the status, the content type and the body.
    async fn page(&self, b: &Browser, path: &str) -> (u16, String, String) {
        let r = self
            .req(b, reqwest::Method::GET, path)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let mime = r
            .headers()
            .get(CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        (status, mime, r.text().await.unwrap())
    }

    async fn json(&self, b: &Browser, method: reqwest::Method, path: &str, body: Value) -> Value {
        let r = self.req(b, method, path).json(&body).send().await.unwrap();
        assert!(r.status().is_success(), "{path}: {}", r.status());
        r.json().await.unwrap()
    }

    /// The owner, named `name`, signed in through setup.
    async fn owner(&self, name: &str) -> Browser {
        let r = self
            .req(
                &Browser::default(),
                reqwest::Method::POST,
                "/api/hosted/setup",
            )
            .json(&json!({"secret": SECRET, "name": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        Browser {
            cookie: cookie_of(&r),
        }
    }

    /// A guest named `name`.
    async fn guest(&self, name: &str) -> Browser {
        let r = self
            .req(&Browser::default(), reqwest::Method::PUT, "/api/viewers/me")
            .json(&json!({"display_name": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        Browser {
            cookie: cookie_of(&r),
        }
    }

    async fn artifact(&self, title: &str) -> String {
        self.ts
            .publish(title, &[("index.html", "<h1>x</h1>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// `owner` puts a bundle of release `bv` on version 1 of `aid`.
    async fn bundle(&self, owner: &Browser, aid: &str, bv: u64) {
        self.bundle_on(owner, aid, 1, bv).await;
    }

    /// `owner` puts a bundle of release `bv` on version `n` of `aid`.
    async fn bundle_on(&self, owner: &Browser, aid: &str, n: u32, bv: u64) {
        let v = self
            .json(
                owner,
                reqwest::Method::GET,
                &format!("/api/artifacts/{aid}/versions/{n}"),
                json!(null),
            )
            .await;
        let content = v["version"]["content_sha256"].as_str().unwrap();
        let r = self
            .req(
                owner,
                reqwest::Method::PUT,
                &format!("/api/artifacts/{aid}/versions/{n}/bundle"),
            )
            .header("content-type", MIME)
            .header("clax-bundle-lineage", LINEAGE)
            .header("clax-bundle-version", bv.to_string())
            .header("clax-bundle-content-sha256", content)
            .body(b"PK\x03\x04 a bundle".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201);
    }

    async fn link(&self, owner: &Browser, aid: &str, access: Option<&str>) {
        self.json(
            owner,
            reqwest::Method::PUT,
            &format!("/api/hosted/artifacts/{aid}/link"),
            json!({"access": access}),
        )
        .await;
    }
}

/// The path of the QR code of `text` as `qrcode` draws it, one unit a
/// module: what the page's inline SVG must hold to encode `text`.
fn qr_path(text: &str) -> String {
    let svg = qrcode::QrCode::new(text.as_bytes())
        .unwrap()
        .render::<qrcode::render::svg::Color>()
        .module_dimensions(1, 1)
        .build();
    let d = svg.split(" d=\"").nth(1).unwrap();
    d.split('"').next().unwrap().to_string()
}

/// The classes the page's phone rule (`@media (max-width:600px) and
/// (pointer:coarse){…}`) hides.
fn phone_hidden(html: &str) -> Vec<String> {
    let rule = html
        .split("@media (max-width:600px) and (pointer:coarse){")
        .nth(1)
        .expect("the page has a phone rule");
    let block = &rule[..rule.find("}}").expect("the phone rule ends")];
    let (selectors, decl) = block.split_once('{').unwrap();
    assert_eq!(decl, "display:none", "{block}");
    selectors
        .split(',')
        .map(|sel| {
            sel.strip_prefix('.')
                .unwrap_or_else(|| panic!("a class selector: {sel}"))
                .to_string()
        })
        .collect()
}

/// The classes of the element that begins at byte `at` of `html` and of
/// every element around it: the tags opened before `at` and not yet closed.
fn classes_around(html: &str, at: usize) -> Vec<String> {
    let body = &html[html.find("<body>").unwrap()..];
    let at = at - (html.len() - body.len());
    let mut open: Vec<(String, Vec<String>)> = Vec::new();
    let mut i = 0;
    while let Some(lt) = body[i..].find('<').map(|p| i + p) {
        if lt > at {
            break;
        }
        let end = lt + body[lt..].find('>').unwrap();
        let tag = &body[lt + 1..end];
        i = end + 1;
        if let Some(name) = tag.strip_prefix('/') {
            let (top, _) = open.pop().expect("a tag to close");
            assert_eq!(top, name, "{body}");
        } else if !tag.ends_with('/') && !matches!(tag.split(' ').next(), Some("meta" | "br")) {
            let name = tag.split(' ').next().unwrap().to_string();
            let classes = tag
                .split("class=\"")
                .nth(1)
                .map(|c| c.split('"').next().unwrap())
                .unwrap_or("")
                .split(' ')
                .map(str::to_string)
                .collect();
            open.push((name, classes));
        }
    }
    open.into_iter().flat_map(|(_, c)| c).collect()
}

#[tokio::test]
async fn get_page_offers_open_in_clax_on_a_phone() {
    // On a touch screen of phone width the page hides the QR code, which
    // that phone cannot scan, and still offers Open in Clax.
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Tip Calculator").await;
    site.bundle(&owner, &aid, 1).await;
    let (status, _, html) = site.page(&owner, &format!("/a/{aid}/get")).await;
    assert_eq!(status, 200, "{html}");
    let hidden = phone_hidden(&html);
    assert_eq!(hidden, ["scan"], "{html}");
    let qr = html.find("<svg class=\"qr\"").expect("a QR code");
    assert!(classes_around(&html, qr).contains(&"scan".to_string()));
    let open = html
        .find("href=\"clax:get?url=")
        .expect("an Open in Clax link");
    let open = html[..open].rfind("<a ").unwrap();
    let around = classes_around(&html, open);
    assert!(around.contains(&"primary".to_string()), "{around:?}");
    for c in &hidden {
        assert!(!around.contains(c), "Open in Clax is inside .{c}: {html}");
    }
}

#[tokio::test]
async fn get_page_has_qr_open_and_download() {
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Tip Calculator").await;
    site.bundle(&owner, &aid, 3).await;
    site.link(&owner, &aid, Some("viewer")).await;
    for b in [&owner, &Browser::default()] {
        let (status, mime, html) = site.page(b, &format!("/a/{aid}/get")).await;
        assert_eq!(
            (status, mime.as_str()),
            (200, "text/html; charset=utf-8"),
            "{html}"
        );
        assert!(html.contains("<h1>Tip Calculator</h1>"), "{html}");
        assert!(html.contains("v3 · by Ada"), "{html}");
        // The QR code is of this page's own link.
        let get = format!("{}/a/{aid}/get", site.origin);
        assert!(html.contains(&qr_path(&get)), "{html}");
        assert!(html.contains("<svg"), "{html}");
        // Open in Clax: the `clax:` form the app takes, of the artifact's
        // URL.
        let artifact = format!("{}/a/{aid}", site.origin);
        let encoded: String = url::form_urlencoded::byte_serialize(artifact.as_bytes()).collect();
        assert!(
            html.contains(&format!("href=\"clax:get?url={encoded}\">Open in Clax</a>")),
            "{html}"
        );
        assert!(
            encoded.starts_with("http%3A%2F%2Flocalhost%3A"),
            "{encoded}"
        );
        assert!(
            html.contains(&format!(
                "href=\"/api/artifacts/{aid}/versions/1/bundle\" download>Download .clax</a>"
            )),
            "{html}"
        );
        assert!(
            html.contains("href=\"https://apps.apple.com/app/clax/id6700000000\""),
            "{html}"
        );
        assert!(
            html.contains(
                "href=\"https://play.google.com/store/apps/details?id=dev.empathic.clax\""
            ),
            "{html}"
        );
        assert!(html.contains(&format!("href=\"/a/{aid}\"")), "{html}");
        // The legal footer, and no script.
        assert!(
            html.contains("href=\"/terms\">Terms of Service</a>"),
            "{html}"
        );
        assert!(html.contains("prefers-color-scheme:dark"), "{html}");
        assert!(!html.contains("<script"), "{html}");
    }
    // The page follows the newest bundle.
    let r = site
        .ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<h1>2</h1>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    site.bundle_on(&owner, &aid, 2, 4).await;
    let (_, _, html) = site.page(&owner, &format!("/a/{aid}/get")).await;
    assert!(html.contains("v4 · by Ada"), "{html}");
    assert!(
        html.contains(&format!(
            "href=\"/api/artifacts/{aid}/versions/2/bundle\" download>"
        )),
        "{html}"
    );
    // Without store pages configured, none is offered.
    let bare = Site::new(ClaxApp::default()).await;
    let owner = bare.owner("Ada").await;
    let aid = bare.artifact("Tip Calculator").await;
    bare.bundle(&owner, &aid, 1).await;
    let (status, _, html) = bare.page(&owner, &format!("/a/{aid}/get")).await;
    assert_eq!(status, 200);
    assert!(
        !html.contains("apps.apple.com") && !html.contains("play.google.com"),
        "{html}"
    );
    assert!(html.contains("Download .clax"), "{html}");
}

#[tokio::test]
async fn get_page_escapes_markup_in_the_title_and_names() {
    let site = Site::new(configured()).await;
    let owner = site.owner("<b>Ada</b> & \"co\"").await;
    let aid = site
        .artifact("<script>alert(1)</script> & <i>tips</i>")
        .await;
    site.bundle(&owner, &aid, 1).await;
    let (status, _, html) = site.page(&owner, &format!("/a/{aid}/get")).await;
    assert_eq!(status, 200, "{html}");
    assert!(
        !html.contains("<script>") && !html.contains("<i>") && !html.contains("<b>"),
        "{html}"
    );
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &lt;i&gt;tips&lt;/i&gt;"),
        "{html}"
    );
    assert!(
        html.contains("by &lt;b&gt;Ada&lt;/b&gt; &amp; &quot;co&quot;"),
        "{html}"
    );
}

#[tokio::test]
async fn get_page_404_without_bundle() {
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Notes").await;
    site.link(&owner, &aid, Some("viewer")).await;
    for b in [&owner, &Browser::default()] {
        let (status, mime, html) = site.page(b, &format!("/a/{aid}/get")).await;
        assert_eq!(
            (status, mime.as_str()),
            (404, "text/html; charset=utf-8"),
            "{html}"
        );
        assert!(html.contains("This page isn’t an app yet."), "{html}");
        assert!(
            !html.contains("<svg") && !html.contains("Download .clax"),
            "{html}"
        );
        assert!(html.contains(&format!("href=\"/a/{aid}\"")), "{html}");
    }
}

#[tokio::test]
async fn get_page_of_a_page_with_its_own_get_file_goes_to_that_file() {
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let no_follow = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (file, to) in [("get", "/v/1/get"), ("get/index.html", "/get/index.html")] {
        let aid = site
            .ts
            .publish(
                "Guide",
                &[
                    ("index.html", "<a href=\"get/\">Get started</a>"),
                    (file, "<h1>Start</h1>"),
                ],
            )
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        for path in [format!("/a/{aid}/get"), format!("/a/{aid}/get/")] {
            let mut r = no_follow
                .get(format!("{}{path}", site.ts.base))
                .header(HOST, site.host());
            if let Some(c) = &owner.cookie {
                r = r.header(COOKIE, format!("clax_session={c}"));
            }
            let res = r.send().await.unwrap();
            assert_eq!(res.status().as_u16(), 307, "{file} {path}");
            assert_eq!(
                res.headers()[reqwest::header::LOCATION].to_str().unwrap(),
                format!("/a/{aid}{to}"),
                "{file} {path}"
            );
        }
    }
}

#[tokio::test]
async fn get_page_restricted_needs_sign_in() {
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Secret plan").await;
    site.bundle(&owner, &aid, 1).await;
    let anyone = Browser::default();
    let gus = site.guest("Gus").await;
    // A restricted app's page answers as a page that does not exist does:
    // the sign-in path, and nothing of the app.
    let (status, mime, never) = site.page(&anyone, &format!("/a/{NEVER}/get")).await;
    assert_eq!(
        (status, mime.as_str()),
        (404, "text/html; charset=utf-8"),
        "{never}"
    );
    assert!(never.contains("href=\"/signin\""), "{never}");
    for (b, path) in [
        (&anyone, format!("/a/{aid}/get")),
        (&anyone, format!("/a/{aid}/get/")),
        (&gus, format!("/a/{aid}/get")),
        (&anyone, "/a/not-an-id/get".to_string()),
    ] {
        let (status, _, html) = site.page(b, &path).await;
        assert_eq!((status, &html), (404, &never), "{path}");
    }
    // The owner reads it; anyone with the link does too.
    let (status, _, html) = site.page(&owner, &format!("/a/{aid}/get")).await;
    assert!(status == 200 && html.contains("Secret plan"), "{html}");
    site.link(&owner, &aid, Some("viewer")).await;
    for b in [&anyone, &gus] {
        let (status, _, html) = site.page(b, &format!("/a/{aid}/get")).await;
        assert!(status == 200 && html.contains("Secret plan"), "{html}");
    }
    // Restricted again.
    site.link(&owner, &aid, None).await;
    let (status, _, html) = site.page(&gus, &format!("/a/{aid}/get")).await;
    assert_eq!((status, &html), (404, &never));
}

#[tokio::test]
async fn well_known_served_only_when_configured() {
    const AASA: &str = "/.well-known/apple-app-site-association";
    const LINKS: &str = "/.well-known/assetlinks.json";
    let anyone = Browser::default();
    let bare = Site::new(ClaxApp::default()).await;
    for path in [AASA, LINKS] {
        let (status, _, _) = bare.page(&anyone, path).await;
        assert_eq!(status, 404, "{path}");
    }
    let site = Site::new(configured()).await;
    let (status, mime, body) = site.page(&anyone, AASA).await;
    assert_eq!((status, mime.as_str()), (200, "application/json"), "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"applinks": {"details": [{
            "appIDs": [format!("{TEAM}.{APP_ID}")],
            "components": [{"/": "/a/????????????/get"}],
        }]}})
    );
    let (status, mime, body) = site.page(&anyone, LINKS).await;
    assert_eq!((status, mime.as_str()), (200, "application/json"), "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!([{
            "relation": ["delegate_permission/common.handle_all_urls"],
            "target": {
                "namespace": "android_app",
                "package_name": APP_ID,
                "sha256_cert_fingerprints": [fingerprint()],
            },
        }])
    );
    // Only on the public host.
    let r = site
        .ts
        .client
        .get(format!("{}{AASA}", site.ts.base))
        .header(HOST, "elsewhere.example")
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 200);
    // One platform alone serves its file alone.
    let ios = Site::new(ClaxApp {
        android: None,
        ..configured()
    })
    .await;
    assert_eq!(ios.page(&anyone, AASA).await.0, 200);
    assert_eq!(ios.page(&anyone, LINKS).await.0, 404);
}

#[tokio::test]
async fn well_known_not_on_a_site_host() {
    // Content is served on hosts of its own, each artifact's site host.
    // The app links are the public host's alone.
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_hosted_with(clock, Some(SECRET), |s| {
        s.clax_app = Arc::new(configured());
    })
    .await;
    let port = ts.addr.port();
    let aid = ts
        .publish("Tip Calculator", &[("index.html", "<h1>x</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    for path in [
        "/.well-known/apple-app-site-association",
        "/.well-known/assetlinks.json",
    ] {
        let at = |host: String| {
            ts.client
                .get(format!("{}{path}", ts.base))
                .header(HOST, host)
                .send()
        };
        assert_eq!(
            at(format!("localhost:{port}")).await.unwrap().status(),
            200,
            "{path}"
        );
        let host = ts.site_host(&aid);
        assert_ne!(
            at(host.clone()).await.unwrap().status(),
            200,
            "{host}{path}"
        );
    }
}

#[tokio::test]
async fn route_cases_reserve_get() {
    // The shell and the daemon read `/a/<id>/get` as the Get in Clax page,
    // not a file named `get`, which only the versioned form reaches.
    let cases: Value =
        serde_json::from_str(include_str!("../../../web/shell/src/route-cases.json")).unwrap();
    let route = |p: &str| {
        cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == p)
            .map(|c| c["route"].clone())
    };
    let get = json!({"kind": "get", "id": NEVER});
    assert_eq!(route(&format!("/a/{NEVER}/get")), Some(get.clone()));
    assert_eq!(route(&format!("/a/{NEVER}/get/")), Some(get));
    assert_eq!(
        route(&format!("/a/{NEVER}/v/2/get")),
        Some(json!({"kind": "artifact", "id": NEVER, "version": 2, "file": "get"}))
    );
    // The daemon serves them so.
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Tip Calculator").await;
    site.bundle(&owner, &aid, 1).await;
    for path in [format!("/a/{aid}/get"), format!("/a/{aid}/get/")] {
        let (status, _, html) = site.page(&owner, &path).await;
        assert!(
            status == 200 && html.contains("Download .clax"),
            "{path}: {html}"
        );
    }
    let (status, _, html) = site.page(&owner, &format!("/a/{aid}/v/1/get")).await;
    assert!(
        status == 200 && html.contains("id=\"clax-boot\"") && !html.contains("Download .clax"),
        "{html}"
    );
}

#[tokio::test]
async fn get_page_on_a_local_daemon_offers_download_alone() {
    // A daemon that is not hosted names its own http:// origin, which the
    // app does not take and a phone cannot reach: no QR code and no Open
    // in Clax, only Download .clax.
    let ts = TestServer::spawn().await;
    let aid = ts
        .publish("Tip Calculator", &[("index.html", "<h1>x</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/versions/1"))
        .await
        .json()
        .await
        .unwrap();
    let content = v["version"]["content_sha256"].as_str().unwrap();
    let r = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{aid}/versions/1/bundle", ts.base)),
        )
        .header("content-type", MIME)
        .header("clax-bundle-lineage", LINEAGE)
        .header("clax-bundle-version", "2")
        .header("clax-bundle-content-sha256", content)
        .body(b"PK\x03\x04 a bundle".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = ts.get_authed(&format!("/a/{aid}/get")).await;
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("<h1>Tip Calculator</h1>"), "{html}");
    assert!(
        !html.contains("<svg") && !html.contains("class=\"scan\""),
        "{html}"
    );
    assert!(
        !html.contains("clax:get") && !html.contains("Open in Clax"),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            "<a class=\"btn primary\" href=\"/api/artifacts/{aid}/versions/1/bundle\" download>Download .clax</a>"
        )),
        "{html}"
    );
}

#[tokio::test]
async fn get_page_refuses_the_owners_agent_as_the_contract_says() {
    // docs/contract.md: GET /a/{aid}/get answers the owner's agents 403
    // not_for_agents, on a hosted Clax as on a local daemon, whether or
    // not the page exists.
    let site = Site::new(configured()).await;
    let owner = site.owner("Ada").await;
    let aid = site.artifact("Tip Calculator").await;
    site.bundle(&owner, &aid, 1).await;
    for path in [
        format!("/a/{aid}/get"),
        format!("/a/{aid}/get/"),
        format!("/a/{NEVER}/get"),
    ] {
        let r = site
            .ts
            .authed(site.req(&Browser::default(), reqwest::Method::GET, &path))
            .header("X-Clax-Session", "s1")
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let body: Value = r.json().await.unwrap();
        assert_eq!(
            (status, body["error"]["code"].as_str()),
            (403, Some("not_for_agents")),
            "{path}: {body}"
        );
    }
    // The owner by hand, with the token alone, still gets the page.
    let r = site
        .ts
        .authed(site.req(
            &Browser::default(),
            reqwest::Method::GET,
            &format!("/a/{aid}/get"),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("Download .clax"));
    // A local daemon answers the same.
    let ts = TestServer::spawn().await;
    let aid = ts.publish("Notes", &[("index.html", "<h1>x</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(ts.client.get(format!("{}/a/{aid}/get", ts.base)))
        .header("X-Clax-Session", "s1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_for_agents", "{body}");
}
