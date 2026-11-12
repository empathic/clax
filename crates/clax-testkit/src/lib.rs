//! Test support for the integration tests of the crates that call the
//! daemon: an in-process daemon on an ephemeral port ([`TestServer`]), its
//! clients, the fixtures it loads, the canary world of people
//! ([`people`]) and the real clients' harness ([`clients`]).
//!
//! Only integration tests (`tests/`) depend on it, never a crate's own
//! code or unit tests: it sits below every crate it tests. It turns on no
//! feature of any crate, and no crate turns on one of its own for tests
//! (`scripts/bazel-derive.py` refuses that).
#![warn(clippy::disallowed_methods)]

mod client;
pub mod clients;
mod clock;
mod events;
pub mod fake_messages;
mod fixture;
pub mod mcp;
pub mod opener;
pub mod passkey;
pub mod people;
pub mod remotes;
mod server;
pub mod where_to_ask;

pub use client::*;
pub use clock::*;
pub use events::*;
pub use fixture::*;
pub use server::*;

/// How long a test kit wait on a condition may take before the test calls
/// it a hang and fails, naming what it waited for. A hang guard only: a
/// loaded machine reaches every condition far sooner, so it never decides
/// whether a test passes; it turns a wait that would never end into a
/// message well inside Bazel's test timeout (`timeout = "long"`,
/// `bazel/cargo.bzl`; 300 s in CI).
pub const HANG: std::time::Duration = std::time::Duration::from_secs(120);

/// `fut`'s output, bounded by the [`HANG`] guard: panics naming `what`
/// when `fut` has none by then. The bound on a wait for something that
/// will happen (a frame, a reply, a task's end) in place of a
/// `timeout(Duration::from_…)` of its own: a wait that does not hang
/// passes on any machine, however loaded.
///
/// # Panics
/// Past the [`HANG`] guard.
#[expect(
    clippy::disallowed_methods,
    reason = "within is the HANG guard on a wait"
)]
pub async fn within<F: std::future::IntoFuture>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(HANG, fut)
        .await
        .unwrap_or_else(|_| panic!("{what}: not after {HANG:?} (hang guard)"))
}

/// [`within`] for a thread: the next value on `rx`, bounded by the
/// [`HANG`] guard, in place of a `recv_timeout(Duration::from_…)` of its
/// own.
///
/// # Panics
/// Past the [`HANG`] guard, or when every sender is gone first, naming
/// `what`.
#[expect(
    clippy::disallowed_methods,
    reason = "recv_within is the HANG guard on a wait"
)]
pub fn recv_within<T>(what: &str, rx: &std::sync::mpsc::Receiver<T>) -> T {
    match rx.recv_timeout(HANG) {
        Ok(v) => v,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("{what}: not after {HANG:?} (hang guard)")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{what}: every sender is gone")
        }
    }
}

/// `f`'s value once it has one, checked again every 50 ms. No wall-clock
/// limit: a load that slows the daemons only delays it, and a condition
/// that never holds is ended by the test runner's timeout, whose output
/// names `what`.
#[expect(
    clippy::disallowed_methods,
    reason = "until polls its condition; nothing is decided by how long it takes"
)]
pub async fn until<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for tries in 0u32.. {
        if let Some(v) = f().await {
            return v;
        }
        if tries == 200 {
            eprintln!("still waiting: {what}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    unreachable!("the loop returns")
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn within_gives_the_output() {
        assert_eq!(within("a ready value", async { 7 }).await, 7);
    }

    #[tokio::test(start_paused = true)]
    async fn within_names_what_hung() {
        let hung = tokio::spawn(within("the reply", std::future::pending::<()>()));
        let err = hung.await.unwrap_err().into_panic();
        let msg = err.downcast_ref::<String>().unwrap();
        assert_eq!(msg, "the reply: not after 120s (hang guard)");
    }

    #[test]
    fn recv_within_gives_the_value_or_names_a_gone_sender() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(3).unwrap();
        assert_eq!(recv_within("a value", &rx), 3);
        drop(tx);
        let err = std::panic::catch_unwind(|| recv_within("a second value", &rx)).unwrap_err();
        let msg = err.downcast_ref::<String>().unwrap();
        assert_eq!(msg, "a second value: every sender is gone");
    }
}
