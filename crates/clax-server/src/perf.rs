//! The latency gate's state the daemon keeps: the calibration database
//! and the gateway's latest signature-verification times. The routes that
//! read them are `crate::routes::perf`.

use clax_core::perf::Calibration;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The daemon's calibration database: built on the first calibration, kept
/// for the daemon's life. The lock lets one calibration run at a time.
pub type Slot = Arc<tokio::sync::Mutex<Option<Calibration>>>;

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
        let mut q = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
