//! What the environment decides about a run (design
//! `docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "The
//! environment's policy"): [`RunPolicy`], which the run loop asks instead
//! of reading a constant or a cap of its own.
//!
//! The run loop is a mechanism. How many steps, credits and minutes a run
//! may take, how large a script's result may be, how long one model call
//! may last, and what the model is sent of the run's history are answered
//! here, per run, by whatever runs ClaxBot (a daemon, the Worker, a
//! host's configuration). A bound the environment does not set
//! ([`RunLimits`], each `None`) is not replaced by one of the run loop's:
//! such a run goes on until it finishes, is cancelled, or its person's
//! credits run out. [`RunLimits::unbounded`] is the limits with no bound
//! set.
//!
//! # The defaults
//!
//! Every method has a default: the instance's [`RunSettings`] unchanged,
//! [`RunLimits::of`] them (every cap the settings name), the core's
//! projection of the history, and [`prompt_too_long`]. [`DefaultPolicy`]
//! is the policy of the defaults, and
//! [`Deps::new`](crate::run::Deps::new) starts with it.
//!
//! The trait's methods are told of a run what the run loop knows of it
//! ([`RunInfo`]: its ID, person, model, kind, request and bindings),
//! which names nothing of a surface's own.
//!
//! # A full context window
//!
//! A model reports its context window full in two ways: the stop reason
//! `model_context_window_exceeded` of an answer (which is charged), and a
//! provider's refusal of the request as too long, which
//! [`RunPolicy::is_overflow`] recognises. On either the run loop asks
//! [`RunPolicy::history`] again, with [`HistoryFor::Overflow`] and the
//! next [`HistoryAsk::level`], and the run **goes on** only when all of
//! these hold:
//!
//! - the answer is [`Shown::Messages`] of strictly fewer bytes
//!   ([`messages_bytes`]) than the messages that overflowed;
//! - it keeps what a turn relies on ([`crate::history::check_shortened`]);
//! - no more than [`RunLimits::max_overflow_retries`] overflows have
//!   followed one another with no step between that was not one;
//! - the turn after it is answered, at the same level, with the same
//!   messages (by [`messages_sha256`]): a policy answers one level the
//!   same way each time it is asked.
//!
//! Otherwise the run **ends**: a `give_up` step with code
//! `context_window` after the stop reason, a `stopped` step with reason
//! `model_error` after the refusal. That is also how it ends under the
//! defaults, and when the policy fails on the ask (the overflowing
//! answer is committed and charged first).
//!
//! So each request after an overflow is shorter, in bytes, than the one
//! that overflowed, whatever the policy answers. Bytes of JSON are the
//! measure because they always decrease and so end the run; they are not
//! a measure of what fits a model's window, which only the provider
//! knows.
//!
//! Going on commits the overflowing turn as a `turn` step whose
//! `payload.next` is `{"kind": "turn", "user": [], "overflow": {…}}`, the
//! `overflow` object holding `level`, `path` (`stop_reason` or
//! `refused`), `sent_bytes`, `shown_bytes` and `shown_sha256`, and after
//! a refusal the provider's `error`. A refused request commits a turn
//! with no assistant blocks, no usage and the stop reason
//! `prompt_too_long`. Such a turn takes a step number, so it counts
//! toward a cap on steps, and its `next` carries no nudge: a model that
//! was told to call a tool, and then overflowed, is told once more
//! before the run gives up for calling none. The run's level is the
//! number of such steps ([`Overflows::of`]); it only grows, and what a
//! level means is the policy's. Every later turn asks the policy with it
//! ([`HistoryFor::NextTurn`]), and a turn that was sent anything but the
//! core's projection records `payload.shown`: `level`, `bytes` and
//! `sha256`. No event is added: the step's `step_done` is a turn's.
//!
//! The committed steps, the journal and the audit are never rewritten. A
//! history is only what the model is shown of them.
//!
//! # What a policy's history must keep
//!
//! Messages a policy answers are checked before any is sent
//! ([`check_shown`]): the seven invariants of
//! [`crate::history::transcript`], with one allowance, that an answer a
//! full window cut off may be left out of the end
//! ([`crate::history::check_shortened`]). A policy may leave out or
//! summarise any step. It may not change the system text (it is never
//! asked for one: the system text and the first request are the steps'
//! and the turn's), drop or rewrite the request the run opens with,
//! reorder stored blocks, part a tool call from its result, or set a
//! cache breakpoint. Messages that break one end the run `failed` with
//! the reason `policy_history_<n>`, `n` the invariant's number; nothing
//! is sent to a provider.
//!
//! A turn whose history is not the core's projection is priced without
//! a cached prefix: all of its input at the rate of input that is not
//! read from a cache, since a shortened history shares no prefix with
//! the request before it.
//!
//! # A policy that fails
//!
//! A run's cancel is read before its policy is asked, so a run can
//! always be stopped. An `Err` from [`RunPolicy::run_settings`],
//! [`RunPolicy::limits`] or [`RunPolicy::history`] before a model call
//! is counted against the run's next step as a failed commit is
//! ([`RunSettings::max_commit_failures`]): the tick waits
//! (`Deps::retry_later`) and asks again, and at the bound the run ends
//! `failed` with the reason `policy_error`, tells its threads, and keeps
//! the policy's error in the step ([`error_text`]: at most
//! [`MAX_ERROR_BYTES`] of it). While it fails, the run takes no step but
//! one the model already asked for (`finish`, `give_up`), under the
//! instance's settings.
//!
//! # What a policy answers later
//!
//! The design gives a policy three more questions, none asked yet:
//! whether a run in some state takes a message (`accepts`), what the
//! model is shown of a value given where it came from (`to_model`), and a
//! call's deadline, parking and retries (`delivery`). Each becomes a
//! method of [`RunPolicy`] with a default, as the methods here have, so
//! adding one changes no implementer.
//!
//! # Bounds that are not a policy's
//!
//! A tick loads every step of its run and folds them, so a run's length
//! is bounded by the memory of what ticks it, whatever is shown. A
//! driver gives a tick a budget (an hour a round on the daemon, the
//! alarm's on the Worker), which ends a model call that outlasts it; the
//! call is then made again. [`RunSettings::lease_secs`] and the
//! concurrency settings are read from the instance's settings by the
//! claim and the driver, before any run is in hand: what
//! [`RunPolicy::run_settings`] answers for them is not read.

