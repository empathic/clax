use crate::wrap_cache::WrapCache;
use clax_core::feedback::Touched;
use clax_core::{EventBus, Home};
use clax_realtime::feedback::{FeedbackCtx, sessions_stop};
use clax_store::Store;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The visibility generation (spec 2026-10-08-multi-agent-multi-person
/// M17): the store's counter, bumped by every committed change to what an
/// agent may see, and a notification of each bump for waking the waiters
/// whose view it changed. The store tells it on its writer, so it only
/// wakes waiters (`Notify::notify_waiters`): no receiver holds anything the
/// writer waits for.
#[derive(Clone)]
pub struct VisGen {
    counter: Arc<AtomicU64>,
    changed: Arc<tokio::sync::Notify>,
}

impl VisGen {
    /// The generation of `store`, listening for its bumps (it replaces any
    /// visibility listener `store` had).
    pub fn install(store: &Store) -> VisGen {
        let changed = Arc::new(tokio::sync::Notify::new());
        let wake = changed.clone();
        store.set_visibility_listener(move |_| wake.notify_waiters());
        VisGen {
            counter: store.visibility_counter(),
            changed,
        }
    }

    /// The current generation: read it before building a `Visibility`, so
    /// a change committed meanwhile makes the result stale, never missed.
    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::Acquire)
    }

    /// Waits until the generation is past `seen` (one a waiter read with
    /// [`VisGen::current`] before it last built its view), and returns the
    /// new generation. Each round makes its wake-up before it reads the
    /// generation, so a bump between the read and the wait still wakes it:
    /// a waiter cannot get that order wrong. Returns at once when the
    /// generation already moved.
    pub async fn wait_after(&self, seen: u64) -> u64 {
        loop {
            // Made first: `notify_waiters` wakes every future that exists
            // when it runs, polled or not.
            let bump = self.changed.notified();
            let now = self.current();
            if now != seen {
                return now;
            }
            bump.await;
        }
    }
}

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
    /// The web UI the shell routes serve (`clax_server::shell`): the binary's
    /// embedded one, or a directory read from disk.
    pub web: Arc<dyn clax_core::embedded::FileTree>,
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
    pub feedback_waiters: Arc<clax_realtime::feedback::FeedbackWaiters>,
    /// Sessions with a `clax feedback follow` connected (Grok's tier 5).
    pub followers: Arc<clax_realtime::feedback::Followers>,
    /// Held while delivery states are read and sent
    /// ([`clax_realtime::feedback::FeedbackCtx::states`]).
    pub feedback_states: Arc<std::sync::Mutex<()>>,
    /// Where the daemon's `codex` is; Codex tier 5 is off without it.
    pub codex: Arc<clax_realtime::push::CodexPush>,
    /// Working records (spec §10 "Working"), in memory.
    pub working: Arc<clax_core::working::Working>,
    /// Viewer presence (spec §10 "Presence"), in memory.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The live rooms of the `room` capability (memory only).
    pub rooms: Arc<clax_realtime::room::Rooms>,
    /// The `sample` capability's provider and settings.
    pub sample: Arc<clax_sample::Sampler>,
    /// The multiplexed event stream's topics and streams (`/api/stream`);
    /// fed by `events` once [`clax_realtime::stream::Hub::listen`] has run.
    pub stream: Arc<clax_realtime::stream::Hub>,
    /// Every artifact's live seats (spec 2026-10-11-clax-dev-hosted-design
    /// §16.1), held by the streams once [`crate::seats::install`] has run.
    pub seats: Arc<crate::seats::LiveSeats>,
    /// Every live page's artifact ID (spec 2026-10-05-chrome-overlay-design
    /// L10), for hiding them from the LAN and serving their snapshots.
    pub live_ids: Arc<clax_realtime::live::LiveIds>,
    /// The Clax extension's live credentials, by hash (spec
    /// 2026-10-05-chrome-overlay-design §5.3).
    pub ext_creds: Arc<crate::extension::Credentials>,
    /// The extension ID in effect for this daemon's home (spec L15): the only
    /// extension that may pair.
    pub extension_id: String,
    /// Polls waiting on each agent question (spec
    /// 2026-10-06-agent-questions-and-inbox §6.1).
    pub questions: Arc<clax_realtime::questions::QuestionWaiters>,
    /// How long a mirrored (hook) question stays open with no poll waiting
    /// on it before it is withdrawn (5 s in the daemon).
    pub question_grace: Duration,
    /// The timers of question polls and graces ([`clax_realtime::questions::TokioSleeper`]
    /// in the daemon).
    pub question_sleeper: Arc<dyn clax_realtime::questions::Sleeper>,
    /// Where question code reads the time, such as a late answer's age
    /// ([`clax_core::working::SystemClock`] in the daemon).
    pub question_clock: Arc<dyn clax_core::working::Clock>,
    /// `[questions] terminal_after_s`, read when the daemon starts.
    pub terminal_after_s: u64,
    /// The latency gate's calibration database, built on first use
    /// (`clax_server::routes::perf`).
    pub calibration: crate::perf::Slot,
    /// How long the gateway's latest signature verifications took, for the
    /// latency gate (`clax_server::routes::perf::verify_times`).
    pub verify_times: Arc<crate::perf::VerifyTimes>,
    /// The journal appender's wake-up, which `store` nudges after each
    /// commit that recorded an audit event.
    pub audit_wake: Arc<crate::audit::AuditWake>,
    /// The journal appender's state, for `GET /api/toolpath/status`.
    pub journal: Arc<crate::audit::JournalStatus>,
    /// What bounds the audit exports: one at a time, a stall limit and a
    /// time limit.
    pub exports: Arc<crate::exports::Exports>,
    /// How Claude Code's call-ID reports wait for their `tool.call`.
    pub call_ids: Arc<clax_realtime::call_ids::CallIdWait>,
    /// The address the daemon listens on: invites need one other than
    /// loopback (spec §6.2).
    pub bind: std::net::IpAddr,
    /// The port the daemon listens on.
    pub port: u16,
    /// The address invites name; `None` to use the bound one or, for an
    /// unspecified bind, the machine's LAN address at each invite.
    pub lan_host: Option<std::net::IpAddr>,
    /// The grant flow's token index and nonce caches (spec §6.4, §6.6).
    pub grant_auth: Arc<crate::grant_auth::GrantAuth>,
    /// The bounds on agents' unverified request bodies (spec §6.6).
    pub agent_gate: Arc<crate::agent::Gate>,
    /// The visibility generation (spec 2026-10-08-multi-agent-multi-person M17).
    pub vis_gen: VisGen,
    /// Debug builds: whether this is the testkit's daemon, which honours
    /// the `x-clax-test-delay-ms` header (so a test can make a real route
    /// slow) and serves `GET /api/_test/audit/ctx` (`clax_server::router`).
    /// `false` in every daemon `serve` starts; the testkit's daemon sets it.
    pub test_daemon: bool,
}

