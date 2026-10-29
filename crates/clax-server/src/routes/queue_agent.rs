//! A queue's reports for its triage members and their agents (spec
//! 2026-10-27-clax-sdk-design §7.3, §7.4), through `Agent<N>`:
//!
//! - `GET /api/queues/<qid>/reports?state=&cursor=` → `{reports, next}`;
//!   `GET /api/queues/<qid>/reports/<tid>` → `{report}`.
//! - `POST …/reports/<tid>/claim`, `…/renew` → a lease (`{lease_id,
//!   expires_at, fence, report}`); `…/release` → `{report}`.
//! - `POST …/reports/<tid>/comments {body}` → 201 `{comment, report}`;
//!   `POST …/reports/<tid>/resolve` → `{report}`.
//! - `PATCH …/reports/<tid> {metadata, if_version}` (the thread's),
//!   `PATCH …/reports/<tid>/artifact {metadata, if_version}` (its external
//!   artifact's) → `{metadata_version, report}`.
//! - `GET …/reports/<tid>/clip`: the PNG; `GET
//!   …/reports/<tid>/comments/<cid>/clip`: a reopen comment's PNG.
//! - `POST …/reports/<tid>/fix {summary, url?, commit?, release?,
//!   verify_url?, verify_after?, fence?}` → `{report}`, `fix_proposed`.
//! - `POST …/reports/<tid>/verify {note?}` and `…/reopen {body}` →
//!   `{report}`: a person's verdict on the reporter's behalf; agents are
//!   refused (403 `not_for_agents`), so no agent marks its own fix
//!   verified.
//! - `GET …/reports/<tid>/diagnostics?capture=` → `{lines}`.
//!
//! The caller acts as a person: the owner (the owner's browser or token,
//! and the owner's agents), or a connected person (their browser, and
//! their agents). Anyone who is not a member of the queue gets 404
//! `not_a_member`, as for a queue that does not exist.
//!
//! An agent claims, renews, releases, replies, proposes fixes, resolves
//! and writes metadata from its session (`X-Clax-Session`; 400 `no_session` without
//! one), and writes to a report only under its own live lease: 409
//! `claim_required` when its session never claimed the report or released
//! it, 409 `lease_lost` when its lease expired or a later one superseded
//! it, 409 `thread_taken` when another session holds it. A person (not an
//! agent) replies, proposes fixes, resolves and writes metadata without a
//! claim.

use super::artifacts::{body, path, publishing_session};
use super::threads::publish_thread;
use axum::Json;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::model::Session;
use clax_core::queue::{ReportOrder, StateFilter};
use clax_core::report::{NewFix, ResolveReason};
use clax_core::working::StopReason;
use clax_core::{CoreError, Event};
use clax_http::agent::{
    Agent, DeniedToGrants, Need, QueueArtifactPatch, QueueClaim, QueueClip, QueueCommentClip,
    QueueDiagnostics, QueueFix, QueueRelease, QueueRenew, QueueReply, QueueReportGet,
    QueueReportPatch, QueueReportsList, QueueResolve, QueueUnwatch, QueueWatch,
};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::{SameOrigin, author};
use clax_realtime::feedback::apply;
use clax_store::Store;
use clax_store::queues::{ReportWriter, Verdict, Visibility};
use serde::Deserialize;
use serde_json::{Value, json};

/// The person a caller acts as on a queue (`Some(None)`: the owner), or
/// `None` for a caller who is no one's (a guest's cookie, no credential).
fn queue_person(c: &Caller) -> Option<Option<String>> {
    match c {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => Some(None),
        Caller::PersonAgent { person, .. } => Some(person.clone()),
        Caller::Person { viewer } => Some(Some(viewer.public_id.clone())),
        Caller::Viewer { .. } | Caller::Anonymous => None,
    }
}

/// 404 `not_a_member` unless the caller of `a` acts as a member of queue
/// `qid` (either role reads, claims and writes reports).
async fn member<N: Need>(s: &AppState, a: &Agent<N>, qid: &str) -> Result<(), ApiError> {
    member_as(s, &a.caller, qid).await
}

