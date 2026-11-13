//! Whole runs of the script step, for the library's unit tests alone:
//! each run below is driven in a home of its own, and what it leaves
//! ([`Left`]: the run's row, every step as stored, every journal row,
//! the live events, the audit records, the usage rows, what reached the
//! host and the web source, the drafts published, the artifact's
//! versions, the replies, the stored draft and the pages kept) is held
//! to a fixture of its own, `tests/scenarios/script.<name>.json`
//! ([`slug`] of the scenario's name). The fixtures were written while
//! the step through the seam left, run for run, what the step before
//! it left; a run of a script crashed before its commit and resumed is
//! held to what that step left on the same path, so a journal written
//! by a build before the seam is the one the fixture holds.
//!
//! The runs: scripts that edit, rebase, publish, reply, read and are
//! refused, a brief's run that creates an artifact, scripts that follow
//! one that published, web calls that reach the source, are answered
//! from the page store and are refused, a script cut off by the call
//! limit, one that throws after it wrote, one whose logs and one whose
//! result do not fit, each crashed before its step's commit and run
//! again where the scenario says so; steps resumed with only the first
//! calls of their script in the journal; a death after the step's
//! commit and one before a cancelled script's stop; a replay that
//! diverges; and scripts cancelled in a loop and after they returned.
//! Each scenario states how its last script ends and after which call
//! ([`Scenario::went_as_meant`]), so that a fixture rewritten by a
//! change that makes a script die early fails here too.
//!
//! The journal row of a web operation the run lacks (refused at check
//! 2 of [`crate::surface`]) is `{code, message}` alone, and the
//! scenarios hold three such rows in all
//! ([`the_refused_rows_are_the_run_loops`]); what such a refusal is
//! answered when the driver loses its claim around the call is held by
//! `tests/web.rs`
//! (`a_web_operation_a_run_lacks_is_refused_as_itself_when_the_claim_is_lost`,
//! `…_in_a_step_that_commits`).
//!
//! A fixture is rewritten as a golden is (`src/golden.rs`):
//! `CLAX_UPDATE_GOLDEN=1 bazel run //crates/clax-bot:clax_bot_lib_test`,
//! and its diff reviewed with the change that caused it.

#[path = "../tests/common/mod.rs"]
pub(crate) mod common;

use self::common::*;
use crate::events::{Event, EventSink};
use crate::run::{CrashPoint, Deps};
use crate::web::{FetchedPage, SearchAnswer, SearchHit};
use crate::{EventHub, Next, TickOpts, Wait};
use clax_bot_sandbox::{CancelToken, Host, HostError, Limits, Sandbox, Script, ScriptOutcome};
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RequestKind, RunStatus, StepKind};
use clax_store::credits::{CostLinePatch, LINE_SCRIPT_COMPUTE, LINE_TOOLPATH_STORAGE};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How a scenario's last script step is interrupted.
#[derive(Clone, Copy, PartialEq)]
enum Cut {
    /// Not at all.
    No,
    /// Its driver dies before the step's commit, and another resumes
    /// the run once the first's claim has lapsed.
    Crash,
    /// The same, and the first attempt ran this other script. When it
    /// is the first calls of the stored script, the second attempt
    /// resumes a step whose journal holds only those; when it is
    /// another script, the second attempt takes another path.
    CrashAfter(&'static str),
    /// Its driver dies after the step's commit.
    CrashAfterCommit,
    /// The run is cancelled once the script's call to this operation
    /// has been answered.
    CancelOnCall(&'static str),
    /// The run is cancelled once the script has returned.
    CancelOnReturn,
    /// The tick is cancelled once the script has returned, and its
    /// driver dies before the `stopped` step's commit; a driver told of
    /// no cancel then runs the step again from its journal.
    CancelOnReturnThenCrash,
}

/// One run to compare.
#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    /// Prepares the home before the run is made.
    setup: fn(&Env),
    /// What happens after the first driver died and before the second
    /// resumes.
    between: fn(&Env),
    /// The run's web operations.
    web: &'static str,
    /// Whether the host has a web source for the first attempt of the
    /// last script, and for what follows it.
    source: (bool, bool),
    brief: bool,
    both_threads: bool,
    /// The scripts, each a step of its own (steps 2, 4, …).
    scripts: &'static [&'static str],
    cut: Cut,
    /// The last call the step of the last script made, as
    /// [`last_call`] writes it (`threads.reply`, or
    /// `db.update!invalid_args` for one that failed); `""` for a script
    /// that made none. A script that ended before the call the scenario
    /// is about does not end with it.
    last_call: &'static str,
    /// The kind of the error the last script ended with, when the
    /// scenario means it to end with one.
    error: Option<&'static str>,
    /// What else the run's steps (as [`Left::steps`] has them) must
    /// show for the scenario to have compared what its name says.
    holds: fn(&Value),
    /// How many journal rows of the run are of a web operation it
    /// lacks.
    decided: usize,
}

impl Scenario {
    /// A scenario whose last script ends with no error after
    /// `last_call`, crashed before that script's commit and resumed.
    fn of(
        name: &'static str,
        scripts: &'static [&'static str],
        last_call: &'static str,
    ) -> Scenario {
        Scenario {
            name,
            setup: |_| {},
            between: |_| {},
            web: "",
            source: (true, true),
            brief: false,
            both_threads: false,
            scripts,
            cut: Cut::Crash,
            last_call,
            error: None,
            holds: |_| {},
            decided: 0,
        }
    }

    fn after(self, setup: fn(&Env)) -> Scenario {
        Scenario { setup, ..self }
    }

    fn then(self, between: fn(&Env)) -> Scenario {
        Scenario { between, ..self }
    }

    fn with_web(self, web: &'static str) -> Scenario {
        Scenario { web, ..self }
    }

    fn cut(self, cut: Cut) -> Scenario {
        Scenario { cut, ..self }
    }

    /// This scenario, whose last script is meant to end with an error
    /// of kind `kind`.
    fn fails(self, kind: &'static str) -> Scenario {
        Scenario {
            error: Some(kind),
            ..self
        }
    }

    fn holds(self, holds: fn(&Value)) -> Scenario {
        Scenario { holds, ..self }
    }

    /// The step of the last script.
    fn last(&self) -> i64 {
        2 * i64::try_from(self.scripts.len()).expect("a few scripts")
    }

    /// Checks that the run `left` is of went as far as the scenario
    /// means: its last script ended as stated, after the call stated,
    /// and with what else the scenario holds it to. So a scenario whose
    /// script dies early on both paths compares two equal failures no
    /// longer.
    fn went_as_meant(&self, left: &Left, way: &str) {
        let name = self.name;
        let step = &left.steps[usize::try_from(self.last()).expect("a step") - 1];
        let payload = &step["payload"];
        assert_eq!(
            payload["outcome"]["error"]["kind"],
            json!(self.error),
            "{name} ({way}): the last script's error: {}",
            payload["outcome"]["error"]
        );
        assert_eq!(
            last_call(payload),
            self.last_call,
            "{name} ({way}): the last call of the last script"
        );
        (self.holds)(&left.steps);
    }
}

