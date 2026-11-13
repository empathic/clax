//! What the environment's policy decides ([`clax_bot::policy`]): a bound
//! it does not set stops nothing; a model call's own timeout; the size of
//! a script's tool result and of its value as the model is told it; a
//! full context window, on which the run ends unless the policy shows a
//! shorter history that a turn can be sent; and a policy that fails.
//!
//! The defaults are held elsewhere: the settings' caps by `limits.rs`,
//! the bytes a model reads and every ending by the fixtures of `golden/`.

use crate::common::*;
use clax_bot::policy::{HistoryAsk, HistoryFor, OverflowPath, RunLimits, RunPolicy, Shown};
use clax_bot::run::{Deps, RunSettings};
use clax_bot::surface::RunInfo;
use clax_bot::{Next, Wait};
use clax_core::CoreError;
use clax_sample::messages::{
    Block, Message, MessagesClient, MessagesError, RetryPolicy, Role, Timeouts,
};
use clax_store::bot_runs::{DriverId, RunStatus, Step, StepKind};
use clax_store::credits::{Unit, row_credits_milli};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a [`Fake`] answers when asked for a history.
#[derive(Clone, Copy, PartialEq)]
enum Shows {
    /// The core's projection, always.
    Core,
    /// At level `n`, the core's projection with the content of its `n`
    /// oldest tool results replaced by `[dropped]`.
    DropsOldestResults,
    /// At any level but 0, the core's projection with the content of its
    /// oldest tool result replaced by `[dropped]`: the same answer at
    /// every level.
    DropsOne,
    /// At level `n`, the core's projection with `n` bytes cut from the
    /// end of its oldest tool result.
    Shaves,
    /// The core's projection as messages of its own: the same, never
    /// less.
    TheSame,
    /// One byte less when asked on an overflow, and the core's
    /// projection for the turn after it.
    ShavesThenForgets,
    /// One byte less when asked on an overflow, and for the turn after it
    /// messages of its own that are not those.
    ShavesThenShowsAll,
    /// At any level but 0, the core's projection without its last
    /// message when that is the user's: it ends on the assistant's
    /// call, unanswered.
    EndsOnACall,
    /// The core's projection with its first text rewritten, from the
    /// first turn on.
    RewritesTheRequest,
    /// The core's projection without its assistant messages, from the
    /// first turn on.
    DropsTheAnswers,
}

/// Which of a [`Fake`]'s answers is an error.
#[derive(Clone, Copy, PartialEq)]
enum Fails {
    Never,
    Settings,
    Limits,
    NextTurn,
    Overflow,
}

/// A policy a test shapes: the settings' limits (or none, `unbounded`)
/// changed by `limits`, a history by `shows`, and an error for the first
/// `times` answers of the kind `fails`; every ask for a history is
/// recorded.
struct Fake {
    limits: Box<dyn Fn(&mut RunLimits) + Send + Sync>,
    unbounded: bool,
    shows: Shows,
    fails: Fails,
    times: usize,
    failed: AtomicUsize,
    asks: Mutex<Vec<HistoryAsk>>,
    /// Words that say a refused request was too long, beside the
    /// default's.
    too_long: Option<&'static str>,
    /// Its errors are 100,000 bytes long.
    long_error: bool,
}

impl Fake {
    fn plain() -> Fake {
        Fake {
            limits: Box::new(|_| {}),
            unbounded: false,
            shows: Shows::Core,
            fails: Fails::Never,
            times: usize::MAX,
            failed: AtomicUsize::new(0),
            asks: Mutex::default(),
            too_long: None,
            long_error: false,
        }
    }

    fn limits(f: impl Fn(&mut RunLimits) + Send + Sync + 'static) -> Arc<Fake> {
        Fake::new(f, Shows::Core)
    }

    fn shows(shows: Shows) -> Arc<Fake> {
        Fake::new(|_| {}, shows)
    }

    fn new(f: impl Fn(&mut RunLimits) + Send + Sync + 'static, shows: Shows) -> Arc<Fake> {
        Arc::new(Fake {
            limits: Box::new(f),
            shows,
            ..Fake::plain()
        })
    }

    /// A policy whose `fails` answers are errors, the first `times` of
    /// them.
    fn failing(fails: Fails, times: usize, shows: Shows) -> Arc<Fake> {
        Arc::new(Fake {
            fails,
            times,
            shows,
            ..Fake::plain()
        })
    }

    fn asks(&self) -> Vec<(HistoryFor, u32)> {
        self.asks
            .lock()
            .unwrap()
            .iter()
            .map(|a| (a.why, a.level))
            .collect()
    }

    fn fail(&self, what: Fails) -> clax_core::Result<()> {
        if self.fails == what && self.failed.fetch_add(1, Ordering::SeqCst) < self.times {
            return Err(match self.long_error {
                true => CoreError::invalid("policy", "é".repeat(50_000)),
                false => CoreError::NotFound,
            });
        }
        Ok(())
    }
}

/// `core` with `change` applied to the content of its `n` oldest tool
/// results.
fn with_results(core: &[Message], n: u32, change: impl Fn(&mut Value)) -> Vec<Message> {
    let mut left = n;
    let mut out = core.to_vec();
    for b in out.iter_mut().flat_map(|m| m.content.iter_mut()) {
        if left == 0 {
            break;
        }
        match b {
            Block::ToolResult { content, .. } => {
                change(content);
                left -= 1;
            }
            Block::Raw(v) if v["type"] == "tool_result" => {
                change(&mut v["content"]);
                left -= 1;
            }
            _ => {}
        }
    }
    out
}

fn drop_results(core: &[Message], n: u32) -> Vec<Message> {
    with_results(core, n, |content| *content = json!("[dropped]"))
}

fn shave(core: &[Message], bytes: u32) -> Vec<Message> {
    with_results(core, 1, |content| {
        let text = content.as_str().unwrap_or_default();
        *content = json!(text[..text.len() - bytes as usize]);
    })
}

impl RunPolicy for Fake {
    fn run_settings(&self, _: &RunInfo, instance: RunSettings) -> clax_core::Result<RunSettings> {
        self.fail(Fails::Settings)?;
        Ok(instance)
    }

    fn limits(&self, _: &RunInfo, settings: &RunSettings) -> clax_core::Result<RunLimits> {
        self.fail(Fails::Limits)?;
        let mut l = if self.unbounded {
            RunLimits::unbounded(settings)
        } else {
            RunLimits::of(settings)
        };
        (self.limits)(&mut l);
        Ok(l)
    }

