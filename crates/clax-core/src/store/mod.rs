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
//! whose request has given up is skipped. A job waiting for the write turn
//! or holding it does not count against the workers that reads need: while
//! fewer than one per reader would be free of writes, the pool starts
//! another. [`Store::shutdown`] drains the queue and joins the threads.
//!
//! Version writes, deletes, and asset writes combine a transaction with
//! file-system work, ordered so that a failure on either side leaves no
//! half-written state.

pub mod artifacts;
pub mod assets;
pub mod attention;
pub mod batches;
pub mod changelog;
pub mod docs;
pub mod exec;
pub mod extension;
pub mod feedback;
pub mod joined;
pub mod live;
pub mod migrations;
#[cfg(test)]
mod plans;
pub mod sessions;
pub mod site;
pub mod threads;
pub mod viewers;
pub mod watches;

use crate::{CoreError, Home, Result};
use exec::{Readers, Workers, Writer};
use rusqlite::Connection;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

/// Rows `ANALYZE` samples per index when `PRAGMA optimize` refreshes statistics.
pub const ANALYSIS_LIMIT: u32 = 400;
/// How often the daemon refreshes planner statistics ([`Store::optimize`]).
pub const OPTIMIZE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

pub struct Store {
    home: Home,
    writer: Writer,
    readers: Readers,
    workers: OnceLock<Workers>,
    /// Set by [`Store::shutdown`].
    shut_down: AtomicBool,
}

impl Store {
    /// Opens (creating when missing) the home's database and migrates it to
    /// the current schema on the write connection.
    pub fn open(home: &Home) -> Result<Store> {
        home.ensure_dirs()?;
        let path = home.db_path();
        let mut conn = exec::open_writer(&path)?;
        migrate(&mut conn)?;
        conn.execute_batch(&format!(
            "PRAGMA analysis_limit={ANALYSIS_LIMIT}; PRAGMA optimize=0x10002;"
        ))?;
        Ok(Store {
            home: home.clone(),
            writer: Writer::new(conn),
            readers: Readers::new(path, exec::reader_count()),
            workers: OnceLock::new(),
            shut_down: AtomicBool::new(false),
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

    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
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
    pub(crate) fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.writer.run(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })
    }

    /// Runs `f` on the write connection outside a transaction, for
    /// statements that manage their own (`PRAGMA optimize`).
    pub(crate) fn with_write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.writer.run(|conn| f(conn))
    }
}

/// Applies every migration past the database's `user_version`, each in its
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
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

#[cfg(test)]
pub(crate) mod test_util {
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
            .create_artifact(validate(req).unwrap(), session)
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
        let (a, _) = store.create_artifact(validate(req).unwrap(), None).unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    /// A live session of `harness` with harness session ID `hsid`; returns its ID.
    pub fn session(store: &Store, harness: &str, hsid: &str) -> String {
        store
            .register_session(RegisterSession {
                harness: harness.into(),
                harness_session_id: Some(hsid.into()),
                cwd: "/w".into(),
                pid: None,
                parent_pid: None,
            })
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
