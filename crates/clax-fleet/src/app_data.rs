//! An instance's per-artifact app data stores in PostgreSQL: [`PgAppData`],
//! each artifact's store a SQLite file `app/<artifact ID>` of the
//! instance's [`PgPages`](crate::pages::PgPages), opened through `clax-pagevfs` with its small
//! limits ([`clax_pagevfs::Limits::SMALL`]).
//!
//! Each artifact has an entry of its own: the instance-wide map is locked
//! only to find or add an entry, never across the database, and a store
//! opens, runs a call and is erased under its entry's lock. At most
//! [`OPEN_STORES`] stores stay open; past it, the stores used least
//! recently that no call holds are closed. A call runs on a blocking
//! thread, as the daemon's own host's do.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clax_core::working::Clock;
use clax_core::{ArtifactId, CoreError, Result};
use clax_http::app_data_host::AppDataHost;
use clax_pagevfs::{Limits, PageStore, Registered};
use clax_store::app_data::{AppData, AppDataOp, AppDataReply};

/// The most artifact stores kept open per instance.
pub const OPEN_STORES: usize = 64;

/// An open artifact store, with what keeps its file in the VFS (dropped
/// after the store, as field order does).
struct Open {
    data: AppData,
    _registered: Registered,
}

/// An artifact's store, as its entry holds it.
enum State {
    /// Not open yet (an entry evicted is dropped with its store).
    Closed,
    Open(Box<Open>),
    /// Erased: a call that finds it answers `NotFound`, as for an artifact
    /// with no store.
    Erased,
}

struct Entry {
    state: Mutex<State>,
    /// The call count when it was last used: the least recent goes first.
    used: AtomicU64,
}

/// The app data host of one instance (see the [module docs](self)).
pub struct PgAppData(Arc<Stores>);

struct Stores {
    pages: Arc<dyn PageStore>,
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<ArtifactId, Arc<Entry>>>,
    calls: AtomicU64,
}

fn file_name(aid: &ArtifactId) -> String {
    format!("app/{aid}")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl PgAppData {
    /// The host keeping each artifact's store in `pages`: the instance's
    /// [`PgPages`](crate::pages::PgPages).
    pub fn new(pages: Arc<dyn PageStore>, clock: Arc<dyn Clock>) -> PgAppData {
        PgAppData(Arc::new(Stores {
            pages,
            clock,
            entries: Mutex::default(),
            calls: AtomicU64::new(0),
        }))
    }
}

impl Stores {
    /// Artifact `aid`'s entry, made when it has none, marked used now; the
    /// stores past [`OPEN_STORES`] that no call holds are closed.
    fn entry(&self, aid: &ArtifactId) -> Arc<Entry> {
        let now = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut entries = lock(&self.entries);
        let e = entries
            .entry(aid.clone())
            .or_insert_with(|| {
                Arc::new(Entry {
                    state: Mutex::new(State::Closed),
                    used: AtomicU64::new(now),
                })
            })
            .clone();
        e.used.store(now, Ordering::SeqCst);
        if entries.len() > OPEN_STORES {
            // Only the map holds an idle entry; one a call holds stays.
            let mut idle: Vec<(u64, ArtifactId)> = entries
                .iter()
                .filter(|(_, e)| Arc::strong_count(e) == 1)
                .map(|(a, e)| (e.used.load(Ordering::SeqCst), a.clone()))
                .collect();
            idle.sort_by_key(|(used, _)| *used);
            let excess = entries.len() - OPEN_STORES;
            for (_, a) in idle.into_iter().take(excess) {
                entries.remove(&a);
            }
        }
        e
    }

    /// Runs `op` on artifact `aid`'s store, opening it (and making it, for
    /// an import) when it is closed.
    fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let entry = self.entry(aid);
        let mut state = lock(&entry.state);
        if matches!(*state, State::Erased) {
            return Err(CoreError::NotFound);
        }
        if matches!(*state, State::Closed) {
            let name = file_name(aid);
            let create = matches!(op, AppDataOp::Import { .. });
            let exists = self
                .pages
                .size(&name)
                .map_err(|e| CoreError::Io(std::io::Error::other(e)))?
                .is_some();
            if !exists && !create {
                return Err(CoreError::NotFound);
            }
            let (conn, registered) =
                clax_pagevfs::open_with(&name, self.pages.clone(), Limits::SMALL)?.into_parts();
            *state = State::Open(Box::new(Open {
                data: AppData::open_on(conn, self.clock.clone())?,
                _registered: registered,
            }));
        }
        let State::Open(open) = &*state else {
            unreachable!("opened above");
        };
        Ok(open.data.apply(op))
    }

    /// Erases artifact `aid`'s store: once a call in progress on it ends,
    /// it is closed and its file deleted, and any call after finds none.
    /// The entry stays in the map, erased, until the file is gone, so a
    /// call that comes in meanwhile waits on it and answers `NotFound`
    /// rather than open the file being deleted.
    fn erase(&self, aid: &ArtifactId) -> Result<()> {
        let entry = self.entry(aid);
        let deleted = {
            let mut state = lock(&entry.state);
            *state = State::Erased;
            self.pages
                .delete(&file_name(aid))
                .map_err(|e| CoreError::Io(std::io::Error::other(e)))
        };
        let mut entries = lock(&self.entries);
        if entries.get(aid).is_some_and(|e| Arc::ptr_eq(e, &entry)) {
            entries.remove(aid);
        }
        deleted
    }
}

