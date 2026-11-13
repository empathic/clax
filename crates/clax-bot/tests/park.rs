//! A run that waits: a script's call that its surface holds unanswered
//! parks the run (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Calls that wait"), and its answer, its deadline or a cancel takes
//! it on. The runs are of the contract fixture's kind, bound to a
//! registration a scripted host serves through the hub, with the hub's
//! calls kept in the store the runs are in (`tests/outside.rs` has the
//! host and the hub; `tests/generic.rs` parks a run of the fake alone,
//! with no broker).
//!
//! Time is the test's: [`Desk::pass`] moves the home's clock and the
//! hub's together, and nothing sleeps.

use crate::common::*;
use crate::outside::{Desk, Kept, TIDYS, Times, desk_dts, lamps};
use crate::resume::crash_once;
use async_trait::async_trait;
use clax_bot::policy::RunLimits;
use clax_bot::run::{CrashPoint, Deps, RunSettings};
use clax_bot::surface::RunInfo;
use clax_bot::{Delivery, Driver, Next, RunPolicy, Wait};
use clax_bot_sandbox::{CancelToken, ErrorKind, Host, Limits, Sandbox, Script, ScriptOutcome};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::credits::LINE_SCRIPT_COMPUTE;
use clax_surface::wire;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DAY: u64 = 24 * 60 * 60;

async fn desk() -> Desk {
    Desk::new(Kept::InTheStore).await
}

impl Desk {
    /// Time passes, for the home and the hub alike.
    fn pass(&self, secs: u64) {
        self.world.pass(&self.env, secs);
    }

    /// The ID of the last question the host was asked.
    fn asked(&self) -> String {
        let st = self.host.state.lock().unwrap();
        let ask = st.calls.iter().rev().find(|c| c.op == "ask");
        ask.expect("the host was asked").id.clone()
    }

    /// The host, back after however long, answers its last question.
    fn answer(&self, answer: &str) {
        let n = 100 + self.host.state.lock().unwrap().calls.len() as u64;
        self.host.back(&desk_dts(), n);
        self.host
            .answer(&self.asked(), wire::Outcome::Result(json!(answer)));
    }

    /// Ticks the run until it ends or waits.
    async fn drive(&self, deps: &Deps, run: &str) -> Next {
        let out = self.env.drive(deps, run, &DriverId::mint()).await;
        out.last().unwrap().next
    }

    /// The operations the host was sent, in order.
    fn sent(&self) -> Vec<String> {
        let st = self.host.state.lock().unwrap();
        st.calls.iter().map(|c| c.op.clone()).collect()
    }

    fn steps(&self, run: &str) -> Vec<clax_store::bot_runs::Step> {
        self.env.store.run_with_steps(run).unwrap().unwrap().1
    }
}

/// A script that reads, asks, and acts on the answer.
const MOVE: &str = r#"const lamps = await clax.desk.lamps();
const answer = await clax.desk.ask("Move the left lamp?");
await clax.desk.notify("The person said " + answer);
return [lamps.length, answer];"#;

