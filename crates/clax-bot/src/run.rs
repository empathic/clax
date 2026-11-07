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
//! turn, or none for a workspace call that goes on without the tick
//! ([`Wait::Budget`]). A step whose commit fails
//! `max_commit_failures` times ends the run `failed`.

use crate::events::{Event, EventSink, step_summary};
use crate::host::{BotHost, INTENT_SEQ, NOTICE_SEQ, OpCtx, run_plan};
use crate::prompt;
use crate::web::WebSource;
use crate::web::page::{sources as web_sources, sources_markdown};
use clax_bot_sandbox::{CancelToken, HostError, Limits, MaybeSendSync, Sandbox};
use clax_core::audit::{Actor, AuditCtx, AuditRecord, Via};
use clax_core::gitctx::GitField;
use clax_core::model::AUTHOR_AGENT;
use clax_core::working::Clock;
use clax_core::{CoreError, Result};
use clax_sample::messages::{
    ApiKey, AssistantMessage, Block, CacheControl, Delta, Message, MessagesClient, MessagesError,
    MessagesRequest, ModelApi, RetryPolicy, Role, StopReason, SystemBlock, Thinking, Timeouts,
    Tool, ToolCall, ToolChoice, Usage,
};
use clax_store::Store;
use clax_store::bot_runs::{
    Committed, DriverId, Run, RunStatus, Step, StepCommit, StepKind, call_record, turn_record,
};
use clax_store::bot_settings::Setting;
use clax_store::credits::{
    Applies, KeySource, LINE_DO_REQUESTS, LINE_HARNESS_FEE, LINE_TOOLPATH_STORAGE, Metered, Rate,
    RateSource, TOKEN_UNITS, Unit, parse_token_line, row_credits_milli, token_line, week_end,
    week_start,
};
use clax_store::queues::{RELEASED_BODY, Visibility};
use clax_store::secrets::{Kek, SecretSlot};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

/// A workspace whose offers are a copy of its word, which grows old while
/// nothing reaches what gave it. That is its own state, apart from
/// [`crate::workspace::Offer::unsure`], the word that a target could not be
/// read: what a run it reaches gets is withheld and reads as confidential while
/// the offers are lapsed, and it says nothing of any run, so nothing is kept of
/// it ([`Deps::confidential`]). It reaches a run that began a workspace call and
/// any [`OffersLapse::lapse_reaches`] names. Any other run has no word of the
/// workspace's to lose: it goes on as a run of a host with no workspace.
pub trait OffersLapse: MaybeSendSync {
    /// Whether what [`crate::workspace::Workspace::offer`] answers now is no
    /// word of the workspace's.
    fn offers_lapsed(&self) -> bool;

