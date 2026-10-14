use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::json;

/// `GET /healthz`: the version, the process ID (`null` in a Worker, which
/// has no process of its own) and the start time.
pub async fn healthz(
    _d: crate::agent::DeniedToGrants,
    State(s): State<AppState>,
) -> Json<serde_json::Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let pid = Some(std::process::id());
    #[cfg(target_arch = "wasm32")]
    let pid: Option<u32> = None;
    Json(json!({"version": s.version, "pid": pid, "started_at": s.started_at}))
}