/// The whole of it: a run reads from its host, asks its person and
/// parks. Parked, it has no driver, no committed step and no charge. A
/// day later the host answers through a `serve`; the answer is
/// journaled with no claim held and the run is due. The step run again
/// is answered from the journal, sends its host nothing it was sent
/// before, goes on, and commits as an ordinary script step; the run
/// finishes, and its host is told.
#[tokio::test]
async fn a_run_parks_on_a_call_that_waits_and_goes_on_when_it_is_answered_a_day_later() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("The lamp is moved."));
    let run = d.run();
    let (deps, driver) = (d.deps(), DriverId::mint());
    assert_eq!(
        d.env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    let spent = d.env.run_row(&run).credits_milli;
    let charged = d.env.usage().len();

    let out = d.env.tick(&deps, &run, &driver).await;
    assert_eq!(out.next, Next::Waiting(Wait::Parked));
    assert_eq!((out.step, out.kind), (Some(2), Some(StepKind::Script)));
    assert_eq!(out.credits_milli, 0);
    let row = d.env.run_row(&run);
    assert_eq!(row.status, RunStatus::Parked);
    assert_eq!((&row.claimed_by, &row.claimed_at), (&None, &None));
    // Nothing is committed and nothing charged.
    assert_eq!((row.credits_milli, d.env.usage().len()), (spent, charged));
    let steps = d.steps(&run);
    assert_eq!(steps.len(), 2);
    assert!(steps[0].committed_at.is_some() && steps[1].committed_at.is_none());
    assert_eq!((steps[1].kind, steps[1].attempt), (StepKind::Script, 1));
    // The journal holds the read that was answered, and no more.
    assert_eq!(
        d.journal(&run, 2),
        [json!({"seq": 0, "op": "desk.lamps", "ok": true, "result": lamps()})]
    );
    assert_eq!(d.sent(), ["lamps", "ask"]);
    let parking = d.env.store.run_parking(&run).unwrap().unwrap();
    assert_eq!(parking.parks, 1);
    assert!(parking.parked_at.is_some());
    let on = parking.waiting_on.unwrap();
    assert_eq!(
        (&on["surface"], &on["op"], &on["key"]),
        (
            &json!("desk"),
            &json!("desk.ask"),
            &json!(format!("{run}:2:1"))
        )
    );
    assert_eq!(
        on["deadline"],
        json!(row.due_at),
        "the run is due at the call's deadline"
    );
    // A tick meanwhile reads the run and does nothing.
    let out = d.env.tick(&deps, &run, &driver).await;
    assert_eq!((out.next, out.step), (Next::Waiting(Wait::Parked), None));
    assert_eq!(d.env.run_row(&run).claimed_by, None);

    // A day passes, less two minutes. The host answers.
    d.pass(DAY - 120);
    d.answer("yes");
    let row = d.env.run_row(&run);
    assert_eq!(
        (row.status, &row.claimed_by, &row.due_at),
        (RunStatus::Queued, &None, &None)
    );
    assert_eq!(
        d.journal(&run, 2)[1],
        json!({"seq": 1, "op": "desk.ask", "ok": true, "result": "yes"})
    );
    let parking = d.env.store.run_parking(&run).unwrap().unwrap();
    assert_eq!((parking.parked_at, parking.waiting_on), (None, None));
    assert!(parking.parked_ms >= i64::try_from((DAY - 120) * 1000).unwrap());

    // The step is run again: answered from the journal, then on.
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    let steps = d.steps(&run);
    assert_eq!(
        steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    assert_eq!(steps[1].attempt, 2);
    let payload = steps[1].payload.as_ref().unwrap();
    assert_eq!(payload["outcome"]["value"], json!([2, "yes"]), "{payload}");
    assert_eq!(payload["outcome"]["error"], Value::Null);
    let made: Vec<(&str, bool)> = payload["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["op"].as_str().unwrap(), c["ok"] == true))
        .collect();
    assert_eq!(
        made,
        [
            ("desk.lamps", true),
            ("desk.ask", true),
            ("desk.notify", true)
        ]
    );
    // The host was sent each call once: neither the read nor the
    // question went out again.
    assert_eq!(d.sent(), ["lamps", "ask", "notify"]);
    for seq in 0..3 {
        assert_eq!(d.host.applied(&format!("{run}:2:{seq}")), 1);
    }
    // The script's compute was charged once, by the attempt that
    // committed.
    let compute = d
        .env
        .usage()
        .iter()
        .filter(|u| u.line == LINE_SCRIPT_COMPUTE && u.run_id.as_deref() == Some(run.as_str()))
        .count();
    assert_eq!(compute, 1);
    // And the host is told how the run ended.
    d.host.round();
    let st = d.host.state.lock().unwrap();
    let [ended] = &st.ended[..] else {
        panic!("{:?}", st.ended);
    };
    assert_eq!(
        (ended.run.as_str(), &ended.end),
        (run.as_str(), &wire::End::Finished {})
    );
    assert_eq!(ended.result, Some(json!({"summary": "The lamp is moved."})));
}

/// A sandbox that lets something happen between a script's parking and
/// its step's hearing of it.
struct Then {
    inner: Arc<dyn Sandbox>,
    after: Box<dyn Fn() + Send + Sync>,
    done: AtomicBool,
}

#[async_trait]
impl Sandbox for Then {
    async fn run(&self, s: &Script, h: &dyn Host, l: &Limits, c: &CancelToken) -> ScriptOutcome {
        let out = self.inner.run(s, h, l, c).await;
        let parked = out
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Parked);
        if parked && !self.done.swap(true, Ordering::SeqCst) {
            (self.after)();
        }
        out
    }
}

/// An answer that arrives after the provider gave up waiting and before
/// the run parks is not lost: the park finds the place journaled, the
/// run is not parked, and its next tick runs the step again with the
/// answer.
#[tokio::test]
async fn an_answer_that_beats_the_park_keeps_the_run_running() {
    let d = Arc::new(desk().await);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let host = d.clone();
    let sandbox = Arc::new(Then {
        inner: d.env.sandbox.clone(),
        after: Box::new(move || host.answer("yes")),
        done: AtomicBool::new(false),
    });
    let mut deps = d.deps();
    deps.sandbox = sandbox;
    let driver = DriverId::mint();
    d.env.tick(&deps, &run, &driver).await;
    let out = d.env.tick(&deps, &run, &driver).await;
    assert_eq!((out.next, out.step), (Next::Runnable, Some(2)));
    let row = d.env.run_row(&run);
    assert_eq!(row.status, RunStatus::Running);
    assert_eq!(row.claimed_by.as_deref(), Some(driver.as_str()));
    assert_eq!(d.env.store.run_parking(&run).unwrap().unwrap().parks, 0);
    let out = d.env.drive(&deps, &run, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!([2, "yes"]));
    assert_eq!(d.sent(), ["lamps", "ask", "notify"]);
    assert_eq!(d.steps(&run)[1].attempt, 2);
}

/// A script that asks twice and says what it was answered or thrown.
const TWICE: &str = r#"const out = [];
for (const q of ["First?", "Second?"]) {
  try { out.push(await clax.desk.ask(q)); } catch (e) { out.push(e.code); }
}
return out;"#;

/// One step may park more than once: each question parks the run, and
/// each attempt replays what came before.
#[tokio::test]
async fn a_step_that_asks_twice_parks_twice() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Twice", TWICE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.answer("one");
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    let parking = d.env.store.run_parking(&run).unwrap().unwrap();
    assert_eq!(parking.parks, 2);
    assert_eq!(
        parking.waiting_on.unwrap()["key"],
        json!(format!("{run}:2:1"))
    );
    d.answer("two");
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.value(&run, 1), json!(["one", "two"]));
    assert_eq!(d.steps(&run)[1].attempt, 3);
    assert_eq!(d.sent(), ["ask", "ask"]);
}

/// What bounds a run's parks.
struct Bounds {
    parks: Option<u32>,
    runs: Option<u32>,
}

impl RunPolicy for Bounds {
    fn limits(&self, _run: &RunInfo, settings: &RunSettings) -> clax_core::Result<RunLimits> {
        let mut limits = RunLimits::of(settings);
        limits.max_parks_per_step = self.parks;
        limits.max_parked_runs = self.runs;
        Ok(limits)
    }
}

