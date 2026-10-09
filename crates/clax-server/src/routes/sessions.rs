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
use std::collections::HashMap;
use std::sync::Arc;

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

/// How long an unmatched call-ID report waits for its `tool.call` in the
/// daemon ([`CallIdWait::grace`]).
pub const CALL_ID_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// How many call-ID reports may wait at once in the daemon
/// ([`CallIdWait::cap`]).
pub const CALL_ID_CAP: usize = 64;

/// How call-ID reports wait for their `tool.call` (spec
/// 2026-10-06-toolpath-audit-design §6.7). The shim reports a call in the
/// background once its result has gone back, and Claude Code runs the
/// PostToolUse hook once it has the result, so the hook's report may come
/// first. A report that finds no call waits up to `grace`, woken when its
/// own session records a `tool.call`; at most `cap` reports wait at once,
/// and one past that records `call_id: null` at once. Two reports looking
/// at once for the same session, tool and argument hash cannot tell their
/// calls apart, so both record `call_id: null`.
pub struct CallIdWait {
    /// How long an unmatched report waits ([`CALL_ID_GRACE`] in the daemon).
    pub grace: std::time::Duration,
    /// How many reports may wait at once ([`CALL_ID_CAP`] in the daemon).
    pub cap: usize,
    /// How many reports are waiting now.
    pub parked: tokio::sync::watch::Sender<usize>,
    /// The wake-up of each session a report is looking in, with how many
    /// reports hold it.
    sessions: std::sync::Mutex<HashMap<String, (Arc<tokio::sync::Notify>, usize)>>,
    /// The reports looking now, by session, bare tool name and argument
    /// hash: how many, and whether more than one has looked at once.
    looking: std::sync::Mutex<HashMap<CallKey, (usize, bool)>>,
}

/// A report's session, bare tool name and argument hash.
type CallKey = (String, String, String);

impl CallIdWait {
    pub fn new(grace: std::time::Duration, cap: usize) -> CallIdWait {
        CallIdWait {
            grace,
            cap,
            parked: tokio::sync::watch::channel(0).0,
            sessions: Default::default(),
            looking: Default::default(),
        }
    }

    /// Counts a report for session `sid`, bare tool `tool` and hash
    /// `args_sha256` as looking until the returned guard drops. When
    /// another such report is already looking, both are contested
    /// ([`Looking::contested`]), and the session's reports are woken so the
    /// earlier one sees it.
    pub fn look(&self, sid: &str, tool: &str, args_sha256: &str) -> Looking<'_> {
        let key = (sid.to_string(), tool.to_string(), args_sha256.to_string());
        let contested = {
            let mut map = self.looking.lock().unwrap_or_else(|e| e.into_inner());
            let entry = map.entry(key.clone()).or_insert((0, false));
            if entry.0 > 0 {
                entry.1 = true;
            }
            entry.0 += 1;
            entry.1
        };
        if contested {
            self.recorded(sid);
        }
        Looking { wait: self, key }
    }

    /// The wake-up for session `sid`, held until the returned guard drops.
    pub fn watch(&self, sid: &str) -> SessionWake<'_> {
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map
            .entry(sid.to_string())
            .or_insert_with(|| (Arc::default(), 0));
        entry.1 += 1;
        SessionWake {
            wait: self,
            sid: sid.to_string(),
            notify: entry.0.clone(),
        }
    }

    /// Wakes the reports looking in session `sid`: it recorded a
    /// `tool.call`.
    pub fn recorded(&self, sid: &str) {
        let map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((notify, _)) = map.get(sid) {
            notify.notify_waiters();
        }
    }

    /// Counts a report as waiting until the returned guard drops; `None`
    /// when [`CallIdWait::cap`] reports already wait.
    fn park(&self) -> Option<Parked<'_>> {
        let cap = self.cap;
        self.parked
            .send_if_modified(|n| {
                let room = *n < cap;
                if room {
                    *n += 1;
                }
                room
            })
            .then(|| Parked(&self.parked))
    }
}

