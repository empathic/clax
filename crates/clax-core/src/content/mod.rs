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
//! An artifact's versions are written in ascending order, and only the
//! newest is ever removed. Version `n` follows its predecessor, the newest
//! present version below `n`: normally `n - 1`, but `n - 1` may be absent
//! (a row can name a version whose content was lost, or an empty first
//! version), and then `n` follows the newest present one below it, or
//! starts the history when there is none, as version 1 does. Carried paths
//! come from `n - 1` only, so a write that carries fails while `n - 1` is
//! absent. A write of `n` fails, changing nothing, while a version above `n`
//! is present; so does [`ContentStore::remove_version`] of `n`. The store
//! never asks for either: it writes `current + 1` under the artifact's lock
//! and removes only what it just wrote, newest first.
//!
//! [`ContentStore::read_file`] reads one file of one version without
//! reading the rest of it (git reads one blob at a ref, the filesystem one
//! file). [`ContentStore::copy_version`] makes a version of one artifact
//! from a version of another (the re-filing of live-page threads copies a
//! page's history onto another page this way); its default reads each file
//! and writes it. [`ContentStore::fork`] makes the first version of a new
//! artifact from a version of another, keeping the source's history where
//! the backend has one (hosted Clax forks an artifact this way); its
//! default is a copy to version 1. [`ContentStore::remove_version`] takes
//! away a version that no row records: the write of a publish whose
//! transaction then failed. [`ContentStore::delete_artifact`] removes every
//! version and asset of an artifact.
//!
//! In git terms: version `n` is the commit tagged `v<n>`, whose first
//! parent leads to its predecessor's commit (none for the first). A write
//! is one atomic update that creates the tag `v<n>` (it must not exist)
//! and moves `main` forward from where it must be (compare and swap), so a
//! second writer of `n`, or any writer that finds `main` moved, is refused
//! and changes nothing. Removing an unrecorded version deletes `v<n>` only
//! while the tag still names the version's commit, and moves `main` back
//! to that commit's parent while `main` is still at that commit; where the
//! remote refuses that move (not a fast forward, or for a first version
//! the deletion of the current branch), `main` keeps the untagged commit,
//! which is never read and which the next write builds on. A remove never
//! discards a commit another writer put on `main`; when `main` has moved
//! past it, the remove fails and leaves both. `clax-content-git` gives the
//! details. A copy rebuilds the source's files in the target's repository
//! (repositories need share no objects), and a fork copies the source's
//! repository with its history and commits version 1 on top.
//!
//! # Pushes
//!
//! A backend over git lets a person, or their agent, push to an artifact's
//! `main` with git or jj. A pushed commit is a version once the store
//! records it ([`crate::Store::reconcile_pushes_async`]), which it does
//! under the artifact's content lock and before every new version:
//! [`ContentStore::pushed_commits`] lists `main`'s first-parent commits
//! above the commit of the version pushed past, oldest first, without the
//! untagged commits a remove left directly above that version (publish
//! attempts, never versions); [`ContentStore::pushed_files`] answers each
//! one's files against the version below it; [`ContentStore::tag_pushed`]
//! makes it version `n` (in git, creates `v<n>` on it, create-only) before
//! the transaction recording `n`, and [`ContentStore::untag_pushed`] undoes
//! that when the transaction fails. `main` stays where the push left it,
//! so the next write finds it at the newest version's commit. Stores that
//! take no pushes (the filesystem's, the in-memory one) list none.
//!
//! Who pushed is kept beside the repository, by whatever serves the push
//! (a [`PushLedger`]): [`ContentStore::pushers`] names the pusher of each
//! pushed commit it knows, [`ContentStore::repo_tokens`] lists the
//! repository's live tokens with whom each was handed to, and
//! [`ContentStore::revoke_repo_token`] revokes one, so a version an
//! agent's person may not see is never recorded while that agent's token
//! can still fetch it. A store with no ledger knows no pusher and no token.
//!
//! # Assets
//!
//! Assets belong to one artifact and are not versioned: an upload makes a new
//! asset with a fresh ID (a ULID) and its own bytes, which never change, and
//! a delete removes it. Every version of the artifact may reference it by
//! its `/_blob/<id>` URL. They are addressed by [`AssetKey`]: the artifact,
//! the asset's ID and the extension its type gives (the filesystem names the
//! file `<id>.<ext>`). An asset is therefore not part of any version: a git
//! backend keeps assets outside the version history (`clax-content-git`
//! keeps them in an asset store of their own the host provides), so a
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
//! The trait is written with `async_trait`. An implementation is `Send +
//! Sync` on every target, so a [`Store`](crate::store::Store) holding it in
//! an `Arc` is too, as the server's state must be (axum's bounds hold on
//! `wasm32` as well). Natively its futures are `Send`; on `wasm32` they need
//! not be, since a Worker's JavaScript futures are `!Send` and run on one
//! thread, and an implementation there keeps its JavaScript bindings in a
//! `SendWrapper` (as the Worker keeps its page store). The store's
//! synchronous entry points drive the futures themselves, which suits an
//! implementation whose futures do their work when first polled, as the
//! filesystem's and the in-memory one do; an implementation that needs an
//! executor is driven through the store's `async` methods instead (see the
//! `store` module's Content for what a synchronous method does when it would
//! wait).
//!
//! # Clips
//!
//! A thread's clip image (the screenshot taken with its first comment)
//! belongs to its artifact, like an asset: not versioned, removed with the
//! artifact. By default a backend keeps it as an asset ([`clip_key`]);
//! [`fs::FsContent`] keeps it at `artifacts/<id>/clips/<thread>.png`, where
//! every home has it.

