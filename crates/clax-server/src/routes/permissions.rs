//! An artifact's approved declaration and the consents under it: the
//! Permissions panel's one read, and the approval of a version's
//! declaration and the withdrawal of one approved name
//! ([`clax_store::capability_approvals`]).
//!
//! - `GET /api/artifacts/<id>/permissions`: the declaration, what of it is
//!   in force, the approval, what waits, and the `net`, `sample` and
//!   `db.read` grants.
//! - `PUT /api/artifacts/<id>/capability-approval` `{version,
//!   declaration_sha256}`: approves the declaration a `pending_capabilities`
//!   showed, the version and declaration it names.
//! - `DELETE /api/artifacts/<id>/capability-approval/<name>`: takes one
//!   gated name out of the approval.
//!
//! Each serves the people who answer for the page, never the page itself
//! or an agent: 403 `not_for_agents` for the owner's agents and `forbidden`
//! for a person's (`Clax-Grant`), 403 `not_for_pages` for a request with
//! the page marker (`x-clax-via: page`), 403 `admin_only` for a caller who
//! may not approve ([`may_approve`]: on a local daemon the owner alone, on
//! a hosted one the page's maker), and on the two writes 403
//! `forbidden_origin` for a request from another origin. 404 for an
//! unknown artifact. An approval or a withdrawal publishes
//! [`Event::Permissions`].

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use clax_core::audit::AuditCtx;
use clax_core::{ArtifactId, CoreError, Event, EventBus};
use clax_http::agent::{DeniedToGrants, admin_only, administers, page_marker};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::hosted::HostedRequest;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::capability_approvals::Approval;
use serde::Deserialize;
use serde_json::{Value, json};

/// Publishes [`Event::Permissions`] for `id` when its approval now is not
/// `before`, the one read before a write that set its declaration: the
/// write narrowed it ([`clax_store::capability_approvals`]), so open views
/// refetch what is in force. An approval made between the two reads
/// publishes one too, which costs a view a refetch.
///
/// # Errors
/// The store's.
pub(crate) fn tell_if_narrowed(
    st: &Store,
    events: &EventBus,
    id: &ArtifactId,
    before: Option<&Approval>,
) -> clax_core::Result<()> {
    if st.capability_approval(id)?.as_ref() != before {
        events.publish(Event::Permissions {
            artifact_id: id.as_str().to_string(),
        });
    }
    Ok(())
}

/// 403 `not_for_pages`: a page never approves or withdraws its own
/// capabilities, nor reads its permissions.
fn not_for_pages() -> ApiError {
    ApiError::forbidden(
        "not_for_pages",
        "a page's capabilities are approved by its owner in Clax, never by the page",
    )
}

/// Whether `c`, who administers the artifact or not (`administers`,
/// [`clax_http::agent::administers`]), may approve its declaration: never
/// an agent; on a local daemon the owner alone, since what approving
/// grants (`net` and `sample`) is spent from the owner's machine and key;
/// on a hosted daemon (`hosted`) who administers it, its maker.
pub(crate) fn may_approve(c: &Caller, administers: bool, hosted: bool) -> bool {
    !c.is_agent() && if hosted { administers } else { c.is_owner() }
}

/// The caller, once it may approve artifact `id`'s declaration
/// ([`may_approve`]), not as a page. Each route has refused the owner's
/// agents first (`refuse_agents`).
async fn approver(
    s: &AppState,
    d: &DeniedToGrants,
    headers: &HeaderMap,
    id: &ArtifactId,
) -> Result<Caller, ApiError> {
    let caller = d.caller(s).await?;
    if caller.is_agent() {
        return Err(clax_http::auth::not_for_agents());
    }
    if page_marker(&caller, headers)? {
        return Err(not_for_pages());
    }
    let (c, id, hosted) = (caller.clone(), id.clone(), s.hosted.is_some());
    let admin = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            administers(st, &c, id.as_str(), hosted)
        })
        .await?;
    if !may_approve(&caller, admin, hosted) {
        return Err(admin_only());
    }
    Ok(caller)
}

/// Who an approval records: `person:<ID>` for a person, `owner` for the
/// owner.
fn by(c: &Caller) -> String {
    match c {
        Caller::Person { viewer } => format!("person:{}", viewer.public_id),
        _ => "owner".to_string(),
    }
}

fn aid_of(p: Result<Path<String>, PathRejection>) -> Result<ArtifactId, ApiError> {
    parse_id(&p.map(|Path(a)| a).map_err(|_| ApiError::not_found())?)
}

