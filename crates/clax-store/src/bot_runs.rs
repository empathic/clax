//! ClaxBot runs and their steps (migrations 50, 64 and 65, spec
//! 2026-10-21-claxbot-design §4.2, §6.3).
//!
//! A **run** is ClaxBot working one request for one person. It moves
//! `queued` → `running` (a driver claimed it) → `finished`, `gave_up` or
//! `stopped`; a cancel makes it `stopping` until the driver writes its
//! `stopped` step. A running run whose step awaits a call that is slow
//! to answer is **parked** by its driver ([`Store::park_run`]): it keeps
//! its open step and its threads and has no driver, until the call's
//! answer or deadline is journaled ([`crate::bot_pending`]) or it is
//! cancelled, each of which makes it `queued` again (`stopping`, for a
//! cancel), due at once, for a driver to claim and execute the step again
//! (spec 2026-11-10-claxbot-surfaces-design, "Calls that wait"). A run
//! is a sequence of **steps**, each begun
//! ([`Store::begin_step`]) and then committed ([`Store::commit_bot_step`])
//! before the next begins, so a run survives a restart: a step begun but
//! not committed is found again ([`Store::open_step`]) and executed again.
//!
//! Steps are **fenced**: a driver begins and commits a step only while it
//! holds the run's claim ([`Store::claim_run`], kept alive across a long
//! turn by [`Store::renew_lease`]), and names the step it expects, so a
//! driver whose claim another took over, or that lost track of the run
//! after a crash, is refused (`run_not_claimed`, `step_mismatch`) instead
//! of running or committing a step twice. A driver is a [`DriverId`],
//! unique per process natively (minted when the daemon starts) and per
//! Durable Object incarnation on the Worker (minted when the instance
//! object is constructed); a driver ID shared by two executors would let
//! both hold one claim, so it is never reused.
//!
//! A step's commit is one transaction: the step's payload, its usage rows
//! ([`crate::credits`]), its draft changes, the run's running credit total
//! and status, and its audit events. The store stamps each event with the
//! run's artifact and its `run` and `step`, fills a `bot.turn`'s or
//! `bot.script`'s `credits_milli` with the step's charge, and records
//! `bot.run_start` when a run is made and `bot.run_end` when a terminal
//! step (`finish`, `give_up`, `stopped`) commits. A web call's usage row
//! is the one charged before its step commits: it is written, and added
//! to the run's total, in the call's own transaction
//! ([`Store::spend_journaled`]).
//!
//! A run's **draft** is an overlay on the version it was based on
//! (`base_version`): each path it wrote, with its content, or deleted.
//! The artifact changes only when the run publishes it, through the
//! artifact's own publish path.
//!
//! A run is also kept as a **kind**, a **request** and **bindings**
//! (migration 64, spec 2026-11-10-claxbot-surfaces-design, "A run"):
//! `bot_runs.kind`, `request_json` ([`ClaxRequest`]) and `bindings_json`
//! ([`Bindings`]), written when the run is made from the same values as
//! its `request`, `thread_ids`, `artifact_id` and `web`, and never
//! changed. A run made before migration 64 holds NULL in all three and is
//! read as made with those values. [`RunAsMade`] is the three as they are
//! kept, which the run
//! loop reads with the run's row and steps
//! ([`Store::run_with_steps_as_made`]); [`Run`] is read from the
//! earlier columns, of which `request`, `thread_ids`, `artifact_id`,
//! `base_version` and `web` are the `clax` surface's own.

use super::Store;
use super::credits::{KeySource, Metered, UsageRow, charge_in, check_model_priced};
use super::roles::set_by;
use clax_core::audit::{AuditCtx, AuditIds, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::Value;

/// Declares a string enum with its stored names.
macro_rules! named {
    ($(#[$m:meta])* $name:ident { $($v:ident = $s:literal,)* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($v,)*
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v,)*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$v => $s,)*
                }
            }

            pub fn parse(s: &str) -> Option<$name> {
                match s {
                    $($s => Some($name::$v),)*
                    _ => None,
                }
            }
        }
    };
}

named! {
    /// Where a run is.
    RunStatus {
        Queued = "queued",
        Running = "running",
        Stopping = "stopping",
        Parked = "parked",
        Finished = "finished",
        GaveUp = "gave_up",
        Stopped = "stopped",
        Failed = "failed",
    }
}

impl RunStatus {
    /// Whether the run has ended.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Finished | RunStatus::GaveUp | RunStatus::Stopped | RunStatus::Failed
        )
    }

    /// Whether the run has not ended: `queued`, `running`, `stopping` or
    /// `parked`. Such a run holds its threads (a second send of one
    /// returns it, and follow-ups queue behind it), is listed for a
    /// driver that starts, and keeps the cost lines that price it.
    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }
}

/// The statuses of a run that has not ended ([`RunStatus::is_live`]), as
/// SQL's `IN` takes them: the one spelling of the set for every query
/// that filters on it. The index `bot_runs_live` (migration 65) is over
/// the same list, so those queries read it.
pub(crate) const LIVE: &str = "('queued', 'running', 'stopping', 'parked')";

/// What a run's request is, as the `request` column has it: one of the
/// two kinds of Clax's own surface, or that the run is of another
/// surface's kind, which `bot_runs.kind` names ([`RunAsMade::kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// `clax.thread`: threads sent to ClaxBot.
    Thread,
    /// `clax.brief`: the brief of a new artifact.
    Brief,
    /// A kind of a surface other than Clax's ([`Store::create_run_of`]):
    /// a run with no thread, and no artifact unless it binds `clax`.
    Other,
}

impl RequestKind {
    /// The kinds of Clax's own surface: every value but
    /// [`RequestKind::Other`], which is no kind of its own.
    pub const ALL: &'static [RequestKind] = &[RequestKind::Thread, RequestKind::Brief];

    pub fn as_str(self) -> &'static str {
        match self {
            RequestKind::Thread => "thread",
            RequestKind::Brief => "brief",
            RequestKind::Other => "other",
        }
    }

    /// The kind of Clax's own surface named `s` (`thread`, `brief`);
    /// `None` for any other name, `other` included.
    pub fn parse(s: &str) -> Option<RequestKind> {
        RequestKind::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// What a `bot_runs.request` of `s` is: a kind of Clax's surface, or
    /// `other`.
    fn of_column(s: &str) -> Option<RequestKind> {
        match s {
            "other" => Some(RequestKind::Other),
            s => RequestKind::parse(s),
        }
    }
}

named! {
    /// What a step is. `failed` ends a run that could not go on (its step
    /// failed to commit too often).
    StepKind {
        Turn = "turn",
        Script = "script",
        Finish = "finish",
        GiveUp = "give_up",
        Stopped = "stopped",
        Failed = "failed",
    }
}

impl StepKind {
    /// The status a committed step of this kind leaves its run in, for a
    /// terminal step.
    pub fn ends_as(self) -> Option<RunStatus> {
        match self {
            StepKind::Finish => Some(RunStatus::Finished),
            StepKind::GiveUp => Some(RunStatus::GaveUp),
            StepKind::Stopped => Some(RunStatus::Stopped),
            StepKind::Failed => Some(RunStatus::Failed),
            StepKind::Turn | StepKind::Script => None,
        }
    }
}

/// One run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run {
    /// A ULID.
    pub id: String,
    /// The artifact of the run's `clax` binding; empty for a run that has
    /// none ([`Run::artifact`]).
    pub artifact_id: String,
    /// The person key the run acts for.
    pub person: String,
    /// The kind of a `clax` run. A run of another surface's kind is told
    /// apart by [`RunAsMade::kind`], never by this.
    pub request: RequestKind,
    pub thread_ids: Vec<String>,
    pub status: RunStatus,
    /// Why it stopped or gave up (`credits_exhausted`, `run_cap`, …).
    pub reason: Option<String>,
    pub model: String,
    pub key_source: KeySource,
    /// The version its draft is based on; 0 for a run with no artifact.
    pub base_version: u32,
    /// Credits charged so far.
    pub credits_milli: i64,
    /// The driver holding the run, and since when.
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    /// When a driver that let the run wait (a stalled sandbox's backoff,
    /// an unreachable model's retry) is to look at it again; `None`: at
    /// once. A driver that starts honours it ([`Store::set_run_due`]).
    /// For a parked run it is the deadline of what it waits on
    /// ([`Store::park_run`]), `None` for no deadline; no driver claims a
    /// parked run, at that time or any other.
    pub due_at: Option<String>,
    /// The grant of the agent that asked for the run (a person's agent of
    /// another machine, or an agent token), whose rights bound the run's
    /// as much as its person's do; `None` for a person's own send and the
    /// owner's agents.
    pub agent_grant: Option<String>,
    /// The shape of the run's web grant when it was made: `""` (no web
    /// tools), `"search"`, `"fetch"` or `"search,fetch"`. Written when the
    /// run is made and never changed.
    pub web: String,
}

/// What `bot_runs` keeps of a run's parking, beside [`Run`]
/// ([`Store::run_parking`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Parking {
    /// When the run parked; `None` for a run that is not parked.
    pub parked_at: Option<String>,
    /// How long the run has been parked in all, in milliseconds, not
    /// counting a parking that has not ended: what a limit on a run's
    /// time leaves out.
    pub parked_ms: i64,
    /// What the run waits on, as the driver that parked it said; `None`
    /// for a run that is not parked. The store reads none of it.
    pub waiting_on: Option<Value>,
    /// How often the run's open step has parked (0 for a run with no
    /// open step): a step's `attempt` counts the times it was begun,
    /// after a crash too, so a limit on parks counts these.
    pub parks: i64,
}

/// A place in a run's call journal: call `seq` of step `step_n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JournalPlace {
    pub step_n: i64,
    pub seq: i64,
}

/// What [`Store::park_run`] found or did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Park {
    /// The run is parked: by this call, or already (then unchanged).
    Parked(Run),
    /// The place the driver would wait on is in the journal already, so
    /// the run was not parked and nothing changed: the driver, which
    /// still holds it, executes the step again now.
    Answered(Run),
}

/// What a run was made as, which `bot_runs` keeps beside [`Run`]'s
/// columns and never changes: what the run loop knows a run by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAsMade {
    /// The run's kind as `<surface>.<kind>` (`bot_runs.kind`):
    /// `clax.thread`, `clax.brief`, or a kind of another surface.
    pub kind: String,
    /// The run's bindings as `bot_runs.bindings_json` keeps them: read
    /// with [`Bindings::parse`].
    pub bindings_json: String,
    /// The run's request as `bot_runs.request_json` keeps it, the value
    /// its kind's surface gave when the run was made (for `clax`, a
    /// [`ClaxRequest`]).
    pub request_json: String,
}

impl RunAsMade {
    /// The run's bindings, read ([`Bindings::read`]).
    ///
    /// # Errors
    /// [`BindingsError::Unsupported`] for bindings a later build wrote,
    /// which this build cannot drive: whoever drives the run ends it
    /// rather than try again. [`BindingsError::Invalid`] for text that
    /// is not bindings.
    pub fn bindings(&self) -> std::result::Result<Bindings, BindingsError> {
        Bindings::read(&self.bindings_json)
    }
}

impl Run {
    /// The run's artifact, for a run that has one: a run with no `clax`
    /// binding has none (`bot_runs.artifact_id` is NULL).
    #[must_use]
    pub fn artifact(&self) -> Option<&str> {
        Some(self.artifact_id.as_str()).filter(|a| !a.is_empty())
    }
}

/// One step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Step {
    pub run_id: String,
    /// From 1.
    pub n: i64,
    pub kind: StepKind,
    /// When the step was first begun (its later attempts keep it).
    pub started_at: String,
    /// How many times the step was begun: 1, and one more each time it is
    /// begun again after a driver lost it uncommitted. A step's live events
    /// are told apart by it (`<step>.<attempt>:<seq>`).
    pub attempt: i64,
    /// `None` until committed: such a step is executed again on resume.
    pub committed_at: Option<String>,
    /// What the step did (the turn's messages, the script's code and
    /// outcome), once committed.
    pub payload: Option<Value>,
    /// What the model calls of the step's attempts reported they used, as
    /// they streamed ([`Store::observe_step`]): one entry per attempt.
    pub observed: Vec<Value>,
    /// How many times committing the step failed ([`Store::note_commit_failure`]).
    pub failures: i64,
}

/// A run to make.
#[derive(Clone, Debug)]
pub struct NewRun {
    pub artifact_id: String,
    pub person: String,
    pub request: RequestKind,
    pub thread_ids: Vec<String>,
    pub model: String,
    pub key_source: KeySource,
    pub base_version: u32,
    /// The grant of the agent that asked for it ([`Run::agent_grant`]).
    pub agent_grant: Option<String>,
    /// The shape of its web grant ([`Run::web`]).
    pub web: String,
}

/// The surface every run made so far is of: Clax's own, which declares
/// the kinds `thread` and `brief`.
pub const CLAX_SURFACE: &str = "clax";

/// The version of [`Bindings`] a `clax` run is written with
/// ([`Bindings::clax`]): the one every run made by [`Store::create_run`]
/// has.
pub const BINDINGS_VERSION: u32 = 1;

/// The version of [`Bindings`] a run of any other surface's kind is
/// written with ([`Bindings::bound`]).
pub const BINDINGS_VERSION_2: u32 = 2;

/// A run's bindings, as `bot_runs.bindings_json` keeps them: fixed when
/// the run is made. [`Bindings::parse`] (or [`Bindings::read`]) is the
/// only way to read the column; its text is this type's serialisation
/// ([`Bindings::to_json`]).
///
/// `v` is the version that says what the other fields mean. This build
/// reads two and refuses every other ([`BindingsError::Unsupported`]),
/// never reading one as another.
///
/// **Version 1** ([`BINDINGS_VERSION`]) is what a `clax` run has:
///
/// ```json
/// {"v":1,"bindings":[{"surface":"clax","hash":null,
///   "scope":{"artifact":"7q3k9mzx2b4t"},
///   "granted":["web.search","web.fetch"]}]}
/// ```
///
/// One binding for each surface the run has, each with `surface`, `hash`,
/// `scope` and `granted`. It does not hold the operations a run has:
/// `granted` is only what the run was given beyond the operations every
/// run of the surface has ([`StoredBinding::granted`]), which the
/// surface's provider says.
///
/// **Version 2** ([`BINDINGS_VERSION_2`]) is what a run of another
/// surface's kind has:
///
/// ```json
/// {"v":2,"bindings":[{"surface":"shell","hash":"9f86…0a08",
///   "registration":"0123456789abcdef0123456789abcdef",
///   "prompt":"6030…c752",
///   "settings":{"call_timeout_s":30,"runs":{"max_steps":20}},
///   "scope":null,"operations":["shell.windows","shell.ask"]}]}
/// ```
///
/// Each binding has `surface`, `hash`, `scope` and `operations`: every
/// operation the run has of that surface ([`StoredBinding::operations`]),
/// fixed when the run was made, so nothing is asked of the running build.
/// `registration` is the registration that serves the binding
/// ([`crate::bot_surfaces::Registration::id`]); a binding without one
/// (Clax's own surface, bound into such a run) leaves the member out.
/// `prompt` is the SHA-256 of the prompt file the run was started with
/// ([`StoredBinding::prompt`]); a binding started with none, which is
/// sent the standard prompt, leaves the member out. `settings` is the
/// registration's settings in force when the run was started, an object
/// the surface's provider reads ([`StoredBinding::settings`]); a binding
/// written before settings were kept with the run leaves the member out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bindings {
    v: u32,
    bindings: Vec<StoredBinding>,
}

/// One surface a run has, as [`Bindings`] keeps it: read only as part of
/// its [`Bindings`], whose version says what its fields mean.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredBinding {
    surface: String,
    hash: Option<String>,
    registration: Option<String>,
    /// Version 2's `prompt`; `None` in version 1.
    prompt: Option<String>,
    /// Version 2's `settings`; `None` in version 1.
    settings: Option<Value>,
    scope: Value,
    /// Version 1's `granted`; empty in version 2.
    granted: Vec<String>,
    /// Version 2's `operations`; `None` in version 1.
    operations: Option<Vec<String>>,
}

/// A stored binding is written with its version's members, in a fixed
/// order: `surface`, `hash`, `scope`, `granted` for version 1 (the bytes
/// a `clax` run has always had), and `surface`, `hash`, `registration`
/// when it has one, `prompt` when it has one, `settings` when it has
/// them, `scope`, `operations` for version 2.
impl Serialize for StoredBinding {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("surface", &self.surface)?;
        m.serialize_entry("hash", &self.hash)?;
        match &self.operations {
            None => {
                m.serialize_entry("scope", &self.scope)?;
                m.serialize_entry("granted", &self.granted)?;
            }
            Some(operations) => {
                if let Some(registration) = &self.registration {
                    m.serialize_entry("registration", registration)?;
                }
                if let Some(prompt) = &self.prompt {
                    m.serialize_entry("prompt", prompt)?;
                }
                if let Some(settings) = &self.settings {
                    m.serialize_entry("settings", settings)?;
                }
                m.serialize_entry("scope", &self.scope)?;
                m.serialize_entry("operations", operations)?;
            }
        }
        m.end()
    }
}

impl StoredBinding {
    /// A binding of version 2: surface `surface` as its definition
    /// `hash` has it (`None` for a surface whose definition is not kept
    /// by hash, as Clax's own), served by `registration` (`None` for
    /// Clax's own), started with the prompt file whose SHA-256 is
    /// `prompt` (`None` for a run that is sent the standard prompt), on
    /// `scope`, with `operations`, each named as the surface's
    /// definition names it.
    #[must_use]
    pub fn bound(
        surface: impl Into<String>,
        hash: Option<String>,
        registration: Option<String>,
        prompt: Option<String>,
        scope: Value,
        operations: Vec<String>,
    ) -> StoredBinding {
        StoredBinding {
            surface: surface.into(),
            hash,
            registration,
            prompt,
            settings: None,
            scope,
            granted: Vec::new(),
            operations: Some(operations),
        }
    }

    /// This binding of version 2, started with `settings`: its
    /// registration's settings in force when the run was started, as an
    /// object the surface's provider reads ([`StoredBinding::settings`]).
    #[must_use]
    pub fn with_settings(self, settings: Value) -> StoredBinding {
        StoredBinding {
            settings: Some(settings),
            ..self
        }
    }

    /// The surface's name: [`CLAX_SURFACE`] for Clax's own.
    #[must_use]
    pub fn surface(&self) -> &str {
        &self.surface
    }

    /// The SHA-256 of the surface's definition as the run was made with
    /// it, in lowercase hex; `None` (`null`) for a binding made before
    /// definitions were kept by hash, which every binding of version 1
    /// is.
    #[must_use]
    pub fn hash(&self) -> Option<&str> {
        self.hash.as_deref()
    }

    /// The registration that serves the binding
    /// ([`crate::bot_surfaces::Registration::id`]): `None` for a binding
    /// of version 1, and for one of version 2 that names none (Clax's
    /// own surface).
    #[must_use]
    pub fn registration(&self) -> Option<&str> {
        self.registration.as_deref()
    }

    /// The SHA-256, in lowercase hex, of the prompt file the run was
    /// started with: the one its registration had then, whose text is
    /// kept under it ([`Store::surface_prompt`]) for as long as the run
    /// is. `None` for a binding of version 1, and for one of version 2
    /// started with no prompt file, whose run is sent the standard
    /// prompt.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    /// The settings the run was started with: its registration's, in
    /// force then, kept so that nothing its host registers later and no
    /// restart changes them for the run. The store does not read them:
    /// they are the surface's provider's (for a host's surface, the
    /// registration's settings as the host protocol names them, less
    /// `allow`, which the binding's operations already hold). `None` for
    /// a binding of version 1 and for one of version 2 written before
    /// settings were kept with the run, whose provider says what stands
    /// in their place.
    #[must_use]
    pub fn settings(&self) -> Option<&Value> {
        self.settings.as_ref()
    }

    /// The provider's own value for what the run is bound to: for `clax`,
    /// `{"artifact": "<artifact ID>"}`.
    #[must_use]
    pub fn scope(&self) -> &Value {
        &self.scope
    }

    /// For a binding of version 1: the operations granted to this run
    /// beyond those every run of the surface has, as the surface names
    /// them, without the surface's name. For `clax` these are the web
    /// operations the run was made with (`web.search`, `web.fetch`, in
    /// that order: what [`Run::web`] records). An empty list is a run
    /// with the surface's standing operations and no more, not a run
    /// with none. Empty for a binding of version 2, which says its
    /// operations whole ([`StoredBinding::operations`]).
    #[must_use]
    pub fn granted(&self) -> &[String] {
        &self.granted
    }

    /// For a binding of version 2: every operation the run has of the
    /// surface, as it was made; an empty list is a run with none. `None`
    /// for a binding of version 1, which does not hold them: its
    /// operations are the surface's standing ones, by its provider's
    /// rule, and [`StoredBinding::granted`].
    #[must_use]
    pub fn operations(&self) -> Option<&[String]> {
        self.operations.as_deref()
    }
}

/// Why a run's bindings could not be read ([`Bindings::read`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingsError {
    /// The bindings are of a version this build does not read: a later
    /// build wrote them. The run cannot be driven by this build, and no
    /// retry changes that.
    Unsupported {
        /// The version they say.
        version: u64,
    },
    /// The text is not bindings of the version it says: what is wrong
    /// with it.
    Invalid(String),
}

impl std::fmt::Display for BindingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingsError::Unsupported { version } => write!(
                f,
                "bindings of version {version}; this build reads versions {BINDINGS_VERSION} \
                 and {BINDINGS_VERSION_2}"
            ),
            BindingsError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for BindingsError {}

/// `bindings_invalid` for either case, with its message: what
/// [`Bindings::parse`] answers.
impl From<BindingsError> for CoreError {
    fn from(e: BindingsError) -> CoreError {
        CoreError::invalid("bindings_invalid", e.to_string())
    }
}

/// The names `value` lists, when it is a list of text.
fn names(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|o| o.as_str().map(str::to_string))
        .collect()
}

impl Bindings {
    /// The bindings of a `clax` run on artifact `artifact_id` whose web
    /// grant has the shape `web` ([`Run::web`]: `search` and `fetch`,
    /// comma-separated; any other part names no operation): version 1.
    #[must_use]
    pub fn clax(artifact_id: &str, web: &str) -> Bindings {
        let has = |part: &str| web.split(',').any(|p| p == part);
        let granted = [("search", "web.search"), ("fetch", "web.fetch")]
            .into_iter()
            .filter(|(part, _)| has(part))
            .map(|(_, op)| op.to_string())
            .collect();
        Bindings {
            v: BINDINGS_VERSION,
            bindings: vec![StoredBinding {
                surface: CLAX_SURFACE.to_string(),
                hash: None,
                registration: None,
                prompt: None,
                settings: None,
                scope: serde_json::json!({ "artifact": artifact_id }),
                granted,
                operations: None,
            }],
        }
    }

    /// Bindings of version 2 holding `bindings` ([`StoredBinding::bound`])
    /// in that order.
    ///
    /// # Errors
    /// [`BindingsError::Invalid`] for no binding, a binding with an empty
    /// surface name, two bindings of one surface, or an operation named
    /// twice in one binding: what [`Bindings::read`] refuses too.
    pub fn bound(bindings: Vec<StoredBinding>) -> std::result::Result<Bindings, BindingsError> {
        let made = Bindings {
            v: BINDINGS_VERSION_2,
            bindings,
        };
        Bindings::read(&made.to_json())
    }

    /// The version these bindings are of: [`BINDINGS_VERSION`] or
    /// [`BINDINGS_VERSION_2`].
    #[must_use]
    pub fn version(&self) -> u32 {
        self.v
    }

    /// [`Bindings::read`], its error as `bindings_invalid`.
    ///
    /// # Errors
    /// `bindings_invalid`, with [`BindingsError`]'s message.
    pub fn parse(json: &str) -> Result<Bindings> {
        Ok(Bindings::read(json)?)
    }

    /// The bindings `json` holds (a `bot_runs.bindings_json`), of
    /// version 1 or 2. A field this build does not know is ignored, and
    /// a `scope` is the surface's own value, taken whatever it is
    /// (`null` included). A key the text repeats is read as its last
    /// value, as `serde_json` reads any object.
    ///
    /// # Errors
    /// [`BindingsError::Unsupported`] for a `v` that is a whole number
    /// other than 1 and 2. [`BindingsError::Invalid`], with a message
    /// that says which, for:
    /// - text that is not JSON, or is JSON other than an object;
    /// - no `v`, or a `v` that is not a whole number;
    /// - no `bindings`, `bindings` that is not a list, or an empty one;
    /// - a binding that is not an object or lacks one of `surface`,
    ///   `hash`, `scope` and its version's list of operations (`granted`
    ///   in version 1, `operations` in version 2; a `hash` of `null` is
    ///   present);
    /// - in version 2, `settings` that are neither an object nor `null`;
    /// - a `surface` that is not text or is empty, a `hash` that is
    ///   neither text nor `null`, in version 2 a `registration` or a
    ///   `prompt` that is neither text nor `null`, a list of operations that is not a
    ///   list of names, or one that names an operation twice;
    /// - two bindings of one surface.
    pub fn read(json: &str) -> std::result::Result<Bindings, BindingsError> {
        let bad = BindingsError::Invalid;
        let value: Value =
            serde_json::from_str(json).map_err(|e| bad(format!("bindings are not JSON: {e}")))?;
        let Some(object) = value.as_object() else {
            return Err(bad(format!("bindings are not a JSON object: {value}")));
        };
        let version = match object.get("v") {
            None => return Err(bad("bindings without a version (`v`)".into())),
            Some(v) => match v.as_u64() {
                Some(n) if n == u64::from(BINDINGS_VERSION) => BINDINGS_VERSION,
                Some(n) if n == u64::from(BINDINGS_VERSION_2) => BINDINGS_VERSION_2,
                Some(n) => return Err(BindingsError::Unsupported { version: n }),
                None => {
                    return Err(bad(format!(
                        "bindings whose version (`v`) is {v}, which is not a whole number"
                    )));
                }
            },
        };
        let list = match object.get("bindings") {
            None => return Err(bad("bindings without `bindings`".into())),
            Some(Value::Array(list)) => list,
            Some(other) => return Err(bad(format!("`bindings` is not a list: {other}"))),
        };
        if list.is_empty() {
            return Err(bad("a run has at least one binding".into()));
        }
        let mut bindings: Vec<StoredBinding> = Vec::with_capacity(list.len());
        for (i, item) in list.iter().enumerate() {
            let Some(fields) = item.as_object() else {
                return Err(bad(format!("binding {i} is not a JSON object: {item}")));
            };
            let field = |name: &str| {
                fields
                    .get(name)
                    .ok_or_else(|| bad(format!("binding {i} has no `{name}`")))
            };
            let surface = match field("surface")? {
                Value::String(s) if s.is_empty() => {
                    return Err(bad(format!("binding {i} has an empty surface name")));
                }
                Value::String(s) => s.clone(),
                other => {
                    return Err(bad(format!(
                        "binding {i} has a `surface` that is not text: {other}"
                    )));
                }
            };
            let text_or_null = |name: &str, value: &Value| match value {
                Value::Null => Ok(None),
                Value::String(h) => Ok(Some(h.clone())),
                other => Err(bad(format!(
                    "the binding of surface {surface} has a `{name}` that is neither text \
                     nor null: {other}"
                ))),
            };
            let hash = text_or_null("hash", field("hash")?)?;
            let scope = field("scope")?.clone();
            let listed = if version == BINDINGS_VERSION {
                "granted"
            } else {
                "operations"
            };
            let listed_value = field(listed)?;
            let ops = names(listed_value).ok_or_else(|| {
                bad(format!(
                    "the binding of surface {surface} has a `{listed}` that is not a list \
                     of names: {listed_value}"
                ))
            })?;
            for (n, op) in ops.iter().enumerate() {
                if ops[..n].contains(op) {
                    return Err(bad(if version == BINDINGS_VERSION {
                        format!("the binding of surface {surface} is granted {op} twice")
                    } else {
                        format!("the binding of surface {surface} names {op} twice")
                    }));
                }
            }
            if bindings.iter().any(|o| o.surface == surface) {
                return Err(bad(format!("two bindings of surface {surface}")));
            }
            bindings.push(if version == BINDINGS_VERSION {
                StoredBinding {
                    surface,
                    hash,
                    registration: None,
                    prompt: None,
                    settings: None,
                    scope,
                    granted: ops,
                    operations: None,
                }
            } else {
                let registration = match fields.get("registration") {
                    None => None,
                    Some(value) => text_or_null("registration", value)?,
                };
                let prompt = match fields.get("prompt") {
                    None => None,
                    Some(value) => text_or_null("prompt", value)?,
                };
                let settings = match fields.get("settings") {
                    None | Some(Value::Null) => None,
                    Some(value @ Value::Object(_)) => Some(value.clone()),
                    Some(other) => {
                        return Err(bad(format!(
                            "the binding of surface {surface} has `settings` that are neither \
                             an object nor null: {other}"
                        )));
                    }
                };
                StoredBinding {
                    surface,
                    hash,
                    registration,
                    prompt,
                    settings,
                    scope,
                    granted: Vec::new(),
                    operations: Some(ops),
                }
            });
        }
        Ok(Bindings {
            v: version,
            bindings,
        })
    }

