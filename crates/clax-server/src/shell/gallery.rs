//! The first-load data the daemon puts into `index.html` for the gallery's
//! pages (`/`, `/inbox`, `/tags`, `/queues…`, `/settings…`, `/people`,
//! `/admin`, `/me`, `/audit`): the answers the gallery would otherwise fetch
//! once its script runs, so its first paint can be its settled layout. The
//! shell reads it with `readGalleryBoot` (`web/shell/src/view/gallery-boot.ts`,
//! whose `GalleryBoot` type is this contract's other half).
//!
//! # The block
//!
//! At the entry's `<!--clax:boot-->` marker ([`super::boot::BOOT_MARK`]), a
//! `<script type="application/json" id="clax-boot">` element
//! ([`super::boot::boot_script`]) holding, escaped by
//! [`super::boot::script_json`]:
//!
//! ```json
//! {"v": 1, "kind": "gallery",
//!  "status": {"viewer": 200, "hosted": 404, "feedback": 200,
//!             "artifacts": 200, "attention": 200},
//!  "viewer": {"viewer": {…}, "may_create": true},
//!  "hosted": null,
//!  "feedback": {"enabled": false, …},
//!  "inbox": true,
//!  "artifacts": {"artifacts": […]},
//!  "attention": {"artifacts": {…}}}
//! ```
//!
//! Each route field ([`Field`]) is the JSON body its route answers this
//! browser: `viewer` `GET /api/viewers/me`, `hosted` `GET /api/hosted/me`,
//! `feedback` `GET /_app/feedback/config`, and on `/` alone (the only page
//! that lists artifacts) `artifacts` `GET /api/artifacts` and `attention`
//! `GET /api/viewers/me/attention`. The route answers it itself: the
//! daemon sends it, in process, through its whole router
//! ([`crate::integrations::instance::Instance::router`]), so every layer and
//! handler (the host rule, the local credential gate, a hosted daemon's gate
//! and a person's rights, the live-page filter) decides it as it decides the
//! shell's own `fetch`. That request is the page load's, as the shell's
//! `fetch` sends it ([`sub_request`]): the load's headers less its
//! `Authorization`, conditional and content-negotiation headers, `Accept:
//! */*`, the `Sec-Fetch-*` of a same-origin `fetch`, and the load's
//! connection and in-process owner mark. So the block never holds more than
//! the page's own requests read.
//!
//! - `status.<field>` is the HTTP status the route answered; the field is
//!   its body when that is `200` with a JSON body, else `null` (`hosted` on
//!   a local daemon: `404`, `null`). A route that did not answer within the
//!   API's timeout ([`clax_http::state::AppState::request_timeout`]),
//!   whose answer could not be read, or that answered a failure that may
//!   not last (`5xx`, `408`, `429`: not [`settled`]) has no status and a
//!   `null` field: the shell asks it itself. A failed field never fails the
//!   page.
//! - `inbox` is whether this browser has an inbox, as `inboxBrowser`
//!   (`web/shell/src/owner.ts`) decides it: `true` when `viewer` names a
//!   connected person who is not revoked, or `GET /api/token` answers `200`
//!   (the browser is the owner's; the token itself never enters the block);
//!   `false` when `GET /api/token` refuses (`401`, `403`) and `viewer` was
//!   answered without such a person; else `null` (not known: the shell asks).
//! - The shell token (`GET /api/token`) is not in the block: it stays a
//!   request of the shell's.
//!
//! The entry is served with no block, its marker removed, when the routes'
//! answers to the load would not be the shell's own ([`answers_as_shell`]):
//! a `Host` the API refuses (a DNS name rebound to this machine), an owner
//! browser session's cookie without its proof (every navigation of a local
//! owner's browser: the shell sends the proof, a navigation cannot), and a
//! load another site started (`Sec-Fetch-Site` `same-site` or `cross-site`,
//! whose answers the routes refuse where the shell's same-origin `fetch`
//! gets them).
//!
//! A cookie a route sets in its answer (`GET /api/viewers/me` reissuing a
//! legacy viewer cookie) is set by the page.

use super::boot::script_json;
use crate::integrations::instance::Instance;
use axum::body::Body;
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri, header};
use clax_http::state::AppState;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// A field of the block answered by one route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// `GET /api/viewers/me`.
    Viewer,
    /// `GET /api/hosted/me`.
    Hosted,
    /// `GET /_app/feedback/config`.
    Feedback,
    /// `GET /api/artifacts`, the gallery's list (`listArtifacts()` in
    /// `web/shell/src/api.ts`).
    Artifacts,
    /// `GET /api/viewers/me/attention` (`getAttention()`).
    Attention,
}

