//! REST routes for harness sessions.

use super::artifacts::{body, path};
use crate::audit::DeferredAudit;
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::push::CodexPush;
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::model::Session;
use clax_core::{CoreError, RegisterSession};
use serde::Deserialize;
use serde_json::{Value, json};

/// The harness names a session may carry.
pub const HARNESSES: [&str; 4] = ["claude", "codex", "grok", "pi"];

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

/// Registers a session; `harness` must be one of [`HARNESSES`]. A
/// `transcript_path` (the harness's extension API's session file) is kept
/// as a join's is. A new session records `session.start`, and a refresh
/// that changes it `session.join`, as its own agent (audit spec §6.8).
pub async fn register(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: DeferredAudit,
    req: Result<Json<RegisterSession>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut r = body(req)?;
    check_harness(&r.harness)?;
    r.transcript_path = transcript(r.transcript_path)?;
    let audit = audit.agent_side();
    let session = s
        .store_call(move |st| st.register_session(&audit, r))
        .await?;
    Ok((StatusCode::CREATED, Json(json!({"session": session}))))
}

/// The longest `transcript_path` a registration or join may give, in bytes.
pub const MAX_TRANSCRIPT_PATH: usize = 4096;

/// A registration's or join's `transcript_path`: empty is none; one longer
/// than [`MAX_TRANSCRIPT_PATH`] or with control characters is 400
/// `invalid_session`.
fn transcript(t: Option<String>) -> Result<Option<String>, ApiError> {
    match t {
        Some(t) if t.len() > MAX_TRANSCRIPT_PATH || t.chars().any(char::is_control) => {
            Err(ApiError::bad_request(
                "invalid_session",
                format!(
                    "transcript_path must be at most {MAX_TRANSCRIPT_PATH} bytes, without control characters"
                ),
            ))
        }
        t => Ok(t.filter(|t| !t.is_empty())),
    }
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
    /// The harness's transcript file, as its hook input names it; a join
    /// without it keeps the recorded value.
    #[serde(default)]
    transcript_path: Option<String>,
}

/// Joins a harness session ID to its session (see `Store::join_session`) and,
/// for Codex, records `codex_home`; a join without it keeps the recorded value.
/// A join that makes or changes the session records `session.start` or
/// `session.join`, as its own agent (audit spec §6.8).
pub async fn join(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: DeferredAudit,
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
    let transcript = transcript(b.transcript_path.clone())?;
    let audit = audit.agent_side();
    let session = s
        .store_call(move |st| {
            let session = st.join_session(
                &audit,
                &b.harness,
                b.parent_pid,
                &b.harness_session_id,
                b.cwd.as_deref(),
                transcript.as_deref(),
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

/// `{"heartbeat": true}` renews the session (records nothing);
/// `{"ended": true}` ends it, recording `session.end` as its agent and the
/// ends of its working records.
pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: DeferredAudit,
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
    let state = s.clone();
    let session = s
        .store_call(move |st| {
            if b.ended {
                let audit = audit.for_session(st, &id)?;
                let ended = st.end_session_touched(&audit, &id)?;
                crate::feedback::apply(&ctx, st, &ended.touched);
                crate::questions::announce_ids(&state, st, &ended.withdrawn_questions);
                ctx.waiters.forget(&id);
                let (stopped, _) = ctx
                    .working
                    .end_session(&id, clax_core::working::End::SessionEnd);
                crate::working::settle(st, &audit, &ctx.events, &ctx.working, &stopped);
                ctx.followers.forget(&id);
                Ok(ended.session)
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
    let (session, codex_home, push_error) = s
        .store_call(move |st| {
            let session = st.get_session(&id)?.ok_or(CoreError::NotFound)?;
            let codex_home = st.codex_home(&id)?;
            let push_error = st.push_error(&id)?;
            Ok((session, codex_home, push_error))
        })
        .await?;
    let following = s.followers.is_following(&session.id);
    let push = push_info(&session, &s.codex, codex_home, push_error, following);
    Ok(Json(json!({"session": session, "push": push})))
}

/// Why nothing wakes an idle Claude Code session that no notice follower
/// polls for: optional, since comments still arrive without push, and how to
/// turn it on.
const CLAUDE_NO_PUSH: &str = "optional: comments sent to this session already arrive with the next clax tool result, at the end of each turn, with the next message, and during wait_for_feedback; push only wakes the session while it is idle. To have a comment wake it, run follow_command in the background after publishing, or launch Claude Code with `claude --dangerously-load-development-channels plugin:clax@clax` (Claude Code channels, a research preview)";

/// How feedback can be pushed to this session (tier 5), and why not when it
/// cannot. For Codex, `last_error` and `last_error_at` hold the latest
/// `codex queue` failure (`null` after a success or before any run); a
/// failure leaves push available, since the next comment is pushed again.
/// For Grok, push is the monitor: available while `following` (a `clax
/// feedback follow` of the session is connected). For Claude Code, the
/// daemon reports a notice follower (a `clax feedback follow --once` or the
/// shim's channel) as tier `notice`; the shim refines it.
fn push_info(
    s: &Session,
    codex: &CodexPush,
    codex_home: Option<String>,
    push_error: Option<(String, String)>,
    following: bool,
) -> Value {
    match s.harness.as_str() {
        "codex" => {
            let reason = codex.reason().or_else(|| {
                s.harness_session_id
                    .is_none()
                    .then(|| "Codex session ID unknown, native push disabled".to_string())
            });
            let tier = reason.is_none().then_some("queue");
            let (last_error, last_error_at) = push_error.unzip();
            json!({"tier": tier, "available": reason.is_none(), "reason": reason, "codex_home": codex_home,
                "last_error": last_error, "last_error_at": last_error_at})
        }
        "pi" => json!({"tier": "inject", "available": true, "reason": null}),
        "grok" => {
            let reason = (!following).then_some(
                "no clax feedback follow is running for this session; the clax-grok skill starts one with Grok's monitor tool after a publish. Meanwhile comments arrive at the end of a turn (Stop hook), on the next clax tool call, or during wait_for_feedback",
            );
            json!({"tier": "monitor", "available": following, "reason": reason})
        }
        "claude" if following => json!({"tier": "notice", "available": true, "reason": null}),
        "claude" => json!({"tier": null, "available": false, "reason": CLAUDE_NO_PUSH}),
        _ => {
            json!({"tier": null, "available": false, "reason": "Claude Code has no native push; comments arrive at the end of a turn (Stop hook), with the next prompt, on the next clax tool call, or during wait_for_feedback"})
        }
    }
}

/// `GET /api/push`: whether the daemon can push to Codex, where its `codex`
/// came from, and why push is unavailable when it is. `bin`, the path of the
/// daemon's `codex` (it usually names the user's home), is included only for
/// a request with the token.
pub async fn push_status(State(s): State<AppState>, headers: axum::http::HeaderMap) -> Json<Value> {
    let mut codex = json!({
        "available": s.codex.available(),
        "source": s.codex.source,
        "reason": s.codex.reason(),
    });
    if crate::auth::has_token(&headers, &s.token) {
        codex["bin"] = json!(
            s.codex
                .bin
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
        );
    }
    Json(json!({ "codex": codex }))
}
