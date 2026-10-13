use crate::client::Client;
use clax_core::Home;
use clax_core::gitctx::{self, GitField};
use clax_core::remote::{
    CredentialUnreadable, SESSION_SECRET_HEADER, Seeker, SessionCredential, SessionFiles,
    SessionKey,
};
use clax_core::secret::Secret;
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
/// How long a remote `SessionStart` join that needs a proof it lacks waits
/// for the session's credential file, which the shim registering at the
/// same moment writes (spec 2026-10-08 §6.8).
const CREDENTIAL_WAIT: Duration = Duration::from_secs(1);
/// How often that wait looks for the file.
const CREDENTIAL_LOOK: Duration = Duration::from_millis(50);
/// The join route, whose answer may issue the session's secret.
const JOIN_PATH: &str = "/api/sessions/join";

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

/// A remote hook's agent session (spec 2026-10-08 §6.8): the credential
/// it proves its session with, found in the machine's session files as
/// join matches a row: by the harness session ID of its input, else by a
/// parent-PID file of its parent or an ancestor, written for a row with no
/// harness session ID or with this one, while that PID named the same
/// process ([`SessionFiles::find`]).
struct RemoteSession {
    files: SessionFiles,
    harness: &'static str,
    hsid: Option<String>,
    parent_pid: u32,
    /// The parent, then its ancestors, found when first needed.
    pids: OnceLock<Vec<u32>>,
    cred: Mutex<Option<SessionCredential>>,
    /// Whether the files were looked in yet.
    looked: Mutex<bool>,
}

impl RemoteSession {
    fn new(
        files: SessionFiles,
        harness: &'static str,
        input: &HookInput,
        parent_pid: u32,
    ) -> RemoteSession {
        RemoteSession {
            files,
            harness,
            hsid: input.session_id.clone().filter(|s| !s.is_empty()),
            parent_pid,
            pids: OnceLock::new(),
            cred: Mutex::new(None),
            looked: Mutex::new(false),
        }
    }

    /// The hook's parent, then its ancestors: the PIDs join tries.
    fn pids(&self) -> &[u32] {
        self.pids.get_or_init(|| {
            std::iter::once(self.parent_pid)
                .chain(ancestors(self.parent_pid))
                .collect()
        })
    }

