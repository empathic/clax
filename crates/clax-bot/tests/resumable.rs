//! A workspace whose calls go on without the tick: a call that outlasts the
//! tick leaves its step open and uncommitted, and the next tick joins it
//! under the same key. Time is paused from the moment a call is held, so what
//! runs out is the tick's budget and no test waits for it.

use crate::common::*;
use crate::workspace::{as_reporter, filed, script_value, with};
use clax_bot::host::INTENT_SEQ;
use clax_bot::workspace::{
    CODE_FIX_COMING, CODE_SHELL_INTERRUPTED, CODE_YIELDED, FakeWorkspace, MAX_OUTPUT_BYTES,
    MIN_CALL_SLACK, Offer, PREVIEW_LOST, PROPOSAL_STRANDED, PreviewSink, Proposal,
};
use clax_bot::{Deps, Driver, Next, TickOpts, TickOutcome, Wait, Workspace};
use clax_bot_sandbox::{CancelToken, HostError};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, Run, RunStatus, StepKind};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HOUR: Duration = Duration::from_secs(3600);

/// A workspace that keeps its commands by key, as a service's jobs do: a
/// held call never answers, and what it began goes on.
#[derive(Default)]
struct Jobs {
    inner: FakeWorkspace,
    /// Calls yet to be held.
    hold: AtomicU32,
    /// The clock is paused by a held call.
    paused: AtomicBool,
    /// The key of each command begun.
    begun: Mutex<Vec<String>>,
    /// The key of each join.
    joined: Mutex<Vec<String>>,
    /// The key of each proposal, repeats included.
    proposed: Mutex<Vec<String>>,
    answers: Mutex<BTreeMap<String, Value>>,
    /// It knows no command it is asked to join.
    forgets: bool,
    /// No join answers, as in an outage: a resumable workspace waits it out.
    down: AtomicBool,
    /// What it offers, [`Offer::CHANGE`] when `None`.
    offered: Mutex<Option<Offer>>,
    /// Every command fails with this code.
    fails_with: Option<&'static str>,
    /// What a held call cancels, and how long after it is held.
    cancels: Mutex<Option<(CancelToken, Duration)>>,
    /// What a held call cancels at the moment it is dropped, which is the
    /// moment it yields.
    cancels_with_the_yield: Mutex<Option<CancelToken>>,
    /// Where it says a fix's preview is up.
    sink: Mutex<Option<Arc<dyn PreviewSink>>>,
    /// What each command prints.
    output: Option<String>,
    /// How long each command takes, with time paused.
    takes: Option<Duration>,
}

/// Cancels when dropped.
struct OnDrop(CancelToken);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Jobs {
    fn holding(calls: u32) -> Jobs {
        Jobs {
            hold: AtomicU32::new(calls),
            ..Jobs::default()
        }
    }

    /// Never answers, when this call is one to hold.
    async fn held(&self) {
        let left = self.hold.load(Ordering::SeqCst);
        if left > 0 {
            self.hold.store(left - 1, Ordering::SeqCst);
            self.freeze();
            if let Some((token, after)) = self.cancels.lock().unwrap().take() {
                tokio::spawn(async move {
                    tokio::time::sleep(after).await;
                    token.cancel();
                });
            }
            let _cancel = self
                .cancels_with_the_yield
                .lock()
                .unwrap()
                .take()
                .map(OnDrop);
            std::future::pending::<()>().await;
        }
    }

    fn freeze(&self) {
        if !self.paused.swap(true, Ordering::SeqCst) {
            tokio::time::pause();
        }
    }

    fn sink(&self) -> Arc<dyn PreviewSink> {
        self.sink.lock().unwrap().clone().unwrap()
    }

    /// Lets time run again, for the model's calls on the real network.
    fn thaw(&self) {
        if self.paused.swap(false, Ordering::SeqCst) {
            tokio::time::resume();
        }
    }
}

