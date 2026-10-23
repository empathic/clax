//! A run survives its driver: a crash at every step boundary is resumed by
//! another driver, each step charged once and each write made once; two
//! drivers racing for one run never both execute a step.

use crate::common::*;
use crate::runs::EDIT;
use clax_bot::run::{CrashPoint, Deps};
use clax_bot::{Next, Wait};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The steps of the edit run, in order.
const STEPS: [StepKind; 4] = [
    StepKind::Turn,
    StepKind::Script,
    StepKind::Turn,
    StepKind::Finish,
];

pub fn crash_once(deps: &mut Deps, at: CrashPoint) {
    let fired = Arc::new(AtomicBool::new(false));
    deps.crash = Some(Arc::new(move |p| {
        p == at && !fired.swap(true, Ordering::SeqCst)
    }));
}

async fn crashed_run(at: CrashPoint) {
    let env = Env::new().await;
    // A turn executed again is requested again.
    let replies = |env: &Env| {
        env.api
            .push(env.script_turn("tu_1", "Make the heading blue", EDIT));
    };
    replies(&env);
    if at
        == (CrashPoint::BeforeCommit {
            step: 1,
            kind: StepKind::Turn,
        })
    {
        replies(&env);
    }
    env.api.push(finish_turn("Made the heading blue."));
    if at
        == (CrashPoint::BeforeCommit {
            step: 3,
            kind: StepKind::Turn,
        })
    {
        env.api.push(finish_turn("Made the heading blue."));
    }
    let run = env.run();
    let mut a = env.deps();
    crash_once(&mut a, at);
    let da = DriverId::mint();
    let out = env.drive(&a, &run, &da).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(Wait::Crashed),
        "{at:?}"
    );
    let terminal = matches!(
        at,
        CrashPoint::AfterCommit {
            kind: StepKind::Finish,
            ..
        }
    );
    let b = env.deps();
    let db = DriverId::mint();
    if !terminal {
        // The dead driver's claim holds until its lease lapses.
        assert_eq!(
            env.tick(&b, &run, &db).await.next,
            Next::Waiting(Wait::NotOurs),
            "{at:?}"
        );
        env.clock.advance(61);
        let out = env.drive(&b, &run, &db).await;
        assert_eq!(
            out.last().unwrap().next,
            Next::Done(RunStatus::Finished),
            "{at:?}"
        );
    }
    let (r, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(r.status, RunStatus::Finished, "{at:?}");
    assert_eq!(
        steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
        STEPS,
        "{at:?}"
    );
    if let CrashPoint::BeforeCommit { step, .. } = at {
        assert_eq!(steps[step as usize - 1].attempt, 2, "{at:?}: begun again");
    }
    // Every model call the provider answered is in the ledger, and each
    // step is charged once: a repeated turn's first call is absorbed.
    let (tokens, charged) = env.ledger_tokens();
    assert_eq!(tokens, env.api.served(), "{at:?}");
    assert_eq!(charged, 8, "{at:?}: two turns' four token rows");
    let rows = env.usage();
    assert_eq!(rows.iter().filter(|r| r.line == "harness.run").count(), 1);
    assert_eq!(
        rows.iter().filter(|r| r.line == "script.compute").count(),
        1,
        "{at:?}"
    );
    assert_eq!(
        r.credits_milli,
        rows.iter().map(|r| r.credits_milli).sum::<i64>()
    );
    if matches!(
        at,
        CrashPoint::BeforeCommit {
            kind: StepKind::Turn,
            ..
        }
    ) {
        assert!(
            rows.iter()
                .any(|r| r.absorbed && r.credits_milli == 0 && r.cost_micro_usd > 0)
        );
        assert!(
            env.store
                .usage_report(env.clock_now())
                .unwrap()
                .absorbed_micro_usd
                > 0
        );
    }
    let events = env.bot_events();
    assert_eq!(
        events.iter().filter(|(k, _)| k == "bot.turn").count(),
        2,
        "{at:?}"
    );
    assert_eq!(
        events.iter().filter(|(k, _)| k == "bot.script").count(),
        1,
        "{at:?}"
    );
    // Written once, however often the script ran.
    let st = env.host.lock();
    assert_eq!(st.versions.len(), 1, "{at:?}");
    assert_eq!(env.replies().len(), 1, "{at:?}");
    // A script run again was answered from its journal: the host saw
    // each write once.
    for op in ["draft.publish", "threads.reply"] {
        assert_eq!(
            st.calls.iter().filter(|(_, o)| o == op).count(),
            1,
            "{at:?}: {op}"
        );
    }
    assert_eq!(env.api.left(), 0, "{at:?}");
}

#[tokio::test]
async fn a_crash_before_any_commit_is_resumed_and_charged_once() {
    for (i, kind) in STEPS.iter().enumerate() {
        crashed_run(CrashPoint::BeforeCommit {
            step: i as i64 + 1,
            kind: *kind,
        })
        .await;
    }
}

#[tokio::test]
async fn a_crash_after_any_commit_is_resumed_and_charged_once() {
    for (i, kind) in STEPS.iter().enumerate() {
        crashed_run(CrashPoint::AfterCommit {
            step: i as i64 + 1,
            kind: *kind,
        })
        .await;
    }
}

#[tokio::test]
async fn two_drivers_racing_for_a_run_execute_each_step_once() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let (a, b) = (env.deps(), env.deps());
    let (da, db) = (DriverId::mint(), DriverId::mint());
    let (x, y) = tokio::join!(env.tick(&a, &run, &da), env.tick(&b, &run, &db));
    let waits = [x.next, y.next]
        .iter()
        .filter(|n| **n == Next::Waiting(Wait::NotOurs))
        .count();
    assert_eq!(waits, 1, "{x:?} {y:?}");
    assert_eq!(env.api.requests().len(), 1);
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].committed_at.is_some());
}

#[tokio::test]
async fn a_scripts_thread_names_are_replaced_once_even_inside_an_id() {
    let mut env = Env::new().await;
    // ULIDs hold T1 and T2 by chance: the replaced ID is never rewritten.
    env.t1 = "01ABT2CDT1".into();
    env.t2 = "01EFT1GHT2".into();
    assert_eq!(
        env.code(r#"reply("T1"); reply("T2"); "TT1""#),
        r#"reply("01ABT2CDT1"); reply("01EFT1GHT2"); "T01ABT2CDT1""#
    );
}
