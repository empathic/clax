//! Linked threads' sync (spec 2026-10-15-linked-threads-design): on a local
//! daemon, one task per remote with at least one linked artifact keeps the
//! artifacts' comment threads in step with their public twins' there.
//!
//! - **Transport.** The task acts with the agent token the remote's device
//!   sign-in left here and nothing else, over the remote's REST routes, in a
//!   session of its own there (the routes the hosted `/mcp` tools call): it
//!   watches the linked public artifacts and long-polls the session's
//!   feedback (`wait_for_feedback`'s route) for liveness. Each wake-up (a
//!   feedback answer, or a local write on a linked artifact) reads the
//!   threads changed since the link's cursor; every [`BACKSTOP`] it reads
//!   them all, which also catches reopenings and approvals that let more
//!   through.
//! - **Each cycle** queues the local writes not queued yet
//!   ([`clax_store::linked`]), sends the outbox in order (each thread's
//!   rows in turn; a thread whose row is held or refused waits, others go
//!   on), then writes here what changed there: new threads with their
//!   anchors translated through the push records, new comments, status
//!   changes, and a send there becomes a send here, so local agents get what
//!   a watch covers, or what someone sent to the owner's agents.
//! - **Up-sends** carry an idempotency key made here: a resend after a lost
//!   answer answers what the first made. A local agent's reply goes up as
//!   the token's person's agent, in a session named as the local agent; on
//!   a thread not sent to an agent there (which takes no agent reply) it
//!   goes up as the person. A person's comment and a new local thread go up
//!   as the person. A local thread on a version never pushed waits for that
//!   version to be pushed (`held`); one on the whole artifact goes up on the
//!   public copy's latest.
//! - **Clips.** A mirrored thread's clip is fetched over the remote's clip
//!   route, with the token, by the path this daemon builds from the public
//!   artifact and thread IDs, and cached under `<home>/remote-cache/clips/`
//!   by the local thread ID. Nothing a remote sends is used as a path.
//! - **Offline.** An unreachable remote backs the task off, doubling from
//!   [`BACKOFF_FIRST`] to [`BACKOFF_CAP`]; [`Linked::status`] says since
//!   when, and once the backoff reaches its cap the waiting rows are marked
//!   not sent, for Retry. A row a try may have made there (its answer
//!   lost) is resent under its key as soon as the remote answers, before
//!   anything is read from there. Local work never waits on the remote.
//!
//! - **Questions.** An `ask` question of the owner's agents about a linked
//!   artifact is asked there too, while the link is two-way: in the sync's
//!   session, against the public copy, under an Idempotency-Key made here
//!   ([`clax_store::linked`]'s `linked_questions`), so a resend after
//!   a lost answer answers the copy the first made. Each cycle reads each
//!   open copy there (by the question route, which a keyed question's
//!   token may read from any of its sessions): an answer or decline there
//!   answers or declines the question here, so the waiting `ask` returns
//!   it; a copy withdrawn there (its session there ended) while the
//!   question is open here is asked again under a new key. A question
//!   answered, declined or withdrawn here first has its copy withdrawn
//!   there. The remote's answers are checked against the questions, as a
//!   person's here are.
//!
//! The timers run on a [`Sleeper`], which tests drive. A hosted daemon runs
//! none of this.

use crate::public::{Client, RemoteFail};
use clax_core::audit::AuditCtx;
use clax_core::{Anchor, Event};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_realtime::feedback::apply;
use clax_store::SendTarget;
use clax_store::approvals::Route;
use clax_store::linked::{LinkedQuestion, MirrorComment, MirrorThread, OutboxRow, RemoteAuthor};
use clax_store::questions::Close;
use clax_store::remotes::RemoteLink;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Notify;

/// How often each remote's threads are read whole, whatever wakes it.
pub const BACKSTOP: Duration = Duration::from_secs(60);
/// The first wait after the remote could not be reached.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(1);
/// The longest wait between tries; reaching it marks waiting rows not sent.
pub const BACKOFF_CAP: Duration = Duration::from_secs(300);
/// The most pages of 200 threads one read of an artifact takes.
const MAX_PAGES: usize = 50;
/// How long one feedback long-poll waits there, in seconds.
pub const WAIT_S: u64 = 25;
/// The name of the sync's own session there, as its people see it.
const SYNC_NAME: &str = "clax-sync";
/// The harness of the sync's own session there: the by-hand one, as a push
/// by hand uses.
const SYNC_HARNESS: &str = crate::routes::sessions::BY_HAND_HARNESS;

pub use clax_realtime::linked::{Linked, SyncStatus, Task};

/// Starts the sync on a local daemon: a supervisor that keeps one task per
/// remote with a linked artifact, and a listener that wakes a remote's task
/// on a local write on one of its artifacts. A hosted daemon runs none.
pub fn listen(s: &AppState) {
    if s.hosted.is_some() || s.linked.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let st = s.clone();
    clax_rt::spawn(async move {
        loop {
            supervise(&st).await;
            st.linked.supervise.notified().await;
        }
    });
    let st = s.clone();
    let mut rx = s.events.subscribe();
    clax_rt::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    st.linked.kick(None);
                    continue;
                }
                Err(_) => return,
            };
            // An agent starting, ending or watching a linked artifact
            // changes whom its watch there names (`local_relay`).
            if let Event::Agents { artifacts } = &ev.event {
                let rids: HashSet<String> = {
                    let linked = st
                        .linked
                        .artifacts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    artifacts
                        .iter()
                        .filter_map(|a| linked.get(a))
                        .flatten()
                        .cloned()
                        .collect()
                };
                for r in rids {
                    st.linked.kick(Some(&r));
                }
                continue;
            }
            let aid = match &ev.event {
                Event::Comment { artifact_id, .. }
                | Event::Thread { artifact_id, .. }
                | Event::ThreadResolved { artifact_id, .. } => artifact_id.clone(),
                Event::Question { question, .. } => match question["artifact"]["id"].as_str() {
                    Some(a) => a.to_string(),
                    None => continue,
                },
                _ => continue,
            };
            let rids = st
                .linked
                .artifacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&aid)
                .cloned()
                .unwrap_or_default();
            for r in rids {
                // Queued (and shown pending) now, even while a request to
                // the remote hangs; the task sends it.
                if scan(&st, &r).await {
                    publish_outbox(&st, &r).await;
                }
                st.linked.kick(Some(&r));
            }
        }
    });
}

/// Queues remote `rid`'s local writes and questions not queued yet;
/// whether it queued any.
async fn scan(s: &AppState, rid: &str) -> bool {
    let r = rid.to_string();
    s.store_call(move |st| Ok(st.scan_linked(&r)? + st.scan_linked_questions(&r)?))
        .await
        .is_ok_and(|n| n > 0)
}