    /// The names this hook's session may be found by, in join's order.
    fn keys(&self) -> Vec<SessionKey<'_>> {
        let mut keys = Vec::new();
        if let Some(id) = &self.hsid {
            keys.push(SessionKey::Harness {
                harness: self.harness,
                id,
            });
        }
        for pid in self.pids() {
            keys.push(SessionKey::Parent {
                harness: self.harness,
                pid: *pid,
            });
        }
        keys
    }

    /// The credential this hook may use, if any.
    fn find(&self) -> Result<Option<SessionCredential>, CredentialUnreadable> {
        // The harness session ID's file first: finding it needs no `ps`.
        if let Some(id) = self.hsid.as_deref()
            && let Some(c) = self.files.read(&SessionKey::Harness {
                harness: self.harness,
                id,
            })?
        {
            return Ok(Some(c));
        }
        self.files.find(&Seeker {
            harness: self.harness,
            harness_session_id: self.hsid.as_deref(),
            pids: self.pids(),
        })
    }

    /// The credential held, read from the files on first use.
    fn current(&self) -> Result<Option<SessionCredential>, CredentialUnreadable> {
        let mut looked = self.looked.lock().unwrap_or_else(|e| e.into_inner());
        let mut cred = self.cred.lock().unwrap_or_else(|e| e.into_inner());
        if !*looked {
            *cred = self.find()?;
            *looked = true;
        }
        Ok(cred.clone())
    }

    fn set(&self, c: Option<SessionCredential>) {
        *self.cred.lock().unwrap_or_else(|e| e.into_inner()) = c;
        *self.looked.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    /// The error of a hook that must act as its session but has no
    /// credential for it.
    fn missing(&self) -> anyhow::Error {
        let names: Vec<String> = self
            .keys()
            .iter()
            .filter_map(SessionKey::file_name)
            .collect();
        anyhow::anyhow!(
            "session_credential_missing: no credential for this {} session in {} (looked for {}): its session has not registered with the remote Clax, or has ended",
            self.harness,
            self.files.dir().display(),
            names.join(", ")
        )
    }

    /// Keeps the credential a join answered (`v`): the new secret it
    /// issued, else the one it proved, written to the files of every name
    /// the session's row has and of this hook's harness session ID.
    fn absorb(&self, v: &serde_json::Value, proved: Option<&SessionCredential>) {
        let Some(sid) = v["session"]["id"].as_str() else {
            return;
        };
        let cred = match v["session_secret"].as_str() {
            Some(s) => Some(SessionCredential {
                session: sid.to_string(),
                secret: Secret::new(s.to_string()),
            }),
            None => proved.filter(|c| c.session == sid).cloned(),
        };
        let Some(cred) = cred else { return };
        let harness = v["session"]["harness"].as_str().unwrap_or(self.harness);
        let parent = v["session"]["parent_pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok());
        // A hook that cannot keep the file still acts this run; the next
        // one says what is missing.
        let _ = self.files.write(
            harness,
            v["session"]["harness_session_id"].as_str(),
            parent,
            self.hsid.as_deref(),
            &cred,
        );
        self.set(Some(cred));
    }

    /// Removes this session's files once it has ended: those holding its
    /// own session. A hook that knows no harness session ID removes
    /// nothing, since the credential it would find by its parent may be
    /// another row's.
    fn forget(&self) {
        if self.hsid.is_none() {
            return;
        }
        if let Ok(Some(c)) = self.current() {
            self.files.remove(&self.keys(), &c.session);
        }
    }
}

/// Whether `e` is a 401 `invalid_session_proof`.
fn proof_refused(e: &anyhow::Error) -> bool {
    e.to_string().starts_with("invalid_session_proof:")
}

/// Whether `e` refuses the credential a join presented: its secret does
/// not prove the session (`invalid_session_proof`: the session ended), or
/// the daemon has no such session (`unknown_session`: another or a reset
/// daemon).
fn credential_refused(e: &anyhow::Error) -> bool {
    proof_refused(e) || e.to_string().starts_with("unknown_session:")
}

/// The join's attempts (spec 2026-10-08 §6.8), `send` making one with a
/// credential or none: with `first`, the credential found, when there is
/// one; then, if that credential is refused or there was none, without
/// one, which makes the session when no live row matches; and when a live
/// row needs a proof this hook does not have yet, one wait for a new
/// credential (`wait`, given those already refused) and one retry with
/// it. The answer, with the credential it was sent with.
///
/// # Errors
/// The last refusal, or any other error at once.
fn join_attempts<T>(
    first: Option<SessionCredential>,
    mut send: impl FnMut(Option<&SessionCredential>) -> anyhow::Result<T>,
    wait: impl FnOnce(&[SessionCredential]) -> Option<SessionCredential>,
) -> anyhow::Result<(T, Option<SessionCredential>)> {
    let mut refused = Vec::new();
    if let Some(c) = first {
        match send(Some(&c)) {
            Ok(v) => return Ok((v, Some(c))),
            Err(e) if credential_refused(&e) => refused.push(c),
            Err(e) => return Err(e),
        }
    }
    let e = match send(None) {
        Ok(v) => return Ok((v, None)),
        Err(e) if proof_refused(&e) => e,
        Err(e) => return Err(e),
    };
    let Some(c) = wait(&refused) else {
        return Err(e);
    };
    let v = send(Some(&c))?;
    Ok((v, Some(c)))
}

/// The daemon as a hook talks to it: every request names the `hook`
/// channel, and, for an event that captures git, carries `x-clax-git`.
/// Every request but a `GET` waits for the capture; a `GET` carries it
/// once it is there. On a remote daemon every request names the hook's
/// session with its secret (spec 2026-10-08 §6.8); a hook with no
/// credential for its session fails, saying which files it looked for,
/// except the join, which makes the session when none is live.
struct HookDaemon<'a> {
    client: &'a Client,
    git: Option<HookGit>,
    remote: Option<RemoteSession>,
}

