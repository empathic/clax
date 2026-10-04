//! Comment threads and their comments. A thread is anchored to one version of
//! a live artifact and starts with the viewer comment that created it; its
//! optional clip is stored at `artifacts/<aid>/clips/<tid>.png`.

use super::Store;
use super::batches::ThreadSend;
use super::feedback::{feedback_states_in, id_array};
use crate::anchor::Anchor;
use crate::feedback::FeedbackState;
use crate::model::{Comment, Thread};
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashMap;

/// Largest accepted clip, in bytes.
pub const MAX_CLIP_BYTES: usize = 5 * 1024 * 1024;
/// Longest accepted comment body, in characters.
pub const MAX_BODY_CHARS: usize = 10_000;
/// Threads per page when a caller gives no limit.
pub const DEFAULT_THREAD_PAGE: usize = 50;
pub const AUTHOR_VIEWER: &str = "viewer";
pub const AUTHOR_AGENT: &str = "agent";
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

fn check_body(body: &str) -> Result<()> {
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

/// Threads of live artifacts; callers append `AND ...` conditions.
pub(crate) const THREAD_SELECT: &str =
    "SELECT t.id, t.artifact_id, t.version_n, t.anchor_json, t.status,
    t.sent_to_agent, t.has_clip, t.created_at, t.resolved_at, t.resolved_by
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
/// The batches that sent each thread of `?1` (a JSON array), oldest first.
pub(crate) const SENDS_OF_MANY: &str =
    "SELECT bt.thread_id, b.id, b.size, b.note, b.sent_by, b.created_at
    FROM json_each(?1) j
    CROSS JOIN batch_threads bt ON bt.thread_id = j.value
    JOIN send_batches b ON b.id = bt.batch_id
    ORDER BY bt.thread_id, b.created_at, b.id";
/// The display name of each viewer of `?1` (a JSON array of public IDs).
pub(crate) const NAMES_OF_MANY: &str = "SELECT v.public_id, v.display_name FROM json_each(?1) j
    CROSS JOIN viewers v INDEXED BY viewers_public_id ON v.public_id = j.value";

/// A page of threads of artifact `?1` (resolved ones too when `?2`) after
/// the thread `?3` (`NULL`: from the start), at most `?4`.
pub(crate) fn list_threads_sql() -> String {
    format!(
        "{THREAD_SELECT} AND t.artifact_id = ?1 AND (?2 OR t.status = 'open')
         AND (?3 IS NULL OR (t.created_at, t.id) > (SELECT created_at, id FROM threads WHERE id = ?3))
         ORDER BY t.created_at, t.id LIMIT ?4"
    )
}

/// What a thread view adds to a stored thread; see [`Store::thread_extras`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadExtras {
    pub feedback_state: Option<FeedbackState>,
    pub addressed_in: Vec<u32>,
    pub sends: Vec<ThreadSend>,
    pub resolved_by_name: Option<String>,
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
    })
}

/// Comments with their author's session harness; callers append `WHERE ...`.
const COMMENT_SELECT: &str = "SELECT c.id, c.thread_id, c.author_kind, c.author_name, c.author_public_id, s.harness AS via_harness, c.via_page, c.body, c.created_at
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
pub(crate) const COMMENTS_OF_MANY: &str = "SELECT c.id, c.thread_id, c.author_kind, c.author_name, c.author_public_id, s.harness AS via_harness, c.via_page, c.body, c.created_at
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

/// The thread `thread_id` of a live artifact, with its comments.
pub(crate) fn thread_in(c: &Connection, thread_id: &str) -> Result<Option<Thread>> {
    let row = c
        .query_row(
            &format!("{THREAD_SELECT} AND t.id = ?1"),
            params![thread_id],
            row_to_thread_row,
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some(row) => {
            let comments = load_comments(c, &row.id)?;
            row.into_thread(comments).map(Some)
        }
    }
}

pub(crate) fn artifact_live(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0)",
        params![id],
        |r| r.get(0),
    )?)
}

