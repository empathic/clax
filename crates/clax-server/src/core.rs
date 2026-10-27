//! The core `clax serve` runs, without a listener: the store, the state,
//! the audit journal, the listeners, the router and the background tasks.
//! [`crate::serve::serve`] runs a [`Core`] behind a socket, beside
//! `daemon.json` and its stale watcher; an app that embeds Clax runs one
//! and feeds its router requests in-process.

use clax_core::Home;
use clax_http::state::AppState;
use clax_store::Store;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// A session unseen for this long, with no live process, is ended by the reaper.
const SESSION_IDLE: Duration = Duration::from_secs(Store::SESSION_IDLE_SECS);
/// How often the core deletes drafts older than
/// [`clax_store::drafts::DRAFT_MAX_AGE`] (and once at start).
const DRAFT_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// How long shutdown waits for the journal appender to close its segment.
/// The appender stops as shutdown begins, beside the connection drain, and
/// catches up for at most 0.5 s; a wedged one is left on its own thread,
/// which the process exit ends. The drain, the tasks' stop
/// ([`TASKS_STOP_TIMEOUT`]) and the journal's stop take at most 6.75 s,
/// inside the 7 s a replacing client waits before it sends SIGTERM; what
/// the journal does not write, the next start writes from the table.
const JOURNAL_STOP_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long [`Core::stop`] waits for its aborted tasks to end: each ends at
/// its next await, unless it is inside a blocking call.
const TASKS_STOP_TIMEOUT: Duration = Duration::from_millis(250);
/// How often grants due to expire are expired and the nonce caches swept.
const GRANT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Tests: called with its home as each [`Core::stop`] ends, once its
/// tasks have ended, the journal has synced and the store has closed.
#[cfg(test)]
pub(crate) static STOPPED: std::sync::Mutex<Option<StoppedHook>> = std::sync::Mutex::new(None);

/// Tests: the hook [`STOPPED`] holds.
#[cfg(test)]
pub(crate) type StoppedHook = Box<dyn Fn(&Home) + Send>;

/// What a [`Core`] starts with.
pub struct CoreConfig {
    /// The Clax home it serves.
    pub home: Home,
    /// Version reported by `/healthz`.
    pub version: &'static str,
    /// How often idle sessions with dead processes are ended (60 s in production).
    pub reap_interval: Duration,
    /// Where the core's `codex` is, for Codex tier 5.
    pub codex: clax_realtime::push::CodexPush,
    /// The `sample` capability's provider and settings.
    pub sample: Arc<clax_sample::Sampler>,
    /// The web UI the shell routes serve.
    pub web: Arc<dyn clax_core::embedded::FileTree>,
    /// Hosted mode ([`clax_http::hosted`]), or `None` for the local daemon.
    pub hosted: Option<Arc<clax_http::hosted::Hosted>>,
    /// The DNS names the API answers to beside this machine's own (the
    /// home's `[serve] trusted_hosts`, which `clax serve` reads); empty for
    /// none.
    pub trusted_hosts: Vec<String>,
    /// Whether a request through `tailscale serve` on one of
    /// `trusted_hosts` is the viewer its tailnet login names (the home's
    /// `[serve] tailscale_identity`).
    pub tailscale_identity: bool,
    /// Serve nothing but `/healthz`, static files and content by ID without
    /// a credential ([`AppState::require_credential`]).
    pub require_credential: bool,
    /// The address `self_base`, `browser_base` and the host checks name;
    /// `serve` passes the one it listens on.
    pub bind: std::net::IpAddr,
    /// The port they name; `serve` passes the bound one.
    pub port: u16,
    /// The origins of an app that embeds the core
    /// ([`AppState::embed`]); `None` for `clax serve`.
    pub embed: Option<clax_http::state::EmbedOrigins>,
    /// Seed the Clax Quickstart into a new local home and move an untouched
    /// one to the web UI's release at start (`[gallery] quickstart`); a
    /// hosted core never does.
    pub quickstart: bool,
}

/// A started core: its state, its router, and the tasks it runs until
/// [`Core::stop`] (its background tasks and its listeners).
pub struct Core {
    state: AppState,
    router: axum::Router,
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    journal_stopped: Option<tokio::task::JoinHandle<()>>,
    /// ClaxBot's driver, whose runs [`Core::stop`] releases.
    bot_driver: Arc<clax_bot::Driver>,
}

