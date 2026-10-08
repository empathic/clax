//! Agent questions in the daemon (spec 2026-10-06-agent-questions-and-inbox):
//! which polls wait on each question, the grace after which a mirrored
//! (hook) question nobody waits on is withdrawn, the owner view, and
//! announcing changes.

use crate::state::AppState;
use clax_core::store::questions::{Close, QuestionRow, Source, Status};
use clax_core::{ArtifactId, CoreError, Event, Store};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// The timers of question polls and graces: real time in the daemon, a
/// clock the test drives in tests.
pub trait Sleeper: Send + Sync {
    /// Completes after `d`.
    fn sleep(&self, d: std::time::Duration) -> futures::future::BoxFuture<'static, ()>;
}

/// [`Sleeper`] on Tokio's clock.
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, d: std::time::Duration) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(d))
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
    next: u64,
    /// Debug builds: how many of each question's polls are still to end at
    /// once, as if their timers had fired ([`QuestionWaiters::expire`]).
    #[cfg(debug_assertions)]
    expired: HashMap<String, usize>,
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
/// without `until`. `changed` is notified whenever the count may change.
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
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), reached).await;
    count()
}

/// The owner view of `q` (spec §5.4): never a session ID, PID or working
/// directory; `project` is the last component of the session's directory;
/// `artifact` is null for none or a deleted one.
pub fn view(st: &Store, q: &QuestionRow) -> clax_core::Result<Value> {
    let agent = st.get_session(&q.session_id)?.map(|s| {
        json!({
            "handle": s.agent_handle,
            "harness": s.harness,
            "project": std::path::Path::new(&s.cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(""),
        })
    });
    let artifact = match q.artifact_id.as_deref().map(ArtifactId::parse) {
        Some(Ok(id)) => st
            .get_artifact(&id)?
            .map(|a| json!({"id": a.id, "title": a.title, "kind": a.kind})),
        _ => None,
    };
    Ok(json!({
        "id": q.id,
        "agent": agent,
        "artifact": artifact,
        "source": q.source,
        "status": q.status.as_str(),
        "questions": q.questions,
        "answers": q.answers,
        "answered_via": q.answered_via,
        "created_at": q.created_at,
        "closed_at": q.closed_at,
    }))
}

/// Publishes `view` (the view of `q`) as a `question` event and wakes the
/// polls of `q`; an answered or declined `ask` question also wakes its
/// session's feedback polls, which carry late answers (spec §6.4).
pub fn announce(s: &AppState, q: &QuestionRow, view: Value) {
    s.events.publish(Event::Question { question: view });
    s.questions.wake(&q.id);
    if q.source == Source::Ask && matches!(q.status, Status::Answered | Status::Declined) {
        s.feedback_waiters.wake(std::iter::once(&q.session_id));
    }
}

/// Announces each of `ids` as it now is (the questions an ended session
/// withdrew). Runs on a store thread.
pub fn announce_ids(s: &AppState, st: &Store, ids: &[String]) {
    for id in ids {
        let r = st
            .question(id)
            .and_then(|q| q.map(|q| view(st, &q).map(|v| (q, v))).transpose());
        match r {
            Ok(Some((q, v))) => announce(s, &q, v),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not announce a withdrawn question"),
        }
    }
}

/// Starts the grace for hook question `qid`: after `s.question_grace`, if
/// no poll has held it and no later grace has started meanwhile, an open
/// `qid` is withdrawn and announced, recording `question.withdraw` (reason
/// `unwaited`) as `system:daemon`. The grace thus runs from the last time a
/// poll let go (or from creation, when none has held it).
pub fn start_grace(s: AppState, qid: String) {
    let generation = s.questions.arm(&qid);
    tokio::spawn(async move {
        s.question_sleeper.sleep(s.question_grace).await;
        if !s.questions.due(&qid, generation) {
            return;
        }
        let st = s.clone();
        let r = s
            .store
            .call(move |db| {
                let q =
                    db.close_question(&clax_core::audit::AuditCtx::DAEMON, &qid, Close::Unwaited)?;
                let v = view(db, &q)?;
                announce(&st, &q, v);
                Ok(())
            })
            .await;
        match r {
            Ok(())
            | Err(CoreError::Invalid {
                code: "question_closed",
                ..
            }) => {}
            Err(e) => tracing::warn!(error = %e, "could not withdraw an unwaited question"),
        }
    });
}

/// [`start_grace`] for `qid`, to run when its last poll lets go.
pub fn arm_grace(s: AppState, qid: String) -> OnLast {
    Box::new(move || start_grace(s, qid))
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
