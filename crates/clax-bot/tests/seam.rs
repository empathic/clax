//! The proof that the seam (`clax_bot::surface`) carries Clax's own
//! surface: the product's provider (`clax_bot::clax::ClaxProvider`),
//! driven through the seam by a driver of this file's own ([`Stepper`]),
//! through scripts that edit, rebase, publish, read, reply and are
//! refused, each crashed before its step commits and run again, so that
//! its calls are replayed; through scripts that follow a script that
//! published, rebased or created an artifact, so that the session is
//! opened over what the committed steps left; and through one that is
//! cancelled. What a step leaves through the seam (its journal rows, the
//! fake host's writes with their arguments, the attempt each call that
//! reached the host was given, the drafts it was given to publish, the
//! versions the artifact has, the script's value, the parts of its
//! commit, the draft rows) is compared with what the same script leaves
//! through the run loop (`clax_bot::tick`), and with the golden that
//! pins those values (`tests/golden/resumed.publish.json`).
//!
//! The same provider is run through the cases every provider passes
//! (`clax_bot::provider_conformance`) in `tests/providers.rs`.
//!
//! [`Stepper`] stands for the run loop's part of a script step. It does
//! what the proof needs of it and no more: it stops on a diverged
//! replay instead of answering `replay_diverged`, and builds no tool
//! result, payload or audit record of its own. Every call goes through
//! [`dispatch`]. It is a second driver of the provider, written apart
//! from the run loop's (`crates/clax-bot/src/script.rs`): a provider
//! that leaves the same under both keeps the seam's contract and not
//! one driver's habits.

use crate::common::*;
use crate::resume::crash_once;
use crate::runs::EDIT;
use clax_bot::clax::ClaxProvider;
use clax_bot::events::{Event, EventSink};
use clax_bot::host::args_hash;
use clax_bot::run::{CrashPoint, bot_ctx};
use clax_bot::surface::{
    Bound, Claim, Fields, StepCx, StepEnd, StepParts, StepSession, SurfaceProvider, dispatch,
    no_place,
};
use clax_bot::web::{FetchedPage, SearchAnswer, SearchHit, WebSource};
use clax_bot::{Next, TickOpts, Wait};
use clax_bot_sandbox::{CancelToken, ErrorKind, Host, HostError, Script};
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{
    CLAX_SURFACE, CallEntry, DriverId, RequestKind, RunStatus, Step, StepCommit, StepKind,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// The provider
// ---------------------------------------------------------------------------

/// Clax's own surface over `env`'s store and fake host, sending web
/// calls to `env`'s web source when `web`, and to none otherwise.
fn provider(env: &Env, web: bool) -> ClaxProvider {
    ClaxProvider::new(
        env.store.clone(),
        env.host.clone(),
        web.then(|| env.web() as Arc<dyn WebSource>),
        env.clock.clone(),
    )
}

// ---------------------------------------------------------------------------
// The run loop's part of a script step, for the proof
// ---------------------------------------------------------------------------

/// The sandbox's host for one attempt of a script step through the
/// seam.
struct Stepper<'a> {
    provider: &'a ClaxProvider,
    session: Box<dyn StepSession + 'a>,
    cx: StepCx<'a>,
    /// The step's journal as the attempt found it.
    journal: BTreeMap<i64, CallEntry>,
    /// The next place, and what the step's `calls` would keep of each
    /// call so far.
    log: Mutex<(i64, Vec<Value>)>,
    /// The operation whose call, once answered, cancels the script.
    cancel_after: Option<&'static str>,
}

#[async_trait::async_trait]
impl Host for Stepper<'_> {
    async fn call(&self, name: &str, raw: Value) -> Result<Value, HostError> {
        let seq = {
            let mut log = self.log.lock().unwrap();
            log.0 += 1;
            log.0 - 1
        };
        let replay = self.journal.get(&seq);
        if let Some(e) = replay {
            assert!(
                e.op() == name && e.args_hash() == args_hash(&raw),
                "call {seq} was {} before and is {name} now",
                e.op()
            );
        }
        let bound = [Bound {
            binding: self.cx.binding,
            provider: self.provider,
            session: &*self.session,
        }];
        let Some(place) = self.cx.place(seq) else {
            // The core's refusal, told as any call's answer is.
            let refusal = no_place(seq);
            let call = json!({"op": name, "target": null, "ok": false, "code": refusal.code});
            self.log.lock().unwrap().1.push(call);
            return Err(refusal);
        };
        let answer = dispatch(&bound, &self.cx.claim, &place, name, &raw, replay).await;
        let code = answer.result().as_ref().err().map(|e| e.code.clone());
        let mut call = json!({"op": name, "target": answer.note().target(),
                              "ok": answer.result().is_ok(), "code": code});
        answer.note().add_to(&mut call);
        self.log.lock().unwrap().1.push(call);
        if self.cancel_after == Some(name) {
            self.cx.cancel.cancel();
        }
        answer.into_result()
    }
}

/// What one attempt of a script step through the seam came to.
#[derive(Debug, PartialEq)]
struct Attempted {
    step: i64,
    attempt: i64,
    /// What the script returned.
    value: Value,
    /// The kind of the error it ended with.
    error: Value,
    /// How the session was told the step ended.
    end: StepEnd,
    parts: StepParts,
    calls: Vec<Value>,
    /// How many of the attempt's web calls reached the host's web
    /// source.
    reached_the_source: usize,
}

/// How one attempt through the seam is made.
#[derive(Clone, Copy, Default)]
struct Try {
    /// The operation whose call, once answered, cancels the script.
    cancel_after: Option<&'static str>,
    /// Whether the host has a web source.
    web: bool,
}

