//! HTTP client for the daemon, with discovery and auto-start.

use anyhow::{Context, anyhow, bail};
use clax_core::Home;
use clax_server::daemon::{
    DaemonInfo, DaemonLock, StartingInfo, browser_host, pid_alive, probe_host, read_daemon_info,
    read_starting_info,
};
use nix::sys::signal::Signal;
use std::net::{IpAddr, Ipv4Addr};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a daemon this client starts may take: `start` to answer when it
/// reports nothing, and `progress` between reports of progress once it
/// reports some (`starting.json`: a first start that records the audit
/// backfill may take minutes, and is waited for while it moves on).
#[derive(Clone, Copy, Debug)]
pub struct StartWindows {
    pub start: Duration,
    pub progress: Duration,
    /// How long a phase with no measurable progress (opening the store,
    /// planning the backfill) may last on its heartbeat alone.
    pub unmeasured: Duration,
}

impl StartWindows {
    /// 5 s to answer, then 30 s between reports of progress, and at most
    /// 10 minutes in a phase that reports only a heartbeat.
    pub fn current() -> StartWindows {
        StartWindows {
            start: Duration::from_secs(5),
            progress: Duration::from_secs(30),
            unmeasured: Duration::from_secs(600),
        }
    }
}

/// Follows a starting daemon's `starting.json`: extends the deadline while
/// it reports progress (rows done or bytes hashed advancing, or a new
/// phase), or, in a phase it cannot measure, a heartbeat until that phase
/// has lasted [`StartWindows::unmeasured`]; and tells the person every 2 s
/// what it is doing.
struct StartWatch {
    pid: u32,
    windows: StartWindows,
    deadline: Instant,
    seen: Option<StartingInfo>,
    progressed: bool,
    /// When the phase last seen began, as this client saw it.
    phase_since: Instant,
    /// A phase outlasted its cap on its heartbeat alone.
    stalled: bool,
    said: Option<Instant>,
}

impl StartWatch {
    fn new(pid: u32, windows: StartWindows) -> StartWatch {
        StartWatch {
            pid,
            windows,
            deadline: Instant::now() + windows.start,
            seen: None,
            progressed: false,
            phase_since: Instant::now(),
            stalled: false,
            said: None,
        }
    }

    fn poll(&mut self, home: &Home) {
        let Some(now) = read_starting_info(home).filter(|s| s.pid == self.pid) else {
            return;
        };
        let new_phase = self.seen.as_ref().is_none_or(|was| was.phase != now.phase);
        if new_phase {
            self.phase_since = Instant::now();
        }
        let advanced = self
            .seen
            .as_ref()
            .is_some_and(|was| (was.done, was.bytes) != (now.done, now.bytes));
        let beat = now.unmeasured()
            && self
                .seen
                .as_ref()
                .is_some_and(|was| was.heartbeat_at != now.heartbeat_at);
        let capped = self.phase_since.elapsed() >= self.windows.unmeasured;
        if new_phase || advanced {
            self.stalled = false;
        } else if now.unmeasured() && capped {
            self.stalled = true;
        }
        let moved = new_phase || advanced || (beat && !capped);
        if moved {
            self.progressed = true;
            self.deadline = self.deadline.max(Instant::now() + self.windows.progress);
        }
        if now.phase != "opening"
            && self
                .said
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            eprintln!("clax: the daemon is starting: {}", now.line());
            self.said = Some(Instant::now());
        }
        self.seen = Some(now);
    }

    /// Why the daemon was given up on.
    fn why(&self) -> String {
        if self.stalled {
            format!(
                "stayed {} for {}s without progress{}",
                self.seen
                    .as_ref()
                    .map_or("starting", |s| match s.phase.as_str() {
                        "opening" => "opening its database",
                        _ => "planning the audit backfill",
                    }),
                self.windows.unmeasured.as_secs(),
                self.seen
                    .as_ref()
                    .map(|s| format!(" (pid {})", s.pid))
                    .unwrap_or_default()
            )
        } else if self.progressed {
            format!(
                "made no progress for {}s while starting{}",
                self.windows.progress.as_secs(),
                self.seen
                    .as_ref()
                    .map(|s| format!(" ({})", s.line()))
                    .unwrap_or_default()
            )
        } else {
            format!(
                "did not become ready within {}s",
                self.windows.start.as_secs_f64()
            )
        }
    }
}

/// Takes the start lock, waiting for it; while another client holds it and a
/// daemon reports its start in `starting.json`, tells `say` every 2 s what
/// that daemon is doing (a first start may record the audit backfill for a
/// while).
fn acquire_saying(home: &Home, say: &mut dyn FnMut(&str)) -> std::io::Result<DaemonLock> {
    let mut said: Option<Instant> = None;
    loop {
        if let Some(lock) = DaemonLock::try_acquire(home)? {
            return Ok(lock);
        }
        if let Some(s) = read_starting_info(home).filter(|s| pid_alive(s.pid))
            && said.is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            say(&format!(
                "clax: waiting for the daemon's first start: {}",
                s.line()
            ));
            said = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub struct Client {
    pub base: String,
    pub token: String,
    pub info: DaemonInfo,
    http: reqwest::blocking::Client,
    /// The channel named in `x-clax-via` on every request, when set.
    via: Option<&'static str>,
}

/// Sends `sig` to process `pid`, ignoring failure (a process that has
/// already exited).
fn signal(pid: u32, sig: Signal) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig);
}

fn probe_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .ok()
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

/// True when the daemon described by `info` is older than version `ours`. A
/// newer daemon is kept (it serves older clients), as is one whose version does
/// not parse.
fn needs_replacing(info: &DaemonInfo, ours: &str) -> bool {
    older(&info.version, ours)
}

/// True when version `theirs` is older than `ours`; false when either does
/// not parse.
fn older(theirs: &str, ours: &str) -> bool {
    match (semver::Version::parse(theirs), semver::Version::parse(ours)) {
        (Ok(theirs), Ok(ours)) => theirs < ours,
        _ => false,
    }
}

/// Appends one timestamped line to the daemon log, best effort: the record
/// of a replacement or rollback that no tracing subscriber may be there to
/// see.
fn log_line(home: &Home, msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.log_path())
    {
        let _ = writeln!(f, "{} clax client: {msg}", clax_core::Store::now());
    }
}

/// True when `e` is a failure to connect at all (nothing listens on the port).
fn is_connect_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_connect)
}

