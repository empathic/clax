use crate::auth::{Conn, is_local_host, is_loopback};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
/// The bearer token, for the shell on this machine. Beyond the `/api` host
/// check ([`crate::auth::require_api_host`]), the peer must be a loopback
/// address and the `Host` a literal local name (not the LAN bind address),
/// else 403 `not_loopback`. A request the browser marks same-origin
/// (`Sec-Fetch-Site: same-origin`: the shell's own) also gets the events
/// cookie ([`crate::auth::events_cookie_name`]), HttpOnly, `SameSite=Strict`
/// and scoped to `/api/events`, so the shell's `EventSource` holds the
/// token's level without the token in its URL.
pub async fn token(
    State(s): State<AppState>,
    ConnectInfo(Conn { peer: addr, .. }): ConnectInfo<Conn>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_loopback(addr) || !is_local_host(host) {
        return Err(ApiError::forbidden(
            "not_loopback",
            "the token is only served to local processes",
        ));
    }
    let mut res = (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"token": s.token})),
    )
        .into_response();
    let same_origin = headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() == b"same-origin");
    if same_origin {
        let cookie = format!(
            "{}={}; Path=/api/events; HttpOnly; SameSite=Strict",
            crate::auth::events_cookie_name(host),
            crate::auth::events_cookie_value(&s.token)
        );
        if let Ok(v) = header::HeaderValue::from_str(&cookie) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    Ok(res)
}
