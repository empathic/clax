//! The histories the run loop's folds are tested on, for the library's
//! unit tests alone: the hand-built steps the folds' own tests share
//! ([`step`], [`open`], [`a_run`]), every step the goldens of
//! `tests/golden/` hold, and histories generated from them (shuffled,
//! cut, left open, begun again, and with payload fields removed or
//! replaced by values of other types) or written as a tick writes a run.
//!
//! Its tests are of [`crate::history`]'s folds over those histories:
//! that neither reads a payload field of Clax's surface
//! (`the_run_loops_folds_read_nothing_clax_keeps_in_a_payload`), and
//! that [`transcript`] keeps the invariants [`check`] holds a history
//! rule to (`the_transcript_of_a_run_keeps_what_a_turn_relies_on`). What
//! each fold gives for a history is held by the folds' own tests
//! (`history::tests`, `clax::tally::tests`).

use crate::clax::Tally;
use crate::history::{Control, check, transcript};
use clax_sample::messages::{Block, Role};
use clax_store::bot_runs::{Step, StepKind};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Hand-built steps
// ---------------------------------------------------------------------------

/// Step `n` of kind `kind`, committed on its first attempt with
/// `payload`.
pub(crate) fn step(n: i64, kind: StepKind, payload: Value) -> Step {
    Step {
        run_id: "r".into(),
        n,
        kind,
        started_at: "2026-10-21T10:00:00.000Z".into(),
        attempt: 1,
        committed_at: Some("2026-10-21T10:00:00.000Z".into()),
        payload: Some(payload),
        observed: Vec::new(),
        failures: 0,
    }
}

/// Step `n` of kind `kind`, begun `attempt` times and not committed.
pub(crate) fn open(n: i64, kind: StepKind, attempt: i64) -> Step {
    Step {
        attempt,
        committed_at: None,
        payload: None,
        observed: vec![json!({"attempt": attempt, "input_tokens": 5})],
        ..step(n, kind, Value::Null)
    }
}

