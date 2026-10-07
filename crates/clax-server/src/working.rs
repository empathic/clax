//! Fan-out of working-record changes (`working` events), the sweeper, and
//! the automatic marks made when feedback reaches a session.

use clax_core::audit::{AuditCtx, SystemReason};
use clax_core::working::{Changed, Working};
use clax_core::{Event, EventBus};
use std::sync::Arc;
use std::time::Duration;

/// How often lapsed records are removed and announced.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Sends each changed artifact's whole list.
fn announce(events: &EventBus, w: &Working, changed: &Changed) {
    for aid in &changed.artifacts {
        events.publish(Event::Working {
            artifact_id: aid.clone(),
            working: w.for_artifact(aid),
        });
    }
}

/// Records the records that `changed` started or ended (audit spec §6.5)
/// under `audit`, those that lapsed as `system:ttl`, and sends each changed
/// artifact's whole list. The registry has changed either way, so a failure
/// to record is logged, not returned. Call it outside any store
/// transaction.
pub fn settle(st: &Store, audit: &AuditCtx, events: &EventBus, w: &Working, changed: &Changed) {
    if let Err(e) = st.record_working(audit, &changed.transitions) {
        tracing::error!(error = %e, "recording working records failed");
    }
    announce(events, w, changed);
}

/// Removes lapsed records, records their ends as `system:ttl`, and
/// announces the artifacts they were on.
pub async fn sweep_and_announce(store: &Arc<Store>, w: &Working, events: &EventBus) {
    let changed = w.sweep();
    if !changed.transitions.is_empty() {
        let ended = changed.transitions.clone();
        let recorded = store
            .call(move |st| st.record_working(&AuditCtx::system(SystemReason::Ttl), &ended))
            .await;
        if let Err(e) = recorded {
            tracing::error!(error = %e, "recording lapsed working records failed");
        }
    }
    announce(events, w, &changed);
}

use crate::feedback::FeedbackCtx;
use clax_core::working::Actor;
use clax_core::{FeedbackItem, Store, Tier};
use std::collections::BTreeMap;

/// Tiers whose takes are hook runs or tool calls: each renews the session's records.
pub fn renew_for_tier(ctx: &FeedbackCtx, session_id: &str, tier: Tier) {
    if matches!(tier, Tier::Piggyback | Tier::StopHook | Tier::PromptHook) {
        ctx.working.renew(session_id);
    }
}

/// Feedback reached `session_id`: marks it working on each item's artifact
/// and thread, records the records it starts under `audit`, and announces
/// the changes.
///
/// # Errors
/// When reading the session fails.
pub fn mark_items(
    ctx: &FeedbackCtx,
    audit: &AuditCtx,
    st: &Store,
    session_id: &str,
    items: &[FeedbackItem],
) -> clax_core::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let Some(sess) = st.get_session(session_id)? else {
        return Ok(());
    };
    let who = Actor {
        session_id: sess.id,
        harness: sess.harness,
        agent: sess.agent_handle,
    };
    let mut by_artifact: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for i in items {
        by_artifact
            .entry(&i.artifact_id)
            .or_default()
            .push(i.thread_id.clone());
    }
    let mut changed = clax_core::working::Changed::default();
    for (aid, tids) in by_artifact {
        changed.merge(ctx.working.mark(&who, aid, &tids));
    }
    settle(st, audit, &ctx.events, &ctx.working, &changed);
    Ok(())
}
