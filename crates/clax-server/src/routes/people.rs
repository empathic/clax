//! People and their agents (spec 2026-10-08-multi-agent-multi-person §6.4,
//! §7.4, §9.6, §11, §13, §15): `GET /api/agents` lists them, `PUT
//! /api/agents/<handle>` renames one; People's reads (`GET /api/people`,
//! `GET /api/people/me`), and the changes to who may do what: a person's
//! rights and revocation, a machine's or one agent's narrowing, one
//! agent's elevation, and ending one agent.
//!
//! Every change is made in a browser on a same-origin request (M20):
//! `Caller::browser()` and `SameOrigin`, 403 `browser_required` for the
//! daemon token alone, 403 for every agent (`not_for_agents` for the
//! owner's, `forbidden` for a person's), so no agent changes its own
//! rights. Each change that alters what an agent may do bumps the
//! visibility generation in its store write (M17), so it applies at the
//! agent's next request and at a waiting agent's next take.

use super::artifacts::{body, path};
use super::grants::{announce, announce_ended};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use clax_core::rights::Rights;
use clax_core::store::approvals::{ScopeKind, UNCONNECTED};
use clax_core::store::people::{AgentRights, Grant, PersonRow};
use clax_core::{CoreError, NameSource, Store};
use clax_http::agent::DeniedToGrants;
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_http::views::grant::grant_view;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameBody {
    name: String,
}

/// `PUT /api/agents/<handle>` `{name}` → `{agent: {handle, name, harness,
/// person}}`: the agent's person (from their browser) or the owner (a
/// browser, or the token from the CLI) names a live agent (spec §11). The
/// agent can no longer rename itself afterwards (`name_locked`). Renaming
/// is not a right and changes nothing else; it records `agent.name` as
/// the namer. The owner's agents get 403
/// `not_for_agents`: an agent names only itself (`PATCH
/// /api/sessions/<id>`). 404 for a handle that names no live agent of the
/// caller's person; 409 `name_taken`; 400 `invalid_name`.
pub async fn rename_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
    req: Result<Json<RenameBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = match d.caller(&s).await? {
        Caller::Owner { .. } => None,
        Caller::Person { viewer } => Some(viewer.public_id),
        Caller::Anonymous => return Err(ApiError::unauthorized()),
        _ => {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the agent's person or the owner renames it",
            ));
        }
    };
    let handle = path(handle)?;
    let b = body(req)?;
    let session = s
        .store_call(move |st| {
            let session = st
                .live_session_by_handle(&handle)?
                .filter(|a| person.is_none() || a.person == person)
                .ok_or(CoreError::NotFound)?;
            let audit = audit.resolve(st)?;
            st.set_session_name(&audit, &session.id, &b.name, NameSource::Person)
        })
        .await?;
    Ok(Json(json!({"agent": {
        "handle": session.agent_handle,
        "name": session.name,
        "harness": session.harness,
        "person": session.person,
    }})))
}

