//! A SQLite VFS that keeps each database file's bytes in a [`PageStore`] as
//! fixed-size chunks, and SQLite's journals and temporary files in memory,
//! for a native host whose connections move between threads (a store's
//! jobs run on whichever thread calls them).
//!
//! It keeps the rules of the Durable Object VFS (`crates/clax-worker/vfs`),
//! whose module docs give the reasoning at length:
//!
//! - Writes to a database file go to an in-memory dirty set, handed whole to
//!   [`PageStore::commit`] when SQLite syncs the file (at each commit, and
//!   at a rollback that restores pages a cache spill wrote), which the store
//!   applies atomically. The store therefore only ever holds the database as
//!   of a committed transaction, and journals live in memory.
//! - A commit that shrinks the file truncates it after its sync; a
//!   truncation the store refuses stays pending for the next commit.
//! - Reads come from the dirty set, then from the store. SQLite's own page
//!   cache, which [`open`] sizes ([`Limits::cache_bytes`]), stays valid
//!   across transactions under `locking_mode=EXCLUSIVE`.
//! - One transaction's dirty set is capped ([`Limits::max_transaction_bytes`]):
//!   a write past it fails with `SQLITE_FULL` and the transaction rolls back.
//! - Locks are no-ops: one connection per database file. The connection
//!   [`open`] makes refuses, through its authorizer, a `journal_mode` other
//!   than `MEMORY`, a `locking_mode` other than `EXCLUSIVE` and
//!   `synchronous=OFF`; the VFS refuses to open a WAL file.
//!
//! What differs is where the VFS keeps its files: one registry for the
//! process behind a mutex, each open file behind a mutex of its own, so a
//! connection may be used from any thread, one thread at a time (as
//! `rusqlite::Connection`, which is `Send` and not `Sync`, is), and stores
//! are told apart by their identity, so two stores' files of one name are
//! two files. The registry's lock is never held across a store's I/O
//! except while a file is deleted; a file's lock is held across its reads
//! and commits, which no other connection makes.
//!
//! A store that more than one process can write (PostgreSQL) must refuse a
//! commit from any process but the database's one owner: the VFS assumes
//! nothing changes the stored file under its connection. `clax-fleet`'s
//! page store fences each commit by its instance lease.
//!
//! SQLite gives each open its own handle, which holds the file it opened.
//! A failed call's message reaches SQLite through `xGetLastError`, and the
//! VFS keeps it per thread: connections on different threads fail apart.
//!
//! The crate's one `unsafe` statement is in [`register`], which hands the
//! VFS to SQLite through `rsqlite-vfs`'s `register_vfs` (its `SAFETY`
//! comment gives the argument); everything else implements `rsqlite-vfs`'s
//! safe traits.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Once};
use std::time::Duration;

use rsqlite_vfs::{
    AccessMode, FileKind, LockLevel, MemChunksFile, OpenOptions, OpenRequest, OpenedFile,
    OsCallback, SQLiteIoMethods, SQLiteVfs, SyncOptions, VfsError, VfsErrorCode, VfsFile,
    VfsResult, VfsStore,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};

/// The name the VFS is registered under.
pub const VFS_NAME: &str = "clax-pages";

/// Bytes per stored chunk, and the page size [`open`] gives a new database,
/// so that every page write replaces exactly one chunk.
pub const CHUNK_SIZE: usize = 4096;

/// An error from a [`PageStore`], reported to SQLite as an I/O error.
pub type StoreError = String;

/// The chunks one commit writes, whole ([`CHUNK_SIZE`] bytes each), by index.
pub type Chunks = BTreeMap<u64, Vec<u8>>;

/// Durable storage for database files as numbered chunks of [`CHUNK_SIZE`]
/// bytes. Chunk `i` holds the file's bytes `i * CHUNK_SIZE ..`; a chunk past
/// the file size is never read, and a missing chunk reads as zeros.
pub trait PageStore: Send + Sync {
    /// The file's size in bytes, or `None` when the store has no such file.
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError>;

    /// One chunk of the file, or `None` when it was never written.
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError>;

    /// Applies one committed transaction atomically (see [`Commit`]): all
    /// of it or none. An error may still mean it applied, as when a
    /// networked store loses the reply to its commit; the VFS then knows
    /// the stored size no longer, and its next commit, which carries the
    /// state SQLite holds after the failure, says so
    /// ([`Commit::stored_size`] is `None`).
    fn commit(&self, file: &str, commit: &Commit) -> Result<(), StoreError>;

    /// Deletes the file and its chunks.
    fn delete(&self, file: &str) -> Result<(), StoreError>;
}

