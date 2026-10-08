//! The tests that serve the web UI from a directory of their own
//! ([`set_web_dist`](clax_server::routes::shell::set_web_dist)), in a test
//! binary apart from `tests/integration.rs`: the override is process-wide,
//! and under `cargo test` the tests of one binary are threads of one process.
//! Each test holds [`web_dist_lock`] while it sets the override and serves
//! pages, so these tests never see each other's. A release build serves the
//! web UI it embeds, so the binary is empty there.
#![cfg(debug_assertions)]

mod bridge_dev;
mod common;
mod shell_boot;
mod shell_entries;

/// Held by a test for as long as it relies on the web UI override it set.
async fn web_dist_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}
