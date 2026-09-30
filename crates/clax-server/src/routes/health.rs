use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::json;

pub async fn healthz(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"version": s.version, "pid": std::process::id(), "started_at": s.started_at}))
}
