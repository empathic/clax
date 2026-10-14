//! The stand-in's repositories as a clax-git [`RepoHost`] over its own
//! database ([`super::db`]): what `RepoDO` serves over smart HTTP and its
//! internal API, and what native tests run the content store on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use clax_git::{GitError, Object, ObjectId, ObjectKind, RefStatus, RefUpdate, Repo, RepoHost};
use rusqlite::Connection;

use super::db;

/// What holds the stand-in's connection: a plain [`Connection`] natively,
/// a `clax_do_vfs::Database` in a Durable Object.
pub trait AsConn {
    fn conn(&self) -> &Connection;
}

impl AsConn for Connection {
    fn conn(&self) -> &Connection {
        self
    }
}

impl AsConn for clax_do_vfs::Database {
    fn conn(&self) -> &Connection {
        self
    }
}

/// The stand-in's repositories over one connection. Cloning shares it.
pub struct LocalHost<C> {
    db: Arc<Mutex<C>>,
}

impl<C> Clone for LocalHost<C> {
    fn clone(&self) -> Self {
        LocalHost {
            db: self.db.clone(),
        }
    }
}

impl<C: AsConn> LocalHost<C> {
    /// The repositories in `conn`, creating the tables when absent.
    pub fn new(conn: C) -> clax_git::Result<LocalHost<C>> {
        db::init(conn.conn())?;
        Ok(LocalHost {
            db: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, C> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the connection. `f` must not await: the lock is held.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(self.lock().conn())
    }

    /// The repository `name`, when it exists.
    pub fn repo(&self, name: &str) -> clax_git::Result<Option<LocalRepo<C>>> {
        Ok(self.with(|c| db::exists(c, name))?.then(|| LocalRepo {
            host: self.clone(),
            name: name.to_string(),
        }))
    }
}

/// One repository of a [`LocalHost`].
pub struct LocalRepo<C> {
    host: LocalHost<C>,
    name: String,
}

impl<C> LocalRepo<C> {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<C: AsConn + clax_git::MaybeSend> Repo for LocalRepo<C> {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        self.host.with(|c| db::read_object(c, &self.name, id))
    }

    async fn has_object(&self, id: &ObjectId) -> clax_git::Result<bool> {
        Ok(self.object_kind(id).await?.is_some())
    }

    async fn object_kind(&self, id: &ObjectId) -> clax_git::Result<Option<ObjectKind>> {
        self.host.with(|c| db::object_kind(c, &self.name, id))
    }

    async fn refs(&self) -> clax_git::Result<BTreeMap<String, ObjectId>> {
        self.host.with(|c| db::refs(c, &self.name))
    }

    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<RefStatus>> {
        self.host
            .with(|c| db::apply(c, &self.name, &objects, updates, atomic))
    }
}

impl<C: AsConn + clax_git::MaybeSend> RepoHost for LocalHost<C> {
    type Repo = LocalRepo<C>;

    async fn open(&self, name: &str) -> clax_git::Result<Option<LocalRepo<C>>> {
        self.repo(name)
    }

    async fn create(&self, name: &str) -> clax_git::Result<()> {
        self.with(|c| db::create(c, name))
    }

    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        self.with(|c| db::fork(c, from, to))
    }

    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        self.with(|c| db::delete(c, name, super::now_ms()))
    }

    fn can_prune(&self) -> bool {
        true
    }

    async fn prune(&self, name: &str) -> clax_git::Result<()> {
        self.with(|c| db::prune(c, name))
    }
}

/// `GitError` for a malformed internal request or answer.
pub fn malformed(what: &str) -> GitError {
    GitError::RepoUnavailable(format!("A malformed repository store message: {what}"))
}
