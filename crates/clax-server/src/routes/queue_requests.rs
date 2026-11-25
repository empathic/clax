//! Queue requests (`docs/contract.md`, "Queue requests"): an agent asks its
//! person for a report queue, the person approves or denies it in their
//! browser, and the agent reads the answer once. No agent makes a queue
//! (`POST /api/queues` refuses them); the store keeps the rules
//! ([`clax_store::queue_requests`]).
//!
//! - `POST /api/queue-requests {name, origins?, reporters_see?, note?}`
//!   (any agent: the owner's, a connected person's, an agent token's, a
//!   scoped one's) → 201 `{request}`, waiting for the agent's person; 403
//!   `not_an_agent` for a person or the owner by hand, who make queues
//!   themselves; 400 as `POST /api/queues` for bad fields, or
//!   `invalid_args` for a note over 500 characters; 409 `request_exists`
//!   for a scoped token that asked once; 429 `limit_reached` with five
//!   waiting.
//! - `GET /api/queue-requests/<id>[?wait=<s>]` (the agent that asked) →
//!   `{request}` as soon as it is not pending, or after `wait` seconds (at
//!   most [`MAX_WAIT_SECS`]); an approved request whose grant was taken
//!   also says `grant_taken: true`. A read changes nothing. The request's
//!   person reads it too. 404 for anyone else.
//! - `POST /api/queue-requests/<id>/take` (the agent that asked) → once
//!   the request is answered, the answer: for an approved one the first
//!   take also makes the queue's service grant, `{request, grant: {id,
//!   name, created_at}, token, private_key}`, this once; later takes say
//!   `grant_taken: true`. A take ends a scoped grant. 409
//!   `request_pending` while it waits; 404 for anyone else.
//! - `GET /api/queue-requests[?status=]` (a person's or the owner's browser,
//!   or the owner by hand) → `{requests}`: those they answer, newest first.
//! - `POST /api/queue-requests/<id>/approve` (the person's browser) →
//!   `{request, queue}`: the queue is made with them as its admin, so a
//!   person who may not make one is refused as `POST /api/queues` refuses
//!   them (403 `queues_admin_only`), the request left pending. While an
//!   agent of the owner's is live it may wait for a confirmation
//!   ([`clax_http::confirm`]). `POST /api/queue-requests/<id>/deny` →
//!   `{request}`. 409 `request_closed` with `request` for one no longer
//!   pending.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use base64::Engine as _;
use clax_core::CoreError;
use clax_core::queue::ReportersSee;
use clax_http::agent::{Agent, DeniedToGrants, QueueRequestAsk, QueueRequestGet, QueueRequestTake};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::confirm::{Confirming, Decision, require_confirmation};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::queue_requests::{Ask, QueueRequest, Requester, Status};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Longest `wait` of a request's read, in seconds.
pub const MAX_WAIT_SECS: u64 = 60;

/// The key a request's long polls wait under, apart from questions'.
fn wait_key(id: &str) -> String {
    format!("queue_request:{id}")
}

/// The agent `c` asks as; `None` for a caller who is no agent.
fn requester(c: &Caller) -> Option<Requester> {
    match c {
        Caller::OwnerAgent { session } => Some(Requester {
            person: None,
            grant_id: None,
            session_id: session.clone(),
        }),
        Caller::PersonAgent {
            person,
            grant,
            session,
            ..
        } => Some(Requester {
            person: person.clone(),
            grant_id: Some(grant.clone()),
            session_id: session.clone(),
        }),
        _ => None,
    }
}

/// The person a browser (or the owner by hand) answers requests as: `None`
/// for the owner, a connected person's public ID.
///
/// # Errors
/// 401 without a credential; 403 for an agent or a viewer who is no
/// connected person.
fn approver(c: &Caller) -> Result<Option<String>, ApiError> {
    match c {
        Caller::Owner { .. } => Ok(None),
        Caller::Person { viewer } => Ok(Some(viewer.public_id.clone())),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            Err(clax_http::auth::not_for_agents())
        }
        Caller::Viewer { .. } => Err(ApiError::forbidden(
            "not_a_person",
            "only the owner and connected people answer queue requests",
        )),
    }
}

/// 409 `request_closed` with the request's view, for `e` when it is the
/// store's `request_closed`.
fn closed_with(e: CoreError, s: &clax_store::Store, id: &str) -> ApiError {
    let closed = matches!(
        e,
        CoreError::Invalid {
            code: "request_closed",
            ..
        }
    );
    let mut err = ApiError::from(e);
    if closed && let Ok(Some(r)) = s.queue_request(id) {
        err.extra.insert("request".into(), json!(r));
    }
    err
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskBody {
    name: String,
    #[serde(default)]
    origins: Vec<String>,
    #[serde(default = "own")]
    reporters_see: ReportersSee,
    /// Why, in the agent's words, shown to the person.
    #[serde(default)]
    note: Option<String>,
}

fn own() -> ReportersSee {
    ReportersSee::Own
}

/// `POST /api/queue-requests`, as the module comment says.
pub async fn ask(
    State(s): State<AppState>,
    a: Agent<QueueRequestAsk>,
    audit: DeferredAudit,
    req: Result<Json<AskBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let who = match requester(&a.caller) {
        Some(w) => w,
        None if matches!(a.caller, Caller::Anonymous) => return Err(ApiError::unauthorized()),
        None => {
            return Err(ApiError::forbidden(
                "not_an_agent",
                "a person makes a queue themselves (POST /api/queues, or the Queues page); an agent asks for one here",
            ));
        }
    };
    let b = body(req)?;
    let r = s
        .store_call(move |st| {
            st.request_queue(
                &audit.resolve(st)?,
                &who,
                Ask {
                    name: b.name,
                    origins: b.origins,
                    reporters_see: b.reporters_see,
                    note: b.note,
                },
            )
        })
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "request": r }))))
}

