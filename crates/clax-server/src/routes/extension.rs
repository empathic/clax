//! `/api/extension` (spec 2026-10-05-chrome-overlay-design §9.2): minting,
//! reporting and revoking the Clax extension's credentials. Token only: the
//! native host mints with the token it reads from `daemon.json`. The
//! `viewer` these answer is the owner viewer, which every credential acts as
//! (spec L6); it is serialized without its cookie value.

use super::artifacts::body;
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use clax_core::extension::CREDENTIAL_TTL_DAYS;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintBody {
    extension_id: String,
}

/// `POST /api/extension/credentials` (W) `{extension_id}`: a new credential
/// for the extension with the ID in effect; 400 `unknown_extension` for any
/// other ID. The extension counts as one of the owner's browsers, so this
/// makes the owner viewer when there is none.
pub async fn mint(
    State(s): State<AppState>,
    _t: RequireToken,
    b: Result<Json<MintBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(b)?;
    if b.extension_id != s.extension_id {
        return Err(ApiError::bad_request(
            "unknown_extension",
            "only the Clax extension installed for this Clax home can pair",
        ));
    }
    let creds = s.ext_creds.clone();
    let id = s.extension_id.clone();
    let (m, owner) = s
        .store_call(move |st| {
            let m = creds.refresh(st, |st| st.mint_extension_credential(&id))?;
            Ok((m, st.owner_viewer(true)?))
        })
        .await?;
    Ok(Json(json!({
        "credential": m.credential,
        "viewer": owner,
        "expires_in_s": CREDENTIAL_TTL_DAYS * 86_400,
    })))
}

/// `GET /api/extension` (W): the ID in effect, how many credentials are
/// live, when one was last used, and the owner viewer (null while there is
/// none; reading this never makes it).
pub async fn status(State(s): State<AppState>, _t: RequireToken) -> Result<Json<Value>, ApiError> {
    let (live, viewer) = s
        .store_call(|st| Ok((st.live_extension_credentials()?, st.owner()?)))
        .await?;
    Ok(Json(json!({
        "extension_id": s.extension_id,
        "live_credentials": live.len(),
        "last_used_at": live.iter().map(|c| c.last_used_at.clone()).max(),
        "viewer": viewer,
    })))
}

/// `DELETE /api/extension/credentials` (W): revokes every credential;
/// `{revoked}` is how many of them were live.
pub async fn revoke(State(s): State<AppState>, _t: RequireToken) -> Result<Json<Value>, ApiError> {
    let creds = s.ext_creds.clone();
    let n = s
        .store_call(move |st| creds.refresh(st, |st| st.revoke_extension_credentials()))
        .await?;
    Ok(Json(json!({"revoked": n})))
}
