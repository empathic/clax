//! Tier 5 for Codex: hands feedback to a Codex session with
//! `codex queue --thread <harness_session_id> --message <payload>`.
//!
//! Measured on Codex 0.158 (docs/contract.md): an idle attached TUI starts a
//! turn within a second, a busy one runs the message as its next turn, and with
//! no client attached the message is held until `codex resume`. Exit 0 means
//! queued, not seen, so the rows stay unacknowledged and the in-band tiers
//! resend them after two minutes.

use crate::feedback::{FeedbackCtx, publish_states};
use artifax_core::feedback::{Tier, Touched, render_items};
use artifax_core::{Store, TakeFeedback};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Deadline for one `codex queue` run.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the daemon's `codex` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexSource {
    /// `ARTIFAX_CODEX_BIN` named the binary.
    Env,
    /// `ARTIFAX_CODEX_BIN` was set and empty: Codex push is off.
    Disabled,
    /// Found on the daemon's `PATH`.
    Path,
    /// Not on the daemon's `PATH`.
    NotFound,
}

/// Where the daemon's `codex` is, if it has one.
#[derive(Clone, Debug)]
pub struct CodexPush {
    pub bin: Option<PathBuf>,
    pub timeout: Duration,
    pub source: CodexSource,
}

impl Default for CodexPush {
    fn default() -> Self {
        CodexPush {
            bin: None,
            timeout: QUEUE_TIMEOUT,
            source: CodexSource::Disabled,
        }
    }
}

impl CodexPush {
    /// From `ARTIFAX_CODEX_BIN` when set (empty disables push; any other value
    /// is the binary, used as is), else `codex` looked up on `path`.
    pub fn from_env(
        artifax_codex_bin: Option<std::ffi::OsString>,
        path: Option<&OsStr>,
    ) -> CodexPush {
        let (bin, source) = match artifax_codex_bin {
            Some(v) if v.is_empty() => (None, CodexSource::Disabled),
            Some(v) => (Some(PathBuf::from(v)), CodexSource::Env),
            None => match find_on_path("codex", path) {
                Some(p) => (Some(p), CodexSource::Path),
                None => (None, CodexSource::NotFound),
            },
        };
        CodexPush {
            bin,
            timeout: QUEUE_TIMEOUT,
            source,
        }
    }
    pub fn available(&self) -> bool {
        self.bin.is_some()
    }
}

/// The first executable regular file named `name` in the directories of `path`.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

#[derive(Debug, PartialEq)]
pub enum QueueOutcome {
    Queued,
    Rejected(Option<i32>),
    TimedOut,
    SpawnFailed(String),
}

/// Runs `<bin> queue --thread <thread> --message <message>` with `CODEX_HOME`
/// set when known, stdio detached, killed after `timeout`.
pub async fn run_queue(
    bin: &Path,
    timeout: Duration,
    thread: &str,
    message: &str,
    codex_home: Option<&str>,
) -> QueueOutcome {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["queue", "--thread", thread, "--message", message])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(h) = codex_home {
        cmd.env("CODEX_HOME", h);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return QueueOutcome::SpawnFailed(e.to_string()),
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) if status.success() => QueueOutcome::Queued,
        Ok(Ok(status)) => QueueOutcome::Rejected(status.code()),
        Ok(Err(e)) => QueueOutcome::SpawnFailed(e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            QueueOutcome::TimedOut
        }
    }
}

