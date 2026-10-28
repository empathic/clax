//! A queue's routes for a deployer's app (spec 2026-10-27-clax-sdk-design
//! §5.3, §7.5), on their own router ([`router`]), which the daemon mounts
//! in front of its own credential layers: they carry their own
//! credentials and answer cross-origin requests.
//!
//! - The app's backend (`Authorization: Clax-Queue <token>` and a
//!   `Clax-Signature` by its service grant's key): `POST
//!   /api/queues/<qid>/reporter-tokens {reporter_id, origin, public_key}`
//!   → `{token, expires_at}`.
//! - A reporter in the app's page (`Authorization: Clax-Reporter <token>`,
//!   a `Clax-Signature` by the reporter's key, and the token's `Origin`):
//!   `POST /api/queues/<qid>/reporter/reports` (multipart) → `{report}`;
//!   `GET /api/queues/<qid>/reporter/reports?cursor=` → `{reports, next}`;
//!   `POST …/reporter/reports/<tid>/comments {body}` → `{report}`; `GET
//!   …/reporter/reports/<tid>/clip`; `OPTIONS` on each.
//!
//! Each answers 501 `not_yet` until it is built.

use super::queues::not_yet;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use clax_http::error::ApiError;
use clax_http::state::AppState;

/// The service and reporter routes, with `state`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/queues/{qid}/reporter-tokens",
            post(mint_token).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports",
            get(my_reports).post(file_report).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/comments",
            post(comment).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/clip",
            get(clip).options(preflight),
        )
        .with_state(state)
}

/// `POST /api/queues/<qid>/reporter-tokens`.
pub async fn mint_token() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reporter/reports`.
pub async fn file_report() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/reporter/reports`.
pub async fn my_reports() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reporter/reports/<tid>/comments`.
pub async fn comment() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/reporter/reports/<tid>/clip`.
pub async fn clip() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `OPTIONS` on each route: the CORS preflight.
pub async fn preflight() -> Result<StatusCode, ApiError> {
    Err(not_yet())
}