use crate::history::{Transcript, check_shortened};
use crate::run::RunSettings;
use crate::surface::RunInfo;
use clax_bot_sandbox::{Limits, MaybeSendSync};
use clax_core::Result;
use clax_sample::messages::{Block, Message, MessagesError, Role};
use clax_store::bot_runs::{Step, StepKind};
use serde_json::{Value, json};
use std::num::NonZeroUsize;
use std::time::Duration;

/// The most bytes of a script's `tool_result` where the environment says
/// nothing else ([`RunLimits::tool_result_bytes`]).
pub const DEFAULT_TOOL_RESULT_BYTES: usize = 100 << 10;

/// How many deltas a turn streams between two records of what it used,
/// where the environment says nothing else ([`RunLimits::observe_every`]).
pub const DEFAULT_OBSERVE_EVERY: NonZeroUsize = NonZeroUsize::new(16).unwrap();

/// How many overflows may follow one another, where the settings' caps
/// apply ([`RunLimits::of`]).
pub const DEFAULT_OVERFLOW_RETRIES: u32 = 4;

/// How many results in a row that fail their kind's check the model is
/// told of and may correct, where the settings' caps apply
/// ([`RunLimits::of`]).
pub const DEFAULT_INVALID_RESULTS: u32 = 3;

/// What bounds one run, as its environment's policy answers it for a
/// tick ([`RunPolicy::limits`]). A `None` is no bound: the run loop puts
/// none of its own in its place.
///
/// The struct is not written as a literal outside this crate. A policy
/// starts from [`RunLimits::of`] (every bound the settings name) or from
/// [`RunLimits::unbounded`] (none) and assigns what it changes. A bound
/// this struct gains later is unset in [`RunLimits::unbounded`], so a
/// policy built on it stays without bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunLimits {
    /// Committed steps after which the run stops (`run_cap`).
    pub max_steps: Option<u32>,
    /// Credits (not milli-credits) the run may spend before it stops
    /// (`run_cap`); a turn's `max_tokens` is also kept within what is
    /// left of them.
    pub max_run_credits: Option<u64>,
    /// Minutes from the run's creation after which it stops (`run_cap`),
    /// not counting the time the run was parked on a call that waits.
    /// While set, a model call is also given no longer than what is left
    /// of them, and a turn is not started with less than
    /// [`RunSettings::min_turn_budget`] left.
    pub max_run_minutes: Option<u32>,
    /// The longest one model call may take, beside the tick's budget and
    /// what is left of [`RunLimits::max_run_minutes`]. `None` sets no
    /// bound of the policy's: the call still ends with the tick's budget
    /// and at the client's own deadlines for headers and between pieces
    /// of the body, which bound a silence and not the call.
    ///
    /// A call this ends is a failed call like one the network cut: it is
    /// made again within the tick as `Deps::retry` allows. When those
    /// attempts are spent the run stops (`stopped`, reason
    /// `model_timeout`) and tells its threads the model took longer to
    /// answer than the run allows; what the attempts streamed is the
    /// instance's cost, not the person's. A timeout of zero stops the
    /// run at its first turn.
    pub model_call_timeout: Option<Duration>,
    /// The most bytes of a script's `tool_result`: its logs are cut to
    /// fit, then the text.
    pub tool_result_bytes: usize,
    /// The sandbox limits of one script. [`Limits::max_result_bytes`] is
    /// the size the `run_script` tool's description tells the model its
    /// value may have ([`bytes_text`]).
    pub script: Limits,
    /// How many deltas a turn streams between two records of what it
    /// used so far.
    pub observe_every: NonZeroUsize,
    /// How many overflows may follow one another, with no step between
    /// that was not one, before the run ends as it does when the policy
    /// shows no less. `None` leaves the rule that each request after an
    /// overflow is shorter than the one that overflowed, which ends
    /// every run on its own.
    pub max_overflow_retries: Option<u32>,
    /// How many times in a row a run's model may call `finish` with a
    /// value its kind's check refuses and be told so (`invalid_result`,
    /// as the call's tool result) to correct it: the next such call
    /// gives the run up, with the code `invalid_result`
    /// ([`crate::finish`]). `None` sets no bound of the policy's: the
    /// run's other bounds end it. It bounds the kinds of a surface that
    /// is not `clax`; a `finish` of Clax's own kinds is read as it
    /// always was, and nothing counts its refusals.
    pub max_invalid_results: Option<u32>,
    /// How many times one step may park on a call that waits. At the
    /// bound the next call that would wait is refused to its script
    /// (`park_limit`) and the step goes on. `None` sets no bound.
    pub max_parks_per_step: Option<u32>,
    /// How many runs acting for one person may be parked at once. A
    /// run that would be one more has its waiting call refused
    /// (`parked_runs_limit`). `None` sets no bound.
    pub max_parked_runs: Option<u32>,
}

