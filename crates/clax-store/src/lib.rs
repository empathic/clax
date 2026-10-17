//! The Clax store: the SQLite schema and its migrations, every query, the
//! visibility filter that reads approvals (`visibility`), and the audit
//! history's projection and export (`toolpath`). It depends on `clax-core`
//! alone among the workspace's crates; the clients (`clax-mcp`,
//! `clax-hooks`) never link it.
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
//!   [`CoreError::ReadTimeout`](clax_core::CoreError::ReadTimeout).
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
//! or holding it, or waiting for an artifact's content lock (which a write
//! holds until its transaction ends), does not count against the workers
//! that reads need: while fewer than one per reader would be free of
//! writes, the pool starts another. [`Store::shutdown`] drains both queues and joins the threads.
//!
//! A store opened on one connection its host opened ([`Store::open_on`],
//! for a Durable Object, which has no threads) has none of these: every
//! job runs inline on the caller's thread on that connection, a read in one
//! read transaction under `PRAGMA query_only`, a write in the same
//! transactions as above, and [`Store::call`] runs its job at once.
//!
//! # Content
//!
//! Version files, asset bytes and thread clip images live in the store's
//! [`ContentStore`] ([`clax_core::content`]; the home's files unless
//! [`Store::with_content`] or [`Store::open_on`] gives another), never in a
//! transaction. Version writes, deletes, and asset and clip writes combine
//! a transaction with content I/O done outside it, ordered so that a
//! failure on either side leaves no half-written state (see the content
//! module's Ordering). Every method that does content I/O has an `async`
//! twin (`publish_version_async`, `refile_threads_async`, …) that awaits
//! the content store and the artifacts' content locks; the synchronous
//! method runs its twin on the calling thread ([`Store::block_on`]):
//!
//! - It never waits for an artifact's content lock that its own thread
//!   holds (an `async` method holding it across an await, as a Durable
//!   Object's request does while its content store awaits a promise):
//!   that wait would never end natively, and on `wasm32` would abort the
//!   instance. It fails at once with [`CoreError::WouldBlock`] instead. A
//!   lock another thread holds is waited for, on the pool or on one
//!   connection shared by threads.
//! - On `wasm32` it never waits at all: when the twin would (the content
//!   store's I/O is not ready when polled) it fails with `WouldBlock`.
//! - On a content store whose I/O needs an executor
//!   ([`ContentStore::needs_executor`]: a Durable Object's, whose bindings
//!   are JavaScript promises) it does nothing at all: it fails with
//!   `WouldBlock` before its first read or write, so it never leaves work
//!   half done. A host with such a store awaits the `async` twins
//!   ([`Store::call_async`]).
//!
//! Every method takes its content locks before it writes anything, so
//! with a content store whose I/O completes when polled (the
//! filesystem's, the in-memory one) a refused lock leaves nothing behind.
//!
//! Local tooling for a home on disk reads the home's files whatever the
//! content store: the audit backfill of a home that predates the audit
//! (nothing to do for a fresh database, as on a Durable Object), and the
//! scans `clax doctor` runs ([`Store::stray_version_dirs`],
//! [`Store::stray_asset_temp_files`], [`assets::AssetRow::path`]); on a
//! home with no files they find nothing.

pub mod approvals;
pub mod artifacts;
pub mod assets;
pub mod attention;
pub mod audit;
pub mod backfill;
pub mod batches;
pub mod changelog;
pub mod device;
pub mod docs;
pub mod elevations;
pub mod engagement;
pub mod exec;
pub mod extension;
pub mod feedback;
pub mod forks;
pub mod github;
pub mod hosted;
pub mod inbox;
pub mod joined;
pub mod linked;
pub mod live;
pub mod migrations;
pub mod people;
#[cfg(test)]
mod plans;
pub mod pushes;
pub mod questions;
pub mod remotes;
pub mod sessions;
pub mod shares;
pub mod site;
pub mod threads;
pub mod viewers;
pub mod visibility;
pub mod watches;

pub mod toolpath;

pub use artifacts::{CorruptRow, MetaPatch};
pub use attention::{AgentPerson, AgentView, Attention, AttentionSummary, Participants, Person};
pub use audit::AuditRow;
pub use feedback::{Delivery, DeliveryViewer, SendTarget, TakeFeedback, Taken};
pub use sessions::{
    EndReason, EndedSession, JoinSession, NameSource, Proved, Reaped, RegisterSession, Registered,
    Restored, RosterAgent,
};
pub use threads::{NewComment, NewThread, ThreadExtras};

