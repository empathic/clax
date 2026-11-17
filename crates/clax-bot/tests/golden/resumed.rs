//! A script run again after a crash: its driver dies when the script has
//! returned and nothing of the step is committed, and another driver
//! executes the step again, each call answered from the journal the
//! first left. What the step leaves is pinned (`resumed.*.json`), and is
//! what the same script leaves in a run that did not crash. Also here:
//! a script that follows one that published or rebased, resumed the same
//! way; a replay that diverged from a first attempt that went further
//! (`audit.replay-diverged.json`); and what a script step's requests
//! line counts.

use super::names::{self as g, Golden};
use super::pins::{GIVEN, PUBLISH, Pins, fetched, price_the_web, sender_says};
use super::scripts::{script_result, someone_publishes};
use crate::common::*;
use crate::resume::crash_once;
use crate::runs::EDIT;
use clax_bot::events::{Event, EventSink};
use clax_bot::run::CrashPoint;
use clax_bot::{Next, TickOpts, Wait};
use clax_bot_sandbox::{CancelToken, Host, Limits, Sandbox, Script, ScriptOutcome};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use serde_json::{Value, json};
use std::sync::Arc;

/// A thread run, prepared by `setup`, that declares the web operations of
/// `shape` and runs each script of `scripts` in a step of its own (steps
/// 2, 4, …), then finishes. With `crash`, its first driver dies before
/// the commit of the last script's step, and a second resumes the run
/// once the first's claim has lapsed.
async fn script_run(setup: fn(&Env), shape: &str, scripts: &[&str], crash: bool) -> Pins {
    let env = Env::new().await;
    setup(&env);
    const IDS: [&str; 2] = ["tu_1", "tu_2"];
    for (id, code) in IDS.iter().zip(scripts) {
        env.api.push(env.script_turn(id, "Script", code));
    }
    env.api.push(finish_turn("done"));
    let run = env.run_with_web(shape);
    if crash {
        let mut dying = env.deps();
        crash_once(
            &mut dying,
            CrashPoint::BeforeCommit {
                step: last_script(scripts),
                kind: StepKind::Script,
            },
        );
        let out = env.drive(&dying, &run, &DriverId::mint()).await;
        assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
        env.clock.advance(61);
    }
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    Pins::new(env, run)
}

/// The step of the last of `scripts`.
fn last_script(scripts: &[&str]) -> i64 {
    2 * i64::try_from(scripts.len()).expect("a few scripts")
}

/// What step `n`, a script's, of [`script_run`] leaves: the step as
/// stored (its attempt, times and payload, which holds what the model is
/// told, the step's `calls`, and the `base` the next script's draft is
/// loaded from), the run's journal, the writes the fake host committed,
/// the drafts it was given to publish, the usage rows the run's steps
/// charged, and the run's audit records.
fn left(p: &Pins, n: i64) -> Value {
    let steps = p.steps(&p.env.api.requests()[0].body);
    json!({
        "audit": p.audit(),
        "drafts_published": p.drafts_published(),
        "journal": p.journal(),
        "step": steps[usize::try_from(n).expect("a step") - 1],
        "usage": p.usage(),
        "writes_in_order": p.writes(),
    })
}

/// `left` without what a crash before the step's commit changes: the
/// attempt that committed it, and when; and the audit records, which
/// say of each call the committing attempt was answered from the
/// journal that it was (`replayed`).
fn across_a_crash(mut left: Value) -> Value {
    assert!(
        left.as_object_mut()
            .expect("what is left")
            .remove("audit")
            .is_some()
    );
    let step = left["step"].as_object_mut().expect("the step");
    for key in ["attempt", "committed_at"] {
        assert!(step.remove(key).is_some(), "the step's {key}");
    }
    left
}

