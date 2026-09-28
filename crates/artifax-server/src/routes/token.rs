use crate::auth::is_loopback;
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{ConnectInfo, State},
};
use std::net::SocketAddr;

pub async fn token(
    State(s): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(addr) {
        return Err(ApiError::forbidden(
            "not_loopback",
            "the token is only served to local processes",
        ));
    }
    Ok(Json(serde_json::json!({"token": s.token})))
}