/// A step that has parked as often as it may does not park again: the
/// call that would wait is refused to its script, journaled, and the
/// step goes on. So is the call of a run whose person has as many runs
/// parked as they may.
#[tokio::test]
async fn a_step_at_its_parks_bound_and_a_person_at_theirs_are_refused_the_wait() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Twice", TWICE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let bounded = |parks, runs| d.deps_under(Arc::new(Bounds { parks, runs }));
    let deps = bounded(Some(1), None);
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    d.answer("one");
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!(["one", "park_limit"]));
    let journal = d.journal(&run, 2);
    assert_eq!(journal[1]["result"]["code"], "park_limit", "{journal:?}");
    // Refused, the call no longer waits on its host.
    assert_eq!(d.host.waiting().0, 0);
    // With no bound a step parks as often as it asks.
    assert_eq!(
        RunLimits::unbounded(&RunSettings::default()).max_parks_per_step,
        None
    );

    // One run of the person's is parked, which is all they may have.
    let ask = "try { return await clax.desk.ask(\"Well?\"); } catch (e) { return e.code; }";
    d.env.api.push(script_turn("tu_1", "Ask", ask));
    let first = d.run();
    let deps = bounded(None, Some(1));
    assert_eq!(d.drive(&deps, &first).await, Next::Waiting(Wait::Parked));
    d.env.api.push(script_turn("tu_1", "Ask", ask));
    d.env.api.push(finish_turn("done"));
    let second = d.run();
    assert_eq!(
        d.drive(&deps, &second).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.value(&second, 1), json!("parked_runs_limit"));
    assert_eq!(d.env.run_row(&first).status, RunStatus::Parked);
}

/// Calls made together: the call after the one that waits never reaches
/// its host before the park, and the call before it is answered and
/// journaled; run again, each is made once.
#[tokio::test]
async fn calls_made_together_park_at_the_one_that_waits() {
    for (code, before, value) in [
        (
            "const [a, l] = await Promise.all([clax.desk.ask(\"Well?\"), clax.desk.lamps()]);\n\
             return [a, l.length];",
            vec!["ask"],
            json!(["yes", 2]),
        ),
        (
            "const [l, a] = await Promise.all([clax.desk.lamps(), clax.desk.ask(\"Well?\")]);\n\
             return [a, l.length];",
            vec!["lamps", "ask"],
            json!(["yes", 2]),
        ),
    ] {
        let d = desk().await;
        d.env.api.push(script_turn("tu_1", "Together", code));
        d.env.api.push(finish_turn("done"));
        let run = d.run();
        assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
        assert_eq!(d.sent(), before, "{code}");
        assert_eq!(d.journal(&run, 2).len(), before.len() - 1);
        d.answer("yes");
        assert_eq!(
            d.drive(&d.deps(), &run).await,
            Next::Done(RunStatus::Finished)
        );
        assert_eq!(d.value(&run, 1), value, "{code}");
        let mut sent = d.sent();
        sent.sort();
        assert_eq!(sent, ["ask", "lamps"]);
        for seq in 0..2 {
            assert_eq!(d.host.applied(&format!("{run}:2:{seq}")), 1);
        }
    }
}

/// The driver dies while it runs the answered step again: the next
/// driver replays it once more, and the host is sent nothing twice.
#[tokio::test]
async fn a_crash_while_resuming_is_replayed_again() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.answer("yes");
    let mut dying = d.deps();
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    assert_eq!(d.drive(&dying, &run).await, Next::Waiting(Wait::Crashed));
    d.pass(61);
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.steps(&run)[1].attempt, 3);
    assert_eq!(d.value(&run, 1), json!([2, "yes"]));
    assert_eq!(d.sent(), ["lamps", "ask", "notify"]);
    for seq in 0..3 {
        assert_eq!(d.host.applied(&format!("{run}:2:{seq}")), 1);
    }
}

/// A sandbox that can never take a script.
struct Busy;

#[async_trait]
impl Sandbox for Busy {
    async fn run(&self, _: &Script, _: &dyn Host, _: &Limits, _: &CancelToken) -> ScriptOutcome {
        ScriptOutcome::failed(ErrorKind::Unavailable, "busy")
    }
}

/// The host is told `resumed` once for each time the parked step was
/// unparked, however many ticks run it again: here the first tick after
/// the answer finds the sandbox busy, the next crashes before its
/// commit, and the third finishes the run.
#[tokio::test]
async fn a_step_run_again_over_several_ticks_is_said_resumed_once() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.answer("yes");
    let mut busy = d.deps();
    busy.sandbox = Arc::new(Busy);
    assert_eq!(d.drive(&busy, &run).await, Next::Waiting(Wait::Stalled));
    d.pass(61);
    let mut dying = d.deps();
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    assert_eq!(d.drive(&dying, &run).await, Next::Waiting(Wait::Crashed));
    d.pass(61);
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    d.host.round();
    let types: Vec<String> = d
        .host
        .state
        .lock()
        .unwrap()
        .events
        .iter()
        .map(|e| e.event["type"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        types.iter().filter(|t| *t == "resumed").count(),
        1,
        "{types:?}"
    );
}

/// A sandbox no script may reach.
struct Never;

#[async_trait]
impl Sandbox for Never {
    async fn run(&self, _: &Script, _: &dyn Host, _: &Limits, _: &CancelToken) -> ScriptOutcome {
        panic!("no script runs here");
    }
}