/// Records the viewers `body` mentions (see [`crate::mentions::mentioned`])
/// for the comment `comment_id`, in the caller's transaction.
fn insert_mentions(tx: &Connection, comment_id: &str, body: &str) -> Result<()> {
    let names: Vec<(String, String)> = {
        let mut st = tx.prepare(
            "SELECT public_id, display_name FROM viewers WHERE display_name IS NOT NULL AND display_name != ''",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for p in crate::mentions::mentioned(body, &names) {
        tx.execute(
            "INSERT OR IGNORE INTO mentions (comment_id, public_id) VALUES (?1, ?2)",
            params![comment_id, p],
        )?;
    }
    Ok(())
}

/// Writes `bytes` to `path` via a temporary file in `dir`, creating `dir`.
fn write_clip(dir: &std::path::Path, path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("png.tmp");
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

impl Store {
    /// Creates a thread on version `t.version_n` of the live artifact `id` with
    /// its first (viewer) comment. Liveness, the version, the rows, and the
    /// clip are checked and written in one transaction; when it fails, no rows
    /// remain and no clip file is left behind. Callers check the clip with
    /// [`clip_problem`] first.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `invalid_anchor` (also
    /// when the version holds no file at `anchor.file`), `invalid_comment`, or
    /// `unknown_version` for bad input.
    pub fn create_thread(&self, id: &ArtifactId, t: NewThread) -> Result<Thread> {
        t.anchor.validate()?;
        check_body(&t.body)?;
        let tid = new_ulid();
        let now = Store::now();
        let clip_path = self.home.clip_path(id, &tid);
        let anchor_json = serde_json::to_string(&t.anchor).expect("anchors serialise");
        let inserted = self.with_tx(|tx| {
            if !artifact_live(tx, id.as_str())? {
                return Err(CoreError::NotFound);
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

            if !files.contains_key(&t.anchor.file) {
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
            let cid = new_ulid();
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
                 VALUES (?1, ?2, 'viewer', ?3, ?4, NULL, ?5, ?6, ?7)",
                params![cid, tid, t.author_name, t.author_public_id, t.via_page, t.body, now],
            )?;
            insert_mentions(tx, &cid, &t.body)?;
            // Written last, still inside the transaction: a failed write rolls
            // the rows back, and a failed commit removes the file below.
            if let Some(bytes) = &t.clip {
                write_clip(&self.home.clips_dir(id), &clip_path, bytes)?;
            }
            Ok(())
        });
        if let Err(e) = inserted {
            if t.clip.is_some() {
                let _ = std::fs::remove_file(&clip_path);
            }
            return Err(e);
        }
        self.get_thread(&tid)?.ok_or(CoreError::NotFound)
    }

    /// Adds a comment. A viewer comment on a resolved thread reopens it; an
    /// agent comment never changes the thread's status.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone; `invalid_author_kind`
    /// or `invalid_comment` for bad input.
    pub fn add_comment(&self, thread_id: &str, c: NewComment) -> Result<Comment> {
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
            created_at: Store::now(),
        };
        self.with_tx(|tx| {
            thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, via_page, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![comment.id, comment.thread_id, comment.author_kind, comment.author_name, comment.author_public_id, c.via_session_id, comment.via_page, comment.body, comment.created_at],
            )?;
            comment.via_harness = match &c.via_session_id {
                Some(sid) => tx
                    .query_row("SELECT harness FROM sessions WHERE id = ?1", params![sid], |r| r.get(0))
                    .optional()?,
                None => None,
            };
            if comment.author_kind == AUTHOR_VIEWER {
                insert_mentions(tx, &comment.id, &comment.body)?;
                tx.execute(
                    "UPDATE threads SET status = 'open', resolved_at = NULL, resolved_by = NULL WHERE id = ?1",
                    params![thread_id],
                )?;
            }
            Ok(())
        })?;
        Ok(comment)
    }

    /// The thread with its comments, or `None` when it or its artifact is gone.
    ///
    /// # Errors
    /// `Corrupt` when its `anchor_json` does not parse.
    pub fn get_thread(&self, thread_id: &str) -> Result<Option<Thread>> {
        self.with_conn(|c| thread_in(c, thread_id))
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
        self.with_conn(|c| {
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
        self.with_conn(|c| {
            let mut stmt = c.prepare_cached(THREAD_STATUSES)?;
            let rows = stmt.query_map(params![id.as_str(), id_array(thread_ids)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What a thread view adds to each of `threads`, in the same order: the
    /// feedback state ([`Store::feedback_state`]), the versions addressing it
    /// ([`Store::addressed_in`]), its sends ([`Store::thread_sends`]), and
    /// the current display name of the viewer named by `resolved_by`.
    pub fn thread_extras(&self, threads: &[Thread], codex_push: bool) -> Result<Vec<ThreadExtras>> {
        if threads.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
        let resolvers: Vec<String> = threads
            .iter()
            .filter_map(|t| t.resolved_by.as_deref()?.strip_prefix("viewer:"))
            .filter(|p| crate::is_public_id(p))
            .map(str::to_string)
            .collect();
        self.with_conn(|c| {
            let mut states = feedback_states_in(c, &ids, codex_push)?;
            let arr = id_array(&ids);
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
                });
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
                    feedback_state: states.remove(&t.id),
                    addressed_in: addressed.remove(&t.id).unwrap_or_default(),
                    sends: sends.remove(&t.id).unwrap_or_default(),
                    resolved_by_name: t
                        .resolved_by
                        .as_deref()
                        .and_then(|by| by.strip_prefix("viewer:"))
                        .and_then(|p| names.get(p).cloned().flatten()),
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
    pub fn resolve_thread(&self, thread_id: &str, by: &str) -> Result<Thread> {
        self.resolve_thread_touched(thread_id, by).map(|(t, _)| t)
    }

    /// [`Store::resolve_thread`], also returning what it changed: the thread is
    /// in `threads` when undelivered rows were deleted (its feedback state then
    /// reflects only delivered rows, or is `None` when none remain); `targets`
    /// is always empty, since no session gains rows.
    pub fn resolve_thread_touched(
        &self,
        thread_id: &str,
        by: &str,
    ) -> Result<(Thread, crate::feedback::Touched)> {
        let touched = self.with_tx(|tx| {
            let t = thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "UPDATE threads SET status = 'resolved', resolved_at = COALESCE(resolved_at, ?2),
                    resolved_by = COALESCE(resolved_by, ?3) WHERE id = ?1",
                params![thread_id, Store::now(), by],
            )?;
            let deleted = tx.execute(
                "DELETE FROM feedback WHERE thread_id = ?1 AND delivered_at IS NULL",
                params![thread_id],
            )?;
            let mut touched = crate::feedback::Touched::default();
            if deleted > 0 {
                touched
                    .threads
                    .insert((t.artifact_id, thread_id.to_string()));
            }
            Ok(touched)
        })?;
        Ok((
            self.get_thread(thread_id)?.ok_or(CoreError::NotFound)?,
            touched,
        ))
    }

    /// Reopens a thread: status `open`, `resolved_at` and `resolved_by`
    /// cleared. Reopening an open thread changes nothing.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone.
    pub fn reopen_thread(&self, thread_id: &str) -> Result<Thread> {
        self.with_tx(|tx| {
            thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "UPDATE threads SET status = 'open', resolved_at = NULL, resolved_by = NULL WHERE id = ?1",
                params![thread_id],
            )?;
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
    pub fn delete_thread(&self, thread_id: &str) -> Result<Thread> {
        self.delete_thread_touched(thread_id).map(|(t, _)| t)
    }

    /// [`Store::delete_thread`], also returning what it changed: `targets`
    /// are the sessions that held undelivered rows of the thread (their
    /// waits and pushes learn the rows are gone); `threads` is empty, since a
    /// deleted thread has no feedback state.
    pub fn delete_thread_touched(
        &self,
        thread_id: &str,
    ) -> Result<(Thread, crate::feedback::Touched)> {
        let (t, targets) = self.with_tx(|tx| {
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
            tx.execute(
                "DELETE FROM mentions WHERE comment_id IN (SELECT id FROM comments WHERE thread_id = ?1)",
                params![thread_id],
            )?;
            tx.execute("DELETE FROM viewer_threads WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM feedback WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM batch_threads WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM comments WHERE thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM threads WHERE id = ?1", params![thread_id])?;
            Ok((t, targets))
        })?;
        if t.has_clip {
            let id = ArtifactId::parse(&t.artifact_id)?;
            let path = self.home.clip_path(&id, &t.id);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not remove a deleted thread's clip"
                ),
            }
        }
        Ok((
            t,
            crate::feedback::Touched {
                targets,
                ..Default::default()
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{anchor, artifact, store};

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

    #[test]
    fn create_thread_stores_anchor_first_comment_and_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(
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
        assert_eq!(
            std::fs::read(st.home().clip_path(&aid, &t.id)).unwrap(),
            PNG
        );
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap(), t);
    }

    #[test]
    fn anchors_name_a_file_of_the_thread_version() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let req: crate::publish::PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": 1,
            "files": {
                "index.html": {"content": "<a href=about.html>about</a>", "encoding": "utf8"},
                "about.html": {"content": "<h2>About</h2>", "encoding": "utf8"}
            }
        }))
        .unwrap();
        st.publish_version(&aid, crate::publish::validate(req).unwrap(), None)
            .unwrap();
        let on = |n: u32, file: &str| {
            let mut nt = new_thread("x", None);
            nt.version_n = n;
            nt.anchor.file = file.into();
            st.create_thread(&aid, nt)
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
        let e = st.create_thread(&aid, nt).unwrap_err();
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
            let e = st.create_thread(&aid, new_thread(body, None)).unwrap_err();
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
                st.create_thread(&aid, new_thread(&format!("c{i}"), None))
                    .unwrap()
                    .id
            })
            .collect();
        st.resolve_thread(&ids[1], "viewer:x").unwrap();
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
        let t = st.create_thread(&aid, new_thread("x", None)).unwrap();
        let r1 = st.resolve_thread(&t.id, "viewer:a").unwrap();
        let r2 = st.resolve_thread(&t.id, "agent:s").unwrap();
        assert_eq!(r1.status, "resolved");
        assert_eq!(r2.resolved_by.as_deref(), Some("viewer:a"));
        assert_eq!(r2.resolved_at, r1.resolved_at);
        st.add_comment(
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
        let sid = crate::store::test_util::session(&st, "codex", "cx");
        let aid = artifact(&st, None);
        let t = st.create_thread(&aid, new_thread("x", None)).unwrap();
        let c = st
            .add_comment(
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
    }

    #[test]
    fn threads_of_deleted_artifacts_are_not_found() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = st.create_thread(&aid, new_thread("x", None)).unwrap();
        st.delete_artifact(&aid).unwrap();
        assert_eq!(st.get_thread(&t.id).unwrap(), None);
        assert!(matches!(
            st.list_threads(&aid, true, None, 10),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.resolve_thread(&t.id, "viewer:a"),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.create_thread(&aid, new_thread("y", None)),
            Err(CoreError::NotFound)
        ));
    }

    fn clip_files(st: &Store, aid: &ArtifactId) -> usize {
        std::fs::read_dir(st.home().clips_dir(aid)).map_or(0, |d| d.count())
    }

    fn thread_rows(st: &Store) -> i64 {
        st.with_conn(|c| Ok(c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn corrupt_anchor_rows_are_skipped_in_lists_and_named_on_lookup() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let ids: Vec<String> = (0..4)
            .map(|i| {
                st.create_thread(&aid, new_thread(&format!("c{i}"), None))
                    .unwrap()
                    .id
            })
            .collect();
        st.with_conn(|c| {
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
        st.delete_artifact(&aid).unwrap();
        let e = st
            .create_thread(&aid, new_thread("x", Some(PNG.to_vec())))
            .unwrap_err();
        assert!(matches!(e, CoreError::NotFound), "{e:?}");
        assert_eq!(clip_files(&st, &aid), 0);
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn failed_insert_writes_no_clip() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        st.with_conn(|c| {
            c.execute_batch(
                "CREATE TRIGGER refuse BEFORE INSERT ON comments BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            st.create_thread(&aid, new_thread("x", Some(PNG.to_vec())))
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
        st.with_conn(|c| {
            c.execute_batch(
                "CREATE TABLE parent (id TEXT PRIMARY KEY);
                 CREATE TABLE child (p TEXT REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);
                 CREATE TRIGGER orphan AFTER INSERT ON comments BEGIN INSERT INTO child VALUES ('none'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            st.create_thread(&aid, new_thread("x", Some(PNG.to_vec())))
                .is_err()
        );
        assert_eq!(clip_files(&st, &aid), 0);
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn unwritable_clip_leaves_no_thread() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        std::fs::write(st.home().clips_dir(&aid), b"not a directory").unwrap();
        assert!(matches!(
            st.create_thread(&aid, new_thread("x", Some(PNG.to_vec()))),
            Err(CoreError::Io(_))
        ));
        assert_eq!(thread_rows(&st), 0);
    }

    #[test]
    fn unknown_cursor_is_refused() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        st.create_thread(&aid, new_thread("x", None)).unwrap();
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
        let t = st.create_thread(&aid, new_thread("x", None)).unwrap();
        let e = st
            .add_comment(
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
        st.resolve_thread(&t.id, "viewer:u_00000000000000000000aa")
            .unwrap();
        let r = st.reopen_thread(&t.id).unwrap();
        assert_eq!(
            (
                r.status.as_str(),
                r.resolved_at.clone(),
                r.resolved_by.clone()
            ),
            ("open", None, None)
        );
        assert_eq!(
            st.reopen_thread(&t.id).unwrap().status,
            "open",
            "reopening an open thread changes nothing"
        );
        st.send_to_agent(&t.id).unwrap();
        let clip = st.home().clip_path(&id, &t.id);
        assert!(clip.exists());
        let gone = st.delete_thread(&t.id).unwrap();
        assert_eq!(gone.id, t.id);
        assert_eq!(st.get_thread(&t.id).unwrap(), None);
        assert!(!clip.exists());
        let left: i64 = st
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM comments) + (SELECT COUNT(*) FROM feedback)",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0);
        assert!(matches!(st.delete_thread(&t.id), Err(CoreError::NotFound)));
        assert!(matches!(st.reopen_thread(&t.id), Err(CoreError::NotFound)));
    }

    #[test]
    fn deleting_names_the_sessions_whose_rows_vanished_and_page_comments_are_marked() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        let id = artifact(&st, Some(&sid));
        let mut nt = new_thread("from the page", None);
        nt.via_page = true;
        let t = st.create_thread(&id, nt).unwrap();
        assert!(t.comments[0].via_page);
        let c = st
            .add_comment(
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
        st.send_to_agent(&t.id).unwrap();
        let (_, touched) = st.delete_thread_touched(&t.id).unwrap();
        assert_eq!(touched.targets.into_iter().collect::<Vec<_>>(), [sid]);
        assert!(touched.threads.is_empty());
    }
}
