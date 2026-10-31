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
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::CoreError;
use clax_core::rights::RightType;
use clax_core::visibility::UNCONNECTED;
use clax_http::agent::{Agent, DeniedToGrants, TokenOrAgent};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_http::views::approval::{approval_view, request_view};
use clax_realtime::feedback::apply_decision;
use clax_rt::time::Instant;
use clax_store::Store;
use clax_store::app_data::AppDataOp;
use clax_store::approvals::{AskApproval, Decision, OVERFLOW_BANNER, RequestStatus, Scope, Via};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Whether a person's agent, whose read rights are `reads`, may read
/// artifact `aid` (gone artifacts are no one's); every other caller may.
/// An agent's lists leave out requests and approvals about artifacts its
/// rights exclude (§7.5).
fn may_read(
    st: &Store,
    reads: &clax_http::agent::ReadRights,
    aid: Option<&str>,
) -> clax_core::Result<bool> {
    let Some(aid) = aid.filter(|_| reads.filters()) else {
        return Ok(true);
    };
    let Ok(id) = clax_core::ArtifactId::parse(aid) else {
        return Ok(false);
    };
    Ok(st.get_artifact(&id)?.is_some_and(|a| reads.allows(&a)))
}

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
/// 404 for a caller who approves nothing (a LAN viewer, a revoked person,
/// no credential); 403 `browser_required` for every other caller: the
/// token alone, the CLI, an agent of anyone's.
fn browser_approver(c: &Caller, what: &str) -> Result<Option<String>, ApiError> {
    match c {
        Caller::Owner { browser: true } => Ok(None),
        Caller::Person { viewer } => Ok(Some(viewer.public_id.clone())),
        // No approver at all (a LAN viewer, a revoked person, no
        // credential): nothing of theirs to change (§9.3).
        Caller::Viewer { .. } | Caller::Anonymous => Err(ApiError::not_found()),
        _ => Err(ApiError::forbidden(
            "browser_required",
            format!("{what} is your person's, in their browser: open People in Clax"),
        )),
    }
}

