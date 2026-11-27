//! The stdio MCP server a harness spawns per session (`clax mcp --agent <harness>`):
//! it ensures a daemon, registers the harness session, serves [`ClaxTools`]
//! over stdin/stdout, heartbeats the session, and ends it when stdin closes.
//! Antigravity's shim serves every conversation of its `agy` process instead,
//! with a session per root conversation ([`Conversations`]).
//!
//! Stdout carries the MCP protocol only; diagnostics go to stderr.

use crate::client::DaemonClient;
pub use crate::client::{Endpoint, Refresh};
use crate::conversations::Conversations;
use crate::tools::{ClaxTools, UpgradeHoldProbe};
use anyhow::Context;
use clax_core::Home;
use clax_core::RegisterSession;
use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How often the session is marked seen while the shim runs.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(60);
/// Deadline for ending the session once the transport has closed.
const END_TIMEOUT: Duration = Duration::from_secs(3);

/// The harness that spawned the shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Grok,
    Antigravity,
}

impl Harness {
    /// The name recorded as the session's `harness`.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
            Harness::Antigravity => clax_core::antigravity::HARNESS,
        }
    }
}

/// The session registration for `harness`, from the environment variable lookup
/// `env`, the process's working directory `current_dir`, and the parent
/// process's working directory `parent_cwd`. The harness session ID is
/// `CLAUDE_CODE_SESSION_ID` under Claude Code and `GROK_SESSION_ID` under
/// Grok Build, else `CLAX_SESSION_ID`; under Antigravity there is none, since
/// each conversation's comes from its calls. The working directory is
/// `CLAUDE_PROJECT_DIR` under Claude Code, else `current_dir`; under Codex and
/// Antigravity it is `parent_cwd`, else empty, because they start the shim in
/// the plugin's own directory; under Grok it is `current_dir`, because Grok
/// starts servers in its own working directory. Empty variables count as unset. The
/// repository and branch that working directory is in name the agent
/// (spec 2026-10-08 §11), read from the files with no `git` process.
pub fn registration(
    harness: Harness,
    env: impl Fn(&str) -> Option<String>,
    current_dir: Option<PathBuf>,
    parent_cwd: Option<PathBuf>,
    pid: u32,
    parent_pid: u32,
) -> RegisterSession {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    let own_id = match harness {
        Harness::Claude => var("CLAUDE_CODE_SESSION_ID"),
        Harness::Grok => var("GROK_SESSION_ID"),
        Harness::Codex | Harness::Antigravity => None,
    };
    let harness_session_id = match harness {
        Harness::Antigravity => None,
        _ => own_id.or_else(|| var("CLAX_SESSION_ID")),
    };
    let dir = match harness {
        Harness::Claude => var("CLAUDE_PROJECT_DIR").map(PathBuf::from).or(current_dir),
        Harness::Codex | Harness::Antigravity => parent_cwd,
        Harness::Grok => current_dir,
    };
    let (repo, branch) = dir
        .as_deref()
        .map(clax_core::names::repo_and_branch)
        .unwrap_or_default();
    let cwd = dir
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    RegisterSession {
        harness: harness.as_str().to_string(),
        harness_session_id,
        cwd,
        pid: Some(pid),
        parent_pid: Some(parent_pid),
        repo,
        branch,
        ..Default::default()
    }
}

/// How long [`process_cwd`] and [`stdin_is_terminal`] wait for `lsof`.
#[cfg(not(target_os = "linux"))]
const LSOF_TIMEOUT: Duration = Duration::from_secs(2);

/// The working directory of process `pid`, if it can be determined.
#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The working directory of process `pid`, if it can be determined within
/// [`LSOF_TIMEOUT`]; read from `lsof -a -p <pid> -d cwd -Fn`, whose `n` line
/// names the directory.
#[cfg(not(target_os = "linux"))]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    let out = run_with_timeout(
        std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]),
        LSOF_TIMEOUT,
    )?;
    parse_lsof_cwd(&out)
}

/// Whether process `pid`'s standard input is a terminal, if that can be
/// determined: where `/proc/<pid>/fd/0` links to names one
/// ([`names_a_terminal`]).
#[cfg(target_os = "linux")]
pub fn stdin_is_terminal(pid: u32) -> Option<bool> {
    std::fs::read_link(format!("/proc/{pid}/fd/0"))
        .ok()
        .map(|p| names_a_terminal(&p.to_string_lossy()))
}

