use crate::wrap_cache::WrapCache;
use artifax_core::{EventBus, Home, Store};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub home: Home,
    pub token: String,
    pub events: EventBus,
    pub started_at: String,
    pub version: &'static str,
    /// Flips to `true` when the daemon begins shutting down; long-lived streams end on it.
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub wrap_cache: Arc<WrapCache>,
    /// Deadline for ordinary `/api` requests.
    pub request_timeout: Duration,
    /// Deadline for the publish routes and asset upload.
    pub publish_timeout: Duration,
    /// Interval between SSE keep-alive comments on `/api/events` (15 s in the daemon).
    pub sse_keep_alive: Duration,
}
