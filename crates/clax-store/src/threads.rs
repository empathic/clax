//! Comment threads and their comments. A thread is anchored to one version of
//! a live artifact and starts with the viewer comment that created it; its
//! optional clip is stored at `artifacts/<aid>/clips/<tid>.png`.

use super::Store;
use super::batches::ThreadSend;
use super::feedback::{Delivery, deliveries_in, id_array};
use super::linked::{CommentKey, Keyed};
use clax_core::anchor::Anchor;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::model::{Comment, Thread};
use clax_core::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashMap;

/// Largest accepted clip, in bytes.
pub const MAX_CLIP_BYTES: usize = clax_core::clips::MAX_BYTES;
/// Longest accepted comment body, in characters.
pub const MAX_BODY_CHARS: usize = 10_000;
/// Threads per page when a caller gives no limit.
pub const DEFAULT_THREAD_PAGE: usize = 50;
pub use clax_core::model::{AUTHOR_AGENT, AUTHOR_VIEWER};
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A thread to create; its first comment is a viewer comment by `author_name`.
#[derive(Clone, Debug)]
pub struct NewThread {
    pub version_n: u32,
    pub anchor: Anchor,
    pub author_name: String,
    /// The authoring viewer's public ID, when a viewer cookie names one.
    pub author_public_id: Option<String>,
    pub body: String,
    pub clip: Option<Vec<u8>>,
    /// The page wrote the first comment through the `comments` capability.
    pub via_page: bool,
}

/// A comment to add. `author_kind` is [`AUTHOR_VIEWER`] or [`AUTHOR_AGENT`].
#[derive(Clone, Debug)]
pub struct NewComment {
    pub author_kind: &'static str,
    pub author_name: String,
    /// The authoring viewer's public ID (viewer comments only).
    pub author_public_id: Option<String>,
    pub via_session_id: Option<String>,
    pub body: String,
    /// The page wrote it through the `comments` capability (viewer comments only).
    pub via_page: bool,
}

/// Why `bytes` cannot be stored as a clip (not a PNG, or over [`MAX_CLIP_BYTES`]),
/// or `None` when it can.
pub fn clip_problem(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Some("the clip is not a PNG image".into());
    }
    if bytes.len() > MAX_CLIP_BYTES {
        return Some(format!("the clip exceeds {MAX_CLIP_BYTES} bytes"));
    }
    None
}

pub(super) fn check_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(CoreError::invalid(
            "invalid_comment",
            "a comment needs text",
        ));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(CoreError::invalid(
            "invalid_comment",
            format!("a comment is at most {MAX_BODY_CHARS} characters"),
        ));
    }
    Ok(())
}

/// The anchor as `thread.open` records it (spec 2026-10-06-toolpath-audit
/// §6.2): its kind, selector, text quote (`quote`, `prefix`, `suffix`),
/// `html_hash`, file and route; not its geometry or custom name.
pub(super) fn anchor_record(a: &Anchor) -> serde_json::Value {
    serde_json::json!({
        "kind": a.kind,
        "selector": a.selector,
        "quote": a.quote,
        "prefix": a.prefix,
        "suffix": a.suffix,
        "html_hash": a.html_hash,
        "file": a.file,
        "route": a.route,
    })
}

/// The `comment.add` record of comment `c` on a thread of artifact `aid`
/// (spec 2026-10-06-toolpath-audit §6.2). The body text is recorded as
/// written; export redacts it on request (§11).
pub(super) fn comment_record(aid: &str, c: &Comment) -> AuditRecord {
    let mut r = AuditRecord::new(AuditKind::CommentAdd, c.created_at.as_str())
        .with("comment_id", c.id.as_str())
        .with("body", c.body.as_str())
        .with("author_kind", c.author_kind.as_str())
        .with("author_name", c.author_name.as_str())
        .with("via_harness", c.via_harness.clone())
        .with("via_page", c.via_page);
    r.ids.artifact = Some(aid.to_string());
    r.ids.thread = Some(c.thread_id.clone());
    r
}

/// The `thread.open` record of thread `tid` on artifact `aid` (audit spec
/// §6.2), made with comment `first_comment_id`.
#[allow(clippy::too_many_arguments)]
pub(super) fn open_record(
    at: &str,
    aid: &str,
    tid: &str,
    version_n: u32,
    anchor: serde_json::Value,
    live_path: Option<String>,
    has_clip: bool,
    first_comment_id: Option<&str>,
) -> AuditRecord {
    thread_record(AuditKind::ThreadOpen, at, aid, tid)
        .with("version_n", version_n)
        .with("anchor", anchor)
        .with("live_path", live_path)
        .with("has_clip", has_clip)
        .with("first_comment_id", first_comment_id)
}

/// The `thread.resolve` record of thread `tid` (spec §6.2): who resolved
/// it, and the version the resolve linked it to.
pub(super) fn resolve_record(
    at: &str,
    aid: &str,
    tid: &str,
    resolved_by: Option<&str>,
    addressed_version: Option<u32>,
) -> AuditRecord {
    thread_record(AuditKind::ThreadResolve, at, aid, tid)
        .with("resolved_by", resolved_by)
        .with("addressed_version", addressed_version)
}

/// The `thread.addressed` record of a resolve's link of thread `tid` to
/// version `n` that no other event carries (spec §6.2).
pub(super) fn addressed_record(at: &str, aid: &str, tid: &str, n: u32) -> AuditRecord {
    thread_record(AuditKind::ThreadAddressed, at, aid, tid)
        .with("version_n", n)
        .with("source", clax_core::changelog::LinkSource::Resolve.as_str())
}

/// A record of `kind` about thread `tid` of artifact `aid`, at `at`.
pub(crate) fn thread_record(kind: AuditKind, at: &str, aid: &str, tid: &str) -> AuditRecord {
    let mut r = AuditRecord::new(kind, at);
    r.ids.artifact = Some(aid.to_string());
    r.ids.thread = Some(tid.to_string());
    r
}

/// A sent thread's owner agent (spec 2026-10-08 §12, M23): the session
/// in its `target_session_id`, set by a send naming an agent, `@name`, a
/// handoff, or the first session to take one of its rows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ThreadOwner {
    /// Its session; never served (views name the handle).
    #[serde(skip)]
    pub session_id: String,
    pub handle: String,
    /// Its agent name (spec §11), when it has one.
    pub name: Option<String>,
    /// Its person's public ID; `None` for the owner's agents.
    pub person: Option<String>,
    /// Whether its session is live; an ended owner owns nothing.
    pub live: bool,
    #[serde(skip)]
    pub harness: String,
}

impl ThreadOwner {
    /// The name `taken_by` and `thread_taken` give: its agent name, else
    /// its harness.
    pub fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.harness)
    }
}

/// The owner session of each thread of `?1` (a JSON array).
const OWNERS_OF_MANY: &str = "SELECT t.id, s.id, s.agent_handle, s.name, s.person,
    s.ended_at IS NULL, s.harness FROM json_each(?1) j
    CROSS JOIN threads t ON t.id = j.value
    JOIN sessions s ON s.id = t.target_session_id";

fn row_to_owner(r: &Row<'_>, at: usize) -> rusqlite::Result<ThreadOwner> {
    Ok(ThreadOwner {
        session_id: r.get(at)?,
        handle: r.get(at + 1)?,
        name: r.get(at + 2)?,
        person: r.get(at + 3)?,
        live: r.get(at + 4)?,
        harness: r.get(at + 5)?,
    })
}

/// Thread `tid`'s owner, live or ended, when it has one.
pub(crate) fn owner_in(c: &Connection, tid: &str) -> Result<Option<ThreadOwner>> {
    Ok(c.query_row(
        "SELECT s.id, s.agent_handle, s.name, s.person, s.ended_at IS NULL, s.harness
         FROM threads t JOIN sessions s ON s.id = t.target_session_id WHERE t.id = ?1",
        params![tid],
        |r| row_to_owner(r, 0),
    )
    .optional()?)
}

/// `Ok` when session `sid` may reply to or resolve thread `tid`: the
/// thread has no owner, an ended owner, or `sid` as its owner (M24).
///
/// # Errors
/// [`CoreError::ThreadTaken`] naming the owner when another live session
/// owns it.
pub(crate) fn check_not_taken_in(c: &Connection, tid: &str, sid: &str) -> Result<()> {
    match owner_in(c, tid)? {
        Some(o) if o.live && o.session_id != sid => Err(CoreError::ThreadTaken {
            name: o.display().to_string(),
        }),
        _ => Ok(()),
    }
}

/// The person of session `sid` (`None`: the owner's agents), or `None`
/// for no such session.
pub(crate) fn session_person_in(c: &Connection, sid: &str) -> Result<Option<Option<String>>> {
    Ok(c.query_row(
        "SELECT person FROM sessions WHERE id = ?1",
        params![sid],
        |r| r.get(0),
    )
    .optional()?)
}

