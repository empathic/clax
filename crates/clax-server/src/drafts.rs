//! Fan-out of draft changes (`drafts` events) and the idle sweep (spec
//! 2026-10-11-clax-dev-hosted-design §16.4).

use clax_core::drafts::Drafts;
use clax_core::{Event, EventBus};
use std::collections::BTreeSet;

/// Sends each of `artifacts`' whole list of live drafts.
pub fn announce(events: &EventBus, drafts: &Drafts, artifacts: &BTreeSet<String>) {
    for aid in artifacts {
        events.publish(Event::Drafts {
            artifact_id: aid.clone(),
            drafts: drafts.for_artifact(aid),
        });
    }
}

/// Ends the drafts that went idle and announces the artifacts they were on.
pub fn sweep_and_announce(drafts: &Drafts, events: &EventBus) {
    let ended = drafts.sweep();
    announce(events, drafts, &ended);
}

/// Ends session `sid`'s draft on `aid` (it published, or discarded it) and
/// announces it when there was one.
pub fn end(events: &EventBus, drafts: &Drafts, sid: &str, aid: &str) -> bool {
    let ended = drafts.end(sid, aid);
    if ended {
        announce(events, drafts, &[aid.to_string()].into());
    }
    ended
}
