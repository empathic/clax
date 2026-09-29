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

/// One `Notify` per session that has long-polled for feedback.
#[derive(Default)]
pub struct FeedbackWaiters(Mutex<HashMap<String, Arc<Notify>>>);

impl FeedbackWaiters {
    pub fn get(&self, session_id: &str) -> Arc<Notify> {
        self.0
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
        if let Some(n) = self.0.lock().unwrap().remove(session_id) {
            n.notify_waiters();
        }
    }

    /// Wakes every long-poll currently waiting for one of `sessions`.
    pub fn wake<'a>(&self, sessions: impl IntoIterator<Item = &'a String>) {
        let map = self.0.lock().unwrap();
        for s in sessions {
            if let Some(n) = map.get(s) {
                n.notify_waiters();
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
}

impl FeedbackCtx {
    /// Whether the daemon can push to Codex sessions with `codex queue`.
    pub fn codex_push(&self) -> bool {
        false
    }
}

impl AppState {
    pub fn feedback_ctx(&self) -> FeedbackCtx {
        FeedbackCtx {
            events: self.events.clone(),
            waiters: self.feedback_waiters.clone(),
            browser_base: self.browser_base.clone(),
        }
    }
}

/// Publishes `feedback_state` for every touched thread and wakes long-polls of
/// every touched target. Failures are logged; the change itself has happened.
pub fn apply(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    for (aid, tid) in &touched.threads {
        match st.feedback_state(tid, ctx.codex_push()) {
            Ok(Some(s)) => ctx.events.publish(Event::feedback_state(aid.clone(), s)),
            Ok(None) => {}
            Err(e) => tracing::warn!(thread = %tid, error = %e, "feedback state unavailable"),
        }
    }
    ctx.waiters.wake(&touched.targets);
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
