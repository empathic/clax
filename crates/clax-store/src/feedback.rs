//! Feedback rows: who a sent thread's viewer comments are addressed to, which
//! tier hands them over, acknowledgement, resends, and retargeting.

use super::Store;
use super::approvals::Route;
use super::artifacts::{SELECT_VERSION, live_artifact_in, row_to_version};
use super::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, take_in, thread_in, thread_record};
use crate::visibility::Visibility;
use clax_core::anchor::Anchor;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::feedback::{
    FeedbackBatch, FeedbackItem, FeedbackPhase, FeedbackState, ItemWithheld, Notice, Placeholder,
    Tier, Touched,
};
use clax_core::model::{Artifact, Feedback, Thread, Version};
use clax_core::visibility::{self, Author, Reader};
use clax_core::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{BTreeMap, HashMap};

/// What a caller wants handed over: rows targeted to `session_id`, for `tier`,
/// optionally only for one artifact; resend-eligible rows too when the tier
/// resends and `include_resends` is set.
#[derive(Clone, Debug)]
pub struct TakeFeedback {
    pub session_id: String,
    pub tier: Tier,
    pub artifact_id: Option<String>,
    pub include_resends: bool,
}

/// `secs_ago` seconds before `now`, as the store writes times.
fn ts(now: chrono::DateTime<chrono::Utc>, secs_ago: i64) -> String {
    clax_core::time::stamp(now - chrono::Duration::seconds(secs_ago))
}

/// Live sessions a sent thread on artifact `aid` goes to: the owner (first)
/// and every watcher, without duplicates.
fn live_targets(c: &Connection, aid: &str, owner: Option<&str>) -> Result<Vec<String>> {
    let mut stmt = c.prepare(
        "SELECT id FROM sessions WHERE ended_at IS NULL
           AND (id = ?2 OR id IN (SELECT session_id FROM watches WHERE artifact_id = ?1))
         ORDER BY CASE WHEN id = ?2 THEN 0 ELSE 1 END, started_at, id",
    )?;
    Ok(stmt
        .query_map(params![aid, owner], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

/// Releases session `sid` after it ended: drops its watches and scope watches; each of its
/// undelivered rows is deleted when another live session is a target of the
/// same comment, else untargeted for the next session that publishes or watches.
pub(crate) fn release_session(tx: &Transaction<'_>, sid: &str, now: &str) -> Result<Touched> {
    let mut touched = Touched::default();
    tx.execute("DELETE FROM watches WHERE session_id = ?1", params![sid])?;
    tx.execute(
        "DELETE FROM live_watches WHERE session_id = ?1",
        params![sid],
    )?;
    let rows: Vec<(String, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT f.id, f.comment_id, f.thread_id, t.artifact_id FROM feedback f JOIN threads t ON t.id = f.thread_id
             WHERE f.target_session_id = ?1 AND f.delivered_at IS NULL",
        )?;
        stmt.query_map(params![sid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, comment_id, tid, aid) in rows {
        let other_live: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM feedback f JOIN sessions s ON s.id = f.target_session_id
              WHERE f.comment_id = ?1 AND f.id <> ?2 AND s.ended_at IS NULL)",
            params![comment_id, id],
            |r| r.get(0),
        )?;
        if other_live {
            tx.execute("DELETE FROM feedback WHERE id = ?1", params![id])?;
        } else {
            tx.execute(
                "UPDATE feedback SET target_session_id = NULL, untargeted_at = ?2, notified_at = NULL WHERE id = ?1",
                params![id, now],
            )?;
        }
        touched.threads.insert((aid, tid));
    }
    Ok(touched)
}

/// Where a send's rows go (spec §10, "Data flow").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendTarget<'a> {
    /// This live owner or watcher only (a session ID); it becomes the thread's target.
    Agent(&'a str),
    /// Every live owner and watcher, or untargeted when none is live; clears the thread's target.
    Everyone,
    /// A later comment, or `@agent`: the thread's target while it is live, else as `Everyone`
    /// (the stored target is kept, and simply no longer matches a live session).
    Thread,
}

/// The sessions `target` names for a thread of `aid`, writing the thread's
/// target when the send sets or clears it; and the one session the send is
/// to, when it is to one (else it is to every live owner and watcher).
fn targets_for(
    tx: &Transaction<'_>,
    thread_id: &str,
    aid: &str,
    owner: Option<&str>,
    target: SendTarget<'_>,
) -> Result<(Vec<String>, Option<String>)> {
    let live = live_targets(tx, aid, owner)?;
    match target {
        SendTarget::Agent(sid) => {
            if !live.iter().any(|s| s == sid) {
                return Err(CoreError::invalid(
                    "unknown_agent",
                    "no live agent on this artifact has that handle",
                ));
            }
            tx.execute(
                "UPDATE threads SET target_session_id = ?2 WHERE id = ?1",
                params![thread_id, sid],
            )?;
            Ok((vec![sid.to_string()], Some(sid.to_string())))
        }
        SendTarget::Everyone => {
            tx.execute(
                "UPDATE threads SET target_session_id = NULL WHERE id = ?1",
                params![thread_id],
            )?;
            Ok((live, None))
        }
        SendTarget::Thread => {
            let stored: Option<String> = tx.query_row(
                "SELECT target_session_id FROM threads WHERE id = ?1",
                params![thread_id],
                |r| r.get(0),
            )?;
            Ok(match stored {
                Some(sid) if live.contains(&sid) => (vec![sid.clone()], Some(sid)),
                _ => (live, None),
            })
        }
    }
}

/// Live session IDs that a send `to` this artifact may name: its live owner
/// and live watchers.
pub(crate) fn live_targets_of(c: &Connection, aid: &str) -> Result<Vec<String>> {
    let owner: Option<String> = c.query_row(
        "SELECT owner_session_id FROM artifacts WHERE id = ?1",
        params![aid],
        |r| r.get(0),
    )?;
    live_targets(c, aid, owner.as_deref())
}

/// What [`send_in`] wrote: the feedback rows it inserted, the one session
/// the send went to (`None`: every live owner and watcher), and whether it
/// changed anything (marked the thread sent, changed its target, or
/// inserted a row).
pub(crate) struct Sent {
    pub feedback_ids: Vec<String>,
    pub to: Option<String>,
    pub changed: bool,
    /// The live sessions the send reached.
    pub targets: Vec<String>,
    /// The send named one agent that did not own the thread before: it
    /// is the new owner (M23), for a `thread.take` record.
    pub took: bool,
    /// The session that owned the thread before the send made another
    /// agent its owner: the caller displaces it ([`displace_owner_in`]).
    pub took_from: Option<String>,
}

/// Session `old` lost thread `tid` to a send naming another agent (spec
/// 2026-10-08 §12): its undelivered rows of the thread are deleted and
/// its open `ask` requests for it withdrawn (`approval.withdraw`, reason
/// `thread_taken`), as a take does; `touched.targets` gets `old`, so its
/// waits learn the rows are gone.
pub(crate) fn displace_owner_in(
    st: &super::Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    tid: &str,
    old: &str,
    now: &str,
    touched: &mut Touched,
) -> Result<()> {
    tx.execute(
        "DELETE FROM feedback WHERE thread_id = ?1 AND target_session_id = ?2 AND delivered_at IS NULL",
        params![tid, old],
    )?;
    st.agents_pinged();
    super::approvals::withdraw_taken_in(st, tx, ctx, tid, old, now)?;
    touched.targets.insert(old.to_string());
    Ok(())
}

/// The stored target session of thread `thread_id`.
fn stored_target(tx: &Transaction<'_>, thread_id: &str) -> Result<Option<String>> {
    Ok(tx.query_row(
        "SELECT target_session_id FROM threads WHERE id = ?1",
        params![thread_id],
        |r| r.get(0),
    )?)
}

/// The `target` of a `thread.send` record (spec 2026-10-06-toolpath-audit
/// §6.2): `{session_id, agent_handle}` of the one session a send went to,
/// or `"watchers"` for every live owner and watcher.
pub(crate) fn send_target_value(c: &Connection, to: Option<&str>) -> Result<serde_json::Value> {
    let Some(sid) = to else {
        return Ok("watchers".into());
    };
    let handle: Option<String> = c
        .query_row(
            "SELECT agent_handle FROM sessions WHERE id = ?1",
            params![sid],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::json!({"session_id": sid, "agent_handle": handle}))
}

/// Hands thread `thread_id`'s untargeted, undelivered rows to session
/// `sid`, first dropping each whose comment `sid` already has a row for
/// (one row per comment and session). Returns how many rows it handed over.
pub(crate) fn give_untargeted_in(
    tx: &Transaction<'_>,
    thread_id: &str,
    sid: &str,
) -> Result<usize> {
    tx.execute(
        "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL
           AND target_session_id IS NULL
           AND comment_id IN (SELECT comment_id FROM feedback WHERE thread_id = ?1 AND target_session_id = ?2)",
        params![thread_id, sid],
    )?;
    Ok(tx.execute(
        "UPDATE feedback SET target_session_id = ?2, untargeted_at = NULL, push_failed_at = NULL,
            notified_at = NULL
         WHERE thread_id = ?1 AND delivered_at IS NULL AND target_session_id IS NULL",
        params![thread_id, sid],
    )?)
}

/// Marks the thread sent to the agent and creates one row per (viewer
/// comment without a row, target of `target`), each carrying `batch_id`.
/// With no target, one untargeted row per comment. Returns the rows it
/// inserted and whom it sent to. See [`Store::send_to_agent`] for the contract.
pub(crate) fn send_in(
    tx: &Transaction<'_>,
    thread_id: &str,
    batch_id: Option<&str>,
    target: SendTarget<'_>,
    touched: &mut Touched,
    now: &str,
) -> Result<Sent> {
    let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
    if t.status == "resolved" {
        return Err(CoreError::invalid(
            "thread_resolved",
            "a resolved thread cannot be sent; add a comment to reopen it",
        ));
    }
    let owner: Option<String> = tx.query_row(
        "SELECT owner_session_id FROM artifacts WHERE id = ?1",
        params![t.artifact_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE threads SET sent_to_agent = 1 WHERE id = ?1",
        params![thread_id],
    )?;
    let target_before = stored_target(tx, thread_id)?;
    let (targets, to) = targets_for(tx, thread_id, &t.artifact_id, owner.as_deref(), target)?;
    let retargeted = stored_target(tx, thread_id)? != target_before;
    // A send to one agent gives it the thread's comments it has no row
    // for, as `@name` does, the untargeted rows first.
    if let SendTarget::Agent(sid) = target {
        give_untargeted_in(tx, thread_id, sid)?;
    }
    let mut inserted = Vec::new();
    for c in t.comments.iter().filter(|c| c.author_kind == AUTHOR_VIEWER) {
        let has_row: bool = match target {
            SendTarget::Agent(sid) => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM feedback WHERE comment_id = ?1 AND target_session_id = ?2)",
                params![c.id, sid],
                |r| r.get(0),
            )?,
            _ => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM feedback WHERE comment_id = ?1)",
                params![c.id],
                |r| r.get(0),
            )?,
        };
        if has_row {
            continue;
        }
        if targets.is_empty() {
            let fid = new_ulid();
            tx.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, untargeted_at, batch_id)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?4, ?5)",
                params![fid, thread_id, c.id, now, batch_id],
            )?;
            inserted.push(fid);
        } else {
            for sid in &targets {
                let fid = new_ulid();
                tx.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, batch_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![fid, thread_id, c.id, sid, now, batch_id],
                )?;
                inserted.push(fid);
            }
            touched.targets.extend(targets.iter().cloned());
        }
    }
    touched
        .threads
        .insert((t.artifact_id.clone(), thread_id.to_string()));
    Ok(Sent {
        changed: !t.sent_to_agent || retargeted || !inserted.is_empty(),
        took: retargeted && matches!(target, SendTarget::Agent(_)),
        took_from: target_before.filter(|_| retargeted && matches!(target, SendTarget::Agent(_))),
        feedback_ids: inserted,
        to,
        targets,
    })
}

/// The `thread.send` record of one send on artifact `aid` (audit spec
/// §6.2): to `target`, the rows it made, the threads it sent; `tid` the
/// thread of a one-thread send, `batch_id` a batch's.
pub(super) fn send_record(
    at: &str,
    aid: &str,
    tid: Option<&str>,
    target: serde_json::Value,
    feedback_ids: Vec<String>,
    batch_id: Option<&str>,
    thread_ids: Vec<String>,
) -> AuditRecord {
    let mut r = AuditRecord::new(AuditKind::ThreadSend, at)
        .with("target", target)
        .with("feedback_ids", feedback_ids);
    if let Some(b) = batch_id {
        r = r.with("batch_id", b);
    }
    r = r.with("thread_ids", thread_ids);
    r.ids.artifact = Some(aid.to_string());
    r.ids.thread = tid.map(str::to_string);
    r
}

/// The `thread.take` record of a send that named `sid` (cause `send`), when
/// it made `sid` the owner of thread `tid` of artifact `aid`.
pub(super) fn take_record(at: &str, aid: &str, tid: &str, sid: &str, cause: &str) -> AuditRecord {
    let mut r = thread_record(AuditKind::ThreadTake, at, aid, tid)
        .with("thread_id", tid)
        .with("session", sid)
        .with("cause", cause);
    r.ids.session = Some(sid.to_string());
    r
}

/// The `feedback.delivered` record of row `feedback_id` of thread `tid` on
/// artifact `aid`, handed to session `sid` by `tier` for the first time.
pub(super) fn delivered_record(
    at: &str,
    aid: &str,
    tid: &str,
    sid: &str,
    feedback_id: &str,
    tier: &str,
) -> AuditRecord {
    let mut r = thread_record(AuditKind::FeedbackDelivered, at, aid, tid)
        .with("feedback_id", feedback_id)
        .with("tier", tier);
    r.ids.session = Some(sid.to_string());
    r
}

struct Pending {
    id: String,
    thread_id: String,
    comment_id: String,
    delivered: bool,
    artifact_id: String,
    title: String,
    version_n: u32,
    anchor_json: String,
    has_clip: bool,
    author: String,
    body: String,
    created_at: String,
    via_page: bool,
    batch: Option<FeedbackBatch>,
    /// A take held it and told the session (`placeholder_at`).
    told: bool,
    /// A take held it because the session may no longer read its
    /// artifact, and told the session so (`no_access_at`).
    gone_told: bool,
    /// The comment's author as the filter reads it: its viewer, or for an
    /// agent's comment, [`Author::System`] (an agent's name is never
    /// marked).
    writer: Author,
}

/// What [`Store::take_feedback`] handed over (spec 2026-10-08 §8.4, M16):
/// the rows whose bodies its reader may see, the placeholders for the
/// held rows it told the session of, and what changed.
#[derive(Clone, Debug, Default)]
pub struct Taken {
    pub items: Vec<FeedbackItem>,
    pub placeholders: Vec<Placeholder>,
    pub touched: Touched,
    /// The view was older than the store's visibility generation when
    /// the take's transaction began: nothing was taken; refresh it and
    /// take again.
    pub stale: bool,
}

/// What [`Store::take_notices`] announced, or `stale` (nothing announced)
/// when the view was older than the store's visibility generation.
#[derive(Clone, Debug, Default)]
pub struct Announced {
    pub notices: Vec<Notice>,
    pub stale: bool,
}

/// Whether `vis`, a reader's view, is older than the store's generation
/// as read inside a write transaction: exact, since the generation is
/// bumped on the writer after each commit that changes visibility.
fn stale_view(st: &Store, vis: &Visibility) -> bool {
    vis.reader().is_some() && vis.generation() != st.visibility_generation()
}

/// What a take's reader may see of one row: its body withheld (by these
/// authors), or its body visible with these other fields withheld.
enum Seen {
    Held(Vec<Author>),
    Body(Option<ItemWithheld>),
}

/// What a filtering take reads of the rows' threads, versions, artifacts
/// and authors' names, each once, on its transaction's connection.
struct Sight<'a> {
    c: &'a Connection,
    vis: &'a Visibility,
    threads: HashMap<String, Option<Thread>>,
    versions: HashMap<(String, u32), Option<Version>>,
    artifacts: HashMap<String, Option<Artifact>>,
    names: HashMap<Author, String>,
}

impl<'a> Sight<'a> {
    fn new(c: &'a Connection, vis: &'a Visibility) -> Sight<'a> {
        Sight {
            c,
            vis,
            threads: HashMap::new(),
            versions: HashMap::new(),
            artifacts: HashMap::new(),
            names: HashMap::new(),
        }
    }

    fn thread(&mut self, tid: &str) -> Result<Option<Thread>> {
        if !self.threads.contains_key(tid) {
            let t = thread_in(self.c, tid)?;
            self.threads.insert(tid.to_string(), t);
        }
        Ok(self.threads[tid].clone())
    }

    fn version(&mut self, aid: &str, n: u32) -> Result<Option<Version>> {
        let key = (aid.to_string(), n);
        if !self.versions.contains_key(&key) {
            let v = self
                .c
                .query_row(
                    &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                    params![aid, n],
                    row_to_version,
                )
                .optional()?
                .transpose()?;
            self.versions.insert(key.clone(), v);
        }
        Ok(self.versions[&key].clone())
    }

    fn artifact(&mut self, aid: &str) -> Result<Option<Artifact>> {
        if !self.artifacts.contains_key(aid) {
            let a = live_artifact_in(self.c, aid)?;
            self.artifacts.insert(aid.to_string(), a);
        }
        Ok(self.artifacts[aid].clone())
    }

    /// The display names of `authors`, joined with ` and `; `Viewer` for
    /// an author with no name.
    fn names(&mut self, authors: &[Author]) -> Result<String> {
        let mut out = Vec::with_capacity(authors.len());
        for a in authors {
            if !self.names.contains_key(a) {
                let raw: Option<String> = match a {
                    Author::Owner => self
                        .c
                        .query_row(
                            "SELECT display_name FROM viewers WHERE owner = 1",
                            [],
                            |r| r.get(0),
                        )
                        .optional()?
                        .flatten(),
                    Author::Person(p) => self
                        .c
                        .query_row(
                            "SELECT display_name FROM viewers WHERE public_id = ?1",
                            params![p],
                            |r| r.get(0),
                        )
                        .optional()?
                        .flatten(),
                    Author::Unconnected | Author::System => None,
                };
                let name = clax_core::feedback::display_name(raw.as_deref().unwrap_or(""));
                // An unconnected author reads `<name> (not connected)`.
                self.names.insert(a.clone(), self.vis.label(&name, a));
            }
            out.push(self.names[a].clone());
        }
        if out.is_empty() {
            out.push(clax_core::feedback::display_name(""));
        }
        Ok(out.join(" and "))
    }

    /// The names withholding `authors` there, when any.
    fn withheld(
        &mut self,
        authors: &[Author],
        aid: &str,
        tid: Option<&str>,
    ) -> Result<Option<String>> {
        let w = self.vis.withheld_authors(authors, aid, tid);
        if w.is_empty() {
            return Ok(None);
        }
        self.names(&w).map(Some)
    }

    /// What the reader may see of row `p`, whose anchor is `anchor`: a
    /// body withheld holds the row; a gone thread or comment holds it as
    /// withheld from no one named.
    fn row(&mut self, p: &Pending, anchor: &Anchor) -> Result<Seen> {
        let (aid, tid) = (p.artifact_id.as_str(), p.thread_id.as_str());
        let Some(t) = self.thread(tid)? else {
            return Ok(Seen::Held(Vec::new()));
        };
        let Some(c) = t.comments.iter().find(|c| c.id == p.comment_id) else {
            return Ok(Seen::Held(Vec::new()));
        };
        let body = self
            .vis
            .withheld_authors(&visibility::authors_of_comment(c), aid, Some(tid));
        if !body.is_empty() {
            return Ok(Seen::Held(body));
        }
        let mut w = ItemWithheld::default();
        if let Some(a) = self.artifact(aid)? {
            w.title = self.withheld(
                &visibility::authors_of_artifact_field(&a, "title"),
                aid,
                None,
            )?;
        }
        // A version that is gone names no authors: withhold what it held.
        match self.version(aid, p.version_n)? {
            Some(v) => {
                w.anchor = self.withheld(&visibility::authors_of_anchor(&t, &v), aid, Some(tid))?;
                w.clip = p.has_clip
                    && !self
                        .vis
                        .sees(&visibility::authors_of_clip(&t, &v), aid, Some(tid));
            }
            None => {
                if !anchor.is_whole() {
                    w.anchor = Some(self.names(&[])?);
                }
                w.clip = p.has_clip;
            }
        }
        if let Some(b) = &p.batch
            && b.note.is_some()
        {
            w.note = !self.vis.sees(
                &visibility::authors_of_batch_note(b.sender.as_deref()),
                aid,
                Some(tid),
            );
        }
        Ok(Seen::Body(Some(w)))
    }
}