/// One attempt of run `run`'s next step, the script its last turn asked
/// for, through the seam, by `driver`: begun, run to the script's end
/// (or to its cancel, once a call to `how.cancel_after` is answered)
/// and finished, and not committed.
async fn attempt(env: &Env, run: &str, driver: &DriverId, how: Try) -> Attempted {
    let store = &env.store;
    store
        .claim_run(run, driver, 60)
        .unwrap()
        .expect("the driver holds the run");
    let (_, steps) = store.run_with_steps(run).unwrap().unwrap();
    let history: Vec<Step> = steps
        .into_iter()
        .filter(|s| s.committed_at.is_some())
        .collect();
    let next = &history.last().expect("a turn").payload.as_ref().unwrap()["next"];
    assert_eq!(next["kind"], "script", "the run's next step");
    let n = store.next_step_n(run).unwrap();
    let step = store.begin_step(run, driver, n, StepKind::Script).unwrap();
    let reached = env.web().calls().len();
    let provider = provider(env, how.web);
    let info = env.info(run);
    let cancel = CancelToken::new();
    let cx = StepCx::new(
        &info,
        info.binding(CLAX_SURFACE).unwrap(),
        n,
        step.attempt,
        &history,
        Claim::new(store, driver, run),
        &cancel,
    );
    provider.working(&info).await;
    let host = Stepper {
        provider: &provider,
        session: provider.begin(cx).await.unwrap(),
        cx,
        journal: store
            .bot_calls(run, n)
            .unwrap()
            .into_iter()
            .map(|c| (c.seq(), c))
            .collect(),
        log: Mutex::new((0, Vec::new())),
        cancel_after: how.cancel_after,
    };
    let script = Script::new(
        next["title"].as_str().unwrap(),
        next["code"].as_str().unwrap(),
    );
    let limits = env.settings.0.lock().unwrap().limits();
    let outcome = env.sandbox.run(&script, &host, &limits, &cancel).await;
    let Stepper { session, log, .. } = host;
    // The run loop's rule (`script.rs`, `Tick::script`): cancelled when
    // the script's error says so, or the token is set when it returns.
    let cancelled = outcome
        .error
        .as_ref()
        .is_some_and(|e| e.kind == ErrorKind::Cancelled)
        || cancel.is_cancelled();
    let end = if cancelled {
        StepEnd::Cancelled
    } else {
        StepEnd::Commit
    };
    Attempted {
        step: n,
        attempt: step.attempt,
        value: json!(outcome.value),
        error: json!(outcome.error.as_ref().map(|e| e.kind)),
        end,
        parts: session.finish(end),
        calls: log.into_inner().unwrap().1,
        reached_the_source: env.web().calls().len() - reached,
    }
}

// ---------------------------------------------------------------------------
// What a step leaves, read back the same way on both paths
// ---------------------------------------------------------------------------

/// What a script step of a run left.
#[derive(Debug, PartialEq)]
struct Left {
    /// The step's journal rows.
    journal: Value,
    /// The writes the fake host committed under the step's keys, in
    /// order, as the goldens have them: a reply's thread, no other
    /// argument.
    writes: Value,
    /// The same writes, each with its arguments: a reply's thread and
    /// body, a publish's note and addresses, a document's path and
    /// data.
    written: Value,
    /// The drafts the fake host was given to publish, in order: each
    /// one's files with their contents, the version it is based on, its
    /// note and the threads it addresses.
    drafts: Value,
    /// The versions the artifact has beyond its first, as the store
    /// keeps them: each one's note and files. (The fake host keeps the
    /// threads a version addresses with the draft it was given, not
    /// with the version.)
    versions: Value,
    /// Every call that reached the fake host with one of the step's
    /// keys, in order, with the attempt the host was told it was of.
    reached_the_host: Value,
    /// What the script returned, and the kind of the error it ended
    /// with.
    value: Value,
    error: Value,
    /// The step's `calls`, without how long each took.
    calls: Value,
    /// The fields of the step's payload that are the `clax` surface's.
    payload: Value,
    /// What the surface adds to what the model is told.
    tool_result: Value,
    /// The run's stored draft.
    draft: Value,
    /// How many `bot.web` records the run left.
    web_records: usize,
    /// How many calls reached the host's web source.
    reached_the_source: usize,
}

/// The payload's fields that are the `clax` surface's.
const FIELDS: [&str; 5] = ["base", "changed", "created", "published", "replied"];

/// `v` with the IDs minted for `env` and `run` replaced by their names,
/// and a comment's ID by `<comment>`, as the goldens have them: a
/// reply's `comment_id`, and the `id` of each comment of a thread as a
/// script reads it.
fn named(env: &Env, run: &str, v: Value) -> Value {
    fn comments(v: &mut Value) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(comments),
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "comment_id" && !x.is_null() {
                        *x = json!("<comment>");
                    } else if k == "comments" && x.is_array() {
                        // A thread as a script reads it.
                        for c in x.as_array_mut().into_iter().flatten() {
                            if c.get("id").is_some_and(|id| !id.is_null()) {
                                c["id"] = json!("<comment>");
                            }
                        }
                    } else {
                        comments(x);
                    }
                }
            }
            _ => {}
        }
    }
    let mut v = v;
    comments(&mut v);
    let mut s = v.to_string();
    for (id, name) in [
        (env.aid.as_str(), "<artifact>"),
        (env.t1.as_str(), "<thread-1>"),
        (env.t2.as_str(), "<thread-2>"),
        (run, "<run>"),
    ] {
        s = s.replace(id, name);
    }
    serde_json::from_str(&s).unwrap()
}

