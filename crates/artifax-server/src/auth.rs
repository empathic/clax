//! Bearer-token gate for write routes and the loopback check for /api/token.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::net::SocketAddr;

pub struct RequireToken;

impl FromRequestParts<AppState> for RequireToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let presented = header.strip_prefix("Bearer ").unwrap_or("");
        if constant_time_eq(presented.as_bytes(), state.token.as_bytes()) {
            Ok(RequireToken)
        } else {
            Err(ApiError::unauthorized())
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().is_loopback()
}
