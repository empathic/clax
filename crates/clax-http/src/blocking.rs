//! Hands store work to the store's own database threads (see
//! [`clax_store::Store::call`]) so SQLite and file I/O never stall a runtime
//! worker and never claim threads from tokio's blocking pool.
//!
//! A handler whose job reads or writes content (version files, assets,
//! clips) runs it with [`AppState::store_call_async`], written against the
//! store's `async` twins: on the daemon's pool the job still runs on a
//! store thread, exactly as a [`AppState::store_call`] job calling the
//! synchronous methods does, and on a store on one connection (a Durable
//! Object's, and every store on `wasm32`) it is awaited on the request's
//! task, so a content store whose I/O needs an executor completes where a
//! synchronous method would fail with `WouldBlock` (503 `busy`). A job that
//! touches no content stays with `store_call`.

use crate::error::ApiError;
use crate::state::AppState;
use clax_store::Store;

impl AppState {
    /// Runs `f` on one of the store's database threads, in the interactive
    /// lane. Calls queue in FIFO order behind a bounded queue. If the
    /// handler future is dropped (the timeout layer gives up) before `f`
    /// starts, `f` never runs; once started, `f` runs to completion, so any
    /// follow-up work that must happen once the store call succeeds (events,
    /// cache invalidation) belongs inside `f`.
    pub async fn store_call<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> clax_core::Result<T> + Send + 'static,
    {
        self.store.call(f).await.map_err(ApiError::from)
    }

    /// [`AppState::store_call`] for a job that awaits the store's `async`
    /// methods ([`clax_store::Store::call_async`]): `f` makes its future
    /// from the store. Same lane, queue and thread as `store_call` on the
    /// pool; awaited on this task on one connection. The returned future
    /// is `Send` on every target, as axum's handlers must be: on `wasm32`,
    /// where a content store's futures need not be, it is wrapped
    /// ([`clax_rt::sendable`]).
    pub async fn store_call_async<T, F, Fut>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(std::sync::Arc<Store>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = clax_core::Result<T>> + clax_rt::SendOnNative + 'static,
    {
        clax_rt::sendable(self.store.call_async(f))
            .await
            .map_err(ApiError::from)
    }

    /// [`AppState::store_call`] in the store's bulk lane
    /// ([`clax_store::Store::call_bulk`]), for reads whose cost grows with
    /// the whole home: interactive calls start ahead of it.
    pub async fn store_call_bulk<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> clax_core::Result<T> + Send + 'static,
    {
        self.store.call_bulk(f).await.map_err(ApiError::from)
    }
}
