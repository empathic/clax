//! How a run ends, one golden an ending ([`Pins::ending`]): `finish` and
//! when its summary and sources are posted, `give_up`, the stops and a
//! failure.

use super::names as g;
use super::pins::{
    DRAFT_ONLY, GIVEN, PUBLISH, Pins, driven, fetched, give_up_turn, kinds, sender_says, spent,
};
use crate::common::*;
use crate::resume::crash_once;
use crate::runs::EDIT;
use clax_bot::run::CrashPoint;
use clax_bot::{Next, Wait};
use clax_bot_sandbox::HostError;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::credits::KeySource;
use clax_store::roles::OWNER;
use serde_json::json;

/// Gives every person `milli` millicredits a week.
fn free_weekly(env: &Env, milli: i64) {
    env.admin_sets(&[(Setting::FreeWeeklyMilli, json!(milli))]);
}

// ---------------------------------------------------------------------------
// finish: when the summary is posted
// ---------------------------------------------------------------------------

// The rule is the run's, not a thread's: the summary is posted on every
// thread of the run when no thread was replied on since the last draft
// write or publish, and on none otherwise. The pages a run fetched are
// listed after the summary, or alone, one place after the summary's,
// when the summary is not posted.

/// A run that never replied posts its summary on each of its threads.
#[tokio::test]
async fn finish_posts_its_summary_on_every_thread_of_a_run_that_did_not_reply() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", PUBLISH));
    env.api.push(finish_turn("Made the heading blue."));
    let run = env.run_on_both_threads("");
    driven(env, run)
        .await
        .pin_ending(g::ENDING_FINISH_NO_REPLY_TWO_THREADS);
}

/// A reply before the last draft write does not count: the summary is
/// posted after it.
#[tokio::test]
async fn finish_posts_its_summary_when_the_only_reply_came_before_a_draft_write() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Plan",
        r#"await clax.threads.reply("T1", "I will make the heading blue."); return 1;"#,
    ));
    env.api.push(env.script_turn("tu_2", "Edit", DRAFT_ONLY));
    env.api.push(finish_turn("Wrote the stylesheet."));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_FINISH_REPLIED_BEFORE_A_DRAFT_WRITE);
}

/// Nor does a reply before the last publish, though the script that
/// published changed no file: one script edits, the next replies, a third
/// publishes the draft as it is, and the summary is posted.
#[tokio::test]
async fn finish_posts_its_summary_when_the_only_reply_came_before_a_publish() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    env.api.push(env.script_turn(
        "tu_2",
        "Say so",
        r#"await clax.threads.reply("T1", "The stylesheet is written; publishing next."); return 1;"#,
    ));
    env.api.push(env.script_turn(
        "tu_3",
        "Publish",
        r#"return (await clax.draft.publish({note: "A stylesheet", addresses: []})).version;"#,
    ));
    env.api.push(finish_turn("Published the stylesheet."));
    let run = env.run();
    let p = driven(env, run).await;
    let steps = p.steps(&p.env.api.requests()[0].body);
    let third = &steps[5]["payload"];
    assert_eq!(
        (&third["changed"], &third["published"]),
        (&json!([]), &json!({"version": 2})),
        "the third script published and changed no file"
    );
    p.pin_ending(g::ENDING_FINISH_REPLIED_BEFORE_A_PUBLISH);
}

/// A run, on its first thread or on both, that fetched [`GIVEN`] in a
/// script ending with `tail`, then finished.
async fn fetched_then(tail: &str, both_threads: bool) -> Pins {
    let env = Env::new().await;
    env.web().page(GIVEN, fetched(GIVEN));
    sender_says(&env, &format!("Read {GIVEN}"));
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        &format!("await clax.web.fetch({GIVEN:?}); {tail}"),
    ));
    env.api.push(finish_turn("Read the page."));
    let run = if both_threads {
        env.run_on_both_threads("fetch")
    } else {
        env.run_with_web("fetch")
    };
    driven(env, run).await
}

/// The pages a run fetched are listed after its summary.
#[tokio::test]
async fn finish_lists_the_sources_after_its_summary() {
    fetched_then("", false)
        .await
        .pin_ending(g::ENDING_FINISH_NO_REPLY_SOURCES);
}

/// On a run of two threads, the summary with its sources is posted on
/// both.
#[tokio::test]
async fn finish_lists_the_sources_after_its_summary_on_every_thread() {
    fetched_then("", true)
        .await
        .pin_ending(g::ENDING_FINISH_NO_REPLY_TWO_THREADS_SOURCES);
}

/// A run that replied itself gets its sources alone, one place after the
/// summary's.
#[tokio::test]
async fn finish_posts_the_sources_alone_when_the_run_replied() {
    fetched_then(r#"await clax.threads.reply("T1", "Mine.");"#, false)
        .await
        .pin_ending(g::ENDING_FINISH_REPLIED_SOURCES);
}

/// A run on two threads that replied on the first alone posts its summary
/// on neither.
#[tokio::test]
async fn finish_posts_no_summary_on_any_thread_when_the_run_replied_on_one() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", EDIT));
    env.api.push(finish_turn("Made the heading blue."));
    let run = env.run_on_both_threads("");
    driven(env, run)
        .await
        .pin_ending(g::ENDING_FINISH_REPLIED_ON_ONE_OF_TWO_THREADS);
}