/// How many times one step may park, under [`RunLimits::of`].
pub const DEFAULT_PARKS_PER_STEP: u32 = 8;
/// How many of one person's runs may be parked at once, under
/// [`RunLimits::of`].
pub const DEFAULT_PARKED_RUNS: u32 = 20;

impl RunLimits {
    /// The limits `settings` name: its three caps, no bound of its own on
    /// a model call (what is left of the run's minutes bounds it),
    /// [`DEFAULT_TOOL_RESULT_BYTES`], [`RunSettings::limits`],
    /// [`DEFAULT_OBSERVE_EVERY`], [`DEFAULT_OVERFLOW_RETRIES`] and
    /// [`DEFAULT_INVALID_RESULTS`].
    #[must_use]
    pub fn of(s: &RunSettings) -> RunLimits {
        RunLimits {
            max_steps: Some(s.max_steps),
            max_run_credits: Some(s.max_run_credits),
            max_run_minutes: Some(s.max_run_minutes),
            max_overflow_retries: Some(DEFAULT_OVERFLOW_RETRIES),
            max_invalid_results: Some(DEFAULT_INVALID_RESULTS),
            max_parks_per_step: Some(DEFAULT_PARKS_PER_STEP),
            max_parked_runs: Some(DEFAULT_PARKED_RUNS),
            ..RunLimits::unbounded(s)
        }
    }

    /// The limits with no bound set: every `Option` is `None`, and the
    /// sizes are [`RunLimits::of`]'s. A run under them ends when it
    /// finishes, gives up, is cancelled or refused, or its person's
    /// credits run out.
    #[must_use]
    pub fn unbounded(s: &RunSettings) -> RunLimits {
        RunLimits {
            max_steps: None,
            max_run_credits: None,
            max_run_minutes: None,
            model_call_timeout: None,
            tool_result_bytes: DEFAULT_TOOL_RESULT_BYTES,
            script: s.limits(),
            observe_every: DEFAULT_OBSERVE_EVERY,
            max_overflow_retries: None,
            max_invalid_results: None,
            max_parks_per_step: None,
            max_parked_runs: None,
        }
    }

    /// The run's credits cap in milli-credits.
    #[must_use]
    pub fn max_run_credits_milli(&self) -> Option<i64> {
        self.max_run_credits
            .map(|c| i64::try_from(c.saturating_mul(1000)).unwrap_or(i64::MAX))
    }

    /// The run's time cap.
    #[must_use]
    pub fn max_run_time(&self) -> Option<Duration> {
        self.max_run_minutes
            .map(|m| Duration::from_secs(u64::from(m) * 60))
    }
}