    /// The text `bot_runs.bindings_json` holds for these bindings.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("serialisable bindings")
    }

    /// Each surface the run has, in the order they were bound.
    #[must_use]
    pub fn all(&self) -> &[StoredBinding] {
        &self.bindings
    }

    /// The run's binding of `surface`, when it has one.
    #[must_use]
    pub fn of(&self, surface: &str) -> Option<&StoredBinding> {
        self.bindings.iter().find(|b| b.surface == surface)
    }
}

/// The request of a `clax` run (`clax.thread` or `clax.brief`), as
/// `bot_runs.request_json` keeps it: the IDs the `clax` surface was given,
/// which it resolves to their contents at the run's first turn, not when
/// the run is made.
///
/// ```json
/// {"thread_ids":["01ARZ3NDEKTSV4RRFFQ69G5FAV"]}
/// ```
///
/// A `clax.thread` run's are the threads sent; a `clax.brief` run's is the
/// one thread whose first comment is the brief. The value is the
/// surface's own, so it carries no version: `bot_runs.kind` names the
/// surface and kind that say how to read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ClaxRequest {
    pub thread_ids: Vec<String>,
}

/// The refusal of [`Store::create_run_of_within`] for a registration
/// with as many runs that have not ended as it may have.
pub const CODE_TOO_MANY_RUNS: &str = "too_many_runs";

/// A run of a kind of a surface other than Clax's, to make
/// ([`Store::create_run_of`]): it has no artifact, no base version, no
/// threads and no web grant of its own.
#[derive(Clone, Debug)]
pub struct NewRunOf {
    /// The run's kind as `<surface>.<kind>` (`shell.triage`); the
    /// surface is not `clax`.
    pub kind: String,
    /// The request as the kind's surface gave it.
    pub request: Value,
    /// The run's bindings, of version 2 ([`Bindings::bound`]), the
    /// kind's surface among them.
    pub bindings: Bindings,
    /// The person key the run acts for.
    pub person: String,
    pub model: String,
    pub key_source: KeySource,
    /// The registration that started the run
    /// ([`crate::bot_surfaces::Registration::id`]).
    pub started_by: String,
    /// The run that started this one, for a child run.
    pub parent_run: Option<String>,
    /// The grant of the agent that asked for it ([`Run::agent_grant`]).
    pub agent_grant: Option<String>,
}

impl NewRun {
    /// The run's kind as `<surface>.<kind>` (`bot_runs.kind`):
    /// `clax.thread` or `clax.brief`.
    #[must_use]
    pub fn kind(&self) -> String {
        format!("{CLAX_SURFACE}.{}", self.request.as_str())
    }

    /// The run's bindings (`bot_runs.bindings_json`): the `clax` surface
    /// on its artifact.
    #[must_use]
    pub fn bindings(&self) -> Bindings {
        Bindings::clax(&self.artifact_id, &self.web)
    }

    /// The run's request (`bot_runs.request_json`).
    #[must_use]
    pub fn clax_request(&self) -> ClaxRequest {
        ClaxRequest {
            thread_ids: self.thread_ids.clone(),
        }
    }
}

/// A change to a run's draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftChange {
    Write {
        path: String,
        content: Vec<u8>,
        content_type: Option<String>,
    },
    Delete {
        path: String,
    },
}

/// One path of a draft's overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftFile {
    pub path: String,
    /// `None`: the draft deleted the path.
    pub content: Option<Vec<u8>>,
    pub content_type: Option<String>,
}

/// What a step's commit writes.
#[derive(Clone, Debug, Default)]
pub struct StepCommit {
    pub payload: Value,
    /// Metered usage, charged to the run's person.
    pub usage: Vec<Metered>,
    /// The step's audit events (`bot.turn`, `bot.script`, `bot.call`, …):
    /// the store sets their artifact, `run` and `step`.
    pub events: Vec<AuditRecord>,
    pub draft: Vec<DraftChange>,
    /// Why a terminal step ended the run.
    pub reason: Option<String>,
    /// The `seq` of each message of the run's inbox the step gave its
    /// model, marked delivered by the commit ([`crate::bot_messages`]).
    pub delivered: Vec<i64>,
}

/// What a step's commit wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Committed {
    pub step: Step,
    pub usage: Vec<UsageRow>,
    /// The step's charge.
    pub credits_milli: i64,
    /// The run after.
    pub run: Run,
}

const RUN_COLS: &str = "id, artifact_id, person, request, thread_ids_json, status, reason, model,
    key_source, base_version, credits_milli, claimed_by, claimed_at, created_at, updated_at, ended_at,
    agent_grant, due_at, web";

/// Column `i` of a `bot_runs` row, `column`, as the `T` its text names.
/// A value `parse` does not know is an error naming the column and the
/// value, never another `T`: a status or a request kind a later build
/// wrote is not one of this build's.
fn known<T>(
    r: &Row<'_>,
    i: usize,
    column: &str,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<T> {
    let text: String = r.get(i)?;
    parse(&text).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            format!("bot_runs.{column} is {text:?}, which this build does not know").into(),
        )
    })
}

/// Column `i` of a row, `column` (`table.column`), stored JSON read as a
/// `T`. Text that does not read as one is a conversion failure naming the
/// column, never a default: a damaged row is refused, not read as some
/// other value.
pub(crate) fn stored_json<T: serde::de::DeserializeOwned>(
    r: &Row<'_>,
    i: usize,
    column: &str,
) -> rusqlite::Result<T> {
    let text: String = r.get(i)?;
    parse_stored(i, column, &text)
}

/// [`stored_json`] for a column that may be `NULL`, which is `None`.
pub(crate) fn stored_json_or_null<T: serde::de::DeserializeOwned>(
    r: &Row<'_>,
    i: usize,
    column: &str,
) -> rusqlite::Result<Option<T>> {
    r.get::<_, Option<String>>(i)?
        .map(|text| parse_stored(i, column, &text))
        .transpose()
}

fn parse_stored<T: serde::de::DeserializeOwned>(
    i: usize,
    column: &str,
    text: &str,
) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            format!("{column} does not read: {e}").into(),
        )
    })
}

/// A `bot_runs` row ([`RUN_COLS`]) as a [`Run`].
///
/// # Errors
/// A conversion failure for a `status` or a `request` this build does not
/// know ([`known`]). A row without an artifact or a base version is read
/// ([`Run::artifact`]); the `request` of a run of another surface's kind
/// is [`RequestKind::Other`].
fn row_to_run(r: &Row<'_>) -> rusqlite::Result<Run> {
    let s = |i: usize| r.get::<_, String>(i);
    Ok(Run {
        id: s(0)?,
        artifact_id: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        person: s(2)?,
        request: known(r, 3, "request", RequestKind::of_column)?,
        thread_ids: serde_json::from_str(&s(4)?).unwrap_or_default(),
        status: known(r, 5, "status", RunStatus::parse)?,
        reason: r.get(6)?,
        model: s(7)?,
        key_source: KeySource::parse(&s(8)?).unwrap_or(KeySource::Instance),
        base_version: r.get::<_, Option<u32>>(9)?.unwrap_or_default(),
        credits_milli: r.get(10)?,
        claimed_by: r.get(11)?,
        claimed_at: r.get(12)?,
        created_at: s(13)?,
        updated_at: s(14)?,
        ended_at: r.get(15)?,
        agent_grant: r.get(16)?,
        due_at: r.get(17)?,
        web: s(18)?,
    })
}

/// What `run` was made as, from its row's `kind`, `bindings_json` and
/// `request_json`. A run made before migration 64 holds NULL in each
/// (the migration adds the columns without writing a row), and is read as
/// made with what [`Store::create_run`] writes for a run of its columns
/// ([`NewRun::kind`], [`NewRun::bindings`], [`NewRun::clax_request`]).
fn as_made(
    run: &Run,
    kind: Option<String>,
    bindings_json: Option<String>,
    request_json: Option<String>,
) -> RunAsMade {
    let before_64 = || NewRun {
        artifact_id: run.artifact_id.clone(),
        person: run.person.clone(),
        request: run.request,
        thread_ids: run.thread_ids.clone(),
        model: run.model.clone(),
        key_source: run.key_source,
        base_version: run.base_version,
        agent_grant: run.agent_grant.clone(),
        web: run.web.clone(),
    };
    RunAsMade {
        kind: kind.unwrap_or_else(|| before_64().kind()),
        bindings_json: bindings_json.unwrap_or_else(|| before_64().bindings().to_json()),
        request_json: request_json.unwrap_or_else(|| {
            serde_json::to_string(&before_64().clax_request()).expect("serialisable IDs")
        }),
    }
}

/// Files `rec` on `subject` ([`Store::commit_bot_step_on`]).
fn file_on(rec: &mut AuditRecord, subject: &AuditIds) {
    let AuditIds {
        artifact,
        artifact2,
        thread,
        session,
        question,
        call,
        origin,
    } = subject;
    rec.ids.artifact = artifact.clone();
    for (own, given) in [
        (&mut rec.ids.artifact2, artifact2),
        (&mut rec.ids.thread, thread),
        (&mut rec.ids.session, session),
        (&mut rec.ids.question, question),
        (&mut rec.ids.call, call),
        (&mut rec.ids.origin, origin),
    ] {
        if own.is_none() {
            own.clone_from(given);
        }
    }
}

/// A `bot_steps` row ([`STEP_COLS`]) as a [`Step`].
///
/// # Errors
/// A conversion failure for a payload or observations that do not read
/// as JSON ([`stored_json`]).
fn row_to_step(r: &Row<'_>) -> rusqlite::Result<Step> {
    let kind: String = r.get(2)?;
    Ok(Step {
        run_id: r.get(0)?,
        n: r.get(1)?,
        kind: StepKind::parse(&kind).unwrap_or(StepKind::Stopped),
        started_at: r.get(3)?,
        attempt: r.get(6)?,
        committed_at: r.get(4)?,
        payload: stored_json_or_null(r, 5, "bot_steps.payload_json")?,
        observed: stored_json_or_null(r, 7, "bot_steps.observed_json")?.unwrap_or_default(),
        failures: r.get(8)?,
    })
}

const STEP_COLS: &str =
    "run_id, n, kind, started_at, committed_at, payload_json, attempt, observed_json, failures";

pub(crate) fn run_in(c: &Connection, id: &str) -> Result<Option<Run>> {
    Ok(
        c.prepare_cached(&format!("SELECT {RUN_COLS} FROM bot_runs WHERE id = ?1"))?
            .query_row(params![id], row_to_run)
            .optional()?,
    )
}

/// [`Store::active_run_on`] on `c`.
fn active_on_in(c: &Connection, threads: &[String]) -> Result<Option<Run>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {RUN_COLS} FROM bot_runs
          WHERE status IN {LIVE}
            AND EXISTS (SELECT 1 FROM json_each(thread_ids_json) j
                         WHERE j.value IN (SELECT value FROM json_each(?1)))
          ORDER BY created_at DESC, id DESC LIMIT 1"
    ))?;
    let ids = serde_json::to_string(threads).expect("serialisable IDs");
    Ok(q.query_row(params![ids], row_to_run).optional()?)
}

/// Whether `e` is a stored value this build does not read: a row a
/// later build wrote, or a damaged one ([`stored_json`] and the other
/// strict readers' conversion failures). Such an error is the row's, and
/// fails what that row is part of alone: its call, its run, or its place
/// in a listing, never a listing or another run.
#[must_use]
pub fn is_unreadable(e: &CoreError) -> bool {
    matches!(e, CoreError::Db(e) if unreadable_db(e))
}

/// [`is_unreadable`] of a database error.
pub(crate) fn unreadable_db(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
    )
}

/// The open step of run `id` on `c`. One whose payload or observations
/// do not read ([`row_to_step`]) is read without them (no payload, no
/// observations), so that the step a run whose history does not read
/// ends with can still be begun and committed: what is written over it
/// is the run's terminal step.
pub(crate) fn open_step_in(c: &Connection, id: &str) -> Result<Option<Step>> {
    let strict = c
        .prepare_cached(&format!(
            "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 AND committed_at IS NULL"
        ))?
        .query_row(params![id], row_to_step)
        .optional();
    match strict {
        Ok(step) => Ok(step),
        Err(e) if unreadable_db(&e) => {
            tracing::warn!(
                "clax-store: the open step of run {id} does not read ({e}): read without its \
                 payload and observations"
            );
            Ok(c.prepare_cached(
                "SELECT run_id, n, kind, started_at, attempt, failures FROM bot_steps
                  WHERE run_id = ?1 AND committed_at IS NULL",
            )?
            .query_row(params![id], |r| {
                let kind: String = r.get(2)?;
                Ok(Step {
                    run_id: r.get(0)?,
                    n: r.get(1)?,
                    kind: StepKind::parse(&kind).unwrap_or(StepKind::Stopped),
                    started_at: r.get(3)?,
                    attempt: r.get(4)?,
                    committed_at: None,
                    payload: None,
                    observed: Vec::new(),
                    failures: r.get(5)?,
                })
            })
            .optional()?)
        }
        Err(e) => Err(e.into()),
    }
}

fn runs_where(
    c: &Connection,
    filter: &str,
    arg: &dyn rusqlite::ToSql,
    limit: u32,
) -> Result<Vec<Run>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {RUN_COLS} FROM bot_runs WHERE {filter} ORDER BY created_at DESC, id DESC LIMIT ?2"
    ))?;
    let rows = q.query_map(params![arg, limit], row_to_run)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `run_invalid` with `message`.
fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid("run_invalid", message)
}

/// Who drives runs: one per process natively (minted when the daemon
/// starts) and one per Durable Object incarnation on the Worker (minted
/// when the instance object is constructed), never shared or reused, so a
/// claim names exactly one live executor and a restarted one is another
/// driver ([`Store::claim_run`] takes a run over from it only once its
/// lease lapses). [`DriverId::mint`] is the only way to make one outside
/// the store's own tests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DriverId(String);

impl DriverId {
    /// A new driver ID, `drv_` and a ULID: never one minted before by this
    /// process (debug builds assert it).
    pub fn mint() -> DriverId {
        let id = DriverId(format!("drv_{}", clax_core::new_ulid()));
        #[cfg(debug_assertions)]
        {
            static MINTED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
                std::sync::Mutex::new(None);
            let fresh = MINTED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(Default::default)
                .insert(id.0.clone());
            debug_assert!(fresh, "driver ID {} minted twice", id.0);
        }
        id
    }

    /// A driver of a fixed name, for the store's tests.
    #[cfg(test)]
    pub(crate) fn named(name: &str) -> DriverId {
        DriverId(name.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DriverId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `run_not_claimed` unless `driver` holds run `run`'s claim.
pub(crate) fn check_claim(run: &Run, driver: &str) -> Result<()> {
    if run.claimed_by.as_deref() == Some(driver) {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "run_not_claimed",
            format!(
                "driver {driver} does not hold run {}; claim it again",
                run.id
            ),
        ))
    }
}

/// Ends run `id`'s parking at `at` on `c` ([`Store::unpark_run`]);
/// whether it was parked.
pub(crate) fn unpark_in(c: &Connection, id: &str, at: &str) -> Result<bool> {
    let parked_at: Option<Option<String>> = c
        .prepare_cached("SELECT parked_at FROM bot_runs WHERE id = ?1 AND status = 'parked'")?
        .query_row(params![id], |r| r.get(0))
        .optional()?;
    let Some(parked_at) = parked_at else {
        return Ok(false);
    };
    let time = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok();
    let waited = parked_at
        .as_deref()
        .and_then(time)
        .zip(time(at))
        .map_or(0, |(from, to)| (to - from).num_milliseconds().max(0));
    c.execute(
        "UPDATE bot_runs SET status = 'queued', parked_ms = parked_ms + ?2, parked_at = NULL,
           waiting_on = NULL, due_at = NULL, updated_at = ?3
         WHERE id = ?1",
        params![id, waited, at],
    )?;
    Ok(true)
}

/// The step a driver of run `id` works on next on `c`: its open step, else
/// the one after the last.
fn next_n_in(c: &Connection, id: &str) -> Result<i64> {
    Ok(match open_step_in(c, id)? {
        Some(open) => open.n,
        None => c.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM bot_steps WHERE run_id = ?1",
            params![id],
            |r| r.get(0),
        )?,
    })
}

/// `step_mismatch` naming the step run `id` is at, unless it is `n`.
fn check_step(c: &Connection, id: &str, n: i64) -> Result<()> {
    let at = next_n_in(c, id)?;
    if at == n {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "step_mismatch",
            format!("run {id} is at step {at}, not {n}; read the run again"),
        ))
    }
}

impl Store {
    /// Makes a queued run, recording `bot.run_start {run, request,
    /// thread_ids, model, key_source}` on its artifact under `ctx`.
    ///
    /// # Errors
    /// `NotFound` for an artifact that does not exist; `model_not_priced`
    /// for a model whose tokens resolve to no rate ([`Store::model_rate`]:
    /// with `require_explicit_price`, one without its token cost lines,
    /// which an admin sets with [`Store::set_model_rates`]); the store's.
    pub fn create_run(&self, ctx: &AuditCtx, new: NewRun) -> Result<Run> {
        self.with_tx(|tx| self.create_run_in(tx, ctx, new, None))
    }

    /// [`Store::create_run`] unless a run that has not ended (queued,
    /// running, stopping or parked) names one of `new`'s threads: then that run,
    /// and nothing is made, in the same transaction, so two sends of one
    /// thread never make two runs.
    ///
    /// # Errors
    /// [`Store::create_run`]'s.
    pub fn create_run_exclusive(
        &self,
        ctx: &AuditCtx,
        new: NewRun,
    ) -> Result<std::result::Result<Run, Run>> {
        self.with_tx(|tx| {
            if let Some(active) = active_on_in(tx, &new.thread_ids)? {
                return Ok(Err(active));
            }
            Ok(Ok(self.create_run_in(tx, ctx, new, None)?))
        })
    }

    /// [`Store::create_run_exclusive`], the run made with `bindings` in
    /// place of [`NewRun::bindings`]: a run of Clax's own kind that other
    /// surfaces were bound into when it was made (`crate::bot_joins`).
    /// The bindings are of version 2 ([`Bindings::bound`]) and hold
    /// `clax`'s on the run's artifact, `{"artifact": "<ID>"}`; its other
    /// columns are a `clax` run's, as [`Store::create_run`] writes them.
    ///
    /// # Errors
    /// `run_invalid` for bindings of another version or without that
    /// `clax` binding; [`Store::create_run`]'s.
    pub fn create_run_exclusive_bound(
        &self,
        ctx: &AuditCtx,
        new: NewRun,
        bindings: &Bindings,
    ) -> Result<std::result::Result<Run, Run>> {
        let scoped = bindings
            .of(CLAX_SURFACE)
            .is_some_and(|b| b.scope()["artifact"].as_str() == Some(new.artifact_id.as_str()));
        if bindings.version() != BINDINGS_VERSION_2 || !scoped {
            return Err(invalid(format!(
                "a run of {} with other surfaces is made with bindings of version \
                 {BINDINGS_VERSION_2} holding {CLAX_SURFACE}'s on its artifact",
                new.kind()
            )));
        }
        self.with_tx(|tx| {
            if let Some(active) = active_on_in(tx, &new.thread_ids)? {
                return Ok(Err(active));
            }
            Ok(Ok(self.create_run_in(tx, ctx, new, Some(bindings))?))
        })
    }

    /// Queues `person` (a person key) for a follow-up run of thread `tid`
    /// once the run working on it ends, behind those queued before (a
    /// person already queued for it keeps their place), in the
    /// transaction that finds that run has not ended: so the run's end,
    /// committed after, always finds them. Returns that run, or `None`,
    /// queueing nothing, when no run works on `tid`.
    ///
    /// # Errors
    /// The store's.
    pub fn queue_follow_up_if_active(&self, tid: &str, person: &str) -> Result<Option<Run>> {
        self.with_tx(|tx| {
            let Some(run) = active_on_in(tx, std::slice::from_ref(&tid.to_string()))? else {
                return Ok(None);
            };
            tx.execute(
                "INSERT OR IGNORE INTO bot_follow_ups (thread_id, person, queued_at)
                 VALUES (?1, ?2, ?3)",
                params![tid, person, self.now()],
            )?;
            Ok(Some(run))
        })
    }

