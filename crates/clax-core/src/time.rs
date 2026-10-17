//! Times as Clax writes them: RFC 3339 in UTC, to the millisecond.

use chrono::{DateTime, Utc};

/// `t` as the store writes times: RFC 3339 in UTC, to the millisecond.
pub fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The system clock's time as the store writes times ([`stamp`]), for times
/// taken outside any store (an agent side's, or the daemon's before its
/// store opens).
pub fn stamp_now() -> String {
    stamp(Utc::now())
}
