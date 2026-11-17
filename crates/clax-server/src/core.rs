//! The core `clax serve` runs, without a listener: the store, the state,
//! the audit journal, the listeners, the router and the background tasks.
//! [`crate::serve::serve`] runs a [`Core`] behind a socket, beside
//! `daemon.json` and its stale watcher; an app that embeds Clax runs one
//! and feeds its router requests in-process.

use clax_core::Home;
use clax_http::state::AppState;
use clax_http::state::InstanceFeedback;
use clax_store::Store;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// A session unseen for this long, with no live process, is ended by the reaper.
const SESSION_IDLE: Duration = Duration::from_secs(Store::SESSION_IDLE_SECS);
/// How often the core deletes drafts older than
/// [`clax_store::drafts::DRAFT_MAX_AGE`] and prunes ended ClaxBot runs
/// ([`Store::prune_ended_runs`]), and once at start; the prune comes
/// again after [`PRUNE_AGAIN`] instead while it leaves more.
const DRAFT_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// The most prune passes one wake-up of the core's pruner runs, one after
/// another while each leaves more ([`Store::prune_ended_runs_while`]).
const PRUNE_PASSES_PER_WAKE: usize = 16;
/// How long, by the store's clock, one wake-up of the pruner begins
/// passes for.
const PRUNE_WAKE_BUDGET: Duration = Duration::from_secs(30);
/// How soon the pruner wakes again after a wake-up that left more: a
/// backlog drains at [`PRUNE_PASSES_PER_WAKE`] passes a wake-up, not one
/// pass an hour.
const PRUNE_AGAIN: Duration = Duration::from_secs(10);
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
    /// Where the price catalog is fetched from: `None` (`clax serve`'s),
    /// as `[bot] price_catalog_url` in effect says
    /// ([`crate::catalog::url_of`]); `Some(url)`, that (`Some(None)`:
    /// never), for an embedder or a test that sets it itself.
    pub catalog_url: Option<Option<String>>,
}