impl Core {
    /// Opens the home's store (its backup, adoption and migrations;
    /// `progress` hears the audit backfill's progress) and starts the
    /// state, the journal, the listeners and the background tasks: the
    /// session reaper, the grant sweeper, the planner statistics refresh,
    /// the drafts sweeper and the working sweeper. Runs on the caller's
    /// Tokio runtime.
    ///
    /// # Errors
    /// When the store does not open or the state cannot be assembled.
    pub async fn start(
        cfg: CoreConfig,
        progress: &mut (dyn FnMut(&clax_store::backfill::Progress) + Send),
    ) -> anyhow::Result<Core> {
        cfg.home.ensure_dirs()?;
        let store = Arc::new(Store::open_reporting(&cfg.home, progress)?);
        // Hooks that waited on the previous daemon are gone: their questions
        // are withdrawn (and kept, as every question is).
        let gone = store.withdraw_hook_questions_on_start()?;
        if !gone.is_empty() {
            tracing::info!(count = gone.len(), "withdrew mirrored questions left open");
        }
        let config = clax_core::config::HomeConfig::load(cfg.home.root());
        let terminal_after_s = match &config {
            Ok(c) => c.questions_terminal_after_s(),
            Err(e) => {
                tracing::warn!(error = %e, "config.toml unreadable; questions use the defaults");
                clax_core::config::TERMINAL_AFTER_S
            }
        };
        // A hosted daemon's app links and store pages: the `[hosted]` that
        // made it hosted, which `clax serve` checked before starting it.
        let clax_app = match (&cfg.hosted, &config) {
            (Some(_), Ok(c)) => c
                .hosted()
                .ok()
                .flatten()
                .map(|h| h.clax_app)
                .unwrap_or_default(),
            _ => clax_core::config::ClaxApp::default(),
        };
        let token = crate::serve::generate_token();
        let started_at = store.now();
        let bind = cfg.bind.to_string();
        let port = cfg.port;
        let (shutdown, _) = watch::channel(false);
        let mut state = AppState::assemble(clax_http::state::StateParts {
            store,
            home: cfg.home.clone(),
            token,
            require_credential: cfg.require_credential,
            started_at,
            version: cfg.version,
            shutdown: shutdown.subscribe(),
            self_base: format!("http://{}:{port}", clax_core::daemon::probe_host(&bind)),
            browser_base: format!("http://{}:{port}", clax_core::daemon::browser_host(&bind)),
            codex: cfg.codex,
            sample: cfg.sample,
            clock: Arc::new(clax_core::working::SystemClock),
            question_sleeper: Arc::new(clax_realtime::questions::TokioSleeper),
            agent_timer: Arc::new(clax_http::agent::TokioTimer::default()),
            extension_id: clax_core::extension::extension_id_in_effect(cfg.home.root()),
            terminal_after_s,
            bind: cfg.bind,
            port,
            trusted_hosts: cfg.trusted_hosts,
            tailscale_identity: cfg.tailscale_identity,
            hosted: cfg.hosted,
            web: cfg.web,
            embed: cfg.embed,
        })?;
        state.clax_app = Arc::new(clax_app);
        if cfg.quickstart {
            state.quickstart = clax_core::quickstart::Release::from_tree(&*state.web).map(Arc::new);
        }
        // The core's `net` keeps a shared disk cache in the home.
        state.net = Arc::new(clax_http::net_cache::daemon_net(cfg.home.root()));
        state.bot = Arc::new(local_bot(&state));
        // ClaxBot's driver: the runs a previous process left, then every new one.
        let bot_driver = crate::bot::driver(&mut state);
        state.audit_wake.install(&state.store);
        // The appender recovers the journal and catches it up on its own
        // thread, so the core serves at once.
        let journal = clax_http::audit::start_journal(
            clax_http::audit::JournalStart {
                store: state.store.clone(),
                dir: clax_http::audit::journal_dir(&cfg.home),
                wake: state.audit_wake.clone(),
                status: state.journal.clone(),
                version: cfg.version.to_string(),
            },
            clax_core::config::HomeConfig::load(cfg.home.root()).and_then(|c| c.toolpath()),
        );
        // The journal stops as shutdown begins, beside the connection drain:
        // it syncs and closes its segment, and the next start writes from the
        // table whatever it did not.
        let journal_stopped = journal.map(|j| {
            let mut begun = shutdown.subscribe();
            tokio::spawn(async move {
                while !*begun.borrow() {
                    if begun.changed().await.is_err() {
                        break;
                    }
                }
                if !j.stop_within(JOURNAL_STOP_TIMEOUT).await {
                    tracing::warn!(
                        "the audit journal did not stop in time; the next start recovers it"
                    );
                }
            })
        });
        let listeners = crate::start_listeners(&state);
        tracing::info!(codex = ?state.codex.bin, source = ?state.codex.source, "codex push");
        tracing::info!(provider = ?state.sample.provider_name(), "sample provider");
        let mut tasks = background_tasks(&state, cfg.reap_interval);
        tasks.extend(listeners);
        tasks.push(crate::bot::start_driver(&bot_driver));
        // The artifacts whose documents the instance still holds move to
        // their own stores now, one at a time, rather than each at its
        // first call.
        {
            let (store, host) = (state.store.clone(), state.app_data.clone());
            tasks.push(tokio::spawn(async move {
                clax_http::app_data_host::move_pending(&store, &*host).await;
            }));
        }
        // A local home is not seeded with the quickstart (only a hosted
        // site's galleries are, at sign-in and invite accept); a quickstart
        // a local home already holds still follows each release until
        // someone touches it.
        if state.hosted.is_none() {
            crate::quickstart::update(&state, None).await;
        }
        let router = crate::router::build_router_with_shutdown(state.clone(), shutdown.clone());
        Ok(Core {
            state,
            router,
            shutdown,
            tasks,
            journal_stopped,
            bot_driver,
        })
    }

