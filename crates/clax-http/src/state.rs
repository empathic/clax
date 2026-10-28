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

/// The origins an app embedding the core gives its WebViews: the shell's,
/// and each artifact's content origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedOrigins {
    /// The shell's origin, e.g. "https://app.clax.invalid" or "clax://app".
    pub shell: String,
    /// An artifact's content origin with `{id}` for its ID, e.g.
    /// "https://{id}.c.clax.invalid" or "clax://{id}.c".
    pub content: String,
}

impl EmbedOrigins {
    /// The content origin of artifact `id`.
    pub fn content_of(&self, id: &str) -> String {
        self.content.replace("{id}", id)
    }

    /// The content origins as site hosts: the scheme and the domain of a
    /// `content` of the form `<scheme>://{id}.<domain>` (no port, path or
    /// user), which the core serves each artifact's site on; `None` for any
    /// other form.
    pub fn site_hosts(&self) -> Option<(&str, &str)> {
        let (scheme, rest) = self.content.split_once("://")?;
        let domain = rest.strip_prefix("{id}.")?;
        let plain = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        };
        (plain(scheme) && plain(domain) && !domain.starts_with('.')).then_some((scheme, domain))
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub home: Home,
    pub token: String,
    /// Every request but [`crate::auth::open_without_credential`]'s needs a
    /// credential, `GET /api/token` included ([`crate::auth::require_credential`]);
    /// `clax serve --require-credential`, off by default.
    pub require_credential: bool,
    pub events: EventBus,
    pub started_at: String,
    pub version: &'static str,
    /// Flips to `true` when the daemon begins shutting down; long-lived streams end on it.
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub wrap_cache: Arc<WrapCache>,
    /// The web UI the shell routes serve (`clax_server::shell`): the binary's
    /// embedded one, or a directory read from disk.
    pub web: Arc<dyn clax_core::embedded::FileTree>,
    /// The quickstart page the web UI carries, when the core seeds and
    /// updates it ([`clax_server::quickstart`]); `None` from
    /// [`AppState::assemble`].
    pub quickstart: Option<Arc<clax_core::quickstart::Release>>,
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
    /// Agents' drafts (spec 2026-10-11-clax-dev-hosted-design §16.4), in memory.
    pub drafts: Arc<clax_core::drafts::Drafts>,
    /// Viewer presence (spec §10 "Presence"), in memory.
    pub presence: Arc<clax_core::presence::Presence>,
    /// The live rooms of the `room` capability (memory only).
    pub rooms: Arc<clax_realtime::room::Rooms>,
    /// The live layers between people on an artifact (memory only).
    pub live_layers: Arc<clax_realtime::live_layers::LiveLayers>,
    /// The `sample` capability's provider and settings.
    pub sample: Arc<clax_sample::Sampler>,
    /// The `net` capability's fetcher ([`crate::net::Net::default`] in the daemon);
    /// none on wasm32, where a Durable Object has no sockets of its own.
    #[cfg(not(target_arch = "wasm32"))]
    pub net: Arc<crate::net::Net>,
    /// The multiplexed event stream's topics and streams (`/api/stream`);
    /// fed by `events` once [`clax_realtime::stream::Hub::listen`] has run.
    pub stream: Arc<clax_realtime::stream::Hub>,
    /// Every artifact's live seats (spec 2026-10-11-clax-dev-hosted-design
    /// §16.1), held by the streams once [`crate::seats::install`] has run.
    pub seats: Arc<crate::seats::LiveSeats>,
    /// The long-lived connections each client of a hosted daemon holds
    /// ([`crate::socket_caps`]).
    pub sockets: Arc<crate::socket_caps::SocketCaps>,
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
    /// Hosted mode ([`crate::hosted`]); `None` for the local daemon.
    pub hosted: Option<Arc<crate::hosted::Hosted>>,
    /// What a hosted Clax says about the Clax app: its app links and store
    /// pages (`[hosted] app_links` and `stores`, or the Worker's vars),
    /// which the host sets after [`AppState::assemble`]; none by default.
    pub clax_app: Arc<clax_core::config::ClaxApp>,
    /// The build identity the host was given, which `/healthz` reports as
    /// `build`: on a Worker the `CLAX_BUILD` var a deploy sets, unique to
    /// each upload, which the host sets after [`AppState::assemble`]; `None`
    /// by default.
    pub build: Option<String>,
    /// The origins of the app that embeds this core, which its shell
    /// documents and content policy name; `None` for a daemon.
    pub embed: Option<EmbedOrigins>,
    /// The host's git remotes ([`crate::git_remotes`]), once it installs
    /// them; empty on the native daemon.
    pub git_remotes: Arc<crate::git_remotes::Slot>,
    /// Linked threads' sync with remotes (`clax_server::linked`); idle on a
    /// hosted daemon.
    pub linked: Arc<clax_realtime::linked::Linked>,
    /// Where each artifact's app data is ([`crate::app_data_host`]):
    /// [`AppState::assemble`] sets the host's default (each artifact's file
    /// in the home natively), which a host with its own stores replaces.
    pub app_data: Arc<dyn crate::app_data_host::AppDataHost>,
    /// The queue owner of each job client ([`crate::jobs_owner`]), in
    /// memory only: empty at every start.
    pub jobs_owners: Arc<crate::jobs_owner::JobsOwners>,
    /// Debug builds: whether this is the testkit's daemon, which honours
    /// the `x-clax-test-delay-ms` header (so a test can make a real route
    /// slow) and serves `GET /api/_test/audit/ctx` (`clax_server::router`).
    /// `false` in every daemon `serve` starts; the testkit's daemon sets it.
    pub test_daemon: bool,
    /// The settings routes' configuration in effect and audit hook
    /// ([`crate::settings`]).
    pub settings: Arc<crate::settings::Settings>,
    /// ClaxBot's keys and run driver seam ([`crate::bot`]); the daemon
    /// and the Worker install theirs after [`AppState::assemble`].
    pub bot: Arc<crate::bot::Bot>,
}

