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
        via_session_id: r.get("via_session_id")?,
        body: r.get("body")?,
        created_at: r.get("created_at")?,
    })
}

fn load_comments(c: &Connection, thread_id: &str) -> Result<Vec<Comment>> {
    let mut stmt = c.prepare(
        "SELECT id, thread_id, author_kind, author_name, via_session_id, body, created_at
         FROM comments WHERE thread_id = ?1 ORDER BY created_at, id",
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

impl Store {
    /// Creates a thread on version `t.version_n` of the live artifact `id` with
    /// its first (viewer) comment. The clip, when given, is written before the
    /// rows and removed again if they cannot be inserted; callers check it
    /// with [`clip_problem`] first.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `invalid_anchor`,
    /// `invalid_comment`, or `unknown_version` for bad input.
    pub fn create_thread(&self, id: &ArtifactId, t: NewThread) -> Result<Thread> {
        t.anchor.validate()?;
        check_body(&t.body)?;
        self.with_conn(|c| {
            if !artifact_live(c, id.as_str())? {
                return Err(CoreError::NotFound);
            }
            let has: bool = c.query_row(
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
            Ok(())
        })?;
        let tid = new_ulid();
        let now = Store::now();
        let clip_path = self.home.clip_path(id, &tid);
        if let Some(bytes) = &t.clip {
            std::fs::create_dir_all(self.home.clips_dir(id))?;
            let tmp = clip_path.with_extension("png.tmp");
            std::fs::write(&tmp, bytes)?;
            if let Err(e) = std::fs::rename(&tmp, &clip_path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
        let anchor_json = serde_json::to_string(&t.anchor).expect("anchors serialise");
        let inserted = self.with_tx(|tx| {
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
    /// `NotFound` when the thread or its artifact is gone; `invalid_comment`.
    pub fn add_comment(&self, thread_id: &str, c: NewComment) -> Result<Comment> {
        check_body(&c.body)?;
        let comment = Comment {
            id: new_ulid(),
            thread_id: thread_id.to_string(),
            author_kind: c.author_kind.to_string(),
            author_name: c.author_name,
            via_session_id: c.via_session_id,
            body: c.body,
            created_at: Store::now(),
        };
        self.with_tx(|tx| {
            thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, via_session_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![comment.id, comment.thread_id, comment.author_kind, comment.author_name, comment.via_session_id, comment.body, comment.created_at],
            )?;
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
    pub fn get_thread(&self, thread_id: &str) -> Result<Option<Thread>> {
        self.with_conn(|c| thread_in(c, thread_id))
    }

    /// Threads of the live artifact `id`, oldest first; resolved ones only with
    /// `include_resolved`. Pages of `limit` start after the thread `cursor`;
    /// the second value is the cursor for the next page, `None` on the last.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact.
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
            for row in rows.into_iter().take(limit) {
                let comments = load_comments(c, &row.id)?;
                threads.push(row.into_thread(comments)?);
            }
            let next = if more { threads.last().map(|t| t.id.clone()) } else { None };
            Ok((threads, next))
        })
    }

    /// Marks the thread resolved by `by` (`viewer:<id>` or `agent:<session>`).
    /// Resolving a resolved thread keeps its first `resolved_at` and `resolved_by`.
    ///
    /// # Errors
    /// `NotFound` when the thread or its artifact is gone.
    pub fn resolve_thread(&self, thread_id: &str, by: &str) -> Result<Thread> {
        self.with_tx(|tx| {
            thread_in(tx, thread_id)?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "UPDATE threads SET status = 'resolved', resolved_at = COALESCE(resolved_at, ?2),
                    resolved_by = COALESCE(resolved_by, ?3) WHERE id = ?1",
                params![thread_id, Store::now(), by],
            )?;
            Ok(())
        })?;
        self.get_thread(thread_id)?.ok_or(CoreError::NotFound)
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
}