/// What the rest of [`Left`] is read beside: what a script came to, on
/// either path.
struct Came {
    value: Value,
    error: Value,
    calls: Value,
    payload: Value,
    tool_result: Value,
    reached_the_source: usize,
}

/// What the store and the fake host hold of step `n` of `run`, with the
/// rest of [`Left`] as `came` has it.
fn left(env: &Env, run: &str, n: i64, came: Came) -> Left {
    let journal: Vec<Value> = env
        .store
        .bot_calls(run, n)
        .unwrap()
        .iter()
        .map(|c| {
            json!({"step": c.step_n(), "attempt": c.attempt(), "seq": c.seq(),
                   "op": c.op(), "ok": c.ok(), "result": c.result()})
        })
        .collect();
    // The step's own: a notice the run loop posts when the run ends
    // is a later step's.
    let of_the_step = |key: &str| key.starts_with(&format!("{run}:{n}:"));
    let host = env.host.lock();
    let writes: Vec<Value> = host
        .writes
        .iter()
        .filter(|w| of_the_step(&w.key))
        .map(|w| {
            let mut v = json!({"key": w.key, "op": w.op});
            if w.op == "threads.reply" {
                v["thread"] = w.args[0].clone();
            }
            v
        })
        .collect();
    let written: Vec<Value> = host
        .writes
        .iter()
        .filter(|w| of_the_step(&w.key))
        .map(|w| json!({"key": w.key, "op": w.op, "args": w.args}))
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
    let reached: Vec<Value> = host
        .calls
        .iter()
        .zip(&host.attempts)
        .filter(|((key, _), _)| of_the_step(key))
        .map(|((key, op), attempt)| json!({"key": key, "op": op, "attempt": attempt}))
        .collect();
    drop(host);
    let aid = ArtifactId::parse(&env.aid).unwrap();
    let current = env
        .store
        .get_artifact(&aid)
        .unwrap()
        .unwrap()
        .current_version;
    let versions: Vec<Value> = (2..=current)
        .map(|n| {
            let v = env.store.get_version(&aid, n).unwrap().unwrap();
            json!({"n": v.n, "note": v.note, "addresses": v.addresses, "files": v.files,
                   "content_sha256": v.content_sha256})
        })
        .collect();
    let draft: Vec<Value> = env
        .store
        .draft_files(run)
        .unwrap()
        .iter()
        .map(|f| {
            json!({"path": f.path, "content_type": f.content_type,
                   "content": f.content.as_ref().map(|c| String::from_utf8_lossy(c))})
        })
        .collect();
    let web_records = env
        .bot_events()
        .iter()
        .filter(|(kind, _)| kind == "bot.web")
        .count();
    let n = |v: Value| named(env, run, v);
    Left {
        journal: n(json!(journal)),
        writes: n(json!(writes)),
        written: n(json!(written)),
        drafts: n(json!(drafts)),
        versions: n(json!(versions)),
        reached_the_host: n(json!(reached)),
        value: n(came.value),
        error: came.error,
        calls: n(came.calls),
        payload: n(came.payload),
        tool_result: n(came.tool_result),
        draft: n(json!(draft)),
        web_records,
        reached_the_source: came.reached_the_source,
    }
}

/// A script and the run it is in.
#[derive(Clone, Copy)]
struct Case {
    /// Prepares the store and the fake host before the run is made.
    setup: fn(&Env),
    /// The run's web operations (`""`, `"search"`, …).
    web: &'static str,
    /// Whether the run is a brief's (which may create an artifact) and
    /// not a thread's.
    brief: bool,
    /// A script the run commits first, through the run loop on both
    /// paths, so that the script compared follows it.
    before: Option<&'static str>,
    /// Whether the host has a web source when the step is first
    /// attempted.
    source: bool,
    /// Whether the host has a web source when the step is run again.
    source_on_resume: bool,
    code: &'static str,
}

impl Case {
    fn of(code: &'static str) -> Case {
        Case {
            setup: |_| {},
            web: "",
            brief: false,
            before: None,
            source: false,
            source_on_resume: false,
            code,
        }
    }

    fn after(self, setup: fn(&Env)) -> Case {
        Case { setup, ..self }
    }

    fn with_web(self, web: &'static str) -> Case {
        Case { web, ..self }
    }

    fn following(self, before: &'static str) -> Case {
        Case {
            before: Some(before),
            ..self
        }
    }

    /// The step the script compared is: 2, or 4 when a script comes
    /// before it.
    fn step(&self) -> i64 {
        if self.before.is_some() { 4 } else { 2 }
    }

    /// The run, prepared, whose turns ask for the scripts.
    fn run(&self, env: &Env) -> String {
        (self.setup)(env);
        if let Some(before) = self.before {
            env.api.push(env.script_turn("tu_0", "First", before));
        }
        env.api.push(env.script_turn("tu_1", "Script", self.code));
        if self.brief {
            assert_eq!(self.web, "", "a brief's run here declares no web operation");
            env.run_as(RequestKind::Brief)
        } else {
            env.run_with_web(self.web)
        }
    }
}

/// The run loop on a host with no web source.
fn without_a_source(env: &Env) -> clax_bot::Deps {
    let mut deps = env.deps();
    deps = deps.without_web();
    deps
}

/// The run loop on a host with a web source when `source`, and with
/// none otherwise.
fn deps_with_source(env: &Env, source: bool) -> clax_bot::Deps {
    if source {
        env.deps()
    } else {
        without_a_source(env)
    }
}