    fn history(&self, _: &RunInfo, core: &[Message], ask: &HistoryAsk) -> clax_core::Result<Shown> {
        self.asks.lock().unwrap().push(*ask);
        let overflow = matches!(ask.why, HistoryFor::Overflow(_));
        self.fail(if overflow {
            Fails::Overflow
        } else {
            Fails::NextTurn
        })?;
        Ok(match self.shows {
            Shows::Core => Shown::Core,
            Shows::RewritesTheRequest => {
                let mut out = core.to_vec();
                out[0].content[0] = Block::text("Make the heading red.");
                Shown::Messages(out)
            }
            Shows::DropsTheAnswers => {
                let mut out = core.to_vec();
                out.retain(|m| m.role != Role::Assistant);
                Shown::Messages(out)
            }
            _ if ask.level == 0 => Shown::Core,
            Shows::DropsOldestResults => Shown::Messages(drop_results(core, ask.level)),
            Shows::DropsOne => Shown::Messages(drop_results(core, 1)),
            Shows::Shaves => Shown::Messages(shave(core, ask.level)),
            Shows::TheSame => Shown::Messages(core.to_vec()),
            Shows::ShavesThenForgets if overflow => Shown::Messages(shave(core, 1)),
            Shows::ShavesThenForgets => Shown::Core,
            Shows::ShavesThenShowsAll if overflow => Shown::Messages(shave(core, 1)),
            Shows::ShavesThenShowsAll => Shown::Messages(core.to_vec()),
            Shows::EndsOnACall => {
                let mut out = core.to_vec();
                if out.len() > 1 && out.last().is_some_and(|m| m.role == Role::User) {
                    out.pop();
                }
                Shown::Messages(out)
            }
        })
    }

    fn is_overflow(&self, error: &MessagesError) -> bool {
        let said = error.to_string();
        self.too_long.is_some_and(|words| said.contains(words))
            || clax_bot::policy::prompt_too_long(error)
    }
}

fn deps(env: &Env, policy: Arc<Fake>) -> Deps {
    env.deps().with_policy(policy)
}

fn kinds(out: &[clax_bot::TickOutcome]) -> Vec<StepKind> {
    out.iter().filter_map(|o| o.kind).collect()
}

/// Whether a reply on the run's threads says `words`.
fn told(env: &Env, words: &str) -> bool {
    env.replies().iter().any(|(_, text)| text.contains(words))
}

fn steps(env: &Env, run: &str) -> Vec<Step> {
    env.store.run_with_steps(run).unwrap().unwrap().1
}

fn payload(env: &Env, run: &str, n: usize) -> Value {
    steps(env, run)[n - 1].payload.clone().unwrap()
}

fn refused(message: &str) -> Reply {
    Reply::Status(
        400,
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": message}})
            .to_string(),
    )
}

const TOO_LONG: &str = "prompt is too long: 210000 tokens > 200000 maximum";

fn too_long() -> Reply {
    refused(TOO_LONG)
}

fn window_full() -> Reply {
    stream(&[B::Text("…")], "model_context_window_exceeded", USAGE)
}

/// A script and the finish after it: four steps.
fn script_then_finish(env: &Env) {
    env.api.push(env.script_turn("tu_1", "Look", "return 1;"));
    env.api.push(finish_turn("Done."));
}

const FOUR_STEPS: [StepKind; 4] = [
    StepKind::Turn,
    StepKind::Script,
    StepKind::Turn,
    StepKind::Finish,
];

/// The milli-credits `tokens` of `unit` cost on the runs' model.
fn credits_for(env: &Env, unit: Unit, tokens: i64) -> i64 {
    let r = env
        .store
        .model_rate("claude-sonnet-5-5", unit)
        .unwrap()
        .unwrap()
        .rate;
    row_credits_milli(
        tokens,
        r.cost_micro_usd,
        r.margin_bp,
        r.fixed_credits_milli,
        r.credit_price_micro_usd,
        r.unit.base(),
    )
    .unwrap()
}

/// The `max_tokens` request `i` asked with.
fn max_tokens(env: &Env, i: usize) -> i64 {
    env.api.requests()[i].body["max_tokens"].as_i64().unwrap()
}

// ---------------------------------------------------------------------------
// Caps the policy does not set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_with_no_cap_on_steps_goes_past_the_settings_and_finishes() {
    let env = Env::new().await;
    env.set(|s| s.max_steps = 2);
    script_then_finish(&env);
    let run = env.run();
    let policy = Fake::limits(|l| l.max_steps = None);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), FOUR_STEPS);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    // The same settings under the default policy stop it at two.
    script_then_finish(&env);
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [StepKind::Turn, StepKind::Script, StepKind::Stopped]
    );
}

#[tokio::test]
async fn a_cap_on_steps_the_policy_sets_is_the_one_that_stops_the_run() {
    let env = Env::new().await;
    script_then_finish(&env);
    let run = env.run();
    let policy = Fake::limits(|l| l.max_steps = Some(1));
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::Stopped]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    assert!(env.replies()[0].1.contains("limit of 1 steps"));
}

#[tokio::test]
async fn a_run_with_no_cap_on_credits_spends_past_the_settings_and_finishes() {
    let env = Env::new().await;
    env.set(|s| s.max_run_credits = 0);
    script_then_finish(&env);
    let run = env.run();
    let policy = Fake::limits(|l| l.max_run_credits = None);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), FOUR_STEPS);
    let r = env.run_row(&run);
    assert_eq!(r.status, RunStatus::Finished);
    assert!(r.credits_milli > 0);
    // Each turn was asked with all the settings' `max_tokens`: no cap on
    // the run's credits lessens it.
    let max = env.settings.0.lock().unwrap().max_tokens;
    for seen in env.api.requests() {
        assert_eq!(seen.body["max_tokens"], max);
    }
    // The same settings under the default policy stop it before a turn.
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert!(told(&env, "limit of 0 credits"));
}

