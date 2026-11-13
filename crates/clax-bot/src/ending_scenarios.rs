//! Whole runs ended each way, for the library's unit tests alone: each
//! run below is ended in a home of its own, and what it leaves
//! ([`Left`]: the run's row, every step as stored, every journal row,
//! the live events, the audit records, the usage rows, the writes the
//! host committed with their words and threads, every call that reached
//! the host with its attempt, the replies on the threads, the roster
//! calls and the follow-ups queued) is held to a fixture of its own,
//! `tests/scenarios/ending.<name>.<cut>.json`. The fixtures were written
//! while a run's end through the seam left, run for run and cut for
//! cut, what the endings before it left.
//!
//! The runs ([`the_scenarios`]): `finish` with and without a reply,
//! sources and a summary; `give_up` by the model and by the run loop;
//! a stop for each kind of reason (a refusal, a cap, spent credits, a
//! cancel, a request that is gone, a model error); and a failure. Each
//! is ended plainly, and with its driver dying at each point of its
//! terminal step ([`Cut`]): once the step is begun and before any
//! notice, after the notices' journal rows and before the commit, and
//! after the commit; a run that died is resumed by another driver. Each
//! scenario states the status it ends in and how many replies its
//! threads hold, so that a fixture rewritten by a change that ends a
//! run otherwise fails here too. (A script cancelled into its `stopped`
//! step is in [`crate::script_scenarios`].)
//!
//! A fixture is rewritten as a golden is (`src/golden.rs`):
//! `CLAX_UPDATE_GOLDEN=1 bazel run //crates/clax-bot:clax_bot_lib_test`,
//! and its diff reviewed with the change that caused it.

use crate::Next;
use crate::Wait;
use crate::run::{CrashPoint, Deps};
use crate::script_scenarios::common::*;
use crate::web::FetchedPage;
use clax_bot_sandbox::HostError;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::credits::{CostLinePatch, KeySource, LINE_SCRIPT_COMPUTE, LINE_TOOLPATH_STORAGE};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Where a run's driver dies in its terminal step.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Cut {
    /// Nowhere.
    No,
    /// Once the step is begun: before any notice.
    Begun,
    /// After the notices' journal rows, before the step's commit.
    BeforeCommit,
    /// After the commit: before the roster is turned off and the
    /// follow-ups are started.
    AfterCommit,
}

impl Cut {
    /// The cut, as its fixtures' names have it.
    fn file_name(self) -> &'static str {
        match self {
            Cut::No => "plain",
            Cut::Begun => "begun",
            Cut::BeforeCommit => "before-commit",
            Cut::AfterCommit => "after-commit",
        }
    }
}

/// What `left` holds, as the pretty JSON its fixture keeps.
fn pinned(left: &Left) -> String {
    let v = json!({
        "run": left.run, "steps": left.steps, "journal": left.journal, "events": left.events,
        "audit": left.audit, "usage": left.usage, "writes": left.writes,
        "reached_the_host": left.reached_the_host, "replies": left.replies,
        "roster": left.roster, "follow_ups": left.follow_ups,
    });
    crate::script_scenarios::fixture_text(&v)
}

/// One run and how it ends.
#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    /// Prepares the home and makes the run.
    make: fn(&Env) -> String,
    /// Puts back what the terminal step's first attempt used up, before
    /// the run is resumed.
    again: fn(&Env),
    /// The ticks before the one that takes the terminal step.
    before: usize,
    /// The terminal step.
    step: i64,
    kind: StepKind,
    /// The status the run ends in, and the replies its threads hold.
    ends: RunStatus,
    replies: usize,
}

const GIVEN: &str = "https://example.com/a";
const PUBLISH: &str = r#"await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const v = await clax.draft.publish({note: "Blue heading", addresses: []});
return v.version;"#;
const DRAFT_ONLY: &str =
    r#"await clax.files.write("extra.css", "h1 { color: blue; }"); return "written";"#;

fn nothing(_: &Env) {}

fn a_page(env: &Env) {
    env.web().page(
        GIVEN,
        Ok(FetchedPage {
            final_url: GIVEN.into(),
            status: 200,
            content_type: "text/markdown".into(),
            text: "# Doc title\nbody".into(),
            links: Vec::new(),
            request_id: None,
        }),
    );
    env.host.lock().context.threads[0].comments[0].text = format!("Read {GIVEN}");
}

