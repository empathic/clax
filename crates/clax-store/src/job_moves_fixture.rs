//! `tests/fixtures/job-moves.json` holds [`may_move`]'s answer for every
//! state, move and question wait; the app's queue checks its own table
//! against the same file (`apps/clax_app/test/queue/job_moves_test.dart`),
//! so the app never plans a move the store refuses.

use crate::jobs::{JobState, Move, may_move};
use serde_json::{Value, json};

const STATES: [JobState; 8] = [
    JobState::Queued,
    JobState::Running,
    JobState::Waiting,
    JobState::Paused,
    JobState::Stopped,
    JobState::Failed,
    JobState::Cancelled,
    JobState::Done,
];

/// Every move, with the name the fixture gives it.
fn moves() -> Vec<(String, Move)> {
    let mut out: Vec<(String, Move)> = STATES
        .iter()
        .map(|s| (format!("to:{}", s.as_str()), Move::To(*s)))
        .collect();
    out.push(("ask".into(), Move::Ask));
    out.push(("close".into(), Move::Close));
    out.push(("step".into(), Move::Step { done: false }));
    out.push(("step:done".into(), Move::Step { done: true }));
    out
}

#[test]
fn the_shared_fixture_is_may_moves_table() {
    let file: Value = serde_json::from_str(include_str!("../tests/fixtures/job-moves.json"))
        .expect("job-moves.json parses");
    let states: Vec<&str> = STATES.iter().map(|s| s.as_str()).collect();
    assert_eq!(file["states"], json!(states));
    let mut want = Vec::new();
    for from in STATES {
        for (by, m) in moves() {
            for waits in [false, true] {
                want.push(json!({
                    "from": from.as_str(),
                    "by": by,
                    "waits": waits,
                    "to": may_move(from, m, waits).map(JobState::as_str),
                }));
            }
        }
    }
    assert_eq!(
        file["moves"],
        Value::Array(want.clone()),
        "tests/fixtures/job-moves.json differs from may_move; its moves should be:\n{}",
        serde_json::to_string(&want).unwrap()
    );
}
