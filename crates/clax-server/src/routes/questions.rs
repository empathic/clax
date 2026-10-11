//! Agent questions (spec 2026-10-06-agent-questions-and-inbox §6.1).
//! Session routes (token) let the asking session create, wait on, withdraw,
//! release and record the terminal answer of its own questions; another
//! session's question is 404. A question view an agent gets passes through
//! its `Visibility` (spec 2026-10-08-multi-agent-multi-person §8.2): the
//! question and its answers are its person's, the artifact's title is its
//! authors'. The person routes (§6.2), person-scoped (spec §18), let the
//! owner list, read, answer, decline and release the questions of the
//! owner's agents, and a connected person those of their own agents;
//! another person's question is 404, any other caller 403, and they keep
//! the viewer routes' origin rules.

use super::artifacts::{body, body_within, path};
use crate::agent::{DeniedToGrants, TokenOrAgent};
use crate::audit::DeferredAudit;
use crate::auth::Caller;
#[cfg(debug_assertions)]
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::feedback::VisFor;
use crate::identity::Identity;
use crate::inbox::recipient_of;
use crate::questions::{announce, arm_grace, start_grace, view as question_view};
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::questions::{Answer, Question, from_claude, from_claude_answers, validate_ask};
use clax_core::store::questions::{Close, ListStatus, NewQuestion, Source, Status};
use clax_core::{CoreError, Store};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::time::Duration;
use tokio::time::Instant;

/// Longest `wait` of a question poll, in seconds.
pub const MAX_WAIT_SECS: u64 = 3600;
/// Largest accepted ask body.
pub const ASK_BODY_LIMIT: usize = 128 * 1024;

/// A question route's failure: an API error, or 409 `question_closed` with
/// the question's view beside the error so the caller can say what
/// happened (spec §6.2).
#[derive(Debug)]
pub enum QuestionError {
    Api(ApiError),
    Closed { message: String, question: Value },
}

impl From<ApiError> for QuestionError {
    fn from(e: ApiError) -> Self {
        QuestionError::Api(e)
    }
}

impl IntoResponse for QuestionError {
    fn into_response(self) -> Response {
        match self {
            QuestionError::Api(e) => e.into_response(),
            QuestionError::Closed { message, question } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": {"code": "question_closed", "message": message},
                    "question": question,
                })),
            )
                .into_response(),
        }
    }
}

/// Checks that session `sid` exists (404) and is live (400
/// `unknown_session`), as every session route does (spec §6.1).
fn live(db: &Store, sid: &str) -> clax_core::Result<()> {
    match db.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "no live session has this ID",
        )),
        Some(_) => Ok(()),
    }
}

/// `v`, a question's view, as the asking agent may see it (spec §8.2): its
/// artifact's title, an author field, through the agent's visibility `vf`
/// ([`crate::feedback::Vis::filter_question`]). Runs on a store thread.
fn for_asker(db: &Store, vf: &VisFor, mut v: Value) -> clax_core::Result<Value> {
    vf.build(db)?.filter_question(db, &mut v)?;
    Ok(v)
}

/// [`for_asker`] on the question of a [`Closed`].
fn closed_for_asker(db: &Store, vf: &VisFor, c: Closed) -> clax_core::Result<Closed> {
    Ok(match c {
        Closed::Done(v) => Closed::Done(for_asker(db, vf, v)?),
        Closed::Already(m, v) => Closed::Already(m, for_asker(db, vf, v)?),
    })
}

/// What applying a [`Close`] came to.
pub(crate) enum Closed {
    /// Applied and announced; the new view.
    Done(Value),
    /// The question's status no longer allowed it; why, and its view.
    Already(String, Value),
}

impl Closed {
    pub(crate) fn respond(self) -> Result<Json<Value>, QuestionError> {
        match self {
            Closed::Done(v) => Ok(Json(json!({"question": v}))),
            Closed::Already(message, question) => Err(QuestionError::Closed { message, question }),
        }
    }
}