#[async_trait::async_trait]
impl Workspace for Jobs {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        let said = self.offered.lock().unwrap().clone();
        said.unwrap_or_else(|| self.inner.offer(run, queue))
    }

    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        *self.sink.lock().unwrap() = Some(sink);
    }

    async fn exec(
        &self,
        _run: &Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        panic!("the run loop gives a resumable workspace a key");
    }

    fn resumable(&self) -> bool {
        true
    }

    async fn exec_keyed(
        &self,
        run: &Run,
        queue: Option<&str>,
        key: &str,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.begun.lock().unwrap().push(key.to_string());
        if let Some(code) = self.fails_with {
            return Err(HostError::new(code, "the service said so"));
        }
        let mut v = self
            .inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await?;
        if let Some(o) = &self.output {
            v["output"] = json!(o);
        }
        self.answers
            .lock()
            .unwrap()
            .insert(key.to_string(), v.clone());
        self.held().await;
        if let Some(takes) = self.takes {
            self.freeze();
            tokio::time::advance(takes).await;
        }
        Ok(v)
    }

    async fn resume_exec(&self, _run: &Run, key: &str) -> Result<Option<Value>, HostError> {
        self.joined.lock().unwrap().push(key.to_string());
        if self.down.load(Ordering::SeqCst) {
            self.freeze();
            std::future::pending::<()>().await;
        }
        self.held().await;
        Ok(if self.forgets {
            None
        } else {
            self.answers.lock().unwrap().get(key).cloned()
        })
    }

    async fn propose(&self, run: &Run, proposal: &Proposal) -> Result<Value, HostError> {
        self.proposed.lock().unwrap().push(proposal.key.clone());
        let v = self.inner.propose(run, proposal).await?;
        self.held().await;
        Ok(v)
    }

    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

/// Ticks `run` with an hour of budget, and lets time run again after. A held
/// call that nothing gives up fails here, not by hanging.
async fn tick_out(env: &Env, w: &Jobs, deps: &Deps, run: &str, d: &DriverId) -> TickOutcome {
    let tick = env.try_tick(deps, run, d, Some(HOUR));
    let out = tokio::time::timeout(HOUR * 24, tick)
        .await
        .expect("the tick ended with its budget")
        .unwrap();
    w.thaw();
    out
}

/// A run with `code` in its first script, ticked past its first turn.
async fn at_the_script(env: &Env, deps: &Deps, code: &str) -> (String, DriverId) {
    env.api.push(env.script_turn("tu_1", "Shell", code));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let d = DriverId::mint();
    assert_eq!(env.tick(deps, &run, &d).await.kind, Some(StepKind::Turn));
    (run, d)
}

fn yielded(out: &TickOutcome) {
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");
    assert_eq!(out.kind, Some(StepKind::Script));
}

/// What `run` has been charged, on its row and in usage rows.
fn charged(env: &Env, run: &str) -> (i64, usize) {
    (env.run_row(run).credits_milli, env.usage().len())
}

/// [`tick_out`], which must yield, charge nothing and leave its step open.
async fn tick_yields(env: &Env, w: &Jobs, deps: &Deps, run: &str, d: &DriverId) {
    let before = charged(env, run);
    yielded(&tick_out(env, w, deps, run, d).await);
    assert_eq!(charged(env, run), before, "a yield charges nothing");
    assert_eq!(
        env.store.open_step(run).unwrap().map(|s| s.kind),
        Some(StepKind::Script)
    );
}

fn finished(out: &[TickOutcome]) {
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
}

const MAKE: &str = r#"const a = await clax.shell.exec("make"); return a.output;"#;

#[tokio::test]
async fn a_command_that_outlasts_its_tick_commits_nothing_and_is_joined_in_the_next() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;

    tick_yields(&env, &w, &deps, &run, &d).await;
    let open = env
        .store
        .open_step(&run)
        .unwrap()
        .expect("the step is open");
    assert_eq!(open.kind, StepKind::Script);
    assert!(w.joined.lock().unwrap().is_empty());

    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
    let c = w.inner.calls.lock().unwrap();
    assert_eq!(c.commands, ["make"], "the command ran once");
    assert_eq!(*w.joined.lock().unwrap(), *w.begun.lock().unwrap());
}

#[tokio::test]
async fn a_command_may_outlast_many_ticks() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(3));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    for _ in 0..3 {
        tick_yields(&env, &w, &deps, &run, &d).await;
    }
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(w.begun.lock().unwrap().len(), 1);
    assert_eq!(w.joined.lock().unwrap().len(), 3);
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
}

#[tokio::test]
async fn an_answer_found_by_joining_is_clipped_and_journaled_like_any_other() {
    let env = Env::new().await;
    let w = Arc::new(Jobs {
        output: Some("x".repeat(MAX_OUTPUT_BYTES * 2)),
        ..Jobs::holding(1)
    });
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    finished(&env.drive(&deps, &run, &d).await);
    let v = script_value(&env, &run)["value"].clone();
    assert!(v.as_str().unwrap().contains("bytes cut"));
}

