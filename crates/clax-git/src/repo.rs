//! The repository interface the rest of the crate works against: objects
//! and refs held by the caller (SQLite rows, Durable Object storage, a
//! remote reached over git smart HTTP, or [`crate::memory`] in tests).
//!
//! The traits use `async fn` without a `Send` bound: in a Worker the
//! futures are not `Send`, and natively they are whatever the
//! implementation's are.

#![allow(async_fn_in_trait)]

use std::collections::BTreeMap;

use crate::error::Result;
use crate::id::ObjectId;
use crate::object::{Object, ObjectKind};

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

/// One repository's objects and refs.
///
/// The object set is closed under reachability from the refs: whatever a
/// ref or a held object names, the repository holds. [`Repo::apply`]
/// writes objects before it moves refs to keep that true.
pub trait Repo {
    /// The object with `id`, or `None` when this repository does not hold it.
    async fn read_object(&self, id: &ObjectId) -> Result<Option<Object>>;

    /// Whether this repository holds `id`.
    async fn has_object(&self, id: &ObjectId) -> Result<bool> {
        Ok(self.read_object(id).await?.is_some())
    }

    /// The type of object `id`, or `None` when this repository does not
    /// hold it.
    async fn object_kind(&self, id: &ObjectId) -> Result<Option<ObjectKind>> {
        Ok(self.read_object(id).await?.map(|o| o.kind))
    }

    /// Every ref (`refs/heads/*`, `refs/tags/*`, …) to the object it names.
    async fn refs(&self) -> Result<BTreeMap<String, ObjectId>>;

    /// The ref `HEAD` names. Clax repositories answer [`MAIN`].
    fn head_ref(&self) -> &str {
        MAIN
    }

    /// Writes `objects`, then applies `updates`, each only when its ref
    /// holds `old` (compare and swap). With `atomic`, either every update
    /// applies or none does: when one is refused, the others answer
    /// [`ATOMIC_FAILED`]. Answers one status per update, in order. Objects
    /// are written even when every update is refused.
    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> Result<Vec<RefStatus>>;
}

/// A set of repositories by name, which can be created, forked and deleted.
pub trait RepoHost {
    type Repo: Repo;

    /// The repository `name`, or `None` when the host has none by that name.
    async fn open(&self, name: &str) -> Result<Option<Self::Repo>>;

    /// Creates the empty repository `name`; `invalid_argument` when it exists.
    async fn create(&self, name: &str) -> Result<()>;

    /// Creates `to` as a copy of `from` with its whole history, branches and
    /// tags. `not_found` when `from` is missing; `invalid_argument` when `to`
    /// exists.
    async fn fork(&self, from: &str, to: &str) -> Result<()>;

    /// Deletes the repository `name`; nothing when it is missing.
    async fn delete(&self, name: &str) -> Result<()>;
}
