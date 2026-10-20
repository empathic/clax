//! The daemon process: serving a home, its start record, the idle reaper,
//! the open-file limit, and shutdown. The discovery files clients read are
//! `clax_core::daemon`.

use crate::core::{Core, CoreConfig};
use clax_core::Home;
use clax_core::daemon::DaemonInfo;
use clax_core::daemon::StartingInfo;
use clax_core::daemon::pid_alive;
use clax_core::daemon::port_held;
use clax_core::daemon::port_range;
use clax_core::daemon::read_daemon_info;
use clax_core::daemon::remove_daemon_info;
use clax_core::daemon::write_daemon_info;
use clax_core::daemon::write_starting_info;
use clax_store::{Store, TrimReport};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
    let core = Core::start(
        CoreConfig {
            home: cfg.home.clone(),
            version: cfg.version,
            reap_interval: cfg.reap_interval,
            codex: cfg.codex,
            sample: cfg.sample,
            web: cfg.web,
            hosted: cfg.hosted,
            require_credential: cfg.require_credential,
            bind: cfg.bind,
            port,
            embed: None,
        },
        &mut |p| starting.progress(p),
    )
    .await?;
    drop(starting);
    let state = core.state();
    let info = DaemonInfo {
        port,
        pid: std::process::id(),
        token: state.token.clone(),
        started_at: state.started_at.clone(),
        bind: cfg.bind.to_string(),
        version: cfg.version.to_string(),
        exe: std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .ok()
            .map(|p| p.display().to_string()),
        commit: Some(clax_core::build_commit().to_string()),
    };
    write_daemon_info(&cfg.home, &info)?;
    let (asked, trimmer) = stop_asked(cfg.stop, &state.store)?;
    let app = core.router();
    let mut shutdown_rx = core.shutdown_requested();
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

    let (fired_tx, fired_rx) = oneshot::channel::<()>();
    let begin = core.shutdown_handle();
    let server = axum::serve(
        clax_http::auth::TunedListener(listener),
        app.into_make_service_with_connect_info::<clax_http::auth::Conn>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = async {
                while !*shutdown_rx.borrow_and_update() {
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
            } => {}
            _ = stale => {}
            _ = asked => {}
        }
        // Tell open SSE streams to end, and start the drain deadline.
        let _ = begin.send(true);
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
    if let Some(t) = trimmer {
        t.abort();
    }
    core.stop().await;
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
