//! Tier 5 for Codex: hands feedback to a Codex session with
//! `codex queue --thread <harness_session_id> --message <payload>`.
//!
//! Measured on Codex 0.158 (docs/contract.md): an idle attached TUI starts a
//! turn within a second, a busy one runs the message as its next turn, and with
//! no client attached the message is held until `codex resume`. Exit 0 means
//! queued, not seen, so the rows stay unacknowledged and the in-band tiers
//! resend them after two minutes.

use crate::feedback::{FeedbackCtx, publish_states};
use clax_core::feedback::{Tier, Touched, render_items};
use clax_core::{Store, TakeFeedback};
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
    /// `CLAX_CODEX_BIN` named an executable file.
    Env,
    /// `CLAX_CODEX_BIN` was set and empty: Codex push is off.
    Disabled,
    /// Found on the daemon's `PATH`.
    Path,
    /// Not on the daemon's `PATH`, or `CLAX_CODEX_BIN` named something
    /// that is not an executable file (then `rejected` holds it).
    NotFound,
}

/// Where the daemon's `codex` is, if it has one.
#[derive(Clone, Debug)]
pub struct CodexPush {
    pub bin: Option<PathBuf>,
    pub timeout: Duration,
    pub source: CodexSource,
    /// What `CLAX_CODEX_BIN` named when it is not an executable file.
    pub rejected: Option<PathBuf>,
}

impl Default for CodexPush {
    fn default() -> Self {
        CodexPush {
            bin: None,
            timeout: QUEUE_TIMEOUT,
            source: CodexSource::Disabled,
            rejected: None,
        }
    }
}

impl CodexPush {
    /// From `CLAX_CODEX_BIN` when set (empty disables push; any other value
    /// is the binary, used as is when it is an executable file and otherwise
    /// `NotFound`), else `codex` looked up on `path`.
    pub fn from_env(clax_codex_bin: Option<std::ffi::OsString>, path: Option<&OsStr>) -> CodexPush {
        let mut rejected = None;
        let (bin, source) = match clax_codex_bin {
            Some(v) if v.is_empty() => (None, CodexSource::Disabled),
            Some(v) if is_executable_file(Path::new(&v)) => {
                (Some(PathBuf::from(v)), CodexSource::Env)
            }
            Some(v) => {
                rejected = Some(PathBuf::from(v));
                (None, CodexSource::NotFound)
            }
            None => match find_on_path("codex", path) {
                Some(p) => (Some(p), CodexSource::Path),
                None => (None, CodexSource::NotFound),
            },
        };
        CodexPush {
            bin,
            timeout: QUEUE_TIMEOUT,
            source,
            rejected,
        }
    }

    /// Why Codex push is unavailable; `None` when it is available.
    pub fn reason(&self) -> Option<String> {
        if self.available() {
            return None;
        }
        Some(match (&self.source, &self.rejected) {
            (CodexSource::Disabled, _) => {
                "Codex push is off: CLAX_CODEX_BIN is set empty".to_string()
            }
            (_, Some(p)) => format!(
                "CLAX_CODEX_BIN names {}, which is not an executable file; native push disabled",
                p.display()
            ),
            _ => "codex is not on the daemon's PATH; native push disabled".to_string(),
        })
    }

    pub fn available(&self) -> bool {
        self.bin.is_some()
    }
}

/// The first executable regular file named `name` in the directories of `path`.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|d| d.join(name))
        .find(|p| is_executable_file(p))
}

fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[derive(Debug, PartialEq)]
pub enum QueueOutcome {
    Queued,
    Rejected(Option<i32>),
    TimedOut,
    SpawnFailed(String),
}