    /// The core's state.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// The router, with `/api/admin/shutdown` wired to
    /// [`Core::shutdown_requested`].
    pub fn router(&self) -> axum::Router {
        self.router.clone()
    }

    /// Turns `true` when the shutdown route or [`Core::stop`] asked the core
    /// to stop; long-lived streams end on it.
    pub fn shutdown_requested(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// What asks the core to stop as the shutdown route does: sending
    /// `true` ends open streams and has the journal begin to close its
    /// segment.
    pub fn shutdown_handle(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }

    /// Stops the core: ends its tasks (the background tasks, the
    /// listeners and the linked threads' sync) and waits for them to end,
    /// waits for the journal to close its segment, and shuts the store
    /// down, as `serve` does after its drain. No task of the core runs
    /// after it returns.
    pub async fn stop(self) {
        self.stop_within(None).await;
    }

    /// [`Core::stop`], waiting at most `store_within` for the store's
    /// queued calls to finish and its threads to stop ([`stop_store`]):
    /// false when they had not by then and are left running on their own
    /// threads, which only a process about to exit may leave behind.
    pub async fn stop_bounded(self, store_within: Duration) -> bool {
        self.stop_within(Some(store_within)).await
    }

    async fn stop_within(self, store_within: Option<Duration>) -> bool {
        let _ = self.shutdown.send(true);
        for t in &self.tasks {
            t.abort();
        }
        // The linked sync's tasks are aborted and awaited with the rest.
        let linked = crate::linked::stop(&self.state);
        let ended = futures::future::join(
            futures::future::join_all(self.tasks),
            futures::future::join_all(linked),
        );
        if tokio::time::timeout(TASKS_STOP_TIMEOUT, ended)
            .await
            .is_err()
        {
            tracing::warn!("the core's tasks did not end in time");
        }
        self.bot_driver.release_all();
        // The journal has closed its segment (it began to as shutdown
        // began) before the store stops.
        if let Some(stopped) = self.journal_stopped {
            let _ = tokio::time::timeout(JOURNAL_STOP_TIMEOUT, stopped).await;
        }
        let store = self.state.store.clone();
        #[cfg(test)]
        let home = self.state.home.clone();
        drop(self.router);
        drop(self.state);
        // Let queued store calls finish and the database threads stop.
        let stopped = match store_within {
            Some(within) => stop_store(store, within).await,
            None => {
                if let Err(e) = tokio::task::spawn_blocking(move || store.shutdown()).await {
                    tracing::warn!(error = %e, "store shutdown failed");
                }
                true
            }
        };
        #[cfg(test)]
        if let Some(f) = &*STOPPED.lock().unwrap_or_else(|e| e.into_inner()) {
            f(&home);
        }
        stopped
    }
}

/// ClaxBot's state for a core starting on `s` (spec
/// 2026-10-21-claxbot-design §8): the `[bot]` settings the configuration
/// in effect sets, laid over the stored ones (an invalid `[bot]` turns
/// ClaxBot off), the home's key-encryption key (`<home>/secrets/kek`,
/// made on first use; none when it cannot be read, so no key is stored),
/// and the instance key in `[bot] api_key_env`'s variable, which takes
/// precedence over a stored one. A hosted core reads no `[bot]` and no
/// variable.
fn local_bot(s: &AppState) -> clax_http::bot::Bot {
    let mut bot = clax_http::bot::Bot::default();
    clax_store::secrets::sweep_leftovers(&s.home, s.store.now_utc());
    bot.kek = match clax_store::secrets::Kek::load_or_create(&s.home) {
        Ok(k) => Some(k),
        Err(e) => {
            tracing::warn!(error = %e, "no key-encryption key: ClaxBot stores no model key");
            None
        }
    };
    if s.hosted.is_some() {
        return bot;
    }
    let Some(cfg) = s.settings.running.as_ref() else {
        return bot;
    };
    let (ov, enabled) = clax_http::bot::configured(cfg);
    s.store.set_bot_overrides(ov);
    bot.enabled = enabled;
    if let Ok(b) = cfg.bot()
        && let Ok(key) = std::env::var(&b.api_key_env)
        && !key.trim().is_empty()
    {
        bot.env_key = Some(clax_http::bot::EnvKey {
            var: b.api_key_env,
            key: clax_core::secret::Secret::new(key.trim().to_string()),
        });
    }
    bot
}

/// Stops `store` ([`Store::shutdown`]), waiting at most `within` for its
/// queued calls to finish and its threads to stop: false when they had not
/// by then, and are left running on their own threads.
pub(crate) async fn stop_store(store: Arc<Store>, within: Duration) -> bool {
    match tokio::time::timeout(
        within,
        tokio::task::spawn_blocking(move || store.shutdown()),
    )
    .await
    {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "store shutdown failed");
            true
        }
        Err(_) => false,
    }
}

