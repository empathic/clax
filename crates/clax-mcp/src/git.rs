//! The shim's git capture (spec 2026-10-06-toolpath-audit-design §9.3):
//! the session's working directory, captured at registration and on each
//! tool call that changes history, on a blocking thread under
//! [`CAPTURE_DEADLINE`](clax_core::gitctx::CAPTURE_DEADLINE). A capture
//! never fails a call: without a context it is an outcome. Git state is
//! never cached, because an edit changes the tree between calls; only the
//! executable and its version are read once.

use crate::calls::GitStart;
use clax_core::gitctx::{self, GitField};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// The variable a test sets to change the capture deadline, in
/// milliseconds; read by debug builds only.
pub const DEADLINE_VAR: &str = "CLAX_TEST_GIT_DEADLINE_MS";

/// Captures git state with one git executable, whose version is read once
/// (capture needs git 2.44 or later; below, every capture is `unavailable`
/// and runs nothing more).
#[derive(Clone, Debug)]
pub struct GitCapture {
    /// The git found on `PATH` when the shim started, and its probe, made
    /// once; `None` when there was none, and every capture is then
    /// `unavailable`.
    git: Option<Arc<Probe>>,
    deadline: Duration,
}

/// A git executable and its [`gitctx::Git`] probe, read on first use.
#[derive(Debug)]
struct Probe {
    path: PathBuf,
    git: OnceLock<gitctx::Git>,
}

impl Probe {
    fn git(&self) -> &gitctx::Git {
        self.git.get_or_init(|| gitctx::Git::probe(&self.path))
    }
}

impl GitCapture {
    /// Captures with the `git` on this process's `PATH`, found once, within
    /// [`CAPTURE_DEADLINE`](gitctx::CAPTURE_DEADLINE). In debug builds
    /// only, tests that drive the built binary may set [`DEADLINE_VAR`] to a
    /// deadline in milliseconds (at most a minute), so a loaded machine
    /// cannot turn their capture into a timeout.
    pub fn from_env() -> GitCapture {
        let deadline = std::env::var(DEADLINE_VAR)
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(gitctx::CAPTURE_DEADLINE, |ms| {
                Duration::from_millis(ms.min(60_000))
            });
        let capture = GitCapture::with(
            gitctx::find_git(std::env::var_os("PATH").as_deref()),
            deadline,
        );
        // Read the version now, off the first call's path.
        if let Some(probe) = capture.git.clone() {
            std::thread::spawn(move || {
                probe.git();
            });
        }
        capture
    }

    /// Captures with `git` (none: `unavailable`), each capture finishing
    /// within `deadline`.
    pub fn with(git: Option<PathBuf>, deadline: Duration) -> GitCapture {
        GitCapture {
            git: git.map(|path| {
                Arc::new(Probe {
                    path,
                    git: OnceLock::new(),
                })
            }),
            deadline,
        }
    }

    /// Starts capturing `cwd` (empty or absent: `no-cwd`).
    pub fn start(&self, cwd: Option<&str>) -> GitStart {
        let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
            return GitStart::Ready(GitField::Capture("no-cwd"));
        };
        let Some(git) = self.git.clone() else {
            return GitStart::Ready(GitField::Capture("unavailable"));
        };
        let cwd = PathBuf::from(cwd);
        let deadline = self.deadline;
        GitStart::Running(tokio::task::spawn_blocking(move || {
            let git = git.git();
            gitctx::capture(&cwd, std::time::Instant::now() + deadline, git)
        }))
    }

    /// Captures `cwd`, waiting for the result.
    pub async fn capture(&self, cwd: Option<&str>) -> GitField {
        match self.start(cwd) {
            GitStart::Ready(f) => f,
            GitStart::Running(t) => t.await.unwrap_or(GitField::Capture("unavailable")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_directory_or_git_there_is_an_outcome() {
        let g = GitCapture::with(None, Duration::from_millis(300));
        assert_eq!(g.capture(None).await, GitField::Capture("no-cwd"));
        assert_eq!(g.capture(Some("")).await, GitField::Capture("no-cwd"));
        assert_eq!(g.capture(Some("/")).await, GitField::Capture("unavailable"));
    }
}
