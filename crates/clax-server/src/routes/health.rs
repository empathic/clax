use axum::{Json, extract::State};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde_json::json;

/// `GET /healthz`: the version, the process ID (`null` in a Worker, which
/// has no process of its own) and the start time.
pub async fn healthz(
    _d: clax_http::agent::DeniedToGrants,
    State(s): State<AppState>,
) -> Json<serde_json::Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let pid = Some(std::process::id());
    #[cfg(target_arch = "wasm32")]
    let pid: Option<u32> = None;
    Json(json!({"version": s.version, "pid": pid, "started_at": s.started_at}))
}

/// `POST /api/core/trim` (token): on a core embedded in an app
/// ([`AppState::embed`]), releases what SQLite caches on every connection
/// and checkpoints the WAL ([`clax_store::Store::trim_memory`]), when the
/// app leaves the screen or the OS asks it for memory; answers
/// `{"trimmed": {"wal_frames": n, "checkpointed": n}}`. 404 on a daemon,
/// whose memory is its own process's.
pub async fn trim(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: clax_http::auth::RequireToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    if s.embed.is_none() {
        return Err(ApiError::not_found());
    }
    let store = s.store.clone();
    let r = clax_rt::spawn_blocking(move || store.trim_memory())
        .await
        .map_err(|e| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("the memory trim did not finish: {e}"),
            )
        })??;
    tracing::info!(
        wal_frames = r.wal_frames,
        checkpointed = r.checkpointed,
        "trimmed memory"
    );
    Ok(Json(json!({
        "trimmed": {"wal_frames": r.wal_frames, "checkpointed": r.checkpointed}
    })))
}
