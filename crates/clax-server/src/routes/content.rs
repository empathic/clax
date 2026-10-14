//! Serves published page content: every HTML page (the index and supporting
//! `text/html` files) wrapped with the bridge, other supporting files as stored.

use crate::routes::artifacts::path;
use crate::shell::bridge_version;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use clax_core::model::CONTRACT_VERSION;
use clax_core::model::FileMeta;
use clax_core::publish::INDEX;
use clax_core::wrap::wrap_page_framed_by;
use clax_core::{ArtifactId, CoreError};
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::host::OnArtifactOrigin;
use clax_http::http_cache;
use clax_http::state::AppState;
use std::sync::Arc;

/// Content served on the main origin must not run same-origin with the API.
const SANDBOX: &str = "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";

/// The `Content-Security-Policy` of a content response: who may frame it,
/// and, for an artifact's own content (`lockdown`: its ID; not a live page's
/// snapshot), what it may load. `own` is the daemon's host.
///
/// - On the main origin (sandbox mode): the sandbox, and no
///   `frame-ancestors`. A sandboxed page has an opaque origin, which no
///   `frame-ancestors` source matches, so any such list would also refuse an
///   artifact's page framed by another of its pages. Framed by another site,
///   the page reaches nothing of the viewer's: it runs at an opaque origin,
///   and its bridge talks only to a parent at the shell's origin.
/// - On an artifact origin (`<id>.localhost[:port]`, subdomain mode): framed
///   only by the artifact's own pages (`'self'`) and the shell, at
///   `localhost` or `127.0.0.1` on the same port, the only hosts from which
///   the shell uses subdomain frames.
///
/// The policy is a separate one: a page's own `<meta>` policy still applies
/// in full beside it, and cannot set `frame-ancestors` itself.
pub(crate) fn content_policy(
    on_artifact_origin: bool,
    req: &HeaderMap,
    own: &str,
    lockdown: Option<&str>,
) -> HeaderValue {
    // The rewrite accepted an artifact host only with a numeric port, or none;
    // on the main origin a `Host` that is not plain is the daemon's own.
    let host = plain_host(req, own);
    let port = host
        .rsplit_once(':')
        .filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map(|(_, p)| format!(":{p}"))
        .unwrap_or_default();
    let frame = if !on_artifact_origin {
        SANDBOX.to_string()
    } else {
        format!("frame-ancestors 'self' http://localhost{port} http://127.0.0.1{port}")
    };
    let policy = match lockdown {
        Some(id) => {
            // An artifact host serves only GET and HEAD, of the artifact's
            // content, `/_clax/`, `/_blob/` and `/healthz` (`host.rs`): no
            // API, so `'self'` names nothing but those.
            let (loadable, files) = if on_artifact_origin {
                ("'self'".to_string(), "'self'".to_string())
            } else {
                (
                    format!(
                        "http://{host}/c/{id}/ http://{host}/_clax/bridge.js http://{host}/_clax/bridge/"
                    ),
                    format!("http://{host}/c/{id}/"),
                )
            };
            format!(
                "{frame}; {}",
                load_policy(&loadable, &files, &blob_sources(host, own, &port))
            )
        }
        None => frame,
    };
    HeaderValue::from_str(&policy).unwrap_or_else(|_| HeaderValue::from_static("sandbox"))
}

