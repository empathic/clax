//! Harness sessions: registration, joining by parent PID, liveness, and reaping.

use super::Store;
use crate::model::Session;
use crate::{CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What a harness shim or hook reports when it starts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegisterSession {
    pub harness: String,
    #[serde(default)]
    pub harness_session_id: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub parent_pid: Option<u32>,
}

const SELECT: &str = "SELECT id, harness, harness_session_id, cwd, pid, parent_pid, started_at,
    last_seen_at, ended_at FROM sessions";

fn row_to_session(r: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get("id")?,
        harness: r.get("harness")?,
        harness_session_id: r.get("harness_session_id")?,
        cwd: r.get("cwd")?,
        pid: r.get("pid")?,
        parent_pid: r.get("parent_pid")?,
        started_at: r.get("started_at")?,
        last_seen_at: r.get("last_seen_at")?,
        ended_at: r.get("ended_at")?,
    })
}

fn fetch(tx: &Transaction<'_>, id: &str) -> Result<Session> {
    Ok(tx.query_row(
        &format!("{SELECT} WHERE id = ?1"),
        params![id],
        row_to_session,
    )?)
}

/// The id of a live row matching `where_clause`, newest first.
fn find_live(
    tx: &Transaction<'_>,
    where_clause: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT id FROM sessions WHERE ended_at IS NULL AND {where_clause}
                 ORDER BY started_at DESC, id DESC LIMIT 1"
            ),
            args,
            |r| r.get(0),
        )
        .optional()?)
}

impl Store {
    /// A session unseen for this long, with no live process, is ended by the reaper;
    /// registration without a harness session ID only adopts rows seen within it.
    pub const SESSION_IDLE_SECS: u64 = 300;