/// The last component of a working directory: an agent's project.
fn project(cwd: &str) -> &str {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// `GET /api/agents` → `{agents: [{handle, name, person, harness, project,
/// live, state, since, blocked: {since, questions, approvals} | null,
/// threads_owned, working: [{artifact_id, title, message}], unread}]}`
/// (spec 2026-10-08-multi-agent-multi-person §13): every live agent and
/// every one that ended within the last hour, live ones first, newest
/// first. `state` is `waiting` (an open question or `ask` approval request,
/// `since` the oldest), `working` (a working record, `since` the oldest's
/// start), `idle` (`since` the session's start) or `ended` (`since` its
/// end). `blocked` is what the inbox's agent strip counts for it: its open
/// questions and approval requests and the oldest's time.
///
/// Scoped to the caller's person (§13, §18): the owner, by hand or in a
/// browser, sees every agent in full; a connected person, and an agent
/// (the owner's, or a person's, with or without a session), sees its
/// person's agents in full and every other agent by `name`, `person`,
/// `live`, `state` and `since` alone, the rest null (never a handle); an
/// agent reads the approval requests in `blocked` of its own session only,
/// so a sibling's counts only its questions ("Own threads only", spec
/// 2026-10-08 §10). An
/// agent's `working` leaves out artifacts its rights exclude and carries no
/// titles, its messages filtered for its reader, and its `unread` is null;
/// a person's view leaves out live pages (L10). `unread` counts the
/// caller's unread inbox items from the agent. 403 for a LAN viewer or no
/// credential.
pub async fn list_agents(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::AgentsList>,
    _o: SameOrigin,
    extensions: axum::http::Extensions,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    use clax_http::views::thread::Handles;
    let handles = Handles::of(&a.caller);
    if handles == Handles::None {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the owner, a connected person or an agent lists agents",
        ));
    }
    let agent = matches!(
        a.caller,
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. }
    );
    let caller_session = a.caller.session().map(str::to_string);
    let recipient = match &a.caller {
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => None,
    };
    let local = !matches!(a.caller, Caller::Person { .. })
        && clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let working = s.working.clone();
    let v = s
        .store_call(move |st| {
            let unread: std::collections::HashMap<String, u32> = if agent {
                Default::default()
            } else {
                st.inbox_agents(recipient.as_deref())?
                    .into_iter()
                    .map(|g| (g.session_id, g.unread))
                    .collect()
            };
            let vis = vf.build(st)?;
            let mut out = Vec::new();
            for r in st.roster()? {
                let x = &r.session;
                let mine = match &handles {
                    Handles::All => true,
                    Handles::Of(p) => x.person == *p,
                    Handles::None => false,
                };
                let records = if x.ended_at.is_none() {
                    working.for_session(&x.id)
                } else {
                    Vec::new()
                };
                let (state, since) = if let Some(end) = &x.ended_at {
                    ("ended", Some(end.clone()))
                } else if r.asking {
                    ("waiting", r.asking_since.clone())
                } else if let Some(first) = records.iter().map(|w| &w.view.started_at).min() {
                    ("working", Some(first.clone()))
                } else {
                    ("idle", Some(x.started_at.clone()))
                };
                if !mine {
                    out.push(json!({
                        "handle": null, "name": x.name, "person": x.person,
                        "harness": null, "project": null, "live": x.ended_at.is_none(),
                        "state": state, "since": since, "blocked": null,
                        "threads_owned": null, "working": null, "unread": null,
                    }));
                    continue;
                }
                let mut kept = Vec::new();
                let mut titles = Vec::new();
                for w in records {
                    let Some(art) = st.get_artifact(&clax_core::ArtifactId::parse(&w.artifact_id)?)?
                    else {
                        continue;
                    };
                    if (!local && art.kind == clax_core::live::KIND_LIVE) || !reads.allows(&art) {
                        continue;
                    }
                    titles.push((!agent).then_some(art.title));
                    kept.push(w);
                }
                let views = vis.filter_roster(st, &kept)?;
                let working: Vec<Value> = kept
                    .iter()
                    .zip(views)
                    .zip(titles)
                    .map(|((w, v), title)| {
                        json!({"artifact_id": w.artifact_id, "title": title, "message": v["message"]})
                    })
                    .collect();
                // An agent reads approval waits of its own session only
                // ("Own threads only", spec §10): a sibling's would time
                // holds on threads not sent to it.
                let sibling = agent && caller_session.as_deref() != Some(x.id.as_str());
                let blocked = if sibling {
                    r.questions_since.as_ref().map(|since| {
                        json!({"since": since, "questions": r.questions, "approvals": 0})
                    })
                } else {
                    r.blocked_since.as_ref().map(|since| {
                        json!({"since": since, "questions": r.questions, "approvals": r.approvals})
                    })
                };
                out.push(json!({
                    "handle": x.agent_handle, "name": x.name, "person": x.person,
                    "harness": x.harness, "project": project(&x.cwd),
                    "live": x.ended_at.is_none(), "state": state, "since": since,
                    "blocked": blocked, "threads_owned": r.threads_owned,
                    "working": working,
                    "unread": (!agent).then(|| unread.get(&x.id).copied().unwrap_or(0)),
                }));
            }
            Ok(json!({"agents": out}))
        })
        .await?;
    Ok(Json(v))
}

