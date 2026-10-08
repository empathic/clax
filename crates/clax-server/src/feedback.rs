//! Fan-out of feedback changes (`feedback_state` events and long-poll
//! wake-ups) and the JSON view of a thread.

use crate::state::AppState;
#[cfg(debug_assertions)]
use clax_core::feedback::Tier;
use clax_core::feedback::Touched;
use clax_core::model::Thread;
use clax_core::store::live::LivePage;
use clax_core::{ArtifactId, Event, EventBus, Store};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// One `Notify` per session that has long-polled for feedback, and how many
/// `wait_for_feedback` long-polls (`tier=wait`, `wait > 0`) each session has
/// in progress. Other tiers' long-polls (the Pi `inject` loop) are woken but
/// never counted. Starting a `wait_for_feedback` long-poll wakes the
/// session's other long-polls, so a parked `inject` or notices poll answers
/// empty at once and leaves the wait its feedback. Those polls answer empty
/// at once for as long as the wait lasts, so their clients pace polls that
/// answer early (the Pi plugin, the MCP shim, `clax feedback follow`).
#[derive(Default)]
pub struct FeedbackWaiters {
    notifies: Mutex<HashMap<String, Arc<Notify>>>,
    active: Mutex<HashMap<String, usize>>,
    /// Notified whenever a `wait_for_feedback` long-poll starts or ends.
    changed: Notify,
    /// Debug builds: how many of each session's `wait_for_feedback` polls
    /// are still to end at once ([`FeedbackWaiters::expire`]).
    #[cfg(debug_assertions)]
    expired: Mutex<HashMap<String, usize>>,
    /// Debug builds: how many feedback long-polls of each session and tier
    /// are parked, waiting to be woken ([`FeedbackWaiters::park`]).
    #[cfg(debug_assertions)]
    parked: Mutex<HashMap<(String, Tier), usize>>,
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
    /// client going away), and wakes the session's parked long-polls so
    /// tier 5 sees the wait. Only `tier=wait` polls enter.
    pub fn enter(self: &Arc<Self>, session_id: &str) -> WaitGuard {
        let session_id = session_id.to_string();
        *self
            .active
            .lock()
            .unwrap()
            .entry(session_id.clone())
            .or_default() += 1;
        self.wake(std::iter::once(&session_id));
        self.changed.notify_waiters();
        WaitGuard {
            waiters: self.clone(),
            session_id,
        }
    }

    /// Debug builds: counts a feedback long-poll of `session_id` and `tier`
    /// as parked until the returned guard is dropped.
    #[cfg(debug_assertions)]
    pub fn park(self: &Arc<Self>, session_id: &str, tier: Tier) -> ParkGuard {
        let key = (session_id.to_string(), tier);
        *self.parked.lock().unwrap().entry(key.clone()).or_default() += 1;
        self.changed.notify_waiters();
        ParkGuard {
            waiters: self.clone(),
            key,
        }
    }

    /// Debug builds: how many feedback long-polls of `session_id` and
    /// `tier` are parked.
    #[cfg(debug_assertions)]
    pub fn parked(&self, session_id: &str, tier: Tier) -> usize {
        self.parked
            .lock()
            .unwrap()
            .get(&(session_id.to_string(), tier))
            .copied()
            .unwrap_or(0)
    }