impl Field {
    /// The field's name in the block.
    pub fn name(self) -> &'static str {
        match self {
            Field::Viewer => "viewer",
            Field::Hosted => "hosted",
            Field::Feedback => "feedback",
            Field::Artifacts => "artifacts",
            Field::Attention => "attention",
        }
    }

    /// The route that answers it.
    pub fn path(self) -> &'static str {
        match self {
            Field::Viewer => "/api/viewers/me",
            Field::Hosted => "/api/hosted/me",
            Field::Feedback => "/_app/feedback/config",
            Field::Artifacts => "/api/artifacts",
            Field::Attention => "/api/viewers/me/attention",
        }
    }
}

/// The fields the page at `path` uses: the header's on every gallery page,
/// and the list's on `/` alone.
pub fn fields_for(path: &str) -> &'static [Field] {
    const HEADER: &[Field] = &[Field::Viewer, Field::Hosted, Field::Feedback];
    const LIST: &[Field] = &[
        Field::Viewer,
        Field::Hosted,
        Field::Feedback,
        Field::Artifacts,
        Field::Attention,
    ];
    if path == "/" { LIST } else { HEADER }
}

/// Where the shell learns whether this browser is the owner's.
const TOKEN_PATH: &str = "/api/token";

/// The largest route answer read into the block; a larger one is left to
/// the shell.
const BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Headers of the page load that the shell's `fetch` does not send, or that
/// would change how the route answers rather than what: the load's
/// `Authorization` (the block is a browser's, which navigates with
/// cookies alone), its conditional and range headers (a route's `304`
/// would carry no body), its content negotiation (a compressed or HTML
/// answer), and the navigation's own `Sec-Fetch-*` and upgrade headers.
const DROPPED: &[&str] = &[
    "authorization",
    "accept",
    "accept-encoding",
    "if-none-match",
    "if-modified-since",
    "if-match",
    "if-unmodified-since",
    "if-range",
    "range",
    "content-length",
    "content-type",
    "connection",
    "upgrade",
    "upgrade-insecure-requests",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-dest",
    "sec-fetch-user",
];

/// Whether the routes would answer the page load as they answer the shell's
/// own `fetch` from the page it loads (see the module docs): the API's host
/// rule holds (a hosted daemon's gate checked the host already), it carries
/// no owner browser session's cookie without its proof, and no other site
/// started it.
pub fn answers_as_shell(headers: &HeaderMap, uri: &Uri, extensions: &Extensions) -> bool {
    let host_ok = clax_http::auth::request_host_allowed(headers, uri, extensions)
        || extensions
            .get::<clax_http::hosted::HostedRequest>()
            .is_some();
    let unproven = extensions
        .get::<clax_http::local_trust::UnprovenSession>()
        .is_some()
        && extensions
            .get::<clax_http::local_trust::OwnerSession>()
            .is_none();
    let started_elsewhere = matches!(
        headers.get("sec-fetch-site").map(HeaderValue::to_str),
        Some(Ok("same-site" | "cross-site")) | Some(Err(_))
    );
    host_ok && !unproven && !started_elsewhere
}

/// The request the shell's `fetch` of `path` makes from the page that
/// `headers` and `extensions` loaded (see [`DROPPED`]): its cookies and
/// `Host` with every other header the load carried, `Accept: */*`, and,
/// when the load named its `Sec-Fetch-Site`, a same-origin `fetch`'s
/// `Sec-Fetch-*`. Of the load's extensions only its connection (natively)
/// and an in-process owner mark (an embedding app's,
/// [`clax_http::local_trust::OwnerSession`]) go with it: the router sets the
/// rest again as it routes.
pub fn sub_request(path: &str, headers: &HeaderMap, extensions: &Extensions) -> Request<Body> {
    let mut h = headers.clone();
    for name in DROPPED {
        h.remove(*name);
    }
    h.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    if headers.contains_key("sec-fetch-site") {
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        h.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        h.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
    }
    let mut req = Request::new(Body::empty());
    *req.method_mut() = Method::GET;
    *req.uri_mut() = path.parse().expect("a route path is a URI");
    *req.headers_mut() = h;
    let x = req.extensions_mut();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(c) = extensions.get::<axum::extract::ConnectInfo<clax_http::auth::Conn>>() {
        x.insert(*c);
    }
    if let Some(m) = extensions.get::<clax_http::local_trust::OwnerSession>() {
        x.insert(m.clone());
    }
    req
}

/// One route's answer: its status, its JSON body when that is `200`, and
/// the cookies it set.
#[derive(Debug, Default)]
pub struct Answer {
    pub status: Option<u16>,
    pub body: Value,
    pub cookies: Vec<HeaderValue>,
}

