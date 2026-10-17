use axum::{Json, extract::State};
use clax_http::state::AppState;
use serde_json::json;

pub async fn healthz(
    _d: clax_http::agent::DeniedToGrants,
    State(s): State<AppState>,
) -> Json<serde_json::Value> {
    Json(json!({"version": s.version, "pid": std::process::id(), "started_at": s.started_at}))
}
