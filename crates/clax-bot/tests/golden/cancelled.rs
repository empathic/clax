//! A script that is cancelled, and a script that fails after it wrote:
//! what each leaves (`ending.stopped.cancelled-script.*.json`,
//! `script.error.after-a-write.json`).
//!
//! A cancelled script's step is begun again as the run's `stopped` step.
//! It keeps what the script published and replied and where a publish
//! left the draft's base, and neither the paths it changed nor any draft
//! row. A script is cancelled when the sandbox says so, and also when
//! the cancel is asked for after the script returned and before its step
//! is committed. A script that throws commits its step as any script
//! does: what it wrote to the draft before the error is kept.

use super::names::{self as g, Golden};
use super::pins::Pins;
use super::scripts::script_result;
use crate::common::*;
use clax_bot::events::{Event, EventSink};
use clax_bot::{Next, TickOpts};
use clax_bot_sandbox::{CancelToken, Host, Limits, Sandbox, Script, ScriptOutcome};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use serde_json::{Value, json};
use std::sync::Arc;

/// Cancels a token when a script's call to `op` has been answered.
struct CancelOnCall {
    op: &'static str,
    token: CancelToken,
}

impl EventSink for CancelOnCall {
    fn emit(&self, _run: &str, _step: i64, _attempt: i64, event: Event) {
        if let Event::Call { op, .. } = &event
            && op == self.op
        {
            self.token.cancel();
        }
    }
}

/// A sandbox that cancels a token once the script it ran has returned:
/// a cancel asked for between a script's end and its step's commit.
struct CancelOnReturn {
    sandbox: Arc<dyn Sandbox>,
    token: CancelToken,
}

#[async_trait::async_trait]
impl Sandbox for CancelOnReturn {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let outcome = self.sandbox.run(script, host, limits, cancel).await;
        self.token.cancel();
        outcome
    }
}