    /// Drops every follow-up queued on `tid` (a thread that can no longer
    /// be sent: deleted, resolved, or of a deleted artifact).
    ///
    /// # Errors
    /// The store's.
    pub fn drop_follow_ups(&self, tid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM bot_follow_ups WHERE thread_id = ?1",
                params![tid],
            )?;
            Ok(())
        })
    }

    /// The threads with a follow-up queued and no run working on them (a
    /// run ended without starting its follow-ups, as a crash after its
    /// last step leaves it), oldest queued first.
    ///
    /// # Errors
    /// The store's.
    pub fn waiting_follow_up_threads(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT thread_id FROM bot_follow_ups GROUP BY thread_id ORDER BY MIN(seq)",
            )?;
            let tids = q
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            let mut out = Vec::new();
            for tid in tids {
                if active_on_in(c, std::slice::from_ref(&tid))?.is_none() {
                    out.push(tid);
                }
            }
            Ok(out)
        })
    }

    /// The first person queued for a follow-up of `tid`
    /// ([`Store::queue_follow_up_if_active`]) as (its place, the person),
    /// left on the queue until [`Store::remove_follow_up`]: a run's end
    /// that finds the thread taken leaves them for that run's end.
    ///
    /// # Errors
    /// The store's.
    pub fn next_follow_up(&self, tid: &str) -> Result<Option<(i64, String)>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT seq, person FROM bot_follow_ups WHERE thread_id = ?1
                 ORDER BY seq LIMIT 1",
            )?
            .query_row(params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
        })
    }

    /// Takes the follow-up at place `seq` ([`Store::next_follow_up`]) off
    /// its queue.
    ///
    /// # Errors
    /// The store's.
    pub fn remove_follow_up(&self, seq: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM bot_follow_ups WHERE seq = ?1", params![seq])?;
            Ok(())
        })
    }

    /// The people queued for a follow-up of `tid`, first first.
    ///
    /// # Errors
    /// The store's.
    pub fn follow_ups(&self, tid: &str) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT person FROM bot_follow_ups WHERE thread_id = ?1 ORDER BY seq",
            )?;
            let v = q
                .query_map(params![tid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(v)
        })
    }

    /// The newest run that has not ended naming one of `threads`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn active_run_on(&self, threads: &[String]) -> Result<Option<Run>> {
        self.with_read(|c| active_on_in(c, threads))
    }

    /// Makes a queued run of a kind of a surface other than Clax's,
    /// recording `bot.run_start {run, request: "other", kind,
    /// thread_ids: [], model, key_source, started_by, parent_run?}` under
    /// `ctx` with no artifact: the record is found by the person it was
    /// made for. The row has no artifact and no base version
    /// ([`Run::artifact`] is `None`), no threads, `request` `other`
    /// ([`RequestKind::Other`]), and `new`'s kind, bindings, request,
    /// `started_by` and `parent_run`.
    ///
    /// Such a run names no thread, so the rule that one run works a
    /// thread at a time ([`Store::create_run_exclusive`]) does not bear
    /// on it. What [`Store::create_run`] holds every run to, it is held
    /// to: its model's tokens resolve to a rate.
    ///
    /// # Errors
    /// `run_invalid` for a kind that is not `<surface>.<kind>` or is of
    /// `clax`, for bindings that are not of version 2 or lack the kind's
    /// surface, or for an empty person; `NotFound` for a `started_by`
    /// that is no registration or a `parent_run` that is no run;
    /// `model_not_priced` ([`Store::create_run`]); the store's.
    pub fn create_run_of(&self, ctx: &AuditCtx, new: NewRunOf) -> Result<Run> {
        let surface = match new.kind.split_once('.') {
            Some((surface, kind)) if !surface.is_empty() && !kind.is_empty() => surface,
            _ => {
                return Err(invalid(format!(
                    "a run's kind is `<surface>.<kind>`, not {:?}",
                    new.kind
                )));
            }
        };
        if surface == CLAX_SURFACE {
            return Err(invalid(format!(
                "{} is a kind of Clax's own surface, whose runs are made on an artifact",
                new.kind
            )));
        }
        if new.bindings.version() != BINDINGS_VERSION_2 {
            return Err(invalid(format!(
                "a run of {} is made with bindings of version {BINDINGS_VERSION_2}",
                new.kind
            )));
        }
        if new.bindings.of(surface).is_none() {
            return Err(invalid(format!(
                "a run of {} has a binding of surface {surface}",
                new.kind
            )));
        }
        if new.person.is_empty() {
            return Err(invalid("a run acts for a person"));
        }
        self.with_tx(|tx| {
            crate::bot_surfaces::registration_in(tx, &new.started_by)?
                .ok_or(CoreError::NotFound)?;
            if let Some(parent) = &new.parent_run {
                run_in(tx, parent)?.ok_or(CoreError::NotFound)?;
            }
            check_model_priced(tx, &self.bot_overrides(), &new.model, new.key_source)?;
            let id = clax_core::new_ulid();
            let now = self.now();
            tx.execute(
                "INSERT INTO bot_runs (id, person, request, status, model, key_source,
                    created_at, updated_at, agent_grant, kind, bindings_json, request_json,
                    started_by, parent_run)
                 VALUES (?1, ?2, 'other', 'queued', ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    new.person,
                    new.model,
                    new.key_source.as_str(),
                    now,
                    new.agent_grant,
                    new.kind,
                    new.bindings.to_json(),
                    new.request.to_string(),
                    new.started_by,
                    new.parent_run
                ],
            )?;
            let mut rec = AuditRecord::new(AuditKind::BotRunStart, now)
                .with("run", id.as_str())
                .with("request", RequestKind::Other.as_str())
                .with("kind", new.kind.as_str())
                .with("thread_ids", Vec::<String>::new())
                .with("model", new.model.as_str())
                .with("key_source", new.key_source.as_str())
                .with("started_by", new.started_by.as_str());
            if let Some(parent) = &new.parent_run {
                rec = rec.with("parent_run", parent.as_str());
            }
            self.record_audit(tx, ctx, rec)?;
            Ok(run_in(tx, &id)?.expect("the run was just made"))
        })
    }

    /// [`Store::create_run_of`], held to `max_live` runs that have not
    /// ended (queued, running, stopping or parked) of `new.started_by`:
    /// they are counted in the transaction that makes the run, so starts
    /// of one registration made at once never pass the bound together.
    ///
    /// # Errors
    /// [`CODE_TOO_MANY_RUNS`] (an `Invalid`) when the registration has
    /// `max_live` such runs, and nothing is written; else those of
    /// [`Store::create_run_of`].
    pub fn create_run_of_within(
        &self,
        ctx: &AuditCtx,
        new: NewRunOf,
        max_live: usize,
    ) -> Result<Run> {
        let surface = match new.kind.split_once('.') {
            Some((surface, kind)) if !surface.is_empty() && !kind.is_empty() => surface,
            _ => {
                return Err(invalid(format!(
                    "a run's kind is `<surface>.<kind>`, not {:?}",
                    new.kind
                )));
            }
        };
        if surface == CLAX_SURFACE {
            return Err(invalid(format!(
                "{} is a kind of Clax's own surface, whose runs are made on an artifact",
                new.kind
            )));
        }
        if new.bindings.version() != BINDINGS_VERSION_2 {
            return Err(invalid(format!(
                "a run of {} is made with bindings of version {BINDINGS_VERSION_2}",
                new.kind
            )));
        }
        if new.bindings.of(surface).is_none() {
            return Err(invalid(format!(
                "a run of {} has a binding of surface {surface}",
                new.kind
            )));
        }
        if new.person.is_empty() {
            return Err(invalid("a run acts for a person"));
        }
        self.with_tx(|tx| {
            crate::bot_surfaces::registration_in(tx, &new.started_by)?
                .ok_or(CoreError::NotFound)?;
            let live: i64 = tx
                .prepare_cached(&format!(
                    "SELECT COUNT(*) FROM bot_runs WHERE started_by = ?1 AND status IN {LIVE}"
                ))?
                .query_row(params![new.started_by], |r| r.get(0))?;
            let live = usize::try_from(live).unwrap_or(0);
            if live >= max_live {
                return Err(CoreError::invalid(
                    CODE_TOO_MANY_RUNS,
                    format!(
                        "the registration has {live} runs that have not ended, which is the \
                         limit: start another when one has ended"
                    ),
                ));
            }
            if let Some(parent) = &new.parent_run {
                run_in(tx, parent)?.ok_or(CoreError::NotFound)?;
            }
            check_model_priced(tx, &self.bot_overrides(), &new.model, new.key_source)?;
            let id = clax_core::new_ulid();
            let now = self.now();
            tx.execute(
                "INSERT INTO bot_runs (id, person, request, status, model, key_source,
                    created_at, updated_at, agent_grant, kind, bindings_json, request_json,
                    started_by, parent_run)
                 VALUES (?1, ?2, 'other', 'queued', ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    new.person,
                    new.model,
                    new.key_source.as_str(),
                    now,
                    new.agent_grant,
                    new.kind,
                    new.bindings.to_json(),
                    new.request.to_string(),
                    new.started_by,
                    new.parent_run
                ],
            )?;
            let mut rec = AuditRecord::new(AuditKind::BotRunStart, now)
                .with("run", id.as_str())
                .with("request", RequestKind::Other.as_str())
                .with("kind", new.kind.as_str())
                .with("thread_ids", Vec::<String>::new())
                .with("model", new.model.as_str())
                .with("key_source", new.key_source.as_str())
                .with("started_by", new.started_by.as_str());
            if let Some(parent) = &new.parent_run {
                rec = rec.with("parent_run", parent.as_str());
            }
            self.record_audit(tx, ctx, rec)?;
            Ok(run_in(tx, &id)?.expect("the run was just made"))
        })
    }

    fn create_run_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        new: NewRun,
        bindings: Option<&Bindings>,
    ) -> Result<Run> {
        {
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                    params![new.artifact_id],
                    |r| r.get(0),
                )
                .optional()?;
            exists.ok_or(CoreError::NotFound)?;
            check_model_priced(tx, &self.bot_overrides(), &new.model, new.key_source)?;
            let id = clax_core::new_ulid();
            let now = self.now();
            tx.execute(
                "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                    model, key_source, base_version, created_at, updated_at, agent_grant, web,
                    kind, bindings_json, request_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?9, ?9, ?10, ?11,
                    ?12, ?13, ?14)",
                params![
                    id,
                    new.artifact_id,
                    new.person,
                    new.request.as_str(),
                    serde_json::to_string(&new.thread_ids).expect("serialisable IDs"),
                    new.model,
                    new.key_source.as_str(),
                    new.base_version,
                    now,
                    new.agent_grant,
                    new.web,
                    new.kind(),
                    bindings.map_or_else(|| new.bindings().to_json(), Bindings::to_json),
                    serde_json::to_string(&new.clax_request()).expect("serialisable IDs")
                ],
            )?;
            let mut rec = AuditRecord::new(AuditKind::BotRunStart, now)
                .with("run", id.as_str())
                .with("request", new.request.as_str())
                .with("thread_ids", new.thread_ids.clone())
                .with("model", new.model.as_str())
                .with("key_source", new.key_source.as_str());
            rec.ids.artifact = Some(new.artifact_id.clone());
            self.record_audit(tx, ctx, rec)?;
            Ok(run_in(tx, &id)?.expect("the run was just made"))
        }
    }

    /// Run `id`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        self.with_read(|c| run_in(c, id))
    }

    /// Run `id` with its steps in order, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn run_with_steps(&self, id: &str) -> Result<Option<(Run, Vec<Step>)>> {
        self.with_read(|c| {
            let Some(run) = run_in(c, id)? else {
                return Ok(None);
            };
            let mut q = c.prepare_cached(&format!(
                "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 ORDER BY n"
            ))?;
            let steps = q
                .query_map(params![id], row_to_step)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some((run, steps)))
        })
    }

    /// [`Store::run_with_steps`] for a view of the run: a step whose
    /// payload or observations do not read ([`row_to_step`]) is left out,
    /// logged, and counted in the third member, so that one damaged row
    /// fails no listing, no usage page and no view of the run.
    ///
    /// # Errors
    /// The store's.
    pub fn run_with_readable_steps(&self, id: &str) -> Result<Option<(Run, Vec<Step>, usize)>> {
        self.with_read(|c| {
            let Some(run) = run_in(c, id)? else {
                return Ok(None);
            };
            let mut q = c.prepare_cached(&format!(
                "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 ORDER BY n"
            ))?;
            let mut steps = Vec::new();
            let mut left_out = 0;
            for step in q.query_map(params![id], row_to_step)? {
                match step {
                    Ok(step) => steps.push(step),
                    Err(e) if unreadable_db(&e) => {
                        tracing::warn!("clax-store: a step of run {id} does not read: {e}");
                        left_out += 1;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(Some((run, steps, left_out)))
        })
    }

    /// Run `id` with what it was made as ([`RunAsMade`]), without its
    /// steps.
    ///
    /// # Errors
    /// The store's.
    pub fn run_as_made(&self, id: &str) -> Result<Option<(Run, RunAsMade)>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {RUN_COLS}, kind, bindings_json, request_json FROM bot_runs
                  WHERE id = ?1"
            ))?
            .query_row(params![id], |r| {
                let run = row_to_run(r)?;
                let made = as_made(&run, r.get(19)?, r.get(20)?, r.get(21)?);
                Ok((run, made))
            })
            .optional()?)
        })
    }

    /// [`Store::run_with_steps`], with what the run was made as
    /// ([`RunAsMade`]), read from the run's row in the same statement
    /// as the rest of it.
    ///
    /// # Errors
    /// The store's.
    pub fn run_with_steps_as_made(&self, id: &str) -> Result<Option<(Run, RunAsMade, Vec<Step>)>> {
        self.with_read(|c| {
            let found = c
                .prepare_cached(&format!(
                    "SELECT {RUN_COLS}, kind, bindings_json, request_json FROM bot_runs
                      WHERE id = ?1"
                ))?
                .query_row(params![id], |r| {
                    let run = row_to_run(r)?;
                    let made = as_made(&run, r.get(19)?, r.get(20)?, r.get(21)?);
                    Ok((run, made))
                })
                .optional()?;
            let Some((run, made)) = found else {
                return Ok(None);
            };
            let mut q = c.prepare_cached(&format!(
                "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 ORDER BY n"
            ))?;
            let steps = q
                .query_map(params![id], row_to_step)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some((run, made, steps)))
        })
    }

    /// Up to `limit` runs on artifact `aid`, newest first.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_artifact(&self, aid: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| runs_where(c, "artifact_id = ?1", &aid, limit))
    }

    /// Up to `limit` runs acting for `person`, newest first.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_person(&self, person: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| runs_where(c, "person = ?1", &person, limit))
    }

    /// Up to `limit` runs registration `registration` started
    /// (`bot_runs.started_by`), newest first.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_started_by(&self, registration: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| runs_where(c, "started_by = ?1", &registration, limit))
    }

    /// How many runs registration `registration` started that have not
    /// ended: queued, running, stopping or parked.
    ///
    /// # Errors
    /// The store's.
    pub fn live_runs_started_by(&self, registration: &str) -> Result<usize> {
        self.with_read(|c| {
            let n: i64 = c
                .prepare_cached(&format!(
                    "SELECT COUNT(*) FROM bot_runs WHERE started_by = ?1 AND status IN {LIVE}"
                ))?
                .query_row(params![registration], |r| r.get(0))?;
            Ok(usize::try_from(n).unwrap_or(0))
        })
    }

    /// Who started run `id` and from which run, as `(started_by,
    /// parent_run)`: a registration's ID and a run's, each `None` where
    /// the run has none (every run of Clax's own kinds). `None` for no
    /// such run.
    ///
    /// # Errors
    /// The store's.
    pub fn run_origin(&self, id: &str) -> Result<Option<(Option<String>, Option<String>)>> {
        self.with_read(|c| {
            Ok(
                c.prepare_cached("SELECT started_by, parent_run FROM bot_runs WHERE id = ?1")?
                    .query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?,
            )
        })
    }

    /// Up to `limit` runs whose request names thread `tid`, newest first:
    /// a thread with one is ClaxBot's.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_thread(&self, tid: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {RUN_COLS} FROM bot_runs
                  WHERE EXISTS (SELECT 1 FROM json_each(thread_ids_json) WHERE value = ?1)
                  ORDER BY created_at DESC, id DESC LIMIT ?2"
            ))?;
            let rows = q.query_map(params![tid, limit], row_to_run)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The runs made in the week `at` falls in (Monday 00:00 UTC to the
    /// next), counted by the person they act for.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_in_week(
        &self,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let start = crate::credits::week_start(at);
        let (from, to) = (
            clax_core::time::stamp(start),
            clax_core::time::stamp(crate::credits::week_end(start)),
        );
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT person, count(*) FROM bot_runs WHERE created_at >= ?1 AND created_at < ?2
                  GROUP BY person",
            )?;
            let rows = q.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The credits of up to `limit` of the newest finished runs, acting
    /// for `person` when given, else for anyone: what a run's estimate is
    /// taken from.
    ///
    /// # Errors
    /// The store's.
    pub fn finished_run_credits(&self, person: Option<&str>, limit: u32) -> Result<Vec<i64>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT credits_milli FROM bot_runs
                  WHERE status = 'finished' AND (?1 IS NULL OR person = ?1)
                  ORDER BY ended_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = q.query_map(params![person, limit], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Every run that has not ended (`queued`, `running`, `stopping`,
    /// `parked`), oldest first: what a driver resumes on start. A parked
    /// run among them is not claimable ([`Store::claim_run`]).
    ///
    /// # Errors
    /// The store's.
    pub fn live_runs(&self) -> Result<Vec<Run>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {RUN_COLS} FROM bot_runs WHERE status IN {LIVE}
                  ORDER BY created_at, id"
            ))?;
            let rows = q.query_map([], row_to_run)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Claims run `id` for `driver`: a queued run becomes running. The
    /// claim is taken when the run is unclaimed, already `driver`'s, or
    /// claimed more than `lease_secs` ago (its driver gone; a lease of 0
    /// or less takes it whatever its age); it is renewed each time. Returns the run when `driver` now holds it, `None` when
    /// another driver does or the run has ended.
    ///
    /// A parked run is never claimed, whatever the lease and whether or
    /// not its `due_at` has passed: the answer is `None` and nothing
    /// changes. It has no driver until what it waits on is journaled
    /// ([`Store::answer_call`], [`Store::expire_calls`]), it is cancelled
    /// or it is unparked ([`Store::unpark_run`]); a driver that claimed
    /// it earlier would hold an answered run from the others.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn claim_run(&self, id: &str, driver: &DriverId, lease_secs: i64) -> Result<Option<Run>> {
        let driver = driver.as_str();
        let now = self.now_utc();
        let stale = clax_core::time::stamp(now - chrono::Duration::seconds(lease_secs.max(0)));
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if run.status.is_terminal() || run.status == RunStatus::Parked {
                return Ok(None);
            }
            let stamp = clax_core::time::stamp(now);
            let free = match (&run.claimed_by, &run.claimed_at) {
                (None, _) => true,
                (Some(d), _) if d == driver => true,
                (Some(_), Some(at)) => lease_secs <= 0 || *at < stale,
                (Some(_), None) => true,
            };
            if !free {
                return Ok(None);
            }
            tx.execute(
                "UPDATE bot_runs SET claimed_by = ?2, claimed_at = ?3, updated_at = ?3,
                   status = CASE status WHEN 'queued' THEN 'running' ELSE status END
                 WHERE id = ?1",
                params![id, driver, stamp],
            )?;
            run_in(tx, id)
        })
    }

    /// Lets go of `driver`'s claim on run `id`, so another driver may
    /// take it at once.
    ///
    /// # Errors
    /// The store's.
    pub fn release_run(&self, id: &str, driver: &DriverId) -> Result<()> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_runs SET claimed_by = NULL, claimed_at = NULL
                  WHERE id = ?1 AND claimed_by = ?2",
                params![id, driver],
            )?;
            Ok(())
        })
    }

    /// Records when run `id` is next due (`None`: at once), as the driver
    /// that let it wait decided, so a driver that starts later (a restarted
    /// daemon, a new Durable Object incarnation) does not tick it early.
    ///
    /// # Errors
    /// The store's.
    pub fn set_run_due(&self, id: &str, at: Option<chrono::DateTime<chrono::Utc>>) -> Result<()> {
        let at = at.map(clax_core::time::stamp);
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_runs SET due_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            Ok(())
        })
    }

    /// Parks run `id` at `now` for `driver`, which holds its claim and is
    /// executing its open step, `place.step_n`: the run becomes `parked`
    /// and unclaimed, with `waiting_on` (what it waits on, for whoever
    /// shows the run; the store reads none of it) and `due_at`, the
    /// deadline of what it waits on (`None`: none), by which
    /// [`Store::expire_calls`] is to be run. The step stays begun and
    /// uncommitted, to be executed again, and counts one more park
    /// ([`Parking::parks`]).
    ///
    /// `place` is the journal place whose row the driver would wait for.
    /// When the journal holds it already (the answer came before the
    /// park), the run is not parked: [`Park::Answered`], nothing changed,
    /// the claim kept. Checking and parking are one transaction, and so
    /// is journaling an answer and unparking ([`Store::answer_call`]), so
    /// an answer is never journaled for a run that then parks unwoken.
    ///
    /// A run that is parked already is returned as it is
    /// ([`Park::Parked`]), whoever asks and whatever they pass: a park
    /// made again changes nothing.
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` unless `driver` holds the run;
    /// `run_invalid` for a run that is not running (a stopping run's next
    /// step is its stopped step) or whose open step is not
    /// `place.step_n`; the store's.
    pub fn park_run(
        &self,
        id: &str,
        driver: &DriverId,
        place: JournalPlace,
        waiting_on: &Value,
        due_at: Option<chrono::DateTime<chrono::Utc>>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Park> {
        let driver = driver.as_str();
        let (at, due) = (
            clax_core::time::stamp(now),
            due_at.map(clax_core::time::stamp),
        );
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if run.status == RunStatus::Parked {
                return Ok(Park::Parked(run));
            }
            check_claim(&run, driver)?;
            if run.status != RunStatus::Running {
                return Err(invalid(format!(
                    "run {id} is {}, not running",
                    run.status.as_str()
                )));
            }
            if open_step_in(tx, id)?.is_none_or(|s| s.n != place.step_n) {
                return Err(invalid(format!(
                    "step {} of run {id} is not begun and uncommitted",
                    place.step_n
                )));
            }
            if journal_holds(tx, id, place.step_n, place.seq)? {
                return Ok(Park::Answered(run));
            }
            tx.execute(
                "UPDATE bot_runs SET status = 'parked', parked_at = ?2, waiting_on = ?3,
                   due_at = ?4, claimed_by = NULL, claimed_at = NULL, updated_at = ?2
                 WHERE id = ?1",
                params![id, at, waiting_on.to_string(), due],
            )?;
            tx.execute(
                "UPDATE bot_steps SET parks = parks + 1 WHERE run_id = ?1 AND n = ?2",
                params![id, place.step_n],
            )?;
            Ok(Park::Parked(run_in(tx, id)?.expect("the run exists")))
        })
    }

    /// Ends run `id`'s parking at `now`: it is `queued` again and
    /// unclaimed, due at once, so a driver may claim it and execute its
    /// open step again; the time it was parked is added to its
    /// `parked_ms`, and its `parked_at` and `waiting_on` are cleared.
    /// Returns the run after, or `None`, changing nothing, for a run that
    /// is not parked: made again, it does nothing.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn unpark_run(&self, id: &str, now: chrono::DateTime<chrono::Utc>) -> Result<Option<Run>> {
        let at = clax_core::time::stamp(now);
        self.with_tx(|tx| {
            run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if !unpark_in(tx, id, &at)? {
                return Ok(None);
            }
            run_in(tx, id)
        })
    }

    /// What run `id` keeps of its parking, or `None` for no such run.
    ///
    /// # Errors
    /// The store's.
    pub fn run_parking(&self, id: &str) -> Result<Option<Parking>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT parked_at, parked_ms, waiting_on,
                        COALESCE((SELECT parks FROM bot_steps
                                   WHERE run_id = bot_runs.id AND committed_at IS NULL), 0)
                   FROM bot_runs WHERE id = ?1",
            )?
            .query_row(params![id], |r| {
                Ok(Parking {
                    parked_at: r.get(0)?,
                    parked_ms: r.get(1)?,
                    waiting_on: stored_json_or_null(r, 2, "bot_runs.waiting_on")?,
                    parks: r.get(3)?,
                })
            })
            .optional()?)
        })
    }

    /// How many runs acting for `person` are parked now.
    ///
    /// # Errors
    /// The store's.
    pub fn parked_runs_of(&self, person: &str) -> Result<i64> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM bot_runs WHERE person = ?1 AND status = 'parked'",
                params![person],
                |r| r.get(0),
            )?)
        })
    }

    /// Renews `driver`'s claim on run `id` (now, by the store's clock), so a
    /// turn longer than the lease keeps it. Returns whether `driver` still
    /// holds the run; `false` once another driver took it over, the run
    /// parked (which lets go of the claim) or the run ended, and the
    /// driver then stops working on it.
    ///
    /// # Errors
    /// The store's.
    pub fn renew_lease(&self, id: &str, driver: &DriverId) -> Result<bool> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            Ok(tx.execute(
                &format!(
                    "UPDATE bot_runs SET claimed_at = ?3 WHERE id = ?1 AND claimed_by = ?2
                       AND status IN {LIVE}"
                ),
                params![id, driver, self.now()],
            )? > 0)
        })
    }

    /// The step a driver of run `id` works on next: its open step, else the
    /// one after the last (1 for a new run).
    ///
    /// # Errors
    /// The store's.
    pub fn next_step_n(&self, id: &str) -> Result<i64> {
        self.with_read(|c| next_n_in(c, id))
    }

    /// The step of run `id` begun and not committed: the one to execute
    /// again on resume.
    ///
    /// # Errors
    /// The store's.
    pub fn open_step(&self, id: &str) -> Result<Option<Step>> {
        self.with_read(|c| open_step_in(c, id))
    }

    /// Begins step `n` of run `id` as `kind`, for `driver`, which holds
    /// the run's claim and expects the run at step `n`
    /// ([`Store::next_step_n`]). When step `n` is already begun and not
    /// committed, that step is the one returned, with `kind` and its
    /// `attempt` one more, its `started_at` the first attempt's (nothing of
    /// it was committed, so it is executed again).
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` when `driver` does not hold the run;
    /// `step_mismatch` when the run is at another step; `run_invalid` for a
    /// run that is not running or stopping, or a `stopping` run's step that
    /// is not `stopped`; `model_not_priced` for a turn whose model no longer
    /// resolves to a token rate (`Store::model_rate`); the store's.
    pub fn begin_step(&self, id: &str, driver: &DriverId, n: i64, kind: StepKind) -> Result<Step> {
        self.begin_step_on(id, driver, n, kind, KeySource::Instance)
    }

    /// [`Store::begin_step`] for a step whose turn runs on `turn_key`, the
    /// source of the key the caller chose for the call: a turn on
    /// [`KeySource::Own`] is not refused for its model's price (its token
    /// rows are waived), a turn on the instance key is. `turn_key` is read
    /// only when `kind` is [`StepKind::Turn`]; no other step is checked for
    /// a price, since none charges tokens. [`Store::begin_step`] passes
    /// [`KeySource::Instance`], the stricter check.
    ///
    /// # Errors
    /// As [`Store::begin_step`].
    pub fn begin_step_on(
        &self,
        id: &str,
        driver: &DriverId,
        n: i64,
        kind: StepKind,
        turn_key: KeySource,
    ) -> Result<Step> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver)?;
            check_step(tx, id, n)?;
            if kind == StepKind::Turn {
                // The key the caller chose for the call, which its token
                // rows are charged on.
                check_model_priced(tx, &self.bot_overrides(), &run.model, turn_key)?;
            }
            match run.status {
                RunStatus::Running => {}
                RunStatus::Stopping if matches!(kind, StepKind::Stopped | StepKind::Failed) => {}
                RunStatus::Stopping => {
                    return Err(invalid("a stopping run's next step is its stopped step"));
                }
                s => return Err(invalid(format!("run {id} is {}, not running", s.as_str()))),
            }
            let now = self.now();
            if let Some(open) = open_step_in(tx, id)? {
                tx.execute(
                    "UPDATE bot_steps SET kind = ?3, attempt = attempt + 1
                      WHERE run_id = ?1 AND n = ?2",
                    params![id, open.n, kind.as_str()],
                )?;
                return Ok(Step {
                    kind,
                    attempt: open.attempt + 1,
                    ..open
                });
            }
            tx.execute(
                "INSERT INTO bot_steps (run_id, n, kind, started_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, n, kind.as_str(), now],
            )?;
            Ok(Step {
                run_id: id.into(),
                n,
                kind,
                started_at: now,
                attempt: 1,
                committed_at: None,
                payload: None,
                observed: Vec::new(),
                failures: 0,
            })
        })
    }

    /// Commits step `n` of run `id` with `commit` for `driver`, which holds
    /// the run's claim, in one transaction (see the module documentation),
    /// events recorded under `ctx`. A terminal step ends the run and
    /// records `bot.run_end {run, outcome, reason, credits_milli}` with the
    /// run's total. It charges the step's own usage rows on top of the
    /// web calls' rows [`Store::spend_journaled`] charged during the step,
    /// which it never rewrites.
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` when `driver` does not hold the run
    /// (another driver took it over); `run_invalid` when step `n` is not
    /// the run's open step; the charge's (`unknown_cost_line`,
    /// `invalid_usage`), which commit nothing; the store's.
    pub fn commit_bot_step(
        &self,
        ctx: &AuditCtx,
        id: &str,
        driver: &DriverId,
        n: i64,
        commit: StepCommit,
    ) -> Result<Committed> {
        self.commit_bot_step_on(ctx, id, driver, n, commit, None, &[])
    }

    /// [`Store::commit_bot_step`] with what a run of any surface adds to
    /// a commit, in the commit's transaction.
    ///
    /// `subject` is what the step's records are filed on: each of the
    /// step's events and the run's `bot.run_end` take the subject's
    /// artifact (none, for a subject without one: the record is then
    /// found by the person it was made for), and each other ID of the
    /// subject where the record names none of its own. `None` files them
    /// on the run's artifact, as the row has it.
    ///
    /// `ends`, for a terminal step, is how the run ended for each
    /// registration to be told, kept until it acknowledges
    /// ([`Store::ends_for`], [`Store::ack_end`]). The run ends and its
    /// ends are kept together, or neither.
    ///
    /// # Errors
    /// As [`Store::commit_bot_step`]; `run_invalid` for `ends` given with
    /// a step that does not end the run; `NotFound` for an end whose
    /// registration does not exist. Each commits nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_bot_step_on(
        &self,
        ctx: &AuditCtx,
        id: &str,
        driver: &DriverId,
        n: i64,
        commit: StepCommit,
        subject: Option<&AuditIds>,
        ends: &[crate::bot_pending::EndFor],
    ) -> Result<Committed> {
        let driver = driver.as_str();
        let at = self.now_utc();
        let ov = self.bot_overrides();
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver)?;
            let open = open_step_in(tx, id)?
                .filter(|s| s.n == n)
                .ok_or_else(|| invalid(format!("step {n} of run {id} is not begun and uncommitted")))?;
            if run.status.is_terminal() {
                return Err(invalid(format!("run {id} has ended")));
            }
            let usage = charge_in(tx, &ov, &run.person, Some((id, n)), &commit.usage, at)?;
            let credits: i64 = usage.iter().map(|u| u.credits_milli).sum();
            for d in &commit.draft {
                match d {
                    DraftChange::Write {
                        path,
                        content,
                        content_type,
                    } => tx.execute(
                        "INSERT INTO bot_draft_files (run_id, path, content, content_type)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (run_id, path) DO UPDATE SET content = excluded.content,
                           content_type = excluded.content_type",
                        params![id, path, content, content_type],
                    )?,
                    DraftChange::Delete { path } => tx.execute(
                        "INSERT INTO bot_draft_files (run_id, path, content, content_type)
                         VALUES (?1, ?2, NULL, NULL)
                         ON CONFLICT (run_id, path) DO UPDATE SET content = NULL, content_type = NULL",
                        params![id, path],
                    )?,
                };
            }
            let now = clax_core::time::stamp(at);
            tx.execute(
                "UPDATE bot_steps SET committed_at = ?3, payload_json = ?4 WHERE run_id = ?1 AND n = ?2",
                params![id, n, now, commit.payload.to_string()],
            )?;
            crate::bot_messages::deliver_in(tx, id, n, &commit.delivered, &now)?;
            // A call of the step still pending was not reached by the
            // attempt that commits: it stops waiting, unjournaled.
            tx.execute(
                "UPDATE bot_pending_calls SET answered_at = ?3
                  WHERE run_id = ?1 AND step_n = ?2 AND answered_at IS NULL",
                params![id, n, now],
            )?;
            let total = run.credits_milli + credits;
            let end = open.kind.ends_as();
            if end.is_none() && !ends.is_empty() {
                return Err(invalid(format!(
                    "step {n} of run {id} does not end the run, so no end is kept with it"
                )));
            }
            for kept in ends {
                crate::bot_pending::record_end_in(tx, id, kept, None, &now)?;
            }
            match end {
                Some(status) => tx.execute(
                    "UPDATE bot_runs SET credits_milli = ?2, status = ?3, reason = ?4, updated_at = ?5,
                       ended_at = ?5, claimed_by = NULL, claimed_at = NULL WHERE id = ?1",
                    params![id, total, status.as_str(), commit.reason, now],
                )?,
                None => tx.execute(
                    "UPDATE bot_runs SET credits_milli = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, total, now],
                )?,
            };
            let of_the_row = AuditIds {
                artifact: run.artifact().map(str::to_string),
                ..AuditIds::default()
            };
            let subject = subject.unwrap_or(&of_the_row);
            for mut rec in commit.events {
                file_on(&mut rec, subject);
                rec.body.insert("run".into(), id.into());
                rec.body.insert("step".into(), n.into());
                if matches!(rec.kind, AuditKind::BotTurn | AuditKind::BotScript) {
                    rec.body.entry("credits_milli".into()).or_insert(credits.into());
                }
                self.record_audit(tx, ctx, rec)?;
            }
            if let Some(status) = end {
                let mut rec = AuditRecord::new(AuditKind::BotRunEnd, now.clone())
                    .with("run", id)
                    .with("outcome", status.as_str())
                    .with("reason", commit.reason.clone())
                    .with("credits_milli", total);
                file_on(&mut rec, subject);
                self.record_audit(tx, ctx, rec)?;
            }
            let step = Step {
                committed_at: Some(now),
                payload: Some(commit.payload),
                ..open
            };
            Ok(Committed {
                step,
                usage,
                credits_milli: credits,
                run: run_in(tx, id)?.expect("the run exists"),
            })
        })
    }

    /// Asks run `id` to stop, as `ctx`: a queued, running or parked run
    /// becomes `stopping`, recorded as `bot.cancel {run, by}` (`by` the
    /// person key of who asked, `*system` for Clax itself), and its driver
    /// writes its `stopped` step at the next boundary. A parked run's
    /// parking ends first, as [`Store::unpark_run`] ends it, so a driver
    /// may claim it at once. The pending calls of a run asked to stop
    /// ([`crate::bot_pending`]) stop waiting, unjournaled, so an answer
    /// that comes later is refused. Returns the run after; an ended or
    /// stopping run is unchanged and records nothing.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn cancel_run(&self, ctx: &AuditCtx, id: &str) -> Result<Run> {
        self.with_tx(|tx| {
            let now = self.now();
            // A parked run is queued again, unclaimed, before it is asked
            // to stop.
            unpark_in(tx, id, &now)?;
            let changed = tx.execute(
                "UPDATE bot_runs SET status = 'stopping', updated_at = ?2
                  WHERE id = ?1 AND status IN ('queued', 'running')",
                params![id, now],
            )?;
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if changed > 0 {
                tx.execute(
                    "UPDATE bot_pending_calls SET answered_at = ?2
                      WHERE run_id = ?1 AND answered_at IS NULL",
                    params![id, now],
                )?;
                let mut rec = AuditRecord::new(AuditKind::BotCancel, now)
                    .with("run", id)
                    .with("by", set_by(ctx));
                rec.ids.artifact = run.artifact().map(str::to_string);
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(run)
        })
    }

    /// Records `entry` as what attempt `attempt` of run `id`'s open step
    /// `n` has used so far (replacing that attempt's earlier entry), for
    /// `driver`, which holds the run: a model call's usage as it streams,
    /// so an attempt that never commits is still accounted for. Returns
    /// whether it was recorded (`false` once the driver lost the run or the
    /// step moved on).
    ///
    /// # Errors
    /// The store's.
    pub fn observe_step(
        &self,
        id: &str,
        driver: &DriverId,
        n: i64,
        attempt: i64,
        entry: Value,
    ) -> Result<bool> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            let Some(run) = run_in(tx, id)? else {
                return Ok(false);
            };
            if run.claimed_by.as_deref() != Some(driver) {
                return Ok(false);
            }
            let Some(open) = open_step_in(tx, id)?.filter(|s| s.n == n) else {
                return Ok(false);
            };
            let mut observed = open.observed;
            let mut entry = entry;
            entry["attempt"] = attempt.into();
            match observed.iter_mut().find(|e| e["attempt"] == attempt) {
                Some(e) => *e = entry,
                None => observed.push(entry),
            }
            tx.execute(
                "UPDATE bot_steps SET observed_json = ?3 WHERE run_id = ?1 AND n = ?2",
                params![id, n, Value::Array(observed).to_string()],
            )?;
            Ok(true)
        })
    }

    /// Counts one more failed commit of run `id`'s open step `n`; returns
    /// how many there have been (0 when `n` is not the open step).
    ///
    /// # Errors
    /// The store's.
    pub fn note_commit_failure(&self, id: &str, n: i64) -> Result<i64> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_steps SET failures = failures + 1
                  WHERE run_id = ?1 AND n = ?2 AND committed_at IS NULL",
                params![id, n],
            )?;
            Ok(tx
                .query_row(
                    "SELECT failures FROM bot_steps WHERE run_id = ?1 AND n = ?2 AND committed_at IS NULL",
                    params![id, n],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    /// The journal of run `id`'s step `n`: every call its scripts made, by
    /// `seq`.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_calls(&self, id: &str, n: i64) -> Result<Vec<CallEntry>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {CALL_COLS} FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 ORDER BY seq"
            ))?;
            let rows = q.query_map(params![id, n], row_to_call)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// How many journal entries run `id` has, of any step, whose
    /// operation is one of `ops` and which are not a failure whose code
    /// is one of `not_done`: a success, or a failure under any other
    /// code (one with no code, or whose result is not JSON, included).
    /// What a count of a run's calls of some operations that were
    /// performed is. 0 for no `ops`.
    ///
    /// Counted in the store: no entry is read into the caller, and of an
    /// entry's result only a failure's `code` is looked at, so a
    /// success's result, however long, is not parsed. The rows looked at
    /// are those of the run, found by the primary key's `run_id` prefix
    /// (no index has `op`): at worst every journal row of the run, that
    /// is its steps times the calls and notices one step journals.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_calls_counted(&self, id: &str, ops: &[&str], not_done: &[&str]) -> Result<u64> {
        if ops.is_empty() {
            return Ok(0);
        }
        let mark = |i: usize| format!("?{}", i + 2);
        let op_marks: Vec<String> = (0..ops.len()).map(mark).collect();
        let code_marks: Vec<String> = (ops.len()..ops.len() + not_done.len()).map(mark).collect();
        // A failure is left out when its result is JSON whose `code` is
        // one of `not_done`; a success's result is never looked at.
        let not_done_sql = if not_done.is_empty() {
            String::new()
        } else {
            format!(
                " AND NOT (ok = 0 AND json_valid(result_json) \
                 AND COALESCE(json_extract(result_json, '$.code') IN ({}), 0))",
                code_marks.join(", ")
            )
        };
        self.with_read(|c| {
            let mut q = c.prepare(&format!(
                "SELECT COUNT(*) FROM bot_calls WHERE run_id = ?1 AND op IN ({}){not_done_sql}",
                op_marks.join(", ")
            ))?;
            let args: Vec<&dyn rusqlite::ToSql> = std::iter::once(&id as &dyn rusqlite::ToSql)
                .chain(ops.iter().map(|op| op as &dyn rusqlite::ToSql))
                .chain(not_done.iter().map(|code| code as &dyn rusqlite::ToSql))
                .collect();
            let n: i64 = q.query_row(args.as_slice(), |r| r.get(0))?;
            Ok(u64::try_from(n).unwrap_or(0))
        })
    }

    /// Whether the journal holds an entry for call `seq` of step `n` of
    /// run `id`, by the primary key alone.
    ///
    /// # Errors
    /// The store's.
    pub fn has_bot_call(&self, id: &str, n: i64, seq: i64) -> Result<bool> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3)",
            )?;
            Ok(q.query_row(params![id, n, seq], |r| r.get::<_, bool>(0))?)
        })
    }

    /// The journal entry of call `seq` (`op` on arguments hashing to
    /// `args_hash`) of attempt `attempt` of step `step_n` of run `run_id`:
    /// made only for the driver holding the run, so no one else can
    /// journal under a key of its own.
    ///
    /// # Errors
    /// `run_not_claimed` unless `driver` holds the run; `NotFound`; the
    /// store's.
    #[allow(clippy::too_many_arguments)]
    pub fn call_entry(
        &self,
        run_id: &str,
        driver: &DriverId,
        step_n: i64,
        attempt: i64,
        seq: i64,
        op: &str,
        args_hash: &str,
    ) -> Result<CallEntry> {
        let run = self.run(run_id)?.ok_or(CoreError::NotFound)?;
        check_claim(&run, driver.as_str())?;
        Ok(CallEntry {
            run_id: run_id.to_string(),
            step_n,
            seq,
            attempt,
            op: op.to_string(),
            args_hash: args_hash.to_string(),
            ok: true,
            result: Value::Null,
        })
    }

    /// Journals `entry` unless its `(run, step, seq)` already is; returns the
    /// entry the journal holds for it (the earlier one when there was one).
    ///
    /// # Errors
    /// The store's.
    pub fn record_bot_call(&self, entry: &CallEntry) -> Result<CallEntry> {
        let fault = self
            .record_fault
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if fault.is_some_and(|f| f(entry)) {
            return Err(CoreError::invalid(
                "record_fault",
                "a failure to write the journal row was injected",
            ));
        }
        self.with_tx(|tx| {
            if let Some(prev) = call_in(tx, entry)? {
                return Ok(prev);
            }
            insert_call(tx, entry, &self.now())?;
            Ok(entry.clone())
        })
    }

    /// Performs a write with its journal entry in one transaction: when the
    /// journal already holds `entry`'s `(run, step, seq)`, answers with what
    /// it recorded (the same op and arguments) or `key_reused` (others),
    /// performing nothing; else runs `write` on the transaction and
    /// journals its value, both committed together, or neither when either
    /// fails.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure as `Invalid` with its code; `write`'s;
    /// the store's.
    pub fn with_bot_journal(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
        write: impl FnOnce(&Tx<'_>) -> Result<Value>,
    ) -> Result<Journaled> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(prev);
        }
        self.with_tx(|tx| {
            guard(&Tx(tx))?;
            let value = write(&Tx(tx))?;
            self.journal_in(tx, entry, value)
        })
    }

    /// The first half of a write to an artifact's own store journaled as
    /// `entry` (ClaxBot's `db.set`, `db.update` and `db.delete`): the
    /// journal's answer when it holds `entry`'s key (the caller writes
    /// nothing), else `None` once `guard` passed in a transaction of its
    /// own. The caller then writes under [`CallEntry::op_id`], which the
    /// artifact's store keeps with the write, and journals it
    /// ([`Store::journal_app_write`]). There is no transaction across the
    /// two stores: a crash between the write and its entry leaves the
    /// write, and the call made again writes nothing more and journals
    /// what the write answered.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure; `guard`'s; the store's.
    pub fn app_write_begin(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
    ) -> Result<Option<Journaled>> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(Some(prev));
        }
        self.with_tx(|tx| guard(&Tx(tx)))?;
        Ok(None)
    }

    /// The second half of [`Store::app_write_begin`]: journals `value`, the
    /// answer of the write the artifact's store made under `entry`'s
    /// operation ID, after `guard` passes again, in one transaction. A
    /// person who lost the right between the write and this call is refused
    /// here, and the write stands without its entry.
    ///
    /// # Errors
    /// `guard`'s; the store's.
    pub fn journal_app_write(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
        value: Value,
    ) -> Result<Journaled> {
        self.with_tx(|tx| {
            guard(&Tx(tx))?;
            self.journal_in(tx, entry, value)
        })
    }

    /// The journal's answer for `entry`'s key, when it holds one: what a
    /// journaled write answers without writing.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure; the store's.
    pub(crate) fn journaled(&self, entry: &CallEntry) -> Result<Option<Journaled>> {
        let prev = self.with_read(|c| call_in(c, entry))?;
        prev.map(|p| answer_of(&p, entry).map(Journaled))
            .transpose()
    }

    /// Journals `value` as `entry`'s answer on `tx`, the write's own
    /// transaction (a failure rolls the write back with it).
    pub(crate) fn journal_in(
        &self,
        tx: &Connection,
        entry: &CallEntry,
        value: Value,
    ) -> Result<Journaled> {
        let fault = self
            .journal_fault
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if fault.is_some_and(|f| f(entry)) {
            return Err(CoreError::invalid(
                "journal_fault",
                "a crash was injected between the write and its journal entry",
            ));
        }
        let done = CallEntry {
            ok: true,
            result: value.clone(),
            ..entry.clone()
        };
        insert_call(tx, &done, &self.now())?;
        Ok(Journaled(value))
    }

    /// Makes every journaled write whose entry `fault` accepts fail after
    /// the write and before its journal row, rolling both back, as a crash
    /// there would; `None` clears it. For crash tests
    /// ([`clax-bot`'s host conformance suite]).
    #[doc(hidden)]
    pub fn set_journal_fault(&self, fault: Option<JournalFault>) {
        *self
            .journal_fault
            .write()
            .unwrap_or_else(|e| e.into_inner()) = fault;
    }

    /// Makes every [`Store::record_bot_call`] whose entry `fault` accepts
    /// fail and write nothing, as a store that cannot write would; `None`
    /// clears it. For tests of what the run loop does when a call's row
    /// cannot be written.
    #[doc(hidden)]
    pub fn set_record_fault(&self, fault: Option<JournalFault>) {
        *self.record_fault.write().unwrap_or_else(|e| e.into_inner()) = fault;
    }

    /// Run `id`'s draft overlay, by path: each path it wrote or deleted.
    ///
    /// # Errors
    /// The store's.
    pub fn draft_files(&self, id: &str) -> Result<Vec<DraftFile>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT path, content, content_type FROM bot_draft_files WHERE run_id = ?1 ORDER BY path",
            )?;
            let rows = q.query_map(params![id], |r| {
                Ok(DraftFile {
                    path: r.get(0)?,
                    content: r.get(1)?,
                    content_type: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Removes what ended runs no longer need, by the store's clock: the
    /// call journal (`bot_calls`), the page store (`bot_web_pages`), the
    /// content (`bot_content`, and each blob no row names any more:
    /// [`crate::bot_content`]), the messages given
    /// ([`crate::bot_messages`]) and the pending calls
    /// (`bot_pending_calls`) of each run that ended over
    /// [`BOT_JOURNAL_KEEP`] ago, the draft overlay (`bot_draft_files`) of
    /// each that ended over [`BOT_DRAFT_KEEP`] ago, the messages never
    /// given of each that ended, and each run's end kept for a
    /// registration (`bot_run_ends`), over
    /// [`crate::bot_pending::BOT_END_KEEP`] ago, acknowledged or not; and
    /// the surfaces' registrations, definitions and prompt files that
    /// nothing needs ([`crate::bot_surfaces::BOT_REGISTRATION_KEEP`],
    /// [`crate::bot_surfaces::BOT_DEF_KEEP`]: a registration that
    /// started a run the store has is kept, a definition a
    /// registration, a pending call or a run's bindings name, and a
    /// prompt file a registration or a run's bindings name). The
    /// runs, their steps, usage and audit events stay. How many rows of
    /// each it removed.
    ///
    /// **Bounded.** A pass is at most [`PRUNE_TXS_PER_PASS`]
    /// transactions, each deleting the rows of [`PRUNE_RUNS_PER_TX`]
    /// runs and [`PRUNE_ROWS_PER_TX`] rows at the most, so no one
    /// transaction grows with what is due (the hosted Worker refuses a
    /// transaction over 16 MiB), and one more for the surfaces'
    /// rows ([`crate::bot_surfaces::PRUNE_SURFACE_ROWS`] of each kind at
    /// the most). The runs are taken oldest end first, found from the
    /// rows they still hold (each table's index on `run_id`), not from
    /// every run that ever ended. What a pass leaves is due at the next,
    /// which takes it up where this one stopped: [`Pruned::more`] says
    /// whether it left any. Each transaction commits on its own, so a
    /// pass that fails keeps what its earlier transactions removed.
    ///
    /// # Errors
    /// The store's, after logging it: a pass that fails is never silent.
    pub fn prune_ended_runs(&self) -> Result<Pruned> {
        let pruned = self.prune_pass();
        if let Err(e) = &pruned {
            tracing::warn!(error = %e, "ClaxBot prune pass failed");
        }
        pruned
    }

    /// [`Store::prune_ended_runs`] again while the pass before left more
    /// ([`Pruned::more`]): `passes` at the most (one at the least), and no
    /// pass after the first begun at or after `until` by the store's
    /// clock. What a caller that wakes now and then runs, so that a
    /// backlog drains a bounded number of passes at each wake-up, and the
    /// caller wakes again soon while the last says more. The sum of what
    /// the passes removed (its `more` the last pass's), and how many
    /// passes ran.
    ///
    /// # Errors
    /// The store's, after logging it; the passes before it keep what
    /// they removed.
    pub fn prune_ended_runs_while(
        &self,
        passes: usize,
        until: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(Pruned, usize)> {
        let mut total = Pruned::default();
        let mut ran = 0;
        loop {
            let pass = self.prune_ended_runs()?;
            ran += 1;
            total.add(&pass);
            total.registrations += pass.registrations;
            total.surface_defs += pass.surface_defs;
            total.surface_prompts += pass.surface_prompts;
            total.blobs += pass.blobs;
            total.provider_failures += pass.provider_failures;
            total.more = pass.more;
            if !pass.more || ran >= passes || until.is_some_and(|u| self.now_utc() >= u) {
                return Ok((total, ran));
            }
        }
    }

    fn prune_pass(&self) -> Result<Pruned> {
        let now = self.now_utc();
        let before = |keep: std::time::Duration| {
            clax_core::time::stamp(
                now - chrono::Duration::from_std(keep).unwrap_or(chrono::Duration::MAX),
            )
        };
        let cuts = PruneCuts {
            journal: before(BOT_JOURNAL_KEEP),
            draft: before(BOT_DRAFT_KEEP),
            ends: before(crate::bot_pending::BOT_END_KEEP),
        };
        let mut pruned = Pruned::default();
        let mut released = Vec::new();
        let capacity = PRUNE_RUNS_PER_TX * PRUNE_TXS_PER_PASS;
        let mut due = self.with_read(|c| due_runs_in(c, &cuts, capacity + 1))?;
        if due.len() > capacity {
            due.truncate(capacity);
            pruned.more = true;
        }
        let (mut next, mut ends_left) = (0, true);
        for _ in 0..PRUNE_TXS_PER_PASS {
            if next >= due.len() && !ends_left {
                break;
            }
            let (taken, ends_more, batch, freed) = self.with_tx(|tx| {
                let mut batch = Pruned::default();
                let mut freed = Vec::new();
                let mut budget = PRUNE_ROWS_PER_TX;
                let mut taken = 0;
                for run in due[next..].iter().take(PRUNE_RUNS_PER_TX) {
                    if !prune_run_in(tx, run, &cuts, &mut budget, &mut batch, &mut freed)? {
                        break;
                    }
                    taken += 1;
                }
                let mut ends_more = ends_left;
                if ends_left && budget > 0 {
                    batch.run_ends = delete_some(
                        tx,
                        "DELETE FROM bot_run_ends WHERE rowid IN
                           (SELECT rowid FROM bot_run_ends WHERE created_at < ?1 LIMIT ?2)",
                        &cuts.ends,
                        budget,
                    )?;
                    ends_more = batch.run_ends == budget;
                }
                Ok((taken, ends_more, batch, freed))
            })?;
            next += taken;
            ends_left = ends_more;
            pruned.add(&batch);
            released.extend(freed);
        }
        pruned.more |= next < due.len() || ends_left;
        let surfaces = self.with_tx(|tx| {
            crate::bot_surfaces::prune_surfaces_in(
                tx,
                &before(crate::bot_surfaces::BOT_REGISTRATION_KEEP),
                &before(crate::bot_surfaces::BOT_DEF_KEEP),
            )
        })?;
        pruned.registrations = surfaces.registrations;
        pruned.surface_defs = surfaces.defs;
        pruned.surface_prompts = surfaces.prompts;
        pruned.more |= surfaces.more;
        // The provider failure log's entries past their age, as many as one
        // transaction of runs' rows may delete.
        pruned.provider_failures = self.with_tx(|tx| {
            crate::provider_failures::prune_aged_in(
                tx,
                &crate::provider_failures::aged_cut(now),
                PRUNE_ROWS_PER_TX,
            )
        })?;
        pruned.more |= pruned.provider_failures == PRUNE_ROWS_PER_TX;
        pruned.blobs = self.release_blobs(released)?;
        Ok(pruned)
    }
}

/// The most runs one transaction of a prune pass takes
/// ([`Store::prune_ended_runs`]).
pub const PRUNE_RUNS_PER_TX: usize = 32;

/// The most rows one transaction of a prune pass deletes, of all the
/// runs' tables together ([`Store::prune_ended_runs`]): a deleted row
/// writes its table's page and one page of each index at the most, five
/// for the most indexed table, so a transaction writes a few thousand
/// pages at the most whatever is due.
pub const PRUNE_ROWS_PER_TX: usize = 512;

/// The most transactions of runs' rows one prune pass makes
/// ([`Store::prune_ended_runs`]).
pub const PRUNE_TXS_PER_PASS: usize = 64;

/// The times before which a prune removes each kind of an ended run's
/// rows ([`Store::prune_ended_runs`]), as stamps.
struct PruneCuts {
    journal: String,
    draft: String,
    ends: String,
}

/// A run with rows due for pruning, and when it ended.
struct DueRun {
    id: String,
    ended_at: String,
}

/// Up to `limit` ended runs that hold a row due under `cuts`, the oldest
/// end first, on `c`. They are found from the rows the runs' tables hold
/// (each through its index on `run_id`), each looked up by its ID, so the
/// read is of what is held, not of every run that ever ended.
fn due_runs_in(c: &Connection, cuts: &PruneCuts, limit: usize) -> Result<Vec<DueRun>> {
    let mut q = c.prepare(
        "WITH held(run_id, cut) AS (
             SELECT run_id, ?1 FROM bot_calls
             UNION SELECT run_id, ?1 FROM bot_web_pages
             UNION SELECT run_id, ?1 FROM bot_pending_calls
             UNION SELECT run_id, ?1 FROM bot_content
             UNION SELECT run_id, ?1 FROM bot_messages WHERE delivered_at IS NOT NULL
             UNION SELECT run_id, ?3 FROM bot_messages WHERE delivered_at IS NULL
             UNION SELECT run_id, ?2 FROM bot_draft_files)
         SELECT r.id, r.ended_at FROM held CROSS JOIN bot_runs r
          WHERE r.id = held.run_id AND r.ended_at IS NOT NULL AND r.ended_at < held.cut
          GROUP BY r.id ORDER BY r.ended_at, r.id LIMIT ?4",
    )?;
    let rows = q.query_map(
        params![
            cuts.journal,
            cuts.draft,
            cuts.ends,
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |r| {
            Ok(DueRun {
                id: r.get(0)?,
                ended_at: r.get(1)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Runs `delete`, whose `?1` is `arg` and `?2` the most rows it may
/// delete, on `c`, and answers how many it deleted.
fn delete_some(c: &Connection, delete: &str, arg: &str, limit: usize) -> Result<usize> {
    Ok(c.prepare_cached(delete)?
        .execute(params![arg, i64::try_from(limit).unwrap_or(i64::MAX)])?)
}

/// Deletes, on `c`, the rows of `run` due under `cuts`, `budget` rows at
/// the most, counting each kind in `pruned` and the blobs of content
/// rows no row names any more in `released`. Whether it deleted every
/// row of the run that is due: `false` when `budget` ran out first, which
/// leaves the rest for a later transaction.
fn prune_run_in(
    c: &Connection,
    run: &DueRun,
    cuts: &PruneCuts,
    budget: &mut usize,
    pruned: &mut Pruned,
    released: &mut Vec<String>,
) -> Result<bool> {
    /// Counts `n` rows deleted in `count` and from `budget`: whether any
    /// of the budget is left.
    fn take(count: &mut usize, n: usize, budget: &mut usize) -> bool {
        *count += n;
        *budget = budget.saturating_sub(n);
        *budget > 0
    }
    let journal = run.ended_at < cuts.journal;
    if journal {
        for (delete, count) in [
            (
                "DELETE FROM bot_calls WHERE rowid IN
                   (SELECT rowid FROM bot_calls WHERE run_id = ?1 LIMIT ?2)",
                &mut pruned.calls,
            ),
            (
                "DELETE FROM bot_web_pages WHERE run_id = ?1 AND url IN
                   (SELECT url FROM bot_web_pages WHERE run_id = ?1 LIMIT ?2)",
                &mut pruned.pages,
            ),
            (
                "DELETE FROM bot_pending_calls WHERE rowid IN
                   (SELECT rowid FROM bot_pending_calls WHERE run_id = ?1 LIMIT ?2)",
                &mut pruned.pending_calls,
            ),
        ] {
            let n = delete_some(c, delete, &run.id, *budget)?;
            if !take(count, n, budget) {
                return Ok(false);
            }
        }
        let (n, freed) = crate::bot_content::prune_run_content_in(c, &run.id, *budget)?;
        released.extend(freed);
        if !take(&mut pruned.contents, n, budget) {
            return Ok(false);
        }
        let all = run.ended_at < cuts.ends;
        let n = crate::bot_messages::prune_run_in(c, &run.id, all, *budget)?;
        if !take(&mut pruned.messages, n, budget) {
            return Ok(false);
        }
    }
    if run.ended_at < cuts.draft {
        let n = delete_some(
            c,
            "DELETE FROM bot_draft_files WHERE rowid IN
               (SELECT rowid FROM bot_draft_files WHERE run_id = ?1 LIMIT ?2)",
            &run.id,
            *budget,
        )?;
        if !take(&mut pruned.draft_files, n, budget) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// How long an ended run's call journal is kept: the journal answers a
/// step's calls made again, so it outlives any driver that lost the run
/// and may still be running one of its steps (a run's wall time is capped
/// far below this).
pub const BOT_JOURNAL_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// How long an ended run's draft is kept, viewable from its run (spec
/// 2026-10-21-claxbot-design §6.3: a run stopped on its limits keeps its
/// draft).
pub const BOT_DRAFT_KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// What [`Store::prune_ended_runs`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Call journal rows.
    pub calls: usize,
    /// Page store rows.
    pub pages: usize,
    /// Draft overlay rows.
    pub draft_files: usize,
    /// Pending calls.
    pub pending_calls: usize,
    /// Run ends kept for a registration.
    pub run_ends: usize,
    /// Messages of runs' inboxes ([`crate::bot_messages`]).
    pub messages: usize,
    /// Surfaces' registrations that nothing needs.
    pub registrations: usize,
    /// Surfaces' definitions that nothing names.
    pub surface_defs: usize,
    /// Surfaces' prompt files that nothing names.
    pub surface_prompts: usize,
    /// Items of runs' content (`bot_content`).
    pub contents: usize,
    /// Blobs of content that no item names any more.
    pub blobs: usize,
    /// Entries of the provider failure log past their age
    /// ([`crate::provider_failures::FAILURE_KEEP`]).
    pub provider_failures: usize,
    /// The pass stopped at its bounds with rows still due: the next pass
    /// goes on where it stopped ([`Store::prune_ended_runs`]).
    pub more: bool,
}

impl Pruned {
    /// Adds `other`'s counts of the runs' rows to these.
    fn add(&mut self, other: &Pruned) {
        self.calls += other.calls;
        self.pages += other.pages;
        self.draft_files += other.draft_files;
        self.pending_calls += other.pending_calls;
        self.run_ends += other.run_ends;
        self.messages += other.messages;
        self.contents += other.contents;
    }
}

/// A store transaction a journaled write runs on ([`Store::with_bot_journal`]);
/// opaque outside the store, which keeps it for the `_in` write paths a
/// journaled write may call.
pub struct Tx<'a>(#[allow(dead_code)] pub(crate) &'a Connection);

/// A write's answer as its call journal recorded it: made only by the
/// store's journaled write paths, which record the entry in the write's
/// own transaction ([`Store::with_bot_journal`],
/// [`Store::add_comment_journaled`], [`Store::resolve_thread_journaled`],
/// [`Store::doc_set_journaled`] and its siblings,
/// [`Store::publish_version_journaled`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Journaled(pub(crate) Value);

impl Journaled {
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }

    #[must_use]
    pub fn into_value(self) -> Value {
        self.0
    }
}

/// A test hook that fails a journaled write after the write
/// ([`Store::set_journal_fault`]).
pub type JournalFault = std::sync::Arc<dyn Fn(&CallEntry) -> bool + Send + Sync>;

/// One entry of a step's call journal: a script's call `seq` (from 0)
/// made in attempt `attempt`, its op and the SHA-256 of its arguments, and
/// what it answered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallEntry {
    pub(crate) run_id: String,
    pub(crate) step_n: i64,
    pub(crate) seq: i64,
    pub(crate) attempt: i64,
    pub(crate) op: String,
    pub(crate) args_hash: String,
    pub(crate) ok: bool,
    /// The value, or `{code, message}` for a failure.
    pub(crate) result: Value,
}

impl CallEntry {
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The operation ID a write to an artifact's own store carries for
    /// this call ([`Store::app_write_begin`]): its place, the same on every
    /// attempt of the step, and its op and arguments as the digest.
    #[must_use]
    pub fn op_id(&self) -> crate::app_data::OpId {
        crate::app_data::OpId {
            id: format!("bot:{}:{}:{}", self.run_id, self.step_n, self.seq),
            digest: format!("{}:{}", self.op, self.args_hash),
        }
    }

    #[must_use]
    pub fn step_n(&self) -> i64 {
        self.step_n
    }

    #[must_use]
    pub fn seq(&self) -> i64 {
        self.seq
    }

    #[must_use]
    pub fn attempt(&self) -> i64 {
        self.attempt
    }

    #[must_use]
    pub fn op(&self) -> &str {
        &self.op
    }

    #[must_use]
    pub fn args_hash(&self) -> &str {
        &self.args_hash
    }

    #[must_use]
    pub fn ok(&self) -> bool {
        self.ok
    }

    #[must_use]
    pub fn result(&self) -> &Value {
        &self.result
    }

    /// The same call, answered `result` (`ok`, or a failure's `{code,
    /// message}`), for [`Store::record_bot_call`].
    #[must_use]
    pub fn answered(self, ok: bool, result: Value) -> CallEntry {
        CallEntry { ok, result, ..self }
    }
}