/// What an artifact's page may load: its own files and the bridge (`own`,
/// source expressions), assets under `/_blob/` on the daemon's hosts
/// (`blobs`), and `data:` and `blob:` URLs. It fetches (`fetch`, XHR,
/// WebSocket, EventSource, beacons, JSON modules) only its own files
/// (`files`) and assets: the network is the `net` capability's, through the
/// shell. No plugins, base URL or form posts. Inline and evaluated scripts
/// and inline styles run, being the page's own.
fn load_policy(own: &str, files: &str, blobs: &str) -> String {
    format!(
        "default-src {own} {blobs} data: blob:; \
         script-src {own} 'unsafe-inline' 'unsafe-eval' data: blob:; \
         style-src {own} 'unsafe-inline' data: blob:; \
         connect-src {files} {blobs}; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// `/_blob/` on each host an asset URL may name: the request's (when plain),
/// the daemon's browser base, and `localhost` and `127.0.0.1` on the
/// request's port.
fn blob_sources(host: &str, own: &str, port: &str) -> String {
    let mut hosts: Vec<String> = Vec::new();
    for h in [
        host.to_string(),
        own.to_string(),
        format!("localhost{port}"),
        format!("127.0.0.1{port}"),
    ] {
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    hosts
        .iter()
        .map(|h| format!("http://{h}/_blob/"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The request's `Host` when it is a plain host and port (letters, digits,
/// `.`, `-`, `:`, `[`, `]`), else `fallback` (the daemon's own host): a
/// crafted `Host` never reaches a policy.
fn plain_host<'a>(req: &'a HeaderMap, fallback: &'a str) -> &'a str {
    let plain = |h: &str| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    };
    req.get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| plain(h))
        .unwrap_or(fallback)
}

/// The host and port in the daemon's browser base URL.
pub(crate) fn own_host(s: &AppState) -> String {
    s.browser_base
        .strip_prefix("http://")
        .unwrap_or(&s.browser_base)
        .trim_end_matches('/')
        .to_string()
}

/// The second policy on a live page's content (spec
/// 2026-10-05-chrome-overlay-design §8.4): of scripts, only Clax's own under
/// `/_clax/` on the request's host run, whatever the snapshot holds; no
/// plugins, base URL, form posts, frames, connections or workers. A `Host`
/// that is not a plain host and port (letters, digits, `.`, `-`, `:`, `[`,
/// `]`) is not trusted: `fallback` (the daemon's own host) is used instead.
fn snapshot_policy(req: &HeaderMap, fallback: &str) -> HeaderValue {
    let host = plain_host(req, fallback);
    HeaderValue::from_str(&format!(
        "script-src http://{host}/_clax/; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'; worker-src 'none'"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("script-src 'none'"))
}

/// Response extension: the content route set this response's policy itself
/// ([`framed_by_shell`]); `host::rewrite_artifact_host` adds the artifact's
/// lockdown policy to every other response for an artifact's content.
#[derive(Clone, Copy)]
pub(crate) struct ContentPolicySet;

/// Sets the content policy of artifact `id`'s content on `res` (`own`: the
/// daemon's host): one that frames it and keeps it to its own files; or, for
/// a `live` page's content, the framing policy and [`snapshot_policy`]
/// beside it, both applying, so a snapshot still loads styles, images and
/// fonts from its site. `Referrer-Policy: no-referrer` keeps the page's URL, which
/// names its artifact ID and the daemon's port, out of the `Referer` of
/// every request the page makes.
fn framed_by_shell(
    mut res: Response,
    origin: &Option<Extension<OnArtifactOrigin>>,
    req: &HeaderMap,
    (id, own): (&str, &str),
    live: bool,
) -> Response {
    let lockdown = (!live).then_some(id);
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        content_policy(origin.is_some(), req, own, lockdown),
    );
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if live {
        res.headers_mut()
            .append(header::CONTENT_SECURITY_POLICY, snapshot_policy(req, own));
    }
    res.extensions_mut().insert(ContentPolicySet);
    res
}

pub async fn redirect_to_slash(
    _d: clax_http::agent::DeniedToGrants,
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Redirect, ApiError> {
    let (aid, n) = path(p)?;
    parse_id(&aid)?;
    Ok(Redirect::permanent(&format!("{n}/")))
}

pub async fn index(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    origin: Option<Extension<OnArtifactOrigin>>,
    req: HeaderMap,
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    // A version's page serves browsers (spec §7.5): agents read its files through the routes the filter covers.
    d.refuse_agents()?;
    let (aid, n) = path(p)?;
    let id = parse_id(&aid)?;
    let own = own_host(&s);
    let live = s.live_ids.contains(id.as_str());
    let aid = id.as_str().to_string();
    let app = (aid.as_str(), own.as_str());
    match lookup(&s, id, n, INDEX.to_string(), true).await? {
        Served::Page(html) => Ok(framed_by_shell(
            http_cache::html(&req, &html),
            &origin,
            &req,
            app,
            live,
        )),
        Served::Raw(..) => Err(ApiError::from(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "index.html is not UTF-8",
        )))),
    }
}

/// Whether a stored content type is `text/html` (parameters ignored).
fn is_html(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/html")
}

/// A file of a version as it is served.
enum Served {
    /// An HTML page wrapped with the bridge ([`wrap_page_framed_by`]).
    Page(Arc<String>),
    /// Any other file, or an HTML file that is not UTF-8: served as stored.
    Raw(Vec<u8>, FileMeta),
}

/// `file` of version `n`, in one store call, its bytes read from the store's
/// content store. It is wrapped when `always_wrap` is set or its stored
/// content type is `text/html`, through the wrap cache, which also remembers
/// a file that is not UTF-8 (served raw); a page the cache holds is not
/// read again. `NotFound` when the artifact, the version, or the file is
/// gone.
async fn lookup(
    s: &AppState,
    id: ArtifactId,
    n: u32,
    file: String,
    always_wrap: bool,
) -> Result<Served, ApiError> {
    let cache = s.wrap_cache.clone();
    let bridge = bridge_version(&*s.web);
    // A page wrapped concurrently with a debug rebuild may keep the old
    // `?v=`; a debug build serves every bridge URL revalidated, current bytes.
    cache.follow_bridge(&bridge);
    // Content on a hosted daemon's content origin talks to the public one.
    let shell = s
        .hosted
        .as_ref()
        .filter(|h| h.content_origin().is_some())
        .map(|h| h.origin.clone());
    s.store_call_async(move |st| async move {
        let meta = st.file_meta(&id, n, &file)?.ok_or(CoreError::NotFound)?;
        if !always_wrap && !is_html(&meta.content_type) {
            return Ok(Served::Raw(read_bytes(&st, &id, n, &file).await?, meta));
        }
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        // Read ahead of the cache, which wraps synchronously; a page it
        // holds is not read. One evicted between the check and the wrap is
        // read on the next pass.
        let mut bytes = None;
        loop {
            if bytes.is_none() && !cache.contains(id.as_str(), n, &file) {
                bytes = Some(read_bytes(&st, &id, n, &file).await?);
            }
            let mut unwrapped = None;
            let wrapped = cache.get_or_wrap(id.as_str(), n, &file, || {
                let Some(bytes) = bytes.take() else {
                    return Err(std::io::Error::other(EVICTED));
                };
                Ok(match String::from_utf8(bytes) {
                    Ok(page) => Some(wrap_page_framed_by(
                        &page,
                        id.as_str(),
                        n,
                        CONTRACT_VERSION,
                        &file,
                        &bridge,
                        shell.as_deref(),
                    )),
                    Err(e) => {
                        unwrapped = Some(e.into_bytes());
                        None
                    }
                })
            });
            return match wrapped {
                Ok(Some(html)) => Ok(Served::Page(html)),
                Ok(None) => Ok(Served::Raw(
                    match unwrapped.or(bytes) {
                        Some(bytes) => bytes,
                        None => read_bytes(&st, &id, n, &file).await?,
                    },
                    meta,
                )),
                Err(e) if e.to_string() == EVICTED => continue,
                Err(e) => Err(CoreError::Io(e)),
            };
        }
    })
    .await
}

/// The error a wrap answers when the page it expected cached has gone.
const EVICTED: &str = "evicted while being read";

/// The bytes of `file` in version `n` of `id`, from the store's content
/// store; `NotFound` when they are missing.
async fn read_bytes(
    st: &clax_store::Store,
    id: &ArtifactId,
    n: u32,
    file: &str,
) -> clax_core::Result<Vec<u8>> {
    st.content()
        .read_file(id, n, file)
        .await?
        .ok_or(CoreError::NotFound)
}

/// A supporting file of a version. A `text/html` file in UTF-8 is served like
/// the index: wrapped with the bridge (`data-file` naming its path),
/// revalidated by the browser on every load, cached by the daemon per file.
/// Any other file is served as stored: immutable, except an HTML file that
/// is not UTF-8, which is revalidated like every page.
pub async fn file(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    origin: Option<Extension<OnArtifactOrigin>>,
    req: HeaderMap,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    // A version's file serves browsers (spec §7.5): agents read it through the routes the filter covers.
    d.refuse_agents()?;
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    let tag_source = format!("{aid}/{n}/{rel}");
    let own = own_host(&s);
    let live = s.live_ids.contains(id.as_str());
    let aid = id.as_str().to_string();
    let app = (aid.as_str(), own.as_str());
    let (bytes, meta) = match lookup(&s, id, n, rel, false).await? {
        Served::Page(html) => {
            return Ok(framed_by_shell(
                http_cache::html(&req, &html),
                &origin,
                &req,
                app,
                live,
            ));
        }
        Served::Raw(bytes, meta) => (bytes, meta),
    };
    let body = Body::from(bytes);
    if is_html(&meta.content_type) {
        // An HTML file that is not UTF-8 is still a page: revalidated, never
        // immutable. A version's files never change, so its artifact, version,
        // path and size identify its bytes.
        let etag = http_cache::etag_of(format!("{tag_source}\0{}", meta.size).as_bytes());
        let ct = meta.content_type.clone();
        let res = http_cache::tagged_as(&req, etag, http_cache::REVALIDATE, move || {
            (
                [
                    (header::CONTENT_TYPE, ct),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                    (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_string()),
                ],
                body,
            )
                .into_response()
        });
        return Ok(framed_by_shell(res, &origin, &req, app, live));
    }
    let cache_control = http_cache::IMMUTABLE;
    let res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type.as_str()),
            (header::CACHE_CONTROL, cache_control),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        body,
    )
        .into_response();
    Ok(framed_by_shell(res, &origin, &req, app, live))
}

/// `GET /api/artifacts/<id>/versions/<n>/files/<path>`: a file's stored bytes,
/// `index.html` included and never wrapped. Served with `Content-Security-Policy:
/// sandbox` because it shares the API's origin. A file an agent channel's
/// reader may not see answers 403 `withheld` with its authors; while the
/// version withholds any file, so does a path it lacks
/// ([`clax_http::views::thread::Vis::filter_file_content`]).
pub async fn raw_file(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::FileContent>,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let found = s
        .store_call_async(move |st| async move {
            let st = &*st;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            if vis.filters() {
                let v = st.get_version(&id, n)?.ok_or(CoreError::NotFound)?;
                if let Some(authors) = vis.filter_file_content(st, &v, &rel)? {
                    return Ok(Err(authors));
                }
            }
            Ok(Ok(st
                .read_file_async(&id, n, &rel)
                .await?
                .ok_or(CoreError::NotFound)?))
        })
        .await?;
    let (bytes, meta) = match found {
        Ok(f) => f,
        Err(authors) => {
            let mut e = ApiError::forbidden(
                "withheld",
                "this file is withheld until your person approves its authors",
            );
            e.extra.insert("authors".into(), serde_json::json!(authors));
            return Err(e);
        }
    };
    let body = Body::from(bytes);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type.as_str()),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_policy_names_a_plain_host_and_never_a_crafted_one() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7480"));
        let p = snapshot_policy(&h, "localhost:7480");
        assert!(
            p.to_str()
                .unwrap()
                .starts_with("script-src http://127.0.0.1:7480/_clax/;")
        );
        h.insert(
            header::HOST,
            HeaderValue::from_static("a 'strict-dynamic' b"),
        );
        let p = snapshot_policy(&h, "localhost:7480");
        let p = p.to_str().unwrap();
        assert!(
            p.starts_with("script-src http://localhost:7480/_clax/;"),
            "{p}"
        );
        assert!(!p.contains("strict-dynamic"));
        h.insert(header::HOST, HeaderValue::from_static("x;script-src *"));
        assert!(
            !snapshot_policy(&h, "localhost:7480")
                .to_str()
                .unwrap()
                .contains('*')
        );
        assert!(
            snapshot_policy(&HeaderMap::new(), "localhost:7480")
                .to_str()
                .unwrap()
                .contains("http://localhost:7480/_clax/")
        );
    }

    #[test]
    fn the_content_policy_names_a_plain_host_and_never_a_crafted_one() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("x;script-src *:80"));
        let p = content_policy(false, &h, "localhost:7480", Some("7q3k9mzx2b4t"));
        let p = p.to_str().unwrap();
        assert!(p.contains("http://localhost:7480/c/7q3k9mzx2b4t/ "), "{p}");
        assert!(!p.contains('*') && !p.contains("x;"), "{p}");
        assert!(p.contains("http://127.0.0.1:7480/_blob/"), "{p}");
        assert!(
            p.contains(
                "connect-src http://localhost:7480/c/7q3k9mzx2b4t/ http://localhost:7480/_blob/"
            ),
            "{p}"
        );
        // Without the lockdown (a live page), the framing policy alone.
        assert_eq!(
            content_policy(false, &h, "localhost:7480", None)
                .to_str()
                .unwrap(),
            SANDBOX
        );
    }
}
