//! Feedback rows: who a sent thread's viewer comments are addressed to, which
//! tier hands them over, acknowledgement, resends, and retargeting.

use super::Store;
use super::threads::{AUTHOR_VIEWER, thread_in, thread_record};
use crate::anchor::Anchor;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::feedback::{
    FeedbackBatch, FeedbackItem, FeedbackPhase, FeedbackState, Notice, Tier, Touched,
};
use crate::model::{Feedback, Thread};
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

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

fn ts(secs_ago: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(secs_ago))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
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
pub(crate) fn release_session(tx: &Transaction<'_>, sid: &str) -> Result<Touched> {
    let now = Store::now();
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
    let now = Store::now();
    let mut inserted = Vec::new();
    for c in t.comments.iter().filter(|c| c.author_kind == AUTHOR_VIEWER) {
        let has_row: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM feedback WHERE comment_id = ?1)",
            params![c.id],
            |r| r.get(0),
        )?;
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
        feedback_ids: inserted,
        to,
    })
}

/// The `feedback.delivered` record of row `feedback_id` of thread `tid` on
/// artifact `aid`, handed to session `sid` by `tier` for the first time.
fn delivered_record(
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
        self.send_to(ctx, thread_id, SendTarget::Thread)
    }

    /// [`Store::send_to_agent`] with an explicit `target`: one agent (which
    /// becomes the thread's target), everyone (clearing the target), or the
    /// thread's own target.
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
        let mut touched = Touched::default();
        self.with_tx(|tx| {
            let sent = send_in(tx, thread_id, None, target, &mut touched)?;
            if !sent.changed {
                return Ok(());
            }
            let aid: String = tx.query_row(
                "SELECT artifact_id FROM threads WHERE id = ?1",
                params![thread_id],
                |r| r.get(0),
            )?;
            let rec = thread_record(AuditKind::ThreadSend, &Store::now(), &aid, thread_id)
                .with("target", send_target_value(tx, sent.to.as_deref())?)
                .with("feedback_ids", sent.feedback_ids)
                .with("thread_ids", vec![thread_id.to_string()]);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })?;
        let thread = self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?;
        Ok((thread, touched))
    }

    /// Hands over rows for `q` and records the hand-over in the same
    /// transaction, so two concurrent callers never receive the same row.
    /// Undelivered rows are marked delivered by `q.tier`; resend-eligible rows
    /// (see [`Tier::resends`]) get `resend_count + 1` and `resent: true`;
    /// in-band tiers also acknowledge. Rows of deleted artifacts and resolved
    /// threads are never handed over. Items are oldest first.
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
    ) -> Result<(Vec<FeedbackItem>, Touched)> {
        let cutoff = ts(Self::RESEND_AFTER_SECS);
        let resends = q.include_resends && q.tier.resends();
        let now = Store::now();
        let base = browser_base.trim_end_matches('/').to_string();
        let pending = self.with_tx(|tx| {
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
                                }),
                                None => None,
                            },
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            // A row whose anchor or artifact ID no longer parses is left
            // untouched (and skipped) rather than marked delivered without
            // being handed over.
            let mut good = Vec::with_capacity(rows.len());
            for p in rows {
                let parsed = serde_json::from_str::<Anchor>(&p.anchor_json)
                    .ok()
                    .zip(ArtifactId::parse(&p.artifact_id).ok());
                match parsed {
                    Some((anchor, aid)) => {
                        let clip_path = p.has_clip.then(|| {
                            self.home.clip_path(&aid, &p.thread_id).to_string_lossy().into_owned()
                        });
                        let page = super::live::live_page_of_conn(tx, &p.artifact_id)?;
                        let live_path: Option<String> = match page {
                            Some(_) => tx.query_row(
                                "SELECT live_path FROM threads WHERE id = ?1",
                                params![p.thread_id],
                                |r| r.get(0),
                            )?,
                            None => None,
                        };
                        let live = page.map(|lp| {
                            crate::feedback::LiveRef {
                                page_url: format!(
                                    "{}{}{}",
                                    lp.origin,
                                    live_path.as_deref().unwrap_or(&lp.path),
                                    anchor.route.as_deref().unwrap_or("")
                                ),
                                snapshot_path: self
                                    .home
                                    .version_dir(&aid, p.version_n)
                                    .join(crate::publish::INDEX)
                                    .to_string_lossy()
                                    .into_owned(),
                            }
                        });
                        good.push((p, anchor, clip_path, live));
                    }
                    None => tracing::warn!(
                        feedback_id = p.id.as_str(),
                        thread_id = p.thread_id.as_str(),
                        artifact_id = p.artifact_id.as_str(),
                        "skipping feedback on a corrupt thread row"
                    ),
                }
            }
            for (p, _, _, _) in &good {
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
            Ok(good)
        })?;
        let mut touched = Touched::default();
        let mut items = Vec::with_capacity(pending.len());
        for (p, anchor, clip_path, live) in pending {
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
                author: p.author,
                via_page: p.via_page,
                body: p.body,
                resent: p.delivered,
                created_at: p.created_at,
                batch: p.batch,
                live,
            });
        }
        Ok((items, touched))
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
        let now = Store::now();
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
    /// `feedback.delivered` under `ctx`.
    pub fn acknowledge(
        &self,
        ctx: &AuditCtx,
        session_id: &str,
        thread_ids: &[String],
    ) -> Result<Touched> {
        self.acknowledge_where(ctx, "thread_id", session_id, thread_ids)
    }

    /// Like [`Store::acknowledge`], for `session_id`'s rows on exactly the
    /// comments `comment_ids`: a row for any other comment on the same thread
    /// (one the session has not seen) stays pending.
    pub fn acknowledge_comments(
        &self,
        ctx: &AuditCtx,
        session_id: &str,
        comment_ids: &[String],
    ) -> Result<Touched> {
        self.acknowledge_where(ctx, "comment_id", session_id, comment_ids)
    }

    /// Acknowledges `session_id`'s unacknowledged rows whose `column`
    /// (`thread_id` or `comment_id`) is one of `ids`, recording the first
    /// delivery of each that was undelivered.
    fn acknowledge_where(
        &self,
        ctx: &AuditCtx,
        column: &str,
        session_id: &str,
        ids: &[String],
    ) -> Result<Touched> {
        debug_assert!(matches!(column, "thread_id" | "comment_id"));
        let now = Store::now();
        let mut touched = Touched::default();
        let undelivered = format!(
            "SELECT f.id, f.thread_id, t.artifact_id FROM feedback f JOIN threads t ON t.id = f.thread_id
             WHERE f.{column} = ?1 AND f.target_session_id = ?2 AND f.acknowledged_at IS NULL
                AND f.delivered_at IS NULL
             ORDER BY f.created_at, f.id"
        );
        let update = format!(
            "UPDATE feedback SET acknowledged_at = ?3, delivered_at = COALESCE(delivered_at, ?3),
                delivery_tier = COALESCE(delivery_tier, 'piggyback'), last_sent_at = COALESCE(last_sent_at, ?3)
             WHERE {column} = ?1 AND target_session_id = ?2 AND acknowledged_at IS NULL
             RETURNING thread_id"
        );
        self.with_tx(|tx| {
            for id in ids {
                let first: Vec<(String, String, String)> = {
                    let mut stmt = tx.prepare_cached(&undelivered)?;
                    stmt.query_map(params![id, session_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for (fid, tid, aid) in &first {
                    let rec =
                        delivered_record(&now, aid, tid, session_id, fid, Tier::Piggyback.as_str());
                    self.record_audit(tx, ctx, rec)?;
                }
                let tids: Vec<String> = {
                    let mut stmt = tx.prepare(&update)?;
                    stmt.query_map(params![id, session_id, now], |r| r.get(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for tid in tids {
                    let aid: String = tx.query_row(
                        "SELECT artifact_id FROM threads WHERE id = ?1",
                        params![tid],
                        |r| r.get(0),
                    )?;
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
    pub fn take_notices(&self, session_id: &str, browser_base: &str) -> Result<Vec<Notice>> {
        let base = browser_base.trim_end_matches('/').to_string();
        let now = Store::now();
        self.with_tx(|tx| {
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
            let mut out = Vec::new();
            for (feedback_id, comment_id, thread_id, artifact_id, title) in rows {
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
            Ok(out)
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

    /// The state of the thread's latest forwarded comment, or `None` when
    /// nothing was forwarded. Any acknowledged row: `acknowledged`; else any
    /// delivered row: `delivered`; else any row targeting a live session:
    /// `sent`, with the tier that session waits on (`queue` for an armed Codex
    /// session with a known session ID when `codex_push` and `codex queue` has
    /// not failed to take the row, `inject` for an
    /// armed Pi session, `stop_hook` for other armed sessions, else
    /// `piggyback`); else `agent_ended`.
    pub fn feedback_state(
        &self,
        thread_id: &str,
        codex_push: bool,
    ) -> Result<Option<FeedbackState>> {
        Ok(self
            .feedback_states(&[thread_id.to_string()], codex_push)?
            .remove(thread_id))
    }

    /// [`Store::feedback_state`] of each of `thread_ids`, by thread ID; a
    /// thread with nothing forwarded is missing.
    pub fn feedback_states(
        &self,
        thread_ids: &[String],
        codex_push: bool,
    ) -> Result<BTreeMap<String, FeedbackState>> {
        if thread_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.with_read(|c| feedback_states_in(c, thread_ids, codex_push))
    }
}

/// The rows [`Store::take_feedback`] hands over: targeted to session `?1`,
/// of artifact `?2` (`NULL`: any), armed watches only when `?3`, resends
/// when `?4` (under `?5` resends, last sent at or before `?6`), and none
/// that `codex queue` failed to take when `?7`.
pub(crate) const TAKE_FEEDBACK: &str = "SELECT f.id, f.thread_id, f.comment_id, f.delivered_at IS NOT NULL AS delivered,
                        t.artifact_id, a.title, t.version_n, t.anchor_json, t.has_clip,
                        c.author_name, c.body, c.created_at, c.via_page,
                        f.batch_id, b.size, b.note, b.sent_by
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

/// The feedback rows of each thread's latest forwarded comment.
pub(crate) const FEEDBACK_STATES: &str =
    "SELECT f.thread_id, f.target_session_id, f.created_at, f.delivered_at, f.delivery_tier, f.acknowledged_at,
            f.resend_count, f.untargeted_at, s.ended_at, s.harness, s.harness_session_id IS NOT NULL,
            COALESCE(w.replies_armed, 0), f.push_failed_at IS NOT NULL
       FROM json_each(?1) j
      CROSS JOIN feedback f ON f.thread_id = j.value
       JOIN threads t ON t.id = f.thread_id
       LEFT JOIN sessions s ON s.id = f.target_session_id
       LEFT JOIN watches w ON w.session_id = f.target_session_id AND w.artifact_id = t.artifact_id
      WHERE f.comment_id = (SELECT l.comment_id FROM feedback l WHERE l.thread_id = j.value
                             ORDER BY l.created_at DESC, l.id DESC LIMIT 1)
      ORDER BY f.thread_id, f.created_at, f.id";

struct StateRow {
    target: Option<String>,
    created_at: String,
    delivered_at: Option<String>,
    tier: Option<String>,
    acknowledged_at: Option<String>,
    resends: u32,
    untargeted_at: Option<String>,
    ended_at: Option<String>,
    harness: Option<String>,
    has_hsid: bool,
    armed: bool,
    push_failed: bool,
}

pub(crate) fn feedback_states_in(
    c: &Connection,
    thread_ids: &[String],
    codex_push: bool,
) -> Result<BTreeMap<String, FeedbackState>> {
    let mut by_thread: BTreeMap<String, Vec<StateRow>> = BTreeMap::new();
    let mut stmt = c.prepare_cached(FEEDBACK_STATES)?;
    let mut rows = stmt.query(params![id_array(thread_ids)])?;
    while let Some(r) = rows.next()? {
        by_thread.entry(r.get(0)?).or_default().push(StateRow {
            target: r.get(1)?,
            created_at: r.get(2)?,
            delivered_at: r.get(3)?,
            tier: r.get(4)?,
            acknowledged_at: r.get(5)?,
            resends: r.get(6)?,
            untargeted_at: r.get(7)?,
            ended_at: r.get(8)?,
            harness: r.get(9)?,
            has_hsid: r.get::<_, Option<bool>>(10)?.unwrap_or(false),
            armed: r.get::<_, i64>(11)? != 0,
            push_failed: r.get(12)?,
        });
    }
    Ok(by_thread
        .into_iter()
        .map(|(tid, rows)| {
            let s = state_of(&tid, &rows, codex_push);
            (tid, s)
        })
        .collect())
}

/// The state of one thread from its latest comment's rows (not empty).
fn state_of(thread_id: &str, rows: &[StateRow], codex_push: bool) -> FeedbackState {
    let tier_of = |r: &StateRow| r.tier.as_deref().and_then(Tier::parse);
    let state = |state, tier, since: &str, resends, exhausted| FeedbackState {
        thread_id: thread_id.to_string(),
        state,
        tier,
        since: since.to_string(),
        resends,
        exhausted,
    };
    if let Some((r, at)) = rows
        .iter()
        .filter_map(|r| r.acknowledged_at.as_deref().map(|at| (r, at)))
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
        .filter_map(|r| r.delivered_at.as_deref().map(|at| (r, at)))
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
    use crate::feedback::{FeedbackPhase, Tier};
    use crate::store::test_util::DAEMON;
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::store::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread};
    use crate::{ArtifactId, Store};

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
        st.take_feedback(DAEMON, &q, BASE).unwrap().0
    }

    #[test]
    fn notices_announce_each_armed_undelivered_row_once() {
        let (_d, st) = store();
        let grok = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&grok));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert!(
            st.take_notices(&grok, "http://h:1").unwrap().is_empty(),
            "unarmed: no notice"
        );
        st.ensure_watch(DAEMON, &grok, &aid).unwrap();
        let n = st.take_notices(&grok, "http://h:1").unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].thread_id, tid);
        assert_eq!(n[0].url, format!("http://h:1/a/{}", aid.as_str()));
        assert!(
            st.take_notices(&grok, "http://h:1").unwrap().is_empty(),
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
        assert!(st.take_notices(&grok, "http://h:1").unwrap().is_empty());
    }

    #[test]
    fn a_retargeted_row_is_announced_to_its_new_session() {
        let (_d, st) = store();
        let first = session(&st, "grok", "g1");
        let aid = artifact(&st, Some(&first));
        st.ensure_watch(DAEMON, &first, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(st.take_notices(&first, "http://h:1").unwrap().len(), 1);
        st.end_session(DAEMON, &first).unwrap();
        let next = session(&st, "grok", "g2");
        st.watch(DAEMON, &next, &aid, true).unwrap();
        st.retarget_untargeted(&aid, &next).unwrap();
        assert_eq!(st.take_notices(&next, "http://h:1").unwrap().len(), 1);
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
        st.send_to(DAEMON, &tid, SendTarget::Everyone).unwrap();
        viewer_says(&st, &tid, "fourth");
        st.send_to_agent(DAEMON, &tid).unwrap();
        assert_eq!(bodies(&owner), ["fourth"], "Everyone cleared the target");
        assert_eq!(bodies(&second), ["fourth"]);
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
            st.take_feedback(DAEMON, &q, BASE).unwrap().0.is_empty(),
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
            .acknowledge(DAEMON, &owner, std::slice::from_ref(&tid))
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
            st.acknowledge_comments(DAEMON, &other, std::slice::from_ref(&first))
                .unwrap()
                .is_empty(),
            "another session's acknowledgement touches nothing"
        );
        let touched = st
            .acknowledge_comments(DAEMON, &owner, std::slice::from_ref(&first))
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
        st.acknowledge(DAEMON, &claude, std::slice::from_ref(&tid))
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
            Err(crate::CoreError::NotFound)
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
        let (items, _) = st.take_feedback(DAEMON, &q, BASE).unwrap();
        assert_eq!(items.len(), 1);
        assert!(crate::feedback::render_item(&items[0]).contains("\nViewer: \"one\"\n"));
        let clip = items[0].clip_path.clone().unwrap();
        assert!(
            std::path::Path::new(&clip).is_absolute() && std::path::Path::new(&clip).exists(),
            "{clip}"
        );
        assert_eq!(take(&st, &owner, Tier::Wait)[0].body, "two");
    }
}
