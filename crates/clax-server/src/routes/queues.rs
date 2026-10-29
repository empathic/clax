//! Report queues' administration (spec 2026-10-27-clax-sdk-design §7.1):
//! a queue's maker and its admins, in a browser session, make and change
//! queues, their members and their service grants.
//!
//! - `GET /api/queues` → `{queues}`: the caller's queues with their role
//!   (also an agent's, for `queue_list`).
//! - `POST /api/queues {name, origins, reporters_see}` → `{queue}`;
//!   `PATCH /api/queues/<qid>`, `DELETE /api/queues/<qid>`.
//! - `GET /api/queues/<qid>/members` → `{members}`; `PUT
//!   /api/queues/<qid>/members/<public_id> {role}`, `DELETE` the same.
//! - `GET /api/queues/<qid>/service-grants` → `{grants}`; `POST
//!   /api/queues/<qid>/service-grants {name}` → `{grant, token,
//!   private_key}`, shown once; `DELETE
//!   /api/queues/<qid>/service-grants/<gid>`.
//!
//! Every route but the listing is a person's in their browser (or the
//! owner by hand), from the daemon's own origin (`SameOrigin`); agents are
//! refused. The owner acts as the person `None`, a connected person as
//! their public ID. A queue the caller is not a member of answers 404
//! `not_a_member`, as an unknown one does; a triage member's change 403
//! `not_queue_admin`.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use base64::Engine as _;
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_core::queue::{ReportersSee, Role};
use clax_http::agent::{Agent, DeniedToGrants, QueuesList};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::queues::{NewQueue, QueuePatch};
use serde::Deserialize;
use serde_json::{Value, json};

/// The person an administering caller acts as: `None` for the owner (their
/// browser or by hand), a connected person's public ID.
///
/// # Errors
/// 401 without a credential; 403 `not_for_agents` for an agent (each
/// handler has refused the owner's first, `refuse_agents`); 403
/// `not_a_person` for a viewer who is no connected person.
async fn admin_person(s: &AppState, d: &DeniedToGrants) -> Result<Option<String>, ApiError> {
    match d.caller(s).await? {
        Caller::Owner { .. } => Ok(None),
        Caller::Person { viewer } => Ok(Some(viewer.public_id)),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            Err(clax_http::auth::not_for_agents())
        }
        Caller::Viewer { .. } => Err(ApiError::forbidden(
            "not_a_person",
            "only the owner and connected people administer queues",
        )),
    }
}

/// 403 `not_queue_admin` for a triage member.
fn not_queue_admin() -> ApiError {
    ApiError::forbidden(
        "not_queue_admin",
        "only a queue's admins change it, its members and its service grants",
    )
}

/// `person`'s role on queue `qid`, when it is `admin`.
///
/// # Errors
/// 404 `not_a_member` for a non-member or no such queue; 403
/// `not_queue_admin` for a triage member.
fn require_admin(st: &clax_store::Store, qid: &str, person: Option<&str>) -> Result<(), ApiError> {
    match st.queue_role(qid, person)? {
        Some(Role::Admin) => Ok(()),
        Some(Role::Triage) => Err(not_queue_admin()),
        None => Err(CoreError::NotAMember.into()),
    }
}

/// [`require_admin`] on the store's thread.
async fn admin_of(s: &AppState, qid: &str, person: Option<String>) -> Result<(), ApiError> {
    let qid = qid.to_string();
    s.store_call(move |st| Ok(require_admin(st, &qid, person.as_deref())))
        .await?
}

/// `GET /api/queues` → `{queues}`: the live queues the caller made or is a
/// member of, oldest first, each `{id, name, origins, reporters_see,
/// created_at, role}`. The owner and the owner's agents read the owner's;
/// a connected person and their agents read theirs; a viewer who is no
/// connected person reads none. 401 without a credential.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<QueuesList>,
) -> Result<Json<Value>, ApiError> {
    let person = match &a.caller {
        Caller::Owner { .. }
        | Caller::OwnerAgent { .. }
        | Caller::PersonAgent { person: None, .. } => None,
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        Caller::PersonAgent {
            person: Some(p), ..
        } => Some(p.clone()),
        Caller::Viewer { .. } => return Ok(Json(json!({"queues": []}))),
        Caller::Anonymous => return Err(ApiError::unauthorized()),
    };
    let queues = s
        .store_call(move |st| st.queues_of(person.as_deref()))
        .await?;
    Ok(Json(json!({ "queues": queues })))
}

/// `POST /api/queues` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    name: String,
    #[serde(default)]
    origins: Vec<String>,
    #[serde(default = "own")]
    reporters_see: ReportersSee,
}

fn own() -> ReportersSee {
    ReportersSee::Own
}

