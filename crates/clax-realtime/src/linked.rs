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
    /// What the last cycle could not read of each link, by its artifact
    /// and remote: a link only, while its remote's other links sync.
    pub read_errors: Mutex<HashMap<(String, String), String>>,
    /// Tests: requests to remotes fail as unreachable while set.
    pub offline: AtomicBool,
    /// Tests: while set, the threads followers' sessions were handed there
    /// wait to be sent here ([`Linked::hold_routes`]).
    pub hold_routes: AtomicBool,
    /// How many threads followers' sessions were handed there wait to be
    /// sent here, as the last cycle left them.
    pub routes_waiting: AtomicUsize,
    /// Tests: a followed artifact's session there is refused each watch it
    /// asks for there, as a remote at its limit (429) refuses one.
    pub refuse_follows: AtomicBool,
    /// Tests: the answers to this many up-sends are lost after the remote
    /// acted on them.
    pub lose_answers: AtomicUsize,
    /// Tests: a read's page size and its pages per cycle, each in place
    /// of the sync's own when not 0 ([`Linked::set_read_limits`]).
    pub read_limits: [AtomicUsize; 2],
    /// Tests: a read to go on waits while set, its page cursor kept.
    pub hold_resumed: AtomicBool,
    pub started: AtomicBool,
    /// The session there each local session watching a followed remote
    /// artifact speaks in, by remote and local session: its ID and secret
    /// there. The sync opens and ends them; that local session's writes
    /// on that remote go up in it.
    pub sessions_there: Mutex<HashMap<(String, String), (String, clax_core::Secret)>>,
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
            read_errors: Mutex::default(),
            offline: AtomicBool::new(false),
            refuse_follows: AtomicBool::new(false),
            hold_routes: AtomicBool::new(false),
            routes_waiting: AtomicUsize::new(0),
            lose_answers: AtomicUsize::new(0),
            read_limits: Default::default(),
            hold_resumed: AtomicBool::new(false),
            started: AtomicBool::new(false),
            sessions_there: Mutex::default(),
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

    /// Tests: while `on`, the threads followers' sessions are handed there
    /// wait (counted in [`Linked::routes_waiting`]) instead of being sent
    /// here.
    #[cfg(debug_assertions)]
    pub fn hold_routes(&self, on: bool) {
        self.hold_routes
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Tests: refuses, while `on`, each watch a followed artifact's session
    /// there asks for, as a remote at its limit does (429).
    #[cfg(debug_assertions)]
    pub fn set_refuse_follows(&self, on: bool) {
        self.refuse_follows
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Tests: loses the answers to the next `n` up-sends after the remote
    /// acted on them.
    #[cfg(debug_assertions)]
    pub fn lose_next_answers(&self, n: usize) {
        self.lose_answers
            .store(n, std::sync::atomic::Ordering::SeqCst);
    }

    /// What the last cycle could not read of artifact `aid`'s link to
    /// remote `rid`; `None` when it read it all.
    pub fn read_error(&self, aid: &str, rid: &str) -> Option<String> {
        self.read_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(aid.to_string(), rid.to_string()))
            .cloned()
    }

    /// Sets, or with `None` clears, what the last cycle could not read of
    /// artifact `aid`'s link to remote `rid`.
    pub fn set_read_error(&self, aid: &str, rid: &str, e: Option<String>) {
        let mut m = self.read_errors.lock().unwrap_or_else(|e| e.into_inner());
        let k = (aid.to_string(), rid.to_string());
        match e {
            Some(e) => {
                m.insert(k, e);
            }
            None => {
                m.remove(&k);
            }
        }
    }

    /// Tests: reads ask pages of `page` threads and take at most `pages`
    /// pages a cycle; 0 keeps the sync's own.
    #[cfg(debug_assertions)]
    pub fn set_read_limits(&self, page: usize, pages: usize) {
        for (a, n) in self.read_limits.iter().zip([page, pages]) {
            a.store(n, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Tests: while `on`, a read that ran out of pages does not go on.
    #[cfg(debug_assertions)]
    pub fn hold_resumed(&self, on: bool) {
        self.hold_resumed
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Limit `i` of [`Linked::read_limits`], or `default` when unset.
    pub fn read_limit(&self, i: usize, default: usize) -> usize {
        match self.read_limits[i].load(std::sync::atomic::Ordering::SeqCst) {
            0 => default,
            n => n,
        }
    }

    /// The session on remote `rid` that local session `sid` speaks in
    /// there, while it watches a followed artifact there: its ID and
    /// secret.
    pub fn session_there(&self, rid: &str, sid: &str) -> Option<(String, clax_core::Secret)> {
        self.sessions_there
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(rid.to_string(), sid.to_string()))
            .cloned()
    }

    /// Records, or with `None` forgets, the session on remote `rid` that
    /// local session `sid` speaks in there.
    pub fn set_session_there(
        &self,
        rid: &str,
        sid: &str,
        there: Option<(String, clax_core::Secret)>,
    ) {
        let mut m = self
            .sessions_there
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let k = (rid.to_string(), sid.to_string());
        match there {
            Some(t) => {
                m.insert(k, t);
            }
            None => {
                m.remove(&k);
            }
        }
    }

    /// Changes how remote `rid` stands.
    pub fn set_status(&self, rid: &str, f: impl FnOnce(&mut SyncStatus)) {
        let mut m = self.status.lock().unwrap_or_else(|e| e.into_inner());
        f(m.entry(rid.to_string()).or_default());
    }
}