impl<'a> HookDaemon<'a> {
    fn new(
        client: &'a Client,
        event: Event,
        input: &HookInput,
        agent: Agent,
        parent_pid: u32,
    ) -> HookDaemon<'a> {
        HookDaemon {
            client,
            git: captures_git(event).then(|| HookGit::new(input.cwd.as_deref())),
            remote: client.remote().map(|r| {
                RemoteSession::new(r.sessions.clone(), agent.harness(), input, parent_pid)
            }),
        }
    }

    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
    ) -> anyhow::Result<serde_json::Value> {
        let Some(remote) = &self.remote else {
            return self.send_as(method, path, body, timeout, None);
        };
        let cred = remote.current()?;
        if path != JOIN_PATH {
            let cred = cred.ok_or_else(|| remote.missing())?;
            return self.send_as(method, path, body, timeout, Some(&cred));
        }
        let (v, used) = join_attempts(
            cred,
            |c| self.send_as(method.clone(), path, body, timeout, c),
            |refused| wait_for_credential(remote, refused, CREDENTIAL_WAIT),
        )?;
        remote.absorb(&v, used.as_ref());
        Ok(v)
    }

    fn send_as(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
        cred: Option<&SessionCredential>,
    ) -> anyhow::Result<serde_json::Value> {
        let git = self.git.as_ref().and_then(|g| {
            if method == reqwest::Method::GET {
                g.now()
            } else {
                Some(g.wait())
            }
        });
        let header = git.and_then(gitctx::encode_header);
        let mut headers: Vec<(&str, &str)> = header
            .as_deref()
            .map(|h| ("x-clax-git", h))
            .into_iter()
            .collect();
        if let Some(c) = cred {
            headers.push(("x-clax-session", &c.session));
            headers.push((SESSION_SECRET_HEADER, c.secret.expose()));
        }
        self.client
            .request_with(method, path, body, timeout, &headers)
    }

    /// Removes a remote session's credential files: its session has ended.
    fn forget_session(&self) {
        if let Some(r) = &self.remote {
            r.forget();
        }
    }
}

