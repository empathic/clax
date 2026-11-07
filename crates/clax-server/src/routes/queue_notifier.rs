//! A queue's notifiers ([`clax_store::queue_notifiers`]), through a public
//! route any product can use.
//!
//! - `GET /api/queues/<qid>/notifiers/zulip` → `{notifier}`: the notifier or
//!   `null`.
//! - `PUT /api/queues/<qid>/notifiers/zulip {site, bot_email, api_key?,
//!   stream, people?, if_rev?}` → the same.
//! - `DELETE /api/queues/<qid>/notifiers/zulip {if_rev}` → 204.
//! - `POST /api/queues/<qid>/notifiers/zulip/test {if_rev}` → the same as
//!   `GET`: a test message has gone to the stream, by the saved settings. No
//!   direct message is tried. 429 `test_too_soon` within
//!   [`clax_store::queue_notifiers::TEST_EVERY_SECS`] of the last; 502
//!   `notifier_failed` with the panel's fixed words when it did not go.
//!
//! These are a queue admin's, as `queue_repo.rs`'s are: from the daemon's own
//! origin, in a browser or by the owner by hand, agents refused 403
//! `not_for_agents`, a non-member and an unknown queue 404 `not_a_member`, a
//! triage member 403 `not_queue_admin`. A write names the `rev` it was made
//! from, and a notifier that moved on answers 400 `notifier_changed`. The key
//! is write-only: no answer, audit record or log carries one, only its last
//! four characters ([`clax_http::bot::key_info`]).

use super::artifacts::{body, path};
use super::queue_repo::{admin_of, queue_caller};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use clax_core::CoreError;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_http::agent::DeniedToGrants;
use clax_http::bot::{Names, key_info, record_view, role_of, view};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::queue_notifiers::{ZulipFields, ZulipNotifier};
use clax_store::roles::check_may_grant_admin;
use serde::Deserialize;
use serde_json::{Value, json};

fn notifier_view(st: &Store, n: &ZulipNotifier) -> clax_core::Result<Value> {
    let mut names = Names::new(st);
    Ok(json!({
        "kind": "zulip",
        "rev": n.rev,
        "site": n.site,
        "bot_email": n.bot_email,
        "stream": n.stream,
        "people": n.people,
        "api_key": key_info(&mut names, n.key.as_ref(), &n.site, "the API key")?,
        "last_ok_at": n.last_ok_at,
        "last_failure": n.last_failure,
        "last_failure_at": n.last_failure_at,
        "direct_failure": n.direct_failure,
        "direct_failure_at": n.direct_failure_at,
        "pending": n.pending,
        "expired": n.expired,
        "set_by": names.person(&n.set_by)?,
        "set_at": n.set_at,
    }))
}

fn answer(st: &Store, n: Option<&ZulipNotifier>) -> clax_core::Result<Value> {
    Ok(json!({"notifier": n.map(|n| notifier_view(st, n)).transpose()?}))
}

/// `GET /api/queues/<qid>/notifiers/zulip` (see the module documentation),
/// recorded as `view.admin {page: "queue_notifier", queue_id}`, once a window.
pub async fn get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, person).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = answer(st, st.zulip_notifier(&qid)?.as_ref())?;
            let rec = view(st, AuditKind::ViewAdmin)
                .with("page", "queue_notifier")
                .with("queue_id", qid.as_str());
            record_view(
                st,
                &bot,
                &who,
                &caller,
                &format!("queue_notifier/{qid}"),
                rec,
            )?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/queues/<qid>/notifiers/zulip`: sets the notifier. `if_rev` is
/// the `rev` this was made from, null or left out for none; `api_key` left
/// out keeps the key set, on the same site. 400 `notifier_changed`, or for a
/// field [`clax_store::Store::set_zulip_notifier`] refuses; 503 `no_kek`.
pub async fn put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<ZulipFields>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    let bot = s.bot.clone();
    bot.kek()?;
    let v = s
        .store_call(move |st| {
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            let by_owner = check_may_grant_admin(role_of(st, &caller)?).is_ok();
            let n = st.set_zulip_notifier(&ctx, kek, &qid, &b, by_owner)?;
            answer(st, Some(&n))
        })
        .await?;
    Ok(Json(v))
}

/// The body of `DELETE` and of the test message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevBody {
    if_rev: String,
}

/// `DELETE /api/queues/<qid>/notifiers/zulip {if_rev}` → 204: removes the
/// notifier, its key and the notices it had not sent (no notifier is fine).
pub async fn delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<RevBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let (_, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| st.clear_zulip_notifier(&ctx, &qid, &b.if_rev))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/queues/<qid>/notifiers/zulip/test {if_rev}`: "Send a test
/// message", recorded as `queue.update {queue_id, op: "notifier", kind, test:
/// true}` before it goes. It uses the saved settings, and reaches the stream
/// alone. 400 `no_notifier`, `notifier_changed`; 429 `test_too_soon`; 502
/// `notifier_failed`; 503 `no_kek`.
pub async fn test(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<RevBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (_, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    let kek = s.bot.kek()?.clone();
    let q = qid.clone();
    let admitted = s
        .store_call(move |st| {
            let Some(n) = st.zulip_notifier(&q)? else {
                return Err(CoreError::invalid(
                    "no_notifier",
                    "this queue has no Zulip notifier",
                ));
            };
            if n.rev != b.if_rev {
                return Err(CoreError::invalid(
                    "notifier_changed",
                    "this notifier has changed since it was read: read it again",
                ));
            }
            st.record_notifier_test(&ctx, &q)
        })
        .await?;
    if !admitted {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "test_too_soon",
            "a test message went a moment ago: try again in half a minute",
        ));
    }
    crate::notifiers::send_test(&s, kek, qid.clone())
        .await
        .map_err(|f| ApiError::new(StatusCode::BAD_GATEWAY, "notifier_failed", f.words))?;
    let v = s
        .store_call(move |st| answer(st, st.zulip_notifier(&qid)?.as_ref()))
        .await?;
    Ok(Json(v))
}
