//! `/mcp`: the Clax tool set over MCP streamable HTTP, behind the bearer token.
//!
//! The tools call this daemon's own REST API at `AppState::self_base` with its
//! token and `x-clax-via: mcp`, so their changes are the sessionless agent's
//! (spec §5.2). rmcp refuses a `Host` other
//! than `localhost`, `127.0.0.1`, `::1`, or the daemon's own address and port
//! (DNS rebinding). The endpoint is not under the request timeout layers: a
//! session's event stream stays open.

use axum::Router;
use axum::extract::Request;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use clax_http::auth::RequireToken;
use clax_http::limits::PUBLISH_BODY_LIMIT;
use clax_http::state::AppState;
use clax_mcp::{ClaxTools, DaemonClient};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

/// `/mcp` is the owner's: a person's agent is refused (`DeniedToGrants`),
/// and the token is required. Its internal client sends the agent-channel
/// marker, so its tools reach the routes as the owner's agent.
async fn require_token(
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    req: Request,
    next: Next,
) -> Response {
    next.run(req).await
}

pub fn router(state: &AppState) -> Router<AppState> {
    let tools = ClaxTools::new(
        DaemonClient::new(state.self_base.clone(), state.token.clone(), None)
            .with_via(clax_mcp::client::VIA),
        state.browser_base.clone(),
        None,
        state.home.log_path(),
    );
    // The daemon's own authority joins the loopback names, so a daemon bound to
    // a specific address (`serve --bind <ip>`) can serve /mcp at that address.
    let own = state
        .self_base
        .split_once("://")
        .map_or(state.self_base.as_str(), |(_, a)| a);
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["localhost", "127.0.0.1", "::1", own])
        .with_max_request_body_bytes(PUBLISH_BODY_LIMIT);
    // End open MCP sessions when the daemon starts shutting down, so they do not
    // hold the graceful drain open.
    let cancel = config.cancellation_token.clone();
    let mut shutdown = state.shutdown.clone();
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async move {
            while !*shutdown.borrow_and_update() {
                if shutdown.changed().await.is_err() {
                    return;
                }
            }
            cancel.cancel();
        });
    }
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .route_service("/mcp", service)
        .route_layer(from_fn_with_state(state.clone(), require_token))
}