/// Run inside a journaled write's transaction before it commits (the
/// acting person may still write here, the run still acts): an error
/// rolls the write and its entry back.
pub type Guard<'a> = &'a (dyn Fn(&Tx<'_>) -> Result<()> + Send + Sync);

pub(crate) const CALL_COLS: &str = "run_id, step_n, seq, attempt, op, args_hash, ok, result_json";

/// A `bot_calls` row ([`CALL_COLS`]) as a [`CallEntry`].
///
/// # Errors
/// A conversion failure for a result that does not read as JSON
/// ([`stored_json`]): the journal never answers a call made again with
/// anything but what it recorded.
pub(crate) fn row_to_call(r: &Row<'_>) -> rusqlite::Result<CallEntry> {
    Ok(CallEntry {
        run_id: r.get(0)?,
        step_n: r.get(1)?,
        seq: r.get(2)?,
        attempt: r.get(3)?,
        op: r.get(4)?,
        args_hash: r.get(5)?,
        ok: r.get(6)?,
        result: stored_json(r, 7, "bot_calls.result_json")?,
    })
}

pub(crate) fn call_in(c: &Connection, e: &CallEntry) -> Result<Option<CallEntry>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {CALL_COLS} FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3"
    ))?
    .query_row(params![e.run_id, e.step_n, e.seq], row_to_call)
    .optional()?)
}