    /// How many `wait_for_feedback` long-polls of `session_id` are in
    /// progress.
    pub fn count(&self, session_id: &str) -> usize {
        self.active
            .lock()
            .unwrap()
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// Notified whenever a `wait_for_feedback` long-poll starts or ends, and
    /// in debug builds whenever a long-poll parks or unparks.
    pub fn changed(&self) -> &Notify {
        &self.changed
    }

    /// Debug builds: ends every `wait_for_feedback` long-poll of
    /// `session_id` now, as if its deadline had passed; how many that is.
    #[cfg(debug_assertions)]
    pub fn expire(&self, session_id: &str) -> usize {
        let n = self.count(session_id);
        if n > 0 {
            self.expired
                .lock()
                .unwrap()
                .insert(session_id.to_string(), n);
            self.wake(std::iter::once(&session_id.to_string()));
        }
        n
    }

    /// Debug builds: whether a `wait_for_feedback` long-poll of
    /// `session_id` is to end now ([`FeedbackWaiters::expire`]); each poll
    /// takes it once.
    #[cfg(debug_assertions)]
    pub fn take_expired(&self, session_id: &str) -> bool {
        let mut e = self.expired.lock().unwrap();
        match e.get_mut(session_id) {
            Some(n) if *n > 0 => {
                *n -= 1;
                if *n == 0 {
                    e.remove(session_id);
                }
                true
            }
            _ => false,
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
        {
            let mut active = self.waiters.active.lock().unwrap();
            if let Some(n) = active.get_mut(&self.session_id) {
                *n -= 1;
                if *n == 0 {
                    active.remove(&self.session_id);
                    #[cfg(debug_assertions)]
                    self.waiters
                        .expired
                        .lock()
                        .unwrap()
                        .remove(&self.session_id);
                }
            }
        }
        self.waiters.changed.notify_waiters();
    }
}

/// A parked feedback long-poll ([`FeedbackWaiters::park`]).
#[cfg(debug_assertions)]
pub struct ParkGuard {
    waiters: Arc<FeedbackWaiters>,
    key: (String, Tier),
}

#[cfg(debug_assertions)]
impl Drop for ParkGuard {
    fn drop(&mut self) {
        {
            let mut parked = self.waiters.parked.lock().unwrap();
            if let Some(n) = parked.get_mut(&self.key) {
                *n -= 1;
                if *n == 0 {
                    parked.remove(&self.key);
                }
            }
        }
        self.waiters.changed.notify_waiters();
    }
}

/// Sessions with a `clax feedback follow` connected: one in a notices
/// long-poll now, or whose last poll ended under [`Followers::RECENT`]
/// ago (the gap while it prints and polls again).
#[derive(Default)]
pub struct Followers {
    active: Mutex<HashMap<String, usize>>,
    last: Mutex<HashMap<String, std::time::Instant>>,
}

impl Followers {
    pub const RECENT: std::time::Duration = std::time::Duration::from_secs(15);

    /// Counts a notices poll of `session_id` until the guard drops.
    pub fn enter(self: &Arc<Self>, session_id: &str) -> FollowGuard {
        *self
            .active
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default() += 1;
        FollowGuard {
            followers: self.clone(),
            session_id: session_id.to_string(),
        }
    }

    /// Whether a follower of `session_id` is connected.
    pub fn is_following(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
            || self
                .last
                .lock()
                .unwrap()
                .get(session_id)
                .is_some_and(|t| t.elapsed() < Self::RECENT)
    }

    /// Drops the record of the ended session `session_id`'s last poll.
    pub fn forget(&self, session_id: &str) {
        self.last.lock().unwrap().remove(session_id);
    }
}

/// An in-progress notices poll ([`Followers::enter`]).
pub struct FollowGuard {
    followers: Arc<Followers>,
    session_id: String,
}

impl Drop for FollowGuard {
    fn drop(&mut self) {
        let mut active = self.followers.active.lock().unwrap();
        if let Some(n) = active.get_mut(&self.session_id) {
            *n -= 1;
            if *n == 0 {
                active.remove(&self.session_id);
            }
        }
        drop(active);
        self.followers
            .last
            .lock()
            .unwrap()
            .insert(self.session_id.clone(), std::time::Instant::now());
    }
}

/// What feedback fan-out needs, cloneable into `store_call` closures.
#[derive(Clone)]
pub struct FeedbackCtx {
    pub events: EventBus,
    pub waiters: Arc<FeedbackWaiters>,
    pub followers: Arc<Followers>,
    pub browser_base: String,
    pub store: Arc<Store>,
    pub codex: Arc<crate::push::CodexPush>,
    pub working: Arc<clax_core::working::Working>,
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
            followers: self.followers.clone(),
            browser_base: self.browser_base.clone(),
            store: self.store.clone(),
            codex: self.codex.clone(),
            working: self.working.clone(),
            handle: tokio::runtime::Handle::current(),
        }
    }
}

