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
mod fixture;
pub mod people;
mod server;

pub use client::*;
pub use clock::*;
pub use events::*;
pub use fixture::*;
pub use server::*;
