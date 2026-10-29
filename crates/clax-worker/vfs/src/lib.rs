//! A SQLite VFS that keeps each database file's bytes in a [`PageStore`] as
//! fixed-size chunks, and SQLite's journals and temporary files in memory.
//!
//! The VFS is built for a single-threaded host whose storage commits a batch
//! of writes atomically, such as a SQLite-backed Cloudflare Durable Object
//! (`ctx.storage.sql` inside `ctx.storage.transactionSync`):
//!
//! - Writes to a database file go to an in-memory dirty set. Nothing reaches
//!   the store until SQLite syncs the file (`xSync`), which hands every
//!   dirty chunk, the new file size and any truncation to
//!   [`PageStore::commit`] in one call, which the store applies atomically.
//!   SQLite syncs the file when it commits a write transaction, and when it
//!   rolls back one whose pages it had already written to the file (a cache
//!   spill): the rollback writes the pages' journaled contents back and
//!   syncs, so the store receives the restored pages, the same bytes as the
//!   last commit.
//! - A commit that shrinks the file truncates it after its sync. A truncation
//!   of a file whose every write is stored is stored at once, on its own
//!   (SQLite truncates such a file only after a commit, or to the size the
//!   last commit recorded); one the store refuses stays pending for the next
//!   commit, and the transaction stays committed. The store therefore only
//!   ever holds the database as of a committed transaction (a truncation not
//!   yet stored leaves only unused chunks past the end the header records),
//!   so no journal needs to survive a crash and journals live in memory.
//! - Reads come from the dirty set, then from the store. There is no clean
//!   cache beyond SQLite's own page cache, which [`open`] sizes
//!   ([`Limits::cache_bytes`]) and keeps valid across transactions with
//!   `locking_mode=EXCLUSIVE`.
//! - One transaction's dirty set is capped ([`Limits::max_transaction_bytes`]);
//!   a write past the cap fails with `SQLITE_FULL` and the transaction rolls
//!   back.
//! - Locks are no-ops: one connection per database file, in one thread.
//!   The connection [`open`] makes refuses settings that would break this
//!   model, through its authorizer: `journal_mode` other than `MEMORY` (a
//!   WAL would keep commits in memory, out of the store; with no journal a
//!   rollback could not restore spilled pages), `locking_mode` other than
//!   `EXCLUSIVE`, and `synchronous=OFF` (a commit that never syncs would
//!   never reach the store). The VFS also refuses to open a WAL file.
//!
//! Each [`open`] gives SQLite a file name of its own, mapped in a
//! thread-local registry to the store and the name the store keeps the file
//! under, so several databases (several Durable Objects in one isolate)
//! share the one registered VFS, and a connection from an earlier open of a
//! name cannot reach a later open's store.
//!
//! SQLite's handle on a file holds the file's name in the registry, never
//! the file, so a connection moved to another thread finds none of its
//! files there, and fails. A failed call's message reaches SQLite through
//! `xGetLastError`, and the VFS keeps it per thread too.
//!
//! The crate's one `unsafe` statement is in `register_vfs`, which hands the
//! VFS to SQLite through `rsqlite-vfs`'s `register_vfs` (its `SAFETY`
//! comment gives the argument); everything else implements `rsqlite-vfs`'s
//! safe traits.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::Once;
use std::time::Duration;

use rsqlite_vfs::{
    AccessMode, FileKind, LockLevel, MemChunksFile, OpenOptions, OpenRequest, OpenedFile,
    OsCallback, SQLiteIoMethods, SQLiteVfs, SyncOptions, VfsError, VfsErrorCode, VfsFile,
    VfsResult, VfsStore,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};

/// The name the VFS is registered under.
pub const VFS_NAME: &str = "clax-do";

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
pub trait PageStore {
    /// The file's size in bytes, or `None` when the store has no such file.
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError>;

    /// One chunk of the file, or `None` when it was never written.
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError>;

    /// Applies one committed transaction atomically: sets the file's size to
    /// `size`, replaces each chunk in `chunks`, and deletes every chunk whose
    /// index is `keep_below` or more. On an error, none of it applies.
    ///
    /// The store may clone `chunks` to read it while it commits, and drops
    /// every clone before it returns: the VFS takes the chunks back.
    fn commit(
        &self,
        file: &str,
        size: u64,
        chunks: &Rc<Chunks>,
        keep_below: u64,
    ) -> Result<(), StoreError>;

