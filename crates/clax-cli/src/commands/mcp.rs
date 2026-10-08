use crate::client::Client;
use clax_core::Home;
use clax_mcp::shim::{self, Endpoint, Harness};
use std::sync::Arc;
use std::time::Duration;

/// The harness that spawns the shim. Pi has no MCP client; its extension
/// calls the daemon directly.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Grok,
}

#[derive(clap::Args)]
pub struct Args {
    /// The harness running this MCP server.
    #[arg(long, value_enum, default_value_t = Agent::Claude)]
    pub agent: Agent,
    /// Milliseconds between session heartbeats.
    #[arg(long, hide = true, default_value_t = shim::DEFAULT_HEARTBEAT.as_millis() as u64)]
    pub heartbeat_interval_ms: u64,
    /// Check that the server could start, then exit: resolve the home, its
    /// `config.toml` and the port, and check that the port is free or this
    /// home's daemon's ([`port_clash`]); print nothing and exit 0; on failure
    /// print a one-line `error: <reason>` and exit 1. Starts no daemon and
    /// sends no request to one. The plugins' wrapper runs it before it execs
    /// `clax mcp`.
    #[arg(long, hide = true)]
    pub preflight: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.preflight {
        return cli
            .port_for(home)
            .and_then(|port| match port_clash(home, port) {
                Some(why) => Err(anyhow::anyhow!("{why}{}", port_fix(cli, home, port))),
                None => Ok(()),
            })
            .map_err(|e| anyhow::anyhow!("{}", one_line(&format!("{e:#}"))));
    }
    let parent_pid = std::os::unix::process::parent_id();
    if matches!(a.agent, Agent::Claude)
        && crate::host::grok_runs_mcp(|k| std::env::var(k).ok(), parent_pid)
    {
        // Grok Build runs the Claude Code copy too; in a Grok session only
        // clax-grok's server acts (spec D17).
        crate::host::log_standdown(home, "mcp");
        let rt = tokio::runtime::Runtime::new()?;
        let result = rt.block_on(clax_mcp::standdown::serve(
            clax_mcp::standdown::GROK_STANDDOWN,
        ));
        rt.shutdown_timeout(Duration::from_millis(100));
        return result;
    }
    // Stdout is the MCP channel: diagnostics go to stderr only.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let harness = match a.agent {
        Agent::Claude => Harness::Claude,
        Agent::Codex => Harness::Codex,
        Agent::Grok => Harness::Grok,
    };
    let (refresh_home, port) = (home.clone(), cli.port_for(home)?);
    let refresh: shim::Refresh = Arc::new(move || {
        let c = Client::connect_matching_version(&refresh_home, port)?;
        Ok(Endpoint {
            browser_base: c.browser_url(""),
            base: c.base,
            token: c.token,
        })
    });
    let discover_home = home.clone();
    let discover: shim::Refresh = Arc::new(move || {
        let c = Client::discover(&discover_home)
            .ok_or_else(|| anyhow::anyhow!("no clax daemon is running"))?;
        Ok(Endpoint {
            browser_base: c.browser_url(""),
            base: c.base,
            token: c.token,
        })
    });
    let hold_home = home.clone();
    let upgrade_hold: clax_mcp::tools::UpgradeHoldProbe = Arc::new(move |daemon_version: &str| {
        crate::client::upgrade_hold_for(&hold_home, daemon_version).map(|h| h.to_json(&hold_home))
    });
    let channel = matches!(a.agent, Agent::Claude).then(|| {
        let ch = clax_mcp::channel::ChannelState::detect(parent_pid);
        crate::hooklog::append(
            home,
            &format!(
                "{} channel agent=claude {}",
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                ch.log_fields(parent_pid)
            ),
        );
        ch
    });
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(shim::run(
        harness,
        home,
        refresh,
        discover,
        Duration::from_millis(a.heartbeat_interval_ms.max(1)),
        Some(upgrade_hold),
        channel,
    ));
    // The stdin reader may still be parked on a blocking thread; do not wait for it.
    rt.shutdown_timeout(Duration::from_millis(100));
    result
}

/// `text` on one line: its lines trimmed, blank ones dropped, joined by spaces.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How long a connection to a loopback port may take to be accepted.
const CONNECT_PROBE: Duration = Duration::from_millis(250);

/// Why a daemon started for `home` could not serve on `port`: something that
/// is not this home's Clax daemon accepts connections there, on `127.0.0.1`
/// or `[::1]`. `None` when nothing does, when `port` is 0 (any free port),
/// when `daemon.json` names a live daemon of this home (the shim uses that
/// one, on whatever port), or when this home's start lock is held (a daemon
/// of this home is being started or replaced).
///
/// The check only opens and closes a TCP connection: it sends nothing, and
/// leaves whatever holds the port running.
pub fn port_clash(home: &Home, port: u16) -> Option<String> {
    use clax_server::daemon::{DaemonLock, pid_alive, read_daemon_info};
    let ours = || read_daemon_info(home).is_some_and(|i| pid_alive(i.pid));
    if port == 0 || ours() {
        return None;
    }
    // Held while it decides, so a daemon of this home that starts meanwhile
    // waits for it rather than being taken for a stranger.
    let _lock = if home.root().is_dir() {
        match DaemonLock::try_acquire(home) {
            Ok(None) => return None,
            Ok(Some(lock)) => Some(lock),
            Err(_) => None,
        }
    } else {
        None
    };
    if ours() || !accepts(port) {
        return None;
    }
    Some(format!(
        "port {port} is already in use by another program (not a Clax daemon for {}), and Clax leaves it alone",
        home.root().display()
    ))
}

/// True when a connection to `port` on `127.0.0.1` or `[::1]` is accepted.
fn accepts(port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|ip| TcpStream::connect_timeout(&SocketAddr::new(ip, port), CONNECT_PROBE).is_ok())
}

/// The fix for a [`port_clash`] on `port`, as a clause that follows it:
/// where the port came from, and a free port to set there instead (the
/// first of the next 20 that accepts no connection).
fn port_fix(cli: &crate::Cli, home: &Home, port: u16) -> String {
    let free = (1..=20u16)
        .filter_map(|i| port.checked_add(i))
        .find(|p| !accepts(*p));
    let to = free.map_or_else(|| "<a free port>".to_string(), |p| p.to_string());
    let config = home.root().join(clax_core::config::FILE);
    if cli.port.is_some() {
        format!("; pass another port to --port, such as {to}")
    } else if std::env::var_os(crate::PORT_ENV).is_some_and(|v| !v.is_empty()) {
        format!(
            "; set {} (it is {port} now) to another port, such as {to}",
            crate::PORT_ENV
        )
    } else {
        format!(
            "; give Clax another port: set `port = {to}` under `[serve]` in {}, or set {}={to} in the agent's environment",
            config.display(),
            crate::PORT_ENV
        )
    }
}
