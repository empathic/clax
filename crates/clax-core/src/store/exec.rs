//! The connections and threads behind [`Store`]: the only write connection,
//! lent to one caller at a time in arrival order; a pool of `query_only`
//! reader connections; worker threads that run [`Store::call`] jobs from a
//! bounded queue; and a background checkpoint thread. See the `store` module doc for
//! how they fit together.

use super::Store;
use crate::{CoreError, Result};
use rusqlite::Connection;
use std::cell::Cell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// How long a connection waits for a lock another process holds (the CLI,
/// `doctor`, a second daemon) before failing with `SQLITE_BUSY`.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one read may run before it is interrupted and fails with
/// [`CoreError::ReadTimeout`].
pub const READ_LIMIT: Duration = Duration::from_secs(10);
/// WAL size, in pages, at which a commit checkpoints by itself. The
/// background checkpoint normally keeps the WAL well below it, so commits
/// rarely pay for a checkpoint; this bounds the WAL when they fall behind.
pub const AUTOCHECKPOINT_PAGES: u32 = 4000;
/// How often the background thread runs a `PASSIVE` checkpoint.
pub const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
/// Most [`Store::call`] jobs queued or running at once; further calls wait
/// (asynchronously, cancellably) for a slot.
pub const QUEUE_CAPACITY: usize = 256;
/// Most reader connections, and the base count of worker threads.
pub const MAX_READERS: usize = 8;
/// Virtual-machine steps between two checks of a read's deadline.
const PROGRESS_STEPS: i32 = 10_000;

/// Reader connections and worker threads: the machine's cores, at least 2
/// and at most [`MAX_READERS`].
pub fn reader_count() -> usize {
    thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, MAX_READERS)
}

/// Opens a connection with the settings every connection shares: a busy
/// timeout and foreign keys.
fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(BUSY_TIMEOUT)?;
    c.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(c)
}

/// The write connection: WAL mode and the autocheckpoint threshold.
pub(crate) fn open_writer(path: &Path) -> Result<Connection> {
    let c = open(path)?;
    c.execute_batch(&format!(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint={AUTOCHECKPOINT_PAGES};"
    ))?;
    Ok(c)
}

/// A connection that refuses every write (`PRAGMA query_only`).
fn open_reader(path: &Path) -> Result<Connection> {
    let c = open(path)?;
    c.execute_batch("PRAGMA query_only=ON;")?;
    Ok(c)
}

/// Milliseconds since a fixed process-wide instant, never 0 (0 means "no
/// deadline" in [`Reader::deadline`]).
fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

// --- the writer ------------------------------------------------------------

thread_local! {
    static ON_WRITER: Cell<bool> = const { Cell::new(false) };
}

/// The write connection while no caller holds it, and the ticket counters
/// that order the callers.
struct Slot {
    conn: Option<Connection>,
    /// The ticket the next caller to arrive takes.
    next: u64,
    /// The ticket whose holder may take the connection.
    serving: u64,
}

/// The only write connection, lent to one caller at a time in arrival
/// order. Each caller takes a ticket, waits for its turn, and runs its job
/// on its own thread with the connection; the connection moves between
/// threads, never a borrow.
pub(crate) struct Writer {
    slot: Mutex<Slot>,
    turn: Condvar,
}

/// A caller's turn with the write connection; ends on drop, including
/// while a panic unwinds, handing the connection to the next ticket.
struct Turn<'a> {
    writer: &'a Writer,
    conn: Option<Connection>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        ON_WRITER.with(|w| w.set(false));
        let mut slot = self.writer.lock();
        slot.conn = self.conn.take();
        slot.serving += 1;
        drop(slot);
        self.writer.turn.notify_all();
    }
}