/// A session's wake-up, held by a report looking in it.
pub struct SessionWake<'a> {
    wait: &'a CallIdWait,
    sid: String,
    pub notify: Arc<tokio::sync::Notify>,
}

impl Drop for SessionWake<'_> {
    fn drop(&mut self) {
        let mut map = self.wait.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.sid) {
            entry.1 -= 1;
            if entry.1 == 0 {
                map.remove(&self.sid);
            }
        }
    }
}

/// A report looking for its call, counted in [`CallIdWait::look`] until
/// dropped.
pub struct Looking<'a> {
    wait: &'a CallIdWait,
    key: CallKey,
}

impl Looking<'_> {
    /// Whether another report for the same session, tool and argument hash
    /// has looked while this one did.
    pub fn contested(&self) -> bool {
        let map = self.wait.looking.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&self.key).is_some_and(|e| e.1)
    }
}

impl Drop for Looking<'_> {
    fn drop(&mut self) {
        let mut map = self.wait.looking.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.key) {
            entry.0 -= 1;
            if entry.0 == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// A waiting report, counted in [`CallIdWait::parked`] until dropped.
struct Parked<'a>(&'a tokio::sync::watch::Sender<usize>);

impl Drop for Parked<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// Whether `bare` names a tool in Clax's own registry.
fn clax_tool(bare: &str) -> bool {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| {
            clax_mcp::tools::ClaxTools::tools()
                .into_iter()
                .map(|t| t.name.to_string())
                .collect()
        })
        .iter()
        .any(|n| n == bare)
}

