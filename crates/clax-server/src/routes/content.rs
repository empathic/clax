//! Serves published page content: every HTML page (the index and supporting
//! `text/html` files) wrapped with the bridge, other supporting files as stored.

use crate::error::ApiError;
use crate::host::OnArtifactOrigin;
use crate::http_cache;
use crate::routes::artifacts::{parse_id, path};
use crate::routes::shell::bridge_version;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use clax_core::model::CONTRACT_VERSION;
use clax_core::model::FileMeta;
use clax_core::publish::INDEX;
use clax_core::wrap::wrap_page;
use clax_core::{ArtifactId, CoreError};
use std::path::PathBuf;
use std::sync::Arc;

/// Content served on the main origin must not run same-origin with the API.
const SANDBOX: &str = "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";

/// The `Content-Security-Policy` of a content response.
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
fn content_policy(origin: &Option<Extension<OnArtifactOrigin>>, req: &HeaderMap) -> HeaderValue {
    if origin.is_none() {
        return HeaderValue::from_static(SANDBOX);
    }
    // The rewrite accepted this host only with a numeric port, or none.
    let port = req
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(':'))
        .map(|(_, p)| format!(":{p}"))
        .unwrap_or_default();
    HeaderValue::from_str(&format!(
        "frame-ancestors 'self' http://localhost{port} http://127.0.0.1{port}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("frame-ancestors 'self'"))
}

/// The host and port in the daemon's browser base URL.
fn own_host(s: &AppState) -> String {
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
    let plain = |h: &str| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    };
    let host = req
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| plain(h))
        .unwrap_or(fallback);
    HeaderValue::from_str(&format!(
        "script-src http://{host}/_clax/; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'; worker-src 'none'"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("script-src 'none'"))
}

/// Sets the content policy on `res`, and for a live page's content (`live`:
/// the daemon's host, the policy's fallback) appends [`snapshot_policy`]:
/// both apply.
fn framed_by_shell(
    mut res: Response,
    origin: &Option<Extension<OnArtifactOrigin>>,
    req: &HeaderMap,
    live: Option<&str>,
) -> Response {
    res.headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, content_policy(origin, req));
    if let Some(fallback) = live {
        res.headers_mut().append(
            header::CONTENT_SECURITY_POLICY,
            snapshot_policy(req, fallback),
        );
    }
    res
}

pub async fn redirect_to_slash(
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Redirect, ApiError> {
    let (aid, n) = path(p)?;
    parse_id(&aid)?;
    Ok(Redirect::permanent(&format!("{n}/")))
}

pub async fn index(
    State(s): State<AppState>,
    origin: Option<Extension<OnArtifactOrigin>>,
    req: HeaderMap,
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n) = path(p)?;
    let id = parse_id(&aid)?;
    let own = own_host(&s);
    let live = s.live_ids.contains(id.as_str()).then_some(own.as_str());
    match lookup(&s, id, n, INDEX.to_string(), true).await? {
        Served::Page(html) => Ok(framed_by_shell(
            http_cache::html(&req, &html),
            &origin,
            &req,
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
    /// An HTML page wrapped with the bridge ([`wrap_page`]).
    Page(Arc<String>),
    /// Any other file, or an HTML file that is not UTF-8: streamed as stored.
    Raw(PathBuf, FileMeta),
}

/// `file` of version `n`, in one store call. It is wrapped when `always_wrap`
/// is set or its stored content type is `text/html`, through the wrap cache,
/// which also remembers a file that is not UTF-8 (served raw). `NotFound` when
/// the artifact, the version, or the file is gone.
async fn lookup(
    s: &AppState,
    id: ArtifactId,
    n: u32,
    file: String,
    always_wrap: bool,
) -> Result<Served, ApiError> {
    let cache = s.wrap_cache.clone();
    let bridge = bridge_version();
    // A page wrapped concurrently with a debug rebuild may keep the old
    // `?v=`; a debug build serves every bridge URL revalidated, current bytes.
    cache.follow_bridge(&bridge);
    s.store_call(move |st| {
        let (disk, meta) = st.file_path(&id, n, &file)?.ok_or(CoreError::NotFound)?;
        if !always_wrap && !is_html(&meta.content_type) {
            return Ok(Served::Raw(disk, meta));
        }
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        let wrapped = cache.get_or_wrap(id.as_str(), n, &file, || {
            Ok(String::from_utf8(std::fs::read(&disk)?)
                .ok()
                .map(|page| wrap_page(&page, id.as_str(), n, CONTRACT_VERSION, &file, &bridge)))
        });
        match wrapped {
            Ok(Some(html)) => Ok(Served::Page(html)),
            Ok(None) => Ok(Served::Raw(disk, meta)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::NotFound),
            Err(e) => Err(CoreError::Io(e)),
        }
    })
    .await
}

/// A supporting file of a version. A `text/html` file in UTF-8 is served like
/// the index: wrapped with the bridge (`data-file` naming its path),
/// revalidated by the browser on every load, cached by the daemon per file.
/// Any other file is streamed as stored: immutable, except an HTML file that
/// is not UTF-8, which is revalidated like every page.
pub async fn file(
    State(s): State<AppState>,
    origin: Option<Extension<OnArtifactOrigin>>,
    req: HeaderMap,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    let tag_source = format!("{aid}/{n}/{rel}");
    let own = own_host(&s);
    let live = s.live_ids.contains(id.as_str()).then_some(own.as_str());
    let (disk, meta) = match lookup(&s, id, n, rel, false).await? {
        Served::Page(html) => {
            return Ok(framed_by_shell(
                http_cache::html(&req, &html),
                &origin,
                &req,
                live,
            ));
        }
        Served::Raw(disk, meta) => (disk, meta),
    };
    let f = tokio::fs::File::open(&disk)
        .await
        .map_err(|_| ApiError::not_found())?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
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
        return Ok(framed_by_shell(res, &origin, &req, live));
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
    Ok(framed_by_shell(res, &origin, &req, live))
}

/// `GET /api/artifacts/<id>/versions/<n>/files/<path>`: a file's stored bytes,
/// `index.html` included and never wrapped. Served with `Content-Security-Policy:
/// sandbox` because it shares the API's origin.
pub async fn raw_file(
    State(s): State<AppState>,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    let (disk, meta) = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.file_path(&id, n, &rel)?.ok_or(CoreError::NotFound)
        })
        .await?;
    let f = tokio::fs::File::open(&disk)
        .await
        .map_err(|_| ApiError::not_found())?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
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
            HeaderValue::from_static("a 'unsafe-inline' b"),
        );
        let p = snapshot_policy(&h, "localhost:7480");
        let p = p.to_str().unwrap();
        assert!(
            p.starts_with("script-src http://localhost:7480/_clax/;"),
            "{p}"
        );
        assert!(!p.contains("unsafe-inline"));
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
}
