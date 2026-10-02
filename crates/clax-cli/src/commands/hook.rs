use crate::client::Client;
use clax_core::Home;
use clax_hooks::events::{self, Daemon};
use clax_hooks::input::HookInput;
use clax_hooks::output::HookOutput;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
/// Grok Build gives `SessionEnd` hooks 1.5 s by default, so `session-end
/// --agent grok` gives up after this long.
const GROK_END_DEADLINE: Duration = Duration::from_millis(1200);
/// Each Grok `session-end` daemon request is abandoned after this long.
const GROK_END_REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);
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
    Grok,
}

/// The deadline for the whole invocation and for each daemon request.
fn budget(agent: Agent, event: Event) -> (Duration, Duration) {
    match (agent, event) {
        (Agent::Grok, Event::SessionEnd) => (GROK_END_DEADLINE, GROK_END_REQUEST_TIMEOUT),
        (_, Event::SessionStart) => (START_DEADLINE, START_REQUEST_TIMEOUT),
        (_, Event::SessionEnd) => (END_DEADLINE, END_REQUEST_TIMEOUT),
        (_, Event::Stop) => (STOP_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
        (_, Event::Prompt) => (PROMPT_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
    }
}

impl Event {
    /// The event's name on the command line.
    fn name(self) -> &'static str {
        match self {
            Event::SessionStart => "session-start",
            Event::SessionEnd => "session-end",
            Event::Stop => "stop",
            Event::Prompt => "prompt",
        }
    }
}

impl Agent {
    fn harness(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Grok => "grok",
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
    fn get_with_timeout(&self, path: &str, timeout: Duration) -> anyhow::Result<serde_json::Value> {
        Client::get_with_timeout(self, path, timeout)
    }
    fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Client::post(self, path, body)
    }
    fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Client::patch(self, path, body)
    }
}

/// Appends this run's line to hooks.log (see [`crate::hooklog`]).
pub fn log_run(home: &Home, agent: &str, event: &str, started: Instant, stderr: Option<&str>) {
    let bin = std::env::current_exe().unwrap_or_default();
    crate::hooklog::append(
        home,
        &crate::hooklog::hook_line(
            chrono::Utc::now(),
            agent,
            event,
            &bin,
            started.elapsed(),
            0,
            stderr,
        ),
    );
}

/// Runs the hook. Never fails the harness: any error or timeout prints one
/// line to stderr and leaves stdout empty. Never starts a daemon. Each run is
/// logged to hooks.log. A Claude Code hook that Grok Build runs stands down:
/// it prints nothing and logs one standdown line.
pub fn run(_cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let started = Instant::now();
    // SAFETY: getppid has no preconditions.
    let parent_pid = unsafe { libc::getppid() } as u32;
    let (agent, event) = (a.agent, a.event);
    let worker_home = home.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle(agent, event, parent_pid, &worker_home));
    });
    let (deadline, _) = budget(agent, event);
    let error = match rx.recv_timeout(deadline) {
        // Stood down: logged by `handle`, nothing to print or log here.
        Ok(Ok(None)) => std::process::exit(0),
        Ok(Ok(Some(out))) => {
            if let Some(line) = out.to_line() {
                let _ = writeln!(std::io::stdout(), "{line}");
            }
            None
        }
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(_) => Some(format!("timed out after {deadline:?}")),
    };
    if let Some(e) = &error {
        eprintln!("clax hook: {e}");
    }
    log_run(
        home,
        agent.harness(),
        event.name(),
        started,
        error.as_deref(),
    );
    // Exit now: a timed-out worker may still be blocked on stdin or the network.
    std::process::exit(0);
}

/// Handles one hook run; `None` means it stood down.
fn handle(
    agent: Agent,
    event: Event,
    parent_pid: u32,
    home: &Home,
) -> anyhow::Result<Option<HookOutput>> {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let input = HookInput::parse(&stdin);
    if matches!(agent, Agent::Claude)
        && crate::host::grok_runs_hook(|k| std::env::var(k).ok(), &input)
    {
        // Grok Build runs the Claude Code copy's hooks too; in a Grok
        // session only clax-grok's hooks act (spec D17).
        crate::host::log_standdown(home, "hook");
        return Ok(None);
    }
    // Grok Build discards an allowing prompt hook's output, and a
    // prompt_hook request marks comments delivered, so Grok's prompt hook
    // does nothing (the plugin does not wire it).
    if matches!((agent, event), (Agent::Grok, Event::Prompt)) {
        return Ok(Some(HookOutput::none()));
    }
    let client = Client::discover(home)
        .ok_or_else(|| anyhow::anyhow!("no clax daemon is running"))?
        .with_timeout(budget(agent, event).1);
    // Codex's `codex queue` finds its app-server socket under CODEX_HOME, which
    // the daemon's own environment may lack.
    let codex_home = std::env::var("CODEX_HOME")
        .ok()
        .filter(|v| !v.is_empty() && matches!(agent, Agent::Codex));
    let out = match event {
        Event::SessionStart if matches!(agent, Agent::Grok) => events::session_start_quiet(
            agent.harness(),
            parent_pid,
            &ancestors(parent_pid),
            &input,
            &client,
        ),
        Event::SessionStart => events::session_start(
            agent.harness(),
            parent_pid,
            &ancestors(parent_pid),
            &input,
            codex_home.as_deref(),
            &client,
        ),
        Event::SessionEnd => events::session_end(agent.harness(), &input, &client),
        Event::Stop => events::stop(agent.harness(), &input, &client),
        Event::Prompt => events::prompt(agent.harness(), &input, &client),
    };
    out.map(Some)
}