fn gives_up(reason: &str) -> Reply {
    stream(
        &[B::Tool(
            "tu_g",
            "give_up",
            json!({"reason": reason}).to_string(),
        )],
        "tool_use",
        USAGE,
    )
}

fn model_error(env: &Env) {
    env.api.push(Reply::Status(
        400,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad request"}}"#
            .into(),
    ));
}

fn request_gone(env: &Env) {
    env.host.lock().fail_next.insert(
        "context".into(),
        HostError::new("not_found", "no thread T1"),
    );
}

fn the_scenarios() -> Vec<Scenario> {
    let finish = |name, make, replies| Scenario {
        name,
        make,
        again: nothing,
        before: 3,
        step: 4,
        kind: StepKind::Finish,
        ends: RunStatus::Finished,
        replies,
    };
    let stopped = |name, make, again, replies| Scenario {
        name,
        make,
        again,
        before: 0,
        step: 1,
        kind: StepKind::Stopped,
        ends: RunStatus::Stopped,
        replies,
    };
    vec![
        finish(
            "finish, no reply, two threads",
            |env| {
                env.api.push(env.script_turn("tu_1", "Edit", PUBLISH));
                env.api.push(finish_turn("Made the heading blue."));
                env.run_on_both_threads("")
            },
            2,
        ),
        finish(
            "finish, replied, sources alone",
            |env| {
                a_page(env);
                let code = format!(
                    r#"await clax.web.fetch({GIVEN:?}); await clax.threads.reply("T1", "Mine.");"#
                );
                env.api.push(env.script_turn("tu_1", "Web", &code));
                env.api.push(finish_turn("Read the page."));
                env.run_with_web("fetch")
            },
            2,
        ),
        finish(
            "finish, no reply, sources, two threads",
            |env| {
                a_page(env);
                let code = format!("await clax.web.fetch({GIVEN:?});");
                env.api.push(env.script_turn("tu_1", "Web", &code));
                env.api.push(finish_turn("Read the page."));
                env.run_on_both_threads("fetch")
            },
            2,
        ),
        Scenario {
            before: 1,
            step: 2,
            ..finish(
                "finish, empty summary",
                |env| {
                    env.api.push(finish_turn(""));
                    env.run_on_both_threads("")
                },
                0,
            )
        },
        Scenario {
            kind: StepKind::GiveUp,
            ends: RunStatus::GaveUp,
            ..finish(
                "give_up, after a script that edited",
                |env| {
                    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
                    env.api.push(gives_up("The page has no heading."));
                    env.run()
                },
                1,
            )
        },
        Scenario {
            before: 2,
            step: 3,
            kind: StepKind::GiveUp,
            ends: RunStatus::GaveUp,
            ..finish(
                "give_up, no tool call, two threads",
                |env| {
                    env.api.push(text_turn("I am thinking."));
                    env.api.push(text_turn("Still thinking."));
                    env.run_on_both_threads("")
                },
                2,
            )
        },
        stopped(
            "stopped, disabled, two threads",
            |env| {
                env.set(|s| s.enabled = false);
                env.run_on_both_threads("")
            },
            nothing,
            2,
        ),
        Scenario {
            before: 1,
            step: 2,
            ..stopped(
                "stopped, the step cap",
                |env| {
                    env.set(|s| s.max_steps = 1);
                    env.api.push(env.script_turn("tu_1", "Edit", DRAFT_ONLY));
                    env.run()
                },
                nothing,
                1,
            )
        },
        stopped(
            "stopped, credits spent",
            |env| {
                env.admin_sets(&[(Setting::FreeWeeklyMilli, json!(0))]);
                env.run()
            },
            nothing,
            1,
        ),
        stopped(
            "stopped, cancelled",
            |env| {
                let run = env.run_on_both_threads("");
                env.store
                    .cancel_run(&crate::run::bot_ctx(OWNER), &run)
                    .unwrap();
                run
            },
            nothing,
            0,
        ),
        stopped(
            "stopped, the request is gone",
            |env| {
                request_gone(env);
                env.run()
            },
            request_gone,
            0,
        ),
        stopped(
            "stopped, a model error",
            |env| {
                model_error(env);
                env.run()
            },
            model_error,
            1,
        ),
        Scenario {
            before: 3,
            kind: StepKind::Failed,
            ends: RunStatus::Failed,
            ..stopped(
                "failed, two threads",
                |env| {
                    // Without its cost line, every step's commit is
                    // refused.
                    env.store
                        .delete_cost_line(&AuditCtx::DAEMON, "do.requests")
                        .unwrap();
                    env.set(|s| s.infra_lines = true);
                    for _ in 0..3 {
                        env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
                    }
                    env.run_on_both_threads("")
                },
                nothing,
                2,
            )
        },
    ]
}

