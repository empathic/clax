//! Artifact content: the files of every version and the assets of every
//! artifact, behind [`ContentStore`].
//!
//! The store ([`crate::Store`]) keeps what Clax knows about content in its
//! database: each version's row lists its files with their sizes, content
//! types and hashes, and each asset's row its type, size and extension. A
//! [`ContentStore`] keeps only the bytes. Every read goes through a row first,
//! so bytes no row names are never served.
//!
//! [`fs::FsContent`] keeps them under the Clax home, in the layout every
//! home already has; [`memory::MemContent`] keeps them in memory. A backend
//! over git (Cloudflare Artifacts: one repository per artifact, one commit
//! per version on `main` tagged `v<n>`) is meant to implement the same trait.
//! The rules below are the ones any implementation keeps, and
//! [`contract`] checks them.
//!
//! # Versions
//!
//! A version is an immutable set of files, a slash-separated path each,
//! identified by its artifact and its number `n` (1, 2, 3, … with no gaps).
//! [`ContentStore::write_version`] makes version `n` from version `n - 1`
//! plus changes: the paths [`VersionWrite::carry`] names keep their bytes from
//! `n - 1` (Clax carries forward every file a publish does not replace or
//! remove), and [`VersionWrite::put`] gives the rest. A backend can carry a
//! file without moving its bytes (git reuses the blob in the new tree). The
//! write is all or nothing and create-only: while version `n` is present it
//! is never replaced, and a write to it fails, changing nothing. Each
//! [`NewFile`] carries the content type Clax records for it; the rows remain
//! the record of types, and a backend may keep them or not (the filesystem
//! does not, and git trees have no place for them).
//!
//! [`ContentStore::read_file`] reads one file of one version without
//! reading the rest of it (git reads one blob at a ref, the filesystem one
//! file). [`ContentStore::copy_version`] makes a version of one artifact
//! from a version of another (the re-filing of live-page threads copies a
//! page's history onto another page this way); its default reads each file
//! and writes it. [`ContentStore::remove_version`] takes away a version that
//! no row records: the write of a publish whose transaction then failed.
//! [`ContentStore::delete_artifact`] removes every version and asset of an
//! artifact.
//!
//! In git terms: version `n` is the commit tagged `v<n>`, whose parent is
//! `v<n - 1>`; a write is one push that creates the tag `v<n>` (it must not
//! exist) and moves `main` from `v<n - 1>`, so a second writer of `n` is
//! refused by the remote; removing an unrecorded version deletes its tag and
//! moves `main` back; a copy rebuilds the source's tree in the target's
//! repository (repositories share no objects), and a whole artifact's
//! history copies with a repository fork.
//!
//! # Assets
//!
//! Assets belong to one artifact and are not versioned: an upload makes a new
//! asset with a fresh ID (a ULID) and its own bytes, which never change, and
//! a delete removes it. Every version of the artifact may reference it by
//! its `/_blob/<id>` URL. They are addressed by [`AssetKey`]: the artifact,
//! the asset's ID and the extension its type gives (the filesystem names the
//! file `<id>.<ext>`). An asset is therefore not part of any version: a git
//! backend keeps assets outside the version history (for example on a ref
//! of their own, one commit per upload, or in an object store), so a
//! version's tree holds only its files.
//!
//! # Ordering
//!
//! The store's methods that run SQL are synchronous; a backend's I/O is
//! asynchronous (JavaScript promises inside a Worker). So content I/O never
//! runs inside a store transaction. Each write orders the two so a failure
//! on either side leaves no half-written state, that is, no row naming bytes
//! that are not there:
//!
//! - **Publish** (and every other new version): under the artifact's content
//!   lock ([`ArtifactLocks`]), the current version is checked, the version's
//!   bytes are written as `n`, and then one transaction checks the current
//!   version again and records `n`. If the transaction fails, `n` is removed.
//!   Bytes at `n` without a row (a process that died between the two) are
//!   never served, and block the next write of `n` until removed, as a stray
//!   version directory always has; `clax doctor` lists them.
//! - **Asset upload**: the bytes are written, then the row inserted; if the
//!   insert fails the bytes are deleted.
//! - **Asset delete**: the bytes are deleted, then the row; deleting bytes
//!   already gone succeeds, so a retry after a failure removes the row.
//! - **Artifact delete**: the transaction marks the artifact deleted, then
//!   its content is removed; a failure there leaves unreferenced bytes
//!   behind and is logged.
//!
//! # Futures
//!
//! The trait is written with `async_trait`. Natively its futures are `Send`
//! and an implementation `Send + Sync`, so the daemon may hold it in an
//! `Arc` and run it from any thread; on `wasm32` neither is required, since a
//! Worker's JavaScript futures are `!Send` and run on one thread. The bound
//! switches with [`MaybeSendSync`]. The store's synchronous entry points
//! (every caller today) drive the futures with [`wait`], which suits an
//! implementation whose futures do their work when first polled, as the
//! filesystem's and the in-memory one do; an implementation that needs an
//! executor is driven through the store's `async` methods instead.

