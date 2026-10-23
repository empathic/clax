//! The run state machine (spec §4.2–4.3, §7.4): [`tick`] loads a run,
//! enforces what must hold before any step ([`refusal`]: the flag,
//! credits, a key, the model's price; the run's caps; a cancel), executes
//! exactly one step and commits it with its usage, Toolpath events and
//! draft changes in one transaction.
//!
//! # Steps
//!
//! - `turn`: one Messages API call over the run's history (the cached
//!   system prompt, the request, every turn and tool result so far), with
//!   the tools `run_script`, `finish` and `give_up`, one call a turn, its
//!   `max_tokens` no more than the person's credits can pay. Its payload
//!   keeps the assistant's blocks verbatim and decides the next step
//!   (`payload.next`).
//! - `script`: the turn's `run_script` code in the sandbox (see
//!   [`crate::script`]).
//! - `finish`, `give_up`, `stopped`, `failed`: terminal; each but a
//!   cancelled stop tells the run's threads why ([`OpCtx`] notices).
//!
//! # What a model call costs
//!
//! A turn's usage is recorded on its open step as it streams
//! ([`Store::observe_step`]), so an attempt that never commits is still
//! accounted for: when the step commits, every earlier attempt's usage is
//! written as rows the instance absorbs (cost kept, no credits: a crash, a
//! lost claim, a cut stream it made again), and a call a person cancelled
//! is charged to them.
//!
//! # Not committing
//!
//! A tick commits nothing and leaves the step open when the driver lost the
//! run's claim ([`Wait::NotOurs`]), when the sandbox could not take the
//! script ([`Wait::Stalled`]), when the model stayed unreachable
//! ([`Wait::Retry`]) or when too little of the tick's budget is left for a
//! turn ([`Wait::Budget`]). A step whose commit fails
//! `max_commit_failures` times ends the run `failed`.

use crate::events::{Event, EventSink, step_summary};
use crate::host::{BotHost, NOTICE_SEQ, OpCtx, run_plan};
use crate::prompt;
use clax_bot_sandbox::{CancelToken, Limits, MaybeSendSync, Sandbox};
use clax_core::audit::{Actor, AuditCtx, AuditRecord, Via};
use clax_core::gitctx::GitField;
use clax_core::working::Clock;
use clax_core::{CoreError, Result};
use clax_sample::messages::{
    ApiKey, AssistantMessage, Block, CacheControl, Delta, Message, MessagesClient, MessagesError,
    MessagesRequest, RetryPolicy, Role, StopReason, SystemBlock, Thinking, Timeouts, Tool,
    ToolCall, ToolChoice, Usage,
};
use clax_store::Store;
use clax_store::bot_runs::{
    Committed, DriverId, Run, RunStatus, Step, StepCommit, StepKind, call_record, turn_record,
};
use clax_store::bot_settings::Setting;
use clax_store::credits::{
    KeySource, LINE_DO_REQUESTS, LINE_HARNESS_FEE, LINE_TOOLPATH_STORAGE, Metered, TOKEN_UNITS,
    Unit, row_credits_milli, token_line, week_end, week_start,
};
use clax_store::secrets::{Kek, SecretSlot};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Settings and dependencies
// ---------------------------------------------------------------------------

/// What a run is allowed, read before every tick (an admin may change it
/// while runs wait). [`RunSettings::from_store`] reads the instance
/// settings the admin pages set; the daemon lays its `[bot]` config over
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSettings {
    /// The instance runs ClaxBot at all.
    pub enabled: bool,
    pub base_url: String,
    pub max_tokens: u32,
    pub max_steps: u32,
    /// Credits (not milli-credits) one run may spend.
    pub max_run_credits: u64,
    pub max_run_minutes: u32,
    /// Runs one driver drives at once.
    pub concurrency: u32,
    /// Runs of one person one driver drives at once.
    pub per_person_concurrency: u32,
    pub script_cpu_ms: u64,
    pub script_memory_mib: u32,
    pub script_max_calls: u32,
    /// How long a driver's claim lasts without renewal.
    pub lease_secs: i64,
    /// How often a step renews its claim (and looks for a cancel) while
    /// it works.
    pub renew_every: Duration,
    /// Charge the Durable Object request and Toolpath storage lines per
    /// step (the hosted Worker's costs).
    pub infra_lines: bool,
    /// `output_config.effort` of each turn; `None` leaves the model's.
    pub effort: Option<String>,
    /// The least time a model call is started with: a tick with less of
    /// its budget left waits for a fresh one ([`Wait::Budget`]), and a run
    /// with less time left stops (`run_cap`). The Worker's alarm gives a
    /// tick at least this.
    pub min_turn_budget: Duration,
    /// The fewest `max_tokens` a turn is asked with: a turn the person's
    /// credits cannot pay this much output for stops the run.
    pub min_turn_tokens: u32,
    /// Failed commits of one step before the run ends `failed`.
    pub max_commit_failures: i64,
}

impl Default for RunSettings {
    fn default() -> Self {
        RunSettings {
            enabled: true,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 32_000,
            max_steps: 40,
            max_run_credits: 200,
            max_run_minutes: 30,
            concurrency: 4,
            per_person_concurrency: 1,
            script_cpu_ms: 2_000,
            script_memory_mib: 64,
            script_max_calls: 200,
            lease_secs: 60,
            renew_every: Duration::from_secs(20),
            infra_lines: false,
            effort: None,
            min_turn_budget: Duration::from_secs(60),
            min_turn_tokens: 1_024,
            max_commit_failures: 3,
        }
    }
}

impl RunSettings {
    /// The instance settings stored in `store` (their defaults where
    /// unset), with the defaults above for what the store does not keep.
    ///
    /// # Errors
    /// The store's.
    pub fn from_store(store: &Store) -> Result<RunSettings> {
        let int = |s: Setting| -> Result<u64> { Ok(store.bot_setting(s)?.as_u64().unwrap_or(0)) };
        let d = RunSettings::default();
        Ok(RunSettings {
            base_url: store
                .bot_setting(Setting::BaseUrl)?
                .as_str()
                .unwrap_or(&d.base_url)
                .to_string(),
            max_tokens: u32::try_from(int(Setting::MaxTokens)?).unwrap_or(d.max_tokens),
            max_steps: u32::try_from(int(Setting::MaxSteps)?).unwrap_or(d.max_steps),
            max_run_credits: int(Setting::MaxRunCredits)?,
            max_run_minutes: u32::try_from(int(Setting::MaxRunMinutes)?)
                .unwrap_or(d.max_run_minutes),
            concurrency: u32::try_from(int(Setting::Concurrency)?).unwrap_or(d.concurrency),
            script_cpu_ms: int(Setting::ScriptCpuMs)?,
            script_memory_mib: u32::try_from(int(Setting::ScriptMemoryMib)?)
                .unwrap_or(d.script_memory_mib),
            script_max_calls: u32::try_from(int(Setting::ScriptMaxCalls)?)
                .unwrap_or(d.script_max_calls),
            ..d
        })
    }

    /// The sandbox limits of one script.
    #[must_use]
    pub fn limits(&self) -> Limits {
        Limits {
            cpu_ms: self.script_cpu_ms,
            memory_bytes: u64::from(self.script_memory_mib) << 20,
            max_calls: self.script_max_calls,
            ..Limits::default()
        }
    }
}

/// Where a tick reads its settings.
pub trait SettingsSource: MaybeSendSync {
    /// The settings now.
    ///
    /// # Errors
    /// The source's.
    fn settings(&self) -> Result<RunSettings>;
}

impl SettingsSource for RunSettings {
    fn settings(&self) -> Result<RunSettings> {
        Ok(self.clone())
    }
}