impl Writer {
    /// Holds `conn` until the store drops.
    pub(crate) fn new(conn: Connection) -> Writer {
        Writer {
            slot: Mutex::new(Slot {
                conn: Some(conn),
                next: 0,
                serving: 0,
            }),
            turn: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Slot> {
        lock(&self.slot)
    }

    /// Runs `f` with the write connection, after every caller that arrived
    /// before this one has finished, and returns its result. Blocks the
    /// calling thread until its turn; `f` runs on the calling thread, so a
    /// panic in `f` unwinds there, and the connection passes to the next
    /// caller either way. On a worker thread, the call counts as a write for
    /// the whole wait and run (see [`Workers`]).
    pub(crate) fn run<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        assert!(
            !ON_WRITER.with(Cell::get),
            "a write job may not start another write job"
        );
        let _lane = WriteLane::enter()?;
        let mut slot = self.lock();
        let ticket = slot.next;
        slot.next += 1;
        while slot.serving != ticket {
            slot = self.turn.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
        let conn = slot
            .conn
            .take()
            .expect("the connection is back between turns");
        drop(slot);
        let mut turn = Turn {
            writer: self,
            conn: Some(conn),
        };
        ON_WRITER.with(|w| w.set(true));
        f(turn.conn.as_mut().expect("a turn holds the connection"))
    }
}

// --- the readers -----------------------------------------------------------

struct Reader {
    conn: Connection,
    /// When the running read must stop ([`now_ms`]), or 0 for no deadline.
    deadline: Arc<AtomicU64>,
}

#[derive(Default)]
struct Idle {
    readers: Vec<Reader>,
    opened: usize,
}

/// Up to `max` `query_only` connections, opened on first use.
pub(crate) struct Readers {
    path: PathBuf,
    max: usize,
    /// The read limit in milliseconds; 0 for none.
    limit_ms: AtomicU64,
    idle: Mutex<Idle>,
    freed: Condvar,
}

/// A checked-out reader, returned to the pool on drop. A read transaction
/// still open (the read panicked) is rolled back first.
struct Lease<'a> {
    pool: &'a Readers,
    reader: Option<Reader>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(r) = self.reader.take() {
            r.deadline.store(0, Ordering::Relaxed);
            if !r.conn.is_autocommit() {
                let _ = r.conn.execute_batch("ROLLBACK");
            }
            lock(&self.pool.idle).readers.push(r);
            self.pool.freed.notify_one();
        }
    }
}

/// `m`'s lock. No lock in this module is held across caller code, so a
/// poisoned lock still guards consistent state.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Readers {
    pub(crate) fn new(path: PathBuf, max: usize) -> Readers {
        Readers {
            path,
            max,
            limit_ms: AtomicU64::new(READ_LIMIT.as_millis() as u64),
            idle: Mutex::new(Idle::default()),
            freed: Condvar::new(),
        }
    }

    pub(crate) fn max(&self) -> usize {
        self.max
    }

    /// Sets the read limit; `None` lifts it.
    pub(crate) fn set_limit(&self, limit: Option<Duration>) {
        let ms = limit.map_or(0, |l| (l.as_millis() as u64).max(1));
        self.limit_ms.store(ms, Ordering::Relaxed);
    }

    /// An idle reader, opening one when none is idle and fewer than `max`
    /// are open, else waiting for one to come back.
    fn lease(&self) -> Result<Lease<'_>> {
        let mut idle = lock(&self.idle);
        loop {
            if let Some(r) = idle.readers.pop() {
                return Ok(Lease {
                    pool: self,
                    reader: Some(r),
                });
            }
            if idle.opened < self.max {
                idle.opened += 1;
                drop(idle);
                return match self.open_one() {
                    Ok(r) => Ok(Lease {
                        pool: self,
                        reader: Some(r),
                    }),
                    Err(e) => {
                        lock(&self.idle).opened -= 1;
                        self.freed.notify_one();
                        Err(e)
                    }
                };
            }
            idle = self
                .freed
                .wait(idle)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn open_one(&self) -> Result<Reader> {
        let conn = open_reader(&self.path)?;
        let deadline = Arc::new(AtomicU64::new(0));
        let d = deadline.clone();
        conn.progress_handler(
            PROGRESS_STEPS,
            Some(move || {
                let at = d.load(Ordering::Relaxed);
                at != 0 && now_ms() > at
            }),
        );
        Ok(Reader { conn, deadline })
    }

    /// Runs `f` on a reader, in one read transaction, so every statement in
    /// `f` sees the same snapshot of the database. With `limited`, a read
    /// still running after the read limit is interrupted and the call fails
    /// with [`CoreError::ReadTimeout`].
    ///
    /// # Panics
    /// When called inside a write job: the write holds the connection that
    /// sees its own uncommitted rows, and a reader would not.
    pub(crate) fn run<T>(
        &self,
        limited: bool,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        assert!(
            !ON_WRITER.with(Cell::get),
            "a write job may not read through a reader connection"
        );
        let lease = self.lease()?;
        let r = lease.reader.as_ref().expect("a lease holds a reader");
        let limit = self.limit_ms.load(Ordering::Relaxed);
        if limited && limit != 0 {
            r.deadline.store(now_ms() + limit, Ordering::Relaxed);
        }
        let mut out = r
            .conn
            .execute_batch("BEGIN")
            .map_err(CoreError::from)
            .and_then(|()| f(&r.conn));
        let deadline = r.deadline.swap(0, Ordering::Relaxed);
        let timed_out = deadline != 0 && now_ms() > deadline;
        // An interrupted statement may already have ended the transaction.
        if !r.conn.is_autocommit() {
            let end = if out.is_ok() { "COMMIT" } else { "ROLLBACK" };
            if let Err(e) = r.conn.execute_batch(end)
                && out.is_ok()
            {
                out = Err(e.into());
            }
        }
        drop(lease);
        out.map_err(|e| match e {
            CoreError::Db(rusqlite::Error::SqliteFailure(f, _))
                if timed_out && f.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                CoreError::ReadTimeout
            }
            e => e,
        })
    }
}