pub mod fs;
pub mod memory;

#[cfg(any(test, feature = "content-contract"))]
pub mod contract;

use crate::{ArtifactId, Result};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

/// `Send + Sync` natively; nothing on `wasm32`, where a Worker's futures and
/// bindings are single-threaded.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}

/// `Send + Sync` natively; nothing on `wasm32`, where a Worker's futures and
/// bindings are single-threaded.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// A file a new version writes: its path, bytes and recorded content type.
#[derive(Clone, Copy, Debug)]
pub struct NewFile<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub content_type: &'a str,
}

/// What version `n` is made of: the paths carried from version `n - 1` and
/// the files written. No path appears twice.
#[derive(Clone, Debug, Default)]
pub struct VersionWrite<'a> {
    pub carry: Vec<&'a str>,
    pub put: Vec<NewFile<'a>>,
}

/// An asset's address: its artifact, its ID and its extension.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetKey {
    pub artifact: ArtifactId,
    pub id: String,
    pub ext: String,
}

impl AssetKey {
    /// The key of `asset`.
    ///
    /// # Errors
    /// `Corrupt` when its artifact ID is not a valid ID.
    pub fn of(asset: &crate::model::Asset) -> Result<AssetKey> {
        let artifact =
            ArtifactId::parse(&asset.artifact_id).map_err(|_| crate::CoreError::Corrupt {
                artifact_id: asset.artifact_id.clone(),
                column: "assets.artifact_id",
                version: None,
            })?;
        Ok(AssetKey {
            artifact,
            id: asset.id.clone(),
            ext: asset.ext.clone(),
        })
    }
}

/// Where artifact content is kept. See the [module docs](self) for the rules
/// every implementation keeps.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait ContentStore: MaybeSendSync {
    /// The bytes of `path` in version `n` of `id`; `None` when the version
    /// or the file is not there.
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>>;

    /// Makes version `n` of `id` from `write`: its carried paths with their
    /// bytes in version `n - 1`, and its put files. All or nothing: on any
    /// error nothing is visible at `n`. Fails, changing nothing, when version
    /// `n` is already present, or when a carried path is not in `n - 1`.
    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()>;

    /// Makes version `to_n` of `to` hold `paths` with their bytes in version
    /// `from_n` of `from`, as [`ContentStore::write_version`] makes a version
    /// (all or nothing, create-only). Fails when a path is not in the source.
    /// The default reads each file and writes it, with an empty content type
    /// (the target's row records the types).
    async fn copy_version(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        to_n: u32,
    ) -> Result<()> {
        let mut read = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = self
                .read_file(from, from_n, path)
                .await?
                .ok_or_else(|| missing(from, from_n, path))?;
            read.push((*path, bytes));
        }
        let write = VersionWrite {
            carry: Vec::new(),
            put: read
                .iter()
                .map(|(path, bytes)| NewFile {
                    path,
                    bytes,
                    content_type: "",
                })
                .collect(),
        };
        self.write_version(to, to_n, &write).await
    }

    /// Removes version `n` of `id`, which no row records. Succeeds when it
    /// is not there.
    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()>;

    /// Removes every version and asset of `id`. Succeeds when there are
    /// none.
    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()>;

    /// Stores `bytes` as the asset `key`, which is visible only once they
    /// are all written.
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()>;

    /// The bytes of asset `key`, or `None` when it is not there.
    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>>;

    /// Removes asset `key`. Succeeds when it is not there.
    async fn delete_asset(&self, key: &AssetKey) -> Result<()>;
}

/// The error for a file a version write or copy expected to find.
pub(crate) fn missing(id: &ArtifactId, n: u32, path: &str) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{path} is not in version {n} of {id}"),
    ))
}

/// The error for a write to a version that is already present.
pub(crate) fn present(id: &ArtifactId, n: u32) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("version {n} of {id} is already present"),
    ))
}