/// A run of five committed steps: a first turn that asks for a script;
/// the script, committed on its second attempt, which edited, published
/// version 2 and replied; a turn that called no tool and is nudged; a
/// turn, committed on its third attempt, whose context window was full;
/// and the give-up that follows.
pub(crate) fn a_run() -> Vec<Step> {
    let call = json!({"title": "Edit", "code": "return 1;"});
    let mut script = step(
        2,
        StepKind::Script,
        json!({
            "tool_use_id": "tu_1", "title": "Edit", "code": "return 1;",
            "result": Block::tool_result("tu_1", "{\"value\":1}", false).to_json(),
            "changed": ["index.html"], "replied": ["t1"],
            "published": {"version": 2}, "created": false, "base": 2,
        }),
    );
    script.attempt = 2;
    let mut full = step(
        4,
        StepKind::Turn,
        json!({
            "assistant": [Block::text("too long").to_json()],
            "stop_reason": "model_context_window_exceeded",
            "next": {"kind": "give_up", "code": "context_window", "reason": "full"},
        }),
    );
    full.attempt = 3;
    vec![
        step(
            1,
            StepKind::Turn,
            json!({
                "system": "the system text",
                "user": [Block::text("the request").to_json()],
                "assistant": [
                    Block::text("Let me look.").to_json(),
                    {"type": "tool_use", "id": "tu_1", "name": "run_script", "input": call},
                ],
                "next": {"kind": "script", "tool_use_id": "tu_1",
                         "title": "Edit", "code": "return 1;"},
                "usage": {"input_tokens": 1000, "cache_read_input_tokens": 2000,
                          "cache_creation_input_tokens": 500, "output_tokens": 300},
                // Not a turn's: no fold may read them here.
                "changed": ["not.a.turns"], "replied": ["t0"], "created": true,
            }),
        ),
        script,
        step(
            3,
            StepKind::Turn,
            json!({
                "assistant": [Block::text("I am done.").to_json()],
                "next": {"kind": "turn", "nudge": true,
                         "user": [Block::text("nudge").to_json()]},
                "usage": {"input_tokens": 7, "billed_micro_usd": 42},
            }),
        ),
        full,
        step(
            5,
            StepKind::GiveUp,
            json!({"code": "context_window", "reason": "full", "notice": "I gave up: full"}),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Generated histories
// ---------------------------------------------------------------------------

const KINDS: [(&str, StepKind); 6] = [
    ("turn", StepKind::Turn),
    ("script", StepKind::Script),
    ("finish", StepKind::Finish),
    ("give_up", StepKind::GiveUp),
    ("stopped", StepKind::Stopped),
    ("failed", StepKind::Failed),
];

/// The step the golden value `v` is, when it is one: an object with a
/// step's `kind` and a `payload`.
fn as_step(v: &Value) -> Option<Step> {
    let kind = v["kind"].as_str()?;
    let (_, kind) = KINDS.iter().find(|(name, _)| *name == kind)?;
    let payload = v.as_object()?.get("payload")?;
    let mut s = step(v["n"].as_i64().unwrap_or(1), *kind, payload.clone());
    s.attempt = v["attempt"].as_i64().unwrap_or(1);
    Some(s)
}

/// Every step anywhere in the golden value `v`, in the order written.
fn steps_in(v: &Value, out: &mut Vec<Step>) {
    if let Some(s) = as_step(v) {
        out.push(s);
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| steps_in(x, out)),
        Value::Object(o) => o.values().for_each(|x| steps_in(x, out)),
        _ => {}
    }
}

/// The goldens of `tests/golden/`, by file name, parsed.
fn goldens() -> Vec<(String, Value)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|f| f.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        // A wire golden is a request body's bytes with IDs replaced, not
        // always JSON a parser takes, and holds no step.
        .filter(|n| !n.ends_with(".wire.json"))
        .map(|n| {
            let text = std::fs::read_to_string(dir.join(&n)).unwrap();
            let v = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{n}: {e}"));
            (n, v)
        })
        .collect()
}

/// A fixed sequence of numbers (xorshift64*), so a failure is the same
/// on every run.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number under `n`.
    fn under(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// `s` with one thing about it changed, as `r` picks: left open, begun
/// again, of another kind, or with a payload field removed, replaced by
/// a value of another type, or taken from another step.
fn disturbed(mut s: Step, pool: &[Step], r: &mut Numbers) -> Step {
    let odd = [
        Value::Null,
        json!(7),
        json!(u64::MAX),
        json!("x"),
        json!(true),
        json!([]),
        json!(["a", "b", 3]),
        json!({"version": 3}),
        json!({"version": "3", "kind": "turn", "nudge": true, "user": [{"type": "text", "text": "t"}]}),
        json!({"kind": "turn", "user": "x"}),
        json!({"input_tokens": 3, "billed_micro_usd": 0}),
        json!({"input_tokens": "3", "output_tokens": 2.5, "billed_micro_usd": "0"}),
        json!([{"type": "text", "text": "t"}, {"type": "tool_use", "id": "i"}, 4]),
    ];
    let keys = [
        "system",
        "user",
        "assistant",
        "next",
        "usage",
        "result",
        "changed",
        "replied",
        "published",
        "created",
        "base",
    ];
    match r.under(8) {
        0 => {
            s.committed_at = None;
            if r.under(2) == 0 {
                s.payload = None;
            }
        }
        1 => s.attempt = 2 + r.under(3) as i64,
        2 => s.kind = KINDS[r.under(KINDS.len())].1,
        3 => s.payload = None,
        4 => {
            if let Some(o) = s.payload.as_mut().and_then(Value::as_object_mut) {
                o.remove(keys[r.under(keys.len())]);
            }
        }
        5 | 6 => {
            if let Some(o) = s.payload.as_mut().and_then(Value::as_object_mut) {
                o.insert(
                    keys[r.under(keys.len())].to_string(),
                    odd[r.under(odd.len())].clone(),
                );
            }
        }
        _ => {
            let key = keys[r.under(keys.len())];
            let from = pool[r.under(pool.len())]
                .payload
                .clone()
                .unwrap_or_default();
            if let (Some(o), Some(v)) = (
                s.payload.as_mut().and_then(Value::as_object_mut),
                from.get(key),
            ) {
                o.insert(key.to_string(), v.clone());
            }
        }
    }
    s
}

/// The runs pinned whole in the goldens, by file name: a thread run, a
/// brief run and a run that uses the web.
fn pinned_runs() -> Vec<(String, Vec<Step>)> {
    let runs: Vec<(String, Vec<Step>)> = goldens()
        .into_iter()
        .filter(|(name, _)| name.starts_with("steps."))
        .map(|(name, v)| {
            let mut steps = Vec::new();
            steps_in(&v, &mut steps);
            assert!(steps.len() >= 3, "{name} holds a run's steps");
            assert_eq!(steps[0].kind, StepKind::Turn, "{name}");
            (name, steps)
        })
        .collect();
    assert_eq!(
        runs.len(),
        3,
        "the thread run, the brief run and the web run"
    );
    runs
}

/// The steps generated histories are drawn from: the hand-built ones and
/// every step the goldens hold.
fn seeds() -> Vec<Step> {
    let mut pool = a_run();
    // A cancelled script's stopped step, with what the script left.
    pool.push(step(
        3,
        StepKind::Stopped,
        json!({"reason": "cancelled", "notice": null, "title": "t", "code": "c",
               "published": {"version": 3}, "replied": ["t9"], "base": 3}),
    ));
    // A turn the provider billed nothing for.
    pool.push(step(
        2,
        StepKind::Turn,
        json!({"assistant": [Block::text("free").to_json()],
               "next": {"kind": "turn", "user": []},
               "usage": {"input_tokens": 3, "billed_micro_usd": 0}}),
    ));
    for (_, v) in goldens() {
        steps_in(&v, &mut pool);
    }
    pool
}

/// 4000 histories of up to 12 steps drawn from `pool`, one step in three
/// disturbed ([`disturbed`]): the same on every run. Most are no run a
/// tick could write.
fn generated(pool: &[Step]) -> Vec<Vec<Step>> {
    let mut r = Numbers(0x9E37_79B9_7F4A_7C15);
    (0..4000)
        .map(|_| {
            let len = r.under(13);
            (0..len)
                .map(|_| {
                    let s = pool[r.under(pool.len())].clone();
                    if r.under(3) == 0 {
                        disturbed(s, pool, &mut r)
                    } else {
                        s
                    }
                })
                .collect()
        })
        .collect()
}

/// The seeds hold every kind of step and each fact a fold reads, so a
/// golden that loses its only `created: true` step fails here with that
/// message rather than leaving the generated histories without one.
#[test]
fn the_seeds_hold_every_kind_of_step_and_each_fact_a_fold_reads() {
    let pool = seeds();
    for (name, kind) in KINDS {
        assert!(pool.iter().any(|s| s.kind == kind), "no {name} step");
    }
    let has = |f: &dyn Fn(&Value) -> bool| pool.iter().any(|s| s.payload.as_ref().is_some_and(f));
    assert!(pool.len() >= 60, "{} steps in the goldens", pool.len());
    assert!(has(&|p| p["next"]["nudge"] == true), "a nudge");
    assert!(has(&|p| p["next"]["code"] == "context_window"));
    assert!(has(&|p| p["usage"]["billed_micro_usd"] == 0), "billed 0");
    assert!(has(&|p| p["base"].is_u64()), "a moved base");
    assert!(has(&|p| p["published"]["version"].is_u64()), "a publish");
    assert!(has(&|p| p["created"] == true), "a created artifact");
    assert!(has(&|p| p["replied"]
        .as_array()
        .is_some_and(|a| !a.is_empty())));
    assert!(has(&|p| p["changed"]
        .as_array()
        .is_some_and(|a| !a.is_empty())));
    assert!(pool.iter().any(|s| s.attempt > 1), "a step begun again");
}

// ---------------------------------------------------------------------------
// The run loop's folds alone
// ---------------------------------------------------------------------------

/// The payload fields Clax's own surface keeps in a step
/// (`crates/clax-bot/src/clax/session.rs`, what a session's `finish`
/// answers for the script step's payload).
const CLAX_FIELDS: [&str; 5] = ["changed", "replied", "published", "created", "base"];

/// `steps` with every payload's [`CLAX_FIELDS`] taken out.
fn without_clax(steps: &[Step]) -> Vec<Step> {
    let mut steps = steps.to_vec();
    for o in steps
        .iter_mut()
        .filter_map(|s| s.payload.as_mut().and_then(Value::as_object_mut))
    {
        for key in CLAX_FIELDS {
            o.remove(key);
        }
    }
    steps
}

/// The run loop's folds know no noun of Clax's by what they read: the
/// control state and the transcript of a history are the same with and
/// without the fields Clax's surface keeps in its payloads, however a
/// read of one might be spelt.
#[test]
fn the_run_loops_folds_read_nothing_clax_keeps_in_a_payload() {
    let pool = seeds();
    let mut histories = generated(&pool);
    for (_, steps) in pinned_runs() {
        histories.extend((0..=steps.len()).map(|n| steps[..n].to_vec()));
    }
    let (mut stripped, mut odd) = (0, 0);
    for (i, steps) in histories.iter().enumerate() {
        let bare = without_clax(steps);
        stripped += usize::from(bare != *steps);
        // A field of the surface's in a step that is no script, where a
        // read of it would be easiest to miss.
        odd += usize::from(steps.iter().any(|s| {
            s.kind == StepKind::Turn
                && s.committed_at.is_some()
                && s.payload
                    .as_ref()
                    .is_some_and(|p| CLAX_FIELDS.iter().any(|k| !p[k].is_null()))
        }));
        assert_eq!(
            Control::of(&bare),
            Control::of(steps),
            "history {i}: the control state read a field of Clax's surface: {steps:#?}"
        );
        assert_eq!(
            transcript(&bare),
            transcript(steps),
            "history {i}: the transcript read a field of Clax's surface: {steps:#?}"
        );
        // And the tally reads them: the fields are not dead.
        if Tally::of(steps) != Tally::of(&bare) {
            assert_ne!(bare, *steps);
        }
    }
    assert!(stripped > 2000, "{stripped} histories held a field");
    assert!(odd > 300, "{odd} histories held one in a committed turn");
    let tallied = histories
        .iter()
        .filter(|h| Tally::of(h) != Tally::of(&without_clax(h)))
        .count();
    assert!(tallied > 1000, "{tallied} histories whose tally reads one");
}

/// A run as a tick writes one, as `r` picks: a first turn with the
/// system text and the request, then what each turn's `next` asks for
/// (a script and its result, a turn answered with a nudge, an error
/// result for each call, or nothing after a pause), to a `finish`, a
/// `give_up` or a stop.
fn a_written_run(r: &mut Numbers) -> Vec<Step> {
    let text = |s: String| Block::text(s).to_json();
    let mut run: Vec<Step> = Vec::new();
    let (mut nudged, mut calls) = (false, 0);
    loop {
        let n = run.len() as i64 + 1;
        if n > 1 && (n > 14 || r.under(12) == 0) {
            run.push(step(
                n,
                StepKind::Stopped,
                json!({"reason": "run_cap", "notice": "n"}),
            ));
            break;
        }
        calls += 2;
        let (id, other) = (format!("tu_{calls}"), format!("tu_{}", calls + 1));
        let call = |id: &str, name: &str| json!({"type": "tool_use", "id": id, "name": name, "input": {"code": "c"}});
        let refused = |id: &str| Block::tool_result(id, "nothing ran", true).to_json();
        let said = text(format!("words {n}"));
        let (assistant, next) = match r.under(11) {
            0..=3 => (
                vec![said, call(&id, "run_script")],
                json!({"kind": "script", "tool_use_id": id, "title": "t", "code": "c"}),
            ),
            4 => (
                vec![call(&id, "finish")],
                json!({"kind": "finish", "summary": "s"}),
            ),
            5 => (
                vec![said, call(&id, "give_up")],
                json!({"kind": "give_up", "code": "model", "reason": "r"}),
            ),
            6 if nudged => (
                vec![said],
                json!({"kind": "give_up", "code": "no_tool_call", "reason": "r"}),
            ),
            6 => (
                vec![said],
                json!({"kind": "turn", "nudge": true, "user": [text("nudge".into())]}),
            ),
            7 => (vec![said], json!({"kind": "turn", "user": []})),
            8 => (
                vec![call(&id, "run_script")],
                json!({"kind": "turn", "user": [refused(&id)]}),
            ),
            9 => (
                vec![said, call(&id, "run_script"), call(&other, "run_script")],
                json!({"kind": "turn", "user": [refused(&id), refused(&other)]}),
            ),
            _ => (
                vec![said],
                json!({"kind": "give_up", "code": "context_window", "reason": "r"}),
            ),
        };
        nudged = next["nudge"] == true;
        let mut payload = json!({"assistant": assistant, "next": next,
                                 "usage": {"input_tokens": 10 * n}});
        if n == 1 {
            payload["system"] = json!("the system text");
            payload["user"] = json!([text("the request".into())]);
        }
        let mut turn = step(n, StepKind::Turn, payload);
        turn.attempt = 1 + r.under(3) as i64;
        run.push(turn);
        match next["kind"].as_str() {
            Some("script") => run.push(step(
                n + 1,
                StepKind::Script,
                json!({"tool_use_id": id, "title": "t", "code": "c",
                       "result": Block::tool_result(&id, "{\"value\":1}", r.under(4) == 0).to_json(),
                       "changed": ["index.html"], "replied": [], "published": null,
                       "created": false, "base": null}),
            )),
            Some("finish") => {
                run.push(step(n + 1, StepKind::Finish, json!({"summary": "s", "posted": true})));
                break;
            }
            Some("give_up") => {
                run.push(step(n + 1, StepKind::GiveUp, json!({"code": "c", "reason": "r"})));
                break;
            }
            _ => {}
        }
    }
    run
}

/// [`transcript`] keeps, on every prefix of a run a tick wrote, what a
/// turn relies on and a history rule is held to ([`check`]); with the
/// run's open step among the steps or without. The histories of
/// [`generated`] are not runs (a script's result without its turn, two
/// first turns, a turn with no request), so invariants 2 to 5 are not
/// theirs to keep; the three that are about a rule and not the run (1,
/// 6 and 7) are checked on them too.
#[test]
fn the_transcript_of_a_run_keeps_what_a_turn_relies_on() {
    let mut runs: Vec<(String, Vec<Step>)> = pinned_runs();
    runs.push(("the hand-built run".into(), a_run()));
    let mut r = Numbers(0x2545_F491_4F6C_DD1D);
    runs.extend((0..600).map(|i| (format!("written run {i}"), a_written_run(&mut r))));
    let (mut prefixes, mut paused, mut several, mut refused) = (0, 0, 0, 0);
    for (name, run) in &runs {
        for n in 0..=run.len() {
            let mut steps = run[..n].to_vec();
            let shown = transcript(&steps);
            assert_eq!(
                check(&steps, &shown),
                Ok(()),
                "the first {n} steps of {name}: {steps:#?}"
            );
            // The step begun next, open, changes nothing.
            if let Some(next) = run.get(n) {
                steps.push(open(next.n, next.kind, 2));
                assert_eq!(
                    check(&steps, &shown),
                    Ok(()),
                    "{name}, {n} and an open step"
                );
            }
            prefixes += 1;
            let last = shown.messages.last();
            paused += usize::from(
                Control::of(&steps).next == json!({"kind": "turn", "user": []})
                    && last.is_some_and(|m| m.role == Role::Assistant),
            );
            let results = last.map_or(0, |m| {
                m.content
                    .iter()
                    .filter(|b| b.to_json()["type"] == "tool_result")
                    .count()
            });
            several += usize::from(results > 1);
            refused += usize::from(results == 1 && Control::of(&steps).committed % 2 == 1);
        }
    }
    assert!(prefixes > 2000, "{prefixes} prefixes");
    assert!(paused > 50, "{paused} ended on a paused turn");
    assert!(several > 50, "{several} answered several calls");
    assert!(refused > 50, "{refused} ended on a result");

    let ok = |e: &Result<(), String>| match e {
        Ok(()) => true,
        Err(e) => ["2:", "3:", "4:", "5:"].iter().any(|n| e.starts_with(n)),
    };
    for (i, steps) in generated(&seeds()).iter().enumerate() {
        let said = check(steps, &transcript(steps));
        assert!(ok(&said), "generated history {i}: {said:?}: {steps:#?}");
    }
}