/// A cancel ends a parked run with no sandbox: its next tick takes the
/// `stopped` step. The answer that comes afterwards is refused, and the
/// host is told the run was stopped.
#[tokio::test]
async fn a_cancel_ends_a_parked_run_and_its_late_answer_is_refused() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.env
        .store
        .cancel_run(&clax_bot::run::bot_ctx(clax_store::roles::OWNER), &run)
        .unwrap();
    assert_eq!(d.env.run_row(&run).status, RunStatus::Stopping);
    let mut deps = d.deps();
    deps.sandbox = Arc::new(Never);
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Stopped));
    assert_eq!(d.env.run_row(&run).reason.as_deref(), Some("cancelled"));
    assert_eq!(
        d.steps(&run).iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::Turn, StepKind::Stopped]
    );
    // The answer is late: nothing is journaled for it.
    d.answer("yes");
    assert_eq!(d.journal(&run, 2).len(), 1);
    assert_eq!(d.env.run_row(&run).status, RunStatus::Stopped);
    d.host.round();
    let st = d.host.state.lock().unwrap();
    assert_eq!(
        st.ended.iter().map(|e| &e.end).collect::<Vec<_>>(),
        [&wire::End::Stopped {
            reason: "cancelled".into()
        }]
    );
}

/// A minute to answer a question, and ten seconds before parking.
fn a_minute() -> Arc<Times> {
    Arc::new(Times(Delivery {
        wait_limit: Some(Duration::from_secs(60)),
        ..Delivery::DAEMON
    }))
}

/// The deadline of the call a run is parked on passes: a tick before it
/// does nothing, and the tick after it has the deadline's failure
/// journaled (an effect that was sent and never answered may have taken
/// effect) and runs the step again, whose script catches it and goes
/// on.
#[tokio::test]
async fn a_deadline_that_passes_while_parked_is_journaled_and_the_run_goes_on() {
    let d = desk().await;
    d.env.api.push(script_turn(
        "tu_1",
        "Ask",
        "try { return await clax.desk.ask(\"Well?\"); } catch (e) { return [e.code, e.retryable]; }",
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let deps = d.deps_under(a_minute());
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    assert!(d.env.run_row(&run).due_at.is_some());
    d.pass(30);
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    assert!(d.journal(&run, 2).is_empty());
    d.pass(31);
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!(["unknown_outcome", false]));
    assert_eq!(d.journal(&run, 2)[0]["result"]["code"], "unknown_outcome");
    assert_eq!(d.steps(&run)[1].attempt, 2);
    assert_eq!(d.sent(), ["ask"]);
}

/// A script that, run again with the answer it waited for, takes a path
/// that never reaches the call: the step ends with `replay_diverged`,
/// and does not commit as if the wait had never been.
#[tokio::test]
async fn a_replay_that_ends_before_the_call_it_waited_for_diverges() {
    let d = desk().await;
    d.env.api.push(script_turn(
        "tu_1",
        "Maybe",
        r#"const lamps = await clax.desk.lamps();
if (lamps.length === 0) return "no lamps";
return await clax.desk.ask("Which lamp?");"#,
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.answer("the left one");
    // The read's journal row is changed under the run, so that the
    // script takes its other path when it is run again.
    let db = clax_core::Home::at(d.env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let changed = c
        .execute(
            "UPDATE bot_calls SET result_json = '[]' WHERE run_id = ?1 AND step_n = 2 AND seq = 0",
            rusqlite::params![run],
        )
        .unwrap();
    assert_eq!(changed, 1);
    drop(c);
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    let steps = d.steps(&run);
    let outcome = &steps[1].payload.as_ref().unwrap()["outcome"];
    assert_eq!(outcome["value"], Value::Null, "{outcome}");
    let message = outcome["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("replay_diverged")
            && message.contains("the script ended before call 1, which it waited for"),
        "{message}"
    );
}

/// A script that asks, then tells its person twice.
const ASK_THEN_TELL: &str = r#"const answer = await clax.desk.ask("Move the left lamp?");
const told = [];
for (const n of [1, 2]) {
  try { await clax.desk.notify("note " + n); told.push("told"); } catch (e) { told.push(e.code); }
}
return [answer, told];"#;

/// An effect the run parked on counts once against its limit: the step
/// run again when it is answered replays it from the journal and does
/// not count it again, so of the two effects after it under a limit of
/// two the first is made and the second refused. The run keeps its
/// limit though its host comes back with no settings.
#[tokio::test]
async fn an_effect_the_run_waited_on_counts_once_against_its_limit() {
    let limit = clax_surface::wire::Settings {
        max_effects_per_run: Some(wire::Bound::At(2)),
        ..Default::default()
    };
    let d = Desk::with(Kept::InTheStore, limit.clone(), crate::outside::desk_host()).await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK_THEN_TELL));
    d.env.api.push(finish_turn("done"));
    let run = d.run_with_settings(&limit);
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.pass(60);
    d.answer("yes");
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    let value = d.steps(&run)[1].payload.as_ref().unwrap()["outcome"]["value"].clone();
    assert_eq!(value, json!(["yes", ["told", "effect_limit"]]));
    assert_eq!(d.sent(), ["ask", "notify"]);
}

/// A host's cap on minutes counts no parked time either: a run whose
/// host gave it one minute, parked for a day, finishes when it is
/// answered.
#[tokio::test]
async fn parked_time_does_not_count_against_a_hosts_minutes() {
    let one = clax_surface::wire::Settings {
        runs: Some(wire::RunSettings {
            max_steps: None,
            max_run_minutes: Some(wire::Bound::At(1)),
        }),
        ..Default::default()
    };
    let d = Desk::with(Kept::InTheStore, one.clone(), crate::outside::desk_host()).await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run_with_settings(&one);
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.pass(DAY - 120);
    d.answer("yes");
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
}

/// The time a run is parked is no part of its minutes: parked for
/// longer than its whole cap, it goes on when it is answered.
#[tokio::test]
async fn parked_time_does_not_count_against_the_runs_minutes() {
    let d = desk().await;
    assert_eq!(d.env.settings.0.lock().unwrap().max_run_minutes, 30);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.pass(31 * 60);
    d.answer("yes");
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    // The same minutes not parked are over the cap.
    d.env.api.push(script_turn("tu_1", "Lamps", "return 1;"));
    let run = d.run();
    let (deps, driver) = (d.deps(), DriverId::mint());
    d.env.tick(&deps, &run, &driver).await;
    d.pass(31 * 60);
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Stopped));
    assert_eq!(d.env.run_row(&run).reason.as_deref(), Some("run_cap"));
}