/// The lesser of two bounds, either of which may be absent.
#[must_use]
pub fn least(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// `bytes` as a tool's description says it: `"<n> KiB"` for a whole
/// number of KiB (64 KiB, 1024 KiB), else `"<n> bytes"`, so the number
/// the model reads is exactly the limit.
#[must_use]
pub fn bytes_text(bytes: u64) -> String {
    if bytes > 0 && bytes.is_multiple_of(1024) {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

/// Why a history is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HistoryFor {
    /// The run's next turn.
    NextTurn,
    /// The model reported its context window full on the messages it was
    /// just sent: less than those is asked for.
    Overflow(OverflowPath),
}

/// How a model reported its context window full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverflowPath {
    /// An answer that stopped with `model_context_window_exceeded`. It is
    /// charged, and its blocks are the last of the history.
    StopReason,
    /// The provider refused the request as too long
    /// ([`RunPolicy::is_overflow`]): nothing was answered or charged.
    Refused,
}

impl OverflowPath {
    /// The path's name in a step's payload.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            OverflowPath::StopReason => "stop_reason",
            OverflowPath::Refused => "refused",
        }
    }
}

/// What a policy is asked with the core's projection
/// ([`RunPolicy::history`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HistoryAsk {
    pub why: HistoryFor,
    /// How many times the run's history was made shorter after an
    /// overflow, counting the one being asked for when `why` is
    /// [`HistoryFor::Overflow`]. It only grows. A policy answers one
    /// level the same way each time it is asked: the turn after an
    /// overflow must be sent what the overflow's ask answered, or the
    /// run ends.
    pub level: u32,
}

impl HistoryAsk {
    /// An ask, as a tick makes one: for a policy's own tests.
    #[must_use]
    pub fn new(why: HistoryFor, level: u32) -> HistoryAsk {
        HistoryAsk { why, level }
    }
}

/// What the model is sent of a run's history.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Shown {
    /// The core's projection, as it is: every committed step.
    Core,
    /// These messages in its place, which must keep what a turn relies
    /// on ([`check_shown`]). They are sent as they are, with the cache
    /// breakpoint put on their last block.
    Messages(Vec<Message>),
}

/// The overflows of a run so far, read from its committed steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overflows {
    /// The run's level: every overflow its history was made shorter
    /// after.
    pub level: u32,
    /// Those at the end of the run, with no step after them.
    pub running: u32,
}

impl Overflows {
    /// The overflows `steps` record: the committed turns whose
    /// `payload.next.overflow` is set.
    #[must_use]
    pub fn of(steps: &[Step]) -> Overflows {
        let mut o = Overflows::default();
        for s in steps.iter().filter(|s| s.committed_at.is_some()) {
            let marked = s.kind == StepKind::Turn
                && s.payload
                    .as_ref()
                    .is_some_and(|p| p["next"]["overflow"].is_object());
            if marked {
                o.level += 1;
                o.running += 1;
            } else {
                o.running = 0;
            }
        }
        o
    }
}

/// The size of `messages` as the rule on overflows measures it: the bytes
/// of each message's JSON, summed. It orders histories so that a run of
/// overflows ends; it does not say what fits a model's window.
#[must_use]
pub fn messages_bytes(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.to_json().to_string().len()).sum()
}

/// The SHA-256 of `messages` as a step's payload names them: of the JSON
/// array of the messages.
#[must_use]
pub fn messages_sha256(messages: &[Message]) -> String {
    let all: Vec<Value> = messages.iter().map(Message::to_json).collect();
    clax_core::audit::sha256_hex(Value::Array(all).to_string().as_bytes())
}

/// What a turn's payload keeps of messages a policy answered
/// (`payload.shown`).
pub(crate) fn shown_record(level: u32, messages: &[Message]) -> Value {
    json!({
        "level": level,
        "bytes": messages_bytes(messages),
        "sha256": messages_sha256(messages),
    })
}

/// Whether `messages`, which a policy answered for a run whose committed
/// steps are `steps`, keep what a turn relies on: on a first turn, whose
/// request message is `request` and is in no step yet, that the first
/// message is the user's and opens with it (invariant 2); and
/// [`check_shortened`] of the transcript they make with `system`, the
/// system text the steps store.
///
/// # Errors
/// The first invariant broken: its number, a colon, and where.
pub fn check_shown(
    steps: &[Step],
    system: Option<&str>,
    request: &[Block],
    messages: Vec<Message>,
) -> std::result::Result<Vec<Message>, String> {
    if !request.is_empty() {
        let opens = messages.first().is_some_and(|m| {
            m.role == Role::User
                && m.content.len() >= request.len()
                && m.content
                    .iter()
                    .zip(request)
                    .all(|(a, b)| a.to_json() == b.to_json())
        });
        if !opens {
            return Err("2: the first message does not open with the request".into());
        }
    }
    let shown = Transcript {
        system: system.map(str::to_string),
        messages,
    };
    check_shortened(steps, &shown)?;
    Ok(shown.messages)
}

