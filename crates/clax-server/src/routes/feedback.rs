//! `GET /api/sessions/<sid>/feedback` (W): hands over feedback for a session by
//! tier, long-polling up to `wait` seconds; `POST .../feedback/ack` (W); and
//! `GET /api/sessions/<sid>/notices` (W), the notices `clax feedback follow`
//! prints.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use clax_core::audit::AuditCtx;
use clax_core::feedback::{Placeholder, ago, quoted, render_delivery, render_notice};
use clax_core::model::Artifact;
use clax_core::questions::render_late;
use clax_core::store::questions::Status;
use clax_core::store::visibility::Visibility;
use clax_core::working::Clock;
use clax_core::{CoreError, Store, TakeFeedback, Tier};
use clax_http::agent::{FeedbackAck, FeedbackNotices, FeedbackPoll, TokenOrAgent};
#[cfg(debug_assertions)]
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_realtime::feedback::apply;
use clax_realtime::questions::QuestionWaiters;
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
        views.push(clax_http::views::question::view(st, q)?);
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

/// The response text: the feedback items' text and each placeholder's
/// line ([`render_delivery`]), then each late-answer block, separated by
/// blank lines; `None` when there is none of them.
fn poll_text(
    items: &[clax_core::FeedbackItem],
    placeholders: &[Placeholder],
    late: &[String],
) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(1 + late.len());
    if let Some(t) = render_delivery(items, placeholders) {
        parts.push(t.trim_end().to_string());
    }
    parts.extend(late.iter().map(|b| b.trim_end().to_string()));
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// What ended a long-poll's wait ([`wait_for_change`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Woke {
    /// The session's waiters were notified: a row, or its session's end.
    Row,
    /// The visibility generation moved past the one the take read.
    Visibility,
    Deadline,
    Stopping,
}

/// Waits for whichever comes first after a take: `notified` (registered
/// before the take), the visibility generation moving past `seen` (read
/// before the take), the deadline, or `stopping`. A row or a bump that
/// landed between the take and this call ends it at once (spec 2026-10-08
/// §8.4, M17).
pub(crate) async fn wait_for_change(
    notified: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
    vis_gen: &clax_http::state::VisGen,
    seen: u64,
    deadline: Instant,
    stopping: std::pin::Pin<&mut (impl std::future::Future<Output = ()> + ?Sized)>,
) -> Woke {
    tokio::select! {
        _ = notified => Woke::Row,
        _ = vis_gen.wait_after(seen) => Woke::Visibility,
        _ = tokio::time::sleep_until(deadline) => Woke::Deadline,
        _ = stopping => Woke::Stopping,
    }
}

/// The view session `sid`'s agent reads with (spec 2026-10-08 §8.4, M17):
/// `vis` refreshed when the visibility generation moved past `seen`, its
/// reader's, built when there is none yet.
///
/// # Errors
/// The store's.
fn view_at(
    st: &Store,
    sid: &str,
    vis: Option<Visibility>,
    seen: u64,
) -> clax_core::Result<Visibility> {
    match vis {
        Some(mut v) => {
            v.refresh_if_stale(st, seen)?;
            Ok(v)
        }
        None => {
            let reader = Visibility::reader_of_session(st, sid)?;
            Visibility::for_reader(st, &reader, seen)
        }
    }
}