/// Stops the daemon `target` describes, for a replacement, with the start
/// lock held. It posts `/api/admin/shutdown` (2 s timeout), waits up to 7 s
/// for the PID to exit and, if it has not and `daemon.json` still names that
/// PID, sends `SIGTERM` and waits 3 s more.
///
/// `answered` is whether `target` answered `/healthz` under the lock. When it
/// did not, and nothing accepts a connection on its port, the daemon is taken
/// as gone: its PID may belong to another process by now.
fn stop_for_replacement(home: &Home, target: &DaemonInfo, answered: bool) -> anyhow::Result<()> {
    let client = Client::from_info(target.clone()).with_timeout(Duration::from_secs(2));
    match client.shutdown() {
        Ok(()) => {}
        Err(e) if !answered && is_connect_error(&e) => {
            log_line(
                home,
                &format!(
                    "nothing listens on port {}; taking pid {} as gone",
                    target.port, target.pid
                ),
            );
            return Ok(());
        }
        Err(e) => {
            log_line(
                home,
                &format!(
                    "the old daemon did not take the shutdown request ({e:#}); waiting for it to exit"
                ),
            );
        }
    }
    let wait = |d: Duration| {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline && pid_alive(target.pid) {
            std::thread::sleep(Duration::from_millis(50));
        }
        !pid_alive(target.pid)
    };
    if wait(Duration::from_secs(7)) {
        return Ok(());
    }
    let recorded = read_daemon_info(home).is_some_and(|i| i.pid == target.pid);
    if recorded {
        log_line(
            home,
            &format!(
                "pid {} did not exit within 7 s; sending SIGTERM",
                target.pid
            ),
        );
        // A PID that daemon.json names under the start lock.
        signal(target.pid, Signal::SIGTERM);
        if wait(Duration::from_secs(3)) {
            return Ok(());
        }
    }
    bail!(
        "clax daemon v{} (pid {}) on port {} did not exit after a shutdown request{}; end it with `kill {}` and try again",
        target.version,
        target.pid,
        target.port,
        if recorded { " and SIGTERM" } else { "" },
        target.pid
    )
}

/// A replacement whose new daemon failed to start; the message says what
/// became of the previous daemon.
#[derive(Debug)]
pub struct UpgradeFailed(String);

impl std::fmt::Display for UpgradeFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpgradeFailed {}

/// `path` resolved through symlinks, or as given when it cannot be.
fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The error for a replacement whose new daemon (`exe`, canonical) failed to
/// start with `err`, after trying to start the previous daemon's executable
/// again on its port and bind address. Always an [`UpgradeFailed`].
fn roll_back(
    home: &Home,
    target: &DaemonInfo,
    exe: &std::path::Path,
    bind: IpAddr,
    err: &anyhow::Error,
    windows: StartWindows,
) -> anyhow::Error {
    let log = home.log_path();
    let head = format!(
        "the new clax daemon ({}) failed to start on port {}: {err:#}",
        exe.display(),
        target.port
    );
    log_line(home, &head);
    let recover = format!(
        "To recover: run `clax stop`, install a build that starts (see {} for why this one did not), then run the command again",
        log.display()
    );
    let none_running = |why: String| {
        log_line(home, &format!("{why}; no clax daemon is running"));
        anyhow::Error::new(UpgradeFailed(format!(
            "{head}. The previous daemon v{} was stopped, and {why}, so no clax daemon is running. {recover}",
            target.version
        )))
    };
    let previous = match target.exe.as_deref() {
        None => return none_running("its executable was not recorded".into()),
        Some(p) => std::path::Path::new(p),
    };
    // The plugins' wrapper keeps the newest managed install older than the
    // one it pins (`<home>/bin/<version>/`), so a daemon started by the
    // previous plugin still has its executable here.
    if !previous.exists() {
        return none_running(format!(
            "its executable {} no longer exists",
            previous.display()
        ));
    }
    if canonical(previous) == exe {
        return none_running(format!(
            "its executable {} was overwritten in place by the build that failed (as `cargo install` or `just install` do), so it cannot be restarted",
            previous.display()
        ));
    }
    let msg = match Client::spawn(home, previous, target.port, bind, true, windows) {
        Ok(c) => {
            log_line(
                home,
                &format!(
                    "rolled back to {} (pid {}) on port {}",
                    previous.display(),
                    c.info.pid,
                    c.info.port
                ),
            );
            format!(
                "{head}. The previous daemon v{} ({}) is running again on port {} (pid {}). See {} for why the new one failed",
                c.info.version,
                previous.display(),
                c.info.port,
                c.info.pid,
                log.display()
            )
        }
        Err(e2) => {
            log_line(
                home,
                &format!(
                    "restarting {} failed too; no clax daemon is running",
                    previous.display()
                ),
            );
            format!(
                "{head}. Restarting the previous daemon ({}) failed too: {e2:#}. No clax daemon is running. {recover}",
                previous.display(),
            )
        }
    };
    anyhow::Error::new(UpgradeFailed(msg))
}

/// How long a failed upgrade to one build is not retried.
const FAILED_UPGRADE_HOLD: Duration = Duration::from_secs(600);

/// Where the last failed upgrade is recorded.
fn failed_upgrade_path(home: &Home) -> std::path::PathBuf {
    home.root().join("logs").join("failed-upgrade.json")
}

/// The key a failed upgrade is recorded under: the version, the canonical
/// executable and its modification time (nanoseconds since the epoch), so
/// a rebuilt or reinstalled executable is tried again at once.
fn upgrade_key(version: &str, exe: &std::path::Path) -> Option<serde_json::Value> {
    let mtime = std::fs::metadata(exe)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(serde_json::json!({
        "version": version,
        "exe": exe.display().to_string(),
        "mtime_ns": mtime.to_string(),
    }))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Records that upgrading the daemon of version `from` to `version` from
/// `exe` failed with `reason` and was rolled back.
fn record_failed_upgrade(
    home: &Home,
    version: &str,
    exe: &std::path::Path,
    from: &str,
    reason: &str,
) {
    if let Some(mut key) = upgrade_key(version, exe) {
        key["at"] = unix_now().into();
        key["from_version"] = from.into();
        key["reason"] = reason.into();
        let _ = std::fs::write(failed_upgrade_path(home), key.to_string());
    }
}

/// A failed upgrade that is not retried until `until`: the daemon of version
/// `from_version` was kept, or restarted, instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeHold {
    /// The version the upgrade was to.
    pub version: String,
    /// The canonical path of the executable that failed to start.
    pub exe: std::path::PathBuf,
    /// The version of the daemon it was to replace, when recorded.
    pub from_version: Option<String>,
    /// Why it failed, and what became of the previous daemon.
    pub reason: String,
    /// When it failed, in seconds since the epoch.
    pub at: u64,
    /// When the hold ends, in seconds since the epoch.
    pub until: u64,
}

/// `secs` since the epoch as an RFC 3339 UTC time.
fn rfc3339(secs: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| secs.to_string())
}

impl UpgradeHold {
    /// What to do about the hold.
    pub fn advice(&self, home: &Home) -> String {
        format!(
            "Upgrades to this build are not tried again until {}. A rebuilt or reinstalled clax is tried at once, so install a build that starts (see {} for why this one did not); or run `clax stop`, then `clax serve`, to try this build again now, with no older daemon to fall back to",
            rfc3339(self.until),
            home.log_path().display()
        )
    }

    /// One line saying that the daemon of version `kept` is kept because of
    /// this hold, and what to do.
    pub fn line(&self, home: &Home, kept: &str) -> String {
        format!(
            "keeping clax daemon v{kept}: upgrading it to v{} ({}) failed at {}. {}",
            self.version,
            self.exe.display(),
            rfc3339(self.at),
            self.advice(home)
        )
    }

