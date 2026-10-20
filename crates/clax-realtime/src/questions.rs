//! Agent questions in the daemon (spec 2026-10-06-agent-questions-and-inbox):
//! which polls wait on each question, and which hook question's grace is
//! due. The view its person sees, announcing changes to that person and
//! starting the grace are the HTTP layer's `views::question`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// The timers of question polls and graces: real time in the daemon, a
/// clock the test drives in tests.
pub trait Sleeper: Send + Sync {
    /// Completes after `d`.
    fn sleep(&self, d: std::time::Duration) -> futures::future::BoxFuture<'static, ()>;

    /// Whether graces run on this sleeper's timers (`true`); `false` leaves
    /// them to the sweep ([`withdraw_due`], from `AppState::sweep`, which
    /// `AppState::next_sweep_in` says when to run): a host that must not
    /// hold a timer open, a Durable Object, whose alarm runs the sweep.
    fn times_graces(&self) -> bool {
        true
    }
}

/// [`Sleeper`] on Tokio's clock for polls, with graces left to the sweep
/// ([`Sleeper::times_graces`]).
pub struct SweptGraces;

impl Sleeper for SweptGraces {
    fn sleep(&self, d: std::time::Duration) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(clax_rt::time::sleep(d))
    }

    fn times_graces(&self) -> bool {
        false
    }
}

/// [`Sleeper`] on Tokio's clock.
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, d: std::time::Duration) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(clax_rt::time::sleep(d))
    }
}

/// Runs when the last poll holding a question lets go.
pub type OnLast = Box<dyn FnOnce() + Send>;

/// Most polls that may hold one question at once.
pub const MAX_POLLS: usize = 4;

/// The polls waiting on each question (one `Notify` per question that has
/// a poll, and how many polls hold it), and the grace timer of each
/// question that has one pending.
#[derive(Default)]
pub struct QuestionWaiters {
    inner: Mutex<Inner>,
    /// Notified whenever a hold starts or ends.
    changed: Notify,
}

#[derive(Default)]
struct Inner {
    held: HashMap<String, (Arc<Notify>, usize)>,
    /// The generation of each question's pending grace timer; a poll
    /// holding the question, or a later timer, replaces or cancels it.
    armed: HashMap<String, u64>,
    /// When each swept grace ([`QuestionWaiters::arm_until`]) is due, by
    /// question, with its generation.
    due_at: HashMap<String, (u64, chrono::DateTime<chrono::Utc>)>,
    next: u64,
    /// Debug builds: how many of each question's polls are still to end at
    /// once, as if their timers had fired ([`QuestionWaiters::expire`]).
    #[cfg(debug_assertions)]
    expired: HashMap<String, usize>,
    /// Debug builds: how many of each question's polls are parked, their
    /// last read having found it open ([`QuestionWaiters::park`]).
    #[cfg(debug_assertions)]
    parked: HashMap<String, usize>,
}

/// Debug builds: one poll counted as parked on a question until dropped
/// ([`QuestionWaiters::park`]).
#[cfg(debug_assertions)]
pub struct ParkGuard {
    waiters: Arc<QuestionWaiters>,
    qid: String,
}

#[cfg(debug_assertions)]
impl Drop for ParkGuard {
    fn drop(&mut self) {
        {
            let mut g = self.waiters.lock();
            if let Some(n) = g.parked.get_mut(&self.qid) {
                *n -= 1;
                if *n == 0 {
                    g.parked.remove(&self.qid);
                }
            }
        }
        self.waiters.changed.notify_waiters();
    }
}

/// One poll's hold on a question ([`QuestionWaiters::hold`]).
pub struct HoldGuard {
    waiters: Arc<QuestionWaiters>,
    qid: String,
    on_last: Option<OnLast>,
}

impl QuestionWaiters {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The notify a poll of `qid` waits on, counting the poll as holding
    /// `qid` until the guard drops, and cancelling any pending grace of
    /// `qid`. `on_last` runs when that drop leaves no poll holding `qid`.
    /// `None` when [`MAX_POLLS`] polls hold `qid` already.
    pub fn hold(
        self: &Arc<Self>,
        qid: &str,
        on_last: Option<OnLast>,
    ) -> Option<(Arc<Notify>, HoldGuard)> {
        let mut g = self.lock();
        let e = g
            .held
            .entry(qid.to_string())
            .or_insert_with(|| (Arc::new(Notify::new()), 0));
        if e.1 >= MAX_POLLS {
            return None;
        }
        e.1 += 1;
        let notify = e.0.clone();
        g.armed.remove(qid);
        g.due_at.remove(qid);
        drop(g);
        self.changed.notify_waiters();
        Some((
            notify,
            HoldGuard {
                waiters: self.clone(),
                qid: qid.to_string(),
                on_last,
            },
        ))
    }

