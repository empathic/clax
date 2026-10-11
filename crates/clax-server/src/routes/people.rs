//! People and their agents (spec 2026-10-08-multi-agent-multi-person §11,
//! §13, §15): `GET /api/agents` lists them, `PUT /api/agents/<handle>`
//! renames one.

use super::artifacts::{body, path};
use crate::agent::DeniedToGrants;
use crate::audit::DeferredAudit;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use clax_core::{CoreError, NameSource};
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