/// What step `n` of `run`, committed by the run loop, came to: the
/// fields of its payload that are `clax`'s, as many of them as it has.
fn the_run_loops_step(env: &Env, run: &str, n: i64) -> Left {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let p = steps[usize::try_from(n).unwrap() - 1]
        .payload
        .clone()
        .unwrap();
    let told: Value = match p["result"]["content"].as_str() {
        Some(content) => serde_json::from_str(content).unwrap(),
        // A stopped step tells the model nothing.
        None => json!({}),
    };
    let calls: Vec<Value> = p["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c.as_object_mut().unwrap().remove("ms");
            c
        })
        .collect();
    let fields: Fields = FIELDS
        .iter()
        .filter_map(|k| p.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect();
    let told: Fields = told
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.as_str() == "published")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    left(
        env,
        run,
        n,
        Came {
            value: p["outcome"]["value"].clone(),
            error: p["outcome"]["error"]["kind"].clone(),
            calls: json!(calls),
            payload: json!(fields),
            tool_result: json!(told),
            reached_the_source: env.web().calls().len(),
        },
    )
}

/// What `case`'s script leaves through the run loop, in a run whose
/// driver dies before the script step's commit and is resumed by
/// another.
async fn in_the_run_loop(case: Case) -> Left {
    let env = Env::new().await;
    let run = case.run(&env);
    let n = case.step();
    env.api.push(finish_turn("done"));
    let mut dying = deps_with_source(&env, case.source);
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: n,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&dying, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    env.clock.advance(61);
    let resumed = deps_with_source(&env, case.source_on_resume);
    let out = env.drive(&resumed, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(
        steps[usize::try_from(n).unwrap() - 1].attempt,
        2,
        "the script step was begun again"
    );
    the_run_loops_step(&env, &run, n)
}

/// What `case`'s script leaves through the seam: the steps before it
/// are the run loop's; the script step is attempted through the seam by
/// a driver that dies before the commit, attempted again by another
/// once the first's claim has lapsed, and committed with the parts the
/// session answered.
async fn through_the_seam(case: Case) -> Left {
    let env = Env::new().await;
    let run = case.run(&env);
    let n = case.step();
    let dying = DriverId::mint();
    for step in 1..n {
        let tick = env.tick(&without_a_source(&env), &run, &dying).await;
        assert_eq!((tick.step, tick.next), (Some(step), Next::Runnable));
    }
    let first = attempt(
        &env,
        &run,
        &dying,
        Try {
            web: case.source,
            ..Try::default()
        },
    )
    .await;
    assert_eq!((first.step, first.attempt), (n, 1));
    // The driver dies here: nothing of the step is committed.
    env.clock.advance(61);
    let driver = DriverId::mint();
    let again = attempt(
        &env,
        &run,
        &driver,
        Try {
            web: case.source_on_resume,
            ..Try::default()
        },
    )
    .await;
    assert_eq!((again.step, again.attempt), (n, 2));
    assert_eq!(
        (&again.value, &again.error, &again.parts, &again.calls),
        (&first.value, &first.error, &first.parts, &first.calls),
        "the attempt that replayed the step (left) came to what the first came to (right)"
    );
    assert_eq!(again.end, StepEnd::Commit);
    let mut payload = again.parts.payload.clone();
    payload.insert("calls".into(), json!(again.calls));
    env.store
        .commit_bot_step(
            &bot_ctx(&env.run_row(&run).person),
            &run,
            &driver,
            n,
            StepCommit {
                payload: Value::Object(payload),
                draft: again.parts.draft.clone(),
                ..StepCommit::default()
            },
        )
        .unwrap();
    left(
        &env,
        &run,
        n,
        Came {
            value: again.value,
            error: again.error,
            calls: json!(again.calls),
            payload: json!(again.parts.payload),
            tool_result: json!(again.parts.tool_result),
            reached_the_source: first.reached_the_source + again.reached_the_source,
        },
    )
}

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

// ---------------------------------------------------------------------------
// The proof
// ---------------------------------------------------------------------------

/// A script that edits, publishes and replies, crashed before its step
/// commits and run again: through the seam it leaves what the run loop
/// leaves, which is what `resumed.publish.json` pins.
#[tokio::test]
async fn a_replayed_publish_behind_a_session_leaves_what_the_run_loop_leaves() {
    let seam = through_the_seam(Case::of(EDIT)).await;
    let looped = in_the_run_loop(Case::of(EDIT)).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );

    // The replayed attempt reached the host with no write: each write's
    // key is there once, from the first attempt.
    let reached = |op: &str| {
        let calls = seam.reached_the_host.as_array().unwrap();
        calls.iter().filter(|c| c["op"] == op).count()
    };
    assert_eq!((reached("draft.publish"), reached("threads.reply")), (1, 1));

    // What the writes were given: the publish's note and the threads
    // it names, the reply's thread and body.
    assert_eq!(
        seam.written,
        json!([
            {"key": "<run>:2:3", "op": "draft.publish",
             "args": ["Blue heading", ["<thread-1>"]]},
            {"key": "<run>:2:4", "op": "threads.reply",
             "args": ["<thread-1>", "The heading is blue now."]},
        ])
    );
    assert_eq!(seam.drafts.as_array().unwrap().len(), 1);
    assert_eq!(seam.drafts[0]["if_version"], 1);
    let page = seam.drafts[0]["files"][0]["content"].as_str().unwrap();
    assert!(page.contains("--accent: #00c;"), "the edit was published");
    assert_eq!(seam.drafts[0]["addresses"], json!(["<thread-1>"]));
    assert_eq!(seam.versions[0]["note"], "Blue heading");

    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden/resumed.publish.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(seam.journal, golden["journal"], "the journal rows");
    assert_eq!(seam.writes, golden["writes_in_order"], "the host's writes");
    let pinned = &golden["step"]["payload"];
    for field in FIELDS {
        assert_eq!(seam.payload[field], pinned[field], "payload.{field}");
    }
    assert_eq!(seam.payload.as_object().unwrap().len(), FIELDS.len());
    assert_eq!(seam.value, pinned["outcome"]["value"], "the script's value");
    let told: Value = serde_json::from_str(pinned["result"]["content"].as_str().unwrap()).unwrap();
    assert_eq!(seam.tool_result, json!({"published": told["published"]}));
    let pinned_calls: Vec<Value> = pinned["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"op": c["op"], "target": c["target"], "ok": c["ok"], "code": c["code"]}))
        .collect();
    assert_eq!(seam.calls, json!(pinned_calls), "the step's calls");
}

/// A script that writes, rebases onto a version someone else published,
/// publishes and replies. Run again, the artifact's current version is
/// the one the script itself published, and the rebase must go to the
/// version it went to, which only its journal entry says.
#[tokio::test]
async fn a_replayed_rebase_behind_a_session_goes_where_it_went() {
    let case = Case::of(
        r#"await clax.files.write("extra.css", "h1 { color: blue; }");
const rebased = await clax.draft.rebase();
const v = await clax.draft.publish({note: "A stylesheet", addresses: ["T1"]});
await clax.threads.reply("T1", "Published the stylesheet.");
return {rebased, version: v.version, files: await clax.files.list()};"#,
    )
    .after(someone_publishes);
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.value["rebased"]["version"], 2);
    assert_eq!(seam.value["version"], 3);
    assert_eq!(seam.payload["base"], 3);
    assert_eq!(
        seam.journal[1]["result"]["version"], 2,
        "the rebase's entry"
    );
    // The draft published is the rebased one: theirs and the run's.
    assert_eq!(seam.drafts[0]["if_version"], 2);
    let paths: Vec<&str> = seam.drafts[0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(paths, ["extra.css", "index.html", "notes.txt"]);
}

/// A publish with a note and no `addresses`: the version addresses the
/// run's threads.
#[tokio::test]
async fn a_publish_that_names_no_threads_addresses_the_runs() {
    let case = Case::of(
        r#"await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "A stylesheet"});
return v.version;"#,
    );
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.value, 2);
    assert_eq!(
        seam.written,
        json!([{"key": "<run>:2:1", "op": "draft.publish",
                "args": ["A stylesheet", ["<thread-1>"]]}])
    );
    assert_eq!(seam.drafts[0]["note"], "A stylesheet");
    assert_eq!(seam.drafts[0]["addresses"], json!(["<thread-1>"]));
    assert_eq!(
        seam.drafts[0]["files"][0],
        json!({"path": "extra.css", "content_type": "text/css",
               "content": "h1 { color: blue; }"})
    );
    assert_eq!(seam.versions.as_array().unwrap().len(), 1);
    assert_eq!(seam.versions[0]["note"], "A stylesheet");
    assert!(seam.versions[0]["files"]["extra.css"].is_object());
    assert!(seam.versions[0]["files"]["index.html"].is_object());
}

