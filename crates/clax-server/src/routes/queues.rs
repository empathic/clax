//! Report queues' administration (spec 2026-10-27-clax-sdk-design §7.1):
//! a queue's maker and its admins, in a browser session, make and change
//! queues, their members and their service grants.
//!
//! - `GET /api/queues` → `{queues}`: the caller's queues with their role
//!   (also an agent's, for `queue_list`).
//! - `POST /api/queues {name, origins, reporters_see}` → `{queue}`;
//!   `PATCH /api/queues/<qid>`, `DELETE /api/queues/<qid>`.
//! - `GET /api/queues/<qid>/members` → `{members}`, its maker first; `PUT
//!   /api/queues/<qid>/members/<public_id> {role}`, `DELETE` the same.
//! - `GET /api/queues/<qid>/people?q=` → `{people}`: the people an admin
//!   may add, found by name or public ID.
//! - `GET /api/queues/<qid>/service-grants` → `{grants}`; `POST
//!   /api/queues/<qid>/service-grants {name, origins?}` → `{grant, token,
//!   private_key}`, shown once; `DELETE
//!   /api/queues/<qid>/service-grants/<gid>`.
//! - `GET /api/admin/queues` → `{members_create, source}`; `PUT
//!   /api/admin/queues {members_create}`: whether members who administer
//!   nothing make queues (the owner and administrators always may).
//!
//! Every route but the listing is a person's in their browser (or the
//! owner by hand), from the daemon's own origin (`SameOrigin`); agents are
//! refused. The owner acts as the person `None`, a connected person as
//! their public ID. The owner and the instance's administrators
//! ([`clax_http::bot::may_admin`]) oversee every queue, in their browser:
//! each is theirs to read and administer as an admin's, member or not
//! (their agents work only the queues they are members of). Any other
//! caller's queue that they are not a member of answers 404
//! `not_a_member`, as an unknown one does; a triage member's change 403
//! `not_queue_admin`.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::QueryRejection;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use base64::Engine as _;
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_core::queue::{ReportersSee, Role};
use clax_http::agent::{Agent, DeniedToGrants, QueuesList};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::queues::{NewQueue, OWNER_MEMBER, QueuePatch};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most people one search of `GET /api/queues/<qid>/people` answers.
pub const MAX_PEOPLE: usize = 20;

/// An administering caller: the person they act as (`None` for the owner,
/// a connected person's public ID), and whether they oversee every queue
/// ([`oversees`]).
#[derive(Clone)]
struct Admin {
    person: Option<String>,
    oversees: bool,
}

/// Whether `c` oversees every queue: the owner or an administrator of the
/// instance ([`clax_http::bot::may_admin`]), in their browser (or the
/// owner by hand), never an agent.
pub(crate) fn oversees(st: &Store, c: &Caller) -> bool {
    matches!(c, Caller::Owner { .. } | Caller::Person { .. })
        && clax_http::bot::may_admin(st, c).is_ok()
}

/// The administering caller of `d`.
///
/// # Errors
/// 401 without a credential; 403 `not_for_agents` for an agent (each
/// handler has refused the owner's first, `refuse_agents`); 403
/// `not_a_person` for a viewer who is no connected person.
async fn admin_person(s: &AppState, d: &DeniedToGrants) -> Result<Admin, ApiError> {
    let caller = d.caller(s).await?;
    let person = match &caller {
        Caller::Owner { .. } => None,
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        Caller::Anonymous => return Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            return Err(clax_http::auth::not_for_agents());
        }
        Caller::Viewer { .. } => {
            return Err(ApiError::forbidden(
                "not_a_person",
                "only the owner and connected people administer queues",
            ));
        }
    };
    let oversees = s.store_call(move |st| Ok(oversees(st, &caller))).await?;
    Ok(Admin { person, oversees })
}

/// 403 `not_queue_admin` for a triage member.
fn not_queue_admin() -> ApiError {
    ApiError::forbidden(
        "not_queue_admin",
        "only a queue's admins change it, its members and its service grants",
    )
}

/// `a`'s role on live queue `qid`: their member role, else admin for a
/// caller who oversees every queue; `None` for anyone else and no such
/// queue.
///
/// # Errors
/// The store's.
fn role_on(st: &Store, qid: &str, a: &Admin) -> clax_core::Result<Option<Role>> {
    if let Some(r) = st.queue_role(qid, a.person.as_deref())? {
        return Ok(Some(r));
    }
    Ok((a.oversees && st.queue(qid)?.is_some()).then_some(Role::Admin))
}