use chrono::{DateTime, Utc};
use clax_core::content::{ArtifactLock, ArtifactLocks, ContentStore, fs::FsContent};
use clax_core::time::stamp;
use clax_core::working::{Clock, SystemClock};
use clax_core::{CoreError, Home, Result};
use exec::{Exec, Single};
use inbox::InboxChange;
use rusqlite::Connection;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

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

/// The installed pushed versions listener, cloned out of its lock.
pub(crate) type PushedListener =
    Arc<dyn Fn(&clax_core::model::Artifact, &clax_core::model::Version) + Send + Sync>;

pub struct Store {
    home: Home,
    /// The pool, or one connection.
    exec: Exec,
    /// Where version files and assets are kept.
    content: Arc<dyn ContentStore>,
    /// Held across each version write and the transaction recording it.
    content_locks: ArtifactLocks,
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
    /// Told of each version recorded from a pushed commit
    /// ([`Store::set_pushed_listener`]).
    pushed_listener: RwLock<Option<PushedListener>>,
    /// Where every time the store writes or compares is read
    /// ([`Store::now`]). IDs are minted from the system clock.
    clock: Arc<dyn Clock>,
    /// When this store last started watching sessions: its opening, or the
    /// daemon's last resume after its machine slept ([`Store::mark_resumed`]).
    /// A remote session's idle time counts from no earlier than this.
    resumed_at: Mutex<String>,
    /// Each GitHub member's caps ([`Store::set_member_caps`]).
    member_caps: RwLock<Option<github::MemberCaps>>,
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
        let conn = exec::open_writer(&path)?;
        let content = Arc::new(FsContent::new(home.clone()));
        Store::assemble(home, conn, |c| Exec::pool(c, path), content, clock, report)
    }

    /// A store on `conn`, one connection its host opened (a Durable
    /// Object's, through its own VFS), where every job runs inline on the
    /// caller's thread ([`exec::Single`]): no reader pool, worker threads,
    /// checkpoints or read limit. Turns on foreign keys, migrates the
    /// database and records the audit backfill on `conn` as
    /// [`Store::open_with`] does; the journal mode and the busy timeout are
    /// the host's. Makes no directories: the files under `home` (version
    /// and asset bytes) are the host's too ([`Home::ensure_dirs`] makes
    /// them on a local disk). Version files and assets are kept in
    /// `content` (on a Durable Object, the host's; [`FsContent`] for the
    /// home's files).
    pub fn open_on(
        home: &Home,
        conn: Connection,
        clock: Arc<dyn Clock>,
        content: Arc<dyn ContentStore>,
    ) -> Result<Store> {
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        Store::assemble(
            home,
            conn,
            |c| Exec::Single(Single::new(c)),
            content,
            clock,
            &mut |_| {},
        )
    }

    /// Migrates the database on `conn`, the write connection, records the
    /// audit backfill and installs the change hooks, then runs the store's
    /// jobs on what `exec` makes of `conn`, keeping content in `content`.
    fn assemble(
        home: &Home,
        mut conn: Connection,
        exec: impl FnOnce(Connection) -> Exec,
        content: Arc<dyn ContentStore>,
        clock: Arc<dyn Clock>,
        report: &mut dyn FnMut(&backfill::Progress),
    ) -> Result<Store> {
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
        ))?;
        let dropped = inbox_changes.clone();
        let dropped_requests = request_changes.clone();
        conn.rollback_hook(Some(move || {
            dropped.lock().unwrap().clear();
            dropped_requests.lock().unwrap().clear();
        }))?;
        Ok(Store {
            home: home.clone(),
            exec: exec(conn),
            content,
            content_locks: ArtifactLocks::default(),
            shut_down: AtomicBool::new(false),
            inbox_changes,
            inbox_listener: RwLock::new(None),
            request_changes,
            request_listener: RwLock::new(None),
            audit: audit::AuditState::default(),
            visibility: approvals::VisState::default(),
            agents_changed: Mutex::new(None),
            agents_listener: RwLock::new(None),
            pushed_listener: RwLock::new(None),
            resumed_at: Mutex::new(stamp(clock.now())),
            member_caps: RwLock::new(None),
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

    /// The store, keeping version files and assets in `content` instead of
    /// the home's files. Content already written elsewhere is not moved.
    pub fn with_content(mut self, content: Arc<dyn ContentStore>) -> Store {
        self.content = content;
        self
    }

    /// Where the store keeps version files and assets.
    pub fn content(&self) -> &Arc<dyn ContentStore> {
        &self.content
    }

    /// Runs `f`, the `async` twin of a synchronous method, on this thread
    /// (see [Content](self#content)); fails at once with
    /// [`CoreError::WouldBlock`], running none of it, when the content
    /// store needs an executor ([`ContentStore::needs_executor`]). While it runs, a content lock this
    /// thread already holds is refused rather than waited for
    /// ([`Store::lock_content`]). Natively the thread otherwise waits as
    /// long as `f` does; on `wasm32`, where nothing else can run while it
    /// waits, `f` runs only as far as it goes without waiting, failing with
    /// [`CoreError::WouldBlock`] where it would wait.
    pub(crate) fn block_on<T>(&self, f: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        if self.content.needs_executor() {
            return Err(CoreError::WouldBlock {
                what: "content I/O that needs an executor; await the async method",
            });
        }
        let _sync = clax_core::content::SyncDriven::enter();
        #[cfg(target_arch = "wasm32")]
        {
            clax_core::content::ready(f).unwrap_or(Err(CoreError::WouldBlock {
                what: "content I/O that is not ready",
            }))
        }
        #[cfg(not(target_arch = "wasm32"))]
        clax_core::content::wait(f)
    }

    /// Waits for, then holds, the content lock of `id`. A worker thread
    /// that has to wait counts as writing meanwhile
    /// ([`exec::WriteLane::waiting`]), since the holder releases the lock
    /// only once its transaction ends.
    ///
    /// # Errors
    /// [`CoreError::WouldBlock`], in a synchronous method
    /// ([`Store::block_on`]), when this thread holds the lock already (an
    /// `async` method holding it across an await): waiting would never end.
    pub(crate) async fn lock_content(
        &self,
        id: &clax_core::ArtifactId,
    ) -> Result<ArtifactLock<'_>> {
        if let Some(held) = self.content_locks.try_lock(id) {
            return Ok(held);
        }
        self.refuse_held_here(&[id])?;
        let _lane = exec::WriteLane::waiting();
        Ok(self.content_locks.lock(id).await)
    }

    /// [`Store::lock_content`] for every one of `ids`, taken in ID order.
    pub(crate) async fn lock_contents(
        &self,
        ids: &[&clax_core::ArtifactId],
    ) -> Result<Vec<ArtifactLock<'_>>> {
        self.refuse_held_here(ids)?;
        let _lane = exec::WriteLane::waiting();
        Ok(self.content_locks.lock_all(ids).await)
    }

    /// [`CoreError::WouldBlock`] when a synchronous method runs on this
    /// thread and this thread holds the content lock of any of `ids`.
    fn refuse_held_here(&self, ids: &[&clax_core::ArtifactId]) -> Result<()> {
        if clax_core::content::driven_synchronously()
            && ids.iter().any(|id| self.content_locks.held_here(id))
        {
            return Err(CoreError::WouldBlock {
                what: "an artifact's content lock, held across an await on this thread",
            });
        }
        Ok(())
    }

    /// First row of `PRAGMA integrity_check`; "ok" when the database is sound.
    /// Not subject to the read limit.
    pub fn integrity_check(&self) -> Result<String> {
        self.exec.read(false, |c| {
            Ok(c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?)
        })
    }

    /// Sets how long a read may run before it is interrupted (default
    /// [`exec::READ_LIMIT`]). One connection has no read limit.
    #[doc(hidden)]
    pub fn set_read_limit(&self, limit: std::time::Duration) {
        self.exec.set_read_limit(Some(limit));
    }

    /// Lets reads run as long as they take, for offline checks of a whole
    /// home (`clax doctor`) rather than requests.
    pub fn lift_read_limit(&self) {
        self.exec.set_read_limit(None);
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

    /// Runs `f` on a reader connection, which refuses writes, in one read
    /// transaction (one snapshot).
    pub(crate) fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.exec.read(true, f)
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
        let (out, changes, requests, recorded, agents) = self.exec.write(|conn| {
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
        self.exec.write(|conn| f(conn))
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
    pub const DAEMON: &clax_core::audit::AuditCtx = &clax_core::audit::AuditCtx::DAEMON;
    use crate::{RegisterSession, Store};
    use clax_core::anchor::{Anchor, AnchorKind};
    use clax_core::publish::{PublishRequest, validate};
    use clax_core::working::{Clock, SystemClock};
    use clax_core::{ArtifactId, Home};
    use std::sync::Arc;

    /// Set to `single` in the run of the store's tests on one connection
    /// ([`super::tests::the_store_tests_pass_on_one_connection`]).
    pub const STORE_MODE_VAR: &str = "CLAX_TEST_STORE";

    /// Whether this run's [`store`]s are on one connection.
    pub fn on_one_connection() -> bool {
        std::env::var(STORE_MODE_VAR).as_deref() == Ok("single")
    }

    /// A store on a new home: the pool ([`pool_store`]), or one connection
    /// ([`single_store`]) when [`on_one_connection`].
    pub fn store() -> (tempfile::TempDir, Store) {
        store_with_clock(Arc::new(SystemClock))
    }

    /// [`store`], reading the time from `clock`.
    pub fn store_with_clock(clock: Arc<dyn Clock>) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = if on_one_connection() {
            open_single(&home, clock)
        } else {
            Store::open_with_clock(&home, clock).unwrap()
        };
        (dir, store)
    }

    /// A store on a new home, through [`Store::open`].
    pub fn pool_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    /// A store on a new home, through [`Store::open_on`] on a connection
    /// opened here in `journal_mode=MEMORY` and `locking_mode=EXCLUSIVE`,
    /// keeping content in memory ([`MemContent`](clax_core::content::memory::MemContent)),
    /// as a Durable Object keeps none in files.
    pub fn single_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = open_single(&Home::at(dir.path().join("ax")), Arc::new(SystemClock));
        (dir, store)
    }

    fn open_single(home: &Home, clock: Arc<dyn Clock>) -> Store {
        home.ensure_dirs().unwrap();
        let conn = rusqlite::Connection::open(home.db_path()).unwrap();
        // The journal and locking a Durable Object's connection has: no WAL,
        // and the one connection holds the lock for its life.
        conn.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA locking_mode=EXCLUSIVE;")
            .unwrap();
        let content = Arc::new(clax_core::content::memory::MemContent::new());
        Store::open_on(home, conn, clock, content).unwrap()
    }

    /// The bytes of `path` in version `n` of `id`, read from `st`'s content
    /// store; `None` when they are not there.
    pub fn content_file(st: &Store, id: &ArtifactId, n: u32, path: &str) -> Option<Vec<u8>> {
        clax_core::content::wait(st.content().read_file(id, n, path)).unwrap()
    }

    /// Whether version `n` of `id` has any content in `st`'s content store
    /// (its `index.html`, which every version has).
    pub fn content_version_exists(st: &Store, id: &ArtifactId, n: u32) -> bool {
        content_file(st, id, n, clax_core::publish::INDEX).is_some()
    }

    /// The clip of thread `tid` of `id` in `st`'s content store.
    pub fn content_clip(st: &Store, id: &ArtifactId, tid: &str) -> Option<Vec<u8>> {
        clax_core::content::wait(st.content().read_clip(id, tid)).unwrap()
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
    use crate::{RegisterSession, Store};
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    /// The store's tests and the journal's (`toolpath::tests`), which open
    /// their stores through `test_util::store`, pass on one connection too:
    /// this test binary runs them again with
    /// [`STORE_MODE_VAR`](super::test_util::STORE_MODE_VAR) set. The pool's
    /// own tests (`exec::tests`) are left out; they open the pool whichever
    /// the mode, and its one-connection tests are run here.
    #[test]
    fn the_store_tests_pass_on_one_connection() {
        if super::test_util::on_one_connection() {
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--skip", "exec::tests::"])
            .env(super::test_util::STORE_MODE_VAR, "single")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stdout}\n{stderr}");
        let passed: usize = stdout
            .lines()
            .find_map(|l| l.strip_prefix("test result: ok. "))
            .and_then(|l| l.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no test result in:\n{stdout}"));
        // The filter matched the store's tests, not nothing.
        assert!(passed > 300, "{passed} passed:\n{stdout}");
    }

    /// A content store whose version writes wait until `open`, as a
    /// Durable Object's wait on a JavaScript promise.
    struct Gated {
        inner: clax_core::content::memory::MemContent,
        open: std::sync::atomic::AtomicBool,
        waker: std::sync::Mutex<Option<std::task::Waker>>,
    }

    impl Gated {
        fn set_open(&self, open: bool) {
            self.open.store(open, std::sync::atomic::Ordering::SeqCst);
            if open && let Some(w) = self.waker.lock().unwrap().take() {
                w.wake();
            }
        }
    }

    #[async_trait::async_trait]
    impl clax_core::content::ContentStore for Gated {
        async fn read_file(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
            path: &str,
        ) -> clax_core::Result<Option<Vec<u8>>> {
            self.inner.read_file(id, n, path).await
        }
        async fn write_version(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
            write: &clax_core::content::VersionWrite<'_>,
        ) -> clax_core::Result<()> {
            std::future::poll_fn(|cx| {
                if self.open.load(std::sync::atomic::Ordering::SeqCst) {
                    std::task::Poll::Ready(())
                } else {
                    *self.waker.lock().unwrap() = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            })
            .await;
            self.inner.write_version(id, n, write).await
        }
        async fn remove_version(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
        ) -> clax_core::Result<()> {
            self.inner.remove_version(id, n).await
        }
        async fn delete_artifact(&self, id: &clax_core::ArtifactId) -> clax_core::Result<()> {
            self.inner.delete_artifact(id).await
        }
        async fn put_asset(
            &self,
            key: &clax_core::content::AssetKey,
            bytes: &[u8],
        ) -> clax_core::Result<()> {
            self.inner.put_asset(key, bytes).await
        }
        async fn read_asset(
            &self,
            key: &clax_core::content::AssetKey,
        ) -> clax_core::Result<Option<Vec<u8>>> {
            self.inner.read_asset(key).await
        }
        async fn delete_asset(&self, key: &clax_core::content::AssetKey) -> clax_core::Result<()> {
            self.inner.delete_asset(key).await
        }
    }

    fn next_version(n: u32, html: &str) -> clax_core::publish::ValidatedPublish {
        clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "if_version": n,
                "files": {"index.html": {"content": html, "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap()
    }

    fn would_block<T: std::fmt::Debug>(r: clax_core::Result<T>) {
        let e = r.unwrap_err();
        assert!(
            matches!(e, clax_core::CoreError::WouldBlock { .. }),
            "{e:?}"
        );
    }

    /// On one connection, a synchronous method never waits for an
    /// artifact's content lock that an async method holds across an await
    /// on the same thread (which would deadlock natively and abort on
    /// wasm32): it fails with `WouldBlock`, changing nothing, and works
    /// again once the holder is done.
    #[test]
    fn a_sync_method_on_one_connection_fails_rather_than_wait_for_a_content_lock() {
        use std::future::Future;
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let conn = rusqlite::Connection::open(home.db_path()).unwrap();
        let gated = Arc::new(Gated {
            inner: clax_core::content::memory::MemContent::new(),
            open: std::sync::atomic::AtomicBool::new(true),
            waker: std::sync::Mutex::new(None),
        });
        let st = Store::open_on(
            &home,
            conn,
            Arc::new(clax_core::working::SystemClock),
            gated.clone(),
        )
        .unwrap();
        let aid = artifact(&st, None);
        gated.set_open(false);

        // An async publish holds the lock, waiting on its content write.
        let ctx = &AuditCtx::DAEMON;
        let mut held =
            std::pin::pin!(st.publish_version_async(ctx, &aid, next_version(1, "<p>2"), None));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(held.as_mut().poll(&mut cx).is_pending());

        would_block(st.publish_version(ctx, &aid, next_version(1, "<p>x"), None));
        would_block(st.delete_artifact(ctx, &aid));
        let clipped = NewThread {
            author_public_id: None,
            version_n: 1,
            anchor: anchor(),
            author_name: "Alex".into(),
            body: "Tighten this.".into(),
            clip: Some(b"png".to_vec()),
            via_page: false,
        };
        would_block(st.create_thread(ctx, &aid, clipped));
        // Reads go on.
        assert_eq!(st.get_artifact(&aid).unwrap().unwrap().current_version, 1);
        assert!(
            st.list_threads(&aid, true, None, 10).unwrap().0.is_empty(),
            "no thread was made"
        );

        gated.set_open(true);
        let (a, _) = clax_core::content::wait(held).unwrap();
        assert_eq!(a.current_version, 2);
        let (a, _) = st
            .publish_version(ctx, &aid, next_version(2, "<p>3"), None)
            .unwrap();
        assert_eq!(a.current_version, 3);
    }

    /// In-memory content that says it needs an executor.
    struct NeedsExecutor(clax_core::content::memory::MemContent);

    #[async_trait::async_trait]
    impl clax_core::content::ContentStore for NeedsExecutor {
        async fn read_file(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
            path: &str,
        ) -> clax_core::Result<Option<Vec<u8>>> {
            self.0.read_file(id, n, path).await
        }
        async fn write_version(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
            write: &clax_core::content::VersionWrite<'_>,
        ) -> clax_core::Result<()> {
            self.0.write_version(id, n, write).await
        }
        async fn remove_version(
            &self,
            id: &clax_core::ArtifactId,
            n: u32,
        ) -> clax_core::Result<()> {
            self.0.remove_version(id, n).await
        }
        async fn delete_artifact(&self, id: &clax_core::ArtifactId) -> clax_core::Result<()> {
            self.0.delete_artifact(id).await
        }
        async fn put_asset(
            &self,
            key: &clax_core::content::AssetKey,
            bytes: &[u8],
        ) -> clax_core::Result<()> {
            self.0.put_asset(key, bytes).await
        }
        async fn read_asset(
            &self,
            key: &clax_core::content::AssetKey,
        ) -> clax_core::Result<Option<Vec<u8>>> {
            self.0.read_asset(key).await
        }
        async fn delete_asset(&self, key: &clax_core::content::AssetKey) -> clax_core::Result<()> {
            self.0.delete_asset(key).await
        }
        fn needs_executor(&self) -> bool {
            true
        }
    }

    /// On a content store that needs an executor, a synchronous method
    /// does nothing at all: no row (not even an artifact's zero-version
    /// one) and no content; its `async` twin, awaited, works.
    #[test]
    fn a_sync_method_on_a_content_store_that_needs_an_executor_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let conn = rusqlite::Connection::open(home.db_path()).unwrap();
        let content = Arc::new(NeedsExecutor(clax_core::content::memory::MemContent::new()));
        let st = Store::open_on(
            &home,
            conn,
            Arc::new(clax_core::working::SystemClock),
            content.clone(),
        )
        .unwrap();
        let first = || {
            clax_core::publish::validate(
                serde_json::from_value(serde_json::json!({
                    "title": "T",
                    "files": {"index.html": {"content": "<p>1", "encoding": "utf8"}}
                }))
                .unwrap(),
            )
            .unwrap()
        };
        let ctx = &AuditCtx::DAEMON;
        would_block(st.create_artifact(ctx, first(), None));
        assert!(st.list_artifacts().unwrap().is_empty());
        assert!(content.0.versions().is_empty());
        let (a, _) =
            clax_core::content::wait(st.create_artifact_async(ctx, first(), None)).unwrap();
        let aid = clax_core::ArtifactId::parse(&a.id).unwrap();
        would_block(st.publish_version(ctx, &aid, next_version(1, "<p>2"), None));
        would_block(st.read_file(&aid, 1, "index.html"));
        assert_eq!(st.get_artifact(&aid).unwrap().unwrap().current_version, 1);
        assert_eq!(content.0.versions().len(), 1);
    }

    /// A content lock another thread holds is waited for by a synchronous
    /// method on one connection, as on the pool: only this thread's own
    /// hold is refused.
    #[test]
    fn a_sync_method_waits_for_a_content_lock_another_thread_holds() {
        let (_d, st) = super::test_util::single_store();
        let aid = artifact(&st, None);
        let held = clax_core::content::wait(st.lock_content(&aid)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let r = st.publish_version(&AuditCtx::DAEMON, &aid, next_version(1, "<p>2"), None);
                tx.send(r.map(|(a, _)| a.current_version)).unwrap();
            });
            assert!(rx.try_recv().is_err(), "waits while the lock is held");
            // This thread's own synchronous publish would never get it.
            would_block(st.publish_version(&AuditCtx::DAEMON, &aid, next_version(1, "<p>x"), None));
            drop(held);
            let n = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
                .unwrap();
            assert_eq!(n, 2);
        });
    }

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
