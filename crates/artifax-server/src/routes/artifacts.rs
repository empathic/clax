//! REST routes for artifacts and versions.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use artifax_core::publish::{PublishRequest, validate};
use artifax_core::{ArtifactId, Event, MetaPatch};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

/// Request body cap for the publish routes. A fully base64-encoded 64 MiB
/// payload is about 85 MiB on the wire, so the cap sits above that;
/// `validate` still enforces the 64 MiB decoded limit.
pub const PUBLISH_BODY_LIMIT: usize = 96 * 1024 * 1024;

pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

fn body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    r.map(|Json(v)| v)
        .map_err(|e| ApiError::bad_request("invalid_json", e.body_text()))
}

pub async fn list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"artifacts": s.store.list_artifacts()?})))
}

pub async fn create(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<PublishRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let p = validate(body(req)?)?;
    let (artifact, version) = s.store.create_artifact(p)?;
    s.events.publish(Event::Version {
        artifact_id: artifact.id.clone(),
        n: version.n,
    });
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"artifact": artifact, "version": version, "url": url})),
    ))
}

pub async fn get(
    State(s): State<AppState>,
    Path(aid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let artifact = s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(
        json!({"artifact": artifact, "versions": s.store.list_versions(&id)?}),
    ))
}

#[derive(Deserialize, Default)]
pub struct PatchBody {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    pinned: Option<bool>,
}

pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    Path(aid): Path<String>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let artifact = s.store.update_meta(
        &id,
        MetaPatch {
            title: b.title,
            description: b.description,
            icon: b.icon,
            pinned: b.pinned,
        },
    )?;
    Ok(Json(json!({"artifact": artifact})))
}

pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    Path(aid): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_id(&aid)?;
    s.store.delete_artifact(&id)?;
    s.events.publish(Event::ArtifactDeleted {
        artifact_id: id.as_str().to_string(),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_versions(
    State(s): State<AppState>,
    Path(aid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"versions": s.store.list_versions(&id)?})))
}

pub async fn publish(
    State(s): State<AppState>,
    _t: RequireToken,
    Path(aid): Path<String>,
    req: Result<Json<PublishRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&aid)?;
    let p = validate(body(req)?)?;
    let (artifact, version) = s.store.publish_version(&id, p)?;
    s.events.publish(Event::Version {
        artifact_id: artifact.id.clone(),
        n: version.n,
    });
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"artifact": artifact, "version": version, "url": url})),
    ))
}

pub async fn get_version(
    State(s): State<AppState>,
    Path((aid, n)): Path<(String, u32)>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let v = s
        .store
        .get_version(&id, n)?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"version": v})))
}

pub async fn files(
    State(s): State<AppState>,
    Path(aid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let a = s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    let v = s
        .store
        .get_version(&id, a.current_version)?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"files": v.files, "version": v.n})))
}