/// Whether process `pid`'s standard input is a terminal, if that can be
/// determined within [`LSOF_TIMEOUT`]; read from `lsof -a -p <pid> -d 0
/// -Fn` ([`parse_lsof_stdin`]).
#[cfg(not(target_os = "linux"))]
pub fn stdin_is_terminal(pid: u32) -> Option<bool> {
    let out = run_with_timeout(
        std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "0", "-Fn"]),
        LSOF_TIMEOUT,
    )?;
    parse_lsof_stdin(&out)
}

/// Whether the file named by the first `n` line of `lsof -Fn` output is a
/// terminal ([`names_a_terminal`]); `None` with no `n` line. A pipe's `n`
/// line names nothing.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lsof_stdin(out: &str) -> Option<bool> {
    out.lines()
        .find_map(|l| l.strip_prefix('n'))
        .map(names_a_terminal)
}

/// Whether `path` names a terminal device: `/dev/tty…` (macOS's
/// pseudo-terminals are `/dev/ttys…`), `/dev/pts/…` (Linux's) or
/// `/dev/console`.
fn names_a_terminal(path: &str) -> bool {
    path.starts_with("/dev/tty") || path.starts_with("/dev/pts/") || path == "/dev/console"
}

/// Runs `cmd` with no stdin and no stderr and returns its stdout, or `None`
/// when it fails to start, exits non-zero, or runs past `timeout` (then it
/// is killed).
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn run_with_timeout(
    cmd: &mut std::process::Command,
    timeout: Duration,
) -> Option<String> {
    use std::io::Read;
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// The directory named by the first `n` line of `lsof -Fn` output.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lsof_cwd(out: &str) -> Option<PathBuf> {
    out.lines()
        .find_map(|l| l.strip_prefix('n'))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// Runs the shim until stdin closes (or SIGTERM arrives), then ends the
/// session. `refresh` finds or starts the daemon and `discover` only finds a
/// running one (both block, so they run on a blocking thread, and must not write
/// to stdout). Startup and tool calls use `refresh`, heartbeats and ending the
/// session use `discover`; either runs again whenever the daemon stops
/// answering, and the session is then registered again. When no daemon can be reached the shim serves anyway, and tool calls
/// retry and report `daemon_unreachable` until one can. The session is marked
/// seen every `heartbeat`. The registration and each tool call that changes
/// history carry the working directory's git state, and each tool call is
/// reported once its result has gone back. `status` reports what `upgrade_hold` says about
/// the daemon's version, when given. With `channel`, the tools declare the
/// Claude Code channel, and when its launch flag is present the shim
/// forwards the session's comment notices as channel events.
///
/// Under Antigravity nothing is registered before a call: each root
/// conversation gets its own session on its first call, heartbeated from
/// then on ([`Conversations`]). Unless the parent is an `agy` whose command
/// line does not ask to print and whose standard input is a terminal
/// ([`clax_core::antigravity::run_is_headless`]), the run is headless:
/// `wait_for_feedback` keeps its bounds and `status` says nothing wakes it;
/// in an interactive run a wait with no `timeout_s` has none. On exit every
/// one is ended, and the files Antigravity's hooks keep for it
/// (`run/tool-hook/antigravity-<ID>`, `run/wake-nudge/antigravity-<ID>.json`)
/// are removed for an ID made of `[A-Za-z0-9._-]`.
pub async fn run(
    harness: Harness,
    home: &Home,
    refresh: Refresh,
    discover: Refresh,
    heartbeat: Duration,
    upgrade_hold: Option<UpgradeHoldProbe>,
    channel: Option<crate::channel::ChannelState>,
) -> anyhow::Result<()> {
    let parent_pid = std::os::unix::process::parent_id();
    let parent_cwd = match harness {
        Harness::Codex | Harness::Antigravity => {
            tokio::task::spawn_blocking(move || process_cwd(parent_pid))
                .await
                .ok()
                .flatten()
        }
        _ => None,
    };
    let reg = registration(
        harness,
        |k| std::env::var(k).ok(),
        std::env::current_dir().ok(),
        parent_cwd,
        std::process::id(),
        parent_pid,
    );
    // Found once; each capture runs git afresh (spec §9.3).
    let git = crate::git::GitCapture::from_env();
    // Remote mode (spec §16, M26): the daemon `remote.json` names, signed;
    // none is started or found here. A `remote.json` that cannot be used
    // fails every call with why (spec §22), never falling back.
    let (refresh, discover, remote) = match clax_core::remote::Mode::of(home) {
        clax_core::remote::Mode::Local => (refresh, discover, None),
        clax_core::remote::Mode::Remote(r) => (refresh, discover, Some(r)),
        clax_core::remote::Mode::Broken(e) => {
            let why = e.to_string();
            let fail: Refresh = Arc::new(move || Err(anyhow::anyhow!("{why}")));
            (fail.clone(), fail, None)
        }
    };
    let connect = {
        let git = git.clone();
        Arc::new(move |reg: RegisterSession| {
            match &remote {
                Some(r) => DaemonClient::managed_remote(r.clone(), reg),
                None => DaemonClient::managed(refresh.clone(), discover.clone(), reg),
            }
            .with_git(git.clone())
        })
    };
    let client = connect(reg.clone());
    if harness != Harness::Antigravity {
        match client.ensure_session().await {
            Ok(()) => tracing::info!(
                session = client.session().map(|s| s.id),
                harness = harness.as_str(),
                "session registered"
            ),
            Err(e) => tracing::warn!("no clax daemon yet; registration pending: {e}"),
        }
    }
    let plugin_version = crate::plugin::root_from_env(
        |k| std::env::var(k).ok(),
        (harness == Harness::Codex)
            .then(|| std::env::current_dir().ok())
            .flatten(),
    )
    .and_then(|root| crate::plugin::manifest_version(&root));
    match &plugin_version {
        Some(v) if v != env!("CARGO_PKG_VERSION") => tracing::warn!(
            plugin_version = %v,
            binary_version = env!("CARGO_PKG_VERSION"),
            "version skew: the plugin and the clax binary differ; reinstall the plugin or update clax"
        ),
        Some(v) => tracing::info!(plugin_version = %v, "plugin version matches the binary"),
        None => tracing::info!("plugin root unknown; plugin version not checked"),
    }
    let mut tools = ClaxTools::new(client.clone(), String::new(), None, home.log_path())
        .with_plugin_version(plugin_version)
        .with_git(git);
    if let Some(probe) = upgrade_hold {
        tools = tools.with_upgrade_hold(probe);
    }
    let forward = channel
        .as_ref()
        .is_some_and(crate::channel::ChannelState::forwards);
    if let Some(ch) = channel {
        tools = tools.with_channel(ch);
    }

    if harness == Harness::Antigravity {
        let headless = tokio::task::spawn_blocking(move || {
            let parent = crate::channel::process_argv(parent_pid).ok();
            clax_core::antigravity::run_is_headless(&[parent], |_| stdin_is_terminal(parent_pid))
        })
        .await
        .unwrap_or(true);
        let tools = tools.with_unbounded_wait(!headless).headless(headless);
        let conversations = Arc::new(Conversations::new(tools, reg, connect, heartbeat));
        let served = serve(conversations.clone(), None).await;
        for root in conversations.end_all(END_TIMEOUT).await {
            remove_hook_files(home, &root);
        }
        return served;
    }
    let beating = tokio::spawn(beat(client.clone(), heartbeat));
    let served = serve(tools.clone(), forward.then(|| client.clone())).await;
    beating.abort();
    end(&tools, END_TIMEOUT).await;
    served
}

/// Marks `client`'s session seen every `every`, from one `every` from now,
/// logging when heartbeats start failing and when they recover. Never ends.
pub(crate) async fn beat(client: DaemonClient, every: Duration) {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut healthy = client.session().is_some();
    loop {
        ticks.tick().await;
        match client.heartbeat().await {
            Ok(_) if !healthy => {
                healthy = true;
                tracing::info!("heartbeat recovered");
            }
            Err(e) if healthy => {
                healthy = false;
                tracing::warn!("heartbeat failed: {e}");
            }
            _ => {}
        }
    }
}

/// Sends the last reports of `tools`' calls, then ends their client's
/// session, each within `within`.
pub(crate) async fn end(tools: &ClaxTools, within: Duration) {
    if tokio::time::timeout(within, tools.settle_reports())
        .await
        .is_err()
    {
        tracing::warn!("tool-call reports still unsent at exit");
    }
    match tokio::time::timeout(within, tools.client().end_session()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("ending the session failed: {e}"),
        Err(_) => tracing::warn!("ending the session timed out"),
    }
}

/// Removes the files Antigravity's hooks keep for conversation `id` in
/// `home`, when `id` is a plain file name of `[A-Za-z0-9._-]`.
fn remove_hook_files(home: &Home, id: &str) {
    let plain = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !plain {
        return;
    }
    let run = home.root().join("run");
    let harness = clax_core::antigravity::HARNESS;
    for f in [
        run.join("tool-hook").join(format!("{harness}-{id}")),
        run.join("wake-nudge").join(format!("{harness}-{id}.json")),
    ] {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("removing {} failed: {e}", f.display())
            }
            _ => {}
        }
    }
}