/// The held rows a take tells its session of, grouped by (thread,
/// withheld authors) in the order first met.
#[derive(Default)]
struct Held {
    groups: Vec<(String, String, Vec<Author>, Vec<String>)>,
    /// Rows on artifacts the session may no longer read, per thread.
    gone: Vec<(String, Vec<String>)>,
}

impl Held {
    /// Counts row `p`, on an artifact the session may no longer read.
    fn add_gone(&mut self, p: &Pending) {
        match self.gone.iter_mut().find(|g| g.0 == p.thread_id) {
            Some(g) => g.1.push(p.id.clone()),
            None => self.gone.push((p.thread_id.clone(), vec![p.id.clone()])),
        }
    }

    /// Counts row `p`, whose body `authors` withhold.
    fn add(&mut self, sight: &mut Sight<'_>, p: &Pending, authors: &[Author]) -> Result<()> {
        let names = sight.names(authors)?;
        match self
            .groups
            .iter_mut()
            .find(|g| g.0 == p.thread_id && g.1 == names)
        {
            Some(g) => g.3.push(p.id.clone()),
            None => self.groups.push((
                p.thread_id.clone(),
                names,
                authors.to_vec(),
                vec![p.id.clone()],
            )),
        }
        Ok(())
    }

    /// Stamps `placeholder_at` on each counted row at `now`, and the
    /// placeholders: each with its artifact's title as `vis` (the take's
    /// view) lets its reader see it, and whether the reader's person has
    /// been asked there; then stamps `no_access_at` on each row on an
    /// artifact the session may no longer read, with one `no_access`
    /// placeholder per thread.
    fn finish(
        self,
        sight: &mut Sight<'_>,
        vis: &Visibility,
        now: &str,
    ) -> Result<Vec<Placeholder>> {
        let approver = match vis.reader() {
            Some(Reader::Person(p)) => Some(p.clone()),
            _ => None,
        };
        let owner: Option<String> = sight
            .c
            .query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        let mut out = Vec::with_capacity(self.groups.len());
        for (tid, author, authors, ids) in self.groups {
            for id in &ids {
                sight.c.execute(
                    "UPDATE feedback SET placeholder_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            let Some(t) = sight.thread(&tid)? else {
                continue;
            };
            let aid = t.artifact_id.clone();
            let title = match sight.artifact(&aid)? {
                Some(a) => {
                    match sight.withheld(
                        &visibility::authors_of_artifact_field(&a, "title"),
                        &aid,
                        None,
                    )? {
                        Some(by) => clax_core::visibility::placeholder::title_by(&by),
                        None => a.title.clone(),
                    }
                }
                None => clax_core::visibility::placeholder::title_by(&sight.names(&[])?),
            };
            let keys: Vec<String> = authors
                .iter()
                .filter_map(|a| match a {
                    Author::Owner => owner.clone(),
                    Author::Person(p) => Some(p.clone()),
                    Author::Unconnected => Some(clax_core::visibility::UNCONNECTED.to_string()),
                    Author::System => None,
                })
                .collect();
            let asked = super::approvals::request_state_in(
                sight.c,
                approver.as_deref(),
                &aid,
                &tid,
                &keys,
            )? == clax_core::visibility::placeholder::ApprovalState::Requested;
            let author_ids = keys
                .iter()
                .filter(|k| clax_core::is_public_id(k))
                .cloned()
                .collect();
            out.push(Placeholder {
                thread_id: tid,
                artifact_id: aid,
                count: ids.len(),
                author: Some(author),
                author_ids,
                title: Some(title),
                asked,
                no_access: false,
            });
        }
        for (tid, ids) in self.gone {
            for id in &ids {
                sight.c.execute(
                    "UPDATE feedback SET no_access_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            let Some(t) = sight.thread(&tid)? else {
                continue;
            };
            out.push(Placeholder {
                thread_id: tid,
                artifact_id: t.artifact_id.clone(),
                count: ids.len(),
                author: None,
                author_ids: Vec::new(),
                title: None,
                asked: false,
                no_access: true,
            });
        }
        Ok(out)
    }
}

fn waiting_on(harness: &str, has_hsid: bool, armed: bool, codex_push: bool) -> Tier {
    match (harness, armed) {
        ("codex", true) if has_hsid && codex_push => Tier::Queue,
        ("pi", true) => Tier::Inject,
        ("claude" | "codex" | "grok", true) => Tier::StopHook,
        _ => Tier::Piggyback,
    }
}

impl Store {
    /// Delivered rows of these tiers are resent after this long unacknowledged.
    pub const RESEND_AFTER_SECS: i64 = 120;
    /// Most resends of one row.
    pub const MAX_RESENDS: u32 = 3;

    /// Marks the thread sent to the agent and creates one row per (viewer
    /// comment without a row, live target): the thread's target while its
    /// session is live, else the artifact's owner session and every live
    /// watcher. With no live target, one untargeted row per comment.
    /// Idempotent; call it again after each new viewer comment on a sent thread.
    ///
    /// "Without a row" is deliberate, not "never delivered": a resolve
    /// deletes the thread's undelivered rows, so when a later viewer comment
    /// reopens the thread, the comments whose rows were withdrawn are sent
    /// again together with the new one. The agent never saw them, and they
    /// are the context the reopening comment answers. Comments whose rows
    /// were delivered keep their rows and are not resent.
    ///
    /// Each send that changes something is recorded as one `thread.send`
    /// under `ctx`, in its transaction: whom it went to, the rows it made,
    /// and the thread. A send that marks nothing new, keeps the target and
    /// makes no row records nothing.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone; `thread_resolved`
    /// for a resolved thread.
    pub fn send_to_agent(&self, ctx: &AuditCtx, thread_id: &str) -> Result<(Thread, Touched)> {
        self.send_routed(ctx, thread_id, SendTarget::Thread, Route::Join, false)
            .map(|(t, touched, _)| (t, touched))
    }

    /// [`Store::send_to_agent`] with an explicit `target`: one agent (which
    /// becomes the thread's target), everyone (clearing the target), or the
    /// thread's own target. It routes (M18): see [`Store::send_routed`].
    ///
    /// # Errors
    /// As [`Store::send_to_agent`], plus `unknown_agent` when `target` names a
    /// session that is not a live owner or watcher of the artifact.
    pub fn send_to(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        target: SendTarget<'_>,
    ) -> Result<(Thread, Touched)> {
        self.send_routed(ctx, thread_id, target, Route::Send, false)
            .map(|(t, touched, _)| (t, touched))
    }

    /// [`Store::send_to`], asking the people of the agents it reaches
    /// (spec 2026-10-08 §9.2) in its transaction: for [`Route::Send`] to
    /// one agent ([`SendTarget::Agent`]: only a send naming an agent asks),
    /// that agent's person, when they may not see some author of the
    /// thread's comments, gets a request per such author, made or joined;
    /// otherwise (a send naming no agent, a later comment on a sent
    /// thread, [`Route::Join`]) only open requests there count the new
    /// content. A page's send (`page`) names
    /// the page and its viewer on the request's item. The `thread.send`
    /// record names the requests (`approval_request_ids`) when there are
    /// any; their IDs are returned too.
    ///
    /// # Errors
    /// As [`Store::send_to`].
    pub fn send_routed(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        target: SendTarget<'_>,
        route: Route,
        page: bool,
    ) -> Result<(Thread, Touched, Vec<String>)> {
        let mut touched = Touched::default();
        // Only a send naming an agent asks its person (§9.2); any other
        // send counts into the requests open there.
        let route = match target {
            SendTarget::Agent(_) => route,
            SendTarget::Everyone | SendTarget::Thread => Route::Join,
        };
        let requests = self.with_tx(|tx| {
            let now = self.now();
            let sent = send_in(tx, thread_id, None, target, &mut touched, &now)?;
            if let Some(old) = &sent.took_from {
                displace_owner_in(self, tx, ctx, thread_id, old, &now, &mut touched)?;
            }
            let requests = super::approvals::route_in(
                self,
                tx,
                ctx,
                thread_id,
                &sent.targets,
                route,
                page,
                &now,
            )?;
            if !sent.changed {
                return Ok(requests);
            }
            let aid: String = tx.query_row(
                "SELECT artifact_id FROM threads WHERE id = ?1",
                params![thread_id],
                |r| r.get(0),
            )?;
            let mut rec = send_record(
                &now,
                &aid,
                Some(thread_id),
                send_target_value(tx, sent.to.as_deref())?,
                sent.feedback_ids,
                None,
                vec![thread_id.to_string()],
            );
            if !requests.is_empty() {
                rec = rec.with("approval_request_ids", requests.clone());
            }
            self.record_audit(tx, ctx, rec)?;
            if sent.took
                && let Some(sid) = sent.to.as_deref()
            {
                self.agents_pinged();
                self.record_audit(tx, ctx, take_record(&now, &aid, thread_id, sid, "send"))?;
            }
            Ok(requests)
        })?;
        let thread = self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?;
        Ok((thread, touched, requests))
    }

    /// Sends the thread to each of the live sessions `sids` (`@name`, spec
    /// §11), each of which must own or watch the artifact: the first
    /// becomes the thread's target, the thread's undelivered untargeted
    /// rows become the first's, and each gets a row for every viewer
    /// comment it has none for, so a named agent receives the thread even
    /// when another agent was sent its comments before. Naming an agent
    /// routes to it (M18): the requests are made under `ctx` as
    /// [`Store::send_routed`] makes them.
    ///
    /// # Errors
    /// As [`Store::send_to`]; `unknown_agent` when a session is not a live
    /// owner or watcher of the artifact. Nothing is written then.
    pub fn send_to_named(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        sids: &[String],
    ) -> Result<(Thread, Touched)> {
        let mut touched = Touched::default();
        self.with_tx(|tx| {
            let Some(first) = sids.first() else {
                return Ok(());
            };
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            if t.status == "resolved" {
                return Err(CoreError::invalid(
                    "thread_resolved",
                    "a resolved thread cannot be sent; add a comment to reopen it",
                ));
            }
            let live = live_targets_of(tx, &t.artifact_id)?;
            if let Some(sid) = sids.iter().find(|s| !live.contains(s)) {
                return Err(CoreError::invalid(
                    "unknown_agent",
                    format!("session {sid} neither owns nor watches this artifact"),
                ));
            }
            let took = stored_target(tx, thread_id)?.as_deref() != Some(first.as_str());
            tx.execute(
                "UPDATE threads SET sent_to_agent = 1, target_session_id = ?2 WHERE id = ?1",
                params![thread_id, first],
            )?;
            // `@name` adds agents to a thread; the earlier ones keep their
            // rows (§11), unlike a send naming one agent.
            let now = self.now();
            give_untargeted_in(tx, thread_id, first)?;
            for c in t.comments.iter().filter(|c| c.author_kind == AUTHOR_VIEWER) {
                for sid in sids {
                    tx.execute(
                        "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                         SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (
                             SELECT 1 FROM feedback WHERE comment_id = ?3 AND target_session_id = ?4)",
                        params![new_ulid(), thread_id, c.id, sid, now],
                    )?;
                }
            }
            super::approvals::route_in(self, tx, ctx, thread_id, sids, Route::Send, false, &now)?;
            if took {
                self.agents_pinged();
                let rec = take_record(&now, &t.artifact_id, thread_id, first, "mention");
                self.record_audit(tx, ctx, rec)?;
            }
            touched.targets.extend(sids.iter().cloned());
            touched
                .threads
                .insert((t.artifact_id.clone(), thread_id.to_string()));
            Ok(())
        })?;
        let thread = self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?;
        Ok((thread, touched))
    }

    /// Hands thread `tid` from session `from` to session `to`, or, with no
    /// `to`, releases it (spec 2026-10-08 §12, `comments_handoff`), in one
    /// transaction. `from` must own it; or, while it has no live owner,
    /// either be an agent of its ended owner's person or be an agent that
    /// could take it implicitly (its person sent one of the thread's viewer
    /// comments it sees, O8).
    ///
    /// A handoff sends the thread to `to`, a live session that may read
    /// and watch the artifact (the caller checks its rights), giving it a
    /// watch when it has none (§10): `to` becomes the owner (`thread.take`,
    /// cause `handoff`), every other session's undelivered rows of the
    /// thread are deleted (and those sessions' open `ask` requests for it
    /// withdrawn, reason `thread_taken`), its untargeted ones become
    /// `to`'s, and `to` gets a row for every viewer comment it has none
    /// for, so it receives the whole thread. It routes as a send naming
    /// `to` (M18): when `to`'s person has not approved an author of the
    /// thread's comments, a request is made or joined; their IDs are
    /// returned. Recorded as `thread.send` (with `approval_request_ids`
    /// when there are any).
    ///
    /// A release by the live owner clears the owner and untargets the
    /// thread's undelivered rows (one per comment), for the next session
    /// that publishes or watches the artifact; recorded as `thread.release
    /// {thread_id, session, reason: explicit}`. A release of a thread with
    /// no live owner changes and records nothing.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone;
    /// [`CoreError::ThreadTaken`] for a caller that may not hand it off;
    /// `thread_resolved` for a handoff of a resolved thread; `unknown_agent`
    /// when `to` is not a live session. Nothing is written then, the watch
    /// included.
    pub fn handoff(
        &self,
        ctx: &AuditCtx,
        tid: &str,
        from: &str,
        to: Option<&str>,
    ) -> Result<(Thread, Touched, Vec<String>)> {
        let seen = self.seen_comments(tid, from)?;
        let mut touched = Touched::default();
        let requests = self.with_tx(|tx| {
            let t = thread_in(tx, tid)?.ok_or(CoreError::NotFound)?;
            let aid = t.artifact_id.clone();
            let now = self.now();
            let owner = super::threads::check_may_hand_off_in(tx, tid, from, &seen)?;
            // The sessions whose undelivered rows change learn of it.
            let holders: Vec<String> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT DISTINCT target_session_id FROM feedback
                     WHERE thread_id = ?1 AND delivered_at IS NULL AND target_session_id IS NOT NULL",
                )?;
                stmt.query_map(params![tid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let Some(to) = to else {
                let Some(owner) = owner.filter(|o| o.live) else {
                    return Ok(Vec::new());
                };
                touched.threads.insert((aid.clone(), tid.to_string()));
                tx.execute(
                    "UPDATE threads SET target_session_id = NULL WHERE id = ?1",
                    params![tid],
                )?;
                self.agents_pinged();
                tx.execute(
                    "UPDATE feedback SET target_session_id = NULL, untargeted_at = ?2,
                        notified_at = NULL, push_failed_at = NULL
                     WHERE thread_id = ?1 AND delivered_at IS NULL AND target_session_id IS NOT NULL",
                    params![tid, now],
                )?;
                // One untargeted row per comment: the next session to
                // publish or watch takes each once.
                tx.execute(
                    "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL
                       AND target_session_id IS NULL
                       AND id NOT IN (SELECT MIN(id) FROM feedback WHERE thread_id = ?1
                         AND delivered_at IS NULL AND target_session_id IS NULL GROUP BY comment_id)",
                    params![tid],
                )?;
                touched.targets.extend(holders);
                let released = owner.session_id.as_str();
                let mut rec = thread_record(AuditKind::ThreadRelease, &now, &aid, tid)
                    .with("thread_id", tid)
                    .with("session", released)
                    .with("reason", "explicit");
                rec.ids.session = Some(released.to_string());
                self.record_audit(tx, ctx, rec)?;
                return Ok(Vec::new());
            };
            if t.status == "resolved" {
                return Err(CoreError::invalid(
                    "thread_resolved",
                    "a resolved thread cannot be handed off; add a comment to reopen it",
                ));
            }
            let live: Option<bool> = tx
                .query_row(
                    "SELECT ended_at IS NULL FROM sessions WHERE id = ?1",
                    params![to],
                    |r| r.get(0),
                )
                .optional()?;
            if live != Some(true) {
                return Err(CoreError::invalid(
                    "unknown_agent",
                    "no live agent has that name or handle",
                ));
            }
            if !live_targets_of(tx, &aid)?.iter().any(|s| s == to) {
                super::watches::ensure_watch_in(self, tx, ctx, to, &ArtifactId::parse(&aid)?)?;
            }
            touched.threads.insert((aid.clone(), tid.to_string()));
            tx.execute(
                "UPDATE threads SET sent_to_agent = 1, target_session_id = ?2 WHERE id = ?1",
                params![tid, to],
            )?;
            self.agents_pinged();
            for other in holders.iter().filter(|h| h.as_str() != to) {
                super::approvals::withdraw_taken_in(self, tx, ctx, tid, other, &now)?;
            }
            tx.execute(
                "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL
                   AND target_session_id IS NOT NULL AND target_session_id <> ?2",
                params![tid, to],
            )?;
            give_untargeted_in(tx, tid, to)?;
            let mut inserted = Vec::new();
            for c in t.comments.iter().filter(|c| c.author_kind == AUTHOR_VIEWER) {
                let fid = new_ulid();
                let n = tx.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                     SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (
                         SELECT 1 FROM feedback WHERE comment_id = ?3 AND target_session_id = ?4)",
                    params![fid, tid, c.id, to, now],
                )?;
                if n > 0 {
                    inserted.push(fid);
                }
            }
            let sessions = vec![to.to_string()];
            let requests = super::approvals::route_in(
                self,
                tx,
                ctx,
                tid,
                &sessions,
                Route::Send,
                false,
                &now,
            )?;
            let mut rec = send_record(
                &now,
                &aid,
                Some(tid),
                send_target_value(tx, Some(to))?,
                inserted,
                None,
                vec![tid.to_string()],
            );
            if !requests.is_empty() {
                rec = rec.with("approval_request_ids", requests.clone());
            }
            self.record_audit(tx, ctx, rec)?;
            if owner.as_ref().is_none_or(|o| o.session_id != to) {
                self.record_audit(tx, ctx, take_record(&now, &aid, tid, to, "handoff"))?;
            }
            touched.targets.extend(holders);
            touched.targets.insert(to.to_string());
            Ok(requests)
        })?;
        let thread = self.get_thread(tid)?.ok_or(CoreError::NotFound)?;
        Ok((thread, touched, requests))
    }

    /// Hands over rows for `q`, as `vis` lets its reader see them (spec
    /// 2026-10-08 §8.4, M16), and records the hand-over in the same
    /// transaction, so two concurrent callers never receive the same row.
    ///
    /// Rows whose comment body the reader may see are handed over as
    /// before, each other field it may not see replaced ([`FeedbackItem`]):
    /// undelivered rows are marked delivered by `q.tier`; resend-eligible
    /// rows (see [`Tier::resends`]) get `resend_count + 1` and `resent:
    /// true`; in-band tiers also acknowledge. Rows whose body it may not
    /// see are held: never marked, never resent. Each held undelivered row
    /// not yet told of is counted in one [`Placeholder`] per (authors,
    /// thread) and gets `placeholder_at`, so the session is told once;
    /// `queue` hands over visible rows only and tells of none. Rows of
    /// deleted artifacts and resolved threads are never handed over. Items
    /// are oldest first. When `vis` is older than the visibility generation
    /// as its transaction reads it (an approval or revocation committed
    /// since the view was built), nothing is taken and the result is
    /// `stale`: the caller refreshes the view and takes again (M17).
    ///
    /// Each row handed over while undelivered is recorded as
    /// `feedback.delivered` (its ID and `q.tier`, the session in the
    /// `session_id` column) under `ctx`, in the hand-over's transaction; a
    /// resend is not recorded.
    pub fn take_feedback(
        &self,
        ctx: &AuditCtx,
        q: &TakeFeedback,
        browser_base: &str,
        vis: &Visibility,
    ) -> Result<Taken> {
        self.take_feedback_readable(ctx, q, browser_base, vis, None)
    }

    /// [`Store::take_feedback`], holding as well every row on an artifact
    /// `readable` says the session may not read (a person's agent whose
    /// rights were narrowed since, spec §7.5): such rows are never handed
    /// over; each undelivered one not yet told of is counted in one
    /// `no_access` [`Placeholder`] per thread ("you no longer have access
    /// to this page", nothing of the page or its comments) and gets
    /// `no_access_at`, so the session is told once; `queue` tells of none.
    /// A row the session may read again loses `no_access_at`, and is
    /// handed over or held as withheld as any other. `None` reads every
    /// artifact.
    pub fn take_feedback_readable(
        &self,
        ctx: &AuditCtx,
        q: &TakeFeedback,
        browser_base: &str,
        vis: &Visibility,
        readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)>,
    ) -> Result<Taken> {
        let cutoff = ts(self.now_utc(), Self::RESEND_AFTER_SECS);
        let resends = q.include_resends && q.tier.resends();
        let now = self.now();
        let base = browser_base.trim_end_matches('/').to_string();
        let taken = self.with_tx(|tx| {
            if stale_view(self, vis) {
                return Ok(None);
            }
            let remote: bool = tx
                .query_row(
                    "SELECT grant_id IS NOT NULL FROM sessions WHERE id = ?1",
                    params![q.session_id],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(false);
            let mut stmt = tx.prepare_cached(TAKE_FEEDBACK)?;
            let rows = stmt
                .query_map(
                    params![
                        q.session_id,
                        q.artifact_id,
                        q.tier.armed_only(),
                        resends,
                        Self::MAX_RESENDS,
                        cutoff,
                        q.tier == Tier::Queue
                    ],
                    |r| {
                        Ok(Pending {
                            id: r.get(0)?,
                            thread_id: r.get(1)?,
                            comment_id: r.get(2)?,
                            delivered: r.get(3)?,
                            artifact_id: r.get(4)?,
                            title: r.get(5)?,
                            version_n: r.get(6)?,
                            anchor_json: r.get(7)?,
                            has_clip: r.get::<_, i64>(8)? != 0,
                            author: r.get(9)?,
                            body: r.get(10)?,
                            created_at: r.get(11)?,
                            via_page: r.get::<_, i64>(12)? != 0,
                            batch: match r.get::<_, Option<String>>(13)? {
                                Some(id) => Some(FeedbackBatch {
                                    id,
                                    size: r.get::<_, Option<u32>>(14)?.unwrap_or(0),
                                    note: r.get(15)?,
                                    sent_by: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                                    sender: r.get(17)?,
                                }),
                                None => None,
                            },
                            told: r.get(18)?,
                            gone_told: r.get(21)?,
                            writer: if r.get::<_, String>(19)? == AUTHOR_AGENT {
                                Author::System
                            } else {
                                Author::of_viewer(r.get::<_, Option<String>>(20)?.as_deref())
                            },
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let mut sight = Sight::new(tx, vis);
            let mut held = Held::default();
            // A row whose anchor or artifact ID no longer parses is left
            // untouched (and skipped) rather than marked delivered without
            // being handed over.
            let mut good = Vec::with_capacity(rows.len());
            for mut p in rows {
                let parsed = serde_json::from_str::<Anchor>(&p.anchor_json)
                    .ok()
                    .zip(ArtifactId::parse(&p.artifact_id).ok());
                let Some((anchor, aid)) = parsed else {
                    tracing::warn!(
                        feedback_id = p.id.as_str(),
                        thread_id = p.thread_id.as_str(),
                        artifact_id = p.artifact_id.as_str(),
                        "skipping feedback on a corrupt thread row"
                    );
                    continue;
                };
                if let Some(may) = readable {
                    match sight.artifact(&p.artifact_id)? {
                        Some(a) if may(&a) => {}
                        _ => {
                            if !p.delivered && !p.gone_told && q.tier != Tier::Queue {
                                held.add_gone(&p);
                            }
                            continue;
                        }
                    }
                }
                // Readable (again): a later loss of access is told again.
                if p.gone_told {
                    tx.execute(
                        "UPDATE feedback SET no_access_at = NULL WHERE id = ?1",
                        params![p.id],
                    )?;
                }
                let withheld = if vis.is_all() {
                    None
                } else {
                    match sight.row(&p, &anchor)? {
                        Seen::Held(authors) => {
                            if !p.delivered && !p.told && q.tier != Tier::Queue {
                                held.add(&mut sight, &p, &authors)?;
                            }
                            continue;
                        }
                        Seen::Body(w) => w,
                    }
                };
                let withheld = withheld.filter(|w| !w.is_empty());
                let shown = withheld.as_ref();
                // A grant session's agent is on another machine: it fetches
                // the clip by reference (spec §16), never by this path; a
                // local agent reads it only where the content is files.
                let clip_path = (p.has_clip && !shown.is_some_and(|w| w.clip))
                    .then(|| {
                        if remote {
                            Some(clax_core::clips::reference(aid.as_str(), &p.thread_id))
                        } else {
                            self.local_clip(&aid, &p.thread_id)
                                .map(|path| path.to_string_lossy().into_owned())
                        }
                    })
                    .flatten();
                if let Some(w) = shown {
                    if w.title.is_some() {
                        p.title = clax_core::visibility::placeholder::title_by(
                            w.title.as_deref().unwrap_or_default(),
                        );
                    }
                    if w.note
                        && let Some(b) = p.batch.as_mut()
                    {
                        b.note = None;
                    }
                }
                let anchor_shown = shown.is_none_or(|w| w.anchor.is_none());
                let page = super::live::live_page_of_conn(tx, &p.artifact_id)?;
                let live_path: Option<String> = match page {
                    Some(_) => tx.query_row(
                        "SELECT live_path FROM threads WHERE id = ?1",
                        params![p.thread_id],
                        |r| r.get(0),
                    )?,
                    None => None,
                };
                let live = page.map(|lp| clax_core::feedback::LiveRef {
                    page_url: format!(
                        "{}{}{}",
                        lp.origin,
                        live_path.as_deref().unwrap_or(&lp.path),
                        if anchor_shown {
                            anchor.route.as_deref().unwrap_or("")
                        } else {
                            ""
                        }
                    ),
                    snapshot_path: self
                        .content
                        .local_file(&aid, p.version_n, clax_core::publish::INDEX)
                        .map(|path| path.to_string_lossy().into_owned()),
                });
                good.push((p, anchor_shown.then_some(anchor), clip_path, live, withheld));
            }
            for (p, _, _, _, _) in &good {
                if p.delivered {
                    tx.execute(
                        "UPDATE feedback SET resend_count = resend_count + 1, last_sent_at = ?2,
                            acknowledged_at = CASE WHEN ?3 THEN ?2 ELSE acknowledged_at END WHERE id = ?1",
                        params![p.id, now, q.tier.in_band()],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE feedback SET delivered_at = ?2, delivery_tier = ?3, last_sent_at = ?2,
                            acknowledged_at = CASE WHEN ?4 THEN ?2 ELSE acknowledged_at END WHERE id = ?1",
                        params![p.id, now, q.tier.as_str(), q.tier.in_band()],
                    )?;
                    let rec = delivered_record(
                        &now,
                        &p.artifact_id,
                        &p.thread_id,
                        &q.session_id,
                        &p.id,
                        q.tier.as_str(),
                    );
                    self.record_audit(tx, ctx, rec)?;
                }
            }
            // The first delivery of an unowned thread's row makes the
            // session its owner when its person sent a comment delivered
            // (M23, O8); a held row makes none.
            let mut took = Touched::default();
            let mut first: Vec<(&str, Vec<String>)> = Vec::new();
            for (p, _, _, _, _) in &good {
                if p.delivered {
                    continue;
                }
                match first.iter_mut().find(|(t, _)| *t == p.thread_id) {
                    Some((_, cs)) => cs.push(p.comment_id.clone()),
                    None => first.push((&p.thread_id, vec![p.comment_id.clone()])),
                }
            }
            for (tid, comments) in first {
                take_in(
                    self,
                    tx,
                    ctx,
                    tid,
                    &q.session_id,
                    "first_delivery",
                    &comments,
                    &now,
                    &mut took,
                )?;
            }
            let placeholders = held.finish(&mut sight, vis, &now)?;
            Ok(Some((good, placeholders, took)))
        })?;
        let Some((pending, placeholders, took)) = taken else {
            return Ok(Taken {
                stale: true,
                ..Taken::default()
            });
        };
        let mut touched = took;
        for p in &placeholders {
            touched
                .threads
                .insert((p.artifact_id.clone(), p.thread_id.clone()));
        }
        let mut items = Vec::with_capacity(pending.len());
        let remote = self.remote_author_labels(
            &pending
                .iter()
                .map(|(p, ..)| p.comment_id.clone())
                .collect::<Vec<_>>(),
        )?;
        for (mut p, anchor, clip_path, live, withheld) in pending {
            // Names of authors who are not connected say so (spec §9.6); a
            // remote's say where they are.
            p.author = match remote.get(&p.comment_id) {
                Some(l) => l.clone(),
                None => vis.label(&p.author, &p.writer),
            };
            if let Some(b) = p.batch.as_mut() {
                for a in visibility::authors_of_batch_note(b.sender.as_deref()) {
                    b.sent_by = vis.label(&b.sent_by, &a);
                }
            }
            touched
                .threads
                .insert((p.artifact_id.clone(), p.thread_id.clone()));
            items.push(FeedbackItem {
                feedback_id: p.id,
                thread_id: p.thread_id,
                comment_id: p.comment_id,
                url: format!("{base}/a/{}", p.artifact_id),
                artifact_id: p.artifact_id,
                artifact_title: p.title,
                version: p.version_n,
                anchor,
                clip_path,
                clip: None,
                author: p.author,
                via_page: p.via_page,
                body: p.body,
                resent: p.delivered,
                created_at: p.created_at,
                batch: p.batch,
                live,
                withheld,
            });
        }
        Ok(Taken {
            items,
            placeholders,
            touched,
            stale: false,
        })
    }

    /// Returns `queue` hand-overs that were not confirmed to undelivered, so
    /// another tier can deliver them, and marks them push-failed: `queue` does
    /// not take them again and [`Store::feedback_state`] reports them waiting
    /// on the in-band tiers, until the row is retargeted to another session.
    /// Each released row is recorded as `feedback.release {feedback_id,
    /// tier: "queue", reason}` under `ctx` (the target in the `session_id`
    /// column), in the release's transaction, so its later delivery by
    /// another tier follows a recorded release.
    pub fn release_feedback(
        &self,
        ctx: &AuditCtx,
        ids: &[String],
        reason: &str,
    ) -> Result<Touched> {
        let mut touched = Touched::default();
        let now = self.now();
        self.with_tx(|tx| {
            for id in ids {
                let n = tx.execute(
                    "UPDATE feedback SET delivered_at = NULL, delivery_tier = NULL, last_sent_at = NULL, push_failed_at = ?2
                     WHERE id = ?1 AND delivery_tier = 'queue' AND acknowledged_at IS NULL",
                    params![id, now],
                )?;
                if n > 0 {
                    let (aid, tid, target): (String, String, Option<String>) = tx.query_row(
                        "SELECT t.artifact_id, f.thread_id, f.target_session_id FROM feedback f JOIN threads t ON t.id = f.thread_id WHERE f.id = ?1",
                        params![id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    let mut rec = thread_record(AuditKind::FeedbackRelease, &now, &aid, &tid)
                        .with("feedback_id", id.as_str())
                        .with("tier", Tier::Queue.as_str())
                        .with("reason", reason);
                    rec.ids.session = target.clone();
                    self.record_audit(tx, ctx, rec)?;
                    touched.threads.insert((aid, tid));
                    touched.targets.extend(target);
                }
            }
            Ok(())
        })?;
        Ok(touched)
    }

    /// Records that `session_id` has seen the threads (it read, replied to, or
    /// resolved them). Rows not yet delivered are marked delivered by
    /// `piggyback`, the in-band tool path, each recorded as
    /// `feedback.delivered` under `ctx`. An undelivered row whose comment
    /// body `vis` withholds from the session's reader was not seen: it
    /// stays held (spec 2026-10-08 M16); a row delivered before its body
    /// became withheld was seen, and is acknowledged (§9.4: what was
    /// delivered stays delivered). Rows on an artifact `readable` says the
    /// session may not read (`None`: every one) are left as they are.
    pub fn acknowledge(
        &self,
        ctx: &AuditCtx,
        session_id: &str,
        thread_ids: &[String],
        vis: &Visibility,
        readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)>,
    ) -> Result<Touched> {
        self.acknowledge_where(ctx, "thread_id", session_id, thread_ids, vis, readable)
    }

    /// Like [`Store::acknowledge`], for `session_id`'s rows on exactly the
    /// comments `comment_ids`: a row for any other comment on the same thread
    /// (one the session has not seen) stays pending.
    pub fn acknowledge_comments(
        &self,
        ctx: &AuditCtx,
        session_id: &str,
        comment_ids: &[String],
        vis: &Visibility,
        readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)>,
    ) -> Result<Touched> {
        self.acknowledge_where(ctx, "comment_id", session_id, comment_ids, vis, readable)
    }

    /// Acknowledges `session_id`'s unacknowledged rows whose `column`
    /// (`thread_id` or `comment_id`) is one of `ids`, on artifacts
    /// `readable` lets it read, but for undelivered rows whose comment body
    /// `vis` withholds, recording the first delivery of each that was
    /// undelivered.
    fn acknowledge_where(
        &self,
        ctx: &AuditCtx,
        column: &str,
        session_id: &str,
        ids: &[String],
        vis: &Visibility,
        readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)>,
    ) -> Result<Touched> {
        debug_assert!(matches!(column, "thread_id" | "comment_id"));
        let now = self.now();
        let mut touched = Touched::default();
        let rows = format!(
            "SELECT f.id, f.thread_id, t.artifact_id, f.comment_id, f.delivered_at IS NULL
             FROM feedback f JOIN threads t ON t.id = f.thread_id
             WHERE f.{column} = ?1 AND f.target_session_id = ?2 AND f.acknowledged_at IS NULL
             ORDER BY f.created_at, f.id"
        );
        self.with_tx(|tx| {
            let mut sight = Sight::new(tx, vis);
            for id in ids {
                let found: Vec<(String, String, String, String, bool)> = {
                    let mut stmt = tx.prepare_cached(&rows)?;
                    stmt.query_map(params![id, session_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for (fid, tid, aid, cid, undelivered) in found {
                    if let Some(may) = readable
                        && !sight.artifact(&aid)?.is_some_and(|a| may(&a))
                    {
                        continue;
                    }
                    if undelivered && !vis.is_all() {
                        let seen = sight.thread(&tid)?.is_some_and(|t| {
                            t.comments.iter().any(|c| {
                                c.id == cid
                                    && vis.sees(
                                        &visibility::authors_of_comment(c),
                                        &aid,
                                        Some(&tid),
                                    )
                            })
                        });
                        if !seen {
                            continue;
                        }
                    }
                    if undelivered {
                        let rec = delivered_record(
                            &now,
                            &aid,
                            &tid,
                            session_id,
                            &fid,
                            Tier::Piggyback.as_str(),
                        );
                        self.record_audit(tx, ctx, rec)?;
                    }
                    tx.execute(
                        "UPDATE feedback SET acknowledged_at = ?2, delivered_at = COALESCE(delivered_at, ?2),
                            delivery_tier = COALESCE(delivery_tier, 'piggyback'), last_sent_at = COALESCE(last_sent_at, ?2)
                         WHERE id = ?1",
                        params![fid, now],
                    )?;
                    if undelivered {
                        take_in(
                            self,
                            tx,
                            ctx,
                            &tid,
                            session_id,
                            "first_delivery",
                            std::slice::from_ref(&cid),
                            &now,
                            &mut touched,
                        )?;
                    }
                    touched.threads.insert((aid, tid));
                }
            }
            Ok(())
        })?;
        Ok(touched)
    }

    /// Gives every untargeted, undelivered row on artifact `id` to `session_id`
    /// (it just published a version or watched the artifact). A row whose
    /// comment already has a row for that session is dropped instead.
    pub fn retarget_untargeted(&self, id: &ArtifactId, session_id: &str) -> Result<Touched> {
        let mut touched = Touched::default();
        self.with_tx(|tx| {
            let rows: Vec<(String, String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT f.id, f.comment_id, f.thread_id FROM feedback f JOIN threads t ON t.id = f.thread_id
                     WHERE t.artifact_id = ?1 AND f.target_session_id IS NULL AND f.delivered_at IS NULL
                     ORDER BY f.created_at, f.id",
                )?;
                stmt.query_map(params![id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (fid, cid, tid) in rows {
                let dup: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM feedback WHERE comment_id = ?1 AND target_session_id = ?2)",
                    params![cid, session_id],
                    |r| r.get(0),
                )?;
                if dup {
                    tx.execute("DELETE FROM feedback WHERE id = ?1", params![fid])?;
                } else {
                    tx.execute(
                        "UPDATE feedback SET target_session_id = ?2, untargeted_at = NULL, push_failed_at = NULL, notified_at = NULL WHERE id = ?1",
                        params![fid, session_id],
                    )?;
                    touched.targets.insert(session_id.to_string());
                }
                touched.threads.insert((id.as_str().to_string(), tid));
            }
            Ok(())
        })?;
        Ok(touched)
    }

    /// Announces the session's rows that no tier has delivered and no
    /// follower has announced, on open threads of live artifacts that the
    /// session watches with replies armed: stamps `notified_at` and returns
    /// one notice per row, oldest first. A row is announced at most once per
    /// target; the stamp is set only where it is still unset, so concurrent
    /// followers never announce the same row twice. Delivery is unaffected:
    /// the rows still wait for tiers 1, 2 and 4.
    ///
    /// Only rows whose comment body `vis` lets its reader see are
    /// announced, on artifacts `readable` lets the session read (`None`:
    /// every one); a held row stays unannounced until it may be seen. A
    /// notice's title is `an artifact by <name>` when the reader may not see
    /// it (spec 2026-10-08 §8.4). A view older than the visibility
    /// generation announces nothing and answers `stale`, as
    /// [`Store::take_feedback`] does.
    pub fn take_notices(
        &self,
        session_id: &str,
        browser_base: &str,
        vis: &Visibility,
        readable: Option<&(dyn Fn(&Artifact) -> bool + Sync)>,
    ) -> Result<Announced> {
        let base = browser_base.trim_end_matches('/').to_string();
        let now = self.now();
        self.with_tx(|tx| {
            if stale_view(self, vis) {
                return Ok(Announced {
                    notices: Vec::new(),
                    stale: true,
                });
            }
            let rows = {
                let mut stmt = tx.prepare(
                    "SELECT f.id, f.comment_id, f.thread_id, t.artifact_id, a.title
                     FROM feedback f
                     JOIN threads t ON t.id = f.thread_id
                     JOIN artifacts a ON a.id = t.artifact_id
                     WHERE f.target_session_id = ?1
                       AND f.delivered_at IS NULL AND f.notified_at IS NULL
                       AND a.deleted_at IS NULL AND t.status = 'open'
                       AND EXISTS (SELECT 1 FROM watches w WHERE w.session_id = f.target_session_id
                                   AND w.artifact_id = t.artifact_id AND w.replies_armed = 1)
                     ORDER BY f.created_at, f.id",
                )?;
                stmt.query_map(params![session_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut sight = Sight::new(tx, vis);
            let mut out = Vec::new();
            for (feedback_id, comment_id, thread_id, artifact_id, mut title) in rows {
                if let Some(may) = readable {
                    match sight.artifact(&artifact_id)? {
                        Some(a) if may(&a) => {}
                        _ => continue,
                    }
                }
                if !vis.is_all() {
                    let Some(t) = sight.thread(&thread_id)? else {
                        continue;
                    };
                    let Some(c) = t.comments.iter().find(|c| c.id == comment_id) else {
                        continue;
                    };
                    if !vis.sees(
                        &visibility::authors_of_comment(c),
                        &artifact_id,
                        Some(&thread_id),
                    ) {
                        continue;
                    }
                    if let Some(a) = sight.artifact(&artifact_id)?
                        && let Some(by) = sight.withheld(
                            &visibility::authors_of_artifact_field(&a, "title"),
                            &artifact_id,
                            None,
                        )?
                    {
                        title = clax_core::visibility::placeholder::title_by(&by);
                    }
                }
                let changed = tx.execute(
                    "UPDATE feedback SET notified_at = ?2
                     WHERE id = ?1 AND notified_at IS NULL AND delivered_at IS NULL",
                    params![feedback_id, now],
                )?;
                if changed == 1 {
                    let url = format!("{base}/a/{artifact_id}");
                    out.push(Notice {
                        feedback_id,
                        comment_id,
                        thread_id,
                        artifact_id,
                        title,
                        url,
                    });
                }
            }
            Ok(Announced {
                notices: out,
                stale: false,
            })
        })
    }

    /// Every feedback row of the thread, oldest first.
    pub fn feedback_rows(&self, thread_id: &str) -> Result<Vec<Feedback>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, thread_id, comment_id, target_session_id, created_at, delivered_at, delivery_tier,
                        acknowledged_at, resend_count, last_sent_at, untargeted_at
                 FROM feedback WHERE thread_id = ?1 ORDER BY created_at, id",
            )?;
            Ok(stmt
                .query_map(params![thread_id], |r| {
                    Ok(Feedback {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        comment_id: r.get(2)?,
                        target_session_id: r.get(3)?,
                        created_at: r.get(4)?,
                        delivered_at: r.get(5)?,
                        delivery_tier: r.get(6)?,
                        acknowledged_at: r.get(7)?,
                        resend_count: r.get(8)?,
                        last_sent_at: r.get(9)?,
                        untargeted_at: r.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The state of the thread's latest forwarded comment as the owner
    /// reads it ([`Delivery::view`] of [`DeliveryViewer::Owner`]), or
    /// `None` when nothing was forwarded. Any acknowledged row:
    /// `acknowledged`; else any delivered row: `delivered`; else any row
    /// held from its live session (`withheld`, naming that session's
    /// person as `approver`) or declined (`declined`); else any row
    /// targeting a live session: `sent`, with the tier that session waits
    /// on (`queue` for an armed Codex session with a known session ID when
    /// `codex_push` and `codex queue` has not failed to take the row,
    /// `inject` for an armed Pi session, `stop_hook` for other armed
    /// sessions, else `piggyback`); else `agent_ended`.
    pub fn feedback_state(
        &self,
        thread_id: &str,
        codex_push: bool,
    ) -> Result<Option<FeedbackState>> {
        Ok(self
            .deliveries(&[thread_id.to_string()], codex_push)?
            .remove(thread_id)
            .and_then(|d| d.view(&DeliveryViewer::Owner)))
    }

    /// The delivery rows of each of `thread_ids` that has any, by thread
    /// ID, each read as [`Delivery::view`] says.
    pub fn deliveries(
        &self,
        thread_ids: &[String],
        codex_push: bool,
    ) -> Result<BTreeMap<String, Delivery>> {
        if thread_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.with_read(|c| deliveries_in(c, thread_ids, codex_push))
    }
}

/// The rows [`Store::take_feedback`] hands over: targeted to session `?1`,
/// of artifact `?2` (`NULL`: any), armed watches only when `?3`, resends
/// when `?4` (under `?5` resends, last sent at or before `?6`), and none
/// that `codex queue` failed to take when `?7`.
pub(crate) const TAKE_FEEDBACK: &str = "SELECT f.id, f.thread_id, f.comment_id, f.delivered_at IS NOT NULL AS delivered,
                        t.artifact_id, a.title, t.version_n, t.anchor_json, t.has_clip,
                        c.author_name, c.body, c.created_at, c.via_page,
                        f.batch_id, b.size, b.note, b.sent_by, b.sender, f.placeholder_at IS NOT NULL,
                        c.author_kind, c.author_public_id, f.no_access_at IS NOT NULL
                 FROM feedback f
                 LEFT JOIN send_batches b ON b.id = f.batch_id
                 JOIN threads t ON t.id = f.thread_id
                 JOIN comments c ON c.id = f.comment_id
                 JOIN artifacts a ON a.id = t.artifact_id
                 WHERE f.target_session_id = ?1
                   AND a.deleted_at IS NULL AND t.status = 'open'
                   AND (?2 IS NULL OR t.artifact_id = ?2)
                   AND (NOT ?3 OR EXISTS (SELECT 1 FROM watches w WHERE w.session_id = f.target_session_id
                                          AND w.artifact_id = t.artifact_id AND w.replies_armed = 1))
                   AND (?7 = 0 OR f.push_failed_at IS NULL)
                   AND (f.delivered_at IS NULL
                        OR (?4 AND f.acknowledged_at IS NULL
                            AND f.delivery_tier IN ('stop_hook', 'prompt_hook', 'queue', 'inject')
                            AND f.resend_count < ?5 AND f.last_sent_at <= ?6))
                 ORDER BY f.created_at, f.id";

/// The JSON array of `ids`, without duplicates, for `json_each`.
pub(crate) fn id_array(ids: &[String]) -> String {
    let set: std::collections::BTreeSet<&String> = ids.iter().collect();
    serde_json::to_string(&set).expect("strings serialise")
}

/// The feedback rows of each thread, oldest first, each with what
/// [`Delivery`] reads of it.
pub(crate) const FEEDBACK_STATES: &str =
    "SELECT f.thread_id, f.target_session_id, f.created_at, f.delivered_at, f.delivery_tier, f.acknowledged_at,
            f.resend_count, f.untargeted_at, s.ended_at, s.harness, s.harness_session_id IS NOT NULL,
            COALESCE(w.replies_armed, 0), f.push_failed_at IS NOT NULL, f.placeholder_at,
            CASE WHEN s.person IS NULL THEN (SELECT display_name FROM viewers WHERE owner = 1)
                 ELSE (SELECT display_name FROM viewers WHERE public_id = s.person) END,
            f.delivered_at IS NULL AND EXISTS (
                SELECT 1 FROM approval_requests r INDEXED BY approval_requests_open
                 WHERE COALESCE(r.approver, '') = COALESCE(s.person, '') AND r.status = 'open'
                   AND r.artifact_id = t.artifact_id AND COALESCE(r.thread_id, '') = f.thread_id
                   AND r.author IN (SELECT c.author_public_id FROM comments c WHERE c.id = f.comment_id
                                    UNION ALL
                                    SELECT CASE pa.author WHEN '*owner'
                                             THEN (SELECT public_id FROM viewers WHERE owner = 1)
                                             ELSE pa.author END
                                      FROM page_authors pa
                                     WHERE pa.kind = 'comment' AND pa.id = f.comment_id)),
            f.last_sent_at,
            s.person, f.comment_id, t.artifact_id,
            (SELECT json_group_array(a) FROM (
                SELECT c.author_public_id AS a FROM comments c
                 WHERE c.id = f.comment_id AND c.author_public_id IS NOT NULL
                UNION ALL
                SELECT CASE pa.author WHEN '*owner'
                         THEN (SELECT public_id FROM viewers WHERE owner = 1)
                         ELSE pa.author END
                  FROM page_authors pa WHERE pa.kind = 'comment' AND pa.id = f.comment_id)),
            f.no_access_at
       FROM json_each(?1) j
      CROSS JOIN feedback f ON f.thread_id = j.value
       JOIN threads t ON t.id = f.thread_id
       LEFT JOIN sessions s ON s.id = f.target_session_id
       LEFT JOIN watches w ON w.session_id = f.target_session_id AND w.artifact_id = t.artifact_id
      ORDER BY f.thread_id, f.created_at, f.id";

#[derive(Clone, Debug, PartialEq)]
struct StateRow {
    target: Option<String>,
    created_at: String,
    delivered_at: Option<String>,
    tier: Option<String>,
    acknowledged_at: Option<String>,
    resends: u32,
    last_sent_at: Option<String>,
    untargeted_at: Option<String>,
    ended_at: Option<String>,
    harness: Option<String>,
    has_hsid: bool,
    armed: bool,
    push_failed: bool,
    /// When a take held it from its session and told it so.
    placeholder_at: Option<String>,
    /// When a take held it because its session may no longer read the
    /// artifact, and told it so.
    no_access_at: Option<String>,
    /// The display name of its session's person.
    person_name: Option<String>,
    /// Undelivered, and an open approval request of its session's person
    /// covers its comment's author there.
    requested: bool,
    /// When its person declined its comment's author there, while that
    /// decline is in force ([`Decline`]) and no request is open.
    declined: Option<String>,
    /// Its session's person (`None`: the owner's, or no session).
    person: Option<String>,
    comment_id: String,
    /// Its comment's authors' public IDs: the comment's own and, for a
    /// comment a page wrote, the page's authors (the held content's
    /// authors, spec §10).
    authors: Vec<String>,
}

impl StateRow {
    /// Undelivered, for a live session.
    fn pending(&self) -> bool {
        self.delivered_at.is_none() && self.target.is_some() && self.ended_at.is_none()
    }

    /// Whether the row waits for its person's approval (spec 2026-10-08
    /// §9.3, §10): pending ([`StateRow::pending`]) and held, either under
    /// an open request or told of (`placeholder_at`) with no decline in
    /// force. An approval releases it (`placeholder_at` cleared as the
    /// content becomes visible); a decline keeps it held but waiting for
    /// nothing. Whether its agent may still read the artifact does not
    /// change it: an author's reading never follows the approver's rights.
    fn waits_for_approval(&self) -> bool {
        self.pending()
            && (self.requested || (self.placeholder_at.is_some() && self.declined.is_none()))
    }

    /// Whether its person declined it (spec §10, "declined by Mia"):
    /// pending, no request open, and a decline of its author there in force.
    fn was_declined(&self) -> bool {
        self.pending() && !self.requested && self.declined.is_some()
    }

    /// Whether it is held because its session may no longer read the
    /// artifact (spec §7.5): pending, and told so (`no_access_at`).
    fn lost_access(&self) -> bool {
        self.pending() && self.no_access_at.is_some()
    }

    /// When a take held it from its session and told it so, while it is
    /// undelivered: the agent read that much of it.
    fn told(&self) -> Option<&str> {
        self.placeholder_at
            .as_deref()
            .filter(|_| self.delivered_at.is_none())
    }

    /// When it last changed: the newest of its own times and its session's
    /// end. Clearing `placeholder_at` (an approval) stamps nothing.
    fn stamp(&self) -> &str {
        [
            self.delivered_at.as_deref(),
            self.acknowledged_at.as_deref(),
            self.placeholder_at.as_deref(),
            self.no_access_at.as_deref(),
            self.untargeted_at.as_deref(),
            self.last_sent_at.as_deref(),
            self.ended_at.as_deref(),
        ]
        .into_iter()
        .flatten()
        .fold(self.created_at.as_str(), |a, b| a.max(b))
    }
}

/// Who reads a thread's delivery state (spec 2026-10-08 §10, §18): each
/// reads the rows [`Delivery::view`] shows it, and nothing of the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryViewer {
    /// The owner's browser, or the owner by hand: every row, whose
    /// approval named.
    Owner,
    /// A connected person's browser (a public ID): the rows of content
    /// they wrote (a comment, or the page that wrote one) and the rows
    /// sent to their own agents, whose approval named.
    Person(String),
    /// An agent (its session; `None`: none named): the rows sent to that
    /// session alone, whose approval never named; every row while nothing
    /// can be held (`every`: no person and the owner's `*unconnected`
    /// switch on, §8.7), as before people existed.
    Agent {
        session: Option<String>,
        every: bool,
    },
    /// A LAN viewer or an anonymous caller: no row. A sent thread reads
    /// "sent to <agent>" from its `sent_to_agent` and `owner`, never its
    /// progress.
    Visitor,
}

/// A thread's delivery rows (one per forwarded comment and target
/// session), from which each reader's state is read ([`Delivery::view`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    thread_id: String,
    rows: Vec<StateRow>,
    codex_push: bool,
}

impl Delivery {
    /// Whether `v` reads row `r` ([`DeliveryViewer`]).
    fn sees(r: &StateRow, v: &DeliveryViewer) -> bool {
        match v {
            DeliveryViewer::Owner => true,
            DeliveryViewer::Person(p) => {
                r.person.as_deref() == Some(p.as_str()) || r.authors.iter().any(|a| a == p)
            }
            DeliveryViewer::Agent { every: true, .. } => true,
            DeliveryViewer::Agent { session, .. } => {
                session.is_some() && r.target.as_deref() == session.as_deref()
            }
            DeliveryViewer::Visitor => false,
        }
    }

    fn visible(&self, v: &DeliveryViewer) -> Vec<&StateRow> {
        self.rows.iter().filter(|r| Self::sees(r, v)).collect()
    }

    /// The thread's delivery state as `v` reads it: the state of the rows
    /// it sees ([`state_of`]), `None` when it sees none. The one reading
    /// every channel serves (views, the page's boot payload, events,
    /// agents' reads), so none can say more than another (spec 2026-10-08
    /// §10, §18):
    /// - the owner and a person: `withheld` and `declined` with whose
    ///   approval, `mine` when it is theirs;
    /// - the owner and the person whose agent lost access: `no_access`,
    ///   with no approver;
    /// - an agent: `withheld` with no approver, and a declined thread's
    ///   plain state ([`plain_state`]);
    /// - a visitor: nothing.
    pub fn view(&self, v: &DeliveryViewer) -> Option<FeedbackState> {
        let rows = self.visible(v);
        if rows.is_empty() {
            return None;
        }
        // Who reads a hold for lost access (spec §7.5, §10): the owner, and
        // the person whose agent lost it; everyone else, the agent
        // included, reads such a row's plain state.
        let knows = |r: &StateRow| match v {
            DeliveryViewer::Owner => true,
            DeliveryViewer::Person(p) => r.person.as_deref() == Some(p.as_str()),
            _ => false,
        };
        let (mut s, by) = state_of(&self.thread_id, &rows, self.codex_push, &knows);
        match v {
            DeliveryViewer::Owner => s.mine = by.is_some_and(|p| p.is_none()),
            DeliveryViewer::Person(p) => {
                s.mine = by.is_some_and(|by| by.as_deref() == Some(p.as_str()));
            }
            DeliveryViewer::Agent { .. } => match s.state {
                FeedbackPhase::Withheld => s.approver = None,
                FeedbackPhase::Declined => s = plain_state(&self.thread_id, &rows, self.codex_push),
                _ => {}
            },
            // It reads no row ([`Delivery::sees`]), so never here.
            DeliveryViewer::Visitor => {}
        }
        Some(s)
    }

    /// The people with rows here: each row's authors and its session's
    /// person. A [`DeliveryViewer::Person`] not among them reads nothing.
    pub fn people(&self) -> std::collections::BTreeSet<String> {
        self.rows
            .iter()
            .flat_map(|r| r.authors.iter().cloned().chain(r.person.clone()))
            .collect()
    }

    /// The people a decision by and about `parties` (recipient keys; the
    /// owner's, empty, left out) concerns here: those parties, and everyone
    /// who wrote content alongside them, the authors of each row whose
    /// authors include a party (a page's co-authors, say).
    pub fn decision_people(&self, parties: &[String]) -> std::collections::BTreeSet<String> {
        let mut out: std::collections::BTreeSet<String> =
            parties.iter().filter(|k| !k.is_empty()).cloned().collect();
        for r in &self.rows {
            if r.authors.iter().any(|a| parties.contains(a)) {
                out.extend(r.authors.iter().cloned());
            }
        }
        out
    }

    /// Whether the newest change to the rows is one `v` sees: its rows'
    /// newest [`StateRow::stamp`] is the thread's. A change to rows `v`
    /// does not see (another author's delivery after an approval, say)
    /// tells it nothing, so it is not told of it.
    pub fn changed_for(&self, v: &DeliveryViewer) -> bool {
        let mine = self.visible(v).into_iter().map(StateRow::stamp).max();
        mine.is_some() && mine == self.rows.iter().map(StateRow::stamp).max()
    }
}

/// A decline in force (spec 2026-10-08 §10): `approver`'s (`None`: the
/// owner) decline of `author` at thread `thread`, decided at `at`, with no
/// approval of that author by that approver covering the thread made or
/// revoked since. A later approval or revocation ends it; a newer open
/// request covers its rows instead.
struct Decline {
    approver: Option<String>,
    author: String,
    thread: String,
    at: String,
}

/// The declined requests of the threads in `?1` (a JSON array of IDs),
/// through the index `approval_requests_by_thread`: never a walk of every
/// request.
pub(crate) const DECLINES: &str =
    "SELECT d.approver, d.author, d.thread_id, d.artifact_id, d.decided_at
       FROM approval_requests d
      WHERE d.thread_id IN (SELECT value FROM json_each(?1))
        AND d.status = 'declined' AND d.decided_at IS NOT NULL";

/// The declines in force at `thread_ids` ([`Decline`]).
fn declines_in(c: &Connection, thread_ids: &str) -> Result<Vec<Decline>> {
    let mut st = c.prepare_cached(DECLINES)?;
    let found = st
        .query_map(params![thread_ids], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (approver, author, thread, aid, at) in found {
        let since: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM approvals a
               WHERE COALESCE(a.approver, '') = COALESCE(?1, '') AND a.author = ?2
                 AND (a.scope = 'everywhere' OR (a.scope = 'artifact' AND a.artifact_id = ?3)
                      OR (a.scope = 'thread' AND a.thread_id = ?4))
                 AND (a.created_at > ?5 OR a.revoked_at > ?5))",
            params![approver, author, aid, thread, at],
            |r| r.get(0),
        )?;
        if !since {
            out.push(Decline {
                approver,
                author,
                thread,
                at,
            });
        }
    }
    Ok(out)
}

pub(crate) fn deliveries_in(
    c: &Connection,
    thread_ids: &[String],
    codex_push: bool,
) -> Result<BTreeMap<String, Delivery>> {
    let ids = id_array(thread_ids);
    let declines = declines_in(c, &ids)?;
    let mut by_thread: BTreeMap<String, Vec<StateRow>> = BTreeMap::new();
    let mut stmt = c.prepare_cached(FEEDBACK_STATES)?;
    let mut rows = stmt.query(params![ids])?;
    while let Some(r) = rows.next()? {
        let tid: String = r.get(0)?;
        let authors: Vec<String> = r
            .get::<_, Option<String>>(20)?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let person: Option<String> = r.get(17)?;
        let requested: bool = r.get(15)?;
        let declined = (!requested)
            .then(|| {
                declines.iter().find(|d| {
                    d.thread == tid && d.approver == person && authors.contains(&d.author)
                })
            })
            .flatten()
            .map(|d| d.at.clone());
        by_thread.entry(tid).or_default().push(StateRow {
            target: r.get(1)?,
            created_at: r.get(2)?,
            delivered_at: r.get(3)?,
            tier: r.get(4)?,
            acknowledged_at: r.get(5)?,
            resends: r.get(6)?,
            last_sent_at: r.get(16)?,
            untargeted_at: r.get(7)?,
            ended_at: r.get(8)?,
            harness: r.get(9)?,
            has_hsid: r.get::<_, Option<bool>>(10)?.unwrap_or(false),
            armed: r.get::<_, i64>(11)? != 0,
            push_failed: r.get(12)?,
            placeholder_at: r.get(13)?,
            no_access_at: r.get(21)?,
            person_name: r.get(14)?,
            requested,
            declined,
            person,
            comment_id: r.get(18)?,
            authors,
        });
    }
    Ok(by_thread
        .into_iter()
        .map(|(tid, rows)| {
            let d = Delivery {
                thread_id: tid.clone(),
                rows,
                codex_push,
            };
            (tid, d)
        })
        .collect())
}

/// The state of one thread from `all`, the rows its reader sees (not
/// empty), with whose approval it is held (`Some(None)`: the owner's;
/// `None` when it is not held): `withheld` while any row, of any of its
/// comments, waits for its person's approval
/// ([`StateRow::waits_for_approval`]; even when a later comment went
/// through); else `declined` while any row was declined
/// ([`StateRow::was_declined`]); else `no_access` while a row `knows`
/// lets its reader know of is held for lost access
/// ([`StateRow::lost_access`]); else the plain state ([`plain_state`]).
/// A withheld or declined reading carries `no_access` when such a row is
/// held for lost access too (spec §10).
fn state_of(
    thread_id: &str,
    all: &[&StateRow],
    codex_push: bool,
    knows: &dyn Fn(&StateRow) -> bool,
) -> (FeedbackState, Option<Option<String>>) {
    // Whether a reader that may know of it reads a hold for lost access
    // beside a withheld or declined reading.
    let gone = all.iter().any(|r| r.lost_access() && knows(r));
    let held = |phase, r: &StateRow, since: &str| {
        let s = FeedbackState {
            thread_id: thread_id.to_string(),
            state: phase,
            tier: None,
            since: since.to_string(),
            resends: 0,
            exhausted: false,
            approver: Some(clax_core::feedback::display_name(
                r.person_name.as_deref().unwrap_or(""),
            )),
            mine: false,
            no_access: gone,
        };
        (s, Some(r.person.clone()))
    };
    if let Some(r) = all.iter().find(|r| r.waits_for_approval()) {
        return held(
            FeedbackPhase::Withheld,
            r,
            r.placeholder_at.as_deref().unwrap_or(&r.created_at),
        );
    }
    if let Some(r) = all.iter().find(|r| r.was_declined()) {
        return held(
            FeedbackPhase::Declined,
            r,
            r.declined.as_deref().unwrap_or(&r.created_at),
        );
    }
    if let Some(r) = all.iter().find(|r| r.lost_access() && knows(r)) {
        let s = FeedbackState {
            thread_id: thread_id.to_string(),
            state: FeedbackPhase::NoAccess,
            tier: None,
            since: r.no_access_at.clone().unwrap_or_default(),
            resends: 0,
            exhausted: false,
            approver: None,
            mine: false,
            no_access: false,
        };
        return (s, None);
    }
    (plain_state(thread_id, all, codex_push), None)
}

/// The delivery state of `all`'s (not empty) newest comment's rows as if
/// nothing were held: a row a take held and told its session of reads as
/// acknowledged (seen by the agent) from then on.
fn plain_state(thread_id: &str, all: &[&StateRow], codex_push: bool) -> FeedbackState {
    let latest = &all.last().expect("rows").comment_id;
    let rows: Vec<&StateRow> = all
        .iter()
        .copied()
        .filter(|r| &r.comment_id == latest)
        .collect();
    let rows = rows.as_slice();
    let tier_of = |r: &StateRow| r.tier.as_deref().and_then(Tier::parse);
    let state = |state, tier, since: &str, resends, exhausted| FeedbackState {
        thread_id: thread_id.to_string(),
        state,
        tier,
        since: since.to_string(),
        resends,
        exhausted,
        approver: None,
        mine: false,
        no_access: false,
    };
    if let Some((r, at)) = rows
        .iter()
        .filter_map(|r| r.acknowledged_at.as_deref().or(r.told()).map(|at| (r, at)))
        .max_by(|a, b| a.1.cmp(b.1))
    {
        return state(
            FeedbackPhase::Acknowledged,
            tier_of(r),
            at,
            r.resends,
            false,
        );
    }
    let delivered: Vec<(&StateRow, &str)> = rows
        .iter()
        .filter_map(|r| r.delivered_at.as_deref().map(|at| (*r, at)))
        .collect();
    if let Some((first, at)) = delivered.iter().min_by(|a, b| a.1.cmp(b.1)) {
        let resends = delivered.iter().map(|(r, _)| r.resends).max().unwrap_or(0);
        let exhausted = delivered
            .iter()
            .all(|(r, _)| r.resends >= Store::MAX_RESENDS);
        return state(
            FeedbackPhase::Delivered,
            tier_of(first),
            at,
            resends,
            exhausted,
        );
    }
    if let Some(r) = rows
        .iter()
        .find(|r| r.target.is_some() && r.ended_at.is_none())
    {
        let push = codex_push && !r.push_failed;
        let tier = waiting_on(
            r.harness.as_deref().unwrap_or(""),
            r.has_hsid,
            r.armed,
            push,
        );
        return state(FeedbackPhase::Sent, Some(tier), &r.created_at, 0, false);
    }
    let since = rows
        .iter()
        .filter_map(|r| r.untargeted_at.clone().or_else(|| r.ended_at.clone()))
        .max()
        .unwrap_or_else(|| rows[0].created_at.clone());
    state(FeedbackPhase::AgentEnded, None, &since, 0, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crate::test_util::DAEMON;
    use crate::test_util::{anchor, artifact, session, store};
    use crate::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread};
    use clax_core::ArtifactId;
    use clax_core::feedback::{FeedbackPhase, Tier};

    const BASE: &str = "http://localhost:7480";

    fn thread(st: &Store, aid: &ArtifactId, body: &str) -> String {
        st.create_thread(
            DAEMON,
            aid,
            NewThread {
                author_public_id: None,
                version_n: 1,
                anchor: anchor(),
                author_name: "Alex".into(),
                body: body.into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn take(st: &Store, sid: &str, tier: Tier) -> Vec<FeedbackItem> {
        let q = TakeFeedback {
            session_id: sid.into(),
            tier,
            artifact_id: None,
            include_resends: true,
        };
        st.take_feedback(DAEMON, &q, BASE, &Visibility::all())
            .unwrap()
            .items
    }

    fn sorted_targets(st: &Store, tid: &str) -> Vec<Option<String>> {
        let mut t = targets(st, tid);
        t.sort();
        t
    }

    fn row(comment: &str, authors: &[&str], person: Option<&str>) -> StateRow {
        StateRow {
            target: Some("s".into()),
            created_at: "2026-10-15T00:00:00Z".into(),
            delivered_at: None,
            tier: None,
            acknowledged_at: None,
            resends: 0,
            last_sent_at: None,
            untargeted_at: None,
            ended_at: None,
            harness: Some("claude".into()),
            has_hsid: true,
            armed: false,
            push_failed: false,
            placeholder_at: None,
            no_access_at: None,
            person_name: Some("Sam".into()),
            requested: true,
            declined: None,
            person: person.map(str::to_string),
            comment_id: comment.into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
        }
    }

    /// Spec §10 (owner ruling on 9c75976c): an author's reading never
    /// changes because of the approver's rights. A row told as withheld
    /// whose agent then lost access reads, for its author, as withheld;
    /// the owner and the agent's person read both.
    #[test]
    fn a_withheld_row_whose_agent_lost_access_still_waits_for_approval_for_its_author() {
        let mut r = row("c1", &["u_mia"], Some("u_sam"));
        // Told it was withheld, no request open, then access was lost.
        r.requested = false;
        r.placeholder_at = Some("2026-10-15T00:01:00Z".into());
        r.no_access_at = Some("2026-10-15T00:02:00Z".into());
        let d = Delivery {
            thread_id: "t".into(),
            rows: vec![r],
            codex_push: false,
        };
        let mia = d.view(&DeliveryViewer::Person("u_mia".into())).unwrap();
        assert_eq!(mia.state, FeedbackPhase::Withheld);
        assert_eq!(mia.approver.as_deref(), Some("Sam"));
        assert!(
            !mia.no_access,
            "the author is not told of the agent's access"
        );
        let owner = d.view(&DeliveryViewer::Owner).unwrap();
        assert_eq!(
            (owner.state, owner.no_access),
            (FeedbackPhase::Withheld, true)
        );
        let sam = d.view(&DeliveryViewer::Person("u_sam".into())).unwrap();
        assert_eq!(
            (sam.state, sam.no_access, sam.mine),
            (FeedbackPhase::Withheld, true, true)
        );
        // The agent reads it as withheld, with no name and no access note.
        let agent = d
            .view(&DeliveryViewer::Agent {
                session: Some("s".into()),
                every: false,
            })
            .unwrap();
        assert_eq!(
            (agent.state, agent.no_access),
            (FeedbackPhase::Withheld, false)
        );
    }

    /// Spec §10 (a7145e76): with an approval request open, the owner and
    /// the agent's person read the hold for lost access beside the wait.
    #[test]
    fn an_open_request_on_a_row_whose_agent_lost_access_reads_both() {
        let mut r = row("c1", &["u_mia"], Some("u_sam"));
        r.no_access_at = Some("2026-10-15T00:02:00Z".into());
        let d = Delivery {
            thread_id: "t".into(),
            rows: vec![r],
            codex_push: false,
        };
        let owner = d.view(&DeliveryViewer::Owner).unwrap();
        assert_eq!(
            (owner.state, owner.no_access),
            (FeedbackPhase::Withheld, true)
        );
        let mia = d.view(&DeliveryViewer::Person("u_mia".into())).unwrap();
        assert_eq!((mia.state, mia.no_access), (FeedbackPhase::Withheld, false));
    }

    /// A row held only for lost access reads `no_access` to the owner and
    /// the agent's person, and plainly "sent" to everyone else, never seen
    /// or delivered.
    #[test]
    fn a_row_held_only_for_lost_access_reads_sent_to_its_author() {
        let mut r = row("c1", &["u_mia"], Some("u_sam"));
        r.requested = false;
        r.no_access_at = Some("2026-10-15T00:02:00Z".into());
        let d = Delivery {
            thread_id: "t".into(),
            rows: vec![r],
            codex_push: false,
        };
        let owner = d.view(&DeliveryViewer::Owner).unwrap();
        assert_eq!(owner.state, FeedbackPhase::NoAccess);
        assert_eq!(owner.since, "2026-10-15T00:02:00Z");
        assert_eq!(
            d.view(&DeliveryViewer::Person("u_sam".into()))
                .unwrap()
                .state,
            FeedbackPhase::NoAccess
        );
        assert_eq!(
            d.view(&DeliveryViewer::Person("u_mia".into()))
                .unwrap()
                .state,
            FeedbackPhase::Sent
        );
    }

    #[test]
    fn a_decision_concerns_its_parties_and_the_contents_co_authors_alone() {
        let d = Delivery {
            thread_id: "t".into(),
            rows: vec![
                row("c1", &["u_lucy", "u_mia", "u_sam"], Some("u_bob")),
                row("c2", &["u_kim"], Some("u_bob")),
            ],
            codex_push: false,
        };
        let told = |p: &[&str]| {
            let p: Vec<String> = p.iter().map(|s| s.to_string()).collect();
            d.decision_people(&p).into_iter().collect::<Vec<_>>()
        };
        assert_eq!(
            told(&["u_bob", "u_mia"]),
            ["u_bob", "u_lucy", "u_mia", "u_sam"],
            "the decider, and the page's comment's authors, not a later commenter"
        );
        assert_eq!(told(&["", "u_nobody"]), ["u_nobody"]);
        // Each co-author reads the hold of that content; the other
        // commenter reads only their own.
        let sam = d.view(&DeliveryViewer::Person("u_sam".into())).unwrap();
        assert_eq!(sam.state, FeedbackPhase::Withheld);
        assert_eq!(sam.approver.as_deref(), Some("Sam"));
    }

    #[test]
    fn a_named_send_reaches_each_named_agent_even_after_another_was_sent_the_thread() {
        let (_d, st) = store();
        let web = session(&st, "claude", "w");
        let api = session(&st, "codex", "a");
        let docs = session(&st, "pi", "d");
        let aid = artifact(&st, Some(&web));
        for s in [&api, &docs] {
            st.ensure_watch(DAEMON, s, &aid).unwrap();
        }
        let tid = thread(&st, &aid, "first");
        st.send_to(DAEMON, &tid, SendTarget::Agent(&api)).unwrap();
        assert_eq!(sorted_targets(&st, &tid), [Some(api.clone())]);
        let (t, touched) = st
            .send_to_named(DAEMON, &tid, &[web.clone(), docs.clone()])
            .unwrap();
        let mut want = vec![Some(api.clone()), Some(web.clone()), Some(docs.clone())];
        want.sort();
        assert_eq!(
            sorted_targets(&st, &tid),
            want,
            "one row each, the earlier kept"
        );
        assert!(t.sent_to_agent);
        assert!(touched.targets.contains(&web) && touched.targets.contains(&docs));
        let target: Option<String> = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT target_session_id FROM threads WHERE id = ?1",
                    [&tid],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(target, Some(web.clone()), "the first named is the target");
        st.send_to_named(DAEMON, &tid, std::slice::from_ref(&web))
            .unwrap();
        assert_eq!(sorted_targets(&st, &tid).len(), 3, "idempotent");
        // A session that neither owns nor watches the artifact: nothing.
        let stranger = session(&st, "grok", "s");
        assert!(matches!(
            st.send_to_named(DAEMON, &tid, &[web.clone(), stranger]),
            Err(CoreError::Invalid {
                code: "unknown_agent",
                ..
            })
        ));
        assert_eq!(sorted_targets(&st, &tid).len(), 3);
        // Untargeted rows become the first named agent's.
        let lone = artifact(&st, None);
        let t2 = thread(&st, &lone, "nobody watches");
        st.send_to_agent(DAEMON, &t2).unwrap();
        assert_eq!(sorted_targets(&st, &t2), [None]);
        st.ensure_watch(DAEMON, &api, &lone).unwrap();
        st.send_to_named(DAEMON, &t2, std::slice::from_ref(&api))
            .unwrap();
        assert_eq!(sorted_targets(&st, &t2), [Some(api.clone())]);
    }

    #[test]
    fn notices_announce_each_armed_undelivered_row_once() {
        let (_d, st) = store();
        let grok = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&grok));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(
            st.take_notices(&grok, "http://h:1", &Visibility::all(), None)
                .unwrap()
                .notices
                .is_empty(),
            "unarmed: no notice"
        );
        st.ensure_watch(DAEMON, &grok, &aid).unwrap();
        let n = st
            .take_notices(&grok, "http://h:1", &Visibility::all(), None)
            .unwrap()
            .notices;
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].thread_id, tid);
        assert_eq!(n[0].url, format!("http://h:1/a/{}", aid.as_str()));
        assert!(
            st.take_notices(&grok, "http://h:1", &Visibility::all(), None)
                .unwrap()
                .notices
                .is_empty(),
            "announced once"
        );
        // The notice delivered nothing: the Stop hook still hands the row over, once.
        assert_eq!(take(&st, &grok, Tier::StopHook).len(), 1);
        assert!(take(&st, &grok, Tier::StopHook).is_empty());
    }

    #[test]
    fn a_delivered_row_is_never_announced() {
        let (_d, st) = store();
        let grok = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&grok));
        st.ensure_watch(DAEMON, &grok, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        take(&st, &grok, Tier::Piggyback);
        assert!(
            st.take_notices(&grok, "http://h:1", &Visibility::all(), None)
                .unwrap()
                .notices
                .is_empty()
        );
    }

    #[test]
    fn a_retargeted_row_is_announced_to_its_new_session() {
        let (_d, st) = store();
        let first = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&first));
        st.ensure_watch(DAEMON, &first, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(
            st.take_notices(&first, "http://h:1", &Visibility::all(), None)
                .unwrap()
                .notices
                .len(),
            1
        );
        st.end_session(DAEMON, &first).unwrap();
        let next = session(&st, "grok", "g2");
        st.watch(DAEMON, &next, &aid, true).unwrap();
        st.retarget_untargeted(&aid, &next).unwrap();
        assert_eq!(
            st.take_notices(&next, "http://h:1", &Visibility::all(), None)
                .unwrap()
                .notices
                .len(),
            1
        );
    }

    fn targets(st: &Store, tid: &str) -> Vec<Option<String>> {
        st.feedback_rows(tid)
            .unwrap()
            .into_iter()
            .map(|f| f.target_session_id)
            .collect()
    }

    /// Moves every row's `last_sent_at` back past the resend window.
    fn age(st: &Store) {
        let old = (chrono::Utc::now() - std::time::Duration::from_secs(200))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        st.with_write(|c| {
            c.execute(
                "UPDATE feedback SET last_sent_at = ?1 WHERE last_sent_at IS NOT NULL",
                [&old],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn owner_only() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        let (t, touched) = st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(t.sent_to_agent);
        assert_eq!(targets(&st, &tid), vec![Some(owner.clone())]);
        assert!(touched.targets.contains(&owner));
        assert!(
            touched
                .threads
                .contains(&(aid.as_str().to_string(), tid.clone()))
        );
    }

    #[test]
    fn owner_and_live_watchers_but_not_ended_ones() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let w1 = session(&st, "codex", "w1");
        let w2 = session(&st, "pi", "w2");
        st.watch(DAEMON, &w1, &aid, true).unwrap();
        st.watch(DAEMON, &w2, &aid, false).unwrap();
        st.end_session(DAEMON, &w2).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let mut got = targets(&st, &tid);
        got.sort();
        let mut want = vec![Some(owner), Some(w1)];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn no_live_session_leaves_one_untargeted_row_then_retargets_on_watch() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        st.end_session(DAEMON, &owner).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(targets(&st, &tid), vec![None]);
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::AgentEnded
        );
        let late = session(&st, "claude", "late");
        st.watch(DAEMON, &late, &aid, true).unwrap();
        let touched = st.retarget_untargeted(&aid, &late).unwrap();
        assert!(touched.targets.contains(&late));
        assert_eq!(targets(&st, &tid), vec![Some(late.clone())]);
        assert_eq!(take(&st, &late, Tier::Piggyback).len(), 1);
    }

    #[test]
    fn comments_withdrawn_by_a_resolve_are_sent_again_when_a_comment_reopens() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "first");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let delivered = take(&st, &owner, Tier::Piggyback);
        assert_eq!(delivered.len(), 1);
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: "unseen".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        st.resolve_thread(DAEMON, &tid, "viewer:anonymous").unwrap();
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: "reopening".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["unseen", "reopening"],
            "the withdrawn comment is sent again; the delivered one is not"
        );
    }

    #[test]
    fn send_is_idempotent_and_later_viewer_comments_are_forwarded() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "first");
        st.send_to_agent(DAEMON, &tid).unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(st.feedback_rows(&tid).unwrap().len(), 1);
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_AGENT,
                author_name: "claude".into(),
                via_session_id: Some(owner.clone()),
                body: "on it".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(
            st.feedback_rows(&tid).unwrap().len(),
            1,
            "agent comments are never forwarded"
        );
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: "second".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    fn viewer_says(st: &Store, tid: &str, body: &str) {
        st.add_comment(
            DAEMON,
            tid,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: body.into(),
                via_page: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn a_send_to_one_agent_is_followed_by_later_comments_until_it_ends() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let watcher = session(&st, "codex", "w");
        let aid = artifact(&st, Some(&owner));
        st.ensure_watch(DAEMON, &watcher, &aid).unwrap();
        let tid = thread(&st, &aid, "first");
        let stranger = session(&st, "codex", "s");
        let err = st
            .send_to(DAEMON, &tid, SendTarget::Agent(&stranger))
            .unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                code: "unknown_agent",
                ..
            }
        ));
        assert!(
            st.feedback_rows(&tid).unwrap().is_empty(),
            "nothing was written"
        );
        st.send_to(DAEMON, &tid, SendTarget::Agent(&watcher))
            .unwrap();
        viewer_says(&st, &tid, "second");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let bodies = |sid: &str| {
            take(&st, sid, Tier::Piggyback)
                .into_iter()
                .map(|i| i.body)
                .collect::<Vec<_>>()
        };
        assert_eq!(bodies(&watcher), ["first", "second"]);
        assert!(bodies(&owner).is_empty());
        st.end_session(DAEMON, &watcher).unwrap();
        viewer_says(&st, &tid, "third");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(
            bodies(&owner),
            ["third"],
            "the target ended: the comment fans out"
        );
        let second = session(&st, "codex", "w2");
        st.ensure_watch(DAEMON, &second, &aid).unwrap();
        st.send_to(DAEMON, &tid, SendTarget::Agent(&second))
            .unwrap();
        // The send to `second` gave it every comment it had no row for.
        assert_eq!(bodies(&second), ["first", "second", "third"]);
        st.send_to(DAEMON, &tid, SendTarget::Everyone).unwrap();
        viewer_says(&st, &tid, "fourth");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(bodies(&owner), ["fourth"], "Everyone cleared the target");
        // The first take of the untargeted send owns the thread (M23):
        // `second`'s row of it is gone.
        assert!(bodies(&second).is_empty());
    }

    #[test]
    fn a_whole_artifact_thread_is_stored_on_no_page_sent_and_rendered() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let before = st
            .events_after(0, 1000)
            .unwrap()
            .last()
            .map_or(0, |r| r.seq);
        let whole: clax_core::Anchor =
            serde_json::from_value(serde_json::json!({"kind": "artifact"})).unwrap();
        let t = st
            .create_thread(
                DAEMON,
                &aid,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: whole,
                    author_name: "Alex".into(),
                    body: "Make the colours warmer.".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(t.anchor.kind, clax_core::AnchorKind::Artifact);
        assert_eq!(t.anchor.file, "", "an artifact anchor is on no page");
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap(), t);
        let open = st
            .events_after(before, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.kind == "thread.open")
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&open.body).unwrap();
        assert_eq!(body["anchor"]["kind"], "artifact");
        assert_eq!(body["anchor"]["file"], "");
        st.send_to_agent(DAEMON, &t.id).unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(items.len(), 1);
        assert!(items[0].anchor.as_ref().unwrap().is_whole());
        let text = clax_core::feedback::render_item(&items[0]);
        assert!(
            text.contains("\nAnchored on: the whole artifact  (v1)\n"),
            "{text}"
        );
    }

    #[test]
    fn piggyback_delivers_once_and_acknowledges() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(items.len(), 1);
        let i = &items[0];
        assert_eq!(
            (
                i.artifact_title.as_str(),
                i.version,
                i.author.as_str(),
                i.resent
            ),
            ("Quarterly Review", 1, "Alex", false)
        );
        assert_eq!(i.url, format!("{BASE}/a/{aid}"));
        let row = &st.feedback_rows(&tid).unwrap()[0];
        assert_eq!(row.delivery_tier.as_deref(), Some("piggyback"));
        assert!(row.acknowledged_at.is_some());
        assert!(take(&st, &owner, Tier::Piggyback).is_empty());
        age(&st);
        assert!(
            take(&st, &owner, Tier::Piggyback).is_empty(),
            "acknowledged rows are never resent"
        );
    }