/// [`member`] for caller `c`.
async fn member_as(s: &AppState, c: &Caller, qid: &str) -> Result<(), ApiError> {
    let Some(person) = queue_person(c) else {
        return Err(CoreError::NotAMember.into());
    };
    let qid = qid.to_string();
    let role = s
        .store_call(move |st| st.queue_role(&qid, person.as_deref()))
        .await?;
    role.map(|_| ()).ok_or_else(|| CoreError::NotAMember.into())
}

/// The live session an agent caller acts from: 400 `no_session` without
/// one; `unknown_session` or `foreign_session` for one that is not live
/// or not its credential's.
fn agent_session(st: &Store, c: &Caller) -> clax_core::Result<Session> {
    let grant = match c {
        Caller::PersonAgent { grant, .. } => Some(grant.as_str()),
        _ => None,
    };
    let Some(sid) = c.session().map(str::to_string) else {
        return Err(no_session());
    };
    let sid = publishing_session(st, &Some(sid), grant)?.ok_or_else(no_session)?;
    st.get_session(&sid)?.ok_or(CoreError::NotFound)
}

fn no_session() -> CoreError {
    CoreError::invalid(
        "no_session",
        "an agent acts on a queue's reports from a session: send X-Clax-Session",
    )
}

/// Who a write to a report is by: the agent's session for an agent
/// caller, else the person themself, named as their comments are.
enum Writer {
    Agent(Box<Session>),
    Person {
        name: String,
        public_id: Option<String>,
    },
}

impl Writer {
    fn of(st: &Store, c: &Caller, who: &Identity) -> clax_core::Result<Writer> {
        if c.is_agent() {
            return Ok(Writer::Agent(Box::new(agent_session(st, c)?)));
        }
        let (name, public_id) = author(st, who)?;
        Ok(Writer::Person { name, public_id })
    }

    fn as_store(&self) -> ReportWriter<'_> {
        match self {
            Writer::Agent(s) => ReportWriter::Agent(s),
            Writer::Person { name, public_id } => ReportWriter::Person {
                name: name.clone(),
                public_id: public_id.clone(),
            },
        }
    }

    fn session(&self) -> Option<&str> {
        match self {
            Writer::Agent(s) => Some(&s.id),
            Writer::Person { .. } => None,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    order: Option<String>,
}

/// `GET /api/queues/<qid>/reports?state=&order=&cursor=` → `{reports,
/// next}`: a page at a time, in order of arrival (`order=oldest`, the
/// default) or latest first (`order=newest`), 400 `invalid_query` for
/// another order; `state` is one of `open`, `claimed`, `fix_proposed`,
/// `verified` and `resolved`, or `active` (`open` or `claimed`); 400
/// `invalid_state` for another.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<QueueReportsList>,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let qid = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let state = match q.state.as_deref() {
        None | Some("") => None,
        Some(x) => Some(StateFilter::parse(x).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_state",
                format!(
                    "state is open, claimed, fix_proposed, verified, resolved or active, not '{x}'"
                ),
            )
        })?),
    };
    let order = match q.order.as_deref() {
        None | Some("") => ReportOrder::Oldest,
        Some(x) => ReportOrder::parse(x).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_query",
                format!("order is oldest or newest, not '{x}'"),
            )
        })?,
    };
    member(&s, &a, &qid).await?;
    let (reports, next) = s
        .store_call(move |st| {
            st.reports_in_order(&qid, Visibility::All, state, q.cursor.as_deref(), order)
        })
        .await?;
    Ok(Json(json!({"reports": reports, "next": next})))
}

/// `GET /api/queues/<qid>/reports/<tid>` → `{report}`.
pub async fn get(
    State(s): State<AppState>,
    a: Agent<QueueReportGet>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let report = s
        .store_call(move |st| {
            st.report(&qid, &tid, Visibility::All)?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"report": report})))
}

/// `POST /api/queues/<qid>/reports/<tid>/claim` → the lease: an agent's
/// session claims the report for [`clax_core::queue::LEASE_SECS`] and owns
/// its thread; 409 `thread_taken` naming the holder while another session
/// holds it. A claim is an agent's alone (400 `no_session` otherwise).
pub async fn claim(
    State(s): State<AppState>,
    a: Agent<QueueClaim>,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let lease = s
        .store_call(move |st| {
            let sess = agent_session(st, &caller)?;
            st.claim(&ctx, &qid, &tid, &sess.id)
        })
        .await?;
    Ok(Json(json!(lease)))
}

