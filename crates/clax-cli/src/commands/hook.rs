use crate::client::Client;
use clax_core::Home;
use clax_core::gitctx::{self, GitField};
use clax_hooks::ask::{self, Asked, Budget};
use clax_hooks::events::{self, Daemon};
use clax_hooks::input::HookInput;
use clax_hooks::output::HookOutput;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock, mpsc};
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
/// The whole `tool` (PostToolUse) invocation is abandoned after this long.
const TOOL_DEADLINE: Duration = Duration::from_secs(2);
/// Each `tool` daemon request is abandoned after this long.
const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// The whole `call-id` (PostToolUse on a Clax tool) invocation is
/// abandoned after this long: within the 5 s the plugin gives it, past the
/// daemon's wait for a call reported late (`CALL_ID_GRACE`, 500 ms).
const CALL_ID_DEADLINE: Duration = Duration::from_secs(3);
/// Each `call-id` daemon request is abandoned after this long.
const CALL_ID_REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
/// Grok Build gives `SessionEnd` hooks 1.5 s by default, so `session-end
/// --agent grok` gives up after this long.
const GROK_END_DEADLINE: Duration = Duration::from_millis(1200);
/// Each Grok `session-end` daemon request is abandoned after this long.
const GROK_END_REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);
/// `ask` (PreToolUse on `AskUserQuestion`): reading stdin, finding the
/// daemon, the session lookup and the question's creation share this
/// deadline; past it the hook prints nothing and the terminal dialog opens.
/// The long poll that follows has its own timeout (see [`ask::Budget`]).
const ASK_SETUP_DEADLINE: Duration = Duration::from_secs(2);
/// Each `ask` daemon request other than the long poll is abandoned after this long.
const ASK_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// After the setup, `ask` gives up waiting on its worker this long after the
/// poll's own timeout: up to three 1 s requests to release the question,
/// with a margin. Claude Code stops the hook at 3600 s; the longest poll
/// timeout is 3310 s.
const ASK_AFTER_POLL: Duration = Duration::from_secs(5);
/// The whole `asked` (PostToolUse on `AskUserQuestion`) invocation is
/// abandoned after this long.
const ASKED_DEADLINE: Duration = Duration::from_secs(2);
/// Each `asked` daemon request is abandoned after this long.
const ASKED_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
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
        (_, Event::Tool) => (TOOL_DEADLINE, TOOL_REQUEST_TIMEOUT),
        (_, Event::CallId) => (CALL_ID_DEADLINE, CALL_ID_REQUEST_TIMEOUT),
        (_, Event::Ask) => (ASK_SETUP_DEADLINE, ASK_REQUEST_TIMEOUT),
        (_, Event::Asked) => (ASKED_DEADLINE, ASKED_REQUEST_TIMEOUT),
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
            Event::Tool => "tool",
            Event::CallId => "call-id",
            Event::Ask => "ask",
            Event::Asked => "asked",
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
    /// A tool call finished; renew the session's working records.
    Tool,
    /// A Clax tool call finished; report the harness's ID for it (Claude
    /// Code only).
    CallId,
    /// Claude Code is about to show AskUserQuestion; offer it in Clax first
    /// (Claude Code only).
    Ask,
    /// AskUserQuestion was answered in the terminal; record the answer in
    /// Clax (Claude Code only).
    Asked,
}

#[derive(clap::Args)]
pub struct Args {
    /// The harness running this hook.
    #[arg(long, value_enum)]
    pub agent: Agent,
    #[command(subcommand)]
    pub event: Event,
}

/// The channel `clax hook` names on every request (spec
/// 2026-10-06-toolpath-audit-design §6.9).
pub const VIA: &str = "hook";

/// Whether `event`'s hook captures the git state of its working directory
/// (spec §9.3): the session join, the hook-driven questions, and the Stop
/// hook, whose turn end ends the session's working records.
fn captures_git(event: Event) -> bool {
    matches!(
        event,
        Event::SessionStart | Event::Stop | Event::Ask | Event::Asked
    )
}

