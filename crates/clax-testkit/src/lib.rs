//! Test support for the integration tests of the crates that call the
//! daemon: an in-process daemon on an ephemeral port ([`TestServer`]), its
//! clients, the fixtures it loads, the canary world of people
//! ([`people`]) and the real clients' harness ([`clients`]).
//!
//! Only integration tests (`tests/`) depend on it, never a crate's own
//! code or unit tests: it sits below every crate it tests. It turns on no
//! feature of any crate, and no crate turns on one of its own for tests
//! (`scripts/bazel-derive.py` refuses that).

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

/// `f`'s value once it has one, checked again every 50 ms. No wall-clock
/// limit: a load that slows the daemons only delays it, and a condition
/// that never holds is ended by the test runner's timeout, whose output
/// names `what`.
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
