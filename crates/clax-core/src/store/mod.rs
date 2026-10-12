//! SQLite-backed store.
//!
//! The database runs in WAL mode behind three kinds of connection, each with
//! a [`BUSY_TIMEOUT`](exec::BUSY_TIMEOUT) so another process holding a lock
//! makes a call wait briefly instead of failing:
//!
//! - **One writer.** There is one write connection, lent to one caller at a
//!   time in arrival order (a ticket queue). Every write goes through
//!   [`Store::with_tx`] (or, for statements that are not transactions,
//!   `with_write`), which blocks the caller until every earlier caller has
//!   finished and then runs the job on the caller's own thread. Parsing,
//!   decoding, hashing and file writes happen before the job starts, so the
//!   connection is held only for short transactions. On open, the
//!   migrations run on this connection before any reader opens.
//! - **Readers.** A pool of `query_only` connections, at most
//!   [`reader_count`](exec::reader_count), opened on first use. Every
//!   read-only method takes one through `with_read`, so reads run
//!   concurrently with each other and with the writer. Each `with_read` runs
//!   in one read transaction, so all its statements see one snapshot. A
//!   write job may not read through a reader. A read still running
//!   after [`READ_LIMIT`](exec::READ_LIMIT) is interrupted and fails with
//!   [`CoreError::ReadTimeout`](crate::CoreError::ReadTimeout).
//! - **Checkpoints.** Once the store serves [`Store::call`], a background
//!   thread runs a `PASSIVE` checkpoint every
//!   [`CHECKPOINT_INTERVAL`](exec::CHECKPOINT_INTERVAL), so commits rarely
//!   reach the [`AUTOCHECKPOINT_PAGES`](exec::AUTOCHECKPOINT_PAGES) threshold
//!   at which they would checkpoint themselves.
//!
//! The methods are synchronous and may be called from any thread. The
//! daemon calls them through [`Store::call`], which runs a job on worker
//! threads (one per reader) fed by a bounded FIFO queue, so a pile-up of
//! requests waits in the queue instead of claiming more threads, and a job
//! whose request has given up is skipped. Reads whose cost grows with the
//! whole home go through [`Store::call_bulk`] instead: a second bounded
//! FIFO queue, which workers take from after the first while still starting
//! one of its jobs at least every few interactive ones, and whose jobs never
//! hold every worker, so cheap requests do not wait behind them. Each queue
//! holds at most [`QUEUE_CAPACITY`](exec::QUEUE_CAPACITY) jobs, so twice
//! that may be queued or running in all. A job waiting for the write turn
//! or holding it does not count against the workers that reads need: while
//! fewer than one per reader would be free of writes, the pool starts
//! another. [`Store::shutdown`] drains both queues and joins the threads.
//!
//! Version writes, deletes, and asset writes combine a transaction with
//! file-system work, ordered so that a failure on either side leaves no
//! half-written state.

pub mod approvals;
pub mod artifacts;
pub mod assets;
pub mod attention;
pub mod audit;
pub mod backfill;
pub mod batches;
pub mod changelog;
pub mod docs;
pub mod elevations;
pub mod exec;
pub mod extension;
pub mod feedback;
pub mod grants;
pub mod inbox;
pub mod joined;
pub mod live;
pub mod migrations;
pub mod people;
#[cfg(test)]
mod plans;
pub mod questions;
pub mod sessions;
pub mod site;
pub mod threads;
pub mod viewers;
pub mod watches;

use crate::working::{Clock, SystemClock};
use crate::{CoreError, Home, Result};
use chrono::{DateTime, Utc};
use exec::{Readers, Workers, Writer};
use inbox::InboxChange;
use rusqlite::Connection;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Rows `ANALYZE` samples per index when `PRAGMA optimize` refreshes statistics.
pub const ANALYSIS_LIMIT: u32 = 400;
/// How often the daemon refreshes planner statistics ([`Store::optimize`]).
pub const OPTIMIZE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// The installed inbox listener, cloned out of its lock to be called.
type SharedListener = Arc<dyn Fn(Vec<InboxChange>) + Send + Sync>;

/// The installed approval request listener, cloned out of its lock.
type RequestListener = Arc<dyn Fn(Vec<i64>) + Send + Sync>;

