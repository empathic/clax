//! What stops a run before its next step: credits (and the unenforced
//! flag that never does), the flag, and the run's own caps; and the
//! `max_tokens` a turn's credits can pay for.

use crate::common::*;
use clax_bot::{Next, Refusal};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::roles::OWNER;
use serde_json::json;

fn free_weekly(env: &Env, milli: i64) {
    env.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::FreeWeeklyMilli, Some(json!(milli)))],
        )
        .unwrap();
}

fn spent(env: &Env) -> i64 {
    env.store
        .week_usage(OWNER, env.clock_now())
        .unwrap()
        .spent_milli
}

fn refusal(env: &Env) -> Option<Refusal> {
    let settings = env.settings.0.lock().unwrap().clone();
    clax_bot::refusal(
        &env.store,
        &settings,
        &env.deps().keys,
        &*env.clock,
        OWNER,
        "claude-sonnet-5-5",
    )
    .unwrap()
}

#[tokio::test]
async fn spent_credits_stop_a_run_before_any_model_call() {
    let env = Env::new().await;
    free_weekly(&env, 0);
    assert_eq!(refusal(&env), Some(Refusal::CreditsExhausted));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].kind, Some(StepKind::Stopped));
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::Stopped, Some("credits_exhausted"))
    );
    assert!(env.api.requests().is_empty());
    assert_eq!(env.replies().len(), 1);
    let notice = &env.replies()[0].1;
    assert!(
        notice.contains("used this week's ClaxBot credits"),
        "{notice}"
    );
    assert!(
        notice.contains("reset on Monday 26 October 2026 at 00:00 UTC"),
        "{notice}"
    );
}

#[tokio::test]
async fn credits_spent_mid_run_stop_it_with_its_draft_saved() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Edit",
        "await clax.files.write('extra.css', 'body{}'); return 1;",
    ));
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    // The week's credits run out after the turn: the script it asked for
    // does not run.
    free_weekly(&env, spent(&env));
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(
        out.iter().filter_map(|o| o.kind).collect::<Vec<_>>(),
        [StepKind::Stopped]
    );
    assert_eq!(
        env.run_row(&run).reason.as_deref(),
        Some("credits_exhausted")
    );
    assert_eq!(env.api.requests().len(), 1);
    let notice = env.replies()[0].1.clone();
    assert!(
        notice.contains("I had not run any scripts yet."),
        "{notice}"
    );
}

#[tokio::test]
async fn a_turn_asks_for_no_more_output_than_the_credits_left_pay_for() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.tick(&deps, &run, &d).await;
    // 30 credits left: the next turn's output is capped below 32000.
    free_weekly(&env, spent(&env) + 30_000);
    env.tick(&deps, &run, &d).await;
    let reqs = env.api.requests();
    assert_eq!(reqs[0].body["max_tokens"], 32000);
    let capped = reqs[1].body["max_tokens"].as_u64().unwrap();
    assert!((1024..32000).contains(&capped), "{capped}");
    // With less than the input and 1024 output tokens can cost, no turn.
    let run2 = env.run();
    free_weekly(&env, spent(&env) + 100);
    let out = env.drive(&deps, &run2, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(
        env.run_row(&run2).reason.as_deref(),
        Some("credits_exhausted")
    );
    assert_eq!(env.api.requests().len(), 2);
}

#[tokio::test]
async fn an_unenforced_person_is_metered_and_never_stopped() {
    let env = Env::new().await;
    free_weekly(&env, 0);
    env.store
        .set_flag(&AuditCtx::DAEMON, OWNER, "claxbot_unenforced", true)
        .unwrap();
    assert_eq!(refusal(&env), None);
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let w = env.store.week_usage(OWNER, env.clock_now()).unwrap();
    assert!(w.exhausted && w.spent_milli > 0, "{w:?}");
    assert!(
        env.api
            .requests()
            .iter()
            .all(|r| r.body["max_tokens"] == 32000)
    );
}

#[tokio::test]
async fn a_flag_turned_off_stops_the_run_and_says_so() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.store
        .set_flag(&AuditCtx::DAEMON, OWNER, "claxbot", false)
        .unwrap();
    assert_eq!(refusal(&env), Some(Refusal::NoFlag));
    let out = env.tick(&deps, &run, &d).await;
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("no_flag"));
    let notice = env.replies()[0].1.clone();
    assert!(notice.contains("not turned on for you"), "{notice}");
}

#[tokio::test]
async fn the_step_cap_stops_a_run_and_says_so() {
    let env = Env::new().await;
    env.set(|s| s.max_steps = 2);
    env.api.push(env.script_turn(
        "tu_1",
        "Edit",
        "await clax.files.write('a.css', 'x'); return 1;",
    ));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        out.iter().filter_map(|o| o.kind).collect::<Vec<_>>(),
        [StepKind::Turn, StepKind::Script, StepKind::Stopped]
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    let notice = env.replies()[0].1.clone();
    assert!(notice.contains("limit of 2 steps"), "{notice}");
    assert!(
        notice.contains("I ran 1 script and edited a.css."),
        "{notice}"
    );
    assert!(notice.contains("draft is saved, unpublished"), "{notice}");
    assert_eq!(env.store.draft_files(&run).unwrap().len(), 1);
}

#[tokio::test]
async fn the_credit_and_time_caps_stop_a_run() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    assert!(env.run_row(&run).credits_milli > 0);
    env.set(|s| s.max_run_credits = 0);
    assert_eq!(
        env.tick(&deps, &run, &d).await.next,
        Next::Done(RunStatus::Stopped)
    );
    assert!(env.replies()[0].1.contains("limit of 0 credits"));

    let run = env.run();
    env.set(|s| s.max_run_credits = 200);
    env.clock.advance(31 * 60);
    assert_eq!(
        env.tick(&deps, &run, &d).await.next,
        Next::Done(RunStatus::Stopped)
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    assert!(env.replies()[1].1.contains("limit of 30 minutes"));
    // Less time left than a turn needs stops the run too.
    let run = env.run();
    env.clock.advance(29 * 60 + 30);
    assert_eq!(
        env.tick(&deps, &run, &d).await.next,
        Next::Done(RunStatus::Stopped)
    );
    assert!(env.replies()[2].1.contains("limit of 30 minutes"));
}
