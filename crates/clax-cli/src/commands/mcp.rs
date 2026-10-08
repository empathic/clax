use crate::client::Client;
use clax_core::Home;
use clax_mcp::shim::{self, Endpoint, Harness};
use clax_server::daemon::{
    DaemonLock, first_unheld_port, pid_alive, port_held, port_range, read_daemon_info,
};
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
    /// `config.toml` and the port, and check that a daemon could serve there
    /// ([`port_clash`]); print nothing and exit 0; on failure
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
            .and_then(|port| match port_clash(cli, home, port) {
                Some(why) => Err(anyhow::anyhow!("{why}")),
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

/// Why a daemon started for `home` could not serve on `port`, which another
/// program holds ([`port_held`]): when `port` was set explicitly (`--port`
/// or `CLAX_PORT`), whenever it is held; otherwise only when every port the
/// daemon would try ([`port_range`]) is held, since the daemon moves to the
/// first free one. `None` when `port` is 0 (any free port), when
/// `daemon.json` names a live daemon of this home (the shim uses that one,
/// on whatever port), or when this home's start lock is held (a daemon of
/// this home is being started or replaced). Whatever holds a port is left
/// running.
pub fn port_clash(cli: &crate::Cli, home: &Home, port: u16) -> Option<String> {
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
    if ours() || !port_held(port) {
        return None;
    }
    let free = first_unheld_port(port);
    let to = free.map_or_else(|| "<a free port>".to_string(), |p| p.to_string());
    let who = format!("not a Clax daemon for {}", home.root().display());
    if cli.port.is_some() {
        return Some(format!(
            "port {port} is already in use by another program ({who}), and Clax leaves it alone; pass another port to --port, such as {to}"
        ));
    }
    if std::env::var_os(crate::PORT_ENV).is_some_and(|v| !v.is_empty()) {
        return Some(format!(
            "port {port} is already in use by another program ({who}), and Clax leaves it alone; set {} (it is {port} now) to another port, such as {to}",
            crate::PORT_ENV
        ));
    }
    if free.is_some() {
        // The daemon moves to that port.
        return None;
    }
    let range = port_range(port);
    Some(format!(
        "ports {}-{} are all in use by other programs ({who}), and Clax leaves them alone; give Clax another port: set `port = <a free port>` under `[serve]` in {}, or set {}=<a free port> in the agent's environment",
        range.start,
        range.end - 1,
        home.root().join(clax_core::config::FILE).display(),
        crate::PORT_ENV
    ))
}