/// What is left of a cap on credits is the cap less what the run spent:
/// a turn's `max_tokens` is no more than that pays for, which is less
/// than the cap itself would.
#[tokio::test]
async fn a_cap_on_credits_keeps_a_turn_within_what_the_run_has_left_of_it() {
    let env = Env::new().await;
    // A first turn that costs far more than a request's input does.
    env.api.push(stream(
        &[
            B::Text("Let me look."),
            B::Tool(
                "tu_1",
                "run_script",
                json!({"title": "Look", "code": "return 1;"}).to_string(),
            ),
        ],
        "tool_use",
        (400_000, 0, 0, 300),
    ));
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let d = DriverId::mint();
    let roomy = deps(&env, Fake::limits(|l| l.max_run_credits = Some(10_000)));
    env.tick(&roomy, &run, &d).await;
    env.tick(&roomy, &run, &d).await;
    let spent = env.run_row(&run).credits_milli;
    let a_request = credits_for(&env, Unit::MtokCacheWrite, 20_000);
    assert!(spent > 2 * a_request, "{spent}");
    // Twenty credits over what it spent, rounded up to a whole credit.
    let cap = u64::try_from(spent / 1000 + 21).unwrap();
    let left = i64::try_from(cap).unwrap() * 1000 - spent;
    let tight = deps(&env, Fake::limits(move |l| l.max_run_credits = Some(cap)));
    assert_eq!(env.tick(&tight, &run, &d).await.kind, Some(StepKind::Turn));
    let asked = max_tokens(&env, 1);
    let per_mtok = credits_for(&env, Unit::MtokOutput, 1_000_000);
    let pays_for = left * 1_000_000 / per_mtok;
    assert!(pays_for < 32_000, "{pays_for}");
    assert!((1024..=pays_for).contains(&asked), "{asked} of {pays_for}");
}

#[tokio::test]
async fn a_run_with_no_cap_on_minutes_goes_on_after_them_and_finishes() {
    let env = Env::new().await;
    script_then_finish(&env);
    let run = env.run();
    let policy = Fake::limits(|l| l.max_run_minutes = None);
    let deps = deps(&env, policy);
    let d = DriverId::mint();
    // Past the settings' 30 minutes before its first turn, and a day
    // later between its steps.
    env.clock.advance(31 * 60);
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    env.clock.advance(86_400);
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(kinds(&out), FOUR_STEPS[1..]);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    // The same clock under the default policy stops a run.
    let run = env.run();
    env.clock.advance(31 * 60);
    let out = env.drive(&env.deps(), &run, &d).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert!(told(&env, "limit of 30 minutes"));
}