/// One committed transaction of a file: set its size to `size`, replace
/// each chunk in `chunks`, and delete every chunk whose index is
/// `keep_below` or more.
///
/// `stored_size` is the size the store held before (0 for a file it did
/// not have), so a store can skip what did not change: the size when it
/// is the same, the deletion when no stored chunk is at `keep_below` or
/// above. It is `None` when the VFS does not know it, after a commit that
/// failed and may have applied: the store then writes both. The chunks
/// are shared, not copied: a store may keep a clone of the [`Arc`] while
/// it commits (to hand them to another thread), and drops every clone
/// before it returns, when the VFS takes them back.
#[derive(Debug)]
pub struct Commit {
    pub size: u64,
    pub stored_size: Option<u64>,
    pub chunks: Arc<Chunks>,
    pub keep_below: u64,
}

impl Commit {
    /// Whether the commit deletes stored chunks: some stored chunk is, or
    /// may be, at `keep_below` or above.
    pub fn truncates(&self) -> bool {
        self.stored_size
            .is_none_or(|s| s.div_ceil(CHUNK_SIZE as u64) > self.keep_below)
    }

    /// Whether the commit sets the file's size: it differs from the stored
    /// size, or the store may not have the file (or its size) yet.
    pub fn resizes(&self) -> bool {
        self.stored_size.is_none_or(|s| s != self.size || s == 0)
    }
}

/// The memory one database's connection may use, beyond the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// SQLite's page cache (`PRAGMA cache_size`), in bytes.
    pub cache_bytes: u64,
    /// The most bytes of chunks one transaction may write. A write past it
    /// fails with `SQLITE_FULL` ("database or disk is full") and SQLite
    /// rolls the transaction back.
    pub max_transaction_bytes: u64,
}

/// SQLite's page cache per connection: 64 MiB. See [`Limits::default`].
pub const CACHE_BYTES: u64 = 64 << 20;

/// The most bytes of chunks one transaction may write: 64 MiB. See
/// [`Limits::default`].
pub const MAX_TRANSACTION_BYTES: u64 = 64 << 20;

impl Limits {
    /// For a small database many of which a process keeps open (an
    /// artifact's app data store): a 2 MiB page cache and 16 MiB
    /// transactions.
    pub const SMALL: Limits = Limits {
        cache_bytes: 2 << 20,
        max_transaction_bytes: 16 << 20,
    };
}

impl Default for Limits {
    /// [`CACHE_BYTES`] and [`MAX_TRANSACTION_BYTES`]. A server process has
    /// room the Worker's isolate does not, and every page the cache keeps
    /// is a read the store is spared: the cache holds an instance's working
    /// set, its rows of metadata and comments (artifact content is kept
    /// outside the database). The cap bounds a transaction, not the
    /// database; a `VACUUM` rewrites every page in one transaction, so it
    /// fails on a database larger than the cap.
    fn default() -> Self {
        Limits {
            cache_bytes: CACHE_BYTES,
            max_transaction_bytes: MAX_TRANSACTION_BYTES,
        }
    }
}

/// Counters for one database file since it was opened in this process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStats {
    /// Chunks read from the store.
    pub chunk_reads: u64,
    /// Commits handed to the store.
    pub commits: u64,
    /// Chunks written to the store, over all commits.
    pub chunk_writes: u64,
}

/// A database file kept in a [`PageStore`].
struct StoredFile {
    /// The name the store keeps the file under.
    name: String,
    store: Arc<dyn PageStore>,
    /// The size with uncommitted writes applied.
    size: u64,
    /// The size the store holds, as of the last commit that succeeded.
    stored_size: u64,
    /// After a commit failed: the largest size the store may hold, since a
    /// failed commit may have applied. `None` while `stored_size` is known.
    unsure: Option<u64>,
    /// Chunks written since the last commit, whole. Every index is below the
    /// size's chunk count.
    dirty: Chunks,
    /// The lowest chunk count the file was truncated to since the last
    /// commit, if it was; chunks at or above it are deleted on commit.
    truncated_to: Option<u64>,
    /// The cap on `dirty`, in chunks.
    max_dirty_chunks: usize,
    /// A write was refused at the cap since the last commit. SQLite then
    /// rolls the transaction back, writing the journaled pages over the
    /// file, so writes are let past the cap until the next commit stores
    /// the restored state.
    refused_at_cap: bool,
    /// Counted in reads, which take the file by shared reference; the file's
    /// own lock keeps them to one thread at a time.
    stats: Cell<FileStats>,
}

fn io_err(code: VfsErrorCode, err: impl std::fmt::Display) -> VfsError {
    VfsError::new(code, err.to_string().into())
}

