//! Where each artifact's app data is reached (spec
//! 2026-10-21-clax-run-site-origins "Each artifact's data in a store of its
//! own"): an [`AppDataHost`] runs an [`AppDataOp`] on one artifact's store.
//! The daemon's is [`LocalAppData`], a SQLite file per artifact in the
//! home; the Worker's calls the artifact's Durable Object. Every
//! deployment wraps its host in [`MovingAppData`], which moves an
//! artifact's rows out of the instance before its store's first call and
//! hands the store the viewer folds it has not applied.
//!
//! The instance decides everything a call needs before it is made (the
//! caller, the artifact's declared rules, a page's authors, the reader's
//! view): see the operations' fields.

use clax_core::content::ArtifactLocks;
use clax_core::{ArtifactId, CoreError, Result};
use clax_store::Store;
use clax_store::app_data::{AppDataOp, AppDataReply};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Runs operations on artifacts' stores. A reply is never
/// [`AppDataReply::Error`]: a failed operation is the call's error.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait AppDataHost: Send + Sync {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply>;
}

/// Each artifact's store as a SQLite file in the home
/// ([`clax_core::Home::app_data_path`]), opened for each call on a
/// blocking thread, keeping time by `clock` (the instance store's).
#[cfg(not(target_arch = "wasm32"))]
pub struct LocalAppData {
    home: clax_core::Home,
    clock: Arc<dyn clax_core::working::Clock>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalAppData {
    pub fn new(home: clax_core::Home, clock: Arc<dyn clax_core::working::Clock>) -> LocalAppData {
        LocalAppData { home, clock }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl AppDataHost for LocalAppData {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let path = self.home.app_data_path(aid);
        let clock = self.clock.clone();
        clax_rt::spawn_blocking(move || {
            clax_store::app_data::AppData::open(&path, clock).map(|ad| ad.apply(op))
        })
        .await
        .map_err(|_| CoreError::TaskFailed)??
        .ok()
    }
}

/// A host with no artifact stores: every call fails. What a host that
/// brings its own ([`crate::state::AppState::app_data`]) starts with.
pub struct NoAppData;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl AppDataHost for NoAppData {
    async fn call(&self, _aid: &ArtifactId, _op: AppDataOp) -> Result<AppDataReply> {
        Err(CoreError::Io(std::io::Error::other(
            "this host keeps no artifact stores",
        )))
    }
}

/// `inner`, with each artifact's rows moved from the instance (`store`)
/// before its store's first call, and the viewer folds its store has not
/// applied handed to it before any call.
///
/// The move exports the artifact's rows, sends [`AppDataOp::Import`],
/// then marks the artifact moved and forgets its rows in one instance
/// transaction ([`Store::forget_app_data`]), under a per-artifact lock so
/// two first calls move once. A move that stops between the import and the
/// forget imports the same rows again next time, which the import treats
/// as already present.
pub struct MovingAppData<H: AppDataHost> {
    inner: H,
    store: Arc<Store>,
    moving: ArtifactLocks,
    /// Each artifact store's fold mark, as it last answered.
    marks: Mutex<HashMap<String, i64>>,
}

impl<H: AppDataHost> MovingAppData<H> {
    pub fn new(inner: H, store: Arc<Store>) -> MovingAppData<H> {
        MovingAppData {
            inner,
            store,
            moving: ArtifactLocks::default(),
            marks: Mutex::new(HashMap::new()),
        }
    }

    async fn state(&self, aid: &ArtifactId) -> Result<clax_store::app_data::AppDataState> {
        let aid = aid.clone();
        self.store.call(move |st| st.app_data_state(&aid)).await
    }

    fn mark(&self, aid: &ArtifactId) -> Option<i64> {
        self.marks.lock().unwrap().get(aid.as_str()).copied()
    }

    fn set_mark(&self, aid: &ArtifactId, mark: i64) {
        self.marks
            .lock()
            .unwrap()
            .insert(aid.as_str().to_string(), mark);
    }

    /// Moves `aid`'s rows to its store, unless another call did first.
    async fn move_rows(&self, aid: &ArtifactId) -> Result<()> {
        let _held = self.moving.lock(aid).await;
        if self.state(aid).await?.moved {
            return Ok(());
        }
        let id = aid.clone();
        let rows = self.store.call(move |st| st.export_app_data(&id)).await?;
        let folded = rows.fold_seq;
        self.inner
            .call(aid, AppDataOp::Import { rows })
            .await?
            .into_done()?;
        let id = aid.clone();
        self.store.call(move |st| st.forget_app_data(&id)).await?;
        // The store's mark is at least the rows' (it may be past them).
        if self.mark(aid).is_none_or(|m| m < folded) {
            self.set_mark(aid, folded);
        }
        Ok(())
    }

    /// Hands `aid`'s store the folds past its mark, when the log's newest
    /// (`newest`) is.
    async fn catch_up(&self, aid: &ArtifactId, newest: i64) -> Result<()> {
        if newest == 0 || self.mark(aid).is_some_and(|m| m >= newest) {
            return Ok(());
        }
        let mark = match self.mark(aid) {
            Some(m) => m,
            None => self
                .inner
                .call(aid, AppDataOp::FoldMark)
                .await?
                .into_folded()?,
        };
        if mark < newest {
            let folds = self
                .store
                .call(move |st| st.app_data_folds_after(mark))
                .await?;
            let mark = self
                .inner
                .call(aid, AppDataOp::Fold { folds })
                .await?
                .into_folded()?;
            self.set_mark(aid, mark);
        } else {
            self.set_mark(aid, mark);
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<H: AppDataHost> AppDataHost for MovingAppData<H> {
    /// `NotFound` for an artifact that is missing or deleted, before its
    /// store is reached.
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let state = self.state(aid).await?;
        if !state.moved {
            self.move_rows(aid).await?;
        }
        self.catch_up(aid, state.folds).await?;
        self.inner.call(aid, op).await
    }
}

/// The host a deployment starts with: natively each artifact's file in
/// `home`, moved into from `store`; on the Worker none, until its own
/// replaces it.
pub fn default_host(store: &Arc<Store>, home: &clax_core::Home) -> Arc<dyn AppDataHost> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Arc::new(MovingAppData::new(
            LocalAppData::new(home.clone(), store.clock()),
            store.clone(),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = home;
        Arc::new(MovingAppData::new(NoAppData, store.clone()))
    }
}

impl crate::state::AppState {
    /// Runs `op` on artifact `aid`'s store ([`AppState::app_data`](crate::state::AppState::app_data)),
    /// then `then` on its outcome, on a task of its own: once the call has
    /// started, a request that gives up (the timeout layer) still sees it
    /// through, so the follow-up a committed change needs (its events)
    /// belongs in `then`, as in [`crate::state::AppState::store_call`]'s job.
    pub async fn app_data_call<T, F>(
        &self,
        aid: &ArtifactId,
        op: AppDataOp,
        then: F,
    ) -> std::result::Result<T, crate::error::ApiError>
    where
        T: Send + 'static,
        F: FnOnce(Result<AppDataReply>) -> Result<T> + Send + 'static,
    {
        let host = self.app_data.clone();
        let aid = aid.clone();
        clax_rt::spawn(async move { then(host.call(&aid, op).await) })
            .await
            .map_err(|_| crate::error::ApiError::from(CoreError::TaskFailed))?
            .map_err(crate::error::ApiError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::audit::AuditCtx;
    use clax_core::db::{Caller, Level};
    use clax_store::app_data::{WireCaller, WriteCtx};
    use clax_store::docs::Pin;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Arc<Store>, ArtifactId) {
        let dir = tempfile::tempdir().unwrap();
        let st = Arc::new(Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap());
        let req = serde_json::from_value(json!({
            "title": "Tracker",
            "capabilities": {"db": {}},
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let (a, _) = st
            .create_artifact(
                &AuditCtx::DAEMON,
                clax_core::publish::validate(req).unwrap(),
                None,
            )
            .unwrap();
        (dir, st, ArtifactId::parse(&a.id).unwrap())
    }

    fn set(path: &str) -> AppDataOp {
        AppDataOp::Set {
            ctx: WriteCtx::of(&AuditCtx::DAEMON),
            caps: json!({"db": {}}),
            path: path.into(),
            data: json!({}),
            pin: Pin::default(),
            caller: WireCaller::from(&Caller::new(Level::Owner, None)),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn first_calls_at_once_move_once_and_each_lands() {
        let (_d, st, aid) = store();
        let host = Arc::new(MovingAppData::new(
            LocalAppData::new(st.home().clone(), st.clock()),
            st.clone(),
        ));
        let calls: Vec<_> = ["t/a", "t/b", "t/c"]
            .into_iter()
            .map(|p| {
                let (host, aid) = (host.clone(), aid.clone());
                tokio::spawn(async move { host.call(&aid, set(p)).await })
            })
            .collect();
        let mut versions = Vec::new();
        for c in calls {
            let w = c.await.unwrap().unwrap().into_written().unwrap();
            versions.push(w.doc.unwrap().version);
        }
        versions.sort_unstable();
        assert_eq!(versions, [1, 2, 3]);
        assert!(st.app_data_state(&aid).unwrap().moved);
    }

    #[tokio::test]
    async fn a_deleted_artifact_is_not_found_before_its_store_is_made() {
        let (_d, st, aid) = store();
        let host = MovingAppData::new(LocalAppData::new(st.home().clone(), st.clock()), st.clone());
        st.delete_artifact(&AuditCtx::DAEMON, &aid).unwrap();
        assert!(matches!(
            host.call(&aid, set("t/a")).await,
            Err(CoreError::NotFound)
        ));
        assert!(!st.home().app_data_path(&aid).exists());
    }
}
