//! Fan-out of feedback changes (`feedback_state` events and long-poll
//! wake-ups) and the JSON view of a thread.

use crate::state::AppState;
use artifax_core::feedback::Touched;
use artifax_core::model::Thread;
use artifax_core::{ArtifactId, Event, EventBus, Store};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// One `Notify` per session that has long-polled for feedback, and how many
/// `wait_for_feedback` long-polls (`tier=wait`, `wait > 0`) each session has
/// in progress. Other tiers' long-polls (the Pi `inject` loop) are woken but
/// never counted.
#[derive(Default)]
pub struct FeedbackWaiters {
    notifies: Mutex<HashMap<String, Arc<Notify>>>,
    active: Mutex<HashMap<String, usize>>,
}

impl FeedbackWaiters {
    pub fn get(&self, session_id: &str) -> Arc<Notify> {
        self.notifies
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }

    /// Wakes any long-poll of the ended session `session_id` (it then answers
    /// empty at its deadline or on its next take) and drops its entry, so the
    /// map holds only sessions that may still poll.
    pub fn forget(&self, session_id: &str) {
        if let Some(n) = self.notifies.lock().unwrap().remove(session_id) {
            n.notify_waiters();
        }
    }

    /// Wakes every long-poll currently waiting for one of `sessions`.
    pub fn wake<'a>(&self, sessions: impl IntoIterator<Item = &'a String>) {
        let map = self.notifies.lock().unwrap();
        for s in sessions {
            if let Some(n) = map.get(s) {
                n.notify_waiters();
            }
        }
    }

    /// Counts a `wait_for_feedback` long-poll of `session_id` as in progress
    /// until the returned guard is dropped (on every exit, including the
    /// client going away). Only `tier=wait` polls enter.
    pub fn enter(self: &Arc<Self>, session_id: &str) -> WaitGuard {
        *self
            .active
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default() += 1;
        WaitGuard {
            waiters: self.clone(),
            session_id: session_id.to_string(),
        }
    }

    /// Whether a `wait_for_feedback` long-poll (`tier=wait`) of `session_id`
    /// is in progress; tier 5 (`codex queue`, Pi `inject`) is skipped while it is.
    pub fn is_waiting(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
    }
}

/// An in-progress long-poll ([`FeedbackWaiters::enter`]).
pub struct WaitGuard {
    waiters: Arc<FeedbackWaiters>,
    session_id: String,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        let mut active = self.waiters.active.lock().unwrap();
        if let Some(n) = active.get_mut(&self.session_id) {
            *n -= 1;
            if *n == 0 {
                active.remove(&self.session_id);
            }
        }
    }
}

/// What feedback fan-out needs, cloneable into `store_call` closures.
#[derive(Clone)]
pub struct FeedbackCtx {
    pub events: EventBus,
    pub waiters: Arc<FeedbackWaiters>,
    pub browser_base: String,
    pub store: Arc<Store>,
    pub codex: Arc<crate::push::CodexPush>,
    /// The runtime `codex queue` runs on; dispatch may be called from blocking threads.
    pub handle: tokio::runtime::Handle,
}

impl FeedbackCtx {
    /// Whether the daemon can push to Codex sessions with `codex queue`.
    pub fn codex_push(&self) -> bool {
        self.codex.available()
    }
}

impl AppState {
    pub fn feedback_ctx(&self) -> FeedbackCtx {
        FeedbackCtx {
            events: self.events.clone(),
            waiters: self.feedback_waiters.clone(),
            browser_base: self.browser_base.clone(),
            store: self.store.clone(),
            codex: self.codex.clone(),
            handle: tokio::runtime::Handle::current(),
        }
    }
}

/// Publishes `feedback_state` for every touched thread. Failures are logged;
/// the change itself has happened.
pub fn publish_states(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    for (aid, tid) in &touched.threads {
        match st.feedback_state(tid, ctx.codex_push()) {
            Ok(Some(s)) => ctx.events.publish(Event::feedback_state(aid.clone(), s)),
            Ok(None) => {}
            Err(e) => tracing::warn!(thread = %tid, error = %e, "feedback state unavailable"),
        }
    }
}

/// Publishes states, wakes the targets' long-polls, and pushes to Codex
/// targets ([`crate::push::dispatch`]). Call after the change is committed.
pub fn apply(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    publish_states(ctx, st, touched);
    ctx.waiters.wake(&touched.targets);
    crate::push::dispatch(ctx, st, &touched.targets);
}

/// The thread as routes return it: the stored fields plus `clip_url`,
/// `clip_path` (only when `with_path`, that is the caller presented the
/// token), and `feedback_state`.
pub fn thread_view(
    st: &Store,
    t: &Thread,
    codex_push: bool,
    with_path: bool,
) -> artifax_core::Result<Value> {
    let mut v = serde_json::to_value(t).expect("threads serialise");
    v["clip_url"] = if t.has_clip {
        json!(format!(
            "/api/artifacts/{}/threads/{}/clip",
            t.artifact_id, t.id
        ))
    } else {
        Value::Null
    };
    v["clip_path"] = if t.has_clip && with_path {
        let id = ArtifactId::parse(&t.artifact_id)?;
        json!(st.home().clip_path(&id, &t.id).to_string_lossy())
    } else {
        Value::Null
    };
    v["feedback_state"] = json!(st.feedback_state(&t.id, codex_push)?);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::FeedbackWaiters;
    use std::sync::Arc;

    #[test]
    fn a_session_waits_while_any_of_its_polls_holds_a_guard() {
        let w = Arc::new(FeedbackWaiters::default());
        assert!(!w.is_waiting("s"));
        let a = w.enter("s");
        let b = w.enter("s");
        assert!(w.is_waiting("s") && !w.is_waiting("t"));
        drop(a);
        assert!(w.is_waiting("s"));
        w.forget("s");
        assert!(
            w.is_waiting("s"),
            "ending wakes polls; they stop waiting when they return"
        );
        drop(b);
        assert!(!w.is_waiting("s"));
    }
}