/// Someone publishes first, and the host is away for one reply.
fn a_conflict_and_a_host_away(env: &Env) {
    someone_publishes(env);
    env.host.lock().fail_next.insert(
        "threads.reply".into(),
        HostError::new("unavailable", "the host is away").retryable(),
    );
}

/// A publish the host refuses (`conflict`: someone published first), a
/// write the host refuses to plan, and a write whose plan fails in its
/// transaction after it was planned: each is journaled by the core as
/// a failure, so the step run again is answered the same and the host
/// is not asked again. A write that follows them succeeds once.
#[tokio::test]
async fn refused_writes_behind_a_session_are_journaled_and_not_asked_of_the_host_again() {
    let case = Case::of(
        r#"const out = [];
const tries = [
  () => clax.draft.publish({note: "Mine"}),
  () => clax.threads.reply("T1", "First."),
  () => clax.db.update("notes", "missing", {x: 1}, {if_version: 7}),
  () => clax.threads.reply("T1", "Second."),
];
for (const f of tries) {
  try { await f(); out.push("done"); } catch (e) { out.push([e.code, e.retryable]); }
}
return out;"#,
    )
    .after(a_conflict_and_a_host_away);
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    let codes: Vec<&Value> = seam.value.as_array().unwrap().iter().collect();
    assert_eq!(codes[0], &json!(["conflict", false]), "the publish");
    assert_eq!(codes[1], &json!(["unavailable", true]), "the reply");
    assert_eq!(codes[2][1], false, "the document write: {}", codes[2]);
    assert_eq!(codes[3], "done");
    // Each refusal has its row, written once, by the first attempt.
    let rows: Vec<(i64, &str, bool, i64)> = seam
        .journal
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["seq"].as_i64().unwrap(),
                r["op"].as_str().unwrap(),
                r["ok"].as_bool().unwrap(),
                r["attempt"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (0, "draft.publish", false, 1),
            (1, "threads.reply", false, 1),
            (2, "db.update", false, 1),
            (3, "threads.reply", true, 1),
        ]
    );
    assert_eq!(seam.journal[0]["result"]["code"], "conflict");
    assert_eq!(
        seam.journal[1]["result"],
        json!({"code": "unavailable", "message": "the host is away", "retryable": true})
    );
    assert_eq!(seam.journal[2]["result"]["code"], codes[2][0]);
    // The attempt that replayed the step did not ask the host again:
    // each call reached it once.
    assert_eq!(
        seam.reached_the_host,
        json!([
            {"key": "<run>:2:0", "op": "draft.publish", "attempt": 1},
            {"key": "<run>:2:1", "op": "threads.reply", "attempt": 1},
            {"key": "<run>:2:2", "op": "db.update", "attempt": 1},
            {"key": "<run>:2:3", "op": "threads.reply", "attempt": 1},
        ])
    );
    assert_eq!(
        seam.written,
        json!([{"key": "<run>:2:3", "op": "threads.reply",
                "args": ["<thread-1>", "Second."]}])
    );
    assert_eq!(seam.drafts, json!([]), "nothing was published");
    assert_eq!(seam.payload["published"], Value::Null);
    assert_eq!(seam.payload["replied"], json!(["<thread-1>"]));
}

