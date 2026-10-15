//! Fan-out of feedback changes (`feedback_state` events and long-poll
//! wake-ups), the JSON view of a thread, and what an agent channel may see
//! of it ([`Vis`]).

use crate::state::AppState;
use clax_core::feedback::Touched;
#[cfg(debug_assertions)]
use clax_core::feedback::{FeedbackState, Tier};
use clax_core::model::{Artifact, Asset, Comment, Thread, Version};
use clax_core::store::approvals::UNCONNECTED;
use clax_core::store::docs::{Acquired, Doc, DocPage, Written};
use clax_core::store::live::LivePage;
use clax_core::store::threads::ThreadOwner;
use clax_core::visibility::placeholder::{self, ApprovalState, WithheldAuthor};
use clax_core::visibility::{self, Author, Reader, Visibility};
use clax_core::working::{SessionWorking, WorkingView};
use clax_core::{ArtifactId, DeliveryViewer, Event, EventBus, Store};
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

/// Publishes `feedback_state` for every touched thread after a change to
/// its delivery rows: to the owner, and to each person the newest change
/// is visible to ([`clax_core::Delivery::changed_for`]), each as they read it
/// ([`clax_core::Delivery::view`]; spec 2026-10-08 §10, §18). Visitors and agents
/// hear none, and nor does a person whose rows did not change: another
/// author's delivery after an approval tells them nothing. Failures are
/// logged; the change itself has happened.
pub fn publish_states(ctx: &FeedbackCtx, st: &Store, touched: &Touched) {
    publish(ctx, st, touched, None);
}

/// [`publish_states`] for an approval decision (made, declined, revoked)
/// by `parties[0]` about `parties[1]`'s content (recipient keys: empty for
/// the owner): to the owner, those two and that content's co-authors
/// ([`clax_core::Delivery::decision_people`]) alone, each as they read it. A
/// decision stamps no row, and no one else reads a held state, so no one
/// else hears of it (spec §10).
pub fn publish_decision(ctx: &FeedbackCtx, st: &Store, touched: &Touched, parties: &[String]) {
    publish(ctx, st, touched, Some(parties));
}

fn publish(ctx: &FeedbackCtx, st: &Store, touched: &Touched, parties: Option<&[String]>) {
    if touched.threads.is_empty() {
        return;
    }
    let ids: Vec<String> = touched.threads.iter().map(|(_, t)| t.clone()).collect();
    let mut all = match st.deliveries(&ids, ctx.codex_push()) {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "feedback states unavailable");
            return;
        }
    };
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

/// The live side of sessions' ends (spec 2026-10-08 §14), once the store
/// committed them with their cleanup: the feedback they released reaches
/// its new targets ([`apply`]), their withdrawn questions are announced,
/// and for each ended session in `sessions` its long-polls are woken and
/// forgotten, its working records end (recorded under `audit`), and its
/// followers stop. Every route that ends sessions calls it.
pub fn sessions_ended(
    s: &AppState,
    ctx: &FeedbackCtx,
    st: &Store,
    audit: &clax_core::audit::AuditCtx,
    touched: &Touched,
    withdrawn_questions: &[String],
    sessions: &[String],
) {
    crate::questions::announce_ids(s, st, withdrawn_questions);
    sessions_stop(ctx, st, audit, touched, sessions);
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

/// The thread as the owner reads it (the live page routes, which only the
/// owner reaches): the stored fields plus `clip_url`,
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
    thread_views_as(st, threads, codex_push, with_path, &DeliveryViewer::Owner)
}

/// The views a `thread` event carries, which reach every subscriber of the
/// artifact: no `clip_path`, and no `feedback_state` (each reader hears
/// its own in `feedback_state` events, spec 2026-10-08 §10), so a client
/// keeps the one it holds.
pub fn thread_event_views(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
) -> clax_core::Result<Vec<Value>> {
    let mut views = thread_views_as(st, threads, codex_push, false, &DeliveryViewer::Visitor)?;
    for v in &mut views {
        if let Some(o) = v.as_object_mut() {
            o.remove("feedback_state");
        }
    }
    Ok(views)
}

/// [`thread_views`] as `viewer` reads them: each `feedback_state` its
/// [`clax_core::Delivery::view`].
pub fn thread_views_as(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
    viewer: &DeliveryViewer,
) -> clax_core::Result<Vec<Value>> {
    Ok(views_and_senders(st, threads, codex_push, with_path, viewer)?.0)
}

/// Each thread view's batch senders, in `sends` order
/// ([`clax_core::store::batches::ThreadSend::sender`], never served).
type Senders = Vec<Vec<Option<String>>>;

/// Each thread view's owner agent, when it has one.
type Owners = Vec<Option<ThreadOwner>>;

