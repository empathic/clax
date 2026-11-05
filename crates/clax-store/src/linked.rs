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
//! - **Deleting a linked thread** here ends its pairing alone
//!   (`unlinked_at`) and drops its unsent writes: its twin on the remote
//!   stays there and is not written here again.
//!
//! On a hosted daemon, `comment_keys` keeps the idempotency key an agent
//! token's comment or new thread named and what it made
//! ([`Store::comment_key`]).

use super::Store;
use super::remotes::RemoteLink;
use super::threads::{anchor_record, check_body, comment_record, open_record, thread_in};
use clax_core::anchor::Anchor;
use clax_core::audit::AuditCtx;
use clax_core::model::{Comment, Thread};
use clax_core::{CoreError, Result, new_ulid};
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

/// Whether outbox row `o`'s link has ended: the artifact is not linked to
/// the remote, or the row's thread pairing was ended (by an unlink or the
/// remote's removal, which end every pairing of the link, or by a link
/// moved to another public copy).
const ENDED: &str = "(NOT EXISTS (SELECT 1 FROM remote_links l
                        WHERE l.artifact_id = o.artifact_id AND l.remote_id = o.remote_id)
     OR EXISTS (SELECT 1 FROM linked_threads lt
                WHERE lt.thread_id = o.thread_id AND lt.unlinked_at IS NOT NULL))";

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

/// A local question's pairing with its copy on a remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedQuestion {
    pub question_id: String,
    pub remote_id: String,
    pub artifact_id: String,
    pub remote_artifact_id: String,
    /// `None` until the remote acknowledged the copy.
    pub remote_question_id: Option<String>,
    /// The Idempotency-Key the copy's creation carries.
    pub idem_key: String,
    /// Tries sent whose answer was never read.
    pub attempts: i64,
    /// Set once it was not sent within the backoff cap, or the remote
    /// refused it: shown as not sent, with Retry.
    pub failed_at: Option<String>,
    /// The local question's status (`open`, `answered`, ...).
    pub status: String,
}

const LQ_COLS: &str = "lq.question_id, lq.remote_id, lq.artifact_id, lq.remote_artifact_id,
    lq.remote_question_id, lq.idem_key, lq.attempts, lq.failed_at, q.status";

fn row_lq(r: &Row<'_>) -> rusqlite::Result<LinkedQuestion> {
    Ok(LinkedQuestion {
        question_id: r.get(0)?,
        remote_id: r.get(1)?,
        artifact_id: r.get(2)?,
        remote_artifact_id: r.get(3)?,
        remote_question_id: r.get(4)?,
        idem_key: r.get(5)?,
        attempts: r.get(6)?,
        failed_at: r.get(7)?,
        status: r.get(8)?,
    })
}

/// What a question view says of its copy on a remote (`linked`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkedQuestionView {
    /// The remote's ID here (for its link's routes), name and origin.
    pub remote_id: String,
    pub remote: String,
    pub remote_origin: String,
    /// The public artifact the copy is about.
    pub url: String,
    /// Whether the remote acknowledged the copy.
    pub sent: bool,
    /// `pending` until the copy is acknowledged, `failed` when it was not
    /// sent (Retry); `None` once sent or both sides closed.
    pub state: Option<&'static str>,
    /// Why the copy is not sent, while `state` is set; once the copy is
    /// done, why the remote's answer was refused here (the question then
    /// stays open here); else `None`.
    pub error: Option<String>,
    /// Whether the answer or decline came from the remote.
    pub answered_there: bool,
    /// The link is gone: the question is local only from then on.
    pub unlinked: bool,
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