/// Starts a task for each connected remote with a link, and stops the
/// others.
async fn supervise(s: &AppState) {
    let Ok((links, remotes)) = s
        .store_call(|st| Ok((st.all_remote_links()?, st.remotes()?)))
        .await
    else {
        return;
    };
    let mut want: HashSet<String> = HashSet::new();
    let mut by_artifact: HashMap<String, Vec<String>> = HashMap::new();
    for l in &links {
        if remotes
            .iter()
            .any(|r| r.id == l.remote_id && r.status == "connected")
        {
            want.insert(l.remote_id.clone());
            by_artifact
                .entry(l.artifact_id.clone())
                .or_default()
                .push(l.remote_id.clone());
        }
    }
    *s.linked.artifacts.lock().unwrap_or_else(|e| e.into_inner()) = by_artifact;
    let mut tasks = s.linked.tasks.lock().unwrap_or_else(|e| e.into_inner());
    tasks.retain(|rid, t| {
        let keep = want.contains(rid) && !t.handle.is_finished();
        if !keep {
            t.handle.abort();
        }
        keep
    });
    for rid in want {
        if tasks.contains_key(&rid) {
            continue;
        }
        let (kick, retry) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let handle = clax_rt::spawn(run(s.clone(), rid.clone(), kick.clone(), retry.clone()));
        tasks.insert(
            rid,
            Task {
                kick,
                retry,
                handle,
            },
        );
    }
}

/// Why a cycle stopped.
#[derive(Debug)]
enum Stop {
    /// The remote could not be reached, or answered as a remote down does:
    /// try again after the backoff.
    Unreachable(String),
    /// The remote is gone here, or not signed in: the task ends.
    Gone,
}

impl From<ApiError> for Stop {
    fn from(e: ApiError) -> Stop {
        Stop::Unreachable(format!("{}: {}", e.code, e.message))
    }
}

/// What a failed request to the remote means for the cycle.
fn classify(f: &RemoteFail) -> Option<String> {
    match f.status {
        None => Some(format!("cannot reach {}: {}", f.origin, f.message)),
        Some(401) => Some(format!("{} refused this Clax's agent token", f.origin)),
        Some(s) if s >= 500 || s == 429 => Some(format!("{} answered {s}", f.origin)),
        _ => None,
    }
}

/// One remote's task.
async fn run(s: AppState, rid: String, kick: Arc<Notify>, retry: Arc<Notify>) {
    let mut sync = Sync {
        s: s.clone(),
        rid: rid.clone(),
        client: None,
        watched: HashMap::new(),
        acks: Vec::new(),
        agents: HashMap::new(),
        asked_in: HashMap::new(),
        copy_waits: HashMap::new(),
    };
    let mut full = true;
    let mut backoff = Duration::ZERO;
    loop {
        match sync.cycle(full).await {
            Ok(()) => {
                backoff = Duration::ZERO;
                let now = s.store.now();
                s.linked.set_status(&rid, |st| {
                    st.unreachable_since = None;
                    st.last_error = None;
                    st.synced_at = Some(now);
                });
            }
            Err(Stop::Gone) => {
                sync.end().await;
                return;
            }
            Err(Stop::Unreachable(why)) => {
                sync.client = None;
                sync.watched.clear();
                backoff = if backoff.is_zero() {
                    BACKOFF_FIRST
                } else {
                    (backoff * 2).min(BACKOFF_CAP)
                };
                let now = s.store.now();
                s.linked.set_status(&rid, |st| {
                    st.unreachable_since.get_or_insert(now);
                    st.last_error = Some(why.clone());
                });
                if backoff >= BACKOFF_CAP {
                    let (r2, name) = (rid.clone(), remote_label(&s, &rid).await);
                    let _ = s
                        .store_call(move |st| {
                            let why = format!("Not sent: {name} unreachable");
                            st.outbox_failed_all(&r2, &why)?;
                            st.linked_questions_failed_all(&r2, &why)
                        })
                        .await;
                    publish_outbox(&s, &rid).await;
                }
                // A local write meanwhile is queued (and shown pending)
                // without trying the remote before the backoff ends.
                let wait = s.linked.sleeper.sleep(backoff);
                tokio::pin!(wait);
                loop {
                    tokio::select! {
                        _ = &mut wait => break,
                        _ = retry.notified() => break,
                        _ = kick.notified() => {
                            if scan(&s, &rid).await {
                                publish_outbox(&s, &rid).await;
                            }
                        }
                    }
                }
                full = true;
                continue;
            }
        }
        let poll = sync.poll();
        tokio::select! {
            r = poll => {
                full = false;
                if let Err(why) = r {
                    // The next cycle meets the outage itself.
                    tracing::debug!(remote = rid.as_str(), error = why.as_str(), "feedback poll failed");
                    sync.client = None;
                    sync.watched.clear();
                    tokio::select! {
                        _ = retry.notified() => {}
                        _ = s.linked.sleeper.sleep(BACKOFF_FIRST) => {}
                    }
                }
            }
            _ = kick.notified() => { full = false; }
            _ = s.linked.sleeper.sleep(BACKSTOP) => { full = true; }
        }
    }
}

/// The remote's name, for messages.
async fn remote_label(s: &AppState, rid: &str) -> String {
    let r = rid.to_string();
    s.store_call(move |st| Ok(st.find_remote(&r)?.map(|r| r.name)))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "the remote".into())
}

/// Republishes the threads with outbox rows of remote `rid`, and its open
/// questions whose copies are not done, so the shell shows their state.
pub(crate) async fn publish_outbox(s: &AppState, rid: &str) {
    let r = rid.to_string();
    let ctx = s.feedback_ctx();
    let st2 = s.clone();
    let _ = s
        .store_call(move |st| {
            let mut seen = HashSet::new();
            for o in st.outbox_of(&r)? {
                if seen.insert(o.thread_id.clone())
                    && let Some(t) = st.get_thread(&o.thread_id)?
                {
                    crate::routes::threads::publish_thread(&ctx, st, &t)?;
                }
            }
            let open: Vec<String> = st
                .live_linked_questions(&r)?
                .into_iter()
                .filter(|q| q.status == "open")
                .map(|q| q.question_id)
                .collect();
            publish_questions(&st2, st, &open)
        })
        .await;
}

/// Announces each of questions `qids` as it now is. Runs on a store
/// thread.
fn publish_questions(
    s: &AppState,
    st: &clax_store::Store,
    qids: &[String],
) -> clax_core::Result<()> {
    for qid in qids {
        if let Some(q) = st.question(qid)? {
            let v = clax_http::views::question::view(st, &q)?;
            clax_http::views::question::announce(s, st, &q, v);
        }
    }
    Ok(())
}

