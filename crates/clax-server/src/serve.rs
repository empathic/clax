//! The daemon process: serving a home, its start record, the idle reaper,
//! the open-file limit, and shutdown. The discovery files clients read are
//! `clax_core::daemon`.

use clax_core::Home;
use clax_core::daemon::DaemonInfo;
use clax_core::daemon::StartingInfo;
use clax_core::daemon::browser_host;
use clax_core::daemon::pid_alive;
use clax_core::daemon::port_held;
use clax_core::daemon::port_range;
use clax_core::daemon::probe_host;
use clax_core::daemon::read_daemon_info;
use clax_core::daemon::remove_daemon_info;
use clax_core::daemon::write_daemon_info;
use clax_core::daemon::write_starting_info;
use clax_http::state::AppState;
use clax_store::{Store, TrimReport};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

/// A session unseen for this long, with no live process, is ended by the reaper.
const SESSION_IDLE: Duration = Duration::from_secs(Store::SESSION_IDLE_SECS);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the daemon deletes drafts older than
/// [`clax_store::drafts::DRAFT_MAX_AGE`] (and once at start).
const DRAFT_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// How long shutdown waits for the journal appender to close its segment.
/// The appender stops as shutdown begins, beside the connection drain
/// ([`DRAIN_TIMEOUT`]), and catches up for at most 0.5 s; a wedged one is
/// left on its own thread, which the process exit ends. The whole shutdown
/// stays within 6.5 s, inside the 7 s a replacing client waits before it
/// sends SIGTERM; what the journal does not write, the next start writes
/// from the table.
const JOURNAL_STOP_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often grants due to expire are expired and the nonce caches swept.
const GRANT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Publishes this daemon's start in `starting.json` until dropped: the
/// phase and backfill progress it is given, and a heartbeat every second
/// from a thread of its own.
pub struct Starting {
    home: Home,
    info: std::sync::Arc<std::sync::Mutex<StartingInfo>>,
    stop: Option<std::sync::mpsc::Sender<()>>,
    beat: Option<std::thread::JoinHandle<()>>,
    written: Instant,
}

impl Starting {
    /// Writes `starting.json` (phase `opening`) and starts the heartbeat.
    pub fn begin(home: &Home) -> Starting {
        let info = std::sync::Arc::new(std::sync::Mutex::new(StartingInfo {
            pid: std::process::id(),
            phase: "opening".into(),
            done: 0,
            total: 0,
            bytes: 0,
            heartbeat_at: clax_core::time::stamp_now(),
        }));
        let write = |home: &Home, info: &StartingInfo| {
            if let Err(e) = write_starting_info(home, info) {
                tracing::warn!(error = %e, "could not write starting.json");
            }
        };
        write(home, &info.lock().unwrap_or_else(|e| e.into_inner()));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (h, i) = (home.clone(), info.clone());
        let beat = std::thread::spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(Duration::from_secs(1))
            {
                let mut now = i.lock().unwrap_or_else(|e| e.into_inner());
                now.heartbeat_at = clax_core::time::stamp_now();
                write(&h, &now);
            }
        });
        Starting {
            home: home.clone(),
            info,
            stop: Some(stop),
            beat: Some(beat),
            written: Instant::now(),
        }
    }

    /// Records the backfill's progress, writing it at most every 100 ms
    /// within a phase.
    pub fn progress(&mut self, p: &clax_store::backfill::Progress) {
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        let phase = p.phase.label().to_string();
        let changed = info.phase != phase;
        info.phase = phase;
        (info.done, info.total, info.bytes) = (p.done, p.total, p.bytes);
        if changed || self.written.elapsed() >= Duration::from_millis(100) {
            if let Err(e) = write_starting_info(&self.home, &info) {
                tracing::warn!(error = %e, "could not write starting.json");
            }
            self.written = Instant::now();
        }
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(b) = self.beat.take() {
            let _ = b.join();
        }
        let _ = std::fs::remove_file(self.home.starting_json());
    }
}

