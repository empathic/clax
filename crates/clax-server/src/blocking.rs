//! Runs synchronous store work on the blocking pool so SQLite and file I/O never stall a runtime worker.

use crate::error::ApiError;
use crate::state::AppState;
use axum::http::StatusCode;
use clax_core::Store;

impl AppState {
    /// Runs `f` on the blocking pool. The timeout layer can cancel the awaiting handler future
    /// after `f` has started, and `f` still runs to completion, so any follow-up work that must
    /// happen once the store call succeeds (events, cache invalidation) belongs inside `f`.
    pub async fn store_call<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> clax_core::Result<T> + Send + 'static,
    {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || f(&store)).await {
            Ok(r) => r.map_err(ApiError::from),
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "storage task failed",
                ))
            }
        }
    }
}
