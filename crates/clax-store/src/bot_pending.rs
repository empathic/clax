//! Calls that wait, and a run's end kept for delivery (migration 65, spec
//! 2026-11-10-claxbot-surfaces-design, "Calls that wait", "Deadlines,
//! retries and what a call may promise" and "A host's surface on a
//! daemon").
//!
//! A **pending call** (`bot_pending_calls`) is a call of a run's open
//! step that was sent to a registration ([`crate::bot_surfaces`]) and
//! whose answer is not in the run's journal yet. It is opened by the
//! driver that holds the run ([`Store::open_call`]), one for each place
//! `(run, step, seq)` of the journal, and it carries everything the
//! journal row of its place will hold but the answer: the operation as
//! the script named it, the hash of its arguments as the script step
//! computes it, and the attempt it was made in. It is opened before
//! the call is sent to its host, so that an answer, however early, finds
//! it: an answer may come before the run parks.
//!
//! A call a run may park on is marked `waits`. A script's calls reach
//! their host one at a time, and a waiting call ends the script there, so
//! a run has at most one such call waiting: [`Store::open_call`] refuses
//! a second (and the index `bot_pending_calls_waiting` holds the table
//! to it). Every other call is answered within the attempt that made it.
//!
//! The row is then the authority to journal the place. A parked run has
//! no driver ([`Store::park_run`]), so the answer is journaled by
//! whoever brings it ([`Store::answer_call`]) or by the deadline
//! ([`Store::expire_calls`]), under the pending row and no claim, in one
//! transaction with the row's end and the run's waking. The journal's
//! primary key holds a place to one row whoever writes it, and a call
//! stops waiting as soon as its place is journaled by anyone, its step
//! commits or its run is asked to stop, so an answer that comes after any
//! of them writes nothing.
//!
//! # The journal row
//!
//! [`Store::answer_call`] and [`Store::expire_calls`] write `bot_calls`
//! rows of the shape the run loop writes for a call its provider answered
//! with a value or a failure (`clax_bot::surface::Claim::record` for
//! `Answer::of`, through [`Store::record_bot_call`]) and reads back on
//! replay (`clax_bot::surface::recorded`):
//!
//! | Column | Holds |
//! |---|---|
//! | `run_id`, `step_n`, `seq` | The pending call's place |
//! | `attempt` | The attempt the call was first opened in, whichever attempt reads it |
//! | `op` | The pending call's `op` |
//! | `args_hash` | The pending call's `args_hash`, never computed here |
//! | `ok` | 1 for a value, 0 for a failure |
//! | `result_json` | The value, or `{"code", "message"}`, with `"retryable": true` when the failure says so |
//! | `at` | `now` |
//!
//! # What a deadline journals
//!
//! A call that reaches its deadline unanswered, or whose registration is
//! no longer live, is journaled as the failure the caller's function
//! gives for it. [`unanswered`] is the design's table of what a script
//! sees, read from the row:
//!
//! | The call was | Code |
//! |---|---|
//! | Never sent ([`Store::mark_sent`], [`Store::accept_call`]) | `unavailable`, retryable |
//! | Sent, a read | `expired` |
//! | Sent, a write or an effect | `unknown_outcome` |
//!
//! Whether a call is sent again before its deadline, and how often, is the
//! hub's (`clax_bot::bot_surfaces`); the store keeps one row for a place,
//! and with it where the call is with its attempts ([`CallTries`]), which
//! the hub writes ([`Store::keep_call_tries`]) and the store reads only to
//! count a run's sendings again ([`Store::call_resends`]) and to say when
//! one is next due ([`Store::next_call_resend`]).
//!
//! # A run's end
//!
//! `bot_run_ends` holds how a run ended for each registration that is to
//! be told, written in the terminal step's transaction
//! ([`Store::commit_bot_step_on`]) or after it
//! ([`Store::record_end`]), listed until acknowledged
//! ([`Store::pending_summary`], [`Store::ends_named`], [`Store::ack_end`]) and removed
//! [`BOT_END_KEEP`] after it was written ([`Store::prune_ended_runs`]).
//!
//! Every function here takes the time as `now`; none reads the store's
//! clock.

use super::Store;
use super::bot_runs::{
    CallEntry, DriverId, check_claim, insert_call, journal_at, journal_holds, open_step_in, run_in,
    unpark_in,
};
use super::bot_surfaces::registration_in;
use chrono::{DateTime, Utc};
use clax_core::time::stamp;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Value, json};

/// How long a run's end is kept for a registration that has not
/// acknowledged it.
pub const BOT_END_KEEP: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// What a pending call's operation is to its surface, which decides what
/// its deadline journals ([`unanswered`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallAccess {
    Read,
    Write,
    Effect,
}

impl CallAccess {
    pub const ALL: &'static [CallAccess] =
        &[CallAccess::Read, CallAccess::Write, CallAccess::Effect];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CallAccess::Read => "read",
            CallAccess::Write => "write",
            CallAccess::Effect => "effect",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<CallAccess> {
        CallAccess::ALL.iter().copied().find(|a| a.as_str() == s)
    }
}

/// What a call that was never answered is journaled as, by what is
/// known of it: whether it was sent to its host ([`PendingCall::sent_at`])
/// and what its operation is. What [`Store::expire_calls`] is given when
/// its caller has no wording of its own.
#[must_use]
pub fn unanswered(call: &PendingCall) -> CallFailure {
    let (code, message, retryable) = match (call.sent_at.is_some(), call.access) {
        (false, _) => (
            "unavailable",
            "no host took the call; nothing happened",
            true,
        ),
        (true, CallAccess::Read) => (
            "expired",
            "its host was sent the call and did not answer in time",
            false,
        ),
        (true, CallAccess::Write | CallAccess::Effect) => (
            "unknown_outcome",
            "its host was sent the call and did not answer in time; it may have taken effect",
            false,
        ),
    };
    CallFailure {
        code: code.into(),
        message: message.into(),
        retryable,
    }
}

/// `failure` as a journal row keeps it.
fn failure_json(failure: &CallFailure) -> Value {
    let mut v = json!({"code": failure.code, "message": failure.message});
    if failure.retryable {
        v["retryable"] = json!(true);
    }
    v
}

/// The rule a call's run gave for sending it again: the attempts it may
/// have in all, and the waits between them, which double up to a
/// longest (`clax_bot::bot_surfaces::Retry`, which this mirrors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RetryRule {
    pub attempts: u32,
    pub backoff_ms: u64,
    pub backoff_max_ms: u64,
}

/// Whether a call is to be sent again although its host accepted it
/// ([`CallTries::resend`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resend {
    /// No: sent as any open call is, and not once its host accepted it.
    #[default]
    None,
    /// An attempt failed, or its host may have lost it, and the next
    /// attempt is planned and not sent yet.
    Planned,
    /// Sent with every `serve` until its host accepts it again: the
    /// planned attempt was sent, or its host asked for it later.
    Due,
}

impl Resend {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Resend::None => "none",
            Resend::Planned => "planned",
            Resend::Due => "due",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Resend> {
        [Resend::None, Resend::Planned, Resend::Due]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

/// Where a pending call is with its attempts, kept with the call by
/// whoever sends it again (the hub of the host protocol,
/// `clax_bot::bot_surfaces`), so that it outlasts the process and a
/// call goes on after a restart as it would have. The store reads none
/// of it but `failures`, which it sums for a run's count
/// ([`Store::call_resends`]), and `not_before`
/// ([`Store::next_call_resend`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CallTries {
    /// The rule its run gave; `None`: none was given.
    pub rule: Option<RetryRule>,
    /// The attempts that failed and were followed by another, each one
    /// of the run's sendings again.
    pub failures: u32,
    /// The call is not sent before this instant.
    pub not_before: Option<String>,
    pub resend: Resend,
    /// Its host asked for it later, and it has not been sent since.
    pub later: bool,
    /// What its last attempt came to, while it has not been sent since:
    /// what its deadline settles it as.
    pub last: Option<CallFailure>,
}

/// A call sent to a registration, as `bot_pending_calls` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PendingCall {
    /// A ULID: what an answer names.
    pub id: String,
    pub run_id: String,
    pub step_n: i64,
    pub seq: i64,
    /// The attempt of the step the call was opened in.
    pub attempt: i64,
    pub registration_id: String,
    /// The hash of the definition the call was made under
    /// ([`crate::bot_surfaces::SurfaceDef::hash`]): what its answer is
    /// checked by, whatever its registration's definition is by then.
    pub def_hash: String,
    /// The operation as the run's journal names it: what the script
    /// called (`shell.settings.set`).
    pub op: String,
    /// The arguments, as the script passed them.
    pub args: Value,
    /// The hash of the arguments as the script step computes it
    /// (`clax_bot::host::args_hash`), given by the caller.
    pub args_hash: String,
    /// The key the provider is sent (`<run>:<step>:<seq>`).
    pub key: String,
    pub access: CallAccess,
    /// Whether the run may park on the call.
    pub waits: bool,
    pub created_at: String,
    /// The call's deadline; `None`: it has none.
    pub expires_at: Option<String>,
    /// When the call was first handed to its host ([`Store::mark_sent`]),
    /// or accepted by it.
    pub sent_at: Option<String>,
    pub accepted_at: Option<String>,
    /// When the call stopped waiting: answered, expired, or found
    /// settled another way.
    pub answered_at: Option<String>,
    /// Where it is with its attempts.
    pub tries: CallTries,
}

/// What is read of a waiting call to decide what to do with it:
/// everything but its arguments ([`Store::pending_summary`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallSummary {
    pub id: String,
    pub run_id: String,
    pub def_hash: String,
    pub op: String,
    pub access: CallAccess,
    pub expires_at: Option<String>,
    /// The bytes of its arguments as JSON, as the store keeps them.
    pub args_bytes: usize,
    /// Whether it was handed to its host, or accepted by it.
    pub sent: bool,
    pub accepted: bool,
    /// Where it is with its attempts.
    pub tries: CallTries,
}

/// The latest instant a pending call's stamps hold: the last
/// millisecond of the year 9999. A later one would be written with a
/// sign (`+10000-…`), which sorts before every stamp of the years 0 to
/// 9999, so a deadline past it is kept as this one, which no clock
/// reaches.
#[must_use]
pub fn latest_stamp() -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59)
        .single()
        .expect("a valid instant")
        + chrono::Duration::milliseconds(999)
}

/// `at`, no later than [`latest_stamp`].
fn bounded(at: DateTime<Utc>) -> DateTime<Utc> {
    at.min(latest_stamp())
}

/// Whether the columns of a `bot_pending_calls` row that [`Place`]
/// reads read: what journaling its place needs.
const PLACE_READS: &str = "COALESCE(typeof(id) = 'text' AND typeof(run_id) = 'text' \
     AND typeof(step_n) = 'integer' AND typeof(seq) = 'integer' AND typeof(attempt) = 'integer' \
     AND typeof(op) = 'text' AND typeof(args_hash) = 'text' \
     AND typeof(waits) = 'integer' AND waits IN (0, 1), 0)";

/// Whether a `bot_pending_calls` row reads: every column a call is read
/// by ([`row_to_pending`], [`Store::pending_summary`]) holds a value this
/// build reads. Its arguments are a JSON array, its access and resend
/// state ones this build knows, its counts whole numbers in range, its
/// rule of attempts given whole or not at all, and its last failure,
/// when it has one, whole. Written so that no value is an error of the
/// query, and no row's is NULL.
const CALL_READS: &str = "COALESCE(CASE WHEN json_valid(args_json) THEN json_type(args_json) = 'array' \
     ELSE 0 END \
     AND (typeof(id) = 'text' AND typeof(run_id) = 'text' \
     AND typeof(step_n) = 'integer' AND typeof(seq) = 'integer' AND typeof(attempt) = 'integer' \
     AND typeof(op) = 'text' AND typeof(args_hash) = 'text' \
     AND typeof(waits) = 'integer' AND waits IN (0, 1)) \
     AND typeof(registration_id) = 'text' AND typeof(def_hash) = 'text' \
     AND typeof(key) = 'text' AND typeof(created_at) = 'text' \
     AND typeof(expires_at) IN ('null', 'text') AND typeof(sent_at) IN ('null', 'text') \
     AND typeof(accepted_at) IN ('null', 'text') AND typeof(answered_at) IN ('null', 'text') \
     AND typeof(not_before) IN ('null', 'text') \
     AND access IN ('read', 'write', 'effect') AND resend IN ('none', 'planned', 'due') \
     AND typeof(later) = 'integer' AND later IN (0, 1) \
     AND typeof(failures) = 'integer' AND failures BETWEEN 0 AND 4294967295 \
     AND ((retry_attempts IS NULL AND retry_backoff_ms IS NULL \
           AND retry_backoff_max_ms IS NULL) \
          OR (typeof(retry_attempts) = 'integer' AND typeof(retry_backoff_ms) = 'integer' \
              AND typeof(retry_backoff_max_ms) = 'integer' \
              AND retry_attempts BETWEEN 0 AND 4294967295 AND retry_backoff_ms >= 0 \
              AND retry_backoff_max_ms >= 0)) \
     AND (last_code IS NULL \
          OR (typeof(last_code) = 'text' AND typeof(last_message) = 'text' \
              AND typeof(last_retryable) = 'integer' AND last_retryable IN (0, 1))), 0)";

/// Whether a `bot_run_ends` row reads: its end is a JSON object and its
/// result, when it has one, JSON.
const END_READS: &str = "(CASE WHEN json_valid(end_json) THEN json_type(end_json) = 'object' \
     ELSE 0 END AND (result_json IS NULL OR json_valid(result_json)))";

/// The most unacknowledged ends one [`Store::expire_ends`] forgets: as
/// many as a transaction of the prune deletes rows
/// ([`crate::bot_runs::PRUNE_ROWS_PER_TX`]).
pub const EXPIRE_ENDS_PER_TX: usize = crate::bot_runs::PRUNE_ROWS_PER_TX;

/// What a pending call whose row does not read is journaled as by
/// [`Store::expire_calls`]: no host is sent it.
fn unreadable_failure() -> CallFailure {
    CallFailure {
        code: "internal_error".into(),
        message: "the call as the store keeps it does not read (a value of its row is not one \
                  this build reads): it is sent to no host"
            .into(),
        retryable: false,
    }
}

/// What is read of an unacknowledged end to decide what to do with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EndSummary {
    pub id: String,
    /// The bytes of its result as JSON; 0 for none.
    pub result_bytes: usize,
}

/// What waits for a registration ([`Store::pending_summary`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PendingSummary {
    /// Its waiting calls, oldest first.
    pub calls: Vec<CallSummary>,
    /// Its unacknowledged ends, oldest first.
    pub ends: Vec<EndSummary>,
}

/// What [`Store::record_end`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeptEnd {
    /// The end is kept: its ID ([`RunEnd::id`]).
    New(String),
    /// An unacknowledged end of the run is kept for the registration
    /// already: its ID. Nothing changed.
    Same(String),
    /// The registration has as many unacknowledged ends as the limit.
    /// Nothing changed.
    Full,
}

/// A call to open ([`Store::open_call`]).
#[derive(Clone, Debug)]
pub struct NewCall {
    pub step_n: i64,
    pub seq: i64,
    pub attempt: i64,
    pub registration_id: String,
    /// [`PendingCall::def_hash`].
    pub def_hash: String,
    /// [`PendingCall::op`].
    pub op: String,
    pub args: Value,
    /// [`PendingCall::args_hash`].
    pub args_hash: String,
    pub key: String,
    pub access: CallAccess,
    /// [`PendingCall::waits`].
    pub waits: bool,
    /// The deadline; `None` for a call with none.
    pub expires_at: Option<DateTime<Utc>>,
}

/// What [`Store::open_call`] did.
#[derive(Clone, Debug, PartialEq)]
pub enum Opened {
    /// The call is new.
    New(PendingCall),
    /// The place had this call already; nothing changed.
    Same(PendingCall),
}

impl Opened {
    /// The call, new or not.
    #[must_use]
    pub fn call(&self) -> &PendingCall {
        match self {
            Opened::New(call) | Opened::Same(call) => call,
        }
    }

    /// The call, new or not.
    #[must_use]
    pub fn into_call(self) -> PendingCall {
        match self {
            Opened::New(call) | Opened::Same(call) => call,
        }
    }
}

/// A failure a provider answered a call with, as the journal keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallFailure {
    pub code: String,
    pub message: String,
    /// Whether the provider said the call may be made again.
    pub retryable: bool,
}

/// Why an accept or an answer changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ignored {
    /// No pending call has the ID.
    Unknown,
    /// The call was sent to another registration.
    NotYours,
    /// The registration is no longer live.
    Replaced,
    /// The call stopped waiting already: it was answered, its deadline
    /// was journaled, its place was journaled by the run's driver, its
    /// step committed without it, or its run was asked to stop.
    Answered,
    /// The call's deadline passed; [`Store::expire_calls`] journals it.
    Expired,
}

/// What [`Store::accept_call`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// The call is accepted as of this call.
    Accepted,
    /// It was accepted before; nothing changed.
    AlreadyAccepted,
    Ignored(Ignored),
}

