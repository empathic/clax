//! The stdio MCP server a harness spawns per session (`artifax mcp --agent <harness>`):
//! it ensures a daemon, registers the harness session, serves [`ArtifaxTools`]
//! over stdin/stdout, heartbeats the session, and ends it when stdin closes.
//!
//! Stdout carries the MCP protocol only; diagnostics go to stderr.

use crate::client::DaemonClient;
pub use crate::client::{Endpoint, Refresh};
use crate::tools::ArtifaxTools;
use anyhow::Context;
use artifax_core::{Home, RegisterSession};
use rmcp::ServiceExt;
use rmcp::transport::stdio;
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
    Pi,
}

impl Harness {
    /// The name recorded as the session's `harness`.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
        }
    }
}

/// The session registration for `harness`, from the environment variable lookup
/// `env`, the process's working directory `current_dir`, and the parent
/// process's working directory `parent_cwd`. The harness session ID is
/// `CLAUDE_CODE_SESSION_ID` under Claude Code, else `ARTIFAX_SESSION_ID`. The
/// working directory is `CLAUDE_PROJECT_DIR` under Claude Code, else
/// `current_dir`; under Codex it is `parent_cwd`, else empty, because Codex
/// starts the shim in the plugin's own directory. Empty variables count as
/// unset.
pub fn registration(
    harness: Harness,
    env: impl Fn(&str) -> Option<String>,
    current_dir: Option<PathBuf>,
    parent_cwd: Option<PathBuf>,
    pid: u32,
    parent_pid: u32,
) -> RegisterSession {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    let claude = harness == Harness::Claude;
    let harness_session_id = claude
        .then(|| var("CLAUDE_CODE_SESSION_ID"))
        .flatten()
        .or_else(|| var("ARTIFAX_SESSION_ID"));
    let dir = match harness {
        Harness::Claude => var("CLAUDE_PROJECT_DIR").map(PathBuf::from).or(current_dir),
        Harness::Codex => parent_cwd,
        Harness::Pi => current_dir,
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
    use std::io::Read;
    let mut child = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + LSOF_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_lsof_cwd(&out)
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
/// seen every `heartbeat`.
pub async fn run(
    harness: Harness,
    home: &Home,
    refresh: Refresh,
    discover: Refresh,
    heartbeat: Duration,
) -> anyhow::Result<()> {
    // SAFETY: getppid has no preconditions and cannot fail.
    let parent_pid = unsafe { libc::getppid() } as u32;
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
        Err(e) => tracing::warn!("no artifax daemon yet; registration pending: {e}"),
    }
    let tools = ArtifaxTools::new(client.clone(), String::new(), None, home.log_path());

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

    let served = serve(tools).await;
    beat.abort();
    match tokio::time::timeout(END_TIMEOUT, client.end_session()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("ending the session failed: {e}"),
        Err(_) => tracing::warn!("ending the session timed out"),
    }
    served
}

/// Serves `tools` over stdin/stdout until the transport closes or SIGTERM arrives.
async fn serve(tools: ArtifaxTools) -> anyhow::Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let service = tools.serve(stdio()).await.context("MCP initialization")?;
    let cancel = service.cancellation_token();
    let signalled = tokio::spawn(async move {
        term.recv().await;
        cancel.cancel();
    });
    let quit = service.waiting().await;
    signalled.abort();
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
    fn claude_uses_its_session_id_and_project_dir() {
        let r = registration(
            Harness::Claude,
            env(&[
                ("CLAUDE_CODE_SESSION_ID", "cc-1"),
                ("CLAUDE_PROJECT_DIR", "/proj"),
                ("ARTIFAX_SESSION_ID", "ax-1"),
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
    fn claude_falls_back_to_artifax_session_id_and_current_dir() {
        let r = registration(
            Harness::Claude,
            env(&[
                ("CLAUDE_CODE_SESSION_ID", ""),
                ("ARTIFAX_SESSION_ID", "ax-1"),
            ]),
            Some(PathBuf::from("/here")),
            None,
            10,
            9,
        );
        assert_eq!(r.harness_session_id.as_deref(), Some("ax-1"));
        assert_eq!(r.cwd, "/here");
    }

    #[test]
    fn other_harnesses_ignore_claude_variables() {
        let vars = env(&[
            ("CLAUDE_CODE_SESSION_ID", "cc-1"),
            ("CLAUDE_PROJECT_DIR", "/proj"),
        ]);
        let r = registration(Harness::Pi, vars, Some(PathBuf::from("/here")), None, 10, 9);
        assert_eq!(r.harness, "pi");
        assert_eq!(r.harness_session_id, None);
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
