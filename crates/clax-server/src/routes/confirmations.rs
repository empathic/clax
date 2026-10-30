//! Browser confirmations ([`clax_http::confirm`]): while an agent session
//! is live, a decision that matters made with the daemon token answers 202
//! `confirmation_pending` and waits for the owner's click here.
//!
//! - `GET /api/confirmations` (the owner) lists the open ones, for the
//!   shell's inbox cards and toasts.
//! - `GET /api/confirmations/<id>?wait=<s>` (the token) reads one, waiting
//!   up to `s` seconds (at most [`MAX_WAIT_SECS`]) while it is open: what
//!   the CLI long-polls before it makes its call again.
//! - `POST /api/confirmations/<id> {approve}` (the owner's browser) answers
//!   one.
//!
//! A hosted daemon asks for none: every route answers 404 there.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::{Caller, RequireToken};
use clax_http::confirm::{self, Confirming};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

pub use clax_core::confirm::MAX_WAIT_SECS;

fn local_only(s: &AppState) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        Err(ApiError::not_found())
    } else {
        Ok(())
    }
}

fn id_of(p: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    p.map(|Path(id)| id)
        .map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))
}

/// `GET /api/confirmations` (the owner) → `{confirmations}`: the open ones,
/// oldest first, each as [`confirm::view`] gives it (its decision in
/// words, and the agent sessions live now). 403 `not_for_agents` for the
/// owner's agents.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reads confirmations")?;
    let st = s.clone();
    let views = s
        .store_call(move |db| {
            db.open_confirmations()?
                .iter()
                .map(|c| confirm::view(&st, db, c))
                .collect::<clax_core::Result<Vec<_>>>()
        })
        .await?;
    Ok(Json(json!({"confirmations": views})))
}

#[derive(Deserialize)]
pub struct WaitQuery {
    wait: Option<u64>,
}

/// `GET /api/confirmations/<id>?wait=<s>` (the token) →
/// `{confirmation}`: confirmation `id` as [`confirm::view`] gives it. While
/// it is open, the answer waits up to `s` seconds (at most
/// [`MAX_WAIT_SECS`]; none without `wait`) for the owner to answer it. 404
/// for none with that ID.
pub async fn get_one(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    let id = id_of(p)?;
    let wait = q
        .map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?
        .0
        .wait
        .unwrap_or(0)
        .min(MAX_WAIT_SECS);
    // Subscribed before the read: an answer between the two is heard.
    let mut heard = s.events.subscribe();
    let v = read(&s, id.clone()).await?;
    if v["status"] != "open" || wait == 0 {
        return Ok(Json(json!({"confirmation": v})));
    }
    let deadline = clax_rt::time::Instant::now() + Duration::from_secs(wait);
    loop {
        let left = deadline.saturating_duration_since(clax_rt::time::Instant::now());
        // The deadline, a closed or lagging bus: read it again now.
        let Ok(Ok(e)) = clax_rt::time::timeout(left, heard.recv()).await else {
            break;
        };
        if let clax_core::Event::Confirmation { confirmation } = &e.event
            && confirmation["id"] == id.as_str()
            && confirmation["status"] != "open"
        {
            break;
        }
    }
    let v = read(&s, id).await?;
    Ok(Json(json!({"confirmation": v})))
}

/// Confirmation `id` as [`confirm::view`] gives it; 404 for none.
async fn read(s: &AppState, id: String) -> Result<Value, ApiError> {
    let st = s.clone();
    s.store_call(move |db| {
        let c = db
            .confirmation(&id)?
            .ok_or(clax_core::CoreError::NotFound)?;
        confirm::view(&st, db, &c)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    approve: bool,
    /// A passkey's assertion over a challenge for `confirm:<id>`, when
    /// passkeys are required.
    #[serde(default)]
    assertion: Option<confirm::Assertion>,
}

/// `POST /api/confirmations/<id> {approve, assertion?}` (the owner's
/// browser) → `{confirmation}`: approves (`true`) or declines confirmation
/// `id`. An owner browser session or the in-process app's frame answers
/// one: 403 `browser_required` for the token, the key cookie and the
/// extension. While passkeys are required, approving from a browser
/// session takes `assertion`, over a challenge issued for `confirm:<id>`
/// (`POST /api/passkeys/challenge`): 403 `passkey_required` without it, 403
/// `passkey_refused` for one that does not check. 404 for none with that
/// ID; 410 `confirmation_expired` past its time; 409
/// `confirmation_answered` for one answered before.
pub async fn answer(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    by: Confirming,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    if !caller.owner_browser() || !by.may_answer() {
        return Err(ApiError::forbidden(
            "browser_required",
            "a confirmation is answered in the owner's browser: open the inbox in Clax",
        ));
    }
    let id = id_of(p)?;
    let Json(b) = req.map_err(|e| ApiError::bad_request("invalid_json", e.body_text()))?;
    let st = s.clone();
    let with_passkey = by.answers_with_passkey();
    let v = s
        .store_call(move |db| {
            if b.approve && with_passkey && db.passkeys_required()? {
                let Some(a) = &b.assertion else {
                    return Err(clax_core::CoreError::invalid(
                        "passkey_required",
                        "confirming needs your passkey (Touch ID or another)",
                    ));
                };
                confirm::check_assertion(&st, db, &format!("confirm:{id}"), a)?;
            }
            let c = db.answer_confirmation(&id, b.approve)?;
            confirm::view(&st, db, &c)
        })
        .await
        .map_err(|e| confirm::passkey_error(answer_error(e)))?;
    s.events.publish(clax_core::Event::Confirmation {
        confirmation: v.clone(),
    });
    Ok(Json(json!({"confirmation": v})))
}

/// The store's refusals of an answer with their statuses: 410 past its
/// time, 409 answered before.
fn answer_error(e: ApiError) -> ApiError {
    match e.code {
        "confirmation_expired" => ApiError::new(axum::http::StatusCode::GONE, e.code, e.message),
        "confirmation_answered" => {
            ApiError::new(axum::http::StatusCode::CONFLICT, e.code, e.message)
        }
        _ => e,
    }
}