pub mod fs;
pub mod memory;

#[cfg(any(test, feature = "content-contract"))]
pub mod contract;

use crate::{ArtifactId, Result};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

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
    /// Who published the version and why, for a backend that keeps it with
    /// the content.
    pub meta: VersionMeta,
}

/// Who published a version and the note they gave, as the store knows them
/// when it writes the version's content. A backend that keeps history
/// records them with the version (git: the commit's author, message and
/// `Clax-Publisher` and `Clax-Agent` trailers), so the history itself shows
/// who made each version; the filesystem and memory stores ignore them.
/// The version's row stays the record Clax reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionMeta {
    /// The publishing person's public ID: the owner's, or a connected
    /// person's. `None` when the store knows no ID for them (the owner of
    /// a home that has not yet given its owner one).
    pub person: Option<String>,
    /// That person's display name, when they have one.
    pub person_name: Option<String>,
    /// The publishing agent's name, when an agent session published.
    pub agent: Option<String>,
    /// The publish note, when one was given.
    pub note: Option<String>,
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
pub trait ContentStore: Send + Sync {
    /// The bytes of `path` in version `n` of `id`; `None` when the version
    /// or the file is not there.
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>>;

    /// Makes version `n` of `id` from `write`: its carried paths with their
    /// bytes in version `n - 1`, and its put files. All or nothing: on any
    /// error nothing is visible at `n`. Fails, changing nothing, when version
    /// `n` or a version above it is present, or when a carried path is not
    /// in `n - 1` (or `n - 1` is absent). See [Versions](self#versions) for
    /// a version whose predecessor is not `n - 1`.
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
            meta: VersionMeta::default(),
        };
        self.write_version(to, to_n, &write).await
    }

    /// Makes version 1 of `to`, a new artifact with no versions, hold
    /// `paths` with their bytes in version `from_n` of `from`, as
    /// [`ContentStore::copy_version`] makes a version (all or nothing,
    /// create-only). A backend that keeps history gives `to` the history of
    /// `from` up to version `from_n` behind its version 1 (nothing `from`
    /// gained after `from_n`), without `from`'s versions, and records the
    /// forking person and agent in `meta` as
    /// [`ContentStore::write_version`] records a publisher (its note aside:
    /// version 1 says where it came from): git forks the repository,
    /// commits version 1 on `from_n`'s commit, keeps only `main` and `v1`,
    /// and drops the objects they do not reach. Fails, changing nothing,
    /// when `to` has a version or a path is not in the source. The default
    /// copies.
    async fn fork(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        meta: &VersionMeta,
    ) -> Result<()> {
        let _ = meta;
        self.copy_version(from, from_n, paths, to, 1).await
    }

    /// Removes version `n` of `id`, which no row records. Succeeds when it
    /// is not there; fails, changing nothing, while a version above `n` is
    /// present (only the newest version is removed).
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

    /// Stores `bytes` as the clip image of thread `thread` of `id`,
    /// replacing any it had; visible only once they are all written. The
    /// default keeps it as the asset [`clip_key`] names, so it goes with
    /// the artifact's other content.
    async fn put_clip(&self, id: &ArtifactId, thread: &str, bytes: &[u8]) -> Result<()> {
        self.put_asset(&clip_key(id, thread), bytes).await
    }

    /// The clip image of thread `thread` of `id`, or `None` when it has
    /// none.
    async fn read_clip(&self, id: &ArtifactId, thread: &str) -> Result<Option<Vec<u8>>> {
        self.read_asset(&clip_key(id, thread)).await
    }

    /// Removes the clip image of thread `thread` of `id`. Succeeds when it
    /// is not there.
    async fn delete_clip(&self, id: &ArtifactId, thread: &str) -> Result<()> {
        self.delete_asset(&clip_key(id, thread)).await
    }

    /// The commits pushed past version `after` of `id` that no version
    /// records, oldest first (see [Pushes](self#pushes)); none (the
    /// default) for a store that is not a repository someone can push to.
    async fn pushed_commits(&self, id: &ArtifactId, after: u32) -> Result<Vec<PushedCommit>> {
        let _ = (id, after);
        Ok(Vec::new())
    }

    /// The files of pushed commit `commit` of `id`, each compared with the
    /// same path in version `base` (see [`PushedContent`]): a store reads
    /// each changed blob once, however many paths name it, and at most
    /// [`MAX_BODY_BYTES`](crate::publish::MAX_BODY_BYTES) of them in all
    /// (or less, as its host bounds it), answering the files past that
    /// [`PushedContent::TooLarge`]; a commit whose trees hold more than
    /// [`MAX_PUSHED_ENTRIES`] is `push_too_large`.
    async fn pushed_files(
        &self,
        id: &ArtifactId,
        commit: &str,
        base: u32,
    ) -> Result<Vec<PushedFile>> {
        let _ = base;
        Err(unsupported(id, commit))
    }

    /// Makes pushed commit `commit` of `id` version `n`, create-only: fails,
    /// changing nothing, when version `n` is present at another commit.
    /// Succeeds when it already is, answering `false` (this call made
    /// nothing, so nothing of it is to be undone); `true` when it made it.
    async fn tag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> Result<bool> {
        let _ = n;
        Err(unsupported(id, commit))
    }

    /// Undoes [`ContentStore::tag_pushed`]: version `n` of `id` stops being
    /// commit `commit`, which stays where the push left it. Succeeds when
    /// version `n` is absent; fails, changing nothing, when it is another
    /// commit.
    async fn untag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> Result<()> {
        let _ = (id, n, commit);
        Ok(())
    }

    /// For each of `commits` of `id`, whoever made a push that left `main`
    /// at it, as far as this store knows (the newest such push); `None`
    /// for one it knows no push to. One answer per commit, in order; all
    /// `None` (the default) for a store with no [`PushLedger`].
    async fn pushers(&self, id: &ArtifactId, commits: &[String]) -> Result<Vec<Option<Pusher>>> {
        let _ = id;
        Ok(vec![None; commits.len()])
    }

    /// The live tokens of `id`'s repository, with whom each was handed to;
    /// none (the default) for a store with no [`PushLedger`].
    async fn repo_tokens(&self, id: &ArtifactId) -> Result<Vec<RepoToken>> {
        let _ = id;
        Ok(Vec::new())
    }

    /// Revokes token `token` of `id`'s repository: once this succeeds, it
    /// opens nothing.
    async fn revoke_repo_token(&self, id: &ArtifactId, token: &str) -> Result<()> {
        Err(unsupported(id, token))
    }

    /// Whether this store's I/O completes only when an executor polls it
    /// (a Durable Object's, whose bindings are JavaScript promises), not
    /// when first polled as the filesystem's and the in-memory one do
    /// (`false`, the default). Then the store's synchronous methods fail at
    /// once with [`crate::CoreError::WouldBlock`], before any I/O or write,
    /// rather than start work they could not finish, and
    /// [`crate::Store::call_async`] awaits its jobs on the caller's task.
    fn needs_executor(&self) -> bool {
        false
    }

    /// The file on this machine that holds `path` of version `n` of `id`,
    /// for an agent on the same machine to open; `None` (the default) when
    /// the bytes are not kept in files.
    fn local_file(&self, id: &ArtifactId, n: u32, path: &str) -> Option<std::path::PathBuf> {
        let _ = (id, n, path);
        None
    }

    /// The file on this machine that holds the clip image of thread
    /// `thread` of `id`, as [`ContentStore::local_file`]; `None` by default.
    fn local_clip(&self, id: &ArtifactId, thread: &str) -> Option<std::path::PathBuf> {
        let _ = (id, thread);
        None
    }
}