/// The model keys a run may use.
#[derive(Clone, Default)]
pub struct Keys {
    /// Opens stored keys (the instance's, people's own).
    pub kek: Option<Kek>,
    /// The instance key from the environment (`[bot] api_key_env`), which
    /// takes precedence over a stored instance key (spec §8.1) while the
    /// base URL in effect is the owner's ([`Store::base_url_is_owners`]):
    /// it is never sent to a URL an admin set.
    pub env_instance: Option<ApiKey>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("kek", &self.kek.is_some())
            .field("env_instance", &self.env_instance.is_some())
            .finish()
    }
}

/// A point where a test makes a tick stop as if its process died.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    /// The step's work is done (the stream ended, the script returned) and
    /// nothing is committed.
    BeforeCommit { step: i64, kind: StepKind },
    /// The step is committed; the next is not begun.
    AfterCommit { step: i64, kind: StepKind },
}

/// A crash hook: `true` stops the tick at that point.
#[cfg(not(target_arch = "wasm32"))]
pub type CrashHook = Arc<dyn Fn(CrashPoint) -> bool + Send + Sync>;
/// A crash hook: `true` stops the tick at that point.
#[cfg(target_arch = "wasm32")]
pub type CrashHook = Arc<dyn Fn(CrashPoint) -> bool>;

/// What a tick works with.
pub struct Deps {
    pub store: Arc<Store>,
    pub client: MessagesClient,
    pub sandbox: Arc<dyn Sandbox>,
    pub host: Arc<dyn BotHost>,
    pub settings: Arc<dyn SettingsSource>,
    pub keys: Keys,
    /// The clock of credits weeks, run time and the driver's waits (the
    /// store's own, in production).
    pub clock: Arc<dyn Clock>,
    /// Retries of a model call within one tick.
    pub retry: RetryPolicy,
    /// How long a run waits after the model stayed unreachable.
    pub retry_later: Duration,
    /// For tests: where to stop as if crashed.
    pub crash: Option<CrashHook>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("keys", &self.keys)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Deps with the default retry policy, a 30 s wait after the model
    /// stays unreachable, and no crash hook.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<Store>,
        client: MessagesClient,
        sandbox: Arc<dyn Sandbox>,
        host: Arc<dyn BotHost>,
        settings: Arc<dyn SettingsSource>,
        keys: Keys,
        clock: Arc<dyn Clock>,
    ) -> Deps {
        Deps {
            store,
            client,
            sandbox,
            host,
            settings,
            keys,
            clock,
            retry: RetryPolicy::default(),
            retry_later: Duration::from_secs(30),
            crash: None,
        }
    }

    pub(crate) fn crashes(&self, at: CrashPoint) -> bool {
        self.crash.as_ref().is_some_and(|h| h(at))
    }
}

/// The audit context a run's steps act under: ClaxBot for its person.
#[must_use]
pub fn bot_ctx(person: &str) -> AuditCtx {
    AuditCtx {
        actor: Actor::Bot {
            on_behalf_of: person.to_string(),
        },
        via: Via::Daemon,
        git: GitField::Absent,
        call: None,
    }
}

// ---------------------------------------------------------------------------
// Refusals and keys
// ---------------------------------------------------------------------------

/// Why ClaxBot may not start (or continue) a run for a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The instance does not run ClaxBot (`[bot] enabled = false`).
    Disabled,
    /// The person's `claxbot` flag is off.
    NoFlag,
    /// The week's credits are spent and the person is not unenforced.
    CreditsExhausted,
    /// No model key: neither the person's own nor the instance's.
    NotConfigured,
    /// The model has no cost lines.
    ModelNotPriced,
}

impl Refusal {
    /// The reason's code (`bot.refused`'s `reason`, a stopped run's).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Disabled => "disabled",
            Refusal::NoFlag => "no_flag",
            Refusal::CreditsExhausted => "credits_exhausted",
            Refusal::NotConfigured => "not_configured",
            Refusal::ModelNotPriced => "model_not_priced",
        }
    }
}

/// Why a run for `person` on `model` may not start (or take its next
/// step) now, or `None` when it may: the one place that decides it, for
/// the send path (which records `bot.refused`) and every tick.
///
/// # Errors
/// The store's.
pub fn refusal(
    store: &Store,
    settings: &RunSettings,
    keys: &Keys,
    clock: &dyn Clock,
    person: &str,
    model: &str,
) -> Result<Option<Refusal>> {
    if !settings.enabled {
        return Ok(Some(Refusal::Disabled));
    }
    let flags = store.flags_of(person)?;
    if !flags.claxbot {
        return Ok(Some(Refusal::NoFlag));
    }
    if !flags.claxbot_unenforced && store.week_usage(person, clock.now())?.exhausted {
        return Ok(Some(Refusal::CreditsExhausted));
    }
    // A base URL stored before the hosted policy applied is refused here,
    // at the point of use, as at its write.
    if !store.base_url_policy().allows(&settings.base_url)
        || choose_key(store, keys, person, &settings.base_url)?.is_none()
    {
        return Ok(Some(Refusal::NotConfigured));
    }
    for unit in TOKEN_UNITS {
        if store.cost_line(&token_line(model, unit))?.is_none() {
            return Ok(Some(Refusal::ModelNotPriced));
        }
    }
    Ok(None)
}

/// A key a turn runs on, where it came from, and its slot when stored.
pub type ChosenKey = (ApiKey, KeySource, Option<SecretSlot>);

/// The key a turn for `person` sends to `base_url`: their own when set
/// and in use and it opens, else the instance's (the environment's first,
/// while the base URL is the owner's). A stored key goes only to the
/// origin it was set for ([`Store::open_for_use`]): a person's own key set
/// for another is not replaced by the instance's (`None`: the run is
/// refused `not_configured`, and the person is told to set it again).
///
/// # Errors
/// The store's.
pub fn choose_key(
    store: &Store,
    keys: &Keys,
    person: &str,
    base_url: &str,
) -> Result<Option<ChosenKey>> {
    let mismatch = |e: &CoreError| matches!(e, CoreError::Invalid { code, .. } if *code == "key_host_mismatch");
    if store.key_source_of(person)? == KeySource::Own
        && let Some(kek) = &keys.kek
    {
        let slot = SecretSlot::Own(person.to_string());
        match store.open_for_use(kek, &slot, base_url) {
            Ok(Some(s)) => {
                return Ok(Some((ApiKey::new(s.expose()), KeySource::Own, Some(slot))));
            }
            Err(e) if mismatch(&e) => return Ok(None),
            _ => {}
        }
    }
    if let Some(k) = &keys.env_instance
        && store.base_url_is_owners()?
    {
        return Ok(Some((k.clone(), KeySource::Instance, None)));
    }
    if let Some(kek) = &keys.kek
        && let Ok(Some(s)) = store.open_for_use(kek, &SecretSlot::Instance, base_url)
    {
        return Ok(Some((
            ApiKey::new(s.expose()),
            KeySource::Instance,
            Some(SecretSlot::Instance),
        )));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Why a tick committed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// Another driver holds the run, or took it over mid-step.
    NotOurs,
    /// The sandbox could not take the script: try again later, uncharged.
    Stalled,
    /// The model stayed unreachable; try again after this.
    Retry(Duration),
    /// Too little of the tick's budget is left to start a turn
    /// ([`RunSettings::min_turn_budget`]): tick it again with a fresh one.
    Budget,
    /// A [`CrashPoint`] hook stopped the tick.
    Crashed,
}

/// What the driver does next with the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Tick it again.
    Runnable,
    Waiting(Wait),
    /// It has ended.
    Done(RunStatus),
}

/// What one tick did.
#[derive(Clone, Debug, PartialEq)]
pub struct TickOutcome {
    pub next: Next,
    /// The step it executed (committed or not).
    pub step: Option<i64>,
    pub kind: Option<StepKind>,
    /// What the commit charged.
    pub credits_milli: i64,
}

impl TickOutcome {
    pub(crate) fn wait(w: Wait, step: Option<i64>, kind: Option<StepKind>) -> TickOutcome {
        TickOutcome {
            next: Next::Waiting(w),
            step,
            kind,
            credits_milli: 0,
        }
    }
}