/// `POST /api/sessions/<sid>/tool-call-ids` (token): Claude Code's
/// PostToolUse hook reports `{tool_use_id, tool_name, args_sha256}` for a
/// Clax tool call, and `tool.call_id` is recorded as the session's agent
/// (spec §6.7). It names the one call that qualifies ([`CallIdScan`]): a
/// `tool.call` of the session with the same bare tool name and argument
/// hash, that ended within 5 s of the report's arrival, and that no harness
/// call ID names yet. With none, the report waits up to
/// [`CallIdWait::grace`] for one, woken by its session's `tool.call`
/// records, then records `call_id: null`; with more than one, it records
/// `null` at once. A report records `null` at once, too, when another
/// report for the same session, tool and argument hash is looking: the
/// two cannot tell their calls apart ([`CallIdWait::look`]). The scans run
/// on a reader; the writer is taken only to record, scanning again.
///
/// 201 `{recorded: true, seq, call_id}`; a harness call ID already
/// recorded answers 200 `{recorded: false}`. A report that fails
/// [`ToolCallIdReport::validate`] is 400 `invalid_tool_call_id`; a tool
/// name that is not one of Clax's own tools is 204, recording nothing; a
/// session that never existed is 404 `unknown_session`.
///
/// [`CallIdScan`]: clax_core::store::audit::CallIdScan
/// [`ToolCallIdReport::validate`]: clax_core::audit::ToolCallIdReport::validate
pub async fn tool_call_ids(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: DeferredAudit,
    id: Result<Path<String>, PathRejection>,
    req: Result<Json<clax_core::audit::ToolCallIdReport>, JsonRejection>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    use clax_core::store::audit::{CallIdMatch, CallIdScan};
    let arrival = chrono::Utc::now();
    let sid = path(id)?;
    if !clax_core::ids::is_ulid(&sid) {
        return Err(ApiError::bad_request(
            "invalid_session",
            "the session ID is not a ULID",
        ));
    }
    let report = body(req)?;
    report
        .validate()
        .map_err(|why| ApiError::bad_request("invalid_tool_call_id", why))?;
    if !clax_tool(report.bare_tool()) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let found = {
        let sid = sid.clone();
        s.store_call(move |st| {
            if st.session_actor(&sid)?.is_none() {
                return Ok(None);
            }
            audit.for_session(st, &sid).map(Some)
        })
        .await?
    };
    let Some(ctx) = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_session",
            "no such session",
        ));
    };
    let report = Arc::new(report);
    let deadline = tokio::time::Instant::now() + s.call_ids.grace;
    let wake = s.call_ids.watch(&sid);
    let looking = s
        .call_ids
        .look(&sid, report.bare_tool(), &report.args_sha256);
    let mut parked = None;
    loop {
        // Listening before looking, so a call recorded in between wakes it.
        let recorded = wake.notify.notified();
        tokio::pin!(recorded);
        recorded.as_mut().enable();
        if looking.contested() {
            break;
        }
        let (r, id) = (report.clone(), sid.clone());
        let scan = s
            .store_call(move |st| st.scan_tool_call_id(&id, &r, arrival))
            .await?;
        if scan != CallIdScan::None || tokio::time::Instant::now() >= deadline {
            break;
        }
        if parked.is_none() {
            parked = s.call_ids.park();
            if parked.is_none() {
                break;
            }
        }
        if tokio::time::timeout_at(deadline, recorded).await.is_err() {
            break;
        }
    }
    let contested = looking.contested();
    let outcome = s
        .store_call(move |st| st.record_tool_call_id(&ctx, &sid, &report, arrival, contested))
        .await?;
    // Held until recorded, so a report arriving meanwhile is contested too.
    drop((parked, looking, wake));
    Ok(match outcome {
        CallIdMatch::Recorded { seq, call_id } => (
            StatusCode::CREATED,
            Json(json!({"recorded": true, "seq": seq, "call_id": call_id})),
        )
            .into_response(),
        CallIdMatch::Repeated => Json(json!({"recorded": false})).into_response(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[test]
    fn a_call_wakes_only_its_own_sessions_reports() {
        let wait = CallIdWait::new(CALL_ID_GRACE, CALL_ID_CAP);
        let a = wait.watch("A");
        let b = wait.watch("B");
        {
            let woken_a = a.notify.notified();
            let woken_b = b.notify.notified();
            tokio::pin!(woken_a, woken_b);
            woken_a.as_mut().enable();
            woken_b.as_mut().enable();
            wait.recorded("A");
            assert!(woken_a.now_or_never().is_some(), "A's report is woken");
            assert!(woken_b.now_or_never().is_none(), "B's report is not");
        }
        drop((a, b));
        assert!(
            wait.sessions.lock().unwrap().is_empty(),
            "a session no report holds is forgotten"
        );
    }

    #[test]
    fn two_reports_for_one_call_contest_each_other() {
        let wait = CallIdWait::new(CALL_ID_GRACE, CALL_ID_CAP);
        let first = wait.look("S", "list", "h");
        assert!(!first.contested());
        // Another session, tool or hash does not contest it.
        let others = [
            wait.look("T", "list", "h"),
            wait.look("S", "read", "h"),
            wait.look("S", "list", "g"),
        ];
        assert!(!first.contested());
        assert!(others.iter().all(|o| !o.contested()));
        let second = wait.look("S", "list", "h");
        assert!(first.contested() && second.contested());
        drop((first, second, others));
        assert!(wait.looking.lock().unwrap().is_empty());
        assert!(!wait.look("S", "list", "h").contested());
    }

    #[test]
    fn no_more_than_cap_reports_wait() {
        let wait = CallIdWait::new(CALL_ID_GRACE, 2);
        let one = wait.park().unwrap();
        let _two = wait.park().unwrap();
        assert!(wait.park().is_none());
        assert_eq!(*wait.parked.borrow(), 2);
        drop(one);
        assert!(wait.park().is_some());
    }

    #[test]
    fn only_clax_tools_are_reported() {
        assert!(clax_tool("publish"));
        assert!(clax_tool("wait_for_feedback"));
        assert!(!clax_tool("frobnicate"));
    }
}
