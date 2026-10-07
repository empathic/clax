//! Watches: which sessions follow which artifacts, and whether replies are
//! armed. Each watch a session starts or stops (`watch.start`,
//! `watch.stop`), and each change of its arming (`watch.update`), is
//! recorded (audit spec 2026-10-06 §6.4) in the write's own transaction; a
//! write that changes nothing records nothing. Watches that go with their
//! session or artifact are not recorded apart: the `session.end` or
//! `artifact.delete` says so.

use super::Store;
use super::live::live_page_of_conn;
use super::threads::artifact_live;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::model::Watch;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

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

/// The arming and source of `sid`'s watch on `aid`, if it has one.
fn current(c: &Connection, sid: &str, aid: &str) -> Result<Option<(bool, String)>> {
    Ok(c.query_row(
        "SELECT replies_armed, source FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
        params![sid, aid],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
    )
    .optional()?)
}

/// What writing a watch row did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wrote {
    /// Made a new watch.
    Inserted,
    /// Changed an existing watch's arming.
    Rearmed,
    /// Nothing.
    Unchanged,
}

/// Why Clax made or re-armed a page watch the session did not ask for by
/// itself (spec §6.4 `cause`).
#[derive(Clone, Copy, Debug)]
pub(super) enum Cause<'a> {
    /// A scope watch of the session covers the page.
    Scope,
    /// The move `move_id` carried a watched thread to the page.
    Move(&'a str),
}

/// `rec` with the target of `sid`'s watch on artifact `aid`: `target`
/// `page` with the page's `origin` and `path` for a live page, else
/// `artifact`; and the IDs.
fn on_artifact(c: &Connection, mut rec: AuditRecord, sid: &str, aid: &str) -> Result<AuditRecord> {
    match live_page_of_conn(c, aid)? {
        Some(p) => {
            rec = rec
                .with("target", "page")
                .with("origin", p.origin.as_str())
                .with("path", p.path.as_str());
            rec.ids.origin = Some(p.origin);
        }
        None => rec = rec.with("target", "artifact"),
    }
    rec.ids.artifact = Some(aid.to_string());
    rec.ids.session = Some(sid.to_string());
    Ok(rec)
}

/// `rec` with `cause` (and a move's `move_id`), when there is one.
pub(super) fn with_cause(rec: AuditRecord, cause: Option<Cause<'_>>) -> AuditRecord {
    match cause {
        None => rec,
        Some(Cause::Scope) => rec.with("cause", "scope"),
        Some(Cause::Move(id)) => rec.with("cause", "move").with("move_id", id),
    }
}

/// A `watch.start` or `watch.stop` of `sid`'s watch on artifact `aid`
/// (spec §6.4).
pub(super) fn artifact_watch_record(
    c: &Connection,
    kind: AuditKind,
    sid: &str,
    aid: &str,
    armed: bool,
    source: &str,
) -> Result<AuditRecord> {
    artifact_watch_record_at(c, kind, &Store::now(), sid, aid, armed, source)
}

/// [`artifact_watch_record`] at `at`.
pub(super) fn artifact_watch_record_at(
    c: &Connection,
    kind: AuditKind,
    at: &str,
    sid: &str,
    aid: &str,
    armed: bool,
    source: &str,
) -> Result<AuditRecord> {
    let rec = AuditRecord::new(kind, at)
        .with("replies_armed", armed)
        .with("source", source);
    on_artifact(c, rec, sid, aid)
}

/// A `watch.update` of `sid`'s watch on artifact `aid`: the `fields` that
/// changed (spec §6.4).
fn artifact_update_record(
    c: &Connection,
    sid: &str,
    aid: &str,
    fields: serde_json::Value,
) -> Result<AuditRecord> {
    let rec = AuditRecord::new(AuditKind::WatchUpdate, Store::now()).with("fields", fields);
    on_artifact(c, rec, sid, aid)
}