/// The git state of the hook's working directory, captured once, on a
/// thread of its own, from the hook's first daemon request on: a hook that
/// makes no request runs no git.
struct HookGit {
    cwd: Option<String>,
    task: Mutex<Option<std::thread::JoinHandle<GitField>>>,
    done: OnceLock<GitField>,
}

impl HookGit {
    fn new(cwd: Option<&str>) -> HookGit {
        HookGit {
            cwd: cwd.map(str::to_string),
            task: Mutex::new(None),
            done: OnceLock::new(),
        }
    }

    /// Starts the capture unless it has started.
    fn start(&self) {
        if self.done.get().is_some() {
            return;
        }
        let mut task = self.task.lock().unwrap_or_else(|e| e.into_inner());
        if task.is_some() {
            return;
        }
        let cwd = self.cwd.clone().filter(|c| !c.is_empty());
        // The deadline starts now: the wait for git's version counts
        // against it.
        let deadline = Instant::now() + clax_mcp::git::deadline_from_env();
        *task = Some(std::thread::spawn(move || {
            let Some(cwd) = cwd else {
                return GitField::Capture("no-cwd");
            };
            match gitctx::find_git(std::env::var_os("PATH").as_deref()) {
                Some(git) => gitctx::GitProbe::new(&git).capture(Path::new(&cwd), deadline),
                None => GitField::Capture("unavailable"),
            }
        }));
    }

    /// The capture, waiting for it (it ends by its deadline).
    fn wait(&self) -> &GitField {
        self.start();
        if let Some(t) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let field = t.join().unwrap_or(GitField::Capture("unavailable"));
            let _ = self.done.set(field);
        }
        self.done.get().unwrap_or(&GitField::Capture("unavailable"))
    }

    /// The capture if it has finished, starting it if it has not started.
    fn now(&self) -> Option<&GitField> {
        self.start();
        let finished = self
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|t| t.is_finished());
        if finished {
            return Some(self.wait());
        }
        self.done.get()
    }
}

/// The daemon as a hook talks to it: every request names the `hook`
/// channel, and, for an event that captures git, carries `x-clax-git`.
/// Every request but a `GET` waits for the capture; a `GET` carries it
/// once it is there.
struct HookDaemon<'a> {
    client: &'a Client,
    git: Option<HookGit>,
}

impl<'a> HookDaemon<'a> {
    fn new(client: &'a Client, event: Event, input: &HookInput) -> HookDaemon<'a> {
        HookDaemon {
            client,
            git: captures_git(event).then(|| HookGit::new(input.cwd.as_deref())),
        }
    }

    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
    ) -> anyhow::Result<serde_json::Value> {
        let git = self.git.as_ref().and_then(|g| {
            if method == reqwest::Method::GET {
                g.now()
            } else {
                Some(g.wait())
            }
        });
        let header = git.and_then(gitctx::encode_header);
        let headers: Vec<(&str, &str)> = header
            .as_deref()
            .map(|h| ("x-clax-git", h))
            .into_iter()
            .collect();
        self.client
            .request_with(method, path, body, timeout, &headers)
    }
}

impl Daemon for HookDaemon<'_> {
    fn browser_url(&self, path: &str) -> String {
        self.client.browser_url(path)
    }
    fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::GET, path, None, None)
    }
    fn get_with_timeout(&self, path: &str, timeout: Duration) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::GET, path, None, Some(timeout))
    }
    fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::POST, path, Some(body), None)
    }
    fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::PATCH, path, Some(body), None)
    }
}

/// Appends this run's line to hooks.log (see [`crate::hooklog`]), with
/// `detail` (`key=value` pairs, never hook input) at its end.
pub fn log_run(
    home: &Home,
    agent: &str,
    event: &str,
    started: Instant,
    stderr: Option<&str>,
    detail: Option<&str>,
) {
    let bin = std::env::current_exe().unwrap_or_default();
    let mut line = crate::hooklog::hook_line(
        chrono::Utc::now(),
        agent,
        event,
        &bin,
        started.elapsed(),
        0,
        stderr,
    );
    if let Some(d) = detail {
        line.push(' ');
        line.push_str(d);
    }
    crate::hooklog::append(home, &line);
}

