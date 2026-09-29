use crate::client::Client;
use artifax_core::Home;
use artifax_hooks::events::{self, Daemon};
use artifax_hooks::input::HookInput;
use artifax_hooks::output::HookOutput;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

/// The whole `session-start` invocation is abandoned after this long.
const START_DEADLINE: Duration = Duration::from_secs(4);
/// Each `session-start` daemon request is abandoned after this long.
const START_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// The whole `session-end` invocation is abandoned after this long: Codex
/// kills its `SessionEnd` hook after 3 s.
const END_DEADLINE: Duration = Duration::from_millis(2500);
/// Each `session-end` daemon request is abandoned after this long.
const END_REQUEST_TIMEOUT: Duration = Duration::from_millis(2000);
/// A Stop hook invocation is abandoned after this long (the plugins give it 10 s).
const STOP_DEADLINE: Duration = Duration::from_secs(8);
/// A prompt-submit hook invocation is abandoned after this long.
const PROMPT_DEADLINE: Duration = Duration::from_secs(4);
/// Each daemon request of the Stop and prompt hooks is abandoned after this long.
const FEEDBACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// How many ancestors above the hook's parent are reported for session joining.
const MAX_ANCESTORS: usize = 6;

/// The parent of `pid`, if it can be determined.
fn parent_of(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}

/// Ancestors of `pid`, nearest first, up to [`MAX_ANCESTORS`], stopping at pid 1.
fn ancestors(pid: u32) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut cur = pid;
    while chain.len() < MAX_ANCESTORS {
        match parent_of(cur) {
            Some(p) if p > 1 && !chain.contains(&p) && p != pid => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
}

impl Event {
    /// The deadline for the whole invocation and for each daemon request.
    fn budget(self) -> (Duration, Duration) {
        match self {
            Event::SessionStart => (START_DEADLINE, START_REQUEST_TIMEOUT),
            Event::SessionEnd => (END_DEADLINE, END_REQUEST_TIMEOUT),
            Event::Stop => (STOP_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
            Event::Prompt => (PROMPT_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
        }
    }
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
    /// The agent is about to stop; hand it pending feedback by blocking the stop.
    Stop,
    /// The person submitted a prompt; add pending feedback as context.
    Prompt,
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
    let (deadline, _) = event.budget();
    match rx.recv_timeout(deadline) {
        Ok(Ok(out)) => {
            if let Some(line) = out.to_line() {
                let _ = writeln!(std::io::stdout(), "{line}");
            }
        }
        Ok(Err(e)) => eprintln!("artifax hook: {e:#}"),
        Err(_) => eprintln!("artifax hook: timed out after {deadline:?}"),
    }
    // Exit now: a timed-out worker may still be blocked on stdin or the network.
    std::process::exit(0);
}

fn handle(agent: Agent, event: Event, parent_pid: u32, home: &Home) -> anyhow::Result<HookOutput> {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let input = HookInput::parse(&stdin);
    let client = Client::discover(home)
        .ok_or_else(|| anyhow::anyhow!("no artifax daemon is running"))?
        .with_timeout(event.budget().1);
    match event {
        Event::SessionStart => events::session_start(
            agent.harness(),
            parent_pid,
            &ancestors(parent_pid),
            &input,
            &client,
        ),
        Event::SessionEnd => events::session_end(agent.harness(), &input, &client),
        Event::Stop => events::stop(agent.harness(), &input, &client),
        Event::Prompt => events::prompt(agent.harness(), &input, &client),
    }
}