/// When a run's script is cancelled.
enum When {
    /// Once its call to this operation has been answered; the script
    /// then loops until the cancel ends it.
    AfterCall(&'static str),
    /// Once it has returned.
    AfterItReturned,
}

/// A thread run whose one script (`code`, step 2) is cancelled `when`.
async fn cancelled(code: &str, when: When) -> Pins {
    let env = Env::new().await;
    env.set(|s| s.script_cpu_ms = 600_000);
    env.api.push(env.script_turn("tu_1", "Script", code));
    let run = env.run();
    let driver = DriverId::mint();
    let token = CancelToken::new();
    let deps = match when {
        When::AfterCall(_) => env.deps(),
        When::AfterItReturned => env.deps_with(Arc::new(CancelOnReturn {
            sandbox: env.sandbox.clone(),
            token: token.clone(),
        })),
    };
    assert_eq!(
        env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    let on_call = CancelOnCall {
        op: match when {
            When::AfterCall(op) => op,
            When::AfterItReturned => "",
        },
        token: token.clone(),
    };
    let opts = TickOpts {
        budget: None,
        cancel: token,
        events: &on_call,
    };
    let out = tokio::time::timeout(PROMPTLY, clax_bot::tick(&deps, &run, &driver, opts))
        .await
        .expect("the cancel ends the script")
        .unwrap();
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    Pins::new(env, run)
}

/// The run's stored draft: each row's path, and whether it deletes.
fn stored_draft(p: &Pins) -> Value {
    p.env
        .store
        .draft_files(&p.run)
        .unwrap()
        .iter()
        .map(|f| json!({"path": f.path, "deleted": f.content.is_none()}))
        .collect()
}

/// What a run whose script was cancelled leaves: how it ended (its
/// `stopped` step whole, the replies, the writes, the roster), the
/// step's attempt, the journal, the drafts given to publish, the stored
/// draft, the usage rows and the audit records.
fn left_cancelled(p: &Pins) -> Value {
    let (run, steps) = p.env.store.run_with_steps(&p.run).unwrap().unwrap();
    assert_eq!(run.reason.as_deref(), Some("cancelled"));
    assert_eq!(
        steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::Turn, StepKind::Stopped],
        "the script's step, begun again as the run's stopped step"
    );
    json!({
        "audit": p.audit(),
        "draft": stored_draft(p),
        "drafts_published": p.drafts_published(),
        "ending": p.ending(),
        "journal": p.journal(),
        "stopped_step_attempt": steps[1].attempt,
        "usage": p.usage(),
    })
}

fn what(golden: Golden) -> String {
    format!(
        "what a cancelled script leaves ({}): its stopped step, the journal, the drafts \
         published, the stored draft, the usage and the audit records",
        golden.file()
    )
}

/// A script that writes, publishes, replies and writes again, cancelled
/// in a loop after a read: the stopped step keeps `published`, `replied`
/// and `base`; the publish and the reply stand; no draft row is kept.
#[tokio::test]
async fn a_script_cancelled_after_it_published_and_replied_is_pinned() {
    let p = cancelled(
        r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.draft.publish({note: "Before the cancel"});
await clax.threads.reply("T1", "Published.");
await clax.files.write("late.css", "p { margin: 0; }");
await clax.artifact.get();
while (true) {}"#,
        When::AfterCall("artifact.get"),
    )
    .await;
    let golden = g::ENDING_STOPPED_CANCELLED_SCRIPT_AFTER_WRITES;
    let left = left_cancelled(&p);
    assert_eq!(left["draft"], json!([]));
    assert_eq!(left["stopped_step_attempt"], 2);
    p.pin(&what(golden), golden, &left);
}

/// A script that only read, cancelled in a loop: nothing published, no
/// reply, and the draft's base where it was.
#[tokio::test]
async fn a_script_cancelled_before_it_wrote_anything_is_pinned() {
    let p = cancelled(
        r#"await clax.files.list();
await clax.artifact.get();
while (true) {}"#,
        When::AfterCall("artifact.get"),
    )
    .await;
    let golden = g::ENDING_STOPPED_CANCELLED_SCRIPT_BEFORE_WRITES;
    p.pin(&what(golden), golden, &left_cancelled(&p));
}

/// A script that wrote, replied and returned a value, with the cancel
/// asked for before its step commits: the step is the run's stopped
/// step all the same, with the script's value in its `outcome`, and the
/// draft write is not kept.
#[tokio::test]
async fn a_script_that_returned_before_the_cancel_is_stopped_all_the_same() {
    let p = cancelled(
        r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.threads.reply("T1", "Written.");
return "done";"#,
        When::AfterItReturned,
    )
    .await;
    let golden = g::ENDING_STOPPED_CANCELLED_SCRIPT_RETURNED;
    let left = left_cancelled(&p);
    assert_eq!(left["draft"], json!([]));
    assert_eq!(
        left["ending"]["last_step"]["payload"]["outcome"]["value"],
        "done"
    );
    p.pin(&what(golden), golden, &left);
}

/// A script that writes to the draft, replies, and then throws: its step
/// commits with the error, the paths it changed and the reply, and the
/// draft keeps the write. A second script reads what the first wrote.
#[tokio::test]
async fn a_script_that_fails_after_a_write_keeps_the_write() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Script",
        r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.threads.reply("T1", "Written.");
throw new Error("after the write");"#,
    ));
    env.api.push(env.script_turn(
        "tu_2",
        "Script",
        r#"return await clax.files.read("extra.css");"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let p = Pins::new(env, run);
    let steps = p.steps(&p.env.api.requests()[0].body);
    let golden = g::SCRIPT_ERROR_AFTER_A_WRITE;
    assert_eq!(
        steps[3]["payload"]["outcome"]["value"],
        "h1 { color: blue; }"
    );
    p.pin(
        &format!(
            "what a script that fails after a write leaves ({}): its step, what the model is \
             told, the next script's step, the journal, the stored draft, the usage and the \
             audit records",
            golden.file()
        ),
        golden,
        &json!({
            "audit": p.audit(),
            "draft": stored_draft(&p),
            "journal": p.journal(),
            "next_script_step": steps[3],
            "result": script_result(&p, 2),
            "step": steps[1],
            "usage": p.usage(),
        }),
    );
}