    /// Deletes the file and its chunks.
    fn delete(&self, file: &str) -> Result<(), StoreError>;
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

/// SQLite's page cache per connection: 8 MiB. See [`Limits::default`].
pub const CACHE_BYTES: u64 = 8 << 20;

/// The most bytes of chunks one transaction may write: 16 MiB. See
/// [`Limits::default`].
pub const MAX_TRANSACTION_BYTES: u64 = 16 << 20;

impl Default for Limits {
    /// [`CACHE_BYTES`] and [`MAX_TRANSACTION_BYTES`], sized for a Workers
    /// isolate's 128 MB, which its JS heap and wasm memory share with every
    /// Durable Object it runs, and which never shrinks once wasm memory has
    /// grown. A transaction holds its dirty chunks (at most the cap), the
    /// memory journal of the pages it overwrote (at most as much again) and
    /// the page cache: at most 40 MiB for a commit, and about 56 MiB for one
    /// refused at the cap (the cache's dirty pages count in the journal, and
    /// the rollback rewrites them), which leaves the rest for the Worker's
    /// code and heap and other objects' caches. sqlite-wasm-rs builds SQLite
    /// with a 16 MiB default cache; natively it is about 2 MB.
    ///
    /// The cap bounds a transaction, not the database: Clax keeps artifact
    /// content outside this database, so its transactions are rows of
    /// metadata and comments, far below it. A `VACUUM` rewrites every page
    /// in one transaction, so it fails on a database larger than the cap.
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
    store: Rc<dyn PageStore>,
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
    stats: RefCell<FileStats>,
}

fn io_err(code: VfsErrorCode, err: impl std::fmt::Display) -> VfsError {
    VfsError::new(code, err.to_string().into())
}

impl StoredFile {
    fn open(name: &str, store: Rc<dyn PageStore>, limits: Limits) -> VfsResult<Self> {
        let size = store
            .size(name)
            .map_err(|e| io_err(VfsErrorCode::IoRead, e))?
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
            stats: RefCell::default(),
        })
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
            self.stats.borrow_mut().chunk_reads += 1;
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

/// The file's I/O, as [`VfsFile`]'s: [`DoFile`] makes the calls.
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

    /// Truncates the file. When the store holds every earlier write, the
    /// truncation is stored at once: SQLite truncates such a file only when
    /// a commit that shrank the database has synced (`pager_end_transaction`
    /// in SQLite's pager) or, rolling back, to the size the last commit
    /// recorded (`pager_playback`), so the store keeps a committed state. A
    /// refused store keeps the truncation pending for the next commit and
    /// reports success: the transaction is already durable, and the stored
    /// database is valid without the truncation, as the chunks past the page
    /// count its header records are never read. Any other truncation is
    /// stored at the next sync, with the writes before it.
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
            let stored_chunks = self.stored_size.div_ceil(CHUNK_SIZE as u64);
            for i in t..keep_below.min(stored_chunks) {
                self.dirty.entry(i).or_insert_with(|| vec![0; CHUNK_SIZE]);
            }
        }
        debug_assert!(self.dirty.range(keep_below..).next().is_none());
        // The store reads the chunks in place, and hands them back.
        let chunks = Rc::new(std::mem::take(&mut self.dirty));
        let result = self
            .store
            .commit(&self.name, self.size, &chunks, keep_below);
        let written = chunks.len() as u64;
        if let Err(e) = result {
            self.dirty = Rc::try_unwrap(chunks).unwrap_or_else(|kept| (*kept).clone());
            return Err(io_err(VfsErrorCode::IoSync, e));
        }
        {
            let mut s = self.stats.borrow_mut();
            s.commits += 1;
            s.chunk_writes += written;
        }
        self.truncated_to = None;
        self.stored_size = self.size;
        self.refused_at_cap = false;
        Ok(())
    }

    fn size(&self) -> u64 {
        self.size
    }
}

/// A file the VFS has open: a database file in a store, or a journal or
/// temporary file in memory.
enum DoFile {
    Stored(StoredFile),
    Memory(MemChunksFile),
}

impl VfsFile for DoFile {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        match self {
            DoFile::Stored(f) => f.read(buf, offset),
            DoFile::Memory(f) => f.read(buf, offset),
        }
    }
    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        match self {
            DoFile::Stored(f) => f.write(buf, offset),
            DoFile::Memory(f) => f.write(buf, offset),
        }
    }
    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        match self {
            DoFile::Stored(f) => f.truncate(size),
            DoFile::Memory(f) => f.truncate(size),
        }
    }
    fn sync(&mut self, options: SyncOptions) -> VfsResult<()> {
        match self {
            DoFile::Stored(f) => f.flush(),
            DoFile::Memory(f) => f.sync(options),
        }
    }
    fn size(&self) -> VfsResult<u64> {
        match self {
            DoFile::Stored(f) => Ok(f.size()),
            DoFile::Memory(f) => f.size(),
        }
    }
    // Locks are no-ops: one connection per database file, in one thread.
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