impl StoredFile {
    fn open(name: &str, store: Arc<dyn PageStore>, limits: Limits) -> VfsResult<Self> {
        let size = store
            .size(name)
            .map_err(|e| io_err(VfsErrorCode::IoRead, e))?
            .unwrap_or(0);
        Ok(StoredFile {
            name: name.to_owned(),
            store,
            size,
            stored_size: size,
            unsure: None,
            dirty: Chunks::new(),
            truncated_to: None,
            max_dirty_chunks: (limits.max_transaction_bytes / CHUNK_SIZE as u64) as usize,
            refused_at_cap: false,
            stats: Cell::default(),
        })
    }

    fn count(&self, f: impl FnOnce(&mut FileStats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }

    /// The current bytes of chunk `index`, whole, zero-filled where unwritten.
    fn chunk(&self, index: u64) -> VfsResult<Vec<u8>> {
        if let Some(c) = self.dirty.get(&index) {
            return Ok(c.clone());
        }
        let stored_chunks = self.stored_size.div_ceil(CHUNK_SIZE as u64);
        let live = self
            .truncated_to
            .map_or(stored_chunks, |t| t.min(stored_chunks));
        let mut buf = if index < live {
            self.count(|s| s.chunk_reads += 1);
            self.store
                .read_chunk(&self.name, index)
                .map_err(|e| io_err(VfsErrorCode::IoRead, e))?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        buf.resize(CHUNK_SIZE, 0);
        Ok(buf)
    }

    /// Whether the store holds every write: nothing is dirty or truncated.
    fn all_stored(&self) -> bool {
        self.dirty.is_empty() && self.truncated_to.is_none() && self.size == self.stored_size
    }
}

/// The file's I/O, as [`VfsFile`]'s: [`Handle`] makes the calls.
impl StoredFile {
    /// Reads what the file holds of `buf`'s range, returning how many bytes.
    fn read(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        let end = offset + buf.len() as u64;
        let available = self.size.min(end).saturating_sub(offset) as usize;
        let mut done = 0usize;
        while done < available {
            let pos = offset + done as u64;
            let index = pos / CHUNK_SIZE as u64;
            let within = (pos % CHUNK_SIZE as u64) as usize;
            let take = (CHUNK_SIZE - within).min(available - done);
            if let Some(c) = self.dirty.get(&index) {
                buf[done..done + take].copy_from_slice(&c[within..within + take]);
            } else {
                let c = self.chunk(index)?;
                buf[done..done + take].copy_from_slice(&c[within..within + take]);
            }
            done += take;
        }
        Ok(available)
    }

    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        let first = offset / CHUNK_SIZE as u64;
        let last = (offset + buf.len() as u64).div_ceil(CHUNK_SIZE as u64);
        let new_chunks = (first..last)
            .filter(|i| !self.dirty.contains_key(i))
            .count();
        if !self.refused_at_cap && self.dirty.len() + new_chunks > self.max_dirty_chunks {
            self.refused_at_cap = true;
            return Err(io_err(
                VfsErrorCode::Full,
                format!(
                    "{}: the transaction writes more than the {} bytes one commit may hold",
                    self.name,
                    self.max_dirty_chunks * CHUNK_SIZE
                ),
            ));
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let index = pos / CHUNK_SIZE as u64;
            let within = (pos % CHUNK_SIZE as u64) as usize;
            let take = (CHUNK_SIZE - within).min(buf.len() - done);
            let mut c = if within == 0 && take == CHUNK_SIZE {
                vec![0; CHUNK_SIZE]
            } else {
                self.chunk(index)?
            };
            c[within..within + take].copy_from_slice(&buf[done..done + take]);
            self.dirty.insert(index, c);
            done += take;
        }
        self.size = self.size.max(offset + buf.len() as u64);
        Ok(())
    }

    /// Truncates the file: stored at once when the store holds every
    /// earlier write (SQLite truncates such a file only after a commit, or
    /// to the size the last commit recorded), kept pending on a refusal,
    /// else stored at the next sync with the writes before it. See
    /// `crates/clax-worker/vfs`'s `StoredFile::truncate` for why a refused
    /// truncation still reports success.
    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        if size >= self.size {
            return Ok(());
        }
        let store_now = self.all_stored();
        let keep = size.div_ceil(CHUNK_SIZE as u64);
        // The kept part of a partial last chunk, so the tail reads as zeros
        // if the file grows again.
        let tail = (size % CHUNK_SIZE as u64) as usize;
        let last = (tail != 0).then(|| self.chunk(keep - 1)).transpose()?;
        self.dirty.split_off(&keep);
        if let Some(mut c) = last {
            c[tail..].fill(0);
            self.dirty.insert(keep - 1, c);
        }
        self.truncated_to = Some(self.truncated_to.map_or(keep, |t| t.min(keep)));
        self.size = size;
        if store_now {
            // On an error the truncation stays pending: see above.
            let _ = self.flush();
        }
        Ok(())
    }

