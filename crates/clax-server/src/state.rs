use crate::wrap_cache::WrapCache;
use clax_core::{EventBus, Home, Store};
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
    /// Agents' drafts (spec 2026-10-11-clax-dev-hosted-design §16.4), in memory.
    pub drafts: Arc<clax_core::drafts::Drafts>,
    /// Viewer presence (spec §10 "Presence"), in memory.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The live rooms of the `room` capability (memory only).
    pub rooms: Arc<crate::room::Rooms>,
    /// The live layers between people on an artifact (memory only).
    pub live_layers: Arc<crate::live_layers::LiveLayers>,
    /// The `sample` capability's provider and settings.
    pub sample: Arc<crate::sample::Sampler>,
    /// The multiplexed event stream's topics and streams (`/api/stream`);
    /// fed by `events` once [`crate::stream::Hub::listen`] has run.
    pub stream: Arc<crate::stream::Hub>,
    /// Every artifact's live seats (spec 2026-10-11-clax-dev-hosted-design
    /// §16.1), held by the streams once [`crate::seats::install`] has run.
    pub seats: Arc<crate::seats::LiveSeats>,
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
    /// Hosted mode ([`crate::hosted`]); `None` for the local daemon.
    pub hosted: Option<Arc<crate::hosted::Hosted>>,
}

/// What a host decides about the state it serves from; [`AppState::assemble`]
/// makes the rest the same way on every host (the daemon, a Durable Object).
pub struct StateParts {
    pub store: Arc<Store>,
    pub home: Home,
    /// The owner's bearer token.
    pub token: String,
    /// When the server started (the store's clock, RFC 3339).
    pub started_at: String,
    pub version: &'static str,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub self_base: String,
    pub browser_base: String,
    pub codex: crate::push::CodexPush,
    pub sample: Arc<crate::sample::Sampler>,
    /// The clock of working records, presence, live seats and questions.
    pub clock: Arc<dyn clax_core::working::Clock>,
    /// The timers of question polls and graces.
    pub question_sleeper: Arc<dyn crate::questions::Sleeper>,
    /// The timer of the agents' request-body gate.
    pub agent_timer: Arc<dyn crate::agent::Timer>,
    pub extension_id: String,
    pub terminal_after_s: u64,
    pub bind: std::net::IpAddr,
    pub port: u16,
    /// Hosted mode ([`crate::hosted`]); `None` for the local daemon.
    pub hosted: Option<Arc<crate::hosted::Hosted>>,
}

impl AppState {
    /// The state for `p`: the daemon's deadlines and caches, empty
    /// in-memory hubs, and what the store holds loaded (live pages,
    /// extension credentials, the grant index, the visibility generation).
    /// Nothing listens yet: [`AppState::start_listeners`] starts that.
    ///
    /// # Errors
    /// The store's.
    pub fn assemble(p: StateParts) -> clax_core::Result<AppState> {
        let live_ids = Arc::new(crate::live::LiveIds::load(&p.store)?);
        let ext_creds = Arc::new(crate::extension::Credentials::load(&p.store)?);
        let grant_auth = Arc::new(crate::grant_auth::GrantAuth::load(&p.store)?);
        let vis_gen = VisGen::install(&p.store);
        Ok(AppState {
            home: p.home,
            token: p.token,
            events: EventBus::new(),
            started_at: p.started_at,
            version: p.version,
            shutdown: p.shutdown,
            wrap_cache: Arc::new(WrapCache::new(crate::wrap_cache::DEFAULT_MAX_BYTES)),
            request_timeout: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(120),
            sse_keep_alive: Duration::from_secs(15),
            self_base: p.self_base,
            browser_base: p.browser_base,
            feedback_waiters: Arc::new(Default::default()),
            followers: Arc::new(Default::default()),
            codex: Arc::new(p.codex),
            working: Arc::new(clax_core::working::Working::new(p.clock.clone())),
            drafts: Arc::new(clax_core::drafts::Drafts::new(p.clock.clone())),
            presence: Arc::new(clax_core::presence::Presence::new(p.clock.clone())),
            rooms: Arc::new(crate::room::Rooms::default()),
            live_layers: Arc::new(crate::live_layers::LiveLayers::new(p.clock.clone())),
            sample: p.sample,
            stream: crate::stream::Hub::new(live_ids.clone()),
            seats: crate::seats::LiveSeats::from_env(p.clock.clone()),
            live_ids,
            ext_creds,
            extension_id: p.extension_id,
            questions: Arc::new(Default::default()),
            question_grace: Duration::from_secs(5),
            question_sleeper: p.question_sleeper,
            question_clock: p.clock,
            terminal_after_s: p.terminal_after_s,
            calibration: Arc::default(),
            audit_wake: crate::audit::AuditWake::new(),
            journal: crate::audit::JournalStatus::off(None),
            exports: crate::routes::toolpath::Exports::new(),
            call_ids: Arc::new(crate::routes::sessions::CallIdWait::new(
                crate::routes::sessions::CALL_ID_GRACE,
                crate::routes::sessions::CALL_ID_CAP,
            )),
            bind: p.bind,
            port: p.port,
            lan_host: None,
            grant_auth,
            agent_gate: Arc::new(crate::agent::Gate::new(p.agent_timer)),
            vis_gen,
            hosted: p.hosted,
            store: p.store,
        })
    }

    /// The in-memory sweeps, due every [`crate::working::SWEEP_INTERVAL`]:
    /// stale working records, presence and idle drafts (announced), streams
    /// detached past their grace, and live seats gone idle. Live layers
    /// need none: each flush is a timer its own edit starts. The daemon runs
    /// it on a loop; a Durable Object from its alarm.
    pub async fn sweep(&self) {
        crate::working::sweep_and_announce(&self.store, &self.working, &self.events).await;
        crate::presence::sweep_and_announce(&self.presence, &self.events);
        crate::drafts::sweep_and_announce(&self.drafts, &self.events);
        self.stream.sweep();
        self.seats.sweep();
        crate::questions::withdraw_due(self).await;
    }

    /// How long until [`AppState::sweep`] next has something to do (zero
    /// once it is due): the earliest of a working record lapsing, a presence
    /// report lapsing or a gone viewer dropping, a draft going idle, a
    /// detached stream's grace ending, a live seat going idle, and a
    /// question's swept grace ending
    /// ([`crate::questions::Sleeper::times_graces`]). `None` when none of them holds
    /// anything that will need it. A host whose sweeps run from a timer it
    /// sets itself (a Durable Object's alarm) sets it for then.
    pub fn next_sweep_in(&self) -> Option<Duration> {
        let now = self.question_clock.now();
        let until =
            |at: chrono::DateTime<chrono::Utc>| (at - now).to_std().unwrap_or(Duration::ZERO);
        [
            self.working.next_lapse().map(until),
            self.presence.next_change().map(until),
            self.drafts.next_idle().map(until),
            self.stream.next_drop_in(),
            self.seats.next_idle_in(),
            self.questions.next_due().map(until),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Starts what turns the store's changes and the event bus into the
    /// streams' events: the stream hub's listener, the live seats' link to
    /// it, and the inbox, approvals and people listeners.
    pub fn start_listeners(&self) {
        self.stream.listen(&self.events);
        crate::seats::install(self);
        crate::inbox::listen(self);
        crate::approvals::listen(self);
        crate::routes::people::listen(self);
        crate::pushes::listen(self);
    }
}

#[cfg(test)]
mod tests {
    use super::VisGen;
    use clax_core::audit::AuditCtx;
    use clax_core::store::approvals::{Scope, UNCONNECTED, Via};
    use clax_core::{Home, Store};

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
