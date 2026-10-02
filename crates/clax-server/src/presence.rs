//! Fan-out of presence changes (`presence` events) and their sweep.

use clax_core::presence::Presence;
use clax_core::{Event, EventBus};

/// Sends the artifact's whole presence list.
pub fn announce(events: &EventBus, p: &Presence, aid: &str) {
    events.publish(Event::Presence {
        artifact_id: aid.to_string(),
        people: p.for_artifact(aid),
    });
}

/// Marks lapsed reports gone, drops old ones, and announces the artifacts they were on.
pub fn sweep_and_announce(p: &Presence, events: &EventBus) {
    for aid in p.sweep() {
        announce(events, p, &aid);
    }
}