    fn flush(&mut self) -> VfsResult<()> {
        if self.all_stored() {
            return Ok(());
        }
        let keep_below = self.size.div_ceil(CHUNK_SIZE as u64);
        // Chunks a truncation dropped and the file then grew back over read
        // as zeros; the store still holds their old bytes, so overwrite them.
        if let Some(t) = self.truncated_to {
            let stored = self
                .unsure
                .map_or(self.stored_size, |u| u.max(self.stored_size));
            let stored_chunks = stored.div_ceil(CHUNK_SIZE as u64);
            for i in t..keep_below.min(stored_chunks) {
                self.dirty.entry(i).or_insert_with(|| vec![0; CHUNK_SIZE]);
            }
        }
        debug_assert!(self.dirty.range(keep_below..).next().is_none());
        // The store reads the chunks in place, and hands them back.
        let commit = Commit {
            size: self.size,
            stored_size: self.unsure.is_none().then_some(self.stored_size),
            chunks: Arc::new(std::mem::take(&mut self.dirty)),
            keep_below,
        };
        let result = self.store.commit(&self.name, &commit);
        let written = commit.chunks.len() as u64;
        if let Err(e) = result {
            self.dirty = Arc::try_unwrap(commit.chunks).unwrap_or_else(|kept| (*kept).clone());
            let may_hold = self
                .unsure
                .map_or(self.stored_size, |u| u.max(self.stored_size));
            self.unsure = Some(may_hold.max(commit.size));
            return Err(io_err(VfsErrorCode::IoSync, e));
        }
        self.count(|s| {
            s.commits += 1;
            s.chunk_writes += written;
        });
        self.truncated_to = None;
        self.stored_size = self.size;
        self.unsure = None;
        self.refused_at_cap = false;
        Ok(())
    }

    fn size(&self) -> u64 {
        self.size
    }
}

/// A file the VFS has open: a database file in a store, or a journal or
/// temporary file in memory.
enum PageFile {
    Stored(StoredFile),
    Memory(MemChunksFile),
    /// A database file a later [`open_with`] of it replaced, or whose
    /// [`Database`] (or [`Registered`]) was dropped: every read or write of
    /// it fails.
    Replaced,
}

fn replaced() -> VfsError {
    not_open(VfsErrorCode::Io, "the database file")
}

/// An open file, shared between the registry and the handle using it.
struct Slot {
    file: Mutex<PageFile>,
    /// The file left the registry, replaced by a later [`open_with`] of it
    /// or with its [`Database`] dropped: its handle's calls fail without
    /// waiting for its lock, which the replacing open holds. Cleared when
    /// that open fails and the file is the latest again.
    detached: AtomicBool,
}

type Shared = Arc<Slot>;

fn shared(file: PageFile) -> Shared {
    Arc::new(Slot {
        file: Mutex::new(file),
        detached: AtomicBool::new(false),
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// SQLite's handle on an open file. Its connection makes one call at a
/// time; the file's lock keeps the call from [`open_with`] replacing the
/// file and from [`stats`] reading it.
struct Handle(Shared);

// The handle goes wherever its connection does, which may be any thread.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Handle>();
};

impl Handle {
    /// The file, locked, unless it left the registry before the call or
    /// while the call waited for it.
    fn file(&self) -> VfsResult<MutexGuard<'_, PageFile>> {
        let detached = || self.0.detached.load(Ordering::SeqCst);
        if detached() {
            return Err(replaced());
        }
        let f = lock(&self.0.file);
        if detached() {
            return Err(replaced());
        }
        Ok(f)
    }
}

impl VfsFile for Handle {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        match &mut *self.file()? {
            PageFile::Stored(f) => f.read(buf, offset),
            PageFile::Memory(f) => f.read(buf, offset),
            PageFile::Replaced => Err(replaced()),
        }
    }
    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        match &mut *self.file()? {
            PageFile::Stored(f) => f.write(buf, offset),
            PageFile::Memory(f) => f.write(buf, offset),
            PageFile::Replaced => Err(replaced()),
        }
    }
    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        match &mut *self.file()? {
            PageFile::Stored(f) => f.truncate(size),
            PageFile::Memory(f) => f.truncate(size),
            PageFile::Replaced => Err(replaced()),
        }
    }
    fn sync(&mut self, options: SyncOptions) -> VfsResult<()> {
        match &mut *self.file()? {
            PageFile::Stored(f) => f.flush(),
            PageFile::Memory(f) => f.sync(options),
            PageFile::Replaced => Err(replaced()),
        }
    }
    fn size(&self) -> VfsResult<u64> {
        match &*self.file()? {
            PageFile::Stored(f) => Ok(f.size()),
            PageFile::Memory(f) => f.size(),
            PageFile::Replaced => Err(replaced()),
        }
    }
    // Locks are no-ops: one connection per database file.
    fn lock(&mut self, _level: LockLevel) -> VfsResult<()> {
        Ok(())
    }
    fn unlock(&mut self, _level: LockLevel) -> VfsResult<()> {
        Ok(())
    }
    fn check_reserved_lock(&self) -> VfsResult<bool> {
        Ok(false)
    }
}