/// How long each notices poll waits.
const NOTICE_WAIT_S: u64 = 50;

/// Forwards the session's comment notices as Claude Code channel events
/// until the transport closes. Each poll stamps the notices it returns, so
/// no other follower announces them again. Daemon errors back off from 1 s
/// to 30 s. An empty answer that came back at once (the daemon answers so
/// while the session is inside `wait_for_feedback`, or while it shuts down)
/// is followed by a 1 s pause before the next poll. The loop never starts a
/// daemon.
async fn forward_notices(client: DaemonClient, peer: rmcp::Peer<rmcp::RoleServer>) {
    use rmcp::model::{CustomNotification, ServerNotification};
    let mut backoff = Duration::from_secs(1);
    loop {
        let polled = tokio::time::Instant::now();
        match client.notices_via(NOTICE_WAIT_S, Some("channel")).await {
            Ok(v) => {
                backoff = Duration::from_secs(1);
                let notices = v["notices"].as_array().cloned().unwrap_or_default();
                let lines = v["lines"].as_array().cloned().unwrap_or_default();
                if notices.is_empty() && polled.elapsed() < Duration::from_secs(1) {
                    tokio::time::sleep_until(polled + Duration::from_secs(1)).await;
                }
                for (n, line) in notices.iter().zip(lines.iter().filter_map(|l| l.as_str())) {
                    let event = CustomNotification::new(
                        crate::channel::METHOD,
                        Some(crate::channel::event_params(n, line)),
                    );
                    if let Err(e) = peer
                        .send_notification(ServerNotification::CustomNotification(event))
                        .await
                    {
                        tracing::info!("channel closed: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("notices poll failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Serves `server` over stdin/stdout until the transport closes or SIGTERM
/// arrives; with `notices`, forwards that client's comment notices as
/// channel events meanwhile.
async fn serve<S: rmcp::ServerHandler>(
    server: S,
    notices: Option<DaemonClient>,
) -> anyhow::Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let transport = crate::probe::stdio(&server);
    let service = server
        .serve(transport)
        .await
        .context("MCP initialization")?;
    let forward =
        notices.map(|client| tokio::spawn(forward_notices(client, service.peer().clone())));
    let cancel = service.cancellation_token();
    let signalled = tokio::spawn(async move {
        term.recv().await;
        cancel.cancel();
    });
    let quit = service.waiting().await;
    signalled.abort();
    if let Some(f) = forward {
        f.abort();
    }
    tracing::info!("transport closed: {quit:?}");
    Ok(())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn grok_uses_its_session_id_and_working_directory() {
        let r = registration(
            Harness::Grok,
            env(&[
                ("GROK_SESSION_ID", "019a-g"),
                ("CLAUDE_CODE_SESSION_ID", "cc-1"),
                ("CLAUDE_PROJECT_DIR", "/claude"),
                ("CLAX_SESSION_ID", "ax-1"),
            ]),
            Some(PathBuf::from("/work")),
            Some(PathBuf::from("/parent")),
            7,
            3,
        );
        assert_eq!(r.harness, "grok");
        assert_eq!(r.harness_session_id.as_deref(), Some("019a-g"));
        assert_eq!(r.cwd, "/work");
    }

    #[test]
    fn grok_without_its_session_id_falls_back_to_clax_session_id() {
        let r = registration(
            Harness::Grok,
            env(&[("GROK_SESSION_ID", ""), ("CLAX_SESSION_ID", "ax-1")]),
            None,
            None,
            7,
            3,
        );
        assert_eq!(r.harness_session_id.as_deref(), Some("ax-1"));
        assert_eq!(r.cwd, "");
    }

    #[test]
    fn claude_uses_its_session_id_and_project_dir() {
        let r = registration(
            Harness::Claude,
            env(&[
                ("CLAUDE_CODE_SESSION_ID", "cc-1"),
                ("CLAUDE_PROJECT_DIR", "/proj"),
                ("CLAX_SESSION_ID", "ax-1"),
            ]),
            Some(PathBuf::from("/here")),
            None,
            10,
            9,
        );
        assert_eq!(
            r,
            RegisterSession {
                harness: "claude".into(),
                harness_session_id: Some("cc-1".into()),
                cwd: "/proj".into(),
                pid: Some(10),
                parent_pid: Some(9),
                ..Default::default()
            }
        );
    }

    #[test]
    fn claude_falls_back_to_clax_session_id_and_current_dir() {
        let r = registration(
            Harness::Claude,
            env(&[("CLAUDE_CODE_SESSION_ID", ""), ("CLAX_SESSION_ID", "ax-1")]),
            Some(PathBuf::from("/here")),
            None,
            10,
            9,
        );
        assert_eq!(r.harness_session_id.as_deref(), Some("ax-1"));
        assert_eq!(r.cwd, "/here");
    }

    #[test]
    fn codex_uses_the_parent_cwd_and_ignores_claude_variables() {
        let vars = env(&[
            ("CLAUDE_CODE_SESSION_ID", "cc-1"),
            ("CLAUDE_PROJECT_DIR", "/proj"),
        ]);
        let r = registration(
            Harness::Codex,
            vars,
            Some(PathBuf::from("/plugin-cache")),
            Some(PathBuf::from("/work")),
            10,
            9,
        );
        assert_eq!(r.harness, "codex");
        assert_eq!(r.harness_session_id, None);
        assert_eq!(r.cwd, "/work");
    }

    #[test]
    fn antigravity_takes_no_session_id_from_the_environment_and_the_parent_cwd() {
        let r = registration(
            Harness::Antigravity,
            env(&[
                ("CLAX_SESSION_ID", "ax-1"),
                ("CLAUDE_CODE_SESSION_ID", "cc-1"),
                ("GROK_SESSION_ID", "g-1"),
                ("CLAUDE_PROJECT_DIR", "/proj"),
            ]),
            Some(PathBuf::from("/plugin")),
            Some(PathBuf::from("/work")),
            10,
            9,
        );
        assert_eq!(r.harness, "antigravity");
        assert_eq!(r.harness_session_id, None);
        assert_eq!(r.cwd, "/work");
    }

    #[test]
    fn the_registration_names_the_repository_and_branch_of_the_working_directory() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("web");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/fix/axis\n").unwrap();
        let r = registration(
            Harness::Claude,
            env(&[("CLAUDE_PROJECT_DIR", repo.join("src").to_str().unwrap())]),
            None,
            None,
            10,
            9,
        );
        assert_eq!(
            (r.repo.as_deref(), r.branch.as_deref()),
            (Some("web"), Some("fix/axis"))
        );
        let none = registration(Harness::Codex, env(&[]), None, None, 10, 9);
        assert_eq!((none.repo, none.branch), (None, None));
    }

    #[test]
    fn lsof_output_names_the_cwd() {
        assert_eq!(
            parse_lsof_cwd("p123\nfcwd\nn/work dir\n"),
            Some(PathBuf::from("/work dir"))
        );
        assert_eq!(parse_lsof_cwd("p123\n"), None);
    }

    #[test]
    fn lsof_output_says_whether_stdin_is_a_terminal() {
        assert_eq!(parse_lsof_stdin("p123\nf0\nn/dev/ttys003\n"), Some(true));
        assert_eq!(parse_lsof_stdin("p123\nf0\nn/dev/pts/4\n"), Some(true));
        assert_eq!(parse_lsof_stdin("p123\nf0\nn\n"), Some(false), "a pipe");
        assert_eq!(parse_lsof_stdin("p123\nf0\nn/dev/null\n"), Some(false));
        assert_eq!(
            parse_lsof_stdin("p123\nf0\nn/work/prompt.txt\n"),
            Some(false)
        );
        assert_eq!(parse_lsof_stdin("p123\n"), None, "no standard input");
        assert_eq!(parse_lsof_stdin(""), None);
    }

    #[test]
    fn a_child_reading_a_pipe_has_no_terminal_on_stdin() {
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let seen = stdin_is_terminal(child.id());
        drop(child.stdin.take());
        child.wait().unwrap();
        assert_eq!(seen, Some(false));
    }

    #[test]
    fn this_process_cwd_is_found() {
        assert_eq!(
            process_cwd(std::process::id()).map(|p| p.canonicalize().unwrap()),
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }

    #[test]
    fn codex_without_a_parent_cwd_registers_an_empty_cwd() {
        let r = registration(
            Harness::Codex,
            env(&[]),
            Some(PathBuf::from("/plugin-cache")),
            None,
            10,
            9,
        );
        assert_eq!(r.cwd, "");
    }

    #[test]
    fn hook_files_are_removed_only_for_a_plain_id() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let run = home.root().join("run");
        for d in ["tool-hook/antigravity-a", "wake-nudge/antigravity-a"] {
            std::fs::create_dir_all(run.join(d)).unwrap();
        }
        let planted = [run.join("x"), run.join("x.json")];
        for f in &planted {
            std::fs::write(f, "").unwrap();
        }
        remove_hook_files(&home, "a/../../x");
        for f in &planted {
            assert!(f.exists(), "{} was removed", f.display());
        }
    }
}