/// Installs the store's agents listener: each committed write that starts,
/// ends or renames an agent publishes `agents` (spec 2026-10-08 §13) to the
/// `agents` topic and to the topics of the artifacts it watched, whose Send
/// menus list live agents only (§14 item 7).
pub fn listen(s: &AppState) {
    let events = s.events.clone();
    s.store.set_agents_listener(move |artifacts| {
        events.publish(clax_core::Event::Agents { artifacts });
    });
}

// ------------------------------------------------- people and their rights

/// 403 unless the owner's browser makes the request (M20): `forbidden` for
/// anyone who is not the owner, `browser_required` for the owner's token
/// alone (the CLI by hand, scripts).
fn owner_browser(caller: &Caller, what: &str) -> Result<(), ApiError> {
    match caller {
        Caller::Owner { browser: true } => Ok(()),
        Caller::Owner { browser: false } => Err(browser_required(what)),
        _ => Err(ApiError::forbidden(
            "forbidden",
            format!("only the owner {what}"),
        )),
    }
}

fn browser_required(what: &str) -> ApiError {
    ApiError::forbidden(
        "browser_required",
        format!("{what} is done in a browser (M20): open People in Clax"),
    )
}

/// The person a change from a browser is by (M20): the owner (`None`) from
/// the owner's browser, a connected person from theirs. 403
/// `browser_required` for the token alone and for agents, 403 `forbidden`
/// for a LAN viewer who is not connected and for no credential.
fn browser_person(caller: &Caller, what: &str) -> Result<Option<String>, ApiError> {
    match caller {
        Caller::Owner { browser: true } => Ok(None),
        Caller::Person { viewer } => Ok(Some(viewer.public_id.clone())),
        Caller::Owner { browser: false }
        | Caller::OwnerAgent { .. }
        | Caller::PersonAgent { .. } => Err(browser_required(what)),
        Caller::Viewer { .. } | Caller::Anonymous => Err(ApiError::forbidden(
            "forbidden",
            "only the owner or a connected person changes their agents",
        )),
    }
}

/// The rights an `{access}` or `{rights}` value names: `null` is none (no
/// narrowing, no elevation, the default rights).
fn rights_of(v: &Value) -> Result<Option<Rights>, ApiError> {
    if v.is_null() {
        return Ok(None);
    }
    Ok(Some(Rights::parse(v)?))
}

/// `{public_id, name, owner}` of granter `g` (`None`: the owner); names
/// are text (§21.9).
fn granter_view(st: &Store, g: Option<&str>) -> clax_core::Result<Value> {
    let raw = match g {
        Some(p) => st.viewer_by_public_id(p)?.and_then(|v| v.display_name),
        None => st.owner()?.and_then(|v| v.display_name),
    };
    let name = clax_core::feedback::person_name(raw.as_deref(), g.is_none());
    Ok(json!({"public_id": g, "name": name, "owner": g.is_none()}))
}

/// An agent's view for People (spec §7.4, O11): who it is, its narrowing,
/// the owner's elevation (`elevate`), every elevation with its granter
/// (`elevations: [{granter: {public_id, name, owner}, access,
/// created_at}]`, the owner's first) and its effective rights (`null` for
/// an agent of this machine, which the daemon token's rights do not
/// limit).
fn agent_view(st: &Store, a: &AgentRights) -> clax_core::Result<Value> {
    let elevations = a
        .elevations
        .iter()
        .map(|e| {
            Ok(json!({
                "granter": granter_view(st, e.granter.as_deref())?,
                "access": e.access.to_json(),
                "created_at": e.created_at,
                "version": e.version,
            }))
        })
        .collect::<clax_core::Result<Vec<Value>>>()?;
    Ok(json!({
        "handle": a.session.agent_handle,
        "name": a.session.name,
        "harness": a.session.harness,
        "person": a.session.person,
        "grant_id": a.session.grant_id,
        "started_at": a.session.started_at,
        "narrow": a.narrow.as_ref().map(Rights::to_json),
        "elevate": a.elevation_by(None).map(Rights::to_json),
        "elevations": elevations,
        "rights": a.rights.as_ref().map(Rights::to_json),
    }))
}