/// A database file [`open`] registered and SQLite has not opened yet.
struct Pending {
    file: StoredFile,
}

/// The VFS's files in this process, by the name SQLite knows them by: for
/// a database file, the one its [`open`] made up.
#[derive(Default)]
struct Registry {
    pending: HashMap<String, Pending>,
    /// The database files SQLite has open.
    files: HashMap<String, Shared>,
    /// The temporary files SQLite opened by name, which a later open of the
    /// name shares.
    memory: HashMap<String, Shared>,
    /// For each store (by the address of its value) and name it keeps a
    /// database file under, the SQLite file name of its latest [`open`].
    /// Two stores may keep files of the same name: each is its own file.
    latest: HashMap<(usize, String), String>,
    /// Opens so far, which make each SQLite file name unique.
    opens: u64,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

fn registry() -> MutexGuard<'static, Registry> {
    lock(&REGISTRY)
}

/// Whether SQLite keeps a file of `kind` for its connection alone: its
/// temporary files, which the VFS keeps in memory.
fn in_memory(kind: Option<FileKind>) -> bool {
    matches!(
        kind,
        Some(
            FileKind::TempDb | FileKind::TempJournal | FileKind::TransientDb | FileKind::SubJournal
        )
    )
}

thread_local! {
    /// The last error of a VFS call on this thread, for `xGetLastError`.
    static LAST_ERROR: RefCell<Option<VfsError>> = const { RefCell::new(None) };
}

struct Store;

impl VfsStore for Store {
    type File = Handle;
    type AppData = ();

    fn record_error(_data: &(), error: VfsError) {
        // Not kept once the thread's locals are gone, as it ends.
        let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = Some(error));
    }

    fn last_error(_data: &()) -> Option<VfsError> {
        LAST_ERROR.try_with(|e| e.borrow().clone()).ok().flatten()
    }

    fn open_file(_data: &(), request: OpenRequest<'_>) -> VfsResult<OpenedFile<Handle>> {
        let options = request.options;
        let opened = |file: Shared| {
            Ok(OpenedFile {
                file: Handle(file),
                access: options.access(),
            })
        };
        let memory = || shared(PageFile::Memory(MemChunksFile::new(CHUNK_SIZE)));
        let Some(file) = request.filename.map(|f| f.path()) else {
            // A temporary file with no name: its handle's alone.
            return opened(memory());
        };
        let kind = options.kind();
        if kind == Some(FileKind::Wal) {
            return Err(io_err(
                VfsErrorCode::CantOpen,
                format!(
                    "{file}: a WAL would keep commits out of the store; journal_mode must be MEMORY"
                ),
            ));
        }
        let mut r = registry();
        if in_memory(kind) {
            // A temporary file opened again by name keeps its contents.
            let f = r
                .memory
                .entry(file.to_owned())
                .or_insert_with(memory)
                .clone();
            return opened(f);
        }
        let f = match r.pending.remove(file) {
            // Read from the store by [`open_with`], outside the registry's lock.
            Some(p) if kind == Some(FileKind::MainDb) => shared(PageFile::Stored(p.file)),
            Some(p) => {
                r.pending.insert(file.to_owned(), p);
                return Err(io_err(
                    VfsErrorCode::CantOpen,
                    format!("{file} is a database file, opened as another kind"),
                ));
            }
            None if r.files.contains_key(file) => {
                return Err(io_err(
                    VfsErrorCode::CantOpen,
                    format!("{file} is open: a database file takes one connection"),
                ));
            }
            None => {
                return Err(io_err(
                    VfsErrorCode::CantOpen,
                    format!("{file} is not a database file open() registered"),
                ));
            }
        };
        r.files.insert(file.to_owned(), f.clone());
        opened(f)
    }

    fn close_file(
        _data: &(),
        name: Option<&str>,
        file: Handle,
        options: OpenOptions,
    ) -> VfsResult<()> {
        if let Some(name) = name
            && options.delete_on_close()
        {
            let mut r = registry();
            // Not a file opened under the name since this one was deleted.
            if r.memory.get(name).is_some_and(|f| Arc::ptr_eq(f, &file.0)) {
                r.memory.remove(name);
            }
        }
        Ok(())
    }

    fn access(_data: &(), file: &str, _mode: AccessMode) -> VfsResult<bool> {
        // SQLite asks only about journals and WAL files, which this VFS never
        // keeps: a temporary file reads as absent, and a database file is
        // created by its open, once.
        if registry().files.contains_key(file) {
            return Err(io_err(
                VfsErrorCode::CantOpen,
                format!("{file} is open: a database file takes one connection"),
            ));
        }
        Ok(false)
    }

    fn full_pathname(_data: &(), name: &str) -> VfsResult<String> {
        Ok(name.to_owned())
    }

    fn delete_file(_data: &(), file: &str, _sync_dir: bool) -> VfsResult<()> {
        let removed = {
            let mut r = registry();
            r.memory.remove(file);
            r.files.remove(file)
        };
        if let Some(f) = removed
            && let PageFile::Stored(f) = &*lock(&f.file)
        {
            f.store
                .delete(&f.name)
                .map_err(|e| io_err(VfsErrorCode::IoDelete, e))?;
        }
        Ok(())
    }
}