/// SQLite's handle on an open file: the name of a file in this thread's
/// registry, or a temporary file with no name, which is the handle's alone.
/// It holds nothing of the registry, which is not `Send`, so a connection
/// moved to another thread finds none of its files there.
enum Handle {
    Named(String),
    Anonymous(MemChunksFile),
}

// The handle goes wherever its connection does, which may be another thread.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<Handle>();
};

impl Handle {
    /// Runs `f` on the file, if it is open on this thread.
    fn with<T>(&mut self, f: impl FnOnce(&mut dyn VfsFile) -> VfsResult<T>) -> VfsResult<T> {
        let name = match self {
            Handle::Anonymous(m) => return f(m),
            Handle::Named(name) => name.as_str(),
        };
        REGISTRY
            .try_with(|r| {
                let Ok(mut r) = r.try_borrow_mut() else {
                    return Err(io_err(VfsErrorCode::Misuse, "the VFS's registry is in use"));
                };
                match r.files.get_mut(name) {
                    Some(file) => f(file),
                    None => Err(not_open(VfsErrorCode::Io, name)),
                }
            })
            .unwrap_or_else(|_| Err(not_open(VfsErrorCode::Io, name)))
    }
}

impl VfsFile for Handle {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        self.with(|f| f.read(buf, offset))
    }
    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        self.with(|f| f.write(buf, offset))
    }
    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        self.with(|f| f.truncate(size))
    }
    fn sync(&mut self, options: SyncOptions) -> VfsResult<()> {
        self.with(|f| f.sync(options))
    }
    fn size(&self) -> VfsResult<u64> {
        let name = match self {
            Handle::Anonymous(m) => return m.size(),
            Handle::Named(name) => name.as_str(),
        };
        REGISTRY
            .try_with(|r| {
                let Ok(r) = r.try_borrow() else {
                    return Err(io_err(VfsErrorCode::Misuse, "the VFS's registry is in use"));
                };
                match r.files.get(name) {
                    Some(file) => file.size(),
                    None => Err(not_open(VfsErrorCode::Io, name)),
                }
            })
            .unwrap_or_else(|_| Err(not_open(VfsErrorCode::Io, name)))
    }
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
    name: String,
    store: Rc<dyn PageStore>,
    limits: Limits,
}

