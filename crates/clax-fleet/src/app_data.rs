//! An instance's per-artifact app data stores in PostgreSQL: [`PgAppData`],
//! each artifact's store a SQLite file `app/<artifact ID>` of the
//! instance's [`PgPages`], opened through `clax-pagevfs`.
//!
//! Open stores are kept, at most [`OPEN_STORES`], each behind a lock of its
//! own (one connection per file, one call at a time on it); the store
//! opened least recently is closed to make room. A call runs on a blocking
//! thread, as the daemon's own host's do.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clax_core::working::Clock;
use clax_core::{ArtifactId, CoreError, Result};
use clax_http::app_data_host::AppDataHost;
use clax_pagevfs::{PageStore, Registered};
use clax_store::app_data::{AppData, AppDataOp, AppDataReply};

use crate::pages::PgPages;

/// The most artifact stores kept open per instance.
pub const OPEN_STORES: usize = 64;

/// An open artifact store, with what keeps its file in the VFS (dropped
/// after the store, as field order does).
struct Open {
    data: AppData,
    _registered: Registered,
}

/// The app data host of one instance (see the [module docs](self)).
pub struct PgAppData(Arc<Stores>);

/// What a call's blocking thread takes of the host.
struct Stores {
    pages: Arc<PgPages>,
    clock: Arc<dyn Clock>,
    open: Mutex<OpenStores>,
}

#[derive(Default)]
struct OpenStores {
    stores: HashMap<ArtifactId, Arc<Mutex<Open>>>,
    /// Opens so far, and the count at each store's open: the oldest goes.
    opens: u64,
    opened_at: HashMap<ArtifactId, u64>,
}

fn file_name(aid: &ArtifactId) -> String {
    format!("app/{aid}")
}

impl PgAppData {
    pub fn new(pages: Arc<PgPages>, clock: Arc<dyn Clock>) -> PgAppData {
        PgAppData(Arc::new(Stores {
            pages,
            clock,
            open: Mutex::default(),
        }))
    }
}

impl Stores {
    fn open_stores(&self) -> std::sync::MutexGuard<'_, OpenStores> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Artifact `aid`'s store, opened (and made, when `create`) if it is not
    /// open: `NotFound` for one the instance has none of, without `create`.
    fn store(&self, aid: &ArtifactId, create: bool) -> Result<Arc<Mutex<Open>>> {
        // Held while a store opens, so two first calls open it once (a
        // second open would replace the first's file in the VFS).
        let mut o = self.open_stores();
        if let Some(s) = o.stores.get(aid) {
            return Ok(s.clone());
        }
        let name = file_name(aid);
        let exists = self
            .pages
            .size(&name)
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?
            .is_some();
        if !exists && !create {
            return Err(CoreError::NotFound);
        }
        let pages: Arc<dyn PageStore> = self.pages.clone();
        let (conn, registered) = clax_pagevfs::open(&name, pages)?.into_parts();
        let data = AppData::open_on(conn, self.clock.clone())?;
        let open = Arc::new(Mutex::new(Open {
            data,
            _registered: registered,
        }));
        o.opens += 1;
        let at = o.opens;
        o.opened_at.insert(aid.clone(), at);
        o.stores.insert(aid.clone(), open.clone());
        // Only a store no call holds is closed: closing one in use would
        // have its next open replace the file under that call.
        while o.stores.len() > OPEN_STORES {
            let idle = o
                .opened_at
                .iter()
                .filter(|(a, _)| o.stores.get(*a).is_some_and(|s| Arc::strong_count(s) == 1))
                .min_by_key(|(_, at)| **at)
                .map(|(a, _)| a.clone());
            let Some(oldest) = idle else {
                break;
            };
            o.opened_at.remove(&oldest);
            o.stores.remove(&oldest);
        }
        Ok(open)
    }

    /// Closes artifact `aid`'s store, once a call in progress on it ends,
    /// and deletes its file; no call opens it meanwhile.
    fn erase(&self, aid: &ArtifactId) -> Result<()> {
        let mut o = self.open_stores();
        o.opened_at.remove(aid);
        let held = o.stores.remove(aid);
        let busy = held
            .as_ref()
            .map(|h| h.lock().unwrap_or_else(|e| e.into_inner()));
        let deleted = self
            .pages
            .delete(&file_name(aid))
            .map_err(|e| CoreError::Io(std::io::Error::other(e)));
        drop(busy);
        drop(held);
        drop(o);
        deleted
    }
}

#[async_trait::async_trait]
impl AppDataHost for PgAppData {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let host = self.0.clone();
        let aid = aid.clone();
        tokio::task::spawn_blocking(move || {
            let create = matches!(op, AppDataOp::Import { .. });
            let store = host.store(&aid, create)?;
            let reply = store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .data
                .apply(op);
            Ok::<_, CoreError>(reply)
        })
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