fn not_open(code: VfsErrorCode, name: &str) -> VfsError {
    io_err(
        code,
        format!(
            "{name} is not open: its connection was closed, or a later open() of the file replaced it"
        ),
    )
}

/// Whether the connection refuses `PRAGMA {name}={value}`: the settings
/// [`open`] makes are the ones the VFS's guarantees rest on.
///
/// - `journal_mode` stays `MEMORY`: the store holds only committed states,
///   a WAL would keep commits out of it, and without a journal a rollback
///   could not restore pages a transaction spilled.
/// - `locking_mode` stays `EXCLUSIVE`: one connection owns the file.
/// - `synchronous` is not `OFF`: the store receives a commit when SQLite
///   syncs the file, which it does not do with `synchronous=OFF`.
fn refuses_pragma(name: &str, value: &str) -> bool {
    let is = |s: &str| value.eq_ignore_ascii_case(s);
    if name.eq_ignore_ascii_case("journal_mode") {
        return !is("memory");
    }
    if name.eq_ignore_ascii_case("locking_mode") {
        return !is("exclusive");
    }
    if name.eq_ignore_ascii_case("synchronous") {
        let syncs = [
            "normal", "full", "extra", "on", "yes", "true", "1", "2", "3",
        ];
        return !syncs.into_iter().any(is);
    }
    false
}

/// The authorizer [`open`] installs on its connection: denies a `PRAGMA`
/// that changes `journal_mode` or `locking_mode`, or sets
/// `synchronous=OFF`, which then fails to prepare with `SQLITE_AUTH` ("not
/// authorized"), and allows everything else.
///
/// A caller that installs an authorizer of its own replaces this one; its
/// authorizer must deny what this one denies, for example by answering
/// `authorize(ctx)` first whenever that is not [`Authorization::Allow`].
pub fn authorize(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Pragma {
            pragma_name,
            pragma_value: Some(value),
        } if refuses_pragma(pragma_name, value) => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

struct IoMethods;

impl SQLiteIoMethods for IoMethods {
    type Store = Store;
}

/// What SQLite asks the VFS for beside files: sleeps, randomness, the time.
struct Os;

impl OsCallback for Os {
    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur)
    }

    fn random(&self, buf: &mut [u8]) -> usize {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        // Randomness for temporary file names; not for cryptography.
        for chunk in buf.chunks_mut(8) {
            let n = RandomState::new().build_hasher().finish().to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
        buf.len()
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        use std::time::{SystemTime, UNIX_EPOCH};
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64))
    }
}

struct Vfs;

impl SQLiteVfs<IoMethods> for Vfs {
    type Os = Os;
    const VERSION: i32 = 1;

    fn os(_data: &()) -> &Os {
        &Os
    }
}

/// Registers the VFS with SQLite for the life of the process: [`register`]
/// calls it once, after SQLite is initialized.
fn register_vfs() -> Result<(), rsqlite_vfs::RegisterVfsError> {
    // SAFETY: the VFS is rsqlite-vfs's own `SQLiteVfs::vfs` and method
    // table (`SQLiteIoMethods::METHODS`) over `Store`'s handles, with none
    // of their callbacks overridden, so their versions, file layout and app
    // data agree. The app data is `()` and the registration is never
    // unregistered, so all the callbacks use outlives every open file.
    // `register` runs this once, under a `Once`, and nothing else in the
    // process registers `VFS_NAME`. The SQLite libsqlite3-sys bundles is
    // built serialized (`SQLITE_THREADSAFE=1`). The VFS keeps its state
    // behind mutexes and in thread-locals, and a handle, which moves with
    // its connection, is `Send` and `Sync` (checked where `Handle` is
    // defined), so the callbacks may run on any thread.
    #[allow(unsafe_code)]
    let registered = unsafe { rsqlite_vfs::register_vfs::<IoMethods, Vfs>(VFS_NAME, (), false) };
    registered.map(|r| {
        r.into_raw();
    })
}