/// [`thread_views_as`], with each view's batch senders and owner.
fn views_and_senders(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
    viewer: &DeliveryViewer,
) -> clax_core::Result<(Vec<Value>, Senders, Owners)> {
    let extras = st.thread_extras(threads, codex_push)?;
    let senders: Senders = extras
        .iter()
        .map(|x| x.sends.iter().map(|b| b.sender.clone()).collect())
        .collect();
    let owners: Owners = extras.iter().map(|x| x.owner.clone()).collect();
    // Each owner's person's name, once per person (§11 "Display").
    let mut person_names: HashMap<Option<String>, String> = HashMap::new();
    for o in owners.iter().flatten() {
        if !person_names.contains_key(&o.person) {
            let p = st.agent_person(o.person.as_deref())?;
            person_names.insert(o.person.clone(), p.name);
        }
    }
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
            v["feedback_state"] = json!(x.delivery.as_ref().and_then(|d| d.view(viewer)));
            v["addressed_in"] = json!(x.addressed_in);
            v["sends"] = json!(x.sends);
            v["resolved_by_name"] = json!(x.resolved_by_name);
            v["addressed_pending"] = match x.addressed_pending {
                Some((harness, at)) => json!({"harness": harness, "at": at}),
                None => Value::Null,
            };
            // The owner's handle only through [`Vis`], to those it may name.
            if let Some(o) = &x.owner {
                v["owner"] = json!({"name": o.name, "person": o.person, "live": o.live,
                    "person_name": person_names.get(&o.person)});
            }
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
        .collect::<clax_core::Result<Vec<Value>>>()
        .map(|views| (views, senders, owners))
}

/// The display names of agents' people, each read once (`None`: the
/// owner), for agent reply author lines.
struct AgentPeople<'a> {
    st: &'a Store,
    names: HashMap<Option<String>, String>,
}

impl<'a> AgentPeople<'a> {
    fn new(st: &'a Store) -> AgentPeople<'a> {
        AgentPeople {
            st,
            names: HashMap::new(),
        }
    }

    fn name(&mut self, person: Option<&str>) -> clax_core::Result<String> {
        let key = person.map(str::to_string);
        if let Some(n) = self.names.get(&key) {
            return Ok(n.clone());
        }
        let n = self.st.agent_person(person)?.name;
        self.names.insert(key, n.clone());
        Ok(n)
    }
}

/// Comment `c`'s view `view` as an agent reads it: a viewer's name says
/// `(not connected)` when the viewer is not (spec §9.6, §19 item 7); an
/// agent's reply from a named session reads `<name> (agent, via
/// <harness>)` when the agent is the reader's own person's, else `<name>
/// (<person>'s agent, via <harness>)` (spec §11 "Display", §19 item 1),
/// and one from a session with no name keeps its stored name.
fn mark_comment(
    vis: &Visibility,
    people: &mut AgentPeople<'_>,
    c: &Comment,
    view: &mut Value,
) -> clax_core::Result<()> {
    if c.author_kind == clax_core::store::threads::AUTHOR_AGENT {
        if let (Some(name), Some(harness)) = (&c.agent_name, &c.via_harness) {
            let reader = match vis.reader() {
                Some(Reader::Person(p)) => Some(p.as_str()),
                _ => None,
            };
            let person = if c.agent_person.as_deref() == reader {
                None
            } else {
                Some(people.name(c.agent_person.as_deref())?)
            };
            view["author_name"] = json!(clax_core::feedback::agent_author(
                name,
                person.as_deref(),
                harness
            ));
        }
        return Ok(());
    }
    let author = Author::of_viewer(c.author_public_id.as_deref());
    if let Some(name) = view["author_name"].as_str() {
        view["author_name"] = json!(vis.label(name, &author));
    }
    Ok(())
}

/// Thread `t`'s view `view` as an agent reads it: every name of an author
/// who is not connected says so (spec §9.6): its viewers' comments, its
/// batches' senders (`senders`, in `sends` order) and the viewer who
/// resolved it; its agents' replies name their agents ([`mark_comment`]).
fn mark_thread(
    vis: &Visibility,
    people: &mut AgentPeople<'_>,
    t: &Thread,
    view: &mut Value,
    senders: &[Option<String>],
) -> clax_core::Result<()> {
    for (i, c) in t.comments.iter().enumerate() {
        mark_comment(vis, people, c, &mut view["comments"][i])?;
    }
    for (i, sender) in senders.iter().enumerate() {
        let send = &mut view["sends"][i];
        if let Some(name) = send["sent_by"].as_str() {
            let mut name = name.to_string();
            for a in visibility::authors_of_batch_note(sender.as_deref()) {
                name = vis.label(&name, &a);
            }
            send["sent_by"] = json!(name);
        }
    }
    if let (Some(by), Some(name)) = (
        t.resolved_by
            .as_deref()
            .filter(|b| b.starts_with("viewer:")),
        view["resolved_by_name"].as_str(),
    ) {
        let name = match visibility::authors_of_resolved_by(by, None).first() {
            Some(a) => vis.label(name, a),
            None => name.to_string(),
        };
        view["resolved_by_name"] = json!(name);
    }
    Ok(())
}

/// Viewer views `people` (each with its public ID at `id_key` and its
/// `display_name`) as an agent reads them: each named viewer who is not
/// connected says so (spec §9.6).
fn mark_viewers(vis: &Visibility, people: &mut Value, id_key: &str) {
    let Some(people) = people.as_array_mut() else {
        return;
    };
    for p in people {
        if let (Some(id), Some(name)) = (p[id_key].as_str(), p["display_name"].as_str()) {
            let name = vis.label(name, &Author::of_viewer(Some(id)));
            p["display_name"] = json!(name);
        }
    }
}

/// An artifact view's `participants` as an agent reads them.
fn mark_participants(vis: &Visibility, view: &mut Value) {
    mark_viewers(vis, &mut view["participants"]["people"], "public_id");
    // Agents' people by their payload-safe names.
    if let Some(agents) = view["participants"]["agents"].as_array_mut() {
        for a in agents {
            if let Some(n) = a["person"]["name"].as_str().map(str::to_string) {
                a["person"]["name"] = json!(clax_core::feedback::display_name(&n));
            }
        }
    }
}

/// The visibility an agent channel's response goes through (spec §8):
/// the caller's reader and the visibility generation read when the
/// request began, before the store reads that build the view (M17).
#[derive(Clone, Debug)]
pub struct VisFor {
    pub reader: Option<Reader>,
    pub generation: u64,
    /// For an agent channel, its session (`None` when it names none):
    /// thread views say `taken_by` when another live agent owns one.
    pub agent: Option<Option<String>>,
    /// Whose agents' handles thread views name in `owner`.
    pub handles: Handles,
    /// How thread views read their delivery state ([`clax_core::Delivery::view`]).
    pub viewer: DeliveryViewer,
}

/// How `caller` reads threads' delivery states (spec 2026-10-08 §10,
/// §18): the owner by hand or in a browser as the owner, a connected
/// person's browser as that person, an agent as its session (every row
/// is decided at [`VisFor::build`]), anyone else as a visitor.
pub fn delivery_viewer(caller: &crate::auth::Caller) -> DeliveryViewer {
    use crate::auth::Caller;
    match caller {
        Caller::Owner { .. } => DeliveryViewer::Owner,
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => DeliveryViewer::Agent {
            session: caller.session().map(str::to_string),
            every: false,
        },
        Caller::Person { viewer } => DeliveryViewer::Person(viewer.public_id.clone()),
        Caller::Viewer { .. } | Caller::Anonymous => DeliveryViewer::Visitor,
    }
}

/// Whose agents' handles a caller's thread views name (spec §13: other
/// people's agents by name and state only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handles {
    /// Every agent's: the owner by hand or in a browser.
    All,
    /// The agents of this person (`None`: the owner's agents).
    Of(Option<String>),
    /// No agent's: LAN viewers and anonymous callers.
    None,
}