/// `GET /api/queue-requests/<id>`, as the module comment says.
pub async fn get(
    State(s): State<AppState>,
    a: Agent<QueueRequestGet>,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<super::questions::WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let Some(who) = requester(&a.caller) else {
        // The request's person, from their browser or by hand.
        let person = approver(&a.caller)?;
        let id1 = id.clone();
        let r = s.store_call(move |st| st.queue_request(&id1)).await?;
        return match r.filter(|r| r.person == person) {
            Some(r) => Ok(Json(json!({ "request": r }))),
            None => Err(ApiError::not_found()),
        };
    };
    let first = mine(&s, &id, &who).await?;
    let wait = Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let r = if first.status == Status::Pending && !wait.is_zero() {
        let key = wait_key(&id);
        if let Some((notify, _hold)) = s.questions.hold(&key, None) {
            let read = |end: super::questions::Ending| {
                let (id1, s) = (id.clone(), s.clone());
                async move {
                    let r = s.store_call(move |st| st.queue_request(&id1)).await?;
                    Ok(r.filter(|r| r.status != Status::Pending || end.done)
                        .map(drop))
                }
            };
            super::questions::long_poll(&s, &key, notify, wait, read).await?;
        }
        mine(&s, &id, &who).await?
    } else {
        first
    };
    let mut v = json!({ "request": r });
    if r.status == Status::Approved && r.taken_at.is_some() {
        v["grant_taken"] = json!(true);
    }
    Ok(Json(v))
}

/// Request `id` when it is `who`'s own; 404 otherwise.
async fn mine(s: &AppState, id: &str, who: &Requester) -> Result<QueueRequest, ApiError> {
    let id1 = id.to_string();
    let r = s.store_call(move |st| st.queue_request(&id1)).await?;
    r.filter(|r| r.grant_id == who.grant_id && r.person == who.person)
        .ok_or_else(ApiError::not_found)
}

/// `POST /api/queue-requests/<id>/take`, as the module comment says.
pub async fn take(
    State(s): State<AppState>,
    a: Agent<QueueRequestTake>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(p)?;
    let who = match requester(&a.caller) {
        Some(w) => w,
        None if matches!(a.caller, Caller::Anonymous) => return Err(ApiError::unauthorized()),
        None => {
            return Err(ApiError::forbidden(
                "not_an_agent",
                "the agent that asked takes the answer; its person reads it with GET",
            ));
        }
    };
    let taken = s
        .store_call(move |st| st.take_queue_request(&audit.resolve(st)?, &id, &who))
        .await?;
    if taken.request.status == Status::Pending {
        let mut err = ApiError::new(
            StatusCode::CONFLICT,
            "request_pending",
            "the request waits for its person: read it with ?wait= until it is answered",
        );
        err.extra.insert("request".into(), json!(taken.request));
        return Err(err);
    }
    let mut v = json!({ "request": taken.request });
    match taken.grant {
        Some((grant, token, key)) => {
            v["grant"] = json!(grant);
            v["token"] = json!(token.expose());
            v["private_key"] = json!(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.pkcs8().expose())
            );
        }
        None if taken.request.status == Status::Approved => {
            v["grant_taken"] = json!(true);
        }
        None => {}
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    status: Option<String>,
}

/// `GET /api/queue-requests[?status=]`, as the module comment says.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = approver(&d.caller(&s).await?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let status = match q.status.as_deref() {
        None | Some("") => None,
        Some(v) => Some(Status::parse(v).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_query",
                "status is pending, approved, denied, cancelled or expired",
            )
        })?),
    };
    let requests = s
        .store_call(move |st| st.queue_requests_for(person.as_deref(), status))
        .await?;
    Ok(Json(json!({ "requests": requests })))
}

/// The browser answering: the owner's or a connected person's (M20).
fn browser_approver(c: &Caller) -> Result<Option<String>, ApiError> {
    if !c.browser() {
        return Err(ApiError::forbidden(
            "browser_required",
            "a queue request is answered in the person's browser",
        ));
    }
    approver(c)
}

/// `POST /api/queue-requests/<id>/approve`, as the module comment says.
pub async fn approve(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    by: Confirming,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(&s).await?;
    let person = browser_approver(&caller)?;
    let id = path(p)?;
    // An approval makes a queue whose admin is the approver, so it is
    // refused wherever making one by hand is.
    let maker = caller.clone();
    s.store_call(move |st| super::queues::may_make(st, &maker))
        .await??;
    require_confirmation(
        &s,
        &caller,
        &by,
        Decision::ApproveQueueRequest {
            request_id: id.clone(),
        },
    )
    .await?;
    let id1 = id.clone();
    let (r, queue) = s
        .store_call(move |st| {
            Ok(st
                .approve_queue_request(&audit.resolve(st)?, &id1, person.as_deref())
                .map_err(|e| closed_with(e, st, &id1)))
        })
        .await??;
    s.questions.wake(&wait_key(&id));
    Ok(Json(json!({ "request": r, "queue": queue })))
}

/// `POST /api/queue-requests/<id>/deny`, as the module comment says.
pub async fn deny(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = browser_approver(&d.caller(&s).await?)?;
    let id = path(p)?;
    let id1 = id.clone();
    let r = s
        .store_call(move |st| {
            Ok(st
                .deny_queue_request(&audit.resolve(st)?, &id1, person.as_deref())
                .map_err(|e| closed_with(e, st, &id1)))
        })
        .await??;
    s.questions.wake(&wait_key(&id));
    Ok(Json(json!({ "request": r })))
}