/// A parked run outlives its driver. A driver that starts over the same
/// store takes over every run but the parked one, which it neither
/// claims nor ticks until its call's deadline; woken for it before, it
/// looks once and leaves it. When the answer is journaled and the
/// driver is woken for the run, it runs the step again and the run
/// ends.
#[tokio::test]
async fn a_parked_run_survives_a_restart_and_is_resumed_when_it_is_answered() {
    let d = desk().await;
    // A run parked under a policy with no limit on a wait, which is due
    // when the hub's own limit ends its call, and then one with the
    // day's.
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    let forever = d.run();
    let unbounded = d.deps_under(Arc::new(Times(Delivery {
        wait_limit: None,
        ..Delivery::DAEMON
    })));
    assert_eq!(
        d.drive(&unbounded, &forever).await,
        Next::Waiting(Wait::Parked)
    );
    assert!(d.env.run_row(&forever).due_at.is_some());
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));

    let driver = Driver::new(DriverId::mint(), Arc::new(d.deps()), d.env.hub.clone());
    assert_eq!(driver.start(true).await.unwrap(), 2);
    let idle = |budget| tokio::time::timeout(PROMPTLY, driver.run_until_idle(budget));
    let report = idle(Duration::from_secs(600)).await.unwrap();
    assert_eq!(report.ticks, Vec::new(), "no parked run is ticked on start");
    // Each is due at its call's deadline.
    let wake = report.next_wake.expect("the deadline is armed");
    assert!(wake > Duration::from_secs(DAY - 60) && wake < Duration::from_secs(DAY + 60));
    for id in [&run, &forever] {
        let row = d.env.run_row(id);
        assert_eq!((row.status, row.claimed_by), (RunStatus::Parked, None));
    }
    // Woken with nothing answered, it looks once and waits again. (The
    // budget is not waited for: a driver that is idle returns at once.
    // It bounds only a driver that would tick the parked run without
    // end.)
    driver.wake(&run);
    let report = idle(Duration::from_secs(2)).await.unwrap();
    let ticked: Vec<(&str, Next)> = report
        .ticks
        .iter()
        .map(|(id, out)| (id.as_str(), out.next))
        .collect();
    assert_eq!(ticked, [(run.as_str(), Next::Waiting(Wait::Parked))]);
    assert!(report.next_wake.is_some());
    // Answered, and the driver woken as whoever journals an answer
    // wakes it.
    d.answer("yes");
    driver.wake(&run);
    let report = idle(Duration::from_secs(600)).await.unwrap();
    assert_eq!(d.env.run_row(&run).status, RunStatus::Finished);
    assert!(matches!(report.ticks.last().unwrap().1.next, Next::Done(_)));
    assert_eq!(d.value(&run, 1), json!([2, "yes"]));
    assert_eq!(d.env.run_row(&forever).status, RunStatus::Parked);
}

/// A clock that does something the `after`th time it is read while a
/// run is parked, counting from 0: what lands between a tick's reading
/// of its run and the driver's settling of that tick.
struct Meanwhile {
    inner: Arc<clax_core::working::ManualClock>,
    store: Arc<clax_store::Store>,
    run: String,
    after: std::sync::atomic::AtomicUsize,
    act: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl clax_core::working::Clock for Meanwhile {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        let parked = self
            .store
            .run(&self.run)
            .ok()
            .flatten()
            .is_some_and(|r| r.status == RunStatus::Parked);
        if parked && self.act.lock().unwrap().is_some() {
            let left = self.after.load(Ordering::SeqCst);
            if left == 0 {
                let act = self.act.lock().unwrap().take();
                if let Some(act) = act {
                    act();
                }
            } else {
                self.after.store(left - 1, Ordering::SeqCst);
            }
        }
        self.inner.now()
    }
}

/// A driver over `d` whose clock does `act` the `after`th time it is
/// read while `run` is parked; `act` is given the driver.
fn driver_with(
    d: &Arc<Desk>,
    run: &str,
    after: usize,
    act: impl FnOnce(&Desk, &Driver) + Send + 'static,
) -> Arc<Driver> {
    let clock = Arc::new(Meanwhile {
        inner: d.env.clock.clone(),
        store: d.env.store.clone(),
        run: run.to_string(),
        after: std::sync::atomic::AtomicUsize::new(after),
        act: std::sync::Mutex::new(None),
    });
    let mut deps = d.deps();
    deps.clock = clock.clone();
    let driver = Arc::new(Driver::new(
        DriverId::mint(),
        Arc::new(deps),
        d.env.hub.clone(),
    ));
    let (desk, woken) = (d.clone(), driver.clone());
    *clock.act.lock().unwrap() = Some(Box::new(move || act(&desk, &woken)));
    driver
}