/// Whether a route's status is its answer to this browser, which the
/// shell's own request would get too: a success or a refusal. A failure
/// (`5xx`), the route's own timeout (`408`) and a rate limit (`429`) may
/// answer otherwise when the shell asks again.
pub fn settled(status: StatusCode) -> bool {
    (status.is_success() || status.is_client_error())
        && status != StatusCode::REQUEST_TIMEOUT
        && status != StatusCode::TOO_MANY_REQUESTS
}

/// `path`'s answer to the page load, from `router` within `limit`; no
/// status when it does not answer in time, its body cannot be read, or its
/// status is not [`settled`].
async fn ask(
    router: axum::Router,
    path: &str,
    headers: &HeaderMap,
    extensions: &Extensions,
    limit: Duration,
) -> Answer {
    use tower::ServiceExt as _;
    let req = sub_request(path, headers, extensions);
    let answered = clax_rt::time::timeout(limit, async move {
        let Ok(res) = router.oneshot(req).await;
        let status = res.status();
        let cookies: Vec<HeaderValue> = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .cloned()
            .collect();
        let json = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        let body = if status == StatusCode::OK && json {
            let bytes = axum::body::to_bytes(res.into_body(), BODY_LIMIT)
                .await
                .ok()?;
            serde_json::from_slice::<Value>(&bytes).ok()?
        } else {
            Value::Null
        };
        Some(Answer {
            status: settled(status).then_some(status.as_u16()),
            body,
            cookies,
        })
    })
    .await;
    match answered {
        Ok(Some(a)) => a,
        Ok(None) => {
            tracing::warn!(
                path,
                "gallery page served without a field: unreadable answer"
            );
            Answer::default()
        }
        Err(_) => {
            tracing::warn!(
                path,
                ?limit,
                "gallery page served without a field: timed out"
            );
            Answer::default()
        }
    }
}

/// The block's `inbox` (see the module docs): whether this browser has an
/// inbox, as `inboxBrowser` decides it, from the viewer route's answer and
/// the token route's status.
pub fn inbox(viewer: &Answer, token: Option<u16>) -> Value {
    let v = &viewer.body["viewer"];
    let person = v["person"].as_bool() == Some(true)
        && v.get("person_revoked_at").is_none_or(Value::is_null);
    if person || token == Some(200) {
        return json!(true);
    }
    if viewer.status.is_some() && matches!(token, Some(401 | 403)) {
        return json!(false);
    }
    Value::Null
}

