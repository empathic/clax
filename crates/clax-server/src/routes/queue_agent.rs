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
//! - `GET …/reports/<tid>/clip`: the PNG.
//!
//! The caller acts as a person: the owner (the owner's browser or token,
//! and the owner's agents), or a connected person (their browser, and
//! their agents). Anyone who is not a member of the queue gets 404
//! `not_a_member`, as for a queue that does not exist.
//!
//! An agent claims, renews, releases, replies, resolves and writes
//! metadata from its session (`X-Clax-Session`; 400 `no_session` without
//! one), and writes to a report only under its own live lease: 409
//! `claim_required` when its session never claimed the report or released
//! it, 409 `lease_lost` when its lease expired or a later one superseded
//! it, 409 `thread_taken` when another session holds it. A person (not an
//! agent) replies, resolves and writes metadata without a claim.

use super::artifacts::{body, path, publishing_session};
use super::threads::publish_thread;
use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::model::Session;
use clax_core::queue::State as ReportState;
use clax_core::working::StopReason;
use clax_core::{CoreError, Event};
use clax_http::agent::{
    Agent, Need, QueueArtifactPatch, QueueClaim, QueueClip, QueueRelease, QueueRenew, QueueReply,
    QueueReportGet, QueueReportPatch, QueueReportsList, QueueResolve,
};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::{SameOrigin, author};
use clax_realtime::feedback::apply;
use clax_store::Store;
use clax_store::queues::{ReportWriter, Visibility};
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
    let Some(person) = queue_person(&a.caller) else {
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
}

/// `GET /api/queues/<qid>/reports?state=open|claimed|resolved&cursor=` →
/// `{reports, next}`: in order of arrival, a page at a time; 400
/// `invalid_state` for another state.
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
        // Only the states the store records are served.
        Some(x) => Some(
            ReportState::parse(x)
                .filter(|st| {
                    matches!(
                        st,
                        ReportState::Open | ReportState::Claimed | ReportState::Resolved
                    )
                })
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_state",
                        format!("state is open, claimed or resolved, not '{x}'"),
                    )
                })?,
        ),
    };
    member(&s, &a, &qid).await?;
    let (reports, next) = s
        .store_call(move |st| {
            st.reports(
                &qid,
                Visibility::All,
                state.map(clax_core::queue::StateFilter::One),
                q.cursor.as_deref(),
            )
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

/// `POST /api/queues/<qid>/reports/<tid>/resolve` → `{report}`: an
/// agent's resolve under its live lease, or a person's.
pub async fn resolve(
    State(s): State<AppState>,
    a: Agent<QueueResolve>,
    _o: SameOrigin,
    who: Identity,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (qid, tid) = path(p)?;
    member(&s, &a, &qid).await?;
    let caller = a.caller.clone();
    let fctx = s.feedback_ctx();
    let app = s.clone();
    let report = s
        .store_call(move |st| {
            let w = Writer::of(st, &caller, &who)?;
            // The route names no reason yet; a resolve without one is `other`.
            let (t, touched, report) = st.resolve_report(
                &ctx,
                &qid,
                &tid,
                &w.as_store(),
                clax_core::report::ResolveReason::Other,
            )?;
            let changed = match w.session() {
                Some(sid) => {
                    fctx.working
                        .thread_done(sid, &t.artifact_id, &tid, StopReason::Resolved)
                }
                None => fctx
                    .working
                    .thread_gone(&t.artifact_id, &tid, StopReason::Resolved),
            };
            clax_realtime::working::settle(st, &ctx, &fctx.events, &fctx.working, &changed);
            fctx.events.publish(Event::ThreadResolved {
                artifact_id: t.artifact_id.clone(),
                thread_id: tid.clone(),
                resolved_by: t.resolved_by.clone().unwrap_or_default(),
                resolved_at: t.resolved_at.clone().unwrap_or_default(),
            });
            apply(&fctx, st, &touched);
            publish_thread(&fctx, st, &t)?;
            clax_http::views::question::announce_thread(&app, st, &tid);
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
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        Body::from(bytes),
    )
        .into_response())
}
