//! `POST /api/admin/perf/calibrate` (hidden, the token only): the daemon
//! latency gate's calibration read ([`clax_core::perf`]), run in this
//! daemon so it shares the process, the store's worker threads and the HTTP
//! path with the gallery requests the gate judges against it.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{RawQuery, State},
    http::StatusCode,
};
use clax_core::perf::Calibration;
use serde_json::{Value, json};
use std::sync::Arc;

/// The daemon's calibration database: built on the first calibration, kept
/// for the daemon's life. The lock lets one calibration run at a time.
pub type Slot = Arc<tokio::sync::Mutex<Option<Calibration>>>;

/// Runs the calibration read once and answers `{"ms": <milliseconds>}`,
/// the read's time measured around it on the store worker. The first call
/// builds the in-memory database (and runs the read once to warm it) before
/// the timed read. It waits in the store's bulk lane, as the gallery list
/// and attention requests it calibrates do, so interactive calls start
/// ahead of it. One runs at a time: a call while another is running is
/// refused with 409 `busy` instead of holding a worker. Only the token
/// itself is admitted: the token narrowed with `?as_level=` (as an agent's
/// db tools narrow it) is refused with 403.
pub async fn calibrate(
    _t: RequireToken,
    State(s): State<AppState>,
    RawQuery(q): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let narrowed = q
        .as_deref()
        .unwrap_or("")
        .split('&')
        .any(|kv| kv.split_once('=').map_or(kv, |(k, _)| k) == "as_level");
    if narrowed {
        return Err(ApiError::forbidden(
            "forbidden",
            "a narrowed credential may not calibrate",
        ));
    }
    let Ok(mut slot) = s.calibration.clone().try_lock_owned() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "busy",
            "a calibration is running",
        ));
    };
    let elapsed = s
        .store_call_bulk(move |_| {
            let cal = match slot.take() {
                Some(cal) => cal,
                None => Calibration::create()?,
            };
            slot.insert(cal).run()
        })
        .await?;
    Ok(Json(json!({ "ms": elapsed.as_secs_f64() * 1000.0 })))
}