// --- the workers -----------------------------------------------------------

type Task = Box<dyn FnOnce() + Send>;

/// How long a worker beyond the base count waits idle before it exits.
const SPARE_IDLE: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Queue {
    tasks: VecDeque<Task>,
    closed: bool,
    /// Worker threads running.
    threads: usize,
    /// Of those, the ones inside a write: waiting for the write turn or
    /// holding it.
    writing: usize,
    /// Names the next worker thread.
    spawned: usize,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Workers that are kept free of writes, so reads always have them.
    base: usize,
    handles: Mutex<Vec<thread::JoinHandle<()>>>,
}

thread_local! {
    /// The pool a worker thread belongs to.
    static WORKER: std::cell::RefCell<Option<Arc<Shared>>> =
        const { std::cell::RefCell::new(None) };
}

impl Shared {
    /// Starts one more worker thread; `q` is this pool's locked queue.
    fn spawn_worker(self: &Arc<Self>, q: &mut Queue) -> Result<()> {
        let shared = self.clone();
        let h = thread::Builder::new()
            .name(format!("clax-db-{}", q.spawned))
            .spawn(move || work(shared))?;
        q.spawned += 1;
        q.threads += 1;
        let mut handles = lock(&self.handles);
        handles.retain(|h| !h.is_finished());
        handles.push(h);
        Ok(())
    }
}

/// A worker thread's stay inside a write. While it lasts the thread counts
/// as writing, and the pool starts another worker if fewer than its base
/// count would otherwise be free, so jobs waiting on the writer never hold
/// every worker and reads keep running behind a slow write.
struct WriteLane(Option<Arc<Shared>>);

impl WriteLane {
    fn enter() -> Result<WriteLane> {
        let Some(shared) = WORKER.with(|w| w.borrow().clone()) else {
            return Ok(WriteLane(None));
        };
        let mut q = lock(&shared.queue);
        q.writing += 1;
        if !q.closed
            && q.threads - q.writing < shared.base
            && let Err(e) = shared.spawn_worker(&mut q)
        {
            q.writing -= 1;
            return Err(e);
        }
        drop(q);
        Ok(WriteLane(Some(shared)))
    }
}

impl Drop for WriteLane {
    fn drop(&mut self) {
        if let Some(shared) = &self.0 {
            lock(&shared.queue).writing -= 1;
        }
    }
}

