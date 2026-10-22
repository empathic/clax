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

/// What [`stop_old_daemon`] found of the daemon a home's `daemon.json`
/// names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldDaemon {
    /// No `daemon.json`, or it names this process, no live process, or one
    /// that is not `clax serve` (its PID taken by another program).
    Absent,
    /// It named a running `clax serve`, which ended after a SIGTERM.
    Stopped(u32),
    /// It names a running `clax serve` that could not be signalled, or
    /// that had not ended when the wait was over.
    Running(u32),
}

/// Stops the `clax serve` a home's `daemon.json` names, before another
/// process opens the home's store in its place (the Clax app's core on a
/// home an earlier app's daemon served): SIGTERM, then up to `wait` for it
/// to end. The process must still be `clax serve` by its command line (a
/// program file named `clax` or `libclax.so`, the first or, for a
/// script, the second argument, and a `serve` argument), so a PID taken
/// since by another program is never signalled. Nothing is removed; the
/// caller retires the file.
pub fn stop_old_daemon(home: &Home, wait: Duration) -> OldDaemon {
    use nix::sys::signal::{Signal, kill};
    let Some(pid) = read_daemon_info(home).map(|i| i.pid) else {
        return OldDaemon::Absent;
    };
    if pid == std::process::id()
        || !pid_alive(pid)
        || !process_args(pid).is_some_and(|a| is_clax_serve(&a))
    {
        return OldDaemon::Absent;
    }
    match kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(()) => {}
        Err(nix::errno::Errno::ESRCH) => return OldDaemon::Stopped(pid),
        Err(_) => return OldDaemon::Running(pid),
    }
    let deadline = std::time::Instant::now() + wait;
    loop {
        if !pid_alive(pid) {
            return OldDaemon::Stopped(pid);
        }
        if std::time::Instant::now() >= deadline {
            return OldDaemon::Running(pid);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether command line `args` runs `clax serve`: the program, or a
/// script's path after its interpreter, is a file named `clax` or
/// `libclax.so` (the Android app's), and an argument is `serve`.
fn is_clax_serve(args: &[String]) -> bool {
    let named = args.iter().take(2).any(|a| {
        std::path::Path::new(a)
            .file_name()
            .is_some_and(|n| n == "clax" || n == "libclax.so")
    });
    named && args.iter().skip(1).any(|a| a == "serve")
}

/// The command line of process `pid`, the program first, read from the
/// kernel: `/proc/<pid>/cmdline` on Linux and Android, `sysinfo`
/// (`KERN_PROCARGS2`) on macOS. A script's is its interpreter, then the
/// script's path, then its arguments. `None` when it cannot be read (no
/// such process, another user's, or another system, such as iOS, whose
/// sandbox shows no other process).
pub fn process_args(pid: u32) -> Option<Vec<String>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        (!args.is_empty()).then_some(args)
    }
    #[cfg(target_os = "macos")]
    {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let pid = Pid::from_u32(pid);
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let args: Vec<String> = sys
            .process(pid)?
            .cmd()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        (!args.is_empty()).then_some(args)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        let _ = pid;
        None
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
    use std::process::{ChildStdin, Command, ExitStatus, Stdio};
    use std::thread::JoinHandle;

    /// A `clax serve` that says `ready`, then blocks on its standard input
    /// until a signal ends it; with `trap '' TERM`, one that a SIGTERM
    /// does not end, ready once the trap is set.
    const SERVE: &str = "#!/bin/sh\necho ready\nread line\n";
    const STUBBORN: &str = "#!/bin/sh\ntrap '' TERM\necho ready\nread line\n";

    /// A daemon's process, run by the test: its PID, a thread that reaps it
    /// (a child that ended is a zombie, alive to `kill(pid, 0)`, until it
    /// is reaped) and answers its exit status, and its standard input, held
    /// open so its `read` blocks until a signal ends it or a line arrives.
    struct Daemon {
        pid: u32,
        reaped: JoinHandle<ExitStatus>,
        stdin: ChildStdin,
    }

    /// Runs the script `text` installed as `name` with `args`, records its
    /// PID in `home`'s `daemon.json`, and waits until it says `ready`.
    fn daemon(home: &Home, name: &str, text: &str, args: &[&str]) -> Daemon {
        let exe = clax_fake_exe::install(&home.root().join("bin").join(name), text);
        let mut child = Command::new(&exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let pid = child.id();
        write_daemon_info(
            home,
            &DaemonInfo {
                port: 1,
                pid,
                token: "t".into(),
                started_at: "2026-10-20T00:00:00Z".into(),
                bind: "127.0.0.1".into(),
                version: "0.5.0".into(),
                exe: None,
                commit: None,
            },
        )
        .unwrap();
        let mut ready = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut ready,
        )
        .unwrap();
        assert_eq!(ready, "ready\n");
        Daemon {
            pid,
            reaped: std::thread::spawn(move || child.wait().unwrap()),
            stdin,
        }
    }

    fn home() -> (tempfile::TempDir, Home) {
        let d = tempfile::tempdir().unwrap();
        let home = Home::at(d.path().join("clax"));
        std::fs::create_dir_all(home.root()).unwrap();
        (d, home)
    }

    #[test]
    fn an_old_clax_serve_is_stopped_by_its_daemon_json() {
        let (_d, home) = home();
        let d = daemon(&home, "clax", SERVE, &["serve", "--port", "1"]);
        assert_eq!(
            stop_old_daemon(&home, Duration::from_secs(600)),
            OldDaemon::Stopped(d.pid)
        );
        let status = d.reaped.join().unwrap();
        assert!(!status.success(), "ended by its signal: {status:?}");
        drop(d.stdin);
    }

    #[test]
    fn an_old_clax_serve_that_will_not_go_is_running() {
        let (_d, home) = home();
        let d = daemon(&home, "clax", STUBBORN, &["serve"]);
        assert_eq!(
            stop_old_daemon(&home, Duration::from_millis(1)),
            OldDaemon::Running(d.pid)
        );
        assert!(pid_alive(d.pid));
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(d.pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
        d.reaped.join().unwrap();
    }

    /// A PID another program holds, or a `clax` that is not `serve`, is
    /// never signalled.
    #[test]
    fn a_daemon_json_naming_another_program_signals_nothing() {
        for (name, args) in [("other", &["serve"][..]), ("clax", &["mcp"][..])] {
            let (_d, home) = home();
            let mut d = daemon(&home, name, SERVE, args);
            assert_eq!(
                stop_old_daemon(&home, Duration::from_secs(600)),
                OldDaemon::Absent,
                "{name} {args:?}"
            );
            // Still running: it reads the line, then exits 0.
            use std::io::Write;
            d.stdin.write_all(b"done\n").unwrap();
            drop(d.stdin);
            assert!(d.reaped.join().unwrap().success(), "{name} {args:?}");
        }
        let (_d, home) = home();
        assert_eq!(stop_old_daemon(&home, Duration::ZERO), OldDaemon::Absent);
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
