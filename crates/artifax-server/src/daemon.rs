//! Daemon discovery file, exclusive start lock, port selection, and shutdown.

use crate::state::AppState;
use artifax_core::{EventBus, Home, Store};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

pub const DEFAULT_PORT: u16 = 7480;
pub const PORT_ATTEMPTS: u16 = 21;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DaemonInfo {
    pub port: u16,
    pub pid: u32,
    pub token: String,
    pub started_at: String,
    pub bind: String,
    pub version: String,
}

pub fn read_daemon_info(home: &Home) -> Option<DaemonInfo> {
    let text = std::fs::read_to_string(home.daemon_json()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_daemon_info(home: &Home, info: &DaemonInfo) -> io::Result<()> {
    let tmp = home.root().join("daemon.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(info).expect("serialisable"))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, home.daemon_json())
}

pub fn remove_daemon_info(home: &Home) {
    let _ = std::fs::remove_file(home.daemon_json());
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only probes for existence.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
    pub fn acquire(home: &Home) -> io::Result<DaemonLock> {
        let f = File::create(home.daemon_lock())?;
        // SAFETY: flock on an owned, open descriptor.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(DaemonLock(f))
    }

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

pub struct ServeConfig {
    pub home: Home,
    pub bind: IpAddr,
    pub port: u16,
    pub version: &'static str,
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

pub async fn serve(
    cfg: ServeConfig,
    ready: Option<oneshot::Sender<DaemonInfo>>,
) -> anyhow::Result<()> {
    cfg.home.ensure_dirs()?;
    let listener = bind_first_free(cfg.bind, cfg.port).await?;
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.home)?);
    let token = generate_token();
    let started_at = Store::now();
    let info = DaemonInfo {
        port,
        pid: std::process::id(),
        token: token.clone(),
        started_at: started_at.clone(),
        bind: cfg.bind.to_string(),
        version: cfg.version.to_string(),
    };
    write_daemon_info(&cfg.home, &info)?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let state = AppState {
        store,
        home: cfg.home.clone(),
        token,
        events: EventBus::new(),
        started_at,
        version: cfg.version,
    };
    let app = crate::build_router_with_shutdown(state, shutdown_tx.clone());
    if let Some(tx) = ready {
        let _ = tx.send(info.clone());
    }
    tracing::info!(port, "artifax daemon listening");

    let home = cfg.home.clone();
    let stale = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Some(other) = read_daemon_info(&home)
                && other.pid != std::process::id()
                && pid_alive(other.pid)
            {
                tracing::warn!(
                    other = other.pid,
                    "another daemon owns daemon.json; exiting"
                );
                return;
            }
        }
    });

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
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
        }
    });
    server.await?;
    if read_daemon_info(&cfg.home).map(|i| i.pid) == Some(std::process::id()) {
        remove_daemon_info(&cfg.home);
    }
    Ok(())
}
