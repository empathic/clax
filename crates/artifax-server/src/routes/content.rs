//! Serves published page content: every HTML page (the index and supporting
//! `text/html` files) wrapped with the bridge, other supporting files as stored.

use crate::error::ApiError;
use crate::host::OnArtifactOrigin;
use crate::routes::artifacts::{parse_id, path};
use crate::state::AppState;
use artifax_core::model::CONTRACT_VERSION;
use artifax_core::publish::INDEX;
use artifax_core::wrap::wrap_page;
use artifax_core::{ArtifactId, CoreError};
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
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
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n) = path(p)?;
    let id = parse_id(&aid)?;
    let html = wrapped_page(&s, id, n, INDEX.to_string())
        .await?
        .ok_or_else(|| {
            ApiError::from(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "index.html is not UTF-8",
            )))
        })?;
    Ok(sandboxed(page_response(&html), &origin))
}

fn page_response(html: &str) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(html.to_string()),
    )
        .into_response()
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

/// The HTML page at `file` of version `n` wrapped with the bridge
/// ([`wrap_page`]), from the wrap cache when present. `None` when the file is
/// not UTF-8, so it cannot be wrapped. `NotFound` when the artifact, the
/// version, or the file is gone.
async fn wrapped_page(
    s: &AppState,
    id: ArtifactId,
    n: u32,
    file: String,
) -> Result<Option<Arc<String>>, ApiError> {
    let cache = s.wrap_cache.clone();
    s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        let (disk, _) = st.file_path(&id, n, &file)?.ok_or(CoreError::NotFound)?;
        let wrapped = cache.get_or_wrap(id.as_str(), n, &file, || {
            let page = String::from_utf8(std::fs::read(&disk)?)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(wrap_page(&page, id.as_str(), n, CONTRACT_VERSION, &file))
        });
        match wrapped {
            Ok(html) => Ok(Some(html)),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::NotFound),
            Err(e) => Err(CoreError::Io(e)),
        }
    })
    .await
}

/// A supporting file of a version. A `text/html` file in UTF-8 is served like
/// the index: wrapped with the bridge (`data-file` naming its path), uncached
/// by the browser, cached by the daemon per file. Any other file is streamed
/// as stored, cached as immutable.
pub async fn file(
    State(s): State<AppState>,
    origin: Option<Extension<OnArtifactOrigin>>,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    let (disk, meta) = {
        let id = id.clone();
        let rel = rel.clone();
        s.store_call(move |st| st.file_path(&id, n, &rel)?.ok_or(CoreError::NotFound))
            .await?
    };
    if is_html(&meta.content_type)
        && let Some(html) = wrapped_page(&s, id, n, rel).await?
    {
        return Ok(sandboxed(page_response(&html), &origin));
    }
    let f = tokio::fs::File::open(&disk)
        .await
        .map_err(|_| ApiError::not_found())?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
    let res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
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