/// Applies `c` to question `qid` under `ctx` and announces it. Runs on a
/// store thread.
pub(crate) fn close(
    s: &AppState,
    db: &Store,
    ctx: &AuditCtx,
    qid: &str,
    c: Close,
) -> clax_core::Result<Closed> {
    match db.close_question(ctx, qid, c) {
        Ok(q) => {
            let v = question_view(db, &q)?;
            announce(s, db, &q, v.clone());
            Ok(Closed::Done(v))
        }
        Err(CoreError::Invalid {
            code: "question_closed",
            message,
        }) => {
            let q = db.question(qid)?.ok_or(CoreError::NotFound)?;
            Ok(Closed::Already(message, question_view(db, &q)?))
        }
        Err(e) => Err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    questions: Value,
    #[serde(default)]
    artifact_id: Option<String>,
    source: String,
    #[serde(default)]
    tool_use_id: Option<String>,
}

/// `POST /api/sessions/<sid>/questions`: records a question set for live
/// session `sid` → 201 `{question, mode, terminal_after_s, surface_open}`
/// (200 with the first question for a `tool_use_id` seen before). A hook
/// question takes Claude Code's AskUserQuestion input as it is, and when it
/// names no artifact, the artifact of the session's newest working record,
/// else the session's artifact (`Store::session_artifact`); it is created
/// moved to the terminal (`mode: "terminal"`) when it is about no artifact,
/// no owner surface is open, or `terminal_after_s` is 0, and otherwise is withdrawn
/// when no poll holds it for the grace. A new question records
/// `question.ask` as the session's agent (and `question.release` when made
/// released).
pub async fn create(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::QuestionsCreate>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let sid = path(p)?;
    let b = body_within(req, "128 KiB")?;
    let source = match b.source.as_str() {
        "ask" => Source::Ask,
        "hook" => Source::Hook,
        _ => {
            return Err(ApiError::bad_request(
                "invalid_question",
                "source is ask or hook",
            ));
        }
    };
    let questions = match source {
        Source::Hook => from_claude(&json!({"questions": b.questions}))?,
        Source::Ask => {
            let qs: Vec<Question> = serde_json::from_value(b.questions)
                .map_err(|e| ApiError::bad_request("invalid_question", e.to_string()))?;
            validate_ask(&qs)?;
            qs
        }
    };
    // A mirrored question waits in Clax only while its person has Clax
    // open: the owner for the owner's agents, a connected person for theirs.
    let asker = match &a.caller {
        Caller::PersonAgent { person, .. } => person.clone(),
        _ => None,
    };
    let surface_open = s
        .stream
        .holds_surface(clax_core::store::inbox::recipient_key(asker.as_deref()));
    let vf = VisFor::of(&a.caller, &s);
    let after = s.terminal_after_s;
    let artifact_id = match (&b.artifact_id, source) {
        (Some(a), _) => Some(a.clone()),
        (None, Source::Hook) => s.working.newest_artifact_of(&sid),
        (None, Source::Ask) => None,
    };
    let named = b.artifact_id.is_some();
    // The artifact through the agent's gate: a named one must parse (and,
    // for a person's agent, be one it may read); a working record's that
    // does not is about none.
    let artifact_id = match artifact_id {
        Some(raw) => match a.named_artifact(&s, &raw).await {
            Ok(id) => Some(id),
            // A malformed ID names no artifact (404), as for any caller.
            Err(e) if named && matches!(a.caller, Caller::PersonAgent { .. }) => return Err(e),
            Err(_) if named => return Err(ApiError::not_found()),
            Err(_) => None,
        },
        None => None,
    };
    // The session's artifact, which a hook question about no live artifact
    // falls back to, goes through the same gate as a named one: a person's
    // agent's watches never hand it a live page, or one it may not read.
    let fallback = if source == Source::Hook && !named {
        let id = sid.clone();
        match s.store_call(move |db| db.session_artifact(&id)).await? {
            Some(raw) => a.named_artifact(&s, &raw).await.ok(),
            None => None,
        }
    } else {
        None
    };
    let tool_use_id = b.tool_use_id;
    let st = s.clone();
    let (row, made, v) = s
        .store_call(move |db| {
            live(db, &sid)?;
            // A named artifact must be live; a working record's may have
            // gone since, and then the question is about the session's
            // artifact, if it has one.
            let mut live = match &artifact_id {
                Some(id) => db.get_artifact(id)?.map(|a| a.id),
                None => None,
            };
            if named && live.is_none() {
                return Err(CoreError::NotFound);
            }
            if live.is_none()
                && let Some(id) = &fallback
            {
                live = db.get_artifact(id)?.map(|a| a.id);
            }
            // A mirrored question waits in Clax only beside a page of its
            // session's: before the session has published or watched one,
            // it is asked in the terminal.
            let released =
                source == Source::Hook && (live.is_none() || !surface_open || after == 0);
            let ctx = audit.for_session(db, &sid)?;
            let (row, made) = db.create_question(
                &ctx,
                NewQuestion {
                    session_id: sid,
                    artifact_id: live,
                    source,
                    tool_use_id,
                    questions,
                    released,
                },
            )?;
            let v = question_view(db, &row)?;
            if made {
                announce(&st, db, &row, v.clone());
            }
            let v = for_asker(db, &vf, v)?;
            Ok((row, made, v))
        })
        .await?;
    if made && row.source == Source::Hook && row.status == Status::Open {
        start_grace(s.clone(), row.id.clone());
    }
    let mode = if row.status == Status::Released {
        "terminal"
    } else {
        "wait"
    };
    let out = json!({"question": v, "mode": mode, "terminal_after_s": after, "surface_open": surface_open});
    let status = if made {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(out)).into_response())
}