/// Pins as `golden` what the last of `scripts` leaves in a run that
/// crashed before that step's commit and was resumed, and checks it is
/// what the script leaves in a run that did not crash.
async fn pin_resumed(golden: Golden, setup: fn(&Env), shape: &str, scripts: &[&str]) {
    let n = last_script(scripts);
    let resumed = script_run(setup, shape, scripts, true).await;
    let plain = script_run(setup, shape, scripts, false).await;
    let (crashed, uncrashed) = (
        resumed.normalised(&left(&resumed, n)),
        plain.normalised(&left(&plain, n)),
    );
    assert_eq!(crashed["step"]["attempt"], 2, "the step was begun again");
    assert_eq!(uncrashed["step"]["attempt"], 1);
    // The calls the resumed attempt was answered from the journal are
    // audited as such, and no call of the run that did not crash is.
    let replayed = |left: &Value| {
        left["audit"]["records"]
            .as_array()
            .expect("the audit records")
            .iter()
            .filter(|r| r["kind"] == "bot.call" && r["body"]["replayed"] == true)
            .count()
    };
    assert!(replayed(&crashed) > 0, "the resumed step's replayed calls");
    assert_eq!(replayed(&uncrashed), 0);
    // A call answered from the journal is told as having taken no time:
    // every call of the resumed step was, and the goldens name every
    // `ms` instead of holding it.
    let (_, steps) = resumed
        .env
        .store
        .run_with_steps(&resumed.run)
        .unwrap()
        .unwrap();
    let calls = steps[usize::try_from(n).expect("a step") - 1]
        .payload
        .as_ref()
        .expect("the step's payload")["calls"]
        .as_array()
        .expect("the step's calls")
        .clone();
    assert!(!calls.is_empty());
    for call in &calls {
        assert_eq!(call["ms"], 0, "a replayed call's `ms`: {call}");
    }
    resumed.pin(
        &format!(
            "what a script run again after a crash leaves ({}): its step, the journal, the \
             host's writes, the drafts published, the usage and the audit records",
            golden.file()
        ),
        golden,
        &left(&resumed, n),
    );
    assert_eq!(
        serde_json::to_string_pretty(&across_a_crash(crashed)).unwrap(),
        serde_json::to_string_pretty(&across_a_crash(uncrashed)).unwrap(),
        "a script run again after a crash (left) leaves what it leaves in a run that did not \
         crash (right), but for the step's attempt and commit time and the audit records"
    );
}

/// A script that edits, publishes and replies: run again, its publish and
/// its reply are answered from the journal, the host writes each once,
/// and the step records the version the draft is now based on.
#[tokio::test]
async fn a_script_that_published_is_resumed_as_it_ran() {
    pin_resumed(g::RESUMED_PUBLISH, |_| {}, "", &[EDIT]).await;
}

/// A script that writes, rebases onto a version someone else published
/// before the run started, publishes, replies and fetches a page: run
/// again, the rebase goes to the version it went to, and the publish, the
/// reply and the fetch are answered from the journal.
#[tokio::test]
async fn a_script_that_rebased_published_and_fetched_is_resumed_as_it_ran() {
    let code = format!(
        r#"await clax.files.write("extra.css", "h1 {{ color: blue; }}");
const rebased = await clax.draft.rebase();
const v = await clax.draft.publish({{note: "A stylesheet", addresses: ["T1"]}});
await clax.threads.reply("T1", "Published the stylesheet.");
const page = await clax.web.fetch("{GIVEN}");
return {{rebased, version: v.version, read: page.totalChars, files: await clax.files.list()}};"#
    );
    pin_resumed(
        g::RESUMED_REBASE_PUBLISH_FETCH,
        |env| {
            price_the_web(env);
            env.web().page(GIVEN, fetched(GIVEN));
            sender_says(env, &format!("Read {GIVEN}"));
            someone_publishes(env, 1);
        },
        "fetch",
        &[&code],
    )
    .await;
}

/// A script after one that published: its draft is loaded at the
/// version the first script published, which the first script's step
/// keeps (`payload.base`). It reads the first script's edit, writes, and
/// publishes over that version (`drafts_published`: `if_version` 2).
#[tokio::test]
async fn a_script_after_a_publish_is_resumed_from_the_published_version() {
    pin_resumed(
        g::RESUMED_SECOND_SCRIPT_AFTER_A_PUBLISH,
        |_| {},
        "",
        &[
            PUBLISH,
            r#"const html = await clax.files.read("index.html");
await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "A stylesheet"});
return {blue: html.includes("--accent: #00c;"), version: v.version, files: await clax.files.list()};"#,
        ],
    )
    .await;
}

/// A script after one that rebased onto a version someone else
/// published before the run started: its draft is loaded at that
/// version, so it reads their file and publishes over their version.
#[tokio::test]
async fn a_script_after_a_rebase_is_resumed_from_the_version_rebased_onto() {
    pin_resumed(
        g::RESUMED_SECOND_SCRIPT_AFTER_A_REBASE,
        |env| someone_publishes(env, 1),
        "",
        &[
            r#"const r = await clax.draft.rebase(); return r.version;"#,
            r#"const notes = await clax.files.read("notes.txt");
const v = await clax.draft.publish({note: "Over theirs"});
return {notes, version: v.version, files: await clax.files.list()};"#,
        ],
    )
    .await;
}

