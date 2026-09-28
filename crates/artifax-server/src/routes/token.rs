use crate::auth::{is_local_host, is_loopback};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;

pub async fn token(
    State(s): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
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
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"token": s.token})),
    )
        .into_response())
}
