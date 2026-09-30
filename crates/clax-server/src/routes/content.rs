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

fn sandboxed(mut res: Response, origin: &Option<Extension<OnArtifactOrigin>>) -> Response {
    if origin.is_none() {
        res.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(SANDBOX),
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
    match lookup(&s, id, n, INDEX.to_string(), true).await? {
        Served::Page(html) => Ok(sandboxed(http_cache::html(&req, &html), &origin)),
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
    let (disk, meta) = match lookup(&s, id, n, rel, false).await? {
        Served::Page(html) => return Ok(sandboxed(http_cache::html(&req, &html), &origin)),
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
        return Ok(sandboxed(res, &origin));
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
    Ok(sandboxed(res, &origin))
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
