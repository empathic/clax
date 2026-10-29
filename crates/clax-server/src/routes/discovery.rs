//! `GET /.well-known/clax` (spec 2026-10-27-clax-sdk-design §4.2): what
//! this Clax is (`local`, `hosted` or `worker`), the contract version, its
//! sign-in methods and capabilities, for any caller with no credential.

use axum::Json;
use axum::extract::State;
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde_json::{Value, json};

/// The version of the contract (`docs/contract.md`) this Clax serves.
pub const CONTRACT_VERSION: &str = "1";

/// `GET /.well-known/clax` → `{kind, contract_version, sign_in,
/// capabilities}`, whatever the credential:
///
/// - `kind`: `hosted` for a hosted Clax, else `local`.
/// - `sign_in`: how a client obtains a credential here. `invite` (a
///   person's invite, redeemed by `clax connect` or, hosted, in the
///   browser) and `reporter_token` (minted by a queue's service grant)
///   everywhere; hosted adds `sign_in_link` and, when it is configured,
///   `github`.
/// - `capabilities`: `queues` (report queues) and `db` (artifacts' app
///   data).
pub async fn get(State(s): State<AppState>, _d: DeniedToGrants) -> Result<Json<Value>, ApiError> {
    let (kind, sign_in) = match &s.hosted {
        Some(h) => {
            let mut sign_in = vec!["invite", "sign_in_link"];
            if h.github.is_some() {
                sign_in.push("github");
            }
            sign_in.push("reporter_token");
            ("hosted", sign_in)
        }
        None => ("local", vec!["invite", "reporter_token"]),
    };
    Ok(Json(json!({
        "kind": kind,
        "contract_version": CONTRACT_VERSION,
        "sign_in": sign_in,
        "capabilities": ["queues", "db"],
    })))
}
