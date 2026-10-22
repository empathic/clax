//! Where each artifact's app data is reached (spec
//! 2026-10-21-clax-run-site-origins "Each artifact's data in a store of its
//! own"): an [`AppDataHost`] runs an [`AppDataOp`] on one artifact's store.
//! The daemon's is [`LocalAppData`], a SQLite file per artifact in the
//! home; the Worker's calls the artifact's Durable Object. Every
//! deployment wraps its host in [`MovingAppData`], which moves an
//! artifact's rows out of the instance before its store's first call,
//! hands the store the viewer folds it has not applied, and copies the
//! audit records each change makes there into the instance's history.
//!
//! The instance decides everything a call needs before it is made (the
//! caller, the artifact's declared rules, a page's authors, the reader's
//! view): see the operations' fields.

use clax_core::content::ArtifactLocks;
use clax_core::{ArtifactId, CoreError, Result};
use clax_store::Store;
use clax_store::app_data::{AppDataOp, AppDataReply};
use clax_store::audit::AuditRow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Runs operations on artifacts' stores. A reply is never
/// [`AppDataReply::Error`]: a failed operation is the call's error. Only
/// [`AppDataOp::Import`] makes a store; any other operation on an artifact
/// that has none is `NotFound`.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait AppDataHost: Send + Sync {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply>;

    /// Removes artifact `aid`'s store, when it has one: for a store an
    /// import made for an artifact deleted meanwhile.
    async fn erase(&self, _aid: &ArtifactId) -> Result<()> {
        Ok(())
    }
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
        use clax_store::app_data::AppData;
        let path = self.home.app_data_path(aid);
        let clock = self.clock.clone();
        clax_rt::spawn_blocking(move || {
            let ad = match op {
                AppDataOp::Import { .. } => AppData::open(&path, clock)?,
                _ => AppData::open_existing(&path, clock)?,
            };
            Ok::<_, CoreError>(ad.apply(op))
        })
        .await
        .map_err(|_| CoreError::TaskFailed)??
        .checked()
    }

    async fn erase(&self, aid: &ArtifactId) -> Result<()> {
        let path = self.home.app_data_path(aid);
        clax_rt::spawn_blocking(move || -> Result<()> {
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.clone().into_os_string();
                p.push(suffix);
                match std::fs::remove_file(&p) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| CoreError::TaskFailed)?
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
/// before its store's first call, the viewer folds its store has not
/// applied handed to it before any call, and the audit records each change
/// makes there copied into the instance's history
/// ([`Store::mirror_app_data_audit`]).
///
/// The move exports the artifact's rows, sends [`AppDataOp::Import`],
/// then marks the artifact moved and forgets its rows in one instance
/// transaction ([`Store::forget_app_data`]), under a per-artifact lock so
/// two first calls move once. A move that stops between the import and the
/// forget imports again next time, which a store that has its rows already
/// ignores; the folds made meanwhile reach it past its own mark. When the
/// artifact is deleted while it moves, the store the import made is erased
/// and the call is `NotFound`.
///
/// The copies of a store's audit records keep up as each change answers;
/// a copy lost (the process stopped between the change and its copy) is
/// made up at the artifact's first call after a start, and at the next
/// change, which finds the copies behind ([`AppDataOp::AuditSince`]).
pub struct MovingAppData<H: AppDataHost> {
    inner: H,
    store: Arc<Store>,
    moving: ArtifactLocks,
    copying: ArtifactLocks,
    /// Each artifact store's fold mark, as it last answered.
    marks: Mutex<HashMap<String, i64>>,
    /// The artifacts whose audit copies were made up since the start.
    caught_up: Mutex<HashSet<String>>,
}

impl<H: AppDataHost> MovingAppData<H> {
    pub fn new(inner: H, store: Arc<Store>) -> MovingAppData<H> {
        MovingAppData {
            inner,
            store,
            moving: ArtifactLocks::default(),
            copying: ArtifactLocks::default(),
            marks: Mutex::new(HashMap::new()),
            caught_up: Mutex::new(HashSet::new()),
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

    /// `op` on `aid`'s store, its audit records copied to the instance.
    async fn send(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
        let (reply, audit) = self.inner.call(aid, op).await?.audited();
        if !audit.is_empty() {
            // The change committed: a copy that fails is made up later,
            // never the caller's failure.
            if let Err(e) = self.copy_audit(aid, audit).await {
                tracing::warn!(artifact = aid.as_str(), error = %e, "an artifact store's audit records were not copied");
            }
        }
        Ok(reply)
    }

    /// Copies `rows` of `aid`'s store into the instance's history, and,
    /// when they do not follow on from what it holds, every record after.
    async fn copy_audit(&self, aid: &ArtifactId, rows: Vec<AuditRow>) -> Result<()> {
        let _held = self.copying.lock(aid).await;
        let id = aid.clone();
        let behind = self
            .store
            .call(move |st| st.mirror_app_data_audit(&id, &rows, false))
            .await?;
        if let Some(seq) = behind {
            self.copy_after(aid, seq).await?;
        }
        Ok(())
    }

    /// Copies every record of `aid`'s store after `seq`; the caller holds
    /// the copying lock.
    async fn copy_after(&self, aid: &ArtifactId, seq: i64) -> Result<()> {
        let rows = self
            .inner
            .call(aid, AppDataOp::AuditSince { seq })
            .await?
            .into_audit()?;
        let id = aid.clone();
        self.store
            .call(move |st| st.mirror_app_data_audit(&id, &rows, true))
            .await?;
        Ok(())
    }

    /// Moves `aid`'s rows to its store, unless another call did first.
    async fn move_rows(&self, aid: &ArtifactId) -> Result<()> {
        let _held = self.moving.lock(aid).await;
        if self.state(aid).await?.moved {
            return Ok(());
        }
        let id = aid.clone();
        let rows = self.store.call(move |st| st.export_app_data(&id)).await?;
        self.inner
            .call(aid, AppDataOp::Import { rows })
            .await?
            .into_done()?;
        let id = aid.clone();
        self.store.call(move |st| st.forget_app_data(&id)).await?;
        // Deleted while it moved: the store the import made goes too.
        if let Err(CoreError::NotFound) = self.state(aid).await {
            self.inner.erase(aid).await?;
            return Err(CoreError::NotFound);
        }
        Ok(())
    }

    /// Hands `aid`'s store the folds past its own mark, when the log's
    /// newest (`newest`) is.
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
                .send(aid, AppDataOp::Fold { folds })
                .await?
                .into_folded()?;
            self.set_mark(aid, mark);
        } else {
            self.set_mark(aid, mark);
        }
        Ok(())
    }

    /// Makes up `aid`'s audit copies once after a start, from where the
    /// instance's stand (`audit_seq`).
    async fn copies_once(&self, aid: &ArtifactId, audit_seq: i64) -> Result<()> {
        if self.caught_up.lock().unwrap().contains(aid.as_str()) {
            return Ok(());
        }
        {
            let _held = self.copying.lock(aid).await;
            self.copy_after(aid, audit_seq).await?;
        }
        self.caught_up
            .lock()
            .unwrap()
            .insert(aid.as_str().to_string());
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
        self.copies_once(aid, state.audit_seq).await?;
        self.catch_up(aid, state.folds).await?;
        self.send(aid, op).await
    }

    async fn erase(&self, aid: &ArtifactId) -> Result<()> {
        self.inner.erase(aid).await
    }
}

/// Moves every artifact whose rows the instance still holds
/// ([`Store::app_data_pending`]) to its store through `host`, one at a
/// time, logging each that fails; returns how many moved. The local daemon
/// runs it once as it starts, so the instance's tables empty without
/// waiting for each artifact's first call; the Worker moves each on its
/// first call.
pub async fn move_pending(store: &Arc<Store>, host: &dyn AppDataHost) -> usize {
    let pending = match store.call(|st| st.app_data_pending()).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "the artifacts to move to their stores were not read");
            return 0;
        }
    };
    let mut moved = 0;
    for aid in pending {
        match host.call(&aid, AppDataOp::FoldMark).await {
            Ok(_) => moved += 1,
            Err(CoreError::NotFound) => {}
            Err(e) => {
                tracing::warn!(artifact = aid.as_str(), error = %e, "an artifact's app data did not move to its store")
            }
        }
    }
    if moved > 0 {
        tracing::info!(moved, "moved artifacts' app data to their own stores");
    }
    moved
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

    fn local(st: &Arc<Store>) -> LocalAppData {
        LocalAppData::new(st.home().clone(), st.clock())
    }

    /// The instance's copies of `aid`'s document records, by operation.
    fn copied(st: &Store, aid: &ArtifactId) -> Vec<String> {
        st.events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "doc.write" && r.ids.artifact.as_deref() == Some(aid.as_str()))
            .map(|r| {
                serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["doc_id"].to_string()
            })
            .collect()
    }

    /// A host whose import deletes the artifact once the store took its
    /// rows: the delete landing while the artifact moves.
    struct DeletedWhileMoving {
        inner: LocalAppData,
        store: Arc<Store>,
    }

    #[async_trait::async_trait]
    impl AppDataHost for DeletedWhileMoving {
        async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply> {
            let import = matches!(op, AppDataOp::Import { .. });
            let reply = self.inner.call(aid, op).await?;
            if import {
                let (st, id) = (self.store.clone(), aid.clone());
                st.call(move |st| st.delete_artifact(&AuditCtx::DAEMON, &id))
                    .await?;
            }
            Ok(reply)
        }

        async fn erase(&self, aid: &ArtifactId) -> Result<()> {
            self.inner.erase(aid).await
        }
    }

    #[tokio::test]
    async fn a_delete_during_the_move_leaves_no_store() {
        let (_d, st, aid) = store();
        let file = st.home().app_data_path(&aid);
        let host = MovingAppData::new(
            DeletedWhileMoving {
                inner: local(&st),
                store: st.clone(),
            },
            st.clone(),
        );
        assert!(matches!(
            host.call(&aid, set("t/a")).await,
            Err(CoreError::NotFound)
        ));
        assert!(!file.exists(), "the store the import made is erased");
    }

    #[tokio::test]
    async fn only_an_import_makes_a_store() {
        let (_d, st, aid) = store();
        let file = st.home().app_data_path(&aid);
        assert!(matches!(
            local(&st).call(&aid, set("t/a")).await,
            Err(CoreError::NotFound)
        ));
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn each_changes_records_are_copied_into_the_instances_history_once() {
        let (_d, st, aid) = store();
        let host = MovingAppData::new(local(&st), st.clone());
        host.call(&aid, set("t/a")).await.unwrap();
        assert_eq!(copied(&st, &aid), ["\"a\""]);
        // A change whose copy was lost (the store wrote it, the process
        // stopped): the next change finds the copies behind.
        local(&st).call(&aid, set("t/b")).await.unwrap();
        host.call(&aid, set("t/c")).await.unwrap();
        assert_eq!(copied(&st, &aid), ["\"a\"", "\"b\"", "\"c\""]);
        // A lost copy and a start: the first call, a read, makes it up.
        local(&st).call(&aid, set("t/d")).await.unwrap();
        let restarted = MovingAppData::new(local(&st), st.clone());
        restarted.call(&aid, AppDataOp::FoldMark).await.unwrap();
        assert_eq!(copied(&st, &aid), ["\"a\"", "\"b\"", "\"c\"", "\"d\""]);
        restarted.call(&aid, set("t/e")).await.unwrap();
        assert_eq!(copied(&st, &aid).len(), 5, "each copied once");
    }

    #[tokio::test]
    async fn an_error_keeps_its_status_code_and_extras_across_the_wire() {
        use clax_core::visibility::Author;
        use clax_store::app_data::WireError;
        let cases = vec![
            CoreError::DocWithheld {
                path: "c/1".into(),
                authors: vec![
                    Author::Owner,
                    Author::Person("u_00000000000000000000aa".into()),
                    Author::Unconnected,
                ],
            },
            CoreError::DocConflict {
                path: "c/1".into(),
                current: Some(3),
            },
            CoreError::DocConflict {
                path: "c/1".into(),
                current: None,
            },
            CoreError::DocPinRequired {
                path: "c/1".into(),
                current: 2,
            },
            CoreError::DocNotFound { path: "c/1".into() },
            CoreError::NotFound,
            CoreError::invalid("invalid_argument", "bad"),
            CoreError::invalid("quota_exceeded", "full"),
            CoreError::invalid("resource_exhausted", "big"),
            CoreError::invalid("old_str_not_found", "absent"),
            CoreError::invalid("old_str_not_unique", "twice"),
            CoreError::invalid("invalid_capabilities", "rules"),
            CoreError::InBatch {
                op: 1,
                path: "c/2".into(),
                error: Box::new(CoreError::invalid("quota_exceeded", "full")),
            },
            CoreError::Db(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("database is locked".into()),
            )),
            CoreError::Corrupt {
                artifact_id: "zafbtm2ff2jg".into(),
                column: "docs.json",
                version: None,
            },
            CoreError::SchemaNewer { found: 3, known: 1 },
        ];
        for e in cases {
            let wire: WireError =
                serde_json::from_str(&serde_json::to_string(&WireError::from(&e)).unwrap())
                    .unwrap();
            let back = CoreError::from(wire);
            if let (
                CoreError::DocWithheld { authors: a, .. },
                CoreError::DocWithheld { authors: b, .. },
            ) = (&e, &back)
            {
                assert_eq!(a, b, "withheld authors");
            }
            let got = crate::error::ApiError::from(back);
            let orig = crate::error::ApiError::from(e);
            assert_eq!(
                (got.status, got.code, &got.extra),
                (orig.status, orig.code, &orig.extra),
                "{}",
                orig.message
            );
        }
        assert_eq!(
            crate::error::ApiError::from(CoreError::from(WireError::from(
                &CoreError::SchemaNewer { found: 3, known: 1 }
            )))
            .code,
            "schema_newer"
        );
    }
}
