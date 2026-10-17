//! How Claude Code's call-ID reports wait for their `tool.call`
//! (`crate::routes::sessions::tool_call_ids`).

use std::collections::HashMap;
use std::sync::Arc;

/// How long an unmatched call-ID report waits for its `tool.call` in the
/// daemon ([`CallIdWait::grace`]).
pub const CALL_ID_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// How many call-ID reports may wait at once in the daemon
/// ([`CallIdWait::cap`]).
pub const CALL_ID_CAP: usize = 64;

/// How call-ID reports wait for their `tool.call` (spec
/// 2026-10-06-toolpath-audit-design §6.7). The shim reports a call in the
/// background once its result has gone back, and Claude Code runs the
/// PostToolUse hook once it has the result, so the hook's report may come
/// first. A report that finds no call waits up to `grace`, woken when its
/// own session records a `tool.call`; at most `cap` reports wait at once,
/// and one past that records `call_id: null` at once. Two reports looking
/// at once for the same session, tool and argument hash cannot tell their
/// calls apart, so both record `call_id: null`.
pub struct CallIdWait {
    /// How long an unmatched report waits ([`CALL_ID_GRACE`] in the daemon).
    pub grace: std::time::Duration,
    /// How many reports may wait at once ([`CALL_ID_CAP`] in the daemon).
    pub cap: usize,
    /// How many reports are waiting now.
    pub parked: tokio::sync::watch::Sender<usize>,
    /// The wake-up of each session a report is looking in, with how many
    /// reports hold it.
    sessions: std::sync::Mutex<HashMap<String, (Arc<tokio::sync::Notify>, usize)>>,
    /// The reports looking now, by session, bare tool name and argument
    /// hash: how many, and whether more than one has looked at once.
    looking: std::sync::Mutex<HashMap<CallKey, (usize, bool)>>,
}

/// A report's session, bare tool name and argument hash.
type CallKey = (String, String, String);

impl CallIdWait {
    pub fn new(grace: std::time::Duration, cap: usize) -> CallIdWait {
        CallIdWait {
            grace,
            cap,
            parked: tokio::sync::watch::channel(0).0,
            sessions: Default::default(),
            looking: Default::default(),
        }
    }

    /// Counts a report for session `sid`, bare tool `tool` and hash
    /// `args_sha256` as looking until the returned guard drops. When
    /// another such report is already looking, both are contested
    /// ([`Looking::contested`]), and the session's reports are woken so the
    /// earlier one sees it.
    pub fn look(&self, sid: &str, tool: &str, args_sha256: &str) -> Looking<'_> {
        let key = (sid.to_string(), tool.to_string(), args_sha256.to_string());
        let contested = {
            let mut map = self.looking.lock().unwrap_or_else(|e| e.into_inner());
            let entry = map.entry(key.clone()).or_insert((0, false));
            if entry.0 > 0 {
                entry.1 = true;
            }
            entry.0 += 1;
            entry.1
        };
        if contested {
            self.recorded(sid);
        }
        Looking { wait: self, key }
    }

    /// The wake-up for session `sid`, held until the returned guard drops.
    pub fn watch(&self, sid: &str) -> SessionWake<'_> {
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map
            .entry(sid.to_string())
            .or_insert_with(|| (Arc::default(), 0));
        entry.1 += 1;
        SessionWake {
            wait: self,
            sid: sid.to_string(),
            notify: entry.0.clone(),
        }
    }

    /// Wakes the reports looking in session `sid`: it recorded a
    /// `tool.call`.
    pub fn recorded(&self, sid: &str) {
        let map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((notify, _)) = map.get(sid) {
            notify.notify_waiters();
        }
    }

    /// Counts a report as waiting until the returned guard drops; `None`
    /// when [`CallIdWait::cap`] reports already wait.
    pub fn park(&self) -> Option<Parked<'_>> {
        let cap = self.cap;
        self.parked
            .send_if_modified(|n| {
                let room = *n < cap;
                if room {
                    *n += 1;
                }
                room
            })
            .then(|| Parked(&self.parked))
    }
}

/// A session's wake-up, held by a report looking in it.
pub struct SessionWake<'a> {
    wait: &'a CallIdWait,
    sid: String,
    pub notify: Arc<tokio::sync::Notify>,
}

impl Drop for SessionWake<'_> {
    fn drop(&mut self) {
        let mut map = self.wait.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.sid) {
            entry.1 -= 1;
            if entry.1 == 0 {
                map.remove(&self.sid);
            }
        }
    }
}

/// A report looking for its call, counted in [`CallIdWait::look`] until
/// dropped.
pub struct Looking<'a> {
    wait: &'a CallIdWait,
    key: CallKey,
}

impl Looking<'_> {
    /// Whether another report for the same session, tool and argument hash
    /// has looked while this one did.
    pub fn contested(&self) -> bool {
        let map = self.wait.looking.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&self.key).is_some_and(|e| e.1)
    }
}

impl Drop for Looking<'_> {
    fn drop(&mut self) {
        let mut map = self.wait.looking.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.key) {
            entry.0 -= 1;
            if entry.0 == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// A waiting report, counted in [`CallIdWait::parked`] until dropped.
pub struct Parked<'a>(&'a tokio::sync::watch::Sender<usize>);

impl Drop for Parked<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[test]
    fn a_call_wakes_only_its_own_sessions_reports() {
        let wait = CallIdWait::new(CALL_ID_GRACE, CALL_ID_CAP);
        let a = wait.watch("A");
        let b = wait.watch("B");
        {
            let woken_a = a.notify.notified();
            let woken_b = b.notify.notified();
            tokio::pin!(woken_a, woken_b);
            woken_a.as_mut().enable();
            woken_b.as_mut().enable();
            wait.recorded("A");
            assert!(woken_a.now_or_never().is_some(), "A's report is woken");
            assert!(woken_b.now_or_never().is_none(), "B's report is not");
        }
        drop((a, b));
        assert!(
            wait.sessions.lock().unwrap().is_empty(),
            "a session no report holds is forgotten"
        );
    }

    #[test]
    fn two_reports_for_one_call_contest_each_other() {
        let wait = CallIdWait::new(CALL_ID_GRACE, CALL_ID_CAP);
        let first = wait.look("S", "list", "h");
        assert!(!first.contested());
        // Another session, tool or hash does not contest it.
        let others = [
            wait.look("T", "list", "h"),
            wait.look("S", "read", "h"),
            wait.look("S", "list", "g"),
        ];
        assert!(!first.contested());
        assert!(others.iter().all(|o| !o.contested()));
        let second = wait.look("S", "list", "h");
        assert!(first.contested() && second.contested());
        drop((first, second, others));
        assert!(wait.looking.lock().unwrap().is_empty());
        assert!(!wait.look("S", "list", "h").contested());
    }

    #[test]
    fn no_more_than_cap_reports_wait() {
        let wait = CallIdWait::new(CALL_ID_GRACE, 2);
        let one = wait.park().unwrap();
        let _two = wait.park().unwrap();
        assert!(wait.park().is_none());
        assert_eq!(*wait.parked.borrow(), 2);
        drop(one);
        assert!(wait.park().is_some());
    }
}
