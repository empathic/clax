//! Watches (spec §6): `PUT`/`DELETE /api/sessions/<sid>/watches/<aid>` (W) and the listing.

use super::artifacts::{parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::feedback::apply;
use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WatchBody {
    replies_armed: Option<bool>,
}

/// Watches the artifact (replies armed unless `replies_armed: false`) and
/// hands the session any feedback on it that had no live target.
pub async fn put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let b: WatchBody = if raw.is_empty() {
        WatchBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    let ctx = s.feedback_ctx();
    let watch = s
        .store_call(move |st| {
            let w = st.watch(&sid, &id, b.replies_armed.unwrap_or(true))?;
            let touched = st.retarget_untargeted(&id, &sid)?;
            apply(&ctx, st, &touched);
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"watch": watch})))
}

pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    s.store_call(move |st| st.unwatch(&sid, &id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The session's watches. Token-gated like every `/api/sessions*` read.
pub async fn list(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let watches = s.store_call(move |st| st.list_watches(&sid)).await?;
    Ok(Json(json!({"watches": watches})))
}
