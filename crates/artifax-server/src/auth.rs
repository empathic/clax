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
    addr.ip().to_canonical().is_loopback()
}

/// True when a `Host` header names this machine literally: `localhost`,
/// `127.0.0.1` or `[::1]`, with an optional `:port`.
pub fn is_local_host(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || tail.starts_with(':') => {
                return inner == "::1";
            }
            _ => return false,
        }
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "localhost" | "127.0.0.1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts_are_accepted() {
        for h in [
            "localhost",
            "localhost:7480",
            "127.0.0.1:1",
            "[::1]:7480",
            "[::1]",
        ] {
            assert!(is_local_host(h), "{h}");
        }
    }

    #[test]
    fn other_hosts_are_rejected() {
        for h in [
            "evil.com",
            "localhost.evil.com",
            "7q3k9mzx2b4t.localhost",
            "",
            "[::1",
            "[::2]:1",
        ] {
            assert!(!is_local_host(h), "{h}");
        }
    }

    #[test]
    fn loopback_peers() {
        for a in ["127.0.0.1:1", "[::1]:1", "[::ffff:127.0.0.1]:1"] {
            assert!(is_loopback(a.parse().unwrap()), "{a}");
        }
        assert!(!is_loopback("10.0.0.1:1".parse().unwrap()));
    }
}