/// `GET /api/approvals` → `{approvals}`: the caller's person's live
/// approvals, oldest first; a person's agent's leaves out those about an
/// artifact its rights exclude. For an agent, an approval's
/// `artifact_title` goes through its visibility: null where the title is
/// withheld from it.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ApprovalsList>,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    let me = whose(&a.caller)?;
    let reads = a.read_rights();
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let v = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            let mut views = Vec::new();
            for x in st.live_approvals(me.as_deref())? {
                if may_read(st, &reads, x.artifact_id.as_deref())? {
                    let mut view = approval_view(st, &x)?;
                    vis.filter_approval(st, &mut view)?;
                    views.push(view);
                }
            }
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
/// first (the open ones only by default; 50, at most 200; a person's
/// agent's leaving out those about an artifact its rights exclude), how many are
/// open, and People's banner (spec §9.2) while routed content waits beyond
/// the open-request limit without a request, else `null`. 400
/// `invalid_query` for another status.
pub async fn requests(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ApprovalRequestsList>,
    _o: SameOrigin,
    q: Result<Query<RequestsQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let me = whose(&a.caller)?;
    let reads = a.read_rights();
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
            let mut views = Vec::new();
            for r in &rows {
                if may_read(st, &reads, Some(&r.artifact_id))? {
                    views.push(request_view(st, r)?);
                }
            }
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
/// keeps them held. 404 for another's request, and on a hosted daemon for
/// one about a page now out of the approver's reach; 409 `request_closed`
/// when it is not open; 400 `invalid_scope`.
pub async fn decide(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<DecideBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let me = browser_approver(&d.caller(&s).await?, "deciding an approval")?;
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
    // On hosted, a request about a page out of the approver's reach now
    // (its share ended, say) is answered as one that does not exist: it
    // lets nothing through and names nothing of the page.
    let member = s.hosted.as_ref().map(|_| hosted.and_then(|h| h.0.member));
    let v = s
        .store_call(move |st| {
            // The request first, so a request of no one's makes no audit row.
            let r = st.approval_request(&rid, me.as_deref())?;
            if let Some(m) = &member {
                let read = (RightType::Artifacts, "read");
                let reads = match m {
                    Some(m) => m.allows_at(st, read, &r.artifact_id)?,
                    None => false,
                };
                if !reads {
                    return Err(CoreError::NotFound);
                }
            }
            let audit = audit.resolve(st)?;
            let d = st.decide_request(&audit, &rid, me.as_deref(), decision, via)?;
            apply_decision(
                &ctx,
                st,
                &d.touched,
                d.request.approver.as_deref(),
                &d.request.author,
            );
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
/// `unknown_thread` (on a hosted daemon also for an artifact out of the
/// approver's reach, or a thread of one).
pub async fn create(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _o: SameOrigin,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
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
    // On hosted, a scope naming an artifact out of the approver's reach (or
    // a thread of one) is answered as one that does not exist.
    if s.hosted.is_some() {
        let member = hosted.and_then(|h| h.0.member);
        let sc = scope.clone();
        let refusal = s
            .store_call(move |st| {
                let reads = |aid: &str| -> clax_core::Result<bool> {
                    match &member {
                        Some(m) => m.allows_at(st, (RightType::Artifacts, "read"), aid),
                        None => Ok(false),
                    }
                };
                Ok(match &sc {
                    Scope::Everywhere => None,
                    Scope::Artifact(a) => {
                        (!reads(a)?).then_some(("unknown_artifact", "the scope names no artifact"))
                    }
                    // A missing thread too, before the store checks the
                    // author: one out of reach answers as it does.
                    Scope::Thread(t) => match st.get_thread(t)? {
                        Some(th) if reads(&th.artifact_id)? => None,
                        _ => Some(("unknown_thread", "the scope names no thread")),
                    },
                })
            })
            .await?;
        if let Some((code, message)) = refusal {
            return Err(ApiError::bad_request(code, message));
        }
    }
    let via = via(&who);
    let ctx = s.feedback_ctx();
    let (made, v) = s
        .store_call(move |st| {
            let before = st.live_approvals(me.as_deref())?.len();
            let audit = audit.resolve(st)?;
            let (a, touched) = st.approve_touched(&audit, me.as_deref(), &b.author, scope, via)?;
            apply_decision(&ctx, st, &touched, me.as_deref(), &a.author);
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
            clax_realtime::feedback::publish_decision(
                &ctx,
                st,
                &touched,
                &clax_realtime::feedback::decision_parties(a.approver.as_deref(), &a.author),
            );
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
fn answer(db: &Store, r: &clax_store::approvals::ApprovalRequest) -> clax_core::Result<Value> {
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
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::ApprovalsAsk>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AskBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let sid = path(p)?;
    let b = body(req)?;
    if b.author.as_deref().is_some_and(|x| {
        x == UNCONNECTED
            || !(clax_core::is_public_id(x) || clax_core::visibility::is_remote_author(x))
    }) {
        return Err(ApiError::bad_request(
            "invalid_args",
            "author is a person's public ID, or an author on a public Clax as a withheld field names them",
        ));
    }
    // The artifact through the agent's gate: named, or the thread's.
    let named = match &b.artifact_id {
        Some(raw) => Some(a.named_artifact(&s, raw).await?),
        None => None,
    };
    if let Some(tid) = b.thread_id.clone() {
        let aid = s
            .store_call(move |db| Ok(db.get_thread(&tid)?.map(|t| t.artifact_id)))
            .await?
            .ok_or_else(ApiError::not_found)?;
        a.named_artifact(&s, &aid).await?;
    }
    // Outside threads, the artifact's documents' authors, from its store:
    // none when it has no store; any other failure fails the ask, which
    // would otherwise ask as if its documents had no authors.
    let doc_authors = match (&b.thread_id, &named) {
        (None, Some(id)) => {
            s.app_data_call(id, AppDataOp::Authors, |r| match r {
                Err(CoreError::NotFound) => Ok(Vec::new()),
                r => r?.into_strings(),
            })
            .await?
        }
        _ => Vec::new(),
    };
    let q = AskApproval {
        session_id: sid.clone(),
        artifact_id: named.map(|id| id.to_string()),
        thread_id: b.thread_id,
        author: b.author,
        doc_authors,
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

/// `GET /api/sessions/<sid>/approvals/<rid>?wait=<s>` (the session's agent)
/// → `{approval_id, status, scope, request, waited_s}` as soon as the
/// request is not open, or after `wait` seconds (at most an hour, as a
/// question's poll), or when the daemon shuts down. 404 unless it is a
/// request of the session's person about an artifact the agent may read; 429 `limit_reached` when four polls
/// hold it already.
pub async fn poll(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::ApprovalsPoll>,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<super::questions::WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, rid) = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let started = Instant::now();
    let wait = Duration::from_secs(q.wait.min(crate::routes::questions::MAX_WAIT_SECS));
    let (sid1, rid1) = (sid.clone(), rid.clone());
    let reads = a.read_rights();
    let approver = s
        .store_call(move |db| {
            let me = live_person(db, &sid1)?;
            let r = db.approval_request(&rid1, me.as_deref())?;
            // A request about an artifact the agent may not read is none of
            // its business (§7.5).
            if !may_read(db, &reads, Some(&r.artifact_id))? {
                return Err(CoreError::NotFound);
            }
            Ok(me)
        })
        .await?;
    let Some((notify, _hold)) = s.questions.hold(&rid, None) else {
        return Err(super::questions::polls_limit("request"));
    };
    let read = |end: super::questions::Ending| {
        let (rid1, me, s) = (rid.clone(), approver.clone(), s.clone());
        async move {
            s.store_call(move |db| {
                let r = db.approval_request(&rid1, me.as_deref())?;
                if r.status == RequestStatus::Open && !end.done {
                    return Ok(None);
                }
                answer(db, &r).map(Some)
            })
            .await
        }
    };
    let mut v = super::questions::long_poll(&s, &rid, notify, wait, read).await?;
    v["waited_s"] = json!(started.elapsed().as_secs());
    Ok(Json(v))
}