/// The asset key the default clip methods keep thread `thread`'s clip
/// image under: `clip-<thread>.png`. Asset IDs are ULIDs, so no asset has
/// it.
pub fn clip_key(id: &ArtifactId, thread: &str) -> AssetKey {
    AssetKey {
        artifact: id.clone(),
        id: format!("clip-{thread}"),
        ext: "png".into(),
    }
}

/// A commit pushed to an artifact's repository that no version records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushedCommit {
    /// Its ID, in hex.
    pub commit: String,
    /// Its whole message.
    pub message: String,
    /// Its first parent is the commit of the version below it (the
    /// previous pushed commit, or the version pushed past): false when
    /// history was rewritten or when [`PushedCommit::collapsed`].
    pub follows: bool,
    /// More commits were pushed than one reconcile records, or the version
    /// pushed past is not in `main`'s history: this commit is `main`'s
    /// head, standing for all of them.
    pub collapsed: bool,
}

/// One file of a pushed commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushedFile {
    pub path: String,
    pub content: PushedContent,
}

/// What a pushed commit holds at a path, against the version it is
/// compared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushedContent {
    /// The same bytes as the same path in that version.
    Same,
    /// Other bytes, or a path that version does not have.
    Bytes(Vec<u8>),
    /// Not a regular file (a symbolic link or a submodule).
    NotAFile,
    /// Other bytes, not read: the files before it in path order changed
    /// as many bytes as the store reads of one commit.
    TooLarge,
}

