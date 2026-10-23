//! Cancelling a run mid-turn and mid-script, and a sandbox that cannot
//! take a script (a stall, retried and never charged).

use crate::common::*;
use async_trait::async_trait;
use clax_bot::events::{Event, EventSink};
use clax_bot::{Next, TickOpts, Wait};
use clax_bot_sandbox::{CancelToken, ErrorKind, Host, Limits, Sandbox, Script, ScriptOutcome};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use std::sync::Arc;

#[tokio::test]
async fn a_cancel_mid_turn_stops_the_run_and_charges_what_the_call_used() {
    let env = Arc::new(Env::new().await);
    env.api.push(Reply::StartThenHang((1000, 2000, 500, 0)));
    let run = env.run();
    let cancel = CancelToken::new();
    let task = {
        let (env, run, cancel) = (env.clone(), run.clone(), cancel.clone());
        tokio::spawn(async move {
            let deps = env.deps();
            clax_bot::tick(
                &deps,
                &run,
                &DriverId::mint(),
                TickOpts {
                    budget: None,
                    cancel,
                    events: &*env.hub,
                },
            )
            .await
            .unwrap()
        })
    };
    // Cancel once everything the provider sent has streamed in.
    let mut live = env.hub.subscribe(&env.store, &run, None).unwrap();
    let mut deltas = 0;
    while deltas < PARTIAL_DELTAS {
        let e = tokio::time::timeout(PROMPTLY, live.next())
            .await
            .expect("the turn streams")
            .unwrap()
            .unwrap();
        if e.event == "turn_delta" {
            deltas += 1;
        }
    }
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &run)
        .unwrap();
    cancel.cancel();
    let out = tokio::time::timeout(PROMPTLY, task).await.unwrap().unwrap();
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(out.kind, Some(StepKind::Stopped));
    let r = env.run_row(&run);
    assert_eq!(r.reason.as_deref(), Some("cancelled"));
    // The provider billed the call's input; the person who cancelled it
    // pays for it.
    // The output, reported only at a stream's end, is estimated from what
    // streamed: never less than the provider bills.
    let (tokens, charged) = env.ledger_tokens();
    let served = env.api.served();
    assert_eq!(
        (tokens.0, tokens.1, tokens.2),
        (served.0, served.1, served.2)
    );
    assert!(tokens.3 >= served.3, "{tokens:?} < {served:?}");
    assert_eq!(charged, 4);
    let rows = env.usage();
    assert!(rows.iter().all(|r| !r.absorbed));
    let out = rows.iter().find(|r| r.line.ends_with(".output")).unwrap();
    assert!(out.estimated && out.credits_milli > 0, "{out:?}");
    let kinds: Vec<String> = env.bot_events().into_iter().map(|(k, _)| k).collect();
    assert_eq!(kinds, ["bot.run_start", "bot.cancel", "bot.run_end"]);
}

/// Cancels a token when a script calls `op`.
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

#[tokio::test]
async fn a_cancel_mid_script_charges_what_it_used_and_keeps_no_draft() {
    let env = Env::new().await;
    env.set(|s| s.script_cpu_ms = 600_000);
    env.api.push(env.script_turn(
        "tu_1",
        "Loop",
        "await clax.files.write('index.html', 'gone'); await clax.artifact.get(); while (true) {}",
    ));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let token = CancelToken::new();
    let sink = CancelOnCall {
        op: "artifact.get",
        token: token.clone(),
    };
    let out = tokio::time::timeout(
        PROMPTLY,
        clax_bot::tick(
            &deps,
            &run,
            &d,
            TickOpts {
                budget: None,
                cancel: token,
                events: &sink,
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("cancelled"));
    assert!(env.store.draft_files(&run).unwrap().is_empty());
    let rows = env.usage();
    let compute = rows
        .iter()
        .find(|r| r.line == "script.compute")
        .expect("compute charged");
    assert_eq!(compute.step_n, Some(2));
    let events = env.bot_events();
    let script = events.iter().find(|(k, _)| k == "bot.script").unwrap();
    assert!(script.1["error"].as_str().is_some());
    assert_eq!(events.iter().filter(|(k, _)| k == "bot.call").count(), 2);
}

/// A sandbox that can never take a script.
struct Unavailable;

#[async_trait]
impl Sandbox for Unavailable {
    async fn run(&self, _: &Script, _: &dyn Host, _: &Limits, _: &CancelToken) -> ScriptOutcome {
        ScriptOutcome::failed(ErrorKind::Unavailable, "too many abandoned scripts")
    }
}

#[tokio::test]
async fn an_unavailable_sandbox_stalls_the_step_uncharged() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let d = DriverId::mint();
    let stalled = env.deps_with(Arc::new(Unavailable));
    env.tick(&stalled, &run, &d).await;
    for _ in 0..3 {
        let out = env.tick(&stalled, &run, &d).await;
        assert_eq!(out.next, Next::Waiting(Wait::Stalled));
    }
    assert!(env.usage().iter().all(|r| r.line != "script.compute"));
    let open = env.store.open_step(&run).unwrap().unwrap();
    assert_eq!((open.n, open.kind), (2, StepKind::Script));
    // The step cap does not count stalls.
    env.set(|s| s.max_steps = 3);
    let out = env.drive(&env.deps(), &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(steps.len(), 4);
    assert_eq!(steps[1].attempt, 4);
}

/// A sandbox whose script held more memory than the configured limit.
struct Peaky;

#[async_trait]
impl Sandbox for Peaky {
    async fn run(&self, _: &Script, _: &dyn Host, _: &Limits, _: &CancelToken) -> ScriptOutcome {
        ScriptOutcome {
            value: Some(serde_json::json!(1)),
            cpu_us: 1_000,
            mem_peak_bytes: 256 << 20,
            ..ScriptOutcome::default()
        }
    }
}

#[tokio::test]
async fn compute_is_billed_at_the_larger_of_the_limit_and_the_peak() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&env.deps_with(Arc::new(Peaky)), &run, &DriverId::mint())
        .await;
    let compute = env
        .usage()
        .into_iter()
        .find(|r| r.line == "script.compute")
        .unwrap();
    // 1 ms of CPU at 256 MiB, not at the configured 64.
    assert_eq!(compute.qty, 256);
}