/// Runs the hook. Never fails the harness: any error or timeout prints one
/// line to stderr and leaves stdout empty, except for a Codex session
/// start's approval notice ([`codex_notice`]), which is printed either way.
/// Never starts a daemon. Each run is
/// logged to hooks.log. A Claude Code hook that Grok Build runs stands down:
/// it prints nothing and logs one standdown line.
pub fn run(_cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let started = Instant::now();
    let parent_pid = std::os::unix::process::parent_id();
    let (agent, event) = (a.agent, a.event);
    if matches!(event, Event::Ask) {
        run_ask(home, agent, started);
    }
    let (deadline, _) = budget(agent, event);
    // The notice is worked out beside the hook's own work, within its deadline.
    let notice_rx = matches!((agent, event), (Agent::Codex, Event::SessionStart)).then(|| {
        let (tx, rx) = mpsc::channel();
        let notice_home = home.clone();
        std::thread::spawn(move || {
            let _ = tx.send(codex_notice(&notice_home, parent_pid));
        });
        rx
    });
    let worker_home = home.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle(agent, event, parent_pid, &worker_home));
    });
    let (out, error) = match rx.recv_timeout(deadline) {
        // Stood down: logged by `handle`, nothing to print or log here.
        Ok(Ok(None)) => std::process::exit(0),
        Ok(Ok(Some(out))) => (out, None),
        // `asked` carries the terminal's answers: its log keeps only the
        // error code, never a message that could quote them.
        Ok(Err(e)) if matches!(event, Event::Asked) => {
            (HookOutput::none(), Some(error_code(&format!("{e:#}"))))
        }
        Ok(Err(e)) => (HookOutput::none(), Some(format!("{e:#}"))),
        Err(_) => (
            HookOutput::none(),
            Some(format!("timed out after {deadline:?}")),
        ),
    };
    let notice = notice_rx.and_then(|rx| {
        rx.recv_timeout(deadline.saturating_sub(started.elapsed()))
            .ok()
            .flatten()
    });
    let out = match &notice {
        Some(n) => out.with_system_message(&n.text),
        None => out,
    };
    if let Some(line) = out.to_line()
        && writeln!(std::io::stdout(), "{line}").is_ok()
        && let Some(n) = &notice
    {
        n.record();
    }
    if let Some(e) = &error {
        eprintln!("clax hook: {e}");
    }
    log_run(
        home,
        agent.harness(),
        event.name(),
        started,
        error.as_deref(),
        None,
    );
    // Exit now: a timed-out worker may still be blocked on stdin or the network.
    std::process::exit(0);
}

/// A session-start notice to show, and how to record that it was shown.
struct Notice {
    text: String,
    marker: std::path::PathBuf,
    tools: Vec<String>,
}

impl Notice {
    /// Records the notice as shown; called once it is printed.
    fn record(&self) {
        let tools: Vec<&str> = self.tools.iter().map(String::as_str).collect();
        crate::codex_approvals::record_notice(&self.marker, &tools);
    }
}