/// The last call of a script step's `calls`: its operation, and after a
/// `!` its code when it failed; `""` when the step made no call.
fn last_call(payload: &Value) -> String {
    match payload["calls"].as_array().and_then(|calls| calls.last()) {
        None => String::new(),
        Some(call) => {
            let op = call["op"].as_str().unwrap_or("");
            match call["code"].as_str() {
                Some(code) => format!("{op}!{code}"),
                None => op.to_string(),
            }
        }
    }
}

/// What the model was told of the script of step `n` of `steps`: the
/// text, and the text parsed when it is JSON.
fn told(steps: &Value, n: usize) -> (String, Value) {
    let text = steps[n - 1]["payload"]["result"]["content"]
        .as_str()
        .expect("what the model was told")
        .to_string();
    let parsed = serde_json::from_str(&text).unwrap_or(Value::Null);
    (text, parsed)
}

/// The value the script of step `n` of `steps` returned.
fn value(steps: &Value, n: usize) -> &Value {
    &steps[n - 1]["payload"]["outcome"]["value"]
}

/// A sandbox that runs another script than the one it is given.
struct Another {
    sandbox: Arc<dyn Sandbox>,
    code: String,
}

#[async_trait::async_trait]
impl Sandbox for Another {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let other = Script {
            code: self.code.clone(),
            ..script.clone()
        };
        self.sandbox.run(&other, host, limits, cancel).await
    }
}

/// A sandbox that cancels a token once the script it ran has returned.
struct CancelOnReturn {
    sandbox: Arc<dyn Sandbox>,
    token: CancelToken,
}

#[async_trait::async_trait]
impl Sandbox for CancelOnReturn {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let outcome = self.sandbox.run(script, host, limits, cancel).await;
        self.token.cancel();
        outcome
    }
}

/// The run's events, passed on to the hub; cancels a token when a call
/// to `op` has been answered.
struct Watching<'a> {
    hub: &'a EventHub,
    op: Option<&'static str>,
    token: CancelToken,
}

impl EventSink for Watching<'_> {
    fn emit(&self, run: &str, step: i64, attempt: i64, event: Event) {
        let cancels = matches!(&event, Event::Call { op, .. } if Some(op.as_str()) == self.op);
        self.hub.emit(run, step, attempt, event);
        if cancels {
            self.token.cancel();
        }
    }
}

fn deps(env: &Env, sandbox: Arc<dyn Sandbox>, source: bool) -> Deps {
    let deps = env.deps_with(sandbox);
    if source { deps } else { deps.without_web() }
}

/// Drives `run` until it ends or waits, telling `events`.
async fn drive(
    env: &Env,
    deps: &Deps,
    run: &str,
    token: &CancelToken,
    op: Option<&'static str>,
) -> Next {
    let driver = DriverId::mint();
    let watching = Watching {
        hub: &env.hub,
        op,
        token: token.clone(),
    };
    for _ in 0..50 {
        let opts = TickOpts {
            budget: None,
            cancel: token.clone(),
            events: &watching,
        };
        let out = tokio::time::timeout(PROMPTLY, crate::tick(deps, run, &driver, opts))
            .await
            .expect("the tick ends")
            .expect("the tick");
        if out.next != Next::Runnable {
            return out.next;
        }
    }
    panic!("the run did not end in 50 ticks");
}

/// What a run leaves.
#[derive(Debug, PartialEq)]
struct Left {
    /// The run's status and reason.
    run: Value,
    /// Every step as stored: its number, kind, attempt, times and
    /// payload.
    steps: Value,
    /// Every journal row: its place, attempt, operation, the hash of its
    /// arguments, its outcome and its result.
    journal: Value,
    /// The live events, in order: each one's ID, name and data.
    events: Value,
    /// The run's `bot.*` audit records: kind, time, actor, IDs and body.
    audit: Value,
    /// The usage rows: step, line, quantity, credits, whether absorbed.
    usage: Value,
    /// The writes the fake host committed, in order, with their
    /// arguments.
    writes: Value,
    /// Every call that reached the fake host, with the attempt it was
    /// told it was of.
    reached_the_host: Value,
    /// The drafts the fake host was given to publish.
    drafts: Value,
    /// The artifact's versions beyond its first, as the store keeps
    /// them.
    versions: Value,
    /// ClaxBot's replies on the artifact's threads, as the store keeps
    /// them.
    replies: Value,
    /// The run's stored draft.
    draft: Value,
    /// The calls that reached the web source.
    reached_the_source: Value,
    /// The pages the run keeps.
    pages: Value,
}

/// The keys whose values a script's run measured.
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