    /// The hold as `status` and `serve --json` report it, as `upgrade_held`.
    pub fn to_json(&self, home: &Home) -> serde_json::Value {
        serde_json::json!({
            "version": self.version,
            "exe": self.exe.display().to_string(),
            "from_version": self.from_version,
            "reason": self.reason,
            "failed_at": rfc3339(self.at),
            "until": rfc3339(self.until),
            "advice": self.advice(home),
        })
    }
}

/// The failed upgrade recorded for `home`, while it holds: less than
/// [`FAILED_UPGRADE_HOLD`] ago, with its executable unchanged since (the
/// same modification time). None when there is none, or it no longer holds.
pub fn upgrade_hold(home: &Home) -> Option<UpgradeHold> {
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(failed_upgrade_path(home)).ok()?).ok()?;
    let version = rec["version"].as_str()?;
    let exe = std::path::Path::new(rec["exe"].as_str()?);
    let key = upgrade_key(version, exe)?;
    if rec["mtime_ns"] != key["mtime_ns"] {
        return None;
    }
    let at = rec["at"].as_u64()?;
    let until = at + FAILED_UPGRADE_HOLD.as_secs();
    (unix_now() < until).then(|| UpgradeHold {
        version: version.to_string(),
        exe: exe.to_path_buf(),
        from_version: rec["from_version"].as_str().map(str::to_string),
        reason: rec["reason"]
            .as_str()
            .unwrap_or("the reason was not recorded")
            .to_string(),
        at,
        until,
    })
}

/// The hold that keeps a daemon of version `daemon_version` from being
/// upgraded: the [`upgrade_hold`] of `home`, when that daemon is older than
/// the version the hold is for.
pub fn upgrade_hold_for(home: &Home, daemon_version: &str) -> Option<UpgradeHold> {
    upgrade_hold(home).filter(|h| older(daemon_version, &h.version))
}

/// True when upgrading to `version` from `exe`, unchanged since, failed
/// less than [`FAILED_UPGRADE_HOLD`] ago.
fn upgrade_recently_failed(home: &Home, version: &str, exe: &std::path::Path) -> bool {
    upgrade_hold(home).is_some_and(|h| h.version == version && h.exe == exe)
}

/// The hold that kept `c`, the daemon this binary connected to, at its
/// older version: one for this binary's version and executable. `clax
/// serve` reports it.
pub fn held_upgrade(home: &Home, c: &Client) -> Option<UpgradeHold> {
    let exe = canonical(&std::env::current_exe().ok()?);
    held_upgrade_of(home, &c.info, env!("CARGO_PKG_VERSION"), &exe)
}

fn held_upgrade_of(
    home: &Home,
    daemon: &DaemonInfo,
    ours: &str,
    exe: &std::path::Path,
) -> Option<UpgradeHold> {
    upgrade_hold(home)
        .filter(|h| needs_replacing(daemon, ours) && h.version == ours && h.exe == exe)
}

/// Warns, once per process, that the daemon `kept` is kept because upgrading
/// it to `ours` from `exe` is held.
fn warn_held(home: &Home, kept: &DaemonInfo, ours: &str, exe: &std::path::Path) {
    static HELD: std::sync::Once = std::sync::Once::new();
    HELD.call_once(|| match held_upgrade_of(home, kept, ours, exe) {
        Some(h) => tracing::warn!("{}", h.line(home, &kept.version)),
        None => tracing::warn!(
            "keeping clax daemon v{}: upgrading it to v{ours} ({}) failed recently; see {}",
            kept.version,
            exe.display(),
            home.log_path().display()
        ),
    });
}

impl Client {
    fn from_info(info: DaemonInfo) -> Client {
        Client {
            base: format!("http://{}:{}", probe_host(&info.bind), info.port),
            token: info.token.clone(),
            info,
            http: http(),
            via: None,
        }
    }

    /// This client naming `via` as its channel (`x-clax-via`, spec
    /// 2026-10-06-toolpath-audit-design §6.9) on every request.
    pub fn with_via(mut self, via: &'static str) -> Client {
        self.via = Some(via);
        self
    }

    /// `req` with the bearer token and the channel, if one is set.
    fn authed(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        let req = req.bearer_auth(&self.token);
        match self.via {
            Some(v) => req.header("x-clax-via", v),
            None => req,
        }
    }

    /// A `method` request to `path` with the token and channel, `body` as
    /// JSON when given, `timeout` for this request alone when given, and
    /// the `headers` named; its JSON answer, or an error naming the
    /// refusal's code and message.
    pub fn request_with(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.authed(self.http.request(method, format!("{}{path}", self.base)));
        if let Some(b) = body {
            req = req.json(b);
        }
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        Self::check(req.send()?)
    }