    #[test]
    fn armed_only_tiers_skip_unarmed_watches_and_prompt_hook_does_not() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        st.watch(DAEMON, &owner, &aid, false).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(take(&st, &owner, Tier::StopHook).is_empty());
        assert!(take(&st, &owner, Tier::Queue).is_empty());
        let p = take(&st, &owner, Tier::PromptHook);
        assert_eq!(p.len(), 1);
        let row = &st.feedback_rows(&tid).unwrap()[0];
        assert_eq!(row.delivery_tier.as_deref(), Some("prompt_hook"));
        assert!(row.acknowledged_at.is_none());
    }

    #[test]
    fn push_deliveries_are_resent_in_band_at_most_three_times() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        st.ensure_watch(DAEMON, &owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(take(&st, &owner, Tier::StopHook).len(), 1);
        assert!(
            take(&st, &owner, Tier::StopHook).is_empty(),
            "not yet 2 minutes"
        );
        for n in 1..=3 {
            age(&st);
            let r = take(&st, &owner, Tier::StopHook);
            assert_eq!(r.len(), 1, "resend {n}");
            assert!(r[0].resent);
        }
        age(&st);
        assert!(
            take(&st, &owner, Tier::StopHook).is_empty(),
            "three resends at most"
        );
        assert!(take(&st, &owner, Tier::Piggyback).is_empty());
        let s = st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(
            (s.state, s.resends, s.exhausted),
            (FeedbackPhase::Delivered, 3, true)
        );
    }

    #[test]
    fn piggyback_resend_acknowledges_and_resends_can_be_excluded() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        st.ensure_watch(DAEMON, &owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        take(&st, &owner, Tier::StopHook);
        age(&st);
        let q = TakeFeedback {
            session_id: owner.clone(),
            tier: Tier::StopHook,
            artifact_id: None,
            include_resends: false,
        };
        assert!(
            st.take_feedback(DAEMON, &q, BASE, &Visibility::all())
                .unwrap()
                .items
                .is_empty(),
            "stop_hook_active excludes resends"
        );
        assert!(
            take(&st, &owner, Tier::PromptHook).is_empty(),
            "prompt_hook never resends"
        );
        let r = take(&st, &owner, Tier::Piggyback);
        assert!(r[0].resent);
        assert!(st.feedback_rows(&tid).unwrap()[0].acknowledged_at.is_some());
    }

    #[test]
    fn acknowledge_stops_resends_and_marks_undelivered_rows_delivered() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let touched = st
            .acknowledge(
                DAEMON,
                &owner,
                std::slice::from_ref(&tid),
                &Visibility::all(),
                None,
            )
            .unwrap();
        assert!(
            touched
                .threads
                .contains(&(aid.as_str().to_string(), tid.clone()))
        );
        let row = &st.feedback_rows(&tid).unwrap()[0];
        assert!(row.acknowledged_at.is_some() && row.delivered_at.is_some());
        assert_eq!(row.delivery_tier.as_deref(), Some("piggyback"));
        assert!(take(&st, &owner, Tier::Piggyback).is_empty());
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Acknowledged
        );
    }

    #[test]
    fn acknowledging_comments_leaves_rows_for_later_comments_unacked() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let other = session(&st, "codex", "x");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "first");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let first = st.get_thread(&tid).unwrap().unwrap().comments[0].id.clone();
        let later = st
            .add_comment(
                DAEMON,
                &tid,
                NewComment {
                    author_public_id: None,
                    author_kind: AUTHOR_VIEWER,
                    author_name: "Alex".into(),
                    via_session_id: None,
                    body: "later".into(),
                    via_page: false,
                },
            )
            .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(
            st.acknowledge_comments(
                DAEMON,
                &other,
                std::slice::from_ref(&first),
                &Visibility::all(),
                None,
            )
            .unwrap()
            .is_empty(),
            "another session's acknowledgement touches nothing"
        );
        let touched = st
            .acknowledge_comments(
                DAEMON,
                &owner,
                std::slice::from_ref(&first),
                &Visibility::all(),
                None,
            )
            .unwrap();
        assert!(
            touched
                .threads
                .contains(&(aid.as_str().to_string(), tid.clone()))
        );
        let rows = st.feedback_rows(&tid).unwrap();
        let acked = |cid: &str| {
            rows.iter()
                .find(|r| r.comment_id == cid)
                .unwrap()
                .acknowledged_at
                .is_some()
        };
        assert!(acked(&first));
        assert!(
            !acked(&later.id),
            "a comment the reader never saw stays pending"
        );
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["later"]
        );
    }

    #[test]
    fn release_returns_queue_claims_to_undelivered() {
        let (_d, st) = store();
        let owner = session(&st, "codex", "cx");
        let aid = artifact(&st, Some(&owner));
        st.ensure_watch(DAEMON, &owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let claimed = take(&st, &owner, Tier::Queue);
        assert_eq!(claimed.len(), 1);
        st.release_feedback(DAEMON, &[claimed[0].feedback_id.clone()], "test")
            .unwrap();
        let row = &st.feedback_rows(&tid).unwrap()[0];
        assert_eq!(
            (row.delivered_at.clone(), row.delivery_tier.clone()),
            (None, None)
        );
        assert_eq!(take(&st, &owner, Tier::Piggyback).len(), 1);
    }

    #[test]
    fn a_released_queue_claim_waits_on_the_in_band_tiers_and_is_not_queued_again() {
        let (_d, st) = store();
        let owner = session(&st, "codex", "cx");
        let aid = artifact(&st, Some(&owner));
        st.watch(DAEMON, &owner, &aid, true).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(
            st.feedback_state(&tid, true).unwrap().unwrap().tier,
            Some(Tier::Queue)
        );
        let claimed = take(&st, &owner, Tier::Queue);
        st.release_feedback(DAEMON, &[claimed[0].feedback_id.clone()], "test")
            .unwrap();
        let s = st.feedback_state(&tid, true).unwrap().unwrap();
        assert_eq!(
            (s.state, s.tier),
            (FeedbackPhase::Sent, Some(Tier::StopHook))
        );
        assert!(
            take(&st, &owner, Tier::Queue).is_empty(),
            "never queued twice"
        );
        // Handed to a new Codex session, the row may be queued for it.
        st.end_session(DAEMON, &owner).unwrap();
        let next = session(&st, "codex", "cx2");
        st.watch(DAEMON, &next, &aid, true).unwrap();
        st.retarget_untargeted(&aid, &next).unwrap();
        assert_eq!(
            st.feedback_state(&tid, true).unwrap().unwrap().tier,
            Some(Tier::Queue)
        );
        assert_eq!(take(&st, &next, Tier::Queue).len(), 1);
    }

    #[test]
    fn ending_a_session_drops_watches_and_hands_rows_on_without_duplicates() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let other = session(&st, "codex", "w");
        st.watch(DAEMON, &other, &aid, true).unwrap();
        st.ensure_watch(DAEMON, &owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let touched = st.end_session_touched(DAEMON, &owner).unwrap().touched;
        assert!(
            touched
                .threads
                .contains(&(aid.as_str().to_string(), tid.clone()))
        );
        assert!(st.list_watches(&owner).unwrap().is_empty());
        assert_eq!(
            targets(&st, &tid),
            vec![Some(other.clone())],
            "a live target remains, so the ended one's row is dropped"
        );
        st.end_session(DAEMON, &other).unwrap();
        assert_eq!(targets(&st, &tid), vec![None]);
        let next = session(&st, "claude", "n");
        st.retarget_untargeted(&aid, &next).unwrap();
        st.retarget_untargeted(&aid, &next).unwrap();
        assert_eq!(targets(&st, &tid), vec![Some(next)]);
    }

    #[test]
    fn an_armed_grok_watch_waits_on_the_stop_hook() {
        let (_d, st) = store();
        let grok = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&grok));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().tier,
            Some(Tier::Piggyback),
            "unarmed: the next tool call"
        );
        st.ensure_watch(DAEMON, &grok, &aid).unwrap();
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().tier,
            Some(Tier::StopHook)
        );
    }

    #[test]
    fn feedback_state_follows_the_row_through_its_life() {
        let (_d, st) = store();
        let claude = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&claude));
        let tid = thread(&st, &aid, "hi");
        assert_eq!(st.feedback_state(&tid, false).unwrap(), None);
        st.send_to_agent(DAEMON, &tid).unwrap();
        let s = st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(
            (s.state, s.tier),
            (FeedbackPhase::Sent, Some(Tier::Piggyback)),
            "unarmed owner waits on its next tool call"
        );
        st.ensure_watch(DAEMON, &claude, &aid).unwrap();
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().tier,
            Some(Tier::StopHook)
        );
        take(&st, &claude, Tier::StopHook);
        let s = st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(
            (s.state, s.tier),
            (FeedbackPhase::Delivered, Some(Tier::StopHook))
        );
        st.acknowledge(
            DAEMON,
            &claude,
            std::slice::from_ref(&tid),
            &Visibility::all(),
            None,
        )
        .unwrap();
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Acknowledged
        );

        let codex = session(&st, "codex", "cx");
        let a2 = artifact(&st, Some(&codex));
        st.ensure_watch(DAEMON, &codex, &a2).unwrap();
        let t2 = thread(&st, &a2, "hi");
        st.send_to_agent(DAEMON, &t2).unwrap();
        assert_eq!(
            st.feedback_state(&t2, true).unwrap().unwrap().tier,
            Some(Tier::Queue)
        );
        assert_eq!(
            st.feedback_state(&t2, false).unwrap().unwrap().tier,
            Some(Tier::StopHook)
        );

        let pi = session(&st, "pi", "p");
        let a3 = artifact(&st, Some(&pi));
        st.ensure_watch(DAEMON, &pi, &a3).unwrap();
        let t3 = thread(&st, &a3, "hi");
        st.send_to_agent(DAEMON, &t3).unwrap();
        assert_eq!(
            st.feedback_state(&t3, false).unwrap().unwrap().tier,
            Some(Tier::Inject)
        );
        st.end_session(DAEMON, &pi).unwrap();
        let s = st.feedback_state(&t3, false).unwrap().unwrap();
        assert_eq!((s.state, s.tier), (FeedbackPhase::AgentEnded, None));
    }

    #[test]
    fn deleted_artifacts_feedback_is_never_taken() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        st.delete_artifact(DAEMON, &aid).unwrap();
        for tier in [Tier::Piggyback, Tier::Wait, Tier::PromptHook] {
            assert!(take(&st, &owner, tier).is_empty(), "{tier:?}");
        }
        assert!(matches!(
            st.send_to_agent(DAEMON, &tid),
            Err(clax_core::CoreError::NotFound)
        ));
    }

    #[test]
    fn corrupt_anchor_rows_are_skipped_and_left_undelivered() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let bad = thread(&st, &aid, "bad");
        let good = thread(&st, &aid, "good");
        st.send_to_agent(DAEMON, &bad).unwrap();
        st.send_to_agent(DAEMON, &good).unwrap();
        st.with_write(|c| {
            c.execute("UPDATE threads SET anchor_json = '{' WHERE id = ?1", [&bad])?;
            Ok(())
        })
        .unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
        let row = &st.feedback_rows(&bad).unwrap()[0];
        assert_eq!(
            (row.delivered_at.clone(), row.acknowledged_at.clone()),
            (None, None)
        );
    }

    #[test]
    fn rows_with_a_corrupt_artifact_id_are_skipped_and_left_undelivered() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let a1 = artifact(&st, Some(&owner));
        let a2 = artifact(&st, Some(&owner));
        let clip = Some(b"\x89PNG\r\n\x1a\nx".to_vec());
        let bad = st
            .create_thread(
                DAEMON,
                &a1,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "A".into(),
                    body: "bad".into(),
                    clip,
                    via_page: false,
                },
            )
            .unwrap()
            .id;
        let good = thread(&st, &a2, "good");
        st.send_to_agent(DAEMON, &bad).unwrap();
        st.send_to_agent(DAEMON, &good).unwrap();
        st.with_write(|c| {
            c.execute_batch("PRAGMA foreign_keys=OFF")?;
            c.execute(
                "UPDATE artifacts SET id = 'NOT-AN-ID' WHERE id = ?1",
                [a1.as_str()],
            )?;
            c.execute(
                "UPDATE threads SET artifact_id = 'NOT-AN-ID' WHERE id = ?1",
                [&bad],
            )?;
            c.execute_batch("PRAGMA foreign_keys=ON")?;
            Ok(())
        })
        .unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
        let row = &st.feedback_rows(&bad).unwrap()[0];
        assert_eq!(
            (row.delivered_at.clone(), row.acknowledged_at.clone()),
            (None, None)
        );
    }

    #[test]
    fn resolved_threads_pending_rows_are_not_taken() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        st.resolve_thread(DAEMON, &tid, "viewer:x").unwrap();
        assert!(take(&st, &owner, Tier::Piggyback).is_empty());
    }

    #[test]
    fn resolving_deletes_undelivered_rows_and_touches_the_thread() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        let (t, touched) = st.resolve_thread_touched(DAEMON, &tid, "viewer:x").unwrap();
        assert_eq!(t.status, "resolved");
        assert!(st.feedback_rows(&tid).unwrap().is_empty());
        assert!(
            touched
                .threads
                .contains(&(aid.as_str().to_string(), tid.clone()))
        );
        assert!(touched.targets.is_empty());
        assert_eq!(st.feedback_state(&tid, false).unwrap(), None);
    }

    #[test]
    fn resolving_keeps_delivered_rows() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(take(&st, &owner, Tier::PromptHook).len(), 1);
        st.resolve_thread(DAEMON, &tid, "viewer:x").unwrap();
        assert_eq!(st.feedback_rows(&tid).unwrap().len(), 1);
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Delivered
        );
    }

    #[test]
    fn artifact_filter_and_clip_paths() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let a1 = artifact(&st, Some(&owner));
        let a2 = artifact(&st, Some(&owner));
        let t1 = st
            .create_thread(
                DAEMON,
                &a1,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "".into(),
                    body: "one".into(),
                    clip: Some(b"\x89PNG\r\n\x1a\nx".to_vec()),
                    via_page: false,
                },
            )
            .unwrap();
        let t2 = thread(&st, &a2, "two");
        st.send_to_agent(DAEMON, &t1.id).unwrap();
        st.send_to_agent(DAEMON, &t2).unwrap();
        let q = TakeFeedback {
            session_id: owner.clone(),
            tier: Tier::Wait,
            artifact_id: Some(a1.as_str().into()),
            include_resends: true,
        };
        let items = st
            .take_feedback(DAEMON, &q, BASE, &Visibility::all())
            .unwrap()
            .items;
        assert_eq!(items.len(), 1);
        assert!(clax_core::feedback::render_item(&items[0]).contains("\nViewer: \"one\"\n"));
        // A clip's path when the content store keeps clips in files, and
        // none when it keeps them elsewhere.
        match st.local_clip(&a1, &t1.id) {
            Some(_) => {
                let clip = items[0].clip_path.clone().unwrap();
                assert!(
                    std::path::Path::new(&clip).is_absolute()
                        && std::path::Path::new(&clip).exists(),
                    "{clip}"
                );
            }
            None => assert_eq!(items[0].clip_path, None),
        }
        assert_eq!(take(&st, &owner, Tier::Wait)[0].body, "two");
    }
}

