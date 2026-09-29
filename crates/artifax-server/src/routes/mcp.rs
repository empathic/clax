//! `/mcp`: the Artifax tool set over MCP streamable HTTP, behind the bearer token.
//!
//! The tools call this daemon's own REST API at `AppState::self_base` with its
//! token, and attribute publishes to no session. rmcp refuses a `Host` other
//! than `localhost`, `127.0.0.1`, `::1`, or the daemon's own address and port
//! (DNS rebinding). The endpoint is not under the request timeout layers: a
//! session's event stream stays open.

use crate::auth::RequireToken;
use crate::routes::artifacts::PUBLISH_BODY_LIMIT;
use crate::state::AppState;
use artifax_mcp::{ArtifaxTools, DaemonClient};
use axum::Router;
use axum::extract::Request;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

async fn require_token(_t: RequireToken, req: Request, next: Next) -> Response {
    next.run(req).await
}

pub fn router(state: &AppState) -> Router<AppState> {
    let tools = ArtifaxTools::new(
        DaemonClient::new(state.self_base.clone(), state.token.clone(), None),
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
