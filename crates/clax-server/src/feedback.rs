//! Fan-out of feedback changes (`feedback_state` events and long-poll
//! wake-ups), the JSON view of a thread, and what an agent channel may see
//! of it ([`Vis`]).

use crate::state::AppState;
#[cfg(debug_assertions)]
use clax_core::feedback::Tier;
use clax_core::feedback::Touched;
use clax_core::model::{Artifact, Comment, Thread, Version};
use clax_core::store::approvals::UNCONNECTED;
use clax_core::store::live::LivePage;
use clax_core::visibility::placeholder::{self, ApprovalState, WithheldAuthor};
use clax_core::visibility::{self, Author, Reader, Visibility};
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
    Ok(views_and_senders(st, threads, codex_push, with_path)?.0)
}

/// Each thread view's batch senders, in `sends` order
/// ([`clax_core::store::batches::ThreadSend::sender`], never served).
type Senders = Vec<Vec<Option<String>>>;

/// [`thread_views`], with each view's batch senders.
fn views_and_senders(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
) -> clax_core::Result<(Vec<Value>, Senders)> {
    let extras = st.thread_extras(threads, codex_push)?;
    let senders: Senders = extras
        .iter()
        .map(|x| x.sends.iter().map(|b| b.sender.clone()).collect())
        .collect();
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
        .collect::<clax_core::Result<Vec<Value>>>()
        .map(|views| (views, senders))
}

/// The visibility an agent channel's response goes through (spec §8):
/// the caller's reader and the visibility generation read when the
/// request began, before the store reads that build the view (M17).
#[derive(Clone, Debug)]
pub struct VisFor {
    pub reader: Option<Reader>,
    pub generation: u64,
}

impl VisFor {
    /// What `caller` may see: agent channels are filtered for their
    /// reader; people are not (§8.5).
    pub fn of(caller: &crate::auth::Caller, s: &AppState) -> VisFor {
        VisFor {
            reader: caller.reader(),
            generation: s.vis_gen.current(),
        }
    }

    /// The view, read from `st`.
    ///
    /// # Errors
    /// The store's.
    pub fn build(&self, st: &Store) -> clax_core::Result<Vis> {
        Ok(Vis(match &self.reader {
            Some(r) => Some(Visibility::for_reader(st, r, self.generation)?),
            None => None,
        }))
    }
}

/// What an agent channel may see of the threads a route returns, field by
/// field (spec §8.2, §8.3, M10); for people ([`VisFor`] without a reader)
/// and for a view that sees everything, the views as they are.
pub struct Vis(Option<Visibility>);

impl Vis {
    /// The filtering view, unless everything is visible.
    fn filtering(&self) -> Option<&Visibility> {
        self.0.as_ref().filter(|v| !v.is_all())
    }

    /// [`thread_views`] of `threads`, each field the reader may not see
    /// replaced by its placeholder ([`Withhold::thread`]).
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
        let (mut views, senders) = views_and_senders(st, threads, codex_push, with_path)?;
        if let Some(vis) = self.filtering() {
            let mut w = Withhold::new(st, vis);
            for ((t, view), senders) in threads.iter().zip(&mut views).zip(&senders) {
                w.thread(t, view, senders)?;
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

    /// Comment `c` of a thread of artifact `aid`, its body withheld as in
    /// its thread's view.
    pub fn filter_comment(&self, aid: &str, c: &Comment) -> Value {
        let mut v = json!(c);
        if let Some(vis) = self.filtering()
            && !vis.sees(&visibility::authors_of_comment(c), aid, Some(&c.thread_id))
        {
            placeholder::comment(&mut v);
        }
        v
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
        let text = vis.withheld_authors(&visibility::authors_of_artifact_text(a), &a.id, None);
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
}

/// Applies one reader's view to thread views, reading what it needs once:
/// the versions anchors are on, the names of withheld authors.
struct Withhold<'a> {
    st: &'a Store,
    vis: &'a Visibility,
    versions: HashMap<(String, u32), Option<Version>>,
    names: HashMap<Author, WithheldAuthor>,
}

impl<'a> Withhold<'a> {
    fn new(st: &'a Store, vis: &'a Visibility) -> Withhold<'a> {
        Withhold {
            st,
            vis,
            versions: HashMap::new(),
            names: HashMap::new(),
        }
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
                    name: clax_core::feedback::display_name(
                        viewer
                            .as_ref()
                            .and_then(|v| v.display_name.as_deref())
                            .unwrap_or(""),
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