/// A rule of the run's, and both halves of check 2 of the web
/// operations (`src/clax/web.rs`): a web operation the run declared, on
/// a host with no web source, which the session refuses; one the run
/// did not declare, which never reaches the session; and that one again
/// with arguments that do not hold, which is refused for its arguments
/// first. Each is journaled once, by the run loop, and answered again
/// the same when the step is run again.
#[tokio::test]
async fn refusals_behind_a_session_are_journaled_once_and_answered_again() {
    let case = Case::of(
        r#"const out = [];
const tries = [
  () => clax.threads.reply("T2", "Not mine."),
  () => clax.web.search("clax"),
  () => clax.web.fetch("https://example.com/a"),
  () => clax.web.fetch(7),
];
for (const f of tries) {
  try { await f(); out.push("answered"); } catch (e) { out.push([e.code, e.message]); }
}
return out;"#,
    )
    .with_web("search");
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.value[0][0], "forbidden");
    assert_eq!(
        seam.value[1],
        json!([
            "web_disabled",
            "clax.web.search is not available to this run"
        ])
    );
    assert_eq!(
        seam.value[2],
        json!([
            "web_disabled",
            "clax.web.fetch is not available to this run"
        ])
    );
    assert_eq!(seam.value[3][0], "invalid_args");
    assert_eq!(
        seam.calls[1],
        json!({"op": "web.search", "target": null, "ok": false, "code": "web_disabled",
               "cost_milli": 0, "web": {"query": "clax"}})
    );
    assert_eq!(
        seam.calls[2]["web"],
        json!({"url": "https://example.com/a"})
    );
    assert_eq!(seam.written, json!([]));
    // Uncharged, uncounted and with no `bot.web` record.
    assert_eq!(seam.web_records, 0);
    // Each of the two `web_disabled` rows is the run loop's own form:
    // the failure, and neither `cost_milli` nor `web`.
    for (at, op) in [(1, "web.search"), (2, "web.fetch")] {
        assert_eq!(
            seam.journal[at],
            json!({"step": 2, "attempt": 1, "seq": at, "op": op, "ok": false,
                   "result": {"code": "web_disabled",
                              "message": format!("clax.{op} is not available to this run")}})
        );
    }
    assert_eq!(seam.journal.as_array().unwrap().len(), 4);
}

/// A host read, and a second publish in one script. Run again, the read
/// is answered from the journal and not from the host, which holds a
/// reply more by then; and the second publish is refused by the
/// session, on each attempt, before the host is asked.
#[tokio::test]
async fn a_replayed_host_read_and_a_second_publish_behind_a_session() {
    let case = Case::of(
        r#"const before = (await clax.threads.read("T1")).comments.length;
await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "Once"});
let second;
try { await clax.draft.publish({note: "Twice"}); second = "published"; }
catch (e) { second = [e.code, e.message]; }
await clax.threads.reply("T1", "Published once.");
return {before, version: v.version, second};"#,
    );
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.value["before"], 1, "the thread before the reply");
    assert_eq!(seam.value["version"], 2);
    assert_eq!(seam.value["second"][0], "already_published");
    // The host was asked for the read once, for one publish and for
    // the reply, each by the first attempt.
    assert_eq!(
        seam.reached_the_host,
        json!([
            {"key": "<run>:2:0", "op": "threads.read", "attempt": 1},
            {"key": "<run>:2:2", "op": "draft.publish", "attempt": 1},
            {"key": "<run>:2:4", "op": "threads.reply", "attempt": 1},
        ])
    );
    assert_eq!(seam.versions.as_array().unwrap().len(), 1);
}

/// A script that edits and publishes, for a script to follow.
const PUBLISH_FIRST: &str = r#"await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const v = await clax.draft.publish({note: "Blue heading"});
return v.version;"#;

/// A script after one that published: its session is opened over a
/// draft based on the version the first script published, which only
/// the first script's committed step says (`payload.base`). It reads
/// the first script's edit, writes, and publishes over that version.
#[tokio::test]
async fn a_script_after_a_publish_starts_from_the_published_version() {
    let case = Case::of(
        r#"const html = await clax.files.read("index.html");
await clax.files.write("extra.css", "h1 { color: blue; }");
const v = await clax.draft.publish({note: "A stylesheet"});
return {blue: html.includes("--accent: #00c;"), version: v.version, files: await clax.files.list()};"#,
    )
    .following(PUBLISH_FIRST);
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(
        seam.error,
        Value::Null,
        "the second publish did not conflict"
    );
    assert_eq!(seam.value["blue"], true, "the first script's edit is read");
    assert_eq!(seam.value["version"], 3);
    // The second step's draft: published over the version the first
    // script published, and based on its own version after that.
    let drafts = seam.drafts.as_array().unwrap();
    assert_eq!(drafts.len(), 2);
    assert_eq!(
        (&drafts[0]["if_version"], &drafts[1]["if_version"]),
        (&json!(1), &json!(2))
    );
    assert_eq!(seam.payload["base"], 3);
    assert_eq!(seam.payload["published"], json!({"version": 3}));
    assert_eq!(seam.payload["changed"], json!(["extra.css"]));
    let stored: Vec<&str> = seam
        .draft
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(
        stored,
        ["extra.css", "index.html"],
        "the run's stored draft"
    );
}

