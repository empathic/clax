//! `GET /.well-known/clax` (spec 2026-10-27-clax-sdk-design §4.2): what
//! this Clax is (`local` or `hosted`; the Worker answers `hosted`), the
//! contract version, its name and origin, its sign-in methods and
//! capabilities, for any caller with no credential.

use axum::Json;
use axum::extract::State;
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde_json::{Value, json};

/// The version of the contract (`docs/contract.md`) this Clax serves.
pub const CONTRACT_VERSION: &str = "1";

/// `GET /.well-known/clax` → `{kind, contract_version, name, origin, api,
/// site_domain?, sign_in, device?, capabilities}`, whatever the credential:
///
/// - `kind`: `hosted` for a hosted Clax, else `local`.
/// - `name`: the hosted instance's configured name, else its host
///   (`localhost:<port>` for a local daemon).
/// - `origin`: the hosted public origin, else `http://localhost:<port>`.
/// - `api`: the API root, `/api`.
/// - `site_domain`: hosted, when artifacts' sites have a domain.
/// - `sign_in`: how a client obtains a credential here. `invite` (a
///   person's invite, redeemed by `clax connect` or, hosted, in the
///   browser) and `reporter_token` (minted by a queue's service grant)
///   everywhere; hosted adds `sign_in_link`, `device` and, when it is
///   configured, `github`.
/// - `device`: hosted only, the paths of the device sign-in (`code`,
///   `approve`, `token`).
/// - `capabilities`: `queues` (report queues) and `db` (artifacts' app
///   data).
pub async fn get(State(s): State<AppState>, _d: DeniedToGrants) -> Result<Json<Value>, ApiError> {
    let mut v = match &s.hosted {
        Some(h) => {
            let mut sign_in = vec!["invite", "sign_in_link", "device"];
            if h.github.is_some() {
                sign_in.push("github");
            }
            sign_in.push("reporter_token");
            let mut v = json!({
                "kind": "hosted",
                "name": h.name.clone().unwrap_or_else(|| h.host.clone()),
                "origin": h.origin,
                "sign_in": sign_in,
                "device": {
                    "code": "/api/hosted/device/code",
                    "approve": "/device",
                    "token": "/api/hosted/device/token",
                },
            });
            if let Some(sites) = &h.sites {
                v["site_domain"] = json!(sites.domain());
            }
            v
        }
        None => json!({
            "kind": "local",
            "name": format!("localhost:{}", s.port),
            "origin": format!("http://localhost:{}", s.port),
            "sign_in": ["invite", "reporter_token"],
        }),
    };
    v["contract_version"] = json!(CONTRACT_VERSION);
    v["api"] = json!("/api");
    v["capabilities"] = json!(["queues", "db"]);
    Ok(Json(v))
}
