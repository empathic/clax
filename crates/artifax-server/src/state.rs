use artifax_core::{EventBus, Home, Store};
use std::sync::Arc;

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
}