/// The person who sent viewer comment `comment_id` to the agents (spec
/// 2026-10-08 O8 as ruled for §12): its writer when the writer is a
/// connected person, else the owner (`None`): the owner, LAN viewers who
/// are not connected and anonymous viewers reach the owner's agents (O6).
pub(crate) fn sender_person_in(c: &Connection, comment_id: &str) -> Result<Option<String>> {
    let p: Option<String> = c
        .query_row(
            "SELECT v.public_id FROM comments k JOIN viewers v ON v.public_id = k.author_public_id
             WHERE k.id = ?1 AND k.author_kind = 'viewer' AND v.person = 1 AND v.owner = 0",
            params![comment_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(p)
}

/// Whether session `sid` may take thread `tid` implicitly (by a first
/// delivery or a reply), or hand off or release it while it has no owner:
/// one of `comment_ids`, the viewer comments of the thread the session
/// received or sees, was sent by its person. O8 settles a first take among
/// one person's agents; another person's agent becomes an owner only by a
/// send naming it or a handoff.
pub(crate) fn may_take_in(c: &Connection, sid: &str, comment_ids: &[String]) -> Result<bool> {
    let Some(person) = session_person_in(c, sid)? else {
        return Ok(false);
    };
    for id in comment_ids {
        if sender_person_in(c, id)? == person {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Thread `tid`'s owner, when session `from` may hand the thread off or
/// release it (spec 2026-10-08 §12): `from` owns it; or it has no live
/// owner and either its ended owner was an agent of `from`'s person or
/// `from` may take it implicitly ([`may_take_in`] over `seen`, the viewer
/// comments of the thread `from` sees).
///
/// # Errors
/// [`CoreError::ThreadTaken`] naming the owner (or nobody) otherwise.
pub(crate) fn check_may_hand_off_in(
    c: &Connection,
    tid: &str,
    from: &str,
    seen: &[String],
) -> Result<Option<ThreadOwner>> {
    let owner = owner_in(c, tid)?;
    let allowed = match &owner {
        Some(o) if o.live => o.session_id == from,
        Some(o) => {
            session_person_in(c, from)?.is_some_and(|p| p == o.person)
                || may_take_in(c, from, seen)?
        }
        None => may_take_in(c, from, seen)?,
    };
    if !allowed {
        return Err(CoreError::ThreadTaken {
            name: owner
                .as_ref()
                .map_or("no agent of yours", |o| o.display())
                .to_string(),
        });
    }
    Ok(owner)
}

/// Makes session `sid` the owner of thread `tid` by an implicit take
/// (`cause` `first_delivery` or `reply`), and answers whether it did: only
/// when the thread has no live owner and [`may_take_in`] lets `sid` take it
/// over `comment_ids` (the comments it was handed, or sees, as it
/// replies). The undelivered rows of the thread held by the other agents of
/// `sid`'s person are deleted, and their sessions' open `ask` requests for
/// the thread withdrawn (`approval.withdraw`, reason `thread_taken`);
/// other people's agents' rows stay. Those sessions are added to
/// `touched.targets`, so their waits learn the rows are gone. Untargeted
/// undelivered rows become `sid`'s (or are deleted, for a comment `sid`
/// has a row for already). Recorded as `thread.take {thread_id, session,
/// cause}` under `ctx`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take_in(
    st: &Store,
    tx: &rusqlite::Transaction<'_>,
    ctx: &AuditCtx,
    tid: &str,
    sid: &str,
    cause: &str,
    comment_ids: &[String],
    now: &str,
    touched: &mut clax_core::feedback::Touched,
) -> Result<bool> {
    if owner_in(tx, tid)?.is_some_and(|o| o.live) || !may_take_in(tx, sid, comment_ids)? {
        return Ok(false);
    }
    let Some(aid) = tx
        .query_row(
            "SELECT artifact_id FROM threads WHERE id = ?1",
            params![tid],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    else {
        return Err(CoreError::NotFound);
    };
    tx.execute(
        "UPDATE threads SET target_session_id = ?2 WHERE id = ?1",
        params![tid, sid],
    )?;
    st.agents_pinged();
    // The other agents of the taker's person.
    const SAME_PERSON: &str = "target_session_id IN (SELECT o.id FROM sessions o, sessions me
        WHERE me.id = ?2 AND o.id <> ?2 AND o.person IS me.person)";
    let others: Vec<String> = {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT DISTINCT target_session_id FROM feedback
             WHERE thread_id = ?1 AND delivered_at IS NULL AND {SAME_PERSON}"
        ))?;
        stmt.query_map(params![tid, sid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.execute(
        &format!(
            "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL AND {SAME_PERSON}"
        ),
        params![tid, sid],
    )?;
    for other in &others {
        super::approvals::withdraw_taken_in(st, tx, ctx, tid, other, now)?;
    }
    tx.execute(
        "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL
           AND target_session_id IS NULL
           AND comment_id IN (SELECT comment_id FROM feedback WHERE thread_id = ?1 AND target_session_id = ?2)",
        params![tid, sid],
    )?;
    let given = tx.execute(
        "UPDATE feedback SET target_session_id = ?2, untargeted_at = NULL, push_failed_at = NULL,
            notified_at = NULL
         WHERE thread_id = ?1 AND delivered_at IS NULL AND target_session_id IS NULL",
        params![tid, sid],
    )?;
    if given > 0 {
        touched.targets.insert(sid.to_string());
    }
    touched.targets.extend(others);
    touched.threads.insert((aid.clone(), tid.to_string()));
    st.record_audit(
        tx,
        ctx,
        super::feedback::take_record(now, &aid, tid, sid, cause),
    )?;
    Ok(true)
}

/// Threads of live artifacts; callers append `AND ...` conditions.
pub(crate) const THREAD_SELECT: &str =
    "SELECT t.id, t.artifact_id, t.version_n, t.anchor_json, t.status,
    t.sent_to_agent, t.has_clip, t.created_at, t.resolved_at, t.resolved_by, t.resolved_by_person,
    t.queue_id
    FROM threads t JOIN artifacts a ON a.id = t.artifact_id WHERE a.deleted_at IS NULL";

/// The status of each thread of `?2` (a JSON array of thread IDs) that
/// belongs to the live artifact `?1`.
pub(crate) const THREAD_STATUSES: &str = "SELECT t.id, t.status FROM json_each(?2) j
    CROSS JOIN threads t ON t.id = j.value
    JOIN artifacts a ON a.id = t.artifact_id
    WHERE t.artifact_id = ?1 AND a.deleted_at IS NULL";
/// The versions addressing each thread of `?1` (a JSON array), ascending.
pub(crate) const ADDRESSED_IN_MANY: &str = "SELECT vt.thread_id, vt.version_n FROM json_each(?1) j
    CROSS JOIN version_threads vt ON vt.thread_id = j.value
    ORDER BY vt.thread_id, vt.version_n";
/// The pending address of each thread of `?1` (a JSON array): harness and time.
pub(crate) const PENDING_OF_MANY: &str =
    "SELECT lp.thread_id, lp.harness, lp.created_at FROM json_each(?1) j
    CROSS JOIN live_pending lp ON lp.thread_id = j.value";
/// The batches that sent each thread of `?1` (a JSON array), oldest first.
pub(crate) const SENDS_OF_MANY: &str =
    "SELECT bt.thread_id, b.id, b.size, b.note, b.sent_by, b.created_at, b.sender
    FROM json_each(?1) j
    CROSS JOIN batch_threads bt ON bt.thread_id = j.value
    JOIN send_batches b ON b.id = bt.batch_id
    ORDER BY bt.thread_id, b.created_at, b.id";
/// The display name of each viewer of `?1` (a JSON array of public IDs).
pub(crate) const NAMES_OF_MANY: &str = "SELECT v.public_id, v.display_name FROM json_each(?1) j
    CROSS JOIN viewers v INDEXED BY viewers_public_id ON v.public_id = j.value";

/// The `live_path` of each thread of `?1` (a JSON array) that has one.
pub(crate) const LIVE_PATHS_OF_MANY: &str = "SELECT t.id, t.live_path FROM json_each(?1) j
    CROSS JOIN threads t ON t.id = j.value WHERE t.live_path IS NOT NULL";
/// The moves of each thread of `?1` (a JSON array), oldest first.
pub(crate) const MOVES_OF_MANY: &str = "SELECT m.thread_id, m.from_artifact_id, m.from_url,
    m.to_artifact_id, m.to_url, m.moved_by, m.rule_id, m.created_at, m.kind
    FROM json_each(?1) j CROSS JOIN thread_moves m ON m.thread_id = j.value
    ORDER BY m.thread_id, m.created_at, m.id";

/// A move of a live page's thread to another page (spec
/// 2026-10-05-chrome-overlay-design §7.1): by the owner, or by a merge rule.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ThreadMove {
    pub from_artifact_id: String,
    /// The thread's URL before the move (its path and route).
    pub from_url: String,
    pub to_artifact_id: String,
    /// The thread's URL after the move.
    pub to_url: String,
    /// `viewer:<public ID>` of the owner who moved it.
    pub moved_by: String,
    /// The mover's current display name, when they have one.
    pub moved_by_name: Option<String>,
    /// The merge rule that moved it, when one did.
    pub rule_id: Option<String>,
    /// `move` (the owner's), `merge` (by `rule_id`), or `unmerge` (back,
    /// when `rule_id` was deleted).
    pub kind: String,
    pub at: String,
}

/// A page of threads of artifact `?1` (resolved ones too when `?2`) after
/// the thread `?3` (`NULL`: from the start), at most `?4`. A queue's
/// reports are left out: they are listed through their queue alone.
pub(crate) fn list_threads_sql() -> String {
    format!(
        "{THREAD_SELECT} AND t.artifact_id = ?1 AND t.queue_id IS NULL AND (?2 OR t.status = 'open')
         AND (?3 IS NULL OR (t.created_at, t.id) > (SELECT created_at, id FROM threads WHERE id = ?3))
         ORDER BY t.created_at, t.id LIMIT ?4"
    )
}

/// What a thread view adds to a stored thread; see [`Store::thread_extras`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadExtras {
    /// Its delivery rows, which each reader reads as [`Delivery::view`]
    /// says; `None` when nothing was forwarded.
    pub delivery: Option<Delivery>,
    pub addressed_in: Vec<u32>,
    pub sends: Vec<ThreadSend>,
    pub resolved_by_name: Option<String>,
    /// A live page's address waiting for its next snapshot: the addressing
    /// agent's harness and when ([`Store::mark_pending`]).
    pub addressed_pending: Option<(String, String)>,
    /// The path a live page's thread was made at, when it is not its page's
    /// path (a merge rule mapped the URL, or a merge re-filed the thread).
    pub live_path: Option<String>,
    /// The thread's moves between live pages, oldest first.
    pub moves: Vec<ThreadMove>,
    /// Its owner agent, live or ended, when it has one (M23).
    pub owner: Option<ThreadOwner>,
    /// Its twin on a remote, when it is linked ([`super::linked`]).
    pub linked: Option<super::linked::LinkedView>,
}

struct ThreadRow {
    id: String,
    artifact_id: String,
    version_n: u32,
    anchor_json: String,
    status: String,
    sent_to_agent: bool,
    has_clip: bool,
    created_at: String,
    resolved_at: Option<String>,
    resolved_by: Option<String>,
    resolved_by_person: Option<String>,
    queue_id: Option<String>,
}

fn row_to_thread_row(r: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        id: r.get("id")?,
        artifact_id: r.get("artifact_id")?,
        version_n: r.get("version_n")?,
        anchor_json: r.get("anchor_json")?,
        status: r.get("status")?,
        sent_to_agent: r.get::<_, i64>("sent_to_agent")? != 0,
        has_clip: r.get::<_, i64>("has_clip")? != 0,
        created_at: r.get("created_at")?,
        resolved_at: r.get("resolved_at")?,
        resolved_by: r.get("resolved_by")?,
        resolved_by_person: r.get("resolved_by_person")?,
        queue_id: r.get("queue_id")?,
    })
}

impl ThreadRow {
    fn into_thread(self, comments: Vec<Comment>) -> Result<Thread> {
        let anchor =
            serde_json::from_str::<Anchor>(&self.anchor_json).map_err(|_| CoreError::Corrupt {
                artifact_id: self.artifact_id.clone(),
                column: "anchor_json",
                version: Some(self.version_n),
            })?;
        Ok(Thread {
            id: self.id,
            artifact_id: self.artifact_id,
            version_n: self.version_n,
            anchor,
            status: self.status,
            sent_to_agent: self.sent_to_agent,
            has_clip: self.has_clip,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by,
            comments,
            resolved_by_person: self.resolved_by_person,
            queue_id: self.queue_id,
        })
    }
}

fn row_to_comment(r: &Row<'_>) -> rusqlite::Result<Comment> {
    Ok(Comment {
        id: r.get("id")?,
        thread_id: r.get("thread_id")?,
        author_kind: r.get("author_kind")?,
        author_name: r.get("author_name")?,
        author_public_id: r.get("author_public_id")?,
        via_harness: r.get("via_harness")?,
        via_page: r.get::<_, i64>("via_page")? != 0,
        body: r.get("body")?,
        created_at: r.get("created_at")?,
        page_authors: json_list(r.get("page_authors")?),
        agent_person: r.get("agent_person")?,
        remote_author: r.get("remote_author")?,
        agent: comment_agent(
            r.get("agent_name")?,
            r.get("agent_person")?,
            r.get("agent_person_name")?,
        ),
    })
}

/// The [`CommentAgent`] of a comment whose session is named `name`, of
/// person `person` (`None`: the owner, as `thread.owner.person` names it)
/// whose display name is `raw`, given as stored, as browsers read names
/// ([`super::attention::browser_name`]); `None` with no name.
fn comment_agent(
    name: Option<String>,
    person: Option<String>,
    raw: Option<String>,
) -> Option<clax_core::model::CommentAgent> {
    let name = name?;
    let person_name = super::attention::browser_name(raw.as_deref(), person.is_none());
    Some(clax_core::model::CommentAgent {
        name,
        person,
        person_name,
    })
}

/// A `json_group_array` of strings, empty when absent or unreadable.
pub(crate) fn json_list(json: Option<String>) -> Vec<String> {
    json.and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Records `authors` as what the page write of `kind` (`comment` or
/// `asset`) `id` carries (spec §8.2, M11).
pub(crate) fn add_page_authors(
    c: &Connection,
    kind: &str,
    id: &str,
    authors: &[String],
) -> Result<()> {
    let mut stmt = c.prepare_cached(
        "INSERT OR IGNORE INTO page_authors (kind, id, author) VALUES (?1, ?2, ?3)",
    )?;
    for a in authors {
        stmt.execute(params![kind, id, a])?;
    }
    Ok(())
}

/// Comments with their author's session harness; callers append `WHERE ...`.
const COMMENT_SELECT: &str = "SELECT c.id, c.thread_id, c.author_kind, c.author_name, c.author_public_id, s.harness AS via_harness, c.via_page, c.body, c.created_at,
        CASE WHEN c.via_page THEN (SELECT json_group_array(pa.author) FROM page_authors pa
                                   WHERE pa.kind = 'comment' AND pa.id = c.id) END AS page_authors,
        s.person AS agent_person, s.name AS agent_name,
        CASE WHEN s.id IS NULL THEN NULL WHEN s.person IS NULL THEN (SELECT display_name FROM viewers WHERE owner = 1)
             ELSE (SELECT display_name FROM viewers WHERE public_id = s.person) END AS agent_person_name,
        (SELECT lc.author_key FROM linked_comments lc WHERE lc.comment_id = c.id) AS remote_author
     FROM comments c LEFT JOIN sessions s ON s.id = c.via_session_id";

fn load_comments(c: &Connection, thread_id: &str) -> Result<Vec<Comment>> {
    let mut stmt = c.prepare_cached(&format!(
        "{COMMENT_SELECT} WHERE c.thread_id = ?1 ORDER BY c.created_at, c.id"
    ))?;
    Ok(stmt
        .query_map(params![thread_id], row_to_comment)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The comments of each of `thread_ids`, oldest first, by thread ID.
pub(crate) const COMMENTS_OF_MANY: &str = "SELECT c.id, c.thread_id, c.author_kind, c.author_name, c.author_public_id, s.harness AS via_harness, c.via_page, c.body, c.created_at,
        CASE WHEN c.via_page THEN (SELECT json_group_array(pa.author) FROM page_authors pa
                                   WHERE pa.kind = 'comment' AND pa.id = c.id) END AS page_authors,
        s.person AS agent_person, s.name AS agent_name,
        CASE WHEN s.id IS NULL THEN NULL WHEN s.person IS NULL THEN (SELECT display_name FROM viewers WHERE owner = 1)
             ELSE (SELECT display_name FROM viewers WHERE public_id = s.person) END AS agent_person_name,
        (SELECT lc.author_key FROM linked_comments lc WHERE lc.comment_id = c.id) AS remote_author
     FROM json_each(?1) j CROSS JOIN comments c ON c.thread_id = j.value
     LEFT JOIN sessions s ON s.id = c.via_session_id
     ORDER BY c.thread_id, c.created_at, c.id";

fn load_comments_many(
    c: &Connection,
    thread_ids: &[String],
) -> Result<HashMap<String, Vec<Comment>>> {
    let mut out: HashMap<String, Vec<Comment>> = HashMap::new();
    if thread_ids.is_empty() {
        return Ok(out);
    }
    let mut stmt = c.prepare_cached(COMMENTS_OF_MANY)?;
    for comment in stmt.query_map(params![id_array(thread_ids)], row_to_comment)? {
        let comment = comment?;
        out.entry(comment.thread_id.clone())
            .or_default()
            .push(comment);
    }
    Ok(out)
}

/// Every thread of the artifacts of `?1` (a JSON array of IDs), resolved
/// ones too, oldest first, but for a queue's reports.
pub(crate) fn threads_of_many_sql() -> String {
    format!(
        "{THREAD_SELECT} AND t.artifact_id IN (SELECT value FROM json_each(?1)) AND t.queue_id IS NULL
         ORDER BY t.created_at, t.id"
    )
}

/// The threads of the artifacts `ids` ([`threads_of_many_sql`]) with their
/// comments; a thread whose anchor does not parse is skipped.
pub(crate) fn threads_of_many(c: &Connection, ids: &[String]) -> Result<Vec<Thread>> {
    let mut stmt = c.prepare_cached(&threads_of_many_sql())?;
    let rows = stmt
        .query_map(params![id_array(ids)], row_to_thread_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut comments_of = load_comments_many(c, &tids)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let comments = comments_of.remove(&row.id).unwrap_or_default();
        let tid = row.id.clone();
        match row.into_thread(comments) {
            Ok(t) => out.push(t),
            Err(CoreError::Corrupt { column, .. }) => {
                tracing::warn!(
                    thread_id = tid.as_str(),
                    column,
                    "skipping a corrupt thread"
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// The thread `thread_id` of a live artifact, with its comments.
pub(crate) fn thread_in(c: &Connection, thread_id: &str) -> Result<Option<Thread>> {
    let row = c
        .prepare_cached(&format!("{THREAD_SELECT} AND t.id = ?1"))?
        .query_row(params![thread_id], row_to_thread_row)
        .optional()?;
    match row {
        None => Ok(None),
        Some(row) => {
            let comments = load_comments(c, &row.id)?;
            row.into_thread(comments).map(Some)
        }
    }
}

/// `comments_off` while artifact `id`'s Comments switch is off
/// ([`Store::set_comments`]), read in the caller's transaction so a
/// comment and the switch turning off are ordered: no thread or comment
/// lands once the switch is off. An artifact with no row passes (the
/// caller refuses it as missing).
fn refuse_comments_off_in(c: &Connection, id: &str) -> Result<()> {
    let on: Option<bool> = c
        .query_row(
            "SELECT comments FROM artifacts WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    if on == Some(false) {
        return Err(CoreError::invalid(
            "comments_off",
            "comments are off on this artifact",
        ));
    }
    Ok(())
}

pub(crate) fn artifact_live(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0)",
        params![id],
        |r| r.get(0),
    )?)
}

/// Records the viewers `body` mentions (see [`clax_core::mentions::mentioned`])
/// for the comment `comment_id`, in the caller's transaction.
fn insert_mentions(tx: &Connection, comment_id: &str, body: &str) -> Result<()> {
    let names: Vec<(String, String)> = {
        let mut st = tx.prepare(
            "SELECT public_id, display_name FROM viewers WHERE display_name IS NOT NULL AND display_name != ''",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for p in clax_core::mentions::mentioned(body, &names) {
        tx.execute(
            "INSERT OR IGNORE INTO mentions (comment_id, public_id) VALUES (?1, ?2)",
            params![comment_id, p],
        )?;
    }
    Ok(())
}

impl Store {
    /// Creates a thread on version `t.version_n` of the live artifact `id` with
    /// its first (viewer) comment. Liveness, the version and the anchor are
    /// checked and the rows inserted in one transaction. A clip is written
    /// to the content store first, under the artifact's content lock (which
    /// a delete of the artifact holds too) and once the artifact is seen
    /// live, so a delete either comes first (nothing is written) or comes
    /// after and removes the clip with the artifact's content. When
    /// anything fails, no rows remain and the clip is removed. Callers
    /// check the clip with [`clip_problem`] first. Records `thread.open`
    /// and the first comment's `comment.add` under `ctx`, in the same
    /// transaction.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `invalid_anchor` (also
    /// when the version holds no file at `anchor.file`; an `artifact` anchor
    /// names none and is stored with its `file` emptied), `invalid_comment`,
    /// or `unknown_version` for bad input.
    pub fn create_thread(&self, ctx: &AuditCtx, id: &ArtifactId, t: NewThread) -> Result<Thread> {
        self.block_on(self.create_thread_async(ctx, id, t))
    }

    /// [`Store::create_thread`], awaiting its content store.
    pub async fn create_thread_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
    ) -> Result<Thread> {
        self.create_thread_inner(ctx, id, t, &[], None, |_, _| Ok((None, Vec::new())))
            .await
            .map(Keyed::made)
    }

    /// [`Store::create_thread`] for a creator who may not see the files
    /// `hidden` of the version: an anchor on one of them is refused exactly
    /// as an anchor on a file the version lacks is, after every check that
    /// comes before that one, so the answer tells nothing of the names
    /// (spec 2026-10-08-multi-agent-multi-person §8.3).
    ///
    /// # Errors
    /// [`Store::create_thread`]'s.
    pub fn create_thread_hiding(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        hidden: &[String],
    ) -> Result<Thread> {
        self.block_on(self.create_thread_hiding_async(ctx, id, t, hidden))
    }

    /// [`Store::create_thread_hiding`], awaiting its content store.
    pub async fn create_thread_hiding_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        hidden: &[String],
    ) -> Result<Thread> {
        self.create_thread_inner(ctx, id, t, hidden, None, |_, _| Ok((None, Vec::new())))
            .await
            .map(Keyed::made)
    }

    /// [`Store::create_thread_hiding_async`] for an agent token's write
    /// keyed `key` (none: as unkeyed): the key is looked up and recorded in
    /// the transaction that writes the thread, so two writes under one key
    /// make one thread, and a thread is never made without its key. A key
    /// recorded already answers what it made ([`Keyed::Before`]), writing
    /// nothing.
    ///
    /// # Errors
    /// [`Store::create_thread`]'s.
    pub async fn create_thread_keyed_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        hidden: &[String],
        key: Option<CommentKey<'_>>,
    ) -> Result<Keyed<Thread>> {
        self.create_thread_inner(ctx, id, t, hidden, key, |_, _| Ok((None, Vec::new())))
            .await
    }

    /// [`Store::create_thread_async`], running `then` with the new thread's
    /// ID in the transaction that writes it: an error from `then` rolls the
    /// thread back. `then` answers the `live_path` it set, which
    /// `thread.open` records, and the events of what else it changed,
    /// recorded under `ctx` after the thread's own.
    pub(crate) async fn create_thread_then_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        then: impl FnOnce(
            &rusqlite::Transaction<'_>,
            &str,
        ) -> Result<(Option<String>, Vec<clax_core::audit::AuditRecord>)>,
    ) -> Result<Thread> {
        self.create_thread_inner(ctx, id, t, &[], None, then)
            .await
            .map(Keyed::made)
    }

    /// [`Store::create_thread_then_async`], an anchor on any of `hidden`
    /// refused as one on a file the version lacks
    /// ([`Store::create_thread_hiding`]), keyed `key`
    /// ([`Store::create_thread_keyed_async`]).
    async fn create_thread_inner(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        hidden: &[String],
        key: Option<CommentKey<'_>>,
        then: impl FnOnce(
            &rusqlite::Transaction<'_>,
            &str,
        ) -> Result<(Option<String>, Vec<clax_core::audit::AuditRecord>)>,
    ) -> Result<Keyed<Thread>> {
        let mut t = t;
        t.anchor = t.anchor.normalized();
        t.anchor.validate()?;
        check_body(&t.body)?;
        let tid = new_ulid();
        let now = self.now();
        let anchor_json = serde_json::to_string(&t.anchor).expect("anchors serialise");
        // The clip goes first, under the lock a delete holds across its
        // transaction and the content's removal.
        let _lock = match &t.clip {
            Some(bytes) => {
                let lock = self.lock_content(id).await?;
                if !self.with_read(|c| artifact_live(c, id.as_str()))? {
                    return Err(CoreError::NotFound);
                }
                self.content.put_clip(id, &tid, bytes).await?;
                Some(lock)
            }
            None => None,
        };
        let mut before = None;
        let made = self.with_tx(|tx| {
            if let Some(k) = key
                && let Some(found) = super::linked::comment_key_in(tx, k)?
            {
                before = Some(found);
                return Ok(());
            }
            if !artifact_live(tx, id.as_str())? {
                return Err(CoreError::NotFound);
            }
            refuse_comments_off_in(tx, id.as_str())?;
            if t.anchor.route.is_some() {
                let kind: String = tx.query_row(
                    "SELECT kind FROM artifacts WHERE id = ?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )?;
                if kind != clax_core::live::KIND_LIVE {
                    return Err(CoreError::invalid(
                        "invalid_anchor",
                        "route is only for live pages",
                    ));
                }
            }
            let files: Option<String> = tx
                .query_row(
                    "SELECT files_json FROM versions WHERE artifact_id = ?1 AND n = ?2",
                    params![id.as_str(), t.version_n],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(files) = files else {
                return Err(CoreError::invalid(
                    "unknown_version",
                    format!("artifact {id} has no version {}", t.version_n),
                ));
            };
            let files: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&files).map_err(|_| CoreError::Corrupt {
                    artifact_id: id.to_string(),
                    column: "files_json",
                    version: Some(t.version_n),
                })?;

            // An `artifact` anchor is on no page; any version holds it.
            if !t.anchor.is_whole()
                && (!files.contains_key(&t.anchor.file) || hidden.contains(&t.anchor.file))
            {
                return Err(CoreError::invalid(
                    "invalid_anchor",
                    format!(
                        "version {} of artifact {id} has no file {}",
                        t.version_n, t.anchor.file
                    ),
                ));
            }

            tx.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, sent_to_agent, has_clip, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'open', 0, ?5, ?6)",
                params![tid, id.as_str(), t.version_n, anchor_json, t.clip.is_some(), now],
            )?;
            // A comment on a seeded quickstart stops Clax's updates to it for
            // good, even if the thread is deleted later.
            tx.execute(
                "UPDATE quickstarts SET stopped_at = ?2 WHERE artifact_id = ?1 AND stopped_at IS NULL",
                params![id.as_str(), now],
            )?;
            let cid = new_ulid();
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
                 VALUES (?1, ?2, 'viewer', ?3, ?4, NULL, ?5, ?6, ?7)",
                params![cid, tid, t.author_name, t.author_public_id, t.via_page, t.body, now],
            )?;
            // A page's comment also carries every author of the page, fixed now (M11).
            let page_authors = if t.via_page {
                let a = super::artifacts::page_authors_in(tx, id.as_str())?;
                add_page_authors(tx, "comment", &cid, &a)?;
                a
            } else {
                Vec::new()
            };
            insert_mentions(tx, &cid, &t.body)?;
            if let Some(k) = key {
                super::linked::record_comment_key_in(tx, k, &tid, &cid, &now)?;
            }
            let (live_path, after) = then(tx, &tid)?;
            let open = open_record(
                &now,
                id.as_str(),
                &tid,
                t.version_n,
                anchor_record(&t.anchor),
                live_path,
                t.clip.is_some(),
                Some(&cid),
            );
            self.record_audit(tx, ctx, open)?;
            let first = Comment {
                id: cid.clone(),
                thread_id: tid.clone(),
                author_kind: AUTHOR_VIEWER.to_string(),
                author_name: t.author_name.clone(),
                author_public_id: t.author_public_id.clone(),
                via_harness: None,
                via_page: t.via_page,
                body: t.body.clone(),
                created_at: now.clone(),
                page_authors,
                agent_person: None,
                remote_author: None,
                agent: None,
            };
            self.record_audit(tx, ctx, comment_record(id.as_str(), &first))?;
            for rec in after {
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(())
        });
        if made.is_err() || before.is_some() {
            if t.clip.is_some()
                && let Err(gone) = self.content.delete_clip(id, &tid).await
            {
                tracing::warn!(
                    artifact = id.as_str(),
                    thread = tid,
                    error = %gone,
                    "could not remove the clip of a thread not made"
                );
            }
            made?;
        }
        if let Some((thread_id, comment_id)) = before {
            return Ok(Keyed::Before {
                thread_id,
                comment_id,
            });
        }
        Ok(Keyed::Made(
            self.get_thread(&tid)?.ok_or(CoreError::NotFound)?,
        ))
    }

    /// The clip image of thread `tid` of `id`, read from the content store;
    /// `None` when the thread has none, or its bytes are missing.
    ///
    /// # Errors
    /// Database or content errors.
    pub fn read_clip(&self, id: &ArtifactId, tid: &str) -> Result<Option<Vec<u8>>> {
        self.block_on(self.read_clip_async(id, tid))
    }

    /// [`Store::read_clip`], awaiting its content store.
    pub async fn read_clip_async(&self, id: &ArtifactId, tid: &str) -> Result<Option<Vec<u8>>> {
        let has: Option<bool> = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT has_clip FROM threads WHERE id = ?1 AND artifact_id = ?2",
                params![tid, id.as_str()],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        if has != Some(true) {
            return Ok(None);
        }
        match self.content.read_clip(id, tid).await? {
            Some(b) => Ok(Some(b)),
            // A mirrored thread's clip is cached apart (`store::linked`).
            None => Ok(self
                .cached_remote_clip(tid)
                .and_then(|p| std::fs::read(p).ok())),
        }
    }

    /// The file on this machine holding the clip of thread `tid` of `id`,
    /// when the content store keeps clips in files
    /// ([`clax_core::content::ContentStore::local_clip`]), for an agent on the
    /// same machine.
    /// A mirrored thread's is its cached copy under the home
    /// ([`Store::remote_clip_file`]), never a path a remote sent.
    pub fn local_clip(&self, id: &ArtifactId, tid: &str) -> Option<std::path::PathBuf> {
        self.content
            .local_clip(id, tid)
            .or_else(|| self.cached_remote_clip(tid))
    }

    /// Adds a comment. A viewer comment on a resolved thread reopens it; an
    /// agent comment never changes the thread's status. Records `comment.add`
    /// under `ctx`, and `thread.reopen` after it when the comment reopened
    /// the thread, in the comment's transaction.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone; `invalid_author_kind`
    /// or `invalid_comment` for bad input.
    pub fn add_comment(&self, ctx: &AuditCtx, thread_id: &str, c: NewComment) -> Result<Comment> {
        self.add_comment_then(ctx, thread_id, c, |_, _, _| Ok(()))
    }

    /// [`Store::add_comment`] for an agent token's write keyed `key` (none:
    /// as unkeyed): the key is looked up and recorded in the comment's
    /// transaction, so two writes under one key make one comment, and a
    /// comment is never made without its key. A key recorded already
    /// answers what it made ([`Keyed::Before`]), writing nothing.
    ///
    /// # Errors
    /// [`Store::add_comment`]'s.
    pub fn add_comment_keyed(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
        key: Option<CommentKey<'_>>,
    ) -> Result<Keyed<Comment>> {
        self.add_comment_keyed_then(ctx, thread_id, c, key, |_, _, _| Ok(()))
    }

    /// An agent reply `c` on thread `tid` of the live page `id` that also
    /// records the thread's pending address (`explicit`, by `harness`; see
    /// [`Store::mark_pending`]), in one transaction: both are written, or
    /// neither.
    ///
    /// # Errors
    /// As [`Store::add_comment`]; `NotFound` also when `tid` is not a thread of `id`.
    pub fn add_addressed_reply(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        tid: &str,
        c: NewComment,
        harness: &str,
    ) -> Result<Comment> {
        self.add_comment_then(ctx, tid, c, |tx, t, _| {
            if t.artifact_id != id.as_str() {
                return Err(CoreError::NotFound);
            }
            super::live::mark_pending_in(tx, id, tid, "explicit", harness, &self.now())?;
            Ok(())
        })
    }

    /// [`Store::add_comment`] journaled as `entry` in the comment's own
    /// transaction (ClaxBot's replies, `crate::bot_runs`): the journal's
    /// answer when it holds `entry`'s key, writing nothing; else the
    /// comment and its entry, `{comment_id}`, together.
    ///
    /// # Errors
    /// [`Store::add_comment`]'s; `key_reused`.
    pub fn add_comment_journaled(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
        entry: &super::bot_runs::CallEntry,
        guard: super::bot_runs::Guard<'_>,
    ) -> Result<super::bot_runs::Journaled> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(prev);
        }
        let mut out = None;
        self.add_comment_then(ctx, thread_id, c, |tx, _, comment| {
            guard(&super::bot_runs::Tx(tx))?;
            out =
                Some(self.journal_in(tx, entry, serde_json::json!({"comment_id": comment.id}))?);
            Ok(())
        })?;
        Ok(out.expect("the comment was journaled"))
    }

    /// [`Store::resolve_thread`] by `by` (for `person`), journaled as
    /// `entry` in the resolve's own transaction, as
    /// [`Store::add_comment_journaled`] is; the value is `null`.
    ///
    /// # Errors
    /// [`Store::resolve_thread`]'s; `key_reused`.
    pub fn resolve_thread_journaled(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        by: &str,
        person: Option<&str>,
        entry: &super::bot_runs::CallEntry,
        guard: super::bot_runs::Guard<'_>,
    ) -> Result<super::bot_runs::Journaled> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(prev);
        }
        let mut out = None;
        self.resolve_thread_then(ctx, thread_id, by, person, |tx, _| {
            guard(&super::bot_runs::Tx(tx))?;
            out = Some(self.journal_in(tx, entry, serde_json::Value::Null)?);
            Ok(None)
        })?;
        Ok(out.expect("the resolve was journaled"))
    }

    /// Thread `tid`'s owner agent, live or ended, when it has one (spec
    /// 2026-10-08 §12).
    ///
    /// # Errors
    /// The store's.
    pub fn thread_owner(&self, tid: &str) -> Result<Option<ThreadOwner>> {
        self.with_read(|c| owner_in(c, tid))
    }

    /// `Ok` when session `sid` may reply to or resolve thread `tid` (M24):
    /// it has no owner, an ended one, or `sid`.
    ///
    /// # Errors
    /// [`CoreError::ThreadTaken`] naming the owner otherwise.
    pub fn check_not_taken(&self, tid: &str, sid: &str) -> Result<()> {
        self.with_read(|c| check_not_taken_in(c, tid, sid))
    }

    /// `Ok` when session `from` may hand thread `tid` off or release it
    /// ([`Store::handoff`]).
    ///
    /// # Errors
    /// [`CoreError::ThreadTaken`] naming the owner otherwise.
    pub fn check_may_hand_off(&self, tid: &str, from: &str) -> Result<()> {
        let seen = self.seen_comments(tid, from)?;
        self.with_read(|c| check_may_hand_off_in(c, tid, from, &seen).map(|_| ()))
    }

    /// The viewer comments of thread `tid` whose bodies session `sid`'s
    /// reader may see (spec 2026-10-08 §8), oldest first; none for a
    /// thread or session that is gone.
    ///
    /// # Errors
    /// The store's.
    pub(crate) fn seen_comments(&self, tid: &str, sid: &str) -> Result<Vec<String>> {
        let Some(t) = self.get_thread(tid)? else {
            return Ok(Vec::new());
        };
        if self.get_session(sid)?.is_none() {
            return Ok(Vec::new());
        }
        let vis = crate::visibility::Visibility::for_session(self, sid)?;
        Ok(t.comments
            .iter()
            .filter(|c| {
                c.author_kind == AUTHOR_VIEWER
                    && vis.sees(
                        &clax_core::visibility::authors_of_comment(c),
                        &t.artifact_id,
                        Some(tid),
                    )
            })
            .map(|c| c.id.clone())
            .collect())
    }

    /// An agent reply `c` on thread `tid` from the session `c` names
    /// (`via_session_id`), in one transaction with the ownership rules
    /// (M23, M24): refused with [`CoreError::ThreadTaken`] when another
    /// live session owns the thread; on a thread with no live owner the
    /// replying session becomes its owner (`thread.take`, cause `reply`)
    /// when its person sent one of the thread's viewer comments it sees
    /// (O8): a reply by another person's agent, or one written seeing none
    /// of the comments, takes nothing.
    /// With `addressed` (the live page and the harness), it also records the
    /// thread's pending address as [`Store::add_addressed_reply`] does.
    /// Returns the comment and the sessions whose rows the take removed.
    ///
    /// # Errors
    /// As [`Store::add_comment`]; `unknown_session` without a session;
    /// `NotFound` when `addressed` names another artifact.
    pub fn add_agent_reply(
        &self,
        ctx: &AuditCtx,
        tid: &str,
        c: NewComment,
        addressed: Option<(&ArtifactId, &str)>,
    ) -> Result<(Comment, clax_core::feedback::Touched)> {
        let (c, touched) = self.add_agent_reply_keyed(ctx, tid, c, addressed, None)?;
        Ok((c.made(), touched))
    }

    /// [`Store::add_agent_reply`] keyed `key`, as [`Store::add_comment_keyed`]
    /// is: a key recorded already answers what it made, and nothing is
    /// written or taken.
    ///
    /// # Errors
    /// [`Store::add_agent_reply`]'s.
    pub fn add_agent_reply_keyed(
        &self,
        ctx: &AuditCtx,
        tid: &str,
        c: NewComment,
        addressed: Option<(&ArtifactId, &str)>,
        key: Option<CommentKey<'_>>,
    ) -> Result<(Keyed<Comment>, clax_core::feedback::Touched)> {
        let Some(sid) = c.via_session_id.clone() else {
            return Err(CoreError::invalid(
                "unknown_session",
                "an agent reply comes from a session",
            ));
        };
        let seen = self.seen_comments(tid, &sid)?;
        let mut touched = clax_core::feedback::Touched::default();
        let comment = self.add_comment_keyed_then(ctx, tid, c, key, |tx, t, _| {
            check_not_taken_in(tx, tid, &sid)?;
            if let Some((id, harness)) = addressed {
                if t.artifact_id != id.as_str() {
                    return Err(CoreError::NotFound);
                }
                super::live::mark_pending_in(tx, id, tid, "explicit", harness, &self.now())?;
            }
            take_in(
                self,
                tx,
                ctx,
                tid,
                &sid,
                "reply",
                &seen,
                &self.now(),
                &mut touched,
            )?;
            Ok(())
        })?;
        Ok((comment, touched))
    }

    /// An agent reply `c` on thread `tid` from the session `c` names, inside
    /// the caller's transaction, with [`Store::add_agent_reply`]'s
    /// ownership rules: `seen` is what that session's reader saw of the
    /// thread's viewer comments ([`Store::seen_comments`], read before the
    /// transaction). What the take changed is added to `touched`.
    ///
    /// # Errors
    /// As [`Store::add_agent_reply`].
    pub(crate) fn add_agent_reply_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        tid: &str,
        c: NewComment,
        seen: &[String],
        touched: &mut clax_core::feedback::Touched,
    ) -> Result<(Comment, Thread)> {
        let Some(sid) = c.via_session_id.clone() else {
            return Err(CoreError::invalid(
                "unknown_session",
                "an agent reply comes from a session",
            ));
        };
        check_not_taken_in(tx, tid, &sid)?;
        let out = self.add_comment_in(tx, ctx, tid, c)?;
        take_in(
            self,
            tx,
            ctx,
            tid,
            &sid,
            "reply",
            seen,
            &self.now(),
            touched,
        )?;
        Ok(out)
    }

    /// [`Store::add_comment`], running `then` with the thread in the
    /// comment's transaction; an error from `then` writes nothing.
    pub(crate) fn add_comment_then(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
        then: impl FnOnce(&rusqlite::Transaction<'_>, &Thread, &Comment) -> Result<()>,
    ) -> Result<Comment> {
        self.add_comment_keyed_then(ctx, thread_id, c, None, then)
            .map(Keyed::made)
    }

    /// [`Store::add_comment_then`] keyed `key`
    /// ([`Store::add_comment_keyed`]): a key recorded already writes nothing
    /// and runs no `then`.
    fn add_comment_keyed_then(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
        key: Option<CommentKey<'_>>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, &Thread, &Comment) -> Result<()>,
    ) -> Result<Keyed<Comment>> {
        self.with_tx(|tx| {
            if let Some(k) = key
                && let Some((thread_id, comment_id)) = super::linked::comment_key_in(tx, k)?
            {
                return Ok(Keyed::Before {
                    thread_id,
                    comment_id,
                });
            }
            let (comment, t) = self.add_comment_keyed_in(tx, ctx, thread_id, c, key)?;
            then(tx, &t, &comment)?;
            Ok(Keyed::Made(comment))
        })
    }

    /// [`Store::add_comment`] inside the caller's transaction: the comment
    /// and its thread as it was before it.
    pub(crate) fn add_comment_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
    ) -> Result<(Comment, Thread)> {
        self.add_comment_keyed_in(tx, ctx, thread_id, c, None)
    }

    /// [`Store::add_comment_in`], recording `key` as having made the
    /// comment when it names one; the caller has checked that no comment
    /// holds it yet.
    fn add_comment_keyed_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        thread_id: &str,
        c: NewComment,
        key: Option<CommentKey<'_>>,
    ) -> Result<(Comment, Thread)> {
        if c.author_kind != AUTHOR_VIEWER && c.author_kind != AUTHOR_AGENT {
            return Err(CoreError::invalid(
                "invalid_author_kind",
                format!(
                    "author_kind is {AUTHOR_VIEWER} or {AUTHOR_AGENT}, not {}",
                    c.author_kind
                ),
            ));
        }
        check_body(&c.body)?;
        let mut comment = Comment {
            id: new_ulid(),
            thread_id: thread_id.to_string(),
            author_kind: c.author_kind.to_string(),
            author_name: c.author_name,
            author_public_id: if c.author_kind == AUTHOR_VIEWER {
                c.author_public_id
            } else {
                None
            },
            via_harness: None,
            via_page: c.via_page && c.author_kind == AUTHOR_VIEWER,
            body: c.body,
            created_at: self.now(),
            page_authors: Vec::new(),
            agent_person: None,
            remote_author: None,
            agent: None,
        };
        {
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            refuse_comments_off_in(tx, &t.artifact_id)?;
            if let Some(k) = key {
                super::linked::record_comment_key_in(
                    tx,
                    k,
                    thread_id,
                    &comment.id,
                    &comment.created_at,
                )?;
            }
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![comment.id, comment.thread_id, comment.author_kind, comment.author_name, comment.author_public_id, c.via_session_id, comment.via_page, comment.body, comment.created_at],
            )?;
            // A page's comment also carries every author of the page, fixed now (M11).
            if comment.via_page {
                comment.page_authors = super::artifacts::page_authors_in(tx, &t.artifact_id)?;
                add_page_authors(tx, "comment", &comment.id, &comment.page_authors)?;
            }
            if let Some(sid) = &c.via_session_id
                && let Some((h, p, n, raw)) = tx
                    .query_row(
                        "SELECT s.harness, s.person, s.name,
                                CASE WHEN s.person IS NULL THEN (SELECT display_name FROM viewers WHERE owner = 1)
                                     ELSE (SELECT display_name FROM viewers WHERE public_id = s.person) END
                           FROM sessions s WHERE s.id = ?1",
                        params![sid],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, Option<String>>(3)?,
                            ))
                        },
                    )
                    .optional()?
            {
                comment.via_harness = Some(h);
                comment.agent = comment_agent(n, p.clone(), raw);
                comment.agent_person = p;
            }
            if comment.author_kind == AUTHOR_AGENT {
                super::inbox::note_reply(
                    tx,
                    &comment.id,
                    thread_id,
                    &t.artifact_id,
                    c.via_session_id.as_deref(),
                    &comment.body,
                    &self.now(),
                )?;
            }
            self.record_audit(tx, ctx, comment_record(&t.artifact_id, &comment))?;
            if comment.author_kind == AUTHOR_VIEWER {
                insert_mentions(tx, &comment.id, &comment.body)?;
                // A reopened report is no longer verified or closed for a
                // reason; a fix waiting for a verdict still waits.
                tx.execute(
                    "UPDATE threads SET status = 'open', resolved_at = NULL, resolved_by = NULL,
                        resolved_by_person = NULL, report_phase = NULLIF(report_phase, 'verified'),
                        resolve_reason = NULL WHERE id = ?1",
                    params![thread_id],
                )?;
                if t.status == "resolved" {
                    let reopen = thread_record(
                        AuditKind::ThreadReopen,
                        &comment.created_at,
                        &t.artifact_id,
                        thread_id,
                    );
                    self.record_audit(tx, ctx, reopen)?;
                }
            }
            Ok((comment, t))
        }
    }

    /// The thread with its comments, or `None` when it or its artifact is gone.
    ///
    /// # Errors
    /// `Corrupt` when its `anchor_json` does not parse.
    pub fn get_thread(&self, thread_id: &str) -> Result<Option<Thread>> {
        self.with_read(|c| thread_in(c, thread_id))
    }

    /// Threads of the live artifact `id`, oldest first; resolved ones only with
    /// `include_resolved`. Pages of `limit` start after the thread `cursor`;
    /// the second value is the cursor for the next page, `None` on the last.
    /// A row whose `anchor_json` does not parse counts against the page but is
    /// logged and left out, so a page can hold fewer than `limit` threads.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `invalid_cursor` when
    /// `cursor` is not a thread of `id`.
    pub fn list_threads(
        &self,
        id: &ArtifactId,
        include_resolved: bool,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Thread>, Option<String>)> {
        let limit = limit.max(1);
        self.with_read(|c| {
            if !artifact_live(c, id.as_str())? {
                return Err(CoreError::NotFound);
            }
            if let Some(cursor) = cursor {
                let known: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1 AND artifact_id = ?2)",
                    params![cursor, id.as_str()],
                    |r| r.get(0),
                )?;
                if !known {
                    return Err(CoreError::invalid(
                        "invalid_cursor",
                        format!("{cursor} is not a thread of artifact {id}"),
                    ));
                }
            }
            let mut stmt = c.prepare_cached(&list_threads_sql())?;
            let rows = stmt
                .query_map(
                    params![id.as_str(), include_resolved, cursor, (limit + 1) as i64],
                    row_to_thread_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let more = rows.len() > limit;
            let ids: Vec<String> = rows.iter().take(limit).map(|r| r.id.clone()).collect();
            let mut comments_of = load_comments_many(c, &ids)?;
            let mut threads = Vec::with_capacity(limit);
            let mut last = None;
            for row in rows.into_iter().take(limit) {
                let thread_id = row.id.clone();
                let comments = comments_of.remove(&thread_id).unwrap_or_default();
                match row.into_thread(comments) {
                    Ok(thread) => threads.push(thread),
                    Err(CoreError::Corrupt {
                        artifact_id,
                        column,
                        ..
                    }) => {
                        tracing::warn!(
                            thread_id = thread_id.as_str(),
                            artifact_id,
                            column,
                            "skipping corrupt thread row"
                        );
                    }
                    Err(e) => return Err(e),
                }
                last = Some(thread_id);
            }
            let next = if more { last } else { None };
            Ok((threads, next))
        })
    }

    /// The status of each of `thread_ids` that is a thread of the live
    /// artifact `id`, by thread ID.
    pub fn thread_statuses(
        &self,
        id: &ArtifactId,
        thread_ids: &[String],
    ) -> Result<HashMap<String, String>> {
        if thread_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(THREAD_STATUSES)?;
            let rows = stmt.query_map(params![id.as_str(), id_array(thread_ids)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What a thread view adds to each of `threads`, in the same order: the
    /// delivery rows ([`Store::deliveries`]), the versions addressing it
    /// ([`Store::addressed_in`]), its sends ([`Store::thread_sends`]), and
    /// the current display name of the viewer named by `resolved_by`.
    pub fn thread_extras(&self, threads: &[Thread], codex_push: bool) -> Result<Vec<ThreadExtras>> {
        if threads.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
        let mut linked = self.linked_views(threads)?;
        let mut resolvers: Vec<String> = threads
            .iter()
            .filter_map(|t| t.resolved_by.as_deref()?.strip_prefix("viewer:"))
            .filter(|p| clax_core::is_public_id(p))
            .map(str::to_string)
            .collect();
        self.with_read(|c| {
            let arr = id_array(&ids);
            let mut live_paths: HashMap<String, String> = HashMap::new();
            let mut moves: HashMap<String, Vec<ThreadMove>> = HashMap::new();
            {
                let mut stmt = c.prepare_cached(LIVE_PATHS_OF_MANY)?;
                let mut rows = stmt.query(params![arr])?;
                while let Some(r) = rows.next()? {
                    live_paths.insert(r.get(0)?, r.get(1)?);
                }
                let mut stmt = c.prepare_cached(MOVES_OF_MANY)?;
                let mut rows = stmt.query(params![arr])?;
                while let Some(r) = rows.next()? {
                    let m = ThreadMove {
                        from_artifact_id: r.get(1)?,
                        from_url: r.get(2)?,
                        to_artifact_id: r.get(3)?,
                        to_url: r.get(4)?,
                        moved_by: r.get(5)?,
                        moved_by_name: None,
                        rule_id: r.get(6)?,
                        at: r.get(7)?,
                        kind: r.get(8)?,
                    };
                    if let Some(p) = m.moved_by.strip_prefix("viewer:")
                        && clax_core::is_public_id(p)
                    {
                        resolvers.push(p.to_string());
                    }
                    moves.entry(r.get(0)?).or_default().push(m);
                }
            }
            let mut states = deliveries_in(c, &ids, codex_push)?;
            let mut addressed: HashMap<String, Vec<u32>> = HashMap::new();
            let mut stmt = c.prepare_cached(ADDRESSED_IN_MANY)?;
            let mut rows = stmt.query(params![arr])?;
            while let Some(r) = rows.next()? {
                addressed.entry(r.get(0)?).or_default().push(r.get(1)?);
            }
            drop(rows);
            let mut sends: HashMap<String, Vec<ThreadSend>> = HashMap::new();
            let mut stmt = c.prepare_cached(SENDS_OF_MANY)?;
            let mut rows = stmt.query(params![arr])?;
            while let Some(r) = rows.next()? {
                sends.entry(r.get(0)?).or_default().push(ThreadSend {
                    batch_id: r.get(1)?,
                    size: r.get(2)?,
                    note: r.get(3)?,
                    sent_by: r.get(4)?,
                    sent_at: r.get(5)?,
                    sender: r.get(6)?,
                });
            }
            drop(rows);
            let mut pending: HashMap<String, (String, String)> = HashMap::new();
            let mut stmt = c.prepare_cached(PENDING_OF_MANY)?;
            let mut rows = stmt.query(params![arr])?;
            while let Some(r) = rows.next()? {
                pending.insert(r.get(0)?, (r.get(1)?, r.get(2)?));
            }
            drop(rows);
            let mut owners: HashMap<String, ThreadOwner> = HashMap::new();
            let mut stmt = c.prepare_cached(OWNERS_OF_MANY)?;
            let mut rows = stmt.query(params![arr])?;
            while let Some(r) = rows.next()? {
                owners.insert(r.get(0)?, row_to_owner(r, 1)?);
            }
            drop(rows);
            let mut names: HashMap<String, Option<String>> = HashMap::new();
            if !resolvers.is_empty() {
                let mut stmt = c.prepare_cached(NAMES_OF_MANY)?;
                let mut rows = stmt.query(params![id_array(&resolvers)])?;
                while let Some(r) = rows.next()? {
                    names.insert(r.get(0)?, r.get(1)?);
                }
            }
            Ok(threads
                .iter()
                .map(|t| ThreadExtras {
                    delivery: states.remove(&t.id),
                    addressed_in: addressed.remove(&t.id).unwrap_or_default(),
                    sends: sends.remove(&t.id).unwrap_or_default(),
                    resolved_by_name: t
                        .resolved_by
                        .as_deref()
                        .and_then(|by| by.strip_prefix("viewer:"))
                        .and_then(|p| names.get(p).cloned().flatten()),
                    addressed_pending: pending.remove(&t.id),
                    owner: owners.remove(&t.id),
                    linked: linked.remove(&t.id),
                    live_path: live_paths.remove(&t.id),
                    moves: moves
                        .remove(&t.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut m| {
                            m.moved_by_name = m
                                .moved_by
                                .strip_prefix("viewer:")
                                .and_then(|p| names.get(p).cloned().flatten());
                            m
                        })
                        .collect(),
                })
                .collect())
        })
    }

    /// Marks the thread resolved by `by` (`viewer:<public ID>`, `viewer:anonymous`,
    /// or `agent:<harness>`; never a cookie or a session ID, since the value is
    /// broadcast).
    /// Resolving a resolved thread keeps its first `resolved_at` and `resolved_by`.
    /// Undelivered feedback rows of the thread are deleted: nobody needs to act
    /// on a resolved thread. See [`Store::resolve_thread_touched`].
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone.
    pub fn resolve_thread(&self, ctx: &AuditCtx, thread_id: &str, by: &str) -> Result<Thread> {
        self.resolve_thread_touched(ctx, thread_id, by)
            .map(|(t, _)| t)
    }

    /// [`Store::resolve_thread`], also returning what it changed: the thread is
    /// in `threads` when undelivered rows were deleted (its feedback state then
    /// reflects only delivered rows, or is `None` when none remain); `targets`
    /// is always empty, since no session gains rows.
    ///
    /// Resolving an open thread records `thread.resolve` under `ctx` in the
    /// same transaction, with `resolved_by` the `by` given and
    /// `addressed_version` the version an agent's resolve linked it to (else
    /// `null`). Resolving a resolved thread changes nothing and records
    /// nothing.
    pub fn resolve_thread_touched(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        by: &str,
    ) -> Result<(Thread, clax_core::feedback::Touched)> {
        self.resolve_thread_then(ctx, thread_id, by, None, |_, _| Ok(None))
    }

    /// An agent's resolve of thread `tid` of artifact `id`, by `by`, that
    /// also records that the agent of `harness` addressed it, in one
    /// transaction: on a live page (`live`) as a pending `resolve` address
    /// ([`Store::mark_pending`]), otherwise linked to the current version
    /// (`link_on_resolve_in`). Both are written, or neither. Returns
    /// what [`Store::resolve_thread_touched`] does. The link rides in the
    /// resolve's `thread.resolve` (`addressed_version`); when the thread was
    /// already resolved, so no `thread.resolve` is recorded, a new link is
    /// recorded as `thread.addressed` (source `resolve`). `person` is whose
    /// agent resolved it (`*owner` or a person's public ID), kept with the
    /// first resolve as the resolver's person
    /// ([`Thread::resolved_by_person`]).
    ///
    /// # Errors
    /// As [`Store::resolve_thread`]; `NotFound` also when `tid` is not a
    /// thread of `id`.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_thread_addressed(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        tid: &str,
        by: &str,
        harness: &str,
        live: bool,
        person: Option<&str>,
    ) -> Result<(Thread, clax_core::feedback::Touched)> {
        self.resolve_thread_then(ctx, tid, by, person, |tx, t| {
            if t.artifact_id != id.as_str() {
                return Err(CoreError::NotFound);
            }
            if live {
                super::live::mark_pending_in(tx, id, tid, "resolve", harness, &self.now())?;
                Ok(None)
            } else {
                super::changelog::link_on_resolve_in(tx, tid, &self.now())
            }
        })
    }

    /// The resolve of thread `tid` of artifact `id` by agent session `sess`
    /// ([`Store::resolve_thread_addressed`], by `agent:<harness>`, its
    /// person the resolver's), refused with [`CoreError::ThreadTaken`] in
    /// the resolve's transaction when another live session owns the
    /// thread (M24).
    ///
    /// # Errors
    /// As [`Store::resolve_thread_addressed`], and `thread_taken`.
    pub fn resolve_thread_as(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        tid: &str,
        sess: &clax_core::model::Session,
        live: bool,
    ) -> Result<(Thread, clax_core::feedback::Touched)> {
        let by = format!("agent:{}", sess.harness);
        let person = sess
            .person
            .as_deref()
            .unwrap_or(clax_core::visibility::OWNER);
        self.resolve_thread_then(ctx, tid, &by, Some(person), |tx, t| {
            check_not_taken_in(tx, tid, &sess.id)?;
            if t.artifact_id != id.as_str() {
                return Err(CoreError::NotFound);
            }
            if live {
                super::live::mark_pending_in(tx, id, tid, "resolve", &sess.harness, &self.now())?;
                Ok(None)
            } else {
                super::changelog::link_on_resolve_in(tx, tid, &self.now())
            }
        })
    }

    /// [`Store::resolve_thread_touched`], running `then` with the thread in
    /// the resolve's transaction; an error from `then` writes nothing. `then`
    /// answers the version it linked the thread to, if any.
    fn resolve_thread_then(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
        by: &str,
        person: Option<&str>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, &Thread) -> Result<Option<u32>>,
    ) -> Result<(Thread, clax_core::feedback::Touched)> {
        let touched =
            self.with_tx(|tx| self.resolve_thread_in(tx, ctx, thread_id, by, person, then))?;
        Ok((
            self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?,
            touched,
        ))
    }

    /// [`Store::resolve_thread_then`] inside the caller's transaction:
    /// what it changed.
    pub(crate) fn resolve_thread_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        thread_id: &str,
        by: &str,
        person: Option<&str>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, &Thread) -> Result<Option<u32>>,
    ) -> Result<clax_core::feedback::Touched> {
        self.resolve_thread_reason_in(tx, ctx, thread_id, by, person, None, then)
    }

    /// [`Store::resolve_thread_in`] of a report closed for `reason`
    /// ([`clax_core::report::ResolveReason`]'s name), which the thread
    /// keeps (`resolve_reason`) and its `thread.resolve` names. Resolving
    /// an open thread also ends a fix's wait for a verdict
    /// (`report_phase` `fix_proposed`); resolving a resolved one changes
    /// neither.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve_thread_reason_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        thread_id: &str,
        by: &str,
        person: Option<&str>,
        reason: Option<&str>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, &Thread) -> Result<Option<u32>>,
    ) -> Result<clax_core::feedback::Touched> {
        {
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            let now = self.now();
            // The resolver's person goes with the resolver: only when this
            // resolve is the one `resolved_by` keeps.
            tx.execute(
                "UPDATE threads SET status = 'resolved', resolved_at = COALESCE(resolved_at, ?2),
                    resolved_by_person = CASE WHEN resolved_by IS NULL THEN ?4 ELSE resolved_by_person END,
                    resolved_by = COALESCE(resolved_by, ?3),
                    report_phase = CASE WHEN status = 'open' THEN NULLIF(report_phase, 'fix_proposed')
                        ELSE report_phase END,
                    resolve_reason = CASE WHEN status = 'open' THEN ?5 ELSE resolve_reason END
                 WHERE id = ?1",
                params![thread_id, now, by, person, reason],
            )?;
            // Who read a row before the undelivered ones go: told, alone,
            // when none is left.
            let readers = deliveries_in(tx, &[thread_id.to_string()], false)?
                .remove(thread_id)
                .map(|d| d.people())
                .unwrap_or_default();
            let deleted = tx.execute(
                "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL",
                params![thread_id],
            )?;
            let linked = then(tx, &t)?;
            if t.status != "resolved" {
                let mut rec = resolve_record(&now, &t.artifact_id, thread_id, Some(by), linked);
                if let Some(r) = reason {
                    rec = rec.with("reason", r);
                }
                self.record_audit(tx, ctx, rec)?;
            }
            // Its open approval requests go with its held rows (spec
            // 2026-10-08 §9.2).
            super::approvals::withdraw_thread_in(
                self,
                tx,
                ctx,
                thread_id,
                "thread_resolved",
                &now,
            )?;
            if let Some(n) = linked.filter(|_| t.status == "resolved") {
                let rec = addressed_record(&now, &t.artifact_id, thread_id, n);
                self.record_audit(tx, ctx, rec)?;
            }
            let mut touched = clax_core::feedback::Touched::default();
            if deleted > 0 {
                touched
                    .threads
                    .insert((t.artifact_id, thread_id.to_string()));
                touched.withdrawn.insert(thread_id.to_string(), readers);
            }
            Ok(touched)
        }
    }

    /// Reopens a thread: status `open`, `resolved_at` and `resolved_by`
    /// cleared (and a report's `verified` phase and resolve reason),
    /// recorded as `thread.reopen` under `ctx` in the same transaction.
    /// Reopening an open thread records nothing.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone.
    pub fn reopen_thread(&self, ctx: &AuditCtx, thread_id: &str) -> Result<Thread> {
        self.with_tx(|tx| {
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "UPDATE threads SET status = 'open', resolved_at = NULL, resolved_by = NULL,
                        resolved_by_person = NULL, report_phase = NULLIF(report_phase, 'verified'),
                        resolve_reason = NULL WHERE id = ?1",
                params![thread_id],
            )?;
            if t.status == "resolved" {
                let rec = thread_record(
                    AuditKind::ThreadReopen,
                    &self.now(),
                    &t.artifact_id,
                    thread_id,
                );
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(())
        })?;
        self.get_thread(thread_id)?.ok_or(CoreError::NotFound)
    }

    /// Deletes a thread with its comments and feedback rows in one
    /// transaction, then its clip file (a missing file is fine; any other
    /// removal failure is logged and leaves an unreferenced file). Returns the
    /// thread as it was.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone.
    pub fn delete_thread(&self, ctx: &AuditCtx, thread_id: &str) -> Result<Thread> {
        self.delete_thread_touched(ctx, thread_id).map(|(t, _)| t)
    }

    /// [`Store::delete_thread_touched`], awaiting its content store.
    pub async fn delete_thread_touched_async(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
    ) -> Result<(Thread, clax_core::Touched)> {
        let (t, targets) = self.delete_thread_rows(ctx, thread_id)?;
        if t.has_clip {
            let id = ArtifactId::parse(&t.artifact_id)?;
            if let Err(e) = self.content.delete_clip(&id, &t.id).await {
                tracing::warn!(
                    artifact = id.as_str(),
                    thread = t.id,
                    error = %e,
                    "could not remove a deleted thread's clip"
                );
            }
        }
        Ok((
            t,
            clax_core::Touched {
                targets,
                ..Default::default()
            },
        ))
    }

    /// [`Store::delete_thread`], also returning what it changed: `targets`
    /// are the sessions that held undelivered rows of the thread (their
    /// waits and pushes learn the rows are gone); `threads` is empty, since a
    /// deleted thread has no feedback state. Recorded as `thread.delete`
    /// (`moved: false`) under `ctx`, in the same transaction.
    pub fn delete_thread_touched(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
    ) -> Result<(Thread, clax_core::feedback::Touched)> {
        self.block_on(self.delete_thread_touched_async(ctx, thread_id))
    }

    /// Deletes thread `thread_id`'s rows, recording `thread.delete`; returns
    /// the thread as it was and the sessions that held undelivered rows of
    /// it. A linked thread's pairing stays, ended (`unlinked_at`), so its
    /// twin on the remote is not written here again; its comments'
    /// pairings and its unsent writes go. A hosted daemon's `comment_keys`
    /// rows stay: a resend under a key that made the thread finds it gone
    /// rather than making it again.
    fn delete_thread_rows(
        &self,
        ctx: &AuditCtx,
        thread_id: &str,
    ) -> Result<(Thread, std::collections::BTreeSet<String>)> {
        self.with_tx(|tx| {
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            let targets = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT target_session_id FROM feedback
                     WHERE thread_id = ?1 AND delivered_at IS NULL AND target_session_id IS NOT NULL",
                )?;
                stmt.query_map(params![thread_id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?
            };
            tx.execute("DELETE FROM version_threads WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM live_pending WHERE thread_id = ?1", params![thread_id])?;
            tx.execute(
                "DELETE FROM mentions WHERE comment_id IN (SELECT id FROM comments WHERE thread_id = ?1)",
                params![thread_id],
            )?;
            tx.execute("DELETE FROM viewer_threads WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM feedback WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM batch_threads WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM thread_moves WHERE thread_id = ?1", params![thread_id])?;
            tx.execute(
                "DELETE FROM page_authors WHERE kind = 'comment'
                 AND id IN (SELECT id FROM comments WHERE thread_id = ?1)",
                params![thread_id],
            )?;
            tx.execute("DELETE FROM comments WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM bot_follow_ups WHERE thread_id = ?1", params![thread_id])?;
            self.thread_going_tx(tx, thread_id)?;
            tx.execute("DELETE FROM threads WHERE id = ?1", params![thread_id])?;
            let now = self.now();
            tx.execute(
                "UPDATE linked_threads SET unlinked_at = COALESCE(unlinked_at, ?2) WHERE thread_id = ?1",
                params![thread_id, now],
            )?;
            tx.execute("DELETE FROM linked_comments WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM linked_outbox WHERE thread_id = ?1", params![thread_id])?;
            let rec = thread_record(AuditKind::ThreadDelete, &now, &t.artifact_id, thread_id)
                .with("moved", false);
            self.record_audit(tx, ctx, rec)?;
            super::approvals::withdraw_thread_in(self, tx, ctx, thread_id, "thread_deleted", &now)?;
            Ok((t, targets))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::DAEMON;
    use crate::test_util::{anchor, artifact, store};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-body";

    fn new_thread(body: &str, clip: Option<Vec<u8>>) -> NewThread {
        NewThread {
            author_public_id: None,
            version_n: 1,
            anchor: anchor(),
            author_name: "Alex".into(),
            body: body.into(),
            clip,
            via_page: false,
        }
    }

    /// The kinds recorded after `seq`, and the newest `seq`.
    fn kinds_after(st: &Store, seq: i64) -> (Vec<String>, i64) {
        let rows = st.events_after(seq, 1000).unwrap();
        let last = rows.last().map_or(seq, |r| r.seq);
        (rows.into_iter().map(|r| r.kind).collect(), last)
    }

    #[test]
    fn thread_changes_record_one_event_each_and_no_change_records_none() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (_, seq) = kinds_after(&st, 0);
        let t = st
            .create_thread(DAEMON, &id, new_thread("hi", None))
            .unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert_eq!(k, ["thread.open", "comment.add"]);
        let viewer = |body: &str| NewComment {
            author_kind: AUTHOR_VIEWER,
            author_name: "Sam".into(),
            author_public_id: None,
            via_session_id: None,
            body: body.into(),
            via_page: false,
        };
        assert!(st.add_comment(DAEMON, &t.id, viewer("  ")).is_err());
        assert!(
            st.add_comment(DAEMON, "01JB0000000000000000000000", viewer("x"))
                .is_err()
        );
        let (k, seq) = kinds_after(&st, seq);
        assert!(k.is_empty(), "{k:?}");
        st.resolve_thread(DAEMON, &t.id, "viewer:anonymous")
            .unwrap();
        st.resolve_thread(DAEMON, &t.id, "viewer:anonymous")
            .unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert_eq!(k, ["thread.resolve"]);
        st.reopen_thread(DAEMON, &t.id).unwrap();
        st.reopen_thread(DAEMON, &t.id).unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert_eq!(k, ["thread.reopen"]);
        st.resolve_thread(DAEMON, &t.id, "viewer:anonymous")
            .unwrap();
        st.add_comment(DAEMON, &t.id, viewer("again")).unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert_eq!(k, ["thread.resolve", "comment.add", "thread.reopen"]);
        // An agent's resolve carries its link in `thread.resolve`; on a
        // thread already resolved it records the link alone.
        st.resolve_thread_addressed(
            DAEMON,
            &id,
            &t.id,
            "agent:claude",
            "claude",
            false,
            Some("*owner"),
        )
        .unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert_eq!(k, ["thread.resolve"]);
        let resolved = st.events_after(seq - 1, 1).unwrap();
        let body: serde_json::Value = serde_json::from_str(&resolved[0].body).unwrap();
        assert_eq!(body["addressed_version"], 1);
        st.reopen_thread(DAEMON, &t.id).unwrap();
        st.resolve_thread(DAEMON, &t.id, "viewer:anonymous")
            .unwrap();
        st.exec
            .write(|c| {
                c.execute("DELETE FROM version_threads", [])?;
                Ok(())
            })
            .unwrap();
        let (_, seq) = kinds_after(&st, seq);
        st.resolve_thread_addressed(
            DAEMON,
            &id,
            &t.id,
            "agent:claude",
            "claude",
            false,
            Some("*owner"),
        )
        .unwrap();
        let rows = st.events_after(seq, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "thread.addressed");
        let body: serde_json::Value = serde_json::from_str(&rows[0].body).unwrap();
        assert_eq!(
            (&body["version_n"], &body["source"]),
            (&1.into(), &"resolve".into())
        );
        let seq = rows[0].seq;
        st.resolve_thread_addressed(
            DAEMON,
            &id,
            &t.id,
            "agent:claude",
            "claude",
            false,
            Some("*owner"),
        )
        .unwrap();
        let (k, seq) = kinds_after(&st, seq);
        assert!(k.is_empty(), "already linked: {k:?}");
        st.delete_thread(DAEMON, &t.id).unwrap();
        let rows = st.events_after(seq, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "thread.delete");
        assert_eq!(rows[0].ids.thread.as_deref(), Some(t.id.as_str()));
        assert_eq!(rows[0].ids.artifact.as_deref(), Some(id.as_str()));
        let body: serde_json::Value = serde_json::from_str(&rows[0].body).unwrap();
        assert_eq!(
            (&body["moved"], &body["via"]),
            (&false.into(), &"daemon".into())
        );
    }

    #[test]
    fn create_thread_stores_anchor_first_comment_and_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(
                DAEMON,
                &aid,
                new_thread("Make this two columns.", Some(PNG.to_vec())),
            )
            .unwrap();
        assert_eq!(t.artifact_id, aid.as_str());
        assert_eq!(t.version_n, 1);
        assert_eq!(t.status, "open");
        assert!(!t.sent_to_agent);
        assert!(t.has_clip);
        assert_eq!(t.anchor, anchor());
        assert_eq!(t.comments.len(), 1);
        assert_eq!(t.comments[0].author_kind, AUTHOR_VIEWER);
        assert_eq!(t.comments[0].author_name, "Alex");
        assert_eq!(st.read_clip(&aid, &t.id).unwrap().unwrap(), PNG);
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap(), t);
    }

    #[test]
    fn anchors_name_a_file_of_the_thread_version() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let req: clax_core::publish::PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": 1,
            "files": {
                "index.html": {"content": "<a href=about.html>about</a>", "encoding": "utf8"},
                "about.html": {"content": "<h2>About</h2>", "encoding": "utf8"}
            }
        }))
        .unwrap();
        st.publish_version(
            DAEMON,
            &aid,
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap();
        let on = |n: u32, file: &str| {
            let mut nt = new_thread("x", None);
            nt.version_n = n;
            nt.anchor.file = file.into();
            st.create_thread(DAEMON, &aid, nt)
        };
        let t = on(2, "about.html").unwrap();
        assert_eq!(t.anchor.file, "about.html");
        assert_eq!(
            st.get_thread(&t.id).unwrap().unwrap().anchor.file,
            "about.html"
        );
        for (n, file) in [(1, "about.html"), (2, "missing.html"), (2, "../about.html")] {
            let e = on(n, file).unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    }
                ),
                "v{n} {file}: {e:?}"
            );
        }
        assert_eq!(on(1, "index.html").unwrap().anchor.file, "index.html");
    }

    #[test]
    fn unknown_version_is_refused_and_writes_no_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let mut nt = new_thread("x", Some(PNG.to_vec()));
        nt.version_n = 9;
        let e = st.create_thread(DAEMON, &aid, nt).unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "unknown_version",
                    ..
                }
            ),
            "{e:?}"
        );
        assert!(
            !st.home().clips_dir(&aid).exists()
                || std::fs::read_dir(st.home().clips_dir(&aid))
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[test]
    fn empty_and_oversized_bodies_are_refused() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        for body in ["", "   \n", &"x".repeat(MAX_BODY_CHARS + 1)] {
            let e = st
                .create_thread(DAEMON, &aid, new_thread(body, None))
                .unwrap_err();
            assert!(matches!(
                e,
                CoreError::Invalid {
                    code: "invalid_comment",
                    ..
                }
            ));
        }
    }

    #[test]
    fn clip_problem_accepts_png_up_to_the_cap() {
        assert_eq!(clip_problem(PNG), None);
        assert!(clip_problem(b"GIF89a").is_some());
        let mut big = PNG.to_vec();
        big.resize(MAX_CLIP_BYTES + 1, 0);
        assert!(clip_problem(&big).is_some());
    }

    #[test]
    fn list_pages_in_creation_order_and_hides_resolved_unless_asked() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let ids: Vec<String> = (0..5)
            .map(|i| {
                st.create_thread(DAEMON, &aid, new_thread(&format!("c{i}"), None))
                    .unwrap()
                    .id
            })
            .collect();
        st.resolve_thread(DAEMON, &ids[1], "viewer:x").unwrap();
        let (open, next) = st.list_threads(&aid, false, None, 50).unwrap();
        assert_eq!(
            open.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            [&ids[0], &ids[2], &ids[3], &ids[4]].map(String::clone)
        );
        assert_eq!(next, None);
        let (page1, next) = st.list_threads(&aid, true, None, 2).unwrap();
        assert_eq!(page1.len(), 2);
        let (page2, _) = st.list_threads(&aid, true, next.as_deref(), 2).unwrap();
        assert_eq!(page2[0].id, ids[2]);
        assert_eq!(page1[1].comments[0].body, "c1");
    }

    #[test]
    fn resolve_keeps_the_first_resolution_and_a_viewer_comment_reopens() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(DAEMON, &aid, new_thread("x", None))
            .unwrap();
        let r1 = st.resolve_thread(DAEMON, &t.id, "viewer:a").unwrap();
        let r2 = st.resolve_thread(DAEMON, &t.id, "agent:s").unwrap();
        assert_eq!(r1.status, "resolved");
        assert_eq!(r2.resolved_by.as_deref(), Some("viewer:a"));
        assert_eq!(r2.resolved_at, r1.resolved_at);
        st.add_comment(
            DAEMON,
            &t.id,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_AGENT,
                author_name: "claude".into(),
                via_session_id: None,
                body: "done".into(),
                via_page: false,
            },
        )
        .unwrap();
        assert_eq!(
            st.get_thread(&t.id).unwrap().unwrap().status,
            "resolved",
            "agent replies do not reopen"
        );
        st.add_comment(
            DAEMON,
            &t.id,
            NewComment {
                author_public_id: None,
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: "not quite".into(),
                via_page: false,
            },
        )
        .unwrap();
        let reopened = st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(reopened.status, "open");
        assert_eq!(reopened.resolved_at, None);
        assert_eq!(reopened.comments.len(), 3);
    }

    #[test]
    fn agent_comments_name_the_harness_never_the_session() {
        let (_d, st) = store();
        let owner = st.owner_viewer(false).unwrap().public_id;
        let sid = crate::test_util::session(&st, "codex", "cx");
        let aid = artifact(&st, None);
        let t = st
            .create_thread(DAEMON, &aid, new_thread("x", None))
            .unwrap();
        let c = st
            .add_comment(
                DAEMON,
                &t.id,
                NewComment {
                    author_public_id: None,
                    author_kind: AUTHOR_AGENT,
                    author_name: "codex".into(),
                    via_session_id: Some(sid.clone()),
                    body: "done".into(),
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(c.via_harness.as_deref(), Some("codex"));
        let got = st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(got.comments[0].via_harness, None, "viewer comments");
        assert_eq!(got.comments[1].via_harness.as_deref(), Some("codex"));
        let json = serde_json::to_string(&got).unwrap() + &serde_json::to_string(&c).unwrap();
        assert!(!json.contains(&sid), "{json}");
        assert!(!json.contains("via_session_id"), "{json}");
        // Its agent, for people's author lines (spec §11 "Display"): the
        // session's name and person, never the session.
        let name = st.get_session(&sid).unwrap().unwrap().name.unwrap();
        let _ = owner;
        // The owner's agents name no person, as `thread.owner` does.
        let want = clax_core::model::CommentAgent {
            name,
            person: None,
            person_name: "The owner".into(),
        };
        assert_eq!(c.agent.as_ref(), Some(&want));
        assert_eq!(got.comments[1].agent.as_ref(), Some(&want));
        assert_eq!(got.comments[0].agent, None);
        let v = serde_json::to_value(&got.comments[1]).unwrap();
        assert_eq!(v["agent"]["person_name"], "The owner", "{v}");
        assert!(
            serde_json::to_value(&got.comments[0])
                .unwrap()
                .get("agent")
                .is_none()
        );
    }

    /// A comment's agent's person is named as stored, for browsers
    /// (agents read the payload-safe author line instead).
    #[test]
    fn a_comment_agents_person_is_named_as_stored() {
        let a = comment_agent(
            Some("web".into()),
            Some("u_s".into()),
            Some("Dr: Who".into()),
        )
        .unwrap();
        assert_eq!(
            (a.person.as_deref(), a.person_name.as_str()),
            (Some("u_s"), "Dr: Who")
        );
        let o = comment_agent(Some("web".into()), None, None).unwrap();
        assert_eq!((o.person, o.person_name.as_str()), (None, "The owner"));
        assert!(comment_agent(None, None, None).is_none());
    }

    #[test]
    fn threads_of_deleted_artifacts_are_not_found() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(DAEMON, &aid, new_thread("x", None))
            .unwrap();
        st.delete_artifact(DAEMON, &aid).unwrap();
        assert_eq!(st.get_thread(&t.id).unwrap(), None);
        assert!(matches!(
            st.list_threads(&aid, true, None, 10),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.resolve_thread(DAEMON, &t.id, "viewer:a"),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.create_thread(DAEMON, &aid, new_thread("y", None)),
            Err(CoreError::NotFound)
        ));
    }

    fn clip_files(st: &Store, aid: &ArtifactId) -> usize {
        std::fs::read_dir(st.home().clips_dir(aid)).map_or(0, |d| d.count())
    }

    fn thread_rows(st: &Store) -> i64 {
        st.with_read(|c| Ok(c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn corrupt_anchor_rows_are_skipped_in_lists_and_named_on_lookup() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let ids: Vec<String> = (0..4)
            .map(|i| {
                st.create_thread(DAEMON, &aid, new_thread(&format!("c{i}"), None))
                    .unwrap()
                    .id
            })
            .collect();
        st.with_write(|c| {
            c.execute(
                "UPDATE threads SET anchor_json = '{' WHERE id = ?1",
                [&ids[1]],
            )?;
            Ok(())
        })
        .unwrap();
        let (all, next) = st.list_threads(&aid, true, None, 50).unwrap();
        assert_eq!(
            all.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            [&ids[0], &ids[2], &ids[3]].map(String::clone)
        );
        assert_eq!(next, None);
        let (page1, next) = st.list_threads(&aid, true, None, 2).unwrap();
        assert_eq!(
            page1.len(),
            1,
            "the corrupt row counts against the page and is left out"
        );
        assert_eq!(page1[0].id, ids[0]);
        assert_eq!(next.as_deref(), Some(ids[1].as_str()));
        let (page2, next) = st.list_threads(&aid, true, next.as_deref(), 2).unwrap();
        assert_eq!(
            page2.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            [&ids[2], &ids[3]].map(String::clone)
        );
        assert_eq!(next, None);
        assert!(matches!(
            st.get_thread(&ids[1]),
            Err(CoreError::Corrupt {
                column: "anchor_json",
                ..
            })
        ));
    }

    #[test]
    fn thread_on_a_deleted_artifact_writes_no_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        st.delete_artifact(DAEMON, &aid).unwrap();
        let e = st
            .create_thread(DAEMON, &aid, new_thread("x", Some(PNG.to_vec())))
            .unwrap_err();
        assert!(matches!(e, CoreError::NotFound), "{e:?}");
        assert_eq!(clip_files(&st, &aid), 0);
        assert_eq!(thread_rows(&st), 0);
        assert!(
            !st.home().artifact_dir(&aid).exists(),
            "no clips directory is recreated"
        );
    }

    #[test]
    fn a_clip_racing_a_delete_leaves_no_directory() {
        let (_d, st) = store();
        for _ in 0..30 {
            let aid = artifact(&st, None);
            std::thread::scope(|s| {
                s.spawn(|| {
                    let _ = st.create_thread(DAEMON, &aid, new_thread("x", Some(PNG.to_vec())));
                });
                s.spawn(|| st.delete_artifact(DAEMON, &aid).unwrap());
            });
            assert!(
                !st.home().artifact_dir(&aid).exists(),
                "the deleted artifact's directory is gone"
            );
        }
    }

    #[test]
    fn failed_insert_writes_no_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        st.with_write(|c| {
            c.execute_batch(
                "CREATE TRIGGER refuse BEFORE INSERT ON comments BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            st.create_thread(DAEMON, &aid, new_thread("x", Some(PNG.to_vec())))
                .is_err()
        );
        assert_eq!(clip_files(&st, &aid), 0);
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn failed_commit_removes_the_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        // A deferred foreign-key violation passes every statement and fails the commit.
        st.with_write(|c| {
            c.execute_batch(
                "CREATE TABLE parent (id TEXT PRIMARY KEY);
                 CREATE TABLE child (p TEXT REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);
                 CREATE TRIGGER orphan AFTER INSERT ON comments BEGIN INSERT INTO child VALUES ('none'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            st.create_thread(DAEMON, &aid, new_thread("x", Some(PNG.to_vec())))
                .is_err()
        );
        assert_eq!(clip_files(&st, &aid), 0);
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn unwritable_clip_leaves_no_thread() {
        // The home's files, where a file in the clips directory's place
        // makes the clip's write fail.
        let (_d, st) = crate::test_util::pool_store();
        let aid = artifact(&st, None);
        std::fs::write(st.home().clips_dir(&aid), b"not a directory").unwrap();
        assert!(matches!(
            st.create_thread(DAEMON, &aid, new_thread("x", Some(PNG.to_vec()))),
            Err(CoreError::Io(_))
        ));
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn unknown_cursor_is_refused() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        st.create_thread(DAEMON, &aid, new_thread("x", None))
            .unwrap();
        let e = st
            .list_threads(&aid, true, Some(&new_ulid()), 10)
            .unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "invalid_cursor",
                    ..
                }
            ),
            "{e:?}"
        );
    }

    #[test]
    fn unknown_author_kind_is_refused() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(DAEMON, &aid, new_thread("x", None))
            .unwrap();
        let e = st
            .add_comment(
                DAEMON,
                &t.id,
                NewComment {
                    author_public_id: None,
                    author_kind: "robot",
                    author_name: "r".into(),
                    via_session_id: None,
                    body: "hi".into(),
                    via_page: false,
                },
            )
            .unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "invalid_author_kind",
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().comments.len(), 1);
    }

    #[test]
    fn reopen_clears_the_resolution_and_delete_removes_everything() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let t = st
            .create_thread(
                DAEMON,
                &id,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Alex".into(),
                    body: "b".into(),
                    clip: Some(b"\x89PNG\r\n\x1a\nclip".to_vec()),
                    via_page: false,
                },
            )
            .unwrap();
        st.resolve_thread(DAEMON, &t.id, "viewer:u_00000000000000000000aa")
            .unwrap();
        let r = st.reopen_thread(DAEMON, &t.id).unwrap();
        assert_eq!(
            (
                r.status.as_str(),
                r.resolved_at.clone(),
                r.resolved_by.clone()
            ),
            ("open", None, None)
        );
        assert_eq!(
            st.reopen_thread(DAEMON, &t.id).unwrap().status,
            "open",
            "reopening an open thread changes nothing"
        );
        st.send_to_agent(DAEMON, &t.id).unwrap();
        let clip = || crate::test_util::content_clip(&st, &id, &t.id);
        assert!(clip().is_some());
        let gone = st.delete_thread(DAEMON, &t.id).unwrap();
        assert_eq!(gone.id, t.id);
        assert_eq!(st.get_thread(&t.id).unwrap(), None);
        assert!(clip().is_none());
        let left: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM comments) + (SELECT COUNT(*) FROM feedback)",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0);
        assert!(matches!(
            st.delete_thread(DAEMON, &t.id),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.reopen_thread(DAEMON, &t.id),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn deleting_names_the_sessions_whose_rows_vanished_and_page_comments_are_marked() {
        let (_d, st) = store();
        let sid = crate::test_util::session(&st, "claude", "h1");
        let id = artifact(&st, Some(&sid));
        let mut nt = new_thread("from the page", None);
        nt.via_page = true;
        let t = st.create_thread(DAEMON, &id, nt).unwrap();
        assert!(t.comments[0].via_page);
        let c = st
            .add_comment(
                DAEMON,
                &t.id,
                NewComment {
                    author_public_id: None,
                    author_kind: AUTHOR_VIEWER,
                    author_name: "Alex".into(),
                    via_session_id: None,
                    body: "typed by hand".into(),
                    via_page: false,
                },
            )
            .unwrap();
        assert!(!c.via_page);
        let agent = st
            .add_comment(
                DAEMON,
                &t.id,
                NewComment {
                    author_public_id: None,
                    author_kind: AUTHOR_AGENT,
                    author_name: "claude".into(),
                    via_session_id: None,
                    body: "on it".into(),
                    via_page: true,
                },
            )
            .unwrap();
        assert!(!agent.via_page, "only viewer comments are page-written");
        let stored = st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(
            stored
                .comments
                .iter()
                .map(|c| c.via_page)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        st.send_to_agent(DAEMON, &t.id).unwrap();
        let (_, touched) = st.delete_thread_touched(DAEMON, &t.id).unwrap();
        assert_eq!(touched.targets.into_iter().collect::<Vec<_>>(), [sid]);
        assert!(touched.threads.is_empty());
    }
}