/// For a Codex session start: the notice that Codex will stop to ask
/// before Clax tools, with how to approve them once
/// ([`crate::codex_approvals::notice`]). Shown once per set of tools (a
/// marker per Codex home under `<home>/run/`, written by [`Notice::record`]
/// once printed, or by `clax init` when the person declines that set);
/// `clax doctor` reports the tools after that. Not given to a session run
/// by `codex exec` or `codex app-server`, which show no hook message to a
/// person. Reads Codex's config and writes nothing there.
fn codex_notice(home: &Home, parent_pid: u32) -> Option<Notice> {
    use crate::codex_approvals as ca;
    let dirs = super::doctor_agent::Dirs::from_env(|k| std::env::var(k).ok())?;
    let path = ca::config_path(&dirs.codex_home);
    let text = ca::read_config(&path).ok()?;
    let a = ca::assess(text.as_deref(), &clax_mcp::tools::ClaxTools::tools()).ok()?;
    if !a.registered {
        return None;
    }
    let marker = ca::notice_marker(home.root(), &dirs.codex_home);
    let tools = a.addable();
    if !ca::notice_due(&marker, &tools) || !shown_to_a_person(parent_pid) {
        return None;
    }
    let text = ca::notice(&a, &path, clax_command().as_deref())?;
    Some(Notice {
        text,
        marker,
        tools: tools.into_iter().map(str::to_string).collect(),
    })
}

/// Codex subcommands whose sessions show no hook message to a person.
/// Where only `ps` gives the arguments (macOS), they are joined with
/// spaces, so a prompt given as Codex's first argument whose first word is
/// one of these reads as the subcommand; the
/// notice is then neither shown nor recorded, and `clax doctor` still
/// reports the tools. `review` is left out: a prompt starting "review" is
/// likelier than `codex review`, whose session start is rare.
const HEADLESS_CODEX: &[&str] = &["exec", "e", "app-server", "exec-server", "mcp-server"];

/// Codex options that take a value as the next argument (`-c key=value`).
const CODEX_VALUE_OPTIONS: &[&str] = &[
    "-c",
    "--config",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-P",
    "--permission-profile",
    "-C",
    "--cd",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-i",
    "--image",
    "--enable",
    "--disable",
    "--local-provider",
    "--remote",
    "--add-dir",
];

/// For one process's arguments: `None` when it is not Codex, else whether
/// it is an interactive Codex: its first argument that is not an option or
/// an option's value is not a headless subcommand (a prompt, or nothing, is
/// interactive).
fn interactive_codex(tokens: &[&str]) -> Option<bool> {
    let at = tokens.iter().take(2).position(|t| {
        let base = t.rsplit('/').next().unwrap_or(t);
        base == "codex" || base == "codex.js"
    })?;
    let mut rest = tokens[at + 1..].iter();
    while let Some(t) = rest.next() {
        if *t == "--" {
            return Some(true);
        }
        if t.starts_with('-') {
            if CODEX_VALUE_OPTIONS.contains(t) {
                rest.next();
            }
            continue;
        }
        return Some(!HEADLESS_CODEX.contains(t));
    }
    Some(true)
}

/// The arguments in a `/proc/<pid>/cmdline` (NUL-separated, exact).
fn cmdline_args(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// A process's arguments: exact from `/proc/<pid>/cmdline` where there is
/// one (Linux), else `ps -o args=` split at whitespace, which cannot tell a
/// quoted prompt from separate words.
fn process_args(pid: u32) -> Option<Vec<String>> {
    if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        return Some(cmdline_args(&raw));
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect(),
    )
}

/// Whether the nearest Codex among the hook's ancestors runs interactively;
/// true when none is found.
fn shown_to_a_person(parent_pid: u32) -> bool {
    std::iter::once(parent_pid)
        .chain(ancestors(parent_pid))
        .find_map(|pid| {
            let args = process_args(pid)?;
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            interactive_codex(&args)
        })
        .unwrap_or(true)
}

