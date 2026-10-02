//! Feedback rows: who a sent thread's viewer comments are addressed to, which
//! tier hands them over, acknowledgement, resends, and retargeting.

use super::Store;
use super::threads::{AUTHOR_VIEWER, thread_in};
use crate::anchor::Anchor;
use crate::feedback::{FeedbackBatch, FeedbackItem, FeedbackPhase, FeedbackState, Tier, Touched};
use crate::model::{Feedback, Thread};
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, Transaction, params};

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

/// Releases session `sid` after it ended: drops its watches; each of its
/// undelivered rows is deleted when another live session is a target of the
/// same comment, else untargeted for the next session that publishes or watches.
pub(crate) fn release_session(tx: &Transaction<'_>, sid: &str) -> Result<Touched> {
    let now = Store::now();
    let mut touched = Touched::default();
    tx.execute("DELETE FROM watches WHERE session_id = ?1", params![sid])?;
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
                "UPDATE feedback SET target_session_id = NULL, untargeted_at = ?2 WHERE id = ?1",
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
/// target when the send sets or clears it.
fn targets_for(
    tx: &Transaction<'_>,
    thread_id: &str,
    aid: &str,
    owner: Option<&str>,
    target: SendTarget<'_>,
) -> Result<Vec<String>> {
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
            Ok(vec![sid.to_string()])
        }
        SendTarget::Everyone => {
            tx.execute(
                "UPDATE threads SET target_session_id = NULL WHERE id = ?1",
                params![thread_id],
            )?;
            Ok(live)
        }
        SendTarget::Thread => {
            let stored: Option<String> = tx.query_row(
                "SELECT target_session_id FROM threads WHERE id = ?1",
                params![thread_id],
                |r| r.get(0),
            )?;
            Ok(match stored {
                Some(sid) if live.contains(&sid) => vec![sid],
                _ => live,
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

/// Marks the thread sent to the agent and creates one row per (viewer
/// comment without a row, target of `target`), each carrying `batch_id`.
/// With no target, one untargeted row per comment. Returns how many rows it
/// inserted. See [`Store::send_to_agent`] for the contract.
pub(crate) fn send_in(
    tx: &Transaction<'_>,
    thread_id: &str,
    batch_id: Option<&str>,
    target: SendTarget<'_>,
    touched: &mut Touched,
) -> Result<usize> {
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
    let targets = targets_for(tx, thread_id, &t.artifact_id, owner.as_deref(), target)?;
    let now = Store::now();
    let mut inserted = 0;
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
            tx.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, untargeted_at, batch_id)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?4, ?5)",
                params![new_ulid(), thread_id, c.id, now, batch_id],
            )?;
            inserted += 1;
        } else {
            for sid in &targets {
                tx.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, batch_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![new_ulid(), thread_id, c.id, sid, now, batch_id],
                )?;
                inserted += 1;
            }
            touched.targets.extend(targets.iter().cloned());
        }
    }
    touched
        .threads
        .insert((t.artifact_id.clone(), thread_id.to_string()));
    Ok(inserted)
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
        ("claude" | "codex", true) => Tier::StopHook,
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
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone; `thread_resolved`
    /// for a resolved thread.
    pub fn send_to_agent(&self, thread_id: &str) -> Result<(Thread, Touched)> {
        self.send_to(thread_id, SendTarget::Thread)
    }

    /// [`Store::send_to_agent`] with an explicit `target`: one agent (which
    /// becomes the thread's target), everyone (clearing the target), or the
    /// thread's own target.
    ///
    /// # Errors
    /// As [`Store::send_to_agent`], plus `unknown_agent` when `target` names a
    /// session that is not a live owner or watcher of the artifact.
    pub fn send_to(&self, thread_id: &str, target: SendTarget<'_>) -> Result<(Thread, Touched)> {
        let mut touched = Touched::default();
        self.with_tx(|tx| send_in(tx, thread_id, None, target, &mut touched).map(|_| ()))?;
        let thread = self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?;
        Ok((thread, touched))
    }

    /// Hands over rows for `q` and records the hand-over in the same
    /// transaction, so two concurrent callers never receive the same row.
    /// Undelivered rows are marked delivered by `q.tier`; resend-eligible rows
    /// (see [`Tier::resends`]) get `resend_count + 1` and `resent: true`;
    /// in-band tiers also acknowledge. Rows of deleted artifacts and resolved
    /// threads are never handed over. Items are oldest first.
    pub fn take_feedback(
        &self,
        q: &TakeFeedback,
        browser_base: &str,
    ) -> Result<(Vec<FeedbackItem>, Touched)> {
        let cutoff = ts(Self::RESEND_AFTER_SECS);
        let resends = q.include_resends && q.tier.resends();
        let now = Store::now();
        let base = browser_base.trim_end_matches('/').to_string();
        let pending = self.with_tx(|tx| {
            let mut stmt = tx.prepare(
                "SELECT f.id, f.thread_id, f.comment_id, f.delivered_at IS NOT NULL AS delivered,
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
                 ORDER BY f.created_at, f.id",
            )?;
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
                        good.push((p, anchor, clip_path));
                    }
                    None => tracing::warn!(
                        feedback_id = p.id.as_str(),
                        thread_id = p.thread_id.as_str(),
                        artifact_id = p.artifact_id.as_str(),
                        "skipping feedback on a corrupt thread row"
                    ),
                }
            }
            for (p, _, _) in &good {
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
                }
            }
            Ok(good)
        })?;
        let mut touched = Touched::default();
        let mut items = Vec::with_capacity(pending.len());
        for (p, anchor, clip_path) in pending {
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
            });
        }
        Ok((items, touched))
    }

    /// Returns `queue` hand-overs that were not confirmed to undelivered, so
    /// another tier can deliver them, and marks them push-failed: `queue` does
    /// not take them again and [`Store::feedback_state`] reports them waiting
    /// on the in-band tiers, until the row is retargeted to another session.
    pub fn release_feedback(&self, ids: &[String]) -> Result<Touched> {
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
    /// `piggyback`, the in-band tool path.
    pub fn acknowledge(&self, session_id: &str, thread_ids: &[String]) -> Result<Touched> {
        self.acknowledge_where("thread_id", session_id, thread_ids)
    }

    /// Like [`Store::acknowledge`], for `session_id`'s rows on exactly the
    /// comments `comment_ids`: a row for any other comment on the same thread
    /// (one the session has not seen) stays pending.
    pub fn acknowledge_comments(
        &self,
        session_id: &str,
        comment_ids: &[String],
    ) -> Result<Touched> {
        self.acknowledge_where("comment_id", session_id, comment_ids)
    }

    /// Acknowledges `session_id`'s unacknowledged rows whose `column`
    /// (`thread_id` or `comment_id`) is one of `ids`.
    fn acknowledge_where(&self, column: &str, session_id: &str, ids: &[String]) -> Result<Touched> {
        debug_assert!(matches!(column, "thread_id" | "comment_id"));
        let now = Store::now();
        let mut touched = Touched::default();
        let update = format!(
            "UPDATE feedback SET acknowledged_at = ?3, delivered_at = COALESCE(delivered_at, ?3),
                delivery_tier = COALESCE(delivery_tier, 'piggyback'), last_sent_at = COALESCE(last_sent_at, ?3)
             WHERE {column} = ?1 AND target_session_id = ?2 AND acknowledged_at IS NULL
             RETURNING thread_id"
        );
        self.with_tx(|tx| {
            for id in ids {
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
                        "UPDATE feedback SET target_session_id = ?2, untargeted_at = NULL, push_failed_at = NULL WHERE id = ?1",
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

    /// Every feedback row of the thread, oldest first.
    pub fn feedback_rows(&self, thread_id: &str) -> Result<Vec<Feedback>> {
        self.with_conn(|c| {
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
        struct Row {
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
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT f.target_session_id, f.created_at, f.delivered_at, f.delivery_tier, f.acknowledged_at,
                        f.resend_count, f.untargeted_at, s.ended_at, s.harness, s.harness_session_id IS NOT NULL,
                        COALESCE(w.replies_armed, 0), f.push_failed_at IS NOT NULL
                 FROM feedback f
                 JOIN threads t ON t.id = f.thread_id
                 LEFT JOIN sessions s ON s.id = f.target_session_id
                 LEFT JOIN watches w ON w.session_id = f.target_session_id AND w.artifact_id = t.artifact_id
                 WHERE f.thread_id = ?1 AND f.comment_id =
                    (SELECT comment_id FROM feedback WHERE thread_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1)
                 ORDER BY f.created_at, f.id",
            )?;
            let rows = stmt
                .query_map(params![thread_id], |r| {
                    Ok(Row {
                        target: r.get(0)?,
                        created_at: r.get(1)?,
                        delivered_at: r.get(2)?,
                        tier: r.get(3)?,
                        acknowledged_at: r.get(4)?,
                        resends: r.get(5)?,
                        untargeted_at: r.get(6)?,
                        ended_at: r.get(7)?,
                        harness: r.get(8)?,
                        has_hsid: r.get::<_, Option<bool>>(9)?.unwrap_or(false),
                        armed: r.get::<_, i64>(10)? != 0,
                        push_failed: r.get(11)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Ok(None);
            }
            let tier_of = |r: &Row| r.tier.as_deref().and_then(Tier::parse);
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
                return Ok(Some(state(FeedbackPhase::Acknowledged, tier_of(r), at, r.resends, false)));
            }
            let delivered: Vec<(&Row, &str)> = rows
                .iter()
                .filter_map(|r| r.delivered_at.as_deref().map(|at| (r, at)))
                .collect();
            if let Some((first, at)) = delivered.iter().min_by(|a, b| a.1.cmp(b.1)) {
                let resends = delivered.iter().map(|(r, _)| r.resends).max().unwrap_or(0);
                let exhausted = delivered.iter().all(|(r, _)| r.resends >= Self::MAX_RESENDS);
                return Ok(Some(state(FeedbackPhase::Delivered, tier_of(first), at, resends, exhausted)));
            }
            if let Some(r) = rows.iter().find(|r| r.target.is_some() && r.ended_at.is_none()) {
                let push = codex_push && !r.push_failed;
                let tier = waiting_on(r.harness.as_deref().unwrap_or(""), r.has_hsid, r.armed, push);
                return Ok(Some(state(FeedbackPhase::Sent, Some(tier), &r.created_at, 0, false)));
            }
            let since = rows
                .iter()
                .filter_map(|r| r.untargeted_at.clone().or_else(|| r.ended_at.clone()))
                .max()
                .unwrap_or_else(|| rows[0].created_at.clone());
            Ok(Some(state(FeedbackPhase::AgentEnded, None, &since, 0, false)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::{FeedbackPhase, Tier};
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::store::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread};
    use crate::{ArtifactId, Store};

    const BASE: &str = "http://localhost:7480";

    fn thread(st: &Store, aid: &ArtifactId, body: &str) -> String {
        st.create_thread(
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
        st.take_feedback(&q, BASE).unwrap().0
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
        st.with_conn(|c| {
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
        let (t, touched) = st.send_to_agent(&tid).unwrap();
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
        st.watch(&w1, &aid, true).unwrap();
        st.watch(&w2, &aid, false).unwrap();
        st.end_session(&w2).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
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
        st.end_session(&owner).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        assert_eq!(targets(&st, &tid), vec![None]);
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::AgentEnded
        );
        let late = session(&st, "claude", "late");
        st.watch(&late, &aid, true).unwrap();
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
        st.send_to_agent(&tid).unwrap();
        let delivered = take(&st, &owner, Tier::Piggyback);
        assert_eq!(delivered.len(), 1);
        st.add_comment(
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
        st.send_to_agent(&tid).unwrap();
        st.resolve_thread(&tid, "viewer:anonymous").unwrap();
        st.add_comment(
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
        st.send_to_agent(&tid).unwrap();
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
        st.send_to_agent(&tid).unwrap();
        st.send_to_agent(&tid).unwrap();
        assert_eq!(st.feedback_rows(&tid).unwrap().len(), 1);
        st.add_comment(
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
        st.send_to_agent(&tid).unwrap();
        assert_eq!(
            st.feedback_rows(&tid).unwrap().len(),
            1,
            "agent comments are never forwarded"
        );
        st.add_comment(
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
        st.send_to_agent(&tid).unwrap();
        let items = take(&st, &owner, Tier::Piggyback);
        assert_eq!(
            items.iter().map(|i| i.body.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    fn viewer_says(st: &Store, tid: &str, body: &str) {
        st.add_comment(
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
        st.ensure_watch(&watcher, &aid).unwrap();
        let tid = thread(&st, &aid, "first");
        let stranger = session(&st, "codex", "s");
        let err = st.send_to(&tid, SendTarget::Agent(&stranger)).unwrap_err();
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
        st.send_to(&tid, SendTarget::Agent(&watcher)).unwrap();
        viewer_says(&st, &tid, "second");
        st.send_to_agent(&tid).unwrap();
        let bodies = |sid: &str| {
            take(&st, sid, Tier::Piggyback)
                .into_iter()
                .map(|i| i.body)
                .collect::<Vec<_>>()
        };
        assert_eq!(bodies(&watcher), ["first", "second"]);
        assert!(bodies(&owner).is_empty());
        st.end_session(&watcher).unwrap();
        viewer_says(&st, &tid, "third");
        st.send_to_agent(&tid).unwrap();
        assert_eq!(
            bodies(&owner),
            ["third"],
            "the target ended: the comment fans out"
        );
        let second = session(&st, "codex", "w2");
        st.ensure_watch(&second, &aid).unwrap();
        st.send_to(&tid, SendTarget::Agent(&second)).unwrap();
        st.send_to(&tid, SendTarget::Everyone).unwrap();
        viewer_says(&st, &tid, "fourth");
        st.send_to_agent(&tid).unwrap();
        assert_eq!(bodies(&owner), ["fourth"], "Everyone cleared the target");
        assert_eq!(bodies(&second), ["fourth"]);
    }

    #[test]
    fn piggyback_delivers_once_and_acknowledges() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
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
        st.watch(&owner, &aid, false).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
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
        st.ensure_watch(&owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
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
        st.ensure_watch(&owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        take(&st, &owner, Tier::StopHook);
        age(&st);
        let q = TakeFeedback {
            session_id: owner.clone(),
            tier: Tier::StopHook,
            artifact_id: None,
            include_resends: false,
        };
        assert!(
            st.take_feedback(&q, BASE).unwrap().0.is_empty(),
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
        st.send_to_agent(&tid).unwrap();
        let touched = st.acknowledge(&owner, std::slice::from_ref(&tid)).unwrap();
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
        st.send_to_agent(&tid).unwrap();
        let first = st.get_thread(&tid).unwrap().unwrap().comments[0].id.clone();
        let later = st
            .add_comment(
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
        st.send_to_agent(&tid).unwrap();
        assert!(
            st.acknowledge_comments(&other, std::slice::from_ref(&first))
                .unwrap()
                .is_empty(),
            "another session's acknowledgement touches nothing"
        );
        let touched = st
            .acknowledge_comments(&owner, std::slice::from_ref(&first))
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
        st.ensure_watch(&owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        let claimed = take(&st, &owner, Tier::Queue);
        assert_eq!(claimed.len(), 1);
        st.release_feedback(&[claimed[0].feedback_id.clone()])
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
        st.watch(&owner, &aid, true).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        assert_eq!(
            st.feedback_state(&tid, true).unwrap().unwrap().tier,
            Some(Tier::Queue)
        );
        let claimed = take(&st, &owner, Tier::Queue);
        st.release_feedback(&[claimed[0].feedback_id.clone()])
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
        st.end_session(&owner).unwrap();
        let next = session(&st, "codex", "cx2");
        st.watch(&next, &aid, true).unwrap();
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
        st.watch(&other, &aid, true).unwrap();
        st.ensure_watch(&owner, &aid).unwrap();
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        let (_, touched) = st.end_session_touched(&owner).unwrap();
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
        st.end_session(&other).unwrap();
        assert_eq!(targets(&st, &tid), vec![None]);
        let next = session(&st, "claude", "n");
        st.retarget_untargeted(&aid, &next).unwrap();
        st.retarget_untargeted(&aid, &next).unwrap();
        assert_eq!(targets(&st, &tid), vec![Some(next)]);
    }

    #[test]
    fn feedback_state_follows_the_row_through_its_life() {
        let (_d, st) = store();
        let claude = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&claude));
        let tid = thread(&st, &aid, "hi");
        assert_eq!(st.feedback_state(&tid, false).unwrap(), None);
        st.send_to_agent(&tid).unwrap();
        let s = st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(
            (s.state, s.tier),
            (FeedbackPhase::Sent, Some(Tier::Piggyback)),
            "unarmed owner waits on its next tool call"
        );
        st.ensure_watch(&claude, &aid).unwrap();
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
        st.acknowledge(&claude, std::slice::from_ref(&tid)).unwrap();
        assert_eq!(
            st.feedback_state(&tid, false).unwrap().unwrap().state,
            FeedbackPhase::Acknowledged
        );

        let codex = session(&st, "codex", "cx");
        let a2 = artifact(&st, Some(&codex));
        st.ensure_watch(&codex, &a2).unwrap();
        let t2 = thread(&st, &a2, "hi");
        st.send_to_agent(&t2).unwrap();
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
        st.ensure_watch(&pi, &a3).unwrap();
        let t3 = thread(&st, &a3, "hi");
        st.send_to_agent(&t3).unwrap();
        assert_eq!(
            st.feedback_state(&t3, false).unwrap().unwrap().tier,
            Some(Tier::Inject)
        );
        st.end_session(&pi).unwrap();
        let s = st.feedback_state(&t3, false).unwrap().unwrap();
        assert_eq!((s.state, s.tier), (FeedbackPhase::AgentEnded, None));
    }

    #[test]
    fn deleted_artifacts_feedback_is_never_taken() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        st.delete_artifact(&aid).unwrap();
        for tier in [Tier::Piggyback, Tier::Wait, Tier::PromptHook] {
            assert!(take(&st, &owner, tier).is_empty(), "{tier:?}");
        }
        assert!(matches!(
            st.send_to_agent(&tid),
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
        st.send_to_agent(&bad).unwrap();
        st.send_to_agent(&good).unwrap();
        st.with_conn(|c| {
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
        st.send_to_agent(&bad).unwrap();
        st.send_to_agent(&good).unwrap();
        st.with_conn(|c| {
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
        st.send_to_agent(&tid).unwrap();
        st.resolve_thread(&tid, "viewer:x").unwrap();
        assert!(take(&st, &owner, Tier::Piggyback).is_empty());
    }

    #[test]
    fn resolving_deletes_undelivered_rows_and_touches_the_thread() {
        let (_d, st) = store();
        let owner = session(&st, "claude", "o");
        let aid = artifact(&st, Some(&owner));
        let tid = thread(&st, &aid, "hi");
        st.send_to_agent(&tid).unwrap();
        let (t, touched) = st.resolve_thread_touched(&tid, "viewer:x").unwrap();
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
        st.send_to_agent(&tid).unwrap();
        assert_eq!(take(&st, &owner, Tier::PromptHook).len(), 1);
        st.resolve_thread(&tid, "viewer:x").unwrap();
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
        st.send_to_agent(&t1.id).unwrap();
        st.send_to_agent(&t2).unwrap();
        let q = TakeFeedback {
            session_id: owner.clone(),
            tier: Tier::Wait,
            artifact_id: Some(a1.as_str().into()),
            include_resends: true,
        };
        let (items, _) = st.take_feedback(&q, BASE).unwrap();
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
