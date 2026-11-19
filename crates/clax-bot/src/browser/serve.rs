//! Serving a clax to the browser from its synthetic origin (spec §6):
//! `https://<artifact ID>.clax-test.invalid/`, every request paused in
//! the CDP `Fetch` domain and answered here ([`Site::answer`]), so a
//! private artifact, or a daemon on `localhost`, is tested from a browser
//! that cannot reach either.
//!
//! # What the origin serves
//!
//! The layout mirrors a site (`docs/contract.md`, "Sites"), checked in
//! this order:
//!
//! 1. A request to any other origin is failed
//!    ([`Decision::Fail`], why [`WHY_ORIGIN`]): navigations, scripts from a
//!    CDN, beacons, all of them. `.invalid` never resolves (RFC 6761), so a
//!    request that escaped interception fails too.
//! 2. A service worker's script (`Service-Worker: script`), of any path:
//!    an empty script.
//! 3. `/_clax/bridge.js` and `/_clax/bridge/…`: the host's own files
//!    ([`BrowserSource::clax_file`]).
//! 4. `/_clax/blob/<asset ID>`: that asset of the artifact.
//! 5. `/_clax/api/…`: a `GET` (or `HEAD`) is the host's read-only answer
//!    as the run's person ([`BrowserSource::site_api`], given the path and
//!    query); any other method is refused, 403 `read_only_test`, so a test
//!    never changes a person's documents.
//! 6. Any other `/_clax/` path: 404.
//! 7. `/` is `index.html`; any other path, percent-decoded, is that file
//!    of the target ([`Pages`]): the draft's, edits not yet committed
//!    included, or the version's. `index.html`, and any `text/html` file
//!    in UTF-8, is wrapped as the site wraps it
//!    ([`clax_core::wrap::wrap_page`]), the bridge tag naming the target's
//!    version (a draft's base version). A file the target lacks: 404.
//!
//! Every answer carries the site's headers
//! ([`BrowserSource::site_headers`], its content security policy among
//! them), so the page is held to exactly the policy it lives under; an
//! answer's own `Content-Type` comes first. An answer of 400 or more is
//! one the result lists as failed ([`Decision::failed`]).

use super::{BrowserSource, Served};
use crate::draft::DraftEntry;
use clax_core::ArtifactId;
use clax_core::model::CONTRACT_VERSION;
use clax_core::wrap::wrap_page;
use clax_store::Store;
use serde_json::Value;
use std::collections::BTreeMap;

/// The domain under which each artifact has its synthetic origin.
pub const TEST_DOMAIN: &str = "clax-test.invalid";

/// Why a request to another origin was failed.
pub const WHY_ORIGIN: &str = "origin";

/// The code of a write to the page runtime's API during a test.
pub const CODE_READ_ONLY_TEST: &str = "read_only_test";

/// The synthetic origin of one artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub artifact: ArtifactId,
    /// `<artifact ID>.clax-test.invalid`.
    pub host: String,
}

impl Origin {
    #[must_use]
    pub fn new(artifact: &ArtifactId) -> Origin {
        Origin {
            artifact: artifact.clone(),
            host: format!("{}.{TEST_DOMAIN}", artifact.as_str()),
        }
    }

    /// The URL of `path`, a path of the artifact (`index.html`, `a/b.css`,
    /// with or without a leading `/`): `index.html` is `/`.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        if path.is_empty() || path == clax_core::publish::INDEX {
            format!("https://{}/", self.host)
        } else {
            format!("https://{}/{path}", self.host)
        }
    }

    /// The path and query of `url` when it is of this origin.
    #[must_use]
    pub fn path_of(&self, url: &str) -> Option<(String, Option<String>)> {
        let u = url::Url::parse(url).ok()?;
        (u.scheme() == "https"
            && u.host_str() == Some(self.host.as_str())
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none())
        .then(|| (u.path().to_string(), u.query().map(str::to_string)))
    }
}

/// The files a test loads.
#[derive(Clone, Debug)]
pub enum Pages {
    /// The run's draft, as the session holds it (edits not yet committed
    /// included), and the version it is based on.
    Draft(BTreeMap<String, DraftEntry>, u32),
    /// A published version of the artifact.
    Version(ArtifactId, u32),
}

impl Pages {
    /// The version a wrapped page's bridge tag names.
    #[must_use]
    pub fn version(&self) -> u32 {
        match self {
            Pages::Draft(_, n) | Pages::Version(_, n) => *n,
        }
    }