/// [`AppState::sites`] without a site domain.
static LOCAL_SITES: std::sync::LazyLock<crate::host::SiteHosts> =
    std::sync::LazyLock::new(crate::host::SiteHosts::local);

impl AppState {
    /// The host-to-artifact lookup ([`crate::host::site_layer`]): hosted
    /// mode's site domain when it has one ([`crate::hosted::Hosted::sites`],
    /// its only source), else `<id>.localhost`.
    pub fn sites(&self) -> &crate::host::SiteHosts {
        self.hosted
            .as_deref()
            .and_then(|h| h.sites.as_ref())
            .unwrap_or(&LOCAL_SITES)
    }

    pub fn feedback_ctx(&self) -> FeedbackCtx {
        FeedbackCtx {
            events: self.events.clone(),
            waiters: self.feedback_waiters.clone(),
            followers: self.followers.clone(),
            browser_base: self.browser_base.clone(),
            store: self.store.clone(),
            codex: self.codex.clone(),
            hosted: self.hosted.is_some(),
            working: self.working.clone(),
            drafts: self.drafts.clone(),
            git_remotes: self.git_remotes.clone(),
            handle: clax_rt::Handle::current(),
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

/// What a host decides about the state it serves from; [`AppState::assemble`]
/// makes the rest the same way on every host (the daemon, a Durable Object).
pub struct StateParts {
    pub store: Arc<Store>,
    pub home: Home,
    /// The owner's bearer token.
    pub token: String,
    /// Whether every request but the exempt ones needs a credential
    /// ([`AppState::require_credential`]).
    pub require_credential: bool,
    /// When the server started (the store's clock, RFC 3339).
    pub started_at: String,
    pub version: &'static str,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub self_base: String,
    pub browser_base: String,
    pub codex: clax_realtime::push::CodexPush,
    pub sample: Arc<clax_sample::Sampler>,
    /// The clock of working records, presence, live seats and questions.
    pub clock: Arc<dyn clax_core::working::Clock>,
    /// The timers of question polls and graces.
    pub question_sleeper: Arc<dyn clax_realtime::questions::Sleeper>,
    /// The timer of the agents' request-body gate.
    pub agent_timer: Arc<dyn crate::agent::Timer>,
    pub extension_id: String,
    pub terminal_after_s: u64,
    pub bind: std::net::IpAddr,
    pub port: u16,
    /// Hosted mode ([`crate::hosted`]); `None` for the local daemon.
    pub hosted: Option<Arc<crate::hosted::Hosted>>,
    /// The web UI the daemon serves.
    pub web: Arc<dyn clax_core::embedded::FileTree>,
    /// The origins of the app that embeds the core
    /// ([`AppState::embed`]); `None` for a daemon.
    pub embed: Option<EmbedOrigins>,
    /// Where artifacts' app data lives, already moving `store`'s documents
    /// out ([`crate::app_data_host::MovingAppData`]); `None` for the home's
    /// ([`crate::app_data_host::default_host`]).
    pub app_data: Option<Arc<dyn crate::app_data_host::AppDataHost>>,
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
        let live_ids = Arc::new(clax_realtime::live::LiveIds::load(&p.store)?);
        let ext_creds = Arc::new(crate::extension::Credentials::load(&p.store)?);
        let grant_auth = Arc::new(crate::grant_auth::GrantAuth::load(&p.store)?);
        let vis_gen = VisGen::install(&p.store);
        let app_data = p
            .app_data
            .unwrap_or_else(|| crate::app_data_host::default_host(&p.store, &p.home));
        let settings = Arc::new(crate::settings::Settings::at_start(
            &p.home,
            p.hosted.is_some(),
        ));
        // A hosted server, and the Worker always, send ClaxBot's instance
        // key only to a public https host.
        if p.hosted.is_some() || cfg!(target_arch = "wasm32") {
            p.store
                .set_base_url_policy(clax_store::bot_settings::BaseUrlPolicy::Hosted);
        }
        Ok(AppState {
            settings,
            bot: Arc::default(),
            home: p.home,
            token: p.token,
            require_credential: p.require_credential,
            events: EventBus::new(),
            started_at: p.started_at,
            version: p.version,
            shutdown: p.shutdown,
            wrap_cache: Arc::new(WrapCache::new(crate::wrap_cache::DEFAULT_MAX_BYTES)),
            #[cfg(not(target_arch = "wasm32"))]
            net: Arc::new(crate::net::Net::default()),
            request_timeout: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(120),
            sse_keep_alive: Duration::from_secs(15),
            self_base: p.self_base,
            browser_base: p.browser_base,
            feedback_waiters: Arc::new(Default::default()),
            followers: Arc::new(Default::default()),
            feedback_states: Arc::new(Default::default()),
            codex: Arc::new(p.codex),
            working: Arc::new(clax_core::working::Working::new(p.clock.clone())),
            drafts: Arc::new(clax_core::drafts::Drafts::new(p.clock.clone())),
            presence: Arc::new(clax_core::presence::Presence::new(p.clock.clone())),
            rooms: Arc::new(clax_realtime::room::Rooms::default()),
            live_layers: Arc::new(clax_realtime::live_layers::LiveLayers::new(p.clock.clone())),
            sample: p.sample,
            stream: clax_realtime::stream::Hub::new(live_ids.clone()),
            seats: crate::seats::LiveSeats::new(
                crate::seats::limits_for(p.embed.is_some(), |k| std::env::var(k).ok()),
                p.clock.clone(),
            ),
            sockets: Arc::default(),
            live_ids,
            ext_creds,
            extension_id: p.extension_id,
            questions: Arc::new(Default::default()),
            question_grace: Duration::from_secs(5),
            question_sleeper: p.question_sleeper,
            question_clock: p.clock,
            terminal_after_s: p.terminal_after_s,
            calibration: Arc::default(),
            verify_times: Arc::default(),
            audit_wake: crate::audit::AuditWake::new(),
            journal: crate::audit::JournalStatus::off(None),
            exports: crate::exports::Exports::new(),
            call_ids: Arc::new(clax_realtime::call_ids::CallIdWait::new(
                clax_realtime::call_ids::CALL_ID_GRACE,
                clax_realtime::call_ids::CALL_ID_CAP,
            )),
            bind: p.bind,
            port: p.port,
            lan_host: None,
            grant_auth,
            agent_gate: Arc::new(crate::agent::Gate::new(p.agent_timer)),
            vis_gen,
            hosted: {
                if let Some(h) = &p.hosted {
                    p.store.set_member_caps(Some(h.member_caps));
                }
                p.hosted
            },
            test_daemon: false,
            app_data,
            web: p.web,
            quickstart: None,
            embed: p.embed,
            clax_app: Arc::default(),
            build: None,
            git_remotes: Arc::default(),
            linked: Arc::new(clax_realtime::linked::Linked::new(Arc::new(
                clax_realtime::questions::TokioSleeper,
            ))),
            jobs_owners: Arc::default(),
            store: p.store,
        })
    }

    /// The in-memory sweeps, due every [`clax_realtime::working::SWEEP_INTERVAL`]:
    /// stale working records, presence and idle drafts (announced), streams
    /// detached past their grace, and live seats gone idle. Live layers
    /// need none: each flush is a timer its own edit starts. The daemon runs
    /// it on a loop; a Durable Object from its alarm.
    pub async fn sweep(&self) {
        clax_realtime::working::sweep_and_announce(&self.store, &self.working, &self.events).await;
        clax_realtime::presence::sweep_and_announce(&self.presence, &self.events);
        clax_realtime::drafts::sweep_and_announce(&self.drafts, &self.events);
        self.stream.sweep();
        self.seats.sweep();
        crate::views::question::withdraw_due(self).await;
        crate::git_remotes::settle(self).await;
    }

    /// Ends the sessions unseen past their idle time on a host that runs
    /// no agent processes and is never down while a client can reach it (a
    /// Durable Object, woken by each request): every live session unseen
    /// for [`Store::REMOTE_SESSION_IDLE_SECS`], counted from its last
    /// request ([`Store::reap_sessions_from`] with no resume), as the
    /// daemon's reaper ends them (`clax_server::serve::reap_idle`): the
    /// feedback they released handed on, their working records ended and
    /// their withdrawn questions announced. Returns how many ended.
    ///
    /// # Errors
    /// The store's.
    pub async fn reap_unseen(&self) -> clax_core::Result<usize> {
        let ctx = self.feedback_ctx();
        let st = self.clone();
        self.store
            .call(move |store| {
                let idle = Duration::from_secs(Store::REMOTE_SESSION_IDLE_SECS);
                let r = store.reap_sessions_from(idle, &|_| false, "")?;
                let ttl = clax_core::audit::AuditCtx::system(clax_core::audit::SystemReason::Ttl);
                clax_realtime::feedback::sessions_stop(&ctx, store, &ttl, &r.touched, &r.ended);
                crate::views::question::announce_ids(&st, store, &r.withdrawn_questions);
                Ok(r.ended.len())
            })
            .await
    }

    /// When [`AppState::reap_unseen`] next has a session to end, or `None`
    /// without a live session.
    ///
    /// # Errors
    /// The store's.
    pub async fn next_unseen(&self) -> clax_core::Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.store
            .call(|st| st.sessions_next_idle(Duration::from_secs(Store::REMOTE_SESSION_IDLE_SECS)))
            .await
    }

    /// How long until [`AppState::sweep`] next has something to do (zero
    /// once it is due): the earliest of a working record lapsing, a presence
    /// report lapsing or a gone viewer dropping, a draft going idle, a
    /// detached stream's grace ending, a live seat going idle, and a
    /// question's swept grace ending
    /// ([`clax_realtime::questions::Sleeper::times_graces`]), and a git token
    /// revocation the host failed ([`crate::git_remotes::RETRY_AFTER`]).
    /// `None` when none of them holds
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
            self.git_remotes
                .due()
                .then_some(crate::git_remotes::RETRY_AFTER),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Starts what turns the store's changes and the event bus into the
    /// streams' events: the stream hub's listener, the live seats' link to
    /// it, and the inbox, approvals, pushes and git remotes listeners. The
    /// daemon's `clax_server::start_listeners` adds the people routes' and
    /// the linked threads' sync. Returns the handles of the tasks they
    /// spawn, which run until aborted.
    pub fn start_listeners(&self) -> Vec<clax_rt::JoinHandle<()>> {
        self.stream.listen(&self.events);
        {
            let store = self.store.clone();
            self.stream.check_reads(move |reader, source| {
                crate::db_caller::reads_now(&store, reader, source)
            });
            let store = self.store.clone();
            self.stream.hide(move |aid| store.is_hidden(aid));
            // A hosted stream's gallery carries what its person reads; only
            // a hosted daemon's streams hold a scoped gallery, so a local
            // daemon never asks.
            let store = self.store.clone();
            self.stream.check_gallery(move |key, aid| {
                crate::hosted::MemberRights::gallery_reads(&store, key, aid)
            });
        }
        crate::seats::install(self);
        let mut tasks = crate::views::inbox::listen(self);
        tasks.push(crate::views::approval::listen(self));
        crate::pushes::listen(self);
        tasks.push(crate::git_remotes::listen(self));
        tasks
    }
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
