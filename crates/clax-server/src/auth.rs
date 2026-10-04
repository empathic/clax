//! Bearer-token gate for write routes, the `Host` check on every `/api` route
//! (DNS rebinding), and the loopback check for /api/token.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::net::SocketAddr;

/// Extractor that admits a request only when its `Authorization` header is
/// `Bearer <token>` with the daemon's token. The scheme name is matched
/// case-insensitively (RFC 9110 section 11.1); the token is compared in constant time.
pub struct RequireToken;

impl FromRequestParts<AppState> for RequireToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if has_token(&parts.headers, &state.token) {
            Ok(RequireToken)
        } else {
            Err(ApiError::unauthorized())
        }
    }
}

/// True when `headers` carry `Authorization: Bearer <token>` (scheme matched
/// case-insensitively, token compared in constant time).
pub fn has_token(headers: &axum::http::HeaderMap, token: &str) -> bool {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = match header.split_once(' ') {
        Some((scheme, t)) if scheme.eq_ignore_ascii_case("bearer") => t.trim_start_matches(' '),
        _ => "",
    };
    constant_time_eq(presented.as_bytes(), token.as_bytes())
}

/// True when `presented` is the daemon's token (compared in constant time).
pub fn token_matches(presented: &str, token: &str) -> bool {
    constant_time_eq(presented.as_bytes(), token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Both ends of the TCP connection a request arrived on. The daemon serves
/// with this as its connect info; `local` is the address the client reached,
/// which is the bind address, or an interface address for an unspecified bind.
#[derive(Clone, Copy, Debug)]
pub struct Conn {
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

impl
    axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tokio::net::TcpListener>>
    for Conn
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::TcpListener>) -> Self {
        let peer = *stream.remote_addr();
        Conn {
            peer,
            local: stream.io().local_addr().unwrap_or(peer),
        }
    }
}

/// The daemon's listener: a TCP listener that sets socket options on each
/// connection it accepts (see [`crate::daemon::tune_connection`]).
pub struct TunedListener(pub tokio::net::TcpListener);

impl axum::serve::Listener for TunedListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (mut io, addr) = axum::serve::Listener::accept(&mut self.0).await;
        crate::daemon::tune_connection(&mut io);
        (io, addr)
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TunedListener>>
    for Conn
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, TunedListener>) -> Self {
        let peer = *stream.remote_addr();
        Conn {
            peer,
            local: stream.io().local_addr().unwrap_or(peer),
        }
    }
}

/// True when `host` (a `Host` header) may address the API: it names this
/// machine literally ([`is_local_host`]), or it is exactly the address and
/// port the connection arrived on (`local`; an IP literal, IPv6 bracketed,
/// the port optional only for port 80). DNS names other than `localhost` are
/// refused, so a page whose name was rebound to this machine cannot use the
/// API.
pub fn api_host_allowed(host: &str, local: Option<SocketAddr>) -> bool {
    if is_local_host(host) {
        return true;
    }
    let Some(local) = local else {
        return false;
    };
    let named = host.parse::<SocketAddr>().ok().or_else(|| {
        let bare = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        bare.parse::<std::net::IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, 80))
    });
    named.is_some_and(|n| {
        n.ip().to_canonical() == local.ip().to_canonical() && n.port() == local.port()
    })
}

/// Whether the request (its `Host`, else the URI's authority, and the
/// connection it arrived on) may read what the API serves: the
/// [`api_host_allowed`] rule, shared by [`require_api_host`] and by the shell
/// page that embeds API data (`crate::boot`).
pub fn request_host_allowed(
    headers: &axum::http::HeaderMap,
    uri: &axum::http::Uri,
    extensions: &axum::http::Extensions,
) -> bool {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()))
        .unwrap_or("");
    let local = extensions
        .get::<axum::extract::ConnectInfo<Conn>>()
        .map(|c| c.0.local);
    api_host_allowed(host, local)
}

/// Middleware: every `/api` path answers 403 `forbidden_host` unless the
/// request's `Host` passes [`api_host_allowed`]. Artifact hosts never reach
/// `/api` (the host rewrite answers them first), and non-API paths (the shell,
/// content, `/healthz`) are not checked.
pub async fn require_api_host(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path();
    if (path == "/api" || path.starts_with("/api/"))
        && !request_host_allowed(req.headers(), req.uri(), req.extensions())
    {
        return ApiError::forbidden(
            "forbidden_host",
            "the API answers only to localhost, 127.0.0.1, [::1], or the address the daemon is bound to",
        )
        .into_response();
    }
    next.run(req).await
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
    fn api_hosts_are_local_names_or_the_connections_own_address() {
        let lan: SocketAddr = "192.168.1.20:7480".parse().unwrap();
        let v6: SocketAddr = "[fe80::1]:7480".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:192.168.1.20]:7480".parse().unwrap();
        for (h, local) in [
            ("localhost:7480", None),
            ("[::1]", None),
            ("192.168.1.20:7480", Some(lan)),
            ("192.168.1.20:7480", Some(mapped)),
            ("[fe80::1]:7480", Some(v6)),
        ] {
            assert!(api_host_allowed(h, local), "{h} {local:?}");
        }
        for (h, local) in [
            ("192.168.1.20:7480", None),
            ("192.168.1.20:7481", Some(lan)),
            ("192.168.1.20", Some(lan)),
            ("192.168.1.21:7480", Some(lan)),
            ("rebind.example:7480", Some(lan)),
            ("mymac.local:7480", Some(lan)),
            ("", Some(lan)),
        ] {
            assert!(!api_host_allowed(h, local), "{h} {local:?}");
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
