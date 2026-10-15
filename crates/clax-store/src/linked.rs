//! Linked threads: a local artifact's comment threads kept in step with its
//! public twin's on a remote (spec 2026-10-15-linked-threads-design).
//!
//! On a local daemon:
//!
//! - **Twins.** Each push records the local version it sent and the public
//!   version it made ([`Store::record_remote_twin`]); a mirrored anchor's
//!   version is translated through them ([`Store::local_twin`],
//!   [`Store::remote_twin`]). A public version with no local twin (published
//!   there directly) keeps its number: the thread is stored on the newest
//!   pushed local version, marked `twin: false` with its `remote_version`.
//! - **Threads and comments** are paired with their twins in
//!   `linked_threads` and `linked_comments`. A thread or comment that came
//!   down from the remote is written here as it is there
//!   ([`Store::mirror_thread`], [`Store::mirror_comment`]): a person's
//!   comment as a viewer comment with no local public ID, an agent's as an
//!   agent comment of no local session, with the author's kind, public ID
//!   and name there kept beside it for display only. Its author is never a
//!   local person, agent or right; under the visibility filter it is of the
//!   unconnected class, which the owner sees.
//! - **Up.** Local writes on a linked artifact are found by
//!   [`Store::scan_linked`] and queued in `linked_outbox` with an
//!   idempotency key made here, so a resend after a lost answer, or the
//!   echo of an up-sent comment, maps to the same row. New local threads
//!   are queued only while the link is two-way (`download_only` off);
//!   replies and resolves on threads already linked always are. A row is
//!   deleted when the remote acknowledges it ([`Store::ack_outbox`]).
//! - **Ending a link** ([`Store::unlink_remote`], [`Store::remove_remote`])
//!   keeps the mirrored threads as read-only history (`unlinked_at`), and
//!   their unsent outbox rows until the owner drops them
//!   ([`Store::drop_unsent`]).
//!
//! On a hosted daemon, `comment_keys` keeps the idempotency key an agent
//! token's comment or new thread named and what it made
//! ([`Store::comment_key`]).

use super::Store;
use super::remotes::RemoteLink;
use super::threads::{anchor_record, check_body, comment_record, open_record, thread_in};
use crate::anchor::Anchor;
use crate::audit::AuditCtx;
use crate::model::{Comment, Thread};
use crate::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use std::collections::HashMap;

/// A thread or comment that started on the remote.
pub const ORIGIN_REMOTE: &str = "remote";
/// A thread or comment that started here.
pub const ORIGIN_LOCAL: &str = "local";

/// A local thread's pairing with its twin on a remote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkedThread {
    pub thread_id: String,
    pub remote_id: String,
    pub artifact_id: String,
    pub remote_artifact_id: String,
    /// `None` until an up-sent thread is acknowledged.
    pub remote_thread_id: Option<String>,
    /// [`ORIGIN_REMOTE`] or [`ORIGIN_LOCAL`].
    pub origin: String,
    /// The public version the thread is on, once known.
    pub remote_version: Option<u32>,
    /// Whether a local version is that public version's twin.
    pub twin: bool,
    /// The status both sides last agreed on.
    pub synced_status: String,
    pub remote_name: String,
    pub remote_origin: String,
    pub unlinked_at: Option<String>,
}

const LT_COLS: &str = "thread_id, remote_id, artifact_id, remote_artifact_id, remote_thread_id,
    origin, remote_version, twin, synced_status, remote_name, remote_origin, unlinked_at";

fn row_lt(r: &Row<'_>) -> rusqlite::Result<LinkedThread> {
    Ok(LinkedThread {
        thread_id: r.get(0)?,
        remote_id: r.get(1)?,
        artifact_id: r.get(2)?,
        remote_artifact_id: r.get(3)?,
        remote_thread_id: r.get(4)?,
        origin: r.get(5)?,
        remote_version: r.get(6)?,
        twin: r.get(7)?,
        synced_status: r.get(8)?,
        remote_name: r.get(9)?,
        remote_origin: r.get(10)?,
        unlinked_at: r.get(11)?,
    })
}

/// Who wrote a comment on the remote: display only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteAuthor {
    /// `viewer` (a person there) or `agent`.
    pub kind: String,
    /// Their public ID there, for a person.
    pub public_id: Option<String>,
    /// Their name there.
    pub name: String,
}

/// A comment from the remote to write here.
#[derive(Clone, Debug)]
pub struct MirrorComment {
    pub remote_comment_id: String,
    pub author: RemoteAuthor,
    pub body: String,
}

/// A thread from the remote to write here.
#[derive(Clone, Debug)]
pub struct MirrorThread {
    pub remote_thread_id: String,
    /// The public version it is on.
    pub remote_version: u32,
    pub anchor: Anchor,
    /// Its first comment.
    pub first: MirrorComment,
}

/// A local write waiting for the remote's acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutboxRow {
    pub seq: i64,
    pub remote_id: String,
    pub artifact_id: String,
    pub thread_id: String,
    /// `thread` (a new thread with its first comment), `comment`,
    /// `resolve` or `reopen`.
    pub kind: String,
    pub comment_id: Option<String>,
    pub idem_key: String,
    /// The local agent that wrote it, when one did (`None`: the owner).
    pub agent_name: Option<String>,
    pub agent_harness: Option<String>,
    /// Tries sent whose answer was never read: while above zero, the
    /// remote may have made what the row asks for.
    pub attempts: i64,
    pub last_error: Option<String>,
    /// Set once it was not sent within the backoff cap: shown as not
    /// sent, with Retry.
    pub failed_at: Option<String>,
    /// The comment's text, for showing an unsent row.
    pub body: Option<String>,
}

const OUT_COLS: &str = "o.seq, o.remote_id, o.artifact_id, o.thread_id, o.kind, o.comment_id,
    o.idem_key, o.agent_name, o.agent_harness, o.attempts, o.last_error, o.failed_at,
    (SELECT body FROM comments WHERE id = o.comment_id)";

fn row_out(r: &Row<'_>) -> rusqlite::Result<OutboxRow> {
    Ok(OutboxRow {
        seq: r.get(0)?,
        remote_id: r.get(1)?,
        artifact_id: r.get(2)?,
        thread_id: r.get(3)?,
        kind: r.get(4)?,
        comment_id: r.get(5)?,
        idem_key: r.get(6)?,
        agent_name: r.get(7)?,
        agent_harness: r.get(8)?,
        attempts: r.get(9)?,
        last_error: r.get(10)?,
        failed_at: r.get(11)?,
        body: r.get(12)?,
    })
}