impl Handles {
    /// The handles `caller` may be shown.
    pub fn of(caller: &crate::auth::Caller) -> Handles {
        use crate::auth::Caller;
        match caller {
            Caller::Owner { .. } => Handles::All,
            Caller::OwnerAgent { .. } => Handles::Of(None),
            Caller::PersonAgent { person, .. } => Handles::Of(person.clone()),
            Caller::Person { viewer } => Handles::Of(Some(viewer.public_id.clone())),
            Caller::Viewer { .. } | Caller::Anonymous => Handles::None,
        }
    }

    fn shows(&self, person: &Option<String>) -> bool {
        match self {
            Handles::All => true,
            Handles::Of(p) => p == person,
            Handles::None => false,
        }
    }
}

impl VisFor {
    /// What `caller` may see: agent channels are filtered for their
    /// reader; people are not (§8.5).
    pub fn of(caller: &crate::auth::Caller, s: &AppState) -> VisFor {
        VisFor {
            reader: caller.reader(),
            generation: s.vis_gen.current(),
            agent: caller
                .is_agent()
                .then(|| caller.session().map(str::to_string)),
            handles: Handles::of(caller),
            viewer: delivery_viewer(caller),
        }
    }

    /// The view, read from `st`.
    ///
    /// # Errors
    /// The store's.
    pub fn build(&self, st: &Store) -> clax_core::Result<Vis> {
        Ok(Vis {
            vis: match &self.reader {
                Some(r) => Some(Visibility::for_reader(st, r, self.generation)?),
                None => None,
            },
            agent: self.agent.clone(),
            handles: self.handles.clone(),
            viewer: match &self.viewer {
                // While nothing can be held, every agent reads every
                // thread's state, as before people existed (§19).
                DeliveryViewer::Agent { session, .. } => DeliveryViewer::Agent {
                    session: session.clone(),
                    every: st.owner_sees_all()?,
                },
                v => v.clone(),
            },
        })
    }
}

