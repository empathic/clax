//! A run's `state` across its scripts (spec
//! docs/superpowers/specs/2026-11-19-claxbot-browser-design.md §17.1):
//! kept with the script step that wrote it, the next script's to start
//! from, the same for a step executed again, capped, and told to the
//! model only when something was not kept.

use crate::common::*;
use crate::resume::crash_once;
use clax_bot::Next;
use clax_bot::run::CrashPoint;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use serde_json::{Value, json};

/// The `tool_result` contents of `run`'s script steps, in order.
fn results(env: &Env, run: &str) -> Vec<Value> {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    steps
        .iter()
        .filter(|s| s.kind == StepKind::Script)
        .map(|s| {
            let p = s.payload.as_ref().expect("committed");
            serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap()
        })
        .collect()
}

/// Runs `scripts` one after the other, then finishes; their results.
async fn scripts(env: &Env, scripts: &[&str]) -> Vec<Value> {
    const IDS: [&str; 4] = ["tu_0", "tu_1", "tu_2", "tu_3"];
    for (id, code) in IDS.iter().zip(scripts) {
        env.api.push(env.script_turn(id, "Step", code));
    }
    env.api.push(finish_turn("done"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    results(env, &run)
}

#[tokio::test]
async fn a_steps_state_is_the_next_scripts() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "const first = JSON.stringify(state); state.n = 1; state.seen = ['a']; return first;",
            "state.n += 1; return [state.n, state.seen];",
            "return state.n;",
        ],
    )
    .await;
    assert_eq!(r[0]["value"], "{}");
    assert_eq!(r[1]["value"], json!([2, ["a"]]));
    assert_eq!(r[2]["value"], 2);
}

/// A result says nothing of a state kept whole: it is the result it was
/// before `state`.
#[tokio::test]
async fn a_result_says_nothing_of_a_state_kept_whole() {
    let env = Env::new().await;
    let r = scripts(&env, &["state.a = { b: [1, 2] }; return 1;"]).await;
    assert!(r[0].get("state").is_none(), "{}", r[0]);
}

/// What a script left that state cannot keep is listed in its result,
/// and the rest is kept.
#[tokio::test]
async fn what_is_not_kept_is_listed_in_the_result() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "state.f = () => 1; state.n = 1; return 1;",
            "return [typeof state.f, state.n];",
        ],
    )
    .await;
    assert_eq!(
        r[0]["state"],
        json!({"dropped": [{"path": "state.f", "why": "a function"}]})
    );
    assert_eq!(r[1]["value"], json!(["undefined", 1]));
}

#[tokio::test]
async fn a_state_over_its_cap_leaves_out_its_largest_members() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "state.big = 'x'.repeat(70000); state.small = 1; return 1;",
            "return [typeof state.big, state.small];",
        ],
    )
    .await;
    let dropped = &r[0]["state"]["dropped"];
    assert_eq!(dropped[0]["path"], "state.big", "{}", r[0]);
    assert!(
        dropped[0]["why"]
            .as_str()
            .unwrap()
            .starts_with("too large (70002 bytes"),
        "{dropped}"
    );
    assert_eq!(r[1]["value"], json!(["undefined", 1]));
}

/// A script the sandbox stopped wrote no state: its result says so, and
/// the next script starts from the state before it.
#[tokio::test]
async fn a_stopped_script_keeps_the_last_state() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "state.n = 1; return 1;",
            "state.n = 2; const a = []; for (;;) a.push(new Array(100000).fill(1));",
            "return state.n;",
        ],
    )
    .await;
    assert_eq!(r[1]["error"]["kind"], "memory_limit", "{}", r[1]);
    assert_eq!(r[1]["state"]["saved"], false, "{}", r[1]);
    assert_eq!(r[2]["value"], 1);
}

/// A step executed again (its commit crashed) starts from the state the
/// last committed step kept, not from what its first attempt left.
#[tokio::test]
async fn a_step_executed_again_starts_from_the_committed_state() {
    let env = Env::new().await;
    let code = "state.n += 1; await clax.files.list(); return state.n;";
    env.api
        .push(env.script_turn("tu_1", "One", "state.n = 1; return 1;"));
    env.api.push(env.script_turn("tu_2", "Two", code));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 4,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let r = results(&env, &run);
    assert_eq!(r[1]["value"], 2, "{}", r[1]);
}

/// A state of tens of thousands of small members is fitted in one pass:
/// it keeps as many as fit, lists the rest, and the next script reads
/// what it kept.
#[tokio::test]
async fn a_state_of_many_small_members_keeps_as_many_as_fit() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "for (let i = 0; i < 40000; i++) state['k' + String(i).padStart(5, '0')] = 'x'.repeat(80); return 1;",
            "const k = Object.keys(state); return [k.length, JSON.stringify(state).length, k[0], k[k.length - 1]];",
        ],
    )
    .await;
    let told = &r[0]["state"];
    let listed = told["dropped"].as_array().unwrap().len() as u64;
    assert_eq!(listed, 20, "{told}");
    // Largest first, and among equals the last first.
    assert_eq!(told["dropped"][0]["path"], "state.k39999", "{told}");
    let kept = r[1]["value"][0].as_u64().unwrap();
    assert_eq!(kept + listed + told["more"].as_u64().unwrap(), 40000);
    // Each member is `"kNNNNN":"x…"` (91 bytes) and a comma; the stored
    // `{"value":…,"handles":[]}` keeps 15 bytes more than the value.
    let value_bytes = r[1]["value"][1].as_u64().unwrap();
    assert_eq!(value_bytes, 2 + kept * 92 - 1);
    let stored = value_bytes + 23;
    let cap = clax_bot::state::CAP_BYTES as u64;
    assert!(stored <= cap && stored + 92 > cap, "{stored} of {cap}");
    assert_eq!(r[1]["value"][2], "k00000");
    assert_eq!(r[1]["value"][3], format!("k{:05}", kept - 1));
}

/// A string cut inside a surrogate pair is kept, its lone half as U+FFFD,
/// with the rest of the state.
#[tokio::test]
async fn a_lone_surrogate_in_state_is_kept_well_formed() {
    let env = Env::new().await;
    let r = scripts(
        &env,
        &[
            "state.note = '\\u{1F600}x'.slice(0, 1); state.n = 1; return 1;",
            "return [state.note === '\\uFFFD', state.n];",
        ],
    )
    .await;
    assert!(r[0].get("state").is_none(), "{}", r[0]);
    assert_eq!(r[1]["value"], json!([true, 1]));
}
