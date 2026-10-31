//! What a daemon holds in memory for connection requests'
//! (`clax_server::routes::remote_requests`) Connects: the requests whose
//! device sign-in a task of this daemon polls, at most one task each, and
//! the Connect lock of each origin, so two Connects of one origin at once
//! start one sign-in, and two polls of one sign-in never spend its code
//! twice.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// The requests driven by this daemon, and the Connect locks.
#[derive(Default)]
pub struct RemoteDrivers {
    driven: Mutex<HashSet<String>>,
    /// Each origin's Connect lock ([`RemoteDrivers::connect_lock`]).
    connect: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// One driver's claim on its request: released when dropped.
pub struct Driving {
    drivers: Arc<RemoteDrivers>,
    id: String,
}

impl Drop for Driving {
    fn drop(&mut self) {
        self.drivers
            .driven
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

impl RemoteDrivers {
    /// The Connect lock of `origin`: held while a Connect of it reads its
    /// request and starts its sign-in, and while one of its sign-ins is
    /// polled. Each origin has its own, so a remote slow to answer (within
    /// its requests' deadline) holds up the Connects and polls of that
    /// origin alone.
    pub fn connect_lock(&self, origin: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.connect
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(origin.to_string())
            .or_default()
            .clone()
    }

    /// Claims request `id` for a new driver; `None` when one drives it.
    pub fn claim(self: &Arc<Self>, id: &str) -> Option<Driving> {
        let fresh = self
            .driven
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string());
        fresh.then(|| Driving {
            drivers: self.clone(),
            id: id.to_string(),
        })
    }

    /// Whether a driver of this daemon drives request `id`.
    pub fn drives(&self, id: &str) -> bool {
        self.driven
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    /// How many requests are driven.
    pub fn count(&self) -> usize {
        self.driven.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Forgets request `id`'s driver, as a restart would, though its task
    /// runs on (tests of a stranded Connect).
    pub fn forget(&self, id: &str) {
        self.driven
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
}