/// A cycle's state between cycles.
struct Sync {
    s: AppState,
    rid: String,
    /// The remote's client, speaking in the sync's session there.
    client: Option<Client>,
    /// The public artifacts the session watches, each with the local agent
    /// its watch names as the one it relays for (`None`: none, the sync's
    /// own name shows).
    watched: HashMap<String, Option<(String, String)>>,
    /// The public threads a feedback poll handed over, acknowledged after
    /// the next cycle wrote them here.
    acks: Vec<String>,
    /// Sessions there named as local agents, by name, each with its agent
    /// handle there: the outbox's agent writes go up in them, and a local
    /// agent's question is asked in its own, which stays (heartbeating)
    /// while the question's copy is open there.
    agents: HashMap<String, (Client, String)>,
    /// The local questions whose copies were asked there, by the name of
    /// the session in `agents` they were asked in.
    asked_in: HashMap<String, String>,
    /// A wait there on each open copy, by its question ID there: one
    /// request each at a time (a remote holds few polls of one question),
    /// kept across feedback polls.
    copy_waits: HashMap<String, clax_rt::JoinHandle<()>>,
}

/// The local agent the sync names as the one it relays for on local
/// artifact `aid`'s public copy: the owner's agent live on it (its owner
/// session or a watcher) most recently active, as its participants list
/// it; `None` when none is.
fn local_relay(st: &clax_store::Store, aid: &str) -> clax_core::Result<Option<(String, String)>> {
    let Ok(id) = clax_core::ArtifactId::parse(aid) else {
        return Ok(None);
    };
    Ok(st
        .participants(&id)?
        .agents
        .into_iter()
        .find(|a| {
            a.live
                && a.person.owner
                && crate::routes::sessions::HARNESSES.contains(&a.harness.as_str())
        })
        .map(|a| (a.name.unwrap_or_else(|| a.harness.clone()), a.harness)))
}

/// What sending one outbox row did.
enum Sent {
    Acked,
    /// Waits (its version not pushed, or its thread not acknowledged yet).
    Waiting,
    /// The remote refused it: shown not sent, with the reason.
    Refused(String),
}

impl Sync {
    fn store(&self) -> &AppState {
        &self.s
    }

    /// Ends the sync's sessions there.
    async fn end(&mut self) {
        if let Some(mut c) = self.client.take() {
            crate::public::sendable(c.end_session()).await;
        }
        for (_, (mut c, _)) in std::mem::take(&mut self.agents) {
            crate::public::sendable(c.end_session()).await;
        }
        self.asked_in.clear();
        for (_, w) in self.copy_waits.drain() {
            w.abort();
        }
    }

    /// Starts a wait on each open copy there that has none, in the sync's
    /// session (a keyed copy is any of the token's sessions' to read), and
    /// ends the waits of copies no longer open here. A wait that finds its
    /// copy no longer open there, or cannot ask, kicks a cycle and ends;
    /// one that ran out waits again.
    async fn wait_on_copies(&mut self) {
        let rid = self.rid.clone();
        let open: HashSet<String> = self
            .s
            .store_call(move |st| {
                Ok(st
                    .live_linked_questions(&rid)?
                    .into_iter()
                    .filter_map(|r| r.remote_question_id)
                    .collect())
            })
            .await
            .unwrap_or_default();
        self.copy_waits.retain(|q, w| {
            let keep = open.contains(q) && !w.is_finished();
            if !keep {
                w.abort();
            }
            keep
        });
        let Some(c) = self.client.as_ref() else {
            return;
        };
        let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
        for q in open {
            if self.copy_waits.contains_key(&q) {
                continue;
            }
            let (c, s, rid) = (c.clone(), self.s.clone(), self.rid.clone());
            let path = format!("/api/sessions/{sid}/questions/{q}?wait={WAIT_S}");
            let w = clax_rt::spawn(async move {
                loop {
                    match crate::public::sendable(c.get(&path)).await {
                        Ok(v) if v["question"]["status"] == "open" => {}
                        _ => break,
                    }
                }
                s.linked.kick(Some(&rid));
            });
            self.copy_waits.insert(q, w);
        }
    }