/// One reaper pass: ends the sessions unseen for `idle` whose process is
/// gone ([`Store::reap_sessions`]), hands on the feedback they released,
/// and ends their working records as `system:ttl` (reason `session_end`,
/// naming the agent in `for_actor`).
///
/// # Errors
/// When the store fails.
pub fn reap_idle(
    ctx: &clax_realtime::feedback::FeedbackCtx,
    store: &Store,
    idle: Duration,
    pid_alive: &dyn Fn(u32) -> bool,
) -> clax_core::Result<clax_store::Reaped> {
    let r = store.reap_sessions(idle, pid_alive)?;
    let ttl = clax_core::audit::AuditCtx::system(clax_core::audit::SystemReason::Ttl);
    clax_realtime::feedback::sessions_stop(ctx, store, &ttl, &r.touched, &r.ended);
    Ok(r)
}

/// Whether the machine slept between two reaper passes: wall time `wall`
/// passed while monotonic time (which stands still in sleep) passed only
/// `mono`, more than [`SLEEP_SLACK`] apart.
pub fn slept(wall: Duration, mono: Duration) -> bool {
    wall > mono + SLEEP_SLACK
}

/// How much more wall time than monotonic time between reaper passes
/// counts as the machine having slept.
pub const SLEEP_SLACK: Duration = Duration::from_secs(60);

pub fn generate_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Most descriptors the daemon asks for: far more connections than one
/// daemon serves, and a finite limit for the processes it spawns, which
/// inherit it (an unlimited one, which macOS allows, breaks programs that
/// close every descriptor up to the limit).
pub const MAX_OPEN_FILES: u64 = 1 << 20;

/// Raises the soft limit on open descriptors to the hard limit, at most
/// [`MAX_OPEN_FILES`], so one daemon can hold thousands of connections.
/// Where the kernel refuses that, the highest of a few lower steps it
/// accepts is taken. Returns the soft limit before and after.
///
/// # Errors
/// When the limit cannot be read.
pub fn raise_open_file_limit() -> io::Result<(u64, u64)> {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).map_err(io::Error::from)?;
    let top = hard.min(MAX_OPEN_FILES);
    if soft >= top {
        return Ok((soft, soft));
    }
    let steps = [1 << 19, 245_760, 1 << 17, 1 << 16, 24_576, 10_240];
    let tries = std::iter::once(top).chain(steps.into_iter().filter(|s| *s < top));
    for want in tries.filter(|w| *w > soft) {
        if setrlimit(Resource::RLIMIT_NOFILE, want, hard).is_ok() {
            let (now, _) = getrlimit(Resource::RLIMIT_NOFILE).map_err(io::Error::from)?;
            return Ok((soft, now));
        }
    }
    Ok((soft, soft))
}

/// Parameters for [`serve`].
pub struct ServeConfig {
    /// Clax home the daemon serves from and writes `daemon.json` into.
    pub home: Home,
    /// Address to listen on.
    pub bind: IpAddr,
    /// First port tried; up to [`PORT_ATTEMPTS`] consecutive ports are tried
    /// when it is busy or another program holds it ([`port_held`]). 0 lets
    /// the OS choose.
    pub port: u16,
    /// Version reported by `/healthz` and recorded in `daemon.json`.
    pub version: &'static str,
    /// How often the stale watcher checks `daemon.json` (30 s in production).
    pub stale_check_interval: std::time::Duration,
    /// How often idle sessions with dead processes are ended (60 s in production).
    pub reap_interval: std::time::Duration,
    /// Where the daemon's `codex` is, for Codex tier 5 (`clax serve` uses
    /// [`clax_realtime::push::CodexPush::from_env`] on its own environment).
    pub codex: clax_realtime::push::CodexPush,
    /// The `sample` capability's provider and settings (`clax serve` uses
    /// [`clax_sample::Sampler::from_home`] on its own environment).
    pub sample: Arc<clax_sample::Sampler>,
    /// The web UI the daemon serves (`clax serve` passes the one the binary
    /// embeds, or a [`clax_core::embedded::DiskTree`] of `--web-dist`).
    pub web: Arc<dyn clax_core::embedded::FileTree>,
    /// Hosted mode ([`clax_http::hosted`]; `clax serve` uses
    /// [`clax_http::hosted::Hosted::from_config`] on the home's `[hosted]`), or
    /// `None` for the local daemon.
    pub hosted: Option<Arc<clax_http::hosted::Hosted>>,
    /// Serve nothing but `/healthz`, static files and content by ID without
    /// a credential ([`clax_http::state::AppState::require_credential`]).
    pub require_credential: bool,
    /// How the daemon is asked to stop. `Signals` is `clax serve`'s: SIGTERM,
    /// Ctrl-C and the shutdown route. `Embedded` is a daemon inside another
    /// app's process: only its control and the shutdown route, and no signal
    /// handler is installed, since the process's signals are the app's.
    pub stop: StopOn,
}

