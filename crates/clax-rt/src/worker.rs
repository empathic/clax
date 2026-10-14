//! The wasm32 side of the seam: tasks on the Worker's event loop and timers
//! on its `setTimeout`.

use futures_channel::oneshot;
use futures_util::future::Abortable;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

/// Why awaiting a [`JoinHandle`] gave no output: the task was aborted.
#[derive(Debug)]
pub struct JoinError(());

impl JoinError {
    /// Always `true`: a task on wasm32 ends without output only when aborted.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        true
    }

    /// Always `false`: a panic aborts the Worker's instance.
    #[must_use]
    pub fn is_panic(&self) -> bool {
        false
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("task was cancelled")
    }
}

impl std::error::Error for JoinError {}

/// A spawned task: await it for its output; drop it to detach the task.
pub struct JoinHandle<T> {
    rx: oneshot::Receiver<T>,
    abort: futures_util::future::AbortHandle,
    done: Arc<AtomicBool>,
}

/// Stops a spawned task without owning its [`JoinHandle`].
#[derive(Clone, Debug)]
pub struct AbortHandle {
    abort: futures_util::future::AbortHandle,
    done: Arc<AtomicBool>,
}

impl AbortHandle {
    /// Stops the task at its next await, as [`JoinHandle::abort`] does.
    pub fn abort(&self) {
        self.abort.abort();
    }

    /// Whether the task has finished or was aborted.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire) || self.abort.is_aborted()
    }
}

impl<T> JoinHandle<T> {
    /// Stops the task at its next await; awaiting the handle then gives
    /// [`JoinError`]. A finished task is unaffected.
    pub fn abort(&self) {
        self.abort.abort();
    }

    /// A handle that stops the task, for whoever does not await it.
    #[must_use]
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle {
            abort: self.abort.clone(),
            done: self.done.clone(),
        }
    }

    /// Whether the task has finished or was aborted.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire) || self.abort.is_aborted()
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|r| r.map_err(|_| JoinError(())))
    }
}

/// Runs `future` on the Worker's event loop. It need not be `Send`: the
/// Worker has one thread (natively tokio's `spawn` requires it).
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    let (tx, rx) = oneshot::channel();
    let (abort, registration) = futures_util::future::AbortHandle::new_pair();
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(out) = Abortable::new(future, registration).await {
            let _ = tx.send(out);
        }
        finished.store(true, Ordering::Release);
    });
    JoinHandle { rx, abort, done }
}

/// Runs `f` now, on the one thread there is, and hands back its finished
/// handle.
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + 'static,
    R: 'static,
{
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(f());
    let (abort, _) = futures_util::future::AbortHandle::new_pair();
    JoinHandle {
        rx,
        abort,
        done: Arc::new(AtomicBool::new(true)),
    }
}

/// The runtime, for spawning from code that runs outside a task: on the
/// Worker there is one event loop, and it is always current.
#[derive(Clone, Debug)]
pub struct Handle(());

/// Never made: a Worker always has its event loop.
#[derive(Debug)]
pub struct TryCurrentError(());

impl fmt::Display for TryCurrentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no runtime")
    }
}

impl std::error::Error for TryCurrentError {}

impl Handle {
    /// The event loop.
    #[must_use]
    pub fn current() -> Handle {
        Handle(())
    }

    /// The event loop; never an error.
    ///
    /// # Errors
    /// None on wasm32.
    pub fn try_current() -> Result<Handle, TryCurrentError> {
        Ok(Handle(()))
    }

    /// [`spawn`] on the event loop.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        spawn(future)
    }

    /// [`spawn_blocking`]: runs `f` now.
    pub fn spawn_blocking<F, R>(&self, f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + 'static,
        R: 'static,
    {
        spawn_blocking(f)
    }
}

/// Sleeps, timeouts and intervals on the Worker's `setTimeout`, with
/// tokio's names.
pub mod time {
    use gloo_timers::future::TimeoutFuture;
    use send_wrapper::SendWrapper;
    use std::future::{Future, IntoFuture};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    pub use std::time::Duration;
    pub use web_time::Instant;

    /// What [`timeout`] gives when the deadline passes first.
    pub mod error {
        use std::fmt;

        /// The deadline passed before the future finished.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Elapsed(pub(super) ());

        impl fmt::Display for Elapsed {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("deadline has elapsed")
            }
        }