/// What a thread view says of its link (`linked`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkedView {
    /// The remote's ID here (for its link's routes), name and origin
    /// ("clax.dev", `https://clax.dev`).
    pub remote_id: String,
    pub remote: String,
    pub remote_origin: String,
    /// Where the thread's public twin is: the public artifact, at the
    /// public version when it has no local twin.
    pub url: String,
    /// [`ORIGIN_REMOTE`] or [`ORIGIN_LOCAL`].
    pub origin: String,
    pub remote_version: Option<u32>,
    pub twin: bool,
    pub unlinked: bool,
    /// The thread's own up-send or status change: `pending`, `held` (on a
    /// version not pushed yet) or `failed`; `None` once acknowledged.
    pub state: Option<&'static str>,
    pub error: Option<String>,
    /// Each comment's link, by local comment ID.
    pub comments: HashMap<String, LinkedCommentView>,
}

/// What a comment view says of its link (`linked`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkedCommentView {
    /// [`ORIGIN_REMOTE`] or [`ORIGIN_LOCAL`].
    pub origin: String,
    /// The remote's name.
    pub remote: String,
    /// For a comment from the remote: its author's kind and name there.
    pub author_kind: Option<String>,
    pub author_name: Option<String>,
    /// `pending` or `failed` while its up-send waits; `None` once sent.
    pub state: Option<&'static str>,
}

/// A comment `c` the owner or one of the owner's agents wrote here: what
/// goes up as the token's person. Another local person's words stay here.
const BY_OWNER: &str = "((c.author_kind = 'viewer'
        AND c.author_public_id = (SELECT public_id FROM viewers WHERE owner = 1))
     OR (c.author_kind = 'agent' AND EXISTS (SELECT 1 FROM sessions s
        WHERE s.id = c.via_session_id AND s.person IS NULL)))";

/// How agents read a remote author: `<name> (on <remote>)`.
pub fn remote_label(name: &str, remote: &str) -> String {
    format!("{name} (on {remote})")
}

/// The idempotency key of a local write's up-send: made here, once.
fn new_key(kind: &str) -> String {
    format!("clax-linked:{kind}:{}", new_ulid().to_lowercase())
}

/// The local agent that wrote comment `cid`, when an agent did: its
/// session's name (else its harness) and harness.
fn agent_of_comment(c: &Connection, cid: &str) -> Result<(Option<String>, Option<String>)> {
    let row: Option<(String, Option<String>, Option<String>)> = c
        .query_row(
            "SELECT c.author_kind, s.name, s.harness FROM comments c
             LEFT JOIN sessions s ON s.id = c.via_session_id WHERE c.id = ?1",
            params![cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((kind, name, harness)) if kind == "agent" => {
            let harness = harness.unwrap_or_else(|| "claude".to_string());
            (Some(name.unwrap_or_else(|| harness.clone())), Some(harness))
        }
        _ => (None, None),
    })
}

#[allow(clippy::too_many_arguments)]
fn enqueue(
    c: &Connection,
    rid: &str,
    aid: &str,
    tid: &str,
    kind: &str,
    cid: Option<&str>,
    agent: (Option<String>, Option<String>),
    now: &str,
) -> Result<String> {
    let key = new_key(kind);
    c.execute(
        "INSERT INTO linked_outbox (remote_id, artifact_id, thread_id, kind, comment_id, idem_key,
            agent_name, agent_harness, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![rid, aid, tid, kind, cid, key, agent.0, agent.1, now],
    )?;
    Ok(key)
}

/// The public version local version `local_n` of `aid` became on public
/// artifact `raid` of remote `rid`, read on `c`: the newest recorded twin,
/// else the link's last push when it is that version.
fn remote_twin_in(
    c: &Connection,
    rid: &str,
    aid: &str,
    raid: &str,
    local_n: u32,
) -> Result<Option<u32>> {
    let n: Option<u32> = c.query_row(
        "SELECT MAX(remote_n) FROM remote_versions
         WHERE remote_id = ?1 AND remote_artifact_id = ?2 AND local_n = ?3",
        params![rid, raid, local_n],
        |r| r.get(0),
    )?;
    if n.is_some() {
        return Ok(n);
    }
    Ok(c.query_row(
        "SELECT remote_n FROM remote_links WHERE artifact_id = ?1 AND remote_id = ?2
            AND remote_artifact_id = ?3 AND pushed_n = ?4",
        params![aid, rid, raid, local_n],
        |r| r.get(0),
    )
    .optional()?)
}