/// The most entries (files and trees, a tree each time it is reached) a
/// store reads of one pushed commit's trees; past them the commit is
/// refused, `push_too_large` ([`ContentStore::pushed_files`]).
pub const MAX_PUSHED_ENTRIES: usize = 10_000;

/// Who pushed, as the push's token says: a connected person's public ID
/// (`None`: the owner) and the agent session the token was handed to, when
/// an agent's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pusher {
    pub person: Option<String>,
    pub session_id: Option<String>,
}

/// A live token of an artifact's repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoToken {
    /// Its ID (never the secret).
    pub id: String,
    /// It may push, not only fetch.
    pub write: bool,
    /// Whom it was handed to.
    pub holder: Pusher,
}

/// What serves pushes to artifacts' repositories keeps of them: who made
/// each push, and the repositories' tokens (see [Pushes](self#pushes)).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait PushLedger: Send + Sync {
    /// [`ContentStore::pushers`].
    async fn pushers(&self, id: &ArtifactId, commits: &[String]) -> Result<Vec<Option<Pusher>>>;
    /// [`ContentStore::repo_tokens`].
    async fn tokens(&self, id: &ArtifactId) -> Result<Vec<RepoToken>>;
    /// [`ContentStore::revoke_repo_token`].
    async fn revoke(&self, id: &ArtifactId, token: &str) -> Result<()>;
}

fn unsupported(id: &ArtifactId, what: &str) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{id} keeps no pushed commit or token {what}"),
    ))
}

/// The error for a file a version write or copy expected to find.
pub fn missing(id: &ArtifactId, n: u32, path: &str) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{path} is not in version {n} of {id}"),
    ))
}