/// For each target that is a live Codex session with a known Codex session ID,
/// claims its undelivered rows on watches with replies armed (tier `queue`)
/// and runs `codex queue` in the background, never blocking the caller.
/// Queued: the claim stands and the new states are published. Non-zero exit:
/// the rows are released, the session is ended (so its rows go to the next
/// session that publishes or watches), and the states are published
/// (`agent_ended` when no other session remains). Timeout or spawn failure:
/// the rows are released for the other tiers. Never retried.
pub fn dispatch(ctx: &FeedbackCtx, st: &Store, targets: &BTreeSet<String>) {
    let Some(bin) = ctx.codex.bin.clone() else {
        return;
    };
    for sid in targets {
        let Ok(Some(session)) = st.get_session(sid) else {
            continue;
        };
        if session.harness != "codex" || session.ended_at.is_some() {
            continue;
        }
        let Some(thread) = session.harness_session_id.clone() else {
            continue;
        };
        let q = TakeFeedback {
            session_id: sid.clone(),
            tier: Tier::Queue,
            artifact_id: None,
            include_resends: false,
        };
        let (items, claimed) = match st.take_feedback(&q, &ctx.browser_base) {
            Ok((items, touched)) if !items.is_empty() => (items, touched),
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(session = %sid, error = %e, "claiming feedback for codex queue failed");
                continue;
            }
        };
        let codex_home = match st.codex_home(sid) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(session = %sid, error = %e, "reading CODEX_HOME failed; queueing without it");
                None
            }
        };
        let ids: Vec<String> = items.iter().map(|i| i.feedback_id.clone()).collect();
        let message = render_items(&items);
        let (ctx, bin, sid, timeout) = (ctx.clone(), bin.clone(), sid.clone(), ctx.codex.timeout);
        ctx.handle.clone().spawn(async move {
            let outcome = run_queue(&bin, timeout, &thread, &message, codex_home.as_deref()).await;
            let store = ctx.store.clone();
            let settled = tokio::task::spawn_blocking(move || {
                let mut touched: Touched = claimed;
                match &outcome {
                    QueueOutcome::Queued => {}
                    QueueOutcome::Rejected(code) => {
                        tracing::warn!(session = %sid, ?code, "codex queue failed; ending the session");
                        match store.release_feedback(&ids) {
                            Ok(t) => touched.merge(t),
                            Err(e) => tracing::warn!(error = %e, "releasing feedback failed"),
                        }
                        match store.end_session_touched(&sid) {
                            Ok((_, t)) => touched.merge(t),
                            Err(e) => tracing::warn!(error = %e, "ending the Codex session failed"),
                        }
                        ctx.waiters.forget(&sid);
                    }
                    QueueOutcome::TimedOut | QueueOutcome::SpawnFailed(_) => {
                        tracing::warn!(session = %sid, ?outcome, "codex queue did not run; leaving the rows to the other tiers");
                        match store.release_feedback(&ids) {
                            Ok(t) => touched.merge(t),
                            Err(e) => tracing::warn!(error = %e, "releasing feedback failed"),
                        }
                    }
                }
                publish_states(&ctx, &store, &touched);
                ctx.waiters.wake(&touched.targets);
            })
            .await;
            if let Err(e) = settled {
                tracing::warn!(error = %e, "settling a codex queue outcome failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn finds_only_executable_files_on_path() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("codex"), "not executable").unwrap();
        let exe = script(b.path(), "codex", "exit 0");
        let path = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(find_on_path("codex", Some(&path)), Some(exe));
        assert_eq!(find_on_path("codex", None), None);
        assert_eq!(find_on_path("nope", Some(&path)), None);
    }

    #[test]
    fn artifax_codex_bin_overrides_path_and_empty_disables() {
        let d = tempfile::tempdir().unwrap();
        let on_path = script(d.path(), "codex", "exit 0");
        let path = std::env::join_paths([d.path()]).unwrap();
        let p = CodexPush::from_env(None, Some(&path));
        assert_eq!(
            (p.bin.clone(), p.source),
            (Some(on_path), CodexSource::Path)
        );
        let p = CodexPush::from_env(Some("".into()), Some(&path));
        assert_eq!(
            (p.bin.clone(), p.source, p.available()),
            (None, CodexSource::Disabled, false)
        );
        let p = CodexPush::from_env(Some("/opt/fake/codex".into()), Some(&path));
        assert_eq!(
            (p.bin.clone(), p.source),
            (Some(PathBuf::from("/opt/fake/codex")), CodexSource::Env)
        );
        let empty = std::env::join_paths([tempfile::tempdir().unwrap().path()]).unwrap();
        assert_eq!(
            CodexPush::from_env(None, Some(&empty)).source,
            CodexSource::NotFound
        );
    }

    #[tokio::test]
    async fn run_queue_reports_each_outcome_and_passes_codex_home() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("out.txt");
        let ok = script(
            d.path(),
            "ok",
            &format!("printf '%s|' \"$@\" \"$CODEX_HOME\" > '{}'", out.display()),
        );
        assert_eq!(
            run_queue(&ok, QUEUE_TIMEOUT, "th-1", "hi\nthere", Some("/cx")).await,
            QueueOutcome::Queued
        );
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "queue|--thread|th-1|--message|hi\nthere|/cx|"
        );
        let bad = script(d.path(), "bad", "exit 3");
        assert_eq!(
            run_queue(&bad, QUEUE_TIMEOUT, "t", "m", None).await,
            QueueOutcome::Rejected(Some(3))
        );
        let slow = script(d.path(), "slow", "sleep 5");
        assert_eq!(
            run_queue(&slow, Duration::from_millis(200), "t", "m", None).await,
            QueueOutcome::TimedOut
        );
        assert!(matches!(
            run_queue(
                Path::new("/nonexistent/codex"),
                QUEUE_TIMEOUT,
                "t",
                "m",
                None
            )
            .await,
            QueueOutcome::SpawnFailed(_)
        ));
    }
}