/// `Ok` when `a` is queue `qid`'s admin ([`role_on`]).
///
/// # Errors
/// 404 `not_a_member` for a non-member or no such queue; 403
/// `not_queue_admin` for a triage member.
fn require_admin(st: &Store, qid: &str, a: &Admin) -> Result<(), ApiError> {
    match role_on(st, qid, a)? {
        Some(Role::Admin) => Ok(()),
        Some(Role::Triage) => Err(not_queue_admin()),
        None => Err(CoreError::NotAMember.into()),
    }
}

/// [`require_admin`] on the store's thread.
async fn admin_of(s: &AppState, qid: &str, a: Admin) -> Result<(), ApiError> {
    let qid = qid.to_string();
    s.store_call(move |st| Ok(require_admin(st, &qid, &a)))
        .await?
}

/// Whether a member who administers nothing may make a queue: the
/// `queues.members_create` setting, which with no value set is on for a
/// local Clax and off for a hosted one.
///
/// # Errors
/// The store's.
pub(crate) fn members_create(st: &Store, hosted: bool) -> clax_core::Result<bool> {
    Ok(st
        .bot_setting(Setting::QueuesMembersCreate)?
        .as_bool()
        .unwrap_or(!hosted))
}

/// `Ok` when `c` may make a queue: the owner, an administrator of the
/// instance, or a connected person while members make queues
/// ([`members_create`]). Approving an agent's queue request makes one, so
/// it asks this too ([`super::queue_requests::approve`]).
///
/// # Errors
/// The store's; within, 403 `queues_admin_only`.
pub(crate) fn may_make(
    st: &Store,
    c: &Caller,
    hosted: bool,
) -> clax_core::Result<Result<(), ApiError>> {
    if matches!(c, Caller::Person { .. }) && !oversees(st, c) && !members_create(st, hosted)? {
        return Ok(Err(ApiError::forbidden(
            "queues_admin_only",
            "an administrator of this Clax makes its report queues",
        )));
    }
    Ok(Ok(()))
}

/// What `c` may do with queues, for the shell's links (`GET
/// /api/viewers/me`'s `queues`): `{member, may_make}`, the number of live
/// queues `c` made or is a member of, and whether they may make one. A
/// caller who is neither the owner nor a connected person, and an agent,
/// is a member of none and makes none.
///
/// # Errors
/// The store's.
pub(crate) fn access(st: &Store, c: &Caller, hosted: bool) -> clax_core::Result<Value> {
    let (person, may_make) = match c {
        Caller::Owner { .. } => (None, true),
        Caller::Person { viewer } => (
            Some(viewer.public_id.clone()),
            oversees(st, c) || members_create(st, hosted)?,
        ),
        _ => return Ok(json!({"member": 0, "may_make": false})),
    };
    let member = st.queues_of(person.as_deref())?.len();
    Ok(json!({"member": member, "may_make": may_make}))
}

