//! A run's place in the trace of the request that started it.
//!
//! A run is ticked by the driver's own task, so the request's span is not
//! current there: the route that starts a run [`remember`]s its
//! `traceparent`, each tick runs in a [`run_span`] continuing it, and the model
//! call sends [`current`]. The trace is held in memory: a run taken over after a
//! restart starts a trace of its own.
//!
//! This crate also builds for wasm32 and has no OpenTelemetry of its own: the
//! HTTP layer, which does, gives it the two things it needs with [`install`]
//! when its export is on. Without that, nothing is remembered or sent.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// What the exporting layer does for this crate.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Makes a span continue the trace of a `traceparent`.
    pub set_parent: fn(&tracing::Span, &str),
    /// The `traceparent` of the current span, if it is in a trace.
    pub current: fn() -> Option<String>,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();
static PARENTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn parents() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    PARENTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Turns tracing of runs on, once, for the life of the process.
pub fn install(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

/// `run` continues the trace of the code running now, the request starting it.
pub fn remember(run: &str) {
    if let Some(traceparent) = current() {
        parents().insert(run.to_string(), traceparent);
    }
}

/// `run` has ended: its trace is no longer kept.
pub fn forget(run: &str) {
    parents().remove(run);
}

/// The span a tick of `run` runs in, continuing the trace it [`remember`]ed.
pub fn run_span(run: &str) -> tracing::Span {
    let span = tracing::info_span!("bot.run", run = %run, "otel.kind" = "internal");
    if let Some(hooks) = HOOKS.get()
        && let Some(traceparent) = parents().get(run)
    {
        (hooks.set_parent)(&span, traceparent);
    }
    span
}

/// The `traceparent` of the current span, for a call out of the process.
pub fn current() -> Option<String> {
    (HOOKS.get()?.current)()
}