/// How the daemon is asked to stop. `Signals` is `clax serve`'s: SIGTERM,
/// Ctrl-C and the shutdown route. `Embedded` is a daemon inside another
/// app's process: only its control and the shutdown route, and no signal
/// handler is installed, since the process's signals are the app's.
pub enum StopOn {
    Signals,
    Embedded(EmbeddedControl),
}

/// What a host app holds to drive a daemon in its own process.
pub struct EmbeddedControl {
    /// `true` asks the daemon to stop, as SIGTERM does for `clax serve`;
    /// so does dropping the sender.
    pub stop: watch::Receiver<bool>,
    /// Each sender asks for one [`clax_store::Store::trim_memory`]; its
    /// report is sent back on it ([`TrimReport::default`] when the trim
    /// failed).
    pub trim: tokio::sync::mpsc::Receiver<std::sync::mpsc::Sender<TrimReport>>,
}

/// A stop asked of the daemon other than through the shutdown route.
type StopAsked = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// The future that completes when `stop` asks the daemon to stop. Under
/// [`StopOn::Signals`] the SIGTERM and SIGINT handlers are installed here,
/// at once, so a signal sent once the daemon is ready is never missed;
/// under [`StopOn::Embedded`] none is, and a task answering the control's
/// trim requests is started on `store`.
fn stop_asked(
    stop: StopOn,
    store: &Arc<Store>,
) -> io::Result<(StopAsked, Option<tokio::task::JoinHandle<()>>)> {
    use tokio::signal::unix::{SignalKind, signal};
    match stop {
        StopOn::Signals => {
            let mut term = signal(SignalKind::terminate())?;
            let mut int = signal(SignalKind::interrupt())?;
            let asked = async move {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = int.recv() => {}
                }
            };
            Ok((Box::pin(asked), None))
        }
        StopOn::Embedded(EmbeddedControl {
            stop: mut rx,
            mut trim,
        }) => {
            let store = store.clone();
            let trimmer = tokio::spawn(async move {
                while let Some(reply) = trim.recv().await {
                    let st = store.clone();
                    let report = match tokio::task::spawn_blocking(move || st.trim_memory()).await {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "memory trim failed");
                            TrimReport::default()
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "memory trim failed");
                            TrimReport::default()
                        }
                    };
                    tracing::info!(
                        wal_frames = report.wal_frames,
                        checkpointed = report.checkpointed,
                        "trimmed memory"
                    );
                    let _ = reply.send(report);
                }
            });
            let asked = async move {
                while !*rx.borrow_and_update() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            };
            Ok((Box::pin(asked), Some(trimmer)))
        }
    }
}

/// Binds the first port of [`port_range`]`(start)` that no other program
/// holds ([`port_held`]) and that binds; `start` 0 lets the OS choose.
async fn bind_first_free(bind: IpAddr, start: u16) -> io::Result<tokio::net::TcpListener> {
    if start == 0 {
        return tokio::net::TcpListener::bind(SocketAddr::new(bind, 0)).await;
    }
    let mut last = None;
    for port in port_range(start) {
        let held = tokio::task::spawn_blocking(move || port_held(port))
            .await
            .unwrap_or(false);
        if held {
            tracing::warn!(port, "another program holds the port; trying the next");
            last = Some(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("port {port} is held by another program"),
            ));
            continue;
        }
        match tokio::net::TcpListener::bind(SocketAddr::new(bind, port)).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no port")))
}