#[derive(Deserialize)]
pub struct WaitQuery {
    #[serde(default)]
    wait: u64,
}

/// `GET /api/sessions/<sid>/questions/<qid>?wait=<s>` → `{question,
/// waited_s}` as soon as the question is not open, or after `wait` seconds
/// (at most [`MAX_WAIT_SECS`]), or when the daemon shuts down. A daemon
/// that shuts down while a poll holds an open hook question withdraws it
/// (and announces it) before answering, as its next start would, recording
/// `question.withdraw` (reason `daemon_stop`) as `system:daemon`: the hook
/// then hands the question to the terminal as withdrawn, rather than
/// trying to release it on a daemon that is going away. An answered
/// or declined result marks it taken. While the poll runs it holds the
/// question, and feedback polls leave the question's answer to it. When
/// the last poll of an open hook question lets go, its grace starts; when
/// the last poll of an `ask` question lets go, its session's feedback polls
/// are woken, so an answer it left untaken is handed over at once. 429 `limit_reached` when
/// [`MAX_POLLS`](crate::questions::MAX_POLLS) polls hold it already (a
/// new poll is refused rather than an older one ended).
pub async fn poll(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::QuestionsPoll>,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, qid) = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let started = Instant::now();
    let wait = Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let vf = VisFor::of(&a.caller, &s);
    let (sid1, qid1) = (sid.clone(), qid.clone());
    let first = s
        .store_call(move |db| {
            live(db, &sid1)?;
            db.session_question(&sid1, &qid1)
        })
        .await?;
    // Only an open mirrored question can be withdrawn for want of a poll.
    // An `ask` question's answer that the last poll leaves untaken is a late
    // answer: its session's feedback polls are woken to take it.
    let on_last: Option<crate::questions::OnLast> = match first.source {
        Source::Hook => (first.status == Status::Open).then(|| arm_grace(s.clone(), qid.clone())),
        Source::Ask => {
            let (waiters, sid) = (s.feedback_waiters.clone(), sid.clone());
            Some(Box::new(move || waiters.wake(std::iter::once(&sid))))
        }
    };
    let Some((notify, _hold)) = s.questions.hold(&qid, on_last) else {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!(
                "at most {} polls wait on one question at once",
                crate::questions::MAX_POLLS
            ),
        ));
    };
    let mut shutdown = s.shutdown.clone();
    // A dropped sender means the state has no shutdown source; never end early then.
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let expiry = s.question_sleeper.sleep(wait);
    tokio::pin!(expiry);
    let mut done = wait.is_zero();
    let mut stopped = false;
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        #[cfg(debug_assertions)]
        if s.questions.take_expired(&qid) {
            done = true;
        }
        let (sid1, qid1, st, vf1) = (sid.clone(), qid.clone(), s.clone(), vf.clone());
        let out = s
            .store_call(move |db| {
                let mut row = db.session_question(&sid1, &qid1)?;
                if row.status == Status::Open && !done {
                    return Ok(None);
                }
                if stopped && row.status == Status::Open && row.source == Source::Hook {
                    close(&st, db, &AuditCtx::DAEMON, &row.id, Close::Shutdown)?;
                    row = db.session_question(&sid1, &qid1)?;
                }
                if matches!(row.status, Status::Answered | Status::Declined) {
                    db.take_question(&row.id)?;
                }
                let v = question_view(db, &row)?;
                for_asker(db, &vf1, v).map(Some)
            })
            .await?;
        if let Some(v) = out {
            return Ok(Json(
                json!({"question": v, "waited_s": started.elapsed().as_secs()}),
            ));
        }
        tokio::select! {
            () = &mut notified => {}
            () = &mut expiry, if !done => done = true,
            () = &mut stopping, if !done => (done, stopped) = (true, true),
        }
    }
}