/// What a run leaves.
#[derive(Debug, PartialEq)]
struct Left {
    run: Value,
    steps: Value,
    journal: Value,
    events: Value,
    audit: Value,
    usage: Value,
    /// The writes the host committed, in order, with their arguments: a
    /// notice's thread and words among them.
    writes: Value,
    /// Every call that reached the host, with the attempt it was told.
    reached_the_host: Value,
    /// ClaxBot's replies on the threads, as the store keeps them.
    replies: Value,
    /// Every roster call: on or off, with the run's status and step.
    roster: Value,
    /// The people queued for a follow-up on each thread.
    follow_ups: Value,
}

const MEASURED: [&str; 8] = [
    "ms",
    "us",
    "busy_us",
    "cpu_us",
    "wall_us",
    "cpu_ms",
    "mem_peak",
    "mem_peak_bytes",
];
const MEASURED_LINES: [&str; 2] = [LINE_SCRIPT_COMPUTE, LINE_TOOLPATH_STORAGE];

/// Replaces what differs between two homes: what a script's run
/// measured, the build's version and commit, and a comment's ID.
fn scrub(v: &mut Value) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(map) => {
            for (k, x) in map.iter_mut() {
                let named = match k.as_str() {
                    k if MEASURED.contains(&k) => "<measured>",
                    "comment_id" => "<comment>",
                    "code_sha256" => "<sha256 of the code>",
                    "clax_version" | "clax_commit" => "<build>",
                    _ => {
                        scrub(x);
                        continue;
                    }
                };
                if !x.is_null() {
                    *x = json!(named);
                }
            }
        }
        Value::String(text) if text.starts_with('{') => {
            if let Ok(mut told) = serde_json::from_str::<Value>(text) {
                scrub(&mut told);
                *text = told.to_string();
            }
        }
        _ => {}
    }
}

/// Reads back what `run` left in `env`.
async fn left(env: &Env, run: &str, live: &mut crate::Subscription) -> Left {
    let store = &env.store;
    let (row, steps) = store.run_with_steps(run).unwrap().unwrap();
    let steps_json: Vec<Value> = steps
        .iter()
        .map(|s| {
            json!({"n": s.n, "kind": s.kind.as_str(), "attempt": s.attempt,
                   "started_at": s.started_at, "committed_at": s.committed_at,
                   "payload": s.payload, "failures": s.failures})
        })
        .collect();
    let journal: Vec<Value> = steps
        .iter()
        .flat_map(|s| store.bot_calls(run, s.n).unwrap())
        .map(|c| {
            json!({"step": c.step_n(), "attempt": c.attempt(), "seq": c.seq(), "op": c.op(),
                   "ok": c.ok(), "result": c.result()})
        })
        .collect();
    let mut events = Vec::new();
    while let Some(e) = live.next().await.expect("no event was missed") {
        events.push(json!({"id": e.id.to_string(), "event": e.event, "data": e.data}));
    }
    let audit: Vec<Value> = store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind.starts_with("bot."))
        .map(|e| {
            let i = &e.ids;
            json!({"kind": e.kind, "at": e.at,
                   "actor": serde_json::from_str::<Value>(&e.actor).unwrap(),
                   "ids": {"artifact": i.artifact, "thread": i.thread, "call": i.call},
                   "body": serde_json::from_str::<Value>(&e.body).unwrap()})
        })
        .collect();
    let usage: Vec<Value> = env
        .usage()
        .iter()
        .map(|r| {
            let qty = if MEASURED_LINES.contains(&r.line.as_str()) {
                json!("<measured>")
            } else {
                json!(r.qty)
            };
            json!({"step": r.step_n, "line": r.line, "qty": qty,
                   "credits_milli": r.credits_milli, "absorbed": r.absorbed})
        })
        .collect();
    let host = env.host.lock();
    let writes: Vec<Value> = host
        .writes
        .iter()
        .map(|w| json!({"key": w.key, "op": w.op, "args": w.args}))
        .collect();
    let reached: Vec<Value> = host
        .calls
        .iter()
        .zip(&host.attempts)
        .map(|((key, op), attempt)| json!({"key": key, "op": op, "attempt": attempt}))
        .collect();
    let roster: Vec<Value> = host
        .roster
        .iter()
        .map(|c| json!({"on": c.on, "status": c.status.map(|s| s.as_str()), "step": c.step}))
        .collect();
    drop(host);
    let follow_ups = json!([
        store.follow_ups(&env.t1).unwrap(),
        store.follow_ups(&env.t2).unwrap()
    ]);
    let ids = [
        (env.aid.clone(), "<artifact>"),
        (env.t1.clone(), "<thread-1>"),
        (env.t2.clone(), "<thread-2>"),
        (run.to_string(), "<run>"),
    ];
    let named = |v: Value| -> Value {
        let mut v = v;
        scrub(&mut v);
        let mut s = v.to_string();
        for (id, name) in &ids {
            s = s.replace(id.as_str(), name);
        }
        serde_json::from_str(&s).unwrap()
    };
    Left {
        run: json!({"status": row.status.as_str(), "reason": row.reason,
                    "credits_milli": row.credits_milli}),
        steps: named(json!(steps_json)),
        journal: named(json!(journal)),
        events: named(json!(events)),
        audit: named(json!(audit)),
        usage: named(json!(usage)),
        writes: named(json!(writes)),
        reached_the_host: named(json!(reached)),
        replies: named(json!(env.replies())),
        roster: named(json!(roster)),
        follow_ups: named(follow_ups),
    }
}