/// The views of `agents` whose person is `person` (`None`: the owner's).
fn agents_of(
    st: &Store,
    agents: &[AgentRights],
    person: &Option<String>,
) -> clax_core::Result<Vec<Value>> {
    agents
        .iter()
        .filter(|a| a.session.person == *person)
        .map(|a| agent_view(st, a))
        .collect()
}

/// Person `pid`'s live approvals of the owner's content (spec §9.5, O12),
/// oldest first: the owner reads these, and no other approval of theirs.
/// Titles are not filtered: the owner's browser sees every artifact (§8.5).
fn approvals_of_yours(st: &Store, pid: &str) -> clax_core::Result<Vec<Value>> {
    let Some(owner) = st.owner()? else {
        return Ok(Vec::new());
    };
    st.live_approvals(Some(pid))?
        .iter()
        .filter(|a| a.author == owner.public_id)
        .map(|a| clax_http::views::approval::approval_view(st, a))
        .collect()
}

/// A person's view for the owner's People: their rights, machines, live
/// agents and live approvals of the owner's content
/// (`approvals_of_yours`); `revoked_at` once revoked (their row stays as
/// history).
fn person_view(
    st: &Store,
    p: &PersonRow,
    grants: &[Grant],
    agents: &[AgentRights],
) -> clax_core::Result<Value> {
    let id = Some(p.viewer.public_id.clone());
    Ok(json!({
        "public_id": p.viewer.public_id,
        "display_name": p.viewer.display_name,
        "revoked_at": p.viewer.person_revoked_at,
        "rights": p.rights.to_json(),
        "grants": grants.iter().filter(|g| g.person == id).map(grant_view).collect::<Vec<_>>(),
        "agents": agents_of(st, agents, &id)?,
        "approvals_of_yours": approvals_of_yours(st, &p.viewer.public_id)?,
    }))
}

/// Person `public_id`'s view, read from `st`.
fn person_of(st: &Store, public_id: &str) -> clax_core::Result<Value> {
    let p = st
        .people()?
        .into_iter()
        .find(|p| p.viewer.public_id == public_id)
        .ok_or(CoreError::NotFound)?;
    person_view(st, &p, &st.list_grants()?, &st.live_agents()?)
}

/// The owner's `*unconnected` switch (spec §9.6): on while the owner's
/// approval of `*unconnected` everywhere is live.
fn unconnected_switch(st: &Store) -> clax_core::Result<Value> {
    let live = st
        .live_approvals(None)?
        .into_iter()
        .find(|a| a.author == UNCONNECTED && a.scope == ScopeKind::Everywhere);
    Ok(json!({"on": live.is_some(), "approval_id": live.map(|a| a.id)}))
}

/// `GET /api/people` (the owner, in a browser or by hand) → `{people:
/// [{public_id, display_name, revoked_at, rights, grants, agents,
/// approvals_of_yours}], unconnected: {on, approval_id}}` (spec §7.4,
/// §9.5, §9.6, §17): every connected person, and each revoked one as
/// history, oldest first, with their machines, live agents and live
/// approvals of the owner's content (O12: never their approvals of anyone
/// else's); and the `*unconnected` switch, which
/// `POST /api/approvals {author: "*unconnected", scope: "everywhere"}`
/// turns on and `DELETE /api/approvals/<approval_id>` off. Agents get 403
/// (`not_for_agents`, `forbidden`).
pub async fn list_people(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    if !caller.is_owner() {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the owner sees people",
        ));
    }
    let v = s
        .store_call(|st| {
            let (grants, agents) = (st.list_grants()?, st.live_agents()?);
            let people = st
                .people()?
                .iter()
                .map(|p| person_view(st, p, &grants, &agents))
                .collect::<clax_core::Result<Vec<Value>>>()?;
            Ok(json!({"people": people, "unconnected": unconnected_switch(st)?}))
        })
        .await?;
    Ok(Json(v))
}