/// Opens the store of `home` (its backup, adoption, migrations and audit
/// backfill) and withdraws the hook questions a previous process left
/// open, on a blocking thread, so a long migration holds none of the
/// runtime's workers: the runtime goes on answering everything else (a
/// fleet's other instances, its health checks) meanwhile. `progress` hears
/// the backfill's progress on the caller's task, in order. Answers the
/// store and the IDs of the questions withdrawn.
async fn open_store(
    home: &Home,
    progress: &mut (dyn FnMut(&clax_store::backfill::Progress) + Send),
) -> anyhow::Result<(Store, Vec<String>)> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = home.clone();
    let opening = tokio::task::spawn_blocking(move || {
        let store = Store::open_reporting(&home, &mut |p| {
            let _ = tx.send(*p);
        })?;
        // Hooks that waited on the previous daemon are gone: their
        // questions are withdrawn (and kept, as every question is).
        let gone = store.withdraw_hook_questions_on_start()?;
        anyhow::Ok((store, gone))
    });
    // Ends when the blocking side drops its sender, as it returns.
    while let Some(p) = rx.recv().await {
        progress(&p);
    }
    opening
        .await
        .map_err(|e| anyhow::anyhow!("opening the store failed: {e}"))?
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
    /// the drafts sweeper, the working sweeper and the price catalog's
    /// refresh. Runs on the caller's Tokio runtime, the store's opening on
    /// one of its blocking threads, so a long migration leaves the
    /// runtime's workers free.
    ///
    /// # Errors
    /// When the store does not open or the state cannot be assembled.
    pub async fn start(
        cfg: CoreConfig,
        progress: &mut (dyn FnMut(&clax_store::backfill::Progress) + Send),
    ) -> anyhow::Result<Core> {
        cfg.home.ensure_dirs()?;
        let (store, gone) = open_store(&cfg.home, progress).await?;
        let store = Arc::new(store);
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
            hosted: cfg.hosted,
            web: cfg.web,
            embed: cfg.embed,
        })?;
        state.clax_app = Arc::new(clax_app);
        state.feedback = match config.as_ref() {
            Ok(c) => feedback_of(c, &state),
            Err(_) => clax_http::state::InstanceFeedback::Off,
        };
        if cfg.quickstart {
            state.quickstart = clax_core::quickstart::Release::from_tree(&*state.web).map(Arc::new);
        }
        // The core's `net` keeps a shared disk cache in the home.
        state.net = Arc::new(clax_http::net_cache::daemon_net(cfg.home.root()));
        state.bot = Arc::new(local_bot(&state, cfg.catalog_url));
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
        // The registered surfaces' deadlines (`clax_http::bot_surfaces`).
        tasks.push(tokio::spawn(clax_http::bot_surfaces::run_timer(
            state.clone(),
        )));
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
        // began) before the store stops; meanwhile the followers' sessions
        // the linked sync held on remotes are ended there, within the same
        // wait.
        let journal = async {
            if let Some(stopped) = self.journal_stopped {
                let _ = tokio::time::timeout(JOURNAL_STOP_TIMEOUT, stopped).await;
            }
        };
        let followers = crate::linked::end_followers(&self.state, JOURNAL_STOP_TIMEOUT);
        let ((), ended) = futures::future::join(journal, followers).await;
        if !ended {
            tracing::warn!(
                "followers' sessions on remotes did not end in time; the remotes reap them"
            );
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

/// The instance's own feedback integration from `config`'s `[feedback]`
/// ([`crate::integrations::feedback`]): its queue, the service grant in
/// its `service_grant_file`, and its `origin` (by default a hosted core's
/// public origin, else the daemon's browser address). Without `[feedback]`
/// it is the instance's own ([`InstanceFeedback::Own`]: on for a hosted
/// core, off for any other). Off, and logged, when `[feedback]` does not
/// check or its grant file does not read; the core then serves on.
fn feedback_of(config: &clax_core::config::HomeConfig, s: &AppState) -> InstanceFeedback {
    let f = match config.feedback() {
        Ok(Some(f)) => f,
        Ok(None) => return InstanceFeedback::Own,
        Err(e) => {
            tracing::warn!(error = %e, "[feedback] is off");
            return InstanceFeedback::Off;
        }
    };
    let instance = s
        .hosted
        .as_ref()
        .map_or_else(|| s.browser_base.clone(), |h| h.origin.clone());
    let origin = f.origin.clone().unwrap_or_else(|| instance.clone());
    let grant = match std::fs::read_to_string(&f.service_grant_file) {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(file = %f.service_grant_file.display(), error = %e, "[feedback] is off: its service_grant_file does not read");
            return InstanceFeedback::Off;
        }
    };
    if let Some(mode) = loose_mode(&f.service_grant_file) {
        tracing::warn!(
            file = %f.service_grant_file.display(),
            mode = format!("{mode:o}"),
            "[feedback] service_grant_file is readable by others: chmod 600 it"
        );
    }
    match crate::integrations::feedback::integration(&f.queue, &instance, &origin, &grant) {
        Ok(i) => {
            tracing::info!(queue = %i.queue_id, origin = %i.origin, "feedback integration on");
            InstanceFeedback::Configured(Arc::new(i))
        }
        Err(e) => {
            let input = match e.input {
                crate::integrations::feedback::Input::Queue => "[feedback] queue".to_string(),
                crate::integrations::feedback::Input::Grant => {
                    f.service_grant_file.display().to_string()
                }
            };
            tracing::warn!(input, error = %e, "[feedback] is off");
            InstanceFeedback::Off
        }
    }
}

