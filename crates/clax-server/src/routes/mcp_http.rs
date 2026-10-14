//! The Clax tool set over MCP Streamable HTTP with JSON responses and no
//! session, through `clax_mcp::http` rather than rmcp's transport: what a
//! Durable Object serves at `/mcp`, where rmcp's service cannot run. The
//! daemon serves it at [`PATH`] beside rmcp's `/mcp`, so both are tested
//! against one suite.
//!
//! It is `/mcp` in every other respect: the owner's, behind the bearer
//! token (a person's agent is refused, `DeniedToGrants`); the same `Host`
//! names admitted; the same body limit; the tools acting as the
//! sessionless agent on `mcp`; and outside the request timeout layers, as
//! a tool may wait. The tools reach the routes through the daemon's own
//! router in-process ([`crate::loopback::Loopback`]), not over HTTP, each
//! request carrying the MCP caller's own `Authorization`
//! ([`ClaxTools::for_caller`]) and never the daemon's token, so a tool
//! call has the rights its caller has and no more.

use crate::auth::RequireToken;
use crate::routes::artifacts::PUBLISH_BODY_LIMIT;
use crate::state::AppState;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use clax_mcp::{ClaxTools, DaemonClient};
use std::sync::Arc;

/// Where the adapter answers: `/mcp` on a Durable Object; beside rmcp's
/// `/mcp` in the daemon.
#[cfg(not(target_arch = "wasm32"))]
pub const PATH: &str = "/mcp/json";
#[cfg(target_arch = "wasm32")]
pub const PATH: &str = "/mcp";

/// As `/mcp`'s: the owner's agents only, with the token.
async fn require_token(
    _d: crate::agent::DeniedToGrants,
    _t: RequireToken,
    req: Request,
    next: Next,
) -> Response {
    next.run(req).await
}

/// The `Host` names the endpoint answers to: the loopback names and the
/// daemon's own authority, as rmcp's service is configured for `/mcp`.
fn allowed_hosts(state: &AppState) -> Vec<String> {
    let own = state
        .self_base
        .split_once("://")
        .map_or(state.self_base.as_str(), |(_, a)| a);
    ["localhost", "127.0.0.1", "::1", own]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether `host` (a `Host` header) is one of `allowed`, compared as rmcp
/// compares them: the name without brackets and ignoring case, and the
/// port only when the allowed entry names one.
fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let split = |a: &str| -> Option<(String, Option<u16>)> {
        let norm = |h: &str| h.trim_matches(['[', ']']).to_ascii_lowercase();
        match axum::http::uri::Authority::try_from(a) {
            Ok(auth) => Some((norm(auth.host()), auth.port_u16())),
            Err(_) if !a.is_empty() => Some((norm(a), None)),
            Err(_) => None,
        }
    };
    let Some((name, port)) = split(host) else {
        return false;
    };
    allowed
        .iter()
        .filter_map(|a| split(a.trim()))
        .any(|(n, p)| n == name && p.is_none_or(|p| Some(p) == port))
}

/// The tool set, shared by the requests. On wasm32 it holds rmcp's `local`
/// tool router, which is not `Send`, so it is wrapped to be (sound because
/// a Worker has one thread; the wrapper panics if used on another).
#[cfg(not(target_arch = "wasm32"))]
type Shared = Arc<ClaxTools>;
#[cfg(target_arch = "wasm32")]
type Shared = Arc<send_wrapper::SendWrapper<ClaxTools>>;

#[cfg(not(target_arch = "wasm32"))]
fn local(tools: ClaxTools) -> ClaxTools {
    tools
}
#[cfg(target_arch = "wasm32")]
fn local(tools: ClaxTools) -> send_wrapper::SendWrapper<ClaxTools> {
    send_wrapper::SendWrapper::new(tools)
}

fn tools_of(shared: &Shared) -> &ClaxTools {
    shared
}

/// The adapter's tool set: the sessionless agent on `mcp`, reaching the
/// router in-process. Its client holds no token: each request's tools act
/// for its caller ([`ClaxTools::for_caller`]).
pub fn tools(state: &AppState) -> ClaxTools {
    let loopback = Arc::new(crate::loopback::Loopback::new(state.clone()));
    ClaxTools::new(
        DaemonClient::new(state.self_base.clone(), String::new(), None)
            .with_via(clax_mcp::client::VIA)
            .with_in_process(loopback),
        state.browser_base.clone(),
        None,
        state.home.log_path(),
    )
}

pub fn router(state: &AppState) -> Router<AppState> {
    let tools = Shared::new(local(tools(state)));
    let allowed = Arc::new(allowed_hosts(state));
    let serve = move |req: Request| {
        let tools = tools.clone();
        let allowed = allowed.clone();
        async move {
            let host = req
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            if !host_allowed(host, &allowed) {
                return (
                    axum::http::StatusCode::FORBIDDEN,
                    "Forbidden: Host header is not allowed",
                )
                    .into_response();
            }
            // The caller's credential, which every request of its tools
            // carries.
            let authorization = crate::loopback::caller_credential(req.headers());
            let (parts, body) = req.into_parts();
            let body = match axum::body::to_bytes(body, PUBLISH_BODY_LIMIT).await {
                Ok(b) => b,
                Err(_) => {
                    return (
                        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "Payload Too Large: request body exceeds {PUBLISH_BODY_LIMIT} bytes"
                        ),
                    )
                        .into_response();
                }
            };
            let req = axum::http::Request::from_parts(parts, body);
            // The tools' futures await `fetch` on wasm32, which is not
            // `Send`; a Worker has one thread.
            let res = clax_rt::sendable(async move {
                let tools = tools_of(&tools).for_caller(authorization);
                clax_mcp::http::handle(&tools, req).await
            })
            .await;
            res.map(axum::body::Body::from)
        }
    };
    Router::new()
        .route_service(PATH, axum::routing::any(serve))
        .layer(DefaultBodyLimit::max(PUBLISH_BODY_LIMIT))
        .route_layer(from_fn_with_state(state.clone(), require_token))
}

#[cfg(test)]
mod tests {
    use super::host_allowed;

    #[test]
    fn hosts_are_matched_as_rmcp_matches_them() {
        let allowed: Vec<String> = ["localhost", "127.0.0.1", "::1", "10.0.0.5:7480"]
            .map(String::from)
            .to_vec();
        for ok in [
            "localhost",
            "LOCALHOST:7480",
            "127.0.0.1:1234",
            "[::1]:7480",
            "10.0.0.5:7480",
        ] {
            assert!(host_allowed(ok, &allowed), "{ok}");
        }
        for no in [
            "",
            "evil.com",
            "10.0.0.5:80",
            "10.0.0.5",
            "localhost.evil.com",
        ] {
            assert!(!host_allowed(no, &allowed), "{no}");
        }
    }
}
