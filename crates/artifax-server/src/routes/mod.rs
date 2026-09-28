pub mod artifacts;
pub mod assets;
pub mod content;
pub mod events;
pub mod health;
pub mod token;

use crate::auth::RequireToken;
use crate::state::AppState;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    handler::Handler,
    http::StatusCode,
    routing::{delete, get, post},
};
use tower_http::cors::{Any, CorsLayer};

pub fn router(state: AppState, shutdown: Option<tokio::sync::watch::Sender<bool>>) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    let publish_limit = DefaultBodyLimit::max(artifacts::PUBLISH_BODY_LIMIT);
    let asset_limit = DefaultBodyLimit::max(21 * 1024 * 1024);
    let asset_routes = Router::new()
        .route(
            "/api/artifacts/{aid}/assets",
            get(assets::list).post(assets::upload),
        )
        .route(
            "/api/artifacts/{aid}/assets/{asset_id}",
            delete(assets::delete),
        )
        .layer(asset_limit);
    let mut r = Router::new()
        .route("/healthz", get(health::healthz).layer(cors))
        .route("/api/token", get(token::token))
        .route("/api/events", get(events::events))
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
        .merge(asset_routes)
        .route("/_blob/{asset_id}", get(assets::blob))
        .route("/c/{aid}/v/{n}", get(content::redirect_to_slash))
        .route("/c/{aid}/v/{n}/", get(content::index))
        .route("/c/{aid}/v/{n}/{*path}", get(content::file));
    if let Some(tx) = shutdown {
        let tx = std::sync::Arc::new(tx);
        r = r.route(
            "/api/admin/shutdown",
            post(move |_t: RequireToken| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(true);
                    StatusCode::ACCEPTED
                }
            }),
        );
    }
    r.with_state(state)
}