impl AppState {
    pub fn feedback_ctx(&self) -> FeedbackCtx {
        FeedbackCtx {
            events: self.events.clone(),
            waiters: self.feedback_waiters.clone(),
            followers: self.followers.clone(),
            browser_base: self.browser_base.clone(),
            store: self.store.clone(),
            codex: self.codex.clone(),
            working: self.working.clone(),
            handle: tokio::runtime::Handle::current(),
            states: self.feedback_states.clone(),
            #[cfg(debug_assertions)]
            between: None,
        }
    }
}

/// The live side of sessions' ends (spec 2026-10-08 §14), once the store
/// committed them with their cleanup: the feedback they released reaches
/// its new targets ([`apply`]), their withdrawn questions are announced,
/// and for each ended session in `sessions` its long-polls are woken and
/// forgotten, its working records end (recorded under `audit`), and its
/// followers stop. Every route that ends sessions calls it.
pub fn sessions_ended(
    s: &AppState,
    ctx: &FeedbackCtx,
    st: &Store,
    audit: &clax_core::audit::AuditCtx,
    touched: &Touched,
    withdrawn_questions: &[String],
    sessions: &[String],
) {
    crate::views::question::announce_ids(s, st, withdrawn_questions);
    sessions_stop(ctx, st, audit, touched, sessions);
}

#[cfg(test)]
mod tests {
    use super::VisGen;
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use clax_core::visibility::UNCONNECTED;
    use clax_store::Store;
    use clax_store::approvals::{Scope, Via};

    #[tokio::test]
    async fn waiting_after_a_generation_wakes_on_the_next_bump() {
        let dir = tempfile::tempdir().unwrap();
        let st = std::sync::Arc::new(Store::open(&Home::at(dir.path().join("ax"))).unwrap());
        let g = VisGen::install(&st);
        let seen = g.current();
        assert_eq!(seen, 0);
        // Already past: returns at once.
        let id = st.live_approvals(None).unwrap()[0].id.clone();
        st.revoke_approval(&AuditCtx::DAEMON, &id, None).unwrap();
        assert_eq!(g.wait_after(seen).await, 1);
        // Not yet: a bump from elsewhere wakes it.
        let (g2, st2) = (g.clone(), st.clone());
        let waiter = tokio::spawn(async move { g2.wait_after(1).await });
        tokio::task::yield_now().await;
        tokio::task::spawn_blocking(move || {
            st2.approve(
                &AuditCtx::DAEMON,
                None,
                UNCONNECTED,
                Scope::Everywhere,
                Via::Shell,
            )
            .unwrap();
        })
        .await
        .unwrap();
        assert_eq!(waiter.await.unwrap(), 2);
    }
}
