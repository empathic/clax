//! The rules of a `clax` run. A session checks each call against them
//! before anything performs the call.
//!
//! - A run reaches only the threads of its request.
//! - Only a brief's run creates an artifact, and at most one.
//! - A run limited to its own artifact creates no artifact.

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
/// `artifacts.create` in a run that is not a brief's, in a run limited to
/// its own artifact ([`Run::is_limited_to_its_artifact`]), or a second
/// time.
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
        if run.is_limited_to_its_artifact() {
            return Err(HostError::new(
                "forbidden",
                "this run is limited to its own artifact",
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

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_store::bot_runs::RunStatus;
    use clax_store::credits::KeySource;
    use serde_json::json;

    fn run(request: RequestKind, agent_grant: Option<&str>) -> Run {
        Run {
            id: "01JRUN".into(),
            artifact_id: "tides".into(),
            person: "p_pat".into(),
            request,
            thread_ids: vec!["t1".into()],
            status: RunStatus::Running,
            reason: None,
            model: "m".into(),
            key_source: KeySource::Instance,
            base_version: 1,
            credits_milli: 0,
            claimed_by: None,
            claimed_at: None,
            created_at: "2026-11-02T10:00:00Z".into(),
            updated_at: "2026-11-02T10:00:00Z".into(),
            ended_at: None,
            due_at: None,
            agent_grant: agent_grant.map(str::to_string),
            web: String::new(),
        }
    }

    fn create(run: &Run, created: bool) -> Result<(), (String, String)> {
        refusal(run, created, "artifacts.create", &[json!({"title": "X"})])
            .map_err(|e| (e.code, e.message))
    }

    #[test]
    fn only_a_brief_a_person_sent_creates_an_artifact_once() {
        let own = run(RequestKind::Brief, None);
        assert_eq!(create(&own, false), Ok(()));
        assert_eq!(
            create(&own, true).unwrap_err().1,
            "this run already created an artifact"
        );
        assert_eq!(
            create(&run(RequestKind::Thread, None), false)
                .unwrap_err()
                .1,
            "only a new artifact's brief may create artifacts"
        );
        // A brief that an agent's grant asked for is limited to its own artifact.
        let limited = run(RequestKind::Brief, Some("g1"));
        assert_eq!(
            create(&limited, false),
            Err((
                "forbidden".to_string(),
                "this run is limited to its own artifact".to_string()
            ))
        );
        // So is one whose artifact is gone from its row.
        let mut orphan = limited.clone();
        orphan.artifact_id.clear();
        assert_eq!(
            create(&orphan, false).unwrap_err().1,
            "this run is limited to its own artifact"
        );
        // Its other operations are its artifact's, as any run's.
        assert!(refusal(&limited, false, "threads.reply", &[json!("t1"), json!("x")]).is_ok());
        assert!(refusal(&limited, false, "artifact.get", &[]).is_ok());
    }
}