/// A script after one that rebased onto a version someone else
/// published: its session is opened over the rebased draft, which has
/// their file, and its publish goes over their version.
#[tokio::test]
async fn a_script_after_a_rebase_starts_from_the_version_rebased_onto() {
    let case = Case::of(
        r#"const notes = await clax.files.read("notes.txt");
const v = await clax.draft.publish({note: "Over theirs"});
return {notes, version: v.version, files: await clax.files.list()};"#,
    )
    .following(r#"const r = await clax.draft.rebase(); return r.version;"#)
    .after(someone_publishes);
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.error, Value::Null);
    assert_eq!(seam.value["notes"], "Theirs.", "their file is in the draft");
    assert_eq!(seam.value["version"], 3);
    let drafts = seam.drafts.as_array().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0]["if_version"], 2);
    assert_eq!(seam.payload["base"], 3);
}

/// A brief's run whose first script created an artifact: a later
/// script's session knows it from the committed step (`payload.created`)
/// and refuses a second, without asking the host.
#[tokio::test]
async fn a_script_after_one_that_created_an_artifact_may_not_create_another() {
    let mut case = Case::of(
        r#"try { await clax.artifacts.create({title: "Again"}); return "created"; }
catch (e) { return [e.code, e.message]; }"#,
    )
    .following(r#"await clax.artifacts.create({title: "New"}); return 1;"#);
    case.brief = true;
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(
        seam.value,
        json!(["forbidden", "this run already created an artifact"])
    );
    assert_eq!(seam.reached_the_host, json!([]));
    assert_eq!(seam.payload["created"], false, "this step created none");
}

/// A web operation the run declared, first attempted on a host with no
/// web source and run again on a host that has one: the refusal the
/// first attempt journaled is answered from its entry, as every web
/// call given one is, and the source is not reached.
#[tokio::test]
async fn a_web_refusal_is_answered_from_its_entry_when_the_host_gains_a_source() {
    let mut case = Case::of(
        r#"try { await clax.web.search("clax"); return "searched"; }
catch (e) { return [e.code, e.message]; }"#,
    )
    .with_web("search");
    case.source_on_resume = true;
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(
        seam.value,
        json!([
            "web_disabled",
            "clax.web.search is not available to this run"
        ])
    );
    assert_eq!(seam.reached_the_source, 0, "no call reached the web source");
    assert_eq!(
        seam.calls,
        json!([{"op": "web.search", "target": null, "ok": false, "code": "web_disabled",
                "cost_milli": 0, "web": {"query": "clax"}}])
    );
    assert_eq!(seam.web_records, 0);
    // One row, written by the first attempt, in the run loop's form.
    assert_eq!(
        seam.journal,
        json!([{"step": 2, "attempt": 1, "seq": 0, "op": "web.search", "ok": false,
                "result": {"code": "web_disabled",
                           "message": "clax.web.search is not available to this run"}}])
    );
}

/// The page the sender's comment names.
const GIVEN: &str = "https://example.com/a";

/// The web source has a page at [`GIVEN`] and results for a search, and
/// the sender's comment names the page.
fn a_page_the_sender_named(env: &Env) {
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

/// A search and a fetch that reach the web source, a fetch of the same
/// page answered from the run's page store, and a fetch the rule
/// refuses: each is journaled by the session with what it charged and
/// its `bot.web` record. Run again, each is answered from its entry:
/// the source is reached once for the search and once for the fetch,
/// nothing is charged again, and the run shows the same of each call.
#[tokio::test]
async fn replayed_web_calls_behind_a_session_reach_the_source_and_charge_once() {
    let mut case = Case::of(
        r#"const found = await clax.web.search("clax");
const page = await clax.web.fetch("https://example.com/a");
const again = await clax.web.fetch("https://example.com/a", {offset: 3});
let refused;
try { await clax.web.fetch("https://example.com/unnamed"); refused = "fetched"; }
catch (e) { refused = e.code; }
return {found: found.results.length, read: page.totalChars, again: again.offset, refused,
        left: page.left};"#,
    )
    .with_web("search,fetch")
    .after(a_page_the_sender_named);
    case.source = true;
    case.source_on_resume = true;
    let seam = through_the_seam(case).await;
    let looped = in_the_run_loop(case).await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.error, Value::Null);
    assert_eq!(seam.value["found"], 1);
    assert_eq!(seam.value["again"], 3);
    assert_eq!(seam.value["refused"], "url_not_in_prior_context");
    assert_eq!(
        seam.reached_the_source, 2,
        "one search and one fetch, by the first attempt alone"
    );
    assert_eq!(seam.web_records, 4, "a `bot.web` record for each call");
    let rows = seam.journal.as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for row in rows {
        assert_eq!(row["attempt"], 1, "written by the first attempt: {row}");
        assert!(row["result"]["cost_milli"].is_i64(), "its charge: {row}");
        assert!(
            row["result"]["web"].is_object(),
            "what the run shows: {row}"
        );
    }
    assert_eq!(rows[0]["result"]["counted"], true);
    assert_eq!(rows[1]["result"]["counted"], true);
    assert!(
        rows[2]["result"].get("counted").is_none(),
        "from the page store"
    );
    let costs: Vec<&Value> = seam
        .calls
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["cost_milli"])
        .collect();
    assert_eq!(costs[2], 0, "a window of a stored page is free");
    assert_eq!(costs[3], 0, "a refused fetch is free");
    assert!(costs[0].as_i64().unwrap() > 0 && costs[1].as_i64().unwrap() > 0);
}

