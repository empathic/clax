//! What model calls cost when they do not commit: a cut stream retried in
//! the tick, a step that cannot commit, a tick without the budget for a
//! turn. Every call the provider answered is in the ledger.

use crate::common::*;
use clax_bot::{Driver, Next, Wait};
use clax_core::audit::AuditCtx;
use clax_sample::messages::RetryPolicy;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn a_cut_stream_retried_in_the_tick_is_absorbed_and_the_retry_charged() {
    let env = Env::new().await;
    env.api.push(Reply::Cut((1000, 0, 0, 0)));
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let mut deps = env.deps();
    deps.retry = RetryPolicy {
        max_attempts: 2,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        max_retry_after: Duration::from_secs(1),
    };
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(env.api.requests().len(), 3);
    let (tokens, charged) = env.ledger_tokens();
    let served = env.api.served();
    assert_eq!(
        (tokens.0, tokens.1, tokens.2),
        (served.0, served.1, served.2)
    );
    assert!(tokens.3 >= served.3, "{tokens:?} < {served:?}");
    assert_eq!(charged, 8);
    let absorbed: Vec<(String, i64, bool)> = env
        .usage()
        .into_iter()
        .filter(|r| r.absorbed)
        .map(|r| (r.line, r.qty, r.estimated))
        .collect();
    let streamed = (PARTIAL_TEXT.len() * PARTIAL_DELTAS).div_ceil(3) as i64;
    assert_eq!(
        absorbed,
        [
            ("claude-sonnet-5-5.input".to_string(), 1000, false),
            ("claude-sonnet-5-5.output".to_string(), streamed, true)
        ]
    );
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(steps[0].attempt, 2, "the retry is a new attempt");
}

#[tokio::test]
async fn a_step_that_cannot_commit_ends_the_run_failed() {
    let env = Env::new().await;
    // Without its cost line, every step's commit is refused.
    env.store
        .delete_cost_line(&AuditCtx::DAEMON, "do.requests")
        .unwrap();
    env.set(|s| s.infra_lines = true);
    for _ in 0..3 {
        env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    }
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    for _ in 0..3 {
        assert!(env.try_tick(&deps, &run, &d, None).await.is_err());
    }
    assert_eq!(env.store.open_step(&run).unwrap().unwrap().failures, 3);
    let out = env.tick(&deps, &run, &d).await;
    assert_eq!(out.next, Next::Done(RunStatus::Failed));
    assert_eq!(out.kind, Some(StepKind::Failed));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("internal_error"));
    let notice = env.replies()[0].1.clone();
    assert!(notice.contains("could not save my work"), "{notice}");
    // The three calls the provider answered are absorbed.
    let (tokens, charged) = env.ledger_tokens();
    assert_eq!(tokens, env.api.served());
    assert_eq!(tokens.0, 3000);
    assert_eq!(charged, 0);
    let ends: Vec<_> = env
        .bot_events()
        .into_iter()
        .filter(|(k, _)| k == "bot.run_end")
        .collect();
    assert_eq!(ends[0].1["outcome"], "failed");
}

#[tokio::test]
async fn a_tick_without_the_budget_for_a_turn_waits_for_a_fresh_one() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let deps = Arc::new(env.deps());
    let d = DriverId::mint();
    let out = env
        .try_tick(&deps, &run, &d, Some(Duration::from_secs(10)))
        .await
        .unwrap();
    assert_eq!(out.next, Next::Waiting(Wait::Budget));
    assert!(env.api.requests().is_empty());
    assert!(
        env.store.open_step(&run).unwrap().is_none(),
        "nothing begun"
    );
    let driver = Driver::new(DriverId::mint(), deps, env.hub.clone());
    driver.resume(true).unwrap();
    let report = driver.run_until_idle(Duration::from_secs(10)).await;
    assert_eq!(report.next_wake, Some(Duration::ZERO));
    assert!(env.api.requests().is_empty());
    // With a budget for a turn, the run goes on to its end.
    env.api.push(finish_turn("done"));
    let report = driver.run_until_idle(Duration::from_secs(120)).await;
    assert_eq!(report.ticks[0].1.kind, Some(StepKind::Turn));
    assert_eq!(report.next_wake, None);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(env.api.requests().len(), 2);
}

#[tokio::test]
async fn a_repeated_call_is_priced_as_it_was_made() {
    use clax_store::credits::CostLinePatch;
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let mut a = env.deps();
    crate::resume::crash_once(
        &mut a,
        clax_bot::run::CrashPoint::BeforeCommit {
            step: 1,
            kind: StepKind::Turn,
        },
    );
    let da = DriverId::mint();
    assert_eq!(
        env.tick(&a, &run, &da).await.next,
        Next::Waiting(Wait::Crashed)
    );
    // The input price changes before the step commits.
    let line = "claude-sonnet-5-5.input";
    let before = env.store.cost_line(line).unwrap().unwrap().cost_micro_usd;
    env.store
        .put_cost_line(
            &AuditCtx::DAEMON,
            line,
            CostLinePatch {
                cost_micro_usd: Some(before * 3),
                ..CostLinePatch::default()
            },
        )
        .unwrap();
    env.clock.advance(61);
    let out = env.tick(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.kind, Some(StepKind::Turn));
    let rows: Vec<_> = env.usage().into_iter().filter(|r| r.line == line).collect();
    assert_eq!(rows.len(), 2);
    let absorbed = rows.iter().find(|r| r.absorbed).unwrap();
    let charged = rows.iter().find(|r| !r.absorbed).unwrap();
    assert_eq!(
        absorbed.rate_cost_micro_usd, before,
        "the first call's price"
    );
    assert!(absorbed.cost_micro_usd > 0);
    assert_eq!(
        charged.rate_cost_micro_usd,
        before * 3,
        "the committed call's price"
    );
}
