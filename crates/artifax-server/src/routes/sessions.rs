//! REST routes for harness sessions.

use super::artifacts::{body, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use artifax_core::{CoreError, RegisterSession};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

pub async fn register(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<RegisterSession>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let r = body(req)?;
    if r.harness.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_session",
            "harness must not be empty",
        ));
    }
    let session = s.store_call(move |st| st.register_session(r)).await?;
    Ok((StatusCode::CREATED, Json(json!({"session": session}))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinBody {
    harness: String,
    parent_pid: u32,
    harness_session_id: String,
    #[serde(default)]
    cwd: Option<String>,
    /// The caller's ancestors, nearest first, tried after `parent_pid`.
    #[serde(default)]
    ancestor_pids: Vec<u32>,
}

pub async fn join(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<JoinBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    if b.harness.trim().is_empty() || b.harness_session_id.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_session",
            "harness and harness_session_id must not be empty",
        ));
    }
    let session = s
        .store_call(move |st| {
            st.join_session(
                &b.harness,
                b.parent_pid,
                &b.harness_session_id,
                b.cwd.as_deref(),
                &b.ancestor_pids,
            )
        })
        .await?;
    Ok(Json(json!({"session": session})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    #[serde(default)]
    heartbeat: bool,
    #[serde(default)]
    ended: bool,
}

pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    id: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(id)?;
    let b = body(req)?;
    if !b.heartbeat && !b.ended {
        return Err(ApiError::bad_request(
            "invalid_session_patch",
            "send {\"heartbeat\": true} or {\"ended\": true}",
        ));
    }
    let session = s
        .store_call(move |st| {
            if b.ended {
                st.end_session(&id)
            } else {
                st.heartbeat(&id)
            }
        })
        .await?;
    Ok(Json(json!({"session": session})))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    live: bool,
}

pub async fn list(
    State(s): State<AppState>,
    q: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let sessions = s.store_call(move |st| st.list_sessions(q.live)).await?;
    Ok(Json(json!({"sessions": sessions})))
}

pub async fn get(
    State(s): State<AppState>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(id)?;
    let session = s
        .store_call(move |st| st.get_session(&id)?.ok_or(CoreError::NotFound))
        .await?;
    Ok(Json(json!({"session": session})))
}
