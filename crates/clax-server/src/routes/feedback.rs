//! `GET /api/sessions/<sid>/feedback` (W): hands over feedback for a session by
//! tier, long-polling up to `wait` seconds; `POST .../feedback/ack` (W); and
//! `GET /api/sessions/<sid>/notices` (W), the notices `clax feedback follow`
//! prints.

use super::artifacts::{body, parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::feedback::apply;
use crate::questions::QuestionWaiters;
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::feedback::{ago, quoted, render_items, render_notice};
use clax_core::questions::render_late;
use clax_core::store::questions::Status;
use clax_core::working::Clock;
use clax_core::{CoreError, Store, TakeFeedback, Tier};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::Instant;

/// Longest accepted `wait`, in seconds.
pub const MAX_WAIT_SECS: u64 = 600;

/// Checks that session `sid` exists and has not ended.
///
/// # Errors
/// `NotFound` for an unknown session; `unknown_session` for an ended one.
fn live_session(st: &Store, sid: &str) -> clax_core::Result<()> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(x) if x.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "the session has ended; feedback is released to other sessions",
        )),
        Some(_) => Ok(()),
    }
}

/// Takes session `sid`'s answered and declined `ask` questions it has not
/// received (marking them received), leaving those a question poll holds
/// to that poll: their views, and one late-answer block per question, its
/// age read from `clock` (spec 2026-10-06-agent-questions-and-inbox §6.4).
fn late_answers(
    st: &Store,
    sid: &str,
    held: &QuestionWaiters,
    clock: &dyn Clock,
) -> clax_core::Result<(Vec<Value>, Vec<String>)> {
    let rows = st.take_late_answers(sid, |qid| held.count(qid) > 0)?;
    let now = clock.now();
    let mut views = Vec::with_capacity(rows.len());
    let mut blocks = Vec::with_capacity(rows.len());
    for q in &rows {
        views.push(crate::questions::view(st, q)?);
        let header = q.questions.first().map_or("", |x| x.header.as_str());
        let head = match q.status {
            Status::Declined => format!(
                "[clax] The person skipped your question {} ({}):",
                quoted(header),
                q.id
            ),
            _ => format!(
                "[clax] The person answered your question {} ({}, asked {}):",
                quoted(header),
                q.id,
                ago(&q.created_at, now)
            ),
        };
        let answers = (q.status == Status::Answered)
            .then_some(q.answers.as_deref())
            .flatten();
        blocks.push(render_late(&head, &q.questions, answers));
    }
    Ok((views, blocks))
}