    /// Whether the workspace's word might matter to `run`, on a report of
    /// `queue` if it is on one, for all this copy of it says.
    fn lapse_reaches(&self, _run: &Run, _queue: Option<&str>) -> bool {
        true
    }
}

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
    /// Where `clax.web.*` calls are sent (spec
    /// 2026-11-02-claxbot-web-tools §4.3); `None` on a host without one,
    /// where every web call is refused `web_disabled`.
    pub web: Option<Arc<dyn WebSource>>,
    /// The run's ephemeral container ([`crate::workspace`]); `None` on a
    /// host without one, where no run declares the shell operations.
    pub workspace: Option<Arc<dyn crate::workspace::Workspace>>,
    /// Says when the workspace's offers have lapsed, on a workspace whose
    /// offers can ([`Deps::with_lapsing_workspace`]).
    pub lapse: Option<Arc<dyn OffersLapse>>,
    /// What tells the host of its workspace's previews.
    previews: Option<Arc<HostPreviews>>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("keys", &self.keys)
            .field("retry", &self.retry)
            .field("web", &self.web.is_some())
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Deps with the default retry policy, a 30 s wait after the model
    /// stays unreachable, no crash hook and no web source.
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
            web: None,
            workspace: None,
            lapse: None,
            previews: None,
        }
    }

    /// These deps, sending web calls to `web`.
    #[must_use]
    pub fn with_web(mut self, web: Arc<dyn WebSource>) -> Deps {
        self.web = Some(web);
        self
    }

    /// These deps, with `workspace` for the runs it offers anything.
    #[must_use]
    pub fn with_workspace(mut self, workspace: Arc<dyn crate::workspace::Workspace>) -> Deps {
        let previews = Arc::new(HostPreviews {
            store: self.store.clone(),
            host: self.host.clone(),
            proposing: Mutex::default(),
            asks_again: workspace.resumable(),
        });
        workspace.previews(previews.clone());
        self.previews = Some(previews);
        self.workspace = Some(workspace);
        self
    }

    /// These deps, with `workspace`, whose offers can lapse.
    #[must_use]
    pub fn with_lapsing_workspace<W>(mut self, workspace: Arc<W>) -> Deps
    where
        W: crate::workspace::Workspace + OffersLapse + 'static,
    {
        self.lapse = Some(workspace.clone());
        self.with_workspace(workspace)
    }

    /// What a lapse of the workspace's offers is to `run`: `None` with none.
    /// `Some(true)` when it reaches the run, as it does a run that cannot be
    /// told apart, and `Some(false)` when the run has nothing to lose by it
    /// ([`OffersLapse`]).
    fn lapse_on(&self, run: &Run) -> Option<bool> {
        let lapse = self.lapse.as_ref().filter(|l| l.offers_lapsed())?;
        Some(match (queue_of(&self.store, run), self.touched(run)) {
            (Ok(queue), Ok(false)) => lapse.lapse_reaches(run, queue.as_deref()),
            _ => true,
        })
    }

    /// Whether the workspace's offers have lapsed for `run` ([`OffersLapse`]).
    pub(crate) fn offers_lapsed(&self, run: &Run) -> bool {
        self.lapse_on(run) == Some(true)
    }

    /// Holds what is told of `run`'s preview until the answer is settled: a
    /// preview already up can be told of before its fix is on the report.
    pub(crate) fn fix_coming(&self, run: &str) -> FixComing {
        let previews = self.previews.clone();
        if let Some(p) = &previews {
            p.lock().entry(run.to_string()).or_default();
        }
        FixComing {
            previews,
            run: run.to_string(),
        }
    }

    /// Whether `run` may hold material a reporter must not read, kept on
    /// the run once true ([`Store::mark_run_confidential`]) and never asked
    /// again: a target edited out, a workspace gone from the host or a
    /// failed lookup cannot lift it. A run that touched a workspace which no
    /// longer answers for it is, and so is one on a thread that holds
    /// such material already ([`Deps::threads_hold_secrets`]). A failed
    /// lookup, the store's or the workspace's ([`Offer::unsure`]), reads as
    /// yes for this tick, and is kept only for a run that touched a
    /// workspace: one that touched none holds nothing, and asks again.
    /// Offers that have lapsed for it ([`OffersLapse`]) read as yes while they
    /// are lapsed, and nothing is kept: they say nothing of this run.
    ///
    /// [`Offer::unsure`]: crate::workspace::Offer::unsure
    ///
    /// # Errors
    /// When it is true and cannot be kept: the tick must not go on as
    /// though it were.
    pub fn confidential(&self, run: &Run) -> Result<bool> {
        Ok(self.standing(run)? != Standing::Clear)
    }

    /// Where `run` stands ([`Deps::confidential`]).
    fn standing(&self, run: &Run) -> Result<Standing> {
        if run.confidential {
            return Ok(Standing::Confidential);
        }
        let keep = match self.looks_confidential(run) {
            Ok(Standing::Lapsed) => return Ok(Standing::Lapsed),
            Ok(s) => s == Standing::Confidential,
            Err(e) => {
                tracing::warn!("clax-bot: telling what run {} holds: {e}", run.id);
                if !self.touched(run).unwrap_or(false) {
                    return Ok(Standing::Confidential);
                }
                true
            }
        };
        if !keep {
            return Ok(Standing::Clear);
        }
        self.store.mark_run_confidential(&run.id)?;
        Ok(Standing::Confidential)
    }

    /// Whether `run` has begun a workspace call.
    fn touched(&self, run: &Run) -> Result<bool> {
        self.store.run_called_from(&run.id, INTENT_SEQ)
    }

    /// Whether a run on `run`'s threads reads what a reporter must not: an
    /// earlier run there was confidential, or a person left a note for the
    /// queue's members, which a run of the queue's reads with the report. A
    /// note a run wrote says no more than that run was allowed to, and a
    /// claim released in no words of its own says nothing.
    fn threads_hold_secrets(&self, run: &Run) -> Result<bool> {
        for tid in &run.thread_ids {
            if self
                .store
                .runs_for_thread(tid, u32::MAX)?
                .iter()
                .any(|r| r.confidential)
            {
                return Ok(true);
            }
            let Some(q) = self.store.get_thread(tid)?.and_then(|t| t.queue_id) else {
                continue;
            };
            let report = self.store.report(&q, tid, Visibility::All)?;
            let by_person = |c: &clax_core::queue::ReportComment| {
                c.triage_only && c.author_kind != AUTHOR_AGENT && c.body != RELEASED_BODY
            };
            if report.is_some_and(|r| r.comments.iter().any(by_person)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn looks_confidential(&self, run: &Run) -> Result<Standing> {
        if self.threads_hold_secrets(run)? {
            return Ok(Standing::Confidential);
        }
        let touched = || self.touched(run).map(Standing::of);
        let Some(w) = &self.workspace else {
            return touched();
        };
        let queue = match queue_of(&self.store, run) {
            Ok(q) => q,
            // Its thread was deleted: what it holds is what it touched.
            Err(CoreError::NotFound) => return touched(),
            Err(e) => return Err(e),
        };
        match self.lapse_on(run) {
            Some(true) => return Ok(Standing::Lapsed),
            Some(false) => return Ok(Standing::Clear),
            None => {}
        }
        let offer = w.offer(run, queue.as_deref());
        if offer.unsure {
            return Err(CoreError::invalid(
                "target_unread",
                "the run's target could not be read",
            ));
        }
        if offer.confidential {
            return Ok(Standing::Confidential);
        }
        if offer.shell {
            Ok(Standing::Clear)
        } else {
            touched()
        }
    }

    /// Whether anyone may be able to read `run`'s target, its pull requests
    /// with it, apart from anything the run itself holds. Not knowing is yes:
    /// only a target that says it is private is taken as private.
    #[must_use]
    pub fn target_may_be_public(&self, run: &Run) -> bool {
        let Some(w) = &self.workspace else {
            return true;
        };
        match queue_of(&self.store, run) {
            Ok(queue) => !w.target_private(run, queue.as_deref()),
            Err(_) => true,
        }
    }

    /// What `run` gets of the host's workspace, if it has one, asked at
    /// each call so a right lost during the run ends its shell. On a report
    /// a run gets nothing unless [`Store::run_reads_queue`], and a run a
    /// reply started gets no change to propose. Confidentiality is the
    /// run's own ([`Deps::confidential`]) or what the workspace says.
    #[must_use]
    pub fn workspace_offer(&self, run: &Run) -> crate::workspace::Offer {
        use crate::workspace::Offer;
        let confidential = run.confidential;
        let Some(w) = &self.workspace else {
            return Offer {
                confidential,
                ..Offer::default()
            };
        };
        let queue = match queue_of(&self.store, run) {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!("clax-bot: finding the queue of run {}: {e}", run.id);
                return Offer {
                    confidential,
                    ..Offer::default()
                };
            }
        };
        if self.lapse_on(run) == Some(false) {
            return Offer {
                confidential,
                ..Offer::default()
            };
        }
        let mut offer = w.offer(run, queue.as_deref());
        offer.confidential |= confidential;
        if let Some(q) = &queue {
            let grant = run.agent_grant.as_deref();
            let reads = self.store.run_reads_queue(&run.person, grant, q);
            if !matches!(reads, Ok(true)) {
                return Offer {
                    confidential: offer.confidential,
                    ..Offer::default()
                };
            }
        }
        if run.woken_by == Some(clax_store::bot_runs::WokenBy::Reply) {
            offer.change = false;
            offer.answers_only = true;
        }
        offer
    }

    /// The queue of the report `run` works on, if it works on one.
    ///
    /// # Errors
    /// The store's, and `NotFound` for a thread of the run that is gone: a
    /// run whose queue cannot be told is not a run with no queue.
    pub(crate) fn run_queue(&self, run: &Run) -> Result<Option<String>> {
        queue_of(&self.store, run)
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

/// A fix on its way to a report, from [`Deps::fix_coming`]. However the call
/// ends what was held is told: awaited through [`FixComing::settled`], or from
/// a task of its own when a Stop or a lost claim drops the step.
pub(crate) struct FixComing {
    previews: Option<Arc<HostPreviews>>,
    run: String,
}

impl FixComing {
    /// The fix is on its report, or will not be.
    pub(crate) async fn settled(mut self) {
        if let Some(p) = self.previews.take() {
            p.settle(&self.run).await;
        }
    }
}

impl Drop for FixComing {
    fn drop(&mut self) {
        if let (Some(p), Ok(rt)) = (self.previews.take(), clax_rt::Handle::try_current()) {
            let run = std::mem::take(&mut self.run);
            rt.spawn(async move { p.settle(&run).await });
        }
    }
}

/// A fix's preview, told to the host as ClaxBot acting for the person of
/// the run that proposed it.
struct HostPreviews {
    store: Arc<Store>,
    host: Arc<dyn crate::host::BotHost>,
    /// The runs between a proposal and its fix, each with what was told of
    /// its preview meanwhile, the latest word.
    proposing: Mutex<HashMap<String, Option<Told>>>,
    /// The workspace keeps a word it was refused and asks again, so a run
    /// between ticks, whose proposal no call here holds, can refuse it.
    asks_again: bool,
}

/// Whether `calls`, one step's journal, hold a `change.propose` begun and
/// never answered.
fn proposal_unanswered(calls: &[clax_store::CallEntry]) -> bool {
    calls.iter().any(|c| {
        c.seq() >= INTENT_SEQ
            && c.op() == crate::workspace::OP_PROPOSE
            && !calls.iter().any(|a| a.seq() == c.seq() - INTENT_SEQ)
    })
}

/// Whether `calls`, a run's journal of `change.propose`, hold a proposal
/// whose change may be out, recorded on the report or not: one begun that no
/// answer shows left nothing ([`proposal_left_nothing`]). A proposal refused
/// before it was begun has no intent here and began nothing.
fn proposal_stands(calls: &[clax_store::CallEntry]) -> bool {
    calls.iter().any(|c| {
        c.seq() >= INTENT_SEQ
            && !calls.iter().any(|a| {
                a.step_n() == c.step_n()
                    && a.seq() == c.seq() - INTENT_SEQ
                    && proposal_left_nothing(c, a)
            })
    })
}

/// Whether `answer` proves the proposal begun at `intent` left nothing: a
/// [`crate::workspace::CODE_CHANGE_REFUSED`] from the attempt that began it,
/// which is the workspace saying this call pushed nothing. An attempt after
/// that came back to a call that may have gone further than anyone heard, so
/// what it was told says nothing of the first. No other failure proves it: a
/// report that no longer takes the fix says so of a change that may have
/// left, a failure once the change is pushed is no refusal, and what nothing
/// here knows may be either.
fn proposal_left_nothing(intent: &clax_store::CallEntry, answer: &clax_store::CallEntry) -> bool {
    !answer.ok()
        && answer.attempt() == intent.attempt()
        && answer.result()["code"] == crate::workspace::CODE_CHANGE_REFUSED
}

/// What a workspace said of a preview.
enum Told {
    /// Its link, and [`crate::workspace::Offer::reviewed_previews`] of what
    /// the run was offered.
    Ready(String, bool),
    Missed,
}

impl HostPreviews {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<Told>>> {
        self.proposing.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Refuses a word about `run` that its fix is not there yet to take.
    fn no_fix_yet(&self, run: &str) -> std::result::Result<(), HostError> {
        if !self.asks_again {
            return Ok(());
        }
        let host_error = |e: CoreError| crate::host::host_error(&e);
        let Some(open) = self.store.open_step(run).map_err(host_error)? else {
            return Ok(());
        };
        let calls = self.store.bot_calls(run, open.n).map_err(host_error)?;
        if proposal_unanswered(&calls) {
            return Err(HostError::new(
                crate::workspace::CODE_FIX_COMING,
                "this run's fix is not recorded yet",
            )
            .retryable());
        }
        Ok(())
    }

    /// Keeps `told` for later when `run` is between a proposal and its fix.
    fn held(&self, run: &str, told: impl FnOnce() -> Told) -> bool {
        match self.lock().get_mut(run) {
            Some(slot) => {
                *slot = Some(told());
                true
            }
            None => false,
        }
    }

    /// Tells what was held for `run`, and holds no more.
    async fn settle(&self, run: &str) {
        let held = self.lock().remove(run).flatten();
        let told = match held {
            Some(Told::Ready(url, reviewed)) => self.ready(run, &url, reviewed).await,
            Some(Told::Missed) => self.missed(run).await,
            None => Ok(()),
        };
        if let Err(e) = told {
            tracing::warn!("clax-bot: telling of run {run}'s preview: {e}");
        }
    }

    async fn ready(
        &self,
        run: &str,
        verify_url: &str,
        reviewed: bool,
    ) -> std::result::Result<(), HostError> {
        let run = self
            .store
            .run(run)
            .map_err(|e| crate::host::host_error(&e))?
            .ok_or_else(|| HostError::new("not_found", format!("no run {run}")))?;
        // A confidential run's preview is the reporter's only where a
        // person approves what a preview serves, by the terms the run ran on.
        let for_reporter = !run.confidential || reviewed;
        self.host
            .preview_ready(&run, &bot_ctx(&run.person), verify_url, for_reporter)
            .await
    }

    async fn missed(&self, run: &str) -> std::result::Result<(), HostError> {
        let run = self
            .store
            .run(run)
            .map_err(|e| crate::host::host_error(&e))?
            .ok_or_else(|| HostError::new("not_found", format!("no run {run}")))?;
        self.host.preview_missed(&run, &bot_ctx(&run.person)).await
    }
}

/// The queue of the report `run` works on: see [`Deps::run_queue`].
fn queue_of(store: &Store, run: &Run) -> Result<Option<String>> {
    for tid in &run.thread_ids {
        let t = store.get_thread(tid)?.ok_or(CoreError::NotFound)?;
        if t.queue_id.is_some() {
            return Ok(t.queue_id);
        }
    }
    Ok(None)
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::workspace::PreviewSink for HostPreviews {
    async fn preview_ready(
        &self,
        run: &str,
        verify_url: &str,
        reviewed: bool,
    ) -> std::result::Result<(), HostError> {
        self.no_fix_yet(run)?;
        if self.held(run, || Told::Ready(verify_url.to_string(), reviewed)) {
            return Ok(());
        }
        self.ready(run, verify_url, reviewed).await
    }

    async fn preview_missed(&self, run: &str) -> std::result::Result<(), HostError> {
        self.no_fix_yet(run)?;
        if self.held(run, || Told::Missed) {
            return Ok(());
        }
        self.missed(run).await
    }

    async fn preview_lost(&self, run: &str) -> std::result::Result<(), HostError> {
        let found = self.store.run(run);
        let Some(run) = found.map_err(|e| crate::host::host_error(&e))? else {
            return Ok(());
        };
        let words = crate::workspace::PREVIEW_LOST;
        self.host
            .note_members(&run, &bot_ctx(&run.person), words)
            .await
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
    /// ([`RunSettings::min_turn_budget`]), or none for a script's workspace
    /// call that goes on without the tick
    /// ([`crate::workspace::Workspace::resumable`]): tick it again with a
    /// fresh one.
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
    /// The threads replied on since a script last changed the draft or
    /// published (in that script or after it): a reply before the work,
    /// such as the plan, leaves the `finish` summary to be posted.
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
        billed_micro_usd: v["billed_micro_usd"].as_u64(),
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
                    // A script that changed the draft or published starts
                    // the replies over: one before it (the plan) does not
                    // say what was done.
                    let wrote = p["changed"].as_array().is_some_and(|c| !c.is_empty())
                        || !p["published"].is_null();
                    if wrote {
                        h.replied.clear();
                    }
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
    let (mut run, steps) = store.run_with_steps(run_id)?.ok_or(CoreError::NotFound)?;
    // Lapsed offers stay out of this: [`Tick::holds_secrets`] reads them as
    // they stand, so a tick they come back in goes by them.
    run.confidential = deps.standing(&run)? == Standing::Confidential;
    let confidential = AtomicBool::new(run.confidential);
    let hist = History::of(&steps);
    let n = store.next_step_n(run_id)?;
    let open = steps.into_iter().find(|s| s.committed_at.is_none());
    let plain = plain_threads(store, &run);
    let t = Tick {
        deps,
        store,
        settings,
        ctx: bot_ctx(&run.person),
        plain,
        run,
        confidential,
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
    /// Whether the run holds confidential material as of now: the run's own
    /// flag as the tick began, or a call of this tick that marked it.
    pub confidential: AtomicBool,
    pub hist: History,
    pub n: i64,
    /// The step begun and not committed, as the tick found it.
    pub open: Option<Step>,
    pub driver: &'a DriverId,
    pub opts: TickOpts<'a>,
    pub started: clax_rt::time::Instant,
    /// The run's threads the tick found on no queue. A thread gets its queue
    /// when it is made, so a reply on one of these need ask the store nothing
    /// and cannot be held by a read that fails. A thread that could not be
    /// read is not one.
    pub plain: BTreeSet<String>,
}

/// The threads of `run` that read as being on no queue.
fn plain_threads(store: &Store, run: &Run) -> BTreeSet<String> {
    let plain =
        |tid: &&String| matches!(store.get_thread(tid), Ok(Some(t)) if t.queue_id.is_none());
    run.thread_ids.iter().filter(plain).cloned().collect()
}

/// Where a run stands on confidential material ([`Deps::confidential`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    Clear,
    Confidential,
    /// Not known: the workspace's offers have lapsed ([`OffersLapse`]).
    Lapsed,
}

impl Standing {
    fn of(confidential: bool) -> Standing {
        if confidential {
            Standing::Confidential
        } else {
            Standing::Clear
        }
    }
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
    pub(crate) fn remaining(&self) -> Duration {
        Duration::from_secs(u64::from(self.settings.max_run_minutes) * 60)
            .saturating_sub(self.elapsed())
    }

    /// What is left of the tick's budget and the run's time.
    pub(crate) fn available(&self) -> Duration {
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
    ) -> std::result::Result<clax_store::CallEntry, HostError> {
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
        OpCtx::new(&self.run, &self.ctx, self.n, attempt, seq).members_only(self.holds_secrets())
    }

    /// [`Self::op_ctx`] for a reply on thread `tid`, the model's or the run's
    /// own notice: members only as well when the fix keeps the run from the
    /// report's reporter ([`Self::fix_holds_replies`]). Reads stay with
    /// [`Self::op_ctx`], since a reply held says nothing of what the run may
    /// read. A hold that cannot be told is taken to stand, on any thread not
    /// known to have no reporter ([`Tick::plain`]).
    pub(crate) fn reply_ctx(&self, attempt: i64, seq: i64, tid: &str) -> OpCtx<'_> {
        let held = self.fix_holds_replies(tid).unwrap_or_else(|e| {
            tracing::warn!("clax-bot: reading the fixes of run {}: {e}", self.run.id);
            true
        });
        let cx = self.op_ctx(attempt, seq);
        let members_only = cx.members_only || held;
        cx.members_only(members_only)
    }

    /// Whether the run's words on report `tid` stop at the queue's people:
    /// from a proposal whose change may be out on ([`proposal_stands`]), for
    /// the rest of the run (a preview that answers changes nothing: what
    /// ClaxBot writes of a fix stays with them), and while a run's fix waits
    /// there, unless this run was woken to answer the reporter of a fix they
    /// have to check. Asked of the store at every reply, so it holds on a step
    /// executed again, a tick that resumes one and either host. A thread no
    /// queue holds has no reporter, and one the tick found so is not asked
    /// of the store at all.
    fn fix_holds_replies(&self, tid: &str) -> Result<bool> {
        if self.plain.contains(tid) {
            return Ok(false);
        }
        let on_report = self
            .store
            .get_thread(tid)?
            .is_some_and(|t| t.queue_id.is_some());
        if !on_report {
            return Ok(false);
        }
        let op = crate::workspace::OP_PROPOSE;
        if proposal_stands(&self.store.bot_calls_of(&self.run.id, op)?) {
            return Ok(true);
        }
        let answers = self.run.woken_by == Some(clax_store::bot_runs::WokenBy::Reply);
        Ok(match self.store.waiting_run_fix_held(tid)? {
            Some(held) => held || !answers,
            None => false,
        })
    }

    /// Whether the run holds confidential material as of now, or the
    /// workspace's offers have lapsed for it, so that it cannot be told.
    pub(crate) fn holds_secrets(&self) -> bool {
        self.confidential.load(Ordering::SeqCst) || self.deps.offers_lapsed(&self.run)
    }

    /// Marks the run as holding confidential material, for good and for the
    /// rest of this tick: a workspace that offers some, before its first
    /// effect. Lapsed offers say nothing of the run, so nothing is marked.
    ///
    /// # Errors
    /// The store's: the call must not go on as though it were kept.
    pub(crate) fn mark_confidential(&self) -> Result<()> {
        if !self.deps.offers_lapsed(&self.run) && !self.confidential.load(Ordering::SeqCst) {
            self.store.mark_run_confidential(&self.run.id)?;
            self.confidential.store(true, Ordering::SeqCst);
        }
        Ok(())
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
            commit.usage.extend(observed_rows(
                &self.run.model,
                &open,
                observed,
                &|line, ks| line_token_rate(self.store, line, ks),
            )?);
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
        self.notify_at(attempt, NOTICE_SEQ, markdown).await;
    }

    /// [`Self::notify`], the `i`th thread's post at journal place `first + i`.
    pub(crate) async fn notify_at(&self, attempt: i64, first: i64, markdown: &str) {
        for (i, t) in self.run.thread_ids.iter().enumerate() {
            let seq = first + i as i64;
            let cx = self.reply_ctx(attempt, seq, t);
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
        self.tell_stranded().await;
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
        self.ended(&c).await;
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

    /// Tells the queue's members when this step ends with a `change.propose`
    /// begun and unanswered: nothing else will say where its change went.
    /// Called again it adds nothing ([`BotHost::note_members`]).
    pub(crate) async fn tell_stranded(&self) {
        let calls = self.store.bot_calls(&self.run.id, self.n);
        if !calls.is_ok_and(|c| proposal_unanswered(&c)) {
            return;
        }
        if !matches!(self.deps.run_queue(&self.run), Ok(Some(_))) {
            return;
        }
        let words = crate::workspace::PROPOSAL_STRANDED;
        let told = self.deps.host.note_members(&self.run, &self.ctx, words);
        if let Err(e) = told.await {
            tracing::warn!(
                "clax-bot: telling of a stranded proposal of {}: {e}",
                self.run.id
            );
        }
    }

    /// Ends the run `failed`: its step could not be committed.
    async fn fail(&self) -> Result<TickOutcome> {
        let s = self.begin(StepKind::Failed)?;
        self.tell_stranded().await;
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
        self.ended(&c).await;
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
        let pages = self.store.web_pages(&self.run.id)?;
        let sources = sources_markdown(&pages);
        match (&sources, post) {
            (Some(list), true) => {
                self.notify(s.attempt, &format!("{summary}{list}")).await;
            }
            (None, true) => self.notify(s.attempt, summary).await,
            // The run replied itself: the list stands alone, after the
            // summary's place.
            (Some(list), false) => {
                self.notify_at(s.attempt, NOTICE_SEQ + 1, list.trim_start())
                    .await;
            }
            (None, false) => {}
        }
        let payload = json!({
            "summary": summary,
            "posted": post,
            "sources": web_sources(&pages).iter()
                .map(|s| json!({"title": s.title.as_deref().unwrap_or(&s.url), "url": s.url}))
                .collect::<Vec<_>>(),
        });
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
        self.ended(&c).await;
        Ok(self.outcome(StepKind::Finish, c))
    }

    /// What every way a run can end does once its end is committed
    /// (`c`): stops showing it working and ends its container. A commit
    /// that did not go through ended nothing: the step runs again.
    async fn ended<T, E>(&self, c: &std::result::Result<T, E>) {
        if c.is_err() {
            return;
        }
        self.deps.host.working(&self.run, false).await;
        if let Some(w) = &self.deps.workspace {
            w.finish(&self.run).await;
        }
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
        self.ended(&c).await;
        Ok(self.outcome(StepKind::GiveUp, c))
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
    fn affordable(
        &self,
        ks: KeySource,
        request_bytes: usize,
    ) -> Result<std::result::Result<u32, Stop>> {
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
        let run_left = i64::try_from(self.settings.max_run_credits.saturating_mul(1000))
            .unwrap_or(i64::MAX)
            .saturating_sub(self.run.credits_milli)
            .max(0);
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
        let input = match self.hist.last_usage {
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
            let mut offer = self.deps.workspace_offer(&self.run);
            offer.confidential |= self.holds_secrets();
            // A confidential run's web calls are refused: it declares none.
            let web = if offer.confidential {
                crate::web::WebShape::default()
            } else {
                crate::web::WebShape::parse(&self.run.web)
            };
            (
                prompt::system_prompt_for(self.run.request, web, offer),
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
        let mut s = self.begin_on(StepKind::Turn, ks)?;
        // The rates the call that answers snapshotted, to charge it at.
        let mut call_rates: Value;
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
                    if deltas.is_multiple_of(OBSERVE_EVERY) {
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
                        s = self.begin_on(StepKind::Turn, ks)?;
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
        if let Some(b) = msg.usage.billed_micro_usd {
            payload["usage"]["billed_micro_usd"] = json!(b);
        }
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
        let mut usage = priced(
            token_rows(&self.run.model, msg.usage, ks, false),
            &call_rates,
            msg.usage.billed_micro_usd,
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
