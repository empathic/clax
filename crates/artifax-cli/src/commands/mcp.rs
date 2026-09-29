use crate::client::Client;
use artifax_core::Home;
use artifax_mcp::shim::{self, Endpoint, Harness};
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
    let (connect_home, port) = (home.clone(), cli.port);
    let connect = move || {
        let c = Client::connect(&connect_home, port)?;
        reap(c.info.pid);
        Ok(Endpoint {
            browser_base: c.browser_url(""),
            base: c.base,
            token: c.token,
        })
    };
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(shim::run(
        harness,
        home,
        connect,
        Duration::from_millis(a.heartbeat_interval_ms.max(1)),
    ));
    // The stdin reader may still be parked on a blocking thread; do not wait for it.
    rt.shutdown_timeout(Duration::from_millis(100));
    result
}

/// Waits on `pid` in the background so that a daemon this process started is
/// reaped when it exits, instead of lingering as a zombie that still looks alive
/// to `artifax stop` for as long as the shim runs. A PID that is not our child
/// returns at once.
fn reap(pid: u32) {
    std::thread::spawn(move || {
        let mut status = 0;
        // SAFETY: waitpid on a single PID with a valid status pointer.
        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
    });
}