/// The VFS's state on this thread. Files are keyed by the name SQLite knows
/// them by: for a database file, the one its [`open`] made up, unique to
/// that open.
#[derive(Default)]
struct Registry {
    pending: HashMap<String, Pending>,
    files: HashMap<String, DoFile>,
    /// For each name a store keeps a database file under, the SQLite file
    /// name of its latest [`open`].
    latest: HashMap<String, String>,
    /// Opens so far, which make each SQLite file name unique.
    opens: u64,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::default();
    /// The last error of a VFS call on this thread, for `xGetLastError`.
    static LAST_ERROR: RefCell<Option<VfsError>> = const { RefCell::new(None) };
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

/// The registry, for a VFS call: an error rather than a panic when the
/// thread's registry is gone or already in use.
fn with_registry<T>(f: impl FnOnce(&mut Registry) -> VfsResult<T>) -> VfsResult<T> {
    REGISTRY
        .try_with(|r| match r.try_borrow_mut() {
            Ok(mut r) => f(&mut r),
            Err(_) => Err(io_err(VfsErrorCode::Misuse, "the VFS's registry is in use")),
        })
        .unwrap_or_else(|_| Err(io_err(VfsErrorCode::Misuse, "the VFS's registry is gone")))
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
        let access = request.options.access();
        let Some(file) = request.filename.map(|f| f.path()) else {
            // A temporary file with no name: its handle's alone.
            return Ok(OpenedFile {
                file: Handle::Anonymous(MemChunksFile::new(CHUNK_SIZE)),
                access,
            });
        };
        let kind = request.options.kind();
        if kind == Some(FileKind::Wal) {
            return Err(io_err(
                VfsErrorCode::CantOpen,
                format!(
                    "{file}: a WAL would keep commits out of the store; journal_mode must be MEMORY"
                ),
            ));
        }
        with_registry(|r| {
            let f = match r.pending.remove(file) {
                Some(p) if kind == Some(FileKind::MainDb) => {
                    DoFile::Stored(StoredFile::open(&p.name, p.store, p.limits)?)
                }
                Some(p) => {
                    r.pending.insert(file.to_owned(), p);
                    return Err(io_err(
                        VfsErrorCode::CantOpen,
                        format!("{file} is a database file, opened as another kind"),
                    ));
                }
                // A temporary file opened again by name keeps its contents.
                None if in_memory(kind) => match r.files.remove(file) {
                    Some(f @ DoFile::Memory(_)) => f,
                    _ => DoFile::Memory(MemChunksFile::new(CHUNK_SIZE)),
                },
                None if matches!(r.files.get(file), Some(DoFile::Stored(_))) => {
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
            r.files.insert(file.to_owned(), f);
            Ok(OpenedFile {
                file: Handle::Named(file.to_owned()),
                access,
            })
        })
    }

    fn close_file(
        _data: &(),
        _name: Option<&str>,
        file: Handle,
        options: OpenOptions,
    ) -> VfsResult<()> {
        if let Handle::Named(name) = file
            && options.delete_on_close()
        {
            with_registry(|r| {
                if matches!(r.files.get(&name), Some(DoFile::Memory(_))) {
                    r.files.remove(&name);
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn access(_data: &(), file: &str, _mode: AccessMode) -> VfsResult<bool> {
        // SQLite asks only about journals and WAL files, which this VFS never
        // keeps: a temporary file reads as absent, and a database file is
        // created by its open, once.
        with_registry(|r| match r.files.get(file) {
            Some(DoFile::Stored(_)) => Err(io_err(
                VfsErrorCode::CantOpen,
                format!("{file} is open: a database file takes one connection"),
            )),
            Some(DoFile::Memory(_)) | None => Ok(false),
        })
    }

    fn full_pathname(_data: &(), name: &str) -> VfsResult<String> {
        Ok(name.to_owned())
    }

    fn delete_file(_data: &(), file: &str, _sync_dir: bool) -> VfsResult<()> {
        with_registry(|r| {
            if let Some(DoFile::Stored(f)) = r.files.remove(file) {
                f.store
                    .delete(&f.name)
                    .map_err(|e| io_err(VfsErrorCode::IoDelete, e))?;
            }
            Ok(())
        })
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

/// What SQLite asks the VFS for beside files: sleeps (none: the isolate has
/// one thread, and no connection waits on another), randomness, the time.
struct Os;

impl OsCallback for Os {
    fn sleep(&self, _dur: Duration) {}

    fn random(&self, buf: &mut [u8]) -> usize {
        os::random(buf);
        buf.len()
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        Ok(os::epoch_ms())
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

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod os {
    // sqlite-wasm-rs's rsqlite-vfs, whose `OsCallback` its shim implements.
    use sqlite_wasm_rs::WasmOsCallback;
    use sqlite_wasm_rs::utils::OsCallback;

    pub fn random(buf: &mut [u8]) {
        WasmOsCallback::random(buf)
    }
    pub fn epoch_ms() -> i64 {
        WasmOsCallback::epoch_timestamp_in_ms()
    }
}

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
mod os {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Randomness for temporary file names; not for cryptography.
    pub fn random(buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let n = RandomState::new().build_hasher().finish().to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
    }
    pub fn epoch_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
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
    // process registers `VFS_NAME`. On wasm32 the isolate has one thread;
    // natively, the SQLite libsqlite3-sys bundles is built serialized
    // (`SQLITE_THREADSAFE=1`). The VFS keeps its state in thread-locals, and
    // a handle, which moves with its connection, is `Send` and holds only
    // a name to look up there, so the callbacks may run on any thread.
    #[allow(unsafe_code)]
    let registered = unsafe { rsqlite_vfs::register_vfs::<IoMethods, Vfs>(VFS_NAME, (), false) };
    registered.map(|r| {
        r.into_raw();
    })
}

fn register() -> rusqlite::Result<()> {
    static ONCE: Once = Once::new();
    let mut result = Ok(());
    ONCE.call_once(|| {
        // Opening a connection initializes SQLite, which a VFS registration
        // needs on builds that do not auto-initialize.
        result = Connection::open_in_memory().map(drop).and_then(|()| {
            register_vfs().map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                    Some(e.to_string()),
                )
            })
        });
    });
    result
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
    name: String,
    sqlite_name: String,
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        // The registry may be gone already when the thread exits.
        let _ = REGISTRY.try_with(|r| {
            let mut r = r.borrow_mut();
            r.pending.remove(&self.sqlite_name);
            r.files.remove(&self.sqlite_name);
            if r.latest.get(&self.name) == Some(&self.sqlite_name) {
                r.latest.remove(&self.name);
            }
        });
    }
}

/// [`open_with`] with [`Limits::default`].
pub fn open(name: &str, store: Rc<dyn PageStore>) -> rusqlite::Result<Database> {
    open_with(name, store, Limits::default())
}

/// Opens (creating if absent) the database file `name` kept in `store`, on
/// this thread, with the settings the VFS relies on:
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
/// A database file is opened by one connection at a time. Opening `name`
/// again replaces the earlier open: that connection's next read or write of
/// the file fails, so a connection left over from an earlier instance cannot
/// write into the later one's store.
pub fn open_with(
    name: &str,
    store: Rc<dyn PageStore>,
    limits: Limits,
) -> rusqlite::Result<Database> {
    register()?;
    let file = REGISTRY.with_borrow_mut(|r| {
        r.opens += 1;
        let sqlite_name = format!("{}/{name}", r.opens);
        if let Some(earlier) = r.latest.insert(name.to_owned(), sqlite_name.clone()) {
            r.pending.remove(&earlier);
            r.files.remove(&earlier);
        }
        r.pending.insert(
            sqlite_name.clone(),
            Pending {
                name: name.to_owned(),
                store,
                limits,
            },
        );
        OpenFile {
            name: name.to_owned(),
            sqlite_name,
        }
    });
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

/// The counters for the database file `name` on this thread, if it is open.
pub fn stats(name: &str) -> Option<FileStats> {
    REGISTRY.with_borrow(|r| match r.files.get(r.latest.get(name)?) {
        Some(DoFile::Stored(f)) => Some(*f.stats.borrow()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A store that counts its commits and keeps nothing else between them
    /// but the chunks.
    #[derive(Default)]
    struct Mem {
        files: RefCell<HashMap<String, (u64, Chunks)>>,
        commits: Cell<u64>,
    }

    impl PageStore for Mem {
        fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
            Ok(self.files.borrow().get(file).map(|f| f.0))
        }
        fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
            Ok(self
                .files
                .borrow()
                .get(file)
                .and_then(|f| f.1.get(&index).cloned()))
        }
        fn commit(
            &self,
            file: &str,
            size: u64,
            chunks: &Rc<Chunks>,
            keep_below: u64,
        ) -> Result<(), StoreError> {
            let mut files = self.files.borrow_mut();
            let f = files.entry(file.to_owned()).or_default();
            f.0 = size;
            f.1.extend(chunks.iter().map(|(i, c)| (*i, c.clone())));
            f.1.split_off(&keep_below);
            self.commits.set(self.commits.get() + 1);
            Ok(())
        }
        fn delete(&self, file: &str) -> Result<(), StoreError> {
            self.files.borrow_mut().remove(file);
            Ok(())
        }
    }

    /// Over many opens, closes and replacements, with temporary files opened
    /// and deleted in between and a stale connection always alive, each
    /// connection reaches only its own store, and the stale one none.
    #[test]
    fn many_opens_of_one_name_each_reach_their_own_store() {
        let mut stale: Option<(Database, Rc<Mem>)> = None;
        // A connection closed while its file stays registered.
        let mut parked: Vec<Registered> = Vec::new();
        for i in 0..300u64 {
            let store = Rc::new(Mem::default());
            let db = open("reuse.db", store.clone()).unwrap();
            db.execute_batch(
                "PRAGMA temp_store=FILE;
                 CREATE TABLE IF NOT EXISTS t(k INTEGER PRIMARY KEY, v BLOB);
                 CREATE TEMP TABLE scratch(v BLOB);
                 INSERT INTO scratch SELECT randomblob(9000) FROM (SELECT 1 UNION SELECT 2);
                 SELECT v FROM scratch ORDER BY v;",
            )
            .unwrap();
            db.execute("INSERT INTO t VALUES (?1, randomblob(100))", [i as i64])
                .unwrap();
            if let Some((old, old_store)) = stale.take() {
                let commits = old_store.commits.get();
                assert!(
                    old.execute("INSERT INTO t VALUES (-1, 'stale')", [])
                        .is_err(),
                    "open {i}: the replaced connection still writes"
                );
                assert_eq!(old_store.commits.get(), commits, "open {i}");
            }
            // Another database, closed while its file stays registered.
            let other = Rc::new(Mem::default());
            let (conn, registered) = open(&format!("other-{}.db", i % 10), other.clone())
                .unwrap()
                .into_parts();
            conn.execute_batch("CREATE TABLE IF NOT EXISTS t(k)")
                .unwrap();
            drop(conn);
            parked.push(registered);
            if parked.len() > 5 {
                parked.remove(0);
            }
            // Only this open's store holds key i.
            let keys: Vec<i64> = db
                .prepare("SELECT k FROM t")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(keys, [i as i64], "open {i}");
            stale = Some((db, store));
        }
    }
}
