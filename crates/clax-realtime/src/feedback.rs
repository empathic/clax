//! Fan-out of feedback changes (`feedback_state` events and long-poll
//! wake-ups). The JSON view of a thread, and what an agent channel may see
//! of it, are the HTTP layer's `views::thread`.

use clax_core::feedback::FeedbackState;
#[cfg(debug_assertions)]
use clax_core::feedback::Tier;
use clax_core::feedback::Touched;
use clax_core::{Event, EventBus};
use clax_store::{DeliveryViewer, Store};
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
    /// The note each restored session's first poll hands over (spec
    /// 2026-10-08 §6.7), by session; kept in memory, so a daemon restart
    /// before that poll drops it.
    notes: Mutex<HashMap<String, String>>,
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
    /// Debug builds: sessions whose feedback polls stop after their take
    /// until released ([`FeedbackWaiters::hold`]), and how many of each
    /// session's polls are stopped there.
    #[cfg(debug_assertions)]
    holds: Mutex<HashMap<String, (Stop, Arc<tokio::sync::Semaphore>)>>,
    #[cfg(debug_assertions)]
    held: Mutex<HashMap<String, usize>>,
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
        self.notes.lock().unwrap().remove(session_id);
        if let Some(n) = self.notifies.lock().unwrap().remove(session_id) {
            n.notify_waiters();
        }
    }

    /// Keeps `note` for session `session_id`'s next feedback poll (one a
    /// harness reads: every tier but `queue`), replacing any earlier one.
    pub fn set_note(&self, session_id: &str, note: String) {
        self.notes
            .lock()
            .unwrap()
            .insert(session_id.to_string(), note);
    }

    /// The note kept for session `session_id`, taken: it is handed over once.
    pub fn take_note(&self, session_id: &str) -> Option<String> {
        self.notes.lock().unwrap().remove(session_id)
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

    /// Debug builds: from now on, each feedback poll of `session_id`
    /// stops at `at` until [`FeedbackWaiters::release`]: tests change what
    /// the session may see in that gap.
    #[cfg(debug_assertions)]
    pub fn hold(&self, session_id: &str, at: Stop) {
        self.holds.lock().unwrap().insert(
            session_id.to_string(),
            (at, Arc::new(tokio::sync::Semaphore::new(0))),
        );
    }

    /// Debug builds: lets every poll of `session_id` stopped by
    /// [`FeedbackWaiters::hold`] go on, and stops no more.
    #[cfg(debug_assertions)]
    pub fn release(&self, session_id: &str) {
        if let Some((_, s)) = self.holds.lock().unwrap().remove(session_id) {
            s.close();
        }
    }

    /// Debug builds: stops here while `session_id` is held at `at`.
    #[cfg(debug_assertions)]
    pub async fn stop_at(&self, session_id: &str, at: Stop) {
        let Some(s) = self
            .holds
            .lock()
            .unwrap()
            .get(session_id)
            .filter(|(stop, _)| *stop == at)
            .map(|(_, s)| s.clone())
        else {
            return;
        };
        *self
            .held
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default() += 1;
        self.changed.notify_waiters();
        // Closed on release; nothing ever adds a permit.
        let _ = s.acquire().await;
        if let Some(n) = self.held.lock().unwrap().get_mut(session_id) {
            *n -= 1;
        }
        self.changed.notify_waiters();
    }

    /// Debug builds: how many feedback polls of `session_id` are stopped
    /// at a hold.
    #[cfg(debug_assertions)]
    pub fn held(&self, session_id: &str) -> usize {
        self.held
            .lock()
            .unwrap()
            .get(session_id)
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

/// Debug builds: where a held feedback poll stops
/// ([`FeedbackWaiters::hold`]).
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Before its first take reads the visibility generation.
    FirstTake,
    /// After it built its view, before the take's transaction.
    BeforeTake,
    /// After a take, before it answers or waits.
    AfterTake,
}

#[cfg(debug_assertions)]
impl Stop {
    pub fn parse(s: &str) -> Option<Stop> {
        match s {
            "first_take" => Some(Stop::FirstTake),
            "before_take" => Some(Stop::BeforeTake),
            "after_take" => Some(Stop::AfterTake),
            _ => None,
        }
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
    last: Mutex<HashMap<String, clax_rt::wall::Instant>>,
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
            .insert(self.session_id.clone(), clax_rt::wall::Instant::now());
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
    pub handle: clax_rt::Handle,
    /// Held while a publish reads delivery states and sends them
    /// ([`publish_states`]), so states go out in the order they were read
    /// and the last one a reader hears is the newest.
    pub states: Arc<Mutex<()>>,
    /// Debug builds: called by every publish between reading the states
    /// and sending them, for tests that race two publishes.
    #[cfg(debug_assertions)]
    pub between: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl FeedbackCtx {
    /// Whether the daemon can push to Codex sessions with `codex queue`.
    pub fn codex_push(&self) -> bool {
        self.codex.available()
    }
}

/// Publishes `feedback_state` for every touched thread after a change to
/// its delivery rows: to the owner, and to each person the newest change
/// is visible to ([`clax_store::Delivery::changed_for`]), each as they read it
/// ([`clax_store::Delivery::view`]; spec 2026-10-08 §10, §18). Visitors and agents
/// hear none, and nor does a person whose rows did not change: another
/// author's delivery after an approval tells them nothing. Failures are
/// logged; the change itself has happened.
pub fn publish_states(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    publish(ctx, st, touched, None);
}

/// [`publish_states`] in a store job of its own, started now and not
/// awaited: for a caller that answers first (a feedback poll's take), so
/// the answer does not wait on its readers' states. The states are read
/// when the job runs, so they are the newest then; a thread event the
/// caller published went out before.
pub fn publish_states_after(ctx: &FeedbackCtx, touched: Touched) {
    if touched.threads.is_empty() {
        return;
    }
    let (c, store) = (ctx.clone(), ctx.store.clone());
    ctx.handle.spawn(async move {
        if let Err(e) = store
            .call(move |st| {
                publish_states(&c, st, &touched);
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %e, "feedback states not published");
        }
    });
}

/// [`publish_states`] for an approval decision (made, declined, revoked)
/// by `parties[0]` about `parties[1]`'s content (recipient keys: empty for
/// the owner): to the owner, those two and that content's co-authors
/// ([`clax_store::Delivery::decision_people`]) alone, each as they read it. A
/// decision stamps no row, and no one else reads a held state, so no one
/// else hears of it (spec §10).
pub fn publish_decision(ctx: &FeedbackCtx, st: &Store, touched: &Touched, parties: &[String]) {
    publish(ctx, st, touched, Some(parties));
}

fn publish(ctx: &FeedbackCtx, st: &Store, touched: &Touched, parties: Option<&[String]>) {
    if touched.threads.is_empty() {
        return;
    }
    // Read and sent in one hold: a publish that read earlier never sends
    // after one that read later.
    let _order = ctx.states.lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = touched.threads.iter().map(|(_, t)| t.clone()).collect();
    let mut all = match st.deliveries(&ids, ctx.codex_push()) {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "feedback states unavailable");
            return;
        }
    };
    #[cfg(debug_assertions)]
    if let Some(between) = &ctx.between {
        between();
    }
    for (aid, tid) in &touched.threads {
        let Some(d) = all.remove(tid) else {
            // No row left (a resolve withdrew them): those who read one,
            // the owner and the people who read a row before they went
            // (`Touched::withdrawn`), are told no state is left; no one
            // else is.
            let readers = touched.withdrawn.get(tid).into_iter().flatten();
            let views = std::iter::once(String::new())
                .chain(readers.cloned())
                .map(|k| (k, None))
                .collect();
            ctx.events.publish(Event::FeedbackState {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                views,
            });
            continue;
        };
        let mut views: Vec<(String, Option<FeedbackState>)> = Vec::new();
        if let Some(s) = d.view(&DeliveryViewer::Owner) {
            views.push((String::new(), Some(s)));
        }
        let people: Vec<String> = match parties {
            // The decider and the author decided about, with that
            // content's co-authors (a page's other authors).
            Some(p) => d.decision_people(p).into_iter().collect(),
            None => d.people().into_iter().collect(),
        };
        for p in people {
            let v = DeliveryViewer::Person(p.clone());
            if parties.is_none() && !d.changed_for(&v) {
                continue;
            }
            if let Some(s) = d.view(&v)
                && !views.iter().any(|(k, _)| *k == p)
            {
                views.push((p, Some(s)));
            }
        }
        if !views.is_empty() {
            ctx.events.publish(Event::FeedbackState {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                views,
            });
        }
    }
}

/// [`sessions_ended`] without announcing questions (the reaper, which has
/// no daemon state): the released feedback handed on, and each session's
/// long-polls, working records and followers ended.
pub fn sessions_stop(
    ctx: &FeedbackCtx,
    st: &Store,
    audit: &clax_core::audit::AuditCtx,
    touched: &Touched,
    sessions: &[String],
) {
    apply(ctx, st, touched);
    for id in sessions {
        ctx.waiters.forget(id);
        let (stopped, _) = ctx
            .working
            .end_session(id, clax_core::working::End::SessionEnd);
        crate::working::settle(st, audit, &ctx.events, &ctx.working, &stopped);
        ctx.followers.forget(id);
    }
}

/// Publishes states, wakes the targets' long-polls, and pushes to Codex
/// targets ([`crate::push::dispatch`]). Call after the change is committed.
pub fn apply(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    publish_states(ctx, st, touched);
    wake(ctx, st, touched);
}

/// [`apply`] without the states: wakes the targets' long-polls and pushes
/// to Codex targets. A caller publishes the states itself
/// ([`publish_states_after`]).
pub fn wake(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    ctx.waiters.wake(&touched.targets);
    crate::push::dispatch(ctx, st, &touched.targets);
}

/// [`apply`] for an approval decision by `approver` about `author`'s
/// content: its states reach the owner and those two alone
/// ([`publish_decision`]).
pub fn apply_decision(
    ctx: &FeedbackCtx,
    st: &Store,
    touched: &Touched,
    approver: Option<&str>,
    author: &str,
) {
    publish_decision(ctx, st, touched, &decision_parties(approver, author));
    ctx.waiters.wake(&touched.targets);
    crate::push::dispatch(ctx, st, &touched.targets);
}

/// The recipient keys a decision by `approver` (`None`: the owner) about
/// `author`'s content concerns ([`publish_decision`]).
pub fn decision_parties(approver: Option<&str>, author: &str) -> Vec<String> {
    vec![approver.unwrap_or_default().to_string(), author.to_string()]
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
