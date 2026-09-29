//! `GET /api/sessions/<sid>/feedback` (W): hands over feedback for a session by
//! tier, long-polling up to `wait` seconds; and `POST .../feedback/ack` (W).

use super::artifacts::{body, parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::feedback::apply;
use crate::state::AppState;
use artifax_core::feedback::render_items;
use artifax_core::{CoreError, Store, TakeFeedback, Tier};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
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
fn live_session(st: &Store, sid: &str) -> artifax_core::Result<()> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(x) if x.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "the session has ended; feedback is released to other sessions",
        )),
        Some(_) => Ok(()),
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

/// Returns `{feedback, text, waited_s}` as soon as rows exist for the session
/// and tier (default `wait`), or after `wait` seconds (capped at 600) with
/// none. A request dropped while waiting takes nothing (the handler future is
/// dropped with the connection). A drop that lands after the wake, while the
/// take is running on the blocking pool, still marks the rows handed over, and
/// in-band tiers acknowledge them; such rows are not resent. `resends=false` leaves out
/// resend-eligible rows. A daemon that begins shutting down answers empty at once.
/// An unknown session is 404; an ended one is 400 `unknown_session`, answered
/// before any wait. A session that ends during the wait answers empty at the
/// deadline.
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
        let ctx = s.feedback_ctx();
        let t = take.clone();
        let items = s
            .store_call(move |st| {
                let (items, touched) = st.take_feedback(&t, &ctx.browser_base)?;
                apply(&ctx, st, &touched);
                Ok(items)
            })
            .await?;
        if !items.is_empty() || Instant::now() >= deadline {
            let text = (!items.is_empty()).then(|| render_items(&items));
            return Ok(Json(
                json!({"feedback": items, "text": text, "waited_s": started.elapsed().as_secs()}),
            ));
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = &mut stopping => {
                return Ok(Json(json!({"feedback": [], "text": null, "waited_s": started.elapsed().as_secs()})));
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckBody {
    thread_ids: Vec<String>,
}

/// Acknowledges the session's rows on `thread_ids`; `{acknowledged: <threads>}`.
/// Unknown session: 404; ended session: 400 `unknown_session`.
pub async fn ack(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    req: Result<Json<AckBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let b = body(req)?;
    let ctx = s.feedback_ctx();
    let n = s
        .store_call(move |st| {
            live_session(st, &sid)?;
            let touched = st.acknowledge(&sid, &b.thread_ids)?;
            apply(&ctx, st, &touched);
            Ok(touched.threads.len())
        })
        .await?;
    Ok(Json(json!({"acknowledged": n})))
}
