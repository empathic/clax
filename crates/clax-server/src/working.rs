//! Fan-out of working-record changes (`working` events), the sweeper, and
//! the automatic marks made when feedback reaches a session.

use clax_core::working::{Changed, Working};
use clax_core::{Event, EventBus};
use std::time::Duration;

/// How often lapsed records are removed and announced.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Sends each changed artifact's whole list.
pub fn announce(events: &EventBus, w: &Working, changed: &Changed) {
    for aid in &changed.0 {
        events.publish(Event::Working {
            artifact_id: aid.clone(),
            working: w.for_artifact(aid),
        });
    }
}

/// Removes lapsed records and announces the artifacts they were on.
pub fn sweep_and_announce(w: &Working, events: &EventBus) {
    let changed = w.sweep();
    announce(events, w, &changed);
}