/// `clax` when an executable `clax` is on `PATH`, so the person can run it
/// from a terminal; `None` otherwise (the plugin then runs a binary of its
/// own, and the notice names the settings instead).
fn clax_command() -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .any(|d| {
            std::fs::metadata(d.join("clax"))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .then(|| "clax".to_string())
}

/// The daemon's error code that leads `error` (`code: message`), or
/// "request failed"; never the message.
fn error_code(error: &str) -> String {
    error
        .split_once(':')
        .map(|(c, _)| c)
        .filter(|c| !c.is_empty() && c.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .unwrap_or("request failed")
        .to_string()
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
        .with_timeout(budget(agent, event).1)
        .with_via(VIA);
    let client = HookDaemon::new(&client, event, &input);
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
        Event::SessionEnd => {
            let out = events::session_end(agent.harness(), &input, &client);
            if let Some(sid) = input.session_id.as_deref().filter(|s| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            }) {
                // The PostToolUse gate's stamp (scripts/tool-hook.sh).
                let _ = std::fs::remove_file(
                    home.root()
                        .join("run/tool-hook")
                        .join(format!("{}-{sid}", agent.harness())),
                );
            }
            out
        }
        Event::Stop => events::stop(agent.harness(), &input, &client),
        Event::Prompt => events::prompt(agent.harness(), &input, &client),
        Event::Tool => events::tool(agent.harness(), &input, &client),
        Event::CallId if matches!(agent, Agent::Claude) => {
            events::tool_call_id(agent.harness(), &input, &client)
        }
        Event::CallId => Ok(HookOutput::none()),
        Event::Asked if matches!(agent, Agent::Claude) => ask::asked(&input, &client),
        // `ask` runs in `run_ask`; neither is wired for other harnesses.
        Event::Ask | Event::Asked => Ok(HookOutput::none()),
    };
    out.map(Some)
}

/// What `run_ask`'s worker reports.
enum AskMsg {
    /// The long poll, with this timeout, is starting: the setup is over.
    Polling(Duration),
    /// The run is over.
    Done(Asked),
    /// The run stood down (a Grok Build session) or the agent is not Claude
    /// Code: nothing is printed or logged here.
    Quiet,
    /// No daemon could be reached.
    Failed(String),
}

/// A [`Daemon`] that reports when the long poll starts.
struct Reporting<'a> {
    client: &'a HookDaemon<'a>,
    tx: mpsc::Sender<AskMsg>,
}

impl Daemon for Reporting<'_> {
    fn browser_url(&self, path: &str) -> String {
        self.client.browser_url(path)
    }
    fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.client.get(path)
    }
    fn get_with_timeout(&self, path: &str, timeout: Duration) -> anyhow::Result<serde_json::Value> {
        let _ = self.tx.send(AskMsg::Polling(timeout));
        self.client.get_with_timeout(path, timeout)
    }
    fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.client.post(path, body)
    }
    fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.client.patch(path, body)
    }
}

/// `clax hook ask`: runs outside [`run`]'s single deadline, because its
/// long poll may hold for up to `terminal_after_s`. Everything before the
/// poll has [`ASK_SETUP_DEADLINE`]; the poll its own timeout. Prints the
/// decision, if any, logs `ask mode=… outcome=… waited_s=…` (never question
/// or answer text), and exits 0.
fn run_ask(home: &Home, agent: Agent, started: Instant) -> ! {
    let (tx, rx) = mpsc::channel();
    let worker_home = home.clone();
    std::thread::spawn(move || {
        let msg = ask_worker(agent, &worker_home, tx.clone());
        let _ = tx.send(msg);
    });
    let setup = ASK_SETUP_DEADLINE.saturating_sub(started.elapsed());
    let msg = match rx.recv_timeout(setup) {
        Ok(AskMsg::Polling(poll)) => rx.recv_timeout(poll + ASK_AFTER_POLL),
        other => other,
    };
    let (asked, error) = match msg {
        Ok(AskMsg::Quiet) => std::process::exit(0),
        Ok(AskMsg::Done(a)) => (Some(a), None),
        Ok(AskMsg::Failed(e)) => (None, Some(e)),
        Ok(AskMsg::Polling(_)) => (None, Some("polled twice".to_string())),
        Err(_) => (None, Some("timed out".to_string())),
    };
    if let Some(line) = asked.as_ref().and_then(|a| a.out.to_line()) {
        let _ = writeln!(std::io::stdout(), "{line}");
    }
    if let Some(e) = &error {
        eprintln!("clax hook: {e}");
    }
    let detail = match &asked {
        Some(a) => format!(
            "ask mode={} outcome={} waited_s={}",
            a.mode.unwrap_or("-"),
            a.outcome.as_str(),
            a.waited.as_secs()
        ),
        None => "ask mode=- outcome=error waited_s=0".to_string(),
    };
    log_run(
        home,
        agent.harness(),
        Event::Ask.name(),
        started,
        error.as_deref(),
        Some(&detail),
    );
    // Exit now: a timed-out worker may still be blocked on stdin or the network.
    std::process::exit(0);
}