    /// Registers a session, reusing a live row where one already stands for it.
    /// An empty `harness_session_id` counts as none.
    ///
    /// With a `harness_session_id`: a live row with the same `(harness,
    /// harness_session_id)` is refreshed and returned; failing that, a live row
    /// with the same `(harness, parent_pid)` and no harness session ID is
    /// adopted and given the ID. Without one: a live row with the same
    /// `(harness, parent_pid)` seen within [`Store::SESSION_IDLE_SECS`] is
    /// refreshed and returned, whether or not it has an ID yet (two shims, or a
    /// restarted shim, under one harness process share a row). Otherwise a new
    /// row is inserted. A refresh overwrites `pid` and `parent_pid` only when
    /// the caller supplies them, and fills an empty `cwd` (a hook-only row).
    pub fn register_session(&self, mut r: RegisterSession) -> Result<Session> {
        if r.harness_session_id.as_deref() == Some("") {
            r.harness_session_id = None;
        }
        self.with_tx(|tx| {
            let now = Store::now();
            let recent = (chrono::Utc::now() - Duration::from_secs(Self::SESSION_IDLE_SECS))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let existing = match (&r.harness_session_id, r.parent_pid) {
                (Some(hid), parent) => {
                    match find_live(
                        tx,
                        "harness = ?1 AND harness_session_id = ?2",
                        params![r.harness, hid],
                    )? {
                        Some(id) => Some(id),
                        None => match parent {
                            Some(p) => find_live(
                                tx,
                                "harness = ?1 AND parent_pid = ?2 AND harness_session_id IS NULL",
                                params![r.harness, p],
                            )?,
                            None => None,
                        },
                    }
                }
                (None, Some(p)) => find_live(
                    tx,
                    "harness = ?1 AND parent_pid = ?2 AND last_seen_at >= ?3",
                    params![r.harness, p, recent],
                )?,
                (None, None) => None,
            };
            if let Some(id) = existing {
                tx.execute(
                    "UPDATE sessions SET pid = COALESCE(?2, pid), parent_pid = COALESCE(?3, parent_pid),
                        harness_session_id = COALESCE(harness_session_id, ?4), last_seen_at = ?5,
                        cwd = CASE WHEN cwd = '' THEN ?6 ELSE cwd END
                     WHERE id = ?1",
                    params![id, r.pid, r.parent_pid, r.harness_session_id, now, r.cwd],
                )?;
                return fetch(tx, &id);
            }
            let id = new_ulid();
            tx.execute(
                "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                    started_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    id,
                    r.harness,
                    r.harness_session_id,
                    r.cwd,
                    r.pid,
                    r.parent_pid,
                    now
                ],
            )?;
            fetch(tx, &id)
        })
    }

    /// Called from a harness hook that knows the harness session ID and the
    /// harness process (`parent_pid`). Returns the live row for that
    /// `(harness, harness_session_id)`; else gives the ID to a live row for
    /// `(harness, parent_pid)` that has none; else inserts a hook-only row (no
    /// `pid`, `cwd` as given or empty) for the shim to adopt when it registers.
    /// A given `cwd` also fills an empty one on an existing row. Hook-only rows
    /// do not heartbeat: unless a shim adopts one, it is reaped after the idle
    /// window ([`Store::SESSION_IDLE_SECS`]).
    /// `ancestor_pids` (nearest first) are tried after `parent_pid` when no row matches.
    pub fn join_session(
        &self,
        harness: &str,
        parent_pid: u32,
        harness_session_id: &str,
        cwd: Option<&str>,
        ancestor_pids: &[u32],
    ) -> Result<Session> {
        self.with_tx(|tx| {
            let now = Store::now();
            let existing = match find_live(
                tx,
                "harness = ?1 AND harness_session_id = ?2",
                params![harness, harness_session_id],
            )? {
                Some(id) => Some(id),
                None => {
                    // The hook's parent, then its ancestors nearest first: a
                    // hook run through a wrapper shell is a grandchild of the
                    // harness the shim registered under.
                    let mut found = None;
                    for pid in std::iter::once(&parent_pid).chain(ancestor_pids) {
                        found = find_live(
                            tx,
                            "harness = ?1 AND parent_pid = ?2 AND harness_session_id IS NULL",
                            params![harness, pid],
                        )?;
                        if found.is_some() {
                            break;
                        }
                    }
                    found
                }
            };
            let id = match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE sessions SET harness_session_id = COALESCE(harness_session_id, ?2),
                            parent_pid = COALESCE(parent_pid, ?3), last_seen_at = ?4,
                            cwd = CASE WHEN cwd = '' THEN COALESCE(?5, cwd) ELSE cwd END
                         WHERE id = ?1",
                        params![id, harness_session_id, parent_pid, now, cwd],
                    )?;
                    id
                }
                None => {
                    let id = new_ulid();
                    tx.execute(
                        "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                            started_at, last_seen_at)
                         VALUES (?1, ?2, ?3, ?6, NULL, ?4, ?5, ?5)",
                        params![id, harness, harness_session_id, parent_pid, now, cwd.unwrap_or("")],
                    )?;
                    id
                }
            };
            fetch(tx, &id)
        })
    }

    /// Records that the session is alive. An ended session is returned as is:
    /// it is never revived.
    ///
    /// # Errors
    /// `NotFound` when no such session exists.
    pub fn heartbeat(&self, id: &str) -> Result<Session> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sessions SET last_seen_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![id, Store::now()],
            )?;
            fetch(tx, id).map_err(not_found)
        })
    }

    /// Ends the session; ending an ended session keeps its first `ended_at`.
    ///
    /// # Errors
    /// `NotFound` when no such session exists.
    pub fn end_session(&self, id: &str) -> Result<Session> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sessions SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![id, Store::now()],
            )?;
            fetch(tx, id).map_err(not_found)
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id],
                row_to_session,
            )
            .optional()?)
        })
    }

    /// Sessions, newest first; only those not ended when `live_only`.
    pub fn list_sessions(&self, live_only: bool) -> Result<Vec<Session>> {
        self.with_conn(|c| {
            let filter = if live_only {
                "WHERE ended_at IS NULL"
            } else {
                ""
            };
            let mut stmt = c.prepare(&format!(
                "{SELECT} {filter} ORDER BY started_at DESC, id DESC"
            ))?;
            Ok(stmt
                .query_map([], row_to_session)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Ends live sessions not seen for `idle` whose `pid` is unknown or no
    /// longer alive. Returns how many were ended.
    pub fn reap_sessions(&self, idle: Duration, pid_alive: &dyn Fn(u32) -> bool) -> Result<usize> {
        let cutoff =
            (chrono::Utc::now() - idle).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.with_tx(|tx| {
            let mut stmt = tx.prepare(
                "SELECT id, pid FROM sessions WHERE ended_at IS NULL AND last_seen_at < ?1",
            )?;
            let stale = stmt
                .query_map(params![cutoff], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<u32>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let now = Store::now();
            let mut n = 0;
            for (id, pid) in stale {
                if pid.is_some_and(pid_alive) {
                    continue;
                }
                n += tx.execute(
                    "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            Ok(n)
        })
    }
}

fn not_found(e: CoreError) -> CoreError {
    match e {
        CoreError::Db(rusqlite::Error::QueryReturnedNoRows) => CoreError::NotFound,
        e => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Home;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    fn reg(
        harness_session_id: Option<&str>,
        pid: Option<u32>,
        parent: Option<u32>,
    ) -> RegisterSession {
        RegisterSession {
            harness: "claude".into(),
            harness_session_id: harness_session_id.map(String::from),
            cwd: "/work".into(),
            pid,
            parent_pid: parent,
        }
    }

    #[test]
    fn phase_1_database_upgrades_to_2() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = rusqlite::Connection::open(home.db_path()).unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[0])
                .unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
        }
        let store = Store::open(&home).unwrap();
        let version: u32 = store
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, super::super::migrations::MIGRATIONS.len() as u32);
        assert!(store.list_sessions(false).unwrap().is_empty());
    }

    #[test]
    fn phase_2_database_upgrades_to_3() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = rusqlite::Connection::open(home.db_path()).unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[0])
                .unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[1])
                .unwrap();
            c.pragma_update(None, "user_version", 2).unwrap();
        }
        let store = Store::open(&home).unwrap();
        let version: u32 = store
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, super::super::migrations::MIGRATIONS.len() as u32);
        for table in [
            "watches",
            "threads",
            "comments",
            "feedback",
            "viewers",
            "session_env",
        ] {
            let n: i64 = store
                .with_conn(|c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        [table],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn register_with_harness_id_is_idempotent() {
        let (_d, store) = store();
        let a = store
            .register_session(reg(Some("h1"), Some(10), Some(5)))
            .unwrap();
        let b = store
            .register_session(reg(Some("h1"), Some(11), None))
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.pid, Some(11));
        assert_eq!(b.parent_pid, Some(5));
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn register_after_end_starts_a_new_session() {
        let (_d, store) = store();
        let a = store.register_session(reg(Some("h1"), None, None)).unwrap();
        store.end_session(&a.id).unwrap();
        let b = store.register_session(reg(Some("h1"), None, None)).unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn shim_first_then_hook_joins() {
        let (_d, store) = store();
        let shim = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.harness_session_id, None);
        let joined = store.join_session("claude", 5, "h1", None, &[]).unwrap();
        assert_eq!(joined.id, shim.id);
        assert_eq!(joined.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(joined.pid, Some(10));
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn hook_joins_a_shim_row_registered_under_an_ancestor() {
        let (_d, store) = store();
        let shim = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        let other = store
            .register_session(reg(None, Some(11), Some(7)))
            .unwrap();
        // Hook's parent is a wrapper shell (4); the harness (5) is next.
        let joined = store
            .join_session("claude", 4, "h1", None, &[5, 7])
            .unwrap();
        assert_eq!(joined.id, shim.id, "nearest matching ancestor wins");
        assert_eq!(joined.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(joined.parent_pid, Some(5));
        assert_ne!(joined.id, other.id);
        assert_eq!(store.list_sessions(true).unwrap().len(), 2);
    }

    #[test]
    fn hook_first_then_shim_adopts() {
        let (_d, store) = store();
        let hook = store.join_session("claude", 5, "h1", None, &[]).unwrap();
        assert_eq!(hook.pid, None);
        let shim = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.id, hook.id);
        assert_eq!(shim.pid, Some(10));
        assert_eq!(shim.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(shim.cwd, "/work");
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn hook_cwd_is_used_for_new_rows_and_fills_empty_ones() {
        let (_d, store) = store();
        let hook = store
            .join_session("claude", 5, "h1", Some("/hook"), &[])
            .unwrap();
        assert_eq!(hook.cwd, "/hook");
        let shim = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.cwd, "/hook", "an existing cwd is kept");
        let empty = store.join_session("codex", 8, "c1", None, &[]).unwrap();
        assert_eq!(empty.cwd, "");
        let filled = store
            .join_session("codex", 8, "c1", Some("/late"), &[])
            .unwrap();
        assert_eq!(filled.id, empty.id);
        assert_eq!(filled.cwd, "/late");
    }

    #[test]
    fn registrations_without_id_under_one_parent_share_a_row() {
        let (_d, store) = store();
        let a = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        let b = store
            .register_session(reg(None, Some(11), Some(5)))
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.pid, Some(11));
        assert_eq!(store.list_sessions(true).unwrap().len(), 1);
        let joined = store.join_session("claude", 5, "h1", None, &[]).unwrap();
        assert_eq!(joined.id, a.id);
    }

    #[test]
    fn stale_row_is_not_adopted_by_parent() {
        let (_d, store) = store();
        let old = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE sessions SET last_seen_at = ?2 WHERE id = ?1",
                    params![
                        old.id,
                        (chrono::Utc::now() - Duration::from_secs(600))
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let new = store
            .register_session(reg(None, Some(12), Some(5)))
            .unwrap();
        assert_ne!(new.id, old.id);
    }

    #[test]
    fn hook_join_is_idempotent_and_other_harnesses_stay_apart() {
        let (_d, store) = store();
        let a = store.join_session("claude", 5, "h1", None, &[]).unwrap();
        let b = store.join_session("claude", 5, "h1", None, &[]).unwrap();
        assert_eq!(a.id, b.id);
        let other = store.join_session("codex", 5, "h1", None, &[]).unwrap();
        assert_ne!(a.id, other.id);
    }

    #[test]
    fn shim_with_harness_id_adopts_hookless_row_by_parent() {
        let (_d, store) = store();
        let shim = store
            .register_session(reg(None, Some(10), Some(5)))
            .unwrap();
        let again = store
            .register_session(reg(Some("h1"), Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.id, again.id);
        assert_eq!(again.harness_session_id.as_deref(), Some("h1"));
    }

    #[test]
    fn heartbeat_and_end() {
        let (_d, store) = store();
        let s = store.register_session(reg(None, Some(10), None)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let beat = store.heartbeat(&s.id).unwrap();
        assert!(beat.last_seen_at > s.last_seen_at);
        let ended = store.end_session(&s.id).unwrap();
        assert!(ended.ended_at.is_some());
        let again = store.end_session(&s.id).unwrap();
        assert_eq!(again.ended_at, ended.ended_at);
        assert_eq!(
            store.heartbeat(&s.id).unwrap().last_seen_at,
            ended.last_seen_at
        );
        assert!(matches!(store.heartbeat("nope"), Err(CoreError::NotFound)));
        assert!(store.list_sessions(true).unwrap().is_empty());
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn reaper_ends_only_idle_sessions_with_dead_or_unknown_pids() {
        let (_d, store) = store();
        let dead = store
            .register_session(reg(Some("dead"), Some(1001), None))
            .unwrap();
        let alive = store
            .register_session(reg(Some("alive"), Some(1002), None))
            .unwrap();
        let hook_only = store.join_session("claude", 7, "hook", None, &[]).unwrap();
        // Everything is fresh: nothing is idle yet.
        let pid_alive = |pid: u32| pid == 1002;
        assert_eq!(
            store
                .reap_sessions(Duration::from_secs(300), &pid_alive)
                .unwrap(),
            0
        );
        std::thread::sleep(Duration::from_millis(20));
        let fresh = store
            .register_session(reg(Some("fresh"), Some(1003), None))
            .unwrap();
        let n = store
            .reap_sessions(Duration::from_millis(10), &pid_alive)
            .unwrap();
        assert_eq!(n, 2);
        let ended = |id: &str| store.get_session(id).unwrap().unwrap().ended_at.is_some();
        assert!(ended(&dead.id));
        assert!(ended(&hook_only.id));
        assert!(!ended(&alive.id));
        assert!(!ended(&fresh.id));
    }
}