/// The measured lines are charged nothing, so that what a run is
/// charged does not depend on what its scripts measured.
fn free_what_is_measured(env: &Env) {
    for line in MEASURED_LINES {
        env.store
            .put_cost_line(
                &AuditCtx::DAEMON,
                line,
                CostLinePatch {
                    cost_micro_usd: Some(0),
                    fixed_credits_milli: Some(Some(0)),
                    ..CostLinePatch::default()
                },
            )
            .unwrap();
    }
}

/// Ticks `run` until it ends or waits. A tick that fails is ticked
/// again, as a driver does: a step whose commit is refused fails its
/// tick.
async fn drive(env: &Env, deps: &Deps, run: &str, driver: &DriverId) -> Next {
    for _ in 0..50 {
        match env.try_tick(deps, run, driver, None).await {
            Ok(out) if out.next != Next::Runnable => return out.next,
            _ => {}
        }
    }
    panic!("the run did not end in 50 ticks");
}

/// Ends `scenario`'s run in a home of its own, its driver dying at
/// `cut`, and reads back what it left.
async fn run(scenario: Scenario, cut: Cut) -> Left {
    let env = Env::new().await;
    free_what_is_measured(&env);
    let run = (scenario.make)(&env);
    let mut live = env.hub.subscribe(&env.store, &run, None).unwrap();
    let (step, kind) = (scenario.step, scenario.kind);
    let what = format!("{} ({cut:?})", scenario.name);
    if cut != Cut::No {
        let mut dying = env.deps();
        let driver = DriverId::mint();
        match cut {
            Cut::Begun => {
                for _ in 0..scenario.before {
                    let _ = env.try_tick(&dying, &run, &driver, None).await;
                }
                env.store
                    .claim_run(&run, &driver, 60)
                    .unwrap()
                    .expect("the driver holds the run");
                env.store
                    .begin_step_on(&run, &driver, step, kind, KeySource::Instance)
                    .unwrap();
            }
            _ => {
                let at = if cut == Cut::AfterCommit {
                    CrashPoint::AfterCommit { step, kind }
                } else {
                    CrashPoint::BeforeCommit { step, kind }
                };
                let fired = Arc::new(AtomicBool::new(false));
                dying.crash = Some(Arc::new(move |p| {
                    p == at && !fired.swap(true, Ordering::SeqCst)
                }));
                let waits = drive(&env, &dying, &run, &driver).await;
                assert_eq!(waits, Next::Waiting(Wait::Crashed), "{what}");
            }
        }
        env.clock.advance(61);
        (scenario.again)(&env);
    }
    let ended = drive(&env, &env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(ended, Next::Done(scenario.ends), "{what}");
    assert_eq!(env.replies().len(), scenario.replies, "{what}");
    left(&env, &run, &mut live).await
}

/// The fixture of the scenario `name` cut at `cut`, from the crate's
/// directory.
fn fixture(name: &str, cut: Cut) -> String {
    format!(
        "tests/scenarios/ending.{}.{}.json",
        crate::script_scenarios::slug(name),
        cut.file_name()
    )
}

/// Every scenario cut at `cut`: what each leaves is what its fixture
/// holds.
async fn check(cut: Cut) {
    for scenario in the_scenarios() {
        let left = run(scenario, cut).await;
        crate::golden::golden(
            &format!("what the run {:?} leaves ({cut:?})", scenario.name),
            &fixture(scenario.name, cut),
            &pinned(&left),
        );
    }
}

#[tokio::test]
async fn a_run_ended_leaves_what_its_fixture_holds() {
    check(Cut::No).await;
}

#[tokio::test]
async fn a_run_that_died_with_its_terminal_step_begun_ends_as_its_fixture_holds() {
    check(Cut::Begun).await;
}

#[tokio::test]
async fn a_run_that_died_before_its_terminal_commit_ends_as_its_fixture_holds() {
    check(Cut::BeforeCommit).await;
}

#[tokio::test]
async fn a_run_that_died_after_its_terminal_commit_is_left_as_its_fixture_holds() {
    check(Cut::AfterCommit).await;
}

/// The fixtures of `tests/scenarios` named `ending.*` are the
/// scenarios' at each cut and [`UNREADABLE`], and no others.
#[test]
fn every_scenario_has_a_fixture_at_each_cut() {
    let mut files = std::collections::BTreeSet::new();
    for scenario in the_scenarios() {
        for cut in [Cut::No, Cut::Begun, Cut::BeforeCommit, Cut::AfterCommit] {
            assert!(
                files.insert(fixture(scenario.name, cut)),
                "{}",
                scenario.name
            );
        }
    }
    assert_eq!(files.len(), 13 * 4);
    files.insert(UNREADABLE.to_string());
    assert_eq!(crate::script_scenarios::fixtures("ending."), files);
}

/// Each kind of terminal step is among the scenarios, and so is a stop
/// with a notice and one without.
#[test]
fn the_scenarios_end_a_run_each_way() {
    let scenarios = the_scenarios();
    for kind in [
        StepKind::Finish,
        StepKind::GiveUp,
        StepKind::Stopped,
        StepKind::Failed,
    ] {
        assert!(scenarios.iter().any(|s| s.kind == kind), "{kind:?}");
    }
    let stops = |noticed: bool| {
        scenarios
            .iter()
            .any(|s| s.kind == StepKind::Stopped && (s.replies > 0) == noticed)
    };
    assert!(stops(true) && stops(false));
}

/// Takes the table of the pages a run fetched away, so that reading a
/// run's sources fails, or puts it back.
fn pages_table(env: &Env, away: bool) {
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let (from, to) = if away {
        ("bot_web_pages", "bot_web_pages_away")
    } else {
        ("bot_web_pages_away", "bot_web_pages")
    };
    c.execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
        .unwrap();
}

/// The fixture of [`unreadable_sources_are_counted_against_the_finish_step`].
const UNREADABLE: &str = "tests/scenarios/ending.sources-unreadable-twice.json";

/// A `finish` whose sources cannot be read for two ticks and then can:
/// each failed read is counted against the step, nothing is posted
/// meanwhile, and the run then finishes as it would have, with the two
/// failures on its step.
#[tokio::test]
async fn unreadable_sources_are_counted_against_the_finish_step() {
    let env = Env::new().await;
    free_what_is_measured(&env);
    a_page(&env);
    let code = format!("await clax.web.fetch({GIVEN:?});");
    env.api.push(env.script_turn("tu_1", "Web", &code));
    env.api.push(finish_turn("Read the page."));
    let run = env.run_with_web("fetch");
    let mut live = env.hub.subscribe(&env.store, &run, None).unwrap();
    let (deps, driver) = (env.deps(), DriverId::mint());
    for _ in 0..3 {
        env.tick(&deps, &run, &driver).await;
    }
    pages_table(&env, true);
    for failed in 1..=2 {
        assert!(env.try_tick(&deps, &run, &driver, None).await.is_err());
        let open = env.store.open_step(&run).unwrap().unwrap();
        assert_eq!((open.attempt, open.failures), (failed, failed));
        assert!(env.replies().is_empty());
    }
    pages_table(&env, false);
    assert_eq!(
        drive(&env, &deps, &run, &driver).await,
        Next::Done(RunStatus::Finished)
    );
    let left = left(&env, &run, &mut live).await;
    assert_eq!(left.steps[3]["failures"], 2);
    crate::golden::golden(
        "what a finish whose sources were unreadable twice leaves",
        UNREADABLE,
        &pinned(&left),
    );
}
