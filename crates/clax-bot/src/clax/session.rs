//! One attempt of one script step, for a run's `clax` binding: the
//! run's draft as the committed steps left it, what the step published,
//! the threads it replied on and whether it created an artifact.
//!
//! [`ClaxSession::call`] names the call's target, holds it to the run's
//! rules ([`super::rules`]) and hands it to the part that performs its
//! operation: the draft's ([`super::draft`]), the publish
//! ([`super::publish`]), the host's reads and writes
//! ([`super::replies`]) or the web's ([`super::web`]). Each says, by the
//! constructor of its [`Answer`], who journals the call
//! ([`crate::surface`], "Who journals a call").

use super::provider::ClaxProvider;
use super::rules;
use crate::api;
use crate::draft::Draft;
use crate::host::{OpCtx, Published};
use crate::surface::{Answer, Call, StepCx, StepEnd, StepParts, StepSession};
use clax_bot_sandbox::HostError;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::Run;
use serde_json::json;
use std::sync::Mutex;

/// The part of a session that performs an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// The session's draft ([`super::draft`]).
    Draft,
    /// The publish of the draft ([`super::publish`]).
    Publish,
    /// The host's reads and writes ([`super::replies`]).
    Host,
    /// The web's ([`super::web`]).
    Web,
}

/// The part that performs each operation of `src/clax.d.ts`, by its
/// dotted name: one entry for each, which a test holds.
const PARTS: &[(&str, Part)] = &[
    ("artifact.get", Part::Host),
    ("artifacts.create", Part::Host),
    ("files.list", Part::Draft),
    ("files.read", Part::Draft),
    ("files.write", Part::Draft),
    ("files.replace", Part::Draft),
    ("files.delete", Part::Draft),
    ("draft.check", Part::Draft),
    ("draft.publish", Part::Publish),
    ("draft.rebase", Part::Draft),
    ("draft.theirs", Part::Draft),
    ("db.collections", Part::Host),
    ("db.list", Part::Host),
    ("db.get", Part::Host),
    ("db.query", Part::Host),
    ("db.set", Part::Host),
    ("db.update", Part::Host),
    ("db.delete", Part::Host),
    ("threads.list", Part::Host),
    ("threads.read", Part::Host),
    ("threads.reply", Part::Host),
    ("threads.resolve", Part::Host),
    (crate::web::OP_SEARCH, Part::Web),
    (crate::web::OP_FETCH, Part::Web),
];

/// The part that performs operation `op`.
fn part(op: &str) -> Option<Part> {
    PARTS.iter().find(|(n, _)| *n == op).map(|&(_, part)| part)
}

/// What a session holds of its step.
pub(super) struct State {
    pub draft: Draft,
    /// What the step's publish answered, once it has published.
    pub published: Option<Published>,
    /// The threads the step replied on, in order.
    pub replied: Vec<String>,
    /// The step created an artifact.
    pub created: bool,
    /// The draft's base moved (a publish or a rebase).
    pub moved: bool,
}

/// A session of Clax's own surface ([`ClaxProvider`]).
pub(super) struct ClaxSession<'s> {
    pub provider: &'s ClaxProvider,
    pub cx: StepCx<'s>,
    /// The run's row.
    pub run: Run,
    /// The audit context the host acts under: ClaxBot for the run's
    /// person.
    pub audit: AuditCtx,
    /// Whether an earlier step of the run created an artifact.
    pub created_before: bool,
    pub state: Mutex<State>,
}

impl ClaxSession<'_> {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The call `call` is, for the host: its step, the attempt that
    /// makes it, and its place.
    pub(super) fn op_ctx<'a>(&'a self, call: &Call<'_>) -> OpCtx<'a> {
        OpCtx::new(
            &self.run,
            &self.audit,
            call.place.step,
            call.place.attempt,
            call.place.seq,
        )
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl StepSession for ClaxSession<'_> {
    async fn call(&self, call: Call<'_>) -> Answer {
        let target = api::target(call.op, call.args, &self.run.artifact_id);
        let created = self.created_before || self.lock().created;
        if let Err(e) = rules::refusal(&self.run, created, call.op, call.args) {
            return Answer::refused(e).naming(target);
        }
        let Some(spec) = api::op(call.op) else {
            return Answer::refused(HostError::new(
                api::CODE_UNKNOWN_OP,
                format!("clax.{} is not an operation", call.op),
            ));
        };
        let Some(part) = part(call.op) else {
            return Answer::refused(HostError::new(
                api::CODE_UNKNOWN_OP,
                format!("clax.{} is not an operation", call.op),
            ));
        };
        let answer = match part {
            Part::Draft => self.draft_op(&call).await,
            Part::Publish => self.publish(&call).await,
            Part::Host => self.host(&call, spec.access()).await,
            Part::Web => self.web(&call).await,
        };
        if answer.result().is_ok() {
            self.count(&call);
        }
        answer.naming(target)
    }

    fn finish(self: Box<Self>, end: StepEnd) -> StepParts {
        let st = self.state.into_inner().unwrap_or_else(|e| e.into_inner());
        let base = st.moved.then_some(st.draft.base_version);
        let mut parts = StepParts::default();
        parts
            .payload
            .insert("published".into(), json!(st.published));
        parts.payload.insert("replied".into(), json!(st.replied));
        parts.payload.insert("base".into(), json!(base));
        if let Some(p) = &st.published {
            parts.tool_result.insert("published".into(), json!(p));
        }
        // A cancelled step keeps what its calls wrote outside it, and
        // nothing that waited for its commit.
        if end == StepEnd::Commit {
            parts
                .payload
                .insert("changed".into(), json!(st.draft.changed_paths()));
            parts.payload.insert("created".into(), json!(st.created));
            parts.draft = st.draft.changes();
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each operation of Clax's surface has one part that performs it,
    /// nothing else has one, and the operations that are charged are the
    /// ones the web performs.
    #[test]
    fn every_operation_has_one_part() {
        let names: Vec<&str> = api::definition().operations().map(|o| o.name).collect();
        let performed: Vec<&str> = PARTS.iter().map(|(n, _)| *n).collect();
        assert_eq!(performed, names);
        for name in names {
            let charged = api::op(name).unwrap().access() == api::Access::Spend;
            assert_eq!(charged, part(name) == Some(Part::Web), "{name}");
        }
        for unknown in ["", "nope.op", "files", "files.read.x", "clax.files.read"] {
            assert!(part(unknown).is_none(), "{unknown}");
        }
    }
}