/// The background tasks a core runs: the session reaper, the grant
/// sweeper, the planner statistics refresh, the drafts sweeper and the
/// working sweeper.
fn background_tasks(state: &AppState, reap_interval: Duration) -> Vec<tokio::task::JoinHandle<()>> {
    let fctx = state.feedback_ctx();
    let reap_state = state.clone();
    let reaper_store = state.store.clone();
    let reaper = tokio::spawn(async move {
        let mut since = (std::time::SystemTime::now(), Instant::now());
        loop {
            tokio::time::sleep(reap_interval).await;
            let now = (std::time::SystemTime::now(), Instant::now());
            let woke = crate::serve::slept(
                now.0.duration_since(since.0).unwrap_or_default(),
                now.1.duration_since(since.1),
            );
            since = now;
            let ctx = fctx.clone();
            let st = reap_state.clone();
            let reaped = reaper_store
                .call(move |store| {
                    if woke {
                        // Remote agents could not reach a sleeping daemon:
                        // their idle time counts from now (spec §14).
                        store.mark_resumed();
                    }
                    let r = crate::serve::reap_idle(
                        &ctx,
                        store,
                        SESSION_IDLE,
                        &clax_core::daemon::pid_alive,
                    )?;
                    clax_http::views::question::announce_ids(&st, store, &r.withdrawn_questions);
                    Ok(r)
                })
                .await;
            match reaped {
                Ok(r) if r.ended.is_empty() => {}
                Ok(r) => tracing::info!(count = r.ended.len(), "ended idle sessions"),
                Err(e) => tracing::warn!(error = %e, "session reaper failed"),
            }
        }
    });

    // Grants due to expire on start, then every minute; the nonce caches
    // forget what is older than their window.
    let grant_state = state.clone();
    let grant_sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(GRANT_SWEEP_INTERVAL);
        loop {
            every.tick().await;
            match crate::routes::grants::sweep(&grant_state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "expired grants"),
                Err(e) => tracing::warn!(code = e.code, "grant sweep failed"),
            }
        }
    });

    let optimize_store = state.store.clone();
    let optimizer = tokio::spawn(async move {
        let mut every = tokio::time::interval(clax_store::OPTIMIZE_INTERVAL);
        every.tick().await;
        loop {
            every.tick().await;
            if let Err(e) = optimize_store.call(|store| store.optimize()).await {
                tracing::warn!(error = %e, "planner statistics refresh failed");
            }
        }
    });

    // Drafts older than
    // their age go at start, the first tick, and every interval after.
    let (drafts_store, drafts_cache) = (state.store.clone(), state.wrap_cache.clone());
    let drafts_sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(DRAFT_SWEEP_INTERVAL);
        loop {
            every.tick().await;
            let cache = drafts_cache.clone();
            let swept = drafts_store
                .call(move |st| crate::routes::app_drafts::sweep(st, &cache, chrono::Utc::now()))
                .await;
            match swept {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "removed old drafts"),
                Err(e) => tracing::warn!(error = %e, "draft sweep failed"),
            }
        }
    });

    let sweep_state = state.clone();
    let sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(clax_realtime::working::SWEEP_INTERVAL);
        loop {
            every.tick().await;
            sweep_state.sweep().await;
        }
    });

    vec![reaper, grant_sweeper, optimizer, drafts_sweeper, sweeper]
}
