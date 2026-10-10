//! People and their agents (spec 2026-10-08-multi-agent-multi-person §11,
//! §13, §15): `PUT /api/agents/<handle>` renames an agent.

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