/// The response text: the feedback items' text, then each late-answer
/// block, separated by blank lines; `None` when there is neither.
fn poll_text(items: &[clax_core::FeedbackItem], late: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(1 + late.len());
    if !items.is_empty() {
        parts.push(render_items(items).trim_end().to_string());
    }
    parts.extend(late.iter().map(|b| b.trim_end().to_string()));
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[derive(Deserialize)]
pub struct FeedbackQuery {
    #[serde(default)]
    wait: u64,
    tier: Option<String>,
    artifact: Option<String>,
    resends: Option<bool>,
}

/// Returns `{feedback, answers, text, waited_s}` as soon as rows exist for
/// the session and tier (default `wait`) or it has late answers, or after
/// `wait` seconds (capped at 600) with neither. `answers` holds the views of
/// the session's answered and declined `ask` questions not yet received,
/// which the poll marks received, and `text` ends with a block for each
/// (spec 2026-10-06-agent-questions-and-inbox §6.4). A question that a
/// question poll holds is left to that poll, so each answer is handed over
/// once. `tier=queue` (the Codex queue) takes no answers and its `answers`
/// is always empty.
///
/// A request dropped while waiting takes nothing (the handler future is
/// dropped with the connection). A drop that lands after the wake, while
/// the take is running on the blocking pool, still marks the rows handed
/// over, and in-band tiers acknowledge them; such rows are not resent.
/// `resends=false` leaves out resend-eligible rows. A daemon that begins
/// shutting down answers empty at once.
/// An unknown session is 404; an ended one is 400 `unknown_session`, answered
/// before any wait. A session that ends during the wait answers empty at the
/// deadline.
///
/// While a `tier=wait` poll with `wait > 0` (`wait_for_feedback`) is in
/// progress, tier 5 is skipped for the session: `codex queue` does not push
/// to it ([`crate::push::dispatch`]), and a `tier=inject` poll (the Pi
/// injection loop) takes nothing and answers empty at once, `{feedback: [],
/// answers: [], text: null, waited_s: 0}` (also when it was already waiting
/// and is woken), so the rows and answers go to the wait poll. The inject
/// poll checks before each take; a wait poll that starts between that check
/// and the take can lose one hand over to it.
///
/// Any holder of the token may read or acknowledge any session's feedback: the
/// token is the local trust boundary, and sessions are not authenticated
/// separately.
pub async fn poll(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<FeedbackQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let tier = match q.tier.as_deref() {
        None => Tier::Wait,
        Some(t) => Tier::parse(t)
            .ok_or_else(|| ApiError::bad_request("invalid_tier", format!("unknown tier '{t}'")))?,
    };
    let artifact = q
        .artifact
        .as_deref()
        .map(parse_id)
        .transpose()?
        .map(|a| a.as_str().to_string());
    let check = sid.clone();
    s.store_call(move |st| live_session(st, &check)).await?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let notify = s.feedback_waiters.get(&sid);
    // While a wait-tier poll waits, tier 5 leaves the session's rows to it.
    let _waiting = (q.wait > 0 && tier == Tier::Wait).then(|| s.feedback_waiters.enter(&sid));
    let mut shutdown = s.shutdown.clone();
    // A dropped sender means the state has no shutdown source; never end early then.
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let take = TakeFeedback {
        session_id: sid,
        tier,
        artifact_id: artifact,
        include_resends: q.resends.unwrap_or(true),
    };
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if take.tier == Tier::Inject && s.feedback_waiters.is_waiting(&take.session_id) {
            return Ok(Json(
                json!({"feedback": [], "answers": [], "text": null, "waited_s": 0}),
            ));
        }
        let ctx = s.feedback_ctx();
        let t = take.clone();
        let (held, clock) = (s.questions.clone(), s.question_clock.clone());
        let (items, (answers, late)) = s
            .store_call(move |st| {
                let (items, touched) = st.take_feedback(&t, &ctx.browser_base)?;
                apply(&ctx, st, &touched);
                crate::working::renew_for_tier(&ctx, &t.session_id, t.tier);
                if t.tier == Tier::Queue {
                    return Ok((items, Default::default()));
                }
                crate::working::mark_items(&ctx, st, &t.session_id, &items)?;
                Ok((items, late_answers(st, &t.session_id, &held, &*clock)?))
            })
            .await?;
        #[cfg(debug_assertions)]
        let expired = _waiting.is_some() && s.feedback_waiters.take_expired(&take.session_id);
        #[cfg(not(debug_assertions))]
        let expired = false;
        if !items.is_empty() || !answers.is_empty() || expired || Instant::now() >= deadline {
            let text = poll_text(&items, &late);
            return Ok(Json(json!({
                "feedback": items,
                "answers": answers,
                "text": text,
                "waited_s": started.elapsed().as_secs(),
            })));
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = &mut stopping => {
                return Ok(Json(json!({"feedback": [], "answers": [], "text": null, "waited_s": started.elapsed().as_secs()})));
            }
        }
    }
}

#[derive(Deserialize)]
pub struct NoticesQuery {
    #[serde(default)]
    wait: u64,
}

