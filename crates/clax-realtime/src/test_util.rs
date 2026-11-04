//! What this crate's unit tests share: the hang guard of
//! `clax_testkit`, which they cannot reach (it depends on this crate).

use std::time::Duration;

/// How long a test's wait on something that will happen may take before
/// the test calls it a hang and fails, naming what it waited for. A hang
/// guard only, as `clax_testkit::HANG`: it never decides whether a test
/// passes.
pub const HANG: Duration = Duration::from_secs(120);

/// `fut`'s output, bounded by the [`HANG`] guard.
///
/// # Panics
/// Past the [`HANG`] guard, naming `what`.
#[allow(
    clippy::disallowed_methods,
    reason = "within is the HANG guard on a wait"
)]
pub async fn within<F: std::future::IntoFuture>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(HANG, fut)
        .await
        .unwrap_or_else(|_| panic!("{what}: not after {HANG:?} (hang guard)"))
}
