//! The draft's operations in a session: `files.*`, `draft.check`,
//! `draft.theirs` and `draft.rebase`, each performed over the session's
//! draft on every attempt of its step, a replayed one included. The
//! journal keeps no value of them but a rebase's, which says where the
//! rebase went.

use super::session::ClaxSession;
use crate::check;
use crate::host::host_error;
use crate::surface::{Answer, Call};
use clax_bot_sandbox::HostError;
use clax_core::ArtifactId;
use serde_json::{Value, json};

impl ClaxSession<'_> {
    /// Performs the draft operation `call` is, for the run loop to
    /// journal: with its value for a rebase, without it for any other.
    pub(super) async fn draft_op(&self, call: &Call<'_>) -> Answer {
        if call.op == "draft.rebase" {
            Answer::of(self.rebase(call).await)
        } else {
            Answer::outcome(self.edit(call))
        }
    }

    /// `files.*`, `draft.check` and `draft.theirs`.
    fn edit(&self, call: &Call<'_>) -> Result<Value, HostError> {
        let s = |i: usize| call.args.get(i).and_then(Value::as_str).unwrap_or("");
        let mut st = self.lock();
        match call.op {
            "draft.check" => {
                Ok(serde_json::to_value(check::check(&st.draft)).expect("serialisable"))
            }
            "files.list" => Ok(st.draft.list()),
            "files.read" => st.draft.read(s(0)).map(Value::String),
            "files.write" => st.draft.write(s(0), s(1)),
            "files.replace" => st.draft.replace(s(0), s(1), s(2)),
            "files.delete" => st.draft.delete(s(0)),
            "draft.theirs" => st.draft.theirs(s(0)).map(|t| json!(t)),
            other => Err(HostError::new(
                "unknown_op",
                format!("{other} is not a draft operation"),
            )),
        }
    }

    /// `draft.rebase`: onto the artifact's current version, or, made
    /// again, onto the version its entry says it went to. An entry of a
    /// rebase that failed says none, and that rebase is performed again
    /// onto the current version.
    async fn rebase(&self, call: &Call<'_>) -> Result<Value, HostError> {
        let store = &self.provider.store;
        let recorded = call.replay.and_then(|e| e.result()["version"].as_u64());
        let version = match recorded {
            Some(v) => u32::try_from(v).unwrap_or(0),
            None => {
                let aid = ArtifactId::parse(&self.run.artifact_id).map_err(|e| host_error(&e))?;
                store
                    .get_artifact(&aid)
                    .map_err(|e| host_error(&e))?
                    .ok_or_else(|| HostError::new("not_found", "the artifact is gone"))?
                    .current_version
            }
        };
        // A copy is rebased and put back whole: the call's future may be
        // dropped at the await, and the session's draft is then as it
        // was.
        let mut draft = self.lock().draft.clone();
        let rebased = draft
            .rebase(store, &self.run, version)
            .await
            .map_err(|e| host_error(&e))?;
        let mut st = self.lock();
        st.draft = draft;
        st.moved = true;
        Ok(serde_json::to_value(rebased).expect("serialisable"))
    }
}