/// What an agent channel may see of the threads a route returns, field by
/// field (spec §8.2, §8.3, M10); for people ([`VisFor`] without a reader)
/// and for a view that sees everything, the views as they are.
pub struct Vis {
    vis: Option<Visibility>,
    /// For an agent channel, its session, if it names one.
    agent: Option<Option<String>>,
    handles: Handles,
    viewer: DeliveryViewer,
}

impl Vis {
    /// The filtering view, unless everything is visible.
    fn filtering(&self) -> Option<&Visibility> {
        self.vis.as_ref().filter(|v| !v.is_all())
    }

    /// Viewer views `people` (an array of objects with the viewer's public
    /// ID at `id_key` and its `display_name`) as the caller reads them: for
    /// an agent channel, each viewer who is not connected named `<name>
    /// (not connected)` (spec §9.6, §19 item 7); for people, as they are.
    pub fn mark_viewers(&self, people: &mut Value, id_key: &str) {
        if let Some(vis) = &self.vis {
            mark_viewers(vis, people, id_key);
        }
    }

    /// Whether anything may be withheld from the reader.
    pub fn filters(&self) -> bool {
        self.filtering().is_some()
    }

    /// [`thread_views`] of `threads`, each field the reader may not see
    /// replaced by its placeholder ([`Withhold::thread`]). For an agent,
    /// a thread another live agent owns says `taken_by: <its name>` (spec
    /// §12; names are not filtered, §21.9).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_threads(
        &self,
        st: &Store,
        threads: &[Thread],
        codex_push: bool,
        with_path: bool,
    ) -> clax_core::Result<Vec<Value>> {
        let (mut views, senders, owners) =
            views_and_senders(st, threads, codex_push, with_path, &self.viewer)?;
        for (view, owner) in views.iter_mut().zip(&owners) {
            if let Some(o) = owner
                && self.handles.shows(&o.person)
            {
                view["owner"]["handle"] = json!(o.handle);
            }
        }
        if let Some(me) = &self.agent {
            // A person's name in an agent's payload is the payload-safe one.
            for view in views.iter_mut() {
                if let Some(n) = view["owner"]["person_name"].as_str().map(str::to_string) {
                    view["owner"]["person_name"] = json!(clax_core::feedback::display_name(&n));
                }
            }
            for (view, owner) in views.iter_mut().zip(&owners) {
                if let Some(o) = owner
                    && o.live
                    && me.as_deref() != Some(o.session_id.as_str())
                {
                    view["taken_by"] = json!(o.display());
                }
            }
        }
        if let Some(vis) = self.filtering() {
            let mut w = Withhold::new(st, vis);
            for ((t, view), senders) in threads.iter().zip(&mut views).zip(&senders) {
                w.thread(t, view, senders)?;
            }
        }
        if let Some(vis) = &self.vis {
            let mut people = AgentPeople::new(st);
            for ((t, view), senders) in threads.iter().zip(&mut views).zip(&senders) {
                mark_thread(vis, &mut people, t, view, senders)?;
            }
        }
        Ok(views)
    }

    /// [`Vis::filter_threads`] of one thread.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_thread(
        &self,
        st: &Store,
        t: &Thread,
        codex_push: bool,
        with_path: bool,
    ) -> clax_core::Result<Value> {
        Ok(self
            .filter_threads(st, std::slice::from_ref(t), codex_push, with_path)?
            .pop()
            .expect("one view per thread"))
    }

    /// Comment `c` of a thread of artifact `aid`, its body withheld and
    /// its author named as in its thread's view.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_comment(&self, st: &Store, aid: &str, c: &Comment) -> clax_core::Result<Value> {
        let mut v = json!(c);
        if let Some(vis) = &self.vis {
            mark_comment(vis, &mut AgentPeople::new(st), c, &mut v)?;
        }
        if let Some(vis) = self.filtering()
            && !vis.sees(&visibility::authors_of_comment(c), aid, Some(&c.thread_id))
        {
            placeholder::comment(&mut v);
        }
        Ok(v)
    }

    /// The authors withholding thread `t`'s clip from the reader, `None`
    /// when it may see it (or the thread has none).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_clip(
        &self,
        st: &Store,
        t: &Thread,
    ) -> clax_core::Result<Option<Vec<WithheldAuthor>>> {
        let Some(vis) = self.filtering() else {
            return Ok(None);
        };
        if !t.has_clip {
            return Ok(None);
        }
        let mut w = Withhold::new(st, vis);
        let withheld = match w.version(&t.artifact_id, t.version_n)? {
            Some(v) => vis.withheld_authors(
                &visibility::authors_of_clip(t, &v),
                &t.artifact_id,
                Some(&t.id),
            ),
            None => Vec::new(),
        };
        let unknown = w.version(&t.artifact_id, t.version_n)?.is_none();
        if withheld.is_empty() && !unknown {
            return Ok(None);
        }
        Ok(Some(w.named(&withheld)?))
    }

    /// An entry of the all-threads listing for artifact `a`, whose
    /// `files` are `version`'s: its `title` null with `withheld: {authors}`
    /// when the artifact's text is withheld, and each file the reader may
    /// not see left out of `files`, counted in `files_withheld`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_listing_entry(
        &self,
        st: &Store,
        a: &Artifact,
        version: Option<&Version>,
        entry: &mut Value,
    ) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let mut w = Withhold::new(st, vis);
        let text = vis.withheld_authors(
            &visibility::authors_of_artifact_field(a, "title"),
            &a.id,
            None,
        );
        if !text.is_empty() {
            entry["title"] = Value::Null;
            entry["withheld"] = json!({"authors": w.named(&text)?});
        }
        let mut hidden = Vec::new();
        if let Some(v) = version {
            for path in v.files.keys() {
                if !vis.sees(&visibility::authors_of_file(v, path), &a.id, None) {
                    hidden.push(path.clone());
                }
            }
        }
        if let Some(files) = entry.get_mut("files").and_then(Value::as_array_mut) {
            files.retain(|f| !hidden.iter().any(|h| f.as_str() == Some(h)));
        }
        entry["files_withheld"] = json!(hidden.len());
        Ok(())
    }

    /// A question's view `view` (crate::questions::view) as its asking
    /// agent sees it: the question, its answers and its agent are the
    /// agent's person's own (spec §8.2); its artifact's title is the
    /// title's authors', null with `withheld: {authors}` beside it in
    /// `artifact` when the reader may not see it.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_question(&self, st: &Store, view: &mut Value) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let Some(aid) = view["artifact"]["id"].as_str() else {
            return Ok(());
        };
        let Some(a) = st.get_artifact(&ArtifactId::parse(aid)?)? else {
            return Ok(());
        };
        let withheld = vis.withheld_authors(
            &visibility::authors_of_artifact_field(&a, "title"),
            &a.id,
            None,
        );
        if !withheld.is_empty() {
            let named = Withhold::new(st, vis).named(&withheld)?;
            view["artifact"]["title"] = Value::Null;
            view["artifact"]["withheld"] = json!({"authors": named});
        }
        Ok(())
    }

    /// An approval's view `view` ([`crate::approvals::approval_view`]) as
    /// the reader may see it: its page's title is the title's authors',
    /// null when the reader may not see it (spec §7.5, §8.2).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_approval(&self, st: &Store, view: &mut Value) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        if view["artifact_title"].is_null() {
            return Ok(());
        }
        let Some(aid) = view["artifact_id"].as_str() else {
            return Ok(());
        };
        let hidden = match st.get_artifact(&ArtifactId::parse(aid)?)? {
            Some(a) => !vis
                .withheld_authors(
                    &visibility::authors_of_artifact_field(&a, "title"),
                    &a.id,
                    None,
                )
                .is_empty(),
            None => true,
        };
        if hidden {
            view["artifact_title"] = Value::Null;
        }
        Ok(())
    }

    /// An artifact's view `view` (its JSON, with or without the fields of
    /// `with_owner`): each of its text fields (title, description, icon,
    /// `capabilities`) the reader may not see null, with `withheld:
    /// {authors}` naming whose they are; and, where the view embeds
    /// `working`, `working`'s records ([`Vis::filter_working`]) in its
    /// place.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_artifact(
        &self,
        st: &Store,
        a: &Artifact,
        working: &[WorkingView],
        view: &mut Value,
    ) -> clax_core::Result<()> {
        if let Some(vis) = &self.vis {
            mark_participants(vis, view);
        }
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let mut w = Withhold::new(st, vis);
        w.artifact(a, view)?;
        if view.get("working").is_some() {
            view["working"] = json!(w.working(&a.id, working)?);
        }
        Ok(())
    }

    /// `records` of artifact `aid` as views list them, each message the
    /// reader may not see (it is its session's person's) null with
    /// `withheld: true`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_working(
        &self,
        st: &Store,
        aid: &str,
        records: &[WorkingView],
    ) -> clax_core::Result<Vec<Value>> {
        match self.filtering() {
            Some(vis) => Withhold::new(st, vis).working(aid, records),
            None => Ok(records.iter().map(|r| json!(r)).collect()),
        }
    }

    /// The working roster's `records`, each message the reader may not see
    /// null with `withheld: true`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_roster(
        &self,
        st: &Store,
        records: &[SessionWorking],
    ) -> clax_core::Result<Vec<Value>> {
        let Some(vis) = self.filtering() else {
            return Ok(records.iter().map(|r| json!(r)).collect());
        };
        let mut w = Withhold::new(st, vis);
        records
            .iter()
            .map(|r| {
                let mut v = json!(r);
                if w.hides_working(&r.artifact_id, &r.session_id)? {
                    placeholder::working(&mut v);
                }
                Ok(v)
            })
            .collect()
    }

    /// Version `v`'s view `view`: its label and note null when the reader
    /// may not see them, and each file it may not see left out of `files`,
    /// counted in `files_withheld` (with the version's `content_sha256`
    /// null when any is).
    pub fn filter_version(&self, v: &Version, view: &mut Value) {
        let Some(vis) = self.filtering() else {
            return;
        };
        if !vis.sees(&visibility::authors_of_version(v), &v.artifact_id, None) {
            placeholder::version_text(view);
        }
        let hidden = withheld_files(vis, v);
        placeholder::version_files(view, &hidden);
    }

    /// A view of version `v`'s `files` (`{files, version}`): each file the
    /// reader may not see left out, counted in `files_withheld`.
    pub fn filter_files(&self, v: &Version, view: &mut Value) {
        if let Some(vis) = self.filtering() {
            placeholder::version_files(view, &withheld_files(vis, v));
        }
    }

    /// A publish's answer: artifact `a` and its new version `v` as JSON,
    /// filtered ([`Vis::filter_artifact`], [`Vis::filter_version`]): the
    /// publisher's reader sees no file it carried forward from someone it
    /// has not approved (spec §7.5, M11).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_published(
        &self,
        st: &Store,
        a: &Artifact,
        v: &Version,
    ) -> clax_core::Result<(Value, Value)> {
        let mut artifact = json!(a);
        self.filter_artifact(st, a, &[], &mut artifact)?;
        let mut version = json!(v);
        self.filter_version(v, &mut version);
        Ok((artifact, version))
    }

    /// The paths of version `v`'s files the reader may not see.
    pub fn filter_hidden_files(&self, v: &Version) -> Vec<String> {
        match self.filtering() {
            Some(vis) => withheld_files(vis, v)
                .into_iter()
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        }
    }

    /// [`Vis::filter_version`] of each of `versions`, as views.
    pub fn filter_versions(&self, versions: &[Version]) -> Vec<Value> {
        versions
            .iter()
            .map(|v| {
                let mut view = json!(v);
                self.filter_version(v, &mut view);
                view
            })
            .collect()
    }

    /// The authors withholding file `path` of version `v` from the reader,
    /// `None` when it may read it. So that the answer tells nothing of a
    /// name the reader guessed, a file that is withheld and one the
    /// version lacks answer alike while the version withholds any file:
    /// with every author of its withheld files. A version that withholds
    /// nothing withholds nothing here either (a missing file is then
    /// missing).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_file_content(
        &self,
        st: &Store,
        v: &Version,
        path: &str,
    ) -> clax_core::Result<Option<Vec<WithheldAuthor>>> {
        let Some(vis) = self.filtering() else {
            return Ok(None);
        };
        let hidden = withheld_files(vis, v);
        if hidden.is_empty() || (v.files.contains_key(path) && !hidden.contains(&path)) {
            return Ok(None);
        }
        let mut authors = Vec::new();
        for p in hidden {
            for a in vis.withheld_authors(&visibility::authors_of_file(v, p), &v.artifact_id, None)
            {
                if !authors.contains(&a) {
                    authors.push(a);
                }
            }
        }
        Ok(Some(Withhold::new(st, vis).named(&authors)?))
    }

    /// Whether an anchor on `file` of version `v` must answer as one on a
    /// file the version lacks: the reader may not see the file's name.
    /// Every version has its `index.html`, so an anchor there tells
    /// nothing and is never refused.
    pub fn filter_anchor_file(&self, v: &Version, file: &str) -> bool {
        let Some(vis) = self.filtering() else {
            return false;
        };
        file != clax_core::publish::INDEX
            && v.files.contains_key(file)
            && !vis.sees(&visibility::authors_of_file(v, file), &v.artifact_id, None)
    }

    /// The view a store read evaluates with, when anything may be
    /// withheld: a query matches and orders only what it lets through
    /// ([`Store::doc_query_visible`]).
    pub fn view(&self) -> Option<&Visibility> {
        self.filtering()
    }

    /// Document `doc` of artifact `aid`, whose authors are `authors`: its
    /// view, or when the reader may not see it the placeholder of a
    /// document read by its path, `{doc_id, version, withheld: {authors}}`
    /// without its data (spec §8.3).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_doc(
        &self,
        st: &Store,
        aid: &str,
        doc: &Doc,
        authors: &[Author],
    ) -> clax_core::Result<Value> {
        if let Some(vis) = self.filtering() {
            let withheld = vis.withheld_authors(authors, aid, None);
            if !withheld.is_empty() {
                let named = Withhold::new(st, vis).named(&withheld)?;
                return Ok(placeholder::document(&doc.id, doc.version, &named));
            }
        }
        Ok(json!(doc))
    }

    /// The document a write of artifact `aid` left, as
    /// [`Vis::filter_doc`] shows it; `None` after a delete.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_written(
        &self,
        st: &Store,
        aid: &str,
        w: &Written,
    ) -> clax_core::Result<Option<Value>> {
        w.doc
            .as_ref()
            .map(|d| self.filter_doc(st, aid, d, &w.authors))
            .transpose()
    }

    /// A query's page as the list route answers it: `{docs, next_cursor}`,
    /// with `withheld_count` (the collection's documents left out,
    /// whatever the filter) when the reader may not see every document.
    /// The store matched, ordered and paged only what the reader may see.
    pub fn filter_doc_page(&self, page: DocPage) -> Value {
        let mut v = json!({"docs": page.docs, "next_cursor": page.next_cursor});
        if let Some(n) = page.withheld {
            v["withheld_count"] = json!(n);
        }
        v
    }

    /// A batch's results, `ops` its operations' names in order: `{op,
    /// path, version, deleted}` each, which hold no document field (the
    /// path is the caller's own), so nothing is withheld.
    pub fn filter_batch(&self, written: &[Written], ops: Vec<String>) -> Vec<Value> {
        written
            .iter()
            .zip(ops)
            .map(|(w, op)| json!({"op": op, "path": w.path, "version": w.doc.as_ref().map(|d| d.version), "deleted": w.deleted}))
            .collect()
    }

    /// A lease's answer: whether it was granted, the document's version,
    /// the lease's expiry and, on a grant, the caller's own holder. It
    /// holds no document field, so nothing is withheld.
    pub fn filter_acquired(&self, a: &Acquired) -> Value {
        json!(a)
    }

    /// `authors`, withheld from the reader, as views name them.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_authors(
        &self,
        st: &Store,
        authors: &[Author],
    ) -> clax_core::Result<Vec<WithheldAuthor>> {
        match self.filtering() {
            Some(vis) => Withhold::new(st, vis).named(authors),
            None => Ok(Vec::new()),
        }
    }

    /// `assets` as views list them, each the reader may not see with its
    /// name and what its bytes decide null, and `withheld: true`.
    pub fn filter_assets(&self, assets: &[Asset]) -> Vec<Value> {
        assets
            .iter()
            .map(|a| {
                let mut v = json!(a);
                if let Some(vis) = self.filtering()
                    && !vis.sees(&visibility::authors_of_asset(a), &a.artifact_id, None)
                {
                    placeholder::asset(&mut v);
                }
                v
            })
            .collect()
    }
}

