//! Watches (spec §6): `PUT`/`DELETE /api/sessions/<sid>/watches/<aid>` (W),
//! the listing, and scope watches on page URLs (`/live-watches`, spec
//! 2026-10-05 L2).

use super::artifacts::{body, parse_id, path};
use super::live::{page_url, page_view};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::feedback::apply;
use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::{ArtifactId, CoreError, Event};
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveWatchBody {
    url: String,
    replies_armed: Option<bool>,
}

/// The pages a scope watch on `origin` + `path` covers, as the tools show it.
fn scope_label(origin: &str, path: &str) -> String {
    if path.ends_with('/') {
        format!("{origin}{path}*")
    } else {
        format!("{origin}{path} and {origin}{path}/*")
    }
}

/// `PUT /api/sessions/<sid>/live-watches` (W): a scope watch on the page
/// `url` names (spec 2026-10-05 L2, §9.3). The page is created (with its
/// placeholder) when it does not exist; the session watches every live page
/// the scope covers and is handed their comments that were waiting with no
/// live target. Answers `{live_watch: {origin, path, scope, replies_armed},
/// page, covered}`.
pub async fn live_put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    b: Result<Json<LiveWatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let b = body(b)?;
    let pu = page_url(&s, &b.url)?;
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let (w, page, artifact, covered) = s
        .store_call(move |st| {
            let (w, _) = st.live_watch(&sid, &pu.key, b.replies_armed.unwrap_or(true))?;
            let page_url = pu.key.page_url();
            let title = page_url
                .split_once("://")
                .map_or(page_url.as_str(), |(_, rest)| rest);
            let e = live_ids.ensure_page(st, &pu.key, title, None, &[])?;
            if e.new_version {
                events.publish(Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
            }
            // The covered pages, now including this one.
            let (w, covered) = st.live_watch(&sid, &pu.key, w.replies_armed)?;
            for aid in &covered {
                let touched = st.retarget_untargeted(&ArtifactId::parse(aid)?, &sid)?;
                apply(&ctx, st, &touched);
            }
            let id = ArtifactId::parse(&e.artifact.id)?;
            let page = st.live_page_of(&id)?.ok_or(CoreError::NotFound)?;
            Ok((w, page, e.artifact, covered))
        })
        .await?;
    Ok(Json(json!({
        "live_watch": {
            "origin": w.origin,
            "path": w.path,
            "scope": scope_label(&w.origin, &w.path),
            "replies_armed": w.replies_armed,
        },
        "page": page_view(&s, &page, &artifact),
        "covered": covered,
    })))
}

#[derive(Deserialize)]
pub struct LiveUnwatchQuery {
    url: String,
}

/// `DELETE /api/sessions/<sid>/live-watches?url=` (W): removes the scope
/// watch on the page `url` names and the watches only it justified.
/// Answers `{removed: [artifact IDs]}`.
pub async fn live_delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<LiveUnwatchQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let removed = s
        .store_call(move |st| st.live_unwatch(&sid, &pu.key))
        .await?;
    Ok(Json(json!({"removed": removed})))
}
