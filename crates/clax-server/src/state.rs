use crate::wrap_cache::WrapCache;
use clax_core::{EventBus, Home, Store};
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
    /// Base URL (`http://<host>:<port>`, no trailing slash) at which this daemon
    /// reaches its own API; the `/mcp` tools call the REST API through it.
    pub self_base: String,
    /// Base URL a browser on this machine uses to reach the daemon; tool results
    /// build artifact URLs from it.
    pub browser_base: String,
    /// Long-polls waiting for feedback, by session.
    pub feedback_waiters: Arc<crate::feedback::FeedbackWaiters>,
    /// Sessions with a `clax feedback follow` connected (Grok's tier 5).
    pub followers: Arc<crate::feedback::Followers>,
    /// Where the daemon's `codex` is; Codex tier 5 is off without it.
    pub codex: Arc<crate::push::CodexPush>,
}