/// Whether the journal holds a row at call `seq` of step `step_n` of
/// run `run_id`, whether or not its result reads.
pub(crate) fn journal_holds(c: &Connection, run_id: &str, step_n: i64, seq: i64) -> Result<bool> {
    Ok(c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3)",
    )?
    .query_row(params![run_id, step_n, seq], |r| r.get(0))?)
}

/// The journal's entry at call `seq` of step `step_n` of run `run_id`,
/// when it holds one.
pub(crate) fn journal_at(
    c: &Connection,
    run_id: &str,
    step_n: i64,
    seq: i64,
) -> Result<Option<CallEntry>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {CALL_COLS} FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3"
    ))?
    .query_row(params![run_id, step_n, seq], row_to_call)
    .optional()?)
}

/// Journals `e` at `at` on `c`. A pending call of its place
/// ([`crate::bot_pending`]) stops waiting with it: once the place has
/// its row, whoever wrote it, an answer to that call is refused.
pub(crate) fn insert_call(c: &Connection, e: &CallEntry, at: &str) -> Result<()> {
    c.prepare_cached(
        "UPDATE bot_pending_calls SET answered_at = ?4
          WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3 AND answered_at IS NULL",
    )?
    .execute(params![e.run_id, e.step_n, e.seq, at])?;
    c.execute(
        "INSERT INTO bot_calls (run_id, step_n, seq, attempt, op, args_hash, ok, result_json, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            e.run_id,
            e.step_n,
            e.seq,
            e.attempt,
            e.op,
            e.args_hash,
            e.ok,
            e.result.to_string(),
            at
        ],
    )?;
    Ok(())
}

/// `key_reused` unless the journal's `prev` is call `e`: the same op on
/// the same arguments.
pub(crate) fn same_call(prev: &CallEntry, e: &CallEntry) -> Result<()> {
    if prev.op != e.op || prev.args_hash != e.args_hash {
        return Err(CoreError::invalid(
            "key_reused",
            format!(
                "call {} of step {} of run {} was {} with other arguments",
                e.seq, e.step_n, e.run_id, prev.op
            ),
        ));
    }
    Ok(())
}

/// The answer the journal's `prev` gives a call `e` at its key.
pub(crate) fn answer_of(prev: &CallEntry, e: &CallEntry) -> Result<Value> {
    same_call(prev, e)?;
    if prev.ok {
        Ok(prev.result.clone())
    } else {
        let code = match prev.result["code"].as_str() {
            Some("forbidden") => "forbidden",
            Some("conflict") => "conflict",
            Some("not_found") => "not_found",
            _ => "call_failed",
        };
        Err(CoreError::invalid(
            code,
            prev.result["message"].as_str().unwrap_or("").to_string(),
        ))
    }
}

/// A `bot.turn` record for a step commit: the model, the key it ran on,
/// the token counts and why the turn stopped (`credits_milli`, `run` and
/// `step` are filled at commit).
pub fn turn_record(
    at: impl Into<String>,
    model: &str,
    key_source: KeySource,
    input_tokens: i64,
    cached_tokens: i64,
    output_tokens: i64,
    stop_reason: &str,
) -> AuditRecord {
    AuditRecord::new(AuditKind::BotTurn, at)
        .with("model", model)
        .with("key_source", key_source.as_str())
        .with("input_tokens", input_tokens)
        .with("cached_tokens", cached_tokens)
        .with("output_tokens", output_tokens)
        .with("stop_reason", stop_reason)
}

/// A `bot.script` record: the script's title, the SHA-256 of its code
/// (the code itself is in the step's payload), the ops it called, its CPU
/// time, peak memory and error.
pub fn script_record(
    at: impl Into<String>,
    title: &str,
    code: &str,
    calls: &[String],
    cpu_ms: i64,
    mem_peak: i64,
    error: Option<&str>,
) -> AuditRecord {
    AuditRecord::new(AuditKind::BotScript, at)
        .with("title", title)
        .with(
            "code_sha256",
            format!("sha256:{}", clax_core::audit::sha256_hex(code.as_bytes())),
        )
        .with("calls", calls.to_vec())
        .with("cpu_ms", cpu_ms)
        .with("mem_peak", mem_peak)
        .with("error", error)
}

