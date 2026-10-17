//! The Clax daemon: every route (`routes`), the router, the shell and content serving (`boot`, `shell_route`), and `serve` (`daemon`). What the routes share is `clax-http`'s.

pub mod boot;
pub mod daemon;
pub mod routes;
pub mod shell_route;

use clax_http::AppState;
use clax_http::{agent, auth, host};

pub fn build_router(state: AppState) -> axum::Router {
    wrap(state.clone(), routes::router(state, None))
}

pub fn build_router_with_shutdown(
    state: AppState,
    shutdown: tokio::sync::watch::Sender<bool>,
) -> axum::Router {
    wrap(state.clone(), routes::router(state, Some(shutdown)))
}

// `Router::layer` runs after route matching, so the host rewrite has to wrap the
// whole inner router (mounted as the outer fallback) to influence routing. The
// `/api` host check runs after the rewrite (the later `layer` is outermost),
// and the header checks of agent credentials after both, before any route.
fn wrap(state: AppState, inner: axum::Router) -> axum::Router {
    axum::Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn_with_state(
            state,
            agent::authenticate,
        ))
        .layer(axum::middleware::from_fn(auth::require_api_host))
        .layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}
