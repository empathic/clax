//! HTTP server for Clax: REST API, content serving, SSE, embedded UI.

pub mod agent;
pub mod audit;
pub mod auth;
pub mod blocking;
pub mod boot;
pub mod call_ids;
pub mod conn;
pub mod daemon;
pub mod db_caller;
pub mod error;
pub mod exports;
pub mod extension;
pub mod feedback;
pub mod grant_auth;
pub mod headers;
pub mod host;
pub mod http_cache;
pub mod identity;
pub mod limits;
pub mod live;
pub mod live_access;
pub mod perf;
pub mod presence;
pub mod push;
pub mod questions;
pub mod room;
pub mod routes;
pub mod shell_route;
pub mod state;
pub mod stream;
pub mod viewer;
pub mod views;
pub mod working;
pub mod wrap_cache;

pub use state::AppState;

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
