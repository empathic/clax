//! Serves published page content: wrapped index and immutable supporting files.

use crate::error::ApiError;
use crate::routes::artifacts::{parse_id, path};
use crate::state::AppState;
use artifax_core::model::CONTRACT_VERSION;
use artifax_core::publish::INDEX;
use artifax_core::wrap::wrap_document;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

pub async fn redirect_to_slash(
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Redirect, ApiError> {
    let (aid, n) = path(p)?;
    let id = parse_id(&aid)?;
    Ok(Redirect::permanent(&format!("/c/{}/v/{n}/", id.as_str())))
}

pub async fn index(
    State(s): State<AppState>,
    p: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n) = path(p)?;
    serve_index(&s, &aid, n).await
}

async fn serve_index(s: &AppState, aid: &str, n: u32) -> Result<Response, ApiError> {
    let id = parse_id(aid)?;
    s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    let (disk, _) = s
        .store
        .file_path(&id, n, INDEX)?
        .ok_or_else(ApiError::not_found)?;
    let page = tokio::fs::read_to_string(&disk)
        .await
        .map_err(|_| ApiError::not_found())?;
    let html = wrap_document(&page, id.as_str(), n, CONTRACT_VERSION);
    Ok(([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response())
}

pub async fn file(
    State(s): State<AppState>,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    if rel == INDEX {
        return Ok(Redirect::permanent(&format!("/c/{}/v/{n}/", id.as_str())).into_response());
    }
    let (disk, meta) = s
        .store
        .file_path(&id, n, &rel)?
        .ok_or_else(ApiError::not_found)?;
    let f = tokio::fs::File::open(&disk)
        .await
        .map_err(|_| ApiError::not_found())?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response())
}