    /// Every saved test file of the target
    /// ([`super::opts::is_test_file`]: `tests/<name>.test.js`), in path
    /// order.
    ///
    /// # Errors
    /// In words: the version is gone, or the store failed.
    pub async fn test_files(&self, store: &Store) -> Result<Vec<String>, String> {
        let saved = |paths: &mut dyn Iterator<Item = &String>| -> Vec<String> {
            paths
                .filter(|p| super::opts::is_test_file(p))
                .cloned()
                .collect()
        };
        match self {
            Pages::Draft(files, _) => Ok(saved(&mut files.keys())),
            Pages::Version(aid, n) => match store.get_version(aid, *n) {
                Ok(Some(v)) => Ok(saved(&mut v.files.keys())),
                Ok(None) => Err(format!("the artifact has no version {n}")),
                Err(e) => Err(e.to_string()),
            },
        }
    }
}

/// One request the browser paused (`Fetch.requestPaused`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paused {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    /// CDP's `resourceType`: `Document`, `Script`, `Fetch`, …
    pub resource_type: String,
}

impl Paused {
    /// The request of a `Fetch.requestPaused` event's parameters; `None`
    /// without a request ID or a URL.
    #[must_use]
    pub fn of(params: &Value) -> Option<Paused> {
        let req = params.get("request")?;
        let headers = req
            .get("headers")
            .and_then(Value::as_object)
            .map(|h| {
                h.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Some(Paused {
            request_id: params.get("requestId")?.as_str()?.to_string(),
            url: req.get("url")?.as_str()?.to_string(),
            method: req
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_string(),
            headers,
            resource_type: params
                .get("resourceType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// The value of header `name`, whatever its case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What to do with a paused request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Answer it with this (`Fetch.fulfillRequest`).
    Fulfill(Served),
    /// Fail it (`Fetch.failRequest`, `BlockedByClient`), listed as
    /// blocked with `why`.
    Fail { why: &'static str },
}

impl Decision {
    /// The status of an answer the result lists as failed: 400 or more.
    #[must_use]
    pub fn failed(&self) -> Option<u16> {
        match self {
            Decision::Fulfill(s) if s.status >= 400 => Some(s.status),
            _ => None,
        }
    }
}

/// Whether `path` (as the page sent it, percent-encoded) spells a `/`,
/// `\` or `.` as an escape.
fn encodes_a_separator(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    ["%2f", "%5c", "%2e"].iter().any(|e| p.contains(e))
}

/// A clax's synthetic origin and the files it serves (see the module
/// docs).
#[derive(Clone, Debug)]
pub struct Site {
    pub origin: Origin,
    pub pages: Pages,
}

impl Site {
    #[must_use]
    pub fn new(artifact: &ArtifactId, pages: Pages) -> Site {
        Site {
            origin: Origin::new(artifact),
            pages,
        }
    }

    /// The answer to `req`, made with `source`'s files and API and
    /// `store`'s versions and assets, `person`'s API reads (see the
    /// module docs).
    pub async fn answer(
        &self,
        req: &Paused,
        source: &dyn BrowserSource,
        store: &Store,
        person: &str,
    ) -> Decision {
        let Some((path, query)) = self.origin.path_of(&req.url) else {
            return Decision::Fail { why: WHY_ORIGIN };
        };
        let artifact = self.origin.artifact.as_str();
        let own = if req
            .header("service-worker")
            .is_some_and(|v| v.eq_ignore_ascii_case("script"))
        {
            plain(200, "text/javascript", Vec::new())
        } else if path.starts_with("/_clax/") && encodes_a_separator(&path) {
            // A host routing the path after decoding it could read `%2F`
            // as a segment and leave the bridge's files or the artifact's
            // API: no Clax path holds one.
            plain(404, TEXT, b"not found".to_vec())
        } else if path == BRIDGE_PATH || path.starts_with(BRIDGE_PARTS) {
            source
                .clax_file(&path)
                .unwrap_or_else(|| plain(404, TEXT, b"not found".to_vec()))
        } else if let Some(asset) = path.strip_prefix(BLOB_PREFIX) {
            self.asset(store, asset).await
        } else if path.starts_with(API_PREFIX) {
            if req.method.eq_ignore_ascii_case("GET") || req.method.eq_ignore_ascii_case("HEAD") {
                let path_and_query = match &query {
                    Some(q) => format!("{path}?{q}"),
                    None => path.clone(),
                };
                source.site_api(person, artifact, &path_and_query).await
            } else {
                plain(
                    403,
                    "application/json",
                    serde_json::json!({"error": {
                        "code": CODE_READ_ONLY_TEST,
                        "message": "the page runtime's API is read-only during a browser test",
                    }})
                    .to_string()
                    .into_bytes(),
                )
            }
        } else if path.starts_with("/_clax/") {
            plain(404, TEXT, b"not found".to_vec())
        } else {
            self.file(store, &path).await
        };
        Decision::Fulfill(with_site_headers(own, source.site_headers(artifact)))
    }

    /// Asset `asset` of the artifact, or 404.
    async fn asset(&self, store: &Store, asset: &str) -> Served {
        match store.read_asset_async(&self.origin.artifact, asset).await {
            Ok(Some((row, bytes))) => plain(200, &row.content_type, bytes),
            Ok(None) => plain(404, TEXT, b"not found".to_vec()),
            Err(e) => plain(500, TEXT, e.to_string().into_bytes()),
        }
    }

    /// The target's file at `path` (`/` is `index.html`), wrapped when it
    /// is a page; 404 when the target lacks it.
    async fn file(&self, store: &Store, path: &str) -> Served {
        let Some(rel) = decode(path.trim_start_matches('/')) else {
            return plain(404, TEXT, b"not found".to_vec());
        };
        let rel = if rel.is_empty() {
            clax_core::publish::INDEX.to_string()
        } else {
            rel
        };
        let found = match &self.pages {
            Pages::Draft(files, _) => files
                .get(&rel)
                .map(|e| (e.content.clone(), e.content_type.clone())),
            Pages::Version(aid, n) => match store.read_file_async(aid, *n, &rel).await {
                Ok(found) => found.map(|(bytes, meta)| (bytes, meta.content_type)),
                Err(e) => return plain(500, TEXT, e.to_string().into_bytes()),
            },
        };
        let Some((bytes, content_type)) = found else {
            return plain(404, TEXT, b"not found".to_vec());
        };
        if rel != clax_core::publish::INDEX && !is_html(&content_type) {
            return plain(200, &content_type, bytes);
        }
        match String::from_utf8(bytes) {
            Ok(page) => plain(
                200,
                "text/html; charset=utf-8",
                wrap_page(
                    &page,
                    self.origin.artifact.as_str(),
                    self.pages.version(),
                    CONTRACT_VERSION,
                    &rel,
                    "",
                )
                .into_bytes(),
            ),
            Err(e) => plain(200, &content_type, e.into_bytes()),
        }
    }
}

/// Where the host serves the bridge.
const BRIDGE_PATH: &str = clax_core::wrap::BRIDGE_PATH;
/// Where the host serves the bridge's parts.
const BRIDGE_PARTS: &str = "/_clax/bridge/";
const BLOB_PREFIX: &str = "/_clax/blob/";
const API_PREFIX: &str = "/_clax/api/";
const TEXT: &str = "text/plain; charset=utf-8";

fn plain(status: u16, content_type: &str, body: Vec<u8>) -> Served {
    Served {
        status,
        headers: vec![("Content-Type".into(), content_type.into())],
        body,
    }
}

/// `own` with the site's `headers` after its own, but for a header it
/// already has.
fn with_site_headers(mut own: Served, headers: Vec<(String, String)>) -> Served {
    for (k, v) in headers {
        if !own.headers.iter().any(|(o, _)| o.eq_ignore_ascii_case(&k)) {
            own.headers.push((k, v));
        }
    }
    own
}

/// Whether a content type is `text/html` (parameters ignored).
fn is_html(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/html")
}

/// `s` percent-decoded, when that is UTF-8 and every escape is whole.
fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::browser::fake::FakeBrowserSource;
    use clax_core::audit::AuditCtx;
    use serde_json::json;

    const AID: &str = "abcdefghjkmn";

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    fn entry(content: &str, content_type: &str) -> DraftEntry {
        DraftEntry {
            content: content.as_bytes().to_vec(),
            content_type: content_type.into(),
        }
    }

    fn draft_site() -> Site {
        let mut files = BTreeMap::new();
        files.insert(
            "index.html".to_string(),
            entry("<h1>Hello</h1>", "text/html"),
        );
        files.insert(
            "app.css".to_string(),
            entry("h1{color:red}", "text/css; charset=utf-8"),
        );
        files.insert(
            "my page.html".to_string(),
            entry("<!doctype html><p>two</p>", "text/html"),
        );
        Site::new(&ArtifactId::parse(AID).unwrap(), Pages::Draft(files, 3))
    }

    fn get(url: &str) -> Paused {
        Paused::of(&json!({
            "requestId": "R1",
            "request": {"url": url, "method": "GET", "headers": {}},
            "resourceType": "Document",
        }))
        .unwrap()
    }

    fn with(method: &str, url: &str, headers: Value, ty: &str) -> Paused {
        Paused::of(&json!({
            "requestId": "R2",
            "request": {"url": url, "method": method, "headers": headers},
            "resourceType": ty,
        }))
        .unwrap()
    }

    fn served(d: Decision) -> Served {
        match d {
            Decision::Fulfill(s) => s,
            Decision::Fail { why } => panic!("failed: {why}"),
        }
    }

    fn header<'a>(s: &'a Served, name: &str) -> Option<&'a str> {
        s.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(s: &Served) -> String {
        String::from_utf8(s.body.clone()).unwrap()
    }

    #[test]
    fn the_origin_is_the_artifacts_under_clax_test_invalid() {
        let o = Origin::new(&ArtifactId::parse(AID).unwrap());
        assert_eq!(o.host, "abcdefghjkmn.clax-test.invalid");
        assert_eq!(
            o.url("index.html"),
            "https://abcdefghjkmn.clax-test.invalid/"
        );
        assert_eq!(
            o.url("/a/b.css"),
            "https://abcdefghjkmn.clax-test.invalid/a/b.css"
        );
        assert_eq!(
            o.path_of("https://abcdefghjkmn.clax-test.invalid/a?x=1"),
            Some(("/a".into(), Some("x=1".into())))
        );
        for other in [
            "http://abcdefghjkmn.clax-test.invalid/",
            "https://abcdefghjkmn.clax-test.invalid:8443/",
            "https://user@abcdefghjkmn.clax-test.invalid/",
            "https://zzzzzzzzzzzz.clax-test.invalid/",
            "https://abcdefghjkmn.clax-test.invalid.evil.example/",
        ] {
            assert_eq!(o.path_of(other), None, "{other}");
        }
    }

    #[tokio::test]
    async fn the_index_is_served_wrapped_with_the_bridge() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        let site = draft_site();
        let s = served(
            site.answer(&get(&site.origin.url("/")), &src, &st, "p")
                .await,
        );
        assert_eq!(s.status, 200);
        assert_eq!(header(&s, "content-type"), Some("text/html; charset=utf-8"));
        let page = text(&s);
        assert!(page.starts_with("<!doctype html>"), "{page}");
        assert!(
            page.contains(
                "<script src=\"/_clax/bridge.js\" data-artifact=\"abcdefghjkmn\" data-version=\"3\""
            ),
            "{page}"
        );
        assert!(page.contains("data-file=\"index.html\""), "{page}");
        assert!(page.contains("<h1>Hello</h1>"), "{page}");
        // Another HTML file, its name percent-encoded, is wrapped too,
        // naming itself.
        let s = served(
            site.answer(&get(&site.origin.url("my%20page.html")), &src, &st, "p")
                .await,
        );
        assert!(text(&s).contains("data-file=\"my page.html\""));
        assert!(text(&s).contains("<p>two</p>"));
        // The bridge itself is the host's.
        src.clax_file(
            "/_clax/bridge.js",
            Served {
                status: 200,
                headers: vec![("Content-Type".into(), "text/javascript".into())],
                body: b"/*bridge*/".to_vec(),
            },
        );
        let s = served(
            site.answer(
                &get(&site.origin.url("_clax/bridge.js?v=abc")),
                &src,
                &st,
                "p",
            )
            .await,
        );
        assert_eq!(text(&s), "/*bridge*/");
        assert_eq!(header(&s, "content-type"), Some("text/javascript"));
        let s = served(
            site.answer(
                &get(&site.origin.url("_clax/bridge/harness-1a2b.js")),
                &src,
                &st,
                "p",
            )
            .await,
        );
        assert_eq!(s.status, 404, "a part the host lacks");
    }

    #[tokio::test]
    async fn a_draft_file_is_served_with_its_content_type() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        let site = draft_site();
        let d = site
            .answer(&get(&site.origin.url("app.css")), &src, &st, "p")
            .await;
        assert_eq!(d.failed(), None);
        let s = served(d);
        assert_eq!(s.status, 200);
        assert_eq!(header(&s, "content-type"), Some("text/css; charset=utf-8"));
        assert_eq!(text(&s), "h1{color:red}");
    }

    #[tokio::test]
    async fn a_version_file_and_an_asset_are_read_from_the_store() {
        let (_d, st) = store();
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "title": "Report",
            "files": {
                "index.html": {"content": "<p>v1</p>", "encoding": "utf8"},
                "data.json": {"content": "{\"a\":1}", "encoding": "utf8"}
            }
        }))
        .unwrap();
        let aid = st
            .create_artifact(
                &AuditCtx::DAEMON,
                clax_core::publish::validate(req).unwrap(),
                None,
            )
            .unwrap()
            .0
            .id;
        let aid = ArtifactId::parse(&aid).unwrap();
        let asset = st
            .add_asset(
                &AuditCtx::DAEMON,
                &aid,
                "image/png",
                crate::browser::fake::ONE_PIXEL_PNG,
            )
            .unwrap();
        let src = FakeBrowserSource::new();
        let site = Site::new(&aid, Pages::Version(aid.clone(), 1));
        let s = served(
            site.answer(&get(&site.origin.url("/")), &src, &st, "p")
                .await,
        );
        assert!(text(&s).contains("<p>v1</p>") && text(&s).contains("data-version=\"1\""));
        let s = served(
            site.answer(&get(&site.origin.url("data.json")), &src, &st, "p")
                .await,
        );
        assert_eq!(text(&s), "{\"a\":1}");
        assert!(
            header(&s, "content-type")
                .unwrap()
                .starts_with("application/json")
        );
        let s = served(
            site.answer(
                &get(&site.origin.url(&format!("_clax/blob/{}", asset.id))),
                &src,
                &st,
                "p",
            )
            .await,
        );
        assert_eq!(s.status, 200);
        assert_eq!(header(&s, "content-type"), Some("image/png"));
        assert_eq!(s.body, crate::browser::fake::ONE_PIXEL_PNG);
        let d = site
            .answer(&get(&site.origin.url("_clax/blob/nope")), &src, &st, "p")
            .await;
        assert_eq!(d.failed(), Some(404));
    }