/// A `watch.start` or `watch.stop` of `sid`'s scope watch on `origin` +
/// `path` (spec §6.4): `target` `scope`, made by the session itself.
pub(super) fn scope_watch_record(
    kind: AuditKind,
    sid: &str,
    origin: &str,
    path: &str,
    armed: bool,
) -> AuditRecord {
    scope_watch_record_at(kind, &Store::now(), sid, origin, path, armed)
}

/// [`scope_watch_record`] at `at`.
pub(super) fn scope_watch_record_at(
    kind: AuditKind,
    at: &str,
    sid: &str,
    origin: &str,
    path: &str,
    armed: bool,
) -> AuditRecord {
    let rec = AuditRecord::new(kind, at)
        .with("replies_armed", armed)
        .with("source", "direct");
    on_scope(rec, sid, origin, path)
}

/// A `watch.update` of `sid`'s scope watch on `origin` + `path`: its new
/// arming.
pub(super) fn scope_update_record(sid: &str, origin: &str, path: &str, armed: bool) -> AuditRecord {
    let rec = AuditRecord::new(AuditKind::WatchUpdate, Store::now())
        .with("fields", serde_json::json!({"replies_armed": armed}));
    on_scope(rec, sid, origin, path)
}

fn on_scope(rec: AuditRecord, sid: &str, origin: &str, path: &str) -> AuditRecord {
    let mut rec = rec
        .with("target", "scope")
        .with("origin", origin)
        .with("path", path);
    rec.ids.session = Some(sid.to_string());
    rec.ids.origin = Some(origin.to_string());
    rec
}

/// The events of what writing each `(session, arming, wrote)` of `rows`
/// did to its watch of page or artifact `aid` with `source`, as
/// [`Store::record_page_watches`] records them.
pub(super) fn page_watch_records(
    c: &Connection,
    aid: &str,
    rows: &[(String, bool, Wrote)],
    source: &str,
    cause: Option<Cause<'_>>,
) -> Result<Vec<AuditRecord>> {
    let mut out = Vec::new();
    for (sid, armed, wrote) in rows {
        let rec = match wrote {
            Wrote::Unchanged => continue,
            Wrote::Inserted => {
                artifact_watch_record(c, AuditKind::WatchStart, sid, aid, *armed, source)?
            }
            Wrote::Rearmed => {
                artifact_update_record(c, sid, aid, serde_json::json!({"replies_armed": armed}))?
            }
        };
        out.push(with_cause(rec, cause));
    }
    Ok(out)
}

impl Store {
    /// Records under `ctx` what writing each `(session, arming, wrote)` of
    /// `rows` did to its watch of page or artifact `aid` with `source`: a
    /// `watch.start` for a new watch, a `watch.update` for a re-armed one,
    /// each with `cause` when Clax made it for one.
    pub(super) fn record_page_watches(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        aid: &str,
        rows: &[(String, bool, Wrote)],
        source: &str,
        cause: Option<Cause<'_>>,
    ) -> Result<()> {
        for rec in page_watch_records(tx, aid, rows, source, cause)? {
            self.record_audit(tx, ctx, rec)?;
        }
        Ok(())
    }

    /// Creates or updates the watch of live session `session_id` on the live
    /// artifact `id`, setting `replies_armed`. The watch is direct: removing
    /// a scope watch that also covers the artifact keeps it. A new watch
    /// records `watch.start` under `ctx`; a change of an existing watch's
    /// arming, or a scope-made watch becoming direct, records `watch.update`
    /// with the fields that changed.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session; `NotFound` for a
    /// missing or deleted artifact.
    pub fn watch(
        &self,
        ctx: &AuditCtx,
        session_id: &str,
        id: &ArtifactId,
        replies_armed: bool,
    ) -> Result<Watch> {
        self.with_tx(|tx| {
            check(tx, session_id, id)?;
            let before = current(tx, session_id, id.as_str())?;
            tx.execute(
                "INSERT INTO watches (session_id, artifact_id, replies_armed, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_id, artifact_id) DO UPDATE SET replies_armed = excluded.replies_armed, source = 'direct'",
                params![session_id, id.as_str(), replies_armed, Store::now()],
            )?;
            let rec = match before {
                None => Some(artifact_watch_record(
                    tx,
                    AuditKind::WatchStart,
                    session_id,
                    id.as_str(),
                    replies_armed,
                    "direct",
                )?),
                Some((armed, source)) => {
                    let mut fields = serde_json::Map::new();
                    if armed != replies_armed {
                        fields.insert("replies_armed".into(), replies_armed.into());
                    }
                    if source != "direct" {
                        fields.insert("source".into(), "direct".into());
                    }
                    if fields.is_empty() {
                        None
                    } else {
                        Some(artifact_update_record(
                            tx,
                            session_id,
                            id.as_str(),
                            fields.into(),
                        )?)
                    }
                }
            };
            if let Some(rec) = rec {
                self.record_audit(tx, ctx, rec)?;
            }
            fetch(tx, session_id, id)
        })
    }

