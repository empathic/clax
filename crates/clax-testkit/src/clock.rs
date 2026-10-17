//! Clocks a test drives by hand: question sleeps and agent-gate timers.

use crate::*;
use std::sync::Arc;
use std::time::Duration;

/// A [`clax_realtime::questions::Sleeper`] whose sleeps end only when the test
/// fires them, by duration ([`ManualSleeper::fire`]).
#[derive(Default)]
pub struct ManualSleeper {
    pending: std::sync::Mutex<Vec<(Duration, tokio::sync::oneshot::Sender<()>)>>,
    changed: tokio::sync::Notify,
}

impl clax_realtime::questions::Sleeper for ManualSleeper {
    fn sleep(&self, d: Duration) -> futures::future::BoxFuture<'static, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().push((d, tx));
        self.changed.notify_waiters();
        Box::pin(async move {
            // A dropped sleeper never fires.
            if rx.await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

impl ManualSleeper {
    /// How many sleeps of `d` are waiting (a sleep whose future was
    /// dropped is not).
    pub fn waiting(&self, d: Duration) -> usize {
        let mut p = self.pending.lock().unwrap();
        p.retain(|(_, tx)| !tx.is_closed());
        p.iter().filter(|(x, _)| *x == d).count()
    }

    /// Returns once `n` sleeps of `d` are waiting; panics after 5 s.
    pub async fn wait_for(&self, d: Duration, n: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.waiting(d) == n {
                return;
            }
            tokio::time::timeout_at(deadline, changed)
                .await
                .unwrap_or_else(|_| {
                    panic!("{n} sleeps of {d:?} within 5 s, have {}", self.waiting(d))
                });
        }
    }

    /// Ends every waiting sleep of `d`; how many.
    pub fn fire(&self, d: Duration) -> usize {
        let mut p = self.pending.lock().unwrap();
        let (due, keep): (Vec<_>, Vec<_>) = p.drain(..).partition(|(x, _)| *x == d);
        *p = keep;
        due.into_iter()
            .filter_map(|(_, tx)| tx.send(()).ok())
            .count()
    }
}

/// A [`clax_server::agent::Timer`] whose time moves only when the test advances
/// it ([`ManualTimer::advance`]).
#[derive(Default)]
pub struct ManualTimer {
    state: std::sync::Mutex<TimerState>,
}

/// The time a [`ManualTimer`] reads, and each pending sleep's deadline.
type TimerState = (Duration, Vec<(Duration, tokio::sync::oneshot::Sender<()>)>);

impl clax_server::agent::Timer for ManualTimer {
    fn now(&self) -> Duration {
        self.state.lock().unwrap().0
    }

    fn sleep_until(&self, at: Duration) -> futures::future::BoxFuture<'static, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut st = self.state.lock().unwrap();
            if at <= st.0 {
                let _ = tx.send(());
            } else {
                st.1.push((at, tx));
            }
        }
        Box::pin(async move {
            if rx.await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

impl ManualTimer {
    /// Moves time on by `d`, ending every sleep due by then; how many.
    pub fn advance(&self, d: Duration) -> usize {
        let mut st = self.state.lock().unwrap();
        st.0 += d;
        let now = st.0;
        let (due, keep): (Vec<_>, Vec<_>) = st.1.drain(..).partition(|(at, _)| *at <= now);
        st.1 = keep;
        due.into_iter()
            .filter_map(|(_, tx)| tx.send(()).ok())
            .count()
    }
}

impl TestServer {
    /// A test daemon whose question polls and graces run on the returned
    /// clock (the grace is 5 s; a poll of `wait=n` sleeps `n` s).
    pub async fn spawn_question_clock() -> (TestServer, Arc<ManualSleeper>) {
        let clock = Arc::new(ManualSleeper::default());
        let c = clock.clone();
        let ts = Self::spawn_with(move |s| s.question_sleeper = c).await;
        (ts, clock)
    }
}