/// A `bot.call` record: one API op a script called, what it named, and
/// whether it was allowed.
pub fn call_record(at: impl Into<String>, op: &str, target: Option<&str>, ok: bool) -> AuditRecord {
    AuditRecord::new(AuditKind::BotCall, at)
        .with("op", op)
        .with("target", target)
        .with("ok", ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credits::{LINE_HARNESS_FEE, Unit, token_line};
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store, store_with_clock};
    use clax_core::audit::Actor;
    use clax_core::working::{Clock, ManualClock};
    use serde_json::json;
    use std::sync::Arc;

    fn bot() -> AuditCtx {
        AuditCtx {
            actor: Actor::Bot {
                on_behalf_of: OWNER.into(),
            },
            ..DAEMON.clone()
        }
    }

    fn new_run_on(st: &Store, model: &str) -> Result<Run> {
        let aid = artifact(st, None).to_string();
        st.create_run(
            &bot(),
            NewRun {
                artifact_id: aid,
                person: OWNER.into(),
                request: RequestKind::Thread,
                thread_ids: vec!["t1".into()],
                model: model.into(),
                key_source: KeySource::Instance,
                base_version: 1,
                agent_grant: None,
                web: String::new(),
            },
        )
    }

    fn new_run(st: &Store) -> Run {
        new_run_on(st, "claude-sonnet-5-5").unwrap()
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_run_is_claimed_once_and_resumes_its_uncommitted_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        assert_eq!(run.status, RunStatus::Queued);
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)),
            "run_not_claimed"
        );
        let held = st
            .claim_run(&run.id, &DriverId::named("d1"), 60)
            .unwrap()
            .unwrap();
        assert_eq!(held.status, RunStatus::Running);
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_none(),
            "held by d1"
        );
        assert!(
            st.claim_run(&run.id, &DriverId::named("d1"), 60)
                .unwrap()
                .is_some(),
            "renewed"
        );
        assert_eq!(st.live_runs().unwrap().len(), 1);
        assert_eq!(st.next_step_n(&run.id).unwrap(), 1);

        let s1 = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)
            .unwrap();
        assert_eq!(s1.n, 1);
        // The driver dies here. On restart the step is found again.
        st.release_run(&run.id, &DriverId::named("d1")).unwrap();
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_some()
        );
        let open = st.open_step(&run.id).unwrap().unwrap();
        assert_eq!((open.n, open.committed_at.clone()), (1, None));
        assert_eq!(st.next_step_n(&run.id).unwrap(), 1);
        let again = st
            .begin_step(&run.id, &DriverId::named("d2"), 1, StepKind::Turn)
            .unwrap();
        assert_eq!(again.n, 1, "the open step, not a new one");
        assert_eq!((s1.attempt, again.attempt), (1, 2), "begun again");
        st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("d2"),
            1,
            StepCommit {
                payload: json!({"messages": ["hi"]}),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(st.open_step(&run.id).unwrap().is_none());
        // A driver that lost track (it thinks step 1 is still open) is
        // refused, and so is one that skips ahead.
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d2"), 1, StepKind::Turn)),
            "step_mismatch"
        );
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d2"), 3, StepKind::Turn)),
            "step_mismatch"
        );
        assert_eq!(
            st.begin_step(&run.id, &DriverId::named("d2"), 2, StepKind::Script)
                .unwrap()
                .n,
            2
        );
        assert_eq!(
            code(st.commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d2"),
                1,
                StepCommit::default()
            )),
            "run_invalid",
            "committed"
        );
    }

    /// What `bot_runs` keeps of run `id` beside what [`Run`] reads: its
    /// kind, bindings and request.
    fn kept(st: &Store, id: &str) -> (String, Bindings, Value) {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT kind, bindings_json, request_json FROM bot_runs WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )?)
        })
        .map(|(kind, bindings, request)| {
            (
                kind,
                Bindings::parse(&bindings).expect("bindings"),
                serde_json::from_str(&request).expect("a request"),
            )
        })
        .unwrap()
    }

    #[test]
    fn a_new_run_keeps_its_kind_its_clax_binding_and_its_request() {
        let (_d, st) = store();
        let aid = artifact(&st, None).to_string();
        let make = |request, threads: &[&str], web: &str| {
            st.create_run(
                &bot(),
                NewRun {
                    artifact_id: aid.clone(),
                    person: OWNER.into(),
                    request,
                    thread_ids: threads.iter().map(|t| t.to_string()).collect(),
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    web: web.into(),
                },
            )
            .unwrap()
        };
        let binding = |granted: &[&str]| Bindings {
            v: 1,
            bindings: vec![StoredBinding {
                surface: "clax".into(),
                hash: None,
                registration: None,
                prompt: None,
                settings: None,
                scope: json!({"artifact": aid}),
                granted: granted.iter().map(|o| o.to_string()).collect(),
                operations: None,
            }],
        };

        let thread = make(RequestKind::Thread, &["t1", "t2"], "search,fetch");
        assert_eq!(
            kept(&st, &thread.id),
            (
                "clax.thread".to_string(),
                binding(&["web.search", "web.fetch"]),
                json!({"thread_ids": ["t1", "t2"]})
            )
        );
        let brief = make(RequestKind::Brief, &["t3"], "");
        assert_eq!(
            kept(&st, &brief.id),
            (
                "clax.brief".to_string(),
                binding(&[]),
                json!({"thread_ids": ["t3"]})
            )
        );
        assert_eq!(
            kept(&st, &make(RequestKind::Thread, &["t4"], "fetch").id).1,
            binding(&["web.fetch"])
        );
        assert_eq!(
            kept(&st, &make(RequestKind::Thread, &["t5"], "search").id).1,
            binding(&["web.search"])
        );
        // The run reads as it did: from the columns it always had.
        assert_eq!(
            (
                thread.request,
                thread.thread_ids.clone(),
                thread.web.as_str()
            ),
            (
                RequestKind::Thread,
                vec!["t1".to_string(), "t2".to_string()],
                "search,fetch"
            )
        );
        assert_eq!(
            (thread.artifact_id.as_str(), thread.base_version),
            (aid.as_str(), 1)
        );
        // The stored text, byte for byte.
        let text: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT bindings_json || ' ' || request_json FROM bot_runs WHERE id = ?1",
                    params![brief.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            text,
            format!(
                r#"{{"v":1,"bindings":[{{"surface":"clax","hash":null,"scope":{{"artifact":"{aid}"}},"granted":[]}}]}} {{"thread_ids":["t3"]}}"#
            )
        );
    }

    #[test]
    fn a_runs_status_may_be_parked_and_no_other_new_word() {
        let (_d, st) = store();
        let run = new_run(&st);
        let set = |status: &str| {
            st.with_write(|c| {
                c.execute(
                    "UPDATE bot_runs SET status = ?2 WHERE id = ?1",
                    params![run.id, status],
                )?;
                Ok(())
            })
        };
        set("parked").unwrap();
        let stored: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT status FROM bot_runs WHERE id = ?1",
                    params![run.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(stored, "parked");
        for word in ["napping", "Parked", ""] {
            assert!(set(word).is_err(), "{word:?} is no status");
        }
        for status in RunStatus::ALL {
            set(status.as_str()).unwrap();
        }
    }

    /// Fails unless `r` is the conversion failure of `column` holding
    /// `value`, showing what `r` was.
    #[track_caller]
    fn assert_unknown<T: std::fmt::Debug>(r: Result<T>, column: &str, value: &str) {
        let (names, holds) = (format!("bot_runs.{column}"), format!("{value:?}"));
        match &r {
            Err(CoreError::Db(rusqlite::Error::FromSqlConversionFailure(_, _, why)))
                if why.to_string().contains(&names) && why.to_string().contains(&holds) => {}
            other => panic!(
                "expected a conversion failure naming {names} and its value {holds}; \
                 got {other:?}"
            ),
        }
    }

    /// A status or a request kind the table holds and this build does not
    /// know is an error wherever the run is read, never another status or
    /// kind: a run a later build left waiting does not read as ended, nor
    /// a review as a thread.
    #[test]
    fn a_run_with_a_status_or_request_this_build_does_not_know_is_not_read() {
        let (_d, st) = store();
        let run = new_run(&st);
        let other = new_run(&st);
        // The table's CHECK takes only the statuses this build knows; a
        // later build's is written past it.
        let set = |column: &str, value: &str| {
            st.with_write(|c| {
                c.pragma_update(None, "ignore_check_constraints", true)?;
                let done = c.execute(
                    &format!("UPDATE bot_runs SET {column} = ?2 WHERE id = ?1"),
                    params![run.id, value],
                );
                c.pragma_update(None, "ignore_check_constraints", false)?;
                done?;
                Ok(())
            })
            .unwrap();
        };
        let threads = ["t1".to_string()];

        set("status", "napping");
        assert_unknown(st.run(&run.id), "status", "napping");
        assert_unknown(st.runs_for_person(OWNER, 10), "status", "napping");
        assert_unknown(
            st.claim_run(&run.id, &DriverId::named("d1"), 60),
            "status",
            "napping",
        );
        assert_unknown(st.cancel_run(&bot(), &run.id), "status", "napping");
        // No query counts it among the runs that have not ended, so those
        // do not read it; the run beside it still reads.
        assert_eq!(st.live_runs().unwrap(), std::slice::from_ref(&other));
        assert_eq!(st.active_run_on(&threads).unwrap(), Some(other.clone()));
        assert_eq!(st.run(&other.id).unwrap(), Some(other));

        set("status", "queued");
        assert_eq!(st.run(&run.id).unwrap().unwrap().status, RunStatus::Queued);
        set("request", "review");
        assert_unknown(st.run(&run.id), "request", "review");
        assert_unknown(st.runs_for_thread("t1", 10), "request", "review");
        assert_unknown(st.live_runs(), "request", "review");
        set("request", "brief");
        assert_eq!(
            st.run(&run.id).unwrap().unwrap().request,
            RequestKind::Brief
        );
    }

    /// A run is read with what it was made as: its kind, bindings and
    /// request as they are kept. A row with no artifact and no base version, of a kind that
    /// is not Clax's, is read: it has no artifact, its commit files the
    /// step's records and the run's end on the subject it is given, or
    /// on none, and a cancel of it names no artifact.
    #[test]
    fn a_run_without_an_artifact_is_read_and_its_records_are_filed_on_the_subject_given() {
        let (_d, st) = store();
        let run = new_run(&st);
        assert_eq!(run.artifact(), Some(run.artifact_id.as_str()));
        let (kind, bindings, request) = kept(&st, &run.id);
        let (row, made, steps) = st.run_with_steps_as_made(&run.id).unwrap().unwrap();
        assert_eq!((&row, steps.len()), (&run, 0));
        assert_eq!(made.kind, kind);
        assert_eq!(Bindings::parse(&made.bindings_json).unwrap(), bindings);
        assert_eq!(
            serde_json::from_str::<Value>(&made.request_json).unwrap(),
            request
        );
        assert_eq!(st.run_with_steps_as_made("no such run").unwrap(), None);

        let bindings =
            r#"{"v":1,"bindings":[{"surface":"shell","hash":null,"scope":null,"granted":[]}]}"#;
        st.with_write(|c| {
            c.execute(
                "UPDATE bot_runs SET artifact_id = NULL, base_version = NULL, kind = 'shell.triage',
                   bindings_json = ?2, request_json = '{\"ask\":\"x\"}' WHERE id = ?1",
                params![run.id, bindings],
            )?;
            Ok(())
        })
        .unwrap();
        let read = st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (
                read.artifact(),
                read.artifact_id.as_str(),
                read.base_version
            ),
            (None, "", 0)
        );
        let (row, made, _) = st.run_with_steps_as_made(&run.id).unwrap().unwrap();
        assert_eq!(row, read);
        assert_eq!(
            made,
            RunAsMade {
                kind: "shell.triage".into(),
                bindings_json: bindings.into(),
                request_json: r#"{"ask":"x"}"#.into(),
            }
        );
        assert_eq!(st.live_runs().unwrap(), std::slice::from_ref(&read));

        // Its steps commit: with no subject, on no artifact; with one, on
        // that, an ID the record has of its own kept.
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap().unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Turn).unwrap();
        let mut own = turn_record(st.now(), "m", KeySource::Instance, 0, 0, 0, "end_turn");
        own.ids.thread = Some("the records own".into());
        let commit = |events| StepCommit {
            payload: json!({}),
            events,
            ..StepCommit::default()
        };
        st.commit_bot_step(&bot(), &run.id, &d, 1, commit(vec![own.clone()]))
            .unwrap();
        st.begin_step(&run.id, &d, 2, StepKind::Turn).unwrap();
        let elsewhere = artifact(&st, None).to_string();
        let subject = AuditIds {
            artifact: Some(elsewhere.clone()),
            thread: Some("the subjects".into()),
            origin: Some("the subjects".into()),
            ..AuditIds::default()
        };
        st.commit_bot_step_on(
            &bot(),
            &run.id,
            &d,
            2,
            commit(vec![own]),
            Some(&subject),
            &[],
        )
        .unwrap();
        st.begin_step(&run.id, &d, 3, StepKind::GiveUp).unwrap();
        st.commit_bot_step_on(
            &bot(),
            &run.id,
            &d,
            3,
            commit(Vec::new()),
            Some(&subject),
            &[],
        )
        .unwrap();
        let other = new_run(&st);
        st.with_write(|c| {
            c.execute(
                "UPDATE bot_runs SET artifact_id = NULL, base_version = NULL WHERE id = ?1",
                params![other.id],
            )?;
            Ok(())
        })
        .unwrap();
        st.cancel_run(&bot(), &other.id).unwrap();
        let filed: Vec<(String, [Option<String>; 3])> = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| matches!(e.kind.as_str(), "bot.turn" | "bot.run_end" | "bot.cancel"))
            .map(|e| (e.kind, [e.ids.artifact, e.ids.thread, e.ids.origin]))
            .collect();
        let s = |text: &str| Some(text.to_string());
        let there = Some(elsewhere);
        assert_eq!(
            filed,
            [
                ("bot.turn".to_string(), [None, s("the records own"), None]),
                (
                    "bot.turn".to_string(),
                    [there.clone(), s("the records own"), s("the subjects")]
                ),
                (
                    "bot.run_end".to_string(),
                    [there, s("the subjects"), s("the subjects")]
                ),
                ("bot.cancel".to_string(), [None, None, None]),
            ]
        );
    }

    #[test]
    fn bindings_are_read_only_at_the_version_this_build_knows() {
        let made = Bindings::clax("7q3k9mzx2b4t", "search,fetch");
        let text = made.to_json();
        assert_eq!(
            text,
            r#"{"v":1,"bindings":[{"surface":"clax","hash":null,"scope":{"artifact":"7q3k9mzx2b4t"},"granted":["web.search","web.fetch"]}]}"#
        );
        let read = Bindings::parse(&text).unwrap();
        assert_eq!(read, made);
        let clax = read.of("clax").expect("the clax binding");
        assert_eq!(
            (clax.surface(), clax.hash(), clax.scope(), clax.granted()),
            (
                "clax",
                None,
                &json!({"artifact": "7q3k9mzx2b4t"}),
                &["web.search".to_string(), "web.fetch".to_string()][..]
            )
        );
        assert!(read.of("shell").is_none());
        assert_eq!(read.all(), std::slice::from_ref(clax));
        assert!(
            Bindings::clax("a", "fetch,other,Search")
                .of("clax")
                .unwrap()
                .granted()
                == ["web.fetch"]
        );
        // A field this build does not know is ignored.
        let more = text.replace(r#""hash":null"#, r#""hash":"ab12","reader_view":"none""#);
        assert_eq!(
            Bindings::parse(&more).unwrap().of("clax").unwrap().hash(),
            Some("ab12")
        );

        // A scope is the surface's own value: `null` is one.
        let null_scope = text.replace(r#"{"artifact":"7q3k9mzx2b4t"}"#, "null");
        assert_eq!(
            Bindings::parse(&null_scope)
                .unwrap()
                .of("clax")
                .unwrap()
                .scope(),
            &Value::Null
        );
        // A repeated key is read as its last value, as serde_json reads it.
        assert_eq!(
            Bindings::parse(&text.replace(r#"{"v":1,"#, r#"{"v":2,"v":1,"#)).unwrap(),
            made
        );
    }

    /// Each text [`Bindings::parse`] refuses, with what its message says
    /// is wrong with it.
    #[test]
    fn bindings_that_are_refused_say_what_is_wrong_with_them() {
        let binding = r#"{"surface":"clax","hash":null,"scope":{},"granted":[]}"#;
        let one = |b: &str| format!(r#"{{"v":1,"bindings":[{b}]}}"#);
        let versioned = |v: &str| format!(r#"{{"v":{v},"bindings":[{binding}]}}"#);
        let cases: Vec<(String, &str)> = vec![
            ("not json".into(), "bindings are not JSON"),
            ("".into(), "bindings are not JSON"),
            ("[]".into(), "bindings are not a JSON object: []"),
            ("null".into(), "bindings are not a JSON object: null"),
            (r#""v""#.into(), "bindings are not a JSON object"),
            (
                format!(r#"{{"bindings":[{binding}]}}"#),
                "bindings without a version (`v`)",
            ),
            (
                versioned("3"),
                "bindings of version 3; this build reads versions 1 and 2",
            ),
            (
                versioned("0"),
                "bindings of version 0; this build reads versions 1 and 2",
            ),
            // A repeated key is read as its last value.
            (
                format!(r#"{{"v":1,"v":3,"bindings":[{binding}]}}"#),
                "bindings of version 3",
            ),
            (
                versioned("1.0"),
                "version (`v`) is 1.0, which is not a whole number",
            ),
            (
                versioned("-1"),
                "version (`v`) is -1, which is not a whole number",
            ),
            (
                versioned(r#""1""#),
                r#"version (`v`) is "1", which is not a whole number"#,
            ),
            (
                versioned("null"),
                "version (`v`) is null, which is not a whole number",
            ),
            (r#"{"v":1}"#.into(), "bindings without `bindings`"),
            (
                r#"{"v":1,"bindings":{}}"#.into(),
                "`bindings` is not a list: {}",
            ),
            (
                r#"{"v":1,"bindings":null}"#.into(),
                "`bindings` is not a list: null",
            ),
            (
                r#"{"v":1,"bindings":[]}"#.into(),
                "a run has at least one binding",
            ),
            (
                one(r#""clax""#),
                r#"binding 0 is not a JSON object: "clax""#,
            ),
            (
                format!(r#"{{"v":1,"bindings":[{binding},[]]}}"#),
                "binding 1 is not a JSON object: []",
            ),
            (
                one(r#"{"hash":null,"scope":{},"granted":[]}"#),
                "binding 0 has no `surface`",
            ),
            (
                one(r#"{"surface":"clax","scope":{},"granted":[]}"#),
                "binding 0 has no `hash`",
            ),
            (
                one(r#"{"surface":"clax","hash":null,"granted":[]}"#),
                "binding 0 has no `scope`",
            ),
            (
                one(r#"{"surface":"clax","hash":null,"scope":{}}"#),
                "binding 0 has no `granted`",
            ),
            // Under version 1 the name `operations` would be read as the
            // run's whole set, which `granted` is not: a version-1 binding
            // spelled with it has no `granted`, and is refused.
            (
                one(r#"{"surface":"clax","hash":null,"scope":{},"operations":[]}"#),
                "binding 0 has no `granted`",
            ),
            (
                one(r#"{"surface":"","hash":null,"scope":{},"granted":[]}"#),
                "binding 0 has an empty surface name",
            ),
            (
                one(r#"{"surface":7,"hash":null,"scope":{},"granted":[]}"#),
                "binding 0 has a `surface` that is not text: 7",
            ),
            (
                one(r#"{"surface":null,"hash":null,"scope":{},"granted":[]}"#),
                "binding 0 has a `surface` that is not text: null",
            ),
            (
                one(r#"{"surface":"clax","hash":12,"scope":{},"granted":[]}"#),
                "the binding of surface clax has a `hash` that is neither text nor null: 12",
            ),
            (
                one(r#"{"surface":"clax","hash":null,"scope":{},"granted":"web.fetch"}"#),
                r#"the binding of surface clax has a `granted` that is not a list of names: "web.fetch""#,
            ),
            (
                one(r#"{"surface":"clax","hash":null,"scope":{},"granted":["web.fetch",1]}"#),
                r#"the binding of surface clax has a `granted` that is not a list of names: ["web.fetch",1]"#,
            ),
            (
                one(r#"{"surface":"clax","hash":null,"scope":{},"granted":null}"#),
                "the binding of surface clax has a `granted` that is not a list of names: null",
            ),
            (
                one(
                    r#"{"surface":"clax","hash":null,"scope":{},"granted":["web.fetch","web.search","web.fetch"]}"#,
                ),
                "the binding of surface clax is granted web.fetch twice",
            ),
            (
                format!(r#"{{"v":1,"bindings":[{binding},{binding}]}}"#),
                "two bindings of surface clax",
            ),
        ];
        for (json, says) in cases {
            match Bindings::parse(&json) {
                Err(CoreError::Invalid { code, message }) => {
                    assert_eq!(code, "bindings_invalid", "the code refusing {json}");
                    assert!(
                        message.contains(says),
                        "{json} was expected to be refused saying {says:?}; it was refused \
                         saying {message:?}"
                    );
                }
                other => panic!(
                    "{json} was expected to be refused saying {says:?}; parse returned {other:?}"
                ),
            }
        }
    }

    /// A run keeps the artifact it was made on after the artifact is
    /// deleted, and reads back; no new run is made on a deleted artifact.
    #[test]
    fn a_run_reads_back_after_its_artifact_is_deleted() {
        let (_d, st) = store();
        let run = new_run(&st);
        let aid = clax_core::ArtifactId::parse(&run.artifact_id).unwrap();
        st.delete_artifact(DAEMON, &aid).unwrap();
        assert_eq!(st.run(&run.id).unwrap(), Some(run.clone()));
        assert_eq!(
            st.runs_for_artifact(&run.artifact_id, 10).unwrap(),
            std::slice::from_ref(&run)
        );
        assert_eq!(st.live_runs().unwrap(), std::slice::from_ref(&run));
        assert_eq!(
            kept(&st, &run.id).1.bindings[0].scope,
            json!({"artifact": run.artifact_id})
        );
        let again = st.create_run(
            &bot(),
            NewRun {
                artifact_id: run.artifact_id.clone(),
                person: OWNER.into(),
                request: RequestKind::Thread,
                thread_ids: vec!["t2".into()],
                model: "claude-sonnet-5-5".into(),
                key_source: KeySource::Instance,
                base_version: 1,
                agent_grant: None,
                web: String::new(),
            },
        );
        assert!(matches!(again, Err(CoreError::NotFound)), "{again:?}");
    }

    /// An ended run's call journal goes a day after it ended and its draft
    /// thirty days after; a run still going keeps both.
    #[test]
    fn ended_runs_lose_their_journal_then_their_draft() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let d = DriverId::named("d1");
        let work = |run: &Run, kind: StepKind| {
            st.claim_run(&run.id, &d, 60).unwrap().unwrap();
            let n = st.next_step_n(&run.id).unwrap();
            st.begin_step(&run.id, &d, n, kind).unwrap();
            let entry = st
                .call_entry(&run.id, &d, n, 1, 0, "threads.read", "h")
                .unwrap();
            st.record_bot_call(&entry).unwrap();
            st.commit_bot_step(
                &bot(),
                &run.id,
                &d,
                n,
                StepCommit {
                    draft: vec![DraftChange::Write {
                        path: "index.html".into(),
                        content: b"<p>".to_vec(),
                        content_type: None,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        };
        let (ended, going) = (new_run(&st), new_run(&st));
        work(&ended, StepKind::Turn);
        work(&ended, StepKind::GiveUp);
        work(&going, StepKind::Turn);
        let calls = |run: &Run| st.bot_calls(&run.id, 1).unwrap().len();
        let drafts = |run: &Run| st.draft_files(&run.id).unwrap().len();
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());

        clock.advance(BOT_JOURNAL_KEEP.as_secs() as i64 + 1);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.draft_files), (2, 0));
        assert_eq!((calls(&ended), drafts(&ended)), (0, 1));
        assert_eq!((calls(&going), drafts(&going)), (1, 1));

        clock.advance(BOT_DRAFT_KEEP.as_secs() as i64);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.draft_files), (0, 1));
        assert_eq!(drafts(&ended), 0);
        assert_eq!((calls(&going), drafts(&going)), (1, 1));
        assert_eq!(
            st.run(&ended.id).unwrap().unwrap().status,
            RunStatus::GaveUp
        );
    }

    /// Writes `runs` finished runs, numbered from `from`, each holding
    /// `calls` rows of call journal, the `i`th ended `i` seconds after
    /// 2026-10-01T00:00:00Z.
    fn seed_ended_runs(st: &Store, from: usize, runs: usize, calls: usize) {
        st.with_write(|c| {
            c.execute(
                "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                        model, key_source, base_version, created_at, updated_at, ended_at)
                 WITH RECURSIVE n(i) AS (SELECT ?1 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?1 + ?2)
                 SELECT printf('01JC%022d', i), 'a', 'u', 'thread', '[]', 'finished',
                        'claude-sonnet-5-5', 'instance', 1, '2026-10-01T00:00:00.000Z',
                        '2026-10-01T00:00:00.000Z',
                        strftime('%Y-%m-%dT%H:%M:%S.000Z', '2026-10-01T00:00:00Z', '+' || i || ' seconds')
                   FROM n",
                params![from as i64, runs as i64],
            )?;
            c.execute(
                "INSERT INTO bot_calls (run_id, step_n, seq, attempt, op, args_hash, ok,
                        result_json, at)
                 WITH RECURSIVE n(i) AS (SELECT ?1 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?1 + ?2),
                      s(j) AS (SELECT 0 UNION ALL SELECT j + 1 FROM s WHERE j + 1 < ?3)
                 SELECT printf('01JC%022d', i), 1, j, 1, 'threads.read', 'h', 1, '{}',
                        '2026-10-01T00:00:00.000Z'
                   FROM n, s",
                params![from as i64, runs as i64, calls as i64],
            )?;
            Ok(())
        })
        .unwrap();
    }

    /// A pass prunes [`PRUNE_TXS_PER_PASS`] transactions of
    /// [`PRUNE_RUNS_PER_TX`] runs at the most, the oldest end first, and
    /// says it left more; the next pass takes up the rest.
    #[test]
    fn a_prune_pass_is_bounded_and_the_next_takes_up_where_it_stopped() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let capacity = PRUNE_RUNS_PER_TX * PRUNE_TXS_PER_PASS;
        seed_ended_runs(&st, 0, capacity + 10, 1);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.more), (capacity, true));
        let held = |i: usize| st.bot_calls(&format!("01JC{i:022}"), 1).unwrap().len();
        assert_eq!(
            (held(0), held(capacity - 1)),
            (0, 0),
            "the oldest went first"
        );
        assert_eq!((held(capacity), held(capacity + 9)), (1, 1));
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.more), (10, false));
        assert_eq!(held(capacity + 9), 0);
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());
    }

    /// Passes run one after another while each leaves more, as many as
    /// the caller allows, and none after the first once the store's clock
    /// reaches the caller's bound.
    #[test]
    fn passes_go_on_while_one_leaves_more_within_their_count_and_their_time() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let capacity = PRUNE_RUNS_PER_TX * PRUNE_TXS_PER_PASS;
        seed_ended_runs(&st, 0, capacity * 2 + 10, 1);
        // The clock's bound reached: one pass, which says more.
        let (p, ran) = st.prune_ended_runs_while(8, Some(clock.now())).unwrap();
        assert_eq!((p.calls, p.more, ran), (capacity, true, 1));
        // Its count reached: one pass more, still more.
        let (p, ran) = st.prune_ended_runs_while(1, None).unwrap();
        assert_eq!((p.calls, p.more, ran), (capacity, true, 1));
        // Room for more: it goes on until a pass leaves nothing.
        let (p, ran) = st
            .prune_ended_runs_while(8, Some(clock.now() + chrono::Duration::hours(1)))
            .unwrap();
        assert_eq!((p.calls, p.more, ran), (10, false, 1));
        seed_ended_runs(&st, capacity * 3, capacity + 1, 1);
        let (p, ran) = st.prune_ended_runs_while(8, None).unwrap();
        assert_eq!((p.calls, p.more, ran), (capacity + 1, false, 2));
    }

    /// A run that holds more rows than one transaction deletes is pruned
    /// across transactions of one pass, each of [`PRUNE_ROWS_PER_TX`] rows
    /// at the most, and a run whose rows are not due is left as it is.
    #[test]
    fn a_run_with_more_rows_than_a_transaction_takes_is_pruned_across_several() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let rows = PRUNE_ROWS_PER_TX * 2 + 7;
        seed_ended_runs(&st, 0, 1, rows);
        let going = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&going.id, &d, 60).unwrap().unwrap();
        st.begin_step(&going.id, &d, 1, StepKind::Turn).unwrap();
        let entry = st
            .call_entry(&going.id, &d, 1, 1, 0, "threads.read", "h")
            .unwrap();
        st.record_bot_call(&entry).unwrap();
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.more), (rows, false));
        assert!(
            st.bot_calls(&format!("01JC{:022}", 0), 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(st.bot_calls(&going.id, 1).unwrap().len(), 1);
    }

    #[test]
    fn a_driver_whose_lease_lapsed_cannot_commit_over_the_one_that_took_over() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("a"), 60)
            .unwrap()
            .unwrap();
        st.begin_step(&run.id, &DriverId::named("a"), 1, StepKind::Turn)
            .unwrap();
        // A long turn renews its lease and keeps the run.
        clock.advance(50);
        assert!(st.renew_lease(&run.id, &DriverId::named("a")).unwrap());
        clock.advance(50);
        assert!(
            st.claim_run(&run.id, &DriverId::named("b"), 60)
                .unwrap()
                .is_none(),
            "renewed 50 s ago"
        );
        // A stalled driver's lease lapses; another takes the run over and
        // runs the open step again.
        clock.advance(11);
        assert!(
            st.claim_run(&run.id, &DriverId::named("b"), 60)
                .unwrap()
                .is_some()
        );
        assert!(
            !st.renew_lease(&run.id, &DriverId::named("a")).unwrap(),
            "a has lost it"
        );
        st.begin_step(&run.id, &DriverId::named("b"), 1, StepKind::Turn)
            .unwrap();
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        let late = st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("a"),
            1,
            StepCommit {
                payload: json!({"by": "a"}),
                usage: vec![Metered::new(&line, 1_000, KeySource::Instance)],
                ..Default::default()
            },
        );
        assert_eq!(code(late), "run_not_claimed");
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("a"), 1, StepKind::Turn)),
            "run_not_claimed"
        );
        let won = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("b"),
                1,
                StepCommit {
                    payload: json!({"by": "b"}),
                    usage: vec![Metered::new(&line, 1_000, KeySource::Instance)],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(won.step.payload, Some(json!({"by": "b"})));
        assert_eq!(won.usage.len(), 1, "charged once");
        assert_eq!(st.usage_rows(OWNER, st.now_utc()).unwrap().len(), 1);
    }

    #[test]
    fn a_model_without_prices_cannot_run() {
        let (_d, st) = store();
        // At the default rates any model runs; with explicit prices
        // required, only one with its cost lines.
        new_run_on(&st, "custom-model").unwrap();
        st.set_bot_settings(
            DAEMON,
            &[(
                crate::bot_settings::Setting::RequireExplicitPrice,
                Some(json!(true)),
            )],
        )
        .unwrap();
        assert_eq!(code(new_run_on(&st, "other-model")), "model_not_priced");
        // A model whose lines go away while a run waits is refused at its
        // next turn, before anything is asked of the provider.
        let run = new_run_on(&st, "claude-opus-5-5").unwrap();
        st.with_write(|c| {
            c.execute(
                "DELETE FROM cost_lines WHERE id = 'claude-opus-5-5.output'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        st.claim_run(&run.id, &DriverId::named("d"), 60).unwrap();
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d"), 1, StepKind::Turn)),
            "model_not_priced"
        );
        // Its stop is still possible.
        st.cancel_run(DAEMON, &run.id).unwrap();
        st.begin_step(&run.id, &DriverId::named("d"), 1, StepKind::Stopped)
            .unwrap();
    }

    #[test]
    fn an_own_key_run_is_never_refused_for_a_missing_price() {
        use crate::secrets::{Kek, SecretSlot};
        let (_d, st) = store();
        st.set_bot_settings(
            DAEMON,
            &[(
                crate::bot_settings::Setting::RequireExplicitPrice,
                Some(json!(true)),
            )],
        )
        .unwrap();
        let aid = artifact(&st, None).to_string();
        let new = |key_source| NewRun {
            artifact_id: aid.clone(),
            person: OWNER.into(),
            request: RequestKind::Thread,
            thread_ids: vec!["t1".into()],
            model: "custom-model".into(),
            key_source,
            base_version: 1,
            agent_grant: None,
            web: String::new(),
        };
        assert_eq!(
            code(st.create_run(&bot(), new(KeySource::Instance))),
            "model_not_priced"
        );
        // On the person's own key its token rows are waived: it runs.
        let run = st.create_run(&bot(), new(KeySource::Own)).unwrap();
        let d = DriverId::named("d");
        st.claim_run(&run.id, &d, 60).unwrap();
        // A turn is checked on the key its caller chose, whatever the
        // person's stored key says: with their own key in use, a turn the
        // caller runs on the instance key (its own key could not be
        // opened) is refused, as is one begun without a key.
        st.set_secret(
            DAEMON,
            &Kek::from_bytes([7; 32]),
            &SecretSlot::Own(OWNER.into()),
            "sk-ant-api03-OWNKEYOWNKEYOWN-wxyz",
        )
        .unwrap();
        assert!(
            st.secret_meta(&SecretSlot::Own(OWNER.into()))
                .unwrap()
                .unwrap()
                .use_own
        );
        assert_eq!(
            code(st.begin_step_on(&run.id, &d, 1, StepKind::Turn, KeySource::Instance)),
            "model_not_priced"
        );
        assert_eq!(
            code(st.begin_step(&run.id, &d, 1, StepKind::Turn)),
            "model_not_priced"
        );
        st.begin_step_on(&run.id, &d, 1, StepKind::Turn, KeySource::Own)
            .unwrap();
        // Another step is not checked for a price.
        st.cancel_run(DAEMON, &run.id).unwrap();
        st.begin_step_on(&run.id, &d, 1, StepKind::Stopped, KeySource::Instance)
            .unwrap();
    }

    #[test]
    fn a_step_commits_its_usage_draft_and_events_together() {
        let (_d, st) = store();
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d1"), 60).unwrap();
        let s = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)
            .unwrap();
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        let turn = |n: i64| {
            turn_record(
                st.now(),
                "claude-sonnet-5-5",
                KeySource::Instance,
                n,
                0,
                10,
                "tool_use",
            )
        };
        // A charge on an unknown line refuses the whole commit.
        let bad = st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("d1"),
            s.n,
            StepCommit {
                payload: json!({}),
                usage: vec![Metered::new("nope", 1, KeySource::Instance)],
                events: vec![turn(1)],
                draft: vec![DraftChange::Delete {
                    path: "a.css".into(),
                }],
                reason: None,
                delivered: vec![],
            },
        );
        assert!(bad.is_err());
        assert!(st.open_step(&run.id).unwrap().is_some());
        assert!(st.draft_files(&run.id).unwrap().is_empty());
        assert!(
            st.events_after(0, 1000)
                .unwrap()
                .iter()
                .all(|e| e.kind != "bot.turn")
        );
        assert!(st.usage_rows(OWNER, st.now_utc()).unwrap().is_empty());

        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d1"),
                s.n,
                StepCommit {
                    payload: json!({"messages": []}),
                    usage: vec![Metered::new(&line, 1_000_000, KeySource::Instance)],
                    events: vec![turn(1_000_000)],
                    draft: vec![
                        DraftChange::Write {
                            path: "index.html".into(),
                            content: b"<p>new</p>".to_vec(),
                            content_type: Some("text/html".into()),
                        },
                        DraftChange::Delete {
                            path: "old.css".into(),
                        },
                    ],
                    reason: None,
                    delivered: vec![],
                },
            )
            .unwrap();
        assert_eq!(done.credits_milli, 280_000);
        assert_eq!(done.run.credits_milli, 280_000);
        assert_eq!(done.usage[0].run_id.as_deref(), Some(run.id.as_str()));
        let draft = st.draft_files(&run.id).unwrap();
        assert_eq!(draft.len(), 2);
        assert_eq!(draft[0].content.as_deref(), Some(&b"<p>new</p>"[..]));
        assert_eq!(draft[1].content, None);
        let turn = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == "bot.turn")
            .unwrap();
        let body: Value = serde_json::from_str(&turn.body).unwrap();
        assert_eq!(body["credits_milli"], 280_000);
        assert_eq!(body["key_source"], "instance");
        assert_eq!(body["run"], run.id.as_str());
        assert_eq!(body["step"], 1);
        assert_eq!(turn.ids.artifact.as_deref(), Some(run.artifact_id.as_str()));
        assert!(turn.actor.contains("\"type\":\"bot\""));
    }

    #[test]
    fn cancel_is_recorded_and_stops_at_the_next_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d1"), 60).unwrap();
        let sam = AuditCtx {
            actor: Actor::Viewer {
                public_id: "u_sam".into(),
                display_name: None,
                person: true,
            },
            ..DAEMON.clone()
        };
        assert_eq!(
            st.cancel_run(&sam, &run.id).unwrap().status,
            RunStatus::Stopping
        );
        // Asking again changes nothing and records nothing.
        st.cancel_run(&sam, &run.id).unwrap();
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)),
            "run_invalid"
        );
        let s = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Stopped)
            .unwrap();
        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d1"),
                s.n,
                StepCommit {
                    payload: json!({}),
                    usage: vec![Metered::new(LINE_HARNESS_FEE, 1, KeySource::Instance)],
                    reason: Some("cancelled".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(done.run.status, RunStatus::Stopped);
        assert_eq!(done.run.reason.as_deref(), Some("cancelled"));
        assert!(done.run.ended_at.is_some() && done.run.claimed_by.is_none());
        assert!(st.live_runs().unwrap().is_empty());
        assert_eq!(
            st.cancel_run(&sam, &run.id).unwrap().status,
            RunStatus::Stopped
        );
        assert!(
            st.claim_run(&run.id, &DriverId::named("d1"), 60)
                .unwrap()
                .is_none()
        );
        let events: Vec<(String, String)> = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("bot."))
            .map(|e| (e.kind, e.body))
            .collect();
        let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["bot.run_start", "bot.cancel", "bot.run_end"]);
        assert!(events[1].1.contains("\"by\":\"u_sam\""), "{}", events[1].1);
        let (r, steps) = st.run_with_steps(&run.id).unwrap().unwrap();
        assert_eq!(r.credits_milli, 1_000);
        assert_eq!(steps.len(), 1);
        assert_eq!(st.runs_for_artifact(&run.artifact_id, 10).unwrap().len(), 1);
        assert_eq!(st.runs_for_person(OWNER, 10).unwrap()[0].id, run.id);
        assert!(st.runs_for_person("u_other", 10).unwrap().is_empty());
    }

    #[test]
    fn a_stale_claim_is_taken_over() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d0"), 60)
            .unwrap()
            .unwrap();
        // A lease of 0 takes a claim at once, as a restarted daemon does.
        st.claim_run(&run.id, &DriverId::named("d1"), 0)
            .unwrap()
            .unwrap();
        clock.advance(30);
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_none()
        );
        clock.advance(31);
        assert_eq!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .unwrap()
                .claimed_by
                .as_deref(),
            Some("d2")
        );
    }

    #[test]
    fn observed_usage_and_commit_failures_are_kept_with_the_open_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Turn).unwrap();
        assert!(
            st.observe_step(&run.id, &d, 1, 1, json!({"input_tokens": 5}))
                .unwrap()
        );
        assert!(
            st.observe_step(&run.id, &d, 1, 1, json!({"input_tokens": 7}))
                .unwrap()
        );
        assert_eq!(
            st.begin_step(&run.id, &d, 1, StepKind::Turn)
                .unwrap()
                .attempt,
            2
        );
        assert!(
            st.observe_step(&run.id, &d, 1, 2, json!({"input_tokens": 9}))
                .unwrap()
        );
        assert!(
            !st.observe_step(&run.id, &DriverId::named("d2"), 1, 2, json!({}))
                .unwrap(),
            "not the claim holder"
        );
        assert!(
            !st.observe_step(&run.id, &d, 2, 1, json!({})).unwrap(),
            "not the open step"
        );
        let open = st.open_step(&run.id).unwrap().unwrap();
        assert_eq!(open.observed.len(), 2);
        assert_eq!(open.observed[0], json!({"input_tokens": 7, "attempt": 1}));
        assert_eq!(st.note_commit_failure(&run.id, 1).unwrap(), 1);
        assert_eq!(st.note_commit_failure(&run.id, 1).unwrap(), 2);
        assert_eq!(st.note_commit_failure(&run.id, 9).unwrap(), 0);
        assert_eq!(st.open_step(&run.id).unwrap().unwrap().failures, 2);
    }

    #[test]
    fn the_call_journal_answers_a_repeated_key_and_refuses_a_reused_one() {
        let (_d, st) = store();
        let run = new_run(&st);
        let entry = CallEntry {
            run_id: run.id.clone(),
            step_n: 1,
            seq: 0,
            attempt: 1,
            op: "threads.reply".into(),
            args_hash: "h1".into(),
            ok: true,
            result: Value::Null,
        };
        let mut writes = 0;
        let v = st
            .with_bot_journal(&entry, &|_| Ok(()), |_| {
                writes += 1;
                Ok(json!({"comment_id": "c1"}))
            })
            .unwrap()
            .into_value();
        let again = st
            .with_bot_journal(
                &CallEntry {
                    attempt: 2,
                    ..entry.clone()
                },
                &|_| Ok(()),
                |_| {
                    writes += 1;
                    Ok(json!({"comment_id": "c2"}))
                },
            )
            .unwrap()
            .into_value();
        assert_eq!(
            (v.clone(), again, writes),
            (json!({"comment_id": "c1"}), v, 1)
        );
        let other = CallEntry {
            args_hash: "h2".into(),
            ..entry.clone()
        };
        assert_eq!(
            code(st.with_bot_journal(&other, &|_| Ok(()), |_| Ok(Value::Null))),
            "key_reused"
        );
        let refused = CallEntry {
            seq: 1,
            ..entry.clone()
        };
        assert_eq!(
            code(
                st.with_bot_journal(&refused, &|_| Ok(()), |_| Err(CoreError::invalid(
                    "forbidden",
                    "no"
                )))
            ),
            "forbidden"
        );
        let read = CallEntry {
            seq: 2,
            op: "threads.read".into(),
            ..entry.clone()
        };
        st.record_bot_call(&read).unwrap();
        let journal = st.bot_calls(&run.id, 1).unwrap();
        assert_eq!(journal.iter().map(|c| c.seq).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(journal[0].result, json!({"comment_id": "c1"}));
    }

    /// A journal entry, a step's payload or its observations whose stored
    /// text does not read as JSON is refused when read, never read as
    /// `null` or none: the journal never answers a call made again with
    /// anything but what it recorded.
    #[test]
    fn a_journal_entry_or_a_step_whose_stored_json_does_not_read_is_refused() {
        let (_d, st) = store();
        let run = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Script).unwrap();
        st.record_bot_call(&CallEntry {
            run_id: run.id.clone(),
            step_n: 1,
            seq: 0,
            attempt: 1,
            op: "threads.read".into(),
            args_hash: "h".into(),
            ok: true,
            result: json!({"threads": []}),
        })
        .unwrap();
        assert_eq!(st.bot_calls(&run.id, 1).unwrap().len(), 1);
        let damage = |sql: &str| {
            st.with_write(|c| {
                c.execute(sql, params![run.id])?;
                Ok(())
            })
            .unwrap();
        };
        damage("UPDATE bot_calls SET result_json = '{\"thr' WHERE run_id = ?1");
        assert!(matches!(st.bot_calls(&run.id, 1), Err(CoreError::Db(_))));
        for sql in [
            "UPDATE bot_steps SET observed_json = 'nope' WHERE run_id = ?1",
            "UPDATE bot_steps SET observed_json = NULL, payload_json = '[' WHERE run_id = ?1",
        ] {
            damage(sql);
            assert!(
                matches!(st.run_with_steps(&run.id), Err(CoreError::Db(_))),
                "{sql}"
            );
        }
    }

    #[test]
    fn a_runs_performed_calls_of_some_operations_are_counted_across_its_steps() {
        let (_d, st) = store();
        let run = new_run(&st);
        let other = new_run(&st);
        let call =
            |run: &str, step_n: i64, seq: i64, op: &str, ok: bool, result: Value| CallEntry {
                run_id: run.to_string(),
                step_n,
                seq,
                attempt: 1,
                op: op.into(),
                args_hash: "h".into(),
                ok,
                result,
            };
        let big = Value::String("x".repeat(64 * 1024));
        for entry in [
            call(&run.id, 3, 0, "desk.notify", true, big.clone()),
            call(&run.id, 1, 1, "desk.ask", true, Value::Null),
            call(&run.id, 1, 0, "desk.lamps", true, Value::Null),
            call(&run.id, 1, 2, "desk.notify", false, json!({"code": "busy"})),
            call(
                &run.id,
                1,
                3,
                "desk.notify",
                false,
                json!({"code": "effect_limit"}),
            ),
            call(
                &run.id,
                1,
                4,
                "desk.notify",
                false,
                json!({"code": "unavailable"}),
            ),
            call(
                &run.id,
                1,
                5,
                "desk.notify",
                false,
                json!({"message": "no code"}),
            ),
            call(&run.id, 2, 0, "desk.notify", false, json!({"code": 7})),
            call(
                &run.id,
                2,
                1,
                "desk.notify",
                true,
                json!({"code": "effect_limit"}),
            ),
            call(&other.id, 1, 0, "desk.notify", true, Value::Null),
        ] {
            st.record_bot_call(&entry).unwrap();
        }
        let not_done = ["effect_limit", "unavailable"];
        let counted = |ops: &[&str]| st.bot_calls_counted(&run.id, ops, &not_done).unwrap();
        // Of `notify`: the success with a long result, `busy`, the
        // failure with no code, the code that is no string, and a
        // success whatever its value holds; not `effect_limit` or
        // `unavailable`, and not another run's.
        assert_eq!(counted(&["desk.notify"]), 5);
        assert_eq!(counted(&["desk.notify", "desk.ask"]), 6);
        assert_eq!(counted(&["desk.lamps"]), 1);
        assert_eq!(counted(&[]), 0);
        assert_eq!(
            st.bot_calls_counted(&run.id, &["desk.notify"], &[])
                .unwrap(),
            7
        );
        // A result that is not JSON counts, as a failure under no code.
        st.with_write(|c| {
            c.execute(
                "UPDATE bot_calls SET result_json = 'not json' WHERE run_id = ?1 AND step_n = 1 \
                 AND seq = 3",
                params![run.id],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(counted(&["desk.notify"]), 6);
        assert!(st.has_bot_call(&run.id, 1, 3).unwrap());
        assert!(!st.has_bot_call(&run.id, 1, 6).unwrap());
        assert!(!st.has_bot_call(&run.id, 4, 0).unwrap());
    }

    #[test]
    fn a_snapshot_rate_charges_as_then_even_without_its_line() {
        let (_d, st) = store();
        let run = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Turn).unwrap();
        let mut rate = st
            .rate_of(&token_line("claude-sonnet-5-5", Unit::MtokInput))
            .unwrap()
            .unwrap();
        rate.cost_micro_usd = 1_000_000;
        let mut m = Metered::absorbed("gone-model.input", 2_000_000, KeySource::Instance);
        m.rate = Some(rate);
        m.estimated = true;
        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &d,
                1,
                StepCommit {
                    payload: json!({}),
                    usage: vec![m],
                    ..Default::default()
                },
            )
            .unwrap();
        let row = &done.usage[0];
        assert_eq!(row.line, "gone-model.input");
        assert_eq!((row.cost_micro_usd, row.credits_milli), (2_000_000, 0));
        assert_eq!(row.rate_cost_micro_usd, 1_000_000);
        assert!(row.absorbed && row.estimated);
        assert_eq!(st.usage_rows(OWNER, st.now_utc()).unwrap()[0], *row);
    }

    #[test]
    fn a_journaled_comment_and_its_entry_commit_together_or_not_at_all() {
        let (_d, st) = store();
        let run = new_run(&st);
        let aid = clax_core::ArtifactId::parse(&run.artifact_id).unwrap();
        let t = st
            .create_thread(
                DAEMON,
                &aid,
                crate::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: crate::test_util::anchor(),
                    author_name: "Alex".into(),
                    body: "Make it blue.".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        let entry = CallEntry {
            run_id: run.id.clone(),
            step_n: 1,
            seq: 0,
            attempt: 1,
            op: "threads.reply".into(),
            args_hash: "h".into(),
            ok: true,
            result: Value::Null,
        };
        let reply = || crate::threads::NewComment {
            author_kind: clax_core::model::AUTHOR_AGENT,
            author_name: "ClaxBot".into(),
            author_public_id: None,
            via_session_id: None,
            body: "Done.".into(),
            via_page: false,
        };
        let comments = |st: &Store| st.get_thread(&t.id).unwrap().unwrap().comments.len();
        st.set_journal_fault(Some(std::sync::Arc::new(|e: &CallEntry| e.seq == 0)));
        assert_eq!(
            code(st.add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))),
            "journal_fault"
        );
        assert_eq!(comments(&st), 1, "the reply rolled back with its entry");
        assert!(st.bot_calls(&run.id, 1).unwrap().is_empty());
        st.set_journal_fault(None);
        let first = st
            .add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))
            .unwrap();
        let again = st
            .add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(comments(&st), 2, "one reply");
        // A guard that refuses (the person lost their rights) writes nothing.
        let refused = CallEntry {
            seq: 2,
            ..entry.clone()
        };
        assert_eq!(
            code(
                st.add_comment_journaled(&bot(), &t.id, reply(), &refused, &|_| {
                    Err(CoreError::invalid("forbidden", "revoked"))
                })
            ),
            "forbidden"
        );
        assert_eq!(comments(&st), 2);
        assert!(first.value()["comment_id"].is_string());
        let resolve = CallEntry {
            seq: 1,
            op: "threads.resolve".into(),
            ..entry.clone()
        };
        st.resolve_thread_journaled(&bot(), &t.id, "agent:claxbot", None, &resolve, &|_| Ok(()))
            .unwrap();
        // A created artifact and its entry commit together, or neither.
        let made = CallEntry {
            seq: 3,
            op: "artifacts.create".into(),
            ..entry.clone()
        };
        let new = || {
            let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
                "title": "Made by a run",
                "files": {"index.html": {"content": "<p>soon</p>", "encoding": "utf8"}}
            }))
            .unwrap();
            clax_core::publish::validate(req).unwrap()
        };
        let count = |st: &Store| {
            st.list_artifacts()
                .unwrap()
                .iter()
                .filter(|a| a.title == "Made by a run")
                .count()
        };
        st.set_journal_fault(Some(std::sync::Arc::new(|e: &CallEntry| e.seq == 3)));
        assert_eq!(
            code(clax_core::content::wait(st.create_artifact_journaled(
                &bot(),
                new(),
                None,
                &made,
                &|_| Ok(())
            ))),
            "journal_fault"
        );
        st.set_journal_fault(None);
        assert_eq!(count(&st), 0, "the artifact went with its entry");
        let a = clax_core::content::wait(st.create_artifact_journaled(
            &bot(),
            new(),
            None,
            &made,
            &|_| Ok(()),
        ))
        .unwrap();
        let b = clax_core::content::wait(st.create_artifact_journaled(
            &bot(),
            new(),
            None,
            &made,
            &|_| Ok(()),
        ))
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(count(&st), 1);
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().status, "resolved");
        let journal: Vec<i64> = st
            .bot_calls(&run.id, 1)
            .unwrap()
            .iter()
            .map(|c| c.seq)
            .collect();
        assert_eq!(journal, [0, 1, 3], "the refused reply journaled nothing");
    }

    fn d(name: &str) -> DriverId {
        DriverId::named(name)
    }

    /// A store on a clock stopped at 10:00 on 2026-11-11, and that clock.
    fn store_at_ten() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::at("2026-11-11T10:00:00Z"));
        let (dir, st) = store_with_clock(clock.clone());
        (dir, st, clock)
    }

    /// A run driver `d1` holds, its first step, a script, begun.
    fn run_in_a_script(st: &Store) -> Run {
        let run = new_run(st);
        st.claim_run(&run.id, &d("d1"), 60).unwrap().unwrap();
        st.begin_step(&run.id, &d("d1"), 1, StepKind::Script)
            .unwrap();
        st.run(&run.id).unwrap().unwrap()
    }

    const FIRST_CALL: JournalPlace = JournalPlace { step_n: 1, seq: 0 };

    /// Parks `run`, which `d1` holds in step 1, on its first call, due
    /// `due_in_secs` from the clock's time (never for `None`).
    fn park(st: &Store, clock: &ManualClock, run: &Run, due_in_secs: Option<i64>) -> Run {
        let now = clock.now();
        let parked = st
            .park_run(
                &run.id,
                &d("d1"),
                FIRST_CALL,
                &json!({"surface": "shell", "op": "ask"}),
                due_in_secs.map(|s| now + chrono::Duration::seconds(s)),
                now,
            )
            .unwrap();
        match parked {
            Park::Parked(run) => run,
            Park::Answered(run) => panic!("not parked: {run:?}"),
        }
    }

    #[test]
    fn the_statuses_of_a_run_that_has_not_ended_are_spelled_once() {
        let live: Vec<String> = RunStatus::ALL
            .iter()
            .filter(|s| s.is_live())
            .map(|s| format!("'{}'", s.as_str()))
            .collect();
        assert_eq!(LIVE, format!("({})", live.join(", ")));
        for status in RunStatus::ALL {
            assert_ne!(status.is_live(), status.is_terminal(), "{status:?}");
        }
        assert!(RunStatus::Parked.is_live());
        assert_eq!(RunStatus::parse("parked"), Some(RunStatus::Parked));
        // The index over those runs is over the same statuses.
        let (_d, st) = store();
        let index: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT sql FROM sqlite_schema WHERE name = 'bot_runs_live'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(
            index.ends_with(&format!("WHERE status IN {LIVE}")),
            "{index}"
        );
    }

    #[test]
    fn a_run_parks_for_the_driver_that_holds_it_and_lets_go_of_its_claim() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        let on = json!({"surface": "shell", "op": "ask"});
        let due = clock.now() + chrono::Duration::hours(24);
        assert_eq!(st.parked_runs_of(OWNER).unwrap(), 0);
        assert_eq!(
            st.run_parking(&run.id).unwrap(),
            Some(Parking {
                parked_at: None,
                parked_ms: 0,
                waiting_on: None,
                parks: 0
            })
        );
        assert_eq!(st.run_parking("nope").unwrap(), None);

        // Refused: another driver, another step, no such run.
        assert_eq!(
            code(st.park_run(&run.id, &d("d2"), FIRST_CALL, &on, Some(due), clock.now())),
            "run_not_claimed"
        );
        let second_step = JournalPlace { step_n: 2, seq: 0 };
        assert_eq!(
            code(st.park_run(&run.id, &d("d1"), second_step, &on, Some(due), clock.now())),
            "run_invalid"
        );
        assert!(matches!(
            st.park_run("nope", &d("d1"), FIRST_CALL, &on, Some(due), clock.now()),
            Err(CoreError::NotFound)
        ));
        assert_eq!(
            st.run(&run.id).unwrap().unwrap(),
            run,
            "a refused park changes nothing"
        );

        clock.advance(10);
        let parked = st
            .park_run(&run.id, &d("d1"), FIRST_CALL, &on, Some(due), clock.now())
            .unwrap();
        let Park::Parked(parked) = parked else {
            panic!("not parked: {parked:?}");
        };
        assert_eq!(
            parked,
            Run {
                status: RunStatus::Parked,
                claimed_by: None,
                claimed_at: None,
                due_at: Some("2026-11-12T10:00:00.000Z".into()),
                updated_at: "2026-11-11T10:00:10.000Z".into(),
                ..run.clone()
            }
        );
        assert_eq!(st.run(&run.id).unwrap().unwrap(), parked);
        assert_eq!(
            st.run_parking(&run.id).unwrap(),
            Some(Parking {
                parked_at: Some("2026-11-11T10:00:10.000Z".into()),
                parked_ms: 0,
                waiting_on: Some(on.clone()),
                parks: 1
            })
        );
        assert_eq!(st.parked_runs_of(OWNER).unwrap(), 1);
        assert_eq!(st.parked_runs_of("u_mia").unwrap(), 0);
        // Its step stays begun and uncommitted.
        let open = st.open_step(&run.id).unwrap().unwrap();
        assert_eq!((open.n, open.attempt, open.committed_at), (1, 1, None));

        // Parked again, by anyone and with anything, nothing changes.
        clock.advance(5);
        for driver in ["d1", "d2"] {
            assert_eq!(
                st.park_run(
                    &run.id,
                    &d(driver),
                    second_step,
                    &json!("other"),
                    None,
                    clock.now()
                )
                .unwrap(),
                Park::Parked(parked.clone())
            );
        }
        assert_eq!(st.run(&run.id).unwrap().unwrap(), parked);

        // The driver that parked it no longer steps it.
        assert_eq!(
            code(st.begin_step(&run.id, &d("d1"), 1, StepKind::Script)),
            "run_not_claimed"
        );
        assert_eq!(
            code(st.commit_bot_step(&bot(), &run.id, &d("d1"), 1, StepCommit::default())),
            "run_not_claimed"
        );
    }

    #[test]
    fn only_a_running_run_parks() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        let stopping = st.cancel_run(&bot(), &run.id).unwrap();
        assert_eq!(stopping.status, RunStatus::Stopping);
        assert_eq!(
            code(st.park_run(&run.id, &d("d1"), FIRST_CALL, &json!({}), None, clock.now())),
            "run_invalid"
        );
        assert_eq!(st.run(&run.id).unwrap().unwrap(), stopping);
        // A run between steps has no step to wait in.
        let idle = new_run(&st);
        st.claim_run(&idle.id, &d("d1"), 60).unwrap().unwrap();
        assert_eq!(
            code(st.park_run(
                &idle.id,
                &d("d1"),
                FIRST_CALL,
                &json!({}),
                None,
                clock.now()
            )),
            "run_invalid"
        );
    }

    #[test]
    fn a_run_whose_awaited_place_is_journaled_does_not_park() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        let entry = st
            .call_entry(&run.id, &d("d1"), 1, 1, 0, "shell.ask", "h0")
            .unwrap()
            .answered(true, json!("yes"));
        st.record_bot_call(&entry).unwrap();
        let held = st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            st.park_run(&run.id, &d("d1"), FIRST_CALL, &json!({}), None, clock.now())
                .unwrap(),
            Park::Answered(held.clone())
        );
        assert_eq!(st.run(&run.id).unwrap().unwrap(), held);
        assert_eq!(held.claimed_by.as_deref(), Some("d1"));
        assert_eq!(st.parked_runs_of(OWNER).unwrap(), 0);
        // Another place of the step is still to wait for.
        let next = JournalPlace { step_n: 1, seq: 1 };
        assert!(matches!(
            st.park_run(&run.id, &d("d1"), next, &json!({}), None, clock.now())
                .unwrap(),
            Park::Parked(_)
        ));
    }

    #[test]
    fn unparking_makes_a_run_claimable_and_counts_the_time_it_was_parked() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        assert_eq!(st.unpark_run(&run.id, clock.now()).unwrap(), None);
        assert_eq!(
            st.run(&run.id).unwrap().unwrap(),
            run,
            "a run that is not parked is left as it is"
        );
        assert!(matches!(
            st.unpark_run("nope", clock.now()),
            Err(CoreError::NotFound)
        ));

        park(&st, &clock, &run, Some(3600));
        clock.advance(90);
        let woken = st.unpark_run(&run.id, clock.now()).unwrap().unwrap();
        assert_eq!(
            woken,
            Run {
                status: RunStatus::Queued,
                claimed_by: None,
                claimed_at: None,
                due_at: None,
                updated_at: "2026-11-11T10:01:30.000Z".into(),
                ..run.clone()
            }
        );
        assert_eq!(
            st.run_parking(&run.id).unwrap(),
            Some(Parking {
                parked_at: None,
                parked_ms: 90_000,
                waiting_on: None,
                parks: 1
            })
        );
        // Unparked again, nothing changes.
        clock.advance(30);
        assert_eq!(st.unpark_run(&run.id, clock.now()).unwrap(), None);
        assert_eq!(st.run(&run.id).unwrap().unwrap(), woken);
        assert_eq!(st.run_parking(&run.id).unwrap().unwrap().parked_ms, 90_000);

        // A driver claims it as it claims a run a driver lost, and its
        // open step is begun again.
        let held = st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        assert_eq!(
            (held.status, held.claimed_by.as_deref()),
            (RunStatus::Running, Some("d2"))
        );
        let again = st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        assert_eq!((again.n, again.attempt), (1, 2));
        // A second parking adds to the first.
        st.park_run(&run.id, &d("d2"), FIRST_CALL, &json!({}), None, clock.now())
            .unwrap();
        clock.advance(30);
        st.unpark_run(&run.id, clock.now()).unwrap().unwrap();
        let twice = st.run_parking(&run.id).unwrap().unwrap();
        assert_eq!(
            (twice.parked_ms, twice.parks),
            (120_000, 2),
            "two parks in a step begun twice"
        );
    }

    /// No driver claims a parked run: not one resuming every run with a
    /// lease of 0, not after the time it parked until.
    #[test]
    fn a_parked_run_is_never_claimed_until_it_is_unparked() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        let parked = park(&st, &clock, &run, Some(3600));
        for lease in [60, 0] {
            for driver in ["d1", "d2"] {
                assert!(
                    st.claim_run(&run.id, &d(driver), lease).unwrap().is_none(),
                    "parked, {driver} with lease {lease}"
                );
            }
        }
        clock.advance(2 * 3600);
        assert!(
            st.claim_run(&run.id, &d("d2"), 0).unwrap().is_none(),
            "past its due time"
        );
        assert_eq!(
            st.run(&run.id).unwrap().unwrap(),
            parked,
            "a claim that is refused changes nothing"
        );
        st.unpark_run(&run.id, clock.now()).unwrap().unwrap();
        let held = st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        assert_eq!(
            (held.status, held.claimed_by.as_deref(), held.due_at),
            (RunStatus::Running, Some("d2"), None)
        );
    }

    #[test]
    fn a_second_send_of_a_parked_runs_thread_returns_that_run() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        let parked = park(&st, &clock, &run, None);
        let threads = ["t1".to_string()];
        assert_eq!(st.active_run_on(&threads).unwrap(), Some(parked.clone()));
        let again = st
            .create_run_exclusive(
                &bot(),
                NewRun {
                    artifact_id: run.artifact_id.clone(),
                    person: OWNER.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec!["t0".into(), "t1".into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    web: String::new(),
                },
            )
            .unwrap();
        assert_eq!(again, Err(parked.clone()), "no second run is made");
        assert_eq!(st.runs_for_thread("t1", 10).unwrap().len(), 1);
        // A follow-up queues behind it, and its thread is not one left
        // waiting with no run.
        assert_eq!(
            st.queue_follow_up_if_active("t1", "u_mia").unwrap(),
            Some(parked)
        );
        assert_eq!(st.follow_ups("t1").unwrap(), ["u_mia"]);
        assert!(st.waiting_follow_up_threads().unwrap().is_empty());
    }

    #[test]
    fn a_parked_run_is_among_the_runs_that_have_not_ended() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        clock.advance(1);
        let queued = new_run(&st);
        let parked = park(&st, &clock, &run, None);
        assert_eq!(st.live_runs().unwrap(), [parked, queued]);
    }

    #[test]
    fn a_parked_runs_claim_is_not_renewed_until_a_driver_holds_it_again() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        assert!(st.renew_lease(&run.id, &d("d1")).unwrap());
        park(&st, &clock, &run, Some(60));
        assert!(
            !st.renew_lease(&run.id, &d("d1")).unwrap(),
            "the driver that parked it let go of it"
        );
        assert_eq!(st.run(&run.id).unwrap().unwrap().claimed_by, None);
        clock.advance(60);
        st.unpark_run(&run.id, clock.now()).unwrap().unwrap();
        st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        assert!(st.renew_lease(&run.id, &d("d2")).unwrap());
        assert!(!st.renew_lease(&run.id, &d("d1")).unwrap());
    }

    #[test]
    fn cancelling_a_parked_run_makes_it_stopping_and_claimable_at_once() {
        let (_d, st, clock) = store_at_ten();
        let run = run_in_a_script(&st);
        park(&st, &clock, &run, None);
        clock.advance(45);
        let stopping = st.cancel_run(&bot(), &run.id).unwrap();
        assert_eq!(
            (
                stopping.status,
                stopping.claimed_by.clone(),
                stopping.due_at.clone()
            ),
            (RunStatus::Stopping, None, None)
        );
        assert_eq!(
            st.run_parking(&run.id).unwrap(),
            Some(Parking {
                parked_at: None,
                parked_ms: 45_000,
                waiting_on: None,
                parks: 1
            })
        );
        let cancels = |st: &Store| {
            st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM audit_events WHERE kind = 'bot.cancel'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap()
        };
        assert_eq!(cancels(&st), 1);
        // Asked again, nothing changes and nothing is recorded.
        assert_eq!(st.cancel_run(&bot(), &run.id).unwrap(), stopping);
        assert_eq!(cancels(&st), 1);
        // A driver takes it and writes its stopped step.
        st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        st.begin_step(&run.id, &d("d2"), 1, StepKind::Stopped)
            .unwrap();
        let end = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &d("d2"),
                1,
                StepCommit {
                    reason: Some("cancelled".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(end.run.status, RunStatus::Stopped);
        assert_eq!(st.parked_runs_of(OWNER).unwrap(), 0);
    }

    const DEF_HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const SHELL: &str = "0123456789abcdef0123456789abcdef";
    /// The SHA-256 of a prompt file, as a binding names it.
    const PROMPT_HASH: &str = "60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752";

    /// A `shell` binding of version 2 served by [`SHELL`], and `clax`
    /// bound beside it.
    fn shell_bindings() -> Bindings {
        Bindings::bound(vec![
            StoredBinding::bound(
                "shell",
                Some(DEF_HASH.into()),
                Some(SHELL.into()),
                None,
                Value::Null,
                vec!["shell.windows".into(), "shell.ask".into()],
            ),
            StoredBinding::bound(
                "clax",
                None,
                None,
                None,
                json!({"artifact": "7q3k9mzx2b4t"}),
                Vec::new(),
            ),
        ])
        .unwrap()
    }

    #[test]
    fn bindings_of_version_2_hold_each_surfaces_registration_and_operations() {
        let made = shell_bindings();
        assert_eq!(made.version(), 2);
        let text = made.to_json();
        assert_eq!(
            text,
            format!(
                r#"{{"v":2,"bindings":[{{"surface":"shell","hash":"{DEF_HASH}","registration":"{SHELL}","scope":null,"operations":["shell.windows","shell.ask"]}},{{"surface":"clax","hash":null,"scope":{{"artifact":"7q3k9mzx2b4t"}},"operations":[]}}]}}"#
            )
        );
        let read = Bindings::parse(&text).unwrap();
        assert_eq!(read, made);
        assert_eq!(read.to_json(), text);
        let shell = read.of("shell").expect("the shell binding");
        assert_eq!(
            (
                shell.surface(),
                shell.hash(),
                shell.registration(),
                shell.scope(),
                shell.operations(),
                shell.granted()
            ),
            (
                "shell",
                Some(DEF_HASH),
                Some(SHELL),
                &Value::Null,
                Some(&["shell.windows".to_string(), "shell.ask".to_string()][..]),
                &[][..]
            )
        );
        // A binding with no registration leaves the member out, and one
        // with no operations has none: an empty list is not "the standing
        // ones".
        let clax = read.of("clax").unwrap();
        assert_eq!(
            (clax.registration(), clax.operations()),
            (None, Some(&[][..]))
        );
        // `registration: null` reads as none; an unknown member is ignored.
        let spelled = text
            .replace(
                r#""hash":null,"scope""#,
                r#""hash":null,"registration":null,"scope""#,
            )
            .replace(r#""scope":null"#, r#""scope":null,"reader_view":"names""#);
        assert_eq!(Bindings::parse(&spelled).unwrap(), made);

        // A binding started with a prompt file names it by its hash,
        // after its registration; one started with none leaves the
        // member out (above), and `prompt: null` reads as none.
        assert_eq!(shell.prompt(), None);
        let prompted = Bindings::bound(vec![StoredBinding::bound(
            "shell",
            Some(DEF_HASH.into()),
            Some(SHELL.into()),
            Some(PROMPT_HASH.into()),
            Value::Null,
            vec!["shell.ask".into()],
        )])
        .unwrap();
        let prompted_text = prompted.to_json();
        assert_eq!(
            prompted_text,
            format!(
                r#"{{"v":2,"bindings":[{{"surface":"shell","hash":"{DEF_HASH}","registration":"{SHELL}","prompt":"{PROMPT_HASH}","scope":null,"operations":["shell.ask"]}}]}}"#
            )
        );
        let read_prompted = Bindings::parse(&prompted_text).unwrap();
        assert_eq!(read_prompted, prompted);
        assert_eq!(
            read_prompted.of("shell").unwrap().prompt(),
            Some(PROMPT_HASH)
        );
        let null_prompt = text.replace(
            r#""scope":null,"operations":["shell.windows""#,
            r#""prompt":null,"scope":null,"operations":["shell.windows""#,
        );
        assert_ne!(null_prompt, text);
        assert_eq!(Bindings::parse(&null_prompt).unwrap(), made);

        // Settings kept with the run are written after the prompt file,
        // read back as they were, and a binding written without them
        // (or with `null`) reads as one that has none.
        let settings = json!({"call_timeout_s": 7, "runs": {"max_steps": 3}});
        let pinned = Bindings::bound(vec![
            StoredBinding::bound(
                "shell",
                Some(DEF_HASH.into()),
                Some(SHELL.into()),
                Some(PROMPT_HASH.into()),
                Value::Null,
                vec!["shell.ask".into()],
            )
            .with_settings(settings.clone()),
        ])
        .unwrap();
        let pinned_text = pinned.to_json();
        assert_eq!(
            pinned_text,
            format!(
                r#"{{"v":2,"bindings":[{{"surface":"shell","hash":"{DEF_HASH}","registration":"{SHELL}","prompt":"{PROMPT_HASH}","settings":{{"call_timeout_s":7,"runs":{{"max_steps":3}}}},"scope":null,"operations":["shell.ask"]}}]}}"#
            )
        );
        let read_pinned = Bindings::parse(&pinned_text).unwrap();
        assert_eq!(read_pinned, pinned);
        assert_eq!(read_pinned.of("shell").unwrap().settings(), Some(&settings));
        assert_eq!(read_prompted.of("shell").unwrap().settings(), None);
        let null_settings = text.replace(
            r#""scope":null,"operations":["shell.windows""#,
            r#""settings":null,"scope":null,"operations":["shell.windows""#,
        );
        assert_ne!(null_settings, text);
        assert_eq!(Bindings::parse(&null_settings).unwrap(), made);

        // A binding of version 1 says no operations, no registration and
        // no prompt file, whatever its text holds.
        let v1 = Bindings::clax("7q3k9mzx2b4t", "fetch");
        assert_eq!(v1.version(), 1);
        let clax = v1.of("clax").unwrap();
        assert_eq!((clax.operations(), clax.registration()), (None, None));
        assert_eq!(clax.prompt(), None);
        assert_eq!(clax.settings(), None);
        let v1_spelled = v1.to_json().replace(
            r#""hash":null,"#,
            &format!(r#""hash":null,"prompt":"{PROMPT_HASH}","settings":{{"max_steps":1}},"#),
        );
        assert_ne!(v1_spelled, v1.to_json());
        assert_eq!(Bindings::parse(&v1_spelled).unwrap(), v1);
        assert_eq!(clax.granted(), ["web.fetch"]);

        // What version 2 refuses.
        let one = |b: &str| format!(r#"{{"v":2,"bindings":[{b}]}}"#);
        let refused = [
            (
                one(r#"{"surface":"shell","hash":null,"scope":null,"granted":[]}"#),
                "binding 0 has no `operations`",
            ),
            (
                one(r#"{"surface":"shell","hash":null,"scope":null,"operations":"all"}"#),
                "`operations` that is not a list of names",
            ),
            (
                one(r#"{"surface":"shell","hash":null,"scope":null,"operations":["a","a"]}"#),
                "the binding of surface shell names a twice",
            ),
            (
                one(
                    r#"{"surface":"shell","hash":null,"registration":7,"scope":null,"operations":[]}"#,
                ),
                "has a `registration` that is neither text nor null: 7",
            ),
            (
                one(r#"{"surface":"shell","hash":null,"prompt":7,"scope":null,"operations":[]}"#),
                "has a `prompt` that is neither text nor null: 7",
            ),
            (
                one(
                    r#"{"surface":"shell","hash":null,"settings":[1],"scope":null,"operations":[]}"#,
                ),
                "has `settings` that are neither an object nor null: [1]",
            ),
            (
                one(
                    r#"{"surface":"shell","hash":null,"settings":"none","scope":null,"operations":[]}"#,
                ),
                "has `settings` that are neither an object nor null: \"none\"",
            ),
            (
                r#"{"v":2,"bindings":[]}"#.to_string(),
                "a run has at least one binding",
            ),
        ];
        for (text, why) in refused {
            match Bindings::read(&text) {
                Err(BindingsError::Invalid(message)) => {
                    assert!(message.contains(why), "{text}: {message}");
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        let binding = |surface: &str, ops: &[&str]| {
            StoredBinding::bound(
                surface,
                None,
                None,
                None,
                Value::Null,
                ops.iter().map(|o| o.to_string()).collect(),
            )
        };
        for bindings in [
            vec![],
            vec![binding("", &[])],
            vec![binding("shell", &[]), binding("shell", &[])],
            vec![binding("shell", &["a", "a"])],
        ] {
            assert!(
                matches!(
                    Bindings::bound(bindings.clone()),
                    Err(BindingsError::Invalid(_))
                ),
                "{bindings:?}"
            );
        }
    }

    /// Bindings of a version a later build wrote are told apart from
    /// bindings that are malformed: the run cannot be driven by this
    /// build, whatever is tried again.
    #[test]
    fn bindings_of_a_later_version_are_unsupported_not_malformed() {
        let later = r#"{"v":3,"bindings":[{"surface":"shell","hash":null,"scope":null,"operations":[],"joins":[]}]}"#;
        assert_eq!(
            Bindings::read(later),
            Err(BindingsError::Unsupported { version: 3 })
        );
        assert_eq!(
            Bindings::read(r#"{"v":0,"bindings":[]}"#),
            Err(BindingsError::Unsupported { version: 0 })
        );
        let made = RunAsMade {
            kind: "shell.triage".into(),
            bindings_json: later.into(),
            request_json: "{}".into(),
        };
        assert_eq!(
            made.bindings(),
            Err(BindingsError::Unsupported { version: 3 })
        );
        match Bindings::parse(later) {
            Err(CoreError::Invalid { code, message }) => {
                assert_eq!(code, "bindings_invalid");
                assert_eq!(
                    message,
                    "bindings of version 3; this build reads versions 1 and 2"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            Bindings::read("not json"),
            Err(BindingsError::Invalid(_))
        ));
        let made = RunAsMade {
            bindings_json: shell_bindings().to_json(),
            ..made
        };
        assert_eq!(made.bindings(), Ok(shell_bindings()));
    }

    /// A store with the registration [`SHELL`] of the surface `shell`.
    fn store_with_shell() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let (dir, st, clock) = store_at_ten();
        st.put_surface_def(DEF_HASH, "{}", clock.now()).unwrap();
        let new = crate::bot_surfaces::NewRegistration {
            name: "shell".into(),
            def_hash: DEF_HASH.into(),
            prompt: None,
            settings: json!({}),
            registered_by: OWNER.into(),
            replaces: None,
        };
        st.issue_registration(DAEMON, SHELL, new, clock.now())
            .unwrap()
            .expect("issued");
        (dir, st, clock)
    }

    #[test]
    fn registrations_and_definitions_that_nothing_needs_are_pruned() {
        const IDLE: &str = "11111111111111111111111111111111";
        const SERVED: &str = "22222222222222222222222222222222";
        const IDLE_DEF: &str = "60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752";
        const UNUSED: &str = "fd61a03af4f77d870fc21e05e7e80678095c92d808cfb3b5c279ee04c74aca13";
        let (_d, st, clock) = store_with_shell();
        // SHELL starts a run and is never served; IDLE is never served
        // and starts none; SERVED starts none and is served later.
        st.create_run_of(DAEMON, triage()).unwrap();
        st.put_surface_def(IDLE_DEF, "{}", clock.now()).unwrap();
        st.put_surface_def(UNUSED, "{}", clock.now()).unwrap();
        for (id, hash) in [(IDLE, IDLE_DEF), (SERVED, DEF_HASH)] {
            let new = crate::bot_surfaces::NewRegistration {
                name: "shell".into(),
                def_hash: hash.into(),
                prompt: None,
                settings: json!({}),
                registered_by: OWNER.into(),
                replaces: None,
            };
            st.issue_registration(DAEMON, id, new, clock.now())
                .unwrap()
                .expect("issued");
        }
        // Within the hour, nothing goes.
        clock.advance(3600);
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());
        // After it, the definition that nothing names.
        clock.advance(1);
        let one_def = Pruned {
            surface_defs: 1,
            ..Pruned::default()
        };
        assert_eq!(st.prune_ended_runs().unwrap(), one_def);
        assert_eq!(st.surface_def(UNUSED).unwrap(), None);
        assert!(st.surface_def(IDLE_DEF).unwrap().is_some());
        // After seven days: the registration that was not served and
        // started no run, and with it its definition. The one that was
        // served since stays, and so does the one whose run the store
        // has, with the definition they and the run's bindings name.
        clock.advance(6 * 24 * 3600);
        assert!(st.touch_registration(SERVED, clock.now()).unwrap());
        clock.advance(24 * 3600);
        let pruned = st.prune_ended_runs().unwrap();
        assert_eq!((pruned.registrations, pruned.surface_defs), (1, 1));
        assert_eq!(st.registration(IDLE).unwrap(), None);
        assert_eq!(st.surface_def(IDLE_DEF).unwrap(), None);
        assert!(st.registration(SERVED).unwrap().is_some());
        assert!(st.registration(SHELL).unwrap().is_some());
        assert!(st.surface_def(DEF_HASH).unwrap().is_some());
        // Seven days after it was last served, the served one too; the
        // run's own stays for as long as the run does.
        clock.advance(7 * 24 * 3600 + 1);
        assert_eq!(st.prune_ended_runs().unwrap().registrations, 1);
        assert_eq!(st.registration(SERVED).unwrap(), None);
        assert!(st.registration(SHELL).unwrap().is_some());
        assert!(st.surface_def(DEF_HASH).unwrap().is_some());
        assert_eq!(st.live_runs_started_by(SHELL).unwrap(), 1);
        assert_eq!(st.live_runs_started_by(IDLE).unwrap(), 0);
    }

    /// A prompt file is kept for as long as a registration's row or a
    /// run's bindings name it, and an hour after it was last given
    /// otherwise.
    #[test]
    fn a_prompt_file_a_run_or_a_registration_names_is_kept_and_one_nothing_names_is_pruned() {
        use clax_core::audit::sha256_hex;
        let (_d, st, clock) = store_with_shell();
        let hash_of = |text: &str| sha256_hex(text.as_bytes());
        let (run_prompt, reg_prompt, unused) =
            ("You are the desk.", "You are the shell.", "You are no one.");
        // A run started with one prompt file, whose registration has
        // gone on with another since; and one nothing names.
        st.change_registration(
            DAEMON,
            SHELL,
            DEF_HASH,
            Some(run_prompt),
            &json!({}),
            clock.now(),
        )
        .unwrap();
        let bindings = Bindings::bound(vec![StoredBinding::bound(
            "shell",
            Some(DEF_HASH.into()),
            Some(SHELL.into()),
            Some(hash_of(run_prompt)),
            Value::Null,
            vec!["shell.ask".into()],
        )])
        .unwrap();
        let run = st
            .create_run_of(
                DAEMON,
                NewRunOf {
                    bindings,
                    ..triage()
                },
            )
            .unwrap();
        st.change_registration(
            DAEMON,
            SHELL,
            DEF_HASH,
            Some(reg_prompt),
            &json!({}),
            clock.now(),
        )
        .unwrap();
        st.put_surface_prompt(&hash_of(unused), unused, clock.now())
            .unwrap();
        let kept = |text: &str| st.surface_prompt(&hash_of(text)).unwrap();
        // Within the hour, nothing goes.
        clock.advance(3600);
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());
        assert_eq!(kept(unused).as_deref(), Some(unused));
        // Given again, the one nothing names is kept an hour from then.
        st.put_surface_prompt(&hash_of(unused), unused, clock.now())
            .unwrap();
        clock.advance(3600);
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());
        // After it: the one nothing names goes; the run's and the
        // registration's stay.
        clock.advance(1);
        let one_prompt = Pruned {
            surface_prompts: 1,
            ..Pruned::default()
        };
        assert_eq!(st.prune_ended_runs().unwrap(), one_prompt);
        assert_eq!(kept(unused), None);
        assert_eq!(kept(run_prompt).as_deref(), Some(run_prompt));
        assert_eq!(kept(reg_prompt).as_deref(), Some(reg_prompt));
        // Long after, with the registration on yet another: the run's
        // is kept for as long as the run is, the registration's former
        // one goes.
        st.change_registration(DAEMON, SHELL, DEF_HASH, None, &json!({}), clock.now())
            .unwrap();
        clock.advance(30 * 24 * 3600);
        assert_eq!(st.prune_ended_runs().unwrap().surface_prompts, 1);
        assert_eq!(kept(reg_prompt), None);
        assert_eq!(kept(run_prompt).as_deref(), Some(run_prompt));
        let (_, made, _) = st
            .run_with_steps_as_made(&run.id)
            .unwrap()
            .expect("the run");
        assert_eq!(
            made.bindings().unwrap().of("shell").unwrap().prompt(),
            Some(hash_of(run_prompt).as_str())
        );
    }

    fn triage() -> NewRunOf {
        NewRunOf {
            kind: "shell.triage".into(),
            request: json!({"ask": "Tidy the desktop", "touches": []}),
            bindings: shell_bindings(),
            person: "u_mia".into(),
            model: "claude-sonnet-5-5".into(),
            key_source: KeySource::Instance,
            started_by: SHELL.into(),
            parent_run: None,
            agent_grant: None,
        }
    }

    #[test]
    fn a_run_of_another_surfaces_kind_is_made_with_no_artifact_and_read() {
        let (_d, st, _clock) = store_with_shell();
        let run = st.create_run_of(&bot(), triage()).unwrap();
        assert_eq!(
            run,
            Run {
                id: run.id.clone(),
                artifact_id: String::new(),
                person: "u_mia".into(),
                request: RequestKind::Other,
                thread_ids: Vec::new(),
                status: RunStatus::Queued,
                reason: None,
                model: "claude-sonnet-5-5".into(),
                key_source: KeySource::Instance,
                base_version: 0,
                credits_milli: 0,
                claimed_by: None,
                claimed_at: None,
                created_at: "2026-11-11T10:00:00.000Z".into(),
                updated_at: "2026-11-11T10:00:00.000Z".into(),
                ended_at: None,
                due_at: None,
                agent_grant: None,
                web: String::new(),
            }
        );
        assert_eq!(run.artifact(), None);
        // The row itself: no artifact, no base version, no thread.
        let row: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT quote(artifact_id) || ' ' || quote(base_version) || ' ' || request
                            || ' ' || thread_ids_json || ' ' || quote(web) || ' ' || kind || ' '
                            || started_by || ' ' || quote(parent_run) || ' ' || request_json
                       FROM bot_runs WHERE id = ?1",
                    params![run.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            row,
            format!(
                r#"NULL NULL other [] '' shell.triage {SHELL} NULL {{"ask":"Tidy the desktop","touches":[]}}"#
            )
        );
        let (read, made, steps) = st.run_with_steps_as_made(&run.id).unwrap().unwrap();
        assert_eq!(read, run);
        assert!(steps.is_empty());
        assert_eq!(made.kind, "shell.triage");
        assert_eq!(made.bindings(), Ok(shell_bindings()));
        assert_eq!(made.bindings_json, shell_bindings().to_json());
        assert_eq!(
            st.run_origin(&run.id).unwrap(),
            Some((Some(SHELL.to_string()), None))
        );
        assert_eq!(st.run_origin("nope").unwrap(), None);

        // It is listed under its person and its registration and among
        // the runs that have not ended; it holds no thread and is on no
        // artifact.
        assert_eq!(
            st.runs_for_person("u_mia", 10).unwrap(),
            std::slice::from_ref(&run)
        );
        assert_eq!(
            st.runs_started_by(SHELL, 10).unwrap(),
            std::slice::from_ref(&run)
        );
        assert!(st.runs_started_by("nope", 10).unwrap().is_empty());
        assert_eq!(st.live_runs().unwrap(), std::slice::from_ref(&run));
        assert_eq!(st.active_run_on(&[]).unwrap(), None);
        assert!(st.runs_for_artifact("", 10).unwrap().is_empty());
        // A second run of the kind for the same person is another run:
        // no thread makes the two exclusive.
        let second = st.create_run_of(&bot(), triage()).unwrap();
        assert_ne!(second.id, run.id);
        // A child names the run that started it; a run of clax's kinds
        // names neither.
        let child = st
            .create_run_of(
                &bot(),
                NewRunOf {
                    parent_run: Some(run.id.clone()),
                    ..triage()
                },
            )
            .unwrap();
        assert_eq!(
            st.run_origin(&child.id).unwrap(),
            Some((Some(SHELL.to_string()), Some(run.id.clone())))
        );
        assert_eq!(st.run_origin(&new_run(&st).id).unwrap(), Some((None, None)));

        // Its start is recorded with no artifact, under whom it acts for.
        let (artifact, actor, body): (Option<String>, String, String) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT artifact_id, actor, body FROM audit_events
                      WHERE kind = 'bot.run_start' ORDER BY seq LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!(artifact, None);
        assert!(actor.contains("bot"), "{actor}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (
                &body["run"],
                &body["request"],
                &body["kind"],
                &body["thread_ids"],
                &body["started_by"],
                body.get("parent_run")
            ),
            (
                &json!(run.id),
                &json!("other"),
                &json!("shell.triage"),
                &json!([]),
                &json!(SHELL),
                None
            )
        );
    }

    #[test]
    fn a_run_of_another_surfaces_kind_is_refused_when_it_is_not_one() {
        let (_d, st, _clock) = store_with_shell();
        let refused = |new: NewRunOf| code(st.create_run_of(&bot(), new));
        for kind in ["triage", "shell.", ".triage", ""] {
            assert_eq!(
                refused(NewRunOf {
                    kind: kind.into(),
                    ..triage()
                }),
                "run_invalid",
                "{kind:?}"
            );
        }
        assert_eq!(
            refused(NewRunOf {
                kind: "clax.thread".into(),
                ..triage()
            }),
            "run_invalid"
        );
        assert_eq!(
            refused(NewRunOf {
                bindings: Bindings::clax("7q3k9mzx2b4t", ""),
                ..triage()
            }),
            "run_invalid",
            "bindings of version 1"
        );
        assert_eq!(
            refused(NewRunOf {
                kind: "review.note".into(),
                ..triage()
            }),
            "run_invalid",
            "no binding of the kind's surface"
        );
        assert_eq!(
            refused(NewRunOf {
                person: String::new(),
                ..triage()
            }),
            "run_invalid"
        );
        for new in [
            NewRunOf {
                started_by: "nope".into(),
                ..triage()
            },
            NewRunOf {
                parent_run: Some("nope".into()),
                ..triage()
            },
        ] {
            assert!(matches!(
                st.create_run_of(&bot(), new),
                Err(CoreError::NotFound)
            ));
        }
        assert!(st.live_runs().unwrap().is_empty(), "nothing was made");
        // A model with no price is refused as for any run.
        st.set_bot_settings(
            DAEMON,
            &[(
                crate::bot_settings::Setting::RequireExplicitPrice,
                Some(json!(true)),
            )],
        )
        .unwrap();
        assert_eq!(
            refused(NewRunOf {
                model: "unpriced-model".into(),
                ..triage()
            }),
            "model_not_priced"
        );
        st.create_run_of(&bot(), triage()).unwrap();
    }

    /// A `clax` run is written and read as before bindings had a second
    /// version, and is driven beside a run of another surface's kind.
    #[test]
    fn a_clax_run_keeps_version_1_beside_a_run_of_another_surfaces_kind() {
        let (_d, st, _clock) = store_with_shell();
        let run = new_run(&st);
        let (kind, bindings, _) = kept(&st, &run.id);
        assert_eq!(kind, "clax.thread");
        assert_eq!(bindings.version(), 1);
        let text: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT request || ' ' || bindings_json FROM bot_runs WHERE id = ?1",
                    params![run.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            text,
            format!(
                r#"thread {{"v":1,"bindings":[{{"surface":"clax","hash":null,"scope":{{"artifact":"{}"}},"granted":[]}}]}}"#,
                run.artifact_id
            )
        );
        let other = st.create_run_of(&bot(), triage()).unwrap();
        // Both are claimed, stepped and committed the same way.
        for id in [&run.id, &other.id] {
            st.claim_run(id, &d("d1"), 60).unwrap().unwrap();
            st.begin_step(id, &d("d1"), 1, StepKind::Turn).unwrap();
            let done = st
                .commit_bot_step(&bot(), id, &d("d1"), 1, StepCommit::default())
                .unwrap();
            assert_eq!(done.run.status, RunStatus::Running);
        }
        assert_eq!(
            st.run(&run.id).unwrap().unwrap().request,
            RequestKind::Thread
        );
        assert_eq!(
            st.active_run_on(&["t1".to_string()]).unwrap().unwrap().id,
            run.id
        );
    }

    /// Migration 64 relaxes `bot_runs` for every connection, not only the
    /// one that ran it: a second connection on the same file, which read
    /// the schema at 63 and prepared its statements then, takes the
    /// relaxed writes once the migration has run (the schema cookie moved,
    /// so it reads the schema again) and still refuses what no build
    /// allows.
    #[test]
    fn a_second_connection_takes_the_writes_migration_64_relaxes() {
        use crate::migrations::{NUMBERING, Numbering, WEB_MIGRATION};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let mut migrating = rusqlite::Connection::open(&path).unwrap();
        let at_63 = Numbering {
            migrations: &NUMBERING.migrations[..WEB_MIGRATION as usize],
            main_marks: NUMBERING.main_marks,
        };
        crate::migrate_with(&mut migrating, &at_63).unwrap();
        migrating
            .execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version,
                        contract_version)
                    VALUES ('7q3k9mzx2b4t', 'Budget', 'x', 'x', 1, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json)
                    VALUES ('7q3k9mzx2b4t', 1, 'x', '{}');
                 INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                        model, key_source, base_version, credits_milli, created_at, updated_at,
                        web)
                    VALUES ('01JC0000000000000000000001', '7q3k9mzx2b4t', '*owner', 'thread',
                        '[]', 'queued', 'm', 'own', 1, 0, 'x', 'x', '');",
            )
            .unwrap();
        // The other connection reads the schema at 63, and prepares its
        // writes on it.
        let other = rusqlite::Connection::open(&path).unwrap();
        let mut unbind = other
            .prepare("UPDATE bot_runs SET artifact_id = NULL, base_version = NULL WHERE id = ?1")
            .unwrap();
        let mut park = other
            .prepare("UPDATE bot_runs SET status = ?2 WHERE id = ?1")
            .unwrap();
        let run = "01JC0000000000000000000001";
        assert!(
            park.execute(rusqlite::params![run, "parked"]).is_err(),
            "at 63 no run parks"
        );
        crate::migrate_with(&mut migrating, &NUMBERING).unwrap();
        assert_eq!(unbind.execute([run]).unwrap(), 1);
        assert_eq!(park.execute(rusqlite::params![run, "parked"]).unwrap(), 1);
        assert!(
            park.execute(rusqlite::params![run, "napping"]).is_err(),
            "a status no build knows"
        );
        let (artifact, status): (Option<String>, String) = other
            .query_row(
                "SELECT artifact_id, status FROM bot_runs WHERE id = ?1",
                [run],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((artifact, status.as_str()), (None, "parked"));
    }

    #[test]
    fn minted_driver_ids_are_unique_across_threads() {
        let ids: Vec<DriverId> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8)
                .map(|_| s.spawn(|| (0..100).map(|_| DriverId::mint()).collect::<Vec<_>>()))
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        let set: std::collections::HashSet<&str> = ids.iter().map(DriverId::as_str).collect();
        assert_eq!(set.len(), ids.len());
        assert!(ids.iter().all(|d| d.as_str().starts_with("drv_")));
    }

    #[test]
    fn script_records_keep_the_code_by_hash() {
        let rec = script_record("t", "Edit", "return 1", &["files.read".into()], 3, 4, None);
        assert_eq!(
            rec.body["code_sha256"],
            format!("sha256:{}", clax_core::audit::sha256_hex(b"return 1"))
        );
        assert!(!rec.body.values().any(|v| v == "return 1"));
        let call = call_record("t", "files.write", Some("index.html"), true);
        assert_eq!(call.body["op"], "files.write");
    }
}