/// With the hosted Worker's usage lines on, a script step's requests
/// line counts the step and each call that reached the run loop, and a
/// turn's counts the step alone.
#[tokio::test]
async fn a_script_steps_requests_line_counts_its_calls() {
    let env = Env::new().await;
    env.set(|s| s.infra_lines = true);
    env.api.push(env.script_turn("tu_1", "Script", EDIT));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    // `EDIT` makes five calls: a read, a replace, a check, a publish and
    // a reply.
    assert_eq!(
        requests(&env),
        [(Some(1), 1), (Some(2), 6), (Some(3), 1), (Some(4), 1)]
    );
}

/// The requests line of each step of `env`'s runs: the step and the
/// quantity.
fn requests(env: &Env) -> Vec<(Option<i64>, i64)> {
    env.usage()
        .iter()
        .filter(|r| r.line == clax_store::credits::LINE_DO_REQUESTS)
        .map(|r| (r.step_n, r.qty))
        .collect()
}

/// A run stopped by its step cap after a script of five calls: the
/// `stopped` step's requests line counts the step alone.
#[tokio::test]
async fn a_stopped_steps_requests_line_counts_no_call() {
    let env = Env::new().await;
    env.set(|s| {
        s.infra_lines = true;
        s.max_steps = 2;
    });
    env.api.push(env.script_turn("tu_1", "Script", EDIT));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    assert_eq!(
        requests(&env),
        [(Some(1), 1), (Some(2), 6), (Some(3), 1)],
        "a turn, a script of five calls, and the stopped step"
    );
}

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

/// A script cancelled after two calls: its step is committed as the
/// run's `stopped` step, whose requests line counts the step alone and
/// neither call.
#[tokio::test]
async fn a_cancelled_scripts_stopped_step_counts_no_call_on_its_requests_line() {
    let env = Env::new().await;
    env.set(|s| {
        s.infra_lines = true;
        s.script_cpu_ms = 600_000;
    });
    env.api.push(env.script_turn(
        "tu_1",
        "Loop",
        "await clax.files.list(); await clax.artifact.get(); while (true) {}",
    ));
    let run = env.run();
    let deps = env.deps();
    let driver = DriverId::mint();
    assert_eq!(
        env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    let token = CancelToken::new();
    let sink = CancelOnCall {
        op: "artifact.get",
        token: token.clone(),
    };
    let opts = TickOpts {
        budget: None,
        cancel: token,
        events: &sink,
    };
    let out = tokio::time::timeout(PROMPTLY, clax_bot::tick(&deps, &run, &driver, opts))
        .await
        .expect("the cancel ends the script")
        .unwrap();
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("cancelled"));
    assert_eq!(
        requests(&env),
        [(Some(1), 1), (Some(2), 1)],
        "a turn, and the script's step committed as stopped"
    );
}

/// A sandbox that runs another script than the one it is given: what
/// stands for a script that took another path the first time it ran.
struct Another {
    sandbox: Arc<dyn Sandbox>,
    code: String,
}

#[async_trait::async_trait]
impl Sandbox for Another {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let other = Script {
            code: self.code.clone(),
            ..script.clone()
        };
        self.sandbox.run(&other, host, limits, cancel).await
    }
}

/// A script whose first attempt made four calls, a reply among them, and
/// died before its commit, and whose second attempt makes the same first
/// call and takes another path at its second, as a script run again
/// does: its first call is made before it has any answer to differ by.
/// What the step that commits leaves: what the model is told (the
/// replay's `completed` names the call that was answered again before
/// it stopped), and the audit records: the script's, a `bot.call` for
/// the call performed again (`replayed`), one for the call that
/// diverged, and one for each row the first attempt left at a place the
/// second did not reach, with the attempt and the place it was made at.
/// The reply the first attempt posted stands.
#[tokio::test]
async fn the_audit_of_a_replay_that_diverged_after_a_longer_first_attempt_is_pinned() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Read then reply",
        "const a = await clax.files.list(); await clax.threads.reply('T1', 'hi'); return a.length;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let mut dying = env.deps_with(Arc::new(Another {
        sandbox: env.sandbox.clone(),
        code: env.code(
            "await clax.files.list(); await clax.files.read('index.html'); \
             await clax.threads.reply('T1', 'first'); await clax.artifact.get(); return 0;",
        ),
    }));
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&dying, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let p = Pins::new(env, run);
    let steps = p.steps(&p.env.api.requests()[0].body);
    p.pin(
        "what a replay that diverged leaves (audit.replay-diverged.json): the audit records, \
         the journal the first attempt left, what the model is told, the step's calls and the \
         host's writes",
        g::AUDIT_REPLAY_DIVERGED,
        &json!({
            "audit": p.audit(),
            "journal": p.journal(),
            "result": script_result(&p, 2),
            "step_calls": steps[1]["payload"]["calls"],
            "writes_in_order": p.writes(),
        }),
    );
}
