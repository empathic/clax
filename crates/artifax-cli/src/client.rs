//! HTTP client for the daemon, with discovery and auto-start.

use anyhow::{Context, anyhow, bail};
use artifax_core::Home;
use artifax_server::daemon::{DaemonInfo, DaemonLock, pid_alive, read_daemon_info};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Client {
    pub base: String,
    pub token: String,
    pub info: DaemonInfo,
    http: reqwest::blocking::Client,
}

/// The host part of a URL that reaches a daemon bound to `bind`: an unspecified
/// address (`0.0.0.0`, `::`) maps to the same-family loopback, a specific
/// address is used as-is, and IPv6 is bracketed. Text that is not an IP address
/// is returned unchanged.
fn probe_host(bind: &str) -> String {
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
fn browser_host(bind: &str) -> String {
    match bind.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => "localhost".to_string(),
        _ => probe_host(bind),
    }
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

impl Client {
    fn from_info(info: DaemonInfo) -> Client {
        Client {
            base: format!("http://{}:{}", probe_host(&info.bind), info.port),
            token: info.token.clone(),
            info,
            http: http(),
        }
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
        let _lock = DaemonLock::acquire(home).context("acquiring daemon lock")?;
        if let Some(c) = Client::discover_with(home, &probe) {
            return Ok(c);
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.log_path())?;
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.args([
            "serve",
            "--foreground",
            "--port",
            &port.to_string(),
            "--bind",
            &bind.to_string(),
        ])
        .env("ARTIFAX_HOME", home.root())
        .current_dir(home.root())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and the closure does nothing else.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().context("spawning artifax serve")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                bail!(
                    "daemon exited during startup ({status}); see {}",
                    home.log_path().display()
                );
            }
            if let Some(c) = Client::discover_with(home, &probe)
                && c.info.pid == child.id()
            {
                return Ok(c);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "daemon did not become ready within 5s; see {}",
            home.log_path().display()
        )
    }

    /// Errors when the running daemon is bound to a different address than `requested`.
    pub fn require_bind(&self, requested: IpAddr) -> anyhow::Result<()> {
        let requested = requested.to_string();
        if self.info.bind != requested {
            bail!(
                "daemon already running bound to {} at {}; run `artifax stop` first, then `artifax serve --bind {}`",
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

    #[expect(dead_code)]
    pub fn healthz(&self) -> anyhow::Result<serde_json::Value> {
        self.get("/healthz")
    }
    /// Status code of an unauthenticated GET, or None if unreachable.
    pub fn http_status(&self, path: &str) -> Option<u16> {
        self.http
            .get(format!("{}{path}", self.base))
            .send()
            .ok()
            .map(|r| r.status().as_u16())
    }
    pub fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        Self::check(self.http.get(format!("{}{path}", self.base)).send()?)
    }
    pub fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.http
                .post(format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .json(body)
                .send()?,
        )
    }
    pub fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(
            self.http
                .patch(format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .json(body)
                .send()?,
        )
    }
    pub fn delete(&self, path: &str) -> anyhow::Result<()> {
        Self::check(
            self.http
                .delete(format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .send()?,
        )
        .map(|_| ())
    }
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.post("/api/admin/shutdown", &serde_json::json!({}))
            .map(|_| ())
    }
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