    /// Creates an armed watch unless one exists; an existing watch is returned
    /// unchanged. Used on publish, so republishing never re-arms replies the
    /// agent turned off. A new watch records `watch.start` under `ctx`.
    ///
    /// # Errors
    /// As [`Store::watch`].
    pub fn ensure_watch(&self, ctx: &AuditCtx, session_id: &str, id: &ArtifactId) -> Result<Watch> {
        self.with_tx(|tx| {
            check(tx, session_id, id)?;
            let n = tx.execute(
                "INSERT OR IGNORE INTO watches (session_id, artifact_id, replies_armed, created_at) VALUES (?1, ?2, 1, ?3)",
                params![session_id, id.as_str(), Store::now()],
            )?;
            if n > 0 {
                let rec = artifact_watch_record(
                    tx,
                    AuditKind::WatchStart,
                    session_id,
                    id.as_str(),
                    true,
                    "direct",
                )?;
                self.record_audit(tx, ctx, rec)?;
            }
            fetch(tx, session_id, id)
        })
    }

    /// Removes the watch; true when one existed, and then records
    /// `watch.stop` under `ctx`.
    pub fn unwatch(&self, ctx: &AuditCtx, session_id: &str, id: &ArtifactId) -> Result<bool> {
        self.with_tx(|tx| {
            let Some((armed, source)) = current(tx, session_id, id.as_str())? else {
                return Ok(false);
            };
            tx.execute(
                "DELETE FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
                params![session_id, id.as_str()],
            )?;
            let rec = artifact_watch_record(
                tx,
                AuditKind::WatchStop,
                session_id,
                id.as_str(),
                armed,
                &source,
            )?;
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
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
        assert!(st.watch(DAEMON, &s, &aid, true).unwrap().replies_armed);
        assert!(!st.watch(DAEMON, &s, &aid, false).unwrap().replies_armed);
        assert!(
            !st.ensure_watch(DAEMON, &s, &aid).unwrap().replies_armed,
            "an existing watch keeps its arming"
        );
        assert_eq!(st.list_watches(&s).unwrap().len(), 1);
        assert!(st.unwatch(DAEMON, &s, &aid).unwrap());
        assert!(!st.unwatch(DAEMON, &s, &aid).unwrap());
        assert!(
            st.ensure_watch(DAEMON, &s, &aid).unwrap().replies_armed,
            "a new watch is armed"
        );
    }

    #[test]
    fn watch_refuses_unknown_or_ended_sessions_and_missing_artifacts() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let s = session(&st, "claude", "h1");
        assert!(matches!(
            st.watch(DAEMON, "nope", &aid, true),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        st.end_session(DAEMON, &s).unwrap();
        assert!(matches!(
            st.watch(DAEMON, &s, &aid, true),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        let s2 = session(&st, "claude", "h2");
        st.delete_artifact(DAEMON, &aid).unwrap();
        assert!(matches!(
            st.watch(DAEMON, &s2, &aid, true),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn watchers_lists_live_sessions_only() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let a = session(&st, "claude", "a");
        let b = session(&st, "codex", "b");
        st.watch(DAEMON, &a, &aid, true).unwrap();
        st.watch(DAEMON, &b, &aid, false).unwrap();
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