/// What [`Store::answer_call`] did.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerOutcome {
    /// The answer is the journal row of the call's place, and the run is
    /// due.
    Journaled {
        run_id: String,
        /// The row written.
        entry: CallEntry,
        /// Whether the run was parked and this answer made it claimable
        /// again: the one time its driver needs waking for it.
        unparked: bool,
    },
    /// The same answer was journaled at the call's place before: the
    /// value, or the failure, the journal row holds is this one. Nothing
    /// changed. A host whose reply to its first answer was lost is told
    /// by this that its answer stands.
    AlreadyJournaled,
    /// Nothing was journaled and no run was made due.
    Ignored(Ignored),
}

/// One call [`Store::expire_calls`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiredCall {
    pub id: String,
    pub run_id: String,
    pub step_n: i64,
    pub seq: i64,
    /// The code of the failure journaled for it; `None` when nothing was
    /// journaled: its place had a row, its step had committed or its run
    /// had ended.
    pub code: Option<String>,
    /// [`AnswerOutcome::Journaled::unparked`].
    pub unparked: bool,
}

/// How a run ended, for one registration to be told
/// ([`Store::commit_bot_step_on`], [`Store::record_end`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EndFor {
    pub registration_id: String,
    /// The end as the protocol spells it (`{"finished": {}}`): the
    /// store reads none of it.
    pub end: Value,
    /// The kind's result, for the registration that declared the kind.
    pub result: Option<Value>,
}

/// A run's end as `bot_run_ends` keeps it for one registration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunEnd {
    /// A ULID: what an acknowledgement names. No pending call has it.
    pub id: String,
    pub run_id: String,
    pub registration_id: String,
    pub end: Value,
    pub result: Option<Value>,
    pub created_at: String,
    /// When the registration acknowledged it.
    pub delivered_at: Option<String>,
}

/// The columns of a call's attempts ([`CallTries`]), as [`tries_at`]
/// reads them.
const TRIES_COLS: &str = "retry_attempts, retry_backoff_ms, retry_backoff_max_ms, failures,
    not_before, resend, later, last_code, last_message, last_retryable";

const PENDING_COLS: &str = "id, run_id, step_n, seq, attempt, registration_id, op, args_json,
    args_hash, key, access, created_at, expires_at, accepted_at, answered_at, waits, sent_at,
    def_hash, retry_attempts, retry_backoff_ms, retry_backoff_max_ms, failures, not_before,
    resend, later, last_code, last_message, last_retryable";

/// A column whose stored value this build does not read: refused, never
/// read as a default.
fn unread(column: usize, what: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, what.into())
}

fn access_at(r: &Row<'_>, column: usize) -> rusqlite::Result<CallAccess> {
    let access: String = r.get(column)?;
    CallAccess::parse(&access).ok_or_else(|| {
        unread(
            column,
            format!("bot_pending_calls.access is {access:?}, which this build does not know"),
        )
    })
}

/// [`TRIES_COLS`] of a row, from column `at`.
fn tries_at(r: &Row<'_>, at: usize) -> rusqlite::Result<CallTries> {
    let count = |column: usize| -> rusqlite::Result<Option<u64>> {
        let n: Option<i64> = r.get(column)?;
        n.map(|n| {
            u64::try_from(n)
                .map_err(|_| unread(column, format!("bot_pending_calls holds {n} as a count")))
        })
        .transpose()
    };
    let small = |column: usize, n: u64| {
        u32::try_from(n)
            .map_err(|_| unread(column, format!("bot_pending_calls holds {n} as a count")))
    };
    let rule = match (count(at)?, count(at + 1)?, count(at + 2)?) {
        (Some(attempts), Some(backoff_ms), Some(backoff_max_ms)) => Some(RetryRule {
            attempts: small(at, attempts)?,
            backoff_ms,
            backoff_max_ms,
        }),
        (None, None, None) => None,
        _ => {
            return Err(unread(
                at,
                "bot_pending_calls holds a part of a call's retry rule".into(),
            ));
        }
    };
    let resend: String = r.get(at + 5)?;
    let last_code: Option<String> = r.get(at + 7)?;
    let last = match last_code {
        Some(code) => Some(CallFailure {
            code,
            message: r.get(at + 8)?,
            retryable: r.get(at + 9)?,
        }),
        None => None,
    };
    let failures: i64 = r.get(at + 3)?;
    let failures = u64::try_from(failures).map_err(|_| {
        unread(
            at + 3,
            format!("bot_pending_calls holds {failures} failures"),
        )
    })?;
    Ok(CallTries {
        rule,
        failures: small(at + 3, failures)?,
        not_before: r.get(at + 4)?,
        resend: Resend::parse(&resend).ok_or_else(|| {
            unread(
                at + 5,
                format!("bot_pending_calls.resend is {resend:?}, which this build does not know"),
            )
        })?,
        later: r.get(at + 6)?,
        last,
    })
}

fn row_to_pending(r: &Row<'_>) -> rusqlite::Result<PendingCall> {
    let args: String = r.get(7)?;
    let args: Value = serde_json::from_str(&args)
        .ok()
        .filter(Value::is_array)
        .ok_or_else(|| unread(7, "bot_pending_calls.args_json is not a JSON array".into()))?;
    Ok(PendingCall {
        id: r.get(0)?,
        run_id: r.get(1)?,
        step_n: r.get(2)?,
        seq: r.get(3)?,
        attempt: r.get(4)?,
        registration_id: r.get(5)?,
        op: r.get(6)?,
        args,
        args_hash: r.get(8)?,
        key: r.get(9)?,
        access: access_at(r, 10)?,
        created_at: r.get(11)?,
        expires_at: r.get(12)?,
        accepted_at: r.get(13)?,
        answered_at: r.get(14)?,
        waits: r.get(15)?,
        sent_at: r.get(16)?,
        def_hash: r.get(17)?,
        tries: tries_at(r, 18)?,
    })
}

/// The columns [`Place`] reads.
const PLACE_COLS: &str = "id, run_id, step_n, seq, attempt, op, args_hash, waits";

/// What journaling a pending call's place needs of its row
/// ([`settle`]): its place, and what the journal row of it holds but
/// the answer. A row whose arguments do not read has these.
struct Place {
    id: String,
    run_id: String,
    step_n: i64,
    seq: i64,
    attempt: i64,
    op: String,
    args_hash: String,
    waits: bool,
}

impl Place {
    fn of(call: &PendingCall) -> Place {
        Place {
            id: call.id.clone(),
            run_id: call.run_id.clone(),
            step_n: call.step_n,
            seq: call.seq,
            attempt: call.attempt,
            op: call.op.clone(),
            args_hash: call.args_hash.clone(),
            waits: call.waits,
        }
    }
}

fn row_to_place(r: &Row<'_>) -> rusqlite::Result<Place> {
    Ok(Place {
        id: r.get(0)?,
        run_id: r.get(1)?,
        step_n: r.get(2)?,
        seq: r.get(3)?,
        attempt: r.get(4)?,
        op: r.get(5)?,
        args_hash: r.get(6)?,
        waits: r.get(7)?,
    })
}

fn pending_in(c: &Connection, id: &str) -> Result<Option<PendingCall>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {PENDING_COLS} FROM bot_pending_calls WHERE id = ?1"
    ))?
    .query_row(params![id], row_to_pending)
    .optional()?)
}

fn pending_at(c: &Connection, run_id: &str, step_n: i64, seq: i64) -> Result<Option<PendingCall>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {PENDING_COLS} FROM bot_pending_calls
          WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3"
    ))?
    .query_row(params![run_id, step_n, seq], row_to_pending)
    .optional()?)
}

const END_COLS: &str =
    "id, run_id, registration_id, end_json, result_json, created_at, delivered_at";

fn row_to_end(r: &Row<'_>) -> rusqlite::Result<RunEnd> {
    let end: String = r.get(3)?;
    let end: Value = serde_json::from_str(&end)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| unread(3, "bot_run_ends.end_json is not a JSON object".into()))?;
    let result: Option<String> = r.get(4)?;
    let result = result
        .map(|v| {
            serde_json::from_str(&v)
                .map_err(|e| unread(4, format!("bot_run_ends.result_json is not JSON: {e}")))
        })
        .transpose()?;
    Ok(RunEnd {
        id: r.get(0)?,
        run_id: r.get(1)?,
        registration_id: r.get(2)?,
        end,
        result,
        created_at: r.get(5)?,
        delivered_at: r.get(6)?,
    })
}

/// Pending call `id` for registration `registration` at `at`, when it is
/// still waiting on that registration; else why not.
fn waiting(
    c: &Connection,
    id: &str,
    registration: &str,
    at: &str,
) -> Result<std::result::Result<PendingCall, Ignored>> {
    // A row that does not read takes no answer: it stops waiting, settled
    // `internal_error`, when the calls are next expired.
    let reads: Option<bool> = c
        .prepare_cached(&format!(
            "SELECT {CALL_READS} FROM bot_pending_calls WHERE id = ?1"
        ))?
        .query_row(params![id], |r| r.get(0))
        .optional()?;
    match reads {
        None => return Ok(Err(Ignored::Unknown)),
        Some(false) => return Ok(Err(Ignored::Answered)),
        Some(true) => {}
    }
    let Some(call) = pending_in(c, id)? else {
        return Ok(Err(Ignored::Unknown));
    };
    if call.registration_id != registration {
        return Ok(Err(Ignored::NotYours));
    }
    if call.answered_at.is_some() {
        return Ok(Err(Ignored::Answered));
    }
    if registration_in(c, registration)?.is_none_or(|r| !r.is_live()) {
        return Ok(Err(Ignored::Replaced));
    }
    if call.expires_at.as_deref().is_some_and(|e| e <= at) {
        return Ok(Err(Ignored::Expired));
    }
    Ok(Ok(call))
}

/// What became of journaling a pending call's place.
enum Settle {
    /// The row was written, and whether that unparked the run.
    Journaled(CallEntry, bool),
    /// Nothing was written: the place had its row already, the step is
    /// no longer the run's open step, or the run has ended.
    Answered,
}

/// Ends `call`'s wait at `at` on `c`: journals `ok` and `result` at its
/// place and makes its run due, unparking it when the call is one the
/// run may park on ([`PendingCall::waits`]), unless the place has a row,
/// its step is no longer the run's open step or its run has ended. The
/// call stops waiting in every case.
fn settle(c: &Connection, call: &Place, ok: bool, result: Value, at: &str) -> Result<Settle> {
    c.execute(
        "UPDATE bot_pending_calls SET answered_at = ?2 WHERE id = ?1",
        params![call.id, at],
    )?;
    let open = match run_in(c, &call.run_id)? {
        Some(run) if !run.status.is_terminal() => open_step_in(c, &call.run_id)?,
        _ => None,
    };
    if open.is_none_or(|s| s.n != call.step_n)
        || journal_holds(c, &call.run_id, call.step_n, call.seq)?
    {
        return Ok(Settle::Answered);
    }
    let entry = CallEntry {
        run_id: call.run_id.clone(),
        step_n: call.step_n,
        seq: call.seq,
        attempt: call.attempt,
        op: call.op.clone(),
        args_hash: call.args_hash.clone(),
        ok,
        result,
    };
    insert_call(c, &entry, at)?;
    // Only the call a run may park on takes the run out of its parking:
    // any other call's row is journaled, and a run parked on another
    // call stays parked on it, with its deadline.
    let unparked = call.waits && unpark_in(c, &call.run_id, at)?;
    c.execute(
        "UPDATE bot_runs SET due_at = NULL WHERE id = ?1 AND (?2 OR status != 'parked')",
        params![call.run_id, call.waits],
    )?;
    Ok(Settle::Journaled(entry, unparked))
}

impl Store {
    /// Opens the call `call` of run `run_id`'s open step for `driver`,
    /// which holds the run's claim: one pending call for the place. When
    /// the place has one already with the same registration, operation,
    /// arguments' hash and key (the step run again after a crash or a
    /// park), that one is returned as it is, answered or not, with the
    /// attempt and deadline it was first opened with
    /// ([`Opened::Same`]).
    ///
    /// The caller opens the call before it sends it to its host: the
    /// claim is needed here, and an answer may come before the run parks.
    ///
    /// # Errors
    /// `NotFound` for a run or a registration that does not exist;
    /// `run_not_claimed` unless `driver` holds the run; `run_invalid`
    /// when step `call.step_n` is not the run's open step; `key_reused`
    /// when the place has a pending call of another registration,
    /// operation, arguments or key; `run_invalid` for a call that waits
    /// while another call of the run that waits is still waiting;
    /// `unavailable` for a registration that is no longer live (nothing
    /// is opened: the caller journals the failure under its claim);
    /// `registration_full` when the registration has `limit` calls
    /// waiting (a place that has its call is answered for before the
    /// limit is looked at); the store's.
    pub fn open_call(
        &self,
        run_id: &str,
        driver: &DriverId,
        call: NewCall,
        limit: Option<usize>,
        now: DateTime<Utc>,
    ) -> Result<Opened> {
        self.open_call_with(run_id, driver, call, None, limit, now)
    }

    /// [`Store::open_call`], keeping with a new call the rule its run
    /// gave for sending it again ([`CallTries::rule`]). A deadline later
    /// than [`latest_stamp`] is kept as that. A call the place has
    /// already keeps the rule it has.
    ///
    /// # Errors
    /// As [`Store::open_call`].
    pub fn open_call_with(
        &self,
        run_id: &str,
        driver: &DriverId,
        call: NewCall,
        rule: Option<RetryRule>,
        limit: Option<usize>,
        now: DateTime<Utc>,
    ) -> Result<Opened> {
        let at = stamp(now);
        let big = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        self.with_tx(|tx| {
            let run = run_in(tx, run_id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver.as_str())?;
            if open_step_in(tx, run_id)?.is_none_or(|s| s.n != call.step_n) {
                return Err(CoreError::invalid(
                    "run_invalid",
                    format!(
                        "step {} of run {run_id} is not begun and uncommitted",
                        call.step_n
                    ),
                ));
            }
            if let Some(prev) = pending_at(tx, run_id, call.step_n, call.seq)? {
                if prev.registration_id != call.registration_id
                    || prev.op != call.op
                    || prev.args_hash != call.args_hash
                    || prev.key != call.key
                {
                    return Err(CoreError::invalid(
                        "key_reused",
                        format!(
                            "call {} of step {} of run {run_id} is waiting as {} with other \
                             arguments, another key or another registration",
                            call.seq, call.step_n, prev.op
                        ),
                    ));
                }
                return Ok(Opened::Same(prev));
            }
            if call.waits {
                let waiting: Option<i64> = tx
                    .query_row(
                        "SELECT seq FROM bot_pending_calls
                          WHERE run_id = ?1 AND waits = 1 AND answered_at IS NULL",
                        params![run_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(seq) = waiting {
                    return Err(CoreError::invalid(
                        "run_invalid",
                        format!(
                            "run {run_id} waits on call {seq} already; a run waits on one call \
                             at a time"
                        ),
                    ));
                }
            }
            let registration =
                registration_in(tx, &call.registration_id)?.ok_or(CoreError::NotFound)?;
            if !registration.is_live() {
                return Err(CoreError::invalid(
                    "unavailable",
                    format!(
                        "registration {} of surface {} is no longer live",
                        registration.id, registration.name
                    ),
                ));
            }
            if let Some(limit) = limit {
                let waiting: usize = tx.query_row(
                    "SELECT count(*) FROM bot_pending_calls
                      WHERE registration_id = ?1 AND answered_at IS NULL",
                    params![call.registration_id],
                    |r| r.get(0),
                )?;
                if waiting >= limit {
                    return Err(CoreError::invalid(
                        "registration_full",
                        format!(
                            "registration {} has {waiting} calls waiting, its limit",
                            registration.id
                        ),
                    ));
                }
            }
            let id = clax_core::new_ulid();
            if !call.args.is_array() {
                return Err(CoreError::invalid(
                    "invalid_args",
                    "a pending call's arguments are a JSON array",
                ));
            }
            tx.execute(
                "INSERT INTO bot_pending_calls (id, run_id, step_n, seq, attempt, registration_id,
                    op, args_json, args_hash, key, access, created_at, expires_at, waits,
                    def_hash, retry_attempts, retry_backoff_ms, retry_backoff_max_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18)",
                params![
                    id,
                    run_id,
                    call.step_n,
                    call.seq,
                    call.attempt,
                    call.registration_id,
                    call.op,
                    call.args.to_string(),
                    call.args_hash,
                    call.key,
                    call.access.as_str(),
                    at,
                    call.expires_at.map(|e| stamp(bounded(e))),
                    call.waits,
                    call.def_hash,
                    rule.map(|r| r.attempts),
                    rule.map(|r| big(r.backoff_ms)),
                    rule.map(|r| big(r.backoff_max_ms)),
                ],
            )?;
            Ok(Opened::New(
                pending_in(tx, &id)?.expect("the call was just opened"),
            ))
        })
    }

    /// Pending call `id`, waiting or not, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn pending_call(&self, id: &str) -> Result<Option<PendingCall>> {
        self.with_read(|c| pending_in(c, id))
    }