fn ask_worker(agent: Agent, home: &Home, tx: mpsc::Sender<AskMsg>) -> AskMsg {
    if !matches!(agent, Agent::Claude) {
        return AskMsg::Quiet;
    }
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let input = HookInput::parse(&stdin);
    if crate::host::grok_runs_hook(|k| std::env::var(k).ok(), &input) {
        crate::host::log_standdown(home, "hook");
        return AskMsg::Quiet;
    }
    let Some(client) = Client::discover(home) else {
        return AskMsg::Failed("no clax daemon is running".to_string());
    };
    let client = client.with_timeout(ASK_REQUEST_TIMEOUT).with_via(VIA);
    let client = HookDaemon::new(&client, Event::Ask, &input);
    let daemon = Reporting {
        client: &client,
        tx,
    };
    AskMsg::Done(ask::ask_logged(&input, &daemon, Budget::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Command lines as `ps -o args=` showed them for a Codex TUI, `codex
    /// exec` and the app server.
    fn interactive(line: &str) -> Option<bool> {
        interactive_codex(&line.split_whitespace().collect::<Vec<_>>())
    }

    #[test]
    fn exact_arguments_keep_a_quoted_prompt_whole() {
        let argv = cmdline_args(b"/usr/bin/codex\0-m\0gpt-6\0exec the plan\0");
        assert_eq!(argv, ["/usr/bin/codex", "-m", "gpt-6", "exec the plan"]);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(interactive_codex(&argv), Some(true));
        assert_eq!(interactive_codex(&["codex", "exec", "hi"]), Some(false));
    }

    #[test]
    fn only_an_interactive_codex_is_shown_the_notice() {
        let bin =
            "/Users/u/.codex/packages/standalone/releases/0.160.1-aarch64-apple-darwin/bin/codex";
        assert_eq!(
            interactive(&format!("{bin} --dangerously-bypass-hook-trust")),
            Some(true)
        );
        assert_eq!(interactive("codex"), Some(true));
        assert_eq!(
            interactive(&format!("{bin} exec --skip-git-repo-check hi")),
            Some(false)
        );
        assert_eq!(interactive("codex app-server"), Some(false));
        assert_eq!(
            interactive("node /usr/lib/node_modules/@openai/codex/bin/codex.js exec x"),
            Some(false)
        );
        assert_eq!(interactive("/bin/zsh -c codex exec"), None);
        // Only the first argument that is not an option or its value counts.
        assert_eq!(
            interactive("codex fix the failing tests then exec them"),
            Some(true)
        );
        // `codex "review the tests"`, as `ps` shows it.
        assert_eq!(interactive("codex review the tests"), Some(true));
        assert_eq!(interactive("codex -m gpt-6 -c a=b exec hi"), Some(false));
        assert_eq!(
            interactive("codex --model gpt-6 --search app-server"),
            Some(false)
        );
        assert_eq!(interactive("codex -c review=1 hello"), Some(true));
        assert_eq!(interactive("codex --enable x"), Some(true));
        assert_eq!(interactive("bash ./scripts/ensure-clax.sh exec hook"), None);
    }

    #[test]
    fn error_code_keeps_only_the_code() {
        assert_eq!(
            error_code("invalid_answer: \"Table\" is not an option"),
            "invalid_answer"
        );
        assert_eq!(
            error_code("error sending request for url (http://127.0.0.1:1/x): timed out"),
            "request failed"
        );
        assert_eq!(error_code("no colon"), "request failed");
    }
}