/// Worker threads taking [`Store::call`] jobs in FIFO order, plus the
/// background checkpoint thread. `base` workers start with the pool; a job
/// that writes may add more (see [`WriteLane`]), and an extra worker idle
/// for [`SPARE_IDLE`] exits. Started on the first call; stopped by
/// [`Store::shutdown`], or when the store drops.
pub(crate) struct Workers {
    shared: Arc<Shared>,
    slots: Arc<tokio::sync::Semaphore>,
    stop_checkpoints: Mutex<Option<mpsc::Sender<()>>>,
    checkpointer: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Workers {
    pub(crate) fn start(base: usize, db: &Path) -> Result<Workers> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            base,
            handles: Mutex::new(Vec::new()),
        });
        let (stop, stopped) = mpsc::channel();
        let path = db.to_path_buf();
        let checkpointer = thread::Builder::new()
            .name("clax-db-checkpoint".into())
            .spawn(move || checkpoints(&path, &stopped))?;
        let workers = Workers {
            shared,
            slots: Arc::new(tokio::sync::Semaphore::new(QUEUE_CAPACITY)),
            stop_checkpoints: Mutex::new(Some(stop)),
            checkpointer: Mutex::new(Some(checkpointer)),
        };
        {
            let mut q = lock(&workers.shared.queue);
            for _ in 0..base {
                // On failure, dropping `workers` stops what started.
                workers.shared.spawn_worker(&mut q)?;
            }
        }
        Ok(workers)
    }

    /// Queues `task`; false once the pool is closed.
    fn push(&self, task: Task) -> bool {
        let mut q = lock(&self.shared.queue);
        if q.closed {
            return false;
        }
        q.tasks.push_back(task);
        drop(q);
        self.shared.ready.notify_one();
        true
    }

    /// Refuses further jobs and stops the checkpoint thread; queued jobs
    /// still run.
    fn close(&self) {
        lock(&self.shared.queue).closed = true;
        self.shared.ready.notify_all();
        lock(&self.stop_checkpoints).take();
    }

    /// Waits for the checkpoint thread to stop.
    fn join_checkpointer(&self) {
        if let Some(h) = lock(&self.checkpointer).take() {
            let _ = h.join();
        }
    }

    /// Closes the pool, then waits for every queued job to finish and every
    /// thread to exit.
    fn drain(&self) {
        self.close();
        self.join_checkpointer();
        let me = thread::current().id();
        loop {
            let handles = std::mem::take(&mut *lock(&self.shared.handles));
            if handles.is_empty() {
                break;
            }
            for h in handles {
                if h.thread().id() != me {
                    let _ = h.join();
                }
            }
        }
    }
}

impl Drop for Workers {
    /// Closes the pool and joins the checkpoint thread. The workers exit on
    /// their own once the queue is empty; they are not joined here, since
    /// the last reference to the store may be dropped on one of them.
    fn drop(&mut self) {
        self.close();
        self.join_checkpointer();
    }
}

fn work(shared: Arc<Shared>) {
    WORKER.with(|w| *w.borrow_mut() = Some(shared.clone()));
    loop {
        let task = {
            let mut q = lock(&shared.queue);
            loop {
                if let Some(t) = q.tasks.pop_front() {
                    break t;
                }
                if q.closed {
                    q.threads -= 1;
                    return;
                }
                let spare = q.threads - q.writing > shared.base;
                if !spare {
                    q = shared.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
                let (next, waited) = shared
                    .ready
                    .wait_timeout(q, SPARE_IDLE)
                    .unwrap_or_else(PoisonError::into_inner);
                q = next;
                if waited.timed_out() && q.tasks.is_empty() && q.threads - q.writing > shared.base {
                    q.threads -= 1;
                    return;
                }
            }
        };
        task();
    }
}

/// Runs a `PASSIVE` checkpoint every [`CHECKPOINT_INTERVAL`] on its own
/// `query_only` connection (a checkpoint copies committed pages into the
/// database file; it changes no content), until `stop` closes.
fn checkpoints(db: &Path, stop: &mpsc::Receiver<()>) {
    let conn = match open_reader(db) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "background checkpoints are off");
            return;
        }
    };
    while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(CHECKPOINT_INTERVAL) {
        if let Err(e) = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())) {
            tracing::warn!(error = %e, "background checkpoint failed");
        }
    }
}

impl Store {
    /// Runs `f` on one of the store's worker threads and returns its result.
    ///
    /// At most [`QUEUE_CAPACITY`] calls are queued or running; a further
    /// call waits for a slot without holding a thread. Jobs start in the
    /// order they were queued. A job whose caller has gone (its future was
    /// dropped, as a request timeout does) before a worker reaches it is
    /// skipped; once started, it runs to completion. A panic in `f` is
    /// logged and returned as [`CoreError::TaskFailed`], as is a call after
    /// [`Store::shutdown`] or one whose worker thread could not start.
    pub async fn call<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> Result<T> + Send + 'static,
    {
        if self.shut_down.load(Ordering::SeqCst) {
            return Err(CoreError::TaskFailed);
        }
        let workers = match self.workers.get() {
            Some(w) => w,
            None => {
                let w = Workers::start(self.readers.max(), &self.home.db_path())?;
                // A concurrent first call may have won; its pool is kept.
                let _ = self.workers.set(w);
                self.workers.get().expect("set above")
            }
        };
        let permit = workers
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("the slots are never closed");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let store = Arc::clone(self);
        let queued = workers.push(Box::new(move || {
            let _permit = permit;
            if tx.is_closed() {
                return;
            }
            let out = catch_unwind(AssertUnwindSafe(|| f(&store)));
            let _ = tx.send(out);
        }));
        if !queued {
            return Err(CoreError::TaskFailed);
        }
        match rx.await {
            Ok(Ok(out)) => out,
            Ok(Err(panic)) => {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                tracing::error!(panic = %msg, "store task panicked");
                Err(CoreError::TaskFailed)
            }
            Err(_) => Err(CoreError::TaskFailed),
        }
    }

