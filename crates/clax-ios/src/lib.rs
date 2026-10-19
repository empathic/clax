//! The Clax daemon inside an iOS app's process: `clax serve`'s
//! configuration ([`daemon::config`]) run on a thread of its own, behind
//! four C functions with scalar arguments (`include/clax_ios.h`).

mod daemon;
mod log;
mod web;

pub use daemon::{EXITED, IDLE, Paths, STARTING, STOPPING, UP, start_at, state, stop, trim};

/// Starts the daemon on its own thread, asking for `port` on 127.0.0.1
/// (0: any free port), with the home under `$HOME`. Returns at once: 0
/// started, 1 already running (starting, up or stopping), 2 no `$HOME`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_start(port: u16) -> i32 {
    match std::env::var_os("HOME") {
        Some(h) => start_at(Paths::under(std::path::Path::new(&h)), port),
        None => 2,
    }
}

/// Asks a running daemon to stop and returns at once; [`clax_state`]
/// says when it has.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_stop() {
    stop()
}

/// 0 never started, 1 starting, 2 up, 3 stopping, or 100 + the status
/// `clax serve` would have exited with (100 a clean stop, 165 a failed
/// migration, 101 another error, 201 a panic on the daemon's thread).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_state() -> i32 {
    state()
}

/// Shrinks SQLite's caches and checkpoints the WAL; returns when done, or
/// after 2 s, or at once when no daemon is up. Safe from any thread.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_trim_memory() {
    let _ = trim(std::time::Duration::from_secs(2));
}