/// How one tick runs.
pub struct TickOpts<'a> {
    /// The most wall time the tick may take (the Worker alarm's budget);
    /// `None` for no bound but the run's own. A turn starts only with at
    /// least [`RunSettings::min_turn_budget`] of it left.
    pub budget: Option<Duration>,
    /// Cancels the step at once (the driver's handle on a cancel).
    pub cancel: CancelToken,
    pub events: &'a dyn EventSink,
}

/// Whether `e` says the driver no longer holds the run (another took it
/// over, or the run moved on).
pub(crate) fn fenced(e: &CoreError) -> bool {
    matches!(
        e,
        CoreError::Invalid {
            code: "run_not_claimed" | "step_mismatch",
            ..
        }
    )
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// What the committed steps add up to.
#[derive(Debug, Default)]
pub(crate) struct History {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// The next step the last committed step asked for.
    pub next: Value,
    /// The last turn's next step was the nudge.
    pub nudged: bool,
    pub replied: BTreeSet<String>,
    /// The version the draft is based on, when a script moved it (a
    /// publish, a rebase).
    pub base: Option<u32>,
    pub published: Option<u32>,
    pub created: bool,
    pub changed: BTreeSet<String>,
    pub scripts: u32,
    pub committed: usize,
    /// The last turn's usage.
    pub last_usage: Option<Usage>,
}

fn blocks(v: &Value) -> Vec<Block> {
    v.as_array()
        .map(|a| a.iter().cloned().map(Block::from_json).collect())
        .unwrap_or_default()
}

pub(crate) fn push(messages: &mut Vec<Message>, role: Role, content: Vec<Block>) {
    if content.is_empty() {
        return;
    }
    match messages.last_mut() {
        Some(m) if m.role == role => m.content.extend(content),
        _ => messages.push(Message { role, content }),
    }
}

fn usage_of(v: &Value) -> Usage {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    Usage {
        input_tokens: n("input_tokens"),
        cache_creation_input_tokens: n("cache_creation_input_tokens"),
        cache_read_input_tokens: n("cache_read_input_tokens"),
        output_tokens: n("output_tokens"),
    }
}

impl History {
    fn of(steps: &[Step]) -> History {
        let mut h = History {
            next: json!({"kind": "turn"}),
            ..History::default()
        };
        for s in steps.iter().filter(|s| s.committed_at.is_some()) {
            h.committed += 1;
            let p = s.payload.clone().unwrap_or(Value::Null);
            if let Some(b) = p["base"].as_u64() {
                h.base = u32::try_from(b).ok();
            }
            if let Some(v) = p["published"]["version"].as_u64() {
                h.published = u32::try_from(v).ok();
            }
            match s.kind {
                StepKind::Turn => {
                    if let Some(sys) = p["system"].as_str() {
                        h.system = Some(sys.to_string());
                    }
                    push(&mut h.messages, Role::User, blocks(&p["user"]));
                    push(&mut h.messages, Role::Assistant, blocks(&p["assistant"]));
                    let next = p["next"].clone();
                    if next["kind"] == "turn" {
                        push(&mut h.messages, Role::User, blocks(&next["user"]));
                    }
                    h.nudged = next["nudge"].as_bool().unwrap_or(false);
                    h.last_usage = Some(usage_of(&p["usage"]));
                    h.next = next;
                }
                StepKind::Script => {
                    h.scripts += 1;
                    push(
                        &mut h.messages,
                        Role::User,
                        vec![Block::from_json(p["result"].clone())],
                    );
                    for t in p["replied"].as_array().into_iter().flatten() {
                        if let Some(t) = t.as_str() {
                            h.replied.insert(t.to_string());
                        }
                    }
                    for c in p["changed"].as_array().into_iter().flatten() {
                        if let Some(c) = c.as_str() {
                            h.changed.insert(c.to_string());
                        }
                    }
                    h.created |= p["created"].as_bool().unwrap_or(false);
                    h.next = json!({"kind": "turn"});
                }
                _ => h.next = json!({"kind": "end"}),
            }
        }
        h
    }
}

// ---------------------------------------------------------------------------
// Turns
// ---------------------------------------------------------------------------

fn stop_reason_str(r: Option<&StopReason>) -> String {
    match r {
        None => "none".into(),
        Some(StopReason::EndTurn) => "end_turn".into(),
        Some(StopReason::ToolUse) => "tool_use".into(),
        Some(StopReason::MaxTokens) => "max_tokens".into(),
        Some(StopReason::StopSequence) => "stop_sequence".into(),
        Some(StopReason::PauseTurn) => "pause_turn".into(),
        Some(StopReason::Refusal) => "refusal".into(),
        Some(StopReason::ModelContextWindowExceeded) => "model_context_window_exceeded".into(),
        Some(StopReason::Other(s)) => s.clone(),
    }
}

/// The tools a turn offers. Their input streams as the model writes it
/// (eager), so a script shows in the run panel while it is written; input
/// that does not parse comes back as [`ToolCall::Invalid`] and is answered
/// with an error result.
fn tools() -> Vec<Tool> {
    let tool = |name: &str, description: &str, schema: Value| Tool {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        cache_control: None,
        eager_input_streaming: true,
    };
    vec![
        tool(
            "run_script",
            "Runs JavaScript against the clax API. `code` is the body of an async function; \
             what it returns (JSON, at most 64 KiB) comes back with its console.log lines, \
             the calls it made and any error.",
            json!({"type": "object", "properties": {
                "title": {"type": "string", "description": "What the script does, in a few words."},
                "code": {"type": "string", "description": "The body of an async function."}},
                "required": ["title", "code"], "additionalProperties": false}),
        ),
        tool(
            "finish",
            "Ends the run: the work is done. The summary tells the person what you did.",
            json!({"type": "object", "properties": {"summary": {"type": "string"}},
                   "required": ["summary"], "additionalProperties": false}),
        ),
        tool(
            "give_up",
            "Ends the run without finishing, saying why.",
            json!({"type": "object", "properties": {"reason": {"type": "string"}},
                   "required": ["reason"], "additionalProperties": false}),
        ),
    ]
}

/// The history with a cache breakpoint on its last block.
fn with_breakpoint(mut messages: Vec<Message>) -> Vec<Message> {
    if let Some(b) = messages.last_mut().and_then(|m| m.content.last_mut()) {
        match b {
            Block::Text { cache_control, .. } | Block::ToolResult { cache_control, .. } => {
                *cache_control = Some(CacheControl::default());
            }
            Block::Raw(v) if matches!(v["type"].as_str(), Some("text" | "tool_result")) => {
                v["cache_control"] = json!({"type": "ephemeral"});
            }
            _ => {}
        }
    }
    messages
}

/// What a turn's reply asks for next.
fn next_after(msg: &AssistantMessage, nudged: bool) -> Value {
    let calls = msg.tool_calls();
    let err = |id: &str, m: &str| Block::tool_result(id, m, true).to_json();
    match msg.stop_reason {
        Some(StopReason::Refusal) => {
            return json!({"kind": "give_up", "code": "refusal",
                          "reason": "The model declined the request."});
        }
        Some(StopReason::ModelContextWindowExceeded) => {
            return json!({"kind": "give_up", "code": "context_window",
                          "reason": "The conversation outgrew the model's context window."});
        }
        Some(StopReason::PauseTurn) => return json!({"kind": "turn", "user": []}),
        Some(StopReason::MaxTokens) => {
            let user: Vec<Value> = if calls.is_empty() {
                vec![
                    Block::text(
                        "Your reply was cut off at max_tokens. Continue more briefly and call a tool.",
                    )
                    .to_json(),
                ]
            } else {
                calls
                    .iter()
                    .map(|c| {
                        err(
                            c.id(),
                            "Your reply was cut off at max_tokens, so this call did not run. \
                             Make it shorter (a smaller script, or several).",
                        )
                    })
                    .collect()
            };
            return json!({"kind": "turn", "user": user});
        }
        _ => {}
    }
    match calls.as_slice() {
        [] if nudged => json!({"kind": "give_up", "code": "no_tool_call",
                               "reason": "The model stopped calling tools."}),
        [] => {
            json!({"kind": "turn", "nudge": true, "user": [Block::text(prompt::NUDGE).to_json()]})
        }
        [call] => match call {
            ToolCall::Invalid { .. } => json!({"kind": "turn",
                "user": [call.invalid_result().expect("an invalid call").to_json()]}),
            ToolCall::Valid { id, name, input } => {
                let s = |k: &str| input[k].as_str().map(str::to_string);
                match (
                    name.as_str(),
                    s("title"),
                    s("code"),
                    s("summary"),
                    s("reason"),
                ) {
                    ("run_script", Some(title), Some(code), _, _) => {
                        json!({"kind": "script", "tool_use_id": id, "title": title, "code": code})
                    }
                    ("finish", _, _, Some(summary), _) => {
                        json!({"kind": "finish", "summary": summary})
                    }
                    ("give_up", _, _, _, Some(reason)) => {
                        json!({"kind": "give_up", "code": "model", "reason": reason})
                    }
                    ("run_script" | "finish" | "give_up", ..) => {
                        json!({"kind": "turn", "user": [err(
                            id,
                            &format!("The input of {name} does not match its schema; nothing ran."),
                        )]})
                    }
                    _ => json!({"kind": "turn", "user": [err(
                        id,
                        &format!("There is no tool {name}; use run_script, finish or give_up."),
                    )]}),
                }
            }
        },
        many => json!({"kind": "turn", "user": many.iter().map(|c| err(
            c.id(),
            "Make one tool call per reply; none of these ran.",
        )).collect::<Vec<_>>()}),
    }
}

/// Usage rows for `u` on `model`, a row per non-zero count.
fn token_rows(model: &str, u: Usage, ks: KeySource, absorbed: bool) -> Vec<Metered> {
    [
        (Unit::MtokInput, u.input_tokens),
        (Unit::MtokCachedInput, u.cache_read_input_tokens),
        (Unit::MtokCacheWrite, u.cache_creation_input_tokens),
        (Unit::MtokOutput, u.output_tokens),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(unit, n)| {
        let qty = i64::try_from(n).unwrap_or(i64::MAX);
        if absorbed {
            Metered::absorbed(token_line(model, unit), qty, ks)
        } else {
            Metered::new(token_line(model, unit), qty, ks)
        }
    })
    .collect()
}

/// Which attempts' observed usage a commit writes how.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observed {
    /// Charged to the person (a call they cancelled).
    pub charge: Option<i64>,
    /// Left out (the attempt the commit charges from its own message).
    pub skip: Option<i64>,
    /// Write nothing observed.
    pub none: bool,
}