    /// The pending call of place `(step_n, seq)` of run `run_id`, waiting
    /// or not, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn pending_call_at(
        &self,
        run_id: &str,
        step_n: i64,
        seq: i64,
    ) -> Result<Option<PendingCall>> {
        self.with_read(|c| pending_at(c, run_id, step_n, seq))
    }

    /// Records, at `now`, that each of the pending calls `ids` was handed
    /// to its host: one sent and never answered is journaled at its
    /// deadline as one that may have happened ([`unanswered`]). A call
    /// sent before keeps its first time; one that is not waiting, or
    /// unknown, is left. How many it marked as sent for the first time.
    ///
    /// Each call that is waiting is also no longer one whose next
    /// attempt waits to be sent: a planned attempt is sent
    /// ([`Resend::Planned`] becomes [`Resend::Due`]), its host's ask for
    /// later is answered, and its last failure is no longer what is
    /// known of it ([`CallTries`]).
    ///
    /// # Errors
    /// The store's.
    pub fn mark_sent(&self, ids: &[String], now: DateTime<Utc>) -> Result<usize> {
        let at = stamp(now);
        self.with_tx(|tx| {
            let mut marked = 0;
            for id in ids {
                marked += tx.execute(
                    "UPDATE bot_pending_calls SET sent_at = ?2
                      WHERE id = ?1 AND sent_at IS NULL AND answered_at IS NULL",
                    params![id, at],
                )?;
                tx.execute(
                    "UPDATE bot_pending_calls
                        SET resend = CASE resend WHEN 'planned' THEN 'due' ELSE resend END,
                            later = 0, last_code = NULL, last_message = NULL,
                            last_retryable = NULL
                      WHERE id = ?1 AND answered_at IS NULL",
                    params![id],
                )?;
            }
            Ok(marked)
        })
    }

    /// Records that `registration` has pending call `id`, at `now`, and
    /// so that it was sent ([`Store::mark_sent`]). A call that is not
    /// waiting on `registration` at `now` is left as it is, and the
    /// outcome says why. A call that is accepted, now or before, is no
    /// longer one to be sent again: its host has it ([`Resend::None`],
    /// and neither an ask for later nor a last failure is kept).
    ///
    /// # Errors
    /// The store's.
    pub fn accept_call(
        &self,
        id: &str,
        registration: &str,
        now: DateTime<Utc>,
    ) -> Result<AcceptOutcome> {
        let at = stamp(now);
        self.with_tx(|tx| {
            let call = match waiting(tx, id, registration, &at)? {
                Ok(call) => call,
                Err(why) => return Ok(AcceptOutcome::Ignored(why)),
            };
            tx.execute(
                "UPDATE bot_pending_calls
                    SET accepted_at = COALESCE(accepted_at, ?2), sent_at = COALESCE(sent_at, ?2),
                        resend = 'none', later = 0, last_code = NULL, last_message = NULL,
                        last_retryable = NULL
                  WHERE id = ?1",
                params![id, at],
            )?;
            Ok(if call.accepted_at.is_some() {
                AcceptOutcome::AlreadyAccepted
            } else {
                AcceptOutcome::Accepted
            })
        })
    }

    /// Takes `registration`'s answer to pending call `id` at `now`: in
    /// one transaction, writes the journal row of the call's place (the
    /// module documentation gives its shape), ends the call's wait, and
    /// makes its run due at once, unparking it when it was parked
    /// ([`Store::unpark_run`]). No claim is asked for: the pending call,
    /// opened under one, names the place.
    ///
    /// It works the same while a driver holds the run (it is `running`,
    /// perhaps about to park): the row is written, the run stays its
    /// driver's, and that driver's park finds the place answered
    /// ([`Store::park_run`]).
    ///
    /// Nothing is journaled and no run is made due, and the outcome says
    /// why, when the ID names no pending call, the call is another
    /// registration's, `registration` is no longer live, the call's
    /// deadline has passed (left for [`Store::expire_calls`]), or the
    /// call stopped waiting before: it was answered, its place was
    /// journaled by the run's driver, its step committed, or its run was
    /// asked to stop.
    ///
    /// # Errors
    /// The store's.
    pub fn answer_call(
        &self,
        id: &str,
        registration: &str,
        result: std::result::Result<Value, CallFailure>,
        now: DateTime<Utc>,
    ) -> Result<AnswerOutcome> {
        let at = stamp(now);
        let (ok, result) = match result {
            Ok(value) => (true, value),
            Err(failure) => (false, failure_json(&failure)),
        };
        self.with_tx(|tx| {
            let call = match waiting(tx, id, registration, &at)? {
                Ok(call) => call,
                Err(Ignored::Answered) => {
                    // The same answer, journaled before: it stands.
                    let place = tx
                        .prepare_cached(&format!(
                            "SELECT {PLACE_COLS} FROM bot_pending_calls
                              WHERE id = ?1 AND {PLACE_READS}"
                        ))?
                        .query_row(params![id], row_to_place)
                        .optional()?;
                    let same = match place {
                        // A row whose result does not read is no answer
                        // that stands.
                        Some(p) => match journal_at(tx, &p.run_id, p.step_n, p.seq) {
                            Ok(row) => row.is_some_and(|row| row.ok == ok && row.result == result),
                            Err(e) if crate::bot_runs::is_unreadable(&e) => false,
                            Err(e) => return Err(e),
                        },
                        None => false,
                    };
                    return Ok(if same {
                        AnswerOutcome::AlreadyJournaled
                    } else {
                        AnswerOutcome::Ignored(Ignored::Answered)
                    });
                }
                Err(why) => return Ok(AnswerOutcome::Ignored(why)),
            };
            Ok(match settle(tx, &Place::of(&call), ok, result, &at)? {
                Settle::Journaled(entry, unparked) => AnswerOutcome::Journaled {
                    run_id: call.run_id,
                    entry,
                    unparked,
                },
                Settle::Answered => AnswerOutcome::Ignored(Ignored::Answered),
            })
        })
    }

    /// Ends, at `now`, the wait of every pending call whose deadline is
    /// `now` or earlier, and of every one whose registration is no longer
    /// live: each is journaled at its place as the failure `failure`
    /// gives for it ([`unanswered`], or the caller's own wording), and
    /// its run made due at once, unparked when it was parked, as an
    /// answer would ([`Store::answer_call`]). A call whose place is in
    /// the journal, whose step committed or whose run ended stops waiting
    /// and journals nothing. Every waiting call whose row does not read
    /// (a value of it is not one this build reads), whatever its
    /// deadline, is journaled `internal_error`, and `failure` is not
    /// asked; one whose place does not read either stops waiting,
    /// unjournaled. Each is logged. Returns the calls it ended, those
    /// that do not read first and each group oldest first; run again at
    /// the same time it finds none.
    ///
    /// # Errors
    /// The store's.
    pub fn expire_calls(
        &self,
        now: DateTime<Utc>,
        failure: &dyn Fn(&PendingCall) -> CallFailure,
    ) -> Result<Vec<ExpiredCall>> {
        self.expire_calls_where(None, now, failure)
    }

    /// [`Store::expire_calls`] for the pending calls of run `run_id`
    /// alone: what a driver that finds one run past the deadline it is
    /// parked until runs, so that no other run is made due behind its
    /// own driver's back.
    ///
    /// # Errors
    /// The store's.
    pub fn expire_calls_of(
        &self,
        run_id: &str,
        now: DateTime<Utc>,
        failure: &dyn Fn(&PendingCall) -> CallFailure,
    ) -> Result<Vec<ExpiredCall>> {
        self.expire_calls_where(Some(run_id), now, failure)
    }

    fn expire_calls_where(
        &self,
        run_id: Option<&str>,
        now: DateTime<Utc>,
        failure: &dyn Fn(&PendingCall) -> CallFailure,
    ) -> Result<Vec<ExpiredCall>> {
        let at = stamp(now);
        // The pending calls a pass ends: every waiting one whose row does
        // not read, whatever its deadline, journaled `internal_error` by
        // its place alone; and of those that read, those past their
        // deadline and those of a registration that is no longer live.
        // Scoped to one run, the pass reads that run's rows alone (its
        // place's unique index).
        let scope = if run_id.is_some() {
            "AND run_id = ?2"
        } else {
            ""
        };
        let due = format!(
            "FROM bot_pending_calls p
              WHERE answered_at IS NULL {scope}
                AND ((expires_at IS NOT NULL AND expires_at <= ?1)
                     OR NOT EXISTS (SELECT 1 FROM bot_registrations g
                                     WHERE g.id = p.registration_id
                                       AND g.replaced_at IS NULL))
                AND {CALL_READS}
              ORDER BY created_at, id"
        );
        self.with_tx(|tx| {
            // A row whose place does not read either cannot be journaled:
            // it stops waiting, so that nothing reads it again.
            let lost = {
                let sql = format!(
                    "UPDATE bot_pending_calls SET answered_at = ?1
                      WHERE answered_at IS NULL {scope} AND NOT {PLACE_READS}"
                );
                match run_id {
                    Some(run) => tx.execute(&sql, params![at, run])?,
                    None => tx.execute(&sql, params![at])?,
                }
            };
            if lost > 0 {
                tracing::warn!(
                    "clax-store: {lost} pending calls whose place does not read stop waiting, \
                     unjournaled"
                );
            }
            let unread: Vec<Place> = {
                let mut q = tx.prepare_cached(&format!(
                    "SELECT {PLACE_COLS} FROM bot_pending_calls
                      WHERE answered_at IS NULL {} AND NOT {CALL_READS}
                      ORDER BY rowid",
                    if run_id.is_some() {
                        "AND run_id = ?1"
                    } else {
                        ""
                    }
                ))?;
                let rows = match run_id {
                    Some(run) => q.query_map(params![run], row_to_place)?,
                    None => q.query_map([], row_to_place)?,
                };
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut ended = Vec::new();
            for place in unread {
                tracing::warn!(
                    "clax-store: pending call {} of run {} does not read as the store keeps it: \
                     settled internal_error",
                    place.id,
                    place.run_id
                );
                let failed = unreadable_failure();
                let row = failure_json(&failed);
                let (code, unparked) = match settle(tx, &place, false, row, &at)? {
                    Settle::Journaled(_, unparked) => (Some(failed.code), unparked),
                    Settle::Answered => (None, false),
                };
                ended.push(ExpiredCall {
                    id: place.id,
                    run_id: place.run_id,
                    step_n: place.step_n,
                    seq: place.seq,
                    code,
                    unparked,
                });
            }
            let due: Vec<PendingCall> = {
                let mut q = tx.prepare_cached(&format!("SELECT {PENDING_COLS} {due}"))?;
                let rows = match run_id {
                    Some(run) => q.query_map(params![at, run], row_to_pending)?,
                    None => q.query_map(params![at], row_to_pending)?,
                };
                rows.collect::<rusqlite::Result<_>>()?
            };
            ended.reserve(due.len());
            for call in due {
                let failed = failure(&call);
                let row = failure_json(&failed);
                let (code, unparked) = match settle(tx, &Place::of(&call), false, row, &at)? {
                    Settle::Journaled(_, unparked) => (Some(failed.code), unparked),
                    Settle::Answered => (None, false),
                };
                ended.push(ExpiredCall {
                    id: call.id,
                    run_id: call.run_id,
                    step_n: call.step_n,
                    seq: call.seq,
                    code,
                    unparked,
                });
            }
            Ok(ended)
        })
    }

    /// Keeps how run `run_id` ended for registration `end.registration_id`
    /// to be told, at `now`, when the terminal step's commit did not
    /// ([`Store::commit_bot_step_on`]). When an unacknowledged end
    /// of the run is kept for that registration already, that one stays
    /// as it was first written ([`KeptEnd::Same`]); else, when the
    /// registration has `limit` unacknowledged ends, nothing is kept
    /// ([`KeptEnd::Full`]). An end that was acknowledged does not stand
    /// in the way of a new one: a run that is continued and ends again
    /// is told of again.
    ///
    /// # Errors
    /// `NotFound` for a run or a registration that does not exist;
    /// `run_invalid` for a run that has not ended; the store's.
    pub fn record_end(
        &self,
        run_id: &str,
        end: &EndFor,
        limit: Option<usize>,
        now: DateTime<Utc>,
    ) -> Result<KeptEnd> {
        let at = stamp(now);
        self.with_tx(|tx| {
            let run = run_in(tx, run_id)?.ok_or(CoreError::NotFound)?;
            if !run.status.is_terminal() {
                return Err(CoreError::invalid(
                    "run_invalid",
                    format!("run {run_id} has not ended"),
                ));
            }
            record_end_in(tx, run_id, end, limit, &at)
        })
    }

    /// What waits for `registration`, without the calls' arguments or
    /// the ends' results: every call waiting on it, whatever its
    /// deadline, with where it is with its attempts, and every end it has
    /// not acknowledged.
    ///
    /// A call whose row does not read is left out: [`Store::expire_calls`]
    /// settles it, never read as a default.
    ///
    /// # Errors
    /// The store's.
    pub fn pending_summary(&self, registration: &str) -> Result<PendingSummary> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT id, def_hash, op, access, expires_at, length(CAST(args_json AS BLOB)),
                        sent_at IS NOT NULL, accepted_at IS NOT NULL, run_id, {TRIES_COLS}
                   FROM bot_pending_calls
                  WHERE registration_id = ?1 AND answered_at IS NULL AND {CALL_READS}
                  ORDER BY created_at, id"
            ))?;
            let calls = q
                .query_map(params![registration], |r| {
                    Ok(CallSummary {
                        id: r.get(0)?,
                        def_hash: r.get(1)?,
                        op: r.get(2)?,
                        access: access_at(r, 3)?,
                        expires_at: r.get(4)?,
                        args_bytes: r.get(5)?,
                        sent: r.get(6)?,
                        accepted: r.get(7)?,
                        run_id: r.get(8)?,
                        tries: tries_at(r, 9)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut q = c.prepare_cached(
                "SELECT id, COALESCE(length(CAST(result_json AS BLOB)), 0) FROM bot_run_ends
                  WHERE registration_id = ?1 AND delivered_at IS NULL
                  ORDER BY created_at, run_id",
            )?;
            let ends = q
                .query_map(params![registration], |r| {
                    Ok(EndSummary {
                        id: r.get(0)?,
                        result_bytes: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(PendingSummary { calls, ends })
        })
    }

    /// Those of the calls waiting on `registration` that `ids` names,
    /// whole, oldest first, but those whose row does not read (their
    /// arguments are not a JSON array), which no host is sent
    /// ([`Store::settle_unreadable_calls`]).
    ///
    /// # Errors
    /// The store's.
    pub fn calls_named(&self, registration: &str, ids: &[String]) -> Result<Vec<PendingCall>> {
        let ids = serde_json::to_string(ids).expect("serialisable IDs");
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {PENDING_COLS} FROM bot_pending_calls
                  WHERE registration_id = ?1 AND answered_at IS NULL
                    AND id IN (SELECT value FROM json_each(?2)) AND {CALL_READS}
                  ORDER BY created_at, id"
            ))?;
            let rows = q.query_map(params![registration, ids], row_to_pending)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Settles, at `now`, those of the calls waiting on `registration`
    /// that `ids` names whose row does not read (their arguments are not
    /// a JSON array): each is journaled `internal_error` at its place and
    /// its run made due, as [`Store::expire_calls`] does at a deadline,
    /// so that no host is sent a call other than the one its script
    /// made. The calls it ended.
    ///
    /// # Errors
    /// The store's.
    pub fn settle_unreadable_calls(
        &self,
        registration: &str,
        ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpiredCall>> {
        let at = stamp(now);
        let ids = serde_json::to_string(ids).expect("serialisable IDs");
        self.with_tx(|tx| {
            let unread: Vec<Place> = {
                let mut q = tx.prepare_cached(&format!(
                    "SELECT {PLACE_COLS} FROM bot_pending_calls
                      WHERE registration_id = ?1 AND answered_at IS NULL
                        AND id IN (SELECT value FROM json_each(?2)) AND NOT {CALL_READS}
                      ORDER BY created_at, id"
                ))?;
                let rows = q.query_map(params![registration, ids], row_to_place)?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut ended = Vec::with_capacity(unread.len());
            for place in unread {
                let failed = unreadable_failure();
                let (code, unparked) = match settle(tx, &place, false, failure_json(&failed), &at)?
                {
                    Settle::Journaled(_, unparked) => (Some(failed.code), unparked),
                    Settle::Answered => (None, false),
                };
                ended.push(ExpiredCall {
                    id: place.id,
                    run_id: place.run_id,
                    step_n: place.step_n,
                    seq: place.seq,
                    code,
                    unparked,
                });
            }
            Ok(ended)
        })
    }

    /// Keeps `tries` with the call `id` waiting on `registration`: where
    /// it is with its attempts ([`CallTries`]). False, and nothing
    /// changes, when no such call is waiting. A `not_before` later than
    /// [`latest_stamp`] is kept as that.
    ///
    /// # Errors
    /// The store's.
    pub fn keep_call_tries(&self, registration: &str, id: &str, tries: &CallTries) -> Result<bool> {
        let big = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let not_before = tries
            .not_before
            .as_deref()
            .map(|t| match DateTime::parse_from_rfc3339(t) {
                Ok(t) => Ok(stamp(bounded(t.with_timezone(&Utc)))),
                Err(e) => Err(CoreError::invalid(
                    "invalid_request",
                    format!("`not_before` is no time stamp: {e}"),
                )),
            })
            .transpose()?;
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE bot_pending_calls
                    SET retry_attempts = ?3, retry_backoff_ms = ?4, retry_backoff_max_ms = ?5,
                        failures = ?6, not_before = ?7, resend = ?8, later = ?9,
                        last_code = ?10, last_message = ?11, last_retryable = ?12
                  WHERE id = ?1 AND registration_id = ?2 AND answered_at IS NULL",
                params![
                    id,
                    registration,
                    tries.rule.map(|r| r.attempts),
                    tries.rule.map(|r| big(r.backoff_ms)),
                    tries.rule.map(|r| big(r.backoff_max_ms)),
                    tries.failures,
                    not_before,
                    tries.resend.as_str(),
                    tries.later,
                    tries.last.as_ref().map(|f| f.code.as_str()),
                    tries.last.as_ref().map(|f| f.message.as_str()),
                    tries.last.as_ref().map(|f| f.retryable),
                ],
            )? > 0)
        })
    }

    /// How many times in all `run_id`'s calls to `registration` were
    /// sent again after a failed attempt: the sum of their
    /// [`CallTries::failures`], settled calls among them, for as long as
    /// the store holds the run's calls.
    ///
    /// # Errors
    /// The store's.
    pub fn call_resends(&self, run_id: &str, registration: &str) -> Result<u32> {
        self.with_read(|c| {
            let n: i64 = c.query_row(
                "SELECT COALESCE(sum(failures), 0) FROM bot_pending_calls
                  WHERE run_id = ?1 AND registration_id = ?2 AND typeof(failures) = 'integer'",
                params![run_id, registration],
                |r| r.get(0),
            )?;
            Ok(u32::try_from(n.max(0)).unwrap_or(u32::MAX))
        })
    }

    /// The earliest [`CallTries::not_before`] of a waiting call that is
    /// later than `after`: when a call that waits to be sent again may be
    /// sent.
    ///
    /// # Errors
    /// The store's.
    pub fn next_call_resend(&self, after: DateTime<Utc>) -> Result<Option<String>> {
        let after = stamp(after);
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT min(not_before) FROM bot_pending_calls
                  WHERE answered_at IS NULL AND not_before > ?1 AND typeof(not_before) = 'text'",
                params![after],
                |r| r.get(0),
            )?)
        })
    }

    /// Marks registration `id` as dropped for good at `now` (its lease
    /// ended, or it made room for another), or, with `None`, as not
    /// dropped (its host went on as it). False for an ID never issued.
    ///
    /// # Errors
    /// The store's.
    pub fn set_registration_dropped(&self, id: &str, now: Option<DateTime<Utc>>) -> Result<bool> {
        let at = now.map(stamp);
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE bot_registrations
                    SET dropped_at = CASE WHEN ?2 IS NULL THEN NULL
                                          ELSE COALESCE(dropped_at, ?2) END
                  WHERE id = ?1",
                params![id, at],
            )? > 0)
        })
    }

    /// Whether registration `id` is marked as dropped for good
    /// ([`Store::set_registration_dropped`]).
    ///
    /// # Errors
    /// The store's.
    pub fn registration_dropped(&self, id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT dropped_at IS NOT NULL FROM bot_registrations WHERE id = ?1",
                params![id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }

    /// The earliest deadline of a waiting call, or `None` when none that
    /// waits has one: when [`Store::expire_calls`] is next to be run.
    ///
    /// # Errors
    /// The store's.
    pub fn next_call_deadline(&self) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT min(expires_at) FROM bot_pending_calls
                  WHERE answered_at IS NULL AND typeof(expires_at) = 'text'",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// Those of `registration`'s unacknowledged ends that `ids` names,
    /// whole, oldest first, but those whose row does not read (its end
    /// is not a JSON object, or its result not JSON), which no host is
    /// sent ([`Store::forget_unreadable_ends`]).
    ///
    /// # Errors
    /// The store's.
    pub fn ends_named(&self, registration: &str, ids: &[String]) -> Result<Vec<RunEnd>> {
        let ids = serde_json::to_string(ids).expect("serialisable IDs");
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {END_COLS} FROM bot_run_ends
                  WHERE registration_id = ?1 AND delivered_at IS NULL
                    AND id IN (SELECT value FROM json_each(?2)) AND {END_READS}
                  ORDER BY created_at, run_id"
            ))?;
            let rows = q.query_map(params![registration, ids], row_to_end)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Forgets those of `registration`'s unacknowledged ends that `ids`
    /// names whose row does not read: there is nothing to tell its host
    /// of them. How many.
    ///
    /// # Errors
    /// The store's.
    pub fn forget_unreadable_ends(&self, registration: &str, ids: &[String]) -> Result<usize> {
        let ids = serde_json::to_string(ids).expect("serialisable IDs");
        self.with_tx(|tx| {
            Ok(tx.execute(
                &format!(
                    "DELETE FROM bot_run_ends
                      WHERE registration_id = ?1 AND delivered_at IS NULL
                        AND id IN (SELECT value FROM json_each(?2)) AND NOT {END_READS}"
                ),
                params![registration, ids],
            )?)
        })
    }

    /// Forgets the unacknowledged ends kept before `before`, oldest
    /// first, [`EXPIRE_ENDS_PER_TX`] at the most, so that no transaction
    /// grows with what is due (the hosted Worker refuses one over 16
    /// MiB): what is left is due at the next call. How many it forgot.
    ///
    /// # Errors
    /// The store's.
    pub fn expire_ends(&self, before: DateTime<Utc>) -> Result<usize> {
        let before = stamp(before);
        self.with_tx(|tx| {
            Ok(tx.execute(
                "DELETE FROM bot_run_ends WHERE rowid IN
                   (SELECT rowid FROM bot_run_ends
                     WHERE delivered_at IS NULL AND created_at < ?1
                     ORDER BY created_at, rowid LIMIT ?2)",
                params![before, EXPIRE_ENDS_PER_TX],
            )?)
        })
    }

    /// Records that `registration` was told of its end `id`
    /// ([`RunEnd::id`]), at `now`. Returns whether it was recorded now;
    /// `false` when it was acknowledged before or `registration` has no
    /// such end.
    ///
    /// # Errors
    /// The store's.
    pub fn ack_end(&self, registration: &str, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let at = stamp(now);
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE bot_run_ends SET delivered_at = ?3
                  WHERE id = ?2 AND registration_id = ?1 AND delivered_at IS NULL",
                params![registration, id, at],
            )? > 0)
        })
    }
}