    /// This client with every request bounded by `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Client {
        self.http = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(timeout)
            .build()
            .expect("client");
        self
    }

    /// A live daemon named by daemon.json that answers `/healthz`, or None.
    /// Never starts one.
    pub fn discover(home: &Home) -> Option<Client> {
        Client::discover_with(home, &probe_client()?)
    }

    fn discover_with(home: &Home, probe: &reqwest::blocking::Client) -> Option<Client> {
        let info = read_daemon_info(home)?;
        if !pid_alive(info.pid) {
            return None;
        }
        let base = format!("http://{}:{}", probe_host(&info.bind), info.port);
        let res = probe.get(format!("{base}/healthz")).send().ok()?;
        res.status().is_success().then(|| Client::from_info(info))
    }

    /// Discover, or start a daemon on `port` (0 = any free port) and wait for it.
    pub fn connect(home: &Home, port: u16) -> anyhow::Result<Client> {
        Client::connect_with_bind(home, port, IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    /// As [`Client::connect`], starting the daemon bound to `bind` when none is running.
    ///
    /// The start lock is held from the re-check until the spawned daemon answers
    /// `/healthz` (or the deadline passes), so concurrent callers start one daemon.
    pub fn connect_with_bind(home: &Home, port: u16, bind: IpAddr) -> anyhow::Result<Client> {
        let probe = probe_client().context("building probe client")?;
        if let Some(c) = Client::discover_with(home, &probe) {
            return Ok(c);
        }
        home.ensure_dirs()?;
        let _lock =
            acquire_saying(home, &mut |m| eprintln!("{m}")).context("acquiring daemon lock")?;
        if let Some(c) = Client::discover_with(home, &probe) {
            return Ok(c);
        }
        Client::spawn_locked(home, &std::env::current_exe()?, port, bind)
    }

    /// Starts `exe serve --foreground` for `home` on `port` and `bind` and
    /// waits up to 5 s for it to answer `/healthz`. The caller holds the
    /// start lock ([`DaemonLock`]).
    pub fn spawn_locked(
        home: &Home,
        exe: &std::path::Path,
        port: u16,
        bind: IpAddr,
    ) -> anyhow::Result<Client> {
        Client::spawn(home, exe, port, bind, false, StartWindows::current())
    }

    /// As [`Client::spawn_locked`], within `windows`: the deadline moves on
    /// while the daemon reports progress in `starting.json`. With
    /// `stop_if_late`, a daemon that has not answered by the deadline is
    /// stopped (`SIGTERM`, then `SIGKILL` after 3 s) so it cannot take the
    /// port a rollback needs.
    fn spawn(
        home: &Home,
        exe: &std::path::Path,
        port: u16,
        bind: IpAddr,
        stop_if_late: bool,
        windows: StartWindows,
    ) -> anyhow::Result<Client> {
        let probe = probe_client().context("building probe client")?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.log_path())?;
        // Stdio is set explicitly and no other descriptor is passed on
        // purpose. A descriptor can still reach the child by accident (a pipe
        // another thread was creating when this fork happened, on platforms
        // without `pipe2`), so `serve --foreground` closes every inherited
        // descriptor above stdio at startup.
        let mut cmd = Command::new(exe);
        cmd.args([
            "serve",
            "--foreground",
            "--port",
            &port.to_string(),
            "--bind",
            &bind.to_string(),
        ])
        .env("CLAX_HOME", home.root())
        .current_dir(home.root())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
        {
            // Its own process group, so signals sent to the spawner's
            // foreground group (a terminal's Ctrl-C) do not reach it.
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().context("spawning clax serve")?;
        let child_pid = child.id();
        // Reap the daemon whenever it exits, so a long-lived caller (the MCP
        // shim) does not keep a zombie that still looks alive.
        let (exited_tx, exited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = exited_tx.send(child.wait());
        });
        let mut watch = StartWatch::new(child_pid, windows);
        while Instant::now() < watch.deadline {
            if let Ok(status) = exited.try_recv() {
                let status = status?;
                bail!(
                    "daemon exited during startup ({status}); see {}",
                    home.log_path().display()
                );
            }
            if let Some(c) = Client::discover_with(home, &probe)
                && c.info.pid == child_pid
            {
                return Ok(c);
            }
            watch.poll(home);
            std::thread::sleep(Duration::from_millis(100));
        }
        if stop_if_late {
            // The child this call spawned, which the reaper thread has not
            // yet reaped (it has not sent its status).
            signal(child_pid, Signal::SIGTERM);
            if exited.recv_timeout(Duration::from_secs(3)).is_err() {
                signal(child_pid, Signal::SIGKILL);
                let _ = exited.recv_timeout(Duration::from_secs(2));
            }
            bail!(
                "daemon {} and was stopped; see {}",
                watch.why(),
                home.log_path().display()
            )
        }
        bail!("daemon {}; see {}", watch.why(), home.log_path().display())
    }

    /// As [`Client::connect`], but a running daemon older than this binary is
    /// replaced through [`Client::replace`] on its own port and bind address;
    /// a newer or equal daemon is kept, with a warning logged once per process
    /// when the versions differ.
    pub fn connect_matching_version(home: &Home, port: u16) -> anyhow::Result<Client> {
        Client::connect_matching_version_with_bind(home, port, IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    /// As [`Client::connect_matching_version`], starting the daemon bound to
    /// `bind` when none is running. The version rule applies to whichever
    /// daemon [`Client::connect_with_bind`] returns, including one another
    /// client started while this one waited for the start lock.
    pub fn connect_matching_version_with_bind(
        home: &Home,
        port: u16,
        bind: IpAddr,
    ) -> anyhow::Result<Client> {
        let exe = std::env::current_exe().context("finding this executable")?;
        Client::connect_matching(home, port, bind, env!("CARGO_PKG_VERSION"), &exe)
    }

    /// [`Client::connect_matching_version_with_bind`] for a binary of version
    /// `ours` at `exe`. An upgrade to `ours` from `exe` that failed and was
    /// rolled back less than 10 minutes ago, with `exe` unchanged since, is
    /// not tried again: the running daemon is kept, with a warning.
    fn connect_matching(
        home: &Home,
        port: u16,
        bind: IpAddr,
        ours: &str,
        exe: &std::path::Path,
    ) -> anyhow::Result<Client> {
        let c = Client::connect_with_bind(home, port, bind)?;
        if !needs_replacing(&c.info, ours) {
            if c.info.version != ours {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    tracing::warn!(
                        "clax daemon v{} differs from this binary (v{ours}); keeping it",
                        c.info.version
                    )
                });
            }
            return Ok(c);
        }
        let exe = canonical(exe);
        if upgrade_recently_failed(home, ours, &exe) {
            warn_held(home, &c.info, ours, &exe);
            return Ok(c);
        }
        Client::replace(
            home,
            &c,
            &exe,
            |info| !needs_replacing(info, ours),
            Some(ours),
        )
        .with_context(|| format!("upgrading the clax daemon to v{ours}"))
    }

    /// Replaces the daemon `old` names with one started from `exe` on the
    /// same port and bind address. Holds the start lock throughout, so no
    /// other client starts a daemon in the gap. Under the lock it re-reads
    /// `daemon.json`: a different live daemon that `accept`s is used as it
    /// is (another client already replaced `old`). Otherwise the daemon
    /// `daemon.json` names now (or `old`, when none answers) is stopped
    /// (see `stop_for_replacement`) and `exe` is started on that daemon's port
    /// and bind address. Each replacement is recorded in the daemon log.
    ///
    /// When `exe` fails to start, the previous daemon's recorded executable,
    /// if it still exists and is not the same file as `exe` (compared after
    /// resolving symlinks), is started again on the same port and bind
    /// address. The error, an [`UpgradeFailed`], then says whether it is
    /// running again or no daemon is running and how to recover, and names
    /// the log.
    ///
    /// With `upgrade`, the version of `exe`, a held upgrade ([`upgrade_hold`])
    /// is checked again under the start lock, and the daemon found there is
    /// kept; and a failed upgrade is recorded before the lock is released.
    /// So clients that waited for the lock while another tried the upgrade
    /// do not each try it again.
    pub fn replace(
        home: &Home,
        old: &Client,
        exe: &std::path::Path,
        accept: impl Fn(&DaemonInfo) -> bool,
        upgrade: Option<&str>,
    ) -> anyhow::Result<Client> {
        Client::replace_within(home, old, exe, accept, upgrade, StartWindows::current())
    }

    /// [`Client::replace`], waiting for the new daemon within `windows`.
    ///
    /// The previous build is not restarted when the new daemon moved the
    /// database to a schema newer than it was (a migration, or the audit
    /// backfill's start, committed before it failed): the previous build
    /// could not open it. The error then says so and that no daemon is
    /// running; the next start of the new build carries on where it
    /// stopped.
    pub fn replace_within(
        home: &Home,
        old: &Client,
        exe: &std::path::Path,
        accept: impl Fn(&DaemonInfo) -> bool,
        upgrade: Option<&str>,
        windows: StartWindows,
    ) -> anyhow::Result<Client> {
        let exe = &canonical(exe);
        home.ensure_dirs()?;
        let _lock =
            acquire_saying(home, &mut |m| eprintln!("{m}")).context("acquiring daemon lock")?;
        let current = Client::discover(home);
        if let Some(c) = &current
            && c.info.pid != old.info.pid
            && accept(&c.info)
        {
            return Ok(Client::from_info(c.info.clone()));
        }
        if let (Some(c), Some(ours)) = (&current, upgrade)
            && upgrade_recently_failed(home, ours, exe)
        {
            warn_held(home, &c.info, ours, exe);
            return Ok(Client::from_info(c.info.clone()));
        }
        let answered = current.is_some();
        let target = current.map_or_else(|| old.info.clone(), |c| c.info);
        let bind: IpAddr = target
            .bind
            .parse()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let what = format!(
            "clax daemon v{} (pid {}{}) on {}:{}",
            target.version,
            target.pid,
            target
                .exe
                .as_deref()
                .map(|e| format!(", {e}"))
                .unwrap_or_default(),
            target.bind,
            target.port
        );
        tracing::info!("replacing {what} with {}", exe.display());
        log_line(home, &format!("replacing {what} with {}", exe.display()));
        // Whether a failed start moved the schema on decides whether the
        // previous build may be restarted, so the swap needs it known.
        let schema = clax_core::store::schema_version(home).map_err(|e| {
            let msg = format!(
                "could not read the database's schema version before replacing the clax daemon v{} with {}: {e}; the running daemon was kept. Run the command again; if this persists, see {}",
                target.version,
                exe.display(),
                home.log_path().display()
            );
            log_line(home, &msg);
            anyhow::Error::new(UpgradeFailed(msg))
        })?;
        stop_for_replacement(home, &target, answered)?;
        let new = match Client::spawn(home, exe, target.port, bind, true, windows) {
            Ok(c) => c,
            Err(e) => {
                let now = clax_core::store::schema_version(home).ok().flatten();
                if let (Some(was), Some(now)) = (schema, now)
                    && now > was
                {
                    let msg = format!(
                        "the new clax daemon ({}) failed to start on port {}: {e:#}. It had already moved the database from schema {was} to {now}, which the previous daemon v{} cannot open, so it was not restarted and no clax daemon is running. Run the command again to start the new build, which carries on where it stopped; see {} for why it failed",
                        exe.display(),
                        target.port,
                        target.version,
                        home.log_path().display()
                    );
                    log_line(home, &msg);
                    return Err(anyhow::Error::new(UpgradeFailed(msg)));
                }
                let e = roll_back(home, &target, exe, bind, &e, windows);
                let Some(ours) = upgrade else { return Err(e) };
                record_failed_upgrade(home, ours, exe, &target.version, &format!("{e:#}"));
                // The hold only keeps a running daemon.
                if Client::discover(home).is_none() {
                    return Err(e);
                }
                return Err(anyhow::Error::new(UpgradeFailed(format!(
                    "{e:#}. Upgrades to this build are not tried again for {} minutes; `clax status` says why and what to do",
                    FAILED_UPGRADE_HOLD.as_secs() / 60
                ))));
            }
        };
        if new.info.port != target.port {
            let msg = format!(
                "the replacement daemon listens on port {}, not {} (the port was taken); open viewers must be reopened",
                new.info.port, target.port
            );
            tracing::warn!("{msg}");
            log_line(home, &msg);
        }
        Ok(new)
    }

    /// Errors when the running daemon is bound to a different address than `requested`.
    pub fn require_bind(&self, requested: IpAddr) -> anyhow::Result<()> {
        let requested = requested.to_string();
        if self.info.bind != requested {
            bail!(
                "daemon already running bound to {} at {}; run `clax stop` first, then `clax serve --bind {}`",
                self.info.bind,
                self.browser_url(""),
                requested
            );
        }
        Ok(())
    }

    pub fn browser_url(&self, path: &str) -> String {
        format!(
            "http://{}:{}{}",
            browser_host(&self.info.bind),
            self.info.port,
            path
        )
    }

    fn check(res: reqwest::blocking::Response) -> anyhow::Result<serde_json::Value> {
        let status = res.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::json!({}));
        }
        let body: serde_json::Value = res.json().unwrap_or(serde_json::json!({}));
        if status.is_success() {
            Ok(body)
        } else {
            let code = body["error"]["code"].as_str().unwrap_or("error");
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("request failed");
            Err(anyhow!("{code}: {msg}"))
        }
    }

    /// Status code of an unauthenticated GET, or None if unreachable.
    pub fn http_status(&self, path: &str) -> Option<u16> {
        self.http
            .get(format!("{}{path}", self.base))
            .send()
            .ok()
            .map(|r| r.status().as_u16())
    }
    /// GET with the bearer token (the session routes need it; the others ignore it).
    pub fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.authed(self.http.get(format!("{}{path}", self.base)))
                .send()?,
        )
    }
    /// [`Client::get`] with this request alone bounded by `timeout`.
    pub fn get_with_timeout(
        &self,
        path: &str,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.authed(self.http.get(format!("{}{path}", self.base)))
                .timeout(timeout)
                .send()?,
        )
    }
    /// GET `path` with `query` and the bearer token, for a response read as
    /// it streams: no deadline on the whole transfer (the daemon gives up
    /// on a reader that stops reading), 5 s to connect. A refusal is an
    /// error naming its code and message.
    pub fn get_stream(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<reqwest::blocking::Response> {
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(None)
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        let res = self
            .authed(http.get(format!("{}{path}", self.base)))
            .query(query)
            .send()?;
        if res.status().is_success() {
            Ok(res)
        } else {
            Self::check(res).map(|_| unreachable!("a failed status is an error"))
        }
    }
    pub fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.authed(self.http.post(format!("{}{path}", self.base)))
                .json(body)
                .send()?,
        )
    }
    pub fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.authed(self.http.patch(format!("{}{path}", self.base)))
                .json(body)
                .send()?,
        )
    }
    pub fn delete(&self, path: &str) -> anyhow::Result<()> {
        Self::check(
            self.authed(self.http.delete(format!("{}{path}", self.base)))
                .send()?,
        )
        .map(|_| ())
    }
    /// A `method` request with the bearer token, with `body` as JSON when
    /// given. The token makes the request the owner's, so on the viewer
    /// routes it acts as the owner identity the owner's browsers share.
    pub fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.authed(self.http.request(method, format!("{}{path}", self.base)));
        if let Some(b) = body {
            req = req.json(b);
        }
        Self::check(req.send()?)
    }
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.post("/api/admin/shutdown", &serde_json::json!({}))
            .map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Records, as a rolled-back upgrade does, that upgrading to `version`
    /// from `exe` failed with `reason` just now.
    pub(crate) fn write_hold(home: &Home, version: &str, exe: &std::path::Path, reason: &str) {
        record_failed_upgrade(home, version, &canonical(exe), "0.0.0", reason);
    }

    fn info(version: &str) -> DaemonInfo {
        DaemonInfo {
            port: 1,
            pid: 2,
            token: "t".into(),
            started_at: "2026-09-28T00:00:00Z".into(),
            bind: "127.0.0.1".into(),
            version: version.into(),
            exe: None,
            commit: None,
        }
    }

    #[test]
    fn only_an_older_daemon_is_replaced() {
        assert!(needs_replacing(&info("0.1.9"), "0.2.0"));
        assert!(needs_replacing(&info("0.2.0-rc.1"), "0.2.0"));
        assert!(!needs_replacing(&info("0.2.0"), "0.2.0"));
        assert!(!needs_replacing(&info("0.3.0"), "0.2.0"));
        assert!(!needs_replacing(&info("test"), "0.2.0"));
    }

    /// The Python interpreter itself, for the fakes' `#!` line: a version
    /// manager's `python3` shim adds a shell start-up to every fake's start,
    /// which under load can exceed the 5 s a daemon gets to become ready.
    fn python3() -> &'static str {
        static PYTHON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        PYTHON.get_or_init(|| {
            let out = std::process::Command::new("python3")
                .args(["-c", "import sys; print(sys.executable)"])
                .output()
                .expect("python3 runs");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        })
    }

    /// A stand-in daemon executable: `serve --foreground --port P --bind B`
    /// listens on 127.0.0.1 (on `listen` instead of P when given), records
    /// B, `version` and its own path in daemon.json, answers `/healthz`, and
    /// on `POST /api/admin/shutdown` exits, or with `hang` never answers.
    /// A file of its own: a rollback restarts the daemon from the canonical
    /// path it recorded.
    fn fake_exe(
        dir: &std::path::Path,
        name: &str,
        version: &str,
        hang: bool,
        listen: Option<u16>,
    ) -> std::path::PathBuf {
        fake_exe_with(dir, name, version, hang, listen, "")
    }

    /// [`fake_exe`] that runs the Python `start` (which sees `home` and
    /// `starting(phase, done, total)`, writing `starting.json` as a daemon
    /// does while it starts) before it listens.
    fn fake_exe_with(
        dir: &std::path::Path,
        name: &str,
        version: &str,
        hang: bool,
        listen: Option<u16>,
        start: &str,
    ) -> std::path::PathBuf {
        let path = dir.join(name);
        let script = format!(
            r#"#!{python}
import json, os, sys, threading, time
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
a = sys.argv
port = int(a[a.index("--port") + 1])
bind = a[a.index("--bind") + 1]
listen = {listen}
home = os.environ["CLAX_HOME"]
me = os.path.realpath(__file__)
# Recorded for the test's guard, which kills every fake when the test ends;
# and a cap, should the test process itself be killed.
pids = os.path.join(os.path.dirname(home), "pids")
os.makedirs(pids, exist_ok=True)
open(os.path.join(pids, str(os.getpid())), "w").close()
threading.Timer(120, lambda: os._exit(0)).start()
def starting(phase, done, total):
    tmp = os.path.join(home, "starting.json.tmp")
    with open(tmp, "w") as f:
        json.dump({{"pid": os.getpid(), "phase": phase, "done": done, "total": total,
                   "bytes": 0, "heartbeat_at": str(time.time())}}, f)
    os.rename(tmp, os.path.join(home, "starting.json"))
{start}
class H(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def reply(self, body):
        b = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        self.reply({{"version": "{version}", "pid": os.getpid()}})
    def do_POST(self):
        if {hang}:
            time.sleep(3600)
        self.reply({{}})
        self.wfile.flush()
        def stop():
            srv.server_close()
            try: os.remove(os.path.join(home, "daemon.json"))
            except OSError: pass
            os._exit(0)
        threading.Thread(target=stop).start()
srv = ThreadingHTTPServer(("127.0.0.1", port if listen is None else listen), H)
srv.daemon_threads = True
info = {{"port": srv.server_address[1], "pid": os.getpid(), "token": "t", "started_at": "s",
        "bind": bind, "version": "{version}", "exe": me}}
tmp = os.path.join(home, "daemon.json.tmp")
with open(tmp, "w") as f: json.dump(info, f)
os.rename(tmp, os.path.join(home, "daemon.json"))
srv.serve_forever()
"#,
            python = python3(),
            listen = listen.map_or("None".to_string(), |p| p.to_string()),
            hang = if hang { "True" } else { "False" },
        );
        clax_fake_exe::install_own(&path, &script)
    }

    /// A test's scratch directory. Every fake daemon started under it
    /// records its PID in `pids/`, and dropping this (when the test ends,
    /// passing or panicking) kills each one still alive, so a failing test
    /// leaves no daemon behind.
    struct Scratch(tempfile::TempDir);

    impl Scratch {
        fn new() -> Scratch {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("pids")).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &std::path::Path {
            self.0.path()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let Ok(entries) = std::fs::read_dir(self.path().join("pids")) else {
                return;
            };
            for pid in entries.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()) {
                // A fake this test started (its PID file).
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
    }

    /// A child process killed when dropped.
    struct KillOnDrop(std::process::Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        clax_fake_exe::install(&dir.join(name), &format!("#!/bin/sh\n{body}\n"))
    }

    /// [`script`] as a file of its own, for a build the code under test
    /// identifies by its file: it runs the canonical path, and an upgrade
    /// hold records that path and its modification time.
    fn script_own(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        clax_fake_exe::install_own(&dir.join(name), &format!("#!/bin/sh\n{body}\n"))
    }

    /// A scratch home under `dir` with a fake daemon from `exe` running on
    /// an ephemeral port bound (as recorded) to `bind`.
    fn running(dir: &std::path::Path, exe: &std::path::Path, bind: &str) -> (Home, Client) {
        let home = Home::at(dir.join("ax"));
        home.ensure_dirs().unwrap();
        let _lock = DaemonLock::acquire(&home).unwrap();
        let c = Client::spawn_locked(&home, exe, 0, bind.parse().unwrap()).unwrap();
        (home, c)
    }

    fn stop(home: &Home) {
        if let Some(c) = Client::discover(home) {
            let _ = c.shutdown();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && pid_alive(c.info.pid) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    fn log(home: &Home) -> String {
        std::fs::read_to_string(home.log_path()).unwrap_or_default()
    }

    #[test]
    fn a_new_daemon_that_fails_to_start_rolls_back_to_the_old_one() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        let bad = script(dir.path(), "bad", "exit 1");
        let e = format!(
            "{:#}",
            Client::replace(&home, &old, &bad, |_| false, None)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("failed to start"), "{e}");
        assert!(e.contains("running again"), "{e}");
        assert!(e.contains("daemon.log"), "{e}");
        let now = Client::discover(&home).expect("the old executable runs again");
        assert_eq!(now.info.version, "0.0.1");
        assert_eq!(now.info.port, old.info.port, "same port");
        assert_ne!(now.info.pid, old.info.pid);
        assert!(log(&home).contains("rolled back"), "{}", log(&home));
        stop(&home);
    }

    #[test]
    fn a_failed_rollback_says_no_daemon_is_running() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        // The old executable now fails too, as the new one does.
        clax_fake_exe::install_own(&old_exe, "#!/bin/sh\nexit 1\n");
        let bad = script(dir.path(), "bad", "exit 1");
        let e = format!(
            "{:#}",
            Client::replace(&home, &old, &bad, |_| false, None)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("failed too"), "{e}");
        assert!(e.contains("No clax daemon is running"), "{e}");
        assert!(Client::discover(&home).is_none());
    }

    #[test]
    fn a_rollback_without_the_old_executable_says_no_daemon_is_running() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        std::fs::remove_file(&old_exe).unwrap();
        let bad = script(dir.path(), "bad", "exit 1");
        let e = format!(
            "{:#}",
            Client::replace(&home, &old, &bad, |_| false, None)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("no clax daemon is running"), "{e}");
        assert!(Client::discover(&home).is_none());
    }

    #[test]
    fn a_new_daemon_that_never_answers_is_stopped_before_the_rollback() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        let pid_file = dir.path().join("slow.pid");
        let slow = script_own(
            dir.path(),
            "slow",
            "d=\"$(dirname \"$0\")\"\necho $$ > \"$d/slow.pid\"\ntouch \"$d/pids/$$\"\nexec sleep 60",
        );
        let e = format!(
            "{:#}",
            Client::replace(&home, &old, &slow, |_| false, None)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("running again"), "{e}");
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!pid_alive(pid), "the late daemon was stopped");
        stop(&home);
    }

    /// Windows for a test: 5 s to report anything (a stand-in's Python can
    /// take seconds to launch under load), then 1 s between reports.
    const QUICK: StartWindows = StartWindows {
        start: Duration::from_secs(5),
        progress: Duration::from_secs(1),
        unmeasured: Duration::from_secs(60),
    };

    /// As [`QUICK`], with a start window a slow start outlasts: 3 s.
    const SHORT_START: StartWindows = StartWindows {
        start: Duration::from_secs(3),
        progress: Duration::from_secs(1),
        unmeasured: Duration::from_secs(60),
    };

    #[test]
    fn a_slow_start_that_reports_progress_is_waited_for_and_not_rolled_back() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        // 4.4 s of backfill, past the start window, reported every 200 ms.
        let slow = fake_exe_with(
            dir.path(),
            "slow",
            "0.0.2",
            false,
            None,
            "for i in range(22):\n    starting('recording', i, 22)\n    time.sleep(0.2)",
        );
        let new = Client::replace_within(&home, &old, &slow, |_| false, None, SHORT_START)
            .expect("the slow start is waited for");
        assert_eq!(new.info.version, "0.0.2");
        assert!(!log(&home).contains("rolled back"), "{}", log(&home));
        stop(&home);
    }

    #[test]
    fn a_start_that_stops_progressing_is_given_up_and_rolled_back() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        let stuck = fake_exe_with(
            dir.path(),
            "stuck",
            "0.0.2",
            false,
            None,
            "starting('recording', 3, 10)\ntime.sleep(60)",
        );
        let e = format!(
            "{:#}",
            Client::replace_within(&home, &old, &stuck, |_| false, None, QUICK)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("made no progress for 1s"), "{e}");
        assert!(e.contains("3 of 10 rows"), "{e}");
        assert!(e.contains("running again"), "{e}");
        stop(&home);
    }

    #[test]
    fn a_new_daemon_that_moved_the_schema_on_is_not_rolled_back_to_the_old_build() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        rusqlite::Connection::open(home.db_path())
            .unwrap()
            .pragma_update(None, "user_version", 19)
            .unwrap();
        // It migrates the database on, then fails.
        let bad = fake_exe_with(
            dir.path(),
            "migrates",
            "0.0.2",
            false,
            None,
            "import sqlite3\nc = sqlite3.connect(os.path.join(home, 'clax.db'))\nc.execute('PRAGMA user_version = 20')\nc.commit()\nos._exit(1)",
        );
        let e = format!(
            "{:#}",
            Client::replace_within(&home, &old, &bad, |_| false, None, QUICK)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("from schema 19 to 20"), "{e}");
        assert!(e.contains("no clax daemon is running"), "{e}");
        assert!(
            Client::discover(&home).is_none(),
            "the old build was not restarted"
        );
        assert!(!log(&home).contains("rolled back"), "{}", log(&home));
    }

    #[test]
    fn a_start_stuck_in_a_phase_it_cannot_measure_is_given_up_after_its_cap() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        // Planning forever, with a live heartbeat.
        let hung = fake_exe_with(
            dir.path(),
            "hung",
            "0.0.2",
            false,
            None,
            "while True:\n    starting('planning', 0, 0)\n    time.sleep(0.2)",
        );
        let windows = StartWindows {
            unmeasured: Duration::from_secs(2),
            ..QUICK
        };
        let e = format!(
            "{:#}",
            Client::replace_within(&home, &old, &hung, |_| false, None, windows)
                .err()
                .expect("replace fails")
        );
        assert!(
            e.contains("stayed planning the audit backfill for 2s without progress"),
            "{e}"
        );
        assert!(e.contains("running again"), "{e}");
        stop(&home);
    }

    #[test]
    fn a_swap_whose_schema_cannot_be_read_first_is_refused_and_keeps_the_daemon() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        // A database that cannot be opened.
        std::fs::create_dir(home.db_path()).unwrap();
        let new = fake_exe(dir.path(), "new", "0.0.2", false, None);
        let e = format!(
            "{:#}",
            Client::replace_within(&home, &old, &new, |_| false, None, QUICK)
                .err()
                .expect("replace is refused")
        );
        assert!(
            e.contains("could not read the database's schema version"),
            "{e}"
        );
        assert!(e.contains("the running daemon was kept"), "{e}");
        let still = Client::discover(&home).expect("the old daemon runs");
        assert_eq!(still.info.pid, old.info.pid);
        stop(&home);
    }

    #[test]
    fn a_client_queued_behind_a_first_start_says_it_is_waiting() {
        let dir = Scratch::new();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let held = DaemonLock::acquire(&home).unwrap();
        // The daemon starting: this test's own process stands in for it.
        std::fs::write(
            home.starting_json(),
            serde_json::json!({"pid": std::process::id(), "phase": "recording", "done": 5,
                "total": 9, "bytes": 0, "heartbeat_at": "t"})
            .to_string(),
        )
        .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let h = home.clone();
        let waiter = std::thread::spawn(move || {
            acquire_saying(&h, &mut |m| {
                let _ = tx.send(m.to_string());
            })
            .map(drop)
        });
        let said = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            said.contains("waiting for the daemon's first start"),
            "{said}"
        );
        assert!(said.contains("5 of 9 rows"), "{said}");
        drop(held);
        waiter.join().unwrap().unwrap();
    }

    #[test]
    fn an_old_daemon_that_ignores_shutdown_is_terminated_promptly() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", true, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        let new_exe = fake_exe(dir.path(), "new", "0.0.2", false, None);
        let started = Instant::now();
        let c = Client::replace(&home, &old, &new_exe, |_| false, None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(c.info.version, "0.0.2");
        assert!(!pid_alive(old.info.pid));
        assert!(log(&home).contains("SIGTERM"), "{}", log(&home));
        stop(&home);
    }

    #[test]
    fn the_replacement_takes_the_bind_of_the_daemon_it_replaces_and_is_logged() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        // Recorded as bound to every address; the fake listens on loopback.
        let (home, current) = running(dir.path(), &old_exe, "0.0.0.0");
        // A caller's stale view of the same daemon, as bound to loopback.
        let mut info = current.info.clone();
        info.bind = "127.0.0.1".into();
        let old = Client::from_info(info);
        let new_exe = fake_exe(dir.path(), "new", "0.0.2", false, None);
        let c = Client::replace(&home, &old, &new_exe, |_| false, None).unwrap();
        assert_eq!(c.info.bind, "0.0.0.0");
        assert_eq!(c.info.port, current.info.port);
        let log = log(&home);
        assert!(log.contains("replacing clax daemon v0.0.1"), "{log}");
        stop(&home);
    }

    #[test]
    fn a_replacement_on_another_port_is_logged() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        let new_exe = fake_exe(dir.path(), "new", "0.0.2", false, Some(0));
        let c = Client::replace(&home, &old, &new_exe, |_| false, None).unwrap();
        assert_ne!(c.info.port, old.info.port);
        let log = log(&home);
        assert!(log.contains(&format!("not {}", old.info.port)), "{log}");
        stop(&home);
    }

    #[test]
    fn a_vanished_daemon_whose_pid_was_reused_is_not_waited_on_or_signalled() {
        let dir = Scratch::new();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        // An unrelated live process holds the old daemon's PID, and nothing
        // listens on its port any more.
        let mut other = KillOnDrop(Command::new("sleep").arg("60").spawn().unwrap());
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut gone = info("0.0.1");
        gone.pid = other.0.id();
        gone.port = port;
        let old = Client::from_info(gone);
        let new_exe = fake_exe(dir.path(), "new", "0.0.2", false, None);
        let started = Instant::now();
        let c = Client::replace(&home, &old, &new_exe, |_| false, None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(c.info.version, "0.0.2");
        assert!(
            other.0.try_wait().unwrap().is_none(),
            "the unrelated process is untouched"
        );
        stop(&home);
    }

    #[test]
    fn an_executable_overwritten_in_place_is_not_restarted_even_through_a_symlink() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, old) = running(dir.path(), &old_exe, "127.0.0.1");
        // An in-place reinstall writes a build that fails over the old path,
        // and this client runs it through a symlink.
        let starts = dir.path().join("starts");
        clax_fake_exe::install_own(
            &old_exe,
            "#!/bin/sh\necho start >> \"$(dirname \"$0\")/starts\"\nexit 1\n",
        );
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&old_exe, &link).unwrap();
        let e = format!(
            "{:#}",
            Client::replace(&home, &old, &link, |_| false, None)
                .err()
                .expect("replace fails")
        );
        assert!(e.contains("overwritten in place"), "{e}");
        assert!(e.contains("no clax daemon is running"), "{e}");
        assert!(e.contains("`clax stop`"), "{e}");
        assert!(!e.contains("failed too"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&starts).unwrap().lines().count(),
            1,
            "the failing build ran once"
        );
        assert!(Client::discover(&home).is_none());
    }

    fn replacements(home: &Home) -> usize {
        log(home).matches("replacing clax daemon").count()
    }

    #[test]
    fn a_failed_upgrade_is_not_retried_for_ten_minutes_unless_its_executable_changes() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, _old) = running(dir.path(), &old_exe, "127.0.0.1");
        let bad = script_own(dir.path(), "bad", "exit 1");
        let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let e = format!(
            "{:#}",
            Client::connect_matching(&home, 0, lo, "0.0.2", &bad)
                .err()
                .expect("the upgrade fails")
        );
        assert!(e.contains("running again"), "{e}");
        assert!(e.contains("not tried again for 10 minutes"), "{e}");
        assert_eq!(replacements(&home), 1);
        let rolled_back = Client::discover(&home).unwrap().info;
        let hold = upgrade_hold_for(&home, &rolled_back.version).expect("the hold is reported");
        assert_eq!(hold.version, "0.0.2");
        assert_eq!(hold.exe, canonical(&bad));
        assert_eq!(hold.from_version.as_deref(), Some("0.0.1"));
        assert!(hold.reason.contains("failed to start"), "{hold:?}");
        assert_eq!(hold.until - hold.at, 600);
        assert_eq!(
            held_upgrade_of(&home, &rolled_back, "0.0.2", &canonical(&bad)),
            Some(hold.clone())
        );
        assert!(upgrade_hold_for(&home, "0.0.2").is_none(), "not older");

        // The same build again: the rolled-back daemon is kept, untouched.
        let c = Client::connect_matching(&home, 0, lo, "0.0.2", &bad).unwrap();
        assert_eq!(c.info.pid, rolled_back.pid);
        assert_eq!(c.info.version, "0.0.1");
        assert_eq!(replacements(&home), 1, "not retried");

        // A rebuilt executable is tried at once.
        std::thread::sleep(Duration::from_millis(20));
        clax_fake_exe::install_own(&bad, "#!/bin/sh\n# rebuilt\nexit 1\n");
        assert!(Client::connect_matching(&home, 0, lo, "0.0.2", &bad).is_err());
        assert_eq!(
            replacements(&home),
            2,
            "retried after the executable changed"
        );

        // So is the same build once ten minutes have passed.
        let path = failed_upgrade_path(&home);
        let mut rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        rec["at"] = (unix_now() - 601).into();
        std::fs::write(&path, rec.to_string()).unwrap();
        assert!(Client::connect_matching(&home, 0, lo, "0.0.2", &bad).is_err());
        assert_eq!(replacements(&home), 3, "retried after ten minutes");
        stop(&home);
    }

    #[test]
    fn clients_waiting_on_the_start_lock_do_not_repeat_a_failed_upgrade() {
        let dir = Scratch::new();
        let old_exe = fake_exe(dir.path(), "old", "0.0.1", false, None);
        let (home, _old) = running(dir.path(), &old_exe, "127.0.0.1");
        let bad = script(dir.path(), "bad", "sleep 0.3\nexit 1");
        let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
        // Each client finds the old daemon before any takes the start lock.
        let ready = std::sync::Barrier::new(3);
        let results: Vec<anyhow::Result<Client>> = std::thread::scope(|s| {
            let clients: Vec<_> = (0..3)
                .map(|_| {
                    s.spawn(|| {
                        ready.wait();
                        Client::connect_matching(&home, 0, lo, "0.0.2", &bad)
                    })
                })
                .collect();
            clients.into_iter().map(|c| c.join().unwrap()).collect()
        });
        assert_eq!(replacements(&home), 1, "{}", log(&home));
        let failed = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(failed, 1, "one client tried the upgrade");
        let now = Client::discover(&home).unwrap().info;
        for c in results.iter().flatten() {
            assert_eq!(c.info.pid, now.pid, "the others use the rolled-back daemon");
            assert_eq!(c.info.version, "0.0.1");
        }
        stop(&home);
    }

    #[test]
    fn no_hold_is_reported_without_a_record_or_once_the_executable_changes() {
        let dir = Scratch::new();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        assert!(upgrade_hold(&home).is_none());
        let exe = script_own(dir.path(), "new", "exit 1");
        write_hold(&home, "0.0.2", &exe, "why");
        assert_eq!(upgrade_hold(&home).unwrap().reason, "why");
        std::thread::sleep(Duration::from_millis(20));
        clax_fake_exe::install_own(&exe, "#!/bin/sh\n# rebuilt\nexit 1\n");
        assert!(
            upgrade_hold(&home).is_none(),
            "a rebuilt executable lifts it"
        );
    }
}