// ---------------------------------------------------------------------------
// A cancelled script
// ---------------------------------------------------------------------------

/// A script that edits, publishes, replies, edits again and is then
/// cancelled, once its `artifact.get` is answered.
const CANCELLED: &str = r#"await clax.files.write("extra.css", "h1 { color: blue; }");
await clax.draft.publish({note: "Before the cancel"});
await clax.threads.reply("T1", "Published.");
await clax.files.write("late.css", "p { margin: 0; }");
await clax.artifact.get();
while (true) {}"#;

/// The operation whose answer cancels [`CANCELLED`].
const CANCEL_AFTER: &str = "artifact.get";

/// Cancels a token when a script's call to `op` has been answered.
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

/// A script's CPU limit long enough that only the cancel ends it.
fn no_cpu_limit(env: &Env) {
    env.set(|s| s.script_cpu_ms = 600_000);
}

/// What [`CANCELLED`] leaves through the run loop.
async fn cancelled_in_the_run_loop() -> Left {
    let env = Env::new().await;
    let run = Case::of(CANCELLED).after(no_cpu_limit).run(&env);
    let deps = without_a_source(&env);
    let driver = DriverId::mint();
    assert_eq!(
        env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    let token = CancelToken::new();
    let sink = CancelOnCall {
        op: CANCEL_AFTER,
        token: token.clone(),
    };
    let opts = TickOpts {
        budget: None,
        cancel: token,
        events: &sink,
    };
    let out = tokio::time::timeout(PROMPTLY, clax_bot::tick(&deps, &run, &driver, opts))
        .await
        .expect("the cancel ends the script")
        .unwrap();
    assert_eq!(out.next, Next::Done(RunStatus::Stopped));
    let (row, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(row.reason.as_deref(), Some("cancelled"));
    assert_eq!(
        (steps.len(), steps[1].kind, steps[1].attempt),
        (2, StepKind::Stopped, 2),
        "the script step, begun again as the run's stopped step"
    );
    the_run_loops_step(&env, &run, 2)
}

/// What [`CANCELLED`] leaves through the seam: the session is told the
/// step was cancelled, and what it answers is committed as the run's
/// stopped step.
async fn cancelled_through_the_seam() -> Left {
    let env = Env::new().await;
    let run = Case::of(CANCELLED).after(no_cpu_limit).run(&env);
    let driver = DriverId::mint();
    let turn = env.tick(&without_a_source(&env), &run, &driver).await;
    assert_eq!((turn.step, turn.next), (Some(1), Next::Runnable));
    let how = Try {
        cancel_after: Some(CANCEL_AFTER),
        ..Try::default()
    };
    let cut = tokio::time::timeout(PROMPTLY, attempt(&env, &run, &driver, how))
        .await
        .expect("the cancel ends the script");
    assert_eq!((cut.step, cut.attempt, cut.end), (2, 1, StepEnd::Cancelled));
    // The same step, begun again as the run's stopped step.
    let stopped = env
        .store
        .begin_step(&run, &driver, 2, StepKind::Stopped)
        .unwrap();
    assert_eq!(stopped.attempt, 2);
    let mut payload = cut.parts.payload.clone();
    payload.insert("reason".into(), json!("cancelled"));
    payload.insert("calls".into(), json!(cut.calls));
    env.store
        .commit_bot_step(
            &bot_ctx(&env.run_row(&run).person),
            &run,
            &driver,
            2,
            StepCommit {
                payload: Value::Object(payload),
                draft: cut.parts.draft.clone(),
                reason: Some("cancelled".into()),
                ..StepCommit::default()
            },
        )
        .unwrap();
    assert_eq!(env.run_row(&run).status, RunStatus::Stopped);
    left(
        &env,
        &run,
        2,
        Came {
            value: cut.value,
            error: cut.error,
            calls: json!(cut.calls),
            payload: json!(cut.parts.payload),
            tool_result: json!({}),
            reached_the_source: cut.reached_the_source,
        },
    )
}

/// A script cancelled mid-way: what it published and wrote stands, the
/// stopped step keeps `published`, `replied` and `base`, and neither
/// `changed`, `created` nor any draft row is kept.
#[tokio::test]
async fn a_cancelled_script_behind_a_session_keeps_what_the_run_loop_keeps() {
    let seam = cancelled_through_the_seam().await;
    let looped = cancelled_in_the_run_loop().await;
    assert_eq!(
        seam, looped,
        "through the seam (left), the run loop (right)"
    );
    assert_eq!(seam.error, "cancelled");
    // Kept: what the script published and replied, and where its
    // publish left the draft's base.
    assert_eq!(
        seam.payload,
        json!({"published": {"version": 2}, "replied": ["<thread-1>"], "base": 2})
    );
    assert_eq!(
        seam.written,
        json!([
            {"key": "<run>:2:1", "op": "draft.publish",
             "args": ["Before the cancel", ["<thread-1>"]]},
            {"key": "<run>:2:2", "op": "threads.reply",
             "args": ["<thread-1>", "Published."]},
        ])
    );
    assert_eq!(seam.versions.as_array().unwrap().len(), 1);
    // Not kept: the paths changed, and the draft, the edit made after
    // the publish with it.
    assert_eq!(seam.draft, json!([]));
    let ops: Vec<&str> = seam
        .calls
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["op"].as_str())
        .collect();
    assert_eq!(
        ops,
        [
            "files.write",
            "draft.publish",
            "threads.reply",
            "files.write",
            "artifact.get"
        ]
    );
}