impl Store {
    /// Records that local version `local_n` of `aid` was pushed as public
    /// version `remote_n` of `raid` on remote `rid`.
    pub fn record_remote_twin(
        &self,
        rid: &str,
        aid: &str,
        raid: &str,
        local_n: u32,
        remote_n: u32,
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO remote_versions (remote_id, artifact_id, remote_artifact_id, local_n, remote_n)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (remote_id, remote_artifact_id, remote_n) DO UPDATE SET
                    artifact_id = excluded.artifact_id, local_n = excluded.local_n",
                params![rid, aid, raid, local_n, remote_n],
            )?;
            Ok(())
        })
    }

    /// The local version that public version `remote_n` of `raid` on
    /// remote `rid` is the twin of: a recorded push, else the link's last
    /// push when it made that version; `None` for a version published there
    /// directly.
    pub fn local_twin(&self, rid: &str, raid: &str, remote_n: u32) -> Result<Option<u32>> {
        self.with_read(|c| {
            let n: Option<u32> = c
                .query_row(
                    "SELECT local_n FROM remote_versions
                     WHERE remote_id = ?1 AND remote_artifact_id = ?2 AND remote_n = ?3",
                    params![rid, raid, remote_n],
                    |r| r.get(0),
                )
                .optional()?;
            if n.is_some() {
                return Ok(n);
            }
            Ok(c.query_row(
                "SELECT pushed_n FROM remote_links
                 WHERE remote_id = ?1 AND remote_artifact_id = ?2 AND remote_n = ?3",
                params![rid, raid, remote_n],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// The public version of `raid` on remote `rid` that local version
    /// `local_n` of `aid` was pushed as; `None` for a version never pushed.
    pub fn remote_twin(
        &self,
        rid: &str,
        aid: &str,
        raid: &str,
        local_n: u32,
    ) -> Result<Option<u32>> {
        self.with_read(|c| remote_twin_in(c, rid, aid, raid, local_n))
    }

    /// Every link on remote `rid`, oldest first.
    pub fn links_of_remote(&self, rid: &str) -> Result<Vec<RemoteLink>> {
        Ok(self
            .all_remote_links()?
            .into_iter()
            .filter(|l| l.remote_id == rid)
            .collect())
    }

    /// Sets whether link `aid`–`rid` is download only: local threads stay
    /// local, while replies and resolves on linked threads still go up.
    /// Made two-way again, it sends only the threads started from then on:
    /// one started while it was download only stays local.
    ///
    /// # Errors
    /// `NotFound` for no such link.
    pub fn set_link_download_only(&self, aid: &str, rid: &str, on: bool) -> Result<()> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE remote_links SET download_only = ?3,
                    threads_since = CASE WHEN download_only = 1 AND ?3 = 0 THEN ?4
                                         ELSE threads_since END
                 WHERE artifact_id = ?1 AND remote_id = ?2",
                params![aid, rid, on, self.now()],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(())
        })
    }

    /// Records the newest activity time read from link `aid`–`rid`'s public
    /// threads: the next read asks for what changed since.
    pub fn set_sync_cursor(&self, aid: &str, rid: &str, cursor: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE remote_links SET sync_cursor = ?3 WHERE artifact_id = ?1 AND remote_id = ?2",
                params![aid, rid, cursor],
            )?;
            Ok(())
        })
    }

    /// Ends link `aid`–`rid`: its threads stay as read-only history and its
    /// unsent writes wait for the owner ([`Store::drop_unsent`]). The link
    /// it was.
    ///
    /// # Errors
    /// `NotFound` for no such link.
    pub fn unlink_remote(&self, aid: &str, rid: &str) -> Result<RemoteLink> {
        let link = self.remote_link(aid, rid)?.ok_or(CoreError::NotFound)?;
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE linked_threads SET unlinked_at = ?3
                 WHERE artifact_id = ?1 AND remote_id = ?2 AND unlinked_at IS NULL",
                params![aid, rid, self.now()],
            )?;
            tx.execute(
                "DELETE FROM remote_links WHERE artifact_id = ?1 AND remote_id = ?2",
                params![aid, rid],
            )?;
            Ok(())
        })?;
        Ok(link)
    }

    /// Thread `tid`'s link, if it has one.
    pub fn linked_thread(&self, tid: &str) -> Result<Option<LinkedThread>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("SELECT {LT_COLS} FROM linked_threads WHERE thread_id = ?1"),
                params![tid],
                row_lt,
            )
            .optional()?)
        })
    }

    /// The local twin of thread `rtid` on remote `rid`.
    pub fn linked_thread_by_remote(&self, rid: &str, rtid: &str) -> Result<Option<LinkedThread>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {LT_COLS} FROM linked_threads WHERE remote_id = ?1 AND remote_thread_id = ?2"
                ),
                params![rid, rtid],
                row_lt,
            )
            .optional()?)
        })
    }

    /// Whether comment `rcid` of remote `rid` has a local twin.
    pub fn has_linked_comment(&self, rid: &str, rcid: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM linked_comments WHERE remote_id = ?1 AND remote_comment_id = ?2)",
                params![rid, rcid],
                |r| r.get(0),
            )?)
        })
    }

    /// Writes thread `t` of remote `link.remote_id` here, with its first
    /// comment, on local version `version_n` (`twin`: whether that is the
    /// public version's twin), as [`ORIGIN_REMOTE`]. Recorded under `ctx`
    /// as `thread.open` and `comment.add`. Answers the thread, or the one
    /// written already for it.
    ///
    /// # Errors
    /// `NotFound` for an artifact gone; `invalid_comment` for an empty body.
    #[allow(clippy::too_many_arguments)]
    pub fn mirror_thread(
        &self,
        ctx: &AuditCtx,
        link: &RemoteLink,
        remote: (&str, &str),
        version_n: u32,
        twin: bool,
        t: MirrorThread,
    ) -> Result<Thread> {
        check_body(&t.first.body)?;
        let anchor = t.anchor.normalized();
        anchor.validate()?;
        let anchor_json = serde_json::to_string(&anchor).expect("anchors serialise");
        let tid = self.with_tx(|tx| {
            if let Some(tid) = tx
                .query_row(
                    "SELECT thread_id FROM linked_threads WHERE remote_id = ?1 AND remote_thread_id = ?2",
                    params![link.remote_id, t.remote_thread_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(tid);
            }
            if !super::threads::artifact_live(tx, &link.artifact_id)? {
                return Err(CoreError::NotFound);
            }
            let now = self.now();
            let tid = new_ulid();
            tx.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, sent_to_agent, has_clip, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'open', 0, 0, ?5)",
                params![tid, link.artifact_id, version_n, anchor_json, now],
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO linked_threads ({LT_COLS}, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'remote', ?6, ?7, 'open', ?8, ?9, NULL, ?10)"
                ),
                params![
                    tid,
                    link.remote_id,
                    link.artifact_id,
                    link.remote_artifact_id,
                    t.remote_thread_id,
                    t.remote_version,
                    twin,
                    remote.0,
                    remote.1,
                    now
                ],
            )?;
            let first = self.insert_mirrored(tx, &tid, &link.artifact_id, &link.remote_id, &t.first)?;
            let open = open_record(
                &now,
                &link.artifact_id,
                &tid,
                version_n,
                anchor_record(&anchor),
                None,
                false,
                Some(&first.id),
            );
            self.record_audit(tx, ctx, open)?;
            self.record_audit(tx, ctx, comment_record(&link.artifact_id, &first))?;
            Ok(tid)
        })?;
        self.get_thread(&tid)?.ok_or(CoreError::NotFound)
    }

    /// Comment `c` of remote `rid` written on local thread `tid`, as
    /// [`ORIGIN_REMOTE`]; `None` when it was written already (or is the
    /// echo of a comment sent from here). Never changes the thread's
    /// status: the remote's status is mirrored on its own.
    ///
    /// # Errors
    /// `NotFound` for a thread gone; `invalid_comment` for an empty body.
    pub fn mirror_comment(
        &self,
        ctx: &AuditCtx,
        tid: &str,
        rid: &str,
        c: &MirrorComment,
    ) -> Result<Option<Comment>> {
        check_body(&c.body)?;
        self.with_tx(|tx| {
            let known: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM linked_comments WHERE remote_id = ?1 AND remote_comment_id = ?2)",
                params![rid, c.remote_comment_id],
                |r| r.get(0),
            )?;
            if known {
                return Ok(None);
            }
            let t = thread_in(tx, tid)?.ok_or(CoreError::NotFound)?;
            let made = self.insert_mirrored(tx, tid, &t.artifact_id, rid, c)?;
            self.record_audit(tx, ctx, comment_record(&t.artifact_id, &made))?;
            Ok(Some(made))
        })
    }

    /// Inserts mirrored comment `c` on thread `tid` of `aid` with its link.
    fn insert_mirrored(
        &self,
        tx: &Connection,
        tid: &str,
        aid: &str,
        rid: &str,
        c: &MirrorComment,
    ) -> Result<Comment> {
        let now = self.now();
        let kind = if c.author.kind == "agent" {
            super::threads::AUTHOR_AGENT
        } else {
            super::threads::AUTHOR_VIEWER
        };
        let cid = new_ulid();
        tx.execute(
            "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, 0, ?5, ?6)",
            params![cid, tid, kind, c.author.name, c.body, now],
        )?;
        tx.execute(
            "INSERT INTO linked_comments (comment_id, thread_id, remote_id, remote_comment_id, idem_key,
                origin, author_kind, author_public_id, author_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'remote', ?6, ?7, ?8, ?9)",
            params![
                cid,
                tid,
                rid,
                c.remote_comment_id,
                format!("clax-linked:down:{rid}:{}", c.remote_comment_id),
                c.author.kind,
                c.author.public_id,
                c.author.name,
                now
            ],
        )?;
        if kind == super::threads::AUTHOR_AGENT {
            super::inbox::note_reply(tx, &cid, tid, aid, None, &c.body, &now)?;
        }
        Ok(Comment {
            id: cid,
            thread_id: tid.to_string(),
            author_kind: kind.to_string(),
            author_name: c.author.name.clone(),
            author_public_id: None,
            via_harness: None,
            via_page: false,
            body: c.body.clone(),
            created_at: now,
            page_authors: Vec::new(),
            agent_person: None,
        })
    }

    /// How agents read the author of each of `cids` that came from a
    /// remote: `<name there> (on <remote>)`, by comment ID.
    pub fn remote_author_labels(&self, cids: &[String]) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        if cids.is_empty() {
            return Ok(out);
        }
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT lc.author_name, lt.remote_name FROM linked_comments lc
                   JOIN linked_threads lt ON lt.thread_id = lc.thread_id
                 WHERE lc.comment_id = ?1 AND lc.origin = 'remote'",
            )?;
            for cid in cids {
                let row: Option<(Option<String>, String)> = stmt
                    .query_row(params![cid], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                if let Some((name, remote)) = row {
                    out.insert(
                        cid.clone(),
                        remote_label(name.as_deref().unwrap_or("Someone"), &remote),
                    );
                }
            }
            Ok(out)
        })
    }

    /// Records that both sides agree thread `tid` is `status`.
    pub fn set_synced_status(&self, tid: &str, status: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE linked_threads SET synced_status = ?2 WHERE thread_id = ?1",
                params![tid, status],
            )?;
            Ok(())
        })
    }

    /// Whether thread `tid` has a status change waiting to go up.
    pub fn status_pending(&self, tid: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM linked_outbox WHERE thread_id = ?1
                    AND kind IN ('resolve', 'reopen'))",
                params![tid],
                |r| r.get(0),
            )?)
        })
    }

    /// Queues the local writes on remote `rid`'s links not queued yet, in
    /// the order they were made: for a two-way link, each thread the owner
    /// or an agent of the owner's started on its artifact since it was
    /// linked (or last made two-way again), with its comments; on every linked thread still linked, each
    /// comment of theirs not linked yet, and each change from the status
    /// both sides agreed on (a resolve only when theirs). Another local
    /// person's words never go up as the token's person. How many rows it
    /// queued.
    pub fn scan_linked(&self, rid: &str) -> Result<usize> {
        let links = self.links_of_remote(rid)?;
        let (rname, rorigin): (String, String) = match self.find_remote(rid)? {
            Some(r) => (r.name, r.origin),
            None => return Ok(0),
        };
        self.with_tx(|tx| {
            let now = self.now();
            let mut queued = 0;
            for l in &links {
                if !l.download_only {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT t.id FROM threads t WHERE t.artifact_id = ?1
                           AND t.created_at >= COALESCE((SELECT threads_since FROM remote_links
                                 WHERE artifact_id = ?1 AND remote_id = ?3), ?2)
                           AND NOT EXISTS (SELECT 1 FROM linked_threads lt WHERE lt.thread_id = t.id)
                           AND EXISTS (SELECT 1 FROM comments c WHERE c.id = (
                                 SELECT id FROM comments WHERE thread_id = t.id
                                 ORDER BY created_at, id LIMIT 1) AND {BY_OWNER})
                         ORDER BY t.created_at, t.id"
                    ))?;
                    let fresh: Vec<String> = stmt
                        .query_map(params![l.artifact_id, l.created_at, rid], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    drop(stmt);
                    for tid in fresh {
                        tx.execute(
                            &format!(
                                "INSERT INTO linked_threads ({LT_COLS}, created_at)
                                 VALUES (?1, ?2, ?3, ?4, NULL, 'local', NULL, 1, 'open', ?5, ?6, NULL, ?7)"
                            ),
                            params![tid, rid, l.artifact_id, l.remote_artifact_id, rname, rorigin, now],
                        )?;
                        let first: Option<String> = tx
                            .query_row(
                                "SELECT id FROM comments WHERE thread_id = ?1 ORDER BY created_at, id LIMIT 1",
                                params![tid],
                                |r| r.get(0),
                            )
                            .optional()?;
                        let Some(first) = first else { continue };
                        let key = enqueue(
                            tx,
                            rid,
                            &l.artifact_id,
                            &tid,
                            "thread",
                            Some(&first),
                            agent_of_comment(tx, &first)?,
                            &now,
                        )?;
                        tx.execute(
                            "INSERT INTO linked_comments (comment_id, thread_id, remote_id, idem_key, origin, created_at)
                             VALUES (?1, ?2, ?3, ?4, 'local', ?5)",
                            params![first, tid, rid, key, now],
                        )?;
                        queued += 1;
                    }
                }
                // Comments on linked threads not linked yet.
                let mut stmt = tx.prepare(&format!(
                    "SELECT c.id, c.thread_id FROM comments c
                       JOIN linked_threads lt ON lt.thread_id = c.thread_id
                     WHERE lt.remote_id = ?1 AND lt.artifact_id = ?2 AND lt.unlinked_at IS NULL
                       AND NOT EXISTS (SELECT 1 FROM linked_comments lc WHERE lc.comment_id = c.id)
                       AND {BY_OWNER}
                     ORDER BY c.created_at, c.id"
                ))?;
                let fresh: Vec<(String, String)> = stmt
                    .query_map(params![rid, l.artifact_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                for (cid, tid) in fresh {
                    let key = enqueue(
                        tx,
                        rid,
                        &l.artifact_id,
                        &tid,
                        "comment",
                        Some(&cid),
                        agent_of_comment(tx, &cid)?,
                        &now,
                    )?;
                    tx.execute(
                        "INSERT INTO linked_comments (comment_id, thread_id, remote_id, idem_key, origin, created_at)
                         VALUES (?1, ?2, ?3, ?4, 'local', ?5)",
                        params![cid, tid, rid, key, now],
                    )?;
                    queued += 1;
                }
                // Status changes made here.
                let mut stmt = tx.prepare(
                    "SELECT t.id, t.status, t.resolved_by FROM threads t
                       JOIN linked_threads lt ON lt.thread_id = t.id
                     WHERE lt.remote_id = ?1 AND lt.artifact_id = ?2 AND lt.unlinked_at IS NULL
                       AND t.status != lt.synced_status
                       AND (t.status = 'open'
                            OR (t.resolved_by LIKE 'agent:%'
                                AND COALESCE(t.resolved_by_person, '*owner') = '*owner')
                            OR t.resolved_by = 'viewer:' ||
                                 (SELECT public_id FROM viewers WHERE owner = 1))
                     ORDER BY t.created_at, t.id",
                )?;
                let changed: Vec<(String, String, Option<String>)> = stmt
                    .query_map(params![rid, l.artifact_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                for (tid, status, by) in changed {
                    let kind = if status == "resolved" { "resolve" } else { "reopen" };
                    let agent = match by.as_deref().and_then(|b| b.strip_prefix("agent:")) {
                        Some(h) => (Some(h.to_string()), Some(h.to_string())),
                        None => (None, None),
                    };
                    enqueue(tx, rid, &l.artifact_id, &tid, kind, None, agent, &now)?;
                    tx.execute(
                        "UPDATE linked_threads SET synced_status = ?2 WHERE thread_id = ?1",
                        params![tid, status],
                    )?;
                    queued += 1;
                }
            }
            Ok(queued)
        })
    }

    /// Remote `rid`'s unacknowledged writes, oldest first.
    pub fn outbox_of(&self, rid: &str) -> Result<Vec<OutboxRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {OUT_COLS} FROM linked_outbox o WHERE o.remote_id = ?1 ORDER BY o.seq"
            ))?;
            Ok(stmt
                .query_map(params![rid], row_out)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The writes on artifact `aid` not sent to a remote it is no longer
    /// linked to (unlinked, or the remote removed), oldest first: what the
    /// owner confirms dropping.
    pub fn unsent_of(&self, aid: &str) -> Result<Vec<OutboxRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {OUT_COLS} FROM linked_outbox o WHERE o.artifact_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM remote_links l
                                   WHERE l.artifact_id = o.artifact_id AND l.remote_id = o.remote_id)
                 ORDER BY o.seq"
            ))?;
            Ok(stmt
                .query_map(params![aid], row_out)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Drops the writes on `aid` that [`Store::unsent_of`] lists; how many.
    pub fn drop_unsent(&self, aid: &str) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "DELETE FROM linked_outbox WHERE artifact_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM remote_links l
                                   WHERE l.artifact_id = linked_outbox.artifact_id
                                     AND l.remote_id = linked_outbox.remote_id)",
                params![aid],
            )?)
        })
    }

    /// Records that outbox row `seq` is about to be sent: until an answer
    /// is read, the remote may have made it (a lost answer, or this daemon
    /// stopping first).
    pub fn outbox_trying(&self, seq: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE linked_outbox SET attempts = attempts + 1 WHERE seq = ?1",
                params![seq],
            )?;
            Ok(())
        })
    }

    /// Records why a try at outbox row `seq` got no answer.
    pub fn outbox_attempted(&self, seq: i64, error: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE linked_outbox SET last_error = ?2 WHERE seq = ?1",
                params![seq, error],
            )?;
            Ok(())
        })
    }

    /// Marks outbox row `seq` refused by the remote with `error`: it made
    /// nothing, and the row waits for Retry.
    pub fn outbox_failed(&self, seq: i64, error: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE linked_outbox SET last_error = ?2, attempts = 0,
                    failed_at = COALESCE(failed_at, ?3) WHERE seq = ?1",
                params![seq, error, self.now()],
            )?;
            Ok(())
        })
    }

    /// Marks every unsent row of remote `rid` not sent with `error`; how many.
    pub fn outbox_failed_all(&self, rid: &str, error: &str) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE linked_outbox SET last_error = ?2, failed_at = ?3
                 WHERE remote_id = ?1 AND failed_at IS NULL",
                params![rid, error, self.now()],
            )?)
        })
    }

    /// Clears the not-sent mark of the rows on artifact `aid` (every
    /// artifact for `None`) of remote `rid`, so they are tried again; how
    /// many.
    pub fn outbox_retry(&self, rid: &str, aid: Option<&str>) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE linked_outbox SET failed_at = NULL
                 WHERE remote_id = ?1 AND (?2 IS NULL OR artifact_id = ?2) AND failed_at IS NOT NULL",
                params![rid, aid],
            )?)
        })
    }

    /// Records the remote's acknowledgement of outbox row `seq` and deletes
    /// it: for a `thread`, its twin `rtid` there, on public version
    /// `remote_version`; for a `thread` or `comment`, the comment `rcid` it
    /// made there.
    pub fn ack_outbox(
        &self,
        seq: i64,
        rtid: Option<&str>,
        rcid: Option<&str>,
        remote_version: Option<u32>,
    ) -> Result<()> {
        self.with_tx(|tx| {
            let row: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT thread_id, comment_id FROM linked_outbox WHERE seq = ?1",
                    params![seq],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((tid, cid)) = row else {
                return Ok(());
            };
            if let Some(rtid) = rtid {
                tx.execute(
                    "UPDATE linked_threads SET remote_thread_id = ?2,
                        remote_version = COALESCE(?3, remote_version) WHERE thread_id = ?1",
                    params![tid, rtid, remote_version],
                )?;
            }
            if let (Some(cid), Some(rcid)) = (cid, rcid) {
                tx.execute(
                    "UPDATE linked_comments SET remote_comment_id = ?2 WHERE comment_id = ?1",
                    params![cid, rcid],
                )?;
            }
            tx.execute("DELETE FROM linked_outbox WHERE seq = ?1", params![seq])?;
            Ok(())
        })
    }

    /// What each of `threads`' views says of its link, by thread ID.
    pub fn linked_views(&self, threads: &[Thread]) -> Result<HashMap<String, LinkedView>> {
        let mut out = HashMap::new();
        if threads.is_empty() {
            return Ok(out);
        }
        self.with_read(|c| {
            let any: bool =
                c.query_row("SELECT EXISTS (SELECT 1 FROM linked_threads)", [], |r| {
                    r.get(0)
                })?;
            if !any {
                return Ok(out);
            }
            for t in threads {
                let Some(lt) = c
                    .query_row(
                        &format!("SELECT {LT_COLS} FROM linked_threads WHERE thread_id = ?1"),
                        params![t.id],
                        row_lt,
                    )
                    .optional()?
                else {
                    continue;
                };
                let mut comments = HashMap::new();
                let mut stmt = c.prepare_cached(
                    "SELECT lc.comment_id, lc.origin, lc.author_kind, lc.author_name,
                        (SELECT CASE WHEN o.failed_at IS NULL THEN 'pending' ELSE 'failed' END
                           FROM linked_outbox o WHERE o.comment_id = lc.comment_id)
                     FROM linked_comments lc WHERE lc.thread_id = ?1",
                )?;
                let mut rows = stmt.query(params![t.id])?;
                while let Some(r) = rows.next()? {
                    let state: Option<String> = r.get(4)?;
                    comments.insert(
                        r.get::<_, String>(0)?,
                        LinkedCommentView {
                            origin: r.get(1)?,
                            remote: lt.remote_name.clone(),
                            author_kind: r.get(2)?,
                            author_name: r.get(3)?,
                            state: state.map(|s| if s == "failed" { "failed" } else { "pending" }),
                        },
                    );
                }
                drop(rows);
                let own: Option<(String, Option<String>, Option<String>)> = c
                    .query_row(
                        "SELECT kind, failed_at, last_error FROM linked_outbox
                         WHERE thread_id = ?1 AND kind IN ('thread', 'resolve', 'reopen')
                         ORDER BY seq LIMIT 1",
                        params![t.id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let (state, error) = match own {
                    None => (None, None),
                    Some((_, Some(_), e)) => (Some("failed"), e),
                    Some((kind, None, e)) => {
                        let held = kind == "thread"
                            && !t.anchor.is_whole()
                            && remote_twin_in(
                                c,
                                &lt.remote_id,
                                &lt.artifact_id,
                                &lt.remote_artifact_id,
                                t.version_n,
                            )?
                            .is_none();
                        (Some(if held { "held" } else { "pending" }), e)
                    }
                };
                let base = format!("{}/a/{}", lt.remote_origin, lt.remote_artifact_id);
                let url = match (lt.twin, lt.remote_version) {
                    (false, Some(n)) => format!("{base}/v/{n}"),
                    _ => base,
                };
                out.insert(
                    t.id.clone(),
                    LinkedView {
                        remote_id: lt.remote_id,
                        remote: lt.remote_name,
                        remote_origin: lt.remote_origin,
                        url,
                        origin: lt.origin,
                        remote_version: lt.remote_version,
                        twin: lt.twin,
                        unlinked: lt.unlinked_at.is_some(),
                        state,
                        error,
                        comments,
                    },
                );
            }
            Ok(out)
        })
    }

    /// Where a mirrored thread `tid`'s clip is cached:
    /// `<home>/remote-cache/clips/<tid>.png`, named by the local thread ID.
    pub fn remote_clip_file(&self, tid: &str) -> std::path::PathBuf {
        self.home
            .root()
            .join("remote-cache")
            .join("clips")
            .join(format!("{tid}.png"))
    }

    /// Caches `bytes` as mirrored thread `tid`'s clip and marks the thread
    /// as having one.
    ///
    /// # Errors
    /// `invalid_clip` for bytes that are not a PNG within the clip limit;
    /// the file system's (a Worker has none: the sync runs only on a local
    /// daemon).
    pub fn cache_remote_clip(&self, tid: &str, bytes: &[u8]) -> Result<std::path::PathBuf> {
        if let Some(p) = super::threads::clip_problem(bytes) {
            return Err(CoreError::invalid("invalid_clip", p));
        }
        let path = self.remote_clip_file(tid);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("png.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE threads SET has_clip = 1 WHERE id = ?1
                   AND EXISTS (SELECT 1 FROM linked_threads WHERE thread_id = ?1 AND origin = 'remote')",
                params![tid],
            )?;
            Ok(())
        })?;
        Ok(path)
    }

    /// Mirrored thread `tid`'s cached clip file, when there is one.
    pub(crate) fn cached_remote_clip(&self, tid: &str) -> Option<std::path::PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Some(self.remote_clip_file(tid)).filter(|p| p.is_file())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = tid;
            None
        }
    }

    /// On a hosted daemon: the thread and comment that agent token
    /// `grant_id`'s comment or new thread keyed `key` made, if one did.
    pub fn comment_key(&self, grant_id: &str, key: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT thread_id, comment_id FROM comment_keys WHERE grant_id = ?1 AND key = ?2",
                params![grant_id, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// On a hosted daemon: records that agent token `grant_id`'s write
    /// keyed `key` made comment `cid` on thread `tid`. A key recorded
    /// already keeps what it made.
    pub fn record_comment_key(
        &self,
        grant_id: &str,
        key: &str,
        tid: &str,
        cid: &str,
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO comment_keys (grant_id, key, thread_id, comment_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![grant_id, key, tid, cid, self.now()],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Home;
    use crate::remotes::{PendingSignIn, SignedInAs};
    use crate::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread};
    use crate::working::ManualClock;
    use crate::{ArtifactId, Secret};
    use std::sync::Arc;

    struct W {
        _d: tempfile::TempDir,
        st: Store,
        clock: Arc<ManualClock>,
        rid: String,
        aid: String,
        /// The owner's public ID.
        owner: String,
    }

    fn world() -> W {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-15T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        let r = st
            .add_pending_remote(
                "https://clax.example",
                "clax.example",
                &PendingSignIn {
                    device_code: Secret::new("cxd".into()),
                    user_code: "WXYZ-4821".into(),
                    verification_url: "https://clax.example/device".into(),
                    expires_at: "2026-10-15T10:10:00.000Z".into(),
                    interval_s: 5,
                },
            )
            .unwrap();
        st.connect_remote(
            &r.id,
            &SignedInAs {
                token: Secret::new("cxa".into()),
                person_public_id: "p1".into(),
                person_name: Some("Ada".into()),
                grant_id: "g1".into(),
            },
        )
        .unwrap();
        let req = serde_json::json!({"title": "Linked",
            "files": {"index.html": {"content": "<p>one</p>", "encoding": "utf8"}}});
        let p = crate::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
        let a = st.create_artifact(&AuditCtx::DAEMON, p, None).unwrap();
        let owner = st.owner_viewer(false).unwrap().public_id;
        W {
            _d: dir,
            st,
            clock,
            rid: r.id,
            aid: a.0.id,
            owner,
        }
    }

    fn link(w: &W, pushed: (u32, u32)) -> RemoteLink {
        let l =
            w.st.record_remote_push(
                &w.aid,
                &w.rid,
                "pub1",
                "https://clax.example/a/pub1",
                pushed,
                None,
            )
            .unwrap();
        w.st.record_remote_twin(&w.rid, &w.aid, "pub1", pushed.0, pushed.1)
            .unwrap();
        l
    }

    fn mirror(text: &str, rcid: &str) -> MirrorComment {
        MirrorComment {
            remote_comment_id: rcid.into(),
            author: RemoteAuthor {
                kind: "viewer".into(),
                public_id: Some("pub_sam".into()),
                name: "Sam".into(),
            },
            body: text.into(),
        }
    }

    fn whole() -> Anchor {
        serde_json::from_value(serde_json::json!({"kind": "artifact"})).unwrap()
    }

    #[test]
    fn mapping_is_idempotent_and_authors_stay_remote() {
        let w = world();
        let l = link(&w, (1, 1));
        let mt = MirrorThread {
            remote_thread_id: "rt1".into(),
            remote_version: 1,
            anchor: whole(),
            first: mirror("make it bluer", "rc1"),
        };
        let remote = ("clax.example", "https://clax.example");
        let t =
            w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt.clone())
                .unwrap();
        let again =
            w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt)
                .unwrap();
        assert_eq!(t.id, again.id, "one local twin per remote thread");
        assert_eq!(t.comments.len(), 1);
        let c = &t.comments[0];
        assert_eq!(
            (
                c.author_kind.as_str(),
                c.author_name.as_str(),
                c.author_public_id.as_deref()
            ),
            (AUTHOR_VIEWER, "Sam", None),
            "never mapped to a local person"
        );
        assert!(
            w.st.mirror_comment(
                &AuditCtx::DAEMON,
                &t.id,
                &w.rid,
                &mirror("make it bluer", "rc1")
            )
            .unwrap()
            .is_none()
        );
        let mut agent = mirror("done", "rc2");
        agent.author.kind = "agent".into();
        let made =
            w.st.mirror_comment(&AuditCtx::DAEMON, &t.id, &w.rid, &agent)
                .unwrap()
                .unwrap();
        assert_eq!(made.author_kind, AUTHOR_AGENT);
        assert!(
            w.st.mirror_comment(&AuditCtx::DAEMON, &t.id, &w.rid, &agent)
                .unwrap()
                .is_none()
        );
        let v =
            w.st.linked_views(&[w.st.get_thread(&t.id).unwrap().unwrap()])
                .unwrap();
        let v = &v[&t.id];
        assert_eq!((v.origin.as_str(), v.twin, v.state), ("remote", true, None));
        assert_eq!(v.comments[&made.id].author_name.as_deref(), Some("Sam"));
        // Nothing mirrored goes back up.
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0);
    }

    #[test]
    fn an_up_sent_comments_echo_maps_to_the_same_row() {
        let w = world();
        let l = link(&w, (1, 1));
        let t =
            w.st.mirror_thread(
                &AuditCtx::DAEMON,
                &l,
                ("clax.example", "https://clax.example"),
                1,
                true,
                MirrorThread {
                    remote_thread_id: "rt1".into(),
                    remote_version: 1,
                    anchor: whole(),
                    first: mirror("hi", "rc1"),
                },
            )
            .unwrap();
        w.clock.advance(1);
        let reply =
            w.st.add_comment(
                &AuditCtx::DAEMON,
                &t.id,
                NewComment {
                    author_kind: AUTHOR_VIEWER,
                    author_name: "Owner".into(),
                    author_public_id: Some(w.owner.clone()),
                    via_session_id: None,
                    body: "on it".into(),
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0, "queued once");
        let out = w.st.outbox_of(&w.rid).unwrap();
        assert_eq!(out[0].comment_id.as_deref(), Some(reply.id.as_str()));
        let key = out[0].idem_key.clone();
        // A lost answer: tried, the row stays with its key, marked as
        // perhaps made there.
        w.st.outbox_trying(out[0].seq).unwrap();
        w.st.outbox_attempted(out[0].seq, "unreachable").unwrap();
        let row = &w.st.outbox_of(&w.rid).unwrap()[0];
        assert_eq!((row.idem_key.as_str(), row.attempts), (key.as_str(), 1));
        // A refusal made nothing there.
        w.st.outbox_failed(out[0].seq, "invalid_args: no").unwrap();
        assert_eq!(w.st.outbox_of(&w.rid).unwrap()[0].attempts, 0);
        w.st.ack_outbox(out[0].seq, None, Some("rc9"), None)
            .unwrap();
        assert!(w.st.outbox_of(&w.rid).unwrap().is_empty());
        // Its echo coming back down is the same row.
        assert!(w.st.has_linked_comment(&w.rid, "rc9").unwrap());
        assert!(
            w.st.mirror_comment(&AuditCtx::DAEMON, &t.id, &w.rid, &mirror("on it", "rc9"))
                .unwrap()
                .is_none()
        );
        assert_eq!(w.st.get_thread(&t.id).unwrap().unwrap().comments.len(), 2);
    }

    #[test]
    fn the_outbox_keeps_each_threads_order() {
        let w = world();
        link(&w, (1, 1));
        w.clock.advance(1);
        let id = ArtifactId::parse(&w.aid).unwrap();
        let t =
            w.st.create_thread(
                &AuditCtx::DAEMON,
                &id,
                NewThread {
                    version_n: 1,
                    anchor: whole(),
                    author_name: "Owner".into(),
                    author_public_id: Some(w.owner.clone()),
                    body: "first".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        for b in ["second", "third"] {
            w.clock.advance(1);
            w.st.add_comment(
                &AuditCtx::DAEMON,
                &t.id,
                NewComment {
                    author_kind: AUTHOR_VIEWER,
                    author_name: "Owner".into(),
                    author_public_id: Some(w.owner.clone()),
                    via_session_id: None,
                    body: b.into(),
                    via_page: false,
                },
            )
            .unwrap();
        }
        // Someone else's word stays here; the owner's resolve goes up.
        w.clock.advance(1);
        w.st.add_comment(
            &AuditCtx::DAEMON,
            &t.id,
            NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: "A LAN viewer".into(),
                author_public_id: None,
                via_session_id: None,
                body: "not the owner's".into(),
                via_page: false,
            },
        )
        .unwrap();
        w.st.resolve_thread(&AuditCtx::DAEMON, &t.id, &format!("viewer:{}", w.owner))
            .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 4);
        let out = w.st.outbox_of(&w.rid).unwrap();
        let kinds: Vec<&str> = out.iter().map(|o| o.kind.as_str()).collect();
        assert_eq!(kinds, ["thread", "comment", "comment", "resolve"]);
        let bodies: Vec<Option<&str>> = out.iter().map(|o| o.body.as_deref()).collect();
        assert_eq!(bodies, [Some("first"), Some("second"), Some("third"), None]);
        assert!(out.windows(2).all(|p| p[0].seq < p[1].seq));
        let v =
            w.st.linked_views(&[w.st.get_thread(&t.id).unwrap().unwrap()])
                .unwrap();
        assert_eq!(v[&t.id].state, Some("pending"));
        // Download only: a new local thread stays local.
        w.st.set_link_download_only(&w.aid, &w.rid, true).unwrap();
        w.clock.advance(1);
        w.st.create_thread(
            &AuditCtx::DAEMON,
            &id,
            NewThread {
                version_n: 1,
                anchor: whole(),
                author_name: "Owner".into(),
                author_public_id: Some(w.owner.clone()),
                body: "private".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0);
        // Two-way again: the thread started meanwhile still stays local,
        // while one started from now on goes up.
        w.clock.advance(1);
        w.st.set_link_download_only(&w.aid, &w.rid, false).unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0, "kept local");
        w.clock.advance(1);
        let later =
            w.st.create_thread(
                &AuditCtx::DAEMON,
                &id,
                NewThread {
                    version_n: 1,
                    anchor: whole(),
                    author_name: "Owner".into(),
                    author_public_id: Some(w.owner.clone()),
                    body: "shared".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        let out = w.st.outbox_of(&w.rid).unwrap();
        assert_eq!(out.last().unwrap().thread_id, later.id);
        w.st.ack_outbox(out.last().unwrap().seq, Some("rt9"), Some("rc9"), Some(1))
            .unwrap();
        // Unlinked: unsent rows wait for the owner, then go.
        w.st.unlink_remote(&w.aid, &w.rid).unwrap();
        assert_eq!(w.st.unsent_of(&w.aid).unwrap().len(), 4);
        assert!(w.st.linked_views(std::slice::from_ref(&t)).unwrap()[&t.id].unlinked);
        assert_eq!(w.st.drop_unsent(&w.aid).unwrap(), 4);
        assert!(w.st.unsent_of(&w.aid).unwrap().is_empty());
    }

    #[test]
    fn anchors_translate_through_the_push_records() {
        let w = world();
        link(&w, (1, 1));
        // Local 2 and 3 pushed as public 2 and 4 (someone published 3 there).
        w.st.record_remote_push(
            &w.aid,
            &w.rid,
            "pub1",
            "https://clax.example/a/pub1",
            (3, 4),
            None,
        )
        .unwrap();
        w.st.record_remote_twin(&w.rid, &w.aid, "pub1", 2, 2)
            .unwrap();
        w.st.record_remote_twin(&w.rid, &w.aid, "pub1", 3, 4)
            .unwrap();
        assert_eq!(w.st.local_twin(&w.rid, "pub1", 1).unwrap(), Some(1));
        assert_eq!(w.st.local_twin(&w.rid, "pub1", 2).unwrap(), Some(2));
        assert_eq!(w.st.local_twin(&w.rid, "pub1", 4).unwrap(), Some(3));
        assert_eq!(
            w.st.remote_twin(&w.rid, &w.aid, "pub1", 3).unwrap(),
            Some(4)
        );
        assert_eq!(
            w.st.remote_twin(&w.rid, &w.aid, "pub1", 2).unwrap(),
            Some(2)
        );
        // A version with no twin: public 3 was published there directly,
        // local 5 was never pushed.
        assert_eq!(w.st.local_twin(&w.rid, "pub1", 3).unwrap(), None);
        assert_eq!(w.st.remote_twin(&w.rid, &w.aid, "pub1", 5).unwrap(), None);
        let l = w.st.remote_link(&w.aid, &w.rid).unwrap().unwrap();
        let t =
            w.st.mirror_thread(
                &AuditCtx::DAEMON,
                &l,
                ("clax.example", "https://clax.example"),
                l.pushed_n.min(1),
                false,
                MirrorThread {
                    remote_thread_id: "rt3".into(),
                    remote_version: 3,
                    anchor: whole(),
                    first: mirror("on their version", "rc3"),
                },
            )
            .unwrap();
        let v = &w.st.linked_views(std::slice::from_ref(&t)).unwrap()[&t.id];
        assert_eq!((v.twin, v.remote_version), (false, Some(3)));
        assert_eq!(v.url, "https://clax.example/a/pub1/v/3");
    }

    #[test]
    fn a_comment_key_keeps_what_it_made() {
        let w = world();
        assert!(w.st.comment_key("g1", "k").unwrap().is_none());
        w.st.record_comment_key("g1", "k", "t1", "c1").unwrap();
        w.st.record_comment_key("g1", "k", "t2", "c2").unwrap();
        assert_eq!(
            w.st.comment_key("g1", "k").unwrap(),
            Some(("t1".into(), "c1".into()))
        );
        assert!(w.st.comment_key("g2", "k").unwrap().is_none());
    }
}