/// The reason a run ends `failed` with when its policy's messages break
/// the invariant `broken` names ([`check_shown`]'s error):
/// `policy_history_<n>`.
#[must_use]
pub fn history_reason(broken: &str) -> String {
    let n: String = broken.chars().take_while(char::is_ascii_digit).collect();
    format!("policy_history_{n}")
}

/// The most bytes of a policy's error a step keeps
/// ([`error_text`]).
pub const MAX_ERROR_BYTES: usize = 500;

/// `error`, a policy's error or the invariant its messages broke, as a
/// `failed` step keeps it in `payload.error`: whole when it is at most
/// [`MAX_ERROR_BYTES`], else its first bytes up to them, on a character
/// boundary, followed by `… [cut]`.
#[must_use]
pub fn error_text(error: &str) -> String {
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_string();
    }
    let mut cut = MAX_ERROR_BYTES;
    while !error.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… [cut]", &error[..cut])
}

/// The reason a run ends `failed` with when its policy failed
/// [`RunSettings::max_commit_failures`] times.
pub const POLICY_ERROR: &str = "policy_error";

/// The reason a run stops with when [`RunLimits::model_call_timeout`]
/// ended its model call and the attempts of one tick are spent.
pub const MODEL_TIMEOUT: &str = "model_timeout";

