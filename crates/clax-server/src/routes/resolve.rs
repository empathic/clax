//! `GET /api/resolve?url=<url>` → what the URL names ([`crate::resolve`]):
//! `{kind: "local", artifact_id, version?}`, `{kind: "remote", remote,
//! origin, name, artifact_id, version?, url}`, `{kind: "sign_in_required",
//! origin, name}` or `{kind: "page", url}`. It changes nothing but the
//! discovery cache.
//!
//! The owner and the owner's agents get every answer; any other caller
//! (a person's agent, a person) only `local` or `page`
//! ([`crate::resolve::resolve_local`]), as does every caller of a hosted
//! daemon. 401 without a credential; 400 `invalid_query` without `url`,
//! 400 `invalid_url` for one [`crate::resolve::resolve`] refuses.

use axum::Json;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::QueryRejection;
use clax_http::agent::{Agent, UrlResolve};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct ResolveQuery {
    url: String,
}

/// `GET /api/resolve?url=`, as the module comment says.
pub async fn get(
    State(s): State<AppState>,
    a: Agent<UrlResolve>,
    _o: SameOrigin,
    q: Result<Query<ResolveQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    if matches!(a.caller, Caller::Anonymous) {
        return Err(ApiError::unauthorized());
    }
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let resolved = match a.caller {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => {
            // A discovery request is a JavaScript fetch on the Worker.
            clax_rt::sendable(crate::resolve::resolve(&s, &q.url)).await?
        }
        _ => crate::resolve::resolve_local(&s, &q.url)?,
    };
    Ok(Json(serde_json::json!(resolved)))
}