#[async_trait::async_trait]
impl AppDataHost for PgAppData {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let host = self.0.clone();
        let aid = aid.clone();
        tokio::task::spawn_blocking(move || host.call(&aid, op))
            .await
            .map_err(|_| CoreError::TaskFailed)??
            .checked()
    }

    async fn erase(&self, aid: &ArtifactId) -> Result<()> {
        let host = self.0.clone();
        let aid = aid.clone();
        tokio::task::spawn_blocking(move || host.erase(&aid))
            .await
            .map_err(|_| CoreError::TaskFailed)?
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use clax_pagevfs::{Commit, MemPages, StoreError};
    use clax_store::app_data::Exported;

    use super::*;

    /// Pages in memory whose deletion, once gated, tells the test it has
    /// begun and waits for the test to let it go on.
    #[derive(Default)]
    struct GatedPages {
        pages: MemPages,
        gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
    }

    impl PageStore for GatedPages {
        fn size(&self, file: &str) -> std::result::Result<Option<u64>, StoreError> {
            self.pages.size(file)
        }
        fn read_chunk(
            &self,
            file: &str,
            index: u64,
        ) -> std::result::Result<Option<Vec<u8>>, StoreError> {
            self.pages.read_chunk(file, index)
        }
        fn commit(&self, file: &str, commit: &Commit) -> std::result::Result<(), StoreError> {
            self.pages.commit(file, commit)
        }
        fn delete(&self, file: &str) -> std::result::Result<(), StoreError> {
            if let Some((began, go)) = lock(&self.gate).take() {
                began.send(()).unwrap();
                go.recv().unwrap();
            }
            self.pages.delete(file)
        }
    }

    /// A call that comes in while an erase deletes the store's file waits
    /// for the erase and finds no store, and leaves no file behind.
    #[test]
    fn a_call_during_an_erase_waits_and_finds_no_store() {
        let pages = Arc::new(GatedPages::default());
        let stores = Arc::new(Stores {
            pages: pages.clone(),
            clock: Arc::new(clax_core::working::SystemClock),
            entries: Mutex::default(),
            calls: AtomicU64::new(0),
        });
        let aid = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        let rows = Exported {
            artifact_id: aid.to_string(),
            ..Exported::default()
        };
        stores.call(&aid, AppDataOp::Import { rows }).unwrap();
        assert!(pages.size(&file_name(&aid)).unwrap().is_some());

        let (began_tx, began) = channel();
        let (go, go_rx) = channel();
        *lock(&pages.gate) = Some((began_tx, go_rx));
        let erase = std::thread::spawn({
            let (stores, aid) = (stores.clone(), aid.clone());
            move || stores.erase(&aid)
        });
        began.recv().unwrap();
        // The erase holds the entry, still in the map, under its lock.
        let entry = lock(&stores.entries).get(&aid).cloned().unwrap();
        let call = std::thread::spawn({
            let (stores, aid) = (stores.clone(), aid.clone());
            move || stores.call(&aid, AppDataOp::FoldMark)
        });
        // The map, the erase, this test and the call each hold the entry.
        while Arc::strong_count(&entry) < 4 {
            std::thread::yield_now();
        }
        drop(entry);
        go.send(()).unwrap();
        erase.join().unwrap().unwrap();
        assert!(matches!(call.join().unwrap(), Err(CoreError::NotFound)));
        assert!(pages.size(&file_name(&aid)).unwrap().is_none());
        assert!(lock(&stores.entries).is_empty());
        assert!(matches!(
            stores.call(&aid, AppDataOp::FoldMark),
            Err(CoreError::NotFound)
        ));
    }
}