        impl std::error::Error for Elapsed {}
    }

    /// Far enough ahead to stand for "never" (about 30 years), for
    /// deadlines past what `Instant` can hold.
    const FAR: Duration = Duration::from_secs(86_400 * 365 * 30);

    fn after(d: Duration) -> Instant {
        let now = Instant::now();
        now.checked_add(d).unwrap_or_else(|| now + FAR)
    }

    /// Whole milliseconds until `deadline`, rounded up, capped at `u32`.
    fn millis_until(deadline: Instant, now: Instant) -> u32 {
        let left = deadline.saturating_duration_since(now);
        let ms = left.as_micros().div_ceil(1000);
        u32::try_from(ms).unwrap_or(u32::MAX)
    }

    /// A future that is ready once its deadline has passed.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    pub struct Sleep {
        deadline: Instant,
        timer: Option<SendWrapper<TimeoutFuture>>,
    }

    impl Sleep {
        /// When it is ready.
        #[must_use]
        pub fn deadline(&self) -> Instant {
            self.deadline
        }

        /// Whether its deadline has passed.
        #[must_use]
        pub fn is_elapsed(&self) -> bool {
            Instant::now() >= self.deadline
        }

        /// Moves the deadline to `deadline`.
        pub fn reset(mut self: Pin<&mut Self>, deadline: Instant) {
            self.deadline = deadline;
            self.timer = None;
        }
    }

    impl Future for Sleep {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            loop {
                let now = Instant::now();
                if now >= self.deadline {
                    self.timer = None;
                    return Poll::Ready(());
                }
                let deadline = self.deadline;
                let timer = self.timer.get_or_insert_with(|| {
                    SendWrapper::new(TimeoutFuture::new(millis_until(deadline, now)))
                });
                match Pin::new(timer).poll(cx) {
                    // Fired: check the clock again, and wait more if it lags.
                    Poll::Ready(()) => self.timer = None,
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
    }

    /// Ready after `d`.
    pub fn sleep(d: Duration) -> Sleep {
        sleep_until(after(d))
    }

    /// Ready at `deadline`.
    pub fn sleep_until(deadline: Instant) -> Sleep {
        Sleep {
            deadline,
            timer: None,
        }
    }

    /// `future`, ended with [`error::Elapsed`] once its deadline passes.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    pub struct Timeout<F> {
        future: Pin<Box<F>>,
        sleep: Sleep,
    }

    impl<F: Future> Future for Timeout<F> {
        type Output = Result<F::Output, error::Elapsed>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if let Poll::Ready(v) = self.future.as_mut().poll(cx) {
                return Poll::Ready(Ok(v));
            }
            Pin::new(&mut self.sleep)
                .poll(cx)
                .map(|()| Err(error::Elapsed(())))
        }
    }

    /// `future`, or [`error::Elapsed`] if it takes longer than `d`.
    pub fn timeout<F: IntoFuture>(d: Duration, future: F) -> Timeout<F::IntoFuture> {
        timeout_at(after(d), future)
    }

    /// `future`, or [`error::Elapsed`] if it is not done by `deadline`.
    pub fn timeout_at<F: IntoFuture>(deadline: Instant, future: F) -> Timeout<F::IntoFuture> {
        Timeout {
            future: Box::pin(future.into_future()),
            sleep: sleep_until(deadline),
        }
    }

    /// What an [`Interval`] does after a tick that came late.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum MissedTickBehavior {
        /// The missed ticks come at once, then the schedule resumes.
        #[default]
        Burst,
        /// The next tick is a period after the late one.
        Delay,
        /// The missed ticks are skipped; the schedule keeps its phase.
        Skip,
    }

    /// Ticks every period, the first at its start.
    #[derive(Debug)]
    pub struct Interval {
        next: Instant,
        period: Duration,
        behavior: MissedTickBehavior,
        sleep: Sleep,
    }

    /// An [`Interval`] whose first tick is now.
    ///
    /// # Panics
    /// When `period` is zero, as tokio's does.
    pub fn interval(period: Duration) -> Interval {
        interval_at(Instant::now(), period)
    }

    /// An [`Interval`] whose first tick is at `start`.
    ///
    /// # Panics
    /// When `period` is zero, as tokio's does.
    pub fn interval_at(start: Instant, period: Duration) -> Interval {
        assert!(period > Duration::ZERO, "`period` must be non-zero.");
        Interval {
            next: start,
            period,
            behavior: MissedTickBehavior::default(),
            sleep: sleep_until(start),
        }
    }

    /// How late a tick may come and still count as on time, as tokio has it.
    const LATE: Duration = Duration::from_millis(5);

    impl Interval {
        /// Waits for the next tick and returns when it was due. Dropping the
        /// future before it is ready loses no tick.
        pub async fn tick(&mut self) -> Instant {
            std::future::poll_fn(|cx| self.poll_tick(cx)).await
        }

        /// Polls for the next tick.
        pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Instant> {
            if self.sleep.deadline != self.next {
                Pin::new(&mut self.sleep).reset(self.next);
            }
            if Pin::new(&mut self.sleep).poll(cx).is_pending() {
                return Poll::Pending;
            }
            let due = self.next;
            let now = Instant::now();
            self.next = if now > due + LATE {
                match self.behavior {
                    MissedTickBehavior::Burst => due + self.period,
                    MissedTickBehavior::Delay => now + self.period,
                    MissedTickBehavior::Skip => {
                        let mut n = due + self.period;
                        while n <= now {
                            n += self.period;
                        }
                        n
                    }
                }
            } else {
                due + self.period
            };
            Poll::Ready(due)
        }

        /// Sets what happens after a late tick.
        pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
            self.behavior = behavior;
        }

        /// What happens after a late tick.
        #[must_use]
        pub fn missed_tick_behavior(&self) -> MissedTickBehavior {
            self.behavior
        }

        /// Restarts the schedule: the next tick is a period from now.
        pub fn reset(&mut self) {
            self.next = after(self.period);
        }

        /// The time between ticks.
        #[must_use]
        pub fn period(&self) -> Duration {
            self.period
        }
    }
}