/// The driver runs until it has nothing to do, promptly.
async fn idle(driver: &Driver) -> clax_bot::IdleReport {
    tokio::time::timeout(PROMPTLY, driver.run_until_idle(Duration::from_secs(600)))
        .await
        .expect("the driver goes idle")
}

/// An answer that is journaled, and wakes the driver, after the tick
/// parked the run and before the driver settled that tick is not lost:
/// the run is ticked again at once, and ends.
#[tokio::test]
async fn an_answer_that_lands_while_the_parking_tick_settles_is_not_lost() {
    let d = Arc::new(desk().await);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let woken = run.clone();
    let driver = driver_with(&d, &run, 0, move |d, driver| {
        d.answer("yes");
        driver.wake(&woken);
    });
    driver.wake(&run);
    let report = idle(&driver).await;
    assert_eq!(d.env.run_row(&run).status, RunStatus::Finished);
    let parked = report
        .ticks
        .iter()
        .filter(|(_, out)| out.next == Next::Waiting(Wait::Parked))
        .count();
    assert_eq!(parked, 1, "{:?}", report.ticks);
    assert_eq!(report.next_wake, None);
    assert_eq!(d.value(&run, 1), json!([2, "yes"]));
}

/// The same answer with no one waking the driver for it: the driver
/// that settles a tick which parked a run finds the run parked no
/// longer, and ticks it again.
#[tokio::test]
async fn a_run_found_answered_when_its_parking_tick_settles_is_ticked_again_unwoken() {
    let d = Arc::new(desk().await);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let driver = driver_with(&d, &run, 0, |d, _| d.answer("yes"));
    driver.wake(&run);
    idle(&driver).await;
    assert_eq!(d.env.run_row(&run).status, RunStatus::Finished);
}

/// A cancel that lands in the same place stops the run: its next tick
/// takes the `stopped` step.
#[tokio::test]
async fn a_cancel_that_lands_while_the_parking_tick_settles_stops_the_run() {
    let d = Arc::new(desk().await);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    let run = d.run();
    let cancelled = run.clone();
    let driver = driver_with(&d, &run, 0, move |d, driver| {
        d.env
            .store
            .cancel_run(
                &clax_bot::run::bot_ctx(clax_store::roles::OWNER),
                &cancelled,
            )
            .unwrap();
        driver.cancel(&cancelled);
    });
    driver.wake(&run);
    let report = idle(&driver).await;
    let row = d.env.run_row(&run);
    assert_eq!(
        (row.status, row.reason.as_deref()),
        (RunStatus::Stopped, Some("cancelled"))
    );
    assert_eq!(report.next_wake, None);
}

/// A run whose row the driver cannot read when it settles the tick that
/// parked it (the store fails that read) is looked at again after a
/// wait, by the driver's clock, rather than left with nothing armed;
/// once the row reads again the run waits for its call's deadline.
#[tokio::test]
async fn a_parked_run_whose_row_cannot_be_read_as_its_tick_settles_is_looked_at_again() {
    let d = Arc::new(desk().await);
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    let run = d.run();
    let db = clax_core::Home::at(d.env._dir.path().join("ax")).db_path();
    let sql = Arc::new(std::sync::Mutex::new(
        rusqlite::Connection::open(&db).unwrap(),
    ));
    sql.lock()
        .unwrap()
        .busy_timeout(Duration::from_secs(10))
        .unwrap();
    let away = sql.clone();
    let driver = driver_with(&d, &run, 0, move |_, _| {
        away.lock()
            .unwrap()
            .execute_batch("ALTER TABLE bot_runs RENAME TO bot_runs_away")
            .unwrap();
    });
    driver.wake(&run);
    let report = idle(&driver).await;
    sql.lock()
        .unwrap()
        .execute_batch("ALTER TABLE bot_runs_away RENAME TO bot_runs")
        .unwrap();
    assert_eq!(
        report.ticks.last().map(|(id, out)| (id.as_str(), out.next)),
        Some((run.as_str(), Next::Waiting(Wait::Parked)))
    );
    assert_eq!(report.next_wake, Some(Duration::from_secs(1)), "{report:?}");
    assert_eq!(d.env.run_row(&run).status, RunStatus::Parked);
    // A message sent meanwhile leaves the wait as it is: the run is not
    // ticked for it before the wait is over.
    driver.messaged(&run);
    let report = idle(&driver).await;
    assert!(report.ticks.is_empty(), "{report:?}");
    assert_eq!(report.next_wake, Some(Duration::from_secs(1)), "{report:?}");
    // Looked at again once its wait is over: the row reads, and the run
    // waits for its call's deadline, which is further off.
    d.pass(1);
    let report = idle(&driver).await;
    assert_eq!(
        report
            .ticks
            .iter()
            .map(|(id, out)| (id.as_str(), out.next))
            .collect::<Vec<_>>(),
        [(run.as_str(), Next::Waiting(Wait::Parked))]
    );
    let next = report.next_wake.expect("the deadline is armed");
    assert!(next > Duration::from_secs(1), "{next:?}");
}

/// A tick that only looks at a parked run (it was woken with nothing
/// journaled) and is overtaken by the answer: the run is ticked again
/// and ends, wherever between the driver's picking of the run and its
/// settling of the tick the answer lands. (Which of the driver's
/// readings of its clock is the tick's own varies with the order it
/// polls what it waits on, so each of the first few is tried.)
#[tokio::test]
async fn an_answer_that_lands_while_a_tick_looks_at_a_parked_run_is_not_lost() {
    for after in 0..5 {
        let d = Arc::new(desk().await);
        d.env.api.push(script_turn("tu_1", "Move", MOVE));
        d.env.api.push(finish_turn("done"));
        let run = d.run();
        assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
        let woken = run.clone();
        let driver = driver_with(&d, &run, after, move |d, driver| {
            d.answer("yes");
            driver.wake(&woken);
        });
        driver.wake(&run);
        let report = idle(&driver).await;
        assert_eq!(
            d.env.run_row(&run).status,
            RunStatus::Finished,
            "after {after} readings: {:?}",
            report.ticks
        );
        assert_eq!(d.value(&run, 1), json!([2, "yes"]));
    }
}

