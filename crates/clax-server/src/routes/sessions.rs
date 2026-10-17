//! REST routes for harness sessions.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::CoreError;
use clax_core::model::Session;
use clax_http::agent::{
    SessionsGet, SessionsJoin, SessionsList, SessionsPatch, SessionsRegister, TokenOrAgent,
};
use clax_http::audit::DeferredAudit;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_realtime::push::CodexPush;
use clax_store::{JoinSession, NameSource, Proved, RegisterSession, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// The harness names a session may carry.
pub const HARNESSES: [&str; 4] = ["claude", "codex", "grok", "pi"];

/// The harness of a session another machine's Clax registers to publish
/// its person's own versions there, made by hand (the CLI or the browser)
/// and by no agent (`clax_http::public`). Taken only from an agent token a
/// device sign-in issued to another Clax (not one minted for an agent); what
/// such a session publishes is recorded as its person's, with no session
/// ([`crate::routes::artifacts::publishing_session`]), so it reads as the
/// person's own publish.
pub const BY_HAND_HARNESS: &str = "clax";

/// `invalid_args` unless `harness` is one of [`HARNESSES`], or
/// [`BY_HAND_HARNESS`] for a device sign-in's agent token (`by_hand`).
fn check_harness(harness: &str, by_hand: bool) -> Result<(), ApiError> {
    if HARNESSES.contains(&harness) || (by_hand && harness == BY_HAND_HARNESS) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "invalid_args",
            format!("harness must be one of {}", HARNESSES.join(", ")),
        ))
    }
}

/// Registers a session under the caller's credential (spec §6.7: a
/// grant's agent under its grant, matching only its grant's rows);
/// `harness` must be one of [`HARNESSES`]. A `transcript_path` (the
/// harness's extension API's session file) is kept as a join's is. A new
/// session records `session.start`, and a refresh that changes it
/// `session.join`, as its own agent (audit spec §6.8). A grant's
/// registration refreshes a live row only when it proved that session, and
/// a new grant row's secret is answered once as `session_secret` (spec
/// 2026-10-08 §6.8).
pub async fn register(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<SessionsRegister>,
    audit: DeferredAudit,
    req: Result<Json<RegisterSession>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut r = body(req)?;
    let by_hand = match (a.grant(), r.harness == BY_HAND_HARNESS) {
        (Some(g), true) => {
            let g = g.to_string();
            s.store_call(move |st| st.is_device_grant(&g)).await?
        }
        _ => false,
    };
    check_harness(&r.harness, by_hand)?;
    r.grant_id = a.grant().map(str::to_string);
    if r.grant_id.is_some() {
        // A process of another machine: never probed as a local one by the
        // reaper (`parent_pid` stays, for matching within the grant).
        r.pid = None;
    }
    r.transcript_path = transcript(r.transcript_path)?;
    let audit = audit.agent_side();
    let proved = a.caller.session().map(str::to_string);
    let (ctx, state) = (s.feedback_ctx(), s.clone());
    let reg = s
        .store_call(move |st| {
            let gid = r.grant_id.clone();
            let reg = st.register_session_proved(&audit, r, Proved::Session(proved.as_deref()))?;
            // An agent token holds at most `BEARER_SESSION_CAP` live
            // sessions: the least recently seen beyond them end once this
            // one stands, so a narrowed one it inherits from (spec §6.9)
            // is still live as it registers.
            if let Some(gid) = &gid {
                let over = st.end_bearer_sessions_beyond(gid, Store::BEARER_SESSION_CAP)?;
                if !over.ended.is_empty() {
                    let ttl =
                        clax_core::audit::AuditCtx::system(clax_core::audit::SystemReason::Ttl);
                    crate::feedback::sessions_ended(
                        &state,
                        &ctx,
                        st,
                        &ttl,
                        &over.touched,
                        &over.withdrawn_questions,
                        &over.ended,
                    );
                }
            }
            Ok(reg)
        })
        .await?;
    restored_note(&s, &reg.session, reg.restored);
    Ok((StatusCode::CREATED, Json(answer(&reg.session, reg.secret))))
}

/// A registration's or join's answer: `{"session": …}`, with
/// `session_secret` when the call issued the new session's secret (spec
/// 2026-10-08 §6.8).
fn answer(session: &Session, secret: Option<clax_core::secret::Secret>) -> Value {
    let mut v = json!({"session": session});
    if let Some(secret) = secret {
        v["session_secret"] = json!(secret.expose());
    }
    v
}