/// The error for a write to a version that is already present.
pub fn present(id: &ArtifactId, n: u32) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("version {n} of {id} is already present"),
    ))
}

/// The error for a write or remove of version `n` while version `above`,
/// newer, is present.
pub fn superseded(id: &ArtifactId, n: u32, above: u32) -> crate::CoreError {
    crate::CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("version {above} of {id}, newer than version {n}, is present"),
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
// On wasm32 the loop's only other arm panics.
#[cfg_attr(target_arch = "wasm32", allow(clippy::never_loop))]
pub fn wait<F: Future>(f: F) -> F::Output {
    let _sync = SyncDriven::enter();
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

std::thread_local! {
    /// How many synchronous store methods ([`crate::Store`]'s `block_on`)
    /// this thread is inside.
    static SYNC_DRIVEN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Marks this thread as running a synchronous store method until dropped.
pub(crate) struct SyncDriven;

impl SyncDriven {
    pub(crate) fn enter() -> SyncDriven {
        SYNC_DRIVEN.with(|n| n.set(n.get() + 1));
        SyncDriven
    }
}

impl Drop for SyncDriven {
    fn drop(&mut self) {
        SYNC_DRIVEN.with(|n| n.set(n.get() - 1));
    }
}

/// Whether this thread is inside a synchronous store method or [`wait`],
/// which drive the content store's futures themselves: natively by parking
/// the thread until they wake it, on `wasm32` only as far as they go
/// without waiting ([`ready`]). A content store whose I/O needs an
/// executor (a runtime's reactor, a JavaScript promise) can fail here with
/// [`crate::CoreError::WouldBlock`] rather than wait for a wake that no
/// one may deliver; the store's `async` methods, awaited by the caller,
/// never run with this set.
pub fn driven_synchronously() -> bool {
    SYNC_DRIVEN.with(|n| n.get() > 0)
}

/// Runs `f` on this thread as far as it goes without waiting: its output
/// when it completes, polling again each time it wakes itself while
/// polled; `None`, dropping it, as soon as it is pending with no wake
/// (it waits on something only another task or an executor can make
/// ready, such as a lock held across an await or a JavaScript promise).
/// Never parks the thread, so it suits a host with one thread.
pub fn ready<F: Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    let woken = Arc::new(Woken(std::sync::atomic::AtomicBool::new(false)));
    let waker = std::task::Waker::from(woken.clone());
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(out) => return Some(out),
            std::task::Poll::Pending => {
                if !woken.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    return None;
                }
            }
        }
    }
}

/// Notes that [`ready`]'s future woke itself.
struct Woken(std::sync::atomic::AtomicBool);

impl std::task::Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
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
    /// The thread each held lock was taken on.
    holders: Mutex<HashMap<String, std::thread::ThreadId>>,
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
        self.held(id);
        ArtifactLock {
            locks: self,
            key: id.as_str().to_string(),
            guard: Some(guard),
        }
    }

    /// Notes that this thread now holds the lock of `id`.
    fn held(&self, id: &ArtifactId) {
        self.holders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.as_str().to_string(), std::thread::current().id());
    }

    /// Whether the lock of `id` is held, and was taken on this thread.
    pub fn held_here(&self, id: &ArtifactId) -> bool {
        self.holders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id.as_str())
            .is_some_and(|t| *t == std::thread::current().id())
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
        self.held(id);
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
        // Before the guard goes, so the next holder is noted after this
        // one is forgotten.
        self.locks
            .holders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
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
    fn ready_runs_a_future_that_needs_no_waiting_and_gives_up_on_one_that_does() {
        let locks = ArtifactLocks::default();
        let id = ArtifactId::generate();
        assert!(ready(async { 7 }) == Some(7));
        let held = ready(locks.lock(&id)).expect("a free lock is ready");
        assert!(locks.held_here(&id));
        assert!(ready(locks.lock(&id)).is_none(), "a held lock is not");
        drop(held);
        assert!(!locks.held_here(&id));
        assert_eq!(locks.entries(), 0, "the abandoned wait leaves no entry");
        assert!(ready(locks.lock(&id)).is_some());
    }

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