/// The installed agents listener, cloned out of its lock.
type AgentsListener = Arc<dyn Fn(Vec<String>) + Send + Sync>;

pub struct Store {
    home: Home,
    writer: Writer,
    readers: Readers,
    workers: OnceLock<Workers>,
    /// Set by [`Store::shutdown`].
    shut_down: AtomicBool,
    /// Changes to `inbox_items` in the write transaction under way, recorded
    /// by the write connection's update hook and dropped by its rollback hook.
    inbox_changes: Arc<Mutex<Vec<InboxChange>>>,
    inbox_listener: RwLock<Option<SharedListener>>,
    /// The rows of `approval_requests` made or changed in the write
    /// transaction under way, recorded by the update hook as `inbox_changes`
    /// are, and the listener told of them after it commits.
    request_changes: Arc<Mutex<Vec<i64>>>,
    request_listener: RwLock<Option<RequestListener>>,
    /// Audit recording state: the nudge and the install ID.
    audit: audit::AuditState,
    /// The visibility generation and its listener (spec
    /// 2026-10-08-multi-agent-multi-person M17).
    visibility: approvals::VisState,
    /// Set by [`Store::agents_changed`] in the write under way, with the
    /// artifacts the changed agents watched; the agents listener hears of
    /// it once the write commits.
    agents_changed: Mutex<Option<std::collections::BTreeSet<String>>>,
    agents_listener: RwLock<Option<AgentsListener>>,
    /// Where every time the store writes or compares is read
    /// ([`Store::now`]). IDs are minted from the system clock.
    clock: Arc<dyn Clock>,
    /// When this store last started watching sessions: its opening, or the
    /// daemon's last resume after its machine slept ([`Store::mark_resumed`]).
    /// A remote session's idle time counts from no earlier than this.
    resumed_at: Mutex<String>,
}

/// `t` as the store writes times: RFC 3339 in UTC, to the millisecond.
pub fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The system clock's time as the store writes times ([`stamp`]), for times
/// taken outside any store (an agent side's, or the daemon's before its
/// store opens).
pub fn stamp_now() -> String {
    stamp(Utc::now())
}

impl Store {
    /// Opens (creating when missing) the home's database, migrates it to
    /// the current schema on the write connection, and records the history
    /// from before the audit there once per home (the audit backfill, see
    /// [`backfill`]), so the store records no change before the history it
    /// follows. The store reads the time from the system clock.
    pub fn open(home: &Home) -> Result<Store> {
        Store::open_reporting(home, &mut |_| {})
    }

    /// [`Store::open`], reporting the audit backfill's progress to
    /// `report` while it runs (only on the first open after the audit
    /// schema arrives, when there is history to record).
    pub fn open_reporting(
        home: &Home,
        report: &mut dyn FnMut(&backfill::Progress),
    ) -> Result<Store> {
        Store::open_with(home, Arc::new(SystemClock), report)
    }

    /// As [`Store::open`], with every time the store reads or writes taken
    /// from `clock`.
    pub fn open_with_clock(home: &Home, clock: Arc<dyn Clock>) -> Result<Store> {
        Store::open_with(home, clock, &mut |_| {})
    }