/// Publishes `feedback_state` for every touched thread. Failures are logged;
/// the change itself has happened.
pub fn publish_states(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    if touched.threads.is_empty() {
        return;
    }
    let ids: Vec<String> = touched.threads.iter().map(|(_, t)| t.clone()).collect();
    match st.feedback_states(&ids, ctx.codex_push()) {
        Ok(mut states) => {
            for (aid, tid) in &touched.threads {
                if let Some(s) = states.remove(tid) {
                    ctx.events.publish(Event::feedback_state(aid.clone(), s));
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "feedback states unavailable"),
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
/// token), `feedback_state`, and `resolved_by_name`: the current display name
/// of the viewer named by `resolved_by` (not the name at the resolve), or
/// `null` when that viewer has none or the thread was not resolved by a
/// viewer; and `addressed_pending`, `{harness, at}` while an agent's address
/// of a live page's thread waits for the page's next snapshot, else `null`.
/// A live page's thread also carries `page_path` (the path it was made at:
/// its page's, unless a merge rule mapped it there), `page_url` (the origin,
/// that path and the thread's route), `moves` (its moves between pages,
/// oldest first) and `snapshot_path` (its version's `index.html` on disk,
/// only when `with_path`, else `null`).
pub fn thread_view(
    st: &Store,
    t: &Thread,
    codex_push: bool,
    with_path: bool,
) -> clax_core::Result<Value> {
    Ok(
        thread_views(st, std::slice::from_ref(t), codex_push, with_path)?
            .pop()
            .expect("one view per thread"),
    )
}

/// [`thread_view`] of each of `threads`, in order, read in one store call.
pub fn thread_views(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
) -> clax_core::Result<Vec<Value>> {
    let extras = st.thread_extras(threads, codex_push)?;
    let mut live: HashMap<&str, Option<LivePage>> = HashMap::new();
    for t in threads {
        if !live.contains_key(t.artifact_id.as_str()) {
            let p = st.live_page_of(&ArtifactId::parse(&t.artifact_id)?)?;
            live.insert(&t.artifact_id, p);
        }
    }
    threads
        .iter()
        .zip(extras)
        .map(|(t, x)| {
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
            v["feedback_state"] = json!(x.feedback_state);
            v["addressed_in"] = json!(x.addressed_in);
            v["sends"] = json!(x.sends);
            v["resolved_by_name"] = json!(x.resolved_by_name);
            v["addressed_pending"] = match x.addressed_pending {
                Some((harness, at)) => json!({"harness": harness, "at": at}),
                None => Value::Null,
            };
            if let Some(Some(p)) = live.get(t.artifact_id.as_str()) {
                let path = x.live_path.as_deref().unwrap_or(&p.path);
                v["page_path"] = json!(path);
                v["page_url"] = json!(format!(
                    "{}{}{}",
                    p.origin,
                    path,
                    t.anchor.route.as_deref().unwrap_or("")
                ));
                v["moves"] = json!(x.moves);
                v["snapshot_path"] = if with_path {
                    let id = ArtifactId::parse(&t.artifact_id)?;
                    json!(
                        st.home()
                            .version_dir(&id, t.version_n)
                            .join(clax_core::publish::INDEX)
                            .to_string_lossy()
                    )
                } else {
                    Value::Null
                };
            }
            Ok(v)
        })
        .collect()
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

    #[tokio::test]
    async fn starting_a_wait_wakes_the_sessions_parked_polls() {
        let w = Arc::new(FeedbackWaiters::default());
        let (mine, other) = (w.get("s"), w.get("t"));
        let woken = mine.notified();
        let untouched = other.notified();
        tokio::pin!(woken, untouched);
        woken.as_mut().enable();
        untouched.as_mut().enable();
        let _g = w.enter("s");
        assert!(futures::poll!(woken).is_ready());
        assert!(futures::poll!(untouched).is_pending());
    }
}
