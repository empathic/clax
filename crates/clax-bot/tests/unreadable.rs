//! A run one of whose own rows, as the store keeps it, does not read (a
//! step's payload or observations, its parking, an entry of its journal)
//! ends `failed` (`history_unreadable`) at its next tick, or stops at a
//! cancel: it never stays live, and nothing else fails with it.

use crate::common::*;
use clax_bot::Next;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};

/// Runs `sql` on the environment's database, its `?1` the run.
fn damage(env: &Env, run: &str, sql: &str) {
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let changed = c.execute(sql, rusqlite::params![run]).unwrap();
    assert_eq!(changed, 1, "{sql}");
}

/// A run whose first turn asked for a script, its turn committed by
/// the driver it returns.
async fn after_a_turn(env: &Env) -> (String, DriverId) {
    env.api
        .push(env.script_turn("tu_1", "Read", "return await clax.threads.read();"));
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let driver = DriverId::mint();
    let out = env.tick(&env.deps(), &run, &driver).await;
    assert_eq!(out.kind, Some(StepKind::Turn));
    (run, driver)
}

#[tokio::test]
async fn a_run_whose_step_or_parking_does_not_read_ends_failed_at_its_next_tick() {
    for sql in [
        "UPDATE bot_steps SET payload_json = '{\"bl' WHERE run_id = ?1 AND n = 1",
        "UPDATE bot_steps SET observed_json = 'nope' WHERE run_id = ?1 AND n = 1",
        "UPDATE bot_runs SET waiting_on = '{' WHERE id = ?1",
    ] {
        let env = Env::new().await;
        let (run, driver) = after_a_turn(&env).await;
        let (other, other_driver) = after_a_turn(&env).await;
        damage(&env, &run, sql);
        let out = env.drive(&env.deps(), &run, &driver).await;
        assert_eq!(
            (out.len(), &out[0].next, out[0].kind),
            (1, &Next::Done(RunStatus::Failed), Some(StepKind::Failed)),
            "{sql}"
        );
        let row = env.run_row(&run);
        assert_eq!(row.reason.as_deref(), Some("history_unreadable"), "{sql}");
        assert!(row.ended_at.is_some() && row.claimed_by.is_none(), "{sql}");
        // Its threads are told; a run beside it goes on to its end.
        assert!(
            env.replies()
                .iter()
                .any(|(_, text)| text.contains("could not read what I had done")),
            "{sql}: {:?}",
            env.replies()
        );
        let out = env.drive(&env.deps(), &other, &other_driver).await;
        assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
        // Views of it read, without the step that does not.
        let (_, steps, left_out) = env.store.run_with_readable_steps(&run).unwrap().unwrap();
        assert_eq!(
            (steps.last().unwrap().kind, left_out),
            (StepKind::Failed, usize::from(!sql.contains("waiting_on"))),
            "{sql}"
        );
    }
}

/// An entry of the open step's journal that does not read ends the run
/// at the step that reads it.
#[tokio::test]
async fn a_run_whose_journal_does_not_read_ends_failed_at_the_step_that_reads_it() {
    let env = Env::new().await;
    let (run, driver) = after_a_turn(&env).await;
    damage(
        &env,
        &run,
        "INSERT INTO bot_calls (run_id, step_n, seq, attempt, op, args_hash, ok, result_json, at)
         VALUES (?1, 2, 0, 1, 'clax.threads.read', 'h', 1, '{\"thr', '2026-10-21T10:00:00Z')",
    );
    let out = env.drive(&env.deps(), &run, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Failed));
    let row = env.run_row(&run);
    assert_eq!(row.reason.as_deref(), Some("history_unreadable"));
}

/// A cancel of such a run completes: it stops at its next tick.
#[tokio::test]
async fn a_cancel_of_a_run_whose_step_does_not_read_stops_it() {
    let env = Env::new().await;
    let (run, driver) = after_a_turn(&env).await;
    damage(
        &env,
        &run,
        "UPDATE bot_steps SET payload_json = '[' WHERE run_id = ?1 AND n = 1",
    );
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &run)
        .unwrap();
    let out = env.drive(&env.deps(), &run, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("cancelled"));
}
