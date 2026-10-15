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
use super::grants::{announce, announce_ended, grant_view};
use crate::agent::DeniedToGrants;
use crate::audit::DeferredAudit;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use clax_core::rights::Rights;
use clax_core::store::approvals::{ScopeKind, UNCONNECTED};
use clax_core::store::people::{AgentRights, Grant, PersonRow};
use clax_core::{CoreError, NameSource, Store};
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
/// `live`, `state` and `since` alone, the rest null (never a handle). An
/// agent's `working` leaves out artifacts its rights exclude and carries no
/// titles, its messages filtered for its reader, and its `unread` is null;
/// a person's view leaves out live pages (L10). `unread` counts the
/// caller's unread inbox items from the agent. 403 for a LAN viewer or no
/// credential.
pub async fn list_agents(
    State(s): State<AppState>,
    a: crate::agent::Agent<crate::agent::AgentsList>,
    _o: SameOrigin,
    extensions: axum::http::Extensions,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    use crate::feedback::Handles;
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
    let recipient = match &a.caller {
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => None,
    };
    let local = !matches!(a.caller, Caller::Person { .. })
        && crate::live::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let vf = crate::feedback::VisFor::of(&a.caller, &s);
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
                let blocked = r.blocked_since.as_ref().map(|since| {
                    json!({"since": since, "questions": r.questions, "approvals": r.approvals})
                });
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

/// An agent's view for People (spec §7.4): who it is, and its narrowing,
/// elevation and effective rights (`null` for an agent of this machine,
/// which the daemon token's rights do not limit).
fn agent_view(a: &AgentRights) -> Value {
    json!({
        "handle": a.session.agent_handle,
        "name": a.session.name,
        "harness": a.session.harness,
        "person": a.session.person,
        "grant_id": a.session.grant_id,
        "started_at": a.session.started_at,
        "narrow": a.narrow.as_ref().map(Rights::to_json),
        "elevate": a.elevate.as_ref().map(Rights::to_json),
        "rights": a.rights.as_ref().map(Rights::to_json),
    })
}

/// A person's view for the owner's People: their rights, machines and live
/// agents; `revoked_at` once revoked (their row stays as history); and
/// the GitHub account they sign in with (`{id, login, avatar_url,
/// blocked_at}`), or `null`.
fn person_view(
    p: &PersonRow,
    grants: &[Grant],
    agents: &[AgentRights],
    github: &std::collections::HashMap<String, clax_core::store::github::GitHubLink>,
) -> Value {
    let id = Some(p.viewer.public_id.clone());
    json!({
        "github": github.get(&p.viewer.public_id),
        "public_id": p.viewer.public_id,
        "display_name": p.viewer.display_name,
        "revoked_at": p.viewer.person_revoked_at,
        "rights": p.rights.to_json(),
        "grants": grants.iter().filter(|g| g.person == id).map(grant_view).collect::<Vec<_>>(),
        "agents": agents.iter().filter(|a| a.session.person == id).map(agent_view).collect::<Vec<_>>(),
    })
}

/// Person `public_id`'s view, read from `st`.
fn person_of(st: &Store, public_id: &str) -> clax_core::Result<Value> {
    let p = st
        .people()?
        .into_iter()
        .find(|p| p.viewer.public_id == public_id)
        .ok_or(CoreError::NotFound)?;
    Ok(person_view(
        &p,
        &st.list_grants()?,
        &st.live_agents()?,
        &st.github_links()?,
    ))
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
/// [{public_id, display_name, revoked_at, rights, grants, agents}],
/// unconnected: {on, approval_id}}` (spec §7.4, §9.6, §17): every connected
/// person, and each revoked one as history, oldest first, with their
/// machines and live agents; and the `*unconnected` switch, which
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
            let github = st.github_links()?;
            let people: Vec<Value> = st
                .people()?
                .iter()
                .map(|p| person_view(p, &grants, &agents, &github))
                .collect();
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
            let agents: Vec<Value> = st
                .live_agents()?
                .iter()
                .filter(|a| a.session.person == me)
                .map(agent_view)
                .collect();
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
    let who = pid.clone();
    let (person, before, after) = s
        .store_call(move |st| {
            let before = st.person_rights(&pid).ok();
            st.set_person_rights(&audit.resolve(st)?, &pid, rights)?;
            let after = st.person_rights(&pid).ok();
            let person = person_of(st, &pid)?;
            announce(&state, json!({"what": "person_rights", "person": person}));
            Ok((person, before, after))
        })
        .await?;
    // Their repository tokens and their agents' follow the rights.
    crate::git_remotes::rights_changed(&s, &who, before.as_ref(), after.as_ref()).await?;
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
    let who = pid.clone();
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
    // Their repository tokens and their agents' go with them.
    crate::git_remotes::rights_changed(&s, &who, None, None).await?;
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
    let narrowing = narrow.is_some();
    let state = s.clone();
    let grant_id = gid.clone();
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
    // Its agents' repository tokens were handed out under the wider rights.
    if narrowing {
        let sessions = s
            .store_call(move |st| st.list_sessions_for(Some(&grant_id), true))
            .await?;
        for sess in sessions {
            crate::git_remotes::revoke_holder(&s, None, Some(&sess.id)).await?;
        }
    }
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
            let view = agent_view(&a);
            announce(&state, json!({"what": "agent_narrowed", "agent": view}));
            Ok(view)
        })
        .await?;
    Ok(Json(json!({"agent": agent})))
}

/// `PUT /api/agents/<handle>/elevate` `{access}` (the owner's browser,
/// spec §7.4, M20) → `{agent}`: that agent alone may also do what `access`
/// allows, beyond its person's rights, from its next request on and until
/// its session ends (§14 clears it). Recorded as `agent.elevate`. 400
/// `token_agent` for an agent of this machine, `invalid_rights`; 404 for a
/// handle that names no live agent.
pub async fn elevate_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
    req: Result<Json<AccessBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner_browser(&caller, "elevates an agent")?;
    let handle = path(handle)?;
    let b = body(req)?;
    let Some(access) = rights_of(&b.access)? else {
        return Err(ApiError::bad_request(
            "invalid_rights",
            "name the access to add; DELETE removes an elevation",
        ));
    };
    set_elevation(s, audit, handle, Some(access)).await
}

/// `DELETE /api/agents/<handle>/elevate` (the owner's browser) →
/// `{agent}`: the agent is back to its person's rights within its
/// narrowings from its next request on. Recorded as `agent.elevate` with
/// `access` null.
pub async fn unelevate_agent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    handle: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner_browser(&caller, "removes an agent's elevation")?;
    let handle = path(handle)?;
    set_elevation(s, audit, handle, None).await
}

async fn set_elevation(
    s: AppState,
    audit: DeferredAudit,
    handle: String,
    access: Option<Rights>,
) -> Result<Json<Value>, ApiError> {
    let state = s.clone();
    let agent = s
        .store_call(move |st| {
            let session = agent_of(st, &handle, &None, true)?;
            let a = st.elevate_agent(&audit.resolve(st)?, &session.id, access)?;
            let view = agent_view(&a);
            announce(&state, json!({"what": "agent_elevated", "agent": view}));
            Ok(view)
        })
        .await?;
    Ok(Json(json!({"agent": agent})))
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
            crate::feedback::sessions_ended(
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
