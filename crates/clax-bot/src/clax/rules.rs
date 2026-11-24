//! The rules of a `clax` run that a session holds a call to before
//! anything performs it: a run reaches only the threads of its request,
//! and only a brief's run creates an artifact, once.

use clax_bot_sandbox::HostError;
use clax_store::bot_runs::{RequestKind, Run};
use serde_json::Value;

/// What `run`'s rules refuse of a call to `op` on `args` (checked
/// against the surface's file), when they refuse it. `created` says
/// whether the run has created an artifact already, in an earlier step
/// or in this one.
///
/// # Errors
/// `forbidden`: a thread that is not part of the request;
/// `artifacts.create` in a run that is not a brief's, or a second time.
pub(super) fn refusal(run: &Run, created: bool, op: &str, args: &[Value]) -> Result<(), HostError> {
    if matches!(op, "threads.read" | "threads.reply" | "threads.resolve") {
        let id = args.first().and_then(Value::as_str).unwrap_or("");
        if !run.thread_ids.iter().any(|t| t == id) {
            return Err(HostError::new(
                "forbidden",
                format!("thread {id} is not part of this request"),
            ));
        }
    }
    if op == "artifacts.create" {
        if run.request != RequestKind::Brief {
            return Err(HostError::new(
                "forbidden",
                "only a new artifact's brief may create artifacts",
            ));
        }
        if created {
            return Err(HostError::new(
                "forbidden",
                "this run already created an artifact",
            ));
        }
    }
    Ok(())
}