/// The same with a fetched page: the sources are posted alone on both
/// threads, at the places after the summary's.
#[tokio::test]
async fn finish_posts_the_sources_alone_on_every_thread_when_the_run_replied_on_one() {
    fetched_then(r#"await clax.threads.reply("T1", "Mine.");"#, true)
        .await
        .pin_ending(g::ENDING_FINISH_REPLIED_ON_ONE_OF_TWO_THREADS_SOURCES);
}

/// An empty summary is posted on no thread, though the run did not reply.
#[tokio::test]
async fn finish_posts_no_empty_summary() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", PUBLISH));
    env.api.push(finish_turn(""));
    let run = env.run_on_both_threads("");
    driven(env, run)
        .await
        .pin_ending(g::ENDING_FINISH_EMPTY_SUMMARY_TWO_THREADS);
}

// ---------------------------------------------------------------------------
// give_up
// ---------------------------------------------------------------------------

/// The model gives up after a script that edited the draft.
#[tokio::test]
async fn a_model_that_gives_up_says_why_and_what_it_did() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    env.api
        .push(give_up_turn("The page has no heading to colour."));
    let run = env.run();
    driven(env, run).await.pin_ending(g::ENDING_GIVE_UP_MODEL);
}

/// A run on two threads that gives up says so on both.
#[tokio::test]
async fn a_run_on_two_threads_that_gives_up_says_so_on_both() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    env.api
        .push(give_up_turn("The page has no heading to colour."));
    let run = env.run_on_both_threads("");
    driven(env, run)
        .await
        .pin_ending(g::ENDING_GIVE_UP_TWO_THREADS);
}

/// What the notice says of scripts that edited nothing: one, then two.
#[tokio::test]
async fn a_notice_counts_the_scripts_that_ran() {
    for (n, golden) in [
        (1, g::ENDING_GIVE_UP_AFTER_ONE_SCRIPT_WITHOUT_EDITS),
        (2, g::ENDING_GIVE_UP_AFTER_TWO_SCRIPTS_WITHOUT_EDITS),
    ] {
        let env = Env::new().await;
        for id in ["tu_1", "tu_2"].into_iter().take(n) {
            env.api.push(env.script_turn(id, "Look", "return 1;"));
        }
        env.api.push(give_up_turn("Nothing to change."));
        let run = env.run();
        driven(env, run).await.pin_ending(golden);
    }
}

/// Two turns in a row without a tool call: the nudge, then the run gives
/// up.
#[tokio::test]
async fn a_model_that_stops_calling_tools_gives_up() {
    let env = Env::new().await;
    env.api.push(text_turn("The heading is fine."));
    env.api.push(text_turn("Really."));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_GIVE_UP_NO_TOOL_CALL);
}

/// The provider's `refusal` stop reason.
#[tokio::test]
async fn a_model_that_declines_gives_up() {
    let env = Env::new().await;
    env.api
        .push(stream(&[B::Text("I cannot help.")], "refusal", USAGE));
    let run = env.run();
    driven(env, run).await.pin_ending(g::ENDING_GIVE_UP_REFUSAL);
}

/// The provider's `model_context_window_exceeded` stop reason.
#[tokio::test]
async fn a_conversation_past_the_context_window_gives_up() {
    let env = Env::new().await;
    env.api.push(stream(
        &[B::Text("…")],
        "model_context_window_exceeded",
        USAGE,
    ));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_GIVE_UP_CONTEXT_WINDOW);
}

// ---------------------------------------------------------------------------
// stopped and failed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spent_credits_stop_a_run_before_it_starts() {
    let env = Env::new().await;
    free_weekly(&env, 0);
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_CREDITS_EXHAUSTED_BEFORE_THE_FIRST_TURN);
}

/// Credits spent after a script that edited and published: the notice
/// says what was done.
#[tokio::test]
async fn credits_spent_mid_run_stop_it_saying_what_it_did() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", PUBLISH));
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
    free_weekly(&env, spent(&env));
    assert_eq!(
        kinds(&env.drive(&deps, &run, &d).await),
        [StepKind::Stopped]
    );
    Pins::new(env, run).pin_ending(g::ENDING_STOPPED_CREDITS_EXHAUSTED_AFTER_PUBLISHING);
}

#[tokio::test]
async fn the_step_cap_stops_a_run() {
    let env = Env::new().await;
    env.set(|s| s.max_steps = 2);
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_RUN_CAP_STEPS);
}

/// A run on two threads stopped at a cap says so on both.
#[tokio::test]
async fn a_cap_stops_a_run_on_two_threads_saying_so_on_both() {
    let env = Env::new().await;
    env.set(|s| s.max_steps = 2);
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    let run = env.run_on_both_threads("");
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_RUN_CAP_TWO_THREADS);
}

#[tokio::test]
async fn the_credit_cap_stops_a_run() {
    let env = Env::new().await;
    env.set(|s| s.max_run_credits = 0);
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_RUN_CAP_CREDITS);
}