/// `GET /api/sessions/<sid>/notices?wait=<s>`: announces the session's
/// armed, undelivered comments that no follower has announced
/// (`Store::take_notices`), as soon as there are any or after `wait`
/// seconds (capped at 600): `{notices, lines, waited_s}`, one line per
/// notice. Announcing delivers nothing. While the session is inside
/// `wait_for_feedback`, answers empty at once and announces nothing. The
/// poll counts as a connected follower for `status`'s `push`. Unknown
/// session: 404; ended: 400 `unknown_session`, before any wait, and also
/// at once when the session ends during the wait, so a follower stops
/// without waiting out its poll. A daemon that begins shutting down
/// answers empty at once.
pub async fn notices(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<NoticesQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let check = sid.clone();
    s.store_call(move |st| live_session(st, &check)).await?;
    let _following = s.followers.enter(&sid);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let notify = s.feedback_waiters.get(&sid);
    let mut shutdown = s.shutdown.clone();
    // A dropped sender means the state has no shutdown source; never end early then.
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let empty = |waited: u64| Json(json!({"notices": [], "lines": [], "waited_s": waited}));
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if s.feedback_waiters.is_waiting(&sid) {
            return Ok(empty(0));
        }
        let base = s.browser_base.clone();
        let who = sid.clone();
        let notices = s
            .store_call(move |st| {
                live_session(st, &who)?;
                st.take_notices(&who, &base)
            })
            .await?;
        if !notices.is_empty() || Instant::now() >= deadline {
            let lines: Vec<String> = notices.iter().map(render_notice).collect();
            return Ok(Json(
                json!({"notices": notices, "lines": lines, "waited_s": started.elapsed().as_secs()}),
            ));
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = &mut stopping => return Ok(empty(started.elapsed().as_secs())),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckBody {
    #[serde(default)]
    thread_ids: Option<Vec<String>>,
    #[serde(default)]
    comment_ids: Option<Vec<String>>,
}

/// Acknowledges the session's rows on `thread_ids` (every row on those
/// threads) and on `comment_ids` (only rows for those comments, so a comment
/// the caller has not seen stays pending); either or both, and a body with
/// neither is 400 `invalid_json`. `{acknowledged: <threads touched>}`.
/// Unknown session: 404; ended session: 400 `unknown_session`.
pub async fn ack(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    req: Result<Json<AckBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let b = body(req)?;
    if b.thread_ids.is_none() && b.comment_ids.is_none() {
        return Err(ApiError::bad_request(
            "invalid_json",
            "pass thread_ids, comment_ids, or both",
        ));
    }
    let ctx = s.feedback_ctx();
    let n = s
        .store_call(move |st| {
            live_session(st, &sid)?;
            let mut touched = st.acknowledge(&sid, b.thread_ids.as_deref().unwrap_or_default())?;
            touched.merge(
                st.acknowledge_comments(&sid, b.comment_ids.as_deref().unwrap_or_default())?,
            );
            apply(&ctx, st, &touched);
            Ok(touched.threads.len())
        })
        .await?;
    Ok(Json(json!({"acknowledged": n})))
}

/// `POST /api/_test/sessions/<sid>/feedback/expire` (debug builds): ends
/// every `wait_for_feedback` poll of session `sid` now, as if its `wait`
/// had run out → `{expired}`, how many polls that was. Tests use it
/// instead of waiting.
#[cfg(debug_assertions)]
pub async fn expire(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    Ok(Json(json!({"expired": s.feedback_waiters.expire(&sid)})))
}

/// `GET /api/_test/sessions/<sid>/feedback/waiters?until=<n>` (debug
/// builds): `{count}`, how many `wait_for_feedback` polls of session `sid`
/// are in progress, answered once the count is `until` (or after 5 s), at
/// once without it.
#[cfg(debug_assertions)]
pub async fn waiters(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<super::questions::UntilQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let w = &s.feedback_waiters;
    let n = crate::questions::count_until(w.changed(), || w.count(&sid), q.until).await;
    Ok(Json(json!({"count": n})))
}