/// Runs the daemon until shutdown is requested, a newer daemon takes over,
/// `daemon.json` is missing on two consecutive stale checks (its home directory
/// was deleted), or [`ServeConfig::stop`] asks it to: a termination signal
/// under [`StopOn::Signals`], its control under [`StopOn::Embedded`].
///
/// `daemon.json` is written exactly once, at startup. That write-once invariant is what
/// makes the stale watcher race-free: a different live pid in the file can only mean
/// another daemon started after this one.
pub async fn serve(
    cfg: ServeConfig,
    ready: Option<oneshot::Sender<DaemonInfo>>,
) -> anyhow::Result<()> {
    cfg.home.ensure_dirs()?;
    // Until it serves, whoever started this daemon follows it in
    // starting.json: a first start after an upgrade may record the audit
    // backfill first.
    let mut starting = Starting::begin(&cfg.home);
    match raise_open_file_limit() {
        Ok((before, after)) => tracing::info!(before, after, "open file limit"),
        Err(e) => tracing::warn!(error = %e, "could not read or raise the open file limit"),
    }
    let listener = bind_first_free(cfg.bind, cfg.port).await?;
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open_reporting(&cfg.home, &mut |p| {
        starting.progress(p)
    })?);
    drop(starting);
    // Hooks that waited on the previous daemon are gone: their questions
    // are withdrawn (and kept, as every question is).
    let gone = store.withdraw_hook_questions_on_start()?;
    if !gone.is_empty() {
        tracing::info!(count = gone.len(), "withdrew mirrored questions left open");
    }
    let terminal_after_s = match clax_core::config::HomeConfig::load(cfg.home.root()) {
        Ok(c) => c.questions_terminal_after_s(),
        Err(e) => {
            tracing::warn!(error = %e, "config.toml unreadable; questions use the defaults");
            clax_core::config::TERMINAL_AFTER_S
        }
    };
    let reaper_store = store.clone();
    let optimize_store = store.clone();
    let drain_store = store.clone();
    let token = generate_token();
    let started_at = store.now();
    let info = DaemonInfo {
        port,
        pid: std::process::id(),
        token: token.clone(),
        started_at: started_at.clone(),
        bind: cfg.bind.to_string(),
        version: cfg.version.to_string(),
        exe: std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .ok()
            .map(|p| p.display().to_string()),
        commit: Some(clax_core::build_commit().to_string()),
    };
    write_daemon_info(&cfg.home, &info)?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let events_shutdown = shutdown_tx.subscribe();
    let mut state = AppState::assemble(clax_http::state::StateParts {
        store,
        home: cfg.home.clone(),
        token,
        require_credential: cfg.require_credential,
        started_at,
        version: cfg.version,
        shutdown: events_shutdown,
        self_base: format!("http://{}:{port}", probe_host(&info.bind)),
        browser_base: format!("http://{}:{port}", browser_host(&info.bind)),
        codex: cfg.codex.clone(),
        sample: cfg.sample.clone(),
        clock: Arc::new(clax_core::working::SystemClock),
        question_sleeper: Arc::new(clax_realtime::questions::TokioSleeper),
        agent_timer: Arc::new(clax_http::agent::TokioTimer::default()),
        extension_id: clax_core::extension::extension_id_in_effect(cfg.home.root()),
        terminal_after_s,
        bind: cfg.bind,
        port,
        hosted: cfg.hosted.clone(),
        web: cfg.web.clone(),
    })?;
    // The daemon's `net` keeps a shared disk cache in the home.
    state.net = Arc::new(clax_http::net_cache::daemon_net(cfg.home.root()));
    state.audit_wake.install(&state.store);
    // The appender recovers the journal and catches it up on its own
    // thread, so the daemon serves at once.
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
        let mut begun = shutdown_tx.subscribe();
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
    crate::start_listeners(&state);
    tracing::info!(codex = ?state.codex.bin, source = ?state.codex.source, "codex push");
    tracing::info!(provider = ?state.sample.provider_name(), "sample provider");
    let fctx = state.feedback_ctx();
    let reap_state = state.clone();
    let grant_state = state.clone();
    let sweep_state = state.clone();
    let (drafts_store, drafts_cache) = (state.store.clone(), state.wrap_cache.clone());
    let (asked, trimmer) = stop_asked(cfg.stop, &state.store)?;
    let app = crate::router::build_router_with_shutdown(state, shutdown_tx.clone());
    if let Some(tx) = ready {
        let _ = tx.send(info.clone());
    }
    tracing::info!(port, "clax daemon listening");

    let home = cfg.home.clone();
    let interval = cfg.stale_check_interval;
    let stale = tokio::spawn(async move {
        let mut missing = 0u32;
        loop {
            tokio::time::sleep(interval).await;
            match read_daemon_info(&home) {
                Some(other) => {
                    missing = 0;
                    if other.pid != std::process::id() && pid_alive(other.pid) {
                        tracing::warn!(
                            other = other.pid,
                            "another daemon owns daemon.json; exiting"
                        );
                        return;
                    }
                }
                None => {
                    missing += 1;
                    if missing >= 2 {
                        tracing::warn!("daemon.json is missing; exiting");
                        return;
                    }
                }
            }
        }
    });

    let reap_interval = cfg.reap_interval;
    let reaper = tokio::spawn(async move {
        let mut since = (std::time::SystemTime::now(), Instant::now());
        loop {
            tokio::time::sleep(reap_interval).await;
            let now = (std::time::SystemTime::now(), Instant::now());
            let woke = slept(
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
                    let r = reap_idle(&ctx, store, SESSION_IDLE, &pid_alive)?;
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

    // Drafts (spec 2026-10-12-flutter-app-round2-design §5.1) older than
    // their age go at start, the first tick, and every interval after.
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

    let sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(clax_realtime::working::SWEEP_INTERVAL);
        loop {
            every.tick().await;
            sweep_state.sweep().await;
        }
    });

    let (fired_tx, fired_rx) = oneshot::channel::<()>();
    let server = axum::serve(
        clax_http::auth::TunedListener(listener),
        app.into_make_service_with_connect_info::<clax_http::auth::Conn>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = async {
                while shutdown_rx.changed().await.is_ok() {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            } => {}
            _ = stale => {}
            _ = asked => {}
        }
        // Tell open SSE streams to end, and start the drain deadline.
        let _ = shutdown_tx.send(true);
        let _ = fired_tx.send(());
    });
    // Graceful shutdown waits for every open connection; bound the drain so a
    // lingering client cannot keep the daemon alive.
    let drain_deadline = async {
        if fired_rx.await.is_ok() {
            tokio::time::sleep(DRAIN_TIMEOUT).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        res = server => res?,
        _ = drain_deadline => tracing::warn!("connections did not drain in time; exiting"),
    }
    reaper.abort();
    grant_sweeper.abort();
    optimizer.abort();
    sweeper.abort();
    drafts_sweeper.abort();
    if let Some(t) = trimmer {
        t.abort();
    }
    // The journal has closed its segment (it began to as shutdown began)
    // before the store stops.
    if let Some(stopped) = journal_stopped {
        let _ = tokio::time::timeout(JOURNAL_STOP_TIMEOUT, stopped).await;
    }
    // Let queued store calls finish and the database threads stop.
    if let Err(e) = tokio::task::spawn_blocking(move || drain_store.shutdown()).await {
        tracing::warn!(error = %e, "store shutdown failed");
    }
    if read_daemon_info(&cfg.home).map(|i| i.pid) == Some(std::process::id()) {
        remove_daemon_info(&cfg.home);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_clock_jump_past_monotonic_time_is_a_sleep() {
        let m = Duration::from_secs(60);
        assert!(!slept(m, m), "an ordinary pass");
        assert!(!slept(m + SLEEP_SLACK, m), "within the slack");
        assert!(slept(Duration::from_secs(3600), m), "an hour asleep");
        assert!(!slept(Duration::ZERO, m), "a clock set back");
    }

    #[test]
    fn the_open_file_limit_ends_at_or_above_where_it_began() {
        let (before, after) = raise_open_file_limit().unwrap();
        assert!(after >= before);
        let (again, _) = raise_open_file_limit().unwrap();
        assert_eq!(again, after);
    }
}
