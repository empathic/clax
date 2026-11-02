//! `GET /.well-known/clax` (spec 2026-10-27-clax-sdk-design §4.2): what
//! this Clax is (`local` or `hosted`; the Worker answers `hosted`), the
//! contract version, its name and origin, its sign-in methods and
//! capabilities, for any caller with no credential.

use axum::Json;
use axum::extract::State;
use clax_core::discovery::{DevicePaths, Discovery};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;

/// The version of the contract (`docs/contract.md`) this Clax serves.
pub const CONTRACT_VERSION: &str = "1";

/// `GET /.well-known/clax` → `{kind, contract_version, name, origin, api,
/// site_domain?, sign_in, device?, capabilities}`, whatever the credential:
///
/// - `kind`: `hosted` for a hosted Clax, else `local`.
/// - `name`: the hosted instance's configured name, else its host. A
///   local daemon's is the host and port of `origin`.
/// - `origin`: the hosted public origin; a local daemon's is where it
///   says it is reached (`AppState::browser_base`: `localhost` on a
///   loopback bind, the LAN address on a LAN bind). Advisory for a local
///   daemon, not an identity, and the same for every caller.
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
pub async fn get(
    State(s): State<AppState>,
    _d: DeniedToGrants,
) -> Result<Json<Discovery>, ApiError> {
    let common = Discovery {
        contract_version: CONTRACT_VERSION.into(),
        api: "/api".into(),
        capabilities: vec!["queues".into(), "db".into()],
        ..Discovery::default()
    };
    Ok(Json(match &s.hosted {
        Some(h) => {
            let mut sign_in = vec!["invite", "sign_in_link", "device"];
            if h.github.is_some() {
                sign_in.push("github");
            }
            sign_in.push("reporter_token");
            Discovery {
                kind: "hosted".into(),
                name: h.name.clone().unwrap_or_else(|| h.host.clone()),
                origin: h.origin.clone(),
                site_domain: h.sites.as_ref().map(|x| x.domain().to_string()),
                sign_in: sign_in.into_iter().map(String::from).collect(),
                device: Some(DevicePaths {
                    code: "/api/hosted/device/code".into(),
                    approve: "/device".into(),
                    token: "/api/hosted/device/token".into(),
                }),
                ..common
            }
        }
        None => Discovery {
            kind: "local".into(),
            name: crate::public::default_name(&s.browser_base),
            origin: s.browser_base.clone(),
            sign_in: vec!["invite".into(), "reporter_token".into()],
            ..common
        },
    }))
}