/// Whether `e` is a provider refusing a request because its prompt is
/// longer than the model's context window, by the default's reading: a
/// refusal with a 4xx status (a 400 `invalid_request_error`, or another
/// typed provider error) whose message holds, in any case, one of
/// `prompt is too long` (the Messages API), `context_length_exceeded`,
/// `maximum context length` (the Chat Completions and Responses APIs) or
/// `context window`.
///
/// The phrases were not checked against a provider's recorded answers.
/// Known to be missed: a provider's error `code` (the client keeps an
/// error's `type` and `message` only, so `context_length_exceeded`
/// matches only when the message repeats it); Gemini's refusal, whose
/// message uses none of the phrases; an error a provider sends mid-stream
/// (status 0); and the Messages API's `exceed context limit`, said when
/// the input and `max_tokens` together are over the window. A miss ends
/// the run `model_error`; a wrong hit asks the policy for a shorter
/// history, which is bounded as every overflow is. An environment that
/// knows its provider answers [`RunPolicy::is_overflow`] itself.
#[must_use]
pub fn prompt_too_long(e: &MessagesError) -> bool {
    let message = match e {
        MessagesError::InvalidRequest { message } => message,
        MessagesError::Api {
            status, message, ..
        } if (400..500).contains(status) => message,
        _ => return false,
    };
    let m = message.to_ascii_lowercase();
    [
        "prompt is too long",
        "context_length_exceeded",
        "maximum context length",
        "context window",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// How long the calls a run makes of a surface outside the process may
/// take (the design's "Deadlines, retries and what a call may
/// promise"). The run loop holds none of these figures: each is the
/// environment's, for each run and each call
/// ([`RunPolicy::delivery`]), and each may be absent. A registration's
/// own settings narrow them and never widen them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// How long a call of an operation that is not tagged `@waits` may
    /// go unanswered: its deadline, from when it is sent. `None` is no
    /// limit of the policy's; the call then has whatever limit what
    /// carries it has.
    pub call_timeout: Option<Duration>,
    /// How long the script that made a call of an operation tagged
    /// `@waits` waits for its answer in the sandbox it is in. A call
    /// not answered by then is answered as waiting
    /// ([`crate::surface::Answer::waiting`]), and its run parks.
    /// `None`: such a call is waited for until its deadline, and its
    /// run never parks on it.
    pub park_after: Option<Duration>,
    /// How long a call of an operation tagged `@waits` may go
    /// unanswered: its deadline, from when it is sent. `None` as for
    /// `call_timeout`.
    pub wait_limit: Option<Duration>,
}

impl Delivery {
    /// The daemon's defaults: 30 seconds for a call, 10 seconds before
    /// a run parks, 24 hours for a call that waits.
    pub const DAEMON: Delivery = Delivery {
        call_timeout: Some(Duration::from_secs(30)),
        park_after: Some(Duration::from_secs(10)),
        wait_limit: Some(Duration::from_secs(24 * 60 * 60)),
    };

    /// How long a call may go unanswered: `wait_limit` for one of an
    /// operation that waits, `call_timeout` for any other.
    #[must_use]
    pub fn limit(&self, waits: bool) -> Option<Duration> {
        if waits {
            self.wait_limit
        } else {
            self.call_timeout
        }
    }
}

impl Default for Delivery {
    /// [`Delivery::DAEMON`].
    fn default() -> Delivery {
        Delivery::DAEMON
    }
}

/// What the environment decides about a run. See the module
/// documentation for what each default is, what the run loop does with a
/// policy's history and what happens when a policy fails.
///
/// A policy answers the same for a run from one tick to the next unless
/// its environment changed: the `run_script` tool's description is built
/// from [`RunLimits::script`], and a description that changes mid-run
/// changes what the model was told and loses the cached prefix.
pub trait RunPolicy: MaybeSendSync {
    /// The settings of `run`'s next step, given the instance's, which the
    /// tick read from its `SettingsSource`: a kind's own model
    /// parameters or script limits, within the instance's.
    /// [`RunSettings::lease_secs`], `concurrency` and
    /// `per_person_concurrency` of the answer are not read.
    ///
    /// # Errors
    /// The policy's, counted against the run ("A policy that fails").
    fn run_settings(&self, run: &RunInfo, instance: RunSettings) -> Result<RunSettings> {
        let _ = run;
        Ok(instance)
    }

    /// What bounds `run` for this tick, given its settings
    /// ([`RunPolicy::run_settings`]).
    ///
    /// # Errors
    /// The policy's, counted against the run ("A policy that fails").
    fn limits(&self, run: &RunInfo, settings: &RunSettings) -> Result<RunLimits> {
        let _ = run;
        Ok(RunLimits::of(settings))
    }

    /// What the model is sent of `run`'s history: `core` is the core's
    /// projection of the committed steps (with the request, on the first
    /// turn), in the order they are sent. Asked before every turn, and
    /// again with [`HistoryFor::Overflow`] when the model reported its
    /// context window full, where `core` also ends with the overflowing
    /// answer's blocks.
    ///
    /// # Errors
    /// The policy's: before a turn, counted against the run ("A policy
    /// that fails"); on an overflow, the run ends as it does when the
    /// policy shows no less.
    fn history(&self, run: &RunInfo, core: &[Message], ask: &HistoryAsk) -> Result<Shown> {
        let _ = (run, core, ask);
        Ok(Shown::Core)
    }

    /// How long `run`'s call of `op` (the operation as a script names
    /// it after `clax.`: `shell.ask`) may take, for a surface outside
    /// the process: asked by that surface's provider for each call it
    /// sends. The daemon's defaults ([`Delivery::DAEMON`]), unless the
    /// policy says otherwise.
    fn delivery(&self, run: &RunInfo, op: &str) -> Delivery {
        let _ = (run, op);
        Delivery::DAEMON
    }

    /// Whether `error`, a provider's refusal of a turn's request that is
    /// not worth trying again, says the request is longer than the
    /// model's context window. The default is [`prompt_too_long`].
    fn is_overflow(&self, error: &MessagesError) -> bool {
        prompt_too_long(error)
    }
}

/// The policy of the defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultPolicy;

impl RunPolicy for DefaultPolicy {}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_sample::messages::{Block, Role};

    #[test]
    fn a_size_is_said_in_kib_only_when_it_is_a_whole_number_of_them() {
        assert_eq!(bytes_text(64 << 10), "64 KiB");
        assert_eq!(bytes_text(1024), "1 KiB");
        assert_eq!(bytes_text(1 << 20), "1024 KiB");
        assert_eq!(bytes_text(1536), "1536 bytes");
        assert_eq!(bytes_text(1000), "1000 bytes");
        assert_eq!(bytes_text(64), "64 bytes");
        assert_eq!(bytes_text(0), "0 bytes");
    }

    #[test]
    fn the_least_of_two_bounds_ignores_an_absent_one() {
        let s = Duration::from_secs;
        assert_eq!(least(Some(s(2)), Some(s(5))), Some(s(2)));
        assert_eq!(least(None, Some(s(5))), Some(s(5)));
        assert_eq!(least(Some(s(2)), None), Some(s(2)));
        assert_eq!(least(None, None), None);
    }

    fn step(n: i64, kind: StepKind, payload: Option<Value>, committed: bool) -> Step {
        Step {
            run_id: "r".into(),
            n,
            kind,
            started_at: String::new(),
            attempt: 1,
            committed_at: committed.then(String::new),
            payload,
            observed: Vec::new(),
            failures: 0,
        }
    }

    #[test]
    fn overflows_are_the_committed_turns_marked_and_run_until_another_step() {
        let over = || Some(json!({"next": {"kind": "turn", "user": [], "overflow": {"level": 1}}}));
        let plain = || Some(json!({"next": {"kind": "turn", "user": []}}));
        assert_eq!(Overflows::of(&[]), Overflows::default());
        let steps = [
            step(1, StepKind::Turn, over(), true),
            step(2, StepKind::Turn, over(), true),
        ];
        assert_eq!(
            Overflows::of(&steps),
            Overflows {
                level: 2,
                running: 2
            }
        );
        let steps = [
            step(1, StepKind::Turn, over(), true),
            step(2, StepKind::Turn, plain(), true),
            step(3, StepKind::Script, Some(json!({})), true),
            step(4, StepKind::Turn, over(), true),
            // An open step counts for nothing.
            step(5, StepKind::Turn, over(), false),
        ];
        assert_eq!(
            Overflows::of(&steps),
            Overflows {
                level: 2,
                running: 1
            }
        );
        // A turn that is not one starts the count over as a script does.
        let steps = [
            step(1, StepKind::Turn, over(), true),
            step(2, StepKind::Turn, plain(), true),
            step(3, StepKind::Turn, over(), true),
        ];
        assert_eq!(
            Overflows::of(&steps),
            Overflows {
                level: 2,
                running: 1
            }
        );
        // The mark is a turn's alone.
        let steps = [step(1, StepKind::Script, over(), true)];
        assert_eq!(Overflows::of(&steps), Overflows::default());
    }

    #[test]
    fn messages_are_measured_and_named_by_their_json() {
        let m = vec![
            Message {
                role: Role::User,
                content: vec![Block::text("a")],
            },
            Message {
                role: Role::Assistant,
                content: vec![Block::text("bc")],
            },
        ];
        let each: usize = m.iter().map(|m| m.to_json().to_string().len()).sum();
        assert_eq!(messages_bytes(&m), each);
        assert!(messages_bytes(&m[..1]) < each);
        assert_eq!(messages_sha256(&m).len(), 64);
        assert_ne!(messages_sha256(&m), messages_sha256(&m[..1]));
        let r = shown_record(3, &m);
        assert_eq!(r["level"], 3);
        assert_eq!(r["bytes"], each);
    }

    #[test]
    fn the_settings_limits_are_their_caps_and_the_default_sizes() {
        let s = RunSettings::default();
        let l = RunLimits::of(&s);
        assert_eq!(
            (l.max_steps, l.max_run_credits, l.max_run_minutes),
            (Some(40), Some(200), Some(30))
        );
        assert_eq!(l.model_call_timeout, None);
        assert_eq!(l.tool_result_bytes, 102_400);
        assert_eq!(l.script, s.limits());
        assert_eq!(l.script.max_result_bytes, 65_536);
        assert_eq!(l.observe_every.get(), 16);
        assert_eq!(l.max_overflow_retries, Some(4));
        assert_eq!(l.max_invalid_results, Some(3));
        assert_eq!(l.max_run_credits_milli(), Some(200_000));
        assert_eq!(l.max_run_time(), Some(Duration::from_secs(1_800)));
    }

    #[test]
    fn unbounded_limits_set_no_bound_and_keep_the_sizes() {
        let s = RunSettings::default();
        let u = RunLimits::unbounded(&s);
        assert_eq!(
            (u.max_steps, u.max_run_credits, u.max_run_minutes),
            (None, None, None)
        );
        assert_eq!((u.model_call_timeout, u.max_overflow_retries), (None, None));
        assert_eq!(u.max_invalid_results, None);
        assert_eq!((u.max_run_credits_milli(), u.max_run_time()), (None, None));
        // What is left is what the settings' limits have, and no more
        // than the bounds differ.
        let mut of = RunLimits::of(&s);
        of.max_steps = None;
        of.max_run_credits = None;
        of.max_run_minutes = None;
        of.max_overflow_retries = None;
        of.max_invalid_results = None;
        assert_eq!(
            (of.max_parks_per_step, of.max_parked_runs),
            (Some(8), Some(20))
        );
        of.max_parks_per_step = None;
        of.max_parked_runs = None;
        assert_eq!(of, u);
    }

    fn invalid(m: &str) -> MessagesError {
        MessagesError::InvalidRequest { message: m.into() }
    }

    fn api(status: u16, m: &str) -> MessagesError {
        MessagesError::Api {
            status,
            kind: "some_error".into(),
            message: m.into(),
        }
    }

    #[test]
    fn each_phrase_alone_says_a_request_is_too_long() {
        for said in [
            "prompt is too long: 210000 tokens > 200000 maximum",
            "Prompt Is Too Long",
            "code context_length_exceeded",
            "This model's maximum context length is 128000 tokens.",
            "Your input exceeds the context window of this model.",
        ] {
            assert!(prompt_too_long(&invalid(said)), "{said}");
            assert!(prompt_too_long(&api(400, said)), "{said}");
        }
        for said in [
            "messages: at least one",
            "prompt is long",
            "context length",
            "maximum context",
            "the window is closed",
            "exceed context limit",
            "",
        ] {
            assert!(!prompt_too_long(&invalid(said)), "{said}");
        }
    }

    #[test]
    fn only_a_refusal_with_a_4xx_status_is_one_for_length() {
        let said = "prompt is too long";
        for (status, is) in [
            (0, false),
            (399, false),
            (400, true),
            (413, true),
            (499, true),
            (500, false),
            (529, false),
        ] {
            assert_eq!(prompt_too_long(&api(status, said)), is, "{status}");
        }
        for e in [
            MessagesError::Overloaded {
                message: said.into(),
            },
            MessagesError::RateLimited {
                retry_after: None,
                message: said.into(),
            },
            MessagesError::Network {
                message: said.into(),
            },
            MessagesError::StreamCut {
                message: said.into(),
            },
            MessagesError::Auth {
                status: 401,
                message: said.into(),
            },
        ] {
            assert!(!prompt_too_long(&e), "{e:?}");
        }
        // The default policy's recogniser is this function.
        assert!(DefaultPolicy.is_overflow(&invalid(said)));
        assert!(!DefaultPolicy.is_overflow(&invalid("bad request")));
    }

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::text(text)],
        }
    }

    #[test]
    fn a_first_turns_messages_open_with_its_request() {
        let request = [Block::text("Make the heading blue.")];
        let ok = vec![user("Make the heading blue.")];
        assert_eq!(check_shown(&[], None, &request, ok.clone()), Ok(ok));
        let mut more = vec![user("Make the heading blue.")];
        more[0].content.push(Block::text("A note."));
        assert!(check_shown(&[], None, &request, more).is_ok());
        for bad in [
            vec![],
            vec![user("Make the heading red.")],
            vec![user("A note."), user("Make the heading blue.")],
            vec![Message {
                role: Role::Assistant,
                content: vec![Block::text("Make the heading blue.")],
            }],
        ] {
            let e = check_shown(&[], None, &request, bad).unwrap_err();
            assert!(e.starts_with("2: "), "{e}");
            assert_eq!(history_reason(&e), "policy_history_2");
        }
        // With no request to add, the steps' own invariants are all
        // there is: here, that roles alternate.
        let e = check_shown(&[], None, &[], vec![user("a"), user("b")]).unwrap_err();
        assert_eq!(history_reason(&e), "policy_history_3");
    }

    #[test]
    fn a_long_error_is_cut_on_a_character_boundary_and_says_so() {
        assert_eq!(error_text("not found"), "not found");
        let exactly = "e".repeat(MAX_ERROR_BYTES);
        assert_eq!(error_text(&exactly), exactly);
        let over = format!("{exactly}f");
        assert_eq!(error_text(&over), format!("{exactly}… [cut]"));
        // A character that straddles the bound is left out whole.
        let wide = format!("{}é{}", "e".repeat(MAX_ERROR_BYTES - 1), "e".repeat(50));
        let cut = error_text(&wide);
        assert_eq!(cut, format!("{}… [cut]", "e".repeat(MAX_ERROR_BYTES - 1)));
    }

    /// The run loop asks the policy: `run.rs` reads each bound, and
    /// `script.rs` each size, from the policy's answer (`limits.*`). Held
    /// on the sources' text without its white space. That the run loop
    /// holds no bound of its own is held by a run
    /// (`tests/policy.rs`, `a_run_with_no_bound_goes_far_past_every_default`).
    #[test]
    fn the_run_loop_reads_each_bound_and_size_from_the_policys_limits() {
        // The code before the file's tests (a `cfg(test)` on a field or
        // a branch is part of the code).
        let dense = |src: &str| -> String {
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap_or(src);
            code.split_whitespace().collect()
        };
        let (run, script) = (
            dense(include_str!("run.rs")),
            dense(include_str!("script.rs")),
        );
        for read in [
            "limits.max_steps",
            "limits.max_run_credits",
            "limits.max_run_minutes",
            "limits.model_call_timeout",
            "limits.observe_every",
            "limits.max_overflow_retries",
            "limits.max_invalid_results",
            "limits.script.max_result_bytes",
        ] {
            assert!(run.contains(read), "run.rs does not read `{read}`");
        }
        for read in [
            "limits.tool_result_bytes",
            "limits.script",
            "limits.max_parks_per_step",
            "limits.max_parked_runs",
        ] {
            assert!(script.contains(read), "script.rs does not read `{read}`");
        }
    }
}