    /// [`Store::open_with_clock`], reporting the audit backfill's progress
    /// to `report` as [`Store::open_reporting`] does.
    pub fn open_with(
        home: &Home,
        clock: Arc<dyn Clock>,
        report: &mut dyn FnMut(&backfill::Progress),
    ) -> Result<Store> {
        home.ensure_dirs()?;
        let path = home.db_path();
        let mut conn = exec::open_writer(&path)?;
        migrate(&mut conn)?;
        backfill::run(&mut conn, home, report)?;
        approvals::record_migration_approval(&mut conn)?;
        conn.execute_batch(&format!(
            "PRAGMA analysis_limit={ANALYSIS_LIMIT}; PRAGMA optimize=0x10002;"
        ))?;
        // Installed after the migrations, so migration 21's history is not announced.
        let inbox_changes = Arc::new(Mutex::new(Vec::new()));
        let request_changes = Arc::new(Mutex::new(Vec::new()));
        let made = inbox_changes.clone();
        let requested = request_changes.clone();
        conn.update_hook(Some(
            move |action: rusqlite::hooks::Action, _db: &str, table: &str, rowid: i64| {
                if table == "inbox_items" {
                    made.lock().unwrap().push(InboxChange {
                        seq: rowid,
                        made: action == rusqlite::hooks::Action::SQLITE_INSERT,
                    });
                } else if table == "approval_requests" {
                    let mut r = requested.lock().unwrap();
                    if !r.contains(&rowid) {
                        r.push(rowid);
                    }
                }
            },
        ));
        let dropped = inbox_changes.clone();
        let dropped_requests = request_changes.clone();
        conn.rollback_hook(Some(move || {
            dropped.lock().unwrap().clear();
            dropped_requests.lock().unwrap().clear();
        }));
        Ok(Store {
            home: home.clone(),
            writer: Writer::new(conn),
            readers: Readers::new(path, exec::reader_count()),
            workers: OnceLock::new(),
            shut_down: AtomicBool::new(false),
            inbox_changes,
            inbox_listener: RwLock::new(None),
            request_changes,
            request_listener: RwLock::new(None),
            audit: audit::AuditState::default(),
            visibility: approvals::VisState::default(),
            agents_changed: Mutex::new(None),
            agents_listener: RwLock::new(None),
            resumed_at: Mutex::new(stamp(clock.now())),
            clock,
        })
    }

    /// Refreshes the query planner's statistics where they are stale
    /// (`PRAGMA optimize`, sampling at most [`ANALYSIS_LIMIT`] rows per
    /// index). Cheap when nothing changed; the daemon runs it every
    /// [`OPTIMIZE_INTERVAL`].
    pub fn optimize(&self) -> Result<()> {
        self.with_write(|c| {
            c.execute_batch(&format!(
                "PRAGMA analysis_limit={ANALYSIS_LIMIT}; PRAGMA optimize;"
            ))?;
            Ok(())
        })
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    /// First row of `PRAGMA integrity_check`; "ok" when the database is sound.
    /// Not subject to the read limit.
    pub fn integrity_check(&self) -> Result<String> {
        self.readers.run(false, |c| {
            Ok(c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?)
        })
    }

    /// Sets how long a read may run before it is interrupted (default
    /// [`exec::READ_LIMIT`]).
    #[doc(hidden)]
    pub fn set_read_limit(&self, limit: std::time::Duration) {
        self.readers.set_limit(Some(limit));
    }

    /// Lets reads run as long as they take, for offline checks of a whole
    /// home (`clax doctor`) rather than requests.
    pub fn lift_read_limit(&self) {
        self.readers.set_limit(None);
    }

    /// The current time by the store's clock, as the store writes times
    /// ([`stamp`]).
    pub fn now(&self) -> String {
        stamp(self.clock.now())
    }

    /// The current time by the store's clock.
    pub fn now_utc(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Records that the daemon resumes watching sessions now, after time in
    /// which it could not hear from them (its machine slept): a remote
    /// session's idle time counts from no earlier than this
    /// ([`Store::reap_sessions`]). Opening the store marks it too.
    pub fn mark_resumed(&self) {
        *self.resumed_at.lock().unwrap() = self.now();
    }

    /// When the daemon last resumed watching sessions ([`Store::mark_resumed`]).
    pub fn resumed_at(&self) -> String {
        self.resumed_at.lock().unwrap().clone()
    }

    /// Runs `f` on a reader connection, which refuses writes.
    pub(crate) fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.readers.run(true, f)
    }

    /// Runs `f` in a transaction on the write connection and commits it when
    /// `f` succeeds (an error or a panic rolls it back). Blocks until every
    /// write that arrived before this one has finished, then runs `f` on the
    /// calling thread.
    ///
    /// The transaction is `IMMEDIATE`: it takes the write lock when it
    /// begins, where a lock held elsewhere (another process, or a reader
    /// briefly holding it to repair a WAL index it saw mid-update) is waited
    /// for under the busy timeout. A deferred transaction that read first
    /// would instead fail at once with `SQLITE_BUSY` on its first write.
    ///
    /// When the committed transaction made or updated inbox items, the inbox
    /// listener hears of them once the write turn is released. When `f`
    /// recorded an audit event ([`Store::record_audit`]), the audit nudge
    /// ([`Store::set_audit_nudge`]) fires after the commit. When `f` changed
    /// what agents may see ([`Store::visibility_changed`]), the visibility
    /// generation is bumped after the commit.
    pub(crate) fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let (out, changes, requests, recorded, agents) = self.writer.run(|conn| {
            self.inbox_changes.lock().unwrap().clear();
            self.request_changes.lock().unwrap().clear();
            let _job = self.audit.begin();
            self.visibility.begin();
            *self.agents_changed.lock().unwrap() = None;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let out = f(&tx)?;
            tx.commit()?;
            // Inside the turn, so generations follow commit order.
            self.visibility.committed();
            // Taken inside the turn: the next writer's changes are its own.
            Ok((
                out,
                std::mem::take(&mut *self.inbox_changes.lock().unwrap()),
                std::mem::take(&mut *self.request_changes.lock().unwrap()),
                self.audit.take_recorded(),
                self.agents_changed.lock().unwrap().take(),
            ))
        })?;
        if recorded {
            self.audit.nudge();
        }
        if !changes.is_empty() {
            // Cloned out, so the listener runs without the lock.
            let listen = self.inbox_listener.read().unwrap().clone();
            if let Some(listen) = listen {
                listen(changes);
            }
        }
        if !requests.is_empty() {
            let listen = self.request_listener.read().unwrap().clone();
            if let Some(listen) = listen {
                listen(requests);
            }
        }
        if let Some(artifacts) = agents {
            let listen = self.agents_listener.read().unwrap().clone();
            if let Some(listen) = listen {
                listen(artifacts.into_iter().collect());
            }
        }
        Ok(out)
    }

