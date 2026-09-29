//! Asset upload, listing, deletion, and the /_blob/<id> byte route.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::routes::artifacts::{parse_id, path};
use crate::state::AppState;
use artifax_core::CoreError;
use axum::Json;
use axum::body::Body;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::PathRejection;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

fn multipart_error(status: StatusCode, message: String) -> ApiError {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "request body exceeds the upload limit",
        )
    } else {
        ApiError::bad_request("invalid_multipart", message)
    }
}

pub async fn upload(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let aid = path(aid)?;
    let id = parse_id(&aid)?;
    let mut mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let content_type = field
            .content_type()
            .map(|s| s.to_string())
            .or_else(|| {
                field
                    .file_name()
                    .map(artifax_core::publish::content_type_for)
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        let asset = s
            .store_call(move |st| st.add_asset(&id, &content_type, &bytes))
            .await?;
        let url = format!("/_blob/{}", asset.id);
        return Ok((
            StatusCode::CREATED,
            Json(json!({"asset": asset, "url": url})),
        ));
    }
    Err(ApiError::bad_request(
        "missing_file",
        "multipart field 'file' is required",
    ))
}

pub async fn list(
    State(s): State<AppState>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = path(aid)?;
    let id = parse_id(&aid)?;
    let assets = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.list_assets(&id)
        })
        .await?;
    Ok(Json(json!({"assets": assets})))
}

pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let (aid, asset_id) = path(params)?;
    let id = parse_id(&aid)?;
    s.store_call(move |st| match st.get_asset(&asset_id)? {
        Some((a, _)) if a.artifact_id == id.as_str() => st.delete_asset(&asset_id),
        _ => Err(CoreError::NotFound),
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn blob(
    State(s): State<AppState>,
    asset_id: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let asset_id = path(asset_id)?;
    let (asset, path) = s
        .store_call(move |st| st.get_asset(&asset_id)?.ok_or(CoreError::NotFound))
        .await?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found())?;
    let len = asset.size.to_string();
    let stream = tokio_util::io::ReaderStream::new(file);
    Ok((
        [
            (header::CONTENT_TYPE, asset.content_type.as_str()),
            (header::CONTENT_LENGTH, len.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}