/// Applies `c` to session `sid`'s question `qid`, as the session's agent
/// whose visibility is `vf`.
async fn session_close(
    s: AppState,
    vf: VisFor,
    audit: DeferredAudit,
    sid: String,
    qid: String,
    c: Close,
) -> Result<Json<Value>, QuestionError> {
    let st = s.clone();
    let closed = s
        .store_call(move |db| {
            live(db, &sid)?;
            db.session_question(&sid, &qid)?;
            let ctx = audit.for_session(db, &sid)?;
            let c = close(&st, db, &ctx, &qid, c)?;
            closed_for_asker(db, &vf, c)
        })
        .await?;
    closed.respond()
}

/// `POST /api/sessions/<sid>/questions/<qid>/withdraw` → `{question}`; 409
/// `question_closed` when it is not open. Records `question.withdraw`
/// (reason `explicit`).
pub async fn withdraw(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::QuestionsWithdraw>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    let (sid, qid) = path(p)?;
    let vf = VisFor::of(&a.caller, &s);
    session_close(s, vf, audit, sid, qid, Close::Withdraw).await
}

/// `POST /api/sessions/<sid>/questions/<qid>/release` → `{question}`: the
/// hook's timer moves a mirrored question to the terminal. 400
/// `not_mirrored` for an `ask` question; 409 `question_closed` when it is
/// not open. Records `question.release` (reason `timer`).
pub async fn release(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::QuestionsRelease>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    let (sid, qid) = path(p)?;
    let vf = VisFor::of(&a.caller, &s);
    session_close(s, vf, audit, sid, qid, Close::Expire).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalBody {
    tool_use_id: String,
    answers: Map<String, Value>,
}

/// `POST /api/sessions/<sid>/questions:terminal`: records the terminal
/// dialog's answers (AskUserQuestion's `answers`, by question text) on the
/// released question of `tool_use_id` → `{question}`; 204 when the session
/// has no released question for that call. Records `question.answer` as
/// the owner, who answered (`answered_via: terminal`).
pub async fn terminal(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::QuestionsTerminal>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<TerminalBody>, JsonRejection>,
) -> Result<Response, QuestionError> {
    let sid = path(p)?;
    let b = body(req)?;
    let st = s.clone();
    let vf = VisFor::of(&a.caller, &s);
    let closed = s
        .store_call(move |db| {
            live(db, &sid)?;
            let Some(q) = db.question_by_tool_use(&sid, &b.tool_use_id)? else {
                return Ok(None);
            };
            if q.status != Status::Released {
                return Ok(None);
            }
            let answers = from_claude_answers(&q.questions, &b.answers);
            let ctx = audit.answerer_in_terminal(db)?;
            let c = close(&st, db, &ctx, &q.id, Close::Terminal { answers })?;
            closed_for_asker(db, &vf, c).map(Some)
        })
        .await?;
    Ok(match closed {
        None => StatusCode::NO_CONTENT.into_response(),
        Some(c) => c.respond()?.into_response(),
    })
}

/// The query of the debug builds' waiter routes.
#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct UntilQuery {
    pub(crate) until: Option<usize>,
}

/// `POST /api/_test/questions/<qid>/expire` (debug builds): ends every
/// poll holding question `qid` now, as if its `wait` had run out →
/// `{expired}`, how many polls that was. Tests use it instead of waiting.
#[cfg(debug_assertions)]
pub async fn expire(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let qid = path(p)?;
    Ok(Json(json!({"expired": s.questions.expire(&qid)})))
}

