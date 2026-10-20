//! How clients find the daemon of a home: the discovery files it writes
//! (`daemon.json`, `starting.json`), the exclusive start lock, the ports a
//! daemon tries, and whether a process still runs. The daemon writes them;
//! every client (the CLI, the MCP shim, the tests) reads them, so they
//! live below the server.

use crate::Home;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 7480;

pub const PORT_ATTEMPTS: u16 = 21;

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
    /// The commit the daemon's binary was built from (full hex, or
    /// `unknown`). Absent in records written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
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

/// What a starting daemon is doing before it serves, in `starting.json`:
/// its `pid`; its `phase` (`opening` the store, then the audit backfill's
/// `planning`, `hashing files`, `recording`); the backfill's rows `done` of
/// `total` and bytes hashed; and `heartbeat_at`, renewed every second
/// while it starts. Removed once it serves (or fails).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StartingInfo {
    pub pid: u32,
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub bytes: u64,
    pub heartbeat_at: String,
}

impl StartingInfo {
    /// Whether the phase has no measurable progress, so a heartbeat alone
    /// shows it working.
    pub fn unmeasured(&self) -> bool {
        self.phase == "opening" || self.phase == "planning"
    }

    /// One line for a person: what the daemon is doing.
    pub fn line(&self) -> String {
        if self.unmeasured() && self.total == 0 {
            return format!("{} (pid {})", self.phase_text(), self.pid);
        }
        format!(
            "{} (pid {}): {} of {} rows, {} MiB hashed",
            self.phase_text(),
            self.pid,
            self.done,
            self.total,
            self.bytes >> 20
        )
    }

    fn phase_text(&self) -> String {
        match self.phase.as_str() {
            "opening" => "opening its database".into(),
            p => format!("recording existing history for the audit log ({p})"),
        }
    }
}

/// `starting.json`, when a starting daemon wrote one.
pub fn read_starting_info(home: &Home) -> Option<StartingInfo> {
    let text = std::fs::read_to_string(home.starting_json()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Atomically replaces `starting.json` with `info`, as `daemon.json` is.
pub fn write_starting_info(home: &Home, info: &StartingInfo) -> io::Result<()> {
    use io::Write;
    let tmp = home
        .root()
        .join(format!("starting.json.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec(info).expect("serialisable"))?;
    std::fs::rename(tmp, home.starting_json())
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

/// An exclusive advisory lock (`flock(2)`) on an open file, held while
/// this value lives. It belongs to the file's open description: dropping
/// the value, or the process ending in any way, releases it, and another
/// open of the same path in this process or any other does not share it.
///
/// `flock` comes through `nix`, so the lock behaves the same on Linux,
/// Android, macOS and iOS (std's `File::lock` family answers
/// `Unsupported` on Android).
#[derive(Debug)]
pub struct FileLock(#[allow(dead_code)] nix::fcntl::Flock<File>);

impl FileLock {
    /// Locks `f`, waiting while another open description holds it
    /// (`LOCK_EX`). A wait interrupted by a signal (`EINTR`) is retried;
    /// any other failure is returned.
    pub fn exclusive(f: File) -> io::Result<FileLock> {
        use nix::fcntl::{Flock, FlockArg};
        let mut f = f;
        loop {
            match Flock::lock(f, FlockArg::LockExclusive) {
                Ok(held) => return Ok(FileLock(held)),
                Err((back, nix::errno::Errno::EINTR)) => f = back,
                Err((_, e)) => return Err(e.into()),
            }
        }
    }

    /// Locks `f` without waiting (`LOCK_EX | LOCK_NB`): `Ok(None)` when
    /// another open description holds it (`EWOULDBLOCK`).
    pub fn try_exclusive(f: File) -> io::Result<Option<FileLock>> {
        use nix::fcntl::{Flock, FlockArg};
        match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
            Ok(held) => Ok(Some(FileLock(held))),
            Err((_, e)) if e == nix::errno::Errno::EWOULDBLOCK => Ok(None),
            Err((_, e)) => Err(e.into()),
        }
    }
}

/// Exclusive advisory lock on `daemon.lock`, released when dropped
/// (a [`FileLock`]).
pub struct DaemonLock(#[allow(dead_code)] FileLock);

impl DaemonLock {
    /// Blocks until the lock is held ([`FileLock::exclusive`]).
    pub fn acquire(home: &Home) -> io::Result<DaemonLock> {
        let f = File::create(home.daemon_lock())?;
        FileLock::exclusive(f).map(DaemonLock)
    }

    /// Takes the lock without waiting ([`FileLock::try_exclusive`]):
    /// `Ok(None)` when another open description holds it.
    pub fn try_acquire(home: &Home) -> io::Result<Option<DaemonLock>> {
        let f = File::create(home.daemon_lock())?;
        Ok(FileLock::try_exclusive(f)?.map(DaemonLock))
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

/// How long [`port_held`] waits for a connection to be accepted.
const HELD_PROBE: Duration = Duration::from_millis(250);

/// True when another program holds `port`: a connection to it on
/// `127.0.0.1` or `[::1]` is accepted. A listener on the wildcard address or
/// on `[::1]` alone does not stop a `127.0.0.1` bind, yet browsers that
/// resolve `localhost` to `[::1]` would reach it, so a port is judged by who
/// answers there, not by whether a bind fails. The probe opens and closes one
/// connection and sends nothing. The daemon skips a held port, and `clax mcp
/// --preflight` asks the same question, so both agree.
pub fn port_held(port: u16) -> bool {
    use std::net::TcpStream;
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|ip| TcpStream::connect_timeout(&SocketAddr::new(ip, port), HELD_PROBE).is_ok())
}

/// The ports a daemon asked for `start` tries, in order: `start` and the
/// next [`PORT_ATTEMPTS`]` - 1`.
pub fn port_range(start: u16) -> std::ops::Range<u16> {
    start..start.saturating_add(PORT_ATTEMPTS)
}

/// The first port of [`port_range`]`(start)` that [`port_held`] does not
/// report, or `None` when every one is held.
pub fn first_unheld_port(start: u16) -> Option<u16> {
    port_range(start).find(|p| !port_held(*p))
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