    /// Drains the store: further [`Store::call`]s fail with
    /// [`CoreError::TaskFailed`], every call already queued runs, and the
    /// worker and checkpoint threads are joined. A write in progress, which
    /// runs on a worker, has finished when this returns. Blocks; must not be
    /// called from inside a [`Store::call`] job.
    pub fn shutdown(&self) {
        self.shut_down.store(true, Ordering::SeqCst);
        if let Some(w) = self.workers.get() {
            w.drain();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::store;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicBool;

    const INSERT_VIEWER: &str = "INSERT INTO viewers (id, public_id, display_name, created_at)
         VALUES ('v1', 'u_00000000000000000000001', 'Ana', '2026-01-01T00:00:00.000Z')";

    fn viewers(st: &Store) -> i64 {
        st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn sqlite_gives_each_connection_its_own_page_cache() {
        // Built per .cargo/config.toml: a shared page cache would serialise
        // the readers on one global mutex.
        let (_d, st) = store();
        let opts: Vec<String> = st
            .with_read(|c| {
                let mut stmt = c.prepare("PRAGMA compile_options")?;
                Ok(stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap();
        assert!(
            !opts.iter().any(|o| o == "ENABLE_MEMORY_MANAGEMENT"),
            "{opts:?}"
        );
        assert!(opts.iter().any(|o| o == "DEFAULT_MEMSTATUS=0"), "{opts:?}");
    }

    #[test]
    fn readers_refuse_writes() {
        let (_d, st) = store();
        let e = st
            .with_read(|c| Ok(c.execute(INSERT_VIEWER, [])?))
            .unwrap_err();
        assert!(
            matches!(&e, CoreError::Db(rusqlite::Error::SqliteFailure(f, _)) if f.code == rusqlite::ErrorCode::ReadOnly),
            "{e:?}"
        );
        assert_eq!(viewers(&st), 0);
        st.with_tx(|tx| Ok(tx.execute(INSERT_VIEWER, [])?)).unwrap();
        assert_eq!(viewers(&st), 1);
    }

    #[test]
    fn a_read_past_the_limit_is_interrupted() {
        let (_d, st) = store();
        st.set_read_limit(Duration::from_millis(100));
        let forever = "WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM r) SELECT COUNT(*) FROM r";
        let started = Instant::now();
        let e = st
            .with_read(|c| Ok(c.query_row(forever, [], |r| r.get::<_, i64>(0))?))
            .unwrap_err();
        assert!(matches!(e, CoreError::ReadTimeout), "{e:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        // The reader is usable again, and integrity checks have no limit.
        assert_eq!(viewers(&st), 0);
        assert_eq!(st.integrity_check().unwrap(), "ok");
    }

    #[test]
    fn reads_run_while_a_write_is_in_progress() {
        let (_d, st) = store();
        let inside = Barrier::new(2);
        let release = Barrier::new(2);
        thread::scope(|s| {
            s.spawn(|| {
                st.with_tx(|tx| {
                    tx.execute(INSERT_VIEWER, [])?;
                    inside.wait();
                    release.wait();
                    Ok(())
                })
                .unwrap();
            });
            inside.wait();
            // The write is open and uncommitted: a read neither waits for it
            // nor sees it.
            assert_eq!(viewers(&st), 0);
            release.wait();
        });
        assert_eq!(viewers(&st), 1);
    }

    #[test]
    fn writes_run_one_at_a_time_in_arrival_order() {
        let (_d, st) = store();
        let order = Mutex::new(Vec::new());
        let running = std::sync::atomic::AtomicUsize::new(0);
        let overlapped = AtomicBool::new(false);
        let first_in = Barrier::new(2);
        let release = AtomicBool::new(false);
        let job = |i: usize| {
            if running.fetch_add(1, Ordering::SeqCst) != 0 {
                overlapped.store(true, Ordering::SeqCst);
            }
            order.lock().unwrap().push(i);
            running.fetch_sub(1, Ordering::SeqCst);
        };
        thread::scope(|s| {
            s.spawn(|| {
                st.with_tx(|_| {
                    first_in.wait();
                    while !release.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    job(0);
                    Ok(())
                })
                .unwrap();
            });
            first_in.wait();
            for i in 1..=5 {
                let (st, job) = (&st, &job);
                s.spawn(move || {
                    st.with_tx(|_| {
                        job(i);
                        Ok(())
                    })
                    .unwrap();
                });
                // Each caller arrives before the next one.
                thread::sleep(Duration::from_millis(20));
            }
            release.store(true, Ordering::SeqCst);
        });
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
        assert!(!overlapped.load(Ordering::SeqCst));
    }

    #[test]
    #[should_panic(expected = "may not start another write job")]
    fn a_write_inside_a_write_is_refused() {
        let (_d, st) = store();
        let _ = st.with_tx(|_| st.with_write(|_| Ok(())));
    }

    #[test]
    fn one_read_sees_one_snapshot() {
        let (_d, st) = store();
        let (before, after) = st
            .with_read(|c| {
                let count = || -> Result<i64> {
                    Ok(c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))?)
                };
                let before = count()?;
                // A write commits between the read's two statements.
                st.with_tx(|tx| Ok(tx.execute(INSERT_VIEWER, [])?))?;
                Ok((before, count()?))
            })
            .unwrap();
        assert_eq!((before, after), (0, 0));
        assert_eq!(viewers(&st), 1, "a later read sees the write");
    }

    #[test]
    fn a_panicking_read_leaves_no_open_transaction() {
        let (_d, st) = store();
        for _ in 0..st.readers.max() {
            let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
                st.with_read(|c| -> Result<()> {
                    c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get::<_, i64>(0))?;
                    panic!("boom")
                })
            }));
            assert!(r.is_err());
        }
        st.with_tx(|tx| Ok(tx.execute(INSERT_VIEWER, [])?)).unwrap();
        // Every reader was handed back with its snapshot released.
        for _ in 0..st.readers.max() {
            assert_eq!(viewers(&st), 1);
        }
    }

    #[test]
    #[should_panic(expected = "may not read through a reader connection")]
    fn a_read_inside_a_write_is_refused() {
        let (_d, st) = store();
        let _ = st.with_tx(|_| st.with_read(|_| Ok(())));
    }

    #[test]
    fn a_lifted_read_limit_lets_a_long_read_finish() {
        let (_d, st) = store();
        st.set_read_limit(Duration::from_millis(1));
        st.lift_read_limit();
        let long = "WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM r WHERE x < 2000000) SELECT COUNT(*) FROM r";
        let n = st
            .with_read(|c| Ok(c.query_row(long, [], |r| r.get::<_, i64>(0))?))
            .unwrap();
        assert_eq!(n, 2_000_000);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_keep_running_while_writes_wait_on_a_lock() {
        let (_d, st) = store();
        let st = Arc::new(st);
        // Another process holds the write lock, so the first write waits on
        // it (busy timeout) and the rest wait for their turn.
        let other = Connection::open(st.home().db_path()).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let n = st.readers.max() + 2;
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writes: Vec<_> = (0..n)
            .map(|i| {
                let (st, started) = (st.clone(), started.clone());
                tokio::spawn(async move {
                    st.call(move |s| {
                        started.fetch_add(1, Ordering::SeqCst);
                        s.with_tx(|tx| {
                            Ok(tx.execute(
                                "INSERT INTO viewers (id, public_id, display_name, created_at)
                                 VALUES (?1, ?2, 'Ana', '2026-01-01T00:00:00.000Z')",
                                rusqlite::params![format!("v{i}"), format!("u_{i:023}")],
                            )?)
                        })
                    })
                    .await
                })
            })
            .collect();
        // More writes than base workers all start: waiting writes do not
        // hold the workers reads need.
        let deadline = Instant::now() + Duration::from_secs(2);
        while started.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < deadline, "writes hold every worker");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let t0 = Instant::now();
        let read =
            tokio::time::timeout(Duration::from_secs(1), st.call(|s| s.integrity_check())).await;
        assert_eq!(
            read.expect("the read ran behind the waiting writes")
                .unwrap(),
            "ok"
        );
        assert!(t0.elapsed() < Duration::from_secs(1));
        other.execute_batch("COMMIT").unwrap();
        for w in writes {
            w.await.unwrap().unwrap();
        }
        assert_eq!(viewers(&st), n as i64);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_drains_queued_calls_and_refuses_new_ones() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let go = Arc::new(AtomicBool::new(false));
        let n = st.readers.max() * 3;
        let calls: Vec<_> = (0..n)
            .map(|_| {
                let (st, done, go) = (st.clone(), done.clone(), go.clone());
                tokio::spawn(async move {
                    st.call(move |s| {
                        while !go.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(1));
                        }
                        thread::sleep(Duration::from_millis(10));
                        done.fetch_add(1, Ordering::SeqCst);
                        s.integrity_check()
                    })
                    .await
                })
            })
            .collect();
        // Every call is queued (holds a slot) before the drain starts; none
        // finishes before `go`.
        let queued = || {
            st.workers
                .get()
                .map_or(0, |w| QUEUE_CAPACITY - w.slots.available_permits())
        };
        while queued() < n {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        go.store(true, Ordering::SeqCst);
        let s = st.clone();
        tokio::task::spawn_blocking(move || s.shutdown())
            .await
            .unwrap();
        assert_eq!(done.load(Ordering::SeqCst), n, "every queued call ran");
        for c in calls {
            assert_eq!(c.await.unwrap().unwrap(), "ok");
        }
        let e = st.call(|s| s.integrity_check()).await.unwrap_err();
        assert!(matches!(e, CoreError::TaskFailed), "{e:?}");
        let threads = lock(&st.workers.get().unwrap().shared.queue).threads;
        assert_eq!(threads, 0, "every worker exited");
    }