/// `GET /api/people/me` (the owner, or a connected person in their
/// browser) → `{person: {public_id, display_name, owner, rights}, grants,
/// agents}` (spec §7.4, §17): the caller's own rights, machines and live
/// agents, for narrowing them. 403 `forbidden` for a LAN viewer who is not
/// connected and for no credential; agents get 403.
pub async fn me(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = match &caller {
        Caller::Owner { .. } => None,
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the owner or a connected person has People",
            ));
        }
    };
    let v = s
        .store_call(move |st| {
            let (viewer, rights) = match &me {
                None => (st.owner()?, Rights::full()),
                Some(p) => (st.viewer_by_public_id(p)?, st.person_rights(p)?),
            };
            let owner_id = viewer.as_ref().map(|v| v.public_id.clone());
            let grants: Vec<Value> = st
                .list_grants()?
                .iter()
                .filter(|g| match &me {
                    None => g.owner,
                    Some(_) => g.person == me && !g.owner,
                })
                .map(grant_view)
                .collect();
            let agents = agents_of(st, &st.live_agents()?, &me)?;
            Ok(json!({
                "person": {
                    "public_id": owner_id,
                    "display_name": viewer.and_then(|v| v.display_name),
                    "owner": me.is_none(),
                    "rights": rights.to_json(),
                },
                "grants": grants,
                "agents": agents,
            }))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RightsBody {
    /// An access array or a preset name; `null` restores the default
    /// (`clax:collaborator`).
    rights: Value,
}

/// `PUT /api/people/<public_id>/rights` `{rights}` (the owner's browser,
/// spec §7.4, M20) → `{person}`: the person's agents may do what `rights`
/// allow (within each narrowing, beyond it where the owner elevated one)
/// from their next request on. Recorded as `person.rights`. 400
/// `invalid_rights`, `not_a_person`; 404 for an unknown public ID.
pub async fn set_rights(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    pid: Result<Path<String>, PathRejection>,
    req: Result<Json<RightsBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner_browser(&caller, "sets a person's rights")?;
    let pid = path(pid)?;
    let b = body(req)?;
    let rights = rights_of(&b.rights)?;
    let state = s.clone();
    let person = s
        .store_call(move |st| {
            st.set_person_rights(&audit.resolve(st)?, &pid, rights)?;
            let person = person_of(st, &pid)?;
            announce(&state, json!({"what": "person_rights", "person": person}));
            Ok(person)
        })
        .await?;
    Ok(Json(json!({"person": person})))
}

/// `DELETE /api/people/<public_id>` (the owner's browser, spec §6.4, M20)
/// → `{person, ended_grants, revoked_invites}`: the person's grants end
/// with their tokens and live sessions (each session's end cleans up as
/// §14 says), their unused invites are revoked, and their browser is a
/// plain viewer's from then on. Their viewer row, authorship and approvals
/// stay as history. Recorded as each `grant.revoke`, `invite.revoke`, then
/// `person.revoke`. 400 `not_a_person`; 404 for an unknown public ID.
pub async fn revoke_person(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    pid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner_browser(&caller, "revokes a person")?;
    let pid = path(pid)?;
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    let v = s
        .store_call(move |st| {
            let r = st.revoke_person(&audit.resolve(st)?, &pid)?;
            announce_ended(&state, &ctx, st, &r.grants, "owner");
            let person = person_of(st, &pid)?;
            announce(&state, json!({"what": "person_revoked", "person": person}));
            Ok(json!({
                "person": person,
                "ended_grants": r.grants.iter().filter(|g| g.ended).map(|g| &g.grant_id).collect::<Vec<_>>(),
                "revoked_invites": r.invites,
            }))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessBody {
    /// An access array or a preset name; `null` for none.
    access: Value,
}

/// `PUT /api/people/me/grants/<gid>/narrow` `{access}` (the grant's person
/// in their browser, the owner for the owner's machines; spec §7.4, M20)
/// → `{grant}`: every agent of that machine may do only what its person's
/// rights within `access` allow, from its next request on; `null` removes
/// the narrowing. Recorded as `grant.narrow`. 400 `not_narrower` for an
/// access that would widen, `invalid_rights`; 404 for a grant that is not
/// the caller's.
pub async fn narrow_grant(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
    req: Result<Json<AccessBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = browser_person(&caller, "narrowing a machine's agents")?;
    let gid = path(gid)?;
    let b = body(req)?;
    let narrow = rights_of(&b.access)?;
    let state = s.clone();
    let grant = s
        .store_call(move |st| {
            let g = st.grant(&gid)?;
            let mine = match &me {
                None => g.owner,
                Some(p) => !g.owner && g.person.as_deref() == Some(p),
            };
            if !mine || g.status != "approved" {
                return Err(CoreError::NotFound);
            }
            let g = st.narrow_grant(&audit.resolve(st)?, &gid, narrow)?;
            announce(
                &state,
                json!({"what": "grant_narrowed", "grant": grant_view(&g)}),
            );
            Ok(g)
        })
        .await?;
    Ok(Json(json!({"grant": grant_view(&grant)})))
}

/// The live agent `handle` names, when the caller's person may change it:
/// any for the owner when `any`, else only the caller's person's (`me`).
fn agent_of(
    st: &Store,
    handle: &str,
    me: &Option<String>,
    any: bool,
) -> clax_core::Result<clax_core::model::Session> {
    st.live_session_by_handle(handle)?
        .filter(|a| (any && me.is_none()) || a.person == *me)
        .ok_or(CoreError::NotFound)
}

/// `PUT /api/agents/<handle>/narrow` `{access}` (the agent's person in
/// their browser, the owner for the owner's agents; spec §7.4, M20) →
/// `{agent}`: that agent alone may do only what its rights within `access`
/// allow, from its next request (or its next take, for a waiting one) on,
/// until its session ends; `null` removes the narrowing. Recorded as
/// `agent.narrow`. 400 `not_narrower` for an access that would widen,
/// `token_agent` for an agent of this machine (the daemon token, which
/// rights do not limit), `invalid_rights`; 404 for a handle that names no
/// live agent of the caller's person.
pub async fn narrow_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
    req: Result<Json<AccessBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = browser_person(&caller, "narrowing an agent")?;
    let handle = path(handle)?;
    let b = body(req)?;
    let narrow = rights_of(&b.access)?;
    let state = s.clone();
    let agent = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &me, false)?;
            let a = st.narrow_agent(&audit.resolve(st)?, &session.id, narrow)?;
            let view = agent_view(st, &a)?;
            announce(&state, json!({"what": "agent_narrowed", "agent": view}));
            Ok(view)
        })
        .await?;
    Ok(Json(json!({"agent": agent})))
}

/// `PUT /api/agents/<handle>/elevate` `{access, if_version?}` (spec §7.4,
/// M20, O11) → `{agent}` (the agent's whole view for the owner and for its
/// own person; for another person, `{handle, name, person, elevation,
/// version}`, their own raise alone): sets the caller's own elevation of
/// that agent, from their browser: the owner's may name any location; a
/// connected person's names only artifacts they created, with actions they
/// hold there. That agent alone may also do what it gives, beyond its
/// person's rights, from its next request on and until its session ends
/// (§14 clears it). Recorded as `agent.elevate`. With `if_version` (the
/// version of the caller's raise the change was built on, `null` for
/// none), a raise changed since is 409 `conflict` with `raise: {access,
/// version} | null`, the caller's raise as it stands, and nothing is
/// written. 400 `token_agent` for an agent of this machine,
/// `invalid_rights` (an empty access too); 403 `not_creator`,
/// `beyond_your_rights`; 404 for a
/// handle that names no live agent.
pub async fn elevate_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
    req: Result<Json<RaiseBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let granter = browser_person(&caller, "raising an agent")?;
    let handle = path(handle)?;
    let b = body(req)?;
    let Some(access) = rights_of(&b.access)? else {
        return Err(ApiError::bad_request(
            "invalid_rights",
            "name the access to add; DELETE removes an elevation",
        ));
    };
    let Some(if_version) = b.if_version else {
        return set_elevation(s, audit, handle, granter, Some(access)).await;
    };
    // Only over the raise the caller saw (spec §7.4): never bring back one
    // that the agent's person or the owner removed since.
    let state = s.clone();
    let answer = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &None, true)?;
            match st.elevate_agent_if(
                &audit.resolve(st)?,
                &session.id,
                granter.as_deref(),
                Some(access),
                if_version.as_deref(),
            ) {
                Ok(a) => {
                    let view = agent_view(st, &a)?;
                    announce(&state, json!({"what": "agent_elevated", "agent": view}));
                    Ok(Ok(granter_answer(granter.as_deref(), &a, view)))
                }
                Err(CoreError::Invalid {
                    code: "raise_changed",
                    message,
                }) => {
                    let current = st
                        .agent_rights(&session.id)?
                        .and_then(|a| a.elevations.into_iter().find(|e| e.granter == granter))
                        .map(|e| json!({"access": e.access.to_json(), "version": e.version}));
                    Ok(Err((message, current)))
                }
                Err(e) => Err(e),
            }
        })
        .await?;
    match answer {
        Ok(agent) => Ok(Json(json!({"agent": agent}))),
        Err((message, current)) => {
            let mut e = ApiError::new(axum::http::StatusCode::CONFLICT, "conflict", message);
            e.extra
                .insert("raise".into(), current.unwrap_or(Value::Null));
            Err(e)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaiseBody {
    /// An access array or a preset name.
    access: Value,
    /// The version of the caller's raise the change was built on (`null`:
    /// none); absent, the raise is replaced whatever it is now.
    #[serde(default, deserialize_with = "present")]
    if_version: Option<Option<String>>,
}

/// A field that may be `null`: present (`Some`, its value or `None`) or
/// absent (`None` through `default`).
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnraiseQuery {
    /// Whose raise to remove: a granter's public ID, or `owner`; the
    /// caller's own when absent.
    #[serde(default)]
    granter: Option<String>,
}

/// `DELETE /api/agents/<handle>/elevate[?granter=<public_id>|owner]` (a
/// browser; spec §7.4, the owner's decision "Both may remove") → `{agent}`,
/// as `PUT` answers the caller: removes one granter's elevation of that
/// agent, the caller's own when `granter` is absent, from its next request
/// on; other granters' stay. The granter removes their own; the agent's
/// person, and the owner, remove any granter's (recorded with `reason`
/// `removed_by_person` or `removed_by_owner`); the granter may raise it
/// again. Recorded as `agent.elevate` with `access` null. 404 `not_raised`,
/// recording nothing, when that granter has no elevation of it or the
/// caller may not remove it, alike.
pub async fn unelevate_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
    q: Result<axum::extract::Query<UnraiseQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = browser_person(&caller, "removing a raise")?;
    let handle = path(handle)?;
    let q = q
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?
        .0;
    let target = match q.granter {
        None => me.clone(),
        Some(g) if g == "owner" => None,
        Some(g) => Some(g),
    };
    if target == me {
        return set_elevation(s, audit, handle, me, None).await;
    }
    let state = s.clone();
    let agent = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &None, true)?;
            let reason = match &me {
                None => "removed_by_owner",
                Some(_) if session.person == me => "removed_by_person",
                // Not theirs to remove: answered as a raise that is not there.
                Some(_) => return Err(clax_core::store::elevations::not_raised()),
            };
            let a = st.remove_raise(&audit.resolve(st)?, &session.id, target.as_deref(), reason)?;
            let view = agent_view(st, &a)?;
            announce(&state, json!({"what": "agent_elevated", "agent": view}));
            Ok(granter_answer(me.as_deref(), &a, view))
        })
        .await?;
    Ok(Json(json!({"agent": agent})))
}

