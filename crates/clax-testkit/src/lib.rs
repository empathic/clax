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
