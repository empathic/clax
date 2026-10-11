//! Approvals and their requests (spec 2026-10-08-multi-agent-multi-person
//! §9, §15, M18–M20, R1, R2).
//!
//! Reading is Own (§7.5): `GET /api/approvals` and `GET
//! /api/approvals/requests` serve the caller's person, the owner's or a
//! connected person's, from their browser, the CLI or their agents, and
//! never the withheld content. Changing is the approver's alone, from
//! their browser on a same-origin request (M20, R2): deciding a request,
//! approving directly and revoking take `Caller::browser()` (403
//! `browser_required` for the token, the CLI, an agent) and `SameOrigin`
//! (403 `forbidden_origin` for a page); another person's request or
//! approval is 404. Every decision and revocation bumps the visibility
//! generation (M17) and wakes every delivery path of the approver's
//! agents, the Codex push included.
//!
//! An agent asks with `POST /api/sessions/<sid>/approvals` and waits with
//! `GET /api/sessions/<sid>/approvals/<rid>?wait=` (as questions wait).

use super::artifacts::{body, path};
use crate::agent::{Agent, DeniedToGrants, TokenOrAgent};
use crate::approvals::{approval_view, request_view};
use crate::audit::DeferredAudit;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::feedback::apply;
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::store::approvals::{
    AskApproval, Decision, OVERFLOW_BANNER, RequestStatus, Scope, UNCONNECTED, Via,
};
use clax_core::{CoreError, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::Instant;

/// Whose approvals a read is about (§7.5 Own): the owner's (`None`) for
/// the owner's browsers, the CLI and the owner's agents (a grant of the
/// owner's row included); a connected person's for their browser and
/// their agents.
///
/// # Errors
/// 403 `forbidden` for a LAN viewer who is not connected, or anyone else.
fn whose(c: &Caller) -> Result<Option<String>, ApiError> {
    match c {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => Ok(None),
        Caller::PersonAgent { person, .. } => Ok(person.clone()),
        Caller::Person { viewer } => Ok(Some(viewer.public_id.clone())),
        Caller::Viewer { .. } | Caller::Anonymous => Err(ApiError::forbidden(
            "forbidden",
            "only the owner or a connected person has approvals",
        )),
    }
}

/// The approver a change is from (M20, R2): the owner (`None`) from the
/// owner's browser, a connected person from theirs.
///
/// # Errors
/// 403 `browser_required` for every other caller: the token alone, the
/// CLI, an agent of anyone's.
fn browser_approver(c: &Caller, what: &str) -> Result<Option<String>, ApiError> {
    match c {
        Caller::Owner { browser: true } => Ok(None),
        Caller::Person { viewer } => Ok(Some(viewer.public_id.clone())),
        _ => Err(ApiError::forbidden(
            "browser_required",
            format!("{what} is your person's, in their browser: open People in Clax"),
        )),
    }
}

/// `GET /api/approvals` → `{approvals}`: the caller's person's live
/// approvals, oldest first.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<crate::agent::ApprovalsList>,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    let me = whose(&a.caller)?;
    let v = s
        .store_call(move |st| {
            let views = st
                .live_approvals(me.as_deref())?
                .iter()
                .map(|x| approval_view(st, x))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({"approvals": views}))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct RequestsQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

/// `GET /api/approvals/requests?status=open|all&limit=<1..200>` →
/// `{requests, open, banner}`: the caller's person's requests, newest
/// first (the open ones only by default; 50, at most 200), how many are
/// open, and People's banner (spec §9.2) while routed content waits beyond
/// the open-request limit without a request, else `null`. 400
/// `invalid_query` for another status.
pub async fn requests(
    State(s): State<AppState>,
    a: Agent<crate::agent::ApprovalRequestsList>,
    _o: SameOrigin,
    q: Result<Query<RequestsQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let me = whose(&a.caller)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let open = match q.status.as_deref() {
        None | Some("open") => true,
        Some("all") => false,
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_query",
                "status is open or all",
            ));
        }
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let v = s
        .store_call(move |st| {
            let (rows, n, overflow) = st.approval_requests(me.as_deref(), open, limit)?;
            let views = rows
                .iter()
                .map(|r| request_view(st, r))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({"requests": views, "open": n,
                      "banner": overflow.then_some(OVERFLOW_BANNER)}))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideBody {
    scope: String,
}

/// Where a browser's change was made: the extension's credential, or the
/// shell.
fn via(who: &Identity) -> Via {
    if who.extension {
        Via::Extension
    } else {
        Via::Shell
    }
}

/// `POST /api/approvals/requests/<rid>/decide` `{scope: "thread" |
/// "artifact" | "everywhere" | "decline"}` (the approver's browser, spec
/// §9.3) → `{request, approval}`. A thread approval covers the author's
/// later replies in that thread (R1). Approving lets the held rows through
/// to every delivery path of the approver's agents at once; declining
/// keeps them held. 404 for another's request; 409 `request_closed` when
/// it is not open; 400 `invalid_scope`.
pub async fn decide(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<DecideBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let me = browser_approver(&caller, "deciding an approval")?;
    let rid = path(p)?;
    let b = body(req)?;
    let Some(decision) = Decision::parse(&b.scope) else {
        return Err(ApiError::bad_request(
            "invalid_scope",
            "scope is thread, artifact, everywhere or decline",
        ));
    };
    let via = via(&who);
    let ctx = s.feedback_ctx();
    let v = s
        .store_call(move |st| {
            // The request first, so a request of no one's makes no audit row.
            st.approval_request(&rid, me.as_deref())?;
            let audit = audit.resolve(st)?;
            let d = st.decide_request(&audit, &rid, me.as_deref(), decision, via)?;
            apply(&ctx, st, &d.touched);
            let approval = d
                .approval
                .as_ref()
                .map(|a| approval_view(st, a))
                .transpose()?;
            Ok(json!({"request": request_view(st, &d.request)?, "approval": approval}))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowBody {
    /// A public ID, or `*unconnected` (the owner's switch, §9.6).
    author: String,
    scope: String,
    #[serde(default)]
    artifact_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
}

/// `POST /api/approvals` `{author, scope, artifact_id?, thread_id?}` (the
/// approver's browser; People's "Always allow", spec §9.3) → 201
/// `{approval}`, or 200 with the live one that already says it. It closes
/// the approver's open requests it covers and lets their held rows
/// through. 400 `invalid_scope`, `invalid_author`, `unknown_artifact`,
/// `unknown_thread`.
pub async fn create(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    who: Identity,
    audit: DeferredAudit,
    req: Result<Json<AllowBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let me = browser_approver(&caller, "approving content")?;
    let b = body(req)?;
    let scope = match (b.scope.as_str(), b.artifact_id, b.thread_id) {
        ("everywhere", None, None) => Scope::Everywhere,
        ("artifact", Some(a), None) => Scope::Artifact(a),
        ("thread", _, Some(t)) => Scope::Thread(t),
        _ => {
            return Err(ApiError::bad_request(
                "invalid_scope",
                "scope is everywhere, artifact with artifact_id, or thread with thread_id",
            ));
        }
    };
    let via = via(&who);
    let ctx = s.feedback_ctx();
    let (made, v) = s
        .store_call(move |st| {
            let before = st.live_approvals(me.as_deref())?.len();
            let audit = audit.resolve(st)?;
            let (a, touched) = st.approve_touched(&audit, me.as_deref(), &b.author, scope, via)?;
            apply(&ctx, st, &touched);
            let made = st.live_approvals(me.as_deref())?.len() > before;
            Ok((made, json!({"approval": approval_view(st, &a)?})))
        })
        .await?;
    let status = if made {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(v)))
}

/// `DELETE /api/approvals/<id>` (the approver's browser, spec §9.4) →
/// `{approval}`: from the next take or read on, what it covered is
/// withheld again unless another live approval covers it; what was
/// delivered stays delivered. 404 unless it is a live approval of the
/// caller's. Revoking the owner's `*unconnected` approval is the switch's
/// off (§9.6).
pub async fn revoke(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let me = browser_approver(&caller, "revoking an approval")?;
    let id = path(p)?;
    let ctx = s.feedback_ctx();
    let v = s
        .store_call(move |st| {
            st.live_approval(&id, me.as_deref())?;
            let audit = audit.resolve(st)?;
            let (a, touched) = st.revoke_approval_touched(&audit, &id, me.as_deref())?;
            // Their waits learn of it at once (M17); nothing is delivered.
            ctx.waiters.wake(&touched.targets);
            Ok(json!({"approval": approval_view(st, &a)?}))
        })
        .await?;
    Ok(Json(v))
}

/// Checks that session `sid` exists (404) and is live (400
/// `unknown_session`); its person (`None`: the owner).
fn live_person(db: &Store, sid: &str) -> clax_core::Result<Option<String>> {
    match db.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "no live session has this ID",
        )),
        Some(s) => Ok(s.person),
    }
}

/// An agent's answer about request `r`: `{approval_id, status, scope,
/// request}` (§9.2: `status` is `open`, `approved`, `declined` or
/// `withdrawn`).
fn answer(
    db: &Store,
    r: &clax_core::store::approvals::ApprovalRequest,
) -> clax_core::Result<Value> {
    Ok(json!({
        "approval_id": r.id,
        "status": r.status,
        "scope": r.decided_scope,
        "request": request_view(db, r)?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskBody {
    #[serde(default)]
    artifact_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    /// A public ID.
    #[serde(default)]
    author: Option<String>,
}

/// `POST /api/sessions/<sid>/approvals` `{artifact_id?, thread_id?,
/// author?}` (the session's agent; `ask` with `approval`, spec §9.2) → 201
/// `{approval_id, status: "open", scope: null, request, requests}`: one
/// request per author whose content at the thread (or, without one, at the
/// artifact outside its threads) is withheld from the agent, made or
/// joined, the first as `approval_id`. 400 `nothing_withheld`,
/// `recently_declined` (a decline there less than ten minutes ago),
/// `invalid_args`; 429 `limit_reached` when the person holds 100 open
/// requests; 404 for a thread or artifact the agent cannot name.
/// Recorded as `approval.request` by the session's agent.
pub async fn ask(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::ApprovalsAsk>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AskBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let sid = path(p)?;
    let b = body(req)?;
    if b.author
        .as_deref()
        .is_some_and(|x| x == UNCONNECTED || !clax_core::is_public_id(x))
    {
        return Err(ApiError::bad_request(
            "invalid_args",
            "author is a person's public ID",
        ));
    }
    // The artifact through the agent's gate: named, or the thread's.
    let named = match &b.artifact_id {
        Some(raw) => Some(a.named_artifact(&s, raw).await?.to_string()),
        None => None,
    };
    if let Some(tid) = b.thread_id.clone() {
        let aid = s
            .store_call(move |db| Ok(db.get_thread(&tid)?.map(|t| t.artifact_id)))
            .await?
            .ok_or_else(ApiError::not_found)?;
        a.named_artifact(&s, &aid).await?;
    }
    let q = AskApproval {
        session_id: sid.clone(),
        artifact_id: named,
        thread_id: b.thread_id,
        author: b.author,
    };
    let v = s
        .store_call(move |db| {
            live_person(db, &sid)?;
            let ctx = audit.for_session(db, &sid)?;
            let rs = db.ask_approval(&ctx, &q)?;
            let mut v = answer(db, &rs[0])?;
            v["requests"] = json!(
                rs.iter()
                    .map(|r| request_view(db, r))
                    .collect::<clax_core::Result<Vec<_>>>()?
            );
            Ok(v)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(v)))
}

#[derive(Deserialize)]
pub struct WaitQuery {
    #[serde(default)]
    wait: u64,
}

/// `GET /api/sessions/<sid>/approvals/<rid>?wait=<s>` (the session's agent)
/// → `{approval_id, status, scope, request, waited_s}` as soon as the
/// request is not open, or after `wait` seconds (at most an hour, as a
/// question's poll), or when the daemon shuts down. 404 unless it is a
/// request of the session's person; 429 `limit_reached` when four polls
/// hold it already.
pub async fn poll(
    State(s): State<AppState>,
    TokenOrAgent(_a): TokenOrAgent<crate::agent::ApprovalsPoll>,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, rid) = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let started = Instant::now();
    let wait = Duration::from_secs(q.wait.min(crate::routes::questions::MAX_WAIT_SECS));
    let (sid1, rid1) = (sid.clone(), rid.clone());
    let approver = s
        .store_call(move |db| {
            let me = live_person(db, &sid1)?;
            db.approval_request(&rid1, me.as_deref())?;
            Ok(me)
        })
        .await?;
    let Some((notify, _hold)) = s.questions.hold(&rid, None) else {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!(
                "at most {} polls wait on one request at once",
                crate::questions::MAX_POLLS
            ),
        ));
    };
    let mut shutdown = s.shutdown.clone();
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let expiry = s.question_sleeper.sleep(wait);
    tokio::pin!(expiry);
    let mut done = wait.is_zero();
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        #[cfg(debug_assertions)]
        if s.questions.take_expired(&rid) {
            done = true;
        }
        let (rid1, me) = (rid.clone(), approver.clone());
        let out = s
            .store_call(move |db| {
                let r = db.approval_request(&rid1, me.as_deref())?;
                if r.status == RequestStatus::Open && !done {
                    return Ok(None);
                }
                answer(db, &r).map(Some)
            })
            .await?;
        if let Some(mut v) = out {
            v["waited_s"] = json!(started.elapsed().as_secs());
            return Ok(Json(v));
        }
        tokio::select! {
            () = &mut notified => {}
            () = &mut expiry, if !done => done = true,
            () = &mut stopping, if !done => done = true,
        }
    }
}
