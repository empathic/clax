//! The git token routes ([`crate::git_remotes`] has the rules): a
//! signed-in person's browser asks at `/api/artifacts/<aid>/git-token`, an
//! agent session at `/api/sessions/<id>/git-token/<aid>` (the MCP
//! `git_remote` tool).

use crate::agent::{DeniedToGrants, TokenOrAgent};
use crate::auth::Caller;
use crate::error::ApiError;
use crate::git_remotes::{Asker, Scope, mint_for, scopes_for};
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use clax_core::CoreError;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBody {
    scope: String,
}

fn scope_of(b: Result<Json<TokenBody>, JsonRejection>) -> Result<Scope, ApiError> {
    let Json(b) = b.map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?;
    Scope::parse(&b.scope)
        .ok_or_else(|| ApiError::bad_request("invalid_args", "scope must be read or write"))
}

/// The app origin a request came to, where a host serving its own
/// remotes serves them: a hosted daemon's public origin, else the
/// request's absolute URI's, else `http://` and its `Host`.
fn origin_of(s: &AppState, uri: &Uri, headers: &HeaderMap) -> Option<String> {
    if let Some(h) = &s.hosted {
        return Some(h.origin.clone());
    }
    if let (Some(scheme), Some(auth)) = (uri.scheme_str(), uri.authority()) {
        return Some(format!("{scheme}://{auth}"));
    }
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty())
        .map(|h| format!("http://{h}"))
}

/// The asker a browser route's caller is: the owner (browser or CLI) or a
/// person who is not a spectator there; 401 with no credential, 403
/// `sign_in_required` for a guest or a LAN viewer who is not a person, 403
/// `spectator` beyond the live seats, and agents refused.
async fn browser_asker(
    s: &AppState,
    d: &DeniedToGrants,
    aid: &clax_core::ArtifactId,
) -> Result<Asker, ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(s).await?;
    match &caller {
        Caller::Owner { .. } => Ok(Asker::Owner),
        Caller::Person { viewer } => {
            crate::seats::require_seat(s, &caller, aid).await?;
            Ok(Asker::Person(viewer.public_id.clone()))
        }
        Caller::Viewer { .. } => Err(ApiError::forbidden(
            "sign_in_required",
            "git remotes are for signed-in people: a guest cannot clone",
        )),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } => Err(crate::auth::not_for_agents()),
        Caller::PersonAgent { .. } => Err(crate::auth::grants_refused()),
    }
}

/// `POST /api/artifacts/<aid>/git-token {scope}` (a signed-in person, same
/// origin; the owner by hand with the token): a token living
/// [`crate::git_remotes::PERSON_TTL_S`], as
/// [`crate::git_remotes::mint_for`] answers it. Agents ask at
/// `POST /api/sessions/<id>/git-token/<aid>`: 403 `not_for_agents` here.
pub async fn mint(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _same: crate::viewer::SameOrigin,
    uri: Uri,
    headers: HeaderMap,
    p: Result<Path<String>, PathRejection>,
    body: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = super::artifacts::parse_id(&super::artifacts::path(p)?)?;
    let scope = scope_of(body)?;
    let asker = browser_asker(&s, &d, &aid).await?;
    let origin = origin_of(&s, &uri, &headers);
    Ok(Json(mint_for(&s, asker, aid, scope, origin).await?))
}

/// `GET /api/artifacts/<aid>/git-token` (as the POST): what the Clone
/// dialog may offer the caller, `{available, scopes, ttl_s}`: `scopes` the
/// token scopes their rights allow (none, with `reason` `{code, message}`,
/// for a guest, a spectator or no credential), `available` false and no
/// scopes on a daemon that serves no git and for a live page.
pub async fn options(
    State(s): State<AppState>,
    d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = super::artifacts::parse_id(&super::artifacts::path(p)?)?;
    let id = aid.clone();
    s.store_call(move |st| st.get_artifact(&id)?.ok_or(CoreError::NotFound).map(drop))
        .await?;
    if s.git_remotes.get().is_none() || s.live_ids.contains(aid.as_str()) {
        return Ok(Json(json!({"available": false, "scopes": []})));
    }
    let asker = match browser_asker(&s, &d, &aid).await {
        Ok(a) => a,
        Err(e) if e.status == StatusCode::UNAUTHORIZED || e.status == StatusCode::FORBIDDEN => {
            return Ok(Json(json!({
                "available": true,
                "scopes": [],
                "reason": {"code": e.code, "message": e.message},
            })));
        }
        Err(e) => return Err(e),
    };
    let scopes: Vec<&str> = scopes_for(&s, &asker, &aid)
        .await?
        .into_iter()
        .map(Scope::as_str)
        .collect();
    Ok(Json(json!({
        "available": true,
        "scopes": scopes,
        "ttl_s": asker.ttl_s(),
    })))
}

/// `POST /api/sessions/<id>/git-token/<aid> {scope}` (an agent session):
/// a token living [`crate::git_remotes::AGENT_TTL_S`] handed to the
/// session and its person, as [`crate::git_remotes::mint_for`] answers
/// it. 403 `withheld` while the person may not see every author of the
/// repository's history.
pub async fn session_mint(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::GitTokenMint>,
    uri: Uri,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
    body: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = super::artifacts::path(p)?;
    let aid = a.artifact()?;
    crate::agent::person_agent_session(&a.caller)?;
    let scope = scope_of(body)?;
    let id = sid.clone();
    s.store_call(move |st| match st.get_session(&id)? {
        None => Err(CoreError::NotFound),
        Some(x) if x.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "the session has ended",
        )),
        Some(_) => Ok(()),
    })
    .await?;
    let asker = match &a.caller {
        Caller::OwnerAgent { .. } => Asker::OwnerAgent { session: sid },
        Caller::PersonAgent { person, rights, .. } => Asker::PersonAgent {
            person: person.clone(),
            session: sid,
            rights: rights.clone(),
        },
        _ => return Err(ApiError::unauthorized()),
    };
    let origin = origin_of(&s, &uri, &headers);
    Ok(Json(mint_for(&s, asker, aid, scope, origin).await?))
}
