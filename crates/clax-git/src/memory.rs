//! An in-memory [`RepoHost`]: objects are content-addressed and shared by
//! every repository of the host (a fork copies refs and the set of objects
//! held, not the objects), and each repository answers only the objects it
//! holds. For tests and a local stand-in store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::{GitError, Result};
use crate::id::ObjectId;
use crate::object::{Object, ObjectKind};
use crate::repo::{ATOMIC_FAILED, RefStatus, RefUpdate, Repo, RepoHost};

#[derive(Default)]
struct RepoState {
    refs: BTreeMap<String, ObjectId>,
    held: HashSet<ObjectId>,
}

#[derive(Default)]
struct Inner {
    objects: HashMap<ObjectId, Arc<Object>>,
    repos: HashMap<String, RepoState>,
}

/// Repositories in memory. Cloning shares them.
#[derive(Clone, Default)]
pub struct MemoryHost {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryHost {
    pub fn new() -> MemoryHost {
        MemoryHost::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The names of the repositories, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().repos.keys().cloned().collect();
        v.sort();
        v
    }

    /// The number of distinct objects stored across every repository.
    pub fn object_count(&self) -> usize {
        self.lock().objects.len()
    }

    /// The repository `name`, when it exists, without going through the
    /// async interface.
    pub fn repo(&self, name: &str) -> Option<MemoryRepo> {
        self.lock().repos.contains_key(name).then(|| MemoryRepo {
            host: self.clone(),
            name: name.to_string(),
        })
    }
}

/// One repository of a [`MemoryHost`].
#[derive(Clone)]
pub struct MemoryRepo {
    host: MemoryHost,
    name: String,
}

impl MemoryRepo {
    pub fn name(&self) -> &str {
        &self.name
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> Result<T>) -> Result<T> {
        let mut inner = self.host.lock();
        if !inner.repos.contains_key(&self.name) {
            return Err(GitError::not_found(format!("No repository {}", self.name)));
        }
        f(&mut inner)
    }

    /// Sets `name` to `id` (or deletes it for `None`) unconditionally, as
    /// `git update-ref` would; for tests that race a writer.
    pub fn force_ref(&self, name: &str, id: Option<ObjectId>) -> Result<()> {
        self.with(|inner| {
            let r = inner.repos.get_mut(&self.name).expect("checked");
            match id {
                Some(id) => {
                    if !r.held.contains(&id) {
                        return Err(GitError::invalid(format!(
                            "No object {id} in {}",
                            self.name
                        )));
                    }
                    r.refs.insert(name.to_string(), id);
                }
                None => {
                    r.refs.remove(name);
                }
            }
            Ok(())
        })
    }
}

impl Repo for MemoryRepo {
    async fn read_object(&self, id: &ObjectId) -> Result<Option<Object>> {
        self.with(|inner| {
            let held = inner.repos[&self.name].held.contains(id);
            Ok(if held {
                inner.objects.get(id).map(|o| (**o).clone())
            } else {
                None
            })
        })
    }

    async fn has_object(&self, id: &ObjectId) -> Result<bool> {
        self.with(|inner| Ok(inner.repos[&self.name].held.contains(id)))
    }

    async fn object_kind(&self, id: &ObjectId) -> Result<Option<ObjectKind>> {
        self.with(|inner| {
            let held = inner.repos[&self.name].held.contains(id);
            Ok(held
                .then(|| inner.objects.get(id).map(|o| o.kind))
                .flatten())
        })
    }

    async fn refs(&self) -> Result<BTreeMap<String, ObjectId>> {
        self.with(|inner| Ok(inner.repos[&self.name].refs.clone()))
    }

    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> Result<Vec<RefStatus>> {
        let with_ids: Vec<(ObjectId, Object)> = objects.into_iter().map(|o| (o.id(), o)).collect();
        self.with(|inner| {
            for (id, o) in with_ids {
                inner.objects.entry(id).or_insert_with(|| Arc::new(o));
                inner
                    .repos
                    .get_mut(&self.name)
                    .expect("checked")
                    .held
                    .insert(id);
            }
            let r = inner.repos.get_mut(&self.name).expect("checked");
            let check = |u: &RefUpdate, refs: &BTreeMap<String, ObjectId>| -> RefStatus {
                let at = refs.get(&u.name).copied().unwrap_or(ObjectId::ZERO);
                if at != u.old {
                    return Err("stale info".to_string());
                }
                if !u.new.is_zero() && !r.held.contains(&u.new) {
                    return Err("missing necessary objects".to_string());
                }
                Ok(())
            };
            let mut statuses: Vec<RefStatus> = updates.iter().map(|u| check(u, &r.refs)).collect();
            if atomic && statuses.iter().any(|s| s.is_err()) {
                for s in &mut statuses {
                    if s.is_ok() {
                        *s = Err(ATOMIC_FAILED.to_string());
                    }
                }
                return Ok(statuses);
            }
            for (u, s) in updates.iter().zip(&mut statuses) {
                if s.is_err() {
                    continue;
                }
                // An earlier update in the same call may have moved this ref.
                let at = r.refs.get(&u.name).copied().unwrap_or(ObjectId::ZERO);
                if at != u.old {
                    *s = Err("stale info".to_string());
                    continue;
                }
                if u.new.is_zero() {
                    r.refs.remove(&u.name);
                } else {
                    r.refs.insert(u.name.clone(), u.new);
                }
            }
            Ok(statuses)
        })
    }
}

impl RepoHost for MemoryHost {
    type Repo = MemoryRepo;

    async fn open(&self, name: &str) -> Result<Option<MemoryRepo>> {
        Ok(self.repo(name))
    }

    async fn create(&self, name: &str) -> Result<()> {
        let mut inner = self.lock();
        if inner.repos.contains_key(name) {
            return Err(GitError::invalid(format!(
                "Repository {name} already exists"
            )));
        }
        inner.repos.insert(name.to_string(), RepoState::default());
        Ok(())
    }

    async fn fork(&self, from: &str, to: &str) -> Result<()> {
        let mut inner = self.lock();
        if inner.repos.contains_key(to) {
            return Err(GitError::invalid(format!("Repository {to} already exists")));
        }
        let src = inner
            .repos
            .get(from)
            .ok_or_else(|| GitError::not_found(format!("No repository {from}")))?;
        let copy = RepoState {
            refs: src.refs.clone(),
            held: src.held.clone(),
        };
        inner.repos.insert(to.to_string(), copy);
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        self.lock().repos.remove(name);
        Ok(())
    }
}
