//! Daemon discovery file, exclusive start lock, port selection, and shutdown.

use crate::state::AppState;
use clax_core::{EventBus, Home, Store};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
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
    // SAFETY: kill with signal 0 only probes for existence.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
    /// Blocks until the lock is held. A wait interrupted by a signal (`EINTR`) is
    /// retried; any other `flock` failure is returned.
    pub fn acquire(home: &Home) -> io::Result<DaemonLock> {
        let f = File::create(home.daemon_lock())?;
        loop {
            // SAFETY: flock on an owned, open descriptor.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(DaemonLock(f));
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// Takes the lock without waiting: `Ok(None)` when another open descriptor
    /// holds it (`EWOULDBLOCK`, which equals `EAGAIN` on Linux and macOS).
    pub fn try_acquire(home: &Home) -> io::Result<Option<DaemonLock>> {
        let f = File::create(home.daemon_lock())?;
        // SAFETY: as above, non-blocking.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(None)
            } else {
                Err(e)
            };
        }
        Ok(Some(DaemonLock(f)))
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
    let listener = bind_first_free(cfg.bind, cfg.port).await?;
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.home)?);
    let reaper_store = store.clone();
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
    };
    tracing::info!(codex = ?state.codex.bin, source = ?state.codex.source, "codex push");
    tracing::info!(provider = ?state.sample.provider_name(), "sample provider");
    let fctx = state.feedback_ctx();
    let (state_working, state_events) = (state.working.clone(), state.events.clone());
    let state_presence = state.presence.clone();
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
            let store = reaper_store.clone();
            let ctx = fctx.clone();
            let reaped = tokio::task::spawn_blocking(move || {
                let r = store.reap_sessions(SESSION_IDLE, &pid_alive)?;
                crate::feedback::apply(&ctx, &store, &r.touched);
                for id in &r.ended {
                    ctx.waiters.forget(id);
                    crate::working::announce(
                        &ctx.events,
                        &ctx.working,
                        &ctx.working.end_session(id),
                    );
                    ctx.followers.forget(id);
                }
                Ok::<_, clax_core::CoreError>(r)
            })
            .await;
            match reaped {
                Ok(Ok(r)) if r.ended.is_empty() => {}
                Ok(Ok(r)) => tracing::info!(count = r.ended.len(), "ended idle sessions"),
                Ok(Err(e)) => tracing::warn!(error = %e, "session reaper failed"),
                Err(e) => tracing::warn!(error = %e, "session reaper task failed"),
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
        }
    });

    let (fired_tx, fired_rx) = oneshot::channel::<()>();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let server = axum::serve(
        listener,
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
    sweeper.abort();
    if read_daemon_info(&cfg.home).map(|i| i.pid) == Some(std::process::id()) {
        remove_daemon_info(&cfg.home);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