#[tokio::test]
async fn a_script_that_catches_the_yield_gets_nothing_more_and_keeps_nothing() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"let first;
           try { first = (await clax.shell.exec("make")).output; }
           catch (e) {
             let then;
             try { await clax.threads.reply("T1", "gave up"); then = "replied"; }
             catch (f) { then = f.code; }
             await clax.files.write("caught.txt", e.code + " " + then);
             return "kept";
           }
           return first;"#,
    )
    .await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    assert!(env.replies().is_empty(), "{:?}", env.replies());
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
    assert!(env.replies().iter().all(|(_, r)| r != "gave up"));
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let s = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    assert_eq!(s.payload.as_ref().unwrap()["changed"], json!([]));
    assert_ne!(script_value(&env, &run)["value"], json!(CODE_YIELDED));
}

#[tokio::test]
async fn a_tick_with_nothing_left_begins_nothing_and_the_next_begins_it_once() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::default());
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"await clax.threads.reply("T1", "starting");
           return (await clax.shell.exec("make")).output;"#,
    )
    .await;
    let out = env
        .try_tick(&deps, &run, &d, Some(Duration::ZERO))
        .await
        .unwrap();
    yielded(&out);
    assert!(w.begun.lock().unwrap().is_empty());
    assert!(!env.store.run_called_from(&run, INTENT_SEQ).unwrap());
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(w.begun.lock().unwrap().len(), 1);
    assert!(
        w.joined.lock().unwrap().is_empty(),
        "there was none to join"
    );
    let said = env.replies();
    assert_eq!(said.iter().filter(|(_, r)| r == "starting").count(), 1);
}

#[tokio::test]
async fn a_command_the_workspace_no_longer_knows_answers_interrupted() {
    let env = Env::new().await;
    let w = Arc::new(Jobs {
        forgets: true,
        ..Jobs::holding(1)
    });
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"try { return await clax.shell.exec("make"); } catch (e) { return e.code; }"#,
    )
    .await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(
        script_value(&env, &run)["value"],
        json!(CODE_SHELL_INTERRUPTED)
    );
    assert_eq!(w.begun.lock().unwrap().len(), 1, "never run again");
}

#[tokio::test]
async fn a_workspace_out_of_reach_when_joined_yields_until_it_returns() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    w.down.store(true, Ordering::SeqCst);
    tick_yields(&env, &w, &deps, &run, &d).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    w.down.store(false, Ordering::SeqCst);
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
    assert_eq!(w.begun.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_replay_is_not_refused_a_call_the_journal_answered() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"await clax.db.set("c", "x", {a: 1});
           return (await clax.shell.exec("make")).output;"#,
    )
    .await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    // The first call was allowed when it was made; what the target says now
    // refuses it.
    *w.offered.lock().unwrap() = Some(Offer::CHANGE.confidential());
    finished(&env.drive(&deps, &run, &d).await);
    assert!(env.run_row(&run).confidential);
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
}

#[tokio::test]
async fn a_services_failure_that_says_yielded_is_an_answer_and_no_yield() {
    let env = Env::new().await;
    let w = Arc::new(Jobs {
        fails_with: Some(CODE_YIELDED),
        ..Jobs::default()
    });
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"try { return await clax.shell.exec("make"); } catch (e) { return e.code; }"#,
    )
    .await;
    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(script_value(&env, &run)["value"], json!(CODE_YIELDED));
    assert_eq!(w.begun.lock().unwrap().len(), 1);
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let n = steps.iter().find(|s| s.kind == StepKind::Script).unwrap().n;
    let calls = env.store.bot_calls(&run, n).unwrap();
    assert!(calls.iter().any(|c| c.seq() == 0), "journaled as an answer");
}

#[tokio::test]
async fn a_tick_with_too_little_left_begins_nothing() {
    let env = Env::new().await;
    let w = Arc::new(Jobs {
        takes: Some(HOUR - MIN_CALL_SLACK / 2),
        ..Jobs::default()
    });
    let deps = with(&env, w.clone());
    let code = r#"await clax.shell.exec("first"); return await clax.shell.exec("second");"#;
    let (run, d) = at_the_script(&env, &deps, code).await;
    yielded(&tick_out(&env, &w, &deps, &run, &d).await);
    assert_eq!(w.begun.lock().unwrap().len(), 1, "the second was not begun");
    let begun = env.store.run_called_from(&run, INTENT_SEQ + 1).unwrap();
    assert!(!begun);
}

