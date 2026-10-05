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
/// cookie ([`crate::auth::events_cookie_name`]), HttpOnly, `SameSite=Strict`,
/// set twice: scoped to `/api/events` and to `/api/stream` (which covers its
/// subscription route), so the shell's event stream holds the token's level
/// without the token in its URL; and the owner cookie
/// ([`crate::identity::owner_cookie_name`]), so the browser is the owner on
/// every route. Such a request is the owner's browser: the viewer its
/// `clax_viewer` cookie names is claimed for the owner
/// ([`clax_core::Store::claim_for_owner`]) and the cookie removed.
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
        if let Some(v) = crate::identity::set_owner_cookie(host, &s.token) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
        if let Some(cookie) = crate::viewer::read(&headers) {
            let claimed = s.store_call(move |st| st.claim_for_owner(&cookie)).await?;
            tracing::debug!(?claimed, "claimed a browser viewer for the owner");
            res.headers_mut()
                .append(header::SET_COOKIE, crate::identity::clear_viewer_cookie());
        }
        for path in ["/api/events", "/api/stream"] {
            let cookie = format!(
                "{}={}; Path={path}; HttpOnly; SameSite=Strict",
                crate::auth::events_cookie_name(host),
                crate::auth::events_cookie_value(&s.token)
            );
            if let Ok(v) = header::HeaderValue::from_str(&cookie) {
                res.headers_mut().append(header::SET_COOKIE, v);
            }
        }
    }
    Ok(res)
}