/// The base URL the links handed to an agent start with: for an agent of
/// another machine (`grant`), `http://` and the `Host` its signed request
/// named, the address that machine reaches the daemon at (the API answers
/// only to a local name or the address the connection arrived on); else
/// the daemon's own browser base, which is this machine's.
fn agent_base(s: &AppState, grant: bool, headers: &HeaderMap) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty());
    match host {
        Some(h) if grant => format!("http://{h}"),
        _ => s.browser_base.clone(),
    }
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
/// What is handed over is filtered for the session's person (spec
/// 2026-10-08 §8.4, M16): a comment whose body the person's agents may not
/// see is held, and the first take that holds it tells the session once,
/// in `withheld` (one placeholder per authors and thread, present only
/// when there are any) and in `text`; that answers the poll as a delivery
/// does. Rows on an artifact a person's agent may not read are held
/// untold. The view, and a person's agent's rights, are read again
/// whenever the visibility generation moves (M17), and a waiting poll
/// wakes on it: an approval or revocation that
/// lands during the wait applies to its next take. The poll reads the
/// generation, takes, and only then waits for a row, a bump or the
/// deadline, so neither a row nor a bump after its take is missed.
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
/// to it ([`clax_realtime::push::dispatch`]), and a `tier=inject` poll (the Pi
/// injection loop) takes nothing and answers empty at once, `{feedback: [],
/// answers: [], text: null, waited_s: 0}`, so the rows and answers go to the
/// wait poll. An inject poll already waiting when the wait poll starts is
/// woken by that start and answers the same way. The inject
/// poll checks before each take; a wait poll that starts between that check
/// and the take can lose one hand over to it.
pub async fn poll(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<FeedbackPoll>,
    headers: HeaderMap,
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
    let artifact = match q.artifact.as_deref() {
        Some(raw) => Some(a.named_artifact(&s, raw).await?.as_str().to_string()),
        None => None,
    };
    let mut rights = a.read_rights();
    let grant = a.grant().map(str::to_string);
    let base = agent_base(&s, grant.is_some(), &headers);
    // Built with the first view: the poll's deliveries are recorded as
    // this session's agent.
    let mut audit: Option<std::sync::Arc<clax_core::audit::AuditCtx>> = None;
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
    let mut vis: Option<Visibility> = None;
    loop {
        #[cfg(debug_assertions)]
        if vis.is_none() {
            s.feedback_waiters
                .stop_at(&take.session_id, clax_realtime::feedback::Stop::FirstTake)
                .await;
        }
        // Registered before the take, and the generation read before it:
        // a row or a bump after the take wakes the wait below.
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = s.vis_gen.current();
        if take.tier == Tier::Inject && s.feedback_waiters.is_waiting(&take.session_id) {
            return Ok(Json(
                json!({"feedback": [], "answers": [], "text": null, "waited_s": 0}),
            ));
        }
        // The view and a person's agent's rights as of `seen`: read again
        // for the first take and whenever the generation moved (M17).
        let (prior_rights, grant) = (rights.clone(), grant.clone());
        let prior = vis.take();
        let sid = take.session_id.clone();
        let (prior_audit, headers) = (audit.take(), headers.clone());
        let (view, rights_now, audit_now) = s
            .store_call(move |st| {
                let audit = match prior_audit {
                    Some(a) => a,
                    None => {
                        live_session(st, &sid)?;
                        std::sync::Arc::new(clax_http::audit::session_ctx(st, &headers, &sid)?)
                    }
                };
                let view = view_at(st, &sid, prior, seen)?;
                let rights = prior_rights.current(st, grant.as_deref(), &sid, seen)?;
                Ok((view, rights, audit))
            })
            .await?;
        audit = Some(audit_now.clone());
        #[cfg(debug_assertions)]
        s.feedback_waiters
            .stop_at(&take.session_id, clax_realtime::feedback::Stop::BeforeTake)
            .await;
        let ctx = s.feedback_ctx();
        let ctx_after = ctx.clone();
        let t = take.clone();
        let (held, clock) = (s.questions.clone(), s.question_clock.clone());
        let audit = audit_now;
        let (view_now, readable_rights) = (view.clone(), rights_now.clone());
        let base = base.clone();
        let (taken, (answers, late)) = s
            .store_call(move |st| {
                let rights = readable_rights;
                let readable = |a: &Artifact| rights.allows(a);
                let readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)> =
                    rights.is_person_agent().then_some(&readable);
                let taken = st.take_feedback_readable(&audit, &t, &base, &view_now, readable)?;
                if taken.stale {
                    return Ok((taken, Default::default()));
                }
                // Its states go out once the poll has answered (below).
                clax_realtime::feedback::wake(&ctx, st, &taken.touched);
                clax_realtime::working::renew_for_tier(&ctx, &t.session_id, t.tier);
                if t.tier == Tier::Queue {
                    return Ok((taken, Default::default()));
                }
                clax_realtime::working::mark_items(&ctx, &audit, st, &t.session_id, &taken.items)?;
                let late = late_answers(st, &t.session_id, &held, &*clock)?;
                Ok((taken, late))
            })
            .await?;
        vis = Some(view);
        rights = rights_now;
        clax_realtime::feedback::publish_states_after(&ctx_after, taken.touched.clone());
        if taken.stale {
            // An approval or revocation committed after the view was
            // built: refresh it and take again.
            continue;
        }
        #[cfg(debug_assertions)]
        s.feedback_waiters
            .stop_at(&take.session_id, clax_realtime::feedback::Stop::AfterTake)
            .await;
        #[cfg(debug_assertions)]
        let expired = _waiting.is_some() && s.feedback_waiters.take_expired(&take.session_id);
        #[cfg(not(debug_assertions))]
        let expired = false;
        let (items, placeholders) = (taken.items, taken.placeholders);
        // A restored session's note (spec 2026-10-08 §6.7) goes with its
        // first poll that a harness reads.
        let note = (take.tier != Tier::Queue)
            .then(|| s.feedback_waiters.take_note(&take.session_id))
            .flatten();
        if !items.is_empty()
            || !placeholders.is_empty()
            || !answers.is_empty()
            || note.is_some()
            || expired
            || Instant::now() >= deadline
        {
            let text = poll_text(&items, &placeholders, &late);
            let text = match (&note, text) {
                (Some(n), Some(t)) => Some(format!("{n}\n\n{t}")),
                (Some(n), None) => Some(n.clone()),
                (None, t) => t,
            };
            let mut out = json!({
                "feedback": items,
                "answers": answers,
                "text": text,
                "waited_s": started.elapsed().as_secs(),
            });
            if !placeholders.is_empty() {
                out["withheld"] = json!(placeholders);
            }
            if let Some(n) = note {
                out["notes"] = json!([n]);
            }
            return Ok(Json(out));
        }
        #[cfg(debug_assertions)]
        let _parked = s.feedback_waiters.park(&take.session_id, take.tier);
        let woke = wait_for_change(
            notified.as_mut(),
            &s.vis_gen,
            seen,
            deadline,
            stopping.as_mut(),
        )
        .await;
        if woke == Woke::Stopping {
            return Ok(Json(
                json!({"feedback": [], "answers": [], "text": null, "waited_s": started.elapsed().as_secs()}),
            ));
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
/// notice. Announcing delivers nothing. Only comments whose bodies the
/// session's person's agents may see are announced, with the title
/// filtered as the feedback text's is (spec 2026-10-08 §8.4), on artifacts
/// a person's agent may read; the view follows the visibility generation
/// as the feedback poll's does, so an approval during the wait announces
/// what it lets through. While the session is inside
/// `wait_for_feedback`, answers empty at once and announces nothing. The
/// poll counts as a connected follower for `status`'s `push`. Unknown
/// session: 404; ended: 400 `unknown_session`, before any wait, and also
/// at once when the session ends during the wait, so a follower stops
/// without waiting out its poll. A daemon that begins shutting down
/// answers empty at once.
pub async fn notices(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<FeedbackNotices>,
    headers: HeaderMap,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<NoticesQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let check = sid.clone();
    s.store_call(move |st| live_session(st, &check)).await?;
    let mut rights = a.read_rights();
    let grant = a.grant().map(str::to_string);
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
    let mut vis: Option<Visibility> = None;
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = s.vis_gen.current();
        if s.feedback_waiters.is_waiting(&sid) {
            return Ok(empty(0));
        }
        let base = agent_base(&s, grant.is_some(), &headers);
        let who = sid.clone();
        let prior = vis.take();
        let (prior_rights, grant) = (rights.clone(), grant.clone());
        let (view, rights_now, announced) = s
            .store_call(move |st| {
                live_session(st, &who)?;
                let view = view_at(st, &who, prior, seen)?;
                let rights = prior_rights.current(st, grant.as_deref(), &who, seen)?;
                let readable = |a: &Artifact| rights.allows(a);
                let readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)> =
                    rights.is_person_agent().then_some(&readable);
                let announced = st.take_notices(&who, &base, &view, readable)?;
                Ok((view, rights, announced))
            })
            .await?;
        vis = Some(view);
        rights = rights_now;
        if announced.stale {
            continue;
        }
        let notices = announced.notices;
        if !notices.is_empty() || Instant::now() >= deadline {
            let lines: Vec<String> = notices.iter().map(render_notice).collect();
            return Ok(Json(
                json!({"notices": notices, "lines": lines, "waited_s": started.elapsed().as_secs()}),
            ));
        }
        let woke = wait_for_change(
            notified.as_mut(),
            &s.vis_gen,
            seen,
            deadline,
            stopping.as_mut(),
        )
        .await;
        if woke == Woke::Stopping {
            return Ok(empty(started.elapsed().as_secs()));
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
/// the caller has not seen stays pending), leaving held every undelivered
/// row whose body the session's person's agents may not see, and every
/// row on an artifact a person's agent may not read; either or both, and a body with
/// neither is 400 `invalid_json`. `{acknowledged: <threads touched>}`.
/// Unknown session: 404; ended session: 400 `unknown_session`.
pub async fn ack(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<FeedbackAck>,
    audit: AuditCtx,
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
    let reads = a.read_rights();
    let n = s
        .store_call(move |st| {
            live_session(st, &sid)?;
            let vis = Visibility::for_session(st, &sid)?;
            let may = |x: &Artifact| reads.allows(x);
            let readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)> =
                reads.is_person_agent().then_some(&may);
            let mut touched = st.acknowledge(
                &audit,
                &sid,
                b.thread_ids.as_deref().unwrap_or_default(),
                &vis,
                readable,
            )?;
            touched.merge(st.acknowledge_comments(
                &audit,
                &sid,
                b.comment_ids.as_deref().unwrap_or_default(),
                &vis,
                readable,
            )?);
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
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    Ok(Json(json!({"expired": s.feedback_waiters.expire(&sid)})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct HoldBody {
    hold: bool,
    at: Option<String>,
}

/// `POST /api/_test/sessions/<sid>/feedback/hold {hold, at}` (debug
/// builds): with `hold: true`, each feedback poll of session `sid` stops at
/// `at` (`after_take`, the default: after a take, before it answers or
/// waits; `before_take`: after it built its view, before the take;
/// `first_take`: before its first take reads the visibility generation),
/// until a `hold: false` lets them all go on (`FeedbackWaiters::hold`) →
/// `{}`; 400 `invalid_json` for another `at`. Tests change what the
/// session may see in that gap.
#[cfg(debug_assertions)]
pub async fn hold(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    b: Result<Json<HoldBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let b = body(b)?;
    if b.hold {
        let at = clax_realtime::feedback::Stop::parse(b.at.as_deref().unwrap_or("after_take"))
            .ok_or_else(|| ApiError::bad_request("invalid_json", "at names no stop"))?;
        s.feedback_waiters.hold(&sid, at);
    } else {
        s.feedback_waiters.release(&sid);
    }
    Ok(Json(json!({})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct WaitersQuery {
    until: Option<usize>,
    parked: Option<String>,
    held: Option<bool>,
}

/// `GET /api/_test/sessions/<sid>/feedback/waiters?until=<n>` (debug
/// builds): `{count}`, how many `wait_for_feedback` polls of session `sid`
/// are in progress, answered once the count is `until` (or after 5 s), at
/// once without it. With `parked=<tier>`, counts instead the session's
/// feedback polls of that tier that are parked waiting to be woken; with
/// `held=true`, its polls stopped after their take (`hold`).
#[cfg(debug_assertions)]
pub async fn waiters(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<WaitersQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let parked =
        match q.parked.as_deref() {
            None => None,
            Some(t) => Some(Tier::parse(t).ok_or_else(|| {
                ApiError::bad_request("invalid_tier", format!("unknown tier '{t}'"))
            })?),
        };
    let w = &s.feedback_waiters;
    let held = q.held.unwrap_or(false);
    let count = || match parked {
        _ if held => w.held(&sid),
        Some(tier) => w.parked(&sid, tier),
        None => w.count(&sid),
    };
    let n = clax_realtime::questions::count_until(w.changed(), count, q.until).await;
    Ok(Json(json!({"count": n})))
}

#[cfg(test)]
mod tests {
    use super::{Woke, wait_for_change};
    use clax_core::audit::AuditCtx;
    use clax_core::{Home, Store};
    use clax_http::state::VisGen;
    use std::time::Duration;
    use tokio::sync::Notify;
    use tokio::time::Instant;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    /// The approval of `*unconnected` the store opens with, revoked: a
    /// visibility change.
    fn bump(st: &Store) {
        let id = st.live_approvals(None).unwrap()[0].id.clone();
        st.revoke_approval(&AuditCtx::DAEMON, &id, None).unwrap();
    }

    /// Paused time: the deadline passes only when nothing else ends the
    /// wait, so which of them ended it is exact.
    #[tokio::test(start_paused = true)]
    async fn a_bump_or_row_between_take_and_wait_ends_the_wait_at_once() {
        let (_d, st) = store();
        let g = VisGen::install(&st);
        let rows = Notify::new();
        let stopping = std::future::pending::<()>();
        tokio::pin!(stopping);
        let deadline = Instant::now() + Duration::from_secs(600);
        // A bump after the generation was read, before the wait.
        let notified = rows.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = g.current();
        bump(&st);
        let start = Instant::now();
        assert_eq!(
            wait_for_change(notified.as_mut(), &g, seen, deadline, stopping.as_mut()).await,
            Woke::Visibility
        );
        assert_eq!(Instant::now(), start, "no time passed");
        // A row after the waiter registered, before the wait.
        let notified = rows.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = g.current();
        rows.notify_waiters();
        assert_eq!(
            wait_for_change(notified.as_mut(), &g, seen, deadline, stopping.as_mut()).await,
            Woke::Row
        );
        assert_eq!(Instant::now(), start);
        // Neither: the deadline, exactly.
        let notified = rows.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = g.current();
        assert_eq!(
            wait_for_change(notified.as_mut(), &g, seen, deadline, stopping.as_mut()).await,
            Woke::Deadline
        );
        assert_eq!(Instant::now(), deadline);
    }

    /// Paused time: a bump during the wait ends it when it lands, not at
    /// the deadline.
    #[tokio::test(start_paused = true)]
    async fn a_bump_during_the_wait_ends_it_when_it_lands() {
        let (_d, st) = store();
        let st = std::sync::Arc::new(st);
        let g = VisGen::install(&st);
        let rows = Notify::new();
        let stopping = std::future::pending::<()>();
        tokio::pin!(stopping);
        let start = Instant::now();
        let deadline = start + Duration::from_secs(600);
        let notified = rows.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let seen = g.current();
        let st2 = st.clone();
        let later = async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            tokio::task::spawn_blocking(move || bump(&st2))
                .await
                .unwrap();
            std::future::pending::<()>().await;
        };
        let woke = tokio::select! {
            w = wait_for_change(notified.as_mut(), &g, seen, deadline, stopping.as_mut()) => w,
            () = later => unreachable!(),
        };
        assert_eq!(woke, Woke::Visibility);
        assert!(Instant::now() < deadline, "{:?}", Instant::now() - start);
    }
}
