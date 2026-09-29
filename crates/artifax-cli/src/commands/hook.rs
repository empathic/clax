use crate::client::Client;
use artifax_core::Home;
use artifax_hooks::events::{self, Daemon};
use artifax_hooks::input::HookInput;
use artifax_hooks::output::HookOutput;
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

/// The whole hook invocation is abandoned after this long.
const DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    fn harness(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, clap::Subcommand)]
pub enum Event {
    /// A harness session started.
    SessionStart,
    /// A harness session ended.
    SessionEnd,
}

#[derive(clap::Args)]
pub struct Args {
    /// The harness running this hook.
    #[arg(long, value_enum)]
    pub agent: Agent,
    #[command(subcommand)]
    pub event: Event,
}

impl Daemon for Client {
    fn browser_url(&self, path: &str) -> String {
        Client::browser_url(self, path)
    }
    fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        Client::get(self, path)
    }
    fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Client::post(self, path, body)
    }
    fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Client::patch(self, path, body)
    }
}

/// Runs the hook. Never fails the harness: any error or timeout prints one
/// line to stderr and leaves stdout empty. Never starts a daemon.
pub fn run(_cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    // SAFETY: getppid has no preconditions.
    let parent_pid = unsafe { libc::getppid() } as u32;
    let (agent, event, home) = (a.agent, a.event, home.clone());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle(agent, event, parent_pid, &home));
    });
    match rx.recv_timeout(DEADLINE) {
        Ok(Ok(out)) => {
            if let Some(line) = out.to_line() {
                println!("{line}");
            }
        }
        Ok(Err(e)) => eprintln!("artifax hook: {e:#}"),
        Err(_) => eprintln!("artifax hook: timed out after {}s", DEADLINE.as_secs()),
    }
    // Exit now: a timed-out worker may still be blocked on stdin or the network.
    std::process::exit(0);
}

fn handle(agent: Agent, event: Event, parent_pid: u32, home: &Home) -> anyhow::Result<HookOutput> {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let input = HookInput::parse(&stdin);
    let client =
        Client::discover(home).ok_or_else(|| anyhow::anyhow!("no artifax daemon is running"))?;
    match event {
        Event::SessionStart => events::session_start(agent.harness(), parent_pid, &input, &client),
        Event::SessionEnd => events::session_end(agent.harness(), &input, &client),
    }
}
