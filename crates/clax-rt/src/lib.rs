//! The async runtime seam of the Clax server and its MCP tools: where they
//! start background tasks, wait on timers and read the clock, so the same
//! code runs on tokio natively and on a Worker's event loop in a Durable
//! Object (wasm32-unknown-unknown).
//!
//! Natively every item is tokio's own, re-exported unchanged: [`spawn`],
//! [`spawn_blocking`], [`JoinHandle`] and the whole of [`time`]
//! (`tokio::time`, so paused test time still applies). On wasm32 they are
//! this crate's, with the same names and the subset of tokio's API the
//! server uses:
//!
//! - [`spawn`] runs the future on the Worker's event loop
//!   (`wasm_bindgen_futures::spawn_local`); dropping its [`JoinHandle`]
//!   detaches it, [`JoinHandle::abort`] stops it at its next await, and
//!   awaiting the handle gives the output or [`JoinError`] (cancelled; a
//!   panic aborts the Worker's instance, so there is no panic error).
//! - [`Handle`] stands for the runtime, for code that spawns from outside
//!   it (a store job): the event loop, always current.
//! - [`spawn_blocking`] runs the closure at once, on the caller's thread
//!   (the only thread), and hands back a finished handle: the store's one
//!   connection (`clax_store::Store::open_on`) does its work inline too.
//! - [`time`]'s sleeps, timeouts and intervals wait on the global
//!   `setTimeout` (cancelled when dropped), and its `Instant` is
//!   `performance.now()` ([`web_time`]). A Worker advances that clock only
//!   across I/O, so a deadline is checked again when its timer fires.
//!
//! A sleep, timeout or interval is `Send` on both, so a handler that awaits
//! one still satisfies axum's bounds on wasm32. On wasm32 [`spawn`] and
//! [`spawn_blocking`] take work that is not `Send` too (the Worker has one
//! thread); natively they need it, as tokio's do.
//!
//! [`wall`] is the clock without a runtime: `std::time` natively and
//! [`web_time`] on wasm32, where `std::time::{Instant, SystemTime}::now`
//! panic.
//!
//! A Worker's timers keep its Durable Object awake while they wait: work
//! that must outlive a request, or wake a sleeping object, belongs on the
//! object's alarm, not on [`time::sleep`].

/// Wall-clock and monotonic time that works without a runtime on every
/// target: `std::time`'s types natively, [`web_time`]'s (the Worker's
/// `Date.now()` and `performance.now()`) on wasm32.
pub mod wall {
    pub use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};
}

#[cfg(not(target_arch = "wasm32"))]
pub use tokio::runtime::{Handle, TryCurrentError};
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::task::{AbortHandle, JoinError, JoinHandle, spawn, spawn_blocking};

/// Timers: tokio's natively, the Worker's `setTimeout` on wasm32.
#[cfg(not(target_arch = "wasm32"))]
pub mod time {
    pub use tokio::time::*;
}

/// Whether [`spawn`] may be called here: natively, inside a tokio runtime;
/// on wasm32, always (the Worker's event loop is always there).
#[must_use]
pub fn in_runtime() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::runtime::Handle::try_current().is_ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
}

/// `future`, as a `Send` future: natively the future itself (which must be
/// `Send`); on wasm32 wrapped so it is, for the Worker's JavaScript futures
/// (a `fetch`), which are not. Sound because a Worker has one thread; the
/// wrapper panics if it is ever polled or dropped on another.
pub fn sendable<F>(future: F) -> impl std::future::Future<Output = F::Output> + Send
where
    F: std::future::Future + SendOnNative,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        future
    }
    #[cfg(target_arch = "wasm32")]
    {
        send_wrapper::SendWrapper::new(future)
    }
}

/// `Send` natively; every type on wasm32 (see [`sendable`]).
#[cfg(not(target_arch = "wasm32"))]
pub trait SendOnNative: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> SendOnNative for T {}
/// `Send` natively; every type on wasm32 (see [`sendable`]).
#[cfg(target_arch = "wasm32")]
pub trait SendOnNative {}
#[cfg(target_arch = "wasm32")]
impl<T> SendOnNative for T {}

#[cfg(target_arch = "wasm32")]
mod worker;
#[cfg(target_arch = "wasm32")]
pub use worker::{
    AbortHandle, Handle, JoinError, JoinHandle, TryCurrentError, spawn, spawn_blocking, time,
};

#[cfg(test)]
mod tests {
    use super::time::{self, Duration, Instant};

    #[tokio::test(start_paused = true)]
    async fn a_timeout_ends_a_future_that_outlasts_it() {
        let slow = time::sleep(Duration::from_secs(10));
        let got = time::timeout(Duration::from_secs(1), slow).await;
        assert!(got.is_err());
        let fast = time::timeout(Duration::from_secs(1), async { 7 }).await;
        assert_eq!(fast.unwrap(), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn a_spawned_task_hands_back_its_output_and_an_aborted_one_is_cancelled() {
        let done = super::spawn(async { 3 });
        assert_eq!(done.await.unwrap(), 3);
        let stuck = super::spawn(std::future::pending::<()>());
        stuck.abort();
        assert!(stuck.await.unwrap_err().is_cancelled());
        let start = Instant::now();
        let mut every = time::interval(Duration::from_secs(2));
        every.tick().await;
        every.tick().await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }

    #[test]
    fn spawn_needs_a_runtime_natively() {
        assert!(!super::in_runtime());
    }
}