    /// Wakes every poll holding `qid`.
    pub fn wake(&self, qid: &str) {
        if let Some((n, _)) = self.lock().held.get(qid) {
            n.notify_waiters();
        }
    }

    /// Debug builds: ends every poll holding `qid` now, as if its timer
    /// had fired; how many polls that is.
    #[cfg(debug_assertions)]
    pub fn expire(&self, qid: &str) -> usize {
        let mut g = self.lock();
        let Some((n, held)) = g.held.get(qid).map(|(n, c)| (n.clone(), *c)) else {
            return 0;
        };
        g.expired.insert(qid.to_string(), held);
        drop(g);
        n.notify_waiters();
        held
    }

    /// Debug builds: whether a poll of `qid` is to end now
    /// ([`QuestionWaiters::expire`]); each poll takes it once.
    #[cfg(debug_assertions)]
    pub fn take_expired(&self, qid: &str) -> bool {
        let mut g = self.lock();
        match g.expired.get_mut(qid) {
            Some(n) if *n > 0 => {
                *n -= 1;
                if *n == 0 {
                    g.expired.remove(qid);
                }
                true
            }
            _ => false,
        }
    }

    /// Debug builds: counts a poll of `qid` as parked, waiting for a wake
    /// after a read that found the question open, until the guard drops.
    /// Tests wait on the count ([`QuestionWaiters::parked`]) so a change
    /// they make next is one the poll's first read did not see.
    #[cfg(debug_assertions)]
    pub fn park(self: &Arc<Self>, qid: &str) -> ParkGuard {
        *self.lock().parked.entry(qid.to_string()).or_default() += 1;
        self.changed.notify_waiters();
        ParkGuard {
            waiters: self.clone(),
            qid: qid.to_string(),
        }
    }

    /// Debug builds: how many polls of `qid` are parked
    /// ([`QuestionWaiters::park`]).
    #[cfg(debug_assertions)]
    pub fn parked(&self, qid: &str) -> usize {
        self.lock().parked.get(qid).copied().unwrap_or(0)
    }

    /// How many polls hold `qid`.
    pub fn count(&self, qid: &str) -> usize {
        self.lock().held.get(qid).map_or(0, |e| e.1)
    }

    /// Notified whenever a hold starts or ends.
    pub fn changed(&self) -> &Notify {
        &self.changed
    }

    /// Starts a grace timer for `qid`, replacing any pending one; its
    /// generation, for [`QuestionWaiters::due`].
    pub fn arm(&self, qid: &str) -> u64 {
        let mut g = self.lock();
        g.next += 1;
        let generation = g.next;
        g.armed.insert(qid.to_string(), generation);
        generation
    }

    /// [`QuestionWaiters::arm`], due at `at`: the sweep withdraws it then
    /// ([`QuestionWaiters::take_due`]).
    pub fn arm_until(&self, qid: &str, at: chrono::DateTime<chrono::Utc>) -> u64 {
        let generation = self.arm(qid);
        self.lock().due_at.insert(qid.to_string(), (generation, at));
        generation
    }

    /// When the next swept grace is due; `None` with none pending.
    pub fn next_due(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.lock().due_at.values().map(|(_, at)| *at).min()
    }

    /// The questions whose swept grace is due at `now` and may withdraw
    /// them ([`QuestionWaiters::due`]); those graces end.
    pub fn take_due(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<String> {
        let ready: Vec<(String, u64)> = {
            let mut g = self.lock();
            let ready: Vec<(String, u64)> = g
                .due_at
                .iter()
                .filter(|(_, (_, at))| *at <= now)
                .map(|(q, (n, _))| (q.clone(), *n))
                .collect();
            for (q, _) in &ready {
                g.due_at.remove(q);
            }
            ready
        };
        ready
            .into_iter()
            .filter(|(q, n)| self.due(q, *n))
            .map(|(q, _)| q)
            .collect()
    }

    /// Whether the grace timer `generation` of `qid` may withdraw it: it is
    /// still the pending one (no poll held `qid` and no later timer started
    /// since) and no poll holds `qid`. Ends that timer either way.
    pub fn due(&self, qid: &str, generation: u64) -> bool {
        let mut g = self.lock();
        if g.armed.get(qid) != Some(&generation) {
            return false;
        }
        g.armed.remove(qid);
        !g.held.contains_key(qid)
    }
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        let last = {
            let mut g = self.waiters.lock();
            let last = g.held.get_mut(&self.qid).is_some_and(|e| {
                e.1 -= 1;
                e.1 == 0
            });
            if last {
                g.held.remove(&self.qid);
                #[cfg(debug_assertions)]
                g.expired.remove(&self.qid);
            }
            last
        };
        self.waiters.changed.notify_waiters();
        if last && let Some(f) = self.on_last.take() {
            f();
        }
    }
}

