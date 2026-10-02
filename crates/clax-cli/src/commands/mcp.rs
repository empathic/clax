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
    /// `config.toml` and the port, print nothing, and exit 0; on failure print
    /// a one-line `error: <reason>` and exit 1. Starts and contacts no daemon.
    /// The plugins' wrapper runs it before it execs `clax mcp`.
    #[arg(long, hide = true)]
    pub preflight: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.preflight {
        return cli
            .port_for(home)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{}", one_line(&format!("{e:#}"))));
    }
    // SAFETY: getppid has no preconditions and cannot fail.
    let parent_pid = unsafe { libc::getppid() } as u32;
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
