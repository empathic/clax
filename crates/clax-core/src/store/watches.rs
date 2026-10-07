//! Watches: which sessions follow which artifacts, and whether replies are armed.

use super::Store;
use super::threads::artifact_live;
use crate::model::Watch;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

fn row_to_watch(r: &Row<'_>) -> rusqlite::Result<Watch> {
    Ok(Watch {
        session_id: r.get("session_id")?,
        artifact_id: r.get("artifact_id")?,
        replies_armed: r.get::<_, i64>("replies_armed")? != 0,
        created_at: r.get("created_at")?,
    })
}

fn check(c: &Connection, session_id: &str, id: &ArtifactId) -> Result<()> {
    let ended: Option<Option<String>> = c
        .query_row(
            "SELECT ended_at FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    if !matches!(ended, Some(None)) {
        return Err(CoreError::invalid(
            "unknown_session",
            format!("no live session {session_id}"),
        ));
    }
    if !artifact_live(c, id.as_str())? {
        return Err(CoreError::NotFound);
    }
    Ok(())
}

fn fetch(c: &Connection, session_id: &str, id: &ArtifactId) -> Result<Watch> {
    Ok(c.query_row(
        "SELECT session_id, artifact_id, replies_armed, created_at FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
        params![session_id, id.as_str()],
        row_to_watch,
    )?)
}

impl Store {
    /// Creates or updates the watch of live session `session_id` on the live
    /// artifact `id`, setting `replies_armed`. The watch is direct: removing
    /// a scope watch that also covers the artifact keeps it.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session; `NotFound` for a
    /// missing or deleted artifact.
    pub fn watch(&self, session_id: &str, id: &ArtifactId, replies_armed: bool) -> Result<Watch> {
        self.with_tx(|tx| {
            check(tx, session_id, id)?;
            tx.execute(
                "INSERT INTO watches (session_id, artifact_id, replies_armed, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_id, artifact_id) DO UPDATE SET replies_armed = excluded.replies_armed, source = 'direct'",
                params![session_id, id.as_str(), replies_armed, Store::now()],
            )?;
            fetch(tx, session_id, id)
        })
    }

    /// Creates an armed watch unless one exists; an existing watch is returned
    /// unchanged. Used on publish, so republishing never re-arms replies the
    /// agent turned off.
    ///
    /// # Errors
    /// As [`Store::watch`].
    pub fn ensure_watch(&self, session_id: &str, id: &ArtifactId) -> Result<Watch> {
        self.with_tx(|tx| {
            check(tx, session_id, id)?;
            tx.execute(
                "INSERT OR IGNORE INTO watches (session_id, artifact_id, replies_armed, created_at) VALUES (?1, ?2, 1, ?3)",
                params![session_id, id.as_str(), Store::now()],
            )?;
            fetch(tx, session_id, id)
        })
    }

    /// Removes the watch; true when one existed.
    pub fn unwatch(&self, session_id: &str, id: &ArtifactId) -> Result<bool> {
        self.with_tx(|c| {
            Ok(c.execute(
                "DELETE FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
                params![session_id, id.as_str()],
            )? > 0)
        })
    }

    /// The session's watches, oldest first.
    pub fn list_watches(&self, session_id: &str) -> Result<Vec<Watch>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT session_id, artifact_id, replies_armed, created_at FROM watches WHERE session_id = ?1 ORDER BY created_at, artifact_id",
            )?;
            Ok(stmt.query_map(params![session_id], row_to_watch)?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Watches on `id` held by sessions that have not ended, oldest first.
    pub fn watchers(&self, id: &ArtifactId) -> Result<Vec<Watch>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT w.session_id, w.artifact_id, w.replies_armed, w.created_at FROM watches w
                 JOIN sessions s ON s.id = w.session_id
                 WHERE w.artifact_id = ?1 AND s.ended_at IS NULL ORDER BY w.created_at, w.session_id",
            )?;
            Ok(stmt.query_map(params![id.as_str()], row_to_watch)?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::CoreError;
    use crate::store::test_util::DAEMON;
    use crate::store::test_util::{artifact, session, store};

    #[test]
    fn watch_upserts_and_ensure_watch_never_rearms() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let s = session(&st, "claude", "h1");
        assert!(st.watch(&s, &aid, true).unwrap().replies_armed);
        assert!(!st.watch(&s, &aid, false).unwrap().replies_armed);
        assert!(
            !st.ensure_watch(&s, &aid).unwrap().replies_armed,
            "an existing watch keeps its arming"
        );
        assert_eq!(st.list_watches(&s).unwrap().len(), 1);
        assert!(st.unwatch(&s, &aid).unwrap());
        assert!(!st.unwatch(&s, &aid).unwrap());
        assert!(
            st.ensure_watch(&s, &aid).unwrap().replies_armed,
            "a new watch is armed"
        );
    }

    #[test]
    fn watch_refuses_unknown_or_ended_sessions_and_missing_artifacts() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let s = session(&st, "claude", "h1");
        assert!(matches!(
            st.watch("nope", &aid, true),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        st.end_session(&s).unwrap();
        assert!(matches!(
            st.watch(&s, &aid, true),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        let s2 = session(&st, "claude", "h2");
        st.delete_artifact(DAEMON, &aid).unwrap();
        assert!(matches!(
            st.watch(&s2, &aid, true),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn watchers_lists_live_sessions_only() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let a = session(&st, "claude", "a");
        let b = session(&st, "codex", "b");
        st.watch(&a, &aid, true).unwrap();
        st.watch(&b, &aid, false).unwrap();
        st.with_write(|c| {
            c.execute("UPDATE sessions SET ended_at = 'x' WHERE id = ?1", [&b])?;
            Ok(())
        })
        .unwrap();
        let w = st.watchers(&aid).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].session_id, a);
    }
}