#[tokio::test]
async fn a_cap_on_minutes_the_policy_sets_is_the_one_that_stops_the_run() {
    let env = Env::new().await;
    let five = || Fake::limits(|l| l.max_run_minutes = Some(5));
    let run = env.run();
    env.clock.advance(6 * 60);
    let out = env
        .drive(&deps(&env, five()), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert!(env.replies()[0].1.contains("limit of 5 minutes"));
    // With less left of them than a turn needs, the run stops too.
    let run = env.run();
    env.clock.advance(4 * 60 + 1);
    let out = env
        .drive(&deps(&env, five()), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert!(env.replies()[1].1.contains("limit of 5 minutes"));
    // With exactly what a turn needs left (the settings' minute), it
    // takes the turn.
    env.api.push(finish_turn("Done."));
    let run = env.run();
    env.clock.advance(4 * 60);
    let out = env
        .drive(&deps(&env, five()), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::Finish]);
}

/// A client whose own deadlines are an hour away, so that only the
/// policy's bound can end a call.
fn patient() -> MessagesClient {
    let hour = Duration::from_secs(3600);
    MessagesClient::new().with_timeouts(Timeouts {
        headers: hour,
        idle: hour,
        total: None,
    })
}

/// The policy's bound on one model call ends a call the provider never
/// answers. It is a failed call: made again as the tick's retries allow,
/// and when those are spent the run stops and says so.
///
/// The 20 ms is the policy's timeout itself, the value under test, and
/// nothing races it: the provider never answers and the client's own
/// deadlines are an hour away, so the only thing that can end either
/// call is that timeout, however late a loaded machine fires it. The
/// assertions are on what the run did once it ended, not on how long it
/// took.
#[tokio::test]
async fn a_model_call_that_outlasts_the_policys_timeout_stops_the_run() {
    let env = Env::new().await;
    env.api.push(Reply::Hang);
    env.api.push(Reply::Hang);
    let run = env.run();
    let policy = Arc::new(Fake {
        unbounded: true,
        limits: Box::new(|l| l.model_call_timeout = Some(Duration::from_millis(20))),
        ..Fake::plain()
    });
    let mut deps = deps(&env, policy);
    deps.client = patient();
    deps.retry = RetryPolicy {
        max_attempts: 2,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        ..RetryPolicy::default()
    };
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(out[0].next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_timeout"));
    assert_eq!(env.api.requests().len(), 2);
    assert!(told(
        &env,
        "the model took longer to answer than this run allows"
    ));
    // A timeout of nothing stops a run at its first turn.
    let run = env.run();
    let policy = Fake::limits(|l| l.model_call_timeout = Some(Duration::ZERO));
    let mut deps = self::deps(&env, policy);
    deps.client = patient();
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_timeout"));
    // No call is made of it.
    assert_eq!(env.api.requests().len(), 2);
}

/// A provider that fails for a moment is tried again within the tick
/// when nothing bounds the tick or the run.
#[tokio::test]
async fn a_failed_call_is_made_again_in_the_tick_of_a_run_with_no_bound() {
    let env = Env::new().await;
    env.api.push(Reply::Status(
        529,
        r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#.into(),
    ));
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let policy = Arc::new(Fake {
        unbounded: true,
        ..Fake::plain()
    });
    let mut deps = deps(&env, policy);
    deps.retry = RetryPolicy {
        max_attempts: 2,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        ..RetryPolicy::default()
    };
    let out = env.tick(&deps, &run, &DriverId::mint()).await;
    assert_eq!((out.kind, out.next), (Some(StepKind::Turn), Next::Runnable));
    assert_eq!(env.api.requests().len(), 2);
}

/// Writes `count` committed turns that said nothing, and `credits_milli`
/// spent, straight to the database as run `run`'s: a run far longer
/// than a test can drive.
fn seed_a_long_run(env: &Env, run: &str, count: i64, credits_milli: i64) {
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let mut c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let at = clax_core::time::stamp(env.clock_now());
    let tx = c.transaction().unwrap();
    for n in 1..=count {
        let mut p = json!({"assistant": [], "stop_reason": "pause_turn", "usage": {},
                           "next": {"kind": "turn", "user": []}});
        if n == 1 {
            p["system"] = json!("You are ClaxBot.");
            p["user"] = json!([{"type": "text", "text": "Make the heading blue."}]);
        }
        tx.execute(
            "INSERT INTO bot_steps (run_id, n, kind, started_at, committed_at, payload_json)
             VALUES (?1, ?2, 'turn', ?3, ?3, ?4)",
            rusqlite::params![run, n, at, p.to_string()],
        )
        .unwrap();
    }
    tx.execute(
        "UPDATE bot_runs SET credits_milli = ?2 WHERE id = ?1",
        rusqlite::params![run, credits_milli],
    )
    .unwrap();
    tx.commit().unwrap();
}

/// Nothing of the run loop's own bounds a run: with no bound set, a run
/// of 5,000 steps that spent 100,000 credits, a year after it was made,
/// takes a turn, runs a script whose result is 300 KiB, and finishes.
/// The settings' own caps are 40 steps, 200 credits and 30 minutes, and
/// the default sizes 64 KiB a value and 100 KiB a result.
///
/// The steps and the credits are written to the database, not driven
/// (5,000 ticks take over a minute), so a bound the run loop held above
/// those figures, or on something else (the bytes of a history, say),
/// would not be seen here.
#[tokio::test]
async fn a_run_with_no_bound_goes_far_past_every_default() {
    let env = Env::new().await;
    let run = env.run();
    seed_a_long_run(&env, &run, 5_000, 100_000_000);
    env.clock.advance(365 * 86_400);
    env.store
        .set_flag(
            &clax_core::audit::AuditCtx::DAEMON,
            clax_store::roles::OWNER,
            "claxbot_unenforced",
            true,
        )
        .unwrap();
    env.api
        .push(env.script_turn("tu_1", "Read", r#"return "x".repeat(300 * 1024);"#));
    env.api.push(finish_turn("Done."));
    let policy = Arc::new(Fake {
        unbounded: true,
        limits: Box::new(|l| {
            l.tool_result_bytes = 1 << 20;
            l.script.max_result_bytes = 1 << 20;
        }),
        ..Fake::plain()
    });
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), FOUR_STEPS);
    assert_eq!(out[0].step, Some(5_001));
    let r = env.run_row(&run);
    assert_eq!((r.status, r.reason), (RunStatus::Finished, None));
    assert!(r.credits_milli > 100_000_000);
    let result = payload(&env, &run, 5_002)["result"].clone();
    assert!(result.get("is_error").is_none());
    assert!(result["content"].as_str().unwrap().len() > 300 * 1024);
    for seen in env.api.requests() {
        assert_eq!(seen.body["max_tokens"], 32_000);
    }
}

// ---------------------------------------------------------------------------
// The tool result and the value's size
// ---------------------------------------------------------------------------

async fn script_result(env: &Env, policy: Arc<Fake>, code: &str) -> (Value, Vec<Seen>) {
    env.api.push(env.script_turn("tu_1", "Run", code));
    let run = env.run();
    let deps = deps(env, policy);
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
    (payload(env, &run, 2)["result"].clone(), env.api.requests())
}

#[tokio::test]
async fn the_policys_tool_result_size_is_where_a_result_is_cut() {
    const CUT: &str = "… [truncated]";
    let env = Env::new().await;
    let code = r#"throw new Error("x".repeat(5000));"#;
    let (result, _) = script_result(&env, Fake::limits(|l| l.tool_result_bytes = 2000), code).await;
    let text = result["content"].as_str().unwrap();
    assert_eq!(text.len(), 2000 + CUT.len());
    assert!(text.ends_with(CUT));
    // The default keeps all of it.
    let (result, _) = script_result(&env, Fake::limits(|_| {}), code).await;
    let text = result["content"].as_str().unwrap();
    assert!(text.len() > 5000 && !text.ends_with(CUT), "{}", text.len());
}

#[tokio::test]
async fn logs_are_cut_to_fit_the_policys_tool_result_size() {
    let env = Env::new().await;
    let code = r#"for (let i = 0; i < 8; i++) console.log("y".repeat(500)); return 1;"#;
    let (result, _) = script_result(&env, Fake::limits(|l| l.tool_result_bytes = 3000), code).await;
    let text = result["content"].as_str().unwrap();
    assert!(text.len() <= 3000, "{}", text.len());
    let v: Value = serde_json::from_str(text).unwrap();
    let logs = v["logs"].as_array().unwrap();
    assert!(logs.len() < 8, "{logs:?}");
    assert_eq!(logs.last().unwrap(), clax_bot_sandbox::LOGS_TRUNCATED);
    assert_eq!(v["value"], 1);
}

fn run_script_description(seen: &Seen) -> String {
    seen.body["tools"][0]["description"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_tool_description_says_the_size_the_sandbox_holds_a_value_to() {
    let env = Env::new().await;
    let code = r#"return "x".repeat(1600);"#;
    // Not a whole number of KiB: said in bytes, and enforced.
    let policy = Fake::limits(|l| l.script.max_result_bytes = 1536);
    let (result, seen) = script_result(&env, policy, code).await;
    let said = run_script_description(&seen[0]);
    assert!(said.contains("(JSON, at most 1536 bytes)"), "{said}");
    assert_eq!(result["is_error"], true);
    let text = result["content"].as_str().unwrap();
    assert!(text.contains("the limit is 1536 bytes"), "{text}");
    // A whole number of KiB: said in KiB, and the same value fits.
    let policy = Fake::limits(|l| l.script.max_result_bytes = 8 << 10);
    let (result, seen) = script_result(&env, policy, code).await;
    let said = run_script_description(seen.last().unwrap());
    assert!(said.contains("(JSON, at most 8 KiB)"), "{said}");
    assert!(result.get("is_error").is_none(), "{result}");
    // The default's words, which `golden/request.*.json` hold whole.
    let (_, seen) = script_result(&env, Fake::limits(|_| {}), code).await;
    let said = run_script_description(seen.last().unwrap());
    assert!(said.contains("(JSON, at most 64 KiB)"), "{said}");
}

// ---------------------------------------------------------------------------
// A full context window
// ---------------------------------------------------------------------------

/// A script whose result is large enough that dropping it is less.
const BIG: &str = r#"return "x".repeat(3000);"#;

const WINDOW_FULL: &str = "The conversation outgrew the model's context window.";
const MODEL_ERROR: &str = "I stopped: the model provider answered with an error.";

#[tokio::test]
async fn a_refused_request_stops_the_run_as_any_model_error_by_default() {
    let env = Env::new().await;
    env.api.push(too_long());
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(
        payload(&env, &run, 1),
        json!({
            "error": format!("invalid request: {TOO_LONG}"),
            "notice": format!("{MODEL_ERROR} I had not run any scripts yet."),
            "reason": "model_error",
        })
    );
    assert_eq!(steps(&env, &run).len(), 1);
}

/// A policy that answers the same messages ends the run on both paths as
/// the default does, and is asked once.
#[tokio::test]
async fn a_policy_that_does_not_show_less_ends_the_run_as_the_default_does() {
    let env = Env::new().await;
    env.api.push(window_full());
    let run = env.run();
    let policy = Fake::shows(Shows::TheSame);
    let out = env
        .drive(&deps(&env, policy.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::GiveUp]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("context_window"));
    assert_eq!(
        payload(&env, &run, 1)["next"],
        json!({"kind": "give_up", "code": "context_window", "reason": WINDOW_FULL})
    );
    assert!(payload(&env, &run, 1).get("shown").is_none());
    assert_eq!(
        policy.asks(),
        [
            (HistoryFor::NextTurn, 0),
            (HistoryFor::Overflow(OverflowPath::StopReason), 1)
        ]
    );

    env.api.push(too_long());
    let run = env.run();
    let policy = Fake::shows(Shows::TheSame);
    let out = env
        .drive(&deps(&env, policy.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
    assert_eq!(
        policy.asks(),
        [
            (HistoryFor::NextTurn, 0),
            (HistoryFor::Overflow(OverflowPath::Refused), 1)
        ]
    );
}

#[tokio::test]
async fn a_full_window_goes_on_with_the_shorter_history_a_policy_shows() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(window_full());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let policy = Fake::shows(Shows::DropsOldestResults);
    let deps = deps(&env, policy.clone());
    let d = DriverId::mint();
    for kind in [StepKind::Turn, StepKind::Script, StepKind::Turn] {
        assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(kind));
    }
    let before = steps(&env, &run);
    // The overflowing turn is a turn like another, charged, with the
    // shorter history as its next step.
    let p = payload(&env, &run, 3);
    assert_eq!(p["stop_reason"], "model_context_window_exceeded");
    assert_eq!(p["usage"]["output_tokens"], USAGE.3);
    let over = &p["next"]["overflow"];
    assert_eq!(
        p["next"],
        json!({"kind": "turn", "user": [], "overflow": {
            "level": 1,
            "path": "stop_reason",
            "sent_bytes": over["sent_bytes"],
            "shown_bytes": over["shown_bytes"],
            "shown_sha256": over["shown_sha256"],
        }})
    );
    let (sent, shown) = (
        over["sent_bytes"].as_u64().unwrap(),
        over["shown_bytes"].as_u64().unwrap(),
    );
    assert!(shown < sent, "{shown} of {sent}");
    assert!(sent - shown > 2500, "{shown} of {sent}");
    assert_eq!(env.run_row(&run).status, RunStatus::Running);

    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::Finish]);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    // The turn after it was sent the result dropped, and says what it
    // was sent: the messages the overflow's ask answered.
    let requests = env.api.requests();
    assert_eq!(requests.len(), 3);
    let results = |seen: &Seen| -> Vec<String> {
        seen.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["content"].as_array().unwrap().clone())
            .filter(|b| b["type"] == "tool_result")
            .map(|b| b["content"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(results(&requests[1])[0].len() > 3000);
    assert_eq!(results(&requests[2]), ["[dropped]"]);
    let p4 = payload(&env, &run, 4);
    assert_eq!(
        p4["shown"],
        json!({"level": 1, "bytes": shown, "sha256": over["shown_sha256"]})
    );
    // The turns before the overflow were sent the core's projection, and
    // say nothing of it.
    assert!(payload(&env, &run, 1).get("shown").is_none());
    assert!(p.get("shown").is_none());
    // The committed steps are what they were: the script's result is
    // whole in its step.
    let after = steps(&env, &run);
    assert_eq!(before[..], after[..3]);
    assert!(
        after[1].payload.as_ref().unwrap()["result"]["content"]
            .as_str()
            .unwrap()
            .len()
            > 3000
    );
    // Every call is charged, the overflowing one too.
    let (tokens, _) = env.ledger_tokens();
    assert_eq!(tokens, env.api.served());
    assert_eq!(
        policy.asks(),
        [
            (HistoryFor::NextTurn, 0),
            (HistoryFor::NextTurn, 0),
            (HistoryFor::Overflow(OverflowPath::StopReason), 1),
            (HistoryFor::NextTurn, 1),
        ]
    );
    // The run's audit has a turn for each call and nothing new.
    let turns: Vec<Value> = env
        .bot_events()
        .into_iter()
        .filter(|(k, _)| k == "bot.turn")
        .map(|(_, b)| b["stop_reason"].clone())
        .collect();
    assert_eq!(
        turns,
        ["tool_use", "model_context_window_exceeded", "tool_use"]
    );
}

#[tokio::test]
async fn a_refused_request_goes_on_with_the_shorter_history_a_policy_shows() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(too_long());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let policy = Fake::shows(Shows::DropsOldestResults);
    let out = env
        .drive(&deps(&env, policy.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    // The refused request is a turn with no answer and no usage.
    let p = payload(&env, &run, 3);
    assert_eq!(p["assistant"], json!([]));
    assert_eq!(p["stop_reason"], "prompt_too_long");
    assert_eq!(
        p["usage"],
        json!({"input_tokens": 0, "cache_read_input_tokens": 0,
               "cache_creation_input_tokens": 0, "output_tokens": 0})
    );
    let over = &p["next"]["overflow"];
    assert_eq!(
        (&over["level"], &over["path"]),
        (&json!(1), &json!("refused"))
    );
    assert_eq!(over["error"], format!("invalid request: {TOO_LONG}"));
    assert!(over["shown_bytes"].as_u64() < over["sent_bytes"].as_u64());
    assert_eq!(p["next"]["kind"], "turn");
    assert_eq!(p["next"]["user"], json!([]));
    let requests = env.api.requests();
    assert_eq!(requests.len(), 3);
    assert!(!requests[2].raw.contains("xxxxxxxx"));
    assert!(requests[1].raw.contains("xxxxxxxx"));
    assert_eq!(
        payload(&env, &run, 4)["shown"]["sha256"],
        over["shown_sha256"]
    );
    // Only the answered calls used tokens.
    let (tokens, _) = env.ledger_tokens();
    assert_eq!(tokens, env.api.served());
    assert_eq!(
        policy.asks(),
        [
            (HistoryFor::NextTurn, 0),
            (HistoryFor::NextTurn, 0),
            (HistoryFor::Overflow(OverflowPath::Refused), 1),
            (HistoryFor::NextTurn, 1),
        ]
    );
}

/// A policy says which refusals are for length: one whose provider says
/// it in other words than the default knows goes on where the default
/// stops.
#[tokio::test]
async fn a_policy_recognises_its_own_providers_refusal_for_length() {
    let other_words = || refused("Input is over the limit of this model.");
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(other_words());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let policy = Arc::new(Fake {
        shows: Shows::DropsOldestResults,
        too_long: Some("over the limit"),
        ..Fake::plain()
    });
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(out.len(), 5);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(
        payload(&env, &run, 3)["next"]["overflow"]["path"],
        "refused"
    );
    // The default's words do not hold it.
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(other_words());
    let run = env.run();
    let out = env
        .drive(
            &deps(&env, Fake::shows(Shows::DropsOldestResults)),
            &run,
            &DriverId::mint(),
        )
        .await;
    assert_eq!(
        kinds(&out),
        [StepKind::Turn, StepKind::Script, StepKind::Stopped]
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
}

/// The turn after an overflow is sent the shorter history the overflow's
/// ask answered, or nothing: a policy that shows less when asked on the
/// overflow and the core's projection for the turn after it ends the run
/// as one that never showed less, whatever bound on overflows it set.
#[tokio::test]
async fn the_turn_after_an_overflow_is_held_to_the_shorter_history() {
    let forgetful = || Fake::new(|l| l.max_overflow_retries = None, Shows::ShavesThenForgets);
    // Other messages than the overflow's ask answered are no better
    // than the core's projection.
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(too_long());
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let policy = Fake::new(|l| l.max_overflow_retries = None, Shows::ShavesThenShowsAll);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(out.last().unwrap().kind, Some(StepKind::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
    assert_eq!(env.api.requests().len(), 2);

    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    for _ in 0..5 {
        env.api.push(too_long());
    }
    let run = env.run();
    let policy = forgetful();
    let out = env
        .drive(&deps(&env, policy.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Stopped
        ]
    );
    // One refused request, and no other sent.
    assert_eq!(env.api.requests().len(), 2);
    assert_eq!(env.api.left(), 4);
    assert_eq!(
        payload(&env, &run, 4),
        json!({
            "error": format!("invalid request: {TOO_LONG}"),
            "notice": format!("{MODEL_ERROR} I ran 1 script."),
            "reason": "model_error",
        })
    );
    assert_eq!(
        policy.asks(),
        [
            (HistoryFor::NextTurn, 0),
            (HistoryFor::NextTurn, 0),
            (HistoryFor::Overflow(OverflowPath::Refused), 1),
            (HistoryFor::NextTurn, 1),
        ]
    );

    // After the stop reason, it gives up for the window.
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(window_full());
    env.api.push(window_full());
    let run = env.run();
    let out = env
        .drive(&deps(&env, forgetful()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::GiveUp
        ]
    );
    assert_eq!(env.api.requests().len(), 2);
    let p = payload(&env, &run, 4);
    assert_eq!(
        (&p["code"], &p["reason"]),
        (&json!("context_window"), &json!(WINDOW_FULL))
    );
}

/// A policy that answers a second overflow as it answered the first has
/// shown no less than the run was last sent, and the run ends.
#[tokio::test]
async fn a_policy_that_repeats_its_answer_on_the_next_overflow_ends_the_run() {
    let repeats = || Fake::new(|l| l.max_overflow_retries = None, Shows::DropsOne);
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    for _ in 0..4 {
        env.api.push(too_long());
    }
    let run = env.run();
    let out = env
        .drive(&deps(&env, repeats()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Stopped
        ]
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
    assert_eq!(env.api.requests().len(), 3);

    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    for _ in 0..4 {
        env.api.push(window_full());
    }
    let run = env.run();
    let out = env
        .drive(&deps(&env, repeats()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::GiveUp
        ]
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("context_window"));
    assert_eq!(env.api.requests().len(), 3);
}

/// Overflows that follow one another are bounded by the policy's limit,
/// whatever the policy shows: past it the run ends as it does when the
/// policy shows no less.
#[tokio::test]
async fn overflows_one_after_another_end_the_run_at_the_policys_limit() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    for _ in 0..3 {
        env.api.push(too_long());
    }
    let run = env.run();
    let policy = Fake::new(|l| l.max_overflow_retries = Some(2), Shows::Shaves);
    let out = env
        .drive(&deps(&env, policy.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Stopped
        ]
    );
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
    let levels: Vec<Value> = (3..=4)
        .map(|n| payload(&env, &run, n)["next"]["overflow"]["level"].clone())
        .collect();
    assert_eq!(levels, [1, 2]);
    // Each was shorter than the request before it, by the byte or two
    // the policy cut.
    for n in 3..=4 {
        let over = payload(&env, &run, n)["next"]["overflow"].clone();
        let cut = over["sent_bytes"].as_u64().unwrap() - over["shown_bytes"].as_u64().unwrap();
        assert!((1..=2).contains(&cut), "{cut}");
    }
    // The third overflow is not put to the policy.
    assert_eq!(
        policy.asks()[2..],
        [
            (HistoryFor::Overflow(OverflowPath::Refused), 1),
            (HistoryFor::NextTurn, 1),
            (HistoryFor::Overflow(OverflowPath::Refused), 2),
            (HistoryFor::NextTurn, 2),
        ]
    );
}

/// A step that is not an overflow starts the count over, a turn as much
/// as a script; and with no limit each answer need only be shorter.
#[tokio::test]
async fn a_step_that_fits_starts_the_count_of_overflows_over() {
    let one = || Fake::new(|l| l.max_overflow_retries = Some(1), Shows::Shaves);
    // A script between two overflows.
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(too_long());
    env.api.push(env.script_turn("tu_2", "Look", "return 1;"));
    env.api.push(too_long());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let out = env.drive(&deps(&env, one()), &run, &DriverId::mint()).await;
    assert_eq!(out.len(), 8);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(payload(&env, &run, 6)["next"]["overflow"]["level"], 2);
    assert_eq!(payload(&env, &run, 7)["shown"]["level"], 2);

    // A turn alone between them: one that called no tool, and was told
    // to.
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(too_long());
    env.api.push(text_turn("Here is what I found."));
    env.api.push(too_long());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let out = env.drive(&deps(&env, one()), &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    assert_eq!(payload(&env, &run, 5)["next"]["overflow"]["level"], 2);

    // No limit: six in a row, each a byte shorter.
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    for _ in 0..6 {
        env.api.push(too_long());
    }
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let policy = Fake::new(|l| l.max_overflow_retries = None, Shows::Shaves);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(out.len(), 10);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
}

/// Messages a policy answers that a turn cannot be sent end the run
/// `failed`, naming the invariant, and no request is made of them.
#[tokio::test]
async fn messages_that_break_what_a_turn_relies_on_fail_the_run_unsent() {
    // The request rewritten, on the first turn.
    let env = Env::new().await;
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let out = env
        .drive(
            &deps(&env, Fake::shows(Shows::RewritesTheRequest)),
            &run,
            &DriverId::mint(),
        )
        .await;
    assert_eq!(kinds(&out), [StepKind::Failed]);
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::Failed, Some("policy_history_2"))
    );
    assert!(env.api.requests().is_empty());
    let p = payload(&env, &run, 1);
    assert_eq!(
        p["error"],
        "2: the first message does not open with the request"
    );
    assert!(told(
        &env,
        "this Clax could not put together what to send the model"
    ));

    // The request rewritten after it is stored, and the answers left
    // out, so that two messages of the user's follow one another.
    for (shows, reason) in [
        (Shows::RewritesTheRequest, "policy_history_2"),
        (Shows::DropsTheAnswers, "policy_history_3"),
    ] {
        let env = Env::new().await;
        env.api.push(env.script_turn("tu_1", "Look", "return 1;"));
        env.api.push(finish_turn("never reached"));
        let run = env.run();
        let d = DriverId::mint();
        let plain = env.deps();
        env.tick(&plain, &run, &d).await;
        env.tick(&plain, &run, &d).await;
        let out = env.drive(&deps(&env, Fake::shows(shows)), &run, &d).await;
        assert_eq!(kinds(&out), [StepKind::Failed]);
        assert_eq!(env.run_row(&run).reason.as_deref(), Some(reason));
        assert_eq!(env.api.requests().len(), 1);
    }
}

/// After a refused request the turn that follows answers the user: a
/// history that ends on the assistant's call, with its result left out,
/// is not sent.
#[tokio::test]
async fn a_history_that_ends_on_an_unanswered_call_after_a_refusal_fails_the_run() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(too_long());
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let out = env
        .drive(
            &deps(&env, Fake::shows(Shows::EndsOnACall)),
            &run,
            &DriverId::mint(),
        )
        .await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Failed
        ]
    );
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::Failed, Some("policy_history_4"))
    );
    assert_eq!(env.api.requests().len(), 2);
    assert_eq!(
        payload(&env, &run, 4)["error"],
        "4: after a turn that said nothing, the last message is not the user's"
    );
}

/// A turn that is sent anything but the core's projection is priced
/// with no cached prefix: its `max_tokens` is no more than the credits
/// left pay for after all of its input at the rate of input that is not
/// read from a cache.
#[tokio::test]
async fn a_turn_on_a_shorter_history_is_priced_without_a_cached_prefix() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Read", BIG));
    env.api.push(window_full());
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let deps = deps(&env, Fake::shows(Shows::DropsOldestResults));
    let d = DriverId::mint();
    for _ in 0..3 {
        env.tick(&deps, &run, &d).await;
    }
    // Ten credits left of the week's.
    let spent = env
        .store
        .week_usage(clax_store::roles::OWNER, env.clock_now())
        .unwrap()
        .spent_milli;
    let left = 10_000;
    env.admin_sets(&[(
        clax_store::bot_settings::Setting::FreeWeeklyMilli,
        json!(spent + left),
    )]);
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let sent = env.api.requests().pop().unwrap();
    // The request's tokens as the run loop counts them (4 bytes each),
    // less a few for the `max_tokens` it was counted with.
    let tokens = i64::try_from(sent.raw.len() / 4).unwrap() - 8;
    let input = credits_for(&env, Unit::MtokCacheWrite, tokens);
    let per_mtok = credits_for(&env, Unit::MtokOutput, 1_000_000);
    let pays_for = (left - input) * 1_000_000 / per_mtok;
    let asked = sent.body["max_tokens"].as_i64().unwrap();
    assert!(pays_for < 32_000, "{pays_for}");
    assert!((1024..=pays_for).contains(&asked), "{asked} of {pays_for}");
    // A cached prefix would have paid for more: the last turn reported
    // 2,500 cached tokens.
    let cached = credits_for(&env, Unit::MtokCachedInput, 2_500)
        + credits_for(&env, Unit::MtokCacheWrite, tokens - 2_500);
    assert!((left - cached) * 1_000_000 / per_mtok > pays_for + 100);
}

// ---------------------------------------------------------------------------
// A policy that fails
// ---------------------------------------------------------------------------

/// A policy that cannot say a run's settings or limits takes no step:
/// each tick counts the failure and waits, and at the bound on failed
/// commits the run ends `failed`, naming the policy, and says so.
#[tokio::test]
async fn a_policy_that_keeps_failing_ends_the_run_failed_at_the_bound() {
    for fails in [Fails::Settings, Fails::Limits, Fails::NextTurn] {
        let env = Env::new().await;
        // The instance's caps are not the run's while its policy cannot
        // say what they are: none stops it.
        if fails != Fails::NextTurn {
            env.set(|s| s.max_steps = 0);
        }
        env.api.push(finish_turn("never reached"));
        let run = env.run();
        let deps = deps(&env, Fake::failing(fails, usize::MAX, Shows::Core));
        let d = DriverId::mint();
        for failures in 1..=2 {
            let out = env.tick(&deps, &run, &d).await;
            assert_eq!(out.next, Next::Waiting(Wait::Retry(deps.retry_later)));
            assert_eq!(out.kind, Some(StepKind::Turn));
            assert_eq!(env.run_row(&run).status, RunStatus::Running);
            assert_eq!(steps(&env, &run)[0].failures, failures);
        }
        let out = env.tick(&deps, &run, &d).await;
        assert_eq!(out.next, Next::Done(RunStatus::Failed));
        let r = env.run_row(&run);
        assert_eq!(
            (r.status, r.reason.as_deref()),
            (RunStatus::Failed, Some("policy_error"))
        );
        let p = payload(&env, &run, 1);
        assert_eq!(p["reason"], "policy_error");
        assert_eq!(p["error"], CoreError::NotFound.to_string());
        assert!(told(
            &env,
            "this Clax could not work out what this run may do"
        ));
        assert!(env.api.requests().is_empty());
    }
}

/// A policy's error is kept in the step cut to a few hundred bytes, on
/// a character boundary, and says it was cut.
#[tokio::test]
async fn a_policys_long_error_is_kept_cut() {
    let env = Env::new().await;
    let run = env.run();
    let deps = deps(
        &env,
        Arc::new(Fake {
            fails: Fails::Limits,
            long_error: true,
            ..Fake::plain()
        }),
    );
    let d = DriverId::mint();
    let mut last = env.tick(&deps, &run, &d).await;
    while last.next != Next::Done(RunStatus::Failed) {
        assert_eq!(last.next, Next::Waiting(Wait::Retry(deps.retry_later)));
        last = env.tick(&deps, &run, &d).await;
    }
    let error = payload(&env, &run, 1)["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(error.len() <= clax_bot::policy::MAX_ERROR_BYTES + "… [cut]".len());
    assert!(error.len() > 400, "{}", error.len());
    assert!(error.ends_with("é… [cut]"), "{error}");
}

/// A run whose policy fails can still be stopped: the cancel is read
/// before the policy's answer is needed.
#[tokio::test]
async fn a_run_whose_policy_fails_is_still_cancelled() {
    for fails in [Fails::Settings, Fails::Limits] {
        let env = Env::new().await;
        let run = env.run();
        let deps = deps(&env, Fake::failing(fails, usize::MAX, Shows::Core));
        let d = DriverId::mint();
        let out = env.tick(&deps, &run, &d).await;
        assert_eq!(out.next, Next::Waiting(Wait::Retry(deps.retry_later)));
        env.store
            .cancel_run(&clax_bot::run::bot_ctx(clax_store::roles::OWNER), &run)
            .unwrap();
        let out = env.tick(&deps, &run, &d).await;
        assert_eq!(out.next, Next::Done(RunStatus::Stopped));
        assert_eq!(env.run_row(&run).reason.as_deref(), Some("cancelled"));
    }
}

/// A policy that fails and then answers lets the run go on, and its
/// failures are the step's they were counted against, not the run's.
#[tokio::test]
async fn a_policy_that_recovers_lets_the_run_go_on() {
    let env = Env::new().await;
    script_then_finish(&env);
    let run = env.run();
    let deps = deps(&env, Fake::failing(Fails::Limits, 2, Shows::Core));
    let d = DriverId::mint();
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out[0].next, Next::Waiting(Wait::Retry(deps.retry_later)));
    let out = env.tick(&deps, &run, &d).await;
    assert_eq!(out.next, Next::Waiting(Wait::Retry(deps.retry_later)));
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(kinds(&out), FOUR_STEPS);
    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert!(steps(&env, &run).iter().skip(1).all(|s| s.failures == 0));
}

/// A step the model already asked for is taken though the policy fails:
/// the run finishes.
#[tokio::test]
async fn a_finish_the_model_asked_for_is_taken_though_the_policy_fails() {
    let env = Env::new().await;
    env.api.push(finish_turn("Done."));
    let run = env.run();
    let d = DriverId::mint();
    env.tick(&env.deps(), &run, &d).await;
    let deps = deps(&env, Fake::failing(Fails::Limits, usize::MAX, Shows::Core));
    let out = env.tick(&deps, &run, &d).await;
    assert_eq!(out.next, Next::Done(RunStatus::Finished));
}

/// A policy that fails when asked on an overflow has shown no less: the
/// answer the provider was paid for is committed and charged, and the
/// run ends as the default ends it, calling the model no more.
#[tokio::test]
async fn a_policy_that_fails_on_an_overflow_ends_the_run_with_the_answer_kept() {
    let env = Env::new().await;
    env.api.push(window_full());
    env.api.push(finish_turn("never reached"));
    let run = env.run();
    let policy = Fake::failing(Fails::Overflow, usize::MAX, Shows::DropsOldestResults);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::GiveUp]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("context_window"));
    assert_eq!(env.api.requests().len(), 1);
    let (tokens, charged) = env.ledger_tokens();
    assert_eq!(tokens, (USAGE.0, USAGE.1, USAGE.2, USAGE.3));
    assert_eq!(charged, 4);

    let env = Env::new().await;
    env.api.push(too_long());
    let run = env.run();
    let policy = Fake::failing(Fails::Overflow, usize::MAX, Shows::DropsOldestResults);
    let out = env
        .drive(&deps(&env, policy), &run, &DriverId::mint())
        .await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("model_error"));
}