/// The block for the page load at `path` (see the module docs), already
/// escaped for its element, and the cookies its routes set; `None` when
/// the load gets none ([`answers_as_shell`]). The routes are asked at once.
pub async fn assemble(
    s: &AppState,
    instance: &Instance,
    path: &str,
    headers: &HeaderMap,
    uri: &Uri,
    extensions: &Extensions,
) -> Option<(String, Vec<HeaderValue>)> {
    if !answers_as_shell(headers, uri, extensions) {
        return None;
    }
    let router = instance.router();
    let limit = s.request_timeout;
    let fields = fields_for(path);
    let asks = fields
        .iter()
        .map(|f| f.path())
        .chain(std::iter::once(TOKEN_PATH))
        .map(|p| ask(router.clone(), p, headers, extensions, limit));
    let mut answers = futures::future::join_all(asks).await;
    let token = answers.pop().expect("the token route was asked");
    let viewer = fields
        .iter()
        .position(|f| *f == Field::Viewer)
        .expect("every gallery page asks for its viewer");
    let has_inbox = inbox(&answers[viewer], token.status);
    let mut status = Map::new();
    let mut block = Map::new();
    let mut cookies = Vec::new();
    block.insert("v".into(), json!(1));
    block.insert("kind".into(), json!("gallery"));
    for (f, a) in fields.iter().zip(answers) {
        if let Some(code) = a.status {
            status.insert(f.name().into(), json!(code));
        }
        block.insert(f.name().into(), a.body);
        cookies.extend(a.cookies);
    }
    // The token route's cookies too: the shell's own request would set them.
    cookies.extend(token.cookies);
    block.insert("inbox".into(), has_inbox);
    block.insert("status".into(), Value::Object(status));
    Some((script_json(&Value::Object(block)), cookies))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn only_the_gallery_lists_artifacts() {
        let names = |p: &str| fields_for(p).iter().map(|f| f.name()).collect::<Vec<_>>();
        assert_eq!(
            names("/"),
            ["viewer", "hosted", "feedback", "artifacts", "attention"]
        );
        for p in ["/inbox", "/tags", "/queues/q/t", "/settings/you", "/people"] {
            assert_eq!(names(p), ["viewer", "hosted", "feedback"], "{p}");
        }
    }

    #[test]
    fn a_sub_request_is_the_shells_fetch_with_the_loads_cookies() {
        let load = headers(&[
            ("host", "localhost:7481"),
            ("cookie", "clax_viewer=abc"),
            ("authorization", "Bearer secret"),
            ("accept", "text/html"),
            ("accept-encoding", "gzip"),
            ("if-none-match", "\"x\""),
            ("sec-fetch-site", "none"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
            ("sec-fetch-user", "?1"),
            ("upgrade-insecure-requests", "1"),
            ("x-clax-owner-proof", "p"),
        ]);
        let mut ext = Extensions::new();
        ext.insert(clax_http::local_trust::OwnerSession::browser());
        // Any other extension, as the router's matched path, stays behind.
        ext.insert(7u32);
        let req = sub_request("/api/viewers/me", &load, &ext);
        assert_eq!(req.method(), Method::GET);
        assert_eq!(req.uri(), "/api/viewers/me");
        let h = req.headers();
        assert_eq!(h["host"], "localhost:7481");
        assert_eq!(h["cookie"], "clax_viewer=abc");
        assert_eq!(h["x-clax-owner-proof"], "p");
        assert_eq!(h["accept"], "*/*");
        assert_eq!(h["sec-fetch-site"], "same-origin");
        assert_eq!(h["sec-fetch-mode"], "cors");
        assert_eq!(h["sec-fetch-dest"], "empty");
        for gone in [
            "authorization",
            "accept-encoding",
            "if-none-match",
            "sec-fetch-user",
            "upgrade-insecure-requests",
        ] {
            assert!(!h.contains_key(gone), "{gone}");
        }
        assert!(
            req.extensions()
                .get::<clax_http::local_trust::OwnerSession>()
                .is_some()
        );
        assert!(
            req.extensions().get::<u32>().is_none(),
            "the router sets the rest again"
        );
        // A load that names no `Sec-Fetch-Site` (no browser's) gets none.
        let bare = sub_request("/api/x", &headers(&[("host", "h")]), &Extensions::new());
        assert!(!bare.headers().contains_key("sec-fetch-site"));
    }

    #[test]
    fn a_load_another_site_started_or_an_unproven_owner_gets_no_block() {
        let uri: Uri = "/".parse().unwrap();
        let host = |extra: &[(&str, &str)]| {
            let mut h = headers(&[("host", "localhost:7481")]);
            for (k, v) in extra {
                h.insert(
                    axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    HeaderValue::from_str(v).unwrap(),
                );
            }
            h
        };
        let none = Extensions::new();
        assert!(answers_as_shell(&host(&[]), &uri, &none));
        for site in ["none", "same-origin"] {
            assert!(
                answers_as_shell(&host(&[("sec-fetch-site", site)]), &uri, &none),
                "{site}"
            );
        }
        for site in ["same-site", "cross-site"] {
            assert!(
                !answers_as_shell(&host(&[("sec-fetch-site", site)]), &uri, &none),
                "{site}"
            );
        }
        assert!(!answers_as_shell(
            &headers(&[("host", "evil.example:7481")]),
            &uri,
            &none
        ));
    }

    #[test]
    fn only_a_success_or_a_lasting_refusal_is_an_answer() {
        for (code, want) in [
            (200, true),
            (204, true),
            (401, true),
            (403, true),
            (404, true),
            (408, false),
            (429, false),
            (500, false),
            (503, false),
            (302, false),
        ] {
            assert_eq!(settled(StatusCode::from_u16(code).unwrap()), want, "{code}");
        }
    }

    #[test]
    fn the_inbox_is_a_live_persons_or_the_owners_and_unknown_otherwise() {
        let viewer = |v: Value| Answer {
            status: Some(200),
            body: json!({"viewer": v}),
            cookies: vec![],
        };
        let person = viewer(json!({"person": true, "person_revoked_at": null}));
        assert_eq!(inbox(&person, None), json!(true));
        assert_eq!(inbox(&person, Some(401)), json!(true));
        let revoked = viewer(json!({"person": true, "person_revoked_at": "2026-01-01T00:00:00Z"}));
        assert_eq!(inbox(&revoked, Some(401)), json!(false));
        let guest = viewer(json!({"display_name": "Ada"}));
        assert_eq!(inbox(&guest, Some(200)), json!(true), "the owner's");
        assert_eq!(inbox(&guest, Some(403)), json!(false));
        assert_eq!(inbox(&guest, None), Value::Null, "the token unknown");
        assert_eq!(inbox(&guest, Some(500)), Value::Null);
        assert_eq!(
            inbox(&Answer::default(), Some(401)),
            Value::Null,
            "the viewer unknown"
        );
    }
}