    /// Marks the write under way (in [`Store::with_tx`]) as changing the
    /// agents roster (a session started, ended or was renamed), read on
    /// `c` before the change: the artifacts session `sid` watches.
    pub(crate) fn agents_changed(&self, c: &Connection, sid: &str) -> Result<()> {
        let watched: Vec<String> = c
            .prepare_cached("SELECT artifact_id FROM watches WHERE session_id = ?1")?
            .query_map(rusqlite::params![sid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.agents_changed
            .lock()
            .unwrap()
            .get_or_insert_default()
            .extend(watched);
        Ok(())
    }

    /// Marks the write under way as changing the agents roster without an
    /// agent starting or ending: a thread's owner changed (`threads_owned`).
    pub(crate) fn agents_pinged(&self) {
        self.agents_changed.lock().unwrap().get_or_insert_default();
    }

    /// Installs `f`, called after each committed write that started, ended
    /// or renamed an agent session, or changed a thread's owner, with the artifacts those agents
    /// watched, outside the write turn and after the other listeners.
    /// Replaces any earlier listener.
    pub fn set_agents_listener(&self, f: impl Fn(Vec<String>) + Send + Sync + 'static) {
        *self.agents_listener.write().unwrap() = Some(Arc::new(f));
    }

    /// Installs `f`, called with the rows (`rowid`s) of `approval_requests`
    /// made or changed by each committed write transaction that changed
    /// any, outside the write turn and after the inbox listener. Replaces
    /// any earlier listener.
    pub fn set_request_listener(&self, f: impl Fn(Vec<i64>) + Send + Sync + 'static) {
        *self.request_listener.write().unwrap() = Some(Arc::new(f));
    }

    /// Runs `f` on the write connection outside a transaction, for
    /// statements that manage their own (`PRAGMA optimize`).
    pub(crate) fn with_write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.writer.run(|conn| f(conn))
    }
}

/// The schema version (`user_version`) of `home`'s database, read without
/// changing it; `None` when there is no database.
///
/// # Errors
/// When the database cannot be read.
pub fn schema_version(home: &Home) -> Result<Option<u32>> {
    let path = home.db_path();
    if !path.exists() {
        return Ok(None);
    }
    let c = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(exec::BUSY_TIMEOUT)?;
    Ok(Some(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
}

/// Applies every migration past the database's `user_version`, each (its
/// SQL, then its [Rust step](migrations::rust_step)) in its
/// own `IMMEDIATE` transaction that re-reads the version under the write
/// lock, so two processes opening the database at once apply each migration
/// once. Refuses a database whose schema is newer than this binary knows
/// ([`CoreError::SchemaNewer`]) without changing it.
fn migrate(conn: &mut Connection) -> Result<()> {
    let known = migrations::MIGRATIONS.len() as u32;
    let check = |version: u32| {
        if version > known {
            Err(CoreError::SchemaNewer {
                found: version,
                known,
            })
        } else {
            Ok(())
        }
    };
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    check(version)?;
    if version == known {
        return Ok(());
    }
    loop {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        check(version)?;
        if version == known {
            return Ok(());
        }
        tx.execute_batch(migrations::MIGRATIONS[version as usize])?;
        migrations::rust_step(&tx, version + 1)?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    /// The audit context of changes a test makes: Clax itself.
    pub const DAEMON: &crate::audit::AuditCtx = &crate::audit::AuditCtx::DAEMON;
    use crate::anchor::{Anchor, AnchorKind};
    use crate::publish::{PublishRequest, validate};
    use crate::{ArtifactId, Home, RegisterSession, Store};

    pub fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    /// A one-version artifact titled "Quarterly Review", owned by `session`.
    pub fn artifact(store: &Store, session: Option<&str>) -> ArtifactId {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Quarterly Review",
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let (a, _) = store
            .create_artifact(DAEMON, validate(req).unwrap(), session)
            .unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    /// A one-version artifact declaring `caps`.
    pub fn artifact_with_caps(store: &Store, caps: serde_json::Value) -> ArtifactId {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Tracker",
            "capabilities": caps,
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let (a, _) = store
            .create_artifact(DAEMON, validate(req).unwrap(), None)
            .unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    /// A live session of `harness` with harness session ID `hsid`; returns its ID.
    pub fn session(store: &Store, harness: &str, hsid: &str) -> String {
        store
            .register_session(
                DAEMON,
                RegisterSession {
                    harness: harness.into(),
                    harness_session_id: Some(hsid.into()),
                    cwd: "/w".into(),
                    pid: None,
                    parent_pid: None,
                    ..Default::default()
                },
            )
            .unwrap()
            .id
    }

    pub fn anchor() -> Anchor {
        Anchor {
            kind: AnchorKind::Element,
            selector: Some("body > main > h2".into()),
            quote: Some("Quarterly goals".into()),
            prefix: Some(String::new()),
            suffix: Some(String::new()),
            html_hash: Some("sha256:00".into()),
            rect: None,
            custom_name: None,
            area: None,
            file: "index.html".into(),
            route: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{anchor, artifact};
    use super::threads::NewThread;
    use crate::audit::AuditCtx;
    use crate::working::ManualClock;
    use crate::{Home, RegisterSession, Store};
    use std::sync::Arc;

    #[test]
    fn store_time_comes_from_the_injected_clock() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-09-01T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        assert_eq!(st.now(), "2026-09-01T10:00:00.000Z");
        let s = st
            .register_session(
                &AuditCtx::DAEMON,
                RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("h1".into()),
                    cwd: "/w".into(),
                    pid: None,
                    parent_pid: None,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(s.started_at, "2026-09-01T10:00:00.000Z");
        assert_eq!(s.last_seen_at, "2026-09-01T10:00:00.000Z");
        clock.advance(90);
        let again = st
            .register_session(
                &AuditCtx::DAEMON,
                RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("h1".into()),
                    cwd: "/w".into(),
                    pid: None,
                    parent_pid: None,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(again.id, s.id);
        assert_eq!(again.started_at, "2026-09-01T10:00:00.000Z");
        assert_eq!(again.last_seen_at, "2026-09-01T10:01:30.000Z");
        let aid = artifact(&st, Some(&s.id));
        let t = st
            .create_thread(
                &AuditCtx::DAEMON,
                &aid,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Alex".into(),
                    body: "Tighten this.".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(t.created_at, "2026-09-01T10:01:30.000Z");
        assert_eq!(t.comments[0].created_at, "2026-09-01T10:01:30.000Z");
    }

    #[test]
    fn the_reaper_reads_the_store_clock() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-09-01T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        let s = super::test_util::session(&st, "claude", "h1");
        let idle = std::time::Duration::from_secs(Store::SESSION_IDLE_SECS);
        assert!(st.reap_sessions(idle, &|_| false).unwrap().ended.is_empty());
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 1);
        assert_eq!(st.reap_sessions(idle, &|_| false).unwrap().ended, vec![s]);
    }
}