fn register() -> rusqlite::Result<()> {
    static ONCE: Once = Once::new();
    static RESULT: Mutex<Option<String>> = Mutex::new(None);
    ONCE.call_once(|| {
        // Opening a connection initializes SQLite, which a VFS registration
        // needs on builds that do not auto-initialize.
        let made = Connection::open_in_memory()
            .map(drop)
            .map_err(|e| e.to_string())
            .and_then(|()| register_vfs().map_err(|e| e.to_string()));
        if let Err(e) = made {
            *lock(&RESULT) = Some(e);
        }
    });
    match &*lock(&RESULT) {
        None => Ok(()),
        Some(e) => Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(format!("the {VFS_NAME} VFS did not register: {e}")),
        )),
    }
}

/// A connection from [`open`]. It derefs to its [`Connection`]; dropping it
/// closes the connection and forgets the file in the VFS.
pub struct Database {
    // Declared first, so dropped before `file` forgets the file.
    conn: Connection,
    file: OpenFile,
}

impl Deref for Database {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for Database {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Database {
    /// The name SQLite knows the database file by, unique to this open.
    pub fn sqlite_name(&self) -> &str {
        &self.file.sqlite_name
    }

    /// The connection, for an owner that takes it by value (a store), and
    /// what keeps its file in the VFS: drop the [`Registered`] only after
    /// the connection, or the connection's next read or write fails.
    pub fn into_parts(self) -> (Connection, Registered) {
        (self.conn, Registered(self.file))
    }
}

/// Keeps a database file of [`Database::into_parts`] in the VFS until it is
/// dropped.
pub struct Registered(#[allow(dead_code)] OpenFile);

/// A database file's place in the registry, removed when dropped.
struct OpenFile {
    key: (usize, String),
    sqlite_name: String,
}

/// A store's identity in the registry: the address of its value, which
/// stays its own while any file of it is registered (the file holds it).
fn store_key(store: &Arc<dyn PageStore>) -> usize {
    Arc::as_ptr(store) as *const () as usize
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        let removed = {
            let mut r = registry();
            r.pending.remove(&self.sqlite_name);
            if r.latest.get(&self.key) == Some(&self.sqlite_name) {
                r.latest.remove(&self.key);
            }
            r.files.remove(&self.sqlite_name)
        };
        // A connection still open on the file (its `Registered` dropped
        // first) fails from its next read or write.
        if let Some(f) = removed {
            f.detached.store(true, Ordering::SeqCst);
        }
    }
}

/// [`open_with`] with [`Limits::default`].
pub fn open(name: &str, store: Arc<dyn PageStore>) -> rusqlite::Result<Database> {
    open_with(name, store, Limits::default())
}

/// Opens (creating if absent) the database file `name` kept in `store`,
/// with the settings the VFS relies on:
///
/// - `journal_mode=MEMORY`: the store holds only committed states, so the
///   rollback journal is needed only to undo an aborted transaction.
/// - `locking_mode=EXCLUSIVE`: SQLite keeps its page cache across
///   transactions, as no other connection writes the file.
/// - `synchronous=NORMAL`: SQLite syncs the database file at each commit,
///   which is when the VFS hands the transaction to the store.
/// - `temp_store=MEMORY`, and `cache_size` from `limits`.
/// - `page_size` of [`CHUNK_SIZE`], for a new database only: an existing
///   one keeps its page size, through a `VACUUM` too.
///
/// The connection's authorizer refuses a `PRAGMA` that changes
/// `journal_mode` or `locking_mode`, or sets `synchronous=OFF`: preparing
/// one fails with `SQLITE_AUTH` ("not authorized"). A caller that installs
/// an authorizer of its own on the connection replaces this one, and must
/// refuse them too, by delegating to [`authorize`].
///
/// `name` is the store's name for the file, so it is unique within a store;
/// two stores are told apart by the VFS, never by name: each store's `db`
/// is its own file. A database file is opened by one connection at a time:
/// opening the same `name` of the same store again replaces the earlier
/// open, whose connection's next read or write of the file fails, so a
/// connection left over from an earlier start cannot write into the later
/// one's. The earlier connection's I/O stops before the store's size is
/// read (one in progress first ends), so the later open reads the file as
/// the earlier one left it; if the later open fails, the earlier
/// connection works again, unless yet another open replaced it.
pub fn open_with(
    name: &str,
    store: Arc<dyn PageStore>,
    limits: Limits,
) -> rusqlite::Result<Database> {
    register()?;
    let key = (store_key(&store), name.to_owned());
    let (file, earlier) = {
        let mut r = registry();
        r.opens += 1;
        let sqlite_name = format!("{}/{name}", r.opens);
        let earlier = r
            .latest
            .insert(key.clone(), sqlite_name.clone())
            .and_then(|earlier| {
                r.pending.remove(&earlier);
                let f = r.files.remove(&earlier)?;
                f.detached.store(true, Ordering::SeqCst);
                Some((earlier, f))
            });
        (OpenFile { key, sqlite_name }, earlier)
    };
    // Held until the open ends: an earlier I/O call already under way ends
    // first, and one that took the file before it left the registry waits
    // here, then fails unless the open fails too.
    let mut held = earlier.as_ref().map(|(_, f)| lock(&f.file));
    match connect(name, store, limits, &file) {
        Ok(conn) => {
            if let Some(f) = held.as_deref_mut() {
                *f = PageFile::Replaced;
            }
            Ok(Database { conn, file })
        }
        Err(e) => {
            if let Some((earlier, f)) = &earlier {
                let mut r = registry();
                if r.latest.get(&file.key) == Some(&file.sqlite_name) {
                    r.latest.insert(file.key.clone(), earlier.clone());
                    r.files.insert(earlier.clone(), f.clone());
                    f.detached.store(false, Ordering::SeqCst);
                }
            }
            drop(held);
            Err(e)
        }
    }
}

/// Reads the store's size for `file`, registers it and opens SQLite's
/// connection on it, with the settings [`open_with`] lists. The store's
/// read is made outside the registry's lock: another database's I/O never
/// waits on this one's store.
fn connect(
    name: &str,
    store: Arc<dyn PageStore>,
    limits: Limits,
    file: &OpenFile,
) -> rusqlite::Result<Connection> {
    let stored = StoredFile::open(name, store, limits).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some(format!("{name}: {e:?}")),
        )
    })?;
    {
        let mut r = registry();
        if r.latest.get(&file.key) != Some(&file.sqlite_name) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!(
                    "{name}: a later open of the file replaced this one"
                )),
            ));
        }
        r.pending
            .insert(file.sqlite_name.clone(), Pending { file: stored });
    }
    let conn = Connection::open_with_flags_and_vfs(
        &file.sqlite_name,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        VFS_NAME,
    )?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size = if pages == 0 {
        format!("PRAGMA page_size={CHUNK_SIZE};")
    } else {
        String::new()
    };
    conn.execute_batch(&format!(
        "{page_size}
         PRAGMA journal_mode=MEMORY;
         PRAGMA locking_mode=EXCLUSIVE;
         PRAGMA synchronous=NORMAL;
         PRAGMA temp_store=MEMORY;
         PRAGMA cache_size=-{};",
        limits.cache_bytes / 1024
    ))?;
    conn.authorizer(Some(authorize))?;
    Ok(conn)
}