impl QueueOutcome {
    /// What went wrong, as recorded in the session's `push.last_error`;
    /// `None` for [`QueueOutcome::Queued`].
    pub fn failure(&self) -> Option<String> {
        match self {
            QueueOutcome::Queued => None,
            QueueOutcome::Rejected(Some(code)) => {
                Some(format!("codex queue exited with code {code}"))
            }
            QueueOutcome::Rejected(None) => Some("codex queue was killed by a signal".to_string()),
            QueueOutcome::TimedOut => Some("codex queue timed out".to_string()),
            QueueOutcome::SpawnFailed(e) => Some(format!("codex queue could not run: {e}")),
        }
    }
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
///
/// A target with a `wait_for_feedback` long-poll (`tier=wait`) in progress is
/// skipped ([`crate::feedback::FeedbackWaiters::is_waiting`]): the woken poll
/// delivers the rows in-band (tier `wait`). A row
/// committed after that poll's last take and before it returns is left to the
/// in-band tiers.
///
/// The claim comes first, so no other tier hands the rows over meanwhile, and
/// its `feedback_state` (`delivered` by `queue`) is published at once: for up
/// to the timeout (10 s) the rows read `delivered` before `codex queue` has
/// confirmed. Exit 0: the claim stands and the session's recorded push error
/// is cleared; nothing more is published. A
/// non-zero exit, a timeout, or a spawn failure: the rows are released and
/// marked push-failed, so they read `sent` waiting on the in-band tiers and
/// `queue` does not take them again, the released states are published, and
/// the failure is recorded on the
/// session (`push.last_error` and `push.last_error_at` in
/// `GET /api/sessions/<id>`). Dispatch never ends a session: a non-zero exit
/// was measured to mean a CLI or app-server failure, never that the session
/// is gone (docs/contract.md). Never retried.
pub fn dispatch(ctx: &FeedbackCtx, st: &Store, targets: &BTreeSet<String>) {
    let Some(bin) = ctx.codex.bin.clone() else {
        return;
    };
    for sid in targets {
        let Ok(Some(session)) = st.get_session(sid) else {
            continue;
        };
        if session.harness != "codex" || session.ended_at.is_some() || ctx.waiters.is_waiting(sid) {
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
        let items = match st.take_feedback(&q, &ctx.browser_base) {
            Ok((items, claimed)) if !items.is_empty() => {
                // Announced now: a dispatch triggered without a `thread` event
                // (a watch or publish retargeting rows) has nothing else to.
                publish_states(ctx, st, &claimed);
                items
            }
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
        let marked = items.clone();
        let (ctx, bin, sid, timeout) = (ctx.clone(), bin.clone(), sid.clone(), ctx.codex.timeout);
        ctx.handle.clone().spawn(async move {
            let outcome = run_queue(&bin, timeout, &thread, &message, codex_home.as_deref()).await;
            let store = ctx.store.clone();
            let settled = store.call(move |store| {
                let mut touched = Touched::default();
                let error = outcome.failure();
                if let Some(reason) = &error {
                    tracing::warn!(session = %sid, reason = %reason, "codex queue failed; leaving the rows to the other tiers");
                    match store.release_feedback(&ids) {
                        Ok(t) => touched.merge(t),
                        Err(e) => tracing::warn!(error = %e, "releasing feedback failed"),
                    }
                }
                if error.is_none()
                    && let Err(e) = crate::working::mark_items(&ctx, store, &sid, &marked)
                {
                    tracing::warn!(session = %sid, error = %e, "marking a queued session working failed");
                }
                if let Err(e) = store.set_push_error(&sid, error.as_deref()) {
                    tracing::warn!(session = %sid, error = %e, "recording the codex queue outcome failed");
                }
                publish_states(&ctx, store, &touched);
                ctx.waiters.wake(&touched.targets);
                Ok(())
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

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        clax_fake_exe::install(&dir.join(name), &format!("#!/bin/sh\n{body}\n"))
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
    fn clax_codex_bin_overrides_path_and_empty_disables() {
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
        let named = script(d.path(), "named-codex", "exit 0");
        let p = CodexPush::from_env(Some(named.clone().into()), Some(&path));
        assert_eq!(
            (p.bin.clone(), p.source, p.reason()),
            (Some(named), CodexSource::Env, None)
        );
        let p = CodexPush::from_env(Some("/opt/fake/codex".into()), Some(&path));
        assert_eq!((p.bin.clone(), p.source), (None, CodexSource::NotFound));
        assert_eq!(
            p.reason().as_deref(),
            Some(
                "CLAX_CODEX_BIN names /opt/fake/codex, which is not an executable file; native push disabled"
            )
        );
        std::fs::write(d.path().join("plain"), "").unwrap();
        let plain = CodexPush::from_env(Some(d.path().join("plain").into()), Some(&path));
        assert_eq!(plain.source, CodexSource::NotFound, "not executable");
        let empty = std::env::join_paths([tempfile::tempdir().unwrap().path()]).unwrap();
        let p = CodexPush::from_env(None, Some(&empty));
        assert_eq!(p.source, CodexSource::NotFound);
        assert_eq!(
            p.reason().as_deref(),
            Some("codex is not on the daemon's PATH; native push disabled")
        );
        assert_eq!(
            CodexPush::default().reason().as_deref(),
            Some("Codex push is off: CLAX_CODEX_BIN is set empty")
        );
    }

    #[tokio::test]
    async fn run_queue_reports_each_outcome_and_passes_codex_home() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("out.txt");
        let ok = script(
            d.path(),
            "ok",
            "printf '%s|' \"$@\" \"$CODEX_HOME\" > \"$(dirname \"$0\")/out.txt\"",
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