    /// `{tests: true}` runs every `tests/<name>.test.js` of the target
    /// (a name may hold `/`, as the inspect view's may), in path order:
    /// the draft's (edits not yet committed included) or the version's,
    /// and nothing else.
    #[tokio::test]
    async fn tests_true_lists_the_targets_test_files() {
        let (_d, st) = store();
        let mut files = BTreeMap::new();
        for path in [
            "index.html",
            "tests/b.test.js",
            "tests/a.test.js",
            "tests/helper.js",
            "tests/deep/c.test.js",
            "a.test.js",
        ] {
            files.insert(path.to_string(), entry("x", "text/javascript"));
        }
        let draft = Pages::Draft(files, 1);
        assert_eq!(
            draft.test_files(&st).await.unwrap(),
            ["tests/a.test.js", "tests/b.test.js", "tests/deep/c.test.js"]
        );
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "title": "Report",
            "files": {
                "index.html": {"content": "<p>v1</p>", "encoding": "utf8"},
                "tests/form.test.js": {"content": "test('x', () => {});", "encoding": "utf8"},
                "tests/util.js": {"content": "export {};", "encoding": "utf8"}
            }
        }))
        .unwrap();
        let aid = st
            .create_artifact(
                &AuditCtx::DAEMON,
                clax_core::publish::validate(req).unwrap(),
                None,
            )
            .unwrap()
            .0
            .id;
        let aid = ArtifactId::parse(&aid).unwrap();
        let version = Pages::Version(aid.clone(), 1);
        assert_eq!(
            version.test_files(&st).await.unwrap(),
            ["tests/form.test.js"]
        );
        assert!(Pages::Version(aid, 7).test_files(&st).await.is_err());
    }

    #[tokio::test]
    async fn a_missing_file_is_a_404_listed_as_failed() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        let site = draft_site();
        for path in ["missing.js", "_clax/other", "_clax/v/1/index.html"] {
            let d = site
                .answer(&get(&site.origin.url(path)), &src, &st, "p")
                .await;
            assert_eq!(d.failed(), Some(404), "{path}");
        }
    }

    #[tokio::test]
    async fn api_gets_go_to_the_host_and_writes_are_refused() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        src.site_api(
            "/_clax/api/db/notes?limit=5",
            Served {
                status: 200,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: br#"{"docs":[]}"#.to_vec(),
            },
        );
        let site = draft_site();
        let url = site.origin.url("_clax/api/db/notes?limit=5");
        let s = served(
            site.answer(
                &with("GET", &url, json!({}), "Fetch"),
                &src,
                &st,
                "person-1",
            )
            .await,
        );
        assert_eq!((s.status, text(&s).as_str()), (200, r#"{"docs":[]}"#));
        assert_eq!(
            src.api_calls(),
            [(
                "person-1".to_string(),
                AID.to_string(),
                "/_clax/api/db/notes?limit=5".to_string()
            )]
        );
        // A path that spells a separator as an escape is not asked of
        // the host.
        for escaped in [
            "_clax/api/db/x%2F..%2F..%2Fapi",
            "_clax/bridge/..%2f..%2Fsecret",
            "_clax/api/db/a%2Eb%5C",
        ] {
            let d = site
                .answer(
                    &with("GET", &site.origin.url(escaped), json!({}), "Fetch"),
                    &src,
                    &st,
                    "person-1",
                )
                .await;
            assert_eq!(d.failed(), Some(404), "{escaped}");
        }
        assert_eq!(src.api_calls().len(), 1);
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let d = site
                .answer(
                    &with(method, &url, json!({}), "Fetch"),
                    &src,
                    &st,
                    "person-1",
                )
                .await;
            assert_eq!(d.failed(), Some(403), "{method}");
            let s = served(d);
            let body: Value = serde_json::from_slice(&s.body).unwrap();
            assert_eq!(body["error"]["code"], CODE_READ_ONLY_TEST);
            assert_eq!(header(&s, "content-type"), Some("application/json"));
        }
        assert_eq!(src.api_calls().len(), 1, "no write reached the host");
    }

    #[tokio::test]
    async fn every_answer_carries_the_site_headers() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        src.set_site_headers(vec![
            (
                "Content-Security-Policy".into(),
                "default-src 'self'; connect-src 'self'".into(),
            ),
            ("X-Content-Type-Options".into(), "nosniff".into()),
        ]);
        let site = draft_site();
        let url = |p: &str| site.origin.url(p);
        let reqs = [
            get(&url("/")),
            get(&url("app.css")),
            get(&url("missing.js")),
            get(&url("_clax/bridge.js")),
            get(&url("_clax/nope")),
            with("GET", &url("_clax/api/user"), json!({}), "Fetch"),
            with("POST", &url("_clax/api/db/x"), json!({}), "Fetch"),
            with(
                "GET",
                &url("sw.js"),
                json!({"Service-Worker": "script"}),
                "Script",
            ),
        ];
        for req in &reqs {
            let s = served(site.answer(req, &src, &st, "p").await);
            assert_eq!(
                header(&s, "content-security-policy"),
                Some("default-src 'self'; connect-src 'self'"),
                "{}",
                req.url
            );
            assert_eq!(
                header(&s, "x-content-type-options"),
                Some("nosniff"),
                "{}",
                req.url
            );
            assert_eq!(
                s.headers
                    .iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .count(),
                1,
                "one content type: {}",
                req.url
            );
        }
        // A service worker's script is an empty script, whatever its path.
        let s = served(site.answer(&reqs[7], &src, &st, "p").await);
        assert_eq!((s.status, s.body.len()), (200, 0));
        assert_eq!(header(&s, "content-type"), Some("text/javascript"));
    }

    #[tokio::test]
    async fn a_navigation_to_another_origin_is_blocked() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        let site = draft_site();
        let away = with(
            "GET",
            "https://elsewhere.example/?d=secret",
            json!({}),
            "Document",
        );
        assert_eq!(
            site.answer(&away, &src, &st, "p").await,
            Decision::Fail { why: WHY_ORIGIN }
        );
        // Another artifact's synthetic origin is another origin too.
        let other = get("https://zzzzzzzzzzzz.clax-test.invalid/");
        assert_eq!(
            site.answer(&other, &src, &st, "p").await,
            Decision::Fail { why: WHY_ORIGIN }
        );
    }

    #[tokio::test]
    async fn a_cdn_script_is_blocked_by_origin() {
        let (_d, st) = store();
        let src = FakeBrowserSource::new();
        let site = draft_site();
        let cdn = with(
            "GET",
            "https://cdn.jsdelivr.net/npm/react@18.3.1/umd/react.production.min.js",
            json!({}),
            "Script",
        );
        let d = site.answer(&cdn, &src, &st, "p").await;
        assert_eq!(d, Decision::Fail { why: WHY_ORIGIN });
        assert_eq!(d.failed(), None, "blocked, not failed");
        assert!(src.api_calls().is_empty());
    }
}