/// The counters for the latest open of the database file `name` of
/// `store`, if it is open.
pub fn stats(store: &Arc<dyn PageStore>, name: &str) -> Option<FileStats> {
    let file = {
        let r = registry();
        r.files
            .get(r.latest.get(&(store_key(store), name.to_owned()))?)?
            .clone()
    };
    match &*lock(&file.file) {
        PageFile::Stored(f) => Some(f.stats.get()),
        PageFile::Memory(_) | PageFile::Replaced => None,
    }
}

/// A [`PageStore`] in memory, for tests and for a database that need not
/// outlive the process.
#[derive(Default)]
pub struct MemPages {
    files: Mutex<HashMap<String, (u64, Chunks)>>,
    commits: std::sync::atomic::AtomicU64,
}

impl MemPages {
    pub fn new() -> MemPages {
        MemPages::default()
    }

    /// Commits applied so far.
    pub fn commits(&self) -> u64 {
        self.commits.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl PageStore for MemPages {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        Ok(lock(&self.files).get(file).map(|f| f.0))
    }
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(lock(&self.files)
            .get(file)
            .and_then(|f| f.1.get(&index).cloned()))
    }
    fn commit(&self, file: &str, c: &Commit) -> Result<(), StoreError> {
        let mut files = lock(&self.files);
        let f = files.entry(file.to_owned()).or_default();
        f.0 = c.size;
        f.1.extend(c.chunks.iter().map(|(i, c)| (*i, c.clone())));
        f.1.split_off(&c.keep_below);
        self.commits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        lock(&self.files).remove(file);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