/// `GET /api/artifacts/<id>/permissions`: `{version, declared, in_force,
/// approved, pending, net: {declared, waiting, grants, granted}, sample:
/// {granted, granted_at}, db_read: [{source_id, title, granted_at}]}`.
/// `version` is the current version and `declared` its declaration;
/// `in_force` what of it is in force; `approved` the approval on record
/// (`null` for none) and `pending` what waits for one (`null` for
/// nothing), as `pending_capabilities`; `net` as `GET
/// /api/artifacts/<id>/net-grants` with `waiting` (the hosts that wait for
/// approval); `sample` the owner's sample grant; `db_read` the grants to
/// read other artifacts' documents, `title` the source's (`null` once it is
/// gone), on a hosted daemon only those of sources the caller lists.
pub async fn permissions(
    State(s): State<AppState>,
    d: DeniedToGrants,
    headers: HeaderMap,
    hosted: Option<Extension<HostedRequest>>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = aid_of(aid)?;
    approver(&s, &d, &headers, &id).await?;
    let scope = s.hosted.as_ref().map(|_| hosted.and_then(|h| h.0.member));
    let v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let net = st.net_access(&id)?;
            let sample = st.sample_grant(&id)?;
            let mut reads = st.grants_of(&id)?;
            if let Some(scope) = &scope {
                reads.retain(|g| scope.as_ref().is_some_and(|m| m.lists(st, &g.source_id)));
            }
            let db_read = reads
                .into_iter()
                .map(|g| {
                    let title = ArtifactId::parse(&g.source_id)
                        .ok()
                        .map(|src| st.get_artifact(&src))
                        .transpose()?
                        .flatten()
                        .map(|src| src.title);
                    Ok(json!({"source_id": g.source_id, "title": title, "granted_at": g.granted_at}))
                })
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({
                "version": a.current_version,
                "declared": a.capabilities,
                "in_force": st.capabilities_in_force(&id)?,
                "approved": st.capability_approval(&id)?,
                "pending": st.pending_capabilities(&id)?,
                "net": {
                    "declared": net.declared,
                    "waiting": net.waiting,
                    "grants": net.grants,
                    "granted": net.all_granted(),
                },
                "sample": {"granted": sample.is_some(), "granted_at": sample},
                "db_read": db_read,
            }))
        })
        .await?;
    Ok(Json(v))
}

/// What an approval names: the version and declaration a
/// `pending_capabilities` showed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveBody {
    version: Option<u32>,
    declaration_sha256: Option<String>,
}

/// `PUT /api/artifacts/<id>/capability-approval` `{version,
/// declaration_sha256}` (both required: 400 `invalid_args`), the
/// `pending_capabilities` the caller was shown: approves the current
/// version's declaration ([`clax_store::Store::approve_capabilities`]);
/// `{approval, in_force}`. The owner's approval also grants the `net` hosts
/// and the `sample` that waited; a hosted member's grants nothing, which
/// leaves those to the owner's answer on the page's first use (the grants
/// stand for the owner's consent, as their own routes, which take the
/// daemon token, do). 409 `stale_version` with `current` when a version was
/// published, or the declaration changed, since what was shown; 403
/// `draft` for an app's draft, held to its app's approval.
pub async fn approve(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    headers: HeaderMap,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    body: Result<Json<ApproveBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = aid_of(aid)?;
    let caller = approver(&s, &d, &headers, &id).await?;
    let Json(b) = body.map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?;
    let (Some(version), Some(sha)) = (b.version, b.declaration_sha256) else {
        return Err(ApiError::bad_request(
            "invalid_args",
            "version and declaration_sha256 are required: send the pending_capabilities shown",
        ));
    };
    let (by, grant) = (by(&caller), caller.is_owner());
    let events = s.events.clone();
    let v = s
        .store_call(move |st| {
            let approval = st.approve_capabilities(&audit, &id, version, &sha, &by, grant)?;
            events.publish(Event::Permissions {
                artifact_id: id.as_str().to_string(),
            });
            Ok(json!({"approval": approval, "in_force": st.capabilities_in_force(&id)?}))
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/artifacts/<id>/capability-approval/<name>`: takes gated
/// capability `name` out of the approval, so the current version's
/// declaration of it waits for approval again; `{withdrawn, in_force}`,
/// `withdrawn` false when it was not approved. Grants made on approval stay
/// (revoke them on their own routes). 400 `invalid_args` for a name that
/// needs no approval; 403 `draft` for an app's draft.
pub async fn withdraw(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    headers: HeaderMap,
    audit: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (aid, name) = p.map(|Path(p)| p).map_err(|_| ApiError::not_found())?;
    let id = parse_id(&aid)?;
    approver(&s, &d, &headers, &id).await?;
    let events = s.events.clone();
    let v = s
        .store_call(move |st| {
            let withdrawn = st.withdraw_capability(&audit, &id, &name)?;
            if withdrawn {
                events.publish(Event::Permissions {
                    artifact_id: id.as_str().to_string(),
                });
            }
            Ok(json!({"withdrawn": withdrawn, "in_force": st.capabilities_in_force(&id)?}))
        })
        .await?;
    Ok(Json(v))
}