/// Rows for the usage `step`'s attempts observed, by `how`; every attempt
/// neither charged nor skipped is absorbed.
fn observed_rows(model: &str, step: &Step, how: Observed) -> Vec<Metered> {
    if how.none {
        return Vec::new();
    }
    let mut out = Vec::new();
    for e in &step.observed {
        let attempt = e["attempt"].as_i64().unwrap_or(0);
        if Some(attempt) == how.skip {
            continue;
        }
        let ks =
            KeySource::parse(e["key_source"].as_str().unwrap_or("")).unwrap_or(KeySource::Instance);
        let m = e["model"].as_str().unwrap_or(model);
        let estimated = e["estimated"].as_bool().unwrap_or(false);
        for mut row in token_rows(m, usage_of(e), ks, Some(attempt) != how.charge) {
            let part = row.line.rsplit('.').next().unwrap_or("").to_string();
            // Priced as the call was, even if the line changed or went.
            row.rate = serde_json::from_value(e["rates"][&part].clone()).ok();
            row.estimated = estimated && part == "output";
            out.push(row);
        }
    }
    out
}

/// The output tokens a stream that streamed `bytes` of text, thinking and
/// tool input used at least, by a conservative estimate (3 bytes a token).
#[must_use]
pub fn estimated_output(bytes: usize) -> u64 {
    bytes.div_ceil(3) as u64
}

/// An observed-usage entry: the provider's figures `u`, the output at
/// least what `streamed` bytes are worth ([`estimated_output`]), and the
/// lines' `rates` snapshotted.
fn observed_entry(model: &str, ks: KeySource, u: Usage, streamed: usize, rates: &Value) -> Value {
    let estimate = estimated_output(streamed);
    json!({
        "model": model,
        "key_source": ks.as_str(),
        "input_tokens": u.input_tokens,
        "cache_read_input_tokens": u.cache_read_input_tokens,
        "cache_creation_input_tokens": u.cache_creation_input_tokens,
        "output_tokens": u.output_tokens.max(estimate),
        "estimated": estimate > u.output_tokens,
        "rates": rates,
    })
}

/// How many deltas a turn streams between two records of what it used.
const OBSERVE_EVERY: usize = 16;

/// Script compute in the line's base, MiB·ms: CPU time at the configured
/// memory limit, rounded up.
#[must_use]
pub fn compute_qty(cpu_us: u64, memory_mib: u32) -> i64 {
    let q = (u128::from(cpu_us) * u128::from(memory_mib)).div_ceil(1000);
    i64::try_from(q).unwrap_or(i64::MAX)
}

/// The per-step lines every step charges beside its own: the harness fee
/// on step 1, and (on the hosted Worker) its requests and storage.
pub(crate) fn step_usage(
    s: &RunSettings,
    n: i64,
    calls: usize,
    payload: &Value,
    ks: KeySource,
) -> Vec<Metered> {
    let mut out = Vec::new();
    if n == 1 {
        out.push(Metered::new(LINE_HARNESS_FEE, 1, ks));
    }
    if s.infra_lines {
        out.push(Metered::new(LINE_DO_REQUESTS, 1 + calls as i64, ks));
        let bytes = payload.to_string().len() as i64;
        out.push(Metered::new(LINE_TOOLPATH_STORAGE, bytes * 30 * 86_400, ks));
    }
    out
}

/// Waits for `work` while renewing the run's claim every
/// `settings.renew_every`; a cancel requested in the store, or a claim
/// lost, cancels `cancel` (and a lost claim sets `lost`).
pub(crate) async fn watched<F: std::future::Future>(
    store: &Store,
    run_id: &str,
    driver: &DriverId,
    every: Duration,
    cancel: &CancelToken,
    lost: &AtomicBool,
    work: F,
) -> F::Output {
    let watch = async {
        loop {
            clax_rt::time::sleep(every).await;
            match store.renew_lease(run_id, driver) {
                Ok(true) => {
                    if matches!(store.run(run_id), Ok(Some(r)) if r.status == RunStatus::Stopping) {
                        cancel.cancel();
                    }
                }
                Ok(false) => {
                    lost.store(true, Ordering::SeqCst);
                    cancel.cancel();
                }
                Err(e) => tracing::warn!("clax-bot: renewing run {run_id}: {e}"),
            }
        }
    };
    futures::pin_mut!(work);
    futures::pin_mut!(watch);
    match futures::future::select(work, watch).await {
        futures::future::Either::Left((v, _)) => v,
        futures::future::Either::Right(((), _)) => unreachable!("the watch never ends"),
    }
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// One tick of run `run_id` for `driver`: see the module documentation.
///
/// # Errors
/// `NotFound` for no such run; the store's and the settings'. A driver
/// that lost the run is not an error ([`Wait::NotOurs`]).
pub async fn tick(
    deps: &Deps,
    run_id: &str,
    driver: &DriverId,
    opts: TickOpts<'_>,
) -> Result<TickOutcome> {
    let started = clax_rt::time::Instant::now();
    let store = &*deps.store;
    let settings = deps.settings.settings()?;
    if store
        .claim_run(run_id, driver, settings.lease_secs)?
        .is_none()
    {
        let r = store.run(run_id)?.ok_or(CoreError::NotFound)?;
        return Ok(if r.status.is_terminal() {
            TickOutcome {
                next: Next::Done(r.status),
                step: None,
                kind: None,
                credits_milli: 0,
            }
        } else {
            TickOutcome::wait(Wait::NotOurs, None, None)
        });
    }
    let (run, steps) = store.run_with_steps(run_id)?.ok_or(CoreError::NotFound)?;
    let hist = History::of(&steps);
    let n = store.next_step_n(run_id)?;
    let open = steps.into_iter().find(|s| s.committed_at.is_none());
    let t = Tick {
        deps,
        store,
        settings,
        ctx: bot_ctx(&run.person),
        run,
        hist,
        n,
        open,
        driver,
        opts,
        started,
    };
    match t.step().await {
        Err(e) if fenced(&e) => Ok(TickOutcome::wait(Wait::NotOurs, Some(n), None)),
        other => other,
    }
}

pub(crate) struct Tick<'a> {
    pub deps: &'a Deps,
    pub store: &'a Store,
    pub settings: RunSettings,
    pub ctx: AuditCtx,
    pub run: Run,
    pub hist: History,
    pub n: i64,
    /// The step begun and not committed, as the tick found it.
    pub open: Option<Step>,
    pub driver: &'a DriverId,
    pub opts: TickOpts<'a>,
    pub started: clax_rt::time::Instant,
}

