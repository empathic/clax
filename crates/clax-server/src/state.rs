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
    /// Working records (spec §10 "Working"), in memory.
    pub working: Arc<clax_core::working::Working>,
    /// Viewer presence (spec §10 "Presence"), in memory.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The live rooms of the `room` capability (memory only).
    pub rooms: Arc<crate::room::Rooms>,
    /// The `sample` capability's provider and settings.
    pub sample: Arc<crate::sample::Sampler>,
    /// The multiplexed event stream's topics and streams (`/api/stream`);
    /// fed by `events` once [`crate::stream::Hub::listen`] has run.
    pub stream: Arc<crate::stream::Hub>,
    /// Every live page's artifact ID (spec 2026-10-05-chrome-overlay-design
    /// L10), for hiding them from the LAN and serving their snapshots.
    pub live_ids: Arc<crate::live::LiveIds>,
    /// The Clax extension's live credentials, by hash (spec
    /// 2026-10-05-chrome-overlay-design §5.3).
    pub ext_creds: Arc<crate::extension::Credentials>,
    /// The extension ID in effect for this daemon's home (spec L15): the only
    /// extension that may pair.
    pub extension_id: String,
    /// Polls waiting on each agent question (spec
    /// 2026-10-06-agent-questions-and-inbox §6.1).
    pub questions: Arc<crate::questions::QuestionWaiters>,
    /// How long a mirrored (hook) question stays open with no poll waiting
    /// on it before it is withdrawn (5 s in the daemon).
    pub question_grace: Duration,
    /// The timers of question polls and graces ([`crate::questions::TokioSleeper`]
    /// in the daemon).
    pub question_sleeper: Arc<dyn crate::questions::Sleeper>,
    /// Where question code reads the time, such as a late answer's age
    /// ([`clax_core::working::SystemClock`] in the daemon).
    pub question_clock: Arc<dyn clax_core::working::Clock>,
    /// `[questions] terminal_after_s`, read when the daemon starts.
    pub terminal_after_s: u64,
    /// The latency gate's calibration database, built on first use
    /// ([`crate::routes::perf`]).
    pub calibration: crate::routes::perf::Slot,
    /// The journal appender's wake-up, which `store` nudges after each
    /// commit that recorded an audit event.
    pub audit_wake: Arc<crate::audit::AuditWake>,
    /// The journal appender's state, for `GET /api/toolpath/status`.
    pub journal: Arc<crate::audit::JournalStatus>,
    /// What bounds the audit exports: one at a time, a stall limit and a
    /// time limit.
    pub exports: Arc<crate::routes::toolpath::Exports>,
    /// How Claude Code's call-ID reports wait for their `tool.call`.
    pub call_ids: Arc<crate::routes::sessions::CallIdWait>,
}