/// The permission bits of the file at `path` when its group or others may
/// read or write it; `None` when they may not, or on a platform without
/// such bits.
fn loose_mode(path: &std::path::Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        (mode & 0o066 != 0).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// ClaxBot's state for a core starting on `s` (spec
/// 2026-10-21-claxbot-design §8): the `[bot]` settings the configuration
/// in effect sets, laid over the stored ones (an invalid `[bot]` turns
/// ClaxBot off), the home's key-encryption key (`<home>/secrets/kek`,
/// made on first use; none when it cannot be read, so no key is stored),
/// and the instance key in `[bot] api_key_env`'s variable, which takes
/// precedence over a stored one. A hosted core reads no `[bot]` and no
/// such variable. Either takes its price catalog's URL from `catalog_url`
/// when given, else from `[bot] price_catalog_url`
/// ([`crate::catalog::url_of`]). Both web operations are configured from
/// the start in the test mode of [`crate::web_source::fixtures`];
/// otherwise the run driver gives the daemon its own source
/// ([`crate::bot::driver`]), which fetches, and searches while the
/// instance holds a Cloudflare account ID and token
/// ([`crate::bot::web_configured`]). `[bot.web]` is laid over the stored
/// `web.*` settings as the rest of `[bot]` is ([`web_overrides`]).
fn local_bot(s: &AppState, catalog_url: Option<Option<String>>) -> clax_http::bot::Bot {
    let catalog_url = catalog_url.unwrap_or_else(|| {
        // A hosted core reads it from the file and the environment too,
        // though no other `[bot]` key.
        let read = match &s.settings.running {
            Some(c) => Some(c.clone()),
            None => clax_core::config::HomeConfig::load(s.home.root()).ok(),
        };
        crate::catalog::url_of(read.as_ref())
    });
    let mut bot = clax_http::bot::Bot {
        catalog_url,
        web_configured: if crate::web_source::fixtures().is_some() {
            clax_bot::web::WebShape::BOTH
        } else {
            clax_bot::web::WebShape::default()
        },
        ..Default::default()
    };
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
    let (ov, enabled) = bot_overrides(cfg);
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

/// The settings `cfg`'s `[bot]` sets, `[bot.web]` included, as overrides
/// of the stored ones, and whether ClaxBot is on
/// ([`clax_http::bot::configured`]). `[bot.web]` is laid over whenever
/// `[bot]` checks, as the rest of `[bot]` is, whether or not it turns
/// ClaxBot on.
pub fn bot_overrides(
    cfg: &clax_core::config::HomeConfig,
) -> (clax_store::bot_settings::Overrides, bool) {
    let (mut ov, enabled) = clax_http::bot::configured(cfg);
    if cfg.bot().is_ok() {
        web_overrides(cfg, &mut ov);
    }
    (ov, enabled)
}

/// The `web.*` settings `cfg`'s `[bot.web]` sets, from the file or the
/// environment, added to `ov` as [`clax_http::bot::configured`] adds the
/// rest of `[bot]`: each `bot.web.<key>` overrides the stored setting
/// `web.<key>`. The Cloudflare account ID is the admin pages' alone.
fn web_overrides(
    cfg: &clax_core::config::HomeConfig,
    ov: &mut clax_store::bot_settings::Overrides,
) {
    use clax_store::bot_settings::Setting;
    for &setting in Setting::ALL {
        if !setting.key().starts_with("web.") || setting == Setting::WebCloudflareAccountId {
            continue;
        }
        let full = format!("bot.{}", setting.key());
        let source = cfg.source(&full);
        if source == clax_core::config::Source::Default {
            continue;
        }
        let Some(v) = cfg.value(&full) else { continue };
        let Ok(v) = v.deserialize::<serde_json::Value>() else {
            continue;
        };
        if let Err(e) = ov.set(setting, &v, source.label()) {
            tracing::warn!("[bot.web] {} is not taken: {e}", setting.key());
        }
    }
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

/// One wake-up of the core's pruner: prune passes one after another
/// while each leaves more, [`PRUNE_PASSES_PER_WAKE`] at the most and none
/// begun [`PRUNE_WAKE_BUDGET`] after the first by the store's clock,
/// logged. How long until the next wake-up: [`PRUNE_AGAIN`] while the
/// last pass left more, else [`DRAFT_SWEEP_INTERVAL`].
fn prune_wake(st: &Store) -> Duration {
    prune_wake_within(st, PRUNE_PASSES_PER_WAKE, PRUNE_WAKE_BUDGET)
}

/// [`prune_wake`] with `passes` and `budget` for its bounds.
fn prune_wake_within(st: &Store, passes: usize, budget: Duration) -> Duration {
    let until = st.now_utc() + chrono::Duration::from_std(budget).unwrap_or(chrono::Duration::MAX);
    match st.prune_ended_runs_while(passes, Some(until)) {
        Ok((p, passes)) => {
            if p != clax_store::bot_runs::Pruned::default() {
                tracing::info!(
                    passes,
                    calls = p.calls,
                    pages = p.pages,
                    draft_files = p.draft_files,
                    pending_calls = p.pending_calls,
                    run_ends = p.run_ends,
                    messages = p.messages,
                    contents = p.contents,
                    registrations = p.registrations,
                    more = p.more,
                    "pruned ended ClaxBot runs"
                );
            }
            if p.more {
                PRUNE_AGAIN
            } else {
                DRAFT_SWEEP_INTERVAL
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "ClaxBot run prune failed");
            DRAFT_SWEEP_INTERVAL
        }
    }
}

/// The background tasks a core runs: the session reaper, the grant
/// sweeper, the planner statistics refresh, the drafts sweeper, the
/// pruner of ended ClaxBot runs, the working sweeper and the price
/// catalog's refresh.
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
                Ok(r) => {
                    tracing::info!(count = r.ended.len(), "ended idle sessions");
                    // A followed remote artifact's session there ends with
                    // its local session.
                    reap_state.linked.kick(None);
                }
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

    // Ended ClaxBot runs' rows past their keep: at start, then every
    // DRAFT_SWEEP_INTERVAL, or every PRUNE_AGAIN while a wake-up leaves
    // more.
    let prune_store = state.store.clone();
    let pruner = tokio::spawn(async move {
        loop {
            let next = prune_store
                .call(|st| Ok(prune_wake(st)))
                .await
                .unwrap_or(DRAFT_SWEEP_INTERVAL);
            tokio::time::sleep(next).await;
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

    // The price catalog, once its cache is a day old: looked at on start
    // and every hour after.
    let catalog = crate::catalog::spawn_refresher(state.clone(), crate::catalog::CHECK_EVERY);

    vec![
        reaper,
        grant_sweeper,
        optimizer,
        drafts_sweeper,
        pruner,
        sweeper,
        catalog,
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn bot_web_is_laid_over_the_stored_web_settings() {
        use clax_store::bot_settings::{Overrides, Setting};
        let cfg = clax_core::config::HomeConfig::from_text(
            std::path::PathBuf::from("/h/config.toml"),
            "[bot.web]\nmax_fetches_per_run = 3\n[bot.web.search]\nprovider = \"linkup\"\n[bot.web.fetch]\nallowlist = [\"*.example.com\"]\n",
            [("CLAX_BOT__WEB__SEARCH__ENABLED".to_string(), "true".to_string())],
        )
        .unwrap();
        let mut ov = Overrides::default();
        super::web_overrides(&cfg, &mut ov);
        let mut want = Overrides::default();
        want.set(Setting::WebMaxFetchesPerRun, &serde_json::json!(3), "file")
            .unwrap();
        want.set(
            Setting::WebSearchProvider,
            &serde_json::json!("linkup"),
            "file",
        )
        .unwrap();
        want.set(
            Setting::WebFetchAllowlist,
            &serde_json::json!(["*.example.com"]),
            "file",
        )
        .unwrap();
        want.set(
            Setting::WebSearchEnabled,
            &serde_json::json!(true),
            "env:CLAX_BOT__WEB__SEARCH__ENABLED",
        )
        .unwrap();
        assert_eq!(ov, want);
        // ClaxBot off keeps the overlay, as it keeps the rest of [bot].
        let off = clax_core::config::HomeConfig::from_text(
            std::path::PathBuf::from("/h/config.toml"),
            "[bot]\nenabled = false\n[bot.web.search]\nlimit = 3\n",
            [],
        )
        .unwrap();
        let (ov, enabled) = super::bot_overrides(&off);
        assert!(!enabled);
        let mut want = Overrides::default();
        want.set(Setting::WebSearchLimit, &serde_json::json!(3), "file")
            .unwrap();
        assert_eq!(ov, want);
        // A value the file may set, the store takes.
        let cfg = clax_core::config::HomeConfig::from_text(
            std::path::PathBuf::from("/h/config.toml"),
            "[bot.web.search]\ngateway = \"\"\nprovider = \" exa \"\n[bot.web.fetch]\nurl_rule = \"any_public \"\n",
            [],
        )
        .unwrap();
        assert!(cfg.bot().is_ok());
        let mut ov = Overrides::default();
        super::web_overrides(&cfg, &mut ov);
        let mut want = Overrides::default();
        want.set(Setting::WebSearchGateway, &serde_json::json!(""), "file")
            .unwrap();
        want.set(
            Setting::WebSearchProvider,
            &serde_json::json!("exa"),
            "file",
        )
        .unwrap();
        want.set(
            Setting::WebFetchUrlRule,
            &serde_json::json!("any_public"),
            "file",
        )
        .unwrap();
        assert_eq!(ov, want);
    }

    /// The pruner goes on while a pass leaves more, within its bounds,
    /// and wakes again soon when it stops with more left.
    #[test]
    fn the_pruner_drains_a_backlog_by_wake_ups_and_wakes_again_soon_while_it_has_more() {
        use clax_store::bot_runs::{PRUNE_RUNS_PER_TX, PRUNE_TXS_PER_PASS};
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("clax"));
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-21T10:00:00Z"));
        let st = clax_store::Store::open_with_clock(&home, clock).unwrap();
        let runs = PRUNE_RUNS_PER_TX * PRUNE_TXS_PER_PASS * 2 + 5;
        rusqlite::Connection::open(home.db_path())
            .unwrap()
            .execute_batch(&format!(
                "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                        model, key_source, base_version, created_at, updated_at, ended_at)
                 WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < {runs})
                 SELECT printf('01JC%022d', i), 'a', 'u', 'thread', '[]', 'finished',
                        'claude-sonnet-5-5', 'instance', 1, '2026-10-01T00:00:00.000Z',
                        '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z'
                   FROM n;
                 INSERT INTO bot_calls (run_id, step_n, seq, attempt, op, args_hash, ok,
                        result_json, at)
                 SELECT id, 1, 0, 1, 'threads.read', 'h', 1, '{{}}', '2026-10-01T00:00:00.000Z'
                   FROM bot_runs;"
            ))
            .unwrap();
        let held = || -> usize {
            rusqlite::Connection::open(home.db_path())
                .unwrap()
                .query_row("SELECT count(*) FROM bot_calls", [], |r| r.get(0))
                .unwrap()
        };
        // One pass a wake-up: it left more, and wakes again soon.
        assert_eq!(
            super::prune_wake_within(&st, 1, super::PRUNE_WAKE_BUDGET),
            super::PRUNE_AGAIN
        );
        assert_eq!(held(), runs - PRUNE_RUNS_PER_TX * PRUNE_TXS_PER_PASS);
        // A wake-up with room goes on until nothing is left, and sleeps
        // until the next interval.
        assert_eq!(super::prune_wake(&st), super::DRAFT_SWEEP_INTERVAL);
        assert_eq!(held(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_grant_file_others_may_read_is_loose() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("g.json");
        std::fs::write(&p, "{}").unwrap();
        for (mode, loose) in [
            (0o600, false),
            (0o400, false),
            (0o640, true),
            (0o604, true),
            (0o620, true),
        ] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(super::loose_mode(&p).is_some(), loose, "{mode:o}");
        }
        assert_eq!(super::loose_mode(&dir.path().join("none")), None);
    }
}