/// Why a run stops, and what it tells its threads.
pub(crate) struct Stop {
    pub reason: &'static str,
    pub notice: Option<String>,
}

impl Stop {
    pub(crate) fn cancelled() -> Stop {
        Stop {
            reason: "cancelled",
            notice: None,
        }
    }
}

/// What a stopped run's notice opens with, for `reason`.
fn stop_lead(reason: &str) -> &'static str {
    match reason {
        "no_flag" => "I stopped: ClaxBot is not turned on for you.",
        "disabled" => "I stopped: ClaxBot is turned off on this Clax.",
        "not_configured" => "I stopped: no model key is set up for ClaxBot.",
        "model_not_priced" => "I stopped: the model this run uses has no price set.",
        "key_rejected" => "I stopped: the model provider refused the key.",
        "model_error" => "I stopped: the model provider answered with an error.",
        "internal_error" => "I stopped: I could not save my work.",
        _ => "I stopped before finishing.",
    }
}

impl Tick<'_> {
    pub(crate) fn emit(&self, attempt: i64, e: Event) {
        self.opts.events.emit(&self.run.id, self.n, attempt, e);
    }

    pub(crate) fn begin(&self, kind: StepKind) -> Result<Step> {
        let s = self
            .store
            .begin_step(&self.run.id, self.driver, self.n, kind)?;
        self.emit(s.attempt, Event::StepStarted { kind });
        Ok(s)
    }

    fn elapsed(&self) -> Duration {
        chrono::DateTime::parse_from_rfc3339(&self.run.created_at)
            .map(|c| {
                (self.deps.clock.now() - c.to_utc())
                    .to_std()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// What is left of the run's time.
    fn remaining(&self) -> Duration {
        Duration::from_secs(u64::from(self.settings.max_run_minutes) * 60)
            .saturating_sub(self.elapsed())
    }

    /// What is left of the tick's budget and the run's time.
    fn available(&self) -> Duration {
        match self.opts.budget {
            Some(b) => b
                .saturating_sub(self.started.elapsed())
                .min(self.remaining()),
            None => self.remaining(),
        }
    }

    /// The journal entry of call `seq` (`op` on `args`, the JSON array the
    /// script passed) of `attempt`: made for this driver alone.
    pub(crate) fn entry(
        &self,
        attempt: i64,
        seq: i64,
        op: &str,
        args: &Value,
    ) -> std::result::Result<clax_store::CallEntry, clax_bot_sandbox::HostError> {
        self.store
            .call_entry(
                &self.run.id,
                self.driver,
                self.n,
                attempt,
                seq,
                op,
                &crate::host::args_hash(args),
            )
            .map_err(|e| crate::host::host_error(&e))
    }

    pub(crate) fn op_ctx(&self, attempt: i64, seq: i64) -> OpCtx<'_> {
        OpCtx::new(&self.run, &self.ctx, self.n, attempt, seq)
    }

    /// Commits the open step and tells the run's events; the step's
    /// observed model usage is added by `observed`. A commit that fails
    /// (not for a lost claim) is counted against the step.
    pub(crate) fn commit(
        &self,
        attempt: i64,
        kind: StepKind,
        mut commit: StepCommit,
        observed: Observed,
    ) -> Result<std::result::Result<Committed, Wait>> {
        if self
            .deps
            .crashes(CrashPoint::BeforeCommit { step: self.n, kind })
        {
            return Ok(Err(Wait::Crashed));
        }
        if let Some(open) = self
            .store
            .open_step(&self.run.id)?
            .filter(|s| s.n == self.n)
        {
            commit
                .usage
                .extend(observed_rows(&self.run.model, &open, observed));
        }
        let summary = step_summary(kind, &commit.payload);
        let c =
            match self
                .store
                .commit_bot_step(&self.ctx, &self.run.id, self.driver, self.n, commit)
            {
                Ok(c) => c,
                Err(e) => {
                    if !fenced(&e) {
                        tracing::warn!(
                            "clax-bot: committing step {} of run {}: {e}",
                            self.n,
                            self.run.id
                        );
                        let _ = self.store.note_commit_failure(&self.run.id, self.n);
                    }
                    return Err(e);
                }
            };
        self.emit(
            attempt,
            Event::StepDone {
                kind,
                credits_milli: Some(c.credits_milli),
                run_credits_milli: c.run.credits_milli,
                summary,
            },
        );
        if c.run.status.is_terminal() {
            self.emit(
                attempt,
                Event::RunEnd {
                    outcome: c.run.status,
                    reason: c.run.reason.clone(),
                    credits_milli: c.run.credits_milli,
                },
            );
        }
        if self
            .deps
            .crashes(CrashPoint::AfterCommit { step: self.n, kind })
        {
            return Ok(Err(Wait::Crashed));
        }
        Ok(Ok(c))
    }

    pub(crate) fn outcome(
        &self,
        kind: StepKind,
        c: std::result::Result<Committed, Wait>,
    ) -> TickOutcome {
        match c {
            Err(w) => TickOutcome::wait(w, Some(self.n), Some(kind)),
            Ok(c) => TickOutcome {
                next: if c.run.status.is_terminal() {
                    Next::Done(c.run.status)
                } else {
                    Next::Runnable
                },
                step: Some(self.n),
                kind: Some(kind),
                credits_milli: c.credits_milli,
            },
        }
    }

    /// The run's model's token lines' rates now, by part (`input`,
    /// `cached_input`, `cache_write`, `output`), to snapshot with a call's
    /// observed usage.
    fn rates(&self) -> Value {
        let mut out = serde_json::Map::new();
        for unit in TOKEN_UNITS {
            let line = token_line(&self.run.model, unit);
            if let Ok(Some(r)) = self.store.rate_of(&line) {
                let part = line.rsplit('.').next().unwrap_or("").to_string();
                out.insert(part, serde_json::to_value(r).expect("serialisable"));
            }
        }
        Value::Object(out)
    }

    pub(crate) fn key_source(&self) -> KeySource {
        self.store
            .key_source_of(&self.run.person)
            .unwrap_or(self.run.key_source)
    }

    fn reset_date(&self) -> String {
        week_end(week_start(self.deps.clock.now()))
            .format("on %A %-d %B %Y at %H:%M UTC")
            .to_string()
    }

    fn credits_notice(&self) -> String {
        format!(
            "I stopped before finishing: you have used this week's ClaxBot credits. {} \
             Your credits reset {}.",
            self.what_i_did(),
            self.reset_date()
        )
    }

    fn cap_notice(&self, limit: &str) -> String {
        format!(
            "I stopped before finishing: this run reached its limit of {limit}. {}",
            self.what_i_did()
        )
    }

    /// The stop for refusal `r`.
    fn refused(&self, r: Refusal) -> Stop {
        let notice = match r {
            Refusal::CreditsExhausted => self.credits_notice(),
            other => format!("{} {}", stop_lead(other.code()), self.what_i_did()),
        };
        Stop {
            reason: r.code(),
            notice: Some(notice),
        }
    }

    /// Why the run must stop before a turn or a script, if it must.
    fn must_stop(&self) -> Result<Option<Stop>> {
        if self.run.status == RunStatus::Stopping || self.opts.cancel.is_cancelled() {
            return Ok(Some(Stop::cancelled()));
        }
        if let Some(r) = refusal(
            self.store,
            &self.settings,
            &self.deps.keys,
            &*self.deps.clock,
            &self.run.person,
            &self.run.model,
        )? {
            return Ok(Some(self.refused(r)));
        }
        let cap = if self.hist.committed >= self.settings.max_steps as usize {
            Some(format!("{} steps", self.settings.max_steps))
        } else if self.run.credits_milli
            >= i64::try_from(self.settings.max_run_credits.saturating_mul(1000)).unwrap_or(i64::MAX)
        {
            Some(format!("{} credits", self.settings.max_run_credits))
        } else if self.remaining().is_zero() {
            Some(format!("{} minutes", self.settings.max_run_minutes))
        } else {
            None
        };
        Ok(cap.map(|limit| Stop {
            reason: "run_cap",
            notice: Some(self.cap_notice(&limit)),
        }))
    }

    /// What the run did, for a stopped run's notice.
    fn what_i_did(&self) -> String {
        let mut s = match self.hist.scripts {
            0 => "I had not run any scripts yet.".to_string(),
            1 => "I ran 1 script".to_string(),
            k => format!("I ran {k} scripts"),
        };
        if self.hist.scripts > 0 {
            if self.hist.changed.is_empty() {
                s.push('.');
            } else {
                let files: Vec<&str> = self.hist.changed.iter().map(String::as_str).collect();
                s.push_str(&format!(" and edited {}.", files.join(", ")));
            }
        }
        if let Some(v) = self.hist.published {
            s.push_str(&format!(" I published version {v}."));
        }
        if !self.hist.changed.is_empty() {
            s.push_str(" My draft is saved, unpublished: open it from the run panel.");
        }
        s
    }

    /// Posts `markdown` on each of the run's threads (a brief's run, on the
    /// brief's thread), journaled at the step's notice places; failures are
    /// logged, not fatal.
    async fn notify(&self, attempt: i64, markdown: &str) {
        for (i, t) in self.run.thread_ids.iter().enumerate() {
            let cx = self.op_ctx(attempt, NOTICE_SEQ + i as i64);
            let seq = NOTICE_SEQ + i as i64;
            let res = match (
                self.entry(attempt, seq, "reply", &json!([t, markdown])),
                self.deps.host.plan_reply(&cx, t, markdown).await,
            ) {
                (Ok(entry), Ok(plan)) => run_plan(plan, entry).await.map(drop),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
            if let Err(e) = res {
                tracing::warn!("clax-bot: run {} could not reply on {t}: {e}", self.run.id);
            }
        }
    }

    /// `bot.call` records for the journal's calls of this step that no
    /// event reported yet (calls of attempts that never committed).
    pub(crate) fn unreported_calls(&self, reported: &BTreeSet<i64>) -> Vec<AuditRecord> {
        let at = self.store.now();
        self.store
            .bot_calls(&self.run.id, self.n)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.seq() < NOTICE_SEQ && !reported.contains(&c.seq()))
            .map(|c| {
                call_record(at.clone(), c.op(), None, c.ok())
                    .with("attempt", c.attempt())
                    .with("seq", c.seq())
            })
            .collect()
    }

    /// Ends the run with a `stopped` step for `stop`, charging `usage`.
    pub(crate) async fn stop(
        &self,
        stop: Stop,
        usage: Vec<Metered>,
        extra: Value,
        events: Vec<AuditRecord>,
        reported: &BTreeSet<i64>,
        observed: Observed,
    ) -> Result<TickOutcome> {
        let s = self.begin(StepKind::Stopped)?;
        if let Some(notice) = &stop.notice {
            self.notify(s.attempt, notice).await;
        }
        let mut payload = json!({"reason": stop.reason, "notice": stop.notice});
        if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        let mut usage = usage;
        usage.extend(step_usage(
            &self.settings,
            self.n,
            0,
            &payload,
            self.key_source(),
        ));
        let mut events = events;
        events.extend(self.unreported_calls(reported));
        let c = self.commit(
            s.attempt,
            StepKind::Stopped,
            StepCommit {
                payload,
                usage,
                events,
                draft: Vec::new(),
                reason: Some(stop.reason.to_string()),
            },
            observed,
        )?;
        self.deps.host.working(&self.run, false).await;
        Ok(self.outcome(StepKind::Stopped, c))
    }

    /// A stop with nothing of its own to charge.
    pub(crate) async fn stop_plain(&self, stop: Stop) -> Result<TickOutcome> {
        self.stop(
            stop,
            Vec::new(),
            Value::Null,
            Vec::new(),
            &BTreeSet::new(),
            Observed::default(),
        )
        .await
    }

    /// Ends the run `failed`: its step could not be committed.
    async fn fail(&self) -> Result<TickOutcome> {
        let s = self.begin(StepKind::Failed)?;
        let notice = format!("{} {}", stop_lead("internal_error"), self.what_i_did());
        self.notify(s.attempt, &notice).await;
        let commit = || StepCommit {
            payload: json!({"reason": "internal_error", "notice": notice}),
            reason: Some("internal_error".into()),
            ..StepCommit::default()
        };
        // The model calls its attempts made are absorbed; when even that
        // cannot be written, the run ends without them.
        let c = match self.commit(s.attempt, StepKind::Failed, commit(), Observed::default()) {
            Err(e) if !fenced(&e) => self.commit(
                s.attempt,
                StepKind::Failed,
                commit(),
                Observed {
                    none: true,
                    ..Observed::default()
                },
            )?,
            other => other?,
        };
        self.deps.host.working(&self.run, false).await;
        Ok(self.outcome(StepKind::Failed, c))
    }

    async fn step(&self) -> Result<TickOutcome> {
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.failures >= self.settings.max_commit_failures)
        {
            return self.fail().await;
        }
        let next = self.hist.next.clone();
        let kind = next["kind"].as_str().unwrap_or("turn");
        // A terminal step the model asked for runs whatever the limits.
        match kind {
            "finish" => return self.finish(next["summary"].as_str().unwrap_or("")).await,
            "give_up" => {
                return self
                    .give_up(
                        next["code"].as_str().unwrap_or("model"),
                        next["reason"].as_str().unwrap_or(""),
                    )
                    .await;
            }
            _ => {}
        }
        if let Some(stop) = self.must_stop()? {
            return self.stop_plain(stop).await;
        }
        self.deps.host.working(&self.run, true).await;
        if kind == "script" {
            return self
                .script(
                    next["tool_use_id"].as_str().unwrap_or(""),
                    next["title"].as_str().unwrap_or(""),
                    next["code"].as_str().unwrap_or(""),
                )
                .await;
        }
        let Some(key) = choose_key(
            self.store,
            &self.deps.keys,
            &self.run.person,
            &self.settings.base_url,
        )?
        else {
            return self.stop_plain(self.refused(Refusal::NotConfigured)).await;
        };
        self.turn(key).await
    }

    async fn finish(&self, summary: &str) -> Result<TickOutcome> {
        let s = self.begin(StepKind::Finish)?;
        let post = self.hist.replied.is_empty() && !summary.is_empty();
        if post {
            self.notify(s.attempt, summary).await;
        }
        let payload = json!({"summary": summary, "posted": post});
        let usage = step_usage(&self.settings, self.n, 0, &payload, self.key_source());
        let c = self.commit(
            s.attempt,
            StepKind::Finish,
            StepCommit {
                payload,
                usage,
                ..StepCommit::default()
            },
            Observed::default(),
        )?;
        self.deps.host.working(&self.run, false).await;
        Ok(self.outcome(StepKind::Finish, c))
    }

    async fn give_up(&self, code: &str, reason: &str) -> Result<TickOutcome> {
        let s = self.begin(StepKind::GiveUp)?;
        let notice = format!("I gave up: {reason} {}", self.what_i_did());
        self.notify(s.attempt, &notice).await;
        let payload = json!({"code": code, "reason": reason, "notice": notice});
        let usage = step_usage(&self.settings, self.n, 0, &payload, self.key_source());
        let c = self.commit(
            s.attempt,
            StepKind::GiveUp,
            StepCommit {
                payload,
                usage,
                reason: Some(code.to_string()),
                ..StepCommit::default()
            },
            Observed::default(),
        )?;
        self.deps.host.working(&self.run, false).await;
        Ok(self.outcome(StepKind::GiveUp, c))
    }

    /// The `max_tokens` the person's credits pay for after the turn's
    /// expected input (the week's and the run's credits, whichever is
    /// less), at most the settings'; or the stop when that is under
    /// [`RunSettings::min_turn_tokens`]. Unlimited on an own key (tokens
    /// cost the person nothing here) and for an unenforced person.
    fn affordable(
        &self,
        ks: KeySource,
        request_bytes: usize,
    ) -> Result<std::result::Result<u32, Stop>> {
        let max = self.settings.max_tokens;
        if ks == KeySource::Own || self.store.flags_of(&self.run.person)?.claxbot_unenforced {
            return Ok(Ok(max));
        }
        let week_left = self
            .store
            .week_usage(&self.run.person, self.deps.clock.now())?
            .remaining_milli();
        let run_left = i64::try_from(self.settings.max_run_credits.saturating_mul(1000))
            .unwrap_or(i64::MAX)
            .saturating_sub(self.run.credits_milli)
            .max(0);
        let left = week_left.min(run_left);
        let price = self.store.credit_price_micro_usd()?;
        let credits = |unit: Unit, qty: u64| -> Result<i64> {
            let Some(line) = self.store.cost_line(&token_line(&self.run.model, unit))? else {
                return Ok(0);
            };
            row_credits_milli(
                i64::try_from(qty).unwrap_or(i64::MAX),
                line.cost_micro_usd,
                line.margin_bp,
                line.fixed_credits_milli,
                price,
                line.unit.base(),
            )
        };
        let tokens = (request_bytes / 4) as u64;
        let input = match self.hist.last_usage {
            Some(u) => {
                let prefix = u.cache_read_input_tokens + u.cache_creation_input_tokens;
                credits(Unit::MtokCachedInput, prefix.min(tokens))?
                    + credits(Unit::MtokInput, tokens.saturating_sub(prefix))?
            }
            None => credits(Unit::MtokInput, tokens)?,
        };
        let per_mtok = credits(Unit::MtokOutput, 1_000_000)?.max(1);
        let n = (i128::from(left - input).max(0) * 1_000_000 / i128::from(per_mtok))
            .min(i128::from(max));
        let n = u32::try_from(n).unwrap_or(max);
        if n >= self.settings.min_turn_tokens.min(max) {
            return Ok(Ok(n));
        }
        Ok(Err(if run_left < week_left {
            Stop {
                reason: "run_cap",
                notice: Some(
                    self.cap_notice(&format!("{} credits", self.settings.max_run_credits)),
                ),
            }
        } else {
            Stop {
                reason: Refusal::CreditsExhausted.code(),
                notice: Some(self.credits_notice()),
            }
        }))
    }

    async fn turn(&self, (key, ks, slot): ChosenKey) -> Result<TickOutcome> {
        if self.remaining() < self.settings.min_turn_budget {
            return self
                .stop_plain(Stop {
                    reason: "run_cap",
                    notice: Some(
                        self.cap_notice(&format!("{} minutes", self.settings.max_run_minutes)),
                    ),
                })
                .await;
        }
        if self.available() < self.settings.min_turn_budget {
            return Ok(TickOutcome::wait(
                Wait::Budget,
                Some(self.n),
                Some(StepKind::Turn),
            ));
        }
        let first = self.hist.committed == 0 || self.hist.system.is_none();
        let (system, user) = if first {
            let cx = match self.deps.host.context(&self.run).await {
                Ok(cx) => cx,
                Err(e) => {
                    tracing::warn!("clax-bot: run {} has no request context: {e}", self.run.id);
                    return self
                        .stop(
                            Stop {
                                reason: "request_gone",
                                notice: None,
                            },
                            Vec::new(),
                            json!({"error": e.to_string()}),
                            Vec::new(),
                            &BTreeSet::new(),
                            Observed::default(),
                        )
                        .await;
                }
            };
            (
                prompt::system_prompt(self.run.request),
                vec![Block::text(prompt::request_message(self.run.request, &cx))],
            )
        } else {
            (self.hist.system.clone().unwrap_or_default(), Vec::new())
        };
        let mut messages = self.hist.messages.clone();
        push(&mut messages, Role::User, user.clone());
        let mut req = MessagesRequest {
            model: self.run.model.clone(),
            max_tokens: self.settings.max_tokens,
            system: vec![SystemBlock {
                text: system.clone(),
                cache_control: Some(CacheControl::default()),
            }],
            messages: with_breakpoint(messages),
            tools: tools(),
            tool_choice: Some(ToolChoice::Auto {
                disable_parallel_tool_use: true,
            }),
            // Summarized: the run panel shows the reasoning as it streams;
            // the model's default, omitted, streams nothing while it thinks.
            thinking: Some(Thinking {
                display: Some("summarized".into()),
            }),
            effort: self.settings.effort.clone(),
            betas: Vec::new(),
            extra: serde_json::Map::new(),
        };
        req.max_tokens = match self.affordable(ks, req.body().to_string().len())? {
            Ok(n) => n,
            Err(stop) => return self.stop_plain(stop).await,
        };
        let mut s = self.begin(StepKind::Turn)?;
        let lost = AtomicBool::new(false);
        let mut failed = 0;
        let cancel = &self.opts.cancel;
        let msg = loop {
            let total = self.available();
            if total < self.settings.min_turn_budget {
                return Ok(TickOutcome::wait(
                    Wait::Retry(self.deps.retry_later),
                    Some(self.n),
                    Some(StepKind::Turn),
                ));
            }
            let client = self.deps.client.clone().with_timeouts(Timeouts {
                total: Some(total),
                ..self.deps.client.timeouts()
            });
            let attempt = s.attempt;
            let events = self.opts.events;
            let (store, run_id, n, driver) = (self.store, self.run.id.clone(), self.n, self.driver);
            let model = self.run.model.clone();
            let rates = self.rates();
            // What the call used so far: the provider's figures and the
            // bytes it streamed, also read once it ends without a message.
            let progress = Arc::new(std::sync::Mutex::new((Usage::default(), 0usize)));
            let observe = {
                let (run_id, model, rates) = (run_id.clone(), model.clone(), rates.clone());
                move |u: Usage, streamed: usize| {
                    let _ = store.observe_step(
                        &run_id,
                        driver,
                        n,
                        attempt,
                        observed_entry(&model, ks, u, streamed, &rates),
                    );
                }
            };
            let seen = progress.clone();
            let on_delta = observe.clone();
            let mut deltas = 0usize;
            let stream = client.stream(&self.settings.base_url, &key, &req, move |d| {
                let mut p = seen.lock().unwrap_or_else(|e| e.into_inner());
                let (v, bytes) = match d {
                    Delta::MessageStart { usage, .. } | Delta::MessageDelta { usage, .. } => {
                        p.0 = usage;
                        on_delta(p.0, p.1);
                        return;
                    }
                    Delta::Text { index, text } => {
                        let b = text.len();
                        (json!({"type": "text", "index": index, "text": text}), b)
                    }
                    Delta::Thinking { index, thinking } => {
                        let b = thinking.len();
                        (
                            json!({"type": "thinking", "index": index, "text": thinking}),
                            b,
                        )
                    }
                    Delta::ToolInput {
                        index,
                        partial_json,
                    } => {
                        let b = partial_json.len();
                        (
                            json!({"type": "tool_input", "index": index, "text": partial_json}),
                            b,
                        )
                    }
                    _ => return,
                };
                p.1 += bytes;
                deltas += 1;
                if deltas.is_multiple_of(OBSERVE_EVERY) {
                    on_delta(p.0, p.1);
                }
                drop(p);
                events.emit(&run_id, n, attempt, Event::TurnDelta(v));
            });
            let raced = async {
                futures::pin_mut!(stream);
                let stopped = cancel.cancelled();
                futures::pin_mut!(stopped);
                match futures::future::select(stream, stopped).await {
                    futures::future::Either::Left((r, _)) => Some(r),
                    futures::future::Either::Right(_) => None,
                }
            };
            let got = watched(
                self.store,
                &self.run.id,
                self.driver,
                self.settings.renew_every,
                cancel,
                &lost,
                raced,
            )
            .await;
            if lost.load(Ordering::SeqCst) {
                return Ok(TickOutcome::wait(
                    Wait::NotOurs,
                    Some(self.n),
                    Some(StepKind::Turn),
                ));
            }
            // A call that ended without its message used at least what it
            // streamed.
            if !matches!(got, Some(Ok(_))) {
                let p = *progress.lock().unwrap_or_else(|e| e.into_inner());
                if p != (Usage::default(), 0) {
                    observe(p.0, p.1);
                }
            }
            let Some(got) = got else {
                // Cancelled mid-turn: what the call used so far is the
                // person's to pay.
                return self
                    .stop(
                        Stop::cancelled(),
                        Vec::new(),
                        Value::Null,
                        Vec::new(),
                        &BTreeSet::new(),
                        Observed {
                            charge: Some(s.attempt),
                            ..Observed::default()
                        },
                    )
                    .await;
            };
            match got {
                Ok(m) => break m,
                Err(e) if e.retryable() => match self.deps.retry.delay(failed, &e) {
                    Some(d) if d + self.settings.min_turn_budget <= self.available() => {
                        failed += 1;
                        let waited = watched(
                            self.store,
                            &self.run.id,
                            self.driver,
                            self.settings.renew_every,
                            cancel,
                            &lost,
                            async {
                                let slept = clax_rt::time::sleep(d);
                                futures::pin_mut!(slept);
                                let stopped = cancel.cancelled();
                                futures::pin_mut!(stopped);
                                matches!(
                                    futures::future::select(slept, stopped).await,
                                    futures::future::Either::Left(_)
                                )
                            },
                        )
                        .await;
                        if lost.load(Ordering::SeqCst) {
                            return Ok(TickOutcome::wait(
                                Wait::NotOurs,
                                Some(self.n),
                                Some(StepKind::Turn),
                            ));
                        }
                        if !waited {
                            return self.stop_plain(Stop::cancelled()).await;
                        }
                        s = self.begin(StepKind::Turn)?;
                    }
                    _ => {
                        tracing::warn!("clax-bot: run {} waits for the model: {e}", self.run.id);
                        return Ok(TickOutcome::wait(
                            Wait::Retry(self.deps.retry_later),
                            Some(self.n),
                            Some(StepKind::Turn),
                        ));
                    }
                },
                Err(e) => {
                    let reason = match e {
                        MessagesError::Auth { .. } => "key_rejected",
                        MessagesError::BadBaseUrl | MessagesError::Redirected { .. } => {
                            Refusal::NotConfigured.code()
                        }
                        _ => "model_error",
                    };
                    let notice = format!("{} {}", stop_lead(reason), self.what_i_did());
                    return self
                        .stop(
                            Stop {
                                reason,
                                notice: Some(notice),
                            },
                            Vec::new(),
                            json!({"error": e.to_string()}),
                            Vec::new(),
                            &BTreeSet::new(),
                            Observed::default(),
                        )
                        .await;
                }
            }
        };
        if let Some(slot) = slot {
            let _ = self.store.mark_secret_ok(&slot);
        }
        let next = next_after(&msg, self.hist.nudged);
        let stop = stop_reason_str(msg.stop_reason.as_ref());
        let assistant: Vec<Value> = msg.content.iter().map(Block::to_json).collect();
        let mut payload = json!({
            "assistant": assistant,
            "stop_reason": stop,
            "model": msg.model,
            "key_source": ks.as_str(),
            "max_tokens": req.max_tokens,
            "usage": {
                "input_tokens": msg.usage.input_tokens,
                "cache_read_input_tokens": msg.usage.cache_read_input_tokens,
                "cache_creation_input_tokens": msg.usage.cache_creation_input_tokens,
                "output_tokens": msg.usage.output_tokens,
            },
            "next": next,
        });
        if first {
            payload["system"] = json!(system);
            payload["user"] = Value::Array(user.iter().map(Block::to_json).collect());
        }
        let u = msg.usage;
        let event = turn_record(
            self.store.now(),
            &self.run.model,
            ks,
            i64::try_from(u.input_tokens).unwrap_or(i64::MAX),
            i64::try_from(u.cache_read_input_tokens).unwrap_or(i64::MAX),
            i64::try_from(u.output_tokens).unwrap_or(i64::MAX),
            &stop,
        )
        .with(
            "cache_write_tokens",
            i64::try_from(u.cache_creation_input_tokens).unwrap_or(i64::MAX),
        );
        let mut usage = token_rows(&self.run.model, msg.usage, ks, false);
        usage.extend(step_usage(&self.settings, self.n, 0, &payload, ks));
        let c = self.commit(
            s.attempt,
            StepKind::Turn,
            StepCommit {
                payload,
                usage,
                events: vec![event],
                ..StepCommit::default()
            },
            Observed {
                skip: Some(s.attempt),
                ..Observed::default()
            },
        )?;
        Ok(self.outcome(StepKind::Turn, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_is_cpu_at_the_memory_limit_rounded_up() {
        assert_eq!(compute_qty(1_000, 64), 64);
        assert_eq!(compute_qty(1, 64), 1);
        assert_eq!(compute_qty(0, 64), 0);
        assert_eq!(compute_qty(2_000_000, 64), 128_000);
    }

    #[test]
    fn history_merges_consecutive_turns_of_one_role() {
        let mut m = Vec::new();
        push(&mut m, Role::User, vec![Block::text("a")]);
        push(&mut m, Role::User, vec![Block::text("b")]);
        push(&mut m, Role::Assistant, vec![]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].content.len(), 2);
    }

    #[test]
    fn earlier_attempts_are_absorbed_and_a_cancelled_one_charged() {
        let step = Step {
            run_id: "r".into(),
            n: 1,
            kind: StepKind::Turn,
            started_at: String::new(),
            attempt: 3,
            committed_at: None,
            payload: None,
            observed: vec![
                json!({"attempt": 1, "key_source": "instance", "input_tokens": 10, "output_tokens": 2}),
                json!({"attempt": 2, "key_source": "instance", "input_tokens": 20}),
                json!({"attempt": 3, "key_source": "instance", "input_tokens": 30}),
            ],
            failures: 0,
        };
        let rows = observed_rows(
            "m",
            &step,
            Observed {
                charge: Some(2),
                skip: Some(3),
                none: false,
            },
        );
        let got: Vec<(String, i64, bool)> = rows
            .iter()
            .map(|r| (r.line.clone(), r.qty, r.absorbed))
            .collect();
        assert_eq!(
            got,
            [
                ("m.input".to_string(), 10, true),
                ("m.output".to_string(), 2, true),
                ("m.input".to_string(), 20, false),
            ]
        );
    }
}