/// Delivery to an agent of another person, and to the owner's agents once
/// people are connected (spec 2026-10-08 §8.3, §8.4, §9.4, M16, M17).
#[cfg(test)]
mod delivery {
    use super::*;
    use crate::approvals::{Scope, Via};
    use crate::people::tests::{approved, store};
    use crate::test_util::{DAEMON, anchor};
    use crate::threads::{AUTHOR_VIEWER, NewComment, NewThread};
    use crate::{RegisterSession, Store};
    use clax_core::feedback::render_delivery;
    use clax_core::publish::{PublishRequest, validate};

    const BASE: &str = "http://h";
    const MIA_TITLE: &str = "Mia's secret plan";
    const MIA_QUOTE: &str = "Mia's quoted words";

    struct Two {
        _dir: tempfile::TempDir,
        clock: std::sync::Arc<clax_core::working::ManualClock>,
        st: Store,
        owner: String,
        sam: String,
        mia: String,
        /// Sam's agent's session.
        sid: String,
    }

    fn two_people() -> Two {
        let (dir, clock, st) = store();
        let owner = st.owner_viewer(true).unwrap().public_id;
        let (sam_grant, _, _) = approved(&st, "Sam", [1; 32]);
        let sam = st.viewer_of_grant(&sam_grant).unwrap().public_id;
        let (mia_grant, _, _) = approved(&st, "Mia", [2; 32]);
        let mia = st.viewer_of_grant(&mia_grant).unwrap().public_id;
        let sid = st
            .register_session(
                DAEMON,
                RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("sam-h1".into()),
                    cwd: "/w".into(),
                    grant_id: Some(sam_grant),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        assert_eq!(
            st.get_session(&sid).unwrap().unwrap().person.as_deref(),
            Some(sam.as_str())
        );
        Two {
            _dir: dir,
            clock,
            st,
            owner,
            sam,
            mia,
            sid,
        }
    }

    impl Two {
        /// An artifact `person` publishes, titled `title`, whose index.html
        /// holds `quote`; Sam's agent watches it with replies armed.
        fn publish(&self, person: &str, title: &str, quote: &str) -> ArtifactId {
            let req: PublishRequest = serde_json::from_value(serde_json::json!({
                "title": title,
                "files": {"index.html": {"content": format!("<main><h2>{quote}</h2></main>"), "encoding": "utf8"}}
            }))
            .unwrap();
            let mut p = validate(req).unwrap();
            p.person = Some(person.to_string());
            let (a, _) = self.st.create_artifact(DAEMON, p, None).unwrap();
            let aid = ArtifactId::parse(&a.id).unwrap();
            self.st.ensure_watch(DAEMON, &self.sid, &aid).unwrap();
            aid
        }

        /// A thread on `aid` by viewer `by`, quoting `quote`, with a clip
        /// and `n` comments in all; sent to the agents.
        fn thread(&self, aid: &ArtifactId, by: &str, body: &str, quote: &str, n: usize) -> String {
            let name = if by == self.mia { "Mia" } else { "Alex" };
            let t = self
                .st
                .create_thread(
                    DAEMON,
                    aid,
                    NewThread {
                        version_n: 1,
                        anchor: clax_core::Anchor {
                            quote: Some(quote.into()),
                            ..anchor()
                        },
                        author_name: name.into(),
                        author_public_id: Some(by.to_string()),
                        body: body.into(),
                        clip: Some(b"\x89PNG\r\n\x1a\nclip".to_vec()),
                        via_page: false,
                    },
                )
                .unwrap();
            for i in 1..n {
                self.st
                    .add_comment(
                        DAEMON,
                        &t.id,
                        NewComment {
                            author_kind: AUTHOR_VIEWER,
                            author_name: name.into(),
                            author_public_id: Some(by.to_string()),
                            via_session_id: None,
                            body: format!("{body} {i}"),
                            via_page: false,
                        },
                    )
                    .unwrap();
            }
            self.st.send_to_agent(DAEMON, &t.id).unwrap();
            t.id
        }

        fn sam_vis(&self) -> Visibility {
            Visibility::for_session(&self.st, &self.sid).unwrap()
        }

        fn take(&self, tier: Tier, vis: &Visibility) -> Taken {
            self.st
                .take_feedback(
                    DAEMON,
                    &TakeFeedback {
                        session_id: self.sid.clone(),
                        tier,
                        artifact_id: None,
                        include_resends: true,
                    },
                    BASE,
                    vis,
                )
                .unwrap()
        }

        fn approve(&self, author: &str, scope: Scope) -> String {
            self.st
                .approve(DAEMON, Some(&self.sam), author, scope, Via::Shell)
                .unwrap()
                .id
        }

        fn rows(&self, tid: &str) -> Vec<Feedback> {
            self.st
                .feedback_rows(tid)
                .unwrap()
                .into_iter()
                .filter(|f| f.target_session_id.as_deref() == Some(&self.sid))
                .collect()
        }
    }

    /// Mia's `n` comments on an artifact of Sam's, sent to Sam's agent.
    fn mia_sends_to_sams_agent(w: &Two, n: usize) -> String {
        let aid = w.publish(&w.sam, "Sam's plan", "Sam's words");
        w.thread(&aid, &w.mia, "MIACANARY", "Sam's words", n)
    }

    /// Another live agent of Sam's, on Sam's grant.
    fn sams_second_agent(w: &Two) -> String {
        let grant = w.st.get_session(&w.sid).unwrap().unwrap().grant_id;
        w.st.register_session(
            DAEMON,
            RegisterSession {
                harness: "codex".into(),
                harness_session_id: Some("sam-h2".into()),
                cwd: "/w".into(),
                grant_id: grant,
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    /// An owner agent watching `aid`.
    fn owner_agent(w: &Two, aid: &ArtifactId) -> String {
        let sid = crate::test_util::session(&w.st, "claude", "owner-h1");
        w.st.ensure_watch(DAEMON, &sid, aid).unwrap();
        sid
    }

    fn agent_reply(sid: &str) -> NewComment {
        NewComment {
            author_kind: crate::threads::AUTHOR_AGENT,
            author_name: "claude".into(),
            author_public_id: None,
            via_session_id: Some(sid.to_string()),
            body: "on it".into(),
            via_page: false,
        }
    }

    /// Spec §16: a grant session's payload names its clip by reference,
    /// never by a path on the daemon's machine; the daemon token's agents
    /// read the clip's file where the content is files.
    #[test]
    fn a_grant_sessions_clip_is_a_reference() {
        let w = two_people();
        let aid = w.publish(&w.sam, "Sam's plan", "Sam's words");
        let owners = owner_agent(&w, &aid);
        let tid = w.thread(&aid, &w.sam, "SAMWORDS", "Sam's words", 1);
        let items = w.take(Tier::Wait, &w.sam_vis()).items;
        assert_eq!(
            items[0].clip_path.as_deref(),
            Some(clax_core::clips::reference(aid.as_str(), &tid).as_str())
        );
        let text = clax_core::feedback::render_item(&items[0]);
        assert!(
            text.contains(&format!("\nClip: clax-clip:{}/{tid}\n", aid.as_str())),
            "{text}"
        );
        let mine =
            w.st.take_feedback(
                DAEMON,
                &TakeFeedback {
                    session_id: owners,
                    tier: Tier::Wait,
                    artifact_id: None,
                    include_resends: true,
                },
                BASE,
                &Visibility::all(),
            )
            .unwrap()
            .items;
        // A local agent reads the clip's file, where the content is files;
        // content kept in memory (a Durable Object's) has no file to name.
        let path = mine[0].clip_path.clone();
        if crate::test_util::on_one_connection() {
            assert_eq!(path, None);
        } else {
            let path = path.unwrap();
            assert!(std::path::Path::new(&path).is_absolute(), "{path}");
            assert!(std::path::Path::new(&path).is_file(), "{path}");
        }
    }

    #[test]
    fn another_persons_agent_takes_nothing_implicitly() {
        let w = two_people();
        // The owner's untargeted send on the owner's artifact; Sam lets the
        // owner's words reach his agents, and his agent takes first.
        let aid = w.publish(&w.owner, "Owner's plan", "Owner's words");
        let owners = owner_agent(&w, &aid);
        w.approve(&w.owner, Scope::Everywhere);
        let tid = w.thread(&aid, &w.owner, "OWNERWORDS", "Owner's words", 1);
        assert_eq!(w.take(Tier::Piggyback, &w.sam_vis()).items.len(), 1);
        assert_eq!(
            w.st.thread_owner(&tid).unwrap(),
            None,
            "Sam's agent owns nothing"
        );
        // Its reply takes nothing either.
        w.st.add_agent_reply(DAEMON, &tid, agent_reply(&w.sid), None)
            .unwrap();
        assert_eq!(w.st.thread_owner(&tid).unwrap(), None);
        // The owner's agent keeps its row, and its take owns the thread.
        let q = TakeFeedback {
            session_id: owners.clone(),
            tier: Tier::Piggyback,
            artifact_id: None,
            include_resends: false,
        };
        let vis = Visibility::for_session(&w.st, &owners).unwrap();
        assert_eq!(
            w.st.take_feedback(DAEMON, &q, BASE, &vis)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(w.st.thread_owner(&tid).unwrap().unwrap().session_id, owners);
        // Sam's agent cannot hand it off now, nor reply.
        assert!(matches!(
            w.st.handoff(DAEMON, &tid, &w.sid, None),
            Err(CoreError::ThreadTaken { .. })
        ));
    }

    #[test]
    fn a_blind_reply_takes_nothing() {
        let w = two_people();
        // Mia's comment, withheld from Sam's agent, on the owner's artifact.
        let aid = w.publish(&w.owner, "Owner's plan", "Owner's words");
        let tid = w.thread(&aid, &w.mia, "MIAWORDS", "Owner's words", 1);
        assert_eq!(w.take(Tier::Piggyback, &w.sam_vis()).placeholders.len(), 1);
        w.st.add_agent_reply(DAEMON, &tid, agent_reply(&w.sid), None)
            .unwrap();
        assert_eq!(w.st.thread_owner(&tid).unwrap(), None);
        // Sam's held row stays for his approval; nothing was deleted.
        assert_eq!(w.rows(&tid).len(), 1);
        // Nor may it release or hand off the unowned thread.
        assert!(matches!(
            w.st.handoff(DAEMON, &tid, &w.sid, None),
            Err(CoreError::ThreadTaken { .. })
        ));
    }

    #[test]
    fn an_ended_owners_thread_is_handed_off_only_by_its_person_or_a_taker() {
        let w = two_people();
        let aid = w.publish(&w.owner, "Owner's plan", "Owner's words");
        let first = owner_agent(&w, &aid);
        let tid = w.thread(&aid, &w.owner, "OWNERWORDS", "Owner's words", 1);
        w.st.send_to(DAEMON, &tid, SendTarget::Agent(&first))
            .unwrap();
        w.st.end_session(DAEMON, &first).unwrap();
        w.approve(&w.owner, Scope::Everywhere);
        // Sam's agent, of another person, may not.
        assert!(matches!(
            w.st.handoff(DAEMON, &tid, &w.sid, Some(&w.sid)),
            Err(CoreError::ThreadTaken { .. })
        ));
        assert_eq!(
            w.st.thread_owner(&tid).unwrap(),
            None,
            "the end released the thread (spec 2026-10-08 §14)"
        );
        // Another of the owner's agents may: the owner sent its comments.
        let second = crate::test_util::session(&w.st, "codex", "owner-h2");
        w.st.handoff(DAEMON, &tid, &second, Some(&w.sid)).unwrap();
        assert_eq!(w.st.thread_owner(&tid).unwrap().unwrap().session_id, w.sid);
    }

    #[test]
    fn a_first_take_withdraws_the_asks_of_the_rows_it_removes() {
        let w = two_people();
        // A comment Sam's page wrote on Mia's page: Sam sent it, and Mia's
        // page is among its authors, withheld from Sam's agents.
        let aid = w.publish(&w.mia, "Mia's plan", "Mia's words");
        let other = sams_second_agent(&w);
        w.st.ensure_watch(DAEMON, &other, &aid).unwrap();
        let t =
            w.st.create_thread(
                DAEMON,
                &aid,
                NewThread {
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Sam".into(),
                    author_public_id: Some(w.sam.clone()),
                    body: "SAMPAGE".into(),
                    clip: None,
                    via_page: true,
                },
            )
            .unwrap();
        w.st.send_to_agent(DAEMON, &t.id).unwrap();
        let asked =
            w.st.ask_approval(
                DAEMON,
                &crate::approvals::AskApproval {
                    session_id: other.clone(),
                    thread_id: Some(t.id.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(asked.len(), 1);
        // Sam's first agent takes it (here through an unfiltered view).
        let r = w.take(Tier::Piggyback, &Visibility::all());
        assert_eq!(r.items.len(), 1);
        assert_eq!(w.st.thread_owner(&t.id).unwrap().unwrap().session_id, w.sid);
        let req = w.st.approval_request(&asked[0].id, Some(&w.sam)).unwrap();
        assert_eq!(req.status.as_str(), "withdrawn");
        let reason: String =
            w.st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT json_extract(body, '$.reason') FROM audit_events
                     WHERE kind = 'approval.withdraw' ORDER BY seq DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(reason, "thread_taken");
    }

    #[test]
    fn a_placeholder_take_makes_no_owner() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 1);
        let vis = w.sam_vis();
        let r = w.take(Tier::Piggyback, &vis);
        assert!(r.items.is_empty());
        assert_eq!(r.placeholders.len(), 1);
        assert_eq!(w.st.thread_owner(&tid).unwrap(), None);
        // Reading the thread (an acknowledgement) delivers nothing held.
        w.st.acknowledge(DAEMON, &w.sid, std::slice::from_ref(&tid), &vis, None)
            .unwrap();
        assert_eq!(w.st.thread_owner(&tid).unwrap(), None);
        // Once Sam approves Mia, the delivery still makes no owner: Mia
        // sent it, and Sam's agent is another person's (O8).
        w.approve(&w.mia, Scope::Everywhere);
        let r = w.take(Tier::Piggyback, &w.sam_vis());
        assert_eq!(r.items.len(), 1);
        assert_eq!(w.st.thread_owner(&tid).unwrap(), None);
    }

    #[test]
    fn withheld_bodies_stay_undelivered_and_get_one_placeholder() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 2);
        let mut vis = w.sam_vis();
        // `queue` hands over visible rows only and tells of none.
        let q = w.take(Tier::Queue, &vis);
        assert!(q.items.is_empty() && q.placeholders.is_empty());
        let r = w.take(Tier::Piggyback, &vis);
        assert!(r.items.is_empty());
        assert_eq!((r.placeholders.len(), r.placeholders[0].count), (1, 2));
        assert_eq!(
            render_delivery(&r.items, &r.placeholders).unwrap(),
            format!(
                "[clax] 2 comments from Mia on \"Sam's plan\" (thread {tid}) are withheld until your person approves them. To ask your person, call ask with {{\"approval\": {{\"thread_id\": \"{tid}\"}}}}."
            )
        );
        assert!(r.touched.threads.iter().any(|(_, t)| t == &tid));
        for tier in [
            Tier::Piggyback,
            Tier::StopHook,
            Tier::PromptHook,
            Tier::Wait,
            Tier::Inject,
        ] {
            let again = w.take(tier, &vis);
            assert!(
                again.items.is_empty() && again.placeholders.is_empty(),
                "once: {tier:?}"
            );
        }
        // Held, not dropped and not delivered; people see it withheld,
        // waiting on Sam.
        assert!(w.rows(&tid).iter().all(|f| f.delivered_at.is_none()));
        let state = w.st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(
            (state.state, state.approver.as_deref(), state.tier),
            (FeedbackPhase::Withheld, Some("Sam"), None)
        );
        // Reading the thread acknowledges nothing it withholds.
        w.st.acknowledge(DAEMON, &w.sid, std::slice::from_ref(&tid), &vis, None)
            .unwrap();
        assert!(w.rows(&tid).iter().all(|f| f.acknowledged_at.is_none()));
        // Approved: delivered at the next take, with the view refreshed.
        w.approve(&w.mia, Scope::Thread(tid.clone()));
        assert_eq!(
            w.st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Sent,
            "approved rows wait for a take"
        );
        assert!(
            w.take(Tier::Piggyback, &vis).items.is_empty(),
            "a stale view"
        );
        vis.refresh_if_stale(&w.st, w.st.visibility_generation())
            .unwrap();
        let r = w.take(Tier::Piggyback, &vis);
        assert_eq!((r.items.len(), r.placeholders.len()), (2, 0));
        assert_eq!(r.items[0].body, "MIACANARY");
        assert!(r.items[0].withheld.is_none(), "Sam's own page");
    }

    #[test]
    fn a_visible_body_is_delivered_with_its_withheld_fields_placeholdered() {
        let w = two_people();
        let aid = w.publish(&w.mia, MIA_TITLE, MIA_QUOTE);
        let tid = w.thread(&aid, &w.owner, "fix the axis", MIA_QUOTE, 1);
        // Sam approves the owner's words, not Mia's.
        w.approve(&w.owner, Scope::Everywhere);
        let vis = w.sam_vis();
        let r = w.take(Tier::Piggyback, &vis);
        assert_eq!((r.items.len(), r.placeholders.len()), (1, 0));
        let text = render_delivery(&r.items, &r.placeholders).unwrap();
        assert!(text.contains("fix the axis"), "{text}");
        assert!(text.contains("on \"an artifact by Mia\""), "{text}");
        assert!(
            text.contains("\nAnchored on: (withheld: page by Mia)  (v1)\n"),
            "{text}"
        );
        assert!(text.contains("\nClip: withheld\n"), "{text}");
        assert!(
            !text.contains(MIA_TITLE) && !text.contains(MIA_QUOTE),
            "{text}"
        );
        let json = serde_json::to_string(&r.items).unwrap();
        assert!(
            !json.contains(MIA_TITLE) && !json.contains(MIA_QUOTE) && !json.contains("clips/"),
            "{json}"
        );
        let i = &r.items[0];
        assert_eq!((i.anchor.is_none(), i.clip_path.is_none()), (true, true));
        assert_eq!(
            i.withheld,
            Some(ItemWithheld {
                title: Some("Mia".into()),
                anchor: Some("Mia".into()),
                clip: true,
                note: false
            })
        );
        assert!(w.rows(&tid)[0].delivered_at.is_some());
    }

    #[test]
    fn revocation_holds_undelivered_rows_again_and_tells_once() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 1);
        // Told, approved, revoked before any take: told once more.
        let r = w.take(Tier::Piggyback, &w.sam_vis());
        assert_eq!(r.placeholders.len(), 1);
        let id = w.approve(&w.mia, Scope::Everywhere);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        let vis = w.sam_vis();
        let r = w.take(Tier::Piggyback, &vis);
        assert_eq!((r.items.len(), r.placeholders.len()), (0, 1));
        assert!(
            w.take(Tier::Piggyback, &vis).placeholders.is_empty(),
            "once"
        );
        // Delivered while approved stays delivered; never resent once
        // withheld again.
        let id = w.approve(&w.mia, Scope::Thread(tid.clone()));
        let vis = w.sam_vis();
        let first = w.take(Tier::StopHook, &vis);
        assert_eq!(first.items.len(), 1);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        w.clock.advance(Store::RESEND_AFTER_SECS + 1);
        let vis = w.sam_vis();
        let again = w.take(Tier::StopHook, &vis);
        assert!(
            again.items.is_empty() && again.placeholders.is_empty(),
            "no resend"
        );
        assert!(w.rows(&tid)[0].delivered_at.is_some(), "still delivered");
    }

    #[test]
    fn a_row_delivered_before_its_body_was_withheld_is_still_acknowledged() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 1);
        let id = w.approve(&w.mia, Scope::Thread(tid.clone()));
        // Delivered, not acknowledged: the prompt hook is not in-band.
        assert_eq!(w.take(Tier::PromptHook, &w.sam_vis()).items.len(), 1);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        let vis = w.sam_vis();
        w.st.acknowledge(DAEMON, &w.sid, std::slice::from_ref(&tid), &vis, None)
            .unwrap();
        assert!(w.rows(&tid)[0].acknowledged_at.is_some());
        assert_eq!(
            w.st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Acknowledged
        );
    }

