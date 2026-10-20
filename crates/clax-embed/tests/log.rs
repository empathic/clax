//! A core opened through the C functions writes its log to
//! `<home>/logs/core.log`. Alone in its binary: the log is the process's
//! one tracing subscriber, installed by the first open.

use clax_embed::{Post, ffi_close, ffi_open};
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex};

/// The open events posted, in order.
#[derive(Default)]
struct Events {
    posted: Mutex<Vec<Value>>,
    arrived: Condvar,
}

impl Post for Events {
    fn post(&self, _port: i64, bytes: &[u8]) -> bool {
        self.posted
            .lock()
            .unwrap()
            .push(serde_json::from_slice(bytes).unwrap_or(Value::Null));
        self.arrived.notify_all();
        true
    }
}

impl Events {
    fn wait_for(&self, kind: &str) {
        let mut posted = self.posted.lock().unwrap();
        while !posted.iter().any(|v| v["type"] == kind) {
            posted = self.arrived.wait(posted).unwrap();
        }
    }
}

#[test]
fn a_core_opened_through_the_c_functions_logs_to_its_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("embed.json"),
        json!({"shell": "clax://app", "content": "clax://{id}.c"}).to_string(),
    )
    .unwrap();
    let events = Arc::new(Events::default());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), events.clone(), 1);
    assert_ne!(core, 0);
    events.wait_for("ready");
    assert_eq!(ffi_close(core), 0);
    events.wait_for("closed");
    let log = std::fs::read_to_string(home.join("logs").join("core.log")).expect("the core's log");
    assert!(log.contains("sample provider"), "{log}");
}
