//! The `net` capability's routes (spec 2026-10-12-flutter-real-apps-design
//! C6; `docs/contract.md` "Fetching from the network"). All take the bearer
//! token: pages reach them only through the owner's shell, which asks the
//! owner's consent before the first grant.
//!
//! - `POST /api/artifacts/<id>/net:fetch`: one fetch under [`crate::net`]'s
//!   rules, to a host the live version declares in `capabilities.net.hosts`
//!   and the owner granted.
//! - `GET /api/artifacts/<id>/net-grants`: the declared hosts and grants.
//! - `PUT /api/artifacts/<id>/net-grants`: grants every declared host.
//! - `DELETE /api/artifacts/<id>/net-grants`: revokes every grant.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::net::{FetchRequest, MAX_BODY, NetError, https_url};
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use clax_core::audit::AuditCtx;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Body limit of the fetch route: a [`MAX_BODY`] body, JSON-escaped, with room.
pub const NET_BODY_LIMIT: usize = 3 * MAX_BODY;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchBody {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

fn path(p: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    p.map(|Path(a)| a).map_err(|_| ApiError::not_found())
}

fn net_error(e: &NetError) -> ApiError {
    let status = match e {
        NetError::Invalid(_) | NetError::NotHttps(_) => StatusCode::BAD_REQUEST,
        NetError::HostRefused { .. } | NetError::Blocked { .. } => StatusCode::FORBIDDEN,
        NetError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        NetError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        NetError::TooManyRedirects | NetError::Upstream(_) => StatusCode::BAD_GATEWAY,
    };
    ApiError::new(status, e.code(), e.to_string())
}

/// `POST /api/artifacts/<id>/net:fetch` (token): `{url, method?, headers?,
/// body?}` (`body` a string) → `{url, status, headers, body}`, `url` the
/// final URL after redirects, `body` the response as text (UTF-8, invalid
/// bytes replaced). 404 for an unknown artifact; 403 `not_declared` when
/// the live version does not declare the URL's host, `not_granted` when the
/// owner has not allowed it, `redirect_refused` or `blocked_address`; 400
/// `invalid_argument` or `https_only`; 413 `too_large`; 504 `timeout`; 502
/// `too_many_redirects` or `upstream_error`.
pub async fn fetch(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
    body: Result<Json<FetchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let Json(b) = body.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let access = s.store_call(move |st| st.net_access(&id)).await?;
    if access.declared.is_empty() {
        return Err(ApiError::forbidden(
            "not_declared",
            "this artifact declares no network hosts (capabilities.net.hosts)",
        ));
    }
    let url = https_url(&b.url).map_err(|e| net_error(&e))?;
    let host = url.host_str().unwrap_or_default();
    if !access.declared.iter().any(|h| h == host) {
        return Err(ApiError::forbidden(
            "not_declared",
            format!("{host} is not in this artifact's capabilities.net.hosts"),
        ));
    }
    let req = FetchRequest {
        url: b.url,
        method: b.method.unwrap_or_else(|| "GET".into()),
        headers: b.headers.into_iter().collect(),
        body: b.body.map(String::into_bytes),
    };
    let res = s
        .net
        .fetch(&|h: &str| access.allows(h), req)
        .await
        .map_err(|e| net_error(&e))?;
    Ok(Json(json!({
        "url": res.url,
        "status": res.status,
        "headers": res.headers,
        "body": String::from_utf8_lossy(&res.body),
    })))
}

/// `GET /api/artifacts/<id>/net-grants` (token): `{declared: [host],
/// grants: [{host, granted_at}], granted}`, `granted` true when every
/// declared host is granted. 404 for an unknown artifact.
pub async fn grants(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let acc = s.store_call(move |st| st.net_access(&id)).await?;
    Ok(Json(
        json!({"declared": acc.declared, "grants": acc.grants, "granted": acc.all_granted()}),
    ))
}

/// `PUT /api/artifacts/<id>/net-grants` (token): the owner lets the app
/// fetch from every host its live version declares; answers as [`grants`].
/// 404 for an unknown artifact, 400 `not_declared` when it declares none.
pub async fn grant(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let acc = s.store_call(move |st| st.grant_net(&audit, &id)).await?;
    Ok(Json(
        json!({"declared": acc.declared, "grants": acc.grants, "granted": acc.all_granted()}),
    ))
}

/// `DELETE /api/artifacts/<id>/net-grants` (token): removes every network
/// grant of the app: `{revoked}`, how many there were. 404 for an unknown
/// artifact.
pub async fn revoke(
    State(s): State<AppState>,
    _t: RequireToken,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let n = s.store_call(move |st| st.revoke_net(&audit, &id)).await?;
    Ok(Json(json!({"revoked": n})))
}
