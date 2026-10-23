use axum::{Json, extract::State};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde_json::json;

/// `GET /healthz`: the version, the process ID (`null` in a Worker, which
/// has no process of its own), the start time, the commit the binary was
/// built from ([`clax_core::build_commit`]) and the build identity its host
/// was given ([`AppState::build`]); either of the last two is `null` when
/// the build was not stamped with it.
pub async fn healthz(
    _d: clax_http::agent::DeniedToGrants,
    State(s): State<AppState>,
) -> Json<serde_json::Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let pid = Some(std::process::id());
    #[cfg(target_arch = "wasm32")]
    let pid: Option<u32> = None;
    let commit = Some(clax_core::build_commit()).filter(|c| !c.is_empty() && *c != "unknown");
    Json(report(
        s.version,
        pid,
        &s.started_at,
        commit,
        s.build.as_deref(),
    ))
}

/// The body of `/healthz`.
fn report(
    version: &str,
    pid: Option<u32>,
    started_at: &str,
    commit: Option<&str>,
    build: Option<&str>,
) -> serde_json::Value {
    json!({
        "version": version,
        "pid": pid,
        "started_at": started_at,
        "commit": commit,
        "build": build,
    })
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

#[cfg(test)]
mod tests {
    use super::report;
    use serde_json::json;

    #[test]
    fn the_report_names_the_commit_and_build_and_null_when_unstamped() {
        assert_eq!(
            report("0.6.0", None, "t", Some("0a2f4805a"), Some("b-1")),
            json!({"version": "0.6.0", "pid": null, "started_at": "t", "commit": "0a2f4805a", "build": "b-1"})
        );
        assert_eq!(
            report("0.6.0", Some(7), "t", None, None),
            json!({"version": "0.6.0", "pid": 7, "started_at": "t", "commit": null, "build": null})
        );
    }
}