async fn set_elevation(
    s: AppState,
    audit: DeferredAudit,
    handle: String,
    granter: Option<String>,
    access: Option<Rights>,
) -> Result<Json<Value>, ApiError> {
    let state = s.clone();
    let agent = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &None, true)?;
            let a =
                st.elevate_agent(&audit.resolve(st)?, &session.id, granter.as_deref(), access)?;
            // The owner's People topic gets the whole view (§18).
            let view = agent_view(st, &a)?;
            announce(&state, json!({"what": "agent_elevated", "agent": view}));
            Ok(granter_answer(granter.as_deref(), &a, view))
        })
        .await?;
    Ok(Json(json!({"agent": agent})))
}

/// What a raise's granter reads back (spec §7.4, §18): the owner, and a
/// person raising their own agent, its whole view; a person raising
/// another's agent, only that agent's handle, name and person and their own
/// raise of it (`elevation`), never its narrowing, other granters' raises,
/// rights or machine.
fn granter_answer(granter: Option<&str>, a: &AgentRights, view: Value) -> Value {
    match granter {
        Some(g) if a.session.person.as_deref() != Some(g) => json!({
            "handle": a.session.agent_handle,
            "name": a.session.name,
            "person": a.session.person,
            "elevation": a.elevation_by(Some(g)).map(Rights::to_json),
            "version": a.elevations.iter().find(|e| e.granter.as_deref() == Some(g)).map(|e| &e.version),
        }),
        _ => view,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaisesQuery {
    #[serde(default)]
    artifact: Option<String>,
}

/// `GET /api/people/me/raises?artifact=<aid>` (the owner's or a connected
/// person's browser; spec §7.4, O11) → `{creator, holds, raisable, raises:
/// [{agent: {handle, name, person, live}, access}]}`: the caller's own
/// elevations of live agents, oldest first, never another granter's. With
/// `artifact`: whether the caller created it (`creator`), what they may
/// give there (`holds`: every action there within their rights there, or
/// null), and the handles of its live participant agents of connected
/// machines, which a raise reaches (`raisable`, empty unless `creator`);
/// without, all three null. 403 for agents, the token alone, a LAN viewer.
pub async fn raises(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    q: Result<axum::extract::Query<RaisesQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = browser_person(&caller, "People")?;
    let q = q
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?
        .0;
    let aid = q
        .artifact
        .map(|a| clax_http::headers::parse_id(&a))
        .transpose()?;
    let v = s
        .store_call(move |st| {
            let holds = match &aid {
                Some(a) => st.creator_holds(me.as_deref(), a.as_str())?,
                None => None,
            };
            // The artifact's live agents a raise reaches: those of connected
            // machines (this machine's act with the token, `token_agent`).
            let mut raisable = Vec::new();
            if let (Some(a), Some(_)) = (&aid, &holds) {
                for x in st.participants(a)?.agents.into_iter().filter(|x| x.live) {
                    if st
                        .live_session_by_handle(&x.handle)?
                        .is_some_and(|s| s.grant_id.is_some())
                    {
                        raisable.push(x.handle);
                    }
                }
            }
            let raises: Vec<Value> = st
                .raises_by(me.as_deref())?
                .into_iter()
                .map(|r| {
                    json!({
                        "agent": {
                            "handle": r.session.agent_handle,
                            "name": r.session.name,
                            "person": r.session.person,
                            "live": r.session.ended_at.is_none(),
                        },
                        "access": r.access.to_json(),
                        "version": r.version,
                    })
                })
                .collect();
            Ok(json!({
                "creator": aid.as_ref().map(|_| holds.is_some()),
                "holds": holds.as_ref().map(Rights::to_json),
                "raisable": aid.as_ref().map(|_| raisable),
                "raises": raises,
            }))
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/agents/<handle>` (the owner's browser for any agent, a
/// connected person's for their own; M20) → `{agent: {handle, name,
/// harness, person, ended_at, released_threads, withdrawn_questions}}`:
/// ends the agent's session as its person (`ended_reason` `person`, spec
/// §14), with every end's cleanup: its questions and `ask` requests
/// withdrawn, its threads released, its narrowing and elevation cleared,
/// its watches and working records ended, and one `ended` note when it
/// held anything. Recorded as `session.end` (reason `person`). 404 for a
/// handle that names no live agent the caller may end.
pub async fn end_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = browser_person(&caller, "ending an agent")?;
    let handle = path(handle)?;
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    let v = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &me, true)?;
            let id = session.id.clone();
            let audit = audit.resolve(st)?;
            let ended = st.end_session_by_person(&audit, &id)?;
            clax_http::state::sessions_ended(
                &state,
                &ctx,
                st,
                &audit,
                &ended.touched,
                &ended.withdrawn_questions,
                std::slice::from_ref(&id),
            );
            let x = &ended.session;
            Ok(json!({"agent": {
                "handle": x.agent_handle,
                "name": x.name,
                "harness": x.harness,
                "person": x.person,
                "ended_at": x.ended_at,
                "released_threads": ended.released_threads,
                "withdrawn_questions": ended.withdrawn_questions,
            }}))
        })
        .await?;
    Ok(Json(v))
}