/// A sandbox that cannot take its first script, and wakes the driver
/// for the run while that tick is still running.
struct WokenMidTick {
    inner: Arc<dyn Sandbox>,
    driver: std::sync::Mutex<Option<Arc<Driver>>>,
    run: String,
    stalled: AtomicBool,
}

#[async_trait]
impl Sandbox for WokenMidTick {
    async fn run(&self, s: &Script, h: &dyn Host, l: &Limits, c: &CancelToken) -> ScriptOutcome {
        if self.stalled.swap(true, Ordering::SeqCst) {
            return self.inner.run(s, h, l, c).await;
        }
        let driver = self.driver.lock().unwrap().clone();
        driver.expect("the driver is set").wake(&self.run);
        ScriptOutcome::failed(ErrorKind::Unavailable, "busy")
    }
}

/// The rule is the driver's for every tick, not the park's alone: a run
/// woken while a tick of it runs is ticked again when that tick
/// settles, whatever the tick would have had it wait for. Here the tick
/// stalls, which would put the run off by the stall's backoff on a
/// clock nothing moves.
#[tokio::test]
async fn a_run_woken_while_it_is_ticked_is_ticked_again_whatever_the_tick_decided() {
    let d = desk().await;
    d.env.api.push(script_turn(
        "tu_1",
        "Lamps",
        "return (await clax.desk.lamps()).length;",
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let sandbox = Arc::new(WokenMidTick {
        inner: d.env.sandbox.clone(),
        driver: std::sync::Mutex::new(None),
        run: run.clone(),
        stalled: AtomicBool::new(false),
    });
    let mut deps = d.deps();
    deps.sandbox = sandbox.clone();
    let driver = Arc::new(Driver::new(
        DriverId::mint(),
        Arc::new(deps),
        d.env.hub.clone(),
    ));
    *sandbox.driver.lock().unwrap() = Some(driver.clone());
    driver.wake(&run);
    let report = idle(&driver).await;
    assert_eq!(
        d.env.run_row(&run).status,
        RunStatus::Finished,
        "{:?}",
        report.ticks
    );
    assert!(
        report
            .ticks
            .iter()
            .any(|(_, out)| out.next == Next::Waiting(Wait::Stalled)),
        "{:?}",
        report.ticks
    );
    assert_eq!(report.next_wake, None);
}

/// A script that asks and says what it was answered or thrown.
const ASK: &str =
    "try { return await clax.desk.ask(\"Well?\"); } catch (e) { return [e.code, e.retryable]; }";

/// The host's registration ends while its run is parked, and the run
/// is taken on: the waiting call cannot be sent again, and its host was
/// sent it an hour ago. The script is not told that nothing happened:
/// an effect that was sent and never answered may have taken effect.
/// A registration dropped for good is not waited for, under a policy
/// that sends calls again as under one that does not.
#[tokio::test]
async fn a_waited_call_whose_host_is_gone_is_answered_by_what_is_known_of_it() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    d.env.api.push(finish_turn("done"));
    // Bound by the hash of its definition, as a run a host starts is:
    // the definition outlives the registration's lease.
    let run = d.run_bound(Some(&d.host.hash), Some(&d.host.registration), &TIDYS);
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.host.online.store(false, Ordering::SeqCst);
    d.pass(3600);
    d.env.store.unpark_run(&run, d.env.clock_now()).unwrap();
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.value(&run, 1), json!(["unknown_outcome", false]));
    let journal = d.journal(&run, 2);
    assert_eq!(
        journal[0]["result"]["code"], "unknown_outcome",
        "{journal:?}"
    );
    assert_eq!(d.sent(), ["ask"]);
}

/// A script that reads, then asks and says what became of the question.
const READ_THEN_ASK: &str = r#"const lamps = await clax.desk.lamps();
let answer;
try { answer = await clax.desk.ask("Move the left lamp?"); } catch (e) { answer = e.code; }
return [lamps.length, answer];"#;

/// The registration of a run bound by none of its definition's hashes
/// ends while the run is parked, and the call's deadline passes: the step
/// run again with no provider for its surface is answered each call the
/// journal holds from the journal (the read it made before, and the
/// deadline's failure of the call it waited on), not told that nothing
/// happened. (The run then cannot finish: its kind is its surface's.)
#[tokio::test]
async fn a_step_run_again_while_its_provider_is_away_is_answered_from_the_journal() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", READ_THEN_ASK));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.host.online.store(false, Ordering::SeqCst);
    d.pass(2 * DAY);
    // Another host's serve is what the hub drops the lapsed lease at.
    let other = d.world.host(
        "desk",
        &crate::outside::registering(&desk_dts(), None, None, wire::Settings::default()),
        crate::outside::desk_host(),
    );
    other.round();
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert!(
        bound.kind_provider().unavailable().is_some(),
        "the registration is gone"
    );
    d.drive(&d.deps(), &run).await;
    let journal = d.journal(&run, 2);
    assert_eq!(journal[0]["op"], "desk.lamps", "{journal:?}");
    assert_eq!(journal[0]["ok"], true, "{journal:?}");
    let asked = journal[1]["result"]["code"].clone();
    assert_eq!(d.value(&run, 1), json!([2, asked]), "{journal:?}");
    assert_eq!(d.sent(), ["lamps", "ask"]);
}