/// `POST /api/queues/<qid>/watch` → 204: the agent's session watches the
/// queue (spec 2026-10-29-feedback-loop-design §7.3) until it unwatches it,
/// ends, or its person leaves the queue: each report filed into it, and each reopened, is then a
/// feedback delivery to the session (`wait_for_feedback`, `clax feedback
/// follow`) naming `queue_get` and `queue_claim`. Watching it again
/// changes nothing. A watch is a session's alone (400 `no_session`
/// otherwise).
pub async fn watch(
    State(s): State<AppState>,
    a: Agent<QueueWatch>,
    _o: SameOrigin,
    p: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let qid = path(p)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    s.store_call(move |st| {
        let sess = agent_session(st, &caller)?;
        st.watch_queue(&qid, &sess.id)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/queues/<qid>/watch` → 204: the agent's session no longer
/// watches the queue; one that did not watch it is fine. It removes only
/// the caller's own watch, so it needs no membership: a session whose
/// person left the queue may unwatch it too. 400 `no_session` without a
/// session.
pub async fn unwatch(
    State(s): State<AppState>,
    a: Agent<QueueUnwatch>,
    _o: SameOrigin,
    p: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let qid = path(p)?;
    let caller = a.caller.clone();
    s.store_call(move |st| {
        let sess = agent_session(st, &caller)?;
        st.unwatch_queue(&qid, &sess.id)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/queues/<qid>/reports/<tid>/renew` → the lease, extended;
/// its errors are a write's (`claim_required`, `lease_lost`,
/// `thread_taken`).
pub async fn renew(
    State(s): State<AppState>,
    a: Agent<QueueRenew>,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let lease = s
        .store_call(move |st| {
            let sess = agent_session(st, &caller)?;
            st.renew(&qid, &tid, &sess.id)
        })
        .await?;
    Ok(Json(json!(lease)))
}

/// `POST /api/queues/<qid>/reports/<tid>/release` → `{report}`, open again;
/// its errors are a write's.
pub async fn release(
    State(s): State<AppState>,
    a: Agent<QueueRelease>,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let report = s
        .store_call(move |st| {
            let sess = agent_session(st, &caller)?;
            st.release(&ctx, &qid, &tid, &sess.id)?;
            st.report(&qid, &tid, Visibility::All)?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"report": report})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyBody {
    body: String,
}

/// `POST /api/queues/<qid>/reports/<tid>/comments {body}` → 201
/// `{comment, report}`: an agent's reply under its live lease, or a
/// person's comment. Neither changes the report's state.
pub async fn reply(
    State(s): State<AppState>,
    a: Agent<QueueReply>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<ReplyBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (qid, tid) = path(p)?;
    let b = body(req)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let fctx = s.feedback_ctx();
    let out = s
        .store_call(move |st| {
            let w = Writer::of(st, &caller, &who)?;
            let (c, report) = st.report_reply(&ctx, &qid, &tid, &w.as_store(), &b.body)?;
            fctx.events.publish(Event::Comment {
                artifact_id: report.artifact.id.clone(),
                thread_id: tid.clone(),
                comment: json!(c),
            });
            if let Some(t) = st.get_thread(&tid)? {
                publish_thread(&fctx, st, &t)?;
            }
            Ok(json!({"comment": c, "report": report}))
        })
        .await?;
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ResolveBody {
    #[serde(default)]
    reason: Option<ResolveReason>,
}

/// `POST /api/queues/<qid>/reports/<tid>/resolve {reason?}` → `{report}`:
/// an agent's resolve under its live lease, or a person's, for `reason`
/// (`fixed`, `duplicate`, `wontfix`, `cannot_reproduce` or `other`, the
/// default; an empty body names none); 400 `invalid_args` for another.
pub async fn resolve(
    State(s): State<AppState>,
    a: Agent<QueueResolve>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    let b: ResolveBody = if raw.iter().all(u8::is_ascii_whitespace) {
        ResolveBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_args", e.to_string()))?
    };
    let reason = b.reason.unwrap_or(ResolveReason::Other);
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let an = Announcer::of(&s);
    let report = s
        .store_call(move |st| {
            let w = Writer::of(st, &caller, &who)?;
            let (t, touched, report) =
                st.resolve_report(&ctx, &qid, &tid, &w.as_store(), reason)?;
            announce_resolved(&an, st, &ctx, &t, w.session(), &touched)?;
            Ok(report)
        })
        .await?;
    Ok(Json(json!({"report": report})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataBody {
    metadata: Value,
    if_version: u64,
}

/// `PATCH /api/queues/<qid>/reports/<tid> {metadata, if_version}` →
/// `{metadata_version, report}`: the report's own metadata; 409
/// `conflict` (`path` `metadata`, `current` its version) when `if_version`
/// is not its version.
pub async fn patch(
    State(s): State<AppState>,
    a: Agent<QueueReportPatch>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<MetadataBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    write_metadata(s, a, who, ctx, p, req, false).await
}

/// `PATCH /api/queues/<qid>/reports/<tid>/artifact {metadata, if_version}`
/// → `{metadata_version, report}`: the metadata of the report's external
/// artifact, which every queue's reports on it share.
pub async fn patch_artifact(
    State(s): State<AppState>,
    a: Agent<QueueArtifactPatch>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<MetadataBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    write_metadata(s, a, who, ctx, p, req, true).await
}

async fn write_metadata<N: Need>(
    s: AppState,
    a: Agent<N>,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<MetadataBody>, JsonRejection>,
    artifact: bool,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    let b = body(req)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let out = s
        .store_call(move |st| {
            let w = Writer::of(st, &caller, &who)?;
            let v = if artifact {
                st.set_report_artifact_metadata(
                    &ctx,
                    &qid,
                    &tid,
                    w.session(),
                    b.metadata,
                    b.if_version,
                )?
            } else {
                st.set_thread_metadata_as(&ctx, &qid, &tid, w.session(), b.metadata, b.if_version)?
            };
            let report = st
                .report(&qid, &tid, Visibility::All)?
                .ok_or(CoreError::NotFound)?;
            Ok(json!({"metadata_version": v, "report": report}))
        })
        .await?;
    Ok(Json(out))
}

/// `GET /api/queues/<qid>/reports/<tid>/clip`: the report's clip PNG,
/// sandboxed as a thread's is; 404 when it has none.
pub async fn clip(
    State(s): State<AppState>,
    a: Agent<QueueClip>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_clip_async(&qid, &tid, Visibility::All)
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(png(bytes, TRIAGE_CLIP_CACHE))
}

/// What a report route tells everyone with, made on the runtime before
/// the store's thread runs.
pub(crate) struct Announcer {
    app: AppState,
    fctx: clax_realtime::feedback::FeedbackCtx,
}

impl Announcer {
    pub(crate) fn of(s: &AppState) -> Announcer {
        Announcer {
            app: s.clone(),
            fctx: s.feedback_ctx(),
        }
    }
}

/// Tells everyone that report thread `t` was resolved (a resolve or a
/// verify), as a thread's resolve does: the working record on it ends
/// (`session`'s, or anyone's for a person's resolve), `thread_resolved`,
/// the feedback the resolve withdrew (`touched`), the `thread` event and
/// the questions on it.
///
/// # Errors
/// [`publish_thread`]'s.
pub(crate) fn announce_resolved(
    an: &Announcer,
    st: &Store,
    ctx: &AuditCtx,
    t: &clax_core::model::Thread,
    session: Option<&str>,
    touched: &clax_core::feedback::Touched,
) -> clax_core::Result<()> {
    let fctx = &an.fctx;
    let changed = match session {
        Some(sid) => fctx
            .working
            .thread_done(sid, &t.artifact_id, &t.id, StopReason::Resolved),
        None => fctx
            .working
            .thread_gone(&t.artifact_id, &t.id, StopReason::Resolved),
    };
    clax_realtime::working::settle(st, ctx, &fctx.events, &fctx.working, &changed);
    fctx.events.publish(Event::ThreadResolved {
        artifact_id: t.artifact_id.clone(),
        thread_id: t.id.clone(),
        resolved_by: t.resolved_by.clone().unwrap_or_default(),
        resolved_at: t.resolved_at.clone().unwrap_or_default(),
    });
    apply(fctx, st, touched);
    publish_thread(fctx, st, t)?;
    clax_http::views::question::announce_thread(&an.app, st, &t.id);
    Ok(())
}

/// Tells everyone that comment `cid` was added to report `tid` and the
/// report changed with it (a fix's summary, a reopen's note): the
/// `comment` event, the `thread` event and the questions on it.
///
/// # Errors
/// [`publish_thread`]'s.
pub(crate) fn announce_comment(
    an: &Announcer,
    st: &Store,
    tid: &str,
    cid: Option<&str>,
) -> clax_core::Result<()> {
    let fctx = &an.fctx;
    let Some(t) = st.get_thread(tid)? else {
        return Ok(());
    };
    if let Some(c) = cid.and_then(|cid| t.comments.iter().find(|c| c.id == cid)) {
        fctx.events.publish(Event::Comment {
            artifact_id: t.artifact_id.clone(),
            thread_id: tid.to_string(),
            comment: json!(c),
        });
    }
    publish_thread(fctx, st, &t)?;
    clax_http::views::question::announce_thread(&an.app, st, tid);
    Ok(())
}

/// The ID of report `r`'s newest comment.
pub(crate) fn last_comment(r: &clax_core::queue::Report) -> Option<String> {
    r.comments.last().map(|c| c.id.clone())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixBody {
    summary: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    verify_url: Option<String>,
    #[serde(default)]
    verify_after: Option<String>,
    #[serde(default)]
    fence: Option<u64>,
}

/// `POST /api/queues/<qid>/reports/<tid>/fix {summary, url?, commit?,
/// release?, verify_url?, verify_after?, fence?}` → `{report}`: a fix
/// proposed (spec 2026-10-29-feedback-loop-design §6.2). Its summary is
/// posted as the writer's comment, the report moves to `fix_proposed`,
/// and its lease ends; it is claimable again once reopened. An agent
/// proposes under its live lease (a write's errors), and with `fence`
/// only under the lease that fence names (409 `lease_lost` for an earlier
/// one); a person needs no claim, sends no fence (one sent is not read),
/// and a person's next fix supersedes the one waiting. 400 `invalid_fix`
/// for a blank summary, a summary over 4 KiB, a `url` or `verify_url`
/// that is not an http(s) URL, or a field too long; 409 `not_claimable`
/// for a `verified` or `resolved` report.
pub async fn fix(
    State(s): State<AppState>,
    a: Agent<QueueFix>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<FixBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    let b = body(req)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let an = Announcer::of(&s);
    let report = s
        .store_call(move |st| {
            let w = Writer::of(st, &caller, &who)?;
            let fix = NewFix {
                summary: b.summary,
                url: b.url,
                commit: b.commit,
                release: b.release,
                verify_url: b.verify_url,
                verify_after: b.verify_after,
            };
            let report = st.propose_fix_fenced(&ctx, &qid, &tid, &w.as_store(), fix, b.fence)?;
            if let Some(sid) = w.session() {
                // Its summary is its last word on the report until a verdict.
                let fctx = &an.fctx;
                let changed =
                    fctx.working
                        .thread_done(sid, &report.artifact.id, &tid, StopReason::Explicit);
                clax_realtime::working::settle(st, &ctx, &fctx.events, &fctx.working, &changed);
            }
            announce_comment(&an, st, &tid, last_comment(&report).as_deref())?;
            Ok(report)
        })
        .await?;
    Ok(Json(json!({"report": report})))
}

/// 404 `not_a_member` unless the caller of a verdict route acts as a
/// member of queue `qid`, and 403 `not_for_agents` for any agent the
/// handler's `refuse_agents` let through: a verdict is a person's (an
/// agent of another machine is refused 403 `forbidden` before).
async fn verdict_caller(s: &AppState, d: &DeniedToGrants, qid: &str) -> Result<(), ApiError> {
    let caller = d.caller(s).await?;
    if caller.is_agent() {
        return Err(clax_http::auth::not_for_agents());
    }
    member_as(s, &caller, qid).await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct VerifyBody {
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/queues/<qid>/reports/<tid>/verify {note?}` → `{report}`,
/// `verified`: a triage person says the fix waiting works, on the
/// reporter's behalf, recorded as theirs (`resolved_by`
/// `viewer:<public ID>`). A person's alone: 403 `not_for_agents` for an
/// agent. 409 `not_fix_proposed` when no fix waits for a verdict.
pub async fn verify(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<VerifyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    d.refuse_agents()?;
    let b = body(req)?;
    verdict_caller(&s, &d, &qid).await?;
    let an = Announcer::of(&s);
    let report = s
        .store_call(move |st| {
            let (name, public_id) = author(st, &who)?;
            let (report, touched) = st.verify_report_touched(
                &ctx,
                &qid,
                &tid,
                Verdict::Person { name, public_id },
                b.note,
                None,
            )?;
            let t = st.get_thread(&tid)?.ok_or(CoreError::NotFound)?;
            announce_resolved(&an, st, &ctx, &t, None, &touched)?;
            Ok(report)
        })
        .await?;
    Ok(Json(json!({"report": report})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReopenBody {
    body: String,
}

/// `POST /api/queues/<qid>/reports/<tid>/reopen {body}` → `{report}`,
/// `open` again: a triage person says a `fix_proposed`, `verified` or
/// `resolved` report is still broken, on the reporter's behalf, with
/// `body` as their comment (spec 2026-10-29-feedback-loop-design §6.3),
/// waking the queue's watching sessions and the session of the report's
/// last lease as the reporter's reopen does. A
/// person's alone: 403 `not_for_agents` for an agent. 409
/// `not_fix_proposed` for an `open` or `claimed` report.
pub async fn reopen(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<ReopenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    d.refuse_agents()?;
    let b = body(req)?;
    verdict_caller(&s, &d, &qid).await?;
    let t = tid.clone();
    let (report, woken) = s
        .store_call_async(move |st| async move {
            let (name, public_id) = author(&st, &who)?;
            st.reopen_report_async(
                &ctx,
                &qid,
                &t,
                Verdict::Person { name, public_id },
                b.body,
                None,
                None,
                None,
            )
            .await
        })
        .await?;
    super::queue_reporter::wake_watchers(&s, woken).await;
    let cid = last_comment(&report);
    let an = Announcer::of(&s);
    let r = s
        .store_call(move |st| announce_comment(&an, st, &tid, cid.as_deref()))
        .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, "announcing a reopened report failed");
    }
    Ok(Json(json!({"report": report})))
}

/// A diagnostics read's query, on the triage and reporter routes alike.
/// Public as [`ListQuery`] is: a public handler's signature names it.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CaptureQuery {
    #[serde(default)]
    pub(crate) capture: Option<u32>,
}

/// `GET /api/queues/<qid>/reports/<tid>/diagnostics?capture=` →
/// `{lines}`: the diagnostics of capture `capture` (0 the filing's, `n`
/// the `n`th reopen's), or without it the latest capture's that has any;
/// `[]` when that capture has none. 400 `invalid_query` for another query.
pub async fn diagnostics(
    State(s): State<AppState>,
    a: Agent<QueueDiagnostics>,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<CaptureQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    member(&s, &a, &qid).await?;
    let lines = s
        .store_call(move |st| st.report_diagnostics(&qid, &tid, Visibility::All, q.capture))
        .await?;
    Ok(Json(json!({"lines": lines.unwrap_or_default()})))
}

/// `GET /api/queues/<qid>/reports/<tid>/comments/<cid>/clip`: the PNG a
/// reopen's comment carries, sandboxed as the report's is; 404 when it
/// carries none.
pub async fn comment_clip(
    State(s): State<AppState>,
    a: Agent<QueueCommentClip>,
    p: Result<Path<(String, String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (qid, tid, cid) = path(p)?;
    member(&s, &a, &qid).await?;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_comment_clip_async(&qid, &tid, &cid, Visibility::All)
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(png(bytes, TRIAGE_CLIP_CACHE))
}

/// How long a member's browser keeps a report's clip.
const TRIAGE_CLIP_CACHE: &str = "private, max-age=3600";

/// A report clip's answer: `image/png`, sandboxed, never sniffed, kept as
/// `cache` (a `Cache-Control` value) says.
pub(crate) fn png(bytes: Vec<u8>, cache: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from(bytes),
    )
        .into_response()
}
