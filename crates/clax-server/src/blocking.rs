//! Hands store work to the store's own database threads (see
//! [`clax_core::Store::call`]) so SQLite and file I/O never stall a runtime
//! worker and never claim threads from tokio's blocking pool.

use crate::error::ApiError;
use crate::state::AppState;
use clax_core::Store;

impl AppState {
    /// Runs `f` on one of the store's database threads. Calls queue in FIFO
    /// order behind a bounded queue. If the handler future is dropped (the
    /// timeout layer gives up) before `f` starts, `f` never runs; once
    /// started, `f` runs to completion, so any follow-up work that must happen
    /// once the store call succeeds (events, cache invalidation) belongs
    /// inside `f`.
    pub async fn store_call<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> clax_core::Result<T> + Send + 'static,
    {
        self.store.call(f).await.map_err(ApiError::from)
    }
}