/// A credential for `remote`'s session that is none of `refused`, looked
/// for every [`CREDENTIAL_LOOK`] for up to `wait` ([`CREDENTIAL_WAIT`]):
/// the file the shim writes when it made the row this join needs a proof
/// for.
fn wait_for_credential(
    remote: &RemoteSession,
    refused: &[SessionCredential],
    wait: Duration,
) -> Option<SessionCredential> {
    let until = Instant::now() + wait;
    loop {
        if let Ok(Some(c)) = remote.find()
            && !refused.iter().any(|t| t.same_as(&c))
        {
            return Some(c);
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(CREDENTIAL_LOOK);
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
        .agent_channel()
        .with_timeout(budget(agent, event).1)
        .with_via(VIA);
    let client = HookDaemon::new(&client, event, &input, agent, parent_pid);
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
            // A remote session's credential goes with it (spec 2026-10-08
            // §6.8).
            client.forget_session();
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
    let client = client
        .agent_channel()
        .with_timeout(ASK_REQUEST_TIMEOUT)
        .with_via(VIA);
    let client = HookDaemon::new(
        &client,
        Event::Ask,
        &input,
        agent,
        std::os::unix::process::parent_id(),
    );
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

#[cfg(test)]
mod join_tests {
    use super::*;
    use std::cell::RefCell;

    fn cred(s: &str) -> SessionCredential {
        SessionCredential {
            session: s.into(),
            secret: Secret::new(format!("cxs_{s}")),
        }
    }

    /// The join's attempts against answers given in order: the credential
    /// each was sent with, and the outcome.
    fn run(
        first: Option<&str>,
        answers: Vec<Result<&'static str, &'static str>>,
        waited: Option<&str>,
    ) -> (Vec<Option<String>>, usize, anyhow::Result<Option<String>>) {
        let sent = RefCell::new(Vec::new());
        let answers = RefCell::new(answers.into_iter());
        let waits = RefCell::new(0);
        let out = join_attempts(
            first.map(cred),
            |c| {
                sent.borrow_mut().push(c.map(|c| c.session.clone()));
                match answers.borrow_mut().next().expect("an answer") {
                    Ok(v) => Ok(v),
                    Err(code) => Err(anyhow::anyhow!("{code}: refused")),
                }
            },
            |_| {
                *waits.borrow_mut() += 1;
                waited.map(cred)
            },
        )
        .map(|(_, c)| c.map(|c| c.session));
        (sent.into_inner(), waits.into_inner(), out)
    }

    #[test]
    fn a_found_credential_is_used_and_a_stale_one_dropped() {
        let (sent, waits, out) = run(Some("S1"), vec![Ok("v")], None);
        assert_eq!((sent, waits), (vec![Some("S1".into())], 0));
        assert_eq!(out.unwrap(), Some("S1".into()));
        // Refused as unproved or as unknown: joined without it.
        for code in ["invalid_session_proof", "unknown_session"] {
            let (sent, waits, out) = run(Some("S1"), vec![Err(code), Ok("v")], None);
            assert_eq!((sent, waits), (vec![Some("S1".into()), None], 0), "{code}");
            assert_eq!(out.unwrap(), None);
        }
    }

    #[test]
    fn a_live_row_without_a_proof_waits_once_and_retries_once() {
        let (sent, waits, out) = run(
            None,
            vec![Err("invalid_session_proof"), Ok("v")],
            Some("S2"),
        );
        assert_eq!((sent, waits), (vec![None, Some("S2".into())], 1));
        assert_eq!(out.unwrap(), Some("S2".into()));
        // The credential it waited for is refused too: no second wait.
        let (sent, waits, out) = run(
            None,
            vec![Err("invalid_session_proof"), Err("invalid_session_proof")],
            Some("S2"),
        );
        assert_eq!((sent.len(), waits), (2, 1));
        assert!(out.is_err());
        // Nothing appears: the refusal stands.
        let (sent, waits, out) = run(None, vec![Err("invalid_session_proof")], None);
        assert_eq!((sent.len(), waits), (1, 1));
        assert!(
            out.unwrap_err()
                .to_string()
                .starts_with("invalid_session_proof")
        );
        // Any other refusal: no wait.
        let (_, waits, out) = run(None, vec![Err("grant_revoked")], Some("S2"));
        assert_eq!(waits, 0);
        assert!(out.is_err());
    }
}

#[cfg(test)]
mod remote_session_tests {
    use super::*;

    fn cred(s: &str) -> SessionCredential {
        SessionCredential {
            session: s.into(),
            secret: clax_core::secret::token(clax_core::secret::SESSION_PREFIX),
        }
    }

    fn remote(files: &SessionFiles, input: &str) -> RemoteSession {
        RemoteSession::new(
            files.clone(),
            "claude",
            &HookInput::parse(input),
            std::process::id(),
        )
    }

    /// The wait passes over the credential just refused: a stale file does
    /// not end the wait; the shim's new one does.
    #[test]
    fn the_wait_takes_a_new_credential_never_the_refused_one() {
        let tmp = tempfile::tempdir().unwrap();
        let files = SessionFiles::at(tmp.path().join("remote-sessions"));
        let r = remote(&files, r#"{"session_id": "w-1"}"#);
        let stale = cred("A");
        files
            .write("claude", Some("w-1"), None, None, &stale)
            .unwrap();
        let refused = [stale.clone()];
        assert!(wait_for_credential(&r, &refused, Duration::ZERO).is_none());
        files
            .write("claude", Some("w-1"), None, None, &cred("B"))
            .unwrap();
        let got = wait_for_credential(&r, &refused, Duration::ZERO).unwrap();
        assert_eq!(got.session, "B");
    }

    /// A session's end removes only files written for its own row: a hook
    /// that knows no session ID removes nothing, and another row's
    /// parent-PID file of the same process stays.
    #[test]
    fn a_hook_forgets_only_its_own_rows_files() {
        let tmp = tempfile::tempdir().unwrap();
        let files = SessionFiles::at(tmp.path().join("remote-sessions"));
        let me = std::process::id();
        files
            .write("claude", Some("b-h"), Some(me), None, &cred("B"))
            .unwrap();
        let ppid = files
            .path(&SessionKey::Parent {
                harness: "claude",
                pid: me,
            })
            .unwrap();
        // Knowing no session ID, it would find B's file by its parent.
        remote(&files, "{}").forget();
        assert!(ppid.exists());
        // Another session of the same process ends.
        files
            .write("claude", Some("c-h"), Some(me), None, &cred("C"))
            .unwrap();
        remote(&files, r#"{"session_id": "c-h"}"#).forget();
        assert!(ppid.exists(), "B's parent-PID file stays");
        assert!(
            !files
                .path(&SessionKey::Harness {
                    harness: "claude",
                    id: "c-h"
                })
                .unwrap()
                .exists()
        );
        remote(&files, r#"{"session_id": "b-h"}"#).forget();
        assert!(!ppid.exists());
    }
}