/// A script that calls an operation its binding lacks (refused, and
/// journaled so), then reads and asks.
const NOTIFY_READ_THEN_ASK: &str = r#"let told;
try { await clax.desk.notify("hi"); told = "told"; } catch (e) { told = e.code; }
const lamps = await clax.desk.lamps();
let answer;
try { answer = await clax.desk.ask("Move the left lamp?"); } catch (e) { answer = e.code; }
return [told, lamps.length, answer];"#;

/// The step run again while its surface's provider is away still has
/// every operation it called before: a binding with no provider lists
/// only the operations its row keeps, and the one the binding refused
/// the first time (`desk.notify`, of the definition and not the run's)
/// is answered its refusal from the journal, where it would otherwise
/// be no function of `clax` at all.
#[tokio::test]
async fn a_step_run_again_while_its_provider_is_away_keeps_the_operations_it_called() {
    let d = desk().await;
    d.env
        .api
        .push(script_turn("tu_1", "Move", NOTIFY_READ_THEN_ASK));
    let run = d.run_bound(
        None,
        Some(&d.host.registration),
        &["desk.lamps", "desk.ask"],
    );
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.host.online.store(false, Ordering::SeqCst);
    d.pass(2 * DAY);
    let other = d.world.host(
        "desk",
        &crate::outside::registering(&desk_dts(), None, None, wire::Settings::default()),
        crate::outside::desk_host(),
    );
    other.round();
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert!(bound.kind_provider().unavailable().is_some());
    assert!(
        !bound.operations().iter().any(|op| op == "desk.notify"),
        "{:?}",
        bound.operations()
    );
    d.drive(&d.deps(), &run).await;
    let journal = d.journal(&run, 2);
    assert_eq!(journal[0]["op"], "desk.notify", "{journal:?}");
    assert_eq!(journal[0]["ok"], false, "{journal:?}");
    let refused = journal[0]["result"]["code"].clone();
    let asked = journal[2]["result"]["code"].clone();
    assert_eq!(d.value(&run, 1), json!([refused, 2, asked]), "{journal:?}");
    assert_eq!(d.sent(), ["lamps", "ask"]);
}

/// The host is away while its run is parked, but within its
/// registration's lease, and the run is taken on: under a policy that
/// sends calls again, the call still waits for its host, and the run
/// parks on it again; nothing is sent meanwhile.
#[tokio::test]
async fn a_waited_call_whose_host_is_away_within_its_lease_is_waited_for() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    d.env.api.push(finish_turn("done"));
    let run = d.run_bound(Some(&d.host.hash), Some(&d.host.registration), &TIDYS);
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.host.online.store(false, Ordering::SeqCst);
    d.pass(60);
    d.env.store.unpark_run(&run, d.env.clock_now()).unwrap();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    assert_eq!(d.journal(&run, 2), Vec::<Value>::new());
    assert_eq!(d.sent(), ["ask"]);
}

/// A run that parks again on the call it was parked on is due when the
/// call was first due: the call's deadline is the one it was opened
/// with, whatever attempt finds it waiting.
#[tokio::test]
async fn a_run_parked_again_on_the_same_call_keeps_the_calls_deadline() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    let due = d.env.run_row(&run).due_at.expect("the call has a deadline");
    d.pass(3600);
    d.host.back(&desk_dts(), 90);
    d.env.store.unpark_run(&run, d.env.clock_now()).unwrap();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    assert_eq!(d.env.run_row(&run).due_at.as_deref(), Some(due.as_str()));
    assert_eq!(d.env.store.run_parking(&run).unwrap().unwrap().parks, 2);
    assert_eq!(d.sent(), ["ask"]);
}

/// The parked time is left out of a run's minutes once: a run that was
/// parked for twenty minutes and has then been at work for longer than
/// its cap stops at the cap.
#[tokio::test]
async fn time_at_work_after_a_park_still_counts_against_the_runs_minutes() {
    let d = desk().await;
    d.env.api.push(script_turn("tu_1", "Move", MOVE));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.pass(20 * 60);
    d.answer("yes");
    // Thirty-one minutes not parked, of the fifty-one since it began.
    d.pass(31 * 60);
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Stopped)
    );
    assert_eq!(d.env.run_row(&run).reason.as_deref(), Some("run_cap"));
}

/// A script that fails on its own before the call it waited for keeps
/// its own error: only a script that would otherwise end cleanly is
/// said to have taken another path.
#[tokio::test]
async fn a_replay_that_fails_before_the_call_it_waited_for_keeps_its_error() {
    let d = desk().await;
    d.env.api.push(script_turn(
        "tu_1",
        "Maybe",
        r#"const lamps = await clax.desk.lamps();
if (lamps.length === 0) throw new Error("no lamps at all");
return await clax.desk.ask("Which lamp?");"#,
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    d.answer("the left one");
    let db = clax_core::Home::at(d.env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c.execute(
        "UPDATE bot_calls SET result_json = '[]' WHERE run_id = ?1 AND step_n = 2 AND seq = 0",
        rusqlite::params![run],
    )
    .unwrap();
    drop(c);
    assert_eq!(
        d.drive(&d.deps(), &run).await,
        Next::Done(RunStatus::Finished)
    );
    let steps = d.steps(&run);
    let message = steps[1].payload.as_ref().unwrap()["outcome"]["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("no lamps at all") && !message.contains("replay_diverged"),
        "{message}"
    );
}
