pub mod artifacts;
pub mod health;
pub mod token;

use crate::state::AppState;
use axum::{Router, extract::DefaultBodyLimit, handler::Handler, routing::get};
use tower_http::cors::{Any, CorsLayer};

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    let publish_limit = DefaultBodyLimit::max(artifacts::PUBLISH_BODY_LIMIT);
    Router::new()
        .route("/healthz", get(health::healthz).layer(cors))
        .route("/api/token", get(token::token))
        .route(
            "/api/artifacts",
            get(artifacts::list).post(artifacts::create.layer(publish_limit)),
        )
        .route(
            "/api/artifacts/{aid}",
            get(artifacts::get)
                .patch(artifacts::patch)
                .delete(artifacts::delete),
        )
        .route(
            "/api/artifacts/{aid}/versions",
            get(artifacts::list_versions).post(artifacts::publish.layer(publish_limit)),
        )
        .route(
            "/api/artifacts/{aid}/versions/{n}",
            get(artifacts::get_version),
        )
        .route("/api/artifacts/{aid}/files", get(artifacts::files))
        .with_state(state)
}