    #[test]
    fn an_acknowledgement_on_an_artifact_the_session_may_not_read_changes_nothing() {
        let w = two_people();
        let aid = w.publish(&w.sam, "Sam's plan", "Sam's words");
        let tid = w.thread(&aid, &w.sam, "Sam's own note", "Sam's words", 1);
        let vis = w.sam_vis();
        let none = |_: &Artifact| false;
        let touched =
            w.st.acknowledge(
                DAEMON,
                &w.sid,
                std::slice::from_ref(&tid),
                &vis,
                Some(&none),
            )
            .unwrap();
        assert!(touched.threads.is_empty());
        let row = &w.rows(&tid)[0];
        assert!(row.delivered_at.is_none() && row.acknowledged_at.is_none());
        w.st.acknowledge(DAEMON, &w.sid, std::slice::from_ref(&tid), &vis, None)
            .unwrap();
        assert!(w.rows(&tid)[0].acknowledged_at.is_some());
    }

    #[test]
    fn revoking_another_authors_approval_retells_nothing() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 2);
        assert_eq!(w.take(Tier::Piggyback, &w.sam_vis()).placeholders.len(), 1);
        let id = w.approve(&w.owner, Scope::Everywhere);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        let r = w.take(Tier::Piggyback, &w.sam_vis());
        assert!(r.placeholders.is_empty(), "{:?}", r.placeholders);
        assert_eq!(
            w.st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Withheld
        );
    }

    #[test]
    fn a_view_older_than_the_generation_takes_nothing() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 1);
        let old = w.sam_vis();
        // An approval and its revocation after the view was built, with a
        // comment between: the take must not judge it by the old view.
        let id = w.approve(&w.mia, Scope::Everywhere);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        let r = w.take(Tier::Piggyback, &old);
        assert!(r.stale && r.items.is_empty() && r.placeholders.is_empty());
        let told: Option<String> =
            w.st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT placeholder_at FROM feedback WHERE thread_id = ?1",
                    [&tid],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(told.is_none(), "nothing written");
        let n = w.st.take_notices(&w.sid, BASE, &old, None).unwrap();
        assert!(n.stale && n.notices.is_empty());
        let r = w.take(Tier::Piggyback, &w.sam_vis());
        assert!(!r.stale);
        assert_eq!(r.placeholders.len(), 1);
    }

    #[test]
    fn a_resend_carries_only_bodies_still_visible() {
        let w = two_people();
        let aid = w.publish(&w.sam, "Sam's plan", "Sam's words");
        let mine = w.thread(&aid, &w.sam, "Sam's own note", "Sam's words", 1);
        let id = w.approve(&w.mia, Scope::Everywhere);
        let theirs = w.thread(&aid, &w.mia, "MIACANARY", "Sam's words", 1);
        let first = w.take(Tier::StopHook, &w.sam_vis());
        assert_eq!(first.items.len(), 2);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        w.clock.advance(Store::RESEND_AFTER_SECS + 1);
        let r = w.take(Tier::StopHook, &w.sam_vis());
        let bodies: Vec<&str> = r.items.iter().map(|i| i.body.as_str()).collect();
        assert_eq!(bodies, ["Sam's own note"]);
        assert!(r.items[0].resent);
        assert_eq!(w.rows(&theirs)[0].resend_count, 0);
        assert_eq!(w.rows(&mine)[0].resend_count, 1);
    }

    #[test]
    fn notices_announce_only_visible_bodies_under_a_filtered_title() {
        let w = two_people();
        let aid = w.publish(&w.mia, MIA_TITLE, MIA_QUOTE);
        let tid = w.thread(&aid, &w.mia, "MIACANARY", MIA_QUOTE, 1);
        let vis = w.sam_vis();
        assert!(
            w.st.take_notices(&w.sid, BASE, &vis, None)
                .unwrap()
                .notices
                .is_empty()
        );
        w.approve(&w.mia, Scope::Thread(tid.clone()));
        let vis = w.sam_vis();
        let n = w.st.take_notices(&w.sid, BASE, &vis, None).unwrap().notices;
        assert_eq!(n.len(), 1);
        assert_eq!(
            n[0].title, "an artifact by Mia",
            "the thread scope covers no title"
        );
        assert!(!clax_core::feedback::render_notice(&n[0]).contains(MIA_TITLE));
    }

    #[test]
    fn rows_on_an_artifact_the_session_may_not_read_are_held_and_told_once() {
        let w = two_people();
        let aid = w.publish(&w.sam, "Sam's plan", "Sam's words");
        let tid = w.thread(&aid, &w.sam, "Sam's own note", "Sam's words", 2);
        let vis = w.sam_vis();
        let q = |tier| TakeFeedback {
            session_id: w.sid.clone(),
            tier,
            artifact_id: None,
            include_resends: true,
        };
        let none = |_: &Artifact| false;
        // `queue` tells of none.
        let r =
            w.st.take_feedback_readable(DAEMON, &q(Tier::Queue), BASE, &vis, Some(&none))
                .unwrap();
        assert!(r.items.is_empty() && r.placeholders.is_empty());
        let r =
            w.st.take_feedback_readable(DAEMON, &q(Tier::Piggyback), BASE, &vis, Some(&none))
                .unwrap();
        assert!(r.items.is_empty());
        assert_eq!(r.placeholders.len(), 1, "{:?}", r.placeholders);
        let p = &r.placeholders[0];
        assert!(p.no_access);
        assert_eq!(
            p.line(),
            format!(
                "[clax] 2 comments in thread {tid} are held: you no longer have access to this page."
            )
        );
        // Nothing of the page or its comments: IDs and the count only.
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            serde_json::json!({"thread_id": tid, "artifact_id": aid.as_str(), "count": 2,
                               "asked": false, "no_access": true})
        );
        assert!(r.touched.threads.iter().any(|(_, t)| t == &tid));
        // Told once.
        let r =
            w.st.take_feedback_readable(DAEMON, &q(Tier::Wait), BASE, &vis, Some(&none))
                .unwrap();
        assert!(r.items.is_empty() && r.placeholders.is_empty());
        assert!(
            w.st.take_notices(&w.sid, BASE, &vis, Some(&none))
                .unwrap()
                .notices
                .is_empty()
        );
        // Held, not delivered: readable again, the rows are handed over.
        assert!(w.rows(&tid).iter().all(|f| f.delivered_at.is_none()));
        assert_eq!(w.take(Tier::Piggyback, &vis).items.len(), 2);
    }

    /// Spec §7.5, §10: a hold for lost access has its own mark, read as
    /// `no_access` by the owner and the agent's person only; when access
    /// returns, a body still withheld is told as withheld, and delivered
    /// once approved.
    #[test]
    fn a_no_access_hold_has_its_own_state_and_access_back_tells_the_withheld_line() {
        let w = two_people();
        let tid = mia_sends_to_sams_agent(&w, 1);
        let vis = w.sam_vis();
        let q = TakeFeedback {
            session_id: w.sid.clone(),
            tier: Tier::Piggyback,
            artifact_id: None,
            include_resends: true,
        };
        let none = |_: &Artifact| false;
        let r =
            w.st.take_feedback_readable(DAEMON, &q, BASE, &vis, Some(&none))
                .unwrap();
        assert_eq!(r.placeholders.len(), 1);
        assert!(r.placeholders[0].no_access);
        let marks = || -> (Option<String>, Option<String>) {
            w.st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT placeholder_at, no_access_at FROM feedback WHERE thread_id = ?1",
                    params![tid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap()
        };
        let (told, gone) = marks();
        assert!(told.is_none() && gone.is_some(), "{told:?} {gone:?}");
        let d = |v: &DeliveryViewer| {
            w.st.deliveries(std::slice::from_ref(&tid), false)
                .unwrap()
                .remove(&tid)
                .unwrap()
                .view(v)
        };
        let owner = d(&DeliveryViewer::Owner).unwrap();
        assert_eq!(owner.state, FeedbackPhase::NoAccess);
        assert_eq!((owner.approver.clone(), owner.mine), (None, false));
        assert_eq!(
            d(&DeliveryViewer::Person(w.sam.clone())).unwrap().state,
            FeedbackPhase::NoAccess
        );
        // The author and the agent read it as plainly sent; a visitor
        // reads nothing.
        assert_eq!(
            d(&DeliveryViewer::Person(w.mia.clone())).unwrap().state,
            FeedbackPhase::Sent
        );
        let agent = DeliveryViewer::Agent {
            session: Some(w.sid.clone()),
            every: false,
        };
        assert_eq!(d(&agent).unwrap().state, FeedbackPhase::Sent);
        assert!(d(&DeliveryViewer::Visitor).is_none());
        // Readable again: the mark goes, and the withheld line is told.
        let r = w.take(Tier::Piggyback, &vis);
        assert!(r.items.is_empty());
        assert_eq!(r.placeholders.len(), 1, "{:?}", r.placeholders);
        assert!(!r.placeholders[0].no_access);
        assert!(
            r.placeholders[0]
                .line()
                .contains("is withheld until your person approves it"),
            "{}",
            r.placeholders[0].line()
        );
        assert!(marks().1.is_none());
        assert_eq!(
            d(&DeliveryViewer::Owner).unwrap().state,
            FeedbackPhase::Withheld
        );
        // Losing access again tells again.
        let r =
            w.st.take_feedback_readable(DAEMON, &q, BASE, &vis, Some(&none))
                .unwrap();
        assert!(
            r.placeholders.iter().any(|p| p.no_access),
            "{:?}",
            r.placeholders
        );
        // Told as withheld and now held for lost access too: the owner
        // reads both, Mia the wait for Sam's approval alone (9c75976c).
        let owner = d(&DeliveryViewer::Owner).unwrap();
        assert_eq!(
            (owner.state, owner.no_access),
            (FeedbackPhase::Withheld, true)
        );
        let mia = d(&DeliveryViewer::Person(w.mia.clone())).unwrap();
        assert_eq!((mia.state, mia.no_access), (FeedbackPhase::Withheld, false));
        // Approved and readable: delivered.
        w.approve(&w.mia, Scope::Thread(tid.clone()));
        let vis = w.sam_vis();
        let r = w.take(Tier::Piggyback, &vis);
        assert_eq!(r.items.len(), 1, "{:?}", r.placeholders);
        assert_eq!(r.items[0].body, "MIACANARY");
    }

    #[test]
    fn the_owners_agents_are_filtered_once_people_are_connected() {
        let w = two_people();
        let owner_sid = crate::test_util::session(&w.st, "claude", "o1");
        let aid = w.publish(&w.mia, MIA_TITLE, MIA_QUOTE);
        w.st.ensure_watch(DAEMON, &owner_sid, &aid).unwrap();
        let tid = w.thread(&aid, &w.mia, "MIACANARY", MIA_QUOTE, 1);
        let vis = Visibility::for_session(&w.st, &owner_sid).unwrap();
        assert!(!vis.is_all());
        let r =
            w.st.take_feedback(
                DAEMON,
                &TakeFeedback {
                    session_id: owner_sid.clone(),
                    tier: Tier::Wait,
                    artifact_id: None,
                    include_resends: true,
                },
                BASE,
                &vis,
            )
            .unwrap();
        assert!(r.items.is_empty());
        assert_eq!(r.placeholders.len(), 1);
        let line = r.placeholders[0].line();
        assert!(
            line.contains("on \"an artifact by Mia\"") && !line.contains(MIA_TITLE),
            "{line}"
        );
        assert!(line.contains(&tid));
    }
}

