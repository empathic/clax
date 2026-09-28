//! HTTP server for Artifax: REST API, content serving, SSE, embedded UI.

pub mod auth;
pub mod error;
pub mod routes;
pub mod state;

pub use state::AppState;

pub fn build_router(state: AppState) -> axum::Router {
    routes::router(state)
}