/// Runs `f` to completion on this thread. For futures that do their work
/// when polled ([`fs::FsContent`]'s and [`memory::MemContent`]'s, and the
/// store's locks), which is what the store's synchronous methods drive;
/// never for one that waits on an executor or a JavaScript promise. Calls
/// may nest.
///
/// # Panics
/// On `wasm32`, when `f` is not ready at once: a Worker has one thread,
/// so whatever would wake it (a promise, another request holding a lock)
/// can never run while this one waits, and parking there returns at once.
pub fn wait<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(out) => return out,
            #[cfg(target_arch = "wasm32")]
            std::task::Poll::Pending => {
                panic!(
                    "content::wait: a future is pending on wasm32; await the store's async methods"
                )
            }
            // Woken by an unpark; a spurious wake polls again.
            #[cfg(not(target_arch = "wasm32"))]
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

/// Wakes [`wait`]'s thread.
struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// One lock per artifact, held from a version write's first check of the
/// current version to the end of the transaction that records it (see
/// [Ordering](self#ordering)), so a second writer of the same version waits
/// and then sees it recorded, as it would waiting for the writer. The locks
/// are futures-aware, so a Worker awaits them; an artifact's entry goes when
/// no one holds or waits for it.
#[derive(Default)]
pub struct ArtifactLocks {
    map: Mutex<HashMap<String, Arc<futures_util::lock::Mutex<()>>>>,
}

/// An artifact's content lock, held until dropped.
pub struct ArtifactLock<'a> {
    locks: &'a ArtifactLocks,
    key: String,
    guard: Option<futures_util::lock::OwnedMutexGuard<()>>,
}

impl ArtifactLocks {
    /// Waits for, then holds, the lock of `id`.
    pub async fn lock(&self, id: &ArtifactId) -> ArtifactLock<'_> {
        let m = {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            map.entry(id.as_str().to_string()).or_default().clone()
        };
        let guard = m.lock_owned().await;
        ArtifactLock {
            locks: self,
            key: id.as_str().to_string(),
            guard: Some(guard),
        }
    }

    /// Holds the lock of `id` when no one else does; `None` otherwise.
    pub fn try_lock(&self, id: &ArtifactId) -> Option<ArtifactLock<'_>> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let m = map.entry(id.as_str().to_string()).or_default().clone();
        let guard = m.try_lock_owned();
        drop(m);
        let Some(guard) = guard else {
            // Held by someone else, so the entry stays.
            return None;
        };
        drop(map);
        Some(ArtifactLock {
            locks: self,
            key: id.as_str().to_string(),
            guard: Some(guard),
        })
    }

    /// Holds the locks of every one of `ids`, taken in ID order so two
    /// callers locking overlapping sets never wait on each other in a cycle.
    pub async fn lock_all(&self, ids: &[&ArtifactId]) -> Vec<ArtifactLock<'_>> {
        let mut sorted: Vec<&ArtifactId> = ids.to_vec();
        sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        sorted.dedup_by(|a, b| a.as_str() == b.as_str());
        let mut out = Vec::with_capacity(sorted.len());
        for id in sorted {
            out.push(self.lock(id).await);
        }
        out
    }

    #[cfg(test)]
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

impl Drop for ArtifactLock<'_> {
    fn drop(&mut self) {
        self.guard.take();
        let mut map = self.locks.map.lock().unwrap_or_else(|e| e.into_inner());
        // Only the map's own reference left: no holder and no waiter.
        if map
            .get(&self.key)
            .is_some_and(|m| Arc::strong_count(m) == 1)
        {
            map.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_entry_goes_once_nobody_holds_it() {
        let locks = ArtifactLocks::default();
        let a = ArtifactId::generate();
        let b = ArtifactId::generate();
        {
            let held = wait(locks.lock_all(&[&b, &a, &b]));
            assert_eq!(held.len(), 2);
            assert_eq!(locks.entries(), 2);
        }
        assert_eq!(locks.entries(), 0);
    }

    #[test]
    fn try_lock_holds_a_free_lock_only() {
        let locks = ArtifactLocks::default();
        let id = ArtifactId::generate();
        let first = locks.try_lock(&id).expect("free");
        assert!(locks.try_lock(&id).is_none());
        assert_eq!(locks.entries(), 1);
        drop(first);
        assert_eq!(locks.entries(), 0);
        let again = locks.try_lock(&id).expect("free again");
        drop(again);
        assert_eq!(locks.entries(), 0);
    }

    #[test]
    fn a_second_holder_waits_for_the_first() {
        let locks = Arc::new(ArtifactLocks::default());
        let id = ArtifactId::generate();
        let first = wait(locks.lock(&id));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let _second = wait(locks.lock(&id));
                tx.send(()).unwrap();
            });
            assert!(rx.try_recv().is_err());
            drop(first);
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        });
        assert_eq!(locks.entries(), 0);
    }
}
