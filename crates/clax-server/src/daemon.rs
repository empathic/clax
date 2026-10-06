//! Daemon discovery file, exclusive start lock, port selection, and shutdown.

use crate::state::AppState;
use clax_core::{EventBus, Home, Store};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

pub const DEFAULT_PORT: u16 = 7480;
pub const PORT_ATTEMPTS: u16 = 21;
/// A session unseen for this long, with no live process, is ended by the reaper.
const SESSION_IDLE: Duration = Duration::from_secs(Store::SESSION_IDLE_SECS);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Discovery record written to `daemon.json` (mode 0600) once at daemon startup;
/// clients read it to find the port and bearer token of the running daemon.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DaemonInfo {
    /// TCP port the daemon listens on.
    pub port: u16,
    /// Process ID of the daemon.
    pub pid: u32,
    /// Bearer token required by write routes.
    pub token: String,
    /// RFC 3339 UTC start time.
    pub started_at: String,
    /// Bind address, as text (normally `127.0.0.1`).
    pub bind: String,
    /// Version of the daemon binary.
    pub version: String,
    /// Canonical path of the daemon's executable, so a client can tell which
    /// build serves (a `just dev` build and an installed one can share a
    /// version). Absent in records written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

/// Reads `daemon.json`; `None` when it is missing, unreadable, or not valid JSON.
pub fn read_daemon_info(home: &Home) -> Option<DaemonInfo> {
    let text = std::fs::read_to_string(home.daemon_json()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Atomically replaces `daemon.json` with `info`: writes a 0600 temp file in the
/// home root, syncs it, and renames it over the old file, so readers never see a
/// partial record.
pub fn write_daemon_info(home: &Home, info: &DaemonInfo) -> io::Result<()> {
    use io::Write;
    let tmp = home
        .root()
        .join(format!("daemon.json.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(info).expect("serialisable"))?;
    f.sync_all()?;
    std::fs::rename(tmp, home.daemon_json())
}

/// Removes `daemon.json`, ignoring errors (including a file that is already gone).
pub fn remove_daemon_info(home: &Home) {
    let _ = std::fs::remove_file(home.daemon_json());
}

/// True when a process with `pid` exists. Probes with `kill(pid, 0)`: success or
/// `EPERM` (the process exists but belongs to another user) both count as alive;
/// `ESRCH` and out-of-range pids (0, or beyond `i32::MAX`) do not.
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
        Ok(()) | Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// The host part of a URL that reaches a daemon bound to `bind`: an unspecified
/// address (`0.0.0.0`, `::`) maps to the same-family loopback, a specific
/// address is used as-is, and IPv6 is bracketed. Text that is not an IP address
/// is returned unchanged.
pub fn probe_host(bind: &str) -> String {
    match bind.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
        Ok(IpAddr::V6(ip)) if ip.is_unspecified() => format!("[{}]", Ipv6Addr::LOCALHOST),
        Ok(IpAddr::V4(ip)) => ip.to_string(),
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        Err(_) => bind.to_string(),
    }
}

/// The host a browser on this machine should use: `localhost` for loopback
/// binds, otherwise the [`probe_host`] address.
pub fn browser_host(bind: &str) -> String {
    match bind.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => "localhost".to_string(),
        _ => probe_host(bind),
    }
}

pub fn generate_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Exclusive advisory lock on `daemon.lock`, released when dropped
/// (the open descriptor holds the lock).
pub struct DaemonLock(#[allow(dead_code)] File);

impl DaemonLock {
    /// Blocks until the lock is held (`flock(LOCK_EX)`, through
    /// [`File::lock`]). A wait interrupted by a signal (`EINTR`) is retried;
    /// any other failure is returned.
    pub fn acquire(home: &Home) -> io::Result<DaemonLock> {
        let f = File::create(home.daemon_lock())?;
        retry_interrupted(|| f.lock())?;
        Ok(DaemonLock(f))
    }

    /// Takes the lock without waiting (`flock(LOCK_EX | LOCK_NB)`, through
    /// [`File::try_lock`]): `Ok(None)` when another open descriptor holds it
    /// (`EWOULDBLOCK`, which equals `EAGAIN` on Linux and macOS).
    pub fn try_acquire(home: &Home) -> io::Result<Option<DaemonLock>> {
        let f = File::create(home.daemon_lock())?;
        match f.try_lock() {
            Ok(()) => Ok(Some(DaemonLock(f))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }
}

/// Runs `op` until it returns anything but an [`io::ErrorKind::Interrupted`]
/// error.
pub fn retry_interrupted<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match op() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            out => return out,
        }
    }
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

/// Socket options for each accepted connection: no Nagle delay, so a small
/// event leaves at once, and TCP keep-alive probes after a minute idle, so a
/// peer that vanished without closing (a sleeping laptop on the LAN) frees
/// its connection and its stream.
pub fn tune_connection(tcp: &mut tokio::net::TcpStream) {
    if let Err(e) = tcp.set_nodelay(true) {
        tracing::debug!(error = %e, "TCP_NODELAY");
    }
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(10));
    if let Err(e) = socket2::SockRef::from(&*tcp).set_tcp_keepalive(&ka) {
        tracing::debug!(error = %e, "TCP keep-alive");
    }
}

/// Parameters for [`serve`].
pub struct ServeConfig {
    /// Clax home the daemon serves from and writes `daemon.json` into.
    pub home: Home,
    /// Address to listen on.
    pub bind: IpAddr,
    /// First port tried; up to [`PORT_ATTEMPTS`] consecutive ports are tried
    /// when it is busy. 0 lets the OS choose.
    pub port: u16,
    /// Version reported by `/healthz` and recorded in `daemon.json`.
    pub version: &'static str,
    /// How often the stale watcher checks `daemon.json` (30 s in production).
    pub stale_check_interval: std::time::Duration,
    /// How often idle sessions with dead processes are ended (60 s in production).
    pub reap_interval: std::time::Duration,
    /// Where the daemon's `codex` is, for Codex tier 5 (`clax serve` uses
    /// [`crate::push::CodexPush::from_env`] on its own environment).
    pub codex: crate::push::CodexPush,
    /// The `sample` capability's provider and settings (`clax serve` uses
    /// [`crate::sample::Sampler::from_home`] on its own environment).
    pub sample: Arc<crate::sample::Sampler>,
}

async fn bind_first_free(bind: IpAddr, start: u16) -> io::Result<tokio::net::TcpListener> {
    let mut last = None;
    for port in start..start.saturating_add(PORT_ATTEMPTS) {
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
/// was deleted), or a termination signal arrives.
///
/// `daemon.json` is written exactly once, at startup. That write-once invariant is what
/// makes the stale watcher race-free: a different live pid in the file can only mean
/// another daemon started after this one.
pub async fn serve(
    cfg: ServeConfig,
    ready: Option<oneshot::Sender<DaemonInfo>>,
) -> anyhow::Result<()> {
    cfg.home.ensure_dirs()?;
    match raise_open_file_limit() {
        Ok((before, after)) => tracing::info!(before, after, "open file limit"),
        Err(e) => tracing::warn!(error = %e, "could not read or raise the open file limit"),
    }
    let listener = bind_first_free(cfg.bind, cfg.port).await?;
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.home)?);
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
    let started_at = Store::now();
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
    };
    write_daemon_info(&cfg.home, &info)?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let events_shutdown = shutdown_tx.subscribe();
    let live_ids = Arc::new(crate::live::LiveIds::load(&store)?);
    let ext_creds = Arc::new(crate::extension::Credentials::load(&store)?);
    let state = AppState {
        store,
        home: cfg.home.clone(),
        token,
        events: EventBus::new(),
        started_at,
        version: cfg.version,
        shutdown: events_shutdown,
        wrap_cache: Arc::new(crate::wrap_cache::WrapCache::new(
            crate::wrap_cache::DEFAULT_MAX_BYTES,
        )),
        request_timeout: Duration::from_secs(30),
        publish_timeout: Duration::from_secs(120),
        sse_keep_alive: Duration::from_secs(15),
        self_base: format!("http://{}:{port}", probe_host(&info.bind)),
        browser_base: format!("http://{}:{port}", browser_host(&info.bind)),
        feedback_waiters: Arc::new(Default::default()),
        followers: Arc::new(Default::default()),
        codex: Arc::new(cfg.codex.clone()),
        working: Arc::new(clax_core::working::Working::new(Arc::new(
            clax_core::working::SystemClock,
        ))),
        presence: Arc::new(clax_core::presence::Presence::new(Arc::new(
            clax_core::working::SystemClock,
        ))),
        rooms: Arc::new(crate::room::Rooms::default()),
        sample: cfg.sample.clone(),
        stream: crate::stream::Hub::new(live_ids.clone()),
        live_ids,
        ext_creds,
        extension_id: clax_core::extension::extension_id_in_effect(cfg.home.root()),
        questions: Arc::new(Default::default()),
        question_grace: Duration::from_secs(5),
        terminal_after_s,
    };
    state.stream.listen(&state.events);
    tracing::info!(codex = ?state.codex.bin, source = ?state.codex.source, "codex push");
    tracing::info!(provider = ?state.sample.provider_name(), "sample provider");
    let fctx = state.feedback_ctx();
    let reap_state = state.clone();
    let (state_working, state_events) = (state.working.clone(), state.events.clone());
    let state_presence = state.presence.clone();
    let state_stream = state.stream.clone();
    let app = crate::build_router_with_shutdown(state, shutdown_tx.clone());
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
        loop {
            tokio::time::sleep(reap_interval).await;
            let ctx = fctx.clone();
            let st = reap_state.clone();
            let reaped = reaper_store
                .call(move |store| {
                    let r = store.reap_sessions(SESSION_IDLE, &pid_alive)?;
                    crate::feedback::apply(&ctx, store, &r.touched);
                    crate::questions::announce_ids(&st, store, &r.withdrawn_questions);
                    for id in &r.ended {
                        ctx.waiters.forget(id);
                        crate::working::announce(
                            &ctx.events,
                            &ctx.working,
                            &ctx.working.end_session(id),
                        );
                        ctx.followers.forget(id);
                    }
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

    let optimizer = tokio::spawn(async move {
        let mut every = tokio::time::interval(clax_core::store::OPTIMIZE_INTERVAL);
        every.tick().await;
        loop {
            every.tick().await;
            if let Err(e) = optimize_store.call(|store| store.optimize()).await {
                tracing::warn!(error = %e, "planner statistics refresh failed");
            }
        }
    });

    let (sweep_working, sweep_events) = (state_working, state_events);
    let sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(crate::working::SWEEP_INTERVAL);
        loop {
            every.tick().await;
            crate::working::sweep_and_announce(&sweep_working, &sweep_events);
            crate::presence::sweep_and_announce(&state_presence, &sweep_events);
            state_stream.sweep();
        }
    });

    let (fired_tx, fired_rx) = oneshot::channel::<()>();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let server = axum::serve(
        crate::auth::TunedListener(listener),
        app.into_make_service_with_connect_info::<crate::auth::Conn>(),
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
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
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
    optimizer.abort();
    sweeper.abort();
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
    fn the_open_file_limit_ends_at_or_above_where_it_began() {
        let (before, after) = raise_open_file_limit().unwrap();
        assert!(after >= before);
        let (again, _) = raise_open_file_limit().unwrap();
        assert_eq!(again, after);
    }

    #[test]
    fn probe_host_maps_unspecified_to_same_family_loopback() {
        assert_eq!(probe_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(probe_host("::"), "[::1]");
    }

    #[test]
    fn probe_host_uses_specific_addresses_as_is_and_brackets_ipv6() {
        assert_eq!(probe_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(probe_host("192.168.1.20"), "192.168.1.20");
        assert_eq!(probe_host("::1"), "[::1]");
        assert_eq!(probe_host("fe80::1"), "[fe80::1]");
        assert_eq!(probe_host("mymac.local"), "mymac.local");
    }

    #[test]
    fn browser_host_is_localhost_only_for_loopback_binds() {
        assert_eq!(browser_host("127.0.0.1"), "localhost");
        assert_eq!(browser_host("::1"), "localhost");
        assert_eq!(browser_host("192.168.1.20"), "192.168.1.20");
        assert_eq!(browser_host("fe80::1"), "[fe80::1]");
        assert_eq!(browser_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(browser_host("::"), "[::1]");
    }
}
