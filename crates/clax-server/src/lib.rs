//! HTTP server for Clax: REST API, content serving, SSE, embedded UI.

pub mod auth;
pub mod blocking;
pub mod daemon;
pub mod db_caller;
pub mod error;
pub mod feedback;
pub mod host;
pub mod http_cache;
pub mod push;
pub mod routes;
pub mod state;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod viewer;
pub mod wrap_cache;

pub use state::AppState;

pub fn build_router(state: AppState) -> axum::Router {
    wrap(routes::router(state, None))
}

pub fn build_router_with_shutdown(
    state: AppState,
    shutdown: tokio::sync::watch::Sender<bool>,
) -> axum::Router {
    wrap(routes::router(state, Some(shutdown)))
}

// `Router::layer` runs after route matching, so the host rewrite has to wrap the
// whole inner router (mounted as the outer fallback) to influence routing. The
// `/api` host check runs after the rewrite (the later `layer` is outermost).
fn wrap(inner: axum::Router) -> axum::Router {
    axum::Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn(auth::require_api_host))
        .layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}
