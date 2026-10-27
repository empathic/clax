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
//! The crate has no `unsafe` code: it implements only `rsqlite-vfs`'s safe
//! traits, and tells files apart without reading their names back from
//! SQLite's file handles (see `Registry::handles`).

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Once};
use std::time::Duration;

use rsqlite_vfs::ffi::{
    SQLITE_CANTOPEN, SQLITE_FULL, SQLITE_IOERR, SQLITE_IOERR_DELETE, SQLITE_IOERR_FSYNC,
    SQLITE_IOERR_READ, SQLITE_IOERR_WRITE, SQLITE_OPEN_MAIN_DB, SQLITE_OPEN_SUBJOURNAL,
    SQLITE_OPEN_TEMP_DB, SQLITE_OPEN_TEMP_JOURNAL, SQLITE_OPEN_TRANSIENT_DB, SQLITE_OPEN_WAL,
    sqlite3_vfs,
};
use rsqlite_vfs::{
    MemChunksFile, SQLiteIoMethods, SQLiteVfs, SQLiteVfsFile, VfsError, VfsFile, VfsResult,
    VfsStore,
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

    /// Applies one committed transaction atomically (see [`Commit`]). On an
    /// error, none of it applies.
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
/// above. The chunks are shared, not copied: a store may keep a clone of
/// the [`Arc`] while it commits (to hand them to another thread), and
/// drops every clone before it returns, when the VFS takes them back.
#[derive(Debug)]
pub struct Commit {
    pub size: u64,
    pub stored_size: u64,
    pub chunks: Arc<Chunks>,
    pub keep_below: u64,
}

impl Commit {
    /// Whether the commit deletes stored chunks: some stored chunk is at
    /// `keep_below` or above.
    pub fn truncates(&self) -> bool {
        self.stored_size.div_ceil(CHUNK_SIZE as u64) > self.keep_below
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
    /// The size the store holds.
    stored_size: u64,
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

fn io_err(code: i32, err: impl std::fmt::Display) -> VfsError {
    VfsError::new(code, err.to_string())
}

impl StoredFile {
    fn open(name: &str, store: Arc<dyn PageStore>, limits: Limits) -> VfsResult<Self> {
        let size = store
            .size(name)
            .map_err(|e| io_err(SQLITE_IOERR_READ, e))?
            .unwrap_or(0);
        Ok(StoredFile {
            name: name.to_owned(),
            store,
            size,
            stored_size: size,
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
                .map_err(|e| io_err(SQLITE_IOERR_READ, e))?
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

impl VfsFile for StoredFile {
    fn read(&self, buf: &mut [u8], offset: usize) -> VfsResult<bool> {
        let offset = offset as u64;
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
        buf[available..].fill(0);
        Ok(available == buf.len())
    }

    fn write(&mut self, buf: &[u8], offset: usize) -> VfsResult<()> {
        let offset = offset as u64;
        let first = offset / CHUNK_SIZE as u64;
        let last = (offset + buf.len() as u64).div_ceil(CHUNK_SIZE as u64);
        let new_chunks = (first..last)
            .filter(|i| !self.dirty.contains_key(i))
            .count();
        if !self.refused_at_cap && self.dirty.len() + new_chunks > self.max_dirty_chunks {
            self.refused_at_cap = true;
            return Err(io_err(
                SQLITE_FULL,
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
    fn truncate(&mut self, size: usize) -> VfsResult<()> {
        let size = size as u64;
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
            let stored_chunks = self.stored_size.div_ceil(CHUNK_SIZE as u64);
            for i in t..keep_below.min(stored_chunks) {
                self.dirty.entry(i).or_insert_with(|| vec![0; CHUNK_SIZE]);
            }
        }
        debug_assert!(self.dirty.range(keep_below..).next().is_none());
        // The store reads the chunks in place, and hands them back.
        let commit = Commit {
            size: self.size,
            stored_size: self.stored_size,
            chunks: Arc::new(std::mem::take(&mut self.dirty)),
            keep_below,
        };
        let result = self.store.commit(&self.name, &commit);
        let written = commit.chunks.len() as u64;
        if let Err(e) = result {
            self.dirty = Arc::try_unwrap(commit.chunks).unwrap_or_else(|kept| (*kept).clone());
            return Err(io_err(SQLITE_IOERR_FSYNC, e));
        }
        self.count(|s| {
            s.commits += 1;
            s.chunk_writes += written;
        });
        self.truncated_to = None;
        self.stored_size = self.size;
        self.refused_at_cap = false;
        Ok(())
    }

    fn size(&self) -> VfsResult<usize> {
        Ok(self.size as usize)
    }
}

/// A file the VFS has open: a database file in a store, or a journal or
/// temporary file in memory.
enum PageFile {
    Stored(StoredFile),
    Memory(MemChunksFile),
}

impl VfsFile for PageFile {
    fn read(&self, buf: &mut [u8], offset: usize) -> VfsResult<bool> {
        match self {
            PageFile::Stored(f) => f.read(buf, offset),
            PageFile::Memory(f) => f.read(buf, offset),
        }
    }
    fn write(&mut self, buf: &[u8], offset: usize) -> VfsResult<()> {
        match self {
            PageFile::Stored(f) => f.write(buf, offset),
            PageFile::Memory(f) => f.write(buf, offset),
        }
    }
    fn truncate(&mut self, size: usize) -> VfsResult<()> {
        match self {
            PageFile::Stored(f) => f.truncate(size),
            PageFile::Memory(f) => f.truncate(size),
        }
    }
    fn flush(&mut self) -> VfsResult<()> {
        match self {
            PageFile::Stored(f) => f.flush(),
            PageFile::Memory(f) => f.flush(),
        }
    }
    fn size(&self) -> VfsResult<usize> {
        match self {
            PageFile::Stored(f) => f.size(),
            PageFile::Memory(f) => f.size(),
        }
    }
}

/// An open file, shared between the registry and the I/O call using it.
type Shared = Arc<Mutex<PageFile>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A database file [`open`] registered and SQLite has not opened yet.
struct Pending {
    file: StoredFile,
}

/// The VFS's files in this process. Files are keyed by the name SQLite
/// knows them by: for a database file, the one its [`open`] made up.
#[derive(Default)]
struct Registry {
    pending: HashMap<String, Pending>,
    files: HashMap<String, Shared>,
    /// The file name behind each open SQLite file handle, by the address of
    /// the handle's copy of the name (`SQLiteVfsFile::name_ptr`).
    ///
    /// `rsqlite-vfs` 0.1.1's `xOpen` makes the name a `String`, passes it to
    /// `VfsStore::add_file` and then leaks that same buffer into the handle,
    /// freeing it in `xClose`. `Store::add_file` records the address it is
    /// passed, and the I/O methods look a handle up by its `name_ptr`, which
    /// compares addresses and never reads through one. While a handle is
    /// open its name's buffer is live, so no other name can be recorded at
    /// that address: the latest entry for a handle's address is its own.
    /// The registry is the process's, as the heap the addresses come from
    /// is. `Store::contains_file` answers `false` for every file it lets
    /// open, so that every open goes through `add_file`.
    handles: HashMap<usize, String>,
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

impl Registry {
    /// The name of the file behind a handle SQLite passes to the VFS, as
    /// `Store::add_file` recorded it.
    fn file_name(&self, file: &SQLiteVfsFile) -> Option<&str> {
        self.handles
            .get(&(file.name_ptr as usize))
            .filter(|name| name.len() == file.name_length)
            .map(String::as_str)
    }

    /// The open file behind a handle.
    fn file(&self, vfs_file: &SQLiteVfsFile, code: i32) -> VfsResult<Shared> {
        let Some(name) = self.file_name(vfs_file) else {
            return Err(not_open(code, "the file"));
        };
        self.files
            .get(name)
            .cloned()
            .ok_or_else(|| not_open(code, name))
    }

    /// Forgets the handles of the file SQLite knows as `name`.
    fn forget_handles(&mut self, name: &str) {
        self.handles.retain(|_, n| n != name);
    }
}

/// Files a connection may keep in memory: SQLite's temporary files.
const MEMORY_FILES: i32 = SQLITE_OPEN_TEMP_DB
    | SQLITE_OPEN_TEMP_JOURNAL
    | SQLITE_OPEN_TRANSIENT_DB
    | SQLITE_OPEN_SUBJOURNAL;

struct Store;

impl VfsStore<PageFile, ()> for Store {
    fn add_file(_vfs: *mut sqlite3_vfs, file: &str, flags: i32) -> VfsResult<()> {
        if flags & SQLITE_OPEN_WAL != 0 {
            return Err(io_err(
                SQLITE_CANTOPEN,
                format!(
                    "{file}: a WAL would keep commits out of the store; journal_mode must be MEMORY"
                ),
            ));
        }
        if file.is_empty() {
            // Empty names would share one address in `Registry::handles`.
            return Err(io_err(SQLITE_CANTOPEN, "a file name is empty"));
        }
        let mut r = registry();
        let f = match r.pending.remove(file) {
            // Read from the store by [`open_with`], outside the registry's lock.
            Some(p) if flags & SQLITE_OPEN_MAIN_DB != 0 => PageFile::Stored(p.file),
            Some(p) => {
                r.pending.insert(file.to_owned(), p);
                return Err(io_err(
                    SQLITE_CANTOPEN,
                    format!("{file} is a database file, opened as another kind"),
                ));
            }
            // A temporary file opened again by name keeps its contents.
            None if flags & MEMORY_FILES != 0 => match r.files.remove(file) {
                Some(f) if matches!(*lock(&f), PageFile::Memory(_)) => {
                    r.files.insert(file.to_owned(), f);
                    r.handles.insert(file.as_ptr() as usize, file.to_owned());
                    return Ok(());
                }
                _ => PageFile::Memory(MemChunksFile::new(CHUNK_SIZE)),
            },
            None => {
                return Err(io_err(
                    SQLITE_CANTOPEN,
                    format!("{file} is not a database file open() registered"),
                ));
            }
        };
        r.files.insert(file.to_owned(), Arc::new(Mutex::new(f)));
        // The buffer `file` points into becomes the handle's name.
        r.handles.insert(file.as_ptr() as usize, file.to_owned());
        Ok(())
    }

    fn contains_file(_vfs: *mut sqlite3_vfs, file: &str) -> VfsResult<bool> {
        // A database file is created by its xOpen (`add_file`), once. Every
        // open must reach `add_file` (see `Registry::handles`), so a
        // temporary file reads as absent; SQLite asks `xAccess` only about
        // journals and WAL files, which this VFS never keeps.
        let open = registry().files.get(file).cloned();
        match open {
            Some(f) if matches!(*lock(&f), PageFile::Stored(_)) => Err(io_err(
                SQLITE_CANTOPEN,
                format!("{file} is open: a database file takes one connection"),
            )),
            _ => Ok(false),
        }
    }

    fn delete_file(_vfs: *mut sqlite3_vfs, file: &str) -> VfsResult<()> {
        let removed = {
            let mut r = registry();
            r.forget_handles(file);
            r.files.remove(file)
        };
        if let Some(f) = removed
            && let PageFile::Stored(f) = &*lock(&f)
        {
            f.store
                .delete(&f.name)
                .map_err(|e| io_err(SQLITE_IOERR_DELETE, e))?;
        }
        Ok(())
    }

    fn with_file<F: Fn(&PageFile) -> VfsResult<i32>>(
        vfs_file: &SQLiteVfsFile,
        f: F,
    ) -> VfsResult<i32> {
        let file = registry().file(vfs_file, SQLITE_IOERR)?;
        f(&lock(&file))
    }

    fn with_file_mut<F: Fn(&mut PageFile) -> VfsResult<i32>>(
        vfs_file: &SQLiteVfsFile,
        f: F,
    ) -> VfsResult<i32> {
        let file = registry().file(vfs_file, SQLITE_IOERR_WRITE)?;
        f(&mut lock(&file))
    }
}

fn not_open(code: i32, name: &str) -> VfsError {
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
    type File = PageFile;
    type AppData = ();
    type Store = Store;
    const VERSION: i32 = 1;
}

struct Vfs;

impl SQLiteVfs<IoMethods> for Vfs {
    const VERSION: i32 = 1;

    fn sleep(dur: Duration) {
        std::thread::sleep(dur)
    }

    fn random(buf: &mut [u8]) {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        // Randomness for temporary file names; not for cryptography.
        for chunk in buf.chunks_mut(8) {
            let n = RandomState::new().build_hasher().finish().to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
    }

    fn epoch_timestamp_in_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }
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
            .and_then(|()| {
                rsqlite_vfs::register_vfs::<IoMethods, Vfs>(VFS_NAME, (), false)
                    .map(drop)
                    .map_err(|e| e.to_string())
            });
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
        let mut r = registry();
        r.pending.remove(&self.sqlite_name);
        r.files.remove(&self.sqlite_name);
        r.forget_handles(&self.sqlite_name);
        if r.latest.get(&self.key) == Some(&self.sqlite_name) {
            r.latest.remove(&self.key);
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
/// one's.
pub fn open_with(
    name: &str,
    store: Arc<dyn PageStore>,
    limits: Limits,
) -> rusqlite::Result<Database> {
    register()?;
    let key = (store_key(&store), name.to_owned());
    // The store's first read, before the registry is locked: another
    // database's I/O never waits on this one's store.
    let stored = StoredFile::open(name, store, limits).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some(format!("{name}: {e:?}")),
        )
    })?;
    let file = {
        let mut r = registry();
        r.opens += 1;
        let sqlite_name = format!("{}/{name}", r.opens);
        if let Some(earlier) = r.latest.insert(key.clone(), sqlite_name.clone()) {
            r.pending.remove(&earlier);
            r.files.remove(&earlier);
        }
        r.pending
            .insert(sqlite_name.clone(), Pending { file: stored });
        OpenFile { key, sqlite_name }
    };
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
    Ok(Database { conn, file })
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
    match &*lock(&file) {
        PageFile::Stored(f) => Some(f.stats.get()),
        PageFile::Memory(_) => None,
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