/// `GET /api/_test/questions/<qid>/waiters?until=<n>` (debug builds):
/// `{count}`, how many polls hold question `qid`, answered once the count
/// is `until` (or after 5 s), at once without it.
#[cfg(debug_assertions)]
pub async fn waiters(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<UntilQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let qid = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let w = &s.questions;
    let n = crate::questions::count_until(w.changed(), || w.count(&qid), q.until).await;
    Ok(Json(json!({"count": n})))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

/// `GET /api/questions?status=open|closed|all&limit=<1..200>` (the owner:
/// the owner's agents' questions; a connected person: their agents') →
/// `{questions, open}`: open questions oldest first, closed ones most
/// recently closed first, and with `all` the open ones newest first before
/// the closed ones; `open` counts every open question of theirs. Default
/// `open`, 50; `limit` is clamped to 1..=200. 400 `invalid_query` for
/// another status.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let which = match q.status.as_deref() {
        None | Some("open") => ListStatus::Open,
        Some("closed") => ListStatus::Closed,
        Some("all") => ListStatus::All,
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_query",
                "status is open, closed or all",
            ));
        }
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let out = s
        .store_call(move |db| {
            let (rows, open) = db.list_questions(me.as_deref(), which, limit)?;
            let views = rows
                .iter()
                .map(|r| question_view(db, r))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({"questions": views, "open": open}))
        })
        .await?;
    Ok(Json(out))
}

/// `GET /api/questions/<qid>` (the owner, or a connected person) →
/// `{question}`; 404 unless one of the caller's agents asked it.
pub async fn get_one(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let qid = path(p)?;
    let v = s
        .store_call(move |db| {
            // Person-scoped (spec §18): another person's agent's question
            // is not the caller's to see.
            let q = db.question_of(&qid, me.as_deref())?;
            question_view(db, &q)
        })
        .await?;
    Ok(Json(json!({"question": v})))
}

/// Applies `c`, from the person `me` (`None`: the owner), to question
/// `qid`: 404 unless one of their agents asked it. The audit context is
/// resolved once the question is found, so a request for none makes no
/// row.
async fn owner_close(
    s: AppState,
    me: Option<String>,
    audit: DeferredAudit,
    qid: String,
    c: Close,
) -> Result<Json<Value>, QuestionError> {
    let st = s.clone();
    s.store_call(move |db| {
        db.question_of(&qid, me.as_deref())?;
        let ctx = audit.resolve(db)?;
        close(&st, db, &ctx, &qid, c)
    })
    .await?
    .respond()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    answers: Vec<Answer>,
}

/// `POST /api/questions/<qid>/answer` (the owner, or a connected person,
/// for their agents' questions), body `{answers}` (spec §5.3) →
/// `{question}`; 404 for another's. `answered_via` is `extension` through
/// the extension gateway, `cli` for the token from no browser of the
/// owner's, else `shell` (a person's browser too). 400 `invalid_answer` for answers that do not fit the questions;
/// 409 `question_closed`, with the question's view, when it is not open.
/// Records `question.answer`.
pub async fn answer(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Json<Value>, QuestionError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let qid = path(p)?;
    let b = body(req)?;
    let via = if who.extension {
        "extension"
    } else if who.token && !who.owner_browser() {
        "cli"
    } else {
        "shell"
    };
    owner_close(
        s,
        me,
        audit,
        qid,
        Close::Answer {
            answers: b.answers,
            via,
        },
    )
    .await
}

/// `POST /api/questions/<qid>/decline` (the owner, or a connected person,
/// for their agents' questions): the person skips the question →
/// `{question}`; 404 for another's; 409 `question_closed` when it is not
/// open. Records `question.decline`.
pub async fn decline(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    owner_close(s, me, audit, path(p)?, Close::Decline).await
}

/// `POST /api/questions/<qid>/release` (the owner, or a connected person,
/// for their agents' questions): "Answer in the terminal" for a mirrored
/// question → `{question}`; 404 for another's; 400 `not_mirrored` for an
/// `ask` question; 409 `question_closed` when it is not open. Records
/// `question.release` (reason `owner`).
pub async fn release_owner(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    owner_close(s, me, audit, path(p)?, Close::Release).await
}