    /// The client, signed in and in the sync's session, watching every
    /// linked public artifact.
    async fn connect(&mut self, links: &[RemoteLink]) -> Result<(), Stop> {
        if self.s.linked.offline.load(Ordering::SeqCst) {
            return Err(Stop::Unreachable("offline".into()));
        }
        if self.client.is_none() {
            let rid = self.rid.clone();
            let (remote, token) = self
                .s
                .store_call(move |st| Ok((st.find_remote(&rid)?, st.remote_token(&rid)?)))
                .await?;
            let (Some(remote), Some(token)) = (remote, token) else {
                return Err(Stop::Gone);
            };
            let mut c = Client::new(&remote.origin, Some(token));
            crate::public::sendable(c.session_for(SYNC_HARNESS, SYNC_NAME))
                .await
                .map_err(|f| {
                    Stop::Unreachable(classify(&f).unwrap_or_else(|| f.message.clone()))
                })?;
            self.client = Some(c);
            self.watched.clear();
        }
        let c = self.client.as_ref().expect("connected");
        let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
        let locals: Vec<String> = links.iter().map(|l| l.artifact_id.clone()).collect();
        let relays = self
            .s
            .store_call(move |st| {
                locals
                    .iter()
                    .map(|aid| local_relay(st, aid))
                    .collect::<clax_core::Result<Vec<_>>>()
            })
            .await?;
        for (l, relay) in links.iter().zip(relays) {
            if self.watched.get(&l.remote_artifact_id) == Some(&relay) {
                continue;
            }
            let path = format!("/api/sessions/{sid}/watches/{}", l.remote_artifact_id);
            let body = json!({"as": relay.as_ref().map(|(name, harness)| json!({"name": name, "harness": harness}))});
            let watched =
                match crate::public::sendable(c.call(reqwest::Method::PUT, &path, Some(&body)))
                    .await
                {
                    // A public Clax older than the relay label watches without it.
                    Err(f) if f.status == Some(400) => {
                        crate::public::sendable(c.call(
                            reqwest::Method::PUT,
                            &path,
                            Some(&json!({})),
                        ))
                        .await
                    }
                    r => r,
                };
            match watched {
                Ok(_) => {
                    self.watched.insert(l.remote_artifact_id.clone(), relay);
                }
                // A public copy gone there (or not this person's to watch)
                // leaves the remote's other links syncing.
                Err(f) if matches!(f.status, Some(403 | 404)) => {
                    tracing::warn!(
                        artifact = l.artifact_id.as_str(),
                        code = f.code.as_str(),
                        "a linked public copy cannot be watched"
                    );
                }
                Err(f) => {
                    return Err(Stop::Unreachable(
                        classify(&f).unwrap_or_else(|| f.message.clone()),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Long-polls the session's feedback there; the threads it handed over
    /// are acknowledged after the next cycle.
    async fn poll(&mut self) -> Result<(), String> {
        let Some(c) = self.client.as_ref() else {
            return Err("not connected".into());
        };
        if self.s.linked.offline.load(Ordering::SeqCst) {
            return Err("offline".into());
        }
        let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
        // An answer to a copy asked in a local agent's session there wakes
        // that session, not the sync's: each open copy has a wait of its
        // own, which starts a cycle once the copy is no longer open there.
        self.wait_on_copies().await;
        let c = self.client.as_ref().expect("connected");
        let v = crate::public::sendable(c.get(&format!(
            "/api/sessions/{sid}/feedback?tier=wait&wait={WAIT_S}"
        )))
        .await
        .map_err(|f| f.message)?;
        for i in v["feedback"].as_array().into_iter().flatten() {
            if let Some(t) = i["thread_id"].as_str() {
                self.acks.push(t.to_string());
            }
        }
        Ok(())
    }

    /// One cycle: queue, send, read.
    async fn cycle(&mut self, full: bool) -> Result<(), Stop> {
        let rid = self.rid.clone();
        let links = self
            .s
            .store_call(move |st| st.links_of_remote(&rid))
            .await?;
        if links.is_empty() {
            return Err(Stop::Gone);
        }
        // Local writes queue whether or not the remote answers.
        if scan(&self.s, &self.rid).await {
            publish_outbox(&self.s, &self.rid).await;
        }
        self.connect(&links).await?;
        self.drain(&links).await?;
        self.questions(&links).await?;
        for l in &links {
            self.read(l, full).await?;
        }
        if !self.acks.is_empty() {
            let c = self.client.as_ref().expect("connected");
            let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
            let ids = std::mem::take(&mut self.acks);
            let _ = crate::public::sendable(c.post(
                &format!("/api/sessions/{sid}/feedback/ack"),
                &json!({"thread_ids": ids}),
            ))
            .await;
        }
        Ok(())
    }

    /// Sends the outbox in order; a thread whose row waits or was refused
    /// holds its later rows back.
    async fn drain(&mut self, links: &[RemoteLink]) -> Result<(), Stop> {
        let rid = self.rid.clone();
        let rows = self.s.store_call(move |st| st.outbox_of(&rid)).await?;
        let mut blocked: HashSet<String> = HashSet::new();
        // The sessions named as local agents: kept for the cycle's questions,
        // which end those no open copy needs.
        let mut agents = std::mem::take(&mut self.agents);
        let mut changed: HashSet<String> = HashSet::new();
        let mut out = Ok(());
        for row in rows {
            let Some(link) = links.iter().find(|l| l.artifact_id == row.artifact_id) else {
                // Unlinked: kept for the owner to drop.
                continue;
            };
            if blocked.contains(&row.thread_id) {
                continue;
            }
            // A row marked not sent waits for Retry, unless a try of it may
            // have been made there with its answer lost: only a resend under
            // its key tells, and until then the remote's copy would come down
            // here as someone else's.
            if row.failed_at.is_some() && !maybe_made(&row) {
                blocked.insert(row.thread_id.clone());
                continue;
            }
            match self.send(&row, link, &mut agents).await {
                Ok(Sent::Acked) => {
                    changed.insert(row.thread_id.clone());
                }
                Ok(Sent::Waiting) => {
                    blocked.insert(row.thread_id.clone());
                }
                Ok(Sent::Refused(why)) => {
                    blocked.insert(row.thread_id.clone());
                    changed.insert(row.thread_id.clone());
                    let seq = row.seq;
                    self.s
                        .store_call(move |st| st.outbox_failed(seq, &why))
                        .await?;
                }
                Err(why) => {
                    let (seq, w2) = (row.seq, why.clone());
                    self.s
                        .store_call(move |st| st.outbox_attempted(seq, &w2))
                        .await?;
                    out = Err(Stop::Unreachable(why));
                    break;
                }
            }
        }
        self.agents = agents;
        let ctx = self.s.feedback_ctx();
        let _ = self
            .s
            .store_call(move |st| {
                for tid in changed {
                    if let Some(t) = st.get_thread(&tid)? {
                        crate::routes::threads::publish_thread(&ctx, st, &t)?;
                    }
                }
                Ok(())
            })
            .await;
        out
    }

    /// A client speaking as local agent `name` (of `harness`) there, and
    /// its session's agent handle there.
    async fn agent_client<'a>(
        &self,
        agents: &'a mut HashMap<String, (Client, String)>,
        name: &str,
        harness: &str,
    ) -> Result<&'a (Client, String), String> {
        if !agents.contains_key(name) {
            let rid = self.rid.clone();
            let (remote, token) = self
                .s
                .store_call(move |st| Ok((st.find_remote(&rid)?, st.remote_token(&rid)?)))
                .await
                .map_err(|e| e.message)?;
            let (Some(remote), Some(token)) = (remote, token) else {
                return Err("the remote is gone".into());
            };
            let mut c = Client::new(&remote.origin, Some(token));
            crate::public::sendable(c.session_for(harness, name))
                .await
                .map_err(|f| classify(&f).unwrap_or(f.message))?;
            let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
            let v = crate::public::sendable(c.get(&format!("/api/sessions/{sid}")))
                .await
                .map_err(|f| classify(&f).unwrap_or(f.message))?;
            let handle = v["session"]["agent_handle"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            agents.insert(name.to_string(), (c, handle));
        }
        Ok(agents.get(name).expect("inserted"))
    }

    /// Writes `body` to `path` as local agent `name` there: refused as
    /// another agent's thread (the sync's own session holds it, from the
    /// feedback it was handed), the sync hands public thread `rtid` to the
    /// agent's session and tries once more. `None` when the thread takes
    /// no agent write there (not sent to an agent): it goes up as the
    /// person.
    #[allow(clippy::too_many_arguments)]
    async fn as_agent(
        &self,
        agents: &mut HashMap<String, (Client, String)>,
        (name, harness): (&str, &str),
        rtid: &str,
        path: &str,
        body: &Value,
        key: Option<&str>,
    ) -> Result<Option<Value>, RemoteFail> {
        let fail = |message: String| RemoteFail {
            origin: String::new(),
            status: None,
            code: "unreachable".into(),
            message,
            body: Value::Null,
        };
        let (ac, handle) = self
            .agent_client(agents, name, harness)
            .await
            .map_err(fail)?;
        let mut handed = false;
        loop {
            let wrote = match key {
                Some(k) => crate::public::sendable(ac.post_keyed(path, body, k)).await,
                None => crate::public::sendable(ac.post(path, body)).await,
            };
            match wrote {
                Ok(v) if v.get("guidance").is_none() => return Ok(Some(v)),
                Ok(_) => return Ok(None),
                Err(f) if f.status == Some(409) && !handed => {
                    handed = true;
                    let c = self.client.as_ref().expect("connected");
                    let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
                    if crate::public::sendable(c.post(
                        &format!("/api/sessions/{sid}/threads/{rtid}/handoff"),
                        &json!({"to": handle}),
                    ))
                    .await
                    .is_err()
                    {
                        return Ok(None);
                    }
                }
                Err(f) if f.status == Some(409) => return Ok(None),
                Err(f) => return Err(f),
            }
        }
    }

    /// Records that row `seq` is about to be sent: from now until its
    /// answer is read, the remote may have made it.
    async fn trying(&self, seq: i64) -> Result<(), String> {
        self.s
            .store_call(move |st| st.outbox_trying(seq))
            .await
            .map_err(|e| e.message)
    }

    /// Whether the answer to an up-send is lost (tests).
    fn lose_answer(&self) -> bool {
        self.s
            .linked
            .lose_answers
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }

    /// Sends outbox row `row` of `link`. `Err` when the remote could not be
    /// reached (the row stays, with its key).
    async fn send(
        &self,
        row: &OutboxRow,
        link: &RemoteLink,
        agents: &mut HashMap<String, (Client, String)>,
    ) -> Result<Sent, String> {
        let tid = row.thread_id.clone();
        let (t, lt) = self
            .s
            .store_call(move |st| Ok((st.get_thread(&tid)?, st.linked_thread(&tid)?)))
            .await
            .map_err(|e| e.message)?;
        let (Some(t), Some(lt)) = (t, lt) else {
            // The thread is gone here: nothing to send.
            let seq = row.seq;
            self.s
                .store_call(move |st| st.ack_outbox(seq, None, None, None))
                .await
                .map_err(|e| e.message)?;
            return Ok(Sent::Acked);
        };
        let c = self.client.as_ref().expect("connected");
        let raid = &link.remote_artifact_id;
        let fail = |f: RemoteFail| -> Result<Sent, String> {
            match classify(&f) {
                Some(why) => Err(why),
                None => Ok(Sent::Refused(format!("{}: {}", f.code, f.message))),
            }
        };
        match row.kind.as_str() {
            "thread" => {
                let (rid, aid, raid2, n) = (
                    self.rid.clone(),
                    link.artifact_id.clone(),
                    raid.clone(),
                    t.version_n,
                );
                let twin = self
                    .s
                    .store_call(move |st| st.remote_twin(&rid, &aid, &raid2, n))
                    .await
                    .map_err(|e| e.message)?;
                let version = match twin {
                    Some(v) => v,
                    None if t.anchor.is_whole() => link.remote_n,
                    None => return Ok(Sent::Waiting),
                };
                let first = t
                    .comments
                    .iter()
                    .find(|x| Some(&x.id) == row.comment_id.as_ref())
                    .or(t.comments.first())
                    .cloned();
                let Some(first) = first else {
                    return Ok(Sent::Refused("the thread has no comment".into()));
                };
                let clip = if t.has_clip {
                    let (id, tid) = (
                        clax_core::ArtifactId::parse(&t.artifact_id).map_err(|e| e.to_string())?,
                        t.id.clone(),
                    );
                    self.s
                        .store_call_async(
                            move |st| async move { st.read_clip_async(&id, &tid).await },
                        )
                        .await
                        .map_err(|e| e.message)?
                } else {
                    None
                };
                let mut form = reqwest::multipart::Form::new()
                    .text(
                        "anchor",
                        serde_json::to_string(&t.anchor).expect("anchors serialise"),
                    )
                    .text("body", first.body.clone())
                    .text("version", version.to_string());
                if let Some(png) = clip
                    && let Ok(part) = reqwest::multipart::Part::bytes(png)
                        .file_name("clip.png")
                        .mime_str("image/png")
                {
                    form = form.part("clip", part);
                }
                self.trying(row.seq).await?;
                let v = match crate::public::sendable(c.post_form_keyed(
                    &format!("/api/artifacts/{raid}/threads"),
                    form,
                    &row.idem_key,
                ))
                .await
                {
                    Ok(v) => v,
                    Err(f) => return fail(f),
                };
                if self.lose_answer() {
                    return Err("the answer was lost".into());
                }
                let rtid = v["thread"]["id"].as_str().map(str::to_string);
                let rcid = v["thread"]["comments"][0]["id"]
                    .as_str()
                    .map(str::to_string);
                let Some(rtid) = rtid else {
                    return Ok(Sent::Refused("the remote answered no thread".into()));
                };
                let seq = row.seq;
                self.s
                    .store_call(move |st| {
                        st.ack_outbox(seq, Some(&rtid), rcid.as_deref(), Some(version))
                    })
                    .await
                    .map_err(|e| e.message)?;
                Ok(Sent::Acked)
            }
            "comment" => {
                let Some(rtid) = lt.remote_thread_id.clone() else {
                    return Ok(Sent::Waiting);
                };
                let Some(body) = row.body.clone() else {
                    // The comment is gone here.
                    let seq = row.seq;
                    self.s
                        .store_call(move |st| st.ack_outbox(seq, None, None, None))
                        .await
                        .map_err(|e| e.message)?;
                    return Ok(Sent::Acked);
                };
                let path = format!("/api/artifacts/{raid}/threads/{rtid}/comments");
                self.trying(row.seq).await?;
                let mut answer: Option<Value> = None;
                if let (Some(name), Some(harness)) = (&row.agent_name, &row.agent_harness) {
                    match self
                        .as_agent(
                            agents,
                            (name, harness),
                            &rtid,
                            &path,
                            &json!({"body": body, "author_kind": "agent"}),
                            Some(&row.idem_key),
                        )
                        .await
                    {
                        Ok(v) => answer = v,
                        Err(f) => return fail(f),
                    }
                }
                let v = match answer {
                    Some(v) => v,
                    None => match crate::public::sendable(c.post_keyed(
                        &path,
                        &json!({"body": body}),
                        &row.idem_key,
                    ))
                    .await
                    {
                        Ok(v) => v,
                        Err(f) => return fail(f),
                    },
                };
                if self.lose_answer() {
                    return Err("the answer was lost".into());
                }
                let rcid = v["comment"]["id"].as_str().map(str::to_string);
                let seq = row.seq;
                self.s
                    .store_call(move |st| st.ack_outbox(seq, None, rcid.as_deref(), None))
                    .await
                    .map_err(|e| e.message)?;
                Ok(Sent::Acked)
            }
            kind => {
                let Some(rtid) = lt.remote_thread_id.clone() else {
                    return Ok(Sent::Waiting);
                };
                let path = format!("/api/artifacts/{raid}/threads/{rtid}/{kind}");
                let mut done = false;
                if kind == "resolve"
                    && let (Some(name), Some(harness)) = (&row.agent_name, &row.agent_harness)
                {
                    match self
                        .as_agent(
                            agents,
                            (name, harness),
                            &rtid,
                            &path,
                            &json!({"as": "agent"}),
                            None,
                        )
                        .await
                    {
                        Ok(v) => done = v.is_some(),
                        Err(f) => return fail(f),
                    }
                }
                if !done && let Err(f) = crate::public::sendable(c.post(&path, &json!({}))).await {
                    return fail(f);
                }
                if self.lose_answer() {
                    return Err("the answer was lost".into());
                }
                let seq = row.seq;
                self.s
                    .store_call(move |st| st.ack_outbox(seq, None, None, None))
                    .await
                    .map_err(|e| e.message)?;
                Ok(Sent::Acked)
            }
        }
    }

    /// Brings remote `rid`'s question copies in step: asks the new ones
    /// there, reads the open ones' answers, and withdraws there those
    /// closed here.
    async fn questions(&mut self, links: &[RemoteLink]) -> Result<(), Stop> {
        let rid = self.rid.clone();
        let rows = self
            .s
            .store_call(move |st| st.live_linked_questions(&rid))
            .await?;
        let mut changed: Vec<String> = Vec::new();
        let mut out = Ok(());
        let mut agents = std::mem::take(&mut self.agents);
        for row in rows {
            match self.question(&row, links, &mut agents).await {
                Ok(true) => changed.push(row.question_id.clone()),
                Ok(false) => {}
                Err(why) => {
                    let (qid, w2) = (row.question_id.clone(), why.clone());
                    self.s
                        .store_call(move |st| st.linked_question_attempted(&qid, &w2))
                        .await?;
                    changed.push(row.question_id.clone());
                    out = Err(Stop::Unreachable(why));
                    break;
                }
            }
        }
        self.keep_askers(agents).await;
        if !changed.is_empty() {
            let st2 = self.s.clone();
            let _ = self
                .s
                .store_call(move |st| publish_questions(&st2, st, &changed))
                .await;
        }
        out
    }

    /// Keeps the sessions named as local agents in which an open copy was
    /// asked, heartbeating them so they outlive the remote's idle reaping
    /// (their copies are withdrawn when they end), and ends the rest.
    async fn keep_askers(&mut self, mut agents: HashMap<String, (Client, String)>) {
        let rid = self.rid.clone();
        // A copy whose ask's answer was lost may have been made there: its
        // session stays until the copy is done.
        let live: HashSet<String> = match self
            .s
            .store_call(move |st| st.live_linked_questions(&rid))
            .await
        {
            Ok(rows) => rows.into_iter().map(|r| r.question_id).collect(),
            // Kept as they are until a cycle can tell.
            Err(_) => {
                self.agents = agents;
                return;
            }
        };
        self.asked_in.retain(|qid, _| live.contains(qid));
        let needed: HashSet<String> = self.asked_in.values().cloned().collect();
        for name in agents.keys().cloned().collect::<Vec<_>>() {
            if needed.contains(&name) {
                continue;
            }
            if let Some((mut c, _)) = agents.remove(&name) {
                crate::public::sendable(c.end_session()).await;
            }
        }
        for (c, _) in agents.values() {
            if let Some((sid, _)) = c.session() {
                let _ = crate::public::sendable(c.call(
                    reqwest::Method::PATCH,
                    &format!("/api/sessions/{sid}"),
                    Some(&json!({"heartbeat": true})),
                ))
                .await;
            }
        }
        self.agents = agents;
    }

    /// Brings question copy `row` in step; whether its view changed here.
    /// `Err` when the remote could not be reached.
    async fn question(
        &mut self,
        row: &LinkedQuestion,
        links: &[RemoteLink],
        agents: &mut HashMap<String, (Client, String)>,
    ) -> Result<bool, String> {
        let qid = row.question_id.clone();
        let linked = links.iter().any(|l| l.artifact_id == row.artifact_id);
        let mut rqid = row.remote_question_id.clone();
        let open_here = row.status == "open" && linked;
        if rqid.is_none() {
            // Not asked there yet. Closed here (or unlinked) before any try
            // could have made it: nothing there to withdraw.
            if !open_here && row.attempts == 0 {
                self.question_done(&qid, false).await?;
                return Ok(true);
            }
            // Refused there, or not sent within the backoff: waits for
            // Retry, unless a try may have made it.
            if row.failed_at.is_some() && row.attempts == 0 {
                return Ok(false);
            }
            match self.ask_there(row, agents).await? {
                Ok(made) => rqid = Some(made),
                Err(why) => {
                    let q2 = qid.clone();
                    self.s
                        .store_call(move |st| st.linked_question_failed(&q2, &why))
                        .await
                        .map_err(|e| e.message)?;
                    return Ok(true);
                }
            }
            if open_here {
                return Ok(true);
            }
        }
        let rqid = rqid.expect("asked there");
        let c = self.client.as_ref().expect("connected");
        let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
        if !open_here {
            // Closed here first: the copy there is withdrawn (or was
            // closed there already).
            let path = format!("/api/sessions/{sid}/questions/{rqid}/withdraw");
            match crate::public::sendable(c.post(&path, &json!({}))).await {
                Ok(_) => {}
                Err(f) => {
                    if let Some(why) = classify(&f) {
                        return Err(why);
                    }
                }
            }
            self.question_done(&qid, false).await?;
            return Ok(false);
        }
        let v = match crate::public::sendable(
            c.get(&format!("/api/sessions/{sid}/questions/{rqid}?wait=0")),
        )
        .await
        {
            Ok(v) => v,
            Err(f) => {
                if let Some(why) = classify(&f) {
                    return Err(why);
                }
                // No longer this token's to read there: asked again.
                self.reask(&qid).await?;
                return Ok(true);
            }
        };
        let q = &v["question"];
        match q["status"].as_str().unwrap_or("open") {
            "answered" => {
                let answers: Option<Vec<clax_core::questions::Answer>> =
                    serde_json::from_value(q["answers"].clone()).ok();
                let (q2, st2) = (qid.clone(), self.s.clone());
                let r = self
                    .s
                    .store_call(move |st| {
                        let Some(answers) = answers else {
                            return Ok(Err("the remote's answer is not one".to_string()));
                        };
                        let c = Close::Answer {
                            answers,
                            via: "shell",
                        };
                        match crate::routes::questions::close(&st2, st, &AuditCtx::DAEMON, &q2, c) {
                            Ok(closed) => Ok(Ok(matches!(
                                closed,
                                crate::routes::questions::Closed::Done(_)
                            ))),
                            Err(clax_core::CoreError::Invalid { message, .. }) => Ok(Err(message)),
                            Err(e) => Err(e),
                        }
                    })
                    .await
                    .map_err(|e| e.message)?;
                match r {
                    Ok(there) => self.question_done(&qid, there).await?,
                    Err(why) => {
                        // An answer that does not fit the questions here is
                        // dropped; the question stays open here, its copy
                        // done.
                        tracing::warn!(
                            question = qid.as_str(),
                            error = why.as_str(),
                            "a remote answer was refused"
                        );
                        let q2 = qid.clone();
                        self.s
                            .store_call(move |st| {
                                st.linked_question_attempted(&q2, &format!("Not answered: {why}"))?;
                                st.linked_question_done(&q2, false)
                            })
                            .await
                            .map_err(|e| e.message)?;
                    }
                }
                Ok(true)
            }
            "declined" => {
                let (q2, st2) = (qid.clone(), self.s.clone());
                let there = self
                    .s
                    .store_call(move |st| {
                        crate::routes::questions::close(
                            &st2,
                            st,
                            &AuditCtx::DAEMON,
                            &q2,
                            Close::Decline,
                        )
                        .map(|c| matches!(c, crate::routes::questions::Closed::Done(_)))
                    })
                    .await
                    .map_err(|e| e.message)?;
                self.question_done(&qid, there).await?;
                Ok(true)
            }
            "open" => Ok(false),
            // Withdrawn there (its session there ended) while open here.
            _ => {
                self.reask(&qid).await?;
                Ok(true)
            }
        }
    }

    /// Asks question copy `row` there under its key: `Ok(Ok(id))` with the
    /// copy's ID there, `Ok(Err(why))` when the remote refused it, `Err`
    /// when it could not be reached. It is asked in a session there named
    /// as the local agent that asked it (in `agents`), so the card there
    /// names that agent; the sync's own session reads and withdraws it, as
    /// a keyed question is any of the token's sessions'.
    async fn ask_there(
        &mut self,
        row: &LinkedQuestion,
        agents: &mut HashMap<String, (Client, String)>,
    ) -> Result<Result<String, String>, String> {
        let qid = row.question_id.clone();
        let (q, asker) = self
            .s
            .store_call(move |st| {
                let q = st.question(&qid)?;
                let asker = match &q {
                    Some(q) => st
                        .get_session(&q.session_id)?
                        .filter(|x| {
                            crate::routes::sessions::HARNESSES.contains(&x.harness.as_str())
                        })
                        .map(|x| (x.name.unwrap_or_else(|| x.harness.clone()), x.harness)),
                    None => None,
                };
                Ok((q, asker))
            })
            .await
            .map_err(|e| e.message)?;
        let Some(q) = q else {
            return Ok(Err("the question is gone".into()));
        };
        let c = match &asker {
            Some((name, harness)) => &self.agent_client(agents, name, harness).await?.0,
            None => self.client.as_ref().expect("connected"),
        };
        let sid = c.session().map(|s| s.0.clone()).unwrap_or_default();
        if let Some((name, _)) = &asker {
            // Before the try: one whose answer is lost may have made it.
            self.asked_in.insert(row.question_id.clone(), name.clone());
        }
        let body = json!({
            "questions": q.questions,
            "artifact_id": row.remote_artifact_id,
            "source": "ask",
        });
        let q2 = row.question_id.clone();
        self.s
            .store_call(move |st| st.linked_question_trying(&q2))
            .await
            .map_err(|e| e.message)?;
        let v = match crate::public::sendable(c.post_keyed(
            &format!("/api/sessions/{sid}/questions"),
            &body,
            &row.idem_key,
        ))
        .await
        {
            Ok(v) => v,
            Err(f) => {
                return match classify(&f) {
                    Some(why) => Err(why),
                    None => Ok(Err(format!("{}: {}", f.code, f.message))),
                };
            }
        };
        if self.lose_answer() {
            return Err("the answer was lost".into());
        }
        let Some(rqid) = v["question"]["id"]
            .as_str()
            .filter(|x| safe_id(x))
            .map(str::to_string)
        else {
            return Ok(Err("the remote answered no question".into()));
        };
        let (q2, r2) = (row.question_id.clone(), rqid.clone());
        self.s
            .store_call(move |st| st.linked_question_made(&q2, &r2))
            .await
            .map_err(|e| e.message)?;
        Ok(Ok(rqid))
    }

    async fn question_done(&self, qid: &str, answered_there: bool) -> Result<(), String> {
        let q = qid.to_string();
        self.s
            .store_call(move |st| st.linked_question_done(&q, answered_there))
            .await
            .map_err(|e| e.message)
    }

    async fn reask(&self, qid: &str) -> Result<(), String> {
        let q = qid.to_string();
        self.s
            .store_call(move |st| st.linked_question_reask(&q))
            .await
            .map_err(|e| e.message)
    }

    /// Reads link `l`'s public threads (those changed since its cursor,
    /// or all when `full`) and writes here what changed.
    async fn read(&mut self, l: &RemoteLink, full: bool) -> Result<(), Stop> {
        let c = self.client.as_ref().expect("connected");
        let since = if full { None } else { l.sync_cursor.clone() };
        let mut threads: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut q = format!(
                "/api/artifacts/{}/threads?include_resolved=true&limit=200",
                l.remote_artifact_id
            );
            if let Some(s) = &since {
                q.push_str(&format!("&since={}", urlencode(s)));
            }
            if let Some(cu) = &cursor {
                q.push_str(&format!("&cursor={}", urlencode(cu)));
            }
            let v = match crate::public::sendable(c.get(&q)).await {
                Ok(v) => v,
                Err(f) => match classify(&f) {
                    Some(why) => return Err(Stop::Unreachable(why)),
                    None if f.status == Some(404) => return Ok(()),
                    None => return Err(Stop::Unreachable(f.message)),
                },
            };
            threads.extend(v["threads"].as_array().cloned().unwrap_or_default());
            match v["next_cursor"].as_str() {
                Some(n) if Some(n) != cursor.as_deref() => cursor = Some(n.to_string()),
                _ => break,
            }
        }
        let mut newest = l.sync_cursor.clone().unwrap_or_default();
        for rt in &threads {
            if let Some(a) = activity(rt)
                && a > newest
            {
                newest = a;
            }
            self.mirror(l, rt).await?;
        }
        if !newest.is_empty() && Some(&newest) != l.sync_cursor.as_ref() {
            let (aid, rid) = (l.artifact_id.clone(), self.rid.clone());
            self.s
                .store_call(move |st| st.set_sync_cursor(&aid, &rid, &newest))
                .await?;
        }
        Ok(())
    }

    /// Writes public thread `rt` of link `l` here: the thread when new, its
    /// comments not here yet, its status and its send.
    async fn mirror(&mut self, l: &RemoteLink, rt: &Value) -> Result<(), Stop> {
        let Some(rtid) = rt["id"].as_str().filter(|x| safe_id(x)).map(str::to_string) else {
            return Ok(());
        };
        // What the token's person may not see yet stays there.
        if rt["anchor_withheld"] == true {
            return Ok(());
        }
        let comments: Vec<(String, MirrorComment)> = rt["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["withheld"] != true)
            .filter_map(|c| {
                let id = c["id"].as_str()?.to_string();
                let body = c["body"].as_str()?.to_string();
                let kind = if c["author_kind"] == "agent" {
                    "agent"
                } else {
                    "viewer"
                };
                Some((
                    id.clone(),
                    MirrorComment {
                        remote_comment_id: id,
                        author: RemoteAuthor {
                            kind: kind.into(),
                            public_id: c["author_public_id"].as_str().map(str::to_string),
                            name: c["author_name"]
                                .as_str()
                                .filter(|n| !n.trim().is_empty())
                                .unwrap_or("Someone")
                                .chars()
                                .take(120)
                                .collect(),
                        },
                        body,
                    },
                ))
            })
            .collect();
        // A new thread starts from its own first comment, once visible.
        let first_seen =
            rt["comments"][0]["withheld"] != true && rt["comments"][0]["body"].is_string();
        let status = rt["status"].as_str().unwrap_or("open").to_string();
        let sent = rt["sent_to_agent"] == true;
        let has_clip = !rt["clip_url"].is_null();
        let remote_version = rt["version_n"].as_u64().unwrap_or(0) as u32;
        let anchor: Option<Anchor> = serde_json::from_value(rt["anchor"].clone()).ok();
        let (rid, link, ctx) = (self.rid.clone(), l.clone(), self.s.feedback_ctx());
        let made = self
            .s
            .store_call(move |st| {
                let audit = AuditCtx::DAEMON;
                let mut fresh_thread = false;
                let lt = match st.linked_thread_by_remote(&rid, &rtid)? {
                    Some(lt) => lt,
                    None => {
                        let (Some(anchor), Some((_, first)), true) =
                            (anchor, comments.first(), first_seen)
                        else {
                            return Ok(None);
                        };
                        let remote = st.find_remote(&rid)?;
                        let Some(remote) = remote else {
                            return Ok(None);
                        };
                        let (version_n, twin) =
                            match st.local_twin(&rid, &link.remote_artifact_id, remote_version)? {
                                Some(n) => (n, true),
                                None => (link.pushed_n, false),
                            };
                        let t = st.mirror_thread(
                            &audit,
                            &link,
                            (&remote.name, &remote.origin),
                            version_n,
                            twin,
                            MirrorThread {
                                remote_thread_id: rtid.clone(),
                                remote_version,
                                anchor,
                                first: first.clone(),
                            },
                        )?;
                        fresh_thread = true;
                        st.linked_thread(&t.id)?
                            .ok_or(clax_core::CoreError::NotFound)?
                    }
                };
                if lt.unlinked_at.is_some() {
                    return Ok(None);
                }
                let tid = lt.thread_id.clone();
                let mut added = Vec::new();
                for (_, c) in &comments {
                    if let Some(made) = st.mirror_comment(&audit, &tid, &rid, c)? {
                        added.push(made);
                    }
                }
                let mut touched = clax_core::feedback::Touched::default();
                let mut changed = fresh_thread || !added.is_empty();
                let mut t = st.get_thread(&tid)?.ok_or(clax_core::CoreError::NotFound)?;
                // Its status there, unless a change here waits to go up.
                if status != lt.synced_status && !st.status_pending(&tid)? {
                    if status == "resolved" && t.status != "resolved" {
                        let (rt2, w) =
                            st.resolve_thread_touched(&audit, &tid, "viewer:anonymous")?;
                        touched.merge(w);
                        ctx.events.publish(Event::ThreadResolved {
                            artifact_id: rt2.artifact_id.clone(),
                            thread_id: tid.clone(),
                            resolved_by: rt2.resolved_by.clone().unwrap_or_default(),
                            resolved_at: rt2.resolved_at.clone().unwrap_or_default(),
                        });
                        t = rt2;
                    } else if status == "open" && t.status == "resolved" {
                        t = st.reopen_thread(&audit, &tid)?;
                    }
                    st.set_synced_status(&tid, &status)?;
                    changed = true;
                }
                // A send there is a send here: to the owner's agents and
                // those watching, as a local send is.
                if t.status == "open" {
                    if sent && !t.sent_to_agent {
                        let (t2, w, _) =
                            st.send_routed(&audit, &tid, SendTarget::Everyone, Route::Send, false)?;
                        touched.merge(w);
                        t = t2;
                        changed = true;
                    } else if t.sent_to_agent
                        && !fresh_thread
                        && added.iter().any(|c| c.author_kind == "viewer")
                    {
                        let (t2, w, _) =
                            st.send_routed(&audit, &tid, SendTarget::Thread, Route::Join, false)?;
                        touched.merge(w);
                        t = t2;
                    }
                }
                for c in &added {
                    ctx.events.publish(Event::Comment {
                        artifact_id: t.artifact_id.clone(),
                        thread_id: tid.clone(),
                        comment: json!(c),
                    });
                }
                apply(&ctx, st, &touched);
                // Only a change is announced: an announcement wakes the
                // sync again.
                if changed {
                    crate::routes::threads::publish_thread(&ctx, st, &t)?;
                }
                Ok(Some((tid, fresh_thread)))
            })
            .await?;
        if let Some((tid, true)) = made
            && has_clip
        {
            self.fetch_clip(l, rt["id"].as_str().unwrap_or_default(), &tid)
                .await;
        }
        Ok(())
    }

    /// Fetches the clip of public thread `rtid` over the remote's clip
    /// route, with the token, and caches it for local thread `tid`. A clip
    /// that cannot be fetched is left out.
    async fn fetch_clip(&self, l: &RemoteLink, rtid: &str, tid: &str) {
        let Some(c) = self.client.as_ref() else {
            return;
        };
        let path = format!(
            "/api/artifacts/{}/threads/{rtid}/clip",
            l.remote_artifact_id
        );
        let max = clax_store::threads::MAX_CLIP_BYTES;
        match crate::public::sendable(c.get_bytes(&path, max)).await {
            Ok((bytes, _)) => {
                let (t2, ctx) = (tid.to_string(), self.store().feedback_ctx());
                let r = self
                    .s
                    .store_call(move |st| {
                        st.cache_remote_clip(&t2, &bytes)?;
                        if let Some(t) = st.get_thread(&t2)? {
                            crate::routes::threads::publish_thread(&ctx, st, &t)?;
                        }
                        Ok(())
                    })
                    .await;
                if let Err(e) = r {
                    tracing::warn!(thread = tid, code = e.code, "a remote clip was not cached");
                }
            }
            Err(f) => tracing::debug!(thread = tid, error = f.message.as_str(), "no remote clip"),
        }
    }
}

/// Whether a try of outbox row `row` may have made its thread or comment
/// there, its answer never read: echoes of those are told from the
/// remote's own only once a resend under its key answers.
fn maybe_made(row: &OutboxRow) -> bool {
    row.attempts > 0 && matches!(row.kind.as_str(), "thread" | "comment")
}

/// Whether `id` is shaped as a Clax ID (letters and digits): what a remote
/// names is used in paths only then.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The newest time a public thread view was made, commented on or resolved.
fn activity(rt: &Value) -> Option<String> {
    let mut at = rt["created_at"].as_str()?.to_string();
    for c in rt["comments"].as_array().into_iter().flatten() {
        if let Some(t) = c["created_at"].as_str()
            && t > at.as_str()
        {
            at = t.to_string();
        }
    }
    if let Some(r) = rt["resolved_at"].as_str()
        && r > at.as_str()
    {
        at = r.to_string();
    }
    Some(at)
}

/// `s` for a query string.
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_the_newest_time_and_ids_are_checked() {
        let rt = json!({"created_at": "2026-10-15T10:00:00.000Z",
            "comments": [{"created_at": "2026-10-15T10:00:00.000Z"}, {"created_at": "2026-10-15T10:05:00.000Z"}],
            "resolved_at": "2026-10-15T10:03:00.000Z"});
        assert_eq!(activity(&rt).as_deref(), Some("2026-10-15T10:05:00.000Z"));
        assert!(safe_id("01JABCDEF"));
        assert!(!safe_id("../../etc"));
        assert!(!safe_id(""));
        assert_eq!(
            urlencode("2026-10-15T10:05:00.000Z"),
            "2026-10-15T10%3A05%3A00.000Z"
        );
    }
}
