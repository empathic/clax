//! Linked threads' sync state on a local daemon (spec
//! 2026-10-15-linked-threads-design): the timers, the task per remote and
//! how each remote stands. The sync itself, which acts through the
//! daemon's routes, is the daemon's `linked`.

use crate::questions::Sleeper;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// How one remote's sync stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncStatus {
    /// Since when the remote could not be reached; `None` while it can.
    pub unreachable_since: Option<String>,
    pub last_error: Option<String>,
    /// When a cycle last finished.
    pub synced_at: Option<String>,
}

pub struct Task {
    pub kick: Arc<Notify>,
    pub retry: Arc<Notify>,
    pub handle: clax_rt::JoinHandle<()>,
}

/// The sync's state on a local daemon: its timers, its tasks and how each
/// remote stands.
pub struct Linked {
    pub sleeper: Arc<dyn Sleeper>,
    pub supervise: Notify,
    pub tasks: Mutex<HashMap<String, Task>>,
    /// Each linked artifact's remotes, as of the last supervision.
    pub artifacts: Mutex<HashMap<String, Vec<String>>>,
    pub status: Mutex<HashMap<String, SyncStatus>>,
    /// Tests: requests to remotes fail as unreachable while set.
    pub offline: AtomicBool,
    /// Tests: the answers to this many up-sends are lost after the remote
    /// acted on them.
    pub lose_answers: AtomicUsize,
    pub started: AtomicBool,
    /// Set under [`Linked::tasks`]'s lock once the sync stopped: no task is
    /// started after.
    pub stopped: AtomicBool,
}

impl Linked {
    /// The sync's state, its timers on `sleeper`.
    pub fn new(sleeper: Arc<dyn Sleeper>) -> Linked {
        Linked {
            sleeper,
            supervise: Notify::new(),
            tasks: Mutex::default(),
            artifacts: Mutex::default(),
            status: Mutex::default(),
            offline: AtomicBool::new(false),
            lose_answers: AtomicUsize::new(0),
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }
    }

    /// How remote `rid`'s sync stands.
    pub fn status(&self, rid: &str) -> SyncStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(rid)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether a task runs for remote `rid`.
    pub fn running(&self, rid: &str) -> bool {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(rid)
    }

    /// Wakes remote `rid`'s task, or every task for `None`.
    pub fn kick(&self, rid: Option<&str>) {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for (id, t) in tasks.iter() {
            if rid.is_none_or(|r| r == id) {
                t.kick.notify_one();
            }
        }
    }

    /// Ends remote `rid`'s backoff now: its task tries again at once (the
    /// owner's Retry). A local write during an outage only queues.
    pub fn retry(&self, rid: &str) {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = tasks.get(rid) {
            t.retry.notify_one();
            t.kick.notify_one();
        }
    }

    /// Tells the supervisor the links or remotes changed: tasks start or
    /// stop to match.
    pub fn links_changed(&self) {
        self.supervise.notify_one();
    }

    /// Tests: makes every request to a remote fail as unreachable while
    /// `on`.
    #[cfg(debug_assertions)]
    pub fn set_offline(&self, on: bool) {
        self.offline.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Tests: loses the answers to the next `n` up-sends after the remote
    /// acted on them.
    #[cfg(debug_assertions)]
    pub fn lose_next_answers(&self, n: usize) {
        self.lose_answers
            .store(n, std::sync::atomic::Ordering::SeqCst);
    }

    /// Changes how remote `rid` stands.
    pub fn set_status(&self, rid: &str, f: impl FnOnce(&mut SyncStatus)) {
        let mut m = self.status.lock().unwrap_or_else(|e| e.into_inner());
        f(m.entry(rid.to_string()).or_default());
    }
}