/// Replaces what differs between two homes or two runs of one script:
/// what a script's run measured, the build's version and commit, and a
/// comment's ID.
fn scrub(v: &mut Value) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(map) => {
            for (k, x) in map.iter_mut() {
                let named = match k.as_str() {
                    k if MEASURED.contains(&k) => "<measured>",
                    "comment_id" => "<comment>",
                    // The hash of a script's code, which names a thread
                    // by its ID.
                    "code_sha256" => "<sha256 of the code>",
                    "clax_version" | "clax_commit" => "<build>",
                    "comments" if x.is_array() => {
                        for c in x.as_array_mut().into_iter().flatten() {
                            if c.get("id").is_some_and(|id| !id.is_null()) {
                                c["id"] = json!("<comment>");
                            }
                        }
                        continue;
                    }
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
        // What the model is told of a script is JSON as text (unless
        // it was cut to fit), in a step's payload and in its event.
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
    // A call's arguments hold the IDs minted for this home, so their
    // hash differs from home to home: each hash is given by the place of
    // the run's first row that has it, which keeps which calls had the
    // same arguments. (That a row's hash is the one a replay looks for
    // is held by the runs resumed across the two paths.)
    let rows: Vec<_> = steps
        .iter()
        .flat_map(|s| store.bot_calls(run, s.n).unwrap())
        .collect();
    let journal: Vec<Value> = rows
        .iter()
        .map(|c| {
            let first = rows
                .iter()
                .find(|r| r.args_hash() == c.args_hash())
                .expect("the row itself");
            json!({"step": c.step_n(), "attempt": c.attempt(), "seq": c.seq(), "op": c.op(),
                   "same_args_as": format!("{}:{}", first.step_n(), first.seq()),
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
                   "ids": {"artifact": i.artifact, "artifact2": i.artifact2, "thread": i.thread,
                           "session": i.session, "question": i.question, "call": i.call,
                           "origin": i.origin},
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
    let drafts: Vec<Value> = host
        .versions
        .iter()
        .map(|d| {
            let files: Vec<Value> = d
                .files
                .iter()
                .map(|f| {
                    json!({"path": f.path, "content_type": f.content_type,
                           "content": String::from_utf8_lossy(&f.content)})
                })
                .collect();
            json!({"files": files, "if_version": d.if_version, "note": d.note,
                   "addresses": d.addresses})
        })
        .collect();
    drop(host);
    let aid = ArtifactId::parse(&env.aid).unwrap();
    let current = store.get_artifact(&aid).unwrap().unwrap().current_version;
    let versions: Vec<Value> = (2..=current)
        .map(|n| {
            let v = store.get_version(&aid, n).unwrap().unwrap();
            json!({"n": v.n, "note": v.note, "addresses": v.addresses, "files": v.files,
                   "content_sha256": v.content_sha256})
        })
        .collect();
    let draft: Vec<Value> = store
        .draft_files(run)
        .unwrap()
        .iter()
        .map(|f| {
            json!({"path": f.path, "content_type": f.content_type,
                   "content": f.content.as_ref().map(|c| String::from_utf8_lossy(c))})
        })
        .collect();
    let pages: Vec<Value> = store
        .web_pages(run)
        .unwrap()
        .iter()
        .map(|p| {
            json!({"url": p.url, "step": p.step_n, "seq": p.seq, "final_url": p.final_url,
                   "status": p.status, "text": p.text, "total_chars": p.total_chars})
        })
        .collect();
    // The IDs minted for this home, by name; an artifact a script
    // created, by the order it was created in.
    let mut ids = vec![
        (env.aid.clone(), "<artifact>".to_string()),
        (env.t1.clone(), "<thread-1>".to_string()),
        (env.t2.clone(), "<thread-2>".to_string()),
        (run.to_string(), "<run>".to_string()),
    ];
    for row in &journal {
        if row["op"] == "artifacts.create"
            && let Some(id) = row["result"]["id"].as_str()
        {
            ids.push((id.to_string(), format!("<created-{}>", ids.len() - 3)));
        }
    }
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
        drafts: named(json!(drafts)),
        versions: named(json!(versions)),
        replies: named(json!(env.replies())),
        draft: named(json!(draft)),
        reached_the_source: named(json!(env.web().calls())),
        pages: named(json!(pages)),
    }
}

/// The lines whose quantity is what a script's run measured: its
/// compute, and the bytes of its step's payload, which holds the
/// measurements.
const MEASURED_LINES: [&str; 2] = [LINE_SCRIPT_COMPUTE, LINE_TOOLPATH_STORAGE];

/// Those lines are charged nothing, so that what a run is charged does
/// not depend on what its scripts measured.
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

/// Drives `scenario` in a home of its own and reads back what it left.
async fn run(scenario: Scenario) -> Left {
    let env = Env::new().await;
    free_what_is_measured(&env);
    env.set(|s| s.script_cpu_ms = 600_000);
    (scenario.setup)(&env);
    const IDS: [&str; 3] = ["tu_1", "tu_2", "tu_3"];
    for (id, code) in IDS.iter().zip(scenario.scripts) {
        env.api.push(env.script_turn(id, "Script", code));
    }
    env.api.push(finish_turn("done"));
    let run = if scenario.brief {
        env.run_as(RequestKind::Brief)
    } else if scenario.both_threads {
        env.run_on_both_threads(scenario.web)
    } else {
        env.run_with_web(scenario.web)
    };
    let mut live = env.hub.subscribe(&env.store, &run, None).unwrap();
    let token = CancelToken::new();
    let (first, then) = scenario.source;
    let plain = || deps(&env, env.sandbox.clone(), then);
    let crash_at = |deps: &mut Deps, at: CrashPoint| {
        let fired = Arc::new(AtomicBool::new(false));
        deps.crash = Some(Arc::new(move |p| {
            p == at && !fired.swap(true, Ordering::SeqCst)
        }));
    };
    let step = scenario.last();
    let ended = match scenario.cut {
        Cut::No => drive(&env, &plain(), &run, &token, None).await,
        Cut::Crash | Cut::CrashAfter(_) | Cut::CrashAfterCommit => {
            let sandbox: Arc<dyn Sandbox> = match scenario.cut {
                Cut::CrashAfter(code) => Arc::new(Another {
                    sandbox: env.sandbox.clone(),
                    code: env.code(code),
                }),
                _ => env.sandbox.clone(),
            };
            let mut dying = deps(&env, sandbox, first);
            let kind = StepKind::Script;
            let at = match scenario.cut {
                Cut::CrashAfterCommit => CrashPoint::AfterCommit { step, kind },
                _ => CrashPoint::BeforeCommit { step, kind },
            };
            crash_at(&mut dying, at);
            let waits = drive(&env, &dying, &run, &token, None).await;
            assert_eq!(waits, Next::Waiting(Wait::Crashed), "{}", scenario.name);
            env.clock.advance(61);
            (scenario.between)(&env);
            drive(&env, &plain(), &run, &token, None).await
        }
        Cut::CancelOnCall(op) => drive(&env, &plain(), &run, &token, Some(op)).await,
        Cut::CancelOnReturn => {
            let sandbox = Arc::new(CancelOnReturn {
                sandbox: env.sandbox.clone(),
                token: token.clone(),
            });
            drive(&env, &deps(&env, sandbox, then), &run, &token, None).await
        }
        Cut::CancelOnReturnThenCrash => {
            let sandbox = Arc::new(CancelOnReturn {
                sandbox: env.sandbox.clone(),
                token: token.clone(),
            });
            let mut dying = deps(&env, sandbox, first);
            let kind = StepKind::Stopped;
            crash_at(&mut dying, CrashPoint::BeforeCommit { step, kind });
            let waits = drive(&env, &dying, &run, &token, None).await;
            assert_eq!(waits, Next::Waiting(Wait::Crashed), "{}", scenario.name);
            env.clock.advance(61);
            (scenario.between)(&env);
            // A driver that was told of no cancel.
            drive(&env, &plain(), &run, &CancelToken::new(), None).await
        }
    };
    let expected = match scenario.cut {
        Cut::CancelOnCall(_) | Cut::CancelOnReturn => RunStatus::Stopped,
        _ => RunStatus::Finished,
    };
    assert_eq!(ended, Next::Done(expected), "{}", scenario.name);
    left(&env, &run, &mut live).await
}

/// The words of a web operation refused at check 2.
const NOT_AVAILABLE: &str = "is not available to this run";

/// Whether `row` is the journal row of a web operation refused at
/// check 2.
fn refused_at_check_2(row: &Value) -> bool {
    row["result"]["code"] == "web_disabled"
        && row["result"]["message"]
            .as_str()
            .is_some_and(|m| m.ends_with(NOT_AVAILABLE))
}

/// Checks that each journal row of a web operation refused at check 2
/// in what `left` holds is `{code, message}` alone, and answers how
/// many there are.
fn the_refused_rows_are_the_run_loops(left: &Left) -> usize {
    let mut rows = 0;
    for row in left.journal.as_array().expect("the journal") {
        if refused_at_check_2(row) {
            let keys: Vec<&String> = row["result"].as_object().unwrap().keys().collect();
            assert_eq!(keys, ["code", "message"], "the run loop's row: {row}");
            rows += 1;
        }
    }
    rows
}

/// `name` as a file name: its letters and digits, lower case, each run
/// of anything else one `-`.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The longest string a fixture keeps whole, in bytes.
const KEPT_WHOLE: usize = 2048;

/// `v` as the pretty JSON a fixture keeps, with a newline after it:
/// every string longer than [`KEPT_WHOLE`] bytes is kept as its length,
/// its SHA-256 and its first characters, so that a script's long logs
/// or value are held byte for byte without being written out.
pub(crate) fn fixture_text(v: &Value) -> String {
    fn shorten(v: &mut Value) {
        match v {
            Value::Array(items) => items.iter_mut().for_each(shorten),
            Value::Object(map) => map.values_mut().for_each(shorten),
            Value::String(s) if s.len() > KEPT_WHOLE => {
                let head: String = s.chars().take(60).collect();
                *s = format!(
                    "<{} bytes, sha256 {}, from {head:?}>",
                    s.len(),
                    clax_core::secret::sha256_hex(s)
                );
            }
            _ => {}
        }
    }
    let mut v = v.clone();
    shorten(&mut v);
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

/// What `left` holds, as the pretty JSON its fixture keeps.
fn pinned(left: &Left) -> String {
    let v = json!({
        "run": left.run, "steps": left.steps, "journal": left.journal, "events": left.events,
        "audit": left.audit, "usage": left.usage, "writes": left.writes,
        "reached_the_host": left.reached_the_host, "drafts": left.drafts,
        "versions": left.versions, "replies": left.replies, "draft": left.draft,
        "reached_the_source": left.reached_the_source, "pages": left.pages,
    });
    fixture_text(&v)
}

/// Drives `scenario` and holds what it leaves to its fixture; the run
/// must have gone as far as the scenario means
/// ([`Scenario::went_as_meant`]).
async fn check(scenario: Scenario) {
    let name = scenario.name;
    let left = run(scenario).await;
    scenario.went_as_meant(&left, "as driven");
    assert_eq!(
        the_refused_rows_are_the_run_loops(&left),
        scenario.decided,
        "{name}: the journal rows of web operations the run lacks"
    );
    crate::golden::golden(
        &format!("what the run {name:?} leaves"),
        &fixture(name),
        &pinned(&left),
    );
}

/// The fixture of the scenario `name`, from the crate's directory.
fn fixture(name: &str) -> String {
    format!("tests/scenarios/script.{}.json", slug(name))
}

// ---------------------------------------------------------------------------
// The runs
// ---------------------------------------------------------------------------

const EDIT: &str = r#"const html = await clax.files.read("index.html");
await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const report = await clax.draft.check();
if (!report.ok) return report;
const v = await clax.draft.publish({note: "Blue heading", addresses: ["T1"]});
await clax.threads.reply("T1", "The heading is blue now.");
console.log("published", v.version);
return {version: v.version, bytes: html.length};"#;

const PUBLISH: &str = r#"await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const v = await clax.draft.publish({note: "Blue heading"});
return v.version;"#;

/// The page the sender's comment names.
const GIVEN: &str = "https://example.com/a";

/// Someone else publishes version 2 of the artifact before the run
/// starts.
fn someone_publishes(env: &Env) {
    let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
        "files": {
            "index.html": {"content": PAGE.replace("Quarterly goals", "NEW goals"), "encoding": "utf8"},
            "notes.txt": {"content": "Theirs.", "encoding": "utf8"}
        },
        "if_version": 1
    }))
    .unwrap();
    env.store
        .publish_version(
            &AuditCtx::DAEMON,
            &ArtifactId::parse(&env.aid).unwrap(),
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap();
}

/// The web source has a page at [`GIVEN`] and an answer to a search,
/// and the sender's comment names the page.
fn a_page_the_sender_named(env: &Env) {
    env.web().page(
        GIVEN,
        Ok(FetchedPage {
            final_url: GIVEN.into(),
            status: 200,
            content_type: "text/markdown".into(),
            text: "# Doc title\nbody".into(),
            links: vec!["https://example.com/linked".into()],
            request_id: None,
        }),
    );
    env.web().search_answers(
        "clax",
        Ok(SearchAnswer {
            hits: vec![SearchHit {
                url: "https://example.com/found".into(),
                title: Some("Found".into()),
                description: Some("About it".into()),
                age: None,
            }],
            request_id: Some("req-1".into()),
            raw: json!({"hits": 1}),
        }),
    );
    env.host.lock().context.threads[0].comments[0].text = format!("Read {GIVEN}");
}

fn theirs_and_a_page(env: &Env) {
    someone_publishes(env);
    a_page_the_sender_named(env);
}

fn a_conflict_and_a_host_away(env: &Env) {
    someone_publishes(env);
    env.host.lock().fail_next.insert(
        "threads.reply".into(),
        HostError::new("unavailable", "the host is away").retryable(),
    );
}

fn a_document(env: &Env) {
    env.set_doc("notes/a", json!({"n": 1}));
}

fn three_calls_a_script(env: &Env) {
    env.set(|s| s.script_max_calls = 3);
}

fn usage_lines_of_the_worker(env: &Env) {
    env.set(|s| s.infra_lines = true);
}

/// Someone publishes version 3 over version 2.
fn someone_publishes_again(env: &Env) {
    let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
        "files": {"index.html": {"content": PAGE.replace("Quarterly goals", "NEWER goals"),
                                 "encoding": "utf8"}},
        "if_version": 2
    }))
    .unwrap();
    env.store
        .publish_version(
            &AuditCtx::DAEMON,
            &ArtifactId::parse(&env.aid).unwrap(),
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap();
}

/// The document the scripts read is written again.
fn the_document_changes(env: &Env) {
    env.set_doc("notes/a", json!({"n": 9}));
}

/// The script of the web calls: a search and a fetch that reach the
/// source, a window from the page store, and refusals.
const WEB_CALLS: &str = r#"const found = await clax.web.search("clax");
const page = await clax.web.fetch("https://example.com/a");
const again = await clax.web.fetch("https://example.com/a", {offset: 3});
const out = [found, page, again];
const tries = [
  () => clax.web.fetch("https://example.com/unnamed"),
  () => clax.web.fetch("https://example.com/a", {format: "raw"}),
  () => clax.web.search(""),
  () => clax.web.fetch("not a url"),
  () => clax.web.fetch("https://example.com/linked"),
  () => clax.web.search("nothing scripted"),
];
for (const f of tries) {
  try { out.push(await f()); } catch (e) { out.push([e.code, e.message, e.retryable]); }
}
return out;"#;

/// What [`WEB_CALLS`] must have come to.
fn the_web_calls_were_made(steps: &Value) {
    let out = value(steps, 2);
    assert_eq!(out.as_array().map(Vec::len), Some(9), "{out}");
    assert_eq!(out[0]["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(out[1]["totalChars"], 16);
    assert_eq!(out[2]["offset"], 3, "a window from the page store");
    assert_eq!(out[3][0], "url_not_in_prior_context");
    assert_eq!(out[4][0], "invalid_args", "the page was read as auto");
    assert_eq!(out[5][0], "invalid_args", "a search for nothing");
    assert_eq!(out[6][0], "invalid_args", "what is no URL");
    assert_eq!(
        out[7][0], "web_unavailable",
        "a page the source does not have"
    );
    assert_eq!(out[8][0], "web_unavailable");
}

/// The script of the writes the host refuses: a publish that conflicts,
/// a reply the host is away for, a document write whose plan fails,
/// then a reply, a rebase, a read of their file and a publish that
/// succeed.
const REFUSED_WRITES: &str = r#"const out = [];
const tries = [
  () => clax.draft.publish({note: "Mine"}),
  () => clax.threads.reply("T1", "First."),
  () => clax.db.update("notes", "missing", {x: 1}, {if_version: 7}),
  () => clax.threads.reply("T1", "Second."),
  () => clax.draft.rebase(),
  () => clax.draft.theirs("notes.txt"),
  () => clax.draft.publish({note: "Over theirs"}),
];
for (const f of tries) {
  try { out.push(await f()); } catch (e) { out.push([e.code, e.retryable]); }
}
return out;"#;

/// What [`REFUSED_WRITES`] must have come to.
fn the_refused_writes_and_what_follows(steps: &Value) {
    let out = value(steps, 2);
    assert_eq!(out[0], json!(["conflict", false]));
    assert_eq!(out[1], json!(["unavailable", true]));
    assert_eq!(out[2][1], false, "the document write: {out}");
    assert!(out[3].is_object(), "the second reply: {out}");
    assert_eq!(out[4]["version"], 2, "the rebase");
    assert_eq!(out[5], "Theirs.");
    assert_eq!(out[6]["version"], 3, "the publish over theirs");
}

/// A script that reads the thread and a document, writes, updates and
/// deletes documents, lists them, publishes once, is refused a second
/// publish, and replies.
const READS_WRITES_AND_A_SECOND_PUBLISH: &str = r#"const before = (await clax.threads.read("T1")).comments.length;
const doc = await clax.db.get("notes", "a");
const set = await clax.db.set("notes", "b", {n: 2}, {});
await clax.db.update("notes", "a", {n: 3}, {if_version: doc.version});
await clax.db.delete("notes", "b", {if_version: set.version});
const listed = await clax.db.list("notes");
await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "Once"});
let second;
try { await clax.draft.publish({note: "Twice"}); second = "published"; }
catch (e) { second = [e.code, e.message]; }
await clax.threads.reply("T1", "Published once.");
return {before, doc, listed, version: v.version, second, cols: await clax.db.collections()};"#;

fn the_scenarios() -> Vec<Scenario> {
    vec![
        Scenario::of("edit, check, publish and reply", &[EDIT], "threads.reply")
            .holds(|steps| assert_eq!(value(steps, 2)["version"], 2)),
        Scenario::of(
            "the same, uncrashed, with the Worker's usage lines",
            &[EDIT],
            "threads.reply",
        )
        .after(usage_lines_of_the_worker)
        .cut(Cut::No)
        .holds(|steps| assert_eq!(value(steps, 2)["version"], 2)),
        Scenario::of(
            "write, rebase, publish, reply and fetch",
            &[r#"await clax.files.write("extra.css", "h1 { color: blue; }");
const rebased = await clax.draft.rebase();
const v = await clax.draft.publish({note: "A stylesheet", addresses: ["T1"]});
await clax.threads.reply("T1", "Published the stylesheet.");
const page = await clax.web.fetch("https://example.com/a");
return {rebased, version: v.version, read: page.totalChars, files: await clax.files.list()};"#],
            "files.list",
        )
        .after(theirs_and_a_page)
        .with_web("fetch")
        .holds(|steps| {
            let out = value(steps, 2);
            assert_eq!((&out["rebased"]["version"], &out["version"]), (&json!(2), &json!(3)));
            assert_eq!(out["read"], 16, "the page was fetched");
        }),
        Scenario {
            both_threads: true,
            ..Scenario::of(
                "a publish that names no thread, on a run of two",
                &[r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.files.delete("extra.css");
await clax.files.write("more.css", "p { margin: 0; }");
const v = await clax.draft.publish({note: "A stylesheet"});
await clax.threads.reply("T2", "On the other thread.");
await clax.threads.resolve("T1");
return [v.version, await clax.threads.list(), await clax.artifact.get()];"#],
                "artifact.get",
            )
            .holds(|steps| {
                let out = value(steps, 2);
                assert_eq!(out[0], 2);
                assert_eq!(out[1][0]["resolved"], true, "the first thread was resolved");
                assert_eq!(out[1][1]["comments"], 2, "the reply on the second thread");
            })
        },
        Scenario::of("writes the host refuses", &[REFUSED_WRITES], "draft.publish")
            .after(a_conflict_and_a_host_away)
            .holds(the_refused_writes_and_what_follows),
        Scenario {
            source: (false, false),
            decided: 2,
            ..Scenario::of(
                "the run's rules, and web operations it lacks",
                &[r#"const out = [];
const tries = [
  () => clax.threads.reply("T2", "Not mine."),
  () => clax.threads.read("T2"),
  () => clax.artifacts.create({title: "No"}),
  () => clax.web.search("clax"),
  () => clax.web.fetch("https://example.com/a"),
  () => clax.web.fetch(7),
  () => clax.files.read("missing.css"),
  () => clax.files.read(),
  () => clax.nothing.here(),
];
for (const f of tries) {
  try { await f(); out.push("answered"); } catch (e) { out.push([e.code, e.message]); }
}
return out;"#],
                "files.read!invalid_args",
            )
            .with_web("search")
            .holds(|steps| {
                let codes: Vec<Value> = value(steps, 2)
                    .as_array()
                    .expect("each try's outcome")
                    .iter()
                    .map(|o| o[0].clone())
                    .collect();
                assert_eq!(
                    json!(codes),
                    json!([
                        "forbidden",
                        "forbidden",
                        "forbidden",
                        "web_disabled",
                        "web_disabled",
                        "invalid_args",
                        "not_found",
                        "invalid_args",
                        null
                    ])
                );
            })
        },
        Scenario {
            source: (false, true),
            decided: 1,
            ..Scenario::of(
                "a web refusal answered from its entry once the host has a source",
                &[r#"try { await clax.web.search("clax"); return "searched"; }
catch (e) { return [e.code, e.message]; }"#],
                "web.search!web_disabled",
            )
            .with_web("search")
            .holds(|steps| assert_eq!(value(steps, 2)[0], "web_disabled"))
        },
        Scenario::of(
            "host reads, document writes and a second publish",
            &[READS_WRITES_AND_A_SECOND_PUBLISH],
            "db.collections",
        )
        .after(a_document)
        .holds(|steps| {
            let out = value(steps, 2);
            assert_eq!(out["second"][0], "already_published", "{out}");
            assert_eq!(out["version"], 2);
            assert_eq!(out["before"], 1);
            assert_eq!(out["doc"]["data"], json!({"n": 1}));
            assert!(!out["listed"].is_null() && !out["cols"].is_null(), "{out}");
        }),
        Scenario::of(
            "a script after one that published",
            &[
                PUBLISH,
                r#"const html = await clax.files.read("index.html");
await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "A stylesheet"});
return {blue: html.includes("--accent: #00c;"), version: v.version, files: await clax.files.list()};"#,
            ],
            "files.list",
        )
        .holds(|steps| {
            assert_eq!(value(steps, 2), 2);
            let out = value(steps, 4);
            assert_eq!((&out["blue"], &out["version"]), (&json!(true), &json!(3)));
        }),
        Scenario::of(
            "a script after one that rebased, and a third after both",
            &[
                r#"const r = await clax.draft.rebase(); return r.version;"#,
                r#"const notes = await clax.files.read("notes.txt");
const v = await clax.draft.publish({note: "Over theirs"});
return {notes, version: v.version, files: await clax.files.list()};"#,
                r#"await clax.files.write("last.css", "a {}"); return await clax.draft.check();"#,
            ],
            "draft.check",
        )
        .after(someone_publishes)
        .holds(|steps| {
            assert_eq!(value(steps, 2), 2);
            assert_eq!(value(steps, 4)["notes"], "Theirs.");
            assert_eq!(value(steps, 4)["version"], 3);
            assert!(value(steps, 6)["ok"].is_boolean());
        }),
        Scenario {
            brief: true,
            ..Scenario::of(
                "a brief's run that creates an artifact, and may not create another",
                &[
                    r#"const made = await clax.artifacts.create({title: "New"}); return made;"#,
                    r#"try { await clax.artifacts.create({title: "Again"}); return "created"; }
catch (e) { return [e.code, e.message]; }"#,
                ],
                "artifacts.create!forbidden",
            )
            .holds(|steps| {
                assert!(value(steps, 2)["id"].is_string(), "the first was created");
                assert_eq!(
                    value(steps, 4),
                    &json!(["forbidden", "this run already created an artifact"])
                );
            })
        },
        Scenario::of(
            "web calls that reach the source, the page store, and refusals",
            &[WEB_CALLS],
            "web.search!web_unavailable",
        )
        .after(a_page_the_sender_named)
        .with_web("search,fetch")
        .holds(the_web_calls_were_made),
        Scenario::of(
            "a replay that takes another path at its second call",
            &["const a = await clax.files.list(); await clax.threads.reply('T1', 'hi'); return a.length;"],
            "threads.reply!replay_diverged",
        )
        .cut(Cut::CrashAfter(
            "await clax.files.list(); await clax.files.read('index.html'); \
             await clax.threads.reply('T1', 'first'); await clax.artifact.get(); return 0;",
        ))
        .fails("host_error")
        .holds(|steps| {
            let (_, told) = told(steps, 2);
            assert_eq!(told["replay"]["completed"], json!(["files.list"]));
        }),
        Scenario::of(
            "a replay that takes another path after a publish",
            &[r#"await clax.files.write("extra.css", "h1 {}");
await clax.draft.check();
return 1;"#],
            "draft.check!replay_diverged",
        )
        .cut(Cut::CrashAfter(
            r#"await clax.files.write("extra.css", "h1 {}");
await clax.draft.publish({note: "First attempt"});
await clax.threads.reply("T1", "Published."); return 0;"#,
        ))
        .fails("host_error")
        .holds(|steps| {
            let (_, told) = told(steps, 2);
            assert_eq!(told["replay"]["diverged_at"], 1);
            assert!(told.get("published").is_none(), "no session published");
        }),
        Scenario::of(
            "a script cancelled after it published and replied",
            &[r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.draft.publish({note: "Before the cancel"});
await clax.threads.reply("T1", "Published.");
await clax.files.write("late.css", "p { margin: 0; }");
await clax.artifact.get();
while (true) {}"#],
            "artifact.get",
        )
        .cut(Cut::CancelOnCall("artifact.get"))
        .fails("cancelled")
        .holds(|steps| {
            assert_eq!(steps[1]["kind"], "stopped");
            assert_eq!(steps[1]["payload"]["published"], json!({"version": 2}));
        }),
        Scenario::of(
            "a script cancelled before it wrote",
            &["await clax.files.list(); await clax.artifact.get(); while (true) {}"],
            "artifact.get",
        )
        .cut(Cut::CancelOnCall("artifact.get"))
        .fails("cancelled")
        .holds(|steps| assert_eq!(steps[1]["kind"], "stopped")),
        Scenario::of(
            "a script cancelled after it returned",
            &[RETURNS_AFTER_A_WRITE_AND_A_REPLY],
            "threads.reply",
        )
        .cut(Cut::CancelOnReturn)
        .holds(|steps| {
            assert_eq!(steps[1]["kind"], "stopped");
            assert_eq!(value(steps, 2), "done");
        }),
        Scenario::of(
            "a script past its call limit, after a write",
            &[r#"await clax.files.write("extra.css", "h1 {}");
await clax.files.list(); await clax.files.list(); await clax.files.list();
return "not reached";"#],
            "files.list",
        )
        .after(three_calls_a_script)
        .fails("call_limit")
        .holds(|steps| {
            assert_eq!(
                steps[1]["payload"]["calls"].as_array().map(Vec::len),
                Some(3),
                "the calls that reached the run loop"
            );
            assert_eq!(steps[1]["payload"]["changed"], json!(["extra.css"]));
        }),
        Scenario::of(
            "a call that is not awaited, and an error after a write and a reply",
            &[
                r#"clax.files.list(); return 1;"#,
                r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.threads.reply("T1", "Written.");
throw new Error("after the write");"#,
                r#"return await clax.files.read("extra.css");"#,
            ],
            "files.read",
        )
        .holds(|steps| {
            let (_, first) = told(steps, 2);
            assert_eq!(first["calls"][0]["code"], "script_finished");
            assert_eq!(steps[1]["payload"]["calls"], json!([]));
            assert_eq!(steps[3]["payload"]["outcome"]["error"]["kind"], "thrown");
            assert_eq!(last_call(&steps[3]["payload"]), "threads.reply");
            assert_eq!(value(steps, 6), "h1 { color: blue; }", "the write was kept");
        }),
        Scenario::of(
            "logs and a result that do not fit, after a publish",
            &[
                r#"await clax.files.write("extra.css", "h1 {}");
await clax.draft.publish({note: "Then logs"});
for (let i = 0; i < 3000; i++) console.log("line " + i + " " + '"'.repeat(60));
return "v".repeat(60000) + "é".repeat(2000);"#,
                r#"await clax.files.write("more.css", "p {}");
await clax.draft.publish({note: "Then a long value"});
throw new Error("é".repeat(60 * 1024));"#,
                "const = 1;",
            ],
            "",
        )
        .fails("syntax")
        .holds(|steps| {
            // The first: its logs were cut by the run loop to make the
            // result fit, with the session's field in it.
            let (text, first) = told(steps, 2);
            assert!(text.len() <= 100 << 10, "{} bytes", text.len());
            let logs = first["logs"].as_array().expect("the logs");
            assert!(!logs.is_empty() && logs.len() < 400, "{} lines", logs.len());
            assert_eq!(logs.last(), Some(&json!(clax_bot_sandbox::LOGS_TRUNCATED)));
            assert_eq!(first["published"], json!({"version": 2}));
            assert_eq!(first["value"].as_str().map(str::len), Some(64_000));
            // The second: no logs to cut, so the text was.
            let (text, second) = told(steps, 4);
            assert_eq!(second, Value::Null, "not JSON once cut");
            assert!(text.ends_with("… [truncated]"));
            assert!(text.contains(r#""published":{"version":3}"#) || text.len() > 100 << 10);
            assert_eq!(last_call(&steps[3]["payload"]), "draft.publish");
        }),
        // A step resumed with only the first calls of its script in the
        // journal, and other deaths than one before a script's commit.
        Scenario::of(
            "the driver dies after the draft calls and before the publish",
            &[EDIT],
            "threads.reply",
        )
        .cut(Cut::CrashAfter(
            r#"await clax.files.read("index.html");
await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
await clax.draft.check(); return 0;"#,
        ))
        .holds(|steps| assert_eq!(value(steps, 2)["version"], 2)),
        Scenario::of(
            "the driver dies after the publish's row and before the reply",
            &[EDIT],
            "threads.reply",
        )
        .cut(Cut::CrashAfter(
            r#"await clax.files.read("index.html");
await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
await clax.draft.check();
await clax.draft.publish({note: "Blue heading", addresses: ["T1"]}); return 0;"#,
        ))
        .holds(|steps| {
            assert_eq!(value(steps, 2)["version"], 2);
            assert_eq!(steps[1]["payload"]["base"], 2, "the replayed publish moved the base");
        }),
        Scenario::of(
            "the driver dies after a charged search and fetch, and the rest is made on resume",
            &[WEB_CALLS],
            "web.search!web_unavailable",
        )
        .after(a_page_the_sender_named)
        .with_web("search,fetch")
        .cut(Cut::CrashAfter(
            r#"await clax.web.search("clax");
await clax.web.fetch("https://example.com/a"); return 0;"#,
        ))
        .holds(the_web_calls_were_made),
        Scenario::of(
            "the driver dies after three refused writes were journaled",
            &[REFUSED_WRITES],
            "draft.publish",
        )
        .after(a_conflict_and_a_host_away)
        .cut(Cut::CrashAfter(
            r#"for (const f of [
  () => clax.draft.publish({note: "Mine"}),
  () => clax.threads.reply("T1", "First."),
  () => clax.db.update("notes", "missing", {x: 1}, {if_version: 7}),
]) { try { await f(); } catch (e) { } }
return 0;"#,
        ))
        .holds(the_refused_writes_and_what_follows),
        Scenario {
            brief: true,
            ..Scenario::of(
                "a brief's driver dies after it created an artifact, and a second is refused on resume",
                &[r#"await clax.artifacts.create({title: "New"});
try { await clax.artifacts.create({title: "Again"}); return "created"; }
catch (e) { return [e.code, e.message]; }"#],
                "artifacts.create!forbidden",
            )
            .cut(Cut::CrashAfter(
                r#"await clax.artifacts.create({title: "New"}); return 0;"#,
            ))
            .holds(|steps| {
                assert_eq!(value(steps, 2)[0], "forbidden");
                assert_eq!(steps[1]["payload"]["created"], true, "the replayed create counts");
            })
        },
        Scenario::of(
            "a rebase is recorded, and someone publishes again before the resume",
            &[r#"const r = await clax.draft.rebase();
let p;
try { p = (await clax.draft.publish({note: "Mine"})).version; } catch (e) { p = e.code; }
return [r.version, p];"#],
            "draft.publish!conflict",
        )
        .after(someone_publishes)
        .cut(Cut::CrashAfter("await clax.draft.rebase(); return 0;"))
        .then(someone_publishes_again)
        .holds(|steps| {
            assert_eq!(
                value(steps, 2),
                &json!([2, "conflict"]),
                "the rebase went where it went, and the artifact has moved on"
            );
        }),
        Scenario::of(
            "a document read is recorded, and the document changes before the resume",
            &[r#"const first = await clax.db.get("notes", "a");
const second = await clax.db.get("notes", "a");
return [first.data, second.data];"#],
            "db.get",
        )
        .after(a_document)
        .cut(Cut::CrashAfter(r#"await clax.db.get("notes", "a"); return 0;"#))
        .then(the_document_changes)
        .holds(|steps| {
            assert_eq!(
                value(steps, 2),
                &json!([{"n": 1}, {"n": 9}]),
                "the first read is the journal's, the second the host's"
            );
        }),
        Scenario::of(
            "the driver dies after the script step's commit",
            &[EDIT],
            "threads.reply",
        )
        .cut(Cut::CrashAfterCommit)
        .holds(|steps| {
            assert_eq!(steps[1]["attempt"], 1, "the step was not run again");
            assert_eq!(value(steps, 2)["version"], 2);
        }),
        Scenario::of(
            "a script cancelled on return whose driver dies before the stop, resumed with no cancel",
            &[RETURNS_AFTER_A_WRITE_AND_A_REPLY],
            "threads.reply",
        )
        .cut(Cut::CancelOnReturnThenCrash)
        .holds(|steps| {
            assert_eq!(steps[1]["kind"], "script", "committed as a script step");
            assert_eq!(steps[1]["attempt"], 3, "the script, the stop, the script again");
            assert_eq!(value(steps, 2), "done");
            assert_eq!(steps[1]["payload"]["changed"], json!(["extra.css"]));
        }),
        Scenario::of(
            "draft operations that fail, replayed",
            &[r#"const out = [];
const tries = [
  () => clax.files.read("missing.css"),
  () => clax.files.replace("index.html", "not in the page", "x"),
  () => clax.files.delete("missing.css"),
  () => clax.draft.theirs("missing.css"),
  () => clax.files.write("../outside.css", "a {}"),
];
for (const f of tries) {
  try { await f(); out.push("done"); } catch (e) { out.push(e.code); }
}
await clax.files.write("kept.css", "a {}");
return out;"#],
            "files.write",
        )
        .holds(|steps| {
            let out = value(steps, 2).as_array().expect("each try's outcome");
            assert_eq!(
                json!(out),
                json!(["not_found", "no_match", "not_found", "done", "invalid_path"]),
                "a file the draft lacks has no other side to read"
            );
            assert_eq!(steps[1]["payload"]["changed"], json!(["kept.css"]));
        }),
    ]
}

/// A script that writes to the draft, replies and returns.
const RETURNS_AFTER_A_WRITE_AND_A_REPLY: &str = r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.threads.reply("T1", "Written.");
return "done";"#;

/// How many tests the scenarios are dealt into, to run side by side.
const GROUPS: usize = 3;

/// Checks every `GROUPS`th scenario from `first` on.
async fn check_group(first: usize) {
    for scenario in the_scenarios().into_iter().skip(first).step_by(GROUPS) {
        check(scenario).await;
    }
}

// Every run leaves what its fixture holds, and each scenario's last
// script ends as the scenario states.

#[tokio::test]
async fn a_script_step_leaves_what_its_fixture_holds_1() {
    check_group(0).await;
}

#[tokio::test]
async fn a_script_step_leaves_what_its_fixture_holds_2() {
    check_group(1).await;
}

#[tokio::test]
async fn a_script_step_leaves_what_its_fixture_holds_3() {
    check_group(2).await;
}

/// The groups hold every scenario, each scenario's name is its own, and
/// the web operations a run lacks are refused in three rows in all: the
/// undeclared fetch and the declared search with no source of one run,
/// and the declared search of another. The fixtures of `tests/scenarios`
/// named `script.*` are the scenarios' and no others.
#[test]
fn every_scenario_is_in_a_group_and_has_a_fixture() {
    let scenarios = the_scenarios();
    let names: Vec<&str> = scenarios.iter().map(|s| s.name).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len());
    assert_eq!(names.len(), 30);
    let dealt: usize = (0..GROUPS)
        .map(|g| names.iter().skip(g).step_by(GROUPS).count())
        .sum();
    assert_eq!(dealt, names.len());
    assert_eq!(scenarios.iter().map(|s| s.decided).sum::<usize>(), 3);
    let files: std::collections::BTreeSet<String> = names.iter().map(|n| fixture(n)).collect();
    assert_eq!(files.len(), names.len(), "two names give one file name");
    assert_eq!(fixtures("script."), files);
}

/// The fixtures under `tests/scenarios` whose names begin with `prefix`,
/// from the crate's directory.
pub(crate) fn fixtures(prefix: &str) -> std::collections::BTreeSet<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|f| f.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(prefix))
        .map(|n| format!("tests/scenarios/{n}"))
        .collect()
}
