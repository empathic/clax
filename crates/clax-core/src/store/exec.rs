//! The connections and threads behind [`Store`]: one writer thread that owns
//! the only write connection, a pool of `query_only` reader connections, a
//! fixed set of worker threads that run [`Store::call`] jobs from a bounded
//! queue, and a background checkpoint thread. See the `store` module doc for
//! how they fit together.

use super::Store;
use crate::{CoreError, Result};
use rusqlite::Connection;
use std::cell::Cell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long a connection waits for a lock another process holds (the CLI,
/// `doctor`, a second daemon) before failing with `SQLITE_BUSY`.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
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
/// Most reader connections and worker threads.
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

/// A job for the writer thread.
trait WriteJob: Send {
    fn run(self: Box<Self>, conn: &mut Connection);
}

/// `f` and the channel its outcome goes back on. Fields drop in declaration
/// order, so `f` (and everything it borrows) is gone before `done` closes:
/// the caller waiting on `done` can rely on that whether the job ran or was
/// dropped unrun.
struct Job<F, T> {
    f: Option<F>,
    done: mpsc::SyncSender<thread::Result<T>>,
}

impl<F, T> WriteJob for Job<F, T>
where
    F: FnOnce(&mut Connection) -> T + Send,
    T: Send,
{
    fn run(mut self: Box<Self>, conn: &mut Connection) {
        let f = self.f.take().expect("a job runs once");
        let out = catch_unwind(AssertUnwindSafe(move || f(conn)));
        let _ = self.done.send(out);
    }
}

/// The writer thread and the FIFO channel it takes jobs from.
pub(crate) struct Writer {
    jobs: Option<mpsc::Sender<Box<dyn WriteJob>>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    /// Starts the writer thread, which owns `conn` until the store drops.
    pub(crate) fn start(mut conn: Connection) -> Result<Writer> {
        let (jobs, rx) = mpsc::channel::<Box<dyn WriteJob>>();
        let thread = thread::Builder::new()
            .name("clax-db-write".into())
            .spawn(move || {
                ON_WRITER.with(|w| w.set(true));
                while let Ok(job) = rx.recv() {
                    job.run(&mut conn);
                }
            })?;
        Ok(Writer {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    /// Runs `f` on the writer thread, after every job queued before it, and
    /// returns its result. Blocks the calling thread until `f` has finished;
    /// a panic in `f` resumes on the calling thread.
    pub(crate) fn run<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send,
        T: Send,
    {
        assert!(
            !ON_WRITER.with(Cell::get),
            "a write job may not queue another write job"
        );
        let (done, outcome) = mpsc::sync_channel(1);
        let job: Box<dyn WriteJob + '_> = Box::new(Job { f: Some(f), done });
        // SAFETY: the job may borrow from this stack frame, so it must not
        // outlive this call. It doesn't: this function returns only after
        // `outcome` yields, which happens after the writer has run the job
        // (consuming `f`) or once `done` has been dropped, and `Job` drops
        // `f` before `done`. Nothing between the send and the receive below
        // can unwind. The two box types differ only in the lifetime bound.
        let job: Box<dyn WriteJob + 'static> = unsafe { std::mem::transmute(job) };
        let jobs = self.jobs.as_ref().expect("the writer runs until drop");
        if let Err(mpsc::SendError(job)) = jobs.send(job) {
            drop(job);
            return Err(CoreError::TaskFailed);
        }
        match outcome.recv() {
            Ok(Ok(out)) => out,
            Ok(Err(panic)) => resume_unwind(panic),
            Err(mpsc::RecvError) => Err(CoreError::TaskFailed),
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // No caller can still be waiting: each holds a borrow of the store.
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// --- the readers -----------------------------------------------------------

struct Reader {
    conn: Connection,
    /// When the running read must stop ([`now_ms`]), or 0 when idle.
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
    limit_ms: AtomicU64,
    idle: Mutex<Idle>,
    freed: Condvar,
}

/// A checked-out reader, returned to the pool on drop.
struct Lease<'a> {
    pool: &'a Readers,
    reader: Option<Reader>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(r) = self.reader.take() {
            r.deadline.store(0, Ordering::Relaxed);
            self.pool.idle.lock().unwrap().readers.push(r);
            self.pool.freed.notify_one();
        }
    }
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

    pub(crate) fn set_limit(&self, limit: Duration) {
        self.limit_ms
            .store(limit.as_millis() as u64, Ordering::Relaxed);
    }

    /// An idle reader, opening one when none is idle and fewer than `max`
    /// are open, else waiting for one to come back.
    fn lease(&self) -> Result<Lease<'_>> {
        let mut idle = self.idle.lock().unwrap();
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
                        self.idle.lock().unwrap().opened -= 1;
                        self.freed.notify_one();
                        Err(e)
                    }
                };
            }
            idle = self.freed.wait(idle).unwrap();
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

    /// Runs `f` on a reader. With `limited`, a read still running after the
    /// read limit is interrupted and the call fails with
    /// [`CoreError::ReadTimeout`].
    pub(crate) fn run<T>(
        &self,
        limited: bool,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let lease = self.lease()?;
        let r = lease.reader.as_ref().expect("a lease holds a reader");
        if limited {
            let limit = self.limit_ms.load(Ordering::Relaxed);
            r.deadline.store(now_ms() + limit, Ordering::Relaxed);
        }
        let out = f(&r.conn);
        let timed_out = limited && now_ms() > r.deadline.load(Ordering::Relaxed);
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

#[derive(Default)]
struct Queue {
    tasks: VecDeque<Task>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

/// Fixed worker threads taking [`Store::call`] jobs in FIFO order, plus the
/// background checkpoint thread. Started on the first call; stopped when the
/// store drops.
pub(crate) struct Workers {
    shared: Arc<Shared>,
    slots: Arc<tokio::sync::Semaphore>,
    stop_checkpoints: Option<mpsc::Sender<()>>,
}

impl Workers {
    pub(crate) fn start(n: usize, db: &Path) -> Workers {
        let shared = Arc::new(Shared::default());
        for i in 0..n {
            let shared = shared.clone();
            thread::Builder::new()
                .name(format!("clax-db-{i}"))
                .spawn(move || work(&shared))
                .expect("spawn a database worker thread");
        }
        let (stop, stopped) = mpsc::channel();
        let path = db.to_path_buf();
        thread::Builder::new()
            .name("clax-db-checkpoint".into())
            .spawn(move || checkpoints(&path, &stopped))
            .expect("spawn the checkpoint thread");
        Workers {
            shared,
            slots: Arc::new(tokio::sync::Semaphore::new(QUEUE_CAPACITY)),
            stop_checkpoints: Some(stop),
        }
    }

    fn push(&self, task: Task) {
        self.shared.queue.lock().unwrap().tasks.push_back(task);
        self.shared.ready.notify_one();
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.stop_checkpoints.take();
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.ready.notify_all();
    }
}

fn work(shared: &Shared) {
    loop {
        let task = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if let Some(t) = q.tasks.pop_front() {
                    break t;
                }
                if q.closed {
                    return;
                }
                q = shared.ready.wait(q).unwrap();
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
    /// logged and returned as [`CoreError::TaskFailed`].
    pub async fn call<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> Result<T> + Send + 'static,
    {
        let workers = self
            .workers
            .get_or_init(|| Workers::start(self.readers.max(), &self.home.db_path()));
        let permit = workers
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("the slots are never closed");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let store = Arc::clone(self);
        workers.push(Box::new(move || {
            let _permit = permit;
            if tx.is_closed() {
                return;
            }
            let out = catch_unwind(AssertUnwindSafe(|| f(&store)));
            let _ = tx.send(out);
        }));
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
    fn writes_run_in_arrival_order_on_one_thread() {
        let (_d, st) = store();
        let order = Mutex::new(Vec::new());
        let names = Mutex::new(std::collections::HashSet::new());
        let first_in = Barrier::new(2);
        let release = AtomicBool::new(false);
        thread::scope(|s| {
            s.spawn(|| {
                st.with_tx(|_| {
                    first_in.wait();
                    while !release.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    order.lock().unwrap().push(0);
                    names
                        .lock()
                        .unwrap()
                        .insert(thread::current().name().map(str::to_owned));
                    Ok(())
                })
                .unwrap();
            });
            first_in.wait();
            for i in 1..=5 {
                let (st, order, names) = (&st, &order, &names);
                s.spawn(move || {
                    st.with_tx(|_| {
                        order.lock().unwrap().push(i);
                        names
                            .lock()
                            .unwrap()
                            .insert(thread::current().name().map(str::to_owned));
                        Ok(())
                    })
                    .unwrap();
                });
                // Each job is queued before the next is sent.
                thread::sleep(Duration::from_millis(20));
            }
            release.store(true, Ordering::SeqCst);
        });
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
        let names = names.into_inner().unwrap();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&Some("clax-db-write".to_string())));
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