#[tokio::test]
async fn the_time_cap_stops_a_run() {
    let env = Env::new().await;
    let run = env.run();
    env.clock.advance(31 * 60);
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_RUN_CAP_MINUTES);
}

#[tokio::test]
async fn a_model_error_stops_a_run() {
    let env = Env::new().await;
    env.api.push(Reply::Status(
        400,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad request"}}"#
            .into(),
    ));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_MODEL_ERROR);
}

#[tokio::test]
async fn a_rejected_key_stops_a_run() {
    let env = Env::new().await;
    env.api.push(Reply::Status(
        401,
        r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#.into(),
    ));
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_KEY_REJECTED);
}

#[tokio::test]
async fn no_key_stops_a_run() {
    let mut env = Env::new().await;
    env.instance_key = false;
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_NOT_CONFIGURED);
}

#[tokio::test]
async fn a_person_without_the_flag_is_stopped() {
    let env = Env::new().await;
    let run = env.run();
    env.store
        .set_flag(&AuditCtx::DAEMON, OWNER, "claxbot", false)
        .unwrap();
    driven(env, run).await.pin_ending(g::ENDING_STOPPED_NO_FLAG);
}

#[tokio::test]
async fn an_instance_that_turned_claxbot_off_stops_a_run() {
    let env = Env::new().await;
    let run = env.run();
    env.set(|s| s.enabled = false);
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_DISABLED);
}

/// A stop that takes the step of a script no attempt committed records
/// each call the script's journal holds, once, as a `bot.call` with its
/// attempt and place, and records no call for the notice it posts
/// itself, whose row is in the same step's journal.
#[tokio::test]
async fn a_stop_records_the_calls_of_a_script_that_never_committed_and_not_its_own_notice() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
    let run = env.run();
    let mut dies = env.deps();
    crash_once(
        &mut dies,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&dies, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    env.set(|s| s.enabled = false);
    env.clock.advance(61);
    let p = driven(env, run).await;
    let net = p.ending_net();
    let calls: Vec<&serde_json::Value> = net["audit"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "bot.call")
        .collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        (&calls[0]["body"]["op"], &calls[0]["body"]["seq"]),
        (&json!("files.write"), &json!(0))
    );
    p.pin_ending(g::ENDING_STOPPED_DISABLED_AFTER_A_SCRIPT_THAT_NEVER_COMMITTED);
}

/// A cancelled run posts nothing.
#[tokio::test]
async fn a_cancelled_run_stops_without_a_notice() {
    let env = Env::new().await;
    let run = env.run();
    env.store
        .cancel_run(&clax_bot::run::bot_ctx(OWNER), &run)
        .unwrap();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_CANCELLED);
}

/// A run whose request cannot be read (its thread is gone) posts nothing.
#[tokio::test]
async fn a_run_whose_request_is_gone_stops_without_a_notice() {
    let env = Env::new().await;
    env.host.lock().fail_next.insert(
        "context".into(),
        HostError::new("not_found", "no thread T1"),
    );
    let run = env.run();
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_REQUEST_GONE);
}

#[tokio::test]
async fn an_unpriced_model_stops_a_run_when_a_price_is_required() {
    let env = Env::new().await;
    let run = env.run_on("acme-model-1", KeySource::Instance);
    env.admin_sets(&[(Setting::RequireExplicitPrice, json!(true))]);
    driven(env, run)
        .await
        .pin_ending(g::ENDING_STOPPED_MODEL_NOT_PRICED);
}

/// A run (`run` makes it) whose first step cannot be committed three
/// times, and so ends `failed`.
async fn failed(run: impl FnOnce(&Env) -> String) -> Pins {
    let env = Env::new().await;
    // Without its cost line, every step's commit is refused.
    env.store
        .delete_cost_line(&AuditCtx::DAEMON, "do.requests")
        .unwrap();
    env.set(|s| s.infra_lines = true);
    for _ in 0..3 {
        env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    }
    let run = run(&env);
    let deps = env.deps();
    let d = DriverId::mint();
    for _ in 0..3 {
        assert!(env.try_tick(&deps, &run, &d, None).await.is_err());
    }
    let out = env.tick(&deps, &run, &d).await;
    assert_eq!(out.next, Next::Done(RunStatus::Failed));
    Pins::new(env, run)
}

/// A step that cannot be committed three times ends the run `failed`.
#[tokio::test]
async fn a_run_that_cannot_save_its_work_fails() {
    let p = failed(Env::run).await;
    p.pin_all([
        p.ending_pinned(g::ENDING_FAILED_INTERNAL_ERROR),
        p.ending_net_pinned(g::ENDING_FAILED_INTERNAL_ERROR),
        p.json(
            "the audit records of a run that failed",
            g::AUDIT_FAILED,
            &p.audit(),
        ),
    ]);
}

/// A run on two threads that fails says so on both.
#[tokio::test]
async fn a_run_on_two_threads_that_fails_says_so_on_both() {
    failed(|env| env.run_on_both_threads(""))
        .await
        .pin_ending(g::ENDING_FAILED_TWO_THREADS);
}
