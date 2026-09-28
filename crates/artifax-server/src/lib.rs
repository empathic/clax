//! HTTP server for Artifax: REST API, content serving, SSE, embedded UI.

pub mod auth;
pub mod error;
pub mod host;
pub mod routes;
pub mod state;

pub use state::AppState;

pub fn build_router(state: AppState) -> axum::Router {
    // `Router::layer` runs after route matching, so the host rewrite has to wrap the
    // whole inner router (mounted as the outer fallback) to influence routing.
    axum::Router::new()
        .fallback_service(routes::router(state))
        .layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}
