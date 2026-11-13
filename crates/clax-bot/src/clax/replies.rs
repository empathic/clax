//! The operations a session asks the host for (reads, and the writes
//! that take effect at once: `threads.reply`, `threads.resolve`, `db.*`,
//! `artifacts.create`), and what a session counts of them for its step:
//! the threads replied on and whether an artifact was created.

use super::session::ClaxSession;
use crate::api::{self, Access};
use crate::host::run_plan;
use crate::surface::{Answer, Call, recorded};
use clax_bot_sandbox::HostError;
use serde_json::Value;

impl ClaxSession<'_> {
    /// A read or a write of the host's. One given its entry is answered
    /// from it and the host is not asked. A read is the run loop's to
    /// journal, with its value; a write that succeeded is journaled in
    /// its own transaction; a write the host or its plan refused is the
    /// run loop's to journal.
    pub(super) async fn host(&self, call: &Call<'_>, access: Access) -> Answer {
        if let Some(e) = call.replay {
            return Answer::of(recorded(e));
        }
        let host = &self.provider.host;
        match access {
            Access::Read => Answer::of(host.read(&self.op_ctx(call), call.op, call.args).await),
            Access::Write => {
                let wrote = async {
                    let plan = host
                        .plan_write(&self.op_ctx(call), call.op, call.args)
                        .await?;
                    let entry = self.cx.claim.entry(call.place, call.op, call.raw)?;
                    run_plan(plan, entry).await
                }
                .await;
                match wrote {
                    Ok(v) => Answer::written(Ok(v)),
                    Err(e) => Answer::refused(e),
                }
            }
            // Unreachable: every charged operation is the web's, and none
            // of Clax's own is an effect (a test in `api` holds both).
            Access::Spend | Access::Effect => Answer::refused(HostError::new(
                api::CODE_UNKNOWN_OP,
                format!("clax.{} is not a host operation", call.op),
            )),
        }
    }

    /// Counts a call that succeeded, first made or answered from its
    /// entry: a reply's thread, and an artifact created.
    pub(super) fn count(&self, call: &Call<'_>) {
        let mut st = self.lock();
        match call.op {
            "threads.reply" => {
                let thread = call.args.first().and_then(Value::as_str).unwrap_or("");
                st.replied.push(thread.to_string());
            }
            "artifacts.create" => st.created = true,
            _ => {}
        }
    }
}