/// `POST /api/queues {name, origins?, reporters_see?}` → 201 `{queue}`:
/// makes a queue whose admin is the caller (`reporters_see` `own` when not
/// given). 400 `invalid_name` (blank or over 120 characters) or
/// `invalid_origin` (not `scheme://host[:port]` of `http` or `https`, or
/// over 32 of them).
pub async fn create(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let b = body(req)?;
    let queue = s
        .store_call(move |st| {
            st.create_queue(
                &ctx,
                person.as_deref(),
                NewQueue {
                    name: b.name,
                    origins: b.origins,
                    reporters_see: b.reporters_see,
                },
            )
        })
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "queue": queue }))))
}

/// `PATCH /api/queues/<qid>` body: each field given is set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    name: Option<String>,
    origins: Option<Vec<String>>,
    reporters_see: Option<ReportersSee>,
}

/// `PATCH /api/queues/<qid> {name?, origins?, reporters_see?}` →
/// `{queue}`: an admin's change, the errors of `POST /api/queues` for bad
/// fields.
pub async fn patch(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    let queue = s
        .store_call(move |st| {
            if let Err(e) = require_admin(st, &qid, person.as_deref()) {
                return Ok(Err(e));
            }
            let row = st.update_queue(
                &ctx,
                &qid,
                QueuePatch {
                    name: b.name,
                    origins: b.origins,
                    reporters_see: b.reporters_see,
                },
            )?;
            Ok(Ok(row.view(Role::Admin)))
        })
        .await??;
    Ok(Json(json!({ "queue": queue })))
}

/// `DELETE /api/queues/<qid>` → 204: an admin deletes the queue; its
/// service grants are revoked and its reporter tokens end, and its reports
/// stay on their artifacts.
pub async fn delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| st.delete_queue(&ctx, &qid)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/queues/<qid>/members` → `{members: [{public_id, name,
/// role}]}`: an admin reads the people added to the queue, in the order
/// they were added (`name` the person's display name, or `null`). The
/// queue's maker is its admin and is not listed.
pub async fn members(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, person).await?;
    let members = s
        .store_call(move |st| {
            let rows = st.queue_members(&qid)?;
            let ids: Vec<String> = rows.iter().map(|(p, _)| p.clone()).collect();
            let viewers = st.viewers_by_public_ids(&ids)?;
            Ok(rows
                .into_iter()
                .map(|(p, role)| {
                    let name = viewers
                        .iter()
                        .find(|v| v.public_id == p)
                        .and_then(|v| v.display_name.clone());
                    json!({"public_id": p, "name": name, "role": role})
                })
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(Json(json!({ "members": members })))
}

/// `PUT /api/queues/<qid>/members/<public_id>` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberBody {
    role: Role,
}

/// `PUT /api/queues/<qid>/members/<public_id> {role}` → 204: an admin adds
/// a connected person at `role` (`admin` or `triage`), or changes their
/// role. 404 `not_found` for a public ID that is no connected, unrevoked
/// person.
pub async fn put_member(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<MemberBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let (qid, member) = path(p)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| {
        match st.viewer_by_public_id(&member)? {
            Some(v) if v.person && v.person_revoked_at.is_none() => {}
            _ => return Err(CoreError::NotFound),
        }
        st.set_member(&ctx, &qid, &member, Some(b.role))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/queues/<qid>/members/<public_id>` → 204: an admin removes
/// a member (a public ID that is none changes nothing).
pub async fn delete_member(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let (qid, member) = path(p)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| st.set_member(&ctx, &qid, &member, None))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/queues/<qid>/service-grants` → `{grants: [{id, name,
/// created_at}]}`: an admin reads the queue's unrevoked service grants,
/// oldest first; never their tokens or keys.
pub async fn grants(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, person).await?;
    let grants = s.store_call(move |st| st.service_grants(&qid)).await?;
    Ok(Json(json!({ "grants": grants })))
}

/// `POST /api/queues/<qid>/service-grants` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantBody {
    name: String,
}

/// `POST /api/queues/<qid>/service-grants {name}` → 201 `{grant: {id,
/// name, created_at}, token, private_key}`: an admin makes a service grant.
/// `token` (`cxq_…`) and `private_key` (the Ed25519 key's PKCS#8 v2
/// document, unpadded base64url) are in this answer alone: Clax keeps the
/// token's SHA-256 and the public key. 400 `invalid_name` for a blank name
/// or one over 120 characters.
pub async fn create_grant(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<GrantBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    let (grant, token, key) = s
        .store_call(move |st| st.create_service_grant(&ctx, &qid, &b.name))
        .await?;
    let private_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.pkcs8().expose());
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "grant": grant,
            "token": token.expose(),
            "private_key": private_key,
        })),
    ))
}

/// `DELETE /api/queues/<qid>/service-grants/<gid>` → 204: an admin revokes
/// a service grant, which ends every reporter token it minted. 404
/// `not_found` for no such unrevoked grant of the queue.
pub async fn revoke_grant(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let (qid, gid) = path(p)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| st.revoke_service_grant(&ctx, &qid, &gid))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