/// The paths of version `v`'s files `vis` withholds, in order.
fn withheld_files<'a>(vis: &Visibility, v: &'a Version) -> Vec<&'a str> {
    v.files
        .keys()
        .filter(|p| !vis.sees(&visibility::authors_of_file(v, p), &v.artifact_id, None))
        .map(String::as_str)
        .collect()
}

/// Applies one reader's view to thread views, reading what it needs once:
/// the versions anchors are on, the names of withheld authors.
struct Withhold<'a> {
    st: &'a Store,
    vis: &'a Visibility,
    versions: HashMap<(String, u32), Option<Version>>,
    names: HashMap<Author, WithheldAuthor>,
    /// Each session's person, as read (`None` inside: the owner's).
    persons: HashMap<String, Option<Option<String>>>,
}

impl<'a> Withhold<'a> {
    fn new(st: &'a Store, vis: &'a Visibility) -> Withhold<'a> {
        Withhold {
            st,
            vis,
            versions: HashMap::new(),
            names: HashMap::new(),
            persons: HashMap::new(),
        }
    }

    /// Whether the reader may not see the message of a working record of
    /// session `sid` on `aid`: its session's person's words. A session
    /// that is gone names no one: its message is withheld.
    fn hides_working(&mut self, aid: &str, sid: &str) -> clax_core::Result<bool> {
        if !self.persons.contains_key(sid) {
            let p = self.st.get_session(sid)?.map(|s| s.person);
            self.persons.insert(sid.to_string(), p);
        }
        Ok(match &self.persons[sid] {
            Some(person) => !self.vis.sees(
                &visibility::authors_of_working(person.as_deref()),
                aid,
                None,
            ),
            None => true,
        })
    }

    /// `records` of `aid` as views, each message withheld as
    /// [`Withhold::hides_working`] says.
    fn working(&mut self, aid: &str, records: &[WorkingView]) -> clax_core::Result<Vec<Value>> {
        records
            .iter()
            .map(|r| {
                let mut v = json!(r);
                if self.hides_working(aid, &r.session_id)? {
                    placeholder::working(&mut v);
                }
                Ok(v)
            })
            .collect()
    }

    /// Artifact `a`'s view: each text field the reader may not see null,
    /// and `withheld: {authors}` when any is.
    fn artifact(&mut self, a: &Artifact, view: &mut Value) -> clax_core::Result<()> {
        let mut fields = Vec::new();
        let mut withheld: Vec<Author> = Vec::new();
        for f in clax_core::store::artifacts::TEXT_FIELDS {
            let w = self.vis.withheld_authors(
                &visibility::authors_of_artifact_field(a, f),
                &a.id,
                None,
            );
            if !w.is_empty() {
                fields.push(f);
                for x in w {
                    if !withheld.contains(&x) {
                        withheld.push(x);
                    }
                }
            }
        }
        if !fields.is_empty() {
            let named = self.named(&withheld)?;
            placeholder::artifact(view, &fields, &named);
        }
        Ok(())
    }

    fn version(&mut self, aid: &str, n: u32) -> clax_core::Result<Option<Version>> {
        let key = (aid.to_string(), n);
        if !self.versions.contains_key(&key) {
            let v = self.st.get_version(&ArtifactId::parse(aid)?, n)?;
            self.versions.insert(key.clone(), v);
        }
        Ok(self.versions[&key].clone())
    }

    /// `authors` as views list them: public ID (none for anonymous
    /// viewers) and current display name.
    fn named(&mut self, authors: &[Author]) -> clax_core::Result<Vec<WithheldAuthor>> {
        let mut out = Vec::with_capacity(authors.len());
        for a in authors {
            if !self.names.contains_key(a) {
                let viewer = match a {
                    Author::Owner => self.st.owner()?,
                    Author::Person(p) => self.st.viewer_by_public_id(p)?,
                    Author::Unconnected | Author::System => None,
                };
                let named = WithheldAuthor {
                    public_id: match a {
                        Author::Person(p) => Some(p.clone()),
                        _ => viewer.as_ref().map(|v| v.public_id.clone()),
                    },
                    name: self.vis.label(
                        &clax_core::feedback::person_name(
                            viewer.as_ref().and_then(|v| v.display_name.as_deref()),
                            *a == Author::Owner,
                        ),
                        a,
                    ),
                };
                self.names.insert(a.clone(), named);
            }
            out.push(self.names[a].clone());
        }
        Ok(out)
    }

    /// Where the reader's approval of `authors` stands at thread `tid` of
    /// artifact `aid`: an open request for any of them (at the thread or
    /// the whole artifact) is `requested`, else a declined one `declined`.
    fn approval(
        &mut self,
        aid: &str,
        tid: &str,
        authors: &[Author],
    ) -> clax_core::Result<ApprovalState> {
        let approver = match self.vis.reader() {
            Some(Reader::Person(p)) => Some(p.clone()),
            _ => None,
        };
        let mut keys = Vec::new();
        for a in authors {
            match a {
                Author::Owner => {
                    if let Some(o) = self.st.owner()? {
                        keys.push(o.public_id);
                    }
                }
                Author::Person(p) => keys.push(p.clone()),
                Author::Unconnected => keys.push(UNCONNECTED.to_string()),
                Author::System => {}
            }
        }
        self.st.request_state(approver.as_deref(), aid, tid, &keys)
    }

    /// Thread `t`'s view, `senders` its batches' senders in `sends` order:
    /// each comment body, the anchor, the clip, `resolved_by` and each
    /// batch note the reader may not see is replaced by its placeholder
    /// (§8.3), and when any is, the thread gains `withheld: {count (its
    /// comments withheld), authors, approval}`.
    fn thread(
        &mut self,
        t: &Thread,
        view: &mut Value,
        senders: &[Option<String>],
    ) -> clax_core::Result<()> {
        let vis = self.vis;
        let (aid, tid) = (t.artifact_id.as_str(), Some(t.id.as_str()));
        let mut withheld: Vec<Author> = Vec::new();
        let mut any = false;
        let mut hides = |authors: &[Author]| {
            let w = vis.withheld_authors(authors, aid, tid);
            for a in &w {
                if !withheld.contains(a) {
                    withheld.push(a.clone());
                }
            }
            !w.is_empty()
        };
        let mut count = 0;
        for (i, c) in t.comments.iter().enumerate() {
            if hides(&visibility::authors_of_comment(c)) {
                placeholder::comment(&mut view["comments"][i]);
                count += 1;
                any = true;
            }
        }
        // A version that is gone names no authors: withhold what it held.
        let version = self.version(aid, t.version_n)?;
        let (anchor, clip) = match &version {
            Some(v) => (
                hides(&visibility::authors_of_anchor(t, v)),
                t.has_clip && hides(&visibility::authors_of_clip(t, v)),
            ),
            None => (!t.anchor.is_whole(), t.has_clip),
        };
        if anchor {
            placeholder::anchor(view);
            any = true;
        }
        if clip {
            placeholder::clip(view);
            any = true;
        }
        if let Some(by) = &t.resolved_by
            && hides(&visibility::authors_of_resolved_by(
                by,
                t.resolved_by_person.as_deref(),
            ))
        {
            placeholder::resolved_by(view);
            any = true;
        }
        for (i, sender) in senders.iter().enumerate() {
            let note = &view["sends"][i]["note"];
            if !note.is_null() && hides(&visibility::authors_of_batch_note(sender.as_deref())) {
                placeholder::batch_note(&mut view["sends"][i]);
                any = true;
            }
        }
        if any {
            let authors = self.named(&withheld)?;
            let approval = self.approval(aid, &t.id, &withheld)?;
            placeholder::thread_summary(view, count, &authors, approval);
        }
        Ok(())
    }
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