    #[test]
    fn a_panicking_write_reaches_the_caller_and_the_writer_survives() {
        let (_d, st) = store();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
            st.with_tx(|tx| -> Result<()> {
                tx.execute(INSERT_VIEWER, [])?;
                panic!("boom")
            })
        }));
        assert!(r.is_err());
        assert_eq!(viewers(&st), 0, "the panicking transaction rolled back");
        st.with_tx(|tx| Ok(tx.execute(INSERT_VIEWER, [])?)).unwrap();
        assert_eq!(viewers(&st), 1);
    }

    #[test]
    fn a_write_waits_for_another_process_lock_instead_of_failing() {
        let (_d, st) = store();
        let other = Connection::open(st.home().db_path()).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        thread::scope(|s| {
            s.spawn(move || {
                thread::sleep(Duration::from_millis(300));
                other.execute_batch("COMMIT").unwrap();
            });
            // Reading before writing: the write lock is still waited for.
            st.with_tx(|tx| {
                tx.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get::<_, i64>(0))?;
                Ok(tx.execute(INSERT_VIEWER, [])?)
            })
            .unwrap();
        });
        assert_eq!(viewers(&st), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_call_whose_caller_gave_up_is_skipped() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let n = st.readers.max();
        // Occupy every worker.
        let gate = Arc::new(Barrier::new(n + 1));
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let busy: Vec<_> = (0..n)
            .map(|_| {
                let (gate, started, st) = (gate.clone(), started.clone(), st.clone());
                tokio::spawn(async move {
                    st.call(move |_| {
                        started.fetch_add(1, Ordering::SeqCst);
                        gate.wait();
                        Ok(())
                    })
                    .await
                })
            })
            .collect();
        while started.load(Ordering::SeqCst) < n {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let gave_up = tokio::time::timeout(
            Duration::from_millis(50),
            st.call(move |_| {
                r.store(true, Ordering::SeqCst);
                Ok(())
            }),
        )
        .await;
        assert!(gave_up.is_err(), "the call timed out while queued");
        gate.wait();
        for b in busy {
            b.await.unwrap().unwrap();
        }
        // A later call runs, after the skipped one would have.
        let after = st.call(|s| s.integrity_check()).await.unwrap();
        assert_eq!(after, "ok");
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_panicking_call_fails_cleanly() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let e = st
            .call(|_| -> Result<()> { panic!("boom") })
            .await
            .unwrap_err();
        assert!(matches!(e, CoreError::TaskFailed));
        assert_eq!(st.call(|s| s.integrity_check()).await.unwrap(), "ok");
    }
}