/// Whether link `l`, read before the transaction `c` is in, still holds:
/// not ended, nor moved to another public artifact since.
fn link_holds_in(c: &Connection, l: &RemoteLink) -> Result<bool> {
    let raid: Option<String> = c
        .query_row(
            "SELECT remote_artifact_id FROM remote_links WHERE artifact_id = ?1 AND remote_id = ?2",
            params![l.artifact_id, l.remote_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raid.as_deref() == Some(l.remote_artifact_id.as_str()))
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

/// [`Store::remote_author_name`], read on `c`.
pub(crate) fn remote_author_name_in(c: &Connection, key: &str) -> Result<Option<String>> {
    let row: Option<(Option<String>, String)> = c
        .query_row(
            "SELECT lc.author_name, lt.remote_name FROM linked_comments lc
               JOIN linked_threads lt ON lt.thread_id = lc.thread_id
             WHERE lc.author_key = ?1 ORDER BY lc.created_at DESC LIMIT 1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(name, remote)| remote_label(name.as_deref().unwrap_or("Someone"), &remote)))
}

/// The key a comment mirrored from remote `rid` by `author` is read by
/// ([`clax_core::visibility::remote_author`]): `None` when the author is
/// the person this Clax is signed in there as (their words are the
/// owner's here), and for every comment read with an agent token
/// ([`super::remotes::CLIENT_KIND_AGENT`]), whose reads the remote already
/// filtered by that person's approvals there. An agent's comment with no
/// person is that Clax's owner's agent's; a viewer's with none is an
/// anonymous viewer's. A remote whose kind is not known yet keys them
/// all, as a device's.
fn mirrored_author_key(c: &Connection, rid: &str, author: &RemoteAuthor) -> Result<Option<String>> {
    let (origin, me, kind): (String, Option<String>, Option<String>) = c.query_row(
        "SELECT origin, person_public_id, client_kind FROM remotes WHERE id = ?1",
        params![rid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if kind.as_deref() == Some(super::remotes::CLIENT_KIND_AGENT) {
        return Ok(None);
    }
    let id = match (&author.public_id, author.kind.as_str()) {
        (Some(p), _) => p.as_str(),
        (None, "agent") => clax_core::visibility::OWNER,
        (None, _) => clax_core::visibility::UNCONNECTED,
    };
    if me.as_deref() == Some(id) {
        return Ok(None);
    }
    Ok(Some(clax_core::visibility::remote_author(&origin, id)))
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

    /// How many public copies on remote `rid` local artifact `aid` has had
    /// a version pushed to, linked or not: an unlink leaves their pushes
    /// recorded.
    pub fn remote_copies(&self, rid: &str, aid: &str) -> Result<u32> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(DISTINCT remote_artifact_id) FROM remote_versions
                 WHERE remote_id = ?1 AND artifact_id = ?2",
                params![rid, aid],
                |r| r.get(0),
            )?)
        })
    }

    /// The local version of `aid` a push of it made public version
    /// `remote_n` of `raid` from, on any remote, connected now or removed
    /// since: what a link made again after a new sign-in bases on.
    pub fn pushed_twin(&self, aid: &str, raid: &str, remote_n: u32) -> Result<Option<u32>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT local_n FROM remote_versions
                 WHERE artifact_id = ?1 AND remote_artifact_id = ?2 AND remote_n = ?3
                 ORDER BY local_n DESC LIMIT 1",
                params![aid, raid, remote_n],
                |r| r.get(0),
            )
            .optional()?)
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

    /// Every link on remote `rid` the sync keeps ([`RemoteLink::synced`]:
    /// not a followed one no local session watches), oldest first.
    pub fn links_of_remote(&self, rid: &str) -> Result<Vec<RemoteLink>> {
        Ok(self
            .all_remote_links()?
            .into_iter()
            .filter(|l| l.remote_id == rid && l.synced())
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

    /// The local twins of those of comments `rcids` of remote `rid` that
    /// have one.
    pub fn mirrored_comments(&self, rid: &str, rcids: &[String]) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT comment_id FROM linked_comments WHERE remote_id = ?1 AND remote_comment_id = ?2",
            )?;
            let mut out = Vec::new();
            for rcid in rcids {
                if let Some(cid) = stmt
                    .query_row(params![rid, rcid], |r| r.get::<_, String>(0))
                    .optional()?
                {
                    out.push(cid);
                }
            }
            Ok(out)
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
    /// `NotFound` for an artifact gone, or `link` ended or moved since it
    /// was read; `invalid_comment` for an empty body.
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
            if !super::threads::artifact_live(tx, &link.artifact_id)? || !link_holds_in(tx, link)? {
                return Err(CoreError::NotFound);
            }
            let now = self.now();
            let tid = new_ulid();
            tx.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, sent_to_agent, has_clip, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'open', 0, 0, ?5)",
                params![tid, link.artifact_id, version_n, anchor_json, now],
            )?;
            // A comment on a seeded quickstart stops Clax's updates to it for
            // good, even if the thread is deleted later.
            tx.execute(
                "UPDATE quickstarts SET stopped_at = ?2 WHERE artifact_id = ?1 AND stopped_at IS NULL",
                params![link.artifact_id, now],
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

    /// Inserts mirrored comment `c` on thread `tid` of `aid` with its link,
    /// keyed by its author there ([`mirrored_author_key`]). The first
    /// comment here by someone else there changes what the owner's agents
    /// may see (their fast path, [`Store::owner_sees_all`], ends).
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
        let key = mirrored_author_key(tx, rid, &c.author)?;
        if key.is_some() {
            let first: bool = tx.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM linked_comments WHERE author_key IS NOT NULL)",
                [],
                |r| r.get(0),
            )?;
            if first {
                self.visibility_changed();
            }
        }
        tx.execute(
            "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, 0, ?5, ?6)",
            params![cid, tid, kind, c.author.name, c.body, now],
        )?;
        tx.execute(
            "INSERT INTO linked_comments (comment_id, thread_id, remote_id, remote_comment_id, idem_key,
                origin, author_kind, author_public_id, author_name, created_at, author_key)
             VALUES (?1, ?2, ?3, ?4, ?5, 'remote', ?6, ?7, ?8, ?9, ?10)",
            params![
                cid,
                tid,
                rid,
                c.remote_comment_id,
                format!("clax-linked:down:{rid}:{}", c.remote_comment_id),
                c.author.kind,
                c.author.public_id,
                c.author.name,
                now,
                key
            ],
        )?;
        // A followed remote artifact's mirror is in no inbox: its page is
        // the remote's.
        if kind == super::threads::AUTHOR_AGENT && !super::remotes::is_mirror_in(tx, aid)? {
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
            remote_author: key,
            // A remote's comment came from no session here: agents read
            // its author through `remote_author_labels`.
            agent: None,
        })
    }

    /// How a mirrored author keyed `key` ([`clax_core::visibility::remote_author`])
    /// is named: `<their name there> (on <remote>)` as their newest
    /// mirrored comment names them; `None` for a key no comment here has.
    pub fn remote_author_name(&self, key: &str) -> Result<Option<String>> {
        self.with_read(|c| remote_author_name_in(c, key))
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

    /// Writes status `status` (`open` or `resolved`) that thread `tid`'s
    /// twin has on the remote here, and records that both sides agree on
    /// it ([`Store::set_synced_status`]), in one transaction: a
    /// [`Store::scan_linked`] never sees the change without its synced
    /// status, so it never queues it back up. A resolve is by
    /// `viewer:anonymous` ([`Store::resolve_thread_touched`]), a reopen as
    /// [`Store::reopen_thread`] does it. Answers the thread as it is now,
    /// what the resolve changed, and whether its status changed here.
    ///
    /// # Errors
    /// `NotFound` when the thread is gone.
    pub fn mirror_status(
        &self,
        ctx: &AuditCtx,
        tid: &str,
        status: &str,
    ) -> Result<(Thread, clax_core::feedback::Touched, bool)> {
        let (touched, flipped) = self.with_tx(|tx| {
            let t = thread_in(tx, tid)?.ok_or(CoreError::NotFound)?;
            let mut touched = clax_core::feedback::Touched::default();
            let flipped = if status == "resolved" && t.status != "resolved" {
                touched =
                    self.resolve_thread_in(tx, ctx, tid, "viewer:anonymous", None, |_, _| {
                        Ok(None)
                    })?;
                true
            } else if status == "open" && t.status == "resolved" {
                self.reopen_thread_in(tx, ctx, tid)?;
                true
            } else {
                false
            };
            tx.execute(
                "UPDATE linked_threads SET synced_status = ?2 WHERE thread_id = ?1",
                params![tid, status],
            )?;
            Ok((touched, flipped))
        })?;
        let t = self.get_thread(tid)?.ok_or(CoreError::NotFound)?;
        Ok((t, touched, flipped))
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
                // Read before this transaction: a link ended (or moved)
                // since queues nothing.
                if !link_holds_in(tx, l)? {
                    continue;
                }
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
                       AND ((t.status = 'open'
                             AND COALESCE(t.reopened_by_person, '*owner') IN
                                 ('*owner', (SELECT public_id FROM viewers WHERE owner = 1)))
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

    /// Remote `rid`'s unacknowledged writes whose link has not ended,
    /// oldest first: those of a thread whose pairing an unlink or the
    /// remote's removal ended are [`Store::unsent_of`]'s, even once the
    /// artifact is linked to the remote again.
    pub fn outbox_of(&self, rid: &str) -> Result<Vec<OutboxRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {OUT_COLS} FROM linked_outbox o WHERE o.remote_id = ?1 AND NOT {ENDED}
                 ORDER BY o.seq"
            ))?;
            Ok(stmt
                .query_map(params![rid], row_out)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The writes on artifact `aid` not sent before its link ended
    /// (unlinked, or the remote removed), oldest first, whether or not it
    /// is linked to that remote again: what the owner confirms dropping.
    /// They are never sent.
    pub fn unsent_of(&self, aid: &str) -> Result<Vec<OutboxRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {OUT_COLS} FROM linked_outbox o WHERE o.artifact_id = ?1 AND {ENDED}
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
                &format!("DELETE FROM linked_outbox AS o WHERE o.artifact_id = ?1 AND {ENDED}"),
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
    /// it: for a `thread` (local thread `tid` of `made`), its twin `rtid`
    /// there, on public version `remote_version`; for a `thread` or
    /// `comment`, the comment `rcid` it made there.
    ///
    /// A `thread` whose row is gone (the thread was deleted here while its
    /// answer was out) still records its twin on the ended pairing, so the
    /// next read does not mirror the twin here as a new thread.
    pub fn ack_outbox(
        &self,
        seq: i64,
        made: Option<(&str, &str)>,
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
                if let Some((tid, rtid)) = made {
                    tx.execute(
                        "UPDATE linked_threads SET remote_thread_id = ?2,
                            remote_version = COALESCE(?3, remote_version)
                         WHERE thread_id = ?1 AND remote_thread_id IS NULL",
                        params![tid, rtid, remote_version],
                    )?;
                }
                return Ok(());
            };
            if let Some((_, rtid)) = made {
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

    /// Pairs, for each two-way link of remote `rid`, each open `ask`
    /// question of the owner's agents about its artifact, asked since it
    /// was linked (or last made two-way again) and not paired yet, with a
    /// copy to make there under a new idempotency key. A download-only
    /// link's questions stay local, as its new threads do; another local
    /// person's agents' questions never go up. An in-terminal question is
    /// never copied: it takes no answer in Clax, there or here. A thread
    /// question is copied as any other, about the remote artifact; its
    /// `thread_id` is this daemon's and is not sent. How many it paired.
    pub fn scan_linked_questions(&self, rid: &str) -> Result<usize> {
        let links = self.links_of_remote(rid)?;
        let (rname, rorigin): (String, String) = match self.find_remote(rid)? {
            Some(r) => (r.name, r.origin),
            None => return Ok(0),
        };
        self.with_tx(|tx| {
            let now = self.now();
            let mut paired = 0;
            for l in links.iter().filter(|l| !l.download_only) {
                if !link_holds_in(tx, l)? {
                    continue;
                }
                let mut stmt = tx.prepare_cached(
                    "SELECT q.id FROM questions q
                     WHERE q.artifact_id = ?1 AND q.status = 'open' AND q.source = 'ask'
                       AND q.in_terminal = 0 AND q.person IS NULL
                       AND q.created_at >= COALESCE((SELECT threads_since FROM remote_links
                             WHERE artifact_id = ?1 AND remote_id = ?3), ?2)
                       AND NOT EXISTS (SELECT 1 FROM linked_questions lq WHERE lq.question_id = q.id)
                     ORDER BY q.created_at, q.id",
                )?;
                let fresh: Vec<String> = stmt
                    .query_map(params![l.artifact_id, l.created_at, rid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                for qid in fresh {
                    tx.execute(
                        "INSERT INTO linked_questions (question_id, remote_id, artifact_id,
                            remote_artifact_id, idem_key, remote_name, remote_origin, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            qid,
                            rid,
                            l.artifact_id,
                            l.remote_artifact_id,
                            new_key("question"),
                            rname,
                            rorigin,
                            now
                        ],
                    )?;
                    paired += 1;
                }
            }
            Ok(paired)
        })
    }

    /// Remote `rid`'s question pairings not done yet, oldest first, with
    /// each local question's status.
    pub fn live_linked_questions(&self, rid: &str) -> Result<Vec<LinkedQuestion>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {LQ_COLS} FROM linked_questions lq JOIN questions q ON q.id = lq.question_id
                 WHERE lq.remote_id = ?1 AND lq.done_at IS NULL ORDER BY lq.created_at, lq.question_id"
            ))?;
            Ok(stmt
                .query_map(params![rid], row_lq)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Question `qid`'s pairing, if it has one.
    pub fn linked_question(&self, qid: &str) -> Result<Option<LinkedQuestion>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {LQ_COLS} FROM linked_questions lq JOIN questions q ON q.id = lq.question_id
                     WHERE lq.question_id = ?1"
                ),
                params![qid],
                row_lq,
            )
            .optional()?)
        })
    }

    /// Records that question `qid`'s copy is about to be asked there: until
    /// an answer is read, the remote may have made it.
    pub fn linked_question_trying(&self, qid: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET attempts = attempts + 1 WHERE question_id = ?1",
            params![qid],
        )
    }

    /// Records why a try at question `qid`'s copy got no answer.
    pub fn linked_question_attempted(&self, qid: &str, error: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET last_error = ?2 WHERE question_id = ?1",
            params![qid, error],
        )
    }

    /// Records that the remote made question `qid`'s copy as `rqid`.
    pub fn linked_question_made(&self, qid: &str, rqid: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET remote_question_id = ?2, attempts = 0, last_error = NULL,
                failed_at = NULL WHERE question_id = ?1",
            params![qid, rqid],
        )
    }

    /// Marks question `qid`'s copy refused by the remote with `error`: it
    /// made nothing, and the copy waits for Retry.
    pub fn linked_question_failed(&self, qid: &str, error: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET last_error = ?2, attempts = 0,
                failed_at = COALESCE(failed_at, ?3) WHERE question_id = ?1",
            params![qid, error, self.now()],
        )
    }

    /// Marks every copy of remote `rid` not asked there yet not sent, with
    /// `error`; how many.
    pub fn linked_questions_failed_all(&self, rid: &str, error: &str) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE linked_questions SET last_error = ?2, failed_at = ?3
                 WHERE remote_id = ?1 AND done_at IS NULL AND remote_question_id IS NULL
                   AND failed_at IS NULL",
                params![rid, error, self.now()],
            )?)
        })
    }

    /// Clears the not-sent mark of remote `rid`'s copies (on artifact `aid`
    /// only, when given), so they are tried again; how many.
    pub fn linked_questions_retry(&self, rid: &str, aid: Option<&str>) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE linked_questions SET failed_at = NULL
                 WHERE remote_id = ?1 AND (?2 IS NULL OR artifact_id = ?2) AND failed_at IS NOT NULL",
                params![rid, aid],
            )?)
        })
    }

    /// Records that question `qid` and its copy are both closed
    /// (`answered_there`: the remote's answer or decline closed it here).
    pub fn linked_question_done(&self, qid: &str, answered_there: bool) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET done_at = ?3, answered_there = ?2, failed_at = NULL,
                last_error = NULL
             WHERE question_id = ?1 AND done_at IS NULL",
            params![qid, answered_there, self.now()],
        )
    }

    /// Marks question `qid`'s copy done with the remote's answer refused
    /// here (`why`: it does not fit the questions): the question stays open
    /// here and its view's `linked.error` says why.
    pub fn linked_question_refused(&self, qid: &str, why: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET done_at = COALESCE(done_at, ?3), answered_there = 0,
                failed_at = NULL, last_error = ?2
             WHERE question_id = ?1",
            params![qid, format!("Not answered: {why}"), self.now()],
        )
    }

    /// Asks question `qid` there again under a new key: its copy was
    /// withdrawn there (its session there ended) while it is open here.
    pub fn linked_question_reask(&self, qid: &str) -> Result<()> {
        self.lq_update(
            "UPDATE linked_questions SET remote_question_id = NULL, idem_key = ?2, attempts = 0,
                last_error = NULL, failed_at = NULL WHERE question_id = ?1",
            params![qid, new_key("question")],
        )
    }

    fn lq_update(&self, sql: &str, p: impl rusqlite::Params) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(sql, p)?;
            Ok(())
        })
    }

    /// What question `qid`'s view says of its copy on a remote
    /// (`linked`), when it has one.
    pub fn linked_question_view(&self, qid: &str) -> Result<Option<LinkedQuestionView>> {
        self.with_read(|c| {
            let row: Option<LinkedQuestionView> = c
                .query_row(
                    "SELECT lq.remote_id, lq.remote_name, lq.remote_origin, lq.remote_artifact_id,
                        lq.remote_question_id, lq.failed_at, lq.last_error, lq.answered_there,
                        lq.done_at,
                        NOT EXISTS (SELECT 1 FROM remote_links l WHERE l.artifact_id = lq.artifact_id
                                      AND l.remote_id = lq.remote_id)
                     FROM linked_questions lq WHERE lq.question_id = ?1",
                    params![qid],
                    |r| {
                        let origin: String = r.get(2)?;
                        let raid: String = r.get(3)?;
                        let rqid: Option<String> = r.get(4)?;
                        let failed: Option<String> = r.get(5)?;
                        let done: Option<String> = r.get(8)?;
                        let unlinked: bool = r.get(9)?;
                        let state = match (&rqid, &failed, &done) {
                            (_, _, Some(_)) => None,
                            (_, Some(_), None) => Some("failed"),
                            (None, None, None) if !unlinked => Some("pending"),
                            _ => None,
                        };
                        Ok(LinkedQuestionView {
                            remote_id: r.get(0)?,
                            remote: r.get(1)?,
                            url: format!("{origin}/a/{raid}"),
                            remote_origin: origin,
                            sent: rqid.is_some(),
                            state,
                            // Once done, an error is why the remote's answer
                            // was refused here ([`Store::linked_question_refused`]).
                            error: if state.is_some() || done.is_some() {
                                r.get(6)?
                            } else {
                                None
                            },
                            answered_there: r.get(7)?,
                            unlinked,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// On a hosted daemon: the thread and comment that agent token
    /// `grant_id`'s comment or new thread keyed `key` made, if one did.
    pub fn comment_key(&self, grant_id: &str, key: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| comment_key_in(c, (grant_id, key)))
    }

    /// Whether comment `cid` was sent to any agent (has a feedback row):
    /// what a replayed keyed comment checks before sending it again.
    pub fn comment_has_feedback(&self, cid: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM feedback WHERE comment_id = ?1)",
                params![cid],
                |r| r.get(0),
            )?)
        })
    }
}

