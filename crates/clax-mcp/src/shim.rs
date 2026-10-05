//! The stdio MCP server a harness spawns per session (`clax mcp --agent <harness>`):
//! it ensures a daemon, registers the harness session, serves [`ClaxTools`]
//! over stdin/stdout, heartbeats the session, and ends it when stdin closes.
//!
//! Stdout carries the MCP protocol only; diagnostics go to stderr.

use crate::client::DaemonClient;
pub use crate::client::{Endpoint, Refresh};
use crate::tools::{ClaxTools, UpgradeHoldProbe};
use anyhow::Context;
use clax_core::{Home, RegisterSession};
use rmcp::ServiceExt;
use std::path::PathBuf;
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
}

impl Harness {
    /// The name recorded as the session's `harness`.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
        }
    }
}

/// The session registration for `harness`, from the environment variable lookup
/// `env`, the process's working directory `current_dir`, and the parent
/// process's working directory `parent_cwd`. The harness session ID is
/// `CLAUDE_CODE_SESSION_ID` under Claude Code and `GROK_SESSION_ID` under
/// Grok Build, else `CLAX_SESSION_ID`. The working directory is
/// `CLAUDE_PROJECT_DIR` under Claude Code, else `current_dir`; under Codex it
/// is `parent_cwd`, else empty, because Codex starts the shim in the plugin's
/// own directory; under Grok it is `current_dir`, because Grok starts servers
/// in its own working directory. Empty variables count as unset.
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
        Harness::Codex => None,
    };
    let harness_session_id = own_id.or_else(|| var("CLAX_SESSION_ID"));
    let dir = match harness {
        Harness::Claude => var("CLAUDE_PROJECT_DIR").map(PathBuf::from).or(current_dir),
        Harness::Codex => parent_cwd,
        Harness::Grok => current_dir,
    };
    let cwd = dir
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    RegisterSession {
        harness: harness.as_str().to_string(),
        harness_session_id,
        cwd,
        pid: Some(pid),
        parent_pid: Some(parent_pid),
    }
}

/// How long [`process_cwd`] waits for `lsof`.
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
/// seen every `heartbeat`. `status` reports what `upgrade_hold` says about
/// the daemon's version, when given. With `channel`, the tools declare the
/// Claude Code channel, and when its launch flag is present the shim
/// forwards the session's comment notices as channel events.
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
        Harness::Codex => tokio::task::spawn_blocking(move || process_cwd(parent_pid))
            .await
            .ok()
            .flatten(),
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
    let client = DaemonClient::managed(refresh, discover, reg);
    match client.ensure_session().await {
        Ok(()) => tracing::info!(
            session = client.session().map(|s| s.id),
            harness = harness.as_str(),
            "session registered"
        ),
        Err(e) => tracing::warn!("no clax daemon yet; registration pending: {e}"),
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
        .with_plugin_version(plugin_version);
    if let Some(probe) = upgrade_hold {
        tools = tools.with_upgrade_hold(probe);
    }
    let forward = channel
        .as_ref()
        .is_some_and(crate::channel::ChannelState::forwards);
    if let Some(ch) = channel {
        tools = tools.with_channel(ch);
    }

    let beat = {
        let client = client.clone();
        tokio::spawn(async move {
            let mut every =
                tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut healthy = client.session().is_some();
            loop {
                every.tick().await;
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
        })
    };

    let served = serve(tools, client.clone(), forward).await;
    beat.abort();
    match tokio::time::timeout(END_TIMEOUT, client.end_session()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("ending the session failed: {e}"),
        Err(_) => tracing::warn!("ending the session timed out"),
    }
    served
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
        match client.notices(NOTICE_WAIT_S).await {
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

/// Serves `tools` over stdin/stdout until the transport closes or SIGTERM
/// arrives; when `forward`, forwards `client`'s comment notices as channel
/// events meanwhile.
async fn serve(tools: ClaxTools, client: DaemonClient, forward: bool) -> anyhow::Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let transport = crate::probe::stdio(&tools);
    let service = tools.serve(transport).await.context("MCP initialization")?;
    let forward = forward.then(|| tokio::spawn(forward_notices(client, service.peer().clone())));
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
    fn lsof_output_names_the_cwd() {
        assert_eq!(
            parse_lsof_cwd("p123\nfcwd\nn/work dir\n"),
            Some(PathBuf::from("/work dir"))
        );
        assert_eq!(parse_lsof_cwd("p123\n"), None);
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
}