#[tokio::test]
async fn a_tick_of_a_small_budget_still_begins_its_first_call() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::default());
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    let small = MIN_CALL_SLACK / 2;
    let out = env.try_tick(&deps, &run, &d, Some(small)).await.unwrap();
    assert_eq!(out.next, Next::Runnable, "{out:?}");
    assert_eq!(w.begun.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_run_short_of_its_own_time_still_begins_a_call() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::default());
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    env.clock.advance(30 * 60 - 3);
    let out = env.try_tick(&deps, &run, &d, Some(HOUR)).await.unwrap();
    assert_eq!(out.next, Next::Runnable);
    assert_eq!(w.begun.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_proposal_that_outlasts_its_tick_is_joined_under_its_key_and_fixed_once() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    assert!(as_reporter(&env, &qid, &report.id).fixes.is_empty());

    finished(&env.drive(&deps, &run, &d).await);
    let keys = w.proposed.lock().unwrap().clone();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1], "one key for both attempts");
    assert_eq!(w.inner.calls.lock().unwrap().proposals.len(), 1);
    assert_eq!(as_reporter(&env, &qid, &report.id).fixes.len(), 1);
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
}

#[tokio::test]
async fn a_reply_after_a_proposal_that_outlasted_its_tick_stays_with_the_queues_people() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let said = "Fix proposed in https://forge.example/pr/1.";
    let code = format!(
        r#"await clax.change.propose({{title: "t", summary: "Fixed."}});
           return await clax.threads.reply({:?}, {said:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    tick_yields(&env, &w, &deps, &run, &d).await;

    finished(&env.drive(&deps, &run, &d).await);
    assert_eq!(script_value(&env, &run)["value"]["members_only"], true);
    let saw = serde_json::to_string(&as_reporter(&env, &qid, &report.id)).unwrap();
    assert!(!saw.contains(said), "{saw}");
}

#[tokio::test]
async fn a_run_out_of_time_with_a_command_going_stops_and_ends_its_container() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    env.clock.advance(31 * 60);
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    assert_eq!(w.inner.calls.lock().unwrap().finished, [run]);
    assert!(w.joined.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_driver_ticks_a_run_that_yielded_at_once() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = Arc::new(with(&env, w.clone()));
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    let driver = Driver::new(d, deps, env.hub.clone());
    driver.resume(true).unwrap();
    let report = driver.run_until_idle(HOUR).await;
    w.thaw();
    assert_eq!(report.next_wake, Some(Duration::ZERO));
    assert_eq!(report.ticks.len(), 1);
    yielded(&report.ticks[0].1);
    let report = driver.run_until_idle(HOUR).await;
    assert_eq!(report.next_wake, None);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
}

/// A run on a report with its proposal held past its first tick.
async fn proposal_yielded(env: &Env, w: &Arc<Jobs>, deps: &Deps) -> (String, String, DriverId) {
    let (_, report) = filed(env, None);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let d = DriverId::mint();
    env.tick(deps, &run, &d).await;
    tick_yields(env, w, deps, &run, &d).await;
    assert!(env.host.lock().member_notes.is_empty());
    (run, report.id, d)
}

#[tokio::test]
async fn a_run_out_of_time_with_a_proposal_begun_tells_its_members() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, _, d) = proposal_yielded(&env, &w, &deps).await;
    env.clock.advance(31 * 60);
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
}

#[tokio::test]
async fn a_run_cancelled_between_ticks_with_a_proposal_begun_tells_its_members() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, _, d) = proposal_yielded(&env, &w, &deps).await;
    env.store.cancel_run(&AuditCtx::DAEMON, &run).unwrap();
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
}

#[tokio::test]
async fn a_step_that_cannot_commit_with_a_proposal_begun_tells_its_members() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, _, d) = proposal_yielded(&env, &w, &deps).await;
    let n = env.store.open_step(&run).unwrap().unwrap().n;
    for _ in 0..3 {
        env.store.note_commit_failure(&run, n).unwrap();
    }
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Failed));
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
}

