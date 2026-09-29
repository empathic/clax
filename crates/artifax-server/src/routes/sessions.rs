//! REST routes for harness sessions.

use super::artifacts::{body, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::push::CodexPush;
use crate::state::AppState;
use artifax_core::model::Session;
use artifax_core::{CoreError, RegisterSession};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

/// The harness names a session may carry.
pub const HARNESSES: [&str; 3] = ["claude", "codex", "pi"];

/// `invalid_args` unless `harness` is one of [`HARNESSES`].
fn check_harness(harness: &str) -> Result<(), ApiError> {
    if HARNESSES.contains(&harness) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "invalid_args",
            format!("harness must be one of {}", HARNESSES.join(", ")),
        ))
    }
}

/// Registers a session; `harness` must be one of [`HARNESSES`].
pub async fn register(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<RegisterSession>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let r = body(req)?;
    check_harness(&r.harness)?;
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
    /// The `CODEX_HOME` the session's Codex runs with, recorded for `codex
    /// queue`. Ignored for other harnesses; a join without it keeps the value
    /// recorded earlier.
    #[serde(default)]
    codex_home: Option<String>,
}

/// Joins a harness session ID to its session (see `Store::join_session`) and,
/// for Codex, records `codex_home`; a join without it keeps the recorded value.
pub async fn join(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<JoinBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    check_harness(&b.harness)?;
    if b.harness_session_id.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_session",
            "harness_session_id must not be empty",
        ));
    }
    let session = s
        .store_call(move |st| {
            let session = st.join_session(
                &b.harness,
                b.parent_pid,
                &b.harness_session_id,
                b.cwd.as_deref(),
                &b.ancestor_pids,
            )?;
            if let Some(h) = b.codex_home.as_deref().filter(|_| b.harness == "codex") {
                st.set_codex_home(&session.id, h)?;
            }
            Ok(session)
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
    let ctx = s.feedback_ctx();
    let session = s
        .store_call(move |st| {
            if b.ended {
                let (session, touched) = st.end_session_touched(&id)?;
                crate::feedback::apply(&ctx, st, &touched);
                ctx.waiters.forget(&id);
                Ok(session)
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

/// Lists sessions. Session rows carry working directories and process IDs,
/// so reading them needs the token.
pub async fn list(
    State(s): State<AppState>,
    _t: RequireToken,
    q: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let sessions = s.store_call(move |st| st.list_sessions(q.live)).await?;
    Ok(Json(json!({"sessions": sessions})))
}

/// One session and how feedback can be pushed to it ([`push_info`]); needs
/// the token, as [`list`] does.
pub async fn get(
    State(s): State<AppState>,
    _t: RequireToken,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(id)?;
    let (session, codex_home) = s
        .store_call(move |st| {
            let session = st.get_session(&id)?.ok_or(CoreError::NotFound)?;
            let codex_home = st.codex_home(&id)?;
            Ok((session, codex_home))
        })
        .await?;
    let push = push_info(&session, &s.codex, codex_home);
    Ok(Json(json!({"session": session, "push": push})))
}

/// How feedback can be pushed to this session (tier 5), and why not when it cannot.
fn push_info(s: &Session, codex: &CodexPush, codex_home: Option<String>) -> Value {
    match s.harness.as_str() {
        "codex" => {
            let reason = codex.reason().or_else(|| {
                s.harness_session_id
                    .is_none()
                    .then(|| "Codex session ID unknown, native push disabled".to_string())
            });
            let tier = reason.is_none().then_some("queue");
            json!({"tier": tier, "available": reason.is_none(), "reason": reason, "codex_home": codex_home})
        }
        "pi" => json!({"tier": "inject", "available": true, "reason": null}),
        _ => {
            json!({"tier": null, "available": false, "reason": "Claude Code has no native push; comments arrive at the end of a turn (Stop hook), with the next prompt, on the next artifax tool call, or during wait_for_feedback"})
        }
    }
}

/// `GET /api/push`: the daemon's `codex`, where it came from, and why push is
/// unavailable when it is.
pub async fn push_status(State(s): State<AppState>) -> Json<Value> {
    Json(json!({"codex": {
        "available": s.codex.available(),
        "bin": s.codex.bin.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "source": s.codex.source,
        "reason": s.codex.reason(),
    }}))
}
