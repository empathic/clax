//! `draft.publish` in a session: the whole draft as the artifact's next
//! version, once in a script.

use super::session::ClaxSession;
use crate::host::{PublishDraft, Published, run_plan};
use crate::surface::{Answer, Call, recorded};
use clax_bot_sandbox::HostError;
use serde_json::Value;

impl ClaxSession<'_> {
    /// The session has published `v`: its draft is based on the
    /// published version, on the first attempt and on a replayed one.
    fn published(&self, v: &Value) -> Result<(), HostError> {
        let p: Published = serde_json::from_value(v.clone())
            .map_err(|e| HostError::new("internal", e.to_string()))?;
        let mut st = self.lock();
        st.draft.base_version = p.version;
        st.published = Some(p);
        st.moved = true;
        Ok(())
    }

    /// Publishes the session's draft over the version it is based on,
    /// addressing the threads the call names, or the run's when it names
    /// none. A second publish in one script is refused. A publish given
    /// its entry is answered from it without asking the host, and a
    /// recorded success moves the draft's base again.
    ///
    /// A success is journaled in the publish's own transaction; a
    /// refusal or a failure is the run loop's to journal.
    pub(super) async fn publish(&self, call: &Call<'_>) -> Answer {
        if let Some(e) = call.replay {
            return Answer::of(recorded(e).and_then(|v| self.published(&v).map(|()| v)));
        }
        let (files, base) = {
            let st = self.lock();
            if st.published.is_some() {
                return Answer::refused(HostError::new(
                    "already_published",
                    "this script already published the draft; publish again from a later script",
                ));
            }
            (st.draft.publish_files(), st.draft.base_version)
        };
        let opts = call.args.first().unwrap_or(&Value::Null);
        let addresses = match opts["addresses"].as_array() {
            Some(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            None => self.run.thread_ids.clone(),
        };
        let wrote = async {
            let plan = self
                .provider
                .host
                .plan_publish(
                    &self.op_ctx(call),
                    PublishDraft {
                        files,
                        if_version: base,
                        note: opts["note"].as_str().map(str::to_string),
                        addresses,
                    },
                )
                .await?;
            let entry = self.cx.claim.entry(call.place, call.op, call.raw)?;
            let got = run_plan(plan, entry).await?;
            self.published(&got)?;
            Ok(got)
        }
        .await;
        match wrote {
            Ok(v) => Answer::written(Ok(v)),
            Err(e) => Answer::refused(e),
        }
    }
}