/// An agent token's idempotency key for a comment or new thread on a
/// hosted daemon: its grant ID and the key.
pub type CommentKey<'a> = (&'a str, &'a str);

/// What a keyed write did: made `T` now, or found the thread and comment
/// an earlier write under the same key made, and wrote nothing.
#[derive(Debug)]
pub enum Keyed<T> {
    Made(T),
    Before {
        thread_id: String,
        comment_id: String,
    },
}

impl<T> Keyed<T> {
    /// What an unkeyed write made (it has no earlier write to find).
    pub(crate) fn made(self) -> T {
        match self {
            Keyed::Made(t) => t,
            Keyed::Before { .. } => unreachable!("only a keyed write finds an earlier one"),
        }
    }
}

/// The thread and comment that write keyed `key` made, if one did.
pub(crate) fn comment_key_in(
    c: &Connection,
    key: CommentKey<'_>,
) -> Result<Option<(String, String)>> {
    Ok(c.query_row(
        "SELECT thread_id, comment_id FROM comment_keys WHERE grant_id = ?1 AND key = ?2",
        params![key.0, key.1],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

/// Records, in the transaction that made them, that the write keyed `key`
/// made comment `cid` on thread `tid` (checked absent first with
/// [`comment_key_in`] in the same transaction).
pub(crate) fn record_comment_key_in(
    tx: &Connection,
    key: CommentKey<'_>,
    tid: &str,
    cid: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO comment_keys (grant_id, key, thread_id, comment_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![key.0, key.1, tid, cid, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remotes::{PendingSignIn, SignedInAs};
    use crate::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread};
    use clax_core::Home;
    use clax_core::working::ManualClock;
    use clax_core::{ArtifactId, Secret};
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
        let p = clax_core::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
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
    fn a_mirrored_thread_on_a_seeded_quickstart_stops_updates_for_good() {
        let w = world();
        let l = link(&w, (1, 1));
        w.st.with_tx(|tx| {
            tx.execute(
                "INSERT INTO quickstarts (person, artifact_id, release, version_n, seeded_at)
                 VALUES ('*owner', ?1, 'a', 1, '2026-10-15T10:00:00Z')",
                [&w.aid],
            )?;
            Ok(())
        })
        .unwrap();
        let stopped = || w.st.quickstart_of(None).unwrap().unwrap().stopped_at;
        assert!(stopped().is_none());
        let mt = MirrorThread {
            remote_thread_id: "rt1".into(),
            remote_version: 1,
            anchor: whole(),
            first: mirror("make it bluer", "rc1"),
        };
        let remote = ("clax.example", "https://clax.example");
        let t =
            w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt)
                .unwrap();
        assert!(stopped().is_some(), "the mirrored thread stopped updates");
        w.st.delete_thread(&AuditCtx::DAEMON, &t.id).unwrap();
        assert!(stopped().is_some(), "deleting it leaves updates stopped");
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
    fn a_status_mirrored_from_the_remote_is_never_queued_back_up() {
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
            w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt)
                .unwrap();
        let synced = || w.st.linked_thread(&t.id).unwrap().unwrap().synced_status;
        let (r, _, flipped) =
            w.st.mirror_status(&AuditCtx::DAEMON, &t.id, "resolved")
                .unwrap();
        assert_eq!((r.status.as_str(), flipped), ("resolved", true));
        assert_eq!(r.resolved_by.as_deref(), Some("viewer:anonymous"));
        assert_eq!(synced(), "resolved");
        // The status and its synced status are one write: a scan right
        // after it sees them agree, so neither change goes back up.
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0);
        let (r, _, flipped) =
            w.st.mirror_status(&AuditCtx::DAEMON, &t.id, "open")
                .unwrap();
        assert_eq!((r.status.as_str(), flipped), ("open", true));
        assert_eq!(synced(), "open");
        assert_eq!(
            w.st.scan_linked(&w.rid).unwrap(),
            0,
            "the reopen is not echoed"
        );
        let (_, _, flipped) =
            w.st.mirror_status(&AuditCtx::DAEMON, &t.id, "open")
                .unwrap();
        assert!(!flipped, "already open");
    }

    #[test]
    fn only_the_owners_reopen_goes_up() {
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
                    first: mirror("make it bluer", "rc1"),
                },
            )
            .unwrap();
        w.st.mirror_status(&AuditCtx::DAEMON, &t.id, "resolved")
            .unwrap();
        // Another local person's comment reopens it here: not theirs to
        // reopen there as the owner's person.
        w.clock.advance(1);
        w.st.add_comment(
            &AuditCtx::DAEMON,
            &t.id,
            NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: "Sam".into(),
                author_public_id: Some("pub_local_sam".into()),
                via_session_id: None,
                body: "still too dark".into(),
                via_page: false,
            },
        )
        .unwrap();
        assert_eq!(w.st.get_thread(&t.id).unwrap().unwrap().status, "open");
        assert_eq!(
            w.st.scan_linked(&w.rid).unwrap(),
            0,
            "Sam's reopen stays here"
        );
        // Resolved again there, then reopened here by the owner: it goes up.
        w.st.mirror_status(&AuditCtx::DAEMON, &t.id, "resolved")
            .unwrap();
        let mut owner = AuditCtx::DAEMON;
        owner.actor = clax_core::audit::Actor::Owner {
            public_id: w.owner.clone(),
        };
        w.st.reopen_thread(&owner, &t.id).unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        let out = w.st.outbox_of(&w.rid).unwrap();
        assert_eq!(out.last().unwrap().kind, "reopen");
    }

    #[test]
    fn a_mirrored_comment_is_keyed_unless_an_agent_token_read_it() {
        use crate::remotes::{CLIENT_KIND_AGENT, CLIENT_KIND_APP};
        for (kind, keyed) in [
            (None, true),
            (Some(CLIENT_KIND_APP), true),
            (Some(CLIENT_KIND_AGENT), false),
        ] {
            let w = world();
            if let Some(k) = kind {
                w.st.set_remote_client_kind(&w.rid, k).unwrap();
            }
            let l = link(&w, (1, 1));
            let mt = MirrorThread {
                remote_thread_id: "rt1".into(),
                remote_version: 1,
                anchor: whole(),
                first: mirror("make it bluer", "rc1"),
            };
            let remote = ("clax.example", "https://clax.example");
            let t =
                w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt)
                    .unwrap();
            let c = &w.st.get_thread(&t.id).unwrap().unwrap().comments[0];
            assert_eq!(
                c.remote_author.as_deref(),
                keyed.then_some("remote:https://clax.example#pub_sam"),
                "{kind:?}"
            );
            // The owner's agents read everything only while nothing is keyed.
            assert_eq!(w.st.owner_sees_all().unwrap(), !keyed, "{kind:?}");
        }
    }

    #[test]
    fn a_link_moved_to_another_public_copy_ends_the_old_copys_threads_and_read() {
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
            w.st.mirror_thread(&AuditCtx::DAEMON, &l, remote, 1, true, mt)
                .unwrap();
        w.st.set_sync_cursor(&w.aid, &w.rid, "2026-10-15T10:00:00.000Z")
            .unwrap();
        w.st.set_link_download_only(&w.aid, &w.rid, true).unwrap();
        // Linked again to the same copy: nothing ends.
        w.st.link_remote(&w.aid, &w.rid, "pub1", "https://clax.example/a/pub1", 1, 1)
            .unwrap();
        assert!(
            w.st.linked_thread(&t.id)
                .unwrap()
                .unwrap()
                .unlinked_at
                .is_none()
        );
        let same = w.st.remote_link(&w.aid, &w.rid).unwrap().unwrap();
        assert_eq!(
            same.sync_cursor.as_deref(),
            Some("2026-10-15T10:00:00.000Z")
        );
        assert!(same.download_only);
        // Moved to another copy: the old copy's thread is history, and the
        // new copy is read from the start, two-way.
        w.clock.advance(1);
        let moved =
            w.st.link_remote(&w.aid, &w.rid, "pub2", "https://clax.example/a/pub2", 1, 3)
                .unwrap();
        assert_eq!(moved.remote_artifact_id, "pub2");
        assert_eq!(moved.sync_cursor, None);
        assert!(!moved.download_only);
        assert!(
            w.st.linked_thread(&t.id)
                .unwrap()
                .unwrap()
                .unlinked_at
                .is_some()
        );
    }

    #[test]
    fn a_link_ended_after_it_was_read_mirrors_nothing() {
        let w = world();
        let l = link(&w, (1, 1));
        w.st.unlink_remote(&w.aid, &w.rid).unwrap();
        let err =
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
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound), "{err:?}");
        assert!(
            w.st.linked_thread_by_remote(&w.rid, "rt1")
                .unwrap()
                .is_none()
        );
        let id = ArtifactId::parse(&w.aid).unwrap();
        assert!(w.st.list_threads(&id, true, None, 10).unwrap().0.is_empty());
    }

    #[test]
    fn deleting_a_linked_thread_ends_its_pairing_and_drops_its_unsent_writes() {
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
        w.st.add_comment(
            &AuditCtx::DAEMON,
            &t.id,
            NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: "Owner".into(),
                author_public_id: Some(w.owner.clone()),
                via_session_id: None,
                body: "unsent".into(),
                via_page: false,
            },
        )
        .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        w.st.delete_thread(&AuditCtx::DAEMON, &t.id).unwrap();
        let lt =
            w.st.linked_thread_by_remote(&w.rid, "rt1")
                .unwrap()
                .unwrap();
        assert_eq!(lt.thread_id, t.id);
        assert!(
            lt.unlinked_at.is_some(),
            "ended, so it is not written again"
        );
        assert!(w.st.outbox_of(&w.rid).unwrap().is_empty());
        assert!(!w.st.has_linked_comment(&w.rid, "rc1").unwrap());
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 0);
    }

    #[test]
    fn writes_left_unsent_by_an_unlink_wait_for_the_owner_after_a_new_link() {
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
        w.st.add_comment(
            &AuditCtx::DAEMON,
            &t.id,
            NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: "Owner".into(),
                author_public_id: Some(w.owner.clone()),
                via_session_id: None,
                body: "unsent".into(),
                via_page: false,
            },
        )
        .unwrap();
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        w.st.unlink_remote(&w.aid, &w.rid).unwrap();
        assert_eq!(w.st.unsent_of(&w.aid).unwrap().len(), 1);
        // Pushed to the same remote again: the old write is still the
        // owner's to retry or drop, never sent on its own.
        link(&w, (1, 2));
        assert!(w.st.outbox_of(&w.rid).unwrap().is_empty(), "not drained");
        assert_eq!(w.st.unsent_of(&w.aid).unwrap().len(), 1);
        assert_eq!(w.st.drop_unsent(&w.aid).unwrap(), 1);
        assert!(w.st.unsent_of(&w.aid).unwrap().is_empty());
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
    fn a_thread_deleted_while_its_up_send_was_out_keeps_its_twin_there() {
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
        assert_eq!(w.st.scan_linked(&w.rid).unwrap(), 1);
        let seq = w.st.outbox_of(&w.rid).unwrap()[0].seq;
        // Deleted here while the up-send is out; then its answer arrives.
        w.st.delete_thread(&AuditCtx::DAEMON, &t.id).unwrap();
        w.st.ack_outbox(seq, Some((&t.id, "rt5")), Some("rc5"), Some(1))
            .unwrap();
        let lt =
            w.st.linked_thread_by_remote(&w.rid, "rt5")
                .unwrap()
                .expect("the ended pairing names its twin there");
        assert_eq!(lt.thread_id, t.id);
        assert!(lt.unlinked_at.is_some(), "so the read writes nothing here");
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
        w.st.ack_outbox(
            out.last().unwrap().seq,
            Some((&later.id, "rt9")),
            Some("rc9"),
            Some(1),
        )
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
    fn a_keyed_thread_or_comment_is_made_once_with_its_key() {
        let w = world();
        let id = ArtifactId::parse(&w.aid).unwrap();
        let new_thread = |version_n| NewThread {
            author_public_id: None,
            version_n,
            anchor: whole(),
            author_name: "Ada".into(),
            body: "hi".into(),
            clip: None,
            via_page: false,
        };
        let key = Some(("g1", "k"));
        // A write refused records no key: a resend may still make it.
        assert!(
            w.st.block_on(w.st.create_thread_keyed_async(
                &AuditCtx::DAEMON,
                &id,
                new_thread(9),
                &[],
                key
            ))
            .is_err()
        );
        assert!(w.st.comment_key("g1", "k").unwrap().is_none());
        let Keyed::Made(t) =
            w.st.block_on(w.st.create_thread_keyed_async(
                &AuditCtx::DAEMON,
                &id,
                new_thread(1),
                &[],
                key,
            ))
            .unwrap()
        else {
            panic!("made")
        };
        let first = t.comments[0].id.clone();
        assert_eq!(
            w.st.comment_key("g1", "k").unwrap(),
            Some((t.id.clone(), first.clone())),
            "recorded with the thread"
        );
        // A second write under the key, however late its own check ran,
        // finds the first and writes nothing.
        match w
            .st
            .block_on(w.st.create_thread_keyed_async(
                &AuditCtx::DAEMON,
                &id,
                new_thread(1),
                &[],
                key,
            ))
            .unwrap()
        {
            Keyed::Before {
                thread_id,
                comment_id,
            } => assert_eq!((thread_id, comment_id), (t.id.clone(), first)),
            Keyed::Made(_) => panic!("made twice"),
        }
        assert_eq!(w.st.list_threads(&id, true, None, 10).unwrap().0.len(), 1);
        // A comment the same way.
        let reply = || NewComment {
            author_kind: AUTHOR_VIEWER,
            author_name: "Ada".into(),
            author_public_id: None,
            via_session_id: None,
            body: "again".into(),
            via_page: false,
        };
        let Keyed::Made(c) =
            w.st.add_comment_keyed(&AuditCtx::DAEMON, &t.id, reply(), Some(("g1", "c")))
                .unwrap()
        else {
            panic!("made")
        };
        assert!(matches!(
            w.st.add_comment_keyed(&AuditCtx::DAEMON, &t.id, reply(), Some(("g1", "c")))
                .unwrap(),
            Keyed::Before { comment_id, .. } if comment_id == c.id
        ));
        assert_eq!(w.st.get_thread(&t.id).unwrap().unwrap().comments.len(), 2);
        assert!(w.st.comment_key("g2", "k").unwrap().is_none());
    }

    #[test]
    fn only_the_owners_agents_open_asks_on_a_two_way_link_are_paired_once() {
        use crate::questions::{Close, NewQuestion, Source};
        let w = world();
        link(&w, (1, 1));
        let qs: Vec<clax_core::questions::Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which?", "header": "Pick", "options": [{"label": "A"}, {"label": "B"}]}
        ]))
        .unwrap();
        let session = |grant: Option<String>, h: &str| {
            w.st.register_session(
                &AuditCtx::DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some(h.into()),
                    cwd: "/w".into(),
                    grant_id: grant,
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        };
        let ask = |sid: &str, artifact: Option<&str>| {
            w.st.create_question(
                &AuditCtx::DAEMON,
                NewQuestion {
                    session_id: sid.into(),
                    artifact_id: artifact.map(Into::into),
                    source: Source::Ask,
                    tool_use_id: None,
                    questions: qs.clone(),
                    released: false,
                    thread_id: None,
                    in_terminal: false,
                },
            )
            .unwrap()
            .0
            .id
        };
        let owners = session(None, "h1");
        let (g, _, _) = crate::people::tests::approved(&w.st, "Sam", [7; 32]);
        let sams = session(Some(g), "h2");
        let mine = ask(&owners, Some(&w.aid));
        let _theirs = ask(&sams, Some(&w.aid));
        let _none = ask(&owners, None);
        let closed = ask(&owners, Some(&w.aid));
        w.st.close_question(&AuditCtx::DAEMON, &closed, Close::Withdraw)
            .unwrap();
        assert_eq!(w.st.scan_linked_questions(&w.rid).unwrap(), 1);
        assert_eq!(w.st.scan_linked_questions(&w.rid).unwrap(), 0, "once");
        let live = w.st.live_linked_questions(&w.rid).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(
            (
                live[0].question_id.as_str(),
                live[0].remote_artifact_id.as_str()
            ),
            (mine.as_str(), "pub1")
        );
        let key = live[0].idem_key.clone();
        let v = w.st.linked_question_view(&mine).unwrap().unwrap();
        assert_eq!(
            (v.state, v.sent, v.unlinked),
            (Some("pending"), false, false)
        );
        // Acknowledged, then withdrawn there while open here: asked again
        // under a new key.
        w.st.linked_question_made(&mine, "rq1").unwrap();
        assert_eq!(
            w.st.linked_question_view(&mine).unwrap().unwrap().state,
            None
        );
        w.st.linked_question_reask(&mine).unwrap();
        let again = w.st.linked_question(&mine).unwrap().unwrap();
        assert_eq!(again.remote_question_id, None);
        assert_ne!(again.idem_key, key);
        // Download only: a new question stays local.
        w.st.set_link_download_only(&w.aid, &w.rid, true).unwrap();
        let held = ask(&owners, Some(&w.aid));
        assert_eq!(w.st.scan_linked_questions(&w.rid).unwrap(), 0);
        assert!(w.st.linked_question(&held).unwrap().is_none());
        // A read while the remote was out leaves an error behind, which
        // the copy's end clears.
        w.st.linked_question_attempted(&mine, "cannot reach clax.example")
            .unwrap();
        w.st.linked_question_done(&mine, true).unwrap();
        assert!(w.st.live_linked_questions(&w.rid).unwrap().is_empty());
        let v = w.st.linked_question_view(&mine).unwrap().unwrap();
        assert!(v.answered_there);
        assert_eq!(v.error, None);
        // A refused answer there says why, once the copy is done.
        w.st.linked_question_refused(&mine, "no option named C")
            .unwrap();
        let v = w.st.linked_question_view(&mine).unwrap().unwrap();
        assert_eq!(
            (v.state, v.error.as_deref(), v.answered_there),
            (None, Some("Not answered: no option named C"), false)
        );
    }

    #[test]
    fn an_in_terminal_question_about_a_linked_artifact_is_not_copied() {
        use crate::questions::{NewQuestion, Source};
        let w = world();
        link(&w, (1, 1));
        let qs: Vec<clax_core::questions::Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which?", "header": "Pick", "options": [{"label": "A"}, {"label": "B"}]}
        ]))
        .unwrap();
        let sid =
            w.st.register_session(
                &AuditCtx::DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("h1".into()),
                    cwd: "/w".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        w.st.create_question(
            &AuditCtx::DAEMON,
            NewQuestion {
                session_id: sid,
                artifact_id: Some(w.aid.clone()),
                source: Source::Ask,
                tool_use_id: None,
                questions: qs,
                released: false,
                thread_id: None,
                in_terminal: true,
            },
        )
        .unwrap();
        assert_eq!(
            w.st.scan_linked_questions(&w.rid).unwrap(),
            0,
            "an in-terminal question takes no answer there"
        );
    }

    #[test]
    fn a_keyed_question_is_made_once_per_grant() {
        use crate::questions::{NewQuestion, Source};
        let w = world();
        let (g, _, _) = crate::people::tests::approved(&w.st, "Sam", [7; 32]);
        let mut sids = Vec::new();
        for h in ["h1", "h2"] {
            sids.push(
                w.st.register_session(
                    &AuditCtx::DAEMON,
                    crate::RegisterSession {
                        harness: "claude".into(),
                        harness_session_id: Some(h.into()),
                        cwd: "/w".into(),
                        grant_id: Some(g.clone()),
                        ..Default::default()
                    },
                )
                .unwrap()
                .id,
            );
        }
        let qs: Vec<clax_core::questions::Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which?", "header": "Pick"}
        ]))
        .unwrap();
        let make = |sid: &str, key: Option<(&str, &str)>| {
            w.st.create_question_keyed(
                &AuditCtx::DAEMON,
                NewQuestion {
                    session_id: sid.into(),
                    artifact_id: None,
                    source: Source::Ask,
                    tool_use_id: None,
                    questions: qs.clone(),
                    released: false,
                    thread_id: None,
                    in_terminal: false,
                },
                key,
            )
            .unwrap()
        };
        let (q1, made) = make(&sids[0], Some((&g, "k")));
        assert!(made);
        let (q2, made) = make(&sids[1], Some((&g, "k")));
        assert_eq!((q2.id.as_str(), made), (q1.id.as_str(), false));
        let (q3, made) = make(&sids[1], Some(("other-grant", "k")));
        assert!(made && q3.id != q1.id);
        // The grant's other session reaches the keyed question only.
        let unkeyed = make(&sids[0], None).0;
        assert!(
            w.st.session_or_keyed_question(&sids[1], Some(&g), &q1.id)
                .is_ok()
        );
        assert!(
            w.st.session_or_keyed_question(&sids[1], Some(&g), &unkeyed.id)
                .is_err()
        );
        assert!(
            w.st.session_or_keyed_question(&sids[1], None, &q1.id)
                .is_err()
        );
        assert!(
            w.st.session_or_keyed_question(&sids[1], Some("other-grant"), &q1.id)
                .is_err()
        );
    }
}