/// [`Store::record_end`] on `c`, for a run known to have ended (or ending
/// in `c`'s transaction).
pub(crate) fn record_end_in(
    c: &Connection,
    run_id: &str,
    end: &EndFor,
    limit: Option<usize>,
    at: &str,
) -> Result<KeptEnd> {
    registration_in(c, &end.registration_id)?.ok_or(CoreError::NotFound)?;
    let held: Option<String> = c
        .query_row(
            "SELECT id FROM bot_run_ends
              WHERE run_id = ?1 AND registration_id = ?2 AND delivered_at IS NULL",
            params![run_id, end.registration_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = held {
        return Ok(KeptEnd::Same(id));
    }
    if let Some(limit) = limit {
        let kept: usize = c.query_row(
            "SELECT count(*) FROM bot_run_ends
              WHERE registration_id = ?1 AND delivered_at IS NULL",
            params![end.registration_id],
            |r| r.get(0),
        )?;
        if kept >= limit {
            return Ok(KeptEnd::Full);
        }
    }
    let id = clax_core::new_ulid();
    c.execute(
        "INSERT INTO bot_run_ends (id, run_id, registration_id, end_json, result_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            run_id,
            end.registration_id,
            end.end.to_string(),
            end.result.as_ref().map(Value::to_string),
            at
        ],
    )?;
    Ok(KeptEnd::New(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_runs::{
        JournalPlace, NewRun, Park, RequestKind, Run, RunStatus, StepCommit, StepKind,
    };
    use crate::bot_surfaces::NewRegistration;
    use crate::credits::KeySource;
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store_with_clock};
    use clax_core::working::{Clock, ManualClock};
    use std::sync::Arc;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    /// The tests' reads of what waits for a registration, each made of
    /// the store's one read path ([`Store::pending_summary`], then
    /// [`Store::calls_named`] or [`Store::ends_named`]): what the hub
    /// reads.
    trait Reads {
        fn calls_for(&self, registration: &str, now: DateTime<Utc>) -> Result<Vec<PendingCall>>;
        fn count_calls(&self, registration: &str) -> Result<usize>;
        fn ends_for(&self, registration: &str, now: DateTime<Utc>) -> Result<Vec<RunEnd>>;
        fn count_ends(&self, registration: &str) -> Result<usize>;
        fn pending_call_by_key(
            &self,
            registration: &str,
            run_id: &str,
            key: &str,
        ) -> Result<Option<PendingCall>>;
    }

    impl Reads for Store {
        fn calls_for(&self, registration: &str, _now: DateTime<Utc>) -> Result<Vec<PendingCall>> {
            let ids: Vec<String> = self
                .pending_summary(registration)?
                .calls
                .into_iter()
                .map(|c| c.id)
                .collect();
            self.calls_named(registration, &ids)
        }
        fn count_calls(&self, registration: &str) -> Result<usize> {
            Ok(self.pending_summary(registration)?.calls.len())
        }
        fn ends_for(&self, registration: &str, _now: DateTime<Utc>) -> Result<Vec<RunEnd>> {
            let ids: Vec<String> = self
                .pending_summary(registration)?
                .ends
                .into_iter()
                .map(|e| e.id)
                .collect();
            self.ends_named(registration, &ids)
        }
        fn count_ends(&self, registration: &str) -> Result<usize> {
            Ok(self.pending_summary(registration)?.ends.len())
        }
        fn pending_call_by_key(
            &self,
            registration: &str,
            run_id: &str,
            key: &str,
        ) -> Result<Option<PendingCall>> {
            Ok(self
                .calls_for(registration, Utc::now())?
                .into_iter()
                .find(|c| c.run_id == run_id && c.key == key))
        }
    }

    fn d(name: &str) -> DriverId {
        DriverId::named(name)
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    /// A store on a clock stopped at 10:00 on 2026-11-11, that clock, and
    /// two live registrations of the surface `shell`.
    struct Env {
        _dir: tempfile::TempDir,
        st: Store,
        clock: Arc<ManualClock>,
        shell: String,
        other: String,
    }

    impl Env {
        fn new() -> Env {
            let clock = Arc::new(ManualClock::at("2026-11-11T10:00:00Z"));
            let (dir, st) = store_with_clock(clock.clone());
            st.put_surface_def(HASH, "{}", clock.now()).unwrap();
            let register = |person: &str| {
                st.register_surface(
                    DAEMON,
                    NewRegistration {
                        name: "shell".into(),
                        def_hash: HASH.into(),
                        prompt: None,
                        settings: json!({}),
                        registered_by: person.into(),
                        replaces: None,
                    },
                    clock.now(),
                )
                .unwrap()
                .id
            };
            let (shell, other) = (register(OWNER), register("u_mia"));
            Env {
                _dir: dir,
                st,
                clock,
                shell,
                other,
            }
        }

        fn now(&self) -> DateTime<Utc> {
            self.clock.now()
        }

        /// A run driver `d1` holds, its first step, a script, begun.
        fn run(&self) -> Run {
            let aid = artifact(&self.st, None).to_string();
            let run = self
                .st
                .create_run(
                    DAEMON,
                    NewRun {
                        artifact_id: aid,
                        person: OWNER.into(),
                        request: RequestKind::Thread,
                        thread_ids: vec!["t1".into()],
                        model: "claude-sonnet-5-5".into(),
                        key_source: KeySource::Instance,
                        base_version: 1,
                        agent_grant: None,
                        web: String::new(),
                    },
                )
                .unwrap();
            self.st.claim_run(&run.id, &d("d1"), 60).unwrap().unwrap();
            self.st
                .begin_step(&run.id, &d("d1"), 1, StepKind::Script)
                .unwrap();
            self.st.run(&run.id).unwrap().unwrap()
        }

        /// The call `shell.ask("Which?")` at call `seq` of step 1 of
        /// `run`, a read due in a day, to the first registration.
        fn ask(&self, run: &Run, seq: i64) -> NewCall {
            NewCall {
                step_n: 1,
                seq,
                attempt: 1,
                registration_id: self.shell.clone(),
                def_hash: HASH.into(),
                op: "shell.ask".into(),
                args: json!(["Which?"]),
                args_hash: "hash-of-which".into(),
                key: format!("{}:1:{seq}", run.id),
                access: CallAccess::Read,
                waits: false,
                expires_at: Some(self.now() + chrono::Duration::hours(24)),
            }
        }

        /// Opens `call` of `run` for driver `driver`.
        fn open_as(&self, run: &Run, driver: &str, call: NewCall) -> PendingCall {
            self.st
                .open_call(&run.id, &d(driver), call, None, self.now())
                .unwrap()
                .into_call()
        }

        /// Opens [`Env::ask`] at call 0 for `d1`, as a call that waits.
        fn open(&self, run: &Run) -> PendingCall {
            let call = NewCall {
                waits: true,
                ..self.ask(run, 0)
            };
            self.open_as(run, "d1", call)
        }

        fn expire(&self) -> Vec<ExpiredCall> {
            self.st.expire_calls(self.now(), &unanswered).unwrap()
        }

        /// The IDs of the calls to hand `registration` now.
        fn handed(&self, registration: &str) -> Vec<String> {
            self.st
                .calls_for(registration, self.now())
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        }

        /// Parks `run` for `d1` on call 0 of step 1, due when the call
        /// expires.
        fn park(&self, run: &Run) -> Run {
            let parked = self
                .st
                .park_run(
                    &run.id,
                    &d("d1"),
                    JournalPlace { step_n: 1, seq: 0 },
                    &json!({"surface": "shell", "op": "ask"}),
                    Some(self.now() + chrono::Duration::hours(24)),
                    self.now(),
                )
                .unwrap();
            match parked {
                Park::Parked(run) => run,
                Park::Answered(run) => panic!("not parked: {run:?}"),
            }
        }

        /// The journal of step 1 of `run`, each row as `(seq, attempt, op,
        /// args_hash, ok, result)`.
        fn journal(&self, run: &Run) -> Vec<(i64, i64, String, String, bool, Value)> {
            self.st
                .bot_calls(&run.id, 1)
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e.seq(),
                        e.attempt(),
                        e.op().to_string(),
                        e.args_hash().to_string(),
                        e.ok(),
                        e.result().clone(),
                    )
                })
                .collect()
        }

        fn parked_ms(&self, run: &Run) -> i64 {
            self.st.run_parking(&run.id).unwrap().unwrap().parked_ms
        }
    }

    /// The journal row an answer `result` to [`Env::ask`] at call 0
    /// leaves.
    fn row(ok: bool, result: Value) -> (i64, i64, String, String, bool, Value) {
        (
            0,
            1,
            "shell.ask".to_string(),
            "hash-of-which".to_string(),
            ok,
            result,
        )
    }

    #[test]
    fn a_call_is_opened_once_for_its_place_by_the_driver_that_holds_the_run() {
        let env = Env::new();
        let run = env.run();
        let call = env.ask(&run, 0);
        assert_eq!(
            code(
                env.st
                    .open_call(&run.id, &d("d2"), call.clone(), None, env.now())
            ),
            "run_not_claimed"
        );
        let second_step = NewCall {
            step_n: 2,
            ..call.clone()
        };
        assert_eq!(
            code(
                env.st
                    .open_call(&run.id, &d("d1"), second_step, None, env.now())
            ),
            "run_invalid"
        );
        assert!(matches!(
            env.st
                .open_call("nope", &d("d1"), call.clone(), None, env.now()),
            Err(CoreError::NotFound)
        ));
        let unregistered = NewCall {
            registration_id: "nope".into(),
            ..call.clone()
        };
        assert!(matches!(
            env.st
                .open_call(&run.id, &d("d1"), unregistered, None, env.now()),
            Err(CoreError::NotFound)
        ));
        assert_eq!(env.st.pending_call_at(&run.id, 1, 0).unwrap(), None);
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 0);

        let opened = env
            .st
            .open_call(&run.id, &d("d1"), call.clone(), None, env.now())
            .unwrap();
        let Opened::New(opened) = opened else {
            panic!("not new: {opened:?}");
        };
        assert_eq!(
            opened,
            PendingCall {
                id: opened.id.clone(),
                run_id: run.id.clone(),
                step_n: 1,
                seq: 0,
                attempt: 1,
                registration_id: env.shell.clone(),
                def_hash: HASH.into(),
                op: "shell.ask".into(),
                args: json!(["Which?"]),
                args_hash: "hash-of-which".into(),
                key: format!("{}:1:0", run.id),
                access: CallAccess::Read,
                waits: false,
                created_at: "2026-11-11T10:00:00.000Z".into(),
                expires_at: Some("2026-11-12T10:00:00.000Z".into()),
                sent_at: None,
                accepted_at: None,
                answered_at: None,
                tries: CallTries::default(),
            }
        );
        let found = Some(opened.clone());
        assert_eq!(env.st.pending_call(&opened.id).unwrap(), found);
        assert_eq!(env.st.pending_call_at(&run.id, 1, 0).unwrap(), found);
        assert_eq!(
            env.st
                .pending_call_by_key(&env.shell, &run.id, &opened.key)
                .unwrap(),
            found
        );
        // A key is looked up within its registration and its run.
        for (registration, run_id, key) in [
            (env.other.as_str(), run.id.as_str(), opened.key.as_str()),
            (env.shell.as_str(), "nope", opened.key.as_str()),
            (env.shell.as_str(), run.id.as_str(), "nope"),
        ] {
            assert_eq!(
                env.st
                    .pending_call_by_key(registration, run_id, key)
                    .unwrap(),
                None
            );
        }
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 1);
        assert_eq!(env.st.count_calls(&env.other).unwrap(), 0);
        assert!(env.journal(&run).is_empty(), "opening journals nothing");

        // The step run again after a crash opens the same call: the row
        // that is there, with the attempt and deadline it was opened with.
        env.clock.advance(30);
        env.st.release_run(&run.id, &d("d1")).unwrap();
        env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        env.st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        let replayed = NewCall {
            attempt: 2,
            expires_at: None,
            ..call.clone()
        };
        assert_eq!(
            env.st
                .open_call(&run.id, &d("d2"), replayed, None, env.now())
                .unwrap(),
            Opened::Same(opened.clone())
        );
        // Another call at the place is refused, whichever part differs.
        let others = [
            NewCall {
                op: "shell.windows".into(),
                ..call.clone()
            },
            NewCall {
                args_hash: "another".into(),
                ..call.clone()
            },
            NewCall {
                key: "another".into(),
                ..call.clone()
            },
            NewCall {
                registration_id: env.other.clone(),
                ..call.clone()
            },
        ];
        for other in others {
            assert_eq!(
                code(env.st.open_call(&run.id, &d("d2"), other, None, env.now())),
                "key_reused"
            );
        }
        assert_eq!(env.st.pending_call(&opened.id).unwrap(), Some(opened));
        // Another place is another call, opened in the attempt that made
        // it.
        let second = env.open_as(
            &run,
            "d2",
            NewCall {
                attempt: 2,
                ..env.ask(&run, 1)
            },
        );
        assert_eq!((second.seq, second.attempt), (1, 2));
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 2);

        // A registration that is no longer live takes no call.
        env.st.replace_registration(&env.other, env.now()).unwrap();
        let gone = NewCall {
            registration_id: env.other.clone(),
            ..env.ask(&run, 2)
        };
        assert_eq!(
            code(env.st.open_call(&run.id, &d("d2"), gone, None, env.now())),
            "unavailable"
        );
        assert_eq!(env.st.pending_call_at(&run.id, 1, 2).unwrap(), None);
    }

    #[test]
    fn a_run_waits_on_one_call_at_a_time() {
        let env = Env::new();
        let run = env.run();
        let first = env.open(&run);
        assert!(first.waits);
        let waits = |seq: i64| NewCall {
            waits: true,
            ..env.ask(&run, seq)
        };
        assert_eq!(
            code(
                env.st
                    .open_call(&run.id, &d("d1"), waits(1), None, env.now())
            ),
            "run_invalid"
        );
        assert_eq!(env.st.pending_call_at(&run.id, 1, 1).unwrap(), None);
        // The same call again is the call; one that does not wait may be
        // opened beside it; another run has a waiting call of its own.
        assert_eq!(
            env.st
                .open_call(&run.id, &d("d1"), waits(0), None, env.now())
                .unwrap(),
            Opened::Same(first.clone())
        );
        assert!(!env.open_as(&run, "d1", env.ask(&run, 1)).waits);
        env.open(&env.run());
        // The table itself holds a run to one.
        let forced = env.st.with_write(|c| {
            c.execute(
                "UPDATE bot_pending_calls SET waits = 1 WHERE run_id = ?1 AND seq = 1",
                params![run.id],
            )?;
            Ok(())
        });
        assert!(forced.is_err(), "two waiting calls of one run");
        // Once it is answered, the run may wait on the next.
        env.st
            .answer_call(&first.id, &env.shell, Ok(json!("a")), env.now())
            .unwrap();
        assert!(env.open_as(&run, "d1", waits(2)).waits);
    }

    #[test]
    fn an_answer_is_journaled_with_no_claim_held_and_wakes_the_parked_run() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        assert_eq!(env.st.run(&run.id).unwrap().unwrap().claimed_by, None);
        env.clock.advance(3600);

        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("The second")), env.now())
            .unwrap();
        let AnswerOutcome::Journaled {
            run_id,
            entry,
            unparked,
        } = answered
        else {
            panic!("not journaled: {answered:?}");
        };
        assert_eq!((run_id.as_str(), unparked), (run.id.as_str(), true));
        assert_eq!(env.journal(&run), [row(true, json!("The second"))]);
        assert_eq!(env.st.bot_calls(&run.id, 1).unwrap(), [entry]);
        let at: String = env
            .st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT at FROM bot_calls WHERE run_id = ?1",
                    params![run.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(at, "2026-11-11T11:00:00.000Z");
        let woken = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (woken.status, woken.claimed_by.clone(), woken.due_at.clone()),
            (RunStatus::Queued, None, None)
        );
        assert_eq!(env.parked_ms(&run), 3_600_000);
        let ended = env.st.pending_call(&call.id).unwrap().unwrap();
        assert_eq!(
            ended.answered_at.as_deref(),
            Some("2026-11-11T11:00:00.000Z")
        );
        assert!(env.handed(&env.shell).is_empty());
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 0);

        // A driver claims the run and executes the step again: the place
        // is in its journal as any call the core journaled, and a call
        // made again at it is answered from that row.
        let held = env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Running);
        let again = env
            .st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        assert_eq!(again.attempt, 2);
        let replayed = env
            .st
            .call_entry(&run.id, &d("d2"), 1, 2, 0, "shell.ask", "hash-of-which")
            .unwrap()
            .answered(true, json!("another answer"));
        let kept = env.st.record_bot_call(&replayed).unwrap();
        assert_eq!(
            (kept.attempt(), kept.ok(), kept.result().clone()),
            (1, true, json!("The second"))
        );
        assert_eq!(env.journal(&run), [row(true, json!("The second"))]);
    }

    /// Two parks in one step: each call's row carries the attempt that
    /// opened it, and the step counts its parks apart from its attempts.
    #[test]
    fn a_step_that_parks_twice_journals_each_call_with_the_attempt_that_opened_it() {
        let env = Env::new();
        let run = env.run();
        let first = env.open(&run);
        env.park(&run);
        env.st
            .answer_call(&first.id, &env.shell, Ok(json!("one")), env.now())
            .unwrap();
        // Attempt 2 is answered call 0 from the journal and waits on
        // call 1.
        env.st.claim_run(&run.id, &d("d1"), 60).unwrap().unwrap();
        env.st
            .begin_step(&run.id, &d("d1"), 1, StepKind::Script)
            .unwrap();
        let second = env.open_as(
            &run,
            "d1",
            NewCall {
                attempt: 2,
                waits: true,
                ..env.ask(&run, 1)
            },
        );
        let parked = env
            .st
            .park_run(
                &run.id,
                &d("d1"),
                JournalPlace { step_n: 1, seq: 1 },
                &json!({}),
                None,
                env.now(),
            )
            .unwrap();
        assert!(matches!(parked, Park::Parked(_)), "{parked:?}");
        env.st
            .answer_call(&second.id, &env.shell, Ok(json!("two")), env.now())
            .unwrap();
        // Attempt 3 makes a third call, answered at once, and commits.
        env.st.claim_run(&run.id, &d("d1"), 60).unwrap().unwrap();
        let third = env
            .st
            .begin_step(&run.id, &d("d1"), 1, StepKind::Script)
            .unwrap();
        assert_eq!(third.attempt, 3);
        let quick = env
            .st
            .call_entry(&run.id, &d("d1"), 1, 3, 2, "shell.windows", "h2")
            .unwrap()
            .answered(true, json!([]));
        env.st.record_bot_call(&quick).unwrap();
        let attempts: Vec<(i64, i64)> =
            env.journal(&run).iter().map(|row| (row.0, row.1)).collect();
        assert_eq!(attempts, [(0, 1), (1, 2), (2, 3)]);
        assert_eq!(env.st.run_parking(&run.id).unwrap().unwrap().parks, 2);
        let done = env
            .st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 1, StepCommit::default())
            .unwrap();
        assert_eq!(done.step.attempt, 3);
        assert_eq!(
            env.st.run_parking(&run.id).unwrap().unwrap().parks,
            0,
            "the next step has parked no times"
        );
    }

    #[test]
    fn a_failure_is_journaled_as_its_code_and_message() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let declined = CallFailure {
            code: "declined".into(),
            message: "The person said no.".into(),
            retryable: false,
        };
        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Err(declined), env.now())
            .unwrap();
        assert!(matches!(
            answered,
            AnswerOutcome::Journaled {
                unparked: false,
                ..
            }
        ));
        assert_eq!(
            env.journal(&run),
            [row(
                false,
                json!({"code": "declined", "message": "The person said no."})
            )]
        );

        let second = env.open_as(&run, "d1", env.ask(&run, 1));
        let busy = CallFailure {
            code: "busy".into(),
            message: "Try again.".into(),
            retryable: true,
        };
        env.st
            .answer_call(&second.id, &env.shell, Err(busy), env.now())
            .unwrap();
        assert_eq!(
            env.journal(&run)[1].5,
            json!({"code": "busy", "message": "Try again.", "retryable": true})
        );
    }

    #[test]
    fn a_second_answer_changes_nothing_and_says_so() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        env.clock.advance(60);
        env.st
            .answer_call(&call.id, &env.shell, Ok(json!("first")), env.now())
            .unwrap();
        let (journal, after, pending) = (
            env.journal(&run),
            env.st.run(&run.id).unwrap().unwrap(),
            env.st.pending_call(&call.id).unwrap(),
        );
        env.clock.advance(60);
        let declined = CallFailure {
            code: "declined".into(),
            message: String::new(),
            retryable: false,
        };
        for result in [Ok(json!("second")), Err(declined)] {
            assert_eq!(
                env.st
                    .answer_call(&call.id, &env.shell, result, env.now())
                    .unwrap(),
                AnswerOutcome::Ignored(Ignored::Answered)
            );
        }
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::Ignored(Ignored::Answered)
        );
        assert_eq!(
            env.st
                .mark_sent(std::slice::from_ref(&call.id), env.now())
                .unwrap(),
            0
        );
        assert_eq!(env.journal(&run), journal);
        assert_eq!(env.journal(&run), [row(true, json!("first"))]);
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), after);
        assert_eq!(env.st.pending_call(&call.id).unwrap(), pending);
        assert_eq!(env.parked_ms(&run), 60_000, "unparked once");
    }

    #[test]
    fn an_answer_after_the_deadline_is_ignored_and_the_deadline_is_journaled() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let parked = env.park(&run);
        env.clock.advance(24 * 3600 - 1);
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::Accepted
        );
        env.clock.advance(1);
        // At its deadline the call has expired.
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("late")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Expired)
        );
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::Ignored(Ignored::Expired)
        );
        assert!(env.journal(&run).is_empty());
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), parked);
        // It waits until its deadline is journaled, which the hub does
        // before it hands a registration its calls.
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&call.id));

        assert_eq!(
            env.expire(),
            [ExpiredCall {
                id: call.id.clone(),
                run_id: run.id.clone(),
                step_n: 1,
                seq: 0,
                code: Some("expired".into()),
                unparked: true,
            }]
        );
        assert_eq!(
            env.journal(&run),
            [row(
                false,
                json!({
                    "code": "expired",
                    "message": "its host was sent the call and did not answer in time"
                })
            )]
        );
        let woken = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (woken.status, woken.claimed_by, woken.due_at),
            (RunStatus::Queued, None, None)
        );
        // The late answer, again: the call has stopped waiting.
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("late")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
        assert_eq!(env.journal(&run).len(), 1);
    }

    #[test]
    fn an_answer_with_an_unknown_id_or_another_registrations_is_ignored() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let parked = env.park(&run);
        let pending = env.st.pending_call(&call.id).unwrap();
        let answer = |id: &str, registration: &str| {
            env.st
                .answer_call(id, registration, Ok(json!(1)), env.now())
                .unwrap()
        };
        assert_eq!(
            answer("nope", &env.shell),
            AnswerOutcome::Ignored(Ignored::Unknown)
        );
        assert_eq!(
            answer(&call.id, &env.other),
            AnswerOutcome::Ignored(Ignored::NotYours)
        );
        assert_eq!(
            answer(&call.id, "nope"),
            AnswerOutcome::Ignored(Ignored::NotYours)
        );
        assert_eq!(
            env.st.accept_call("nope", &env.shell, env.now()).unwrap(),
            AcceptOutcome::Ignored(Ignored::Unknown)
        );
        assert_eq!(
            env.st.accept_call(&call.id, &env.other, env.now()).unwrap(),
            AcceptOutcome::Ignored(Ignored::NotYours)
        );
        assert!(env.journal(&run).is_empty());
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), parked);
        assert_eq!(env.st.pending_call(&call.id).unwrap(), pending);

        // Once its registration is no longer live, its own answer is
        // ignored too.
        env.st.replace_registration(&env.shell, env.now()).unwrap();
        assert_eq!(
            answer(&call.id, &env.shell),
            AnswerOutcome::Ignored(Ignored::Replaced)
        );
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::Ignored(Ignored::Replaced)
        );
        assert!(env.journal(&run).is_empty());
    }

    #[test]
    fn sending_and_accepting_are_each_recorded_once() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let quiet = env.open_as(&run, "d1", env.ask(&run, 1));
        env.clock.advance(5);
        let ids = [call.id.clone(), "nope".to_string()];
        assert_eq!(env.st.mark_sent(&ids, env.now()).unwrap(), 1);
        env.clock.advance(5);
        assert_eq!(env.st.mark_sent(&ids, env.now()).unwrap(), 0);
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::Accepted
        );
        env.clock.advance(5);
        assert_eq!(
            env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
            AcceptOutcome::AlreadyAccepted
        );
        let accepted = env.st.pending_call(&call.id).unwrap().unwrap();
        assert_eq!(
            (accepted.sent_at.as_deref(), accepted.accepted_at.as_deref()),
            (
                Some("2026-11-11T10:00:05.000Z"),
                Some("2026-11-11T10:00:10.000Z")
            )
        );
        assert_eq!(accepted.answered_at, None);
        // Accepting a call that was not marked sent says it was sent.
        env.st
            .accept_call(&quiet.id, &env.shell, env.now())
            .unwrap();
        let quiet = env.st.pending_call(&quiet.id).unwrap().unwrap();
        assert_eq!(quiet.sent_at, quiet.accepted_at);
        assert_eq!(quiet.sent_at.as_deref(), Some("2026-11-11T10:00:15.000Z"));
        // A sent and accepted call still goes out until it is answered.
        assert_eq!(
            env.st.calls_for(&env.shell, env.now()).unwrap(),
            [accepted, quiet]
        );
    }

    /// A run that is woken while its call is still out (a message for it,
    /// say) is executed again by a driver, and the answer arrives while
    /// that driver holds the claim, before it reaches the call: the answer
    /// is the place's one row, the driver's own write at the place keeps
    /// it, and the driver's park finds the place answered.
    #[test]
    fn an_answer_that_races_a_resuming_driver_and_lands_first_is_the_one_row() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        env.clock.advance(600);
        env.st.unpark_run(&run.id, env.now()).unwrap().unwrap();
        let held = env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Running);
        env.st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        let waited = env.parked_ms(&run);
        assert_eq!(waited, 600_000);

        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("in time")), env.now())
            .unwrap();
        assert!(
            matches!(
                answered,
                AnswerOutcome::Journaled {
                    unparked: false,
                    ..
                }
            ),
            "{answered:?}"
        );
        let after = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (
                after.status,
                after.claimed_by.as_deref(),
                after.due_at.clone()
            ),
            (RunStatus::Running, Some("d2"), None),
            "the run is its driver's still, and due"
        );

        // The driver reaches the call: opening it again gives the call,
        // answered; its own write at the place keeps the answer's row;
        // and a park on the place finds it answered.
        let reopened = NewCall {
            attempt: 2,
            waits: true,
            ..env.ask(&run, 0)
        };
        let reopened = env
            .st
            .open_call(&run.id, &d("d2"), reopened, None, env.now())
            .unwrap();
        assert!(matches!(&reopened, Opened::Same(same) if same.id == call.id));
        assert!(reopened.call().answered_at.is_some());
        let own = env
            .st
            .call_entry(&run.id, &d("d2"), 1, 2, 0, "shell.ask", "hash-of-which")
            .unwrap()
            .answered(false, json!({"code": "expired", "message": "too late"}));
        assert_eq!(
            env.st.record_bot_call(&own).unwrap().result(),
            &json!("in time")
        );
        let parked = env
            .st
            .park_run(
                &run.id,
                &d("d2"),
                JournalPlace { step_n: 1, seq: 0 },
                &json!({}),
                None,
                env.now(),
            )
            .unwrap();
        assert_eq!(parked, Park::Answered(after));
        assert_eq!(env.journal(&run), [row(true, json!("in time"))]);
        assert_eq!(env.parked_ms(&run), waited, "unparked once");
        // The deadline, later, finds nothing to end.
        env.clock.advance(48 * 3600);
        assert!(env.expire().is_empty());
        assert_eq!(env.journal(&run).len(), 1);
    }

    /// The other order: the resuming driver journals the place first (it
    /// gave the call up), then the answer arrives. The driver's row
    /// stands; the answer is refused, writes nothing and wakes nothing.
    #[test]
    fn an_answer_that_races_a_resuming_driver_and_lands_second_writes_nothing() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        env.clock.advance(600);
        env.st.unpark_run(&run.id, env.now()).unwrap().unwrap();
        env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        env.st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        let own = env
            .st
            .call_entry(&run.id, &d("d2"), 1, 2, 0, "shell.ask", "hash-of-which")
            .unwrap()
            .answered(false, json!({"code": "expired", "message": "too late"}));
        env.st.record_bot_call(&own).unwrap();
        let (before, waited) = (env.st.run(&run.id).unwrap().unwrap(), env.parked_ms(&run));
        // Journaling the place ended the call's wait.
        assert!(
            env.st
                .pending_call(&call.id)
                .unwrap()
                .unwrap()
                .answered_at
                .is_some()
        );
        assert!(env.handed(&env.shell).is_empty());

        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("in time")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
        assert_eq!(
            env.journal(&run),
            [(
                0,
                2,
                "shell.ask".to_string(),
                "hash-of-which".to_string(),
                false,
                json!({"code": "expired", "message": "too late"})
            )]
        );
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), before);
        assert_eq!(env.parked_ms(&run), waited);
        env.clock.advance(48 * 3600);
        assert!(env.expire().is_empty());
        assert_eq!(env.journal(&run).len(), 1);
    }

    /// An answer and the park of the driver that waits for it, in both
    /// orders: the run ends claimable with one row, woken once.
    #[test]
    fn an_answer_and_a_park_in_either_order_leave_one_row_and_a_run_to_execute() {
        let place = JournalPlace { step_n: 1, seq: 0 };
        // The answer first, while the driver holds the run: the park
        // does not happen.
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("quick")), env.now())
            .unwrap();
        assert!(matches!(
            answered,
            AnswerOutcome::Journaled {
                unparked: false,
                ..
            }
        ));
        let held = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            env.st
                .park_run(&run.id, &d("d1"), place, &json!({}), None, env.now())
                .unwrap(),
            Park::Answered(held.clone())
        );
        assert_eq!(
            (held.status, held.claimed_by.as_deref()),
            (RunStatus::Running, Some("d1"))
        );
        assert_eq!(env.journal(&run), [row(true, json!("quick"))]);
        let parking = env.st.run_parking(&run.id).unwrap().unwrap();
        assert_eq!((parking.parked_ms, parking.parks), (0, 0));

        // The park first: the answer unparks.
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        env.clock.advance(7);
        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("slow")), env.now())
            .unwrap();
        assert!(matches!(
            answered,
            AnswerOutcome::Journaled { unparked: true, .. }
        ));
        let woken = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (woken.status, woken.claimed_by.clone(), woken.due_at.clone()),
            (RunStatus::Queued, None, None)
        );
        assert_eq!(env.journal(&run), [row(true, json!("slow"))]);
        assert_eq!(env.parked_ms(&run), 7_000);
        // Nothing after wakes it again.
        env.clock.advance(48 * 3600);
        assert_eq!(env.st.unpark_run(&run.id, env.now()).unwrap(), None);
        assert!(env.expire().is_empty());
        assert_eq!(env.parked_ms(&run), 7_000);
    }

    /// One run's deadline is ended without touching another's: the
    /// other run's call, as far past its own deadline, waits for whoever
    /// runs the pass for every run.
    #[test]
    fn a_runs_own_deadline_is_ended_and_no_other_runs() {
        let e = Env::new();
        let (mine, theirs) = (e.run(), e.run());
        let (call, other) = (e.open(&mine), e.open(&theirs));
        e.park(&mine);
        e.park(&theirs);
        e.clock.advance(25 * 3600);
        let ended =
            e.st.expire_calls_of(&mine.id, e.now(), &unanswered)
                .unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(
            (ended[0].id.as_str(), ended[0].unparked),
            (call.id.as_str(), true)
        );
        assert_eq!(
            e.st.run(&mine.id).unwrap().unwrap().status,
            RunStatus::Queued
        );
        let left = e.st.run(&theirs.id).unwrap().unwrap();
        assert_eq!(left.status, RunStatus::Parked);
        assert!(left.due_at.is_some());
        let theirs_call = e.st.pending_call(&other.id).unwrap().unwrap();
        assert_eq!(theirs_call.answered_at, None);
        // Made again it finds none; the pass for every run ends the other.
        assert!(
            e.st.expire_calls_of(&mine.id, e.now(), &unanswered)
                .unwrap()
                .is_empty()
        );
        assert_eq!(e.expire().len(), 1);
    }

    /// An answer to a call that is not the one its run may park on is
    /// journaled, and leaves a run parked on another call parked, with
    /// its deadline: only the waited call's answer takes the run on.
    #[test]
    fn an_answer_to_a_call_the_run_does_not_wait_on_leaves_it_parked() {
        let e = Env::new();
        let run = e.run();
        let waited = e.open(&run);
        let sibling = e.open_as(&run, "d1", e.ask(&run, 1));
        assert!(waited.waits && !sibling.waits);
        let parked = e.park(&run);
        let answered =
            e.st.answer_call(&sibling.id, &e.shell, Ok(json!("late")), e.now())
                .unwrap();
        let AnswerOutcome::Journaled { unparked, .. } = answered else {
            panic!("{answered:?}");
        };
        assert!(!unparked);
        let row = e.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            (row.status, &row.due_at),
            (RunStatus::Parked, &parked.due_at)
        );
        assert_eq!(e.st.run_parking(&run.id).unwrap().unwrap().parks, 1);
        // The waited call's answer does.
        let answered =
            e.st.answer_call(&waited.id, &e.shell, Ok(json!("yes")), e.now())
                .unwrap();
        assert!(matches!(
            answered,
            AnswerOutcome::Journaled { unparked: true, .. }
        ));
        assert_eq!(
            e.st.run(&run.id).unwrap().unwrap().status,
            RunStatus::Queued
        );
    }

    #[test]
    fn a_deadline_journals_what_is_known_of_each_call() {
        let env = Env::new();
        // One parked run for each case, made a second apart; every call
        // due a day after the first.
        let deadline = env.now() + chrono::Duration::hours(24);
        let case = |access: CallAccess, sent: bool, accepted: bool| {
            env.clock.advance(1);
            let run = env.run();
            let call = env.open_as(
                &run,
                "d1",
                NewCall {
                    access,
                    waits: true,
                    expires_at: Some(deadline),
                    ..env.ask(&run, 0)
                },
            );
            if sent {
                assert_eq!(
                    env.st
                        .mark_sent(std::slice::from_ref(&call.id), env.now())
                        .unwrap(),
                    1
                );
            }
            if accepted {
                assert_eq!(
                    env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
                    AcceptOutcome::Accepted
                );
            }
            env.park(&run);
            (run, call)
        };
        let unavailable = json!({
            "code": "unavailable",
            "message": "no host took the call; nothing happened",
            "retryable": true
        });
        let expired = json!({
            "code": "expired",
            "message": "its host was sent the call and did not answer in time"
        });
        let unknown = json!({
            "code": "unknown_outcome",
            "message":
                "its host was sent the call and did not answer in time; it may have taken effect"
        });
        let cases = [
            (case(CallAccess::Read, false, false), &unavailable),
            (case(CallAccess::Read, true, false), &expired),
            (case(CallAccess::Read, false, true), &expired),
            (case(CallAccess::Write, false, false), &unavailable),
            (case(CallAccess::Write, true, false), &unknown),
            (case(CallAccess::Effect, false, false), &unavailable),
            (case(CallAccess::Effect, true, true), &unknown),
        ];
        // A call with no deadline, parked for good.
        env.clock.advance(1);
        let patient = env.run();
        let open_ended = env.open_as(
            &patient,
            "d1",
            NewCall {
                waits: true,
                expires_at: None,
                ..env.ask(&patient, 0)
            },
        );
        env.park(&patient);

        // Before the deadline nothing expires.
        env.clock.advance((deadline - env.now()).num_seconds() - 1);
        assert!(env.expire().is_empty());
        env.clock.advance(1);
        assert_eq!(env.now(), deadline);
        let ended = env.expire();
        assert_eq!(
            ended,
            cases
                .iter()
                .map(|((run, call), failure)| ExpiredCall {
                    id: call.id.clone(),
                    run_id: run.id.clone(),
                    step_n: 1,
                    seq: 0,
                    code: failure["code"].as_str().map(str::to_string),
                    unparked: true,
                })
                .collect::<Vec<_>>(),
            "oldest first"
        );
        for (n, ((run, call), failure)) in cases.iter().enumerate() {
            assert_eq!(env.journal(run), [row(false, (*failure).clone())], "{n}");
            let woken = env.st.run(&run.id).unwrap().unwrap();
            assert_eq!(
                (woken.status, woken.claimed_by, woken.due_at),
                (RunStatus::Queued, None, None),
                "{n}"
            );
            // Parked from the second it was made in to the deadline.
            let parked_secs = 24 * 3600 - (n as i64 + 1);
            assert_eq!(env.parked_ms(run), parked_secs * 1000, "{n}");
            assert_eq!(
                env.st
                    .pending_call(&call.id)
                    .unwrap()
                    .unwrap()
                    .answered_at
                    .as_deref(),
                Some("2026-11-12T10:00:00.000Z")
            );
        }
        // Run again, it finds none; the call with no deadline waits on.
        assert!(env.expire().is_empty());
        env.clock.advance(365 * 24 * 3600);
        assert!(env.expire().is_empty());
        assert!(env.journal(&patient).is_empty());
        assert_eq!(
            env.st.run(&patient.id).unwrap().unwrap().status,
            RunStatus::Parked
        );
        assert_eq!(
            env.st.calls_for(&env.shell, env.now()).unwrap(),
            [open_ended]
        );
    }

    #[test]
    fn a_deadline_is_journaled_in_the_callers_words_when_it_gives_them() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.park(&run);
        env.clock.advance(24 * 3600);
        let worded = |call: &PendingCall| CallFailure {
            code: "unavailable".into(),
            message: format!("clax.{}: no host took the call", call.op),
            retryable: true,
        };
        let ended = env.st.expire_calls(env.now(), &worded).unwrap();
        assert_eq!(
            (ended[0].id.as_str(), ended[0].code.as_deref()),
            (call.id.as_str(), Some("unavailable"))
        );
        assert_eq!(
            env.journal(&run),
            [row(
                false,
                json!({
                    "code": "unavailable",
                    "message": "clax.shell.ask: no host took the call",
                    "retryable": true
                })
            )]
        );
    }

    #[test]
    fn the_calls_of_a_registration_that_is_gone_fail_before_their_deadline() {
        let env = Env::new();
        let unsent = env.run();
        let first = env.open(&unsent);
        env.park(&unsent);
        env.clock.advance(1);
        let sent = env.run();
        let second = env.open_as(
            &sent,
            "d1",
            NewCall {
                access: CallAccess::Effect,
                waits: true,
                expires_at: None,
                ..env.ask(&sent, 0)
            },
        );
        env.st
            .accept_call(&second.id, &env.shell, env.now())
            .unwrap();
        env.park(&sent);
        assert!(env.expire().is_empty());

        env.clock.advance(20);
        env.st.replace_registration(&env.shell, env.now()).unwrap();
        let ended = env.expire();
        assert_eq!(
            ended
                .iter()
                .map(|e| (e.id.as_str(), e.code.as_deref(), e.unparked))
                .collect::<Vec<_>>(),
            [
                (first.id.as_str(), Some("unavailable"), true),
                (second.id.as_str(), Some("unknown_outcome"), true)
            ]
        );
        assert_eq!(env.journal(&unsent)[0].5["code"], "unavailable");
        assert_eq!(env.journal(&sent)[0].5["code"], "unknown_outcome");
        assert_eq!(env.parked_ms(&unsent), 21_000);
    }

    /// A pending call left when its step commits without it cannot be
    /// answered later.
    #[test]
    fn a_call_left_when_its_step_commits_stops_waiting_unjournaled() {
        let env = Env::new();
        let run = env.run();
        let left = env.open(&run);
        let other = env.run();
        let kept = env.open(&other);
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 1, StepCommit::default())
            .unwrap();
        let after = env.st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            env.st
                .pending_call(&left.id)
                .unwrap()
                .unwrap()
                .answered_at
                .as_deref(),
            Some("2026-11-11T10:00:00.000Z")
        );
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&kept.id));
        assert_eq!(
            env.st
                .answer_call(&left.id, &env.shell, Ok(json!(1)), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
        assert!(env.journal(&run).is_empty());
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), after);
        // Its deadline finds nothing; the other run's call is untouched.
        env.clock.advance(24 * 3600);
        assert_eq!(
            env.expire()
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            [kept.id.as_str()]
        );
        assert!(env.journal(&run).is_empty());
    }

    /// Cancelling a run ends the wait of its pending calls, so an answer
    /// that comes later is refused; the stopped step is written as for
    /// any run asked to stop.
    #[test]
    fn cancelling_a_run_voids_its_pending_calls() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let beside = env.open_as(&run, "d1", env.ask(&run, 1));
        env.park(&run);
        let other = env.run();
        let kept = env.open(&other);
        env.clock.advance(30);
        let stopping = env.st.cancel_run(DAEMON, &run.id).unwrap();
        assert_eq!(
            (stopping.status, stopping.claimed_by.clone()),
            (RunStatus::Stopping, None)
        );
        for id in [&call.id, &beside.id] {
            assert_eq!(
                env.st
                    .pending_call(id)
                    .unwrap()
                    .unwrap()
                    .answered_at
                    .as_deref(),
                Some("2026-11-11T10:00:30.000Z")
            );
        }
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&kept.id));
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("late")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
        assert!(env.journal(&run).is_empty());
        assert_eq!(env.st.run(&run.id).unwrap().unwrap(), stopping);
        env.clock.advance(48 * 3600);
        assert_eq!(env.expire().len(), 1, "the other run's call alone");
        // The next tick writes the stopped step.
        env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        env.st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Stopped)
            .unwrap();
        let end = env
            .st
            .commit_bot_step(DAEMON, &run.id, &d("d2"), 1, StepCommit::default())
            .unwrap();
        assert_eq!(end.run.status, RunStatus::Stopped);
    }

    #[test]
    fn a_registration_is_handed_its_own_waiting_calls_oldest_first() {
        let env = Env::new();
        let run = env.run();
        let first = env.open(&run);
        env.clock.advance(1);
        let second = env.open_as(&run, "d1", env.ask(&run, 1));
        env.clock.advance(1);
        let theirs = env.open_as(
            &run,
            "d1",
            NewCall {
                registration_id: env.other.clone(),
                ..env.ask(&run, 2)
            },
        );
        assert_eq!(
            env.st.calls_for(&env.shell, env.now()).unwrap(),
            [first.clone(), second.clone()]
        );
        assert_eq!(env.st.calls_for(&env.other, env.now()).unwrap(), [theirs]);
        assert!(env.st.calls_for("nope", env.now()).unwrap().is_empty());
        // Handed out again until answered.
        assert_eq!(env.handed(&env.shell).len(), 2);
        env.st
            .answer_call(&first.id, &env.shell, Ok(json!("a")), env.now())
            .unwrap();
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&second.id));
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 1);
        // Past its deadline it still waits until the deadline is
        // journaled, which the hub does before it hands calls out.
        env.clock.advance(24 * 3600);
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 1);
        env.expire();
        assert!(env.handed(&env.shell).is_empty());
        assert_eq!(env.st.count_calls(&env.shell).unwrap(), 0);
    }

    /// A run of a host's kind, made with no artifact, goes the whole way
    /// through the store: claimed, stepped, parked on a call to the
    /// registration that started it, answered with no claim held,
    /// executed again and finished, its end kept for that registration.
    #[test]
    fn a_run_of_a_hosts_kind_parks_on_its_call_is_answered_and_finishes() {
        use crate::bot_runs::{Bindings, NewRunOf, RequestKind, StoredBinding};
        let env = Env::new();
        let bindings = Bindings::bound(vec![StoredBinding::bound(
            "shell",
            Some(HASH.into()),
            Some(env.shell.clone()),
            None,
            Value::Null,
            vec!["shell.ask".into()],
        )])
        .unwrap();
        let run = env
            .st
            .create_run_of(
                DAEMON,
                NewRunOf {
                    kind: "shell.triage".into(),
                    request: json!({"ask": "Which window?"}),
                    bindings: bindings.clone(),
                    person: OWNER.into(),
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    started_by: env.shell.clone(),
                    parent_run: None,
                    agent_grant: None,
                },
            )
            .unwrap();
        assert_eq!((run.artifact(), run.request), (None, RequestKind::Other));

        // A driver reads it as it was made, finds who serves its binding,
        // and runs a script that asks.
        env.st.claim_run(&run.id, &d("d1"), 60).unwrap().unwrap();
        let (_, made, _) = env.st.run_with_steps_as_made(&run.id).unwrap().unwrap();
        let stored = made.bindings().unwrap();
        let shell = stored.of("shell").unwrap();
        assert_eq!(shell.registration(), Some(env.shell.as_str()));
        assert_eq!(shell.operations(), Some(&["shell.ask".to_string()][..]));
        env.st
            .begin_step(&run.id, &d("d1"), 1, StepKind::Script)
            .unwrap();
        let call = env.open(&run);
        assert_eq!(call.registration_id, shell.registration().unwrap());
        env.park(&run);
        assert_eq!(env.st.parked_runs_of(OWNER).unwrap(), 1);
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&call.id));

        env.clock.advance(120);
        let answered = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("The left one")), env.now())
            .unwrap();
        assert!(matches!(
            answered,
            AnswerOutcome::Journaled { unparked: true, .. }
        ));
        assert_eq!(env.journal(&run), [row(true, json!("The left one"))]);

        // Executed again, the step is answered from the journal and
        // commits; the run finishes and its end is kept for the
        // registration that started it.
        env.st.claim_run(&run.id, &d("d2"), 60).unwrap().unwrap();
        let again = env
            .st
            .begin_step(&run.id, &d("d2"), 1, StepKind::Script)
            .unwrap();
        assert_eq!(again.attempt, 2);
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d2"), 1, StepCommit::default())
            .unwrap();
        env.st
            .begin_step(&run.id, &d("d2"), 2, StepKind::Finish)
            .unwrap();
        let end = EndFor {
            registration_id: env.shell.clone(),
            end: json!({"finished": {}}),
            result: Some(json!({"window": "left"})),
        };
        let done = env
            .st
            .commit_bot_step_on(
                DAEMON,
                &run.id,
                &d("d2"),
                2,
                StepCommit::default(),
                None,
                std::slice::from_ref(&end),
            )
            .unwrap();
        assert_eq!(done.run.status, RunStatus::Finished);
        assert_eq!(done.run.artifact(), None);
        let ends = env.st.ends_for(&env.shell, env.now()).unwrap();
        assert_eq!(
            (ends.len(), ends[0].run_id.as_str(), &ends[0].result),
            (1, run.id.as_str(), &end.result)
        );
        assert_eq!(env.parked_ms(&run), 120_000);
        // Its records name no artifact.
        let filed: Vec<(String, Option<String>)> = env
            .st
            .with_read(|c| {
                let mut q = c.prepare(
                    "SELECT kind, artifact_id FROM audit_events
                      WHERE json_extract(body, '$.run') = ?1 ORDER BY seq",
                )?;
                let rows = q.query_map(params![run.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(
            filed,
            [
                ("bot.run_start".to_string(), None),
                ("bot.run_end".to_string(), None)
            ]
        );
        assert_eq!(env.st.runs_started_by(&env.shell, 10).unwrap(), [done.run]);
    }

    fn finished() -> EndFor {
        EndFor {
            registration_id: String::new(),
            end: json!({"finished": {}}),
            result: Some(json!({"asked": 2})),
        }
    }

    #[test]
    fn a_runs_end_is_kept_in_its_terminal_commit_until_acknowledged() {
        let env = Env::new();
        let run = env.run();
        let ends = [
            EndFor {
                registration_id: env.shell.clone(),
                ..finished()
            },
            EndFor {
                registration_id: env.other.clone(),
                result: None,
                ..finished()
            },
        ];
        let commit = |n: i64, ends: &[EndFor]| {
            env.st.commit_bot_step_on(
                DAEMON,
                &run.id,
                &d("d1"),
                n,
                StepCommit::default(),
                None,
                ends,
            )
        };
        // A step that does not end the run keeps no end, and commits
        // nothing.
        assert_eq!(code(commit(1, &ends)), "run_invalid");
        assert!(env.st.open_step(&run.id).unwrap().is_some());
        assert_eq!(
            code(env.st.record_end(&run.id, &ends[0], None, env.now())),
            "run_invalid"
        );
        commit(1, &[]).unwrap();
        env.st
            .begin_step(&run.id, &d("d1"), 2, StepKind::Finish)
            .unwrap();
        // An end for a registration that does not exist commits nothing.
        let unknown = [EndFor {
            registration_id: "nope".into(),
            ..finished()
        }];
        assert!(matches!(commit(2, &unknown), Err(CoreError::NotFound)));
        assert_eq!(
            env.st.run(&run.id).unwrap().unwrap().status,
            RunStatus::Running
        );
        env.clock.advance(30);
        assert_eq!(commit(2, &ends).unwrap().run.status, RunStatus::Finished);

        let mine = env.st.ends_for(&env.shell, env.now()).unwrap();
        let kept = RunEnd {
            id: mine[0].id.clone(),
            run_id: run.id.clone(),
            registration_id: env.shell.clone(),
            end: json!({"finished": {}}),
            result: Some(json!({"asked": 2})),
            created_at: "2026-11-11T10:00:30.000Z".into(),
            delivered_at: None,
        };
        assert_eq!(mine, std::slice::from_ref(&kept));
        let theirs = env.st.ends_for(&env.other, env.now()).unwrap();
        assert_eq!(
            theirs,
            [RunEnd {
                id: theirs[0].id.clone(),
                registration_id: env.other.clone(),
                result: None,
                ..kept.clone()
            }]
        );
        assert_ne!(theirs[0].id, kept.id);
        assert!(env.st.ends_for("nope", env.now()).unwrap().is_empty());
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 1);
        // Kept again after the commit, it is the one first written.
        env.clock.advance(5);
        let again = EndFor {
            registration_id: env.shell.clone(),
            end: json!({"failed": {"code": "x"}}),
            result: None,
        };
        assert_eq!(
            env.st.record_end(&run.id, &again, None, env.now()).unwrap(),
            KeptEnd::Same(kept.id.clone())
        );
        assert_eq!(
            env.st.ends_for(&env.shell, env.now()).unwrap(),
            std::slice::from_ref(&kept)
        );

        // Acknowledged once, by the registration it is for.
        assert!(!env.st.ack_end(&env.other, &kept.id, env.now()).unwrap());
        assert!(!env.st.ack_end(&env.shell, "nope", env.now()).unwrap());
        assert!(env.st.ack_end(&env.shell, &kept.id, env.now()).unwrap());
        assert!(!env.st.ack_end(&env.shell, &kept.id, env.now()).unwrap());
        assert!(env.st.ends_for(&env.shell, env.now()).unwrap().is_empty());
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 0);
        assert_eq!(env.st.count_ends(&env.other).unwrap(), 1);
    }

    #[test]
    fn an_end_is_kept_after_the_commit_for_a_run_that_ended_without_one() {
        let env = Env::new();
        let run = env.run();
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 1, StepCommit::default())
            .unwrap();
        env.st
            .begin_step(&run.id, &d("d1"), 2, StepKind::GiveUp)
            .unwrap();
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 2, StepCommit::default())
            .unwrap();
        let end = EndFor {
            registration_id: env.shell.clone(),
            end: json!({"gave_up": {"code": "stuck", "reason": "No way on."}}),
            result: None,
        };
        assert!(matches!(
            env.st.record_end("nope", &end, None, env.now()),
            Err(CoreError::NotFound)
        ));
        let unregistered = EndFor {
            registration_id: "nope".into(),
            ..end.clone()
        };
        assert!(matches!(
            env.st.record_end(&run.id, &unregistered, None, env.now()),
            Err(CoreError::NotFound)
        ));
        let KeptEnd::New(id) = env.st.record_end(&run.id, &end, None, env.now()).unwrap() else {
            panic!("not new");
        };
        assert_eq!(
            env.st.record_end(&run.id, &end, None, env.now()).unwrap(),
            KeptEnd::Same(id.clone())
        );
        let kept = env.st.ends_for(&env.shell, env.now()).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (
                kept[0].id.as_str(),
                kept[0].run_id.as_str(),
                &kept[0].end,
                &kept[0].result
            ),
            (id.as_str(), run.id.as_str(), &end.end, &None)
        );
    }

    /// A run that has ended, with no end kept.
    fn ended(env: &Env) -> Run {
        let run = env.run();
        env.st
            .begin_step(&run.id, &d("d1"), 1, StepKind::GiveUp)
            .unwrap();
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 1, StepCommit::default())
            .unwrap()
            .run
    }

    #[test]
    fn a_registration_keeps_ends_up_to_its_limit_and_a_new_one_after_an_acknowledgement() {
        let env = Env::new();
        let end = EndFor {
            registration_id: env.shell.clone(),
            ..finished()
        };
        let keep = |run: &Run, limit: Option<usize>| {
            env.st.record_end(&run.id, &end, limit, env.now()).unwrap()
        };
        let (a, b, c) = (ended(&env), ended(&env), ended(&env));
        let KeptEnd::New(first) = keep(&a, Some(2)) else {
            panic!("not new");
        };
        env.clock.advance(1);
        let KeptEnd::New(second) = keep(&b, Some(2)) else {
            panic!("not new");
        };
        // At the limit a new end is refused, and one that is held is
        // answered for all the same.
        assert_eq!(keep(&c, Some(2)), KeptEnd::Full);
        assert_eq!(keep(&a, Some(2)), KeptEnd::Same(first.clone()));
        assert_eq!(keep(&a, Some(0)), KeptEnd::Same(first.clone()));
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 2);
        // The other registration has a count of its own.
        let theirs = EndFor {
            registration_id: env.other.clone(),
            ..finished()
        };
        assert!(matches!(
            env.st
                .record_end(&c.id, &theirs, Some(1), env.now())
                .unwrap(),
            KeptEnd::New(_)
        ));

        // What waits, without results; then the ends named, whole.
        let summary = env.st.pending_summary(&env.shell).unwrap();
        assert!(summary.calls.is_empty());
        assert_eq!(
            summary.ends,
            [
                EndSummary {
                    id: first.clone(),
                    result_bytes: r#"{"asked":2}"#.len()
                },
                EndSummary {
                    id: second.clone(),
                    result_bytes: r#"{"asked":2}"#.len()
                }
            ]
        );
        let ids = [second.clone(), "nope".to_string(), first.clone()];
        let named = env.st.ends_named(&env.shell, &ids).unwrap();
        assert_eq!(
            named.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            [first.as_str(), second.as_str()],
            "oldest first"
        );
        assert_eq!(named[0].run_id, a.id);
        assert!(env.st.ends_named(&env.other, &ids).unwrap().is_empty());

        // Acknowledged, an end frees its place, and the run's next end
        // is a new one.
        assert!(env.st.ack_end(&env.shell, &first, env.now()).unwrap());
        assert!(env.st.ends_named(&env.shell, &ids[2..]).unwrap().is_empty());
        env.clock.advance(1);
        let KeptEnd::New(again) = keep(&a, Some(2)) else {
            panic!("not new");
        };
        assert_ne!(again, first);
        assert_eq!(keep(&c, Some(2)), KeptEnd::Full);

        // Ends kept before a time are forgotten, unless acknowledged.
        let cut = env.now();
        env.clock.advance(1);
        assert_eq!(
            env.st.expire_ends(cut).unwrap(),
            2,
            "the second, and the other registration's"
        );
        assert_eq!(
            env.st
                .pending_summary(&env.shell)
                .unwrap()
                .ends
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>(),
            [again]
        );
        assert_eq!(env.st.expire_ends(env.now()).unwrap(), 1);
        assert_eq!(env.st.expire_ends(env.now()).unwrap(), 0);
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 0);
    }

    /// The ends due are forgotten [`EXPIRE_ENDS_PER_TX`] at a time,
    /// oldest first: one call's transaction does not grow with them.
    #[test]
    fn ends_are_forgotten_a_bounded_number_at_a_time_oldest_first() {
        let env = Env::new();
        let run = env.run();
        let due = EXPIRE_ENDS_PER_TX + 3;
        env.st
            .with_tx(|tx| {
                for n in 0..due {
                    tx.execute(
                        "INSERT INTO bot_run_ends (id, run_id, registration_id, end_json,
                            created_at)
                         VALUES (?1, ?2, ?3, '{\"finished\": {}}', ?4)",
                        params![
                            format!("e{n:05}"),
                            format!("{}-{n}", run.id),
                            env.shell,
                            format!("2026-11-10T00:00:{:02}.{:03}Z", n / 1000, n % 1000)
                        ],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(env.st.expire_ends(env.now()).unwrap(), EXPIRE_ENDS_PER_TX);
        let left: Vec<String> = env
            .st
            .pending_summary(&env.shell)
            .unwrap()
            .ends
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(
            left,
            (EXPIRE_ENDS_PER_TX..due)
                .map(|n| format!("e{n:05}"))
                .collect::<Vec<_>>(),
            "the oldest went first"
        );
        assert_eq!(env.st.expire_ends(env.now()).unwrap(), 3);
        assert_eq!(env.st.expire_ends(env.now()).unwrap(), 0);
    }

    #[test]
    fn a_registration_takes_calls_up_to_its_limit_and_they_are_read_by_summary_and_by_id() {
        let env = Env::new();
        let run = env.run();
        assert_eq!(env.st.next_call_deadline().unwrap(), None);
        let open = |seq: i64, limit: Option<usize>| {
            env.st
                .open_call(&run.id, &d("d1"), env.ask(&run, seq), limit, env.now())
        };
        let first = open(0, Some(2)).unwrap().into_call();
        env.clock.advance(1);
        let second = open(1, Some(2)).unwrap().into_call();
        // At the limit a new call is refused; a place that has its call
        // is answered for before the limit is looked at.
        assert_eq!(code(open(2, Some(2))), "registration_full");
        assert_eq!(env.st.pending_call_at(&run.id, 1, 2).unwrap(), None);
        assert_eq!(open(0, Some(2)).unwrap(), Opened::Same(first.clone()));
        assert_eq!(open(0, Some(0)).unwrap(), Opened::Same(first.clone()));
        // Another registration has a count of its own.
        let theirs = NewCall {
            registration_id: env.other.clone(),
            expires_at: None,
            ..env.ask(&run, 3)
        };
        env.st
            .open_call(&run.id, &d("d1"), theirs, Some(1), env.now())
            .unwrap();

        env.st
            .mark_sent(std::slice::from_ref(&first.id), env.now())
            .unwrap();
        env.st
            .accept_call(&second.id, &env.shell, env.now())
            .unwrap();
        let summary = env.st.pending_summary(&env.shell).unwrap();
        assert!(summary.ends.is_empty());
        let call = |c: &PendingCall, sent: bool, accepted: bool| CallSummary {
            id: c.id.clone(),
            run_id: c.run_id.clone(),
            tries: CallTries::default(),
            def_hash: HASH.into(),
            op: "shell.ask".into(),
            access: CallAccess::Read,
            expires_at: c.expires_at.clone(),
            args_bytes: r#"["Which?"]"#.len(),
            sent,
            accepted,
        };
        assert_eq!(
            summary.calls,
            [call(&first, true, false), call(&second, true, true)]
        );
        let ids = [second.id.clone(), "nope".to_string(), first.id.clone()];
        let named = env.st.calls_named(&env.shell, &ids).unwrap();
        assert_eq!(
            named
                .iter()
                .map(|c| (c.id.as_str(), &c.args, c.key.as_str()))
                .collect::<Vec<_>>(),
            [
                (first.id.as_str(), &json!(["Which?"]), first.key.as_str()),
                (second.id.as_str(), &json!(["Which?"]), second.key.as_str())
            ],
            "oldest first, whole"
        );
        assert!(env.st.calls_named(&env.other, &ids).unwrap().is_empty());
        // The earliest deadline of a call that waits; a call with none
        // has no part in it.
        assert_eq!(env.st.next_call_deadline().unwrap(), first.expires_at);
        env.st
            .answer_call(&first.id, &env.shell, Ok(json!("a")), env.now())
            .unwrap();
        assert_eq!(env.st.next_call_deadline().unwrap(), second.expires_at);
        assert!(
            env.st
                .calls_named(&env.shell, &ids[2..])
                .unwrap()
                .is_empty(),
            "an answered call is no longer handed out"
        );
        // An answered call frees its place under the limit.
        assert!(matches!(open(2, Some(2)), Ok(Opened::New(_))));
    }

    #[test]
    fn ended_runs_lose_their_pending_calls_with_their_journal_and_their_ends_after_a_week() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.st
            .commit_bot_step(DAEMON, &run.id, &d("d1"), 1, StepCommit::default())
            .unwrap();
        env.st
            .begin_step(&run.id, &d("d1"), 2, StepKind::Finish)
            .unwrap();
        let ends = [
            EndFor {
                registration_id: env.shell.clone(),
                ..finished()
            },
            EndFor {
                registration_id: env.other.clone(),
                ..finished()
            },
        ];
        env.st
            .commit_bot_step_on(
                DAEMON,
                &run.id,
                &d("d1"),
                2,
                StepCommit::default(),
                None,
                &ends,
            )
            .unwrap();
        let theirs = env.st.ends_for(&env.other, env.now()).unwrap();
        env.st
            .ack_end(&env.other, &theirs[0].id, env.now())
            .unwrap();
        // A run that has not ended keeps its pending call.
        let live = env.run();
        let waiting = env.open(&live);

        let none = crate::bot_runs::Pruned::default();
        env.clock.advance(24 * 3600);
        assert_eq!(
            env.st.prune_ended_runs().unwrap(),
            none,
            "a day, to the second"
        );
        env.clock.advance(1);
        let pruned = env.st.prune_ended_runs().unwrap();
        assert_eq!((pruned.pending_calls, pruned.run_ends), (1, 0));
        assert_eq!(env.st.pending_call(&call.id).unwrap(), None);
        assert!(env.st.pending_call(&waiting.id).unwrap().is_some());
        assert_eq!(env.st.ends_for(&env.shell, env.now()).unwrap().len(), 1);

        env.clock.advance(6 * 24 * 3600 - 1);
        assert_eq!(
            env.st.prune_ended_runs().unwrap(),
            none,
            "a week, to the second"
        );
        assert_eq!(env.st.ends_for(&env.shell, env.now()).unwrap().len(), 1);
        env.clock.advance(1);
        // Past the week an end is no longer delivered: the hub forgets
        // it (`expire_ends`), and pruning removes it.
        let pruned = env.st.prune_ended_runs().unwrap();
        assert_eq!(
            (pruned.pending_calls, pruned.run_ends),
            (0, 2),
            "acknowledged or not"
        );
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 0);
        assert!(env.st.pending_call(&waiting.id).unwrap().is_some());
    }

    /// A pruned run's pending call takes where it was with its attempts
    /// with it: no sending again is planned for it, and its run counts
    /// none. The call of a run that has not ended keeps its own, however
    /// many prune passes go by.
    #[test]
    fn a_pruned_runs_calls_take_their_attempts_with_them() {
        let env = Env::new();
        let rule = RetryRule {
            attempts: 3,
            backoff_ms: 1_000,
            backoff_max_ms: 8_000,
        };
        let planned = |not_before: &str| CallTries {
            rule: Some(rule),
            failures: 1,
            not_before: Some(not_before.into()),
            resend: Resend::Planned,
            later: false,
            last: None,
        };
        let opened = |run: &Run, not_before: &str| {
            let call = env
                .st
                .open_call_with(
                    &run.id,
                    &d("d1"),
                    env.ask(run, 0),
                    Some(rule),
                    None,
                    env.now(),
                )
                .unwrap()
                .into_call();
            let tries = planned(not_before);
            assert!(
                env.st
                    .keep_call_tries(&env.shell, &call.id, &tries)
                    .unwrap()
            );
            call
        };
        let ended = env.run();
        let gone = opened(&ended, "2026-11-20T00:00:00.000Z");
        let live = env.run();
        let kept = opened(&live, "2026-11-21T00:00:00.000Z");
        assert_eq!(
            env.st.next_call_resend(env.now()).unwrap().as_deref(),
            Some("2026-11-20T00:00:00.000Z")
        );
        env.st
            .commit_bot_step(DAEMON, &ended.id, &d("d1"), 1, StepCommit::default())
            .unwrap();
        env.st
            .begin_step(&ended.id, &d("d1"), 2, StepKind::Finish)
            .unwrap();
        env.st
            .commit_bot_step(DAEMON, &ended.id, &d("d1"), 2, StepCommit::default())
            .unwrap();
        // Its run's end settles the call: it is sent again no more, and
        // its row, attempts and all, waits for the prune.
        assert_eq!(
            env.st.next_call_resend(env.now()).unwrap().as_deref(),
            Some("2026-11-21T00:00:00.000Z")
        );
        assert!(env.st.pending_call(&gone.id).unwrap().is_some());
        assert_eq!(env.st.call_resends(&ended.id, &env.shell).unwrap(), 1);

        env.clock.advance(24 * 3600 + 1);
        let pruned = env.st.prune_ended_runs().unwrap();
        assert_eq!(pruned.pending_calls, 1);
        assert_eq!(env.st.pending_call(&gone.id).unwrap(), None);
        assert_eq!(env.st.call_resends(&ended.id, &env.shell).unwrap(), 0);
        assert_eq!(
            env.st.next_call_resend(env.now()).unwrap().as_deref(),
            Some("2026-11-21T00:00:00.000Z"),
            "only the live run's call is planned to be sent again"
        );
        for _ in 0..3 {
            env.st.prune_ended_runs().unwrap();
        }
        let left = env.st.pending_call(&kept.id).unwrap().expect("not pruned");
        assert_eq!(left.tries, planned("2026-11-21T00:00:00.000Z"));
    }

    /// Where a call is with its attempts is kept with it: its run's rule
    /// from when it was opened, and what the hub writes after; handing
    /// it to its host makes a planned attempt one that was sent, and
    /// accepting it makes it one not to send again. A run's sendings
    /// again are the sum of its calls' failures, settled ones among
    /// them.
    #[test]
    fn a_calls_attempts_are_kept_with_it_and_counted_for_its_run() {
        let env = Env::new();
        let run = env.run();
        let rule = RetryRule {
            attempts: 3,
            backoff_ms: 1_000,
            backoff_max_ms: 8_000,
        };
        let opened = |seq: i64| {
            env.st
                .open_call_with(
                    &run.id,
                    &d("d1"),
                    env.ask(&run, seq),
                    Some(rule),
                    None,
                    env.now(),
                )
                .unwrap()
                .into_call()
        };
        let (a, b) = (opened(0), opened(1));
        let tries = |id: &str| {
            env.st
                .pending_summary(&env.shell)
                .unwrap()
                .calls
                .into_iter()
                .find(|c| c.id == id)
                .map(|c| c.tries)
        };
        assert_eq!(
            tries(&a.id),
            Some(CallTries {
                rule: Some(rule),
                ..CallTries::default()
            })
        );
        assert_eq!(a.tries, tries(&a.id).unwrap(), "read whole as summary");
        let failed = CallFailure {
            code: "invalid".into(),
            message: "busy".into(),
            retryable: true,
        };
        let planned = CallTries {
            rule: Some(rule),
            failures: 1,
            not_before: Some("2026-11-11T10:00:01.000Z".into()),
            resend: Resend::Planned,
            later: true,
            last: Some(failed.clone()),
        };
        assert!(env.st.keep_call_tries(&env.shell, &a.id, &planned).unwrap());
        assert!(
            !env.st.keep_call_tries(&env.other, &a.id, &planned).unwrap(),
            "another registration's call"
        );
        assert_eq!(tries(&a.id).as_ref(), Some(&planned));
        assert_eq!(env.st.pending_call(&a.id).unwrap().unwrap().tries, planned);
        assert_eq!(
            env.st.next_call_resend(env.now()).unwrap().as_deref(),
            Some("2026-11-11T10:00:01.000Z")
        );
        assert_eq!(
            env.st
                .next_call_resend(env.now() + chrono::Duration::seconds(1))
                .unwrap(),
            None
        );
        // Sent: the planned attempt is made.
        env.st
            .mark_sent(std::slice::from_ref(&a.id), env.now())
            .unwrap();
        assert_eq!(
            tries(&a.id),
            Some(CallTries {
                resend: Resend::Due,
                later: false,
                last: None,
                ..planned.clone()
            })
        );
        // Accepted: its host has it.
        env.st.accept_call(&a.id, &env.shell, env.now()).unwrap();
        assert_eq!(
            tries(&a.id),
            Some(CallTries {
                resend: Resend::None,
                later: false,
                last: None,
                ..planned.clone()
            })
        );
        // The run's count, across its calls, settled ones among them.
        env.st
            .keep_call_tries(
                &env.shell,
                &b.id,
                &CallTries {
                    failures: 2,
                    ..planned.clone()
                },
            )
            .unwrap();
        assert_eq!(env.st.call_resends(&run.id, &env.shell).unwrap(), 3);
        env.st
            .answer_call(&b.id, &env.shell, Ok(json!("b")), env.now())
            .unwrap();
        assert_eq!(env.st.call_resends(&run.id, &env.shell).unwrap(), 3);
        assert_eq!(env.st.call_resends(&run.id, &env.other).unwrap(), 0);
        assert_eq!(env.st.call_resends("nope", &env.shell).unwrap(), 0);
    }

    /// The same answer sent again for a call it settled is told apart
    /// from one that came too late or says otherwise: it stands.
    #[test]
    fn the_same_answer_sent_again_is_told_it_was_journaled() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let journaled = env
            .st
            .answer_call(&call.id, &env.shell, Ok(json!("a")), env.now())
            .unwrap();
        assert!(matches!(journaled, AnswerOutcome::Journaled { .. }));
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("a")), env.now())
                .unwrap(),
            AnswerOutcome::AlreadyJournaled
        );
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.shell, Ok(json!("b")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
        assert_eq!(
            env.st
                .answer_call(&call.id, &env.other, Ok(json!("a")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::NotYours)
        );
        // A call its deadline settled was not settled by the answer.
        let late = env.run();
        let expiring = env.open_as(
            &late,
            "d1",
            NewCall {
                expires_at: Some(env.now() + chrono::Duration::seconds(1)),
                ..env.ask(&late, 0)
            },
        );
        env.clock.advance(1);
        env.expire();
        assert_eq!(
            env.st
                .answer_call(&expiring.id, &env.shell, Ok(json!("a")), env.now())
                .unwrap(),
            AnswerOutcome::Ignored(Ignored::Answered)
        );
    }

    /// A call whose stored arguments do not read is sent to no host and
    /// read as nothing else: it is refused when read by itself, left out
    /// of the calls handed out, and journaled `internal_error`, at once
    /// when it is named, or at its deadline. An end that does not read is
    /// left out and forgotten.
    #[test]
    fn a_call_or_an_end_that_does_not_read_is_refused_and_settled_never_defaulted() {
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        let damage = |sql: &str| {
            env.st.with_tx(|tx| Ok(tx.execute(sql, [])?)).unwrap();
        };
        damage("UPDATE bot_pending_calls SET args_json = '{not json'");
        assert!(env.st.pending_call(&call.id).is_err(), "refused, not null");
        let ids = [call.id.clone()];
        assert!(env.st.calls_named(&env.shell, &ids).unwrap().is_empty());
        let ended = env
            .st
            .settle_unreadable_calls(&env.shell, &ids, env.now())
            .unwrap();
        assert_eq!(
            ended
                .iter()
                .map(|e| (e.id.as_str(), e.code.as_deref(), e.unparked))
                .collect::<Vec<_>>(),
            [(call.id.as_str(), Some("internal_error"), false)]
        );
        assert_eq!(env.journal(&run)[0].5["code"], "internal_error");
        // At its deadline, too.
        let late = env.run();
        let opened = env.open(&late);
        damage(&format!(
            "UPDATE bot_pending_calls SET args_json = '{{\"a\": 1}}' WHERE id = '{}'",
            opened.id
        ));
        env.clock.advance(24 * 3600);
        let ended = env.expire();
        assert_eq!(
            ended
                .iter()
                .map(|e| (e.id.as_str(), e.code.as_deref()))
                .collect::<Vec<_>>(),
            [(opened.id.as_str(), Some("internal_error"))]
        );
        // An end whose end is not an object.
        damage(&format!(
            "INSERT INTO bot_run_ends (id, run_id, registration_id, end_json, created_at)
             VALUES ('e1', '{}', '{}', 'null', '2026-11-11T10:00:00.000Z')",
            run.id, env.shell
        ));
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 1);
        let named = ["e1".to_string()];
        assert!(env.st.ends_named(&env.shell, &named).unwrap().is_empty());
        assert_eq!(
            env.st.forget_unreadable_ends(&env.shell, &named).unwrap(),
            1
        );
        assert_eq!(env.st.count_ends(&env.shell).unwrap(), 0);
    }

    /// A call whose row holds, in any column a call is read by, a value
    /// this build does not read fails that call alone: it is left out of
    /// what waits for its registration, takes no answer, and the next
    /// pass over the calls, before or after its deadline, journals it
    /// `internal_error` so that its run is told; no other registration's
    /// calls and no pass fail. One whose place does not read either
    /// stops waiting, unjournaled.
    #[test]
    fn a_call_whose_row_does_not_read_in_any_column_fails_that_call_alone() {
        let damages = [
            "args_json = '{not json'",
            "args_json = '{\"a\": 1}'",
            "access = 'later'",
            "resend = 'soon'",
            "failures = -1",
            "failures = 4294967296",
            "failures = 'many'",
            "later = 2",
            "retry_attempts = 3, retry_backoff_ms = NULL, retry_backoff_max_ms = NULL",
            "retry_attempts = 4294967296, retry_backoff_ms = 1, retry_backoff_max_ms = 1",
            "retry_attempts = 3, retry_backoff_ms = -1, retry_backoff_max_ms = 1",
            "last_code = 'busy', last_message = NULL, last_retryable = 1",
            "last_code = 'busy', last_message = 'x', last_retryable = 'yes'",
            "expires_at = x'01'",
            "sent_at = x'01'",
            "accepted_at = x'00'",
            "not_before = x'01'",
            "def_hash = x'01'",
            "key = x'01'",
            "registration_id = x'01'",
            "created_at = x'01'",
        ];
        for damage in damages {
            let env = Env::new();
            let run = env.run();
            let call = env.open(&run);
            let fine = env.run();
            let other = env.open_as(
                &fine,
                "d1",
                NewCall {
                    registration_id: env.other.clone(),
                    ..env.ask(&fine, 0)
                },
            );
            env.st
                .with_tx(|tx| {
                    tx.execute_batch("PRAGMA ignore_check_constraints = ON")?;
                    tx.execute(
                        &format!("UPDATE bot_pending_calls SET {damage} WHERE id = ?1"),
                        params![call.id],
                    )?;
                    tx.execute_batch("PRAGMA ignore_check_constraints = OFF")?;
                    Ok(())
                })
                .unwrap();
            assert!(
                env.st.pending_summary(&env.shell).unwrap().calls.is_empty(),
                "{damage}: left out"
            );
            assert_eq!(
                env.st
                    .pending_summary(&env.other)
                    .unwrap()
                    .calls
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                [other.id.as_str()],
                "{damage}: another registration's calls are read"
            );
            assert_eq!(
                env.st
                    .answer_call(&call.id, &env.shell, Ok(json!("a")), env.now())
                    .unwrap(),
                AnswerOutcome::Ignored(Ignored::Answered),
                "{damage}: takes no answer"
            );
            assert_eq!(
                env.st.accept_call(&call.id, &env.shell, env.now()).unwrap(),
                AcceptOutcome::Ignored(Ignored::Answered),
                "{damage}"
            );
            // Before its deadline, the pass settles it.
            let ended = env.expire();
            assert_eq!(
                ended
                    .iter()
                    .map(|e| (e.id.as_str(), e.code.as_deref()))
                    .collect::<Vec<_>>(),
                [(call.id.as_str(), Some("internal_error"))],
                "{damage}"
            );
            assert_eq!(env.journal(&run)[0].5["code"], "internal_error", "{damage}");
            // After every deadline, the pass completes, and the other
            // registration's call is settled at its own.
            env.clock.advance(24 * 3600);
            let ended = env.expire();
            assert_eq!(
                ended.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
                [other.id.as_str()],
                "{damage}"
            );
            assert!(env.st.next_call_deadline().unwrap().is_none(), "{damage}");
        }
        // A row whose place does not read stops waiting, unjournaled.
        let env = Env::new();
        let run = env.run();
        let call = env.open(&run);
        env.st
            .with_tx(|tx| {
                tx.execute(
                    "UPDATE bot_pending_calls SET step_n = 'one' WHERE id = ?1",
                    params![call.id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(env.st.pending_summary(&env.shell).unwrap().calls.is_empty());
        assert!(env.expire().is_empty());
        assert!(env.journal(&run).is_empty());
        let waiting: i64 = env
            .st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM bot_pending_calls WHERE answered_at IS NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(waiting, 0, "it stops waiting");
    }

    /// A deadline after the year 9999 is kept as the last instant of it,
    /// which sorts after every other: such a call is not past its
    /// deadline.
    #[test]
    fn a_deadline_past_the_year_9999_is_kept_as_its_last_instant() {
        let env = Env::new();
        let run = env.run();
        let far = DateTime::<Utc>::MAX_UTC;
        let call = env.open_as(
            &run,
            "d1",
            NewCall {
                expires_at: Some(far),
                ..env.ask(&run, 0)
            },
        );
        assert_eq!(call.expires_at.as_deref(), Some("9999-12-31T23:59:59.999Z"));
        assert!(env.expire().is_empty(), "not expired");
        assert_eq!(env.handed(&env.shell), std::slice::from_ref(&call.id));
        assert_eq!(
            env.st.next_call_deadline().unwrap().as_deref(),
            Some("9999-12-31T23:59:59.999Z")
        );
    }

    /// A pass over one run's calls reads that run's rows by its place's
    /// index, not every open call.
    #[test]
    fn a_pass_over_one_runs_calls_reads_only_that_runs_rows() {
        let env = Env::new();
        let plan: Vec<String> = env
            .st
            .with_read(|c| {
                let mut q = c.prepare(&format!(
                    "EXPLAIN QUERY PLAN SELECT {PENDING_COLS} FROM bot_pending_calls p
                      WHERE answered_at IS NULL AND run_id = ?2
                        AND ((expires_at IS NOT NULL AND expires_at <= ?1)
                             OR NOT EXISTS (SELECT 1 FROM bot_registrations g
                                             WHERE g.id = p.registration_id
                                               AND g.replaced_at IS NULL))
                        AND {CALL_READS}
                      ORDER BY created_at, id"
                ))?;
                let rows = q.query_map(params!["now", "run"], |r| r.get(3))?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("sqlite_autoindex_bot_pending_calls")),
            "{plan:?}"
        );
    }

    /// A registration dropped for good is kept as dropped until its host
    /// goes on as it.
    #[test]
    fn a_registration_is_kept_as_dropped_until_it_is_gone_on_with() {
        let env = Env::new();
        assert!(!env.st.registration_dropped(&env.shell).unwrap());
        assert!(
            env.st
                .set_registration_dropped(&env.shell, Some(env.now()))
                .unwrap()
        );
        assert!(env.st.registration_dropped(&env.shell).unwrap());
        assert!(!env.st.registration_dropped(&env.other).unwrap());
        env.st.set_registration_dropped(&env.shell, None).unwrap();
        assert!(!env.st.registration_dropped(&env.shell).unwrap());
        assert!(!env.st.set_registration_dropped("nope", None).unwrap());
    }
}
