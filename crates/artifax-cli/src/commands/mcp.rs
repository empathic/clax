use crate::client::Client;
use artifax_core::Home;
use artifax_mcp::shim::{self, Endpoint, Harness};
use std::sync::Arc;
use std::time::Duration;

/// The harness that spawns the shim.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Pi,
}

#[derive(clap::Args)]
pub struct Args {
    /// The harness running this MCP server.
    #[arg(long, value_enum, default_value_t = Agent::Claude)]
    pub agent: Agent,
    /// Milliseconds between session heartbeats.
    #[arg(long, hide = true, default_value_t = shim::DEFAULT_HEARTBEAT.as_millis() as u64)]
    pub heartbeat_interval_ms: u64,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
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
        Agent::Pi => Harness::Pi,
    };
    let (refresh_home, port) = (home.clone(), cli.port);
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
            .ok_or_else(|| anyhow::anyhow!("no artifax daemon is running"))?;
        Ok(Endpoint {
            browser_base: c.browser_url(""),
            base: c.base,
            token: c.token,
        })
    });
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(shim::run(
        harness,
        home,
        refresh,
        discover,
        Duration::from_millis(a.heartbeat_interval_ms.max(1)),
    ));
    // The stdin reader may still be parked on a blocking thread; do not wait for it.
    rt.shutdown_timeout(Duration::from_millis(100));
    result
}