/// `count()` once it equals `until`, or after 5 s; `count()` at once
/// without `until`. `changed` is notified whenever the count may change
/// (a hold's or a park's start or end).
/// For the debug builds' waiter routes, so tests wait without polling.
#[cfg(debug_assertions)]
pub async fn count_until(
    changed: &Notify,
    count: impl Fn() -> usize,
    until: Option<usize>,
) -> usize {
    let Some(n) = until else {
        return count();
    };
    let reached = async {
        loop {
            let c = changed.notified();
            tokio::pin!(c);
            c.as_mut().enable();
            if count() == n {
                return;
            }
            c.await;
        }
    };
    let _ = clax_rt::time::timeout(std::time::Duration::from_secs(5), reached).await;
    count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_last_hold_to_let_go_runs_its_callback_once() {
        let w = Arc::new(QuestionWaiters::default());
        let ran = Arc::new(AtomicUsize::new(0));
        let cb = |r: &Arc<AtomicUsize>| -> OnLast {
            let r = r.clone();
            Box::new(move || {
                r.fetch_add(1, Ordering::SeqCst);
            })
        };
        let (n1, g1) = w.hold("q", Some(cb(&ran))).unwrap();
        let (n2, g2) = w.hold("q", Some(cb(&ran))).unwrap();
        assert!(Arc::ptr_eq(&n1, &n2), "one notify per question");
        assert_eq!((w.count("q"), w.count("other")), (2, 0));
        drop(g1);
        assert_eq!((w.count("q"), ran.load(Ordering::SeqCst)), (1, 0));
        drop(g2);
        assert_eq!((w.count("q"), ran.load(Ordering::SeqCst)), (0, 1));
        assert!(w.lock().held.is_empty(), "no entry outlives its polls");
    }

    #[test]
    fn at_most_max_polls_hold_a_question() {
        let w = Arc::new(QuestionWaiters::default());
        let guards: Vec<_> = (0..MAX_POLLS).map(|_| w.hold("q", None).unwrap()).collect();
        assert!(w.hold("q", None).is_none());
        assert!(w.hold("other", None).is_some());
        drop(guards);
        assert!(w.hold("q", None).is_some());
    }

    #[test]
    fn only_the_latest_grace_with_no_poll_since_is_due() {
        let w = Arc::new(QuestionWaiters::default());
        let first = w.arm("q");
        let second = w.arm("q");
        assert!(!w.due("q", first), "a later grace replaced it");
        assert!(w.due("q", second));
        assert!(!w.due("q", second), "a timer is due once");
        let third = w.arm("q");
        let hold = w.hold("q", None).unwrap();
        drop(hold);
        assert!(!w.due("q", third), "a poll held it since");
        let fourth = w.arm("q");
        let _hold = w.hold("q", None).unwrap();
        let fifth = w.arm("q");
        assert!(!w.due("q", fourth));
        assert!(!w.due("q", fifth), "a poll holds it now");
    }

    #[test]
    fn a_swept_grace_is_due_at_its_time_unless_a_poll_held_it() {
        let w = Arc::new(QuestionWaiters::default());
        let t0 = chrono::DateTime::from_timestamp(1_000, 0).unwrap();
        let later = t0 + chrono::Duration::seconds(5);
        w.arm_until("q", later);
        w.arm_until("held", later);
        assert_eq!(w.next_due(), Some(later));
        assert!(w.take_due(t0).is_empty(), "not yet");
        drop(w.hold("held", None).unwrap());
        assert_eq!(w.take_due(later), vec!["q".to_string()]);
        assert_eq!(w.next_due(), None, "each grace is due once");
    }

    #[tokio::test]
    async fn wake_reaches_a_poll_that_has_not_started_waiting() {
        let w = Arc::new(QuestionWaiters::default());
        let (n, _g) = w.hold("q", None).unwrap();
        let notified = n.notified();
        w.wake("q");
        w.wake("nobody");
        tokio::time::timeout(std::time::Duration::from_secs(5), notified)
            .await
            .expect("woken");
    }
}