/// A policy that keeps the run each of its methods was told of.
#[derive(Default)]
struct Told(Mutex<Vec<(&'static str, RunInfo)>>);

impl RunPolicy for Told {
    fn run_settings(&self, run: &RunInfo, instance: RunSettings) -> clax_core::Result<RunSettings> {
        self.0.lock().unwrap().push(("run_settings", run.clone()));
        Ok(instance)
    }

    fn limits(&self, run: &RunInfo, settings: &RunSettings) -> clax_core::Result<RunLimits> {
        self.0.lock().unwrap().push(("limits", run.clone()));
        Ok(RunLimits::of(settings))
    }

    fn history(&self, run: &RunInfo, _: &[Message], _: &HistoryAsk) -> clax_core::Result<Shown> {
        self.0.lock().unwrap().push(("history", run.clone()));
        Ok(Shown::Core)
    }
}

/// Each question a policy is asked is about the run the tick is of, as
/// the run loop knows it: its ID, person, model, kind, request and
/// bindings, read from its row, and nothing else. With two runs in the
/// store, each tick's policy is told of its own.
#[tokio::test]
async fn a_policy_is_told_the_run_as_the_run_loop_knows_it() {
    let env = Env::new().await;
    let policy = Arc::new(Told::default());
    let deps = env.deps().with_policy(policy.clone());
    env.api.push(finish_turn("done"));
    env.api.push(finish_turn("done"));
    let first = env.run_with_web("fetch");
    env.drive(&deps, &first, &DriverId::mint()).await;
    let second = env.run_on_both_threads("");
    env.drive(&deps, &second, &DriverId::mint()).await;
    let (first, second) = (env.info(&first), env.info(&second));
    assert_ne!(first, second);
    assert_eq!(first.kind, "clax.thread");
    assert!(first.bindings[0].has("web.fetch") && !second.bindings[0].has("web.fetch"));
    let told = policy.0.lock().unwrap();
    let of = |run: &RunInfo| -> Vec<&'static str> {
        told.iter()
            .filter(|(_, told)| told == run)
            .map(|(asked, _)| *asked)
            .collect()
    };
    // A turn's tick asks all three; the finish's tick asks no history.
    let asked = [
        "run_settings",
        "limits",
        "history",
        "run_settings",
        "limits",
    ];
    assert_eq!(of(&first), asked);
    assert_eq!(of(&second), asked);
    assert_eq!(told.len(), 2 * asked.len(), "and of no other run");
}
