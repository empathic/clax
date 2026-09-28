pub mod health;
pub mod token;

use crate::auth::RequireToken;
use crate::state::AppState;
use axum::{
    Router,
    http::StatusCode,
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    Router::new()
        .route("/healthz", get(health::healthz).layer(cors))
        .route("/api/token", get(token::token))
        // Placeholder so the auth gate is testable; replaced by the real publish route.
        .route(
            "/api/artifacts",
            post(|_: RequireToken| async { StatusCode::NOT_IMPLEMENTED }),
        )
        .with_state(state)
}
