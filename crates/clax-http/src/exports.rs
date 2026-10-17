//! What bounds the daemon's audit exports (`clax_server::routes::toolpath`).

use std::sync::Arc;
use std::time::Duration;

/// What bounds the daemon's exports: how many run at once (each holds a
/// store worker, a reader connection and its snapshot), how long one waits
/// for its reader to take the next chunk, and how long one may run in all.
pub struct Exports {
    permits: Arc<tokio::sync::Semaphore>,
    /// How long a chunk waits for room before the export gives up on its
    /// reader.
    pub stall: Duration,
    /// How long an export may run before it is ended.
    pub ceiling: Duration,
}

impl Exports {
    /// One export at a time; 60 s for a stalled reader; 15 minutes in all.
    pub fn new() -> Arc<Exports> {
        Arc::new(Exports::with_limits(
            Duration::from_secs(60),
            Duration::from_secs(15 * 60),
        ))
    }

    /// One export at a time, with these stall and time limits.
    pub fn with_limits(stall: Duration, ceiling: Duration) -> Exports {
        Exports {
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            stall,
            ceiling,
        }
    }

    /// The permits, for a test that holds one.
    pub fn permits(&self) -> Arc<tokio::sync::Semaphore> {
        self.permits.clone()
    }
}