/// `GET /api/queues` → `{queues}`: the live queues the caller made or is a
/// member of, oldest first, each `{id, name, origins, reporters_see,
/// created_at, role, member}`. The owner and the owner's agents read the
/// owner's; a connected person and their agents read theirs; a viewer who
/// is no connected person reads none. A caller who oversees every queue
/// ([`oversees`]) reads every live queue, at their member role where they
/// are one and else as admin with `member` false. 401 without a
/// credential.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<QueuesList>,
) -> Result<Json<Value>, ApiError> {
    if matches!(a.caller, Caller::Owner { .. } | Caller::Person { .. }) {
        let c = a.caller.clone();
        let queues = s
            .store_call(move |st| {
                let person = match &c {
                    Caller::Person { viewer } => Some(viewer.public_id.clone()),
                    _ => None,
                };
                let rows = if oversees(st, &c) {
                    st.every_queue(person.as_deref())?
                } else {
                    st.queues_of(person.as_deref())?
                        .into_iter()
                        .map(|q| (q, true))
                        .collect()
                };
                Ok(rows
                    .into_iter()
                    .map(|(q, member)| {
                        let mut v = json!(q);
                        v["member"] = json!(member);
                        v
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        return Ok(Json(json!({ "queues": queues })));
    }
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
/// given), listed first among its members. A connected person who
/// administers nothing makes one only while `queues.members_create` is on
/// ([`members_create`]): else 403 `queues_admin_only`. Each of `origins` is an exact origin or an origin pattern
/// (`https://app-pr-*.example.com`, [`clax_core::origin::OriginPattern::parse`]).
/// 400 `invalid_name` (blank or over 120 characters) or `invalid_origin`
/// (neither `scheme://host[:port]` of `http` or `https` nor such a pattern,
/// or over 32 of them).
pub async fn create(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let b = body(req)?;
    let hosted = s.hosted.is_some();
    let person = a.person.clone();
    let queue = s
        .store_call(move |st| {
            if a.person.is_some() && !a.oversees && !members_create(st, hosted)? {
                return Ok(Err(ApiError::forbidden(
                    "queues_admin_only",
                    "an administrator of this Clax makes its report queues",
                )));
            }
            st.create_queue(
                &ctx,
                person.as_deref(),
                NewQueue {
                    name: b.name,
                    origins: b.origins,
                    reporters_see: b.reporters_see,
                },
            )
            .map(Ok)
        })
        .await??;
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
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    let queue = s
        .store_call(move |st| {
            if let Err(e) = require_admin(st, &qid, &a) {
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
            let role = st
                .queue_role(&qid, a.person.as_deref())?
                .unwrap_or(Role::Admin);
            Ok(Ok(row.view(role)))
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
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, a).await?;
    s.store_call(move |st| st.delete_queue(&ctx, &qid)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/queues/<qid>/members` → `{members: [{public_id, name, role,
/// maker, owner}]}`: an admin reads the queue's members, its maker first
/// (`maker` true, always `admin`), then in the order they were added.
/// `name` is the person's display name, or `null`; `owner` marks the
/// owner, whose `public_id` is their viewer's (`*owner` before they have
/// one).
pub async fn members(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, a).await?;
    let members = s
        .store_call(move |st| {
            let maker = st
                .queue(&qid)?
                .ok_or(CoreError::NotAMember)?
                .person
                .unwrap_or_else(|| OWNER_MEMBER.to_string());
            let rows = st.queue_members(&qid)?;
            let owner = st.owner()?;
            let ids: Vec<String> = rows.iter().map(|(p, _)| p.clone()).collect();
            let viewers = st.viewers_by_public_ids(&ids)?;
            Ok(rows
                .into_iter()
                .map(|(p, role)| {
                    let is_maker = p == maker;
                    if p == OWNER_MEMBER {
                        let id = owner
                            .as_ref()
                            .map_or(OWNER_MEMBER, |o| o.public_id.as_str());
                        let name = owner.as_ref().and_then(|o| o.display_name.clone());
                        return json!({"public_id": id, "name": name, "role": role,
                                      "maker": is_maker, "owner": true});
                    }
                    let name = viewers
                        .iter()
                        .find(|v| v.public_id == p)
                        .and_then(|v| v.display_name.clone());
                    json!({"public_id": p, "name": name, "role": role,
                           "maker": is_maker, "owner": false})
                })
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(Json(json!({ "members": members })))
}

/// The member key ([`clax_store::queues::member_key`]) of `public_id`: the
/// owner's for `*owner` or the owner's viewer's public ID, else the
/// public ID itself.
fn member_of(st: &Store, public_id: &str) -> clax_core::Result<String> {
    if public_id == OWNER_MEMBER || st.owner()?.is_some_and(|o| o.public_id == public_id) {
        return Ok(OWNER_MEMBER.to_string());
    }
    Ok(public_id.to_string())
}

/// `GET /api/queues/<qid>/people` query.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeopleQuery {
    #[serde(default)]
    q: String,
}

/// `GET /api/queues/<qid>/people?q=<text>` → `{people: [{public_id, name,
/// avatar_url, owner}]}`: an admin finds the people they may add, at most
/// [`MAX_PEOPLE`]: the owner and the connected, unrevoked people (on a
/// hosted Clax its members) whose name contains `q`, ignoring case, or
/// whose public ID is `q`, the owner first ([`Store::search_people`]). It
/// shows nothing more of a person: no rights, machines or revocations
/// (`GET /api/people` stays the owner's). A blank `q` finds no one; 400
/// `invalid_argument` for one over 100 characters.
pub async fn people(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
    q: Result<Query<PeopleQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    if q.q.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "invalid_argument",
            "q is at most 100 characters",
        ));
    }
    admin_of(&s, &qid, a).await?;
    let people = s
        .store_call(move |st| st.search_people(&q.q, MAX_PEOPLE))
        .await?;
    Ok(Json(json!({ "people": people })))
}

/// `PUT /api/queues/<qid>/members/<public_id>` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberBody {
    role: Role,
}

/// `PUT /api/queues/<qid>/members/<public_id> {role}` → 204: an admin adds
/// a connected person, or the owner (`*owner` or the owner's public ID),
/// at `role` (`admin` or `triage`), or changes their role. 404 `not_found`
/// for a public ID that is neither the owner's nor a connected, unrevoked
/// person's; 400 `queue_maker` for the queue's maker, who stays its admin.
pub async fn put_member(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<MemberBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let (qid, member) = path(p)?;
    let b = body(req)?;
    admin_of(&s, &qid, a).await?;
    s.store_call(move |st| {
        let key = member_of(st, &member)?;
        if key != OWNER_MEMBER {
            match st.viewer_by_public_id(&member)? {
                Some(v) if v.person && v.person_revoked_at.is_none() => {}
                _ => return Err(CoreError::NotFound),
            }
        }
        st.set_member(&ctx, &qid, &key, Some(b.role))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/queues/<qid>/members/<public_id>` → 204: an admin removes
/// a member, the owner by `*owner` or their public ID (a public ID that is
/// none changes nothing). 400 `queue_maker` for the queue's maker.
pub async fn delete_member(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let (qid, member) = path(p)?;
    admin_of(&s, &qid, a).await?;
    s.store_call(move |st| {
        let key = member_of(st, &member)?;
        st.set_member(&ctx, &qid, &key, None)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/queues/<qid>/service-grants` → `{grants: [{id, name,
/// origins, created_at}]}`: an admin reads the queue's unrevoked service grants,
/// oldest first; never their tokens or keys.
pub async fn grants(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, a).await?;
    let grants = s.store_call(move |st| st.service_grants(&qid)).await?;
    Ok(Json(json!({ "grants": grants })))
}

/// `POST /api/queues/<qid>/service-grants` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantBody {
    name: String,
    #[serde(default)]
    origins: Option<Vec<String>>,
}

/// `POST /api/queues/<qid>/service-grants {name, origins?}` → 201
/// `{grant: {id, name, origins, created_at}, token, private_key}`: an
/// admin makes a service grant. With `origins` (some of the queue's
/// origins and patterns, as it lists them) the grant mints reporter tokens
/// only for an origin one of them allows: a grant limited to the queue's
/// preview pattern is what preview deployments hold, so none holds one
/// that mints for the production site. `origins` is `null` on a grant of
/// every origin the queue allows.
/// `token` (`cxq_…`) and `private_key` (the Ed25519 key's PKCS#8 v2
/// document, unpadded base64url) are in this answer alone: Clax keeps the
/// token's SHA-256 and the public key. 400 `invalid_name` for a blank name
/// or one over 120 characters; 400 `invalid_origin` for an empty
/// `origins`, or one the queue does not list.
pub async fn create_grant(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<GrantBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let a = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, a).await?;
    let (grant, token, key) = s
        .store_call(move |st| {
            st.create_service_grant_for(&ctx, &qid, &b.name, b.origins.as_deref())
        })
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
    let a = admin_person(&s, &d).await?;
    let (qid, gid) = path(p)?;
    admin_of(&s, &qid, a).await?;
    s.store_call(move |st| st.revoke_service_grant(&ctx, &qid, &gid))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/admin/queues` and `PUT /api/admin/queues`'s answer: whether a
/// member who administers nothing makes queues, and where that comes from
/// (`default`: on for a local Clax, off for a hosted one; `db`: an
/// administrator set it).
fn policy_view(st: &Store, hosted: bool) -> clax_core::Result<Value> {
    let source = st
        .effective_bot_settings()?
        .get(&Setting::QueuesMembersCreate)
        .map_or_else(|| "default".to_string(), |e| e.source.label());
    Ok(json!({
        "members_create": members_create(st, hosted)?,
        "source": source,
    }))
}

/// `Ok` when `d`'s caller is the owner or an administrator of the instance.
///
/// # Errors
/// As [`clax_http::bot::may_admin`]: 401, 403 `forbidden`.
async fn instance_admin(s: &AppState, d: &DeniedToGrants) -> Result<(), ApiError> {
    let caller = d.caller(s).await?;
    s.store_call(move |st| Ok(clax_http::bot::may_admin(st, &caller).map(|_| ())))
        .await?
}

/// `GET /api/admin/queues` → `{members_create, source}` ([`policy_view`]):
/// the owner and administrators read whether members make report queues.
pub async fn policy(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    instance_admin(&s, &d).await?;
    let hosted = s.hosted.is_some();
    let v = s.store_call(move |st| policy_view(st, hosted)).await?;
    Ok(Json(v))
}

/// `PUT /api/admin/queues` body: `members_create` `true` or `false`, or
/// `null` for the default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyBody {
    members_create: Value,
}

/// `PUT /api/admin/queues {members_create}` → `{members_create, source}`:
/// the owner or an administrator turns members' queue making on or off
/// (`null`: back to the default), recorded as `admin.settings {fields:
/// ["queues.members_create"], values}`. Existing queues and their admins
/// keep working either way. 400 `invalid_setting` for a value that is no
/// boolean or null.
pub async fn set_policy(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<PolicyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    instance_admin(&s, &d).await?;
    let b = body(req)?;
    let hosted = s.hosted.is_some();
    let value = (!b.members_create.is_null()).then_some(b.members_create);
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_bot_settings(&ctx, &[(Setting::QueuesMembersCreate, value)])?;
            policy_view(st, hosted)
        })
        .await?;
    Ok(Json(v))
}
