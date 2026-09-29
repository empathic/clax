//! Comment threads and their comments. A thread is anchored to one version of
//! a live artifact and starts with the viewer comment that created it; its
//! optional clip is stored at `artifacts/<aid>/clips/<tid>.png`.

use super::Store;
use crate::anchor::Anchor;
use crate::model::{Comment, Thread};
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, params};

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
    pub body: String,
    pub clip: Option<Vec<u8>>,
}

/// A comment to add. `author_kind` is [`AUTHOR_VIEWER`] or [`AUTHOR_AGENT`].
#[derive(Clone, Debug)]
pub struct NewComment {
    pub author_kind: &'static str,
    pub author_name: String,
    pub via_session_id: Option<String>,
    pub body: String,
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
        via_harness: r.get("via_harness")?,
        body: r.get("body")?,
        created_at: r.get("created_at")?,
    })
}

fn load_comments(c: &Connection, thread_id: &str) -> Result<Vec<Comment>> {
    let mut stmt = c.prepare(
        "SELECT c.id, c.thread_id, c.author_kind, c.author_name, s.harness AS via_harness, c.body, c.created_at
         FROM comments c LEFT JOIN sessions s ON s.id = c.via_session_id
         WHERE c.thread_id = ?1 ORDER BY c.created_at, c.id",
    )?;
    Ok(stmt
        .query_map(params![thread_id], row_to_comment)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
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
    /// `NotFound` for a missing or deleted artifact; `invalid_anchor`,
    /// `invalid_comment`, or `unknown_version` for bad input.
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
            let has: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM versions WHERE artifact_id = ?1 AND n = ?2)",
                params![id.as_str(), t.version_n],
                |r| r.get(0),
            )?;
            if !has {
                return Err(CoreError::invalid(
                    "unknown_version",
                    format!("artifact {id} has no version {}", t.version_n),
                ));
            }
            tx.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, sent_to_agent, has_clip, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'open', 0, ?5, ?6)",
                params![tid, id.as_str(), t.version_n, anchor_json, t.clip.is_some(), now],
            )?;
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, via_session_id, body, created_at)
                 VALUES (?1, ?2, 'viewer', ?3, NULL, ?4, ?5)",
                params![new_ulid(), tid, t.author_name, t.body, now],
            )?;
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
            via_harness: None,
            body: c.body,
            created_at: Store::now(),
        };
        self.with_tx(|tx| {
            thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, via_session_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![comment.id, comment.thread_id, comment.author_kind, comment.author_name, c.via_session_id, comment.body, comment.created_at],
            )?;
            comment.via_harness = match &c.via_session_id {
                Some(sid) => tx
                    .query_row("SELECT harness FROM sessions WHERE id = ?1", params![sid], |r| r.get(0))
                    .optional()?,
                None => None,
            };
            if comment.author_kind == AUTHOR_VIEWER {
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
            let mut stmt = c.prepare(&format!(
                "{THREAD_SELECT} AND t.artifact_id = ?1 AND (?2 OR t.status = 'open')
                 AND (?3 IS NULL OR (t.created_at, t.id) > (SELECT created_at, id FROM threads WHERE id = ?3))
                 ORDER BY t.created_at, t.id LIMIT ?4"
            ))?;
            let rows = stmt
                .query_map(params![id.as_str(), include_resolved, cursor, (limit + 1) as i64], row_to_thread_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let more = rows.len() > limit;
            let mut threads = Vec::with_capacity(limit);
            let mut last = None;
            for row in rows.into_iter().take(limit) {
                let thread_id = row.id.clone();
                let comments = load_comments(c, &thread_id)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{anchor, artifact, store};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-body";

    fn new_thread(body: &str, clip: Option<Vec<u8>>) -> NewThread {
        NewThread {
            version_n: 1,
            anchor: anchor(),
            author_name: "Alex".into(),
            body: body.into(),
            clip,
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
                author_kind: AUTHOR_AGENT,
                author_name: "claude".into(),
                via_session_id: None,
                body: "done".into(),
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
                author_kind: AUTHOR_VIEWER,
                author_name: "Alex".into(),
                via_session_id: None,
                body: "not quite".into(),
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
                    author_kind: AUTHOR_AGENT,
                    author_name: "codex".into(),
                    via_session_id: Some(sid.clone()),
                    body: "done".into(),
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
                    author_kind: "robot",
                    author_name: "r".into(),
                    via_session_id: None,
                    body: "hi".into(),
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
}
