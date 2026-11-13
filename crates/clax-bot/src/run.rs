//! The run state machine (spec §4.2–4.3, §7.4): [`tick`] loads a run,
//! enforces what must hold before any step ([`refusal`]: the flag,
//! credits, a key, the model's price; the run's caps; a cancel), executes
//! exactly one step and commits it with its usage, Toolpath events and
//! draft changes in one transaction.
//!
//! # Steps
//!
//! - `turn`: one model call over the run's history (the cached
//!   system prompt, the request, every turn and tool result so far), with
//!   the tools `run_script`, `finish` and `give_up`, one call a turn, its
//!   `max_tokens` no more than the person's credits can pay. Its payload
//!   keeps the assistant's blocks verbatim and decides the next step
//!   (`payload.next`).
//! - `script`: the turn's `run_script` code in the sandbox (see
//!   [`crate::script`]).
//! - `finish`, `give_up`, `stopped`, `failed`: terminal
//!   ([`crate::ending`]): what a run's end does beside ending it is each
//!   of its surfaces' own ([`SurfaceProvider::ending`] and `ended`).
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
//! `max_commit_failures` times ends the run `failed`; so does one whose
//! [`RunPolicy`] fails that often before it (see [`crate::policy`], "A
//! policy that fails").

use crate::ending::Ending;
use crate::events::{Event, EventSink, step_summary};
use crate::finish::{self, Finish};
use crate::history::{Control, committed_and_open, push, transcript, usage_of};
use crate::host::{BotHost, NOTICE_SEQ};
use crate::policy::{
    self, DefaultPolicy, HistoryAsk, HistoryFor, OverflowPath, Overflows, RunLimits, RunPolicy,
    Shown,
};
use crate::prompt;
use crate::registry::{Providers, RunSurfaces};
use crate::surface::{End, Progress, SurfaceProvider};
use crate::web::WebSource;
use clax_bot_sandbox::{CancelToken, Limits, MaybeSendSync, Sandbox};
use clax_core::audit::{Actor, AuditCtx, AuditRecord, Via};
use clax_core::gitctx::GitField;
use clax_core::working::Clock;
use clax_core::{CoreError, Result};
use clax_sample::messages::{
    ApiKey, AssistantMessage, Block, CacheControl, Delta, Message, MessagesClient, MessagesError,
    MessagesRequest, ModelApi, RetryPolicy, Role, StopReason, SystemBlock, Thinking, Timeouts,
    Tool, ToolCall, ToolChoice, Usage,
};
use clax_store::Store;
use clax_store::bot_pending::EndFor;
use clax_store::bot_runs::{
    Committed, DriverId, Run, RunStatus, Step, StepCommit, StepKind, call_record, turn_record,
};
use clax_store::bot_settings::Setting;
use clax_store::credits::{
    Applies, KeySource, LINE_DO_REQUESTS, LINE_HARNESS_FEE, LINE_TOOLPATH_STORAGE, Metered, Rate,
    RateSource, TOKEN_UNITS, Unit, parse_token_line, row_credits_milli, token_line, week_end,
    week_start,
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
    /// The API the provider at `base_url` speaks; the history stays the
    /// Messages API's whatever it is.
    pub api: ModelApi,
    pub base_url: String,
    /// The model takes images in a tool result: what
    /// `Block::tool_result_with_images` is told. Nothing in a model's
    /// metadata says so (`ModelInfo`, the price catalog), so it is a
    /// setting, off by default: a model is shown no image unless this says
    /// it takes them. The daemon's `[bot] model_takes_images` sets it for
    /// every model of the instance; nothing stored sets it.
    pub model_takes_images: bool,
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
            api: ModelApi::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model_takes_images: false,
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
            api: store
                .bot_setting(Setting::Api)?
                .as_str()
                .and_then(ModelApi::parse)
                .unwrap_or(d.api),
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
    pub settings: Arc<dyn SettingsSource>,
    /// What the environment decides about each run: its limits and what
    /// the model is sent of its history. [`Deps::new`] starts with the
    /// defaults, which are the run loop on `settings` alone.
    pub policy: Arc<dyn RunPolicy>,
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
    /// The provider of each surface a run may be bound to
    /// ([`Providers`]). [`Deps::new`] registers Clax's own over the
    /// host it is given, which the deps do not keep; a binding of a surface with none registered is served by
    /// [`crate::registry::Unavailable`].
    pub providers: Providers,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("keys", &self.keys)
            .field("retry", &self.retry)
            .field("providers", &self.providers)
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Deps with the default run policy ([`DefaultPolicy`]), the default
    /// retry policy, a 30 s wait after the model stays unreachable, no
    /// crash hook and no web source, and with Clax's own surface
    /// provided over `host` ([`Deps::providers`]): the provider holds
    /// the host, and the deps do not.
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
        let mut providers = Providers::new();
        providers.register_clax(store.clone(), host, None, clock.clone());
        Deps {
            store,
            client,
            sandbox,
            settings,
            policy: Arc::new(DefaultPolicy),
            keys,
            clock,
            retry: RetryPolicy::default(),
            retry_later: Duration::from_secs(30),
            crash: None,
            providers,
        }
    }

    /// These deps, asking `policy` about each run.
    #[must_use]
    pub fn with_policy(mut self, policy: Arc<dyn RunPolicy>) -> Deps {
        self.policy = policy;
        self
    }

    /// These deps, sending the web calls of Clax's own surface to `web`
    /// (the registry's `set_clax_web`).
    #[must_use]
    pub fn with_web(mut self, web: Arc<dyn WebSource>) -> Deps {
        self.providers.set_clax_web(Some(web));
        self
    }

    /// These deps, with no web source: Clax's own surface refuses every
    /// web call `web_disabled` (the registry's `set_clax_web`).
    #[must_use]
    pub fn without_web(mut self) -> Deps {
        self.providers.set_clax_web(None);
        self
    }

    /// These deps, with `provider` registered for the bindings of
    /// `surface` ([`Providers::register`]).
    #[must_use]
    pub fn with_provider(
        mut self,
        surface: impl Into<String>,
        provider: Arc<dyn SurfaceProvider>,
    ) -> Deps {
        self.providers.register(surface, provider);
        self
    }

    /// These deps, with `providers` and no other: Clax's own surface is
    /// provided only when `providers` holds it.
    #[must_use]
    pub fn with_providers(mut self, providers: Providers) -> Deps {
        self.providers = providers;
        self
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
    /// The model has no token rate ([`Store::model_rate`]) and the run
    /// would use the instance's key.
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
    if !store.base_url_policy().allows(&settings.base_url) {
        return Ok(Some(Refusal::NotConfigured));
    }
    let Some((_, ks, _)) = choose_key(store, keys, person, &settings.base_url)? else {
        return Ok(Some(Refusal::NotConfigured));
    };
    // Tokens on the person's own key cost them nothing here: a missing
    // price refuses only a run on the instance's.
    if ks == KeySource::Instance {
        for unit in TOKEN_UNITS {
            if store.model_rate(model, unit)?.is_none() {
                return Ok(Some(Refusal::ModelNotPriced));
            }
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
    /// The run is parked on a call that waits: no driver holds it. It
    /// is ticked again when the call's answer is journaled (whoever
    /// journals it wakes the driver, [`crate::Driver::wake`]), at the
    /// call's deadline, or on a cancel.
    Parked,
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
///
/// `max_result_bytes` is the most the JSON of a script's returned value
/// may take ([`RunLimits::script`]), which `run_script`'s description
/// tells the model ([`policy::bytes_text`]). `finish` takes the run's
/// kind's result ([`Finish::tool`]).
fn tools(max_result_bytes: u64, finish: &Finish<'_>) -> Vec<Tool> {
    let (finish_description, finish_schema) = finish.tool();
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
            &format!(
                "Runs JavaScript against the clax API. `code` is the body of an async function; \
                 what it returns (JSON, at most {}) comes back with its console.log lines, \
                 the calls it made and any error.",
                policy::bytes_text(max_result_bytes)
            ),
            json!({"type": "object", "properties": {
                "title": {"type": "string", "description": "What the script does, in a few words."},
                "code": {"type": "string", "description": "The body of an async function."}},
                "required": ["title", "code"], "additionalProperties": false}),
        ),
        tool("finish", &finish_description, finish_schema),
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

/// Why a run whose model's context window is full gives up.
const WINDOW_FULL: &str = "The conversation outgrew the model's context window.";

/// What a turn's reply asks for next; a call of `finish` is read as
/// `finish` says ([`Finish::read`]).
fn next_after(msg: &AssistantMessage, nudged: bool, finish: &Finish<'_>) -> Value {
    let calls = msg.tool_calls();
    let err = |id: &str, m: &str| Block::tool_result(id, m, true).to_json();
    match msg.stop_reason {
        Some(StopReason::Refusal) => {
            return json!({"kind": "give_up", "code": "refusal",
                          "reason": "The model declined the request."});
        }
        Some(StopReason::ModelContextWindowExceeded) => {
            return json!({"kind": "give_up", "code": "context_window", "reason": WINDOW_FULL});
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
                if name == "finish"
                    && let Some(next) = finish.read(id, input)
                {
                    return next;
                }
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

/// Resolves the rate of token line `line` on a `key` key now
/// ([`token_rate`]), for a row whose call snapshotted none.
type Resolve<'a> = &'a dyn Fn(&str, KeySource) -> Result<Option<Rate>>;

/// `rows` (one call's on a `ks` key, from [`token_rows`]) at the `rates`
/// snapshotted with the call (by part, `Tick::rates`), each row without a
/// rate there charged at its line's when committed; and, when the
/// provider billed the call `billed` micro-USD, at that cost ([`bill`]),
/// each row without a snapshotted rate first priced by `resolve`, so none
/// is charged on top of the billed cost.
///
/// # Errors
/// `resolve`'s.
fn priced(
    mut rows: Vec<Metered>,
    rates: &Value,
    billed: Option<u64>,
    ks: KeySource,
    resolve: Resolve<'_>,
) -> Result<Vec<Metered>> {
    for row in &mut rows {
        let part = row.line.rsplit('.').next().unwrap_or("");
        row.rate = serde_json::from_value(rates[part].clone()).ok();
    }
    if let Some(b) = billed {
        for row in rows.iter_mut().filter(|r| r.rate.is_none()) {
            row.rate = resolve(&row.line, ks)?;
        }
        bill(&mut rows, b);
    }
    Ok(rows)
}

/// Scales the cost rates of `rows` (one call's token rows) so their costs
/// sum to `billed` micro-USD, what the provider billed for the call. Each
/// row keeps its quantity, margin and fixed credits: a cost-based row
/// charges its share of the billed cost at its margin and is sourced
/// `provider`; a credit-based one charges its fixed credits still and
/// keeps its source, its rescaled cost informational.
///
/// The billed cost is shared in proportion to each row's cost at its
/// resolved rate; when those are all 0 (rows priced in credits alone, as
/// the default rates are), to its credits, which rank the units as the
/// rate card does; when those are 0 too, to its quantity. The rounding
/// remainder goes to the row with the largest share. A row's cost is
/// ⌈qty × rate ÷ base⌉ of an integer rate, so it is exact for a row of at
/// most a million tokens and short by under qty ÷ a million micro-USD for
/// a larger one. A row without a rate is left as it is.
fn bill(rows: &mut [Metered], billed: u64) {
    let weights = |f: &dyn Fn(i64, &Rate) -> i128| -> Vec<i128> {
        rows.iter()
            .map(|m| m.rate.map_or(0, |r| f(m.qty, &r)))
            .collect()
    };
    let cost = |q: i64, r: &Rate| {
        let base = i128::from(r.unit.base().max(1));
        (i128::from(q.max(0)) * i128::from(r.cost_micro_usd.max(0)) + base - 1) / base
    };
    let credits = |q: i64, r: &Rate| {
        i128::from(
            row_credits_milli(
                q,
                r.cost_micro_usd,
                r.margin_bp,
                r.fixed_credits_milli,
                r.credit_price_micro_usd,
                r.unit.base(),
            )
            .unwrap_or(0),
        )
    };
    let qty = |q: i64, _: &Rate| i128::from(q.max(0));
    let mut w = weights(&cost);
    if w.iter().all(|x| *x == 0) {
        w = weights(&credits);
    }
    if w.iter().all(|x| *x == 0) {
        w = weights(&qty);
    }
    let total: i128 = w.iter().sum();
    if total == 0 {
        return;
    }
    let billed = i128::from(billed);
    let mut shares: Vec<i128> = w.iter().map(|x| billed * x / total).collect();
    let largest = (0..w.len()).max_by_key(|&i| (w[i], std::cmp::Reverse(i)));
    if let Some(i) = largest {
        shares[i] += billed - shares.iter().sum::<i128>();
    }
    for (m, share) in rows.iter_mut().zip(shares) {
        let qty = i128::from(m.qty.max(0));
        if let Some(r) = &mut m.rate {
            let per_unit = if qty == 0 {
                0
            } else {
                share * i128::from(r.unit.base().max(1)) / qty
            };
            r.cost_micro_usd = i64::try_from(per_unit).unwrap_or(i64::MAX);
            if r.fixed_credits_milli.is_none() {
                r.source = RateSource::Provider;
            }
        }
    }
}

/// The rate a turn on `model` on a `ks` key charges token unit `unit` at:
/// the model's ([`Store::model_rate`]); on an own key, for a model without
/// one, nothing (cost 0 on the instance key, which an own key is waived,
/// sourced `default`: no price was set).
///
/// # Errors
/// The store's.
fn token_rate(store: &Store, model: &str, unit: Unit, ks: KeySource) -> Result<Option<Rate>> {
    if let Some(r) = store.model_rate(model, unit)? {
        return Ok(Some(r.rate));
    }
    if ks == KeySource::Own {
        return Ok(Some(Rate {
            unit,
            cost_micro_usd: 0,
            margin_bp: 0,
            fixed_credits_milli: None,
            applies: Applies::InstanceKey,
            credit_price_micro_usd: store.credit_price_micro_usd()?,
            source: RateSource::Default,
        }));
    }
    Ok(None)
}

/// [`token_rate`] of token line `line` (`model.unit`) on a `ks` key;
/// `None` for a line that names no token unit.
///
/// # Errors
/// The store's.
fn line_token_rate(store: &Store, line: &str, ks: KeySource) -> Result<Option<Rate>> {
    match parse_token_line(line) {
        Some((model, unit)) => token_rate(store, model, unit, ks),
        None => Ok(None),
    }
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
/// neither charged nor skipped is absorbed. A billed attempt's row
/// without a snapshotted rate is priced by `resolve` ([`priced`]).
///
/// # Errors
/// `resolve`'s.
fn observed_rows(
    model: &str,
    step: &Step,
    how: Observed,
    resolve: Resolve<'_>,
) -> Result<Vec<Metered>> {
    if how.none {
        return Ok(Vec::new());
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
        let u = usage_of(e);
        // Priced as the call was, even if the line changed or went.
        let mut rows = priced(
            token_rows(m, u, ks, Some(attempt) != how.charge),
            &e["rates"],
            u.billed_micro_usd,
            ks,
            resolve,
        )?;
        for row in &mut rows {
            row.estimated = estimated && row.line.ends_with(".output");
        }
        out.extend(rows);
    }
    Ok(out)
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
    let mut e = json!({
        "model": model,
        "key_source": ks.as_str(),
        "input_tokens": u.input_tokens,
        "cache_read_input_tokens": u.cache_read_input_tokens,
        "cache_creation_input_tokens": u.cache_creation_input_tokens,
        "output_tokens": u.output_tokens.max(estimate),
        "estimated": estimate > u.output_tokens,
        "rates": rates,
    });
    if let Some(b) = u.billed_micro_usd {
        e["billed_micro_usd"] = json!(b);
    }
    e
}

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
    let instance = deps.settings.settings()?;
    // A parked run is no driver's: before its call's deadline the tick
    // has nothing to do, and claims nothing. At the deadline the call's
    // failure is journaled, which makes the run due, and the tick goes
    // on to claim it and run its step again.
    if let Some(parked) = store.run(run_id)?.filter(|r| r.status == RunStatus::Parked) {
        let now = deps.clock.now();
        let late = parked
            .due_at
            .as_deref()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| at.to_utc() <= now);
        if !late {
            return Ok(TickOutcome::wait(Wait::Parked, None, None));
        }
        expire_waits(store, run_id, now)?;
    }
    if store
        .claim_run(run_id, driver, instance.lease_secs)?
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
    let (run, made, steps) = store
        .run_with_steps_as_made(run_id)?
        .ok_or(CoreError::NotFound)?;
    let n = store.next_step_n(run_id)?;
    let (steps, open) = committed_and_open(steps);
    // The run as the run loop knows it and the provider of each of its
    // bindings, from the row just read.
    let surfaces = deps.providers.bind_or_unreadable(&run, &made)?;
    // A policy that fails leaves the instance's settings in its place for
    // this tick, which takes no step under them but a stop or one the
    // model already asked for ([`Tick::policy_failed`]).
    let asked = deps
        .policy
        .run_settings(&surfaces.info, instance.clone())
        .and_then(|settings| Ok((deps.policy.limits(&surfaces.info, &settings)?, settings)));
    // Each binding's surface may lower the policy's caps, never raise
    // them (a host's `runs.max_steps` and `runs.max_run_minutes`): a run
    // that reaches one stops as at the policy's.
    let (settings, limits, policy_error) = match asked {
        Ok((limits, settings)) => (
            settings,
            surfaces
                .info
                .bindings
                .iter()
                .fold(limits, |limits, binding| limits.within(&binding.limits)),
            None,
        ),
        Err(e) => (
            instance.clone(),
            RunLimits::of(&instance),
            Some(e.to_string()),
        ),
    };
    let overflows = Overflows::of(&steps);
    // The time the run spent parked is no part of its minutes.
    let parking = store.run_parking(run_id)?;
    let parked = parking
        .as_ref()
        .map_or(0, |p| u64::try_from(p.parked_ms).unwrap_or(0));
    let t = Tick {
        parked: Duration::from_millis(parked),
        deps,
        store,
        settings,
        limits,
        policy_error,
        overflows,
        ctx: bot_ctx(&run.person),
        run,
        hist: Control::of(&steps),
        surfaces,
        steps,
        n,
        open,
        driver,
        opts,
        started,
    };
    // A script step that parked before is run again: it resumes.
    if t.open.as_ref().is_some_and(|s| s.kind == StepKind::Script)
        && parking.is_some_and(|p| p.parks > 0)
    {
        t.surfaces
            .kind_provider()
            .progress(&t.surfaces.info, &Progress::Resumed { step: n });
    }
    match t.step().await {
        Err(e) if fenced(&e) => Ok(TickOutcome::wait(Wait::NotOurs, Some(n), None)),
        other => other,
    }
}

/// Ends the wait of a parked run whose call's deadline has come: the
/// store journals, for each pending call of the run that is due (and of
/// no other run: each run's deadline is its own driver entry's), the
/// failure the
/// host protocol gives it by what is known of the call
/// ([`crate::bot_surfaces::unanswered`]: `unavailable`, `expired` or
/// `unknown_outcome`), which makes its run due. A run that is parked
/// still (its call has no pending row: its surface keeps its calls
/// elsewhere) is unparked, and its step run again asks the surface.
fn expire_waits(store: &Store, run_id: &str, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    store.expire_calls_of(run_id, now, &crate::broker::stored::deadline_failure)?;
    if store
        .run(run_id)?
        .is_some_and(|r| r.status == RunStatus::Parked)
    {
        store.unpark_run(run_id, now)?;
    }
    Ok(())
}

/// The reason of a run stopped because it was cancelled.
pub(crate) const CANCELLED: &str = "cancelled";

/// The stop of a run that reached a cap: `limit` is the cap with its
/// value, as the run's policy states it (`40 steps`, `200 credits`,
/// `30 minutes`).
fn capped(limit: String) -> End {
    End::Stopped {
        reason: "run_cap".into(),
        limit: Some(limit),
        credits_reset: None,
    }
}

pub(crate) struct Tick<'a> {
    /// How long the run has been parked in all, before this tick.
    pub parked: Duration,
    pub deps: &'a Deps,
    pub store: &'a Store,
    pub settings: RunSettings,
    /// What bounds the run, as its policy answered for this tick.
    pub limits: RunLimits,
    /// Why the policy could not say this tick's settings or limits, when
    /// it could not: `settings` and `limits` are then the instance's.
    pub policy_error: Option<String>,
    /// The overflows the committed steps record.
    pub overflows: Overflows,
    pub ctx: AuditCtx,
    /// The run's row. The run loop reads what every run has of it (its
    /// ID, person, model, status, credits and times); its kind, request
    /// and bindings it reads from `surfaces`, and nothing of a
    /// surface's own columns.
    pub run: Run,
    /// What the tick decides by, of the committed steps.
    pub hist: Control,
    /// The run as the run loop knows it, with its bindings' providers
    /// and what its audit records are filed on, made once from `run`
    /// ([`Providers::bind`]).
    pub surfaces: RunSurfaces,
    /// The run's committed steps as the tick read them, in order
    /// ([`committed_and_open`]): what [`transcript`] shows a model of.
    pub steps: Vec<Step>,
    pub n: i64,
    /// The step begun and not committed, as the tick found it.
    pub open: Option<Step>,
    pub driver: &'a DriverId,
    pub opts: TickOpts<'a>,
    pub started: clax_rt::time::Instant,
}

/// How a turn's model call ended, when the turn commits.
enum Answer {
    Message(AssistantMessage),
    /// The provider refused the request as too long and the policy shows
    /// less: the turn's next step.
    TooLong(Value),
}

impl Tick<'_> {
    pub(crate) fn emit(&self, attempt: i64, e: Event) {
        self.opts.events.emit(&self.run.id, self.n, attempt, e);
    }

    pub(crate) fn begin(&self, kind: StepKind) -> Result<Step> {
        self.begin_on(kind, KeySource::Instance)
    }

    /// [`Tick::begin`] for a step whose turn runs on a `turn_key` key: a
    /// turn on the person's own key is not refused for its model's price
    /// ([`Store::begin_step_on`]).
    pub(crate) fn begin_on(&self, kind: StepKind, turn_key: KeySource) -> Result<Step> {
        let s = self
            .store
            .begin_step_on(&self.run.id, self.driver, self.n, kind, turn_key)?;
        self.emit(s.attempt, Event::StepStarted { kind });
        Ok(s)
    }

    /// How long the run has been at work: since it was made, less the
    /// time it was parked.
    fn elapsed(&self) -> Duration {
        chrono::DateTime::parse_from_rfc3339(&self.run.created_at)
            .map(|c| {
                (self.deps.clock.now() - c.to_utc())
                    .to_std()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
            .saturating_sub(self.parked)
    }

    /// What is left of the run's time; `None` for a run with no cap on
    /// it.
    fn remaining(&self) -> Option<Duration> {
        self.limits
            .max_run_time()
            .map(|cap| cap.saturating_sub(self.elapsed()))
    }

    /// What is left of the tick's budget and the run's time; `None` when
    /// neither bounds it.
    fn available(&self) -> Option<Duration> {
        policy::least(
            self.opts
                .budget
                .map(|b| b.saturating_sub(self.started.elapsed())),
            self.remaining(),
        )
    }

    /// Whether less than a turn's least time is left of `bound`.
    fn too_short(&self, bound: Option<Duration>) -> bool {
        bound.is_some_and(|b| b < self.settings.min_turn_budget)
    }

    /// The stop of a run that reached its cap on minutes.
    fn minutes_stop(&self) -> End {
        let minutes = self.limits.max_run_minutes.unwrap_or_default();
        capped(format!("{minutes} minutes"))
    }

    /// Commits the open step and tells the run's events; the step's
    /// observed model usage is added by `observed`. A commit that fails
    /// (not for a lost claim) is counted against the step.
    pub(crate) fn commit(
        &self,
        attempt: i64,
        kind: StepKind,
        commit: StepCommit,
        observed: Observed,
    ) -> Result<std::result::Result<Committed, Wait>> {
        self.commit_ending(attempt, kind, commit, observed, &[])
    }

    /// [`Tick::commit`], for a terminal step: `ends` is how the run
    /// ends for each registration its bindings' providers named
    /// ([`SurfaceProvider::end_for`]), kept in the commit's
    /// transaction.
    pub(crate) fn commit_ending(
        &self,
        attempt: i64,
        kind: StepKind,
        mut commit: StepCommit,
        observed: Observed,
        ends: &[EndFor],
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
            commit.usage.extend(observed_rows(
                &self.run.model,
                &open,
                observed,
                &|line, ks| line_token_rate(self.store, line, ks),
            )?);
        }
        let summary = step_summary(kind, &commit.payload);
        let delivered = commit.delivered.clone();
        let c = match self.store.commit_bot_step_on(
            &self.ctx,
            &self.run.id,
            self.driver,
            self.n,
            commit,
            Some(&self.surfaces.subject),
            ends,
        ) {
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
        // The step that ends the run is told by the run's `ended`, which
        // its commit kept: an event of it could reach the host after
        // that.
        if !c.run.status.is_terminal() {
            self.surfaces.kind_provider().progress(
                &self.surfaces.info,
                &Progress::Step {
                    step: self.n,
                    kind,
                    delivered: &delivered,
                },
            );
        }
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

    /// The run's model's token rates now on a `ks` key ([`token_rate`]),
    /// by part (`input`, `cached_input`, `cache_write`, `output`), to
    /// snapshot with a call's usage; a unit without one is left out.
    ///
    /// # Errors
    /// The store's.
    fn rates(&self, ks: KeySource) -> Result<Value> {
        let mut out = serde_json::Map::new();
        for unit in TOKEN_UNITS {
            let line = token_line(&self.run.model, unit);
            if let Some(r) = token_rate(self.store, &self.run.model, unit, ks)? {
                let part = line.rsplit('.').next().unwrap_or("").to_string();
                out.insert(part, serde_json::to_value(r).expect("serialisable"));
            }
        }
        Ok(Value::Object(out))
    }

    pub(crate) fn key_source(&self) -> KeySource {
        self.store
            .key_source_of(&self.run.person)
            .unwrap_or(self.run.key_source)
    }

    /// The stop of a run whose person has spent the week's credits,
    /// with when they reset.
    fn credits_stop(&self) -> End {
        End::Stopped {
            reason: Refusal::CreditsExhausted.code().into(),
            limit: None,
            credits_reset: Some(week_end(week_start(self.deps.clock.now()))),
        }
    }

    /// The stop for refusal `r`.
    fn refused(&self, r: Refusal) -> End {
        match r {
            Refusal::CreditsExhausted => self.credits_stop(),
            other => End::stopped(other.code()),
        }
    }

    /// Why the run must stop before a turn or a script, if it must.
    fn must_stop(&self) -> Result<Option<End>> {
        if self.run.status == RunStatus::Stopping || self.opts.cancel.is_cancelled() {
            return Ok(Some(End::stopped(CANCELLED)));
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
        // The limits of a tick whose policy failed are not the policy's.
        if self.policy_error.is_some() {
            return Ok(None);
        }
        // Each cap the policy sets, in this order; one it does not set
        // stops nothing.
        let steps = self
            .limits
            .max_steps
            .filter(|max| self.hist.committed >= *max as usize)
            .map(|max| format!("{max} steps"));
        let credits = || {
            let max = self.limits.max_run_credits?;
            let milli = self.limits.max_run_credits_milli()?;
            (self.run.credits_milli >= milli).then(|| format!("{max} credits"))
        };
        let minutes = || {
            let max = self.limits.max_run_minutes?;
            self.remaining()?
                .is_zero()
                .then(|| format!("{max} minutes"))
        };
        Ok(steps.or_else(credits).or_else(minutes).map(capped))
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

    /// A stop with nothing of its own to charge: `end` is an
    /// [`End::Stopped`].
    pub(crate) async fn stop_plain(&self, end: End) -> Result<TickOutcome> {
        self.end(Ending::stopped(end)).await
    }

    /// Ends the run `failed` for `reason`: its step could not be
    /// committed (`internal_error`), or its policy failed or answered
    /// messages that cannot be sent, where `error` says how.
    async fn fail(&self, reason: &str, error: Option<&str>) -> Result<TickOutcome> {
        self.end(Ending::failed(reason, error)).await
    }

    async fn step(&self) -> Result<TickOutcome> {
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.failures >= self.settings.max_commit_failures)
        {
            // The failures of a step whose policy fails are the policy's.
            return match &self.policy_error {
                Some(e) => self.fail(policy::POLICY_ERROR, Some(e)).await,
                None => self.fail("internal_error", None).await,
            };
        }
        let next = self.hist.next.clone();
        let kind = next["kind"].as_str().unwrap_or("turn");
        // A cancel wins over everything, a step the model asked for
        // included: a stopping run takes no step but its `stopped` one
        // (`Store::begin_step` refuses any other).
        if self.run.status == RunStatus::Stopping {
            return self.stop_plain(End::stopped(CANCELLED)).await;
        }
        // A run this build cannot read as it was made takes no step
        // but its last: it ends `failed`, for what could not be read.
        if let Some((code, why)) = self.surfaces.unreadable() {
            return self.fail(code, Some(why)).await;
        }
        // A terminal step the model asked for runs whatever the limits.
        match kind {
            "finish" => return self.end(finish::ending(&next)).await,
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
        if let Some(e) = &self.policy_error {
            let kind = if kind == "script" {
                StepKind::Script
            } else {
                StepKind::Turn
            };
            return self.policy_failed(kind, e).await;
        }
        self.working().await;
        if kind == "script" {
            let (tool_use_id, title, code) = (
                next["tool_use_id"].as_str().unwrap_or(""),
                next["title"].as_str().unwrap_or(""),
                next["code"].as_str().unwrap_or(""),
            );
            return self.script_step(tool_use_id, title, code).await;
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

    /// Runs the script the last turn asked for, over the run's surfaces
    /// ([`Tick::surfaces`]).
    async fn script_step(&self, tool_use_id: &str, title: &str, code: &str) -> Result<TickOutcome> {
        let bound = &self.surfaces;
        self.script(&bound.info, &bound.surfaces(), tool_use_id, title, code)
            .await
    }

    /// Ends the run because it gave up, for `reason`.
    async fn give_up(&self, code: &str, reason: &str) -> Result<TickOutcome> {
        self.end(Ending::gave_up(code, reason)).await
    }

    /// What a tick does when the policy failed before a step of `kind`
    /// did anything: the failure is counted against the step as a failed
    /// commit is, and the tick waits to ask again, or, at
    /// [`RunSettings::max_commit_failures`], ends the run `failed`
    /// (`policy_error`).
    async fn policy_failed(&self, kind: StepKind, error: &str) -> Result<TickOutcome> {
        tracing::warn!(
            "clax-bot: the policy failed for run {}: {error}",
            self.run.id
        );
        self.begin_on(kind, self.key_source())?;
        let failures = self.store.note_commit_failure(&self.run.id, self.n)?;
        if failures >= self.settings.max_commit_failures {
            return self.fail(policy::POLICY_ERROR, Some(error)).await;
        }
        Ok(TickOutcome::wait(
            Wait::Retry(self.deps.retry_later),
            Some(self.n),
            Some(kind),
        ))
    }

    /// Stops a run whose model call the policy's bound on one call ended
    /// (`error` says how), the tick's attempts spent.
    async fn timed_out(&self, error: &str) -> Result<TickOutcome> {
        self.end(Ending::stopped(End::stopped(policy::MODEL_TIMEOUT)).with_error(json!(error)))
            .await
    }

    /// Ends a run whose history after an overflow is not the shorter one
    /// the overflow's ask answered (`overflow` is the last turn's
    /// `next.overflow`): as the overflow would have ended it had the
    /// policy shown no less.
    async fn overflow_ending(&self, overflow: &Value) -> Result<TickOutcome> {
        if overflow["path"] != OverflowPath::Refused.as_str() {
            return self.give_up("context_window", WINDOW_FULL).await;
        }
        self.end(Ending::stopped(End::stopped("model_error")).with_error(overflow["error"].clone()))
            .await
    }

    /// The `max_tokens` the person's credits pay for after the turn's
    /// expected input (the week's and the run's credits, whichever is
    /// less), at most the settings'; or the stop when that is under
    /// [`RunSettings::min_turn_tokens`]. The input is priced as the
    /// provider bills it: the previous turn's cached prefix at the
    /// cached-input rate, and the rest at the cache-write rate on the
    /// Messages API (whose request carries the cache breakpoint,
    /// [`with_breakpoint`]) or at the input rate on the others. Each token
    /// unit is priced at the model's rate ([`Store::model_rate`]); a unit
    /// an own key is waived (one that applies on the instance key) costs
    /// nothing. Unlimited for an unenforced person and when no unit costs
    /// anything (an own key on instance-key rates).
    ///
    /// `cached_prefix` says the request repeats the last turn's: a
    /// request a policy put together shares no prefix with the one
    /// before it, and all of its input is priced as not read from a
    /// cache.
    fn affordable(
        &self,
        ks: KeySource,
        request_bytes: usize,
        cached_prefix: bool,
    ) -> Result<std::result::Result<u32, End>> {
        let max = self.settings.max_tokens;
        let rate = |unit: Unit| -> Result<Option<Rate>> {
            Ok(self
                .store
                .model_rate(&self.run.model, unit)?
                .map(|r| r.rate)
                .filter(|r| !(ks == KeySource::Own && r.applies == Applies::InstanceKey)))
        };
        // Only the Messages API request carries the cache breakpoint.
        let uncached = if self.settings.api == ModelApi::Anthropic {
            Unit::MtokCacheWrite
        } else {
            Unit::MtokInput
        };
        let rates = [
            rate(uncached)?,
            rate(Unit::MtokCachedInput)?,
            rate(Unit::MtokOutput)?,
        ];
        if (ks == KeySource::Own && rates.iter().all(Option::is_none))
            || self.store.flags_of(&self.run.person)?.claxbot_unenforced
        {
            return Ok(Ok(max));
        }
        let week_left = self
            .store
            .week_usage(&self.run.person, self.deps.clock.now())?
            .remaining_milli();
        // A run with no cap on its credits has the week's.
        let run_left = self.limits.max_run_credits_milli().map_or(i64::MAX, |cap| {
            cap.saturating_sub(self.run.credits_milli).max(0)
        });
        let left = week_left.min(run_left);
        let [uncached_rate, cached_rate, output_rate] = rates;
        let credits = |rate: Option<Rate>, qty: u64| -> Result<i64> {
            let Some(r) = rate else {
                return Ok(0);
            };
            row_credits_milli(
                i64::try_from(qty).unwrap_or(i64::MAX),
                r.cost_micro_usd,
                r.margin_bp,
                r.fixed_credits_milli,
                r.credit_price_micro_usd,
                r.unit.base(),
            )
        };
        let tokens = (request_bytes / 4) as u64;
        let input = match self.hist.last_usage.filter(|_| cached_prefix) {
            Some(u) => {
                let prefix = u.cache_read_input_tokens + u.cache_creation_input_tokens;
                credits(cached_rate, prefix.min(tokens))?
                    + credits(uncached_rate, tokens.saturating_sub(prefix))?
            }
            None => credits(uncached_rate, tokens)?,
        };
        let per_mtok = credits(output_rate, 1_000_000)?.max(1);
        let n = (i128::from(left - input).max(0) * 1_000_000 / i128::from(per_mtok))
            .min(i128::from(max));
        let n = u32::try_from(n).unwrap_or(max);
        if n >= self.settings.min_turn_tokens.min(max) {
            return Ok(Ok(n));
        }
        Ok(Err(if run_left < week_left {
            capped(format!(
                "{} credits",
                self.limits.max_run_credits.unwrap_or_default()
            ))
        } else {
            self.credits_stop()
        }))
    }

    /// The next step of a turn whose model reported its context window
    /// full (see [`crate::policy`], "A full context window"): a turn on
    /// the shorter history the policy answers, or `None` when the run
    /// ends as it does without one. `sent_bytes` is the size of the
    /// messages a policy answered for this turn (`None` when it was sent
    /// the core's projection), `user` the request the turn added and
    /// `assistant` the overflowing answer's blocks. The answer is held to
    /// what a turn relies on by the turn that sends it.
    fn overflowed(
        &self,
        path: OverflowPath,
        sent_bytes: Option<u64>,
        user: &[Block],
        assistant: &[Block],
    ) -> Option<Value> {
        let running = self.overflows.running + 1;
        if self
            .limits
            .max_overflow_retries
            .is_some_and(|max| running > max)
        {
            return None;
        }
        let mut core = transcript(&self.steps).messages;
        push(&mut core, Role::User, user.to_vec());
        let sent = match sent_bytes {
            Some(b) => usize::try_from(b).unwrap_or(usize::MAX),
            None => policy::messages_bytes(&core),
        };
        push(&mut core, Role::Assistant, assistant.to_vec());
        let ask = HistoryAsk::new(HistoryFor::Overflow(path), self.overflows.level + 1);
        // A policy that fails here shows no less: the turn is committed
        // with what it used, and the run ends.
        let shorter = match self.deps.policy.history(&self.surfaces.info, &core, &ask) {
            Ok(Shown::Messages(shorter)) => shorter,
            Ok(_) => return None,
            Err(e) => {
                tracing::warn!("clax-bot: the policy failed for run {}: {e}", self.run.id);
                return None;
            }
        };
        let shown = policy::messages_bytes(&shorter);
        if shown >= sent {
            return None;
        }
        Some(json!({"kind": "turn", "user": [], "overflow": {
            "level": ask.level,
            "path": path.as_str(),
            "sent_bytes": sent,
            "shown_bytes": shown,
            "shown_sha256": policy::messages_sha256(&shorter),
        }}))
    }

    async fn turn(&self, (key, ks, slot): ChosenKey) -> Result<TickOutcome> {
        if self.too_short(self.remaining()) {
            return self.stop_plain(self.minutes_stop()).await;
        }
        if self.too_short(self.available()) {
            return Ok(TickOutcome::wait(
                Wait::Budget,
                Some(self.n),
                Some(StepKind::Turn),
            ));
        }
        // The system text and whether this is the first request are the
        // steps', whatever a policy shows of them.
        let core = transcript(&self.steps);
        let stored_system = core.system.clone();
        let first = self.hist.committed == 0 || core.system.is_none();
        let (system, user) = if first {
            // The request is the kind's surface's to resolve, and the
            // run loop's to render.
            let info = &self.surfaces.info;
            let kind = self.surfaces.kind_provider();
            // The prompt file the run was started with, when the kind's
            // surface has one of its own. One that cannot be given is
            // never replaced by another.
            let own = match kind.prompt() {
                Ok(own) => own,
                Err(e) if e.retryable => {
                    return Err(CoreError::invalid(
                        "prompt_unread",
                        format!("the prompt file of run {}: {}", self.run.id, e.message),
                    ));
                }
                Err(e) => {
                    tracing::warn!("clax-bot: run {} has no prompt file: {e}", self.run.id);
                    return self.fail(&e.code, Some(&e.message)).await;
                }
            };
            let request = kind.request(info).await.map_err(|e| e.to_string());
            let request = request.and_then(|resolved| {
                let bound = self.surfaces.definitions();
                let system = prompt::system_of(info, kind.definition(), &bound, own)?;
                let message = prompt::request_of(info, kind.definition(), &resolved, own)?;
                Ok((system, message))
            });
            match request {
                Ok((system, request)) => (system, vec![Block::text(request)]),
                Err(e) => {
                    tracing::warn!("clax-bot: run {} has no request context: {e}", self.run.id);
                    return self
                        .end(Ending::stopped(End::stopped("request_gone")).with_error(json!(e)))
                        .await;
                }
            }
        } else {
            (core.system.unwrap_or_default(), Vec::new())
        };
        let mut messages = core.messages;
        // The messages sent to the run that it was not given yet end
        // the turn's user message, after the request on a first turn;
        // a turn that continues the model's own answer takes none.
        let continues =
            user.is_empty() && messages.last().is_some_and(|m| m.role == Role::Assistant);
        let (inbox, delivered) = self.inbox(continues)?;
        let sent: Vec<Block> = user.iter().cloned().chain(inbox).collect();
        push(&mut messages, Role::User, sent.clone());
        // What the policy shows of them, and its record when that is not
        // the core's projection.
        let ask = HistoryAsk::new(HistoryFor::NextTurn, self.overflows.level);
        let answered = match self
            .deps
            .policy
            .history(&self.surfaces.info, &messages, &ask)
        {
            Ok(answered) => answered,
            Err(e) => return self.policy_failed(StepKind::Turn, &e.to_string()).await,
        };
        // After an overflow the turn is sent the shorter history the
        // overflow's ask answered, or the run ends.
        let after_overflow = self.hist.next.get("overflow").filter(|o| o.is_object());
        let shown = match answered {
            Shown::Messages(m) => {
                let m = match policy::check_shown(&self.steps, stored_system.as_deref(), &user, m) {
                    Ok(m) => m,
                    Err(broken) => {
                        return self
                            .fail(&policy::history_reason(&broken), Some(&broken))
                            .await;
                    }
                };
                let record = policy::shown_record(ask.level, &m);
                if let Some(overflow) = after_overflow
                    && overflow["shown_sha256"] != record["sha256"]
                {
                    return self.overflow_ending(overflow).await;
                }
                messages = m;
                Some(record)
            }
            _ => {
                if let Some(overflow) = after_overflow {
                    return self.overflow_ending(overflow).await;
                }
                None
            }
        };
        let sent_bytes = shown.as_ref().and_then(|r| r["bytes"].as_u64());
        // `finish` takes the kind's result.
        let finish = Finish::of(
            &self.surfaces.info,
            self.surfaces.kind_provider().definition(),
            self.limits.max_invalid_results,
            &self.hist.next,
        );
        // The images of a script step no answered turn came after, read
        // from the store now ([`crate::images::show`]), and the items they
        // are: marked shown once this turn commits the model's answer.
        let (messages, showing) = crate::images::show(
            self.store,
            &self.run.id,
            &self.steps,
            self.settings.model_takes_images,
            messages,
        )?;
        let mut req = MessagesRequest {
            model: self.run.model.clone(),
            max_tokens: self.settings.max_tokens,
            system: vec![SystemBlock {
                text: system.clone(),
                cache_control: Some(CacheControl::default()),
            }],
            messages: with_breakpoint(messages),
            tools: tools(self.limits.script.max_result_bytes, &finish),
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
        let request_bytes = req.body().to_string().len();
        req.max_tokens = match self.affordable(ks, request_bytes, shown.is_none())? {
            Ok(n) => n,
            Err(stop) => return self.stop_plain(stop).await,
        };
        if self.limits.model_call_timeout.is_some_and(|t| t.is_zero()) {
            return self.timed_out("no time is allowed for a model call").await;
        }
        let mut s = self.begin_on(StepKind::Turn, ks)?;
        // The rates the call that answers snapshotted, to charge it at.
        let mut call_rates: Value;
        let lost = AtomicBool::new(false);
        let mut failed = 0;
        let cancel = &self.opts.cancel;
        let observe_every = self.limits.observe_every.get();
        let answer = loop {
            let total = self.available();
            if self.too_short(total) {
                return Ok(TickOutcome::wait(
                    Wait::Retry(self.deps.retry_later),
                    Some(self.n),
                    Some(StepKind::Turn),
                ));
            }
            // The call ends with the tick's budget, the run's time and
            // the policy's bound on one call, whichever is first.
            let client = self.deps.client.clone().with_timeouts(Timeouts {
                total: policy::least(total, self.limits.model_call_timeout),
                ..self.deps.client.timeouts()
            });
            // The policy's bound, when it is the first of them.
            let timeout = self
                .limits
                .model_call_timeout
                .filter(|t| total.is_none_or(|a| *t <= a));
            let called = clax_rt::time::Instant::now();
            let attempt = s.attempt;
            let events = self.opts.events;
            let (store, run_id, n, driver) = (self.store, self.run.id.clone(), self.n, self.driver);
            let model = self.run.model.clone();
            let rates = self.rates(ks)?;
            call_rates = rates.clone();
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
            let stream = client.stream(
                self.settings.api,
                &self.settings.base_url,
                &key,
                &req,
                move |d| {
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
                    if deltas.is_multiple_of(observe_every) {
                        on_delta(p.0, p.1);
                    }
                    drop(p);
                    events.emit(&run_id, n, attempt, Event::TurnDelta(v));
                },
            );
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
                    .end(Ending {
                        observed: Observed {
                            charge: Some(s.attempt),
                            ..Observed::default()
                        },
                        ..Ending::stopped(End::stopped(CANCELLED))
                    })
                    .await;
            };
            match got {
                Ok(m) => break Answer::Message(m),
                Err(e) if e.retryable() => match self.deps.retry.delay(failed, &e) {
                    Some(d)
                        if self
                            .available()
                            .is_none_or(|a| d + self.settings.min_turn_budget <= a) =>
                    {
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
                            return self.stop_plain(End::stopped(CANCELLED)).await;
                        }
                        s = self.begin_on(StepKind::Turn, ks)?;
                    }
                    // The policy's bound on a call ended the last of the
                    // tick's attempts: the run stops, where a provider
                    // that cannot be reached is waited for.
                    _ if matches!(e, MessagesError::StreamCut { .. })
                        && timeout.is_some_and(|t| called.elapsed() >= t) =>
                    {
                        return self.timed_out(&e.to_string()).await;
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
                    // A request refused as too long goes on when the
                    // policy shows less; else it stops as any refusal.
                    if self.deps.policy.is_overflow(&e)
                        && let Some(mut next) =
                            self.overflowed(OverflowPath::Refused, sent_bytes, &sent, &[])
                    {
                        next["overflow"]["error"] = json!(e.to_string());
                        break Answer::TooLong(next);
                    }
                    let reason = match e {
                        MessagesError::Auth { .. } => "key_rejected",
                        MessagesError::BadBaseUrl | MessagesError::Redirected { .. } => {
                            Refusal::NotConfigured.code()
                        }
                        _ => "model_error",
                    };
                    return self
                        .end(Ending::stopped(End::stopped(reason)).with_error(json!(e.to_string())))
                        .await;
                }
            }
        };
        // What the step keeps of the answer: a refused request has no
        // blocks and used nothing.
        let answered = matches!(answer, Answer::Message(_));
        let (assistant, stop, model, u, next) = match answer {
            Answer::Message(msg) => {
                if let Some(slot) = slot {
                    let _ = self.store.mark_secret_ok(&slot);
                }
                let mut next = next_after(&msg, self.hist.nudged, &finish);
                if msg.stop_reason == Some(StopReason::ModelContextWindowExceeded)
                    && let Some(shorter) =
                        self.overflowed(OverflowPath::StopReason, sent_bytes, &sent, &msg.content)
                {
                    next = shorter;
                }
                let assistant: Vec<Value> = msg.content.iter().map(Block::to_json).collect();
                (
                    assistant,
                    stop_reason_str(msg.stop_reason.as_ref()),
                    msg.model,
                    msg.usage,
                    next,
                )
            }
            Answer::TooLong(next) => (
                Vec::new(),
                "prompt_too_long".to_string(),
                self.run.model.clone(),
                Usage::default(),
                next,
            ),
        };
        let mut payload = json!({
            "assistant": assistant,
            "stop_reason": stop,
            "model": model,
            "key_source": ks.as_str(),
            "max_tokens": req.max_tokens,
            "usage": {
                "input_tokens": u.input_tokens,
                "cache_read_input_tokens": u.cache_read_input_tokens,
                "cache_creation_input_tokens": u.cache_creation_input_tokens,
                "output_tokens": u.output_tokens,
            },
            "next": next,
        });
        if let Some(b) = u.billed_micro_usd {
            payload["usage"]["billed_micro_usd"] = json!(b);
        }
        if first {
            payload["system"] = json!(system);
        }
        if first || !delivered.is_empty() {
            payload["user"] = Value::Array(sent.iter().map(Block::to_json).collect());
        }
        if let Some(shown) = shown {
            payload["shown"] = shown;
        }
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
        let mut usage = priced(
            token_rows(&self.run.model, u, ks, false),
            &call_rates,
            u.billed_micro_usd,
            ks,
            &|line, ks| line_token_rate(self.store, line, ks),
        )?;
        usage.extend(step_usage(&self.settings, self.n, 0, &payload, ks));
        let c = self.commit(
            s.attempt,
            StepKind::Turn,
            StepCommit {
                payload,
                usage,
                events: vec![event],
                delivered,
                ..StepCommit::default()
            },
            Observed {
                skip: Some(s.attempt),
                ..Observed::default()
            },
        )?;
        if answered
            && c.is_ok()
            && let Err(e) = self.store.mark_bot_content_shown(&self.run.id, &showing)
        {
            tracing::warn!(
                "clax-bot: run {}'s images were not marked shown: {e}",
                self.run.id
            );
        }
        Ok(self.outcome(StepKind::Turn, c))
    }

    /// The messages of the run's inbox it was not given yet
    /// (`Store::undelivered_messages`), in the order they arrived, each
    /// as a text block ([`prompt::message_of`]), and their `seq`s, for
    /// the turn's commit to mark delivered: none when the turn
    /// `continues` the model's own answer, and none read for a run
    /// whose kind takes no messages (every `clax` run's).
    fn inbox(&self, continues: bool) -> Result<(Vec<Block>, Vec<i64>)> {
        let info = &self.surfaces.info;
        let surface = self.surfaces.kind_provider().definition();
        let takes = info
            .kind
            .split_once('.')
            .and_then(|(_, kind)| surface.kind(kind))
            .is_some_and(|kind| kind.message.is_some());
        if continues || !takes {
            return Ok((Vec::new(), Vec::new()));
        }
        Ok(self
            .store
            .undelivered_messages(&self.run.id)?
            .into_iter()
            .map(|m| {
                let text = prompt::message_of(info, surface, m.seq, &m.body);
                (Block::text(text), m.seq)
            })
            .unzip())
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
            &|_, _| Ok(None),
        )
        .unwrap();
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

    fn rate(unit: Unit, cost: i64, fixed: Option<i64>) -> Rate {
        Rate {
            unit,
            cost_micro_usd: cost,
            margin_bp: 4_000,
            fixed_credits_milli: fixed,
            applies: Applies::InstanceKey,
            credit_price_micro_usd: 10_000,
            source: RateSource::Catalog,
        }
    }

    fn row(part: &str, qty: i64, r: Rate) -> Metered {
        Metered {
            rate: Some(r),
            ..Metered::new(format!("m.{part}"), qty, KeySource::Instance)
        }
    }

    /// The costs `rows` charge (⌈qty × rate ÷ base⌉ each).
    fn costs(rows: &[Metered]) -> Vec<i64> {
        rows.iter()
            .map(|m| {
                let r = m.rate.unwrap();
                (m.qty * r.cost_micro_usd).div_euclid(r.unit.base())
                    + i64::from((m.qty * r.cost_micro_usd) % r.unit.base() != 0)
            })
            .collect()
    }

    #[test]
    fn a_billed_cost_is_shared_by_resolved_cost_and_sourced_provider() {
        let mut rows = vec![
            row("input", 1_000, rate(Unit::MtokInput, 3_000_000, None)),
            row("output", 300, rate(Unit::MtokOutput, 15_000_000, None)),
        ];
        // Resolved costs 3000 and 4500: 10 000 billed is shared 4000:6000.
        bill(&mut rows, 10_000);
        assert_eq!(costs(&rows), [4_000, 6_000]);
        assert!(rows.iter().all(|m| {
            let r = m.rate.unwrap();
            r.source == RateSource::Provider && r.margin_bp == 4_000
        }));
    }

    #[test]
    fn credit_rows_share_by_credits_and_keep_them() {
        // Default rates: cost 0, fixed credits 560 000 and 2 800 000 a
        // million tokens: 560 and 840 milli-credits here.
        let mut rows = vec![
            row("input", 1_000, rate(Unit::MtokInput, 0, Some(560_000))),
            row("output", 300, rate(Unit::MtokOutput, 0, Some(2_800_000))),
        ];
        bill(&mut rows, 1_400);
        assert_eq!(costs(&rows), [560, 840]);
        assert!(
            rows.iter()
                .all(|m| m.rate.unwrap().source == RateSource::Catalog),
            "credit-based rows keep their source"
        );
        assert_eq!(rows[0].rate.unwrap().fixed_credits_milli, Some(560_000));
        assert_eq!(rows[1].rate.unwrap().fixed_credits_milli, Some(2_800_000));
    }

    #[test]
    fn the_rounding_remainder_goes_to_the_largest_share() {
        let r = rate(Unit::MtokInput, 0, None);
        let mut rows = vec![row("input", 10, r), row("cached_input", 30, r)];
        // By quantity, 1:3 of 7: 1 and 5, and the remainder 1 to the larger.
        bill(&mut rows, 7);
        assert_eq!(costs(&rows), [1, 6]);
        let mut even = vec![row("input", 10, r), row("output", 10, r)];
        bill(&mut even, 7);
        assert_eq!(costs(&even), [4, 3], "a tie goes to the first");
    }

    #[test]
    fn a_billed_cost_survives_the_observed_entry() {
        let u = Usage {
            input_tokens: 1_000,
            output_tokens: 300,
            billed_micro_usd: Some(10_000),
            ..Usage::default()
        };
        let rates = json!({
            "input": rate(Unit::MtokInput, 3_000_000, None),
            "output": rate(Unit::MtokOutput, 15_000_000, None),
        });
        let mut e = observed_entry("m", KeySource::Instance, u, 0, &rates);
        assert_eq!(usage_of(&e), u);
        e["attempt"] = json!(1);
        let step = Step {
            run_id: "r".into(),
            n: 1,
            kind: StepKind::Turn,
            started_at: String::new(),
            attempt: 2,
            committed_at: None,
            payload: None,
            observed: vec![e],
            failures: 0,
        };
        let rows = observed_rows("m", &step, Observed::default(), &|_, _| Ok(None)).unwrap();
        assert_eq!(costs(&rows), [4_000, 6_000]);
        assert!(rows.iter().all(|m| m.absorbed));
    }

    /// The credits `m` charges at its rate.
    fn credits_of(m: &Metered) -> i64 {
        let r = m.rate.unwrap();
        row_credits_milli(
            m.qty,
            r.cost_micro_usd,
            r.margin_bp,
            r.fixed_credits_milli,
            r.credit_price_micro_usd,
            r.unit.base(),
        )
        .unwrap()
    }

    #[test]
    fn a_mixed_card_bills_the_cost_based_rows_and_keeps_the_credit_based_ones() {
        let mut rows = vec![
            row("input", 1_000, rate(Unit::MtokInput, 3_000_000, None)),
            row("output", 300, rate(Unit::MtokOutput, 0, Some(2_800_000))),
        ];
        bill(&mut rows, 5_000);
        assert_eq!(costs(&rows), [5_000, 0]);
        assert_eq!(rows[0].rate.unwrap().source, RateSource::Provider);
        assert_eq!(rows[1].rate.unwrap().source, RateSource::Catalog);
        // $0.005 at 40 % over a $0.01 credit; the fixed 840 unchanged.
        assert_eq!(rows.iter().map(credits_of).collect::<Vec<_>>(), [700, 840]);
    }

    #[test]
    fn a_row_over_a_million_tokens_falls_short_by_under_its_millions() {
        let mut rows = vec![
            row("input", 3_000_001, rate(Unit::MtokInput, 3_000_000, None)),
            row("output", 300, rate(Unit::MtokOutput, 15_000_000, None)),
        ];
        bill(&mut rows, 10_000_007);
        let sum: i64 = costs(&rows).iter().sum();
        assert!((10_000_007 - 4..=10_000_007).contains(&sum), "{sum}");
    }

    #[test]
    fn a_billed_cost_of_0_charges_no_credits_on_cost_based_rows() {
        let mut rows = vec![
            row("input", 1_000, rate(Unit::MtokInput, 3_000_000, None)),
            row("output", 300, rate(Unit::MtokOutput, 15_000_000, None)),
        ];
        bill(&mut rows, 0);
        assert_eq!(costs(&rows), [0, 0]);
        assert_eq!(rows.iter().map(credits_of).collect::<Vec<_>>(), [0, 0]);
    }

    #[test]
    fn a_billed_row_without_a_snapshotted_rate_is_resolved_before_billing() {
        let u = Usage {
            input_tokens: 1_000,
            output_tokens: 300,
            ..Usage::default()
        };
        // The snapshot lacks the output rate.
        let rates = json!({"input": rate(Unit::MtokInput, 3_000_000, None)});
        let resolve = |line: &str, _: KeySource| {
            Ok(line
                .ends_with(".output")
                .then(|| rate(Unit::MtokOutput, 15_000_000, None)))
        };
        let rows = priced(
            token_rows("m", u, KeySource::Instance, false),
            &rates,
            Some(10_000),
            KeySource::Instance,
            &resolve,
        )
        .unwrap();
        assert_eq!(costs(&rows), [4_000, 6_000]);
        assert!(
            rows.iter()
                .all(|m| m.rate.unwrap().source == RateSource::Provider)
        );
        // Unbilled, a row without a snapshot is left to the commit's line.
        let rows = priced(
            token_rows("m", u, KeySource::Instance, false),
            &rates,
            None,
            KeySource::Instance,
            &resolve,
        )
        .unwrap();
        assert!(rows[1].rate.is_none());
    }

    #[test]
    fn an_own_key_is_charged_nothing_for_a_model_without_a_rate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        store
            .set_bot_settings(
                &AuditCtx::DAEMON,
                &[(Setting::RequireExplicitPrice, Some(json!(true)))],
            )
            .unwrap();
        let unit = Unit::MtokOutput;
        assert_eq!(
            token_rate(&store, "acme-model-1", unit, KeySource::Instance).unwrap(),
            None
        );
        let r = token_rate(&store, "acme-model-1", unit, KeySource::Own)
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                r.unit,
                r.cost_micro_usd,
                r.fixed_credits_milli,
                r.applies,
                r.source
            ),
            (unit, 0, None, Applies::InstanceKey, RateSource::Default)
        );
        assert_eq!(
            line_token_rate(&store, "acme-model-1.output", KeySource::Own).unwrap(),
            Some(r)
        );
        assert_eq!(
            line_token_rate(&store, "script.compute", KeySource::Own).unwrap(),
            None
        );
    }
}
