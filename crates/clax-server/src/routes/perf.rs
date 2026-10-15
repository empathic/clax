//! The daemon latency gate's hidden routes, for the token only:
//!
//! - `POST /api/admin/perf/calibrate`: the calibration read
//!   ([`clax_core::perf`]), run in this daemon so it shares the process, the
//!   store's worker threads and the HTTP path with the gallery requests the
//!   gate judges against it;
//! - `GET /api/admin/perf/verify`: how long the gateway's latest signature
//!   verifications took ([`VerifyTimes`]), the gate's `sig_verify_us`.

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
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    _d: crate::agent::DeniedToGrants,
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

/// The durations of the gateway's latest signature verifications
/// (`agent::verify_layer`: the signed string built, the body hashed and the
/// signature checked), at most [`VerifyTimes::KEPT`], newest last, in whole
/// microseconds.
#[derive(Debug, Default)]
pub struct VerifyTimes(Mutex<VecDeque<u32>>);

impl VerifyTimes {
    /// How many durations are kept.
    pub const KEPT: usize = 1024;

    /// Keeps `d`, dropping the oldest beyond [`VerifyTimes::KEPT`].
    pub fn record(&self, d: Duration) {
        let us = u32::try_from(d.as_micros()).unwrap_or(u32::MAX);
        let mut q = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.len() == Self::KEPT {
            q.pop_front();
        }
        q.push_back(us);
    }

    /// How many are kept, and their median in microseconds (`None` when
    /// none are).
    pub fn median(&self) -> (usize, Option<f64>) {
        let mut v: Vec<u32> = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        v.sort_unstable();
        let n = v.len();
        let median = match n {
            0 => None,
            _ if n % 2 == 1 => Some(f64::from(v[n / 2])),
            _ => Some((f64::from(v[n / 2 - 1]) + f64::from(v[n / 2])) / 2.0),
        };
        (n, median)
    }
}

/// `GET /api/admin/perf/verify` (the token only) → `{"count": <n>,
/// "median_us": <µs or null>}`: the gateway's latest signature
/// verifications ([`VerifyTimes`]).
pub async fn verify_times(
    _d: crate::agent::DeniedToGrants,
    _t: RequireToken,
    State(s): State<AppState>,
) -> Json<Value> {
    let (count, median) = s.verify_times.median();
    Json(json!({ "count": count, "median_us": median }))
}