#[cfg(test)]
mod ownership {
    use super::*;
    use crate::test_util::{DAEMON, anchor, artifact, session, store};
    use crate::threads::{AUTHOR_AGENT, NewComment, NewThread};

    fn thread(st: &Store, aid: &ArtifactId, body: &str) -> String {
        st.create_thread(
            DAEMON,
            aid,
            NewThread {
                author_public_id: None,
                version_n: 1,
                anchor: anchor(),
                author_name: "Alex".into(),
                body: body.into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn take(st: &Store, sid: &str) -> Taken {
        st.take_feedback(
            DAEMON,
            &TakeFeedback {
                session_id: sid.into(),
                tier: Tier::Piggyback,
                artifact_id: None,
                include_resends: false,
            },
            "http://h",
            &Visibility::all(),
        )
        .unwrap()
    }

    fn reply(sid: &str, body: &str) -> NewComment {
        NewComment {
            author_kind: AUTHOR_AGENT,
            author_name: "claude".into(),
            author_public_id: None,
            via_session_id: Some(sid.to_string()),
            body: body.into(),
            via_page: false,
        }
    }

    /// The bodies of the thread.take and thread.release records, oldest first.
    fn ownership_records(st: &Store) -> Vec<(String, serde_json::Value)> {
        st.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT kind, body FROM audit_events
                 WHERE kind IN ('thread.take', 'thread.release') ORDER BY seq",
            )?;
            Ok(stmt
                .query_map([], |r| {
                    let body: String = r.get(1)?;
                    Ok((r.get(0)?, serde_json::from_str(&body).unwrap()))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap()
    }

    /// Two agents watching one artifact, a thread sent to neither by name.
    fn two_watchers() -> (tempfile::TempDir, Store, String, String, String) {
        let (d, st) = store();
        let a = session(&st, "claude", "a");
        let b = session(&st, "codex", "b");
        let aid = artifact(&st, None);
        for s in [&a, &b] {
            st.ensure_watch(DAEMON, s, &aid).unwrap();
        }
        let tid = thread(&st, &aid, "fix the chart");
        st.send_to(DAEMON, &tid, SendTarget::Everyone).unwrap();
        (d, st, a, b, tid)
    }

    #[test]
    fn first_take_has_one_winner() {
        let (_d, st, a, b, tid) = two_watchers();
        let start = std::sync::Barrier::new(2);
        let (ta, tb) = std::thread::scope(|s| {
            let ha = s.spawn(|| {
                start.wait();
                take(&st, &a)
            });
            let hb = s.spawn(|| {
                start.wait();
                take(&st, &b)
            });
            (ha.join().unwrap(), hb.join().unwrap())
        });
        let got: Vec<&str> = [(&a, &ta), (&b, &tb)]
            .iter()
            .filter(|(_, t)| !t.items.is_empty())
            .map(|(s, _)| s.as_str())
            .collect();
        assert_eq!(got.len(), 1, "exactly one take gets the comment");
        let winner = got[0].to_string();
        let loser = if winner == a { &b } else { &a };
        let owner = st.thread_owner(&tid).unwrap().unwrap();
        assert_eq!(owner.session_id, winner);
        assert!(owner.live);
        assert!(
            st.feedback_rows(&tid)
                .unwrap()
                .iter()
                .all(|f| f.target_session_id.as_deref() == Some(winner.as_str())),
            "the loser's undelivered row is gone"
        );
        assert!(take(&st, loser).items.is_empty());
        let recs = ownership_records(&st);
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].0, "thread.take");
        assert_eq!(recs[0].1["cause"], "first_delivery");
        assert_eq!(recs[0].1["session"], winner.as_str());
        // A later comment goes to the owner alone.
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_kind: crate::threads::AUTHOR_VIEWER,
                author_name: "Alex".into(),
                author_public_id: None,
                via_session_id: None,
                body: "and the axis".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(take(&st, loser).items.is_empty());
        assert_eq!(take(&st, &winner).items.len(), 1);
    }

    #[test]
    fn non_owner_reply_and_resolve_are_thread_taken() {
        let (_d, st, a, b, tid) = two_watchers();
        st.set_session_name(DAEMON, &a, "web-fix", crate::NameSource::Agent)
            .unwrap();
        assert_eq!(take(&st, &a).items.len(), 1);
        let err = st.add_agent_reply(DAEMON, &tid, reply(&b, "mine"), None);
        assert!(
            matches!(&err, Err(CoreError::ThreadTaken { name }) if name == "web-fix"),
            "{err:?}"
        );
        let t = st.get_thread(&tid).unwrap().unwrap();
        assert_eq!(t.comments.len(), 1, "nothing was written");
        let aid = ArtifactId::parse(&t.artifact_id).unwrap();
        let sess_b = st.get_session(&b).unwrap().unwrap();
        assert!(matches!(
            st.resolve_thread_as(DAEMON, &aid, &tid, &sess_b, false),
            Err(CoreError::ThreadTaken { .. })
        ));
        assert_eq!(st.get_thread(&tid).unwrap().unwrap().status, "open");
        // The owner replies and resolves.
        st.add_agent_reply(DAEMON, &tid, reply(&a, "done"), None)
            .unwrap();
        let sess_a = st.get_session(&a).unwrap().unwrap();
        let (t, _) = st
            .resolve_thread_as(DAEMON, &aid, &tid, &sess_a, false)
            .unwrap();
        assert_eq!(t.status, "resolved");
    }

    #[test]
    fn owner_ended_threads_accept_any_agent() {
        let (_d, st, a, b, tid) = two_watchers();
        assert_eq!(take(&st, &a).items.len(), 1);
        st.end_session(DAEMON, &a).unwrap();
        assert_eq!(
            st.thread_owner(&tid).unwrap(),
            None,
            "the end released the thread (spec 2026-10-08 §14)"
        );
        st.check_not_taken(&tid, &b).unwrap();
        st.add_agent_reply(DAEMON, &tid, reply(&b, "picking it up"), None)
            .unwrap();
        let owner = st.thread_owner(&tid).unwrap().unwrap();
        assert_eq!(owner.session_id, b, "the first reply makes it the owner");
        let recs = ownership_records(&st);
        assert_eq!(recs.last().unwrap().1["cause"], "reply");
        // A thread with no owner accepts any agent too.
        let c = session(&st, "pi", "c");
        let aid = artifact(&st, None);
        let t2 = thread(&st, &aid, "plain");
        st.send_to(DAEMON, &t2, SendTarget::Everyone).unwrap();
        st.add_agent_reply(DAEMON, &t2, reply(&c, "on it"), None)
            .unwrap();
        assert_eq!(st.thread_owner(&t2).unwrap().unwrap().session_id, c);
    }

    #[test]
    fn handoff_resends_and_release_untargets() {
        let (_d, st, a, b, tid) = two_watchers();
        assert_eq!(take(&st, &a).items.len(), 1);
        // A non-owner may not hand it off.
        assert!(matches!(
            st.handoff(DAEMON, &tid, &b, Some(&a)),
            Err(CoreError::ThreadTaken { .. })
        ));
        let (t, touched, requests) = st.handoff(DAEMON, &tid, &a, Some(&b)).unwrap();
        assert!(requests.is_empty(), "the owner's own agent asks no one");
        assert!(touched.targets.contains(&b));
        assert!(t.sent_to_agent);
        assert_eq!(st.thread_owner(&tid).unwrap().unwrap().session_id, b);
        let got = take(&st, &b);
        assert_eq!(got.items.len(), 1, "the whole thread is resent");
        assert_eq!(got.items[0].body, "fix the chart");
        st.check_not_taken(&tid, &b).unwrap();
        assert!(st.check_not_taken(&tid, &a).is_err());
        // The new owner releases it: no owner, its new rows untargeted.
        st.add_comment(
            DAEMON,
            &tid,
            NewComment {
                author_kind: crate::threads::AUTHOR_VIEWER,
                author_name: "Alex".into(),
                author_public_id: None,
                via_session_id: None,
                body: "one more".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &tid).unwrap();
        st.handoff(DAEMON, &tid, &b, None).unwrap();
        assert_eq!(st.thread_owner(&tid).unwrap(), None);
        let pending: Vec<_> = st
            .feedback_rows(&tid)
            .unwrap()
            .into_iter()
            .filter(|f| f.delivered_at.is_none())
            .collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].target_session_id, None, "untargeted");
        let kinds: Vec<(String, String)> = ownership_records(&st)
            .into_iter()
            .map(|(k, b)| {
                let why = b
                    .get("cause")
                    .or_else(|| b.get("reason"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                (k, why)
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("thread.take".to_string(), "first_delivery".to_string()),
                ("thread.take".to_string(), "handoff".to_string()),
                ("thread.release".to_string(), "explicit".to_string()),
            ]
        );
        // Released, any agent may take it again.
        st.check_not_taken(&tid, &a).unwrap();
    }

    fn viewer_says(st: &Store, tid: &str, body: &str) {
        st.add_comment(
            DAEMON,
            tid,
            NewComment {
                author_kind: crate::threads::AUTHOR_VIEWER,
                author_name: "Alex".into(),
                author_public_id: None,
                via_session_id: None,
                body: body.into(),
                via_page: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn a_release_leaves_one_untargeted_row_per_comment() {
        let (_d, st, a, b, tid) = two_watchers();
        viewer_says(&st, &tid, "and the axis");
        st.send_to_named(DAEMON, &tid, &[a.clone(), b.clone()])
            .unwrap();
        assert_eq!(st.thread_owner(&tid).unwrap().unwrap().session_id, a);
        assert_eq!(
            st.feedback_rows(&tid).unwrap().len(),
            4,
            "two holders, two comments"
        );
        st.handoff(DAEMON, &tid, &a, None).unwrap();
        let mut pending: Vec<(String, Option<String>)> = st
            .feedback_rows(&tid)
            .unwrap()
            .into_iter()
            .filter(|f| f.delivered_at.is_none())
            .map(|f| (f.comment_id, f.target_session_id))
            .collect();
        pending.sort();
        assert_eq!(pending.len(), 2, "{pending:?}");
        assert_ne!(pending[0].0, pending[1].0, "one per comment");
        assert!(pending.iter().all(|(_, t)| t.is_none()));
        // A second release, with no live owner, changes nothing.
        let before = ownership_records(&st).len();
        st.handoff(DAEMON, &tid, &a, None).unwrap();
        assert_eq!(ownership_records(&st).len(), before);
    }

    #[test]
    fn a_resolved_thread_is_not_handed_off_and_gives_no_watch() {
        let (_d, st, a, _b, tid) = two_watchers();
        assert_eq!(take(&st, &a).items.len(), 1);
        let c = session(&st, "pi", "c");
        let aid = ArtifactId::parse(&st.get_thread(&tid).unwrap().unwrap().artifact_id).unwrap();
        st.resolve_thread(DAEMON, &tid, "viewer:anonymous").unwrap();
        assert!(matches!(
            st.handoff(DAEMON, &tid, &a, Some(&c)),
            Err(CoreError::Invalid {
                code: "thread_resolved",
                ..
            })
        ));
        assert!(
            !st.list_watches(&c)
                .unwrap()
                .iter()
                .any(|w| w.artifact_id == aid.as_str()),
            "a refused handoff gives no watch"
        );
        // A handoff that goes through gives the watch.
        viewer_says(&st, &tid, "reopened");
        st.handoff(DAEMON, &tid, &a, Some(&c)).unwrap();
        assert!(
            st.list_watches(&c)
                .unwrap()
                .iter()
                .any(|w| w.artifact_id == aid.as_str())
        );
    }

    #[test]
    fn a_take_hands_a_comment_over_once_when_its_row_is_also_untargeted() {
        let (_d, st) = store();
        let a = session(&st, "claude", "a");
        let aid = artifact(&st, None);
        let tid = thread(&st, &aid, "nobody watched");
        st.send_to(DAEMON, &tid, SendTarget::Everyone).unwrap();
        assert_eq!(st.feedback_rows(&tid).unwrap()[0].target_session_id, None);
        // `a` holds a row of the same comment too.
        let cid = st.get_thread(&tid).unwrap().unwrap().comments[0].id.clone();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                 VALUES ('f-dup', ?1, ?2, ?3, '2026-01-01T00:00:00.000Z')",
                rusqlite::params![tid, cid, a],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(take(&st, &a).items.len(), 1);
        assert!(take(&st, &a).items.is_empty(), "handed over once");
        assert_eq!(st.feedback_rows(&tid).unwrap().len(), 1);
    }

    #[test]
    fn a_send_naming_an_agent_takes_the_thread() {
        let (_d, st, a, b, tid) = two_watchers();
        st.send_to(DAEMON, &tid, SendTarget::Agent(&b)).unwrap();
        assert_eq!(st.thread_owner(&tid).unwrap().unwrap().session_id, b);
        st.send_to(DAEMON, &tid, SendTarget::Agent(&b)).unwrap();
        let recs = ownership_records(&st);
        assert_eq!(recs.len(), 1, "a send to the owner takes nothing: {recs:?}");
        assert_eq!(recs[0].1["cause"], "send");
        assert!(st.check_not_taken(&tid, &a).is_err());
    }

    /// A thread whose comment `a` already holds a delivered row for, with
    /// an untargeted row of the same comment left by a release.
    fn delivered_and_untargeted() -> (tempfile::TempDir, Store, String, String) {
        let (d, st) = store();
        let a = session(&st, "claude", "a");
        let aid = artifact(&st, None);
        let tid = thread(&st, &aid, "nobody watched");
        st.send_to(DAEMON, &tid, SendTarget::Everyone).unwrap();
        assert_eq!(st.feedback_rows(&tid).unwrap()[0].target_session_id, None);
        let cid = st.get_thread(&tid).unwrap().unwrap().comments[0].id.clone();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at,
                    delivered_at, delivery_tier)
                 VALUES ('f-done', ?1, ?2, ?3, '2026-01-01T00:00:00.000Z',
                    '2026-01-01T00:00:01.000Z', 'piggyback')",
                rusqlite::params![tid, cid, a],
            )?;
            Ok(())
        })
        .unwrap();
        st.ensure_watch(DAEMON, &a, &aid).unwrap();
        (d, st, a, tid)
    }

    /// The untargeted rows whose comment the agent already has a row for
    /// are dropped, not handed to it a second time.
    #[test]
    fn a_send_naming_an_agent_that_already_has_the_comment_drops_its_untargeted_row() {
        let (_d, st, a, tid) = delivered_and_untargeted();
        st.send_to(DAEMON, &tid, SendTarget::Agent(&a)).unwrap();
        let rows = st.feedback_rows(&tid).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].id, "f-done");
        assert!(take(&st, &a).items.is_empty(), "delivered stays delivered");
    }

    #[test]
    fn a_mention_of_an_agent_that_already_has_the_comment_drops_its_untargeted_row() {
        let (_d, st, a, tid) = delivered_and_untargeted();
        st.send_to_named(DAEMON, &tid, std::slice::from_ref(&a))
            .unwrap();
        let rows = st.feedback_rows(&tid).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].id, "f-done");
    }
}
