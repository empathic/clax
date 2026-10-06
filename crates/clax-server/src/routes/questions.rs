//! Agent questions (spec 2026-10-06-agent-questions-and-inbox §6.1).
//! Session routes (token) let the asking session create, wait on, withdraw,
//! release and record the terminal answer of its own questions; another
//! session's question is 404.

use super::artifacts::{body, body_within, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::questions::{announce, arm_grace, start_grace, view};
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::questions::{Question, from_claude, from_claude_answers, validate_ask};
use clax_core::store::questions::{Close, NewQuestion, Source, Status};
use clax_core::{ArtifactId, CoreError, Store};
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

/// Applies `c` to question `qid` and announces it. Runs on a store thread.
pub(crate) fn close(s: &AppState, db: &Store, qid: &str, c: Close) -> clax_core::Result<Closed> {
    match db.close_question(qid, c) {
        Ok(q) => {
            let v = view(db, &q)?;
            announce(s, &q, v.clone());
            Ok(Closed::Done(v))
        }
        Err(CoreError::Invalid {
            code: "question_closed",
            message,
        }) => {
            let q = db.question(qid)?.ok_or(CoreError::NotFound)?;
            Ok(Closed::Already(message, view(db, &q)?))
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
/// question takes Claude Code's AskUserQuestion input as it is, and the
/// artifact of the session's newest working record when it names none; it
/// is created moved to the terminal (`mode: "terminal"`) when no owner
/// surface is open or `terminal_after_s` is 0, and otherwise is withdrawn
/// when no poll holds it for the grace.
pub async fn create(
    State(s): State<AppState>,
    _t: RequireToken,
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
    let surface_open = s.stream.holds_owner_topics();
    let after = s.terminal_after_s;
    let released = source == Source::Hook && (!surface_open || after == 0);
    let artifact_id = match (&b.artifact_id, source) {
        (Some(a), _) => Some(a.clone()),
        (None, Source::Hook) => s.working.newest_artifact_of(&sid),
        (None, Source::Ask) => None,
    };
    let named = b.artifact_id.is_some();
    let tool_use_id = b.tool_use_id;
    let st = s.clone();
    let (row, made, v) = s
        .store_call(move |db| {
            live(db, &sid)?;
            // A named artifact must be live; a working record's may have
            // gone since, and then the question is about none.
            let live = match artifact_id.as_deref().map(ArtifactId::parse) {
                Some(Ok(id)) => db.get_artifact(&id)?.map(|a| a.id),
                _ => None,
            };
            if named && live.is_none() {
                return Err(CoreError::NotFound);
            }
            let (row, made) = db.create_question(NewQuestion {
                session_id: sid,
                artifact_id: live,
                source,
                tool_use_id,
                questions,
                released,
            })?;
            let v = view(db, &row)?;
            if made {
                announce(&st, &row, v.clone());
            }
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
/// (at most [`MAX_WAIT_SECS`]), or when the daemon shuts down. An answered
/// or declined result marks it taken. While the poll runs it holds the
/// question; when the last poll of an open hook question lets go, its
/// grace starts. 429 `limit_reached` when
/// [`MAX_POLLS`](crate::questions::MAX_POLLS) polls hold it already (a
/// new poll is refused rather than an older one ended).
pub async fn poll(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, qid) = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let (sid1, qid1) = (sid.clone(), qid.clone());
    let first = s
        .store_call(move |db| {
            live(db, &sid1)?;
            db.session_question(&sid1, &qid1)
        })
        .await?;
    // Only an open mirrored question can be withdrawn for want of a poll.
    let grace = (first.source == Source::Hook && first.status == Status::Open)
        .then(|| arm_grace(s.clone(), qid.clone()));
    let Some((notify, _hold)) = s.questions.hold(&qid, grace) else {
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
    let mut stop = false;
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (sid1, qid1) = (sid.clone(), qid.clone());
        let done = stop || Instant::now() >= deadline;
        let out = s
            .store_call(move |db| {
                let row = db.session_question(&sid1, &qid1)?;
                if row.status == Status::Open && !done {
                    return Ok(None);
                }
                if matches!(row.status, Status::Answered | Status::Declined) {
                    db.take_question(&row.id)?;
                }
                view(db, &row).map(Some)
            })
            .await?;
        if let Some(v) = out {
            return Ok(Json(
                json!({"question": v, "waited_s": started.elapsed().as_secs()}),
            ));
        }
        tokio::select! {
            () = &mut notified => {}
            () = tokio::time::sleep_until(deadline) => {}
            () = &mut stopping => stop = true,
        }
    }
}

/// Applies `c` to session `sid`'s question `qid`.
async fn session_close(
    s: AppState,
    sid: String,
    qid: String,
    c: Close,
) -> Result<Json<Value>, QuestionError> {
    let st = s.clone();
    let closed = s
        .store_call(move |db| {
            live(db, &sid)?;
            db.session_question(&sid, &qid)?;
            close(&st, db, &qid, c)
        })
        .await?;
    closed.respond()
}

/// `POST /api/sessions/<sid>/questions/<qid>/withdraw` → `{question}`; 409
/// `question_closed` when it is not open.
pub async fn withdraw(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    let (sid, qid) = path(p)?;
    session_close(s, sid, qid, Close::Withdraw).await
}

/// `POST /api/sessions/<sid>/questions/<qid>/release` → `{question}`: the
/// hook's timer moves a mirrored question to the terminal. 400
/// `not_mirrored` for an `ask` question; 409 `question_closed` when it is
/// not open.
pub async fn release(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    let (sid, qid) = path(p)?;
    session_close(s, sid, qid, Close::Release).await
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
/// has no released question for that call.
pub async fn terminal(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<TerminalBody>, JsonRejection>,
) -> Result<Response, QuestionError> {
    let sid = path(p)?;
    let b = body(req)?;
    let st = s.clone();
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
            close(&st, db, &q.id, Close::Terminal { answers }).map(Some)
        })
        .await?;
    Ok(match closed {
        None => StatusCode::NO_CONTENT.into_response(),
        Some(c) => c.respond()?.into_response(),
    })
}

/// `GET /api/_test/questions/<qid>/waiters` (debug builds): `{count}`, how
/// many polls hold question `qid`.
#[cfg(debug_assertions)]
pub async fn waiters(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let qid = path(p)?;
    Ok(Json(json!({"count": s.questions.count(&qid)})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    answers: Vec<clax_core::questions::Answer>,
}

/// `POST /api/_test/questions/<qid>/answer` (debug builds): answers
/// question `qid` through the shell, as the owner's answer route does.
#[cfg(debug_assertions)]
pub async fn test_answer(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Json<Value>, QuestionError> {
    let qid = path(p)?;
    let b = body(req)?;
    let st = s.clone();
    s.store_call(move |db| {
        close(
            &st,
            db,
            &qid,
            Close::Answer {
                answers: b.answers,
                via: "shell",
            },
        )
    })
    .await?
    .respond()
}