/// Ticks `run` with `cancel`, an hour of budget, and lets time run again.
async fn tick_cancelled(
    env: &Env,
    w: &Jobs,
    deps: &Deps,
    run: &str,
    d: &DriverId,
    cancel: CancelToken,
) -> TickOutcome {
    let opts = TickOpts {
        budget: Some(HOUR),
        cancel,
        events: &*env.hub,
    };
    let tick = clax_bot::tick(deps, run, d, opts);
    let out = tokio::time::timeout(HOUR * 24, tick)
        .await
        .unwrap()
        .unwrap();
    w.thaw();
    out
}

#[tokio::test]
async fn a_cancel_while_a_proposal_is_held_stops_the_run_and_tells_its_members() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Arc::new(Jobs::holding(1));
    let cancel = CancelToken::new();
    *w.cancels.lock().unwrap() = Some((cancel.clone(), Duration::from_secs(1)));
    let deps = with(&env, w.clone());
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    let out = tick_cancelled(&env, &w, &deps, &run, &d, cancel).await;
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
    assert_eq!(w.inner.calls.lock().unwrap().finished, [run]);
}

#[tokio::test]
async fn a_cancel_that_lands_on_a_yield_stops_the_run() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let cancel = CancelToken::new();
    *w.cancels_with_the_yield.lock().unwrap() = Some(cancel.clone());
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(
        &env,
        &deps,
        r#"try { await clax.shell.exec("make"); } catch (e) { return e.code; }"#,
    )
    .await;
    let out = tick_cancelled(&env, &w, &deps, &run, &d, cancel).await;
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("cancelled"));
    assert_eq!(w.inner.calls.lock().unwrap().finished, [run]);
}

#[tokio::test]
async fn a_command_left_begun_tells_nobody_of_a_proposal() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    tick_yields(&env, &w, &deps, &run, &d).await;
    env.clock.advance(31 * 60);
    env.drive(&deps, &run, &d).await;
    assert!(env.host.lock().member_notes.is_empty());
}

#[tokio::test]
async fn a_preview_told_before_its_fix_is_refused_until_the_fix_is_there() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, tid, d) = proposal_yielded(&env, &w, &deps).await;
    let url = "https://pr-1.preview.example";
    let sink = w.sink();
    let e = sink.preview_ready(&run, url, true).await.unwrap_err();
    assert_eq!(e.code, CODE_FIX_COMING);
    let e = sink.preview_missed(&run).await.unwrap_err();
    assert_eq!(e.code, CODE_FIX_COMING);
    finished(&env.drive(&deps, &run, &d).await);
    sink.preview_ready(&run, url, true).await.unwrap();
    assert_eq!(env.host.lock().previews.len(), 1, "for thread {tid}");
}

#[tokio::test]
async fn a_preview_of_a_stranded_proposal_is_taken_and_comes_to_nothing() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::holding(1));
    let deps = with(&env, w.clone());
    let (run, _, d) = proposal_yielded(&env, &w, &deps).await;
    env.clock.advance(31 * 60);
    env.drive(&deps, &run, &d).await;
    w.sink().preview_missed(&run).await.unwrap();
}

#[tokio::test]
async fn a_preview_given_up_on_tells_the_members() {
    let env = Env::new().await;
    let w = Arc::new(Jobs::default());
    let deps = with(&env, w.clone());
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    finished(&env.drive(&deps, &run, &d).await);
    w.sink().preview_lost(&run).await.unwrap();
    assert_eq!(env.host.lock().member_notes, [PREVIEW_LOST]);
    w.sink().preview_lost("no-such-run").await.unwrap();
}

/// Takes two hours over each command, and is not resumable.
struct Slow(FakeWorkspace);

#[async_trait::async_trait]
impl Workspace for Slow {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.0.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        tokio::time::pause();
        tokio::time::sleep(HOUR * 2).await;
        tokio::time::resume();
        self.0.exec(run, queue, command, cwd, timeout_secs).await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.0.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.0.finish(run).await;
    }
}

#[tokio::test]
async fn a_workspace_that_is_not_resumable_is_waited_for_past_the_budget() {
    let env = Env::new().await;
    let deps = with(&env, Arc::new(Slow(FakeWorkspace::default())));
    let (run, d) = at_the_script(&env, &deps, MAKE).await;
    let out = env.try_tick(&deps, &run, &d, Some(HOUR)).await.unwrap();
    assert_eq!(out.next, Next::Runnable);
    assert_eq!(out.kind, Some(StepKind::Script));
    assert_eq!(script_value(&env, &run)["value"], json!("ran 1"));
}