/// Keeps a restored session's note for its first feedback poll (spec
/// 2026-10-08 §6.7).
fn restored_note(s: &AppState, session: &Session, restored: Option<clax_store::Restored>) {
    if let Some(r) = restored {
        s.feedback_waiters.set_note(&session.id, r.note);
    }
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
    /// The repository and branch of the hook's `cwd` (spec §11), which
    /// name a row the join makes or a hook's row named from its harness.
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

/// Joins a harness session ID to its session among the caller's
/// credential's (see `Store::join_session`) and, for Codex, records
/// `codex_home`; a join without it keeps the recorded value. A grant's
/// `codex_home` names a directory of another machine and is ignored. A join that makes or changes the session records
/// `session.start` or `session.join`, as its own agent (audit spec §6.8).
/// A grant's join takes a live row only when it proved that session, and a
/// row it makes answers its secret once as `session_secret` (spec
/// 2026-10-08 §6.8).
pub async fn join(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<SessionsJoin>,
    audit: DeferredAudit,
    req: Result<Json<JoinBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let grant = a.grant().map(str::to_string);
    let b = body(req)?;
    check_harness(&b.harness, false)?;
    if b.harness_session_id.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_session",
            "harness_session_id must not be empty",
        ));
    }
    let transcript = transcript(b.transcript_path.clone())?;
    let audit = audit.agent_side();
    let proved = a.caller.session().map(str::to_string);
    let reg = s
        .store_call(move |st| {
            let reg = st.join_session_proved(
                &audit,
                JoinSession {
                    harness: b.harness.clone(),
                    parent_pid: b.parent_pid,
                    harness_session_id: b.harness_session_id,
                    cwd: b.cwd,
                    ancestor_pids: b.ancestor_pids,
                    transcript_path: transcript,
                    repo: b.repo,
                    branch: b.branch,
                },
                grant.as_deref(),
                Proved::Session(proved.as_deref()),
            )?;
            if let Some(h) = b
                .codex_home
                .as_deref()
                .filter(|_| b.harness == "codex" && grant.is_none())
            {
                st.set_codex_home(&reg.session.id, h)?;
            }
            Ok(reg)
        })
        .await?;
    restored_note(&s, &reg.session, reg.restored);
    Ok(Json(answer(&reg.session, reg.secret)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    #[serde(default)]
    heartbeat: bool,
    #[serde(default)]
    ended: bool,
    /// The agent's own name for itself (spec §11).
    #[serde(default)]
    name: Option<String>,
}

/// Names, heartbeats or ends one of the caller's sessions (the extractor
/// checks the session is the caller's). `{"name": …}` is the agent naming
/// itself (spec §11: 409 `name_taken`, 409 `name_locked` once its person
/// named it, 400 `invalid_name`), done before a heartbeat or an end in the
/// same request. `{"heartbeat": true}` renews the session (records
/// nothing); `{"ended": true}` ends it, recording `session.end` as its
/// agent and the ends of its working records.
pub async fn patch(
    State(s): State<AppState>,
    _a: TokenOrAgent<SessionsPatch>,
    audit: DeferredAudit,
    id: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(id)?;
    let b = body(req)?;
    if !b.heartbeat && !b.ended && b.name.is_none() {
        return Err(ApiError::bad_request(
            "invalid_session_patch",
            "send {\"heartbeat\": true}, {\"ended\": true} or {\"name\": \"…\"}",
        ));
    }
    let ctx = s.feedback_ctx();
    let state = s.clone();
    let session = s
        .store_call(move |st| {
            let audit = audit.for_session(st, &id)?;
            let named = match &b.name {
                Some(n) => Some(st.set_session_name(&audit, &id, n, NameSource::Agent)?),
                None => None,
            };
            if b.ended {
                let ended = st.end_session_touched(&audit, &id)?;
                clax_http::state::sessions_ended(
                    &state,
                    &ctx,
                    st,
                    &audit,
                    &ended.touched,
                    &ended.withdrawn_questions,
                    std::slice::from_ref(&id),
                );
                Ok(ended.session)
            } else if b.heartbeat {
                st.heartbeat(&id)
            } else {
                Ok(named.expect("a name was set"))
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

/// Lists the caller's credential's sessions: a grant's own, or the token's
/// (spec §6.7). Session rows carry working directories and process IDs,
/// so reading them needs a credential.
pub async fn list(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<SessionsList>,
    q: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let grant = a.grant().map(str::to_string);
    let sessions = s
        .store_call(move |st| st.list_sessions_for(grant.as_deref(), q.live))
        .await?;
    Ok(Json(json!({"sessions": sessions})))
}

/// One of the caller's sessions, how feedback can be pushed to it
/// ([`push_info`]), and whom it acts for and with which rights (spec §16,
/// the `status` tool): `person` is `{public_id, name, owner}` (the
/// owner's agents act for the owner, whose `public_id` is null before the
/// owner has an identity), and `rights` the caller's rights (the access
/// array; the daemon token's are the full preset). Needs a credential, as
/// [`list`] does.
pub async fn get(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<SessionsGet>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(id)?;
    let rights = match &a.caller {
        clax_http::auth::Caller::PersonAgent { rights, .. } => rights.to_json(),
        _ => clax_core::rights::Rights::full().to_json(),
    };
    let (session, codex_home, push_error, person) = s
        .store_call(move |st| {
            let session = st.get_session(&id)?.ok_or(CoreError::NotFound)?;
            let codex_home = st.codex_home(&id)?;
            let push_error = st.push_error(&id)?;
            let person = match &session.person {
                Some(p) => {
                    let v = st.viewer_by_public_id(p)?;
                    json!({"public_id": p, "owner": false,
                           "name": clax_core::feedback::person_name(
                               v.as_ref().and_then(|v| v.display_name.as_deref()), false)})
                }
                None => {
                    let v = st.owner()?;
                    json!({"public_id": v.as_ref().map(|v| v.public_id.clone()), "owner": true,
                           "name": clax_core::feedback::person_name(
                               v.as_ref().and_then(|v| v.display_name.as_deref()), true)})
                }
            };
            Ok((session, codex_home, push_error, person))
        })
        .await?;
    let live = Live::of(&s.followers, &s.feedback_waiters, &session.id);
    let away = elsewhere(&session, s.hosted.is_some());
    let push = push_info(&session, &s.codex, codex_home, push_error, live, away);
    Ok(Json(
        json!({"session": session, "push": push, "person": person, "rights": rights}),
    ))
}

/// Why nothing wakes an idle Codex session on another machine.
const CODEX_ELSEWHERE: &str = "this Codex session runs on another machine than this Clax, whose codex queue cannot reach it; comments arrive at the end of its turn (Stop hook), on its next clax tool call, or during wait_for_feedback";

/// Whether session `s` runs on another machine than this daemon: a
/// grant's session, or any session of a hosted daemon.
pub fn elsewhere(s: &Session, hosted: bool) -> bool {
    hosted || s.grant_id.is_some()
}

/// Why nothing wakes an idle Claude Code session that no notice follower
/// polls for: optional, since comments still arrive without push, and how to
/// turn it on.
const CLAUDE_NO_PUSH: &str = "optional: comments sent to this session already arrive with the next clax tool result, at the end of each turn, with the next message, and during wait_for_feedback; push only wakes the session while it is idle. To have a comment wake it, run follow_command in the background after publishing, or launch Claude Code with `claude --dangerously-load-development-channels plugin:clax@clax` (Claude Code channels, a research preview)";

/// What is connected for a session that can wake it ([`Followers`]).
///
/// [`Followers`]: crate::feedback::Followers
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    /// A `clax feedback follow` polls its notices.
    pub follower: bool,
    /// The Claude Code shim polls its notices for the channel.
    pub channel: bool,
    /// The Pi extension's injection loop polls, or holds while the
    /// session's `wait_for_feedback` (which hands it every comment) waits.
    pub inject: bool,
}

impl Live {
    pub fn of(
        followers: &crate::feedback::Followers,
        waiters: &crate::feedback::FeedbackWaiters,
        sid: &str,
    ) -> Live {
        use crate::feedback::FollowKind;
        Live {
            follower: followers.has(sid, FollowKind::Follower),
            channel: followers.has(sid, FollowKind::Channel),
            inject: followers.has(sid, FollowKind::Inject) || waiters.is_waiting(sid),
        }
    }
}

/// Why the Pi extension does not wake this session.
const PI_NO_LOOP: &str = "the Clax Pi extension's injection loop is not polling for this session (Pi has not started it, it has shut down, or the client is not the Pi extension); comments arrive on the next clax tool call or during wait_for_feedback";

/// How feedback can be pushed to this session (tier 5), and why not when it
/// cannot. For Codex, `last_error` and `last_error_at` hold the latest
/// `codex queue` failure (`null` after a success or before any run); a
/// failure leaves push available, since the next comment is pushed again.
/// For Pi, push is the extension's injection loop: available while it
/// polls. For Grok, push is the monitor: available while a `clax feedback
/// follow` of the session is connected. For Claude Code, the daemon
/// reports a notice follower as tier `notice`, and the shim's channel
/// alone as tier `channel` with `sure: false` (Claude Code never says
/// whether it registered the channel); the shim refines it.
fn push_info(
    s: &Session,
    codex: &CodexPush,
    codex_home: Option<String>,
    push_error: Option<(String, String)>,
    live: Live,
    elsewhere: bool,
) -> Value {
    match s.harness.as_str() {
        // This daemon's `codex` never queues to a session on another
        // machine (`dispatch` skips it), whatever its own PATH holds.
        "codex" if elsewhere => {
            json!({"tier": null, "available": false, "other_machine": true,
                "reason": CODEX_ELSEWHERE, "codex_home": codex_home,
                "last_error": null, "last_error_at": null})
        }
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
        "pi" => json!({"tier": "inject", "available": live.inject,
            "reason": (!live.inject).then_some(PI_NO_LOOP)}),
        "grok" => {
            let reason = (!live.follower).then_some(
                "no clax feedback follow is running for this session; the clax-grok skill starts one with Grok's monitor tool after a publish. Meanwhile comments arrive at the end of a turn (Stop hook), on the next clax tool call, or during wait_for_feedback",
            );
            json!({"tier": "monitor", "available": live.follower, "reason": reason})
        }
        "claude" if live.follower => json!({"tier": "notice", "available": true, "reason": null}),
        "claude" if live.channel => {
            json!({"tier": "channel", "available": true, "sure": false, "reason": null})
        }
        "claude" => json!({"tier": null, "available": false, "reason": CLAUDE_NO_PUSH}),
        _ => {
            json!({"tier": null, "available": false, "reason": "Claude Code has no native push; comments arrive at the end of a turn (Stop hook), with the next prompt, on the next clax tool call, or during wait_for_feedback"})
        }
    }
}

/// Whether a comment wakes a live session while it is idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    Yes,
    No,
    /// Only through a Claude Code channel, which Claude Code may have
    /// refused without saying so.
    Unsure,
}

impl Wake {
    /// `wakes` as the page reads it: `true`, `false` or `"unsure"`.
    pub fn to_json(self) -> Value {
        match self {
            Wake::Yes => json!(true),
            Wake::No => json!(false),
            Wake::Unsure => json!("unsure"),
        }
    }
}

/// Whether a comment sent to a live session of `harness` wakes it while it
/// is idle, as [`push_info`] reports `available` (and `sure`): Codex
/// through `codex queue` (the daemon's `codex`, the harness session ID
/// known, and the session on this daemon's machine, not `elsewhere`), Pi
/// through its extension's injection loop, Grok Build while a notice
/// follower is connected, and Claude Code while one is, or unsure while
/// only its channel polls.
pub fn wakes_when_idle(
    harness: &str,
    harness_session_known: bool,
    elsewhere: bool,
    codex: &CodexPush,
    live: Live,
) -> Wake {
    let yes = |b: bool| if b { Wake::Yes } else { Wake::No };
    match harness {
        "codex" => yes(!elsewhere && codex.available() && harness_session_known),
        "pi" => yes(live.inject),
        "grok" => yes(live.follower),
        "claude" if live.follower => Wake::Yes,
        "claude" if live.channel => Wake::Unsure,
        _ => Wake::No,
    }
}

/// Whose agents' `wakes` a listing shows.
#[derive(Clone, Debug)]
enum Audience {
    /// The owner: every agent's.
    All,
    /// A connected person (by public ID): their own agents' only.
    Person(String),
}

/// What [`wakes_when_idle`] reads of the daemon (its `codex`, the
/// connected followers and waits), and whose agents it is shown for.
#[derive(Clone)]
pub struct Wakes {
    codex: Arc<CodexPush>,
    followers: Arc<crate::feedback::Followers>,
    waiters: Arc<crate::feedback::FeedbackWaiters>,
    hosted: bool,
    audience: Audience,
}

impl Wakes {
    /// For the owner: every agent's `wakes`.
    pub fn of(s: &AppState) -> Wakes {
        Wakes {
            codex: s.codex.clone(),
            followers: s.followers.clone(),
            waiters: s.feedback_waiters.clone(),
            hosted: s.hosted.is_some(),
            audience: Audience::All,
        }
    }

    /// [`Wakes::of`] for `caller`'s views: none for an agent, whose
    /// outputs stay as they were (the page is what shows `wakes`).
    pub fn for_caller(s: &AppState, caller: &clax_http::auth::Caller) -> Option<Wakes> {
        (!caller.is_agent()).then(|| Wakes::of(s))
    }

    /// These `wakes` for a delivery viewer: every agent's for the owner, a
    /// connected person's own agents' for that person, none for anyone
    /// else.
    pub fn for_viewer(&self, v: &clax_core::DeliveryViewer) -> Option<Wakes> {
        let audience = match v {
            clax_core::DeliveryViewer::Owner => Audience::All,
            clax_core::DeliveryViewer::Person(p) => Audience::Person(p.clone()),
            _ => return None,
        };
        Some(Wakes {
            audience,
            ..self.clone()
        })
    }

    /// Whether this listing shows `wakes` for an agent of person `pid`
    /// (its public ID; `None` for the owner's agents, whose person is
    /// `owner`).
    fn shows(&self, pid: Option<&str>, owner_agent: bool) -> bool {
        match &self.audience {
            Audience::All => true,
            Audience::Person(me) => !owner_agent && pid == Some(me.as_str()),
        }
    }

    fn live(&self, sid: &str) -> Live {
        Live::of(&self.followers, &self.waiters, sid)
    }

    /// `wakes` of live session `x`, when this listing shows it.
    pub fn session(&self, x: &Session) -> Option<Value> {
        self.shows(x.person.as_deref(), x.person.is_none())
            .then(|| {
                wakes_when_idle(
                    &x.harness,
                    x.harness_session_id.is_some(),
                    elsewhere(x, self.hosted),
                    &self.codex,
                    self.live(&x.id),
                )
                .to_json()
            })
    }

    /// Sets `wakes` on each live agent of `p`, when `w` is given.
    pub fn mark_with(w: Option<&Wakes>, p: &mut clax_store::attention::Participants) {
        if let Some(w) = w {
            w.mark(p);
        }
    }

    /// Sets `wakes` on each live agent of `p` this listing shows it for.
    pub fn mark(&self, p: &mut clax_store::attention::Participants) {
        for a in p.agents.iter_mut().filter(|a| a.live) {
            if !self.shows(a.person.public_id.as_deref(), a.person.owner) {
                continue;
            }
            a.wakes = Some(
                wakes_when_idle(
                    &a.harness,
                    a.harness_session_known,
                    self.hosted || a.grant,
                    &self.codex,
                    self.live(&a.session_id),
                )
                .to_json(),
            );
        }
    }
}

/// `GET /api/push`: whether the daemon can push to Codex, where its `codex`
/// came from, and why push is unavailable when it is. `bin`, the path of the
/// daemon's `codex` (it usually names the user's home), is included only for
/// a request with the token.
pub async fn push_status(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
) -> Json<Value> {
    let mut codex = json!({
        "available": s.codex.available(),
        "source": s.codex.source,
        "reason": s.codex.reason(),
    });
    if d.holds_token() {
        codex["bin"] = json!(
            s.codex
                .bin
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
        );
    }
    Json(json!({ "codex": codex }))
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

/// `POST /api/sessions/<sid>/tool-call-ids` (token, or an agent of another
/// machine for its own session): Claude Code's
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
/// [`CallIdScan`]: clax_store::audit::CallIdScan
/// [`ToolCallIdReport::validate`]: clax_core::audit::ToolCallIdReport::validate
pub async fn tool_call_ids(
    State(s): State<AppState>,
    _a: clax_http::agent::TokenOrAgent<clax_http::agent::ToolCallIdReport>,
    audit: DeferredAudit,
    id: Result<Path<String>, PathRejection>,
    req: Result<Json<clax_core::audit::ToolCallIdReport>, JsonRejection>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    use clax_store::audit::{CallIdMatch, CallIdScan};
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
    let deadline = clax_rt::time::Instant::now() + s.call_ids.grace;
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
        if scan != CallIdScan::None || clax_rt::time::Instant::now() >= deadline {
            break;
        }
        if parked.is_none() {
            parked = s.call_ids.park();
            if parked.is_none() {
                break;
            }
        }
        if clax_rt::time::timeout_at(deadline, recorded).await.is_err() {
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

    #[test]
    fn wakes_when_idle_is_what_push_info_reports_available() {
        let off = CodexPush::from_env(Some("".into()), None);
        let on = CodexPush::from_env(Some("/bin/sh".into()), None);
        assert!(on.available());
        let lives = [
            Live::default(),
            Live {
                follower: true,
                ..Live::default()
            },
            Live {
                channel: true,
                ..Live::default()
            },
            Live {
                inject: true,
                ..Live::default()
            },
            Live {
                follower: true,
                channel: true,
                inject: true,
            },
        ];
        for harness in ["claude", "codex", "grok", "pi", "by-hand"] {
            for codex in [&off, &on] {
                for live in lives {
                    for hs in [None, Some("h".to_string())] {
                        for elsewhere in [false, true] {
                            let session: Session = serde_json::from_value(json!({
                                "id": "s", "harness": harness, "pid": null, "parent_pid": 1, "cwd": "/", "ended_at": null, "agent_handle": "a",
                                "started_at": "2026-01-01T00:00:00Z", "last_seen_at": "2026-01-01T00:00:00Z",
                                "harness_session_id": hs,
                            }))
                            .unwrap();
                            let push = push_info(&session, codex, None, None, live, elsewhere);
                            let want = match (push["available"] == true, push["sure"] == false) {
                                (false, _) => Wake::No,
                                (true, true) => Wake::Unsure,
                                (true, false) => Wake::Yes,
                            };
                            assert_eq!(
                                wakes_when_idle(harness, hs.is_some(), elsewhere, codex, live),
                                want,
                                "{harness} {live:?} hs={hs:?} elsewhere={elsewhere} {push}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The daemon's own `codex` never queues to a session on another
    /// machine (a grant's, or any on a hosted daemon): push says so.
    #[test]
    fn codex_on_another_machine_is_not_woken_by_this_daemons_codex() {
        let on = CodexPush::from_env(Some("/bin/sh".into()), None);
        let session: Session = serde_json::from_value(json!({
            "id": "s", "harness": "codex", "pid": null, "parent_pid": 1, "cwd": "/", "ended_at": null, "agent_handle": "a",
            "started_at": "2026-01-01T00:00:00Z", "last_seen_at": "2026-01-01T00:00:00Z",
            "harness_session_id": "h",
        }))
        .unwrap();
        let here = push_info(&session, &on, None, None, Live::default(), false);
        assert_eq!(
            (&here["available"], &here["tier"]),
            (&json!(true), &json!("queue"))
        );
        let there = push_info(&session, &on, None, None, Live::default(), true);
        assert_eq!(there["available"], false, "{there}");
        assert_eq!(there["tier"], Value::Null);
        assert_eq!(there["other_machine"], true);
        assert!(
            there["reason"]
                .as_str()
                .unwrap()
                .contains("another machine"),
            "{there}"
        );
        assert_eq!(
            wakes_when_idle("codex", true, true, &on, Live::default()),
            Wake::No
        );
    }

    /// A client named pi is woken only while the extension's injection
    /// loop polls (or its wait_for_feedback waits); a channel alone leaves
    /// Claude Code unsure.
    #[test]
    fn pi_needs_its_injection_loop_and_a_channel_alone_is_unsure() {
        let off = CodexPush::from_env(Some("".into()), None);
        assert_eq!(
            wakes_when_idle("pi", true, false, &off, Live::default()),
            Wake::No
        );
        assert_eq!(
            wakes_when_idle(
                "pi",
                true,
                false,
                &off,
                Live {
                    inject: true,
                    ..Live::default()
                }
            ),
            Wake::Yes
        );
        let channel = Live {
            channel: true,
            ..Live::default()
        };
        assert_eq!(
            wakes_when_idle("claude", true, false, &off, channel),
            Wake::Unsure
        );
        assert_eq!(Wake::Unsure.to_json(), json!("unsure"));
        let both = Live {
            channel: true,
            follower: true,
            ..Live::default()
        };
        assert_eq!(
            wakes_when_idle("claude", true, false, &off, both),
            Wake::Yes
        );
    }

    #[test]
    fn only_clax_tools_are_reported() {
        assert!(clax_tool("publish"));
        assert!(clax_tool("wait_for_feedback"));
        assert!(!clax_tool("frobnicate"));
    }
}
