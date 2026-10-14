//! The repository interface the rest of the crate works against: objects
//! and refs held by the caller (SQLite rows, Durable Object storage, a
//! remote reached over git smart HTTP, or [`crate::memory`] in tests).
//!
//! The traits' futures are `Send` natively ([`MaybeSend`]), so a caller
//! that needs `Send` futures (an async trait object shared across threads,
//! as clax-core's content store is) can run them; on `wasm32` they need not
//! be, since a Worker's JavaScript futures are `!Send` and run on one
//! thread. An implementation may still write its methods as `async fn`.

use std::collections::BTreeMap;
use std::future::Future;

use crate::error::Result;
use crate::id::ObjectId;
use crate::object::{Commit, Object, ObjectKind, Signature, parse_trailers};

/// `Send` natively; nothing on `wasm32`, where a Worker's futures are
/// single-threaded.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}
/// `Send` natively; nothing on `wasm32`, where a Worker's futures are
/// single-threaded.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Send + Sync` natively; nothing on `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}
/// `Send + Sync` natively; nothing on `wasm32`.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// The branch every Clax version is a commit on.
pub const MAIN: &str = "refs/heads/main";

/// A ref update: `name` moves from `old` to `new`. `old` is
/// [`ObjectId::ZERO`] when the ref must not exist; `new` is
/// [`ObjectId::ZERO`] to delete it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefUpdate {
    pub name: String,
    pub old: ObjectId,
    pub new: ObjectId,
}

impl RefUpdate {
    pub fn new(name: impl Into<String>, old: Option<ObjectId>, new: Option<ObjectId>) -> RefUpdate {
        RefUpdate {
            name: name.into(),
            old: old.unwrap_or(ObjectId::ZERO),
            new: new.unwrap_or(ObjectId::ZERO),
        }
    }

    pub fn is_delete(&self) -> bool {
        self.new.is_zero()
    }
}

/// The outcome of one ref update: `Ok`, or the reason it was refused (as
/// git words it in `report-status`, such as `stale info`).
pub type RefStatus = std::result::Result<(), String>;

/// The reason an atomic update gives the refs that did not fail themselves.
pub const ATOMIC_FAILED: &str = "atomic push failed";

/// What Clax reads of a commit: its tree, parents, people and message.
/// Every field but the ID comes from the commit's own fields, so a
/// repository whose API answers commits as metadata (Cloudflare Artifacts)
/// gives it without the raw object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    pub commit: ObjectId,
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    /// The whole message, trailers included, with one trailing newline removed.
    pub message: String,
    /// The message's trailers, in order.
    pub trailers: Vec<(String, String)>,
}

impl CommitMeta {
    pub fn from_commit(id: ObjectId, c: Commit) -> CommitMeta {
        let message = c
            .message
            .strip_suffix('\n')
            .unwrap_or(&c.message)
            .to_string();
        let trailers = parse_trailers(&message);
        CommitMeta {
            commit: id,
            tree: c.tree,
            parents: c.parents,
            author: c.author,
            committer: c.committer,
            message,
            trailers,
        }
    }

    /// The value of the first trailer `key`.
    pub fn trailer(&self, key: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// The metadata of `object` (whose ID is `id`) when it is a commit; `None`
/// for any other type.
///
/// # Errors
/// `repo_unavailable` for a commit that does not parse.
pub fn commit_meta_of(id: ObjectId, object: &Object) -> Result<Option<CommitMeta>> {
    if object.kind != ObjectKind::Commit {
        return Ok(None);
    }
    Ok(Some(CommitMeta::from_commit(
        id,
        Commit::parse(&object.data)?,
    )))
}

/// One repository's objects and refs.
///
/// The object set is closed under reachability from the refs: whatever a
/// ref or a held object names, the repository holds. [`Repo::apply`]
/// writes objects before it moves refs to keep that true.
pub trait Repo: MaybeSendSync {
    /// The object with `id`, or `None` when this repository does not hold it.
    fn read_object(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = Result<Option<Object>>> + MaybeSend;

    /// The commit `id` as metadata, or `None` when this repository holds no
    /// commit `id`. The default parses [`Repo::read_object`]; a repository
    /// whose API answers commits as metadata rather than raw objects
    /// overrides it. The crate reads commits through this for versions,
    /// trees and logs, and raw commits only where git's own encoding is the
    /// point: upload-pack's packs and receive-pack's checks.
    fn read_commit_meta(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = Result<Option<CommitMeta>>> + MaybeSend {
        async move {
            match self.read_object(id).await? {
                Some(o) => commit_meta_of(*id, &o),
                None => Ok(None),
            }
        }
    }

    /// Whether this repository holds `id`.
    fn has_object(&self, id: &ObjectId) -> impl Future<Output = Result<bool>> + MaybeSend {
        async move { Ok(self.read_object(id).await?.is_some()) }
    }

    /// The type of object `id`, or `None` when this repository does not
    /// hold it.
    fn object_kind(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = Result<Option<ObjectKind>>> + MaybeSend {
        async move { Ok(self.read_object(id).await?.map(|o| o.kind)) }
    }

    /// Every ref (`refs/heads/*`, `refs/tags/*`, …) to the object it names.
    fn refs(&self) -> impl Future<Output = Result<BTreeMap<String, ObjectId>>> + MaybeSend;

    /// The ref `HEAD` names. Clax repositories answer [`MAIN`].
    fn head_ref(&self) -> &str {
        MAIN
    }

    /// Writes `objects`, then applies `updates`, each only when its ref
    /// holds `old` (compare and swap). With `atomic`, either every update
    /// applies or none does: when one is refused, the others answer
    /// [`ATOMIC_FAILED`]. Answers one status per update, in order. Objects
    /// are written even when every update is refused.
    fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> impl Future<Output = Result<Vec<RefStatus>>> + MaybeSend;
}

/// A set of repositories by name, which can be created, forked and deleted.
pub trait RepoHost: MaybeSendSync {
    type Repo: Repo;

    /// The repository `name`, or `None` when the host has none by that name.
    fn open(&self, name: &str) -> impl Future<Output = Result<Option<Self::Repo>>> + MaybeSend;

    /// Creates the empty repository `name`; `invalid_argument` when it exists.
    fn create(&self, name: &str) -> impl Future<Output = Result<()>> + MaybeSend;

    /// Creates `to` as a copy of `from` with its whole history, branches and
    /// tags. `not_found` when `from` is missing; `invalid_argument` when `to`
    /// exists.
    fn fork(&self, from: &str, to: &str) -> impl Future<Output = Result<()>> + MaybeSend;

    /// Deletes the repository `name`; nothing when it is missing.
    fn delete(&self, name: &str) -> impl Future<Output = Result<()>> + MaybeSend;
}
