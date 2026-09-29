//! Serves published page content: wrapped index and immutable supporting files.

use crate::error::ApiError;
use crate::host::OnArtifactOrigin;
use crate::routes::artifacts::{parse_id, path};
use crate::state::AppState;
use artifax_core::CoreError;
use artifax_core::model::CONTRACT_VERSION;
use artifax_core::publish::INDEX;
use artifax_core::wrap::wrap_document;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

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
    Ok(sandboxed(serve_index(&s, &aid, n).await?, &origin))
}

async fn serve_index(s: &AppState, aid: &str, n: u32) -> Result<Response, ApiError> {
    let id = parse_id(aid)?;
    let cache = s.wrap_cache.clone();
    let html = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let (disk, _) = st.file_path(&id, n, INDEX)?.ok_or(CoreError::NotFound)?;
            cache
                .get_or_wrap(id.as_str(), n, || {
                    std::fs::read_to_string(&disk)
                        .map(|page| wrap_document(&page, id.as_str(), n, CONTRACT_VERSION))
                })
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        CoreError::NotFound
                    } else {
                        CoreError::Io(e)
                    }
                })
        })
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Html(html.to_string()),
    )
        .into_response())
}

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
    let (disk, meta) = s
        .store_call(move |st| st.file_path(&id, n, &rel)?.ok_or(CoreError::NotFound))
        .await?;
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
