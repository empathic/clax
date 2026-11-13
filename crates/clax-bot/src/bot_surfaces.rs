//! The hub of the host protocol
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "A
//! host's surface on a daemon"; `docs/contract.md`, "ClaxBot
//! surfaces"): the surfaces hosts registered at run time, and the rules
//! by which their calls go out to them and their answers come back. The
//! daemon's routes (a host's side) and the run loop's provider for an
//! outside surface (a run's side) share one [`Hub`].
//!
//! It is logic and nothing more. It reads no clock (every method that
//! depends on the time is given it, a [`Millis`]), spawns nothing,
//! waits for nothing and does no I/O. What outlasts the process is not
//! kept here: the calls on their way and where each is with its
//! attempts, the ended runs to tell a host of, the registration IDs
//! that were issued and which of them were dropped, and the definitions
//! calls were made under are in whatever implements [`Pending`], which
//! every method that touches them is lent. [`MemoryPending`] keeps them in
//! memory, for tests. The hub itself holds the registrations, which
//! are leases and end with the process, and names only this module's
//! types, `clax_surface`'s and JSON values.
//!
//! # A call, from a run to its host and back
//!
//! | # | Side | Does |
//! |---|---|---|
//! | 1 | Run | [`Hub::enqueue`]: the registration that serves the run, the operation, its checked arguments, its key and its deadline. The call is opened in the store ([`Pending::open_call`]) with the hash of the definition it is made under, and the registration's wake hook is called |
//! | 2 | Host | [`Hub::open`] with the answers its `serve` brought; then, when [`Hub::ready`] says so (at once, on the hook, or when the request's wait is over), [`Hub::close`], which answers what the request is answered with |
//! | 3 | Host | A later [`Hub::open`] brings the call's answer. A result is checked against the operation's declared result in the call's own definition ([`clax_surface::Operation::check_answer`]) and settles the call as its value, or as `invalid_result`; an error settles it as that error, unless the call is to be sent again ("Sending a call again"); `accepted` and `retry_after_ms` settle nothing |
//! | 4 | Run | Reads the settled call from the store. [`Opened::settled`] and [`Hub::expire`] name the calls that were settled, for whoever wakes their runs |
//!
//! Every call has a deadline: the one [`Hub::enqueue`] is given, or
//! [`HubLimits::call_ms`] from then. A call that is not settled when
//! its deadline comes is settled with the failure its last attempt came
//! to when it has not been sent since ("Sending a call again"), and
//! otherwise with [`unanswered`], the error the design names
//! ("Deadlines, retries and what a call may promise"):
//!
//! | The call was | The script is thrown |
//! |---|---|
//! | Never sent to its host | `unavailable`: nothing happened |
//! | Sent, a read | `expired` |
//! | Sent, a write or an effect | `unknown_outcome`: it may have taken effect |
//!
//! "Sent" is that a `serve` was answered with the call, or that its
//! host answered `accepted` for it. The hub cannot know that the host
//! read what it was sent, so every call that was sent counts as one its
//! host may have.
//!
//! # What holds
//!
//! - **A call is sent to the registration it was opened for and to no
//!   other,** and an answer settles a call only when the registration
//!   that brings it is the call's own. Many registrations of one name
//!   may be live; each has its own calls. A request names its
//!   registration; none is guessed.
//! - **A call is never sent at or after its deadline,** and an answer
//!   that arrives at or after it is ignored: the deadline's error
//!   stands.
//! - **A call is settled once.** The first result or error for it
//!   stands; a later one, one for an ID the store does not hold, and
//!   one for another registration's call are ignored. `accepted` and
//!   `retry_after_ms` settle nothing, so neither keeps a result or an
//!   error that follows it, in the same `serve` or a later one, from
//!   settling the call.
//! - **A call that is not settled is sent again** with every `serve` of
//!   its registration until it is settled or its host has answered
//!   `accepted` for it; one its host answered `retry_after_ms` for is
//!   left out until that long has passed, and one waiting out the wait
//!   after a failed attempt until that wait is over ("Sending a call
//!   again").
//! - **An answer is checked by the definition its call was made
//!   under,** whatever its registration has registered since.
//! - **No call is lost.** Every call [`Hub::enqueue`] opened is
//!   settled: by an answer or by its deadline.
//! - **Everything a host or a script can grow has a limit**
//!   ([`HubLimits`]), and reaching one is an error, never a panic.
//!
//! # Keys and `@once`
//!
//! A key (`<run>:<step>:<seq>`) is passed through to the host as it is
//! given. While the store holds a call, settled or not, a second
//! [`Hub::enqueue`] with its key, for the same registration, run,
//! operation and arguments, is that call: the same ID is answered and
//! the host sees no second call ([`OpenedCall::Same`]); with anything
//! else it is refused ([`EnqueueError::KeyReused`]). So two different
//! calls are never sent under one key, and a call's ID and key are the
//! same every time it is sent. Both are [`Pending::open_call`]'s to
//! hold to.
//!
//! What the hub does not promise: that a write or an effect sent twice
//! with one key takes effect once is the host's promise; and a call
//! goes out again until its host says `accepted`, so a host of a
//! `@once` operation, which cannot tell a repeat, says `accepted` as
//! soon as it has the call, and until it has, tells a call it was sent
//! twice by its ID.
//!
//! # Sending a call again
//!
//! Each call comes with its rule ([`Enqueue::retry`], [`Retry`]: the
//! attempts it may have in all, and the waits between them, which
//! double up to a longest). An attempt fails, and the call is sent
//! again under its ID and key once the wait after it is over, when:
//!
//! | The attempt | Sent again | Unless |
//! |---|---|---|
//! | Its host answered an error that may be tried again (`unavailable`, or a declared code with `retryable: true`) | After the wait, with every `serve` until it is settled or accepted | The call is accepted and its operation is `@once` |
//! | Its host accepted it and may have lost it: it serves again after it was not connected, or goes on as its registration (it started again) | After the wait, with every `serve` until it is accepted again or settled | Its operation is `@once`: never sent again once accepted |
//!
//! and in either case no more often than the rule's attempts, no more
//! than [`HubLimits::resends_per_run`] times across a run's calls of one
//! registration, and
//! only when the wait ends before the call's deadline, which no attempt
//! moves. A host's retryable error that is not sent again is the call's
//! outcome at once (still `retryable`); one that is followed by a wait
//! the deadline comes in is its outcome at the deadline. A failure that
//! does not say it may be tried again settles the call at once. The
//! answers of one `serve` are taken in their order, each finding the
//! call as the answers before it left it, as if each came in a `serve`
//! of its own: an `accepted` before a retryable error has the call
//! accepted when the error is taken, and an error for an attempt whose
//! next one is planned and not sent yet takes no attempt more.
//! `retry_after_ms` is the host's own account, and no failure: the call
//! stays open until its deadline, as long as its host asks for it
//! later, and is sent again when the host asks, but no sooner than the
//! rule's first wait ([`Retry::backoff_ms`]), so that a host answering
//! `retry_after_ms: 0` is not sent the call again at once; it spends no
//! attempt and none of the sendings again. A call its host had accepted
//! and asks for later is sent again then, as one that may have been lost
//! is, until its host accepts it again; but one of an `@once` operation,
//! which is never sent again once accepted, is settled at once,
//! `unavailable`: its host said it cannot take it, and nothing else will.
//! A call whose last word from its host is `retry_after_ms`, and that is
//! not sent again before its deadline, is settled `unavailable` then:
//! its host said it could not take it. A host that is not connected is waited
//! for, within the deadline, by a call whose rule has more than one
//! attempt, while the hub holds its registration; nothing is sent to it
//! meanwhile, so a host that is away costs nothing, and one that comes
//! and goes spends one attempt each time it comes back, with waits that
//! grow. The answers a returning `serve` brings are taken first: a call
//! one of them settles or accepts again is not taken for lost. A
//! registration the hub dropped ([`Hub::dropped`]) takes no
//! call. With [`Retry::NONE`] a
//! call is never sent again after a failure, a host that is away takes
//! no call, and `retry_after_ms` is waited exactly, as before rules were
//! given.
//!
//! Where a call is with its attempts ([`Tries`]: its rule, its failures,
//! the instant before which it is not sent, whether it is to be sent
//! again, whether its host asked for it later, its last failure) is kept
//! with the call by the store ([`CallSummary::tries`],
//! [`Pending::keep_tries`]), written with each change, and the count of
//! a run's sendings again is the store's sum of its calls' failures
//! ([`Pending::resends`]). So a hub that starts again, or meets a call
//! after it dropped the call's registration, goes on with it as it would
//! have: the same attempts left, the same wait, the same rule and the
//! same last failure. A call with no rule kept (none was given) is sent
//! again under [`HubLimits::retry_kept`] until its run makes it again
//! with its key, when the run's rule is kept with it. A call made again
//! with its key goes on with the attempts it had.
//!
//! # Registrations
//!
//! - **An ID is issued once, for one surface.** A new registration's
//!   ID is the caller's to draw ([`Hub::register`], `fresh`): 128
//!   random bits as 32 lowercase hexadecimal digits. It is recorded
//!   with its surface's name ([`Pending::issue_registration`]) before
//!   it is answered.
//! - **Going on as a registration.** A host that registers with an ID
//!   is that registration again when the ID was issued for the same
//!   surface, whether or not the hub still holds it; any other ID is
//!   refused. Its definition may have changed: the calls opened before
//!   are checked by the definition they were opened under, which the
//!   store keeps by hash ([`Pending::keep_definition`]). While a
//!   registration is held, registering as it replaces its definition
//!   and leaves its open `serve`s open, so two hosts that register as
//!   one registration are both its hosts: each `serve` is sent
//!   everything due.
//! - **Connected.** A registration is connected while a `serve` of it
//!   is open (between [`Hub::open`] and [`Hub::close`]), and for
//!   [`HubLimits::connected_ms`] after: after its last `close`, and
//!   after it was registered. A `serve` that is not closed keeps its
//!   registration connected only for the time it said it would wait;
//!   closed later, it is still answered with what is due. A
//!   registration that
//!   is not connected takes no new call
//!   ([`EnqueueError::NotConnected`]: the script is thrown
//!   `unavailable`); the calls it has stay, each until its own
//!   deadline, so a host that comes back in time answers them.
//! - **A lease.** A registration that stays unconnected for
//!   [`HubLimits::lease_ms`] more is dropped from the hub, and so is
//!   the oldest unconnected one when a new registration would be one
//!   too many ([`HubLimits::registrations`],
//!   [`HubLimits::registrations_per_name`]); a new one is refused only
//!   when every registration that could make room is connected. What a
//!   dropped registration had stays in the store, for its host to go
//!   on with.

use clax_core::audit::sha256_hex;
use clax_surface::wire::{
    self, Answer, CODE_EXPIRED, CODE_INTERNAL_ERROR, CODE_INVALID_PROMPT, CODE_INVALID_RESULT,
    CODE_INVALID_SETTINGS, CODE_NAME_MISMATCH, CODE_PROMPT_TOO_LARGE, CODE_STORAGE_FAILED,
    CODE_TOO_MANY_ANSWERS, CODE_TOO_MANY_REGISTRATIONS, CODE_UNAVAILABLE, CODE_UNKNOWN_OUTCOME,
    CODE_UNKNOWN_REGISTRATION, CallKind, Delivery, End, ErrorBody, Notation, Outcome,
    RegisterRequest, Registered, Settings, WITHHELD_CONTENT, Withheld,
};
use clax_surface::{
    Author, Definition, Errors, Limits, Pattern, Reserved, RunScope, canonical_manifest, parse,
    parse_manifest,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// What a script is thrown for arguments too large to send
/// ([`EnqueueError::ArgsTooLarge`]).
pub const CODE_INVALID_ARGS: &str = "invalid_args";
/// What a script is thrown for an operation its surface does not have,
/// or that is withheld ([`EnqueueError::UnknownOp`],
/// [`EnqueueError::Withheld`]).
pub const CODE_UNKNOWN_OP: &str = "unknown_op";
/// What a script is thrown when its key was given to another call
/// ([`EnqueueError::KeyReused`]).
pub const CODE_KEY_REUSED: &str = "key_reused";

/// An instant, in milliseconds since the Unix epoch: deadlines are
/// kept in a store that outlasts the process, so the clock is one that
/// means the same after a restart. A wall clock can step back; an
/// instant earlier than one the hub was given before is read as that
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Millis(pub u64);

impl Millis {
    /// The instant `ms` after this one.
    #[must_use]
    pub fn after(self, ms: u64) -> Millis {
        Millis(self.0.saturating_add(ms))
    }

    /// The milliseconds from `earlier` to this instant: 0 when
    /// `earlier` is later.
    #[must_use]
    pub fn since(self, earlier: Millis) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// Why a call failed, as its script is thrown it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallFailure {
    pub code: String,
    pub message: String,
    /// Whether the same call may get past the failure if made again.
    pub retryable: bool,
}

impl CallFailure {
    /// A failure that making the call again would not change.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> CallFailure {
        CallFailure {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable(self) -> CallFailure {
        CallFailure {
            retryable: true,
            ..self
        }
    }
}

/// How often a call is sent again, and how long apart, when an attempt
/// fails in a way that sending it again may get past ([`Enqueue::retry`];
/// the module's documentation, "Sending a call again"). Every attempt
/// is within the call's deadline, which no retry moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retry {
    /// The attempts a call may have in all: the first and every one
    /// after a failure. 1 (or 0) sends no call again after a failure,
    /// and holds no call for a host that is not connected.
    pub attempts: u32,
    /// The wait, in milliseconds, before the second attempt. Each later
    /// wait is twice the one before.
    pub backoff_ms: u64,
    /// The longest wait between two attempts, in milliseconds.
    pub backoff_max_ms: u64,
}

impl Retry {
    /// No call is sent again after a failure.
    pub const NONE: Retry = Retry {
        attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
    };

    /// The daemon's: three attempts, one second apart and then two.
    pub const DAEMON: Retry = Retry {
        attempts: 3,
        backoff_ms: 1_000,
        backoff_max_ms: 8_000,
    };

    /// Whether a call that has failed `failures` attempts may have one
    /// more.
    #[must_use]
    pub fn another(&self, failures: u32) -> bool {
        failures < self.attempts
    }

    /// The wait after the `failures`th failed attempt (1 for the first):
    /// [`Retry::backoff_ms`] doubled for each failure before it, no
    /// longer than [`Retry::backoff_max_ms`].
    #[must_use]
    pub fn wait_after(&self, failures: u32) -> u64 {
        let doublings = failures.saturating_sub(1).min(63);
        self.backoff_ms
            .saturating_mul(1u64 << doublings)
            .min(self.backoff_max_ms.max(self.backoff_ms))
    }

    /// Whether a host that is not connected is waited for: a call that
    /// may be tried again is opened for it, and sent when it serves.
    #[must_use]
    pub fn waits_for_host(&self) -> bool {
        self.attempts > 1
    }
}

// ---------------------------------------------------------------------------
// What is kept outside the hub
// ---------------------------------------------------------------------------

/// A call to open ([`Pending::open_call`]).
#[derive(Clone, Debug, PartialEq)]
pub struct NewCall {
    /// The ID of the registration that serves the call.
    pub registration: String,
    /// The hash of the definition the call is made under
    /// ([`Pending::keep_definition`]).
    pub definition: String,
    /// The run that makes the call.
    pub run: String,
    /// The operation, by the name its definition gives it
    /// ([`clax_surface::Operation::name`]): `shell.settings.set`.
    pub op: String,
    /// One value for each parameter.
    pub args: Vec<Value>,
    /// The bytes of `args` as JSON.
    pub args_bytes: usize,
    /// The call's key.
    pub key: String,
    pub kind: CallKind,
    /// When the call is settled if nothing has settled it.
    pub deadline: Millis,
    /// What a run a join bound the surface into is on
    /// ([`wire::Call::joined`]).
    pub joined: Option<Value>,
    /// The rule its run gave for sending it again: kept with the call
    /// ([`Tries::rule`]).
    pub retry: Retry,
}

/// Whether an open call is to be sent again although its host accepted
/// it ([`Tries::resend`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resend {
    /// No: it is sent as any open call is, and not once its host
    /// accepted it.
    #[default]
    None,
    /// An attempt failed, or its host may have lost it, and the next
    /// attempt is planned and not sent yet: new to its host once its
    /// wait is over, and an error for the attempt before takes no attempt
    /// more.
    Planned,
    /// Sent with every `serve` until its host accepts it again: the
    /// planned attempt was sent, or its host, which had accepted it,
    /// asked for it later.
    Due,
}

/// Where an open call is with its attempts ([`Retry`]; the module's
/// documentation, "Sending a call again"). The store keeps it with the
/// call ([`CallSummary::tries`], [`Pending::keep_tries`]), so that it
/// outlasts the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tries {
    /// The rule its run gave ([`Enqueue::retry`]); `None`: none was
    /// given, and [`HubLimits::retry_kept`] stands for it.
    pub rule: Option<Retry>,
    /// The attempts that failed and were followed by another, each one
    /// of its run's sendings again ([`Pending::resends`]).
    pub failures: u32,
    /// Not sent before this instant: the wait after a failed attempt, or
    /// the one its host asked for (`retry_after_ms`).
    pub not_before: Millis,
    pub resend: Resend,
    /// Its host asked for it later, and it was not sent since: new to its
    /// host once more.
    pub later: bool,
    /// What its last attempt came to, while it has not been sent since:
    /// what it is settled as when its deadline comes first.
    pub last: Option<CallFailure>,
}

/// What the hub reads of an open call to decide what to do with it:
/// everything but its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSummary {
    /// The call's ID: the store's own, the same for as long as it holds
    /// the call, and unlike every ID of an ended run it holds.
    pub id: String,
    /// The run that made it.
    pub run: String,
    /// The hash of the definition the call was made under.
    pub definition: String,
    pub op: String,
    pub kind: CallKind,
    pub deadline: Millis,
    /// The bytes of its arguments as JSON ([`NewCall::args_bytes`]).
    pub args_bytes: usize,
    /// Whether a `serve` was answered with the call
    /// ([`Pending::sent_calls`]), or its host accepted it.
    pub sent: bool,
    /// Whether its host answered `accepted` for it
    /// ([`Pending::accept_call`]).
    pub accepted: bool,
    /// Where it is with its attempts, as the store keeps it.
    pub tries: Tries,
}

/// What [`Pending::answer_call`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answered {
    /// The call is settled with the answer.
    Settled,
    /// The call was settled before with this same answer: it stands, and
    /// nothing changed.
    AlreadySettled,
    /// Nothing changed: no such open call, its deadline is not after
    /// now, or it was settled with something else.
    Ignored,
}

/// An open call as it is sent: a [`CallSummary`] with the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCall {
    pub summary: CallSummary,
    pub run: String,
    pub args: Vec<Value>,
    pub key: String,
    /// What a run a join bound the surface into is on
    /// ([`wire::Call::joined`]).
    pub joined: Option<Value>,
}

/// What the hub reads of an unacknowledged end to decide what to do
/// with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndSummary {
    /// The end's ID: the store's own, and unlike every ID of a call it
    /// holds.
    pub id: String,
    /// The bytes of its result as JSON ([`NewEnd::result_bytes`]).
    pub result_bytes: usize,
}

/// What waits for a registration ([`Pending::pending_for`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingSummary {
    /// Its open calls, the oldest first.
    pub calls: Vec<CallSummary>,
    /// Its unacknowledged ends, the oldest first.
    pub ends: Vec<EndSummary>,
}

/// What [`Pending::open_call`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenedCall {
    /// The call is new: its ID.
    New(String),
    /// The store holds a call with this key, settled or not, for the
    /// same registration, run, operation and arguments: its ID. Nothing
    /// changed.
    Same(String),
    /// The store holds a call with this key that is not this one.
    /// Nothing changed.
    KeyReused,
    /// The registration has as many open calls as the limit. Nothing
    /// changed.
    Full,
}

/// A run's end to record for the registration that started the run
/// ([`Pending::record_end`]).
#[derive(Clone, Debug, PartialEq)]
pub struct NewEnd {
    pub registration: String,
    pub run: String,
    pub end: End,
    /// The kind's result, for a run that finished.
    pub result: Option<Value>,
    /// The bytes of `result` as JSON: 0 for none.
    pub result_bytes: usize,
}

/// What [`Pending::record_end`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedEnd {
    /// The end is recorded: its ID.
    New(String),
    /// The store holds an unacknowledged end of this run for this
    /// registration: its ID. Nothing changed.
    Same(String),
    /// The registration has as many unacknowledged ends as the limit.
    /// Nothing changed.
    Full,
}

/// A run's end that its registration has not acknowledged, as it is
/// sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingEnd {
    pub id: String,
    pub run: String,
    pub end: End,
    pub result: Option<Value>,
    /// The `seq`s of the messages the run took and never gave its model
    /// ([`wire::Ended::undelivered`]), as the store that keeps the run's
    /// inbox has them; none in memory, which keeps no inbox.
    pub undelivered: Vec<u64>,
}

/// Where what outlasts the process is kept: the store, for a daemon.
/// Each method is one change or one reading, and whole: it happens or
/// it does not.
///
/// A call is **open** from [`Pending::open_call`] until it is
/// **settled**, by [`Pending::answer_call`] or
/// [`Pending::expire_calls`]. What is done with a settled call (who
/// reads its outcome, and when it is forgotten) is not the hub's; for
/// as long as the store holds it, its key is its own
/// ([`OpenedCall::Same`], [`OpenedCall::KeyReused`]), so a store that
/// drops a settled call's arguments keeps a digest of them.
pub trait Pending {
    /// Why the store could not be read or written.
    type Error: fmt::Display;

    /// Records, at `now`, that `id` is a registration of the surface
    /// `name`, first registered with the definition `hash`. False, and
    /// nothing changes, when the ID was issued before.
    fn issue_registration(
        &mut self,
        id: &str,
        name: &str,
        hash: &str,
        now: Millis,
    ) -> Result<bool, Self::Error>;

    /// The surface the registration ID was issued for.
    fn registration_name(&mut self, id: &str) -> Result<Option<String>, Self::Error>;

    /// The settings the registration `id` was last registered with, as
    /// a store that keeps them has them (its registrant's JSON): what a
    /// run whose binding keeps no settings of its own is given while the
    /// hub does not hold the registration. `None` for a store that keeps
    /// none, which is what this answers unless it is written.
    fn registration_settings(&mut self, _id: &str) -> Result<Option<Value>, Self::Error> {
        Ok(None)
    }

    /// Records, at `now`, that the registration `id` was gone on with,
    /// under the definition `hash` ([`Pending::keep_definition`] has
    /// kept it): a store that keeps a registration's definition,
    /// prompt file and settings, and a record of who changed them,
    /// brings them up to date when any differs. One that keeps none
    /// has nothing to do, which is what this does unless it is
    /// written.
    fn change_registration(
        &mut self,
        _id: &str,
        _hash: &str,
        _now: Millis,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Keeps a definition's canonical manifest
    /// ([`clax_surface::canonical_manifest`]) under its hash, at `now`.
    /// One that is kept already stays as it is.
    fn keep_definition(
        &mut self,
        hash: &str,
        canonical: &str,
        now: Millis,
    ) -> Result<(), Self::Error>;

    /// The canonical manifest kept under `hash`.
    fn definition_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error>;

    /// Keeps a prompt file's text under `hash`, the SHA-256 of its
    /// bytes in lowercase hex, at `now`. One that is kept already stays
    /// as it is, last kept at `now`: a store that drops a prompt file
    /// nothing names keeps it from then.
    fn keep_prompt(&mut self, hash: &str, text: &str, now: Millis) -> Result<(), Self::Error>;

    /// The prompt file's text kept under `hash`.
    fn prompt_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error>;

    /// Opens `call` at `now`, unless the store holds a call with its
    /// key, or its registration has `limit` open calls
    /// ([`OpenedCall`]). A key the store holds is answered for before
    /// the limit is looked at.
    fn open_call(
        &mut self,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error>;

    /// What waits for the registration: its open calls without their
    /// arguments, and its unacknowledged ends without their results.
    fn pending_for(&mut self, registration: &str) -> Result<PendingSummary, Self::Error>;

    /// Those of the registration's open calls that `ids` names, whole,
    /// the oldest first.
    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error>;

    /// Marks these calls as sent. Each that is open is then no longer
    /// one whose next attempt waits to be sent: a [`Resend::Planned`]
    /// one is [`Resend::Due`], and neither [`Tries::later`] nor
    /// [`Tries::last`] is kept.
    fn sent_calls(&mut self, ids: &[String]) -> Result<(), Self::Error>;

    /// Marks the registration's open call `id` as accepted, and so as
    /// sent, whether or not it was accepted before; its host has it, so
    /// it is no longer one to send again: [`Resend::None`], and neither
    /// [`Tries::later`] nor [`Tries::last`] is kept. False when it has no
    /// such open call.
    fn accept_call(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error>;

    /// Settles the registration's open call `id` with `outcome`, at
    /// `now`. Nothing changes when it has no such open call, or the
    /// call's deadline is not after `now` ([`Answered::Ignored`]), or it
    /// was settled before ([`Answered::AlreadySettled`] when with this
    /// same outcome).
    fn answer_call(
        &mut self,
        registration: &str,
        id: &str,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<Answered, Self::Error>;

    /// Keeps `tries` with the registration's open call `id`. False, and
    /// nothing changes, when it has no such open call.
    fn keep_tries(
        &mut self,
        registration: &str,
        id: &str,
        tries: &Tries,
    ) -> Result<bool, Self::Error>;

    /// How many times in all `run`'s calls to `registration` were sent
    /// again after a failed attempt: the sum of their
    /// [`Tries::failures`], those settled among them, for as long as the
    /// store holds them.
    fn resends(&mut self, run: &str, registration: &str) -> Result<u32, Self::Error>;

    /// The earliest [`Tries::not_before`] of an open call that is later
    /// than `after`.
    fn next_resend(&mut self, after: Millis) -> Result<Option<Millis>, Self::Error>;

    /// Marks the registration `id` as dropped for good at `at` (its lease
    /// ended, or it made room for another), or as not dropped with `None`
    /// (its host went on as it).
    fn set_dropped(&mut self, id: &str, at: Option<Millis>) -> Result<(), Self::Error>;

    /// Whether the registration `id` is marked as dropped for good.
    fn is_dropped(&mut self, id: &str) -> Result<bool, Self::Error>;

    /// Records that a `serve` of the registration `id` was taken at
    /// `now`, whether or not it is answered: a store that keeps when a
    /// registration was last served keeps this. One that keeps none has
    /// nothing to do, which is what this does unless it is written.
    fn served(&mut self, _id: &str, _now: Millis) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Settles every open call whose deadline is not after `now`, each
    /// with the failure `failure` gives for it, and answers their IDs.
    fn expire_calls(
        &mut self,
        now: Millis,
        failure: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error>;

    /// The earliest deadline of an open call.
    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error>;

    /// Records a run's end at `now`, unless the store holds an
    /// unacknowledged end of the run for the registration, or the
    /// registration has `limit` unacknowledged ends ([`RecordedEnd`]).
    /// An end the store holds is answered for before the limit is
    /// looked at.
    fn record_end(
        &mut self,
        end: NewEnd,
        limit: usize,
        now: Millis,
    ) -> Result<RecordedEnd, Self::Error>;

    /// Those of the registration's unacknowledged ends that `ids`
    /// names, whole, the oldest first.
    fn ends_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingEnd>, Self::Error>;

    /// Marks the registration's end `id` as acknowledged, at `now`.
    /// False when it has no such unacknowledged end.
    fn acknowledge_end(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error>;

    /// Forgets the unacknowledged ends recorded before `before`, oldest
    /// first, and answers how many. A store may forget a bounded number
    /// at each call (the store's forgets
    /// `clax_store::bot_pending::EXPIRE_ENDS_PER_TX`); the rest go at the
    /// calls after.
    fn expire_ends(&mut self, before: Millis) -> Result<usize, Self::Error>;
}

/// What `call` is settled as when its deadline comes: the failure its
/// last attempt came to, when it has not been sent since
/// ([`Tries::last`]), else [`unanswered`].
fn settled_at_deadline(call: &CallSummary) -> CallFailure {
    call.tries.last.clone().unwrap_or_else(|| unanswered(call))
}

/// What a call is settled as whose host said it cannot take it now
/// (`retry_after_ms`) and that is not sent again: at its deadline, or at
/// once for an accepted call of an `@once` operation, which is never sent
/// again. Its host did not take it: nothing happened.
fn not_taken(call: &CallSummary) -> CallFailure {
    CallFailure::new(
        CODE_UNAVAILABLE,
        format!(
            "clax.{}: its host could not take the call and was not sent it again. Nothing happened",
            shown(&call.op)
        ),
    )
    .retryable()
}

/// What a call that was not answered by its deadline is settled as
/// (the module's documentation has the table).
#[must_use]
pub fn unanswered(call: &CallSummary) -> CallFailure {
    let op = shown(&call.op);
    match (call.sent, call.kind) {
        (false, _) => CallFailure::new(
            CODE_UNAVAILABLE,
            format!("clax.{op}: no host took the call before its deadline. Nothing happened"),
        )
        .retryable(),
        (true, CallKind::Read) => CallFailure::new(
            CODE_EXPIRED,
            format!("clax.{op}: its host was sent the call and did not answer before its deadline"),
        ),
        (true, CallKind::Write | CallKind::Effect) => CallFailure::new(
            CODE_UNKNOWN_OUTCOME,
            format!(
                "clax.{op}: its host was sent the call and did not answer before its deadline. It may have taken effect"
            ),
        ),
    }
}

// ---------------------------------------------------------------------------
// Limits and errors
// ---------------------------------------------------------------------------

/// The bounds of a [`Hub`]. Every figure is the environment's to
/// choose; [`HubLimits::default`] gives the daemon's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubLimits {
    /// The limits a registered definition is read under, and its values
    /// checked under.
    pub definition: Limits,
    /// The registrations held at once. Default 32. A definition keeps
    /// what its limits let it (megabytes at the most under the default
    /// ones: `clax_surface`, "Input from outside"), so this bounds the
    /// memory registering can take.
    pub registrations: usize,
    /// The registrations of one name held at once. Default 8.
    pub registrations_per_name: usize,
    /// The definitions of no held registration that are kept ready to
    /// check answers by; one more is read from the store when an
    /// answer needs it. Default 16.
    pub definitions: usize,
    /// The bytes of a registration's prompt file. Default 64 KiB.
    pub prompt_bytes: usize,
    /// The bytes of a registration's settings, as JSON. Default 16 KiB.
    pub settings_bytes: usize,
    /// How often one registration may be gone on with in a way that
    /// changes its definition, its prompt file or its settings, in
    /// each [`HubLimits::changes_ms`]. Default 8.
    pub changes: usize,
    /// The window [`HubLimits::changes`] is counted in, in
    /// milliseconds. Default 60,000.
    pub changes_ms: u64,
    /// The runs one registration may have started that have not ended.
    /// The hub starts no run: whoever does refuses one more. Default
    /// 16.
    pub runs_per_registration: usize,
    /// The bytes of a request's body. The hub is handed messages, not
    /// bytes: whoever reads the body refuses a longer one
    /// ([`wire::CODE_TOO_LARGE`]) before it is read. It bounds what the
    /// answers of one `serve` hold together. Default 8 MiB.
    pub request_bytes: usize,
    /// The open calls of one registration. Default 256.
    pub calls_per_registration: usize,
    /// The unacknowledged ended runs of one registration. Default 256.
    pub ended_per_registration: usize,
    /// The bytes of one call's arguments, as JSON. Default 1 MiB.
    pub args_bytes: usize,
    /// The bytes of one call's result as its host answers it, and of
    /// the result an ended run carries, as the hub writes it as JSON
    /// (no white space between its values). Default 1 MiB.
    pub result_bytes: usize,
    /// The bytes, in UTF-8, kept of the message of an error a host
    /// answers; the rest is cut. Default 4 KiB.
    pub message_bytes: usize,
    /// The bytes of a call's key, of a run's ID and of the ID an
    /// answer names. Default 256.
    pub id_bytes: usize,
    /// The answers one `serve` may bring. Default 1,024.
    pub answers_per_serve: usize,
    /// The deliveries one `serve` is answered with; the rest go out
    /// with the next. Default 64.
    pub deliveries_per_serve: usize,
    /// The bytes one `serve`'s deliveries hold together, counting each
    /// call's arguments and each end's result as JSON; the rest go out
    /// with the next. One delivery is always sent, whatever its size.
    /// Default 8 MiB.
    pub deliveries_bytes: usize,
    /// The `serve`s of one registration open at once. One more ends
    /// the oldest. Default 4.
    pub serves_per_registration: usize,
    /// The longest a `serve` is held, in milliseconds. Default 12,000.
    pub serve_wait_ms: u64,
    /// How long after its last `serve` a registration is connected, in
    /// milliseconds. Default 15,000.
    pub connected_ms: u64,
    /// How long a registration that is not connected is kept, in
    /// milliseconds. Default 300,000.
    pub lease_ms: u64,
    /// How long a call that is given no deadline waits, in
    /// milliseconds. Default 24 hours.
    pub call_ms: u64,
    /// How long an ended run is delivered, in milliseconds. Default
    /// seven days.
    pub ended_ms: u64,
    /// Whether the daemon keeps content (its store keeps blobs:
    /// `clax_store::bot_content`). While it does not (`false`, the
    /// default), an operation whose parameters or result hold a
    /// `Content` or an `Image` is withheld ([`withheld`]); while it
    /// does, none is, and a result's references are filled in from the
    /// store before it settles its call ([`check_answers_filled`]).
    pub content: bool,
    /// The most bytes of one item of content a host uploads. Default
    /// 20 MiB ([`clax_store::bot_content::CONTENT_ITEM_BYTES`]).
    pub content_item_bytes: u64,
    /// The most bytes of all of one run's content together. Default
    /// 100 MiB ([`clax_store::bot_content::CONTENT_RUN_BYTES`]).
    pub content_run_bytes: u64,
    /// The most bytes of the content of all runs together: an upload
    /// that would pass it is refused
    /// ([`clax_store::bot_content::CODE_CONTENT_STORE_FULL`]). Default 1
    /// GiB ([`clax_store::bot_content::CONTENT_TOTAL_BYTES`]).
    pub content_total_bytes: u64,
    /// The uploads of one registration in flight at once; one more is
    /// refused before its body is read. Default 2.
    pub uploads_per_registration: usize,
    /// The events ([`Hub::event`]) kept for one registration until a
    /// `serve` takes them; one more drops the oldest. Default 256.
    pub events_per_registration: usize,
    /// The messages a run may hold that it has not been given yet
    /// (`POST …/runs/<id>/messages`). Default 32.
    pub run_messages: usize,
    /// The bytes of one message to a run, as JSON. Default 64 KiB.
    pub run_message_bytes: usize,
    /// How a call the store holds with no rule kept ([`Tries::rule`]
    /// `None`: none was given) is sent again, until its run makes it
    /// again with its key (its step runs again) and the run's rule is
    /// kept with it. Default [`Retry::NONE`]: such a call is not sent
    /// again after a failure, nor when its host may have lost it, as
    /// every call was before rules were given.
    pub retry_kept: Retry,
    /// How many times in all the calls of one run to one registration
    /// are sent again after a failed attempt (a host's retryable error,
    /// or a host that lost a call it had accepted): each registration a
    /// run calls has its own count, so that one host cannot spend
    /// another's, counted from what the store keeps of the run's calls
    /// ([`Pending::resends`]), so a restart forgets none. A failure past
    /// it is the call's outcome. A host's `retry_after_ms` is not
    /// counted. Default 64.
    pub resends_per_run: u32,
}

impl HubLimits {
    /// The limits on content, as the store holds an upload to them.
    #[must_use]
    pub fn content_limits(&self) -> clax_store::bot_content::ContentLimits {
        clax_store::bot_content::ContentLimits {
            item_bytes: self.content_item_bytes,
            run_bytes: self.content_run_bytes,
            total_bytes: self.content_total_bytes,
        }
    }
}

impl Default for HubLimits {
    fn default() -> Self {
        HubLimits {
            definition: Limits::default(),
            registrations: 32,
            registrations_per_name: 8,
            definitions: 16,
            prompt_bytes: 64 * 1024,
            settings_bytes: 16 * 1024,
            changes: 8,
            changes_ms: 60_000,
            runs_per_registration: 16,
            request_bytes: 8 * 1024 * 1024,
            calls_per_registration: 256,
            ended_per_registration: 256,
            args_bytes: 1024 * 1024,
            result_bytes: 1024 * 1024,
            message_bytes: 4 * 1024,
            id_bytes: 256,
            answers_per_serve: 1024,
            deliveries_per_serve: 64,
            deliveries_bytes: 8 * 1024 * 1024,
            serves_per_registration: 4,
            serve_wait_ms: 12_000,
            connected_ms: 15_000,
            lease_ms: 300_000,
            call_ms: 24 * 60 * 60 * 1000,
            ended_ms: 7 * 24 * 60 * 60 * 1000,
            content: false,
            content_item_bytes: clax_store::bot_content::CONTENT_ITEM_BYTES,
            content_run_bytes: clax_store::bot_content::CONTENT_RUN_BYTES,
            content_total_bytes: clax_store::bot_content::CONTENT_TOTAL_BYTES,
            uploads_per_registration: 2,
            events_per_registration: 256,
            run_messages: 32,
            run_message_bytes: 64 * 1024,
            retry_kept: Retry::NONE,
            resends_per_run: 64,
        }
    }
}

/// Why a surface was not registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// The request is of a version of the protocol other than
    /// [`wire::VERSION`].
    UnsupportedVersion(u64),
    /// The request names a registration that was not issued, or was
    /// issued for another surface.
    UnknownRegistration,
    /// The ID the caller drew for a new registration is not 32
    /// lowercase hexadecimal digits, or was issued before: it was not
    /// drawn at random.
    FreshInvalid,
    /// The prompt file is longer than [`HubLimits::prompt_bytes`].
    PromptTooLarge { bytes: usize, limit: usize },
    /// The prompt file is not one for the surface
    /// ([`crate::prompt::check_prompt_file`]): the message says why.
    InvalidPrompt(String),
    /// A setting is not one: the message says which.
    InvalidSettings(String),
    /// The definition's text is not a definition.
    InvalidDefinition(Errors),
    /// The definition's surface has another name than the one it is
    /// registered under.
    NameMismatch { route: String, definition: String },
    /// As many registrations are held as [`HubLimits::registrations`]
    /// or [`HubLimits::registrations_per_name`] allows, and every one
    /// that could make room is connected.
    TooManyRegistrations { limit: usize, of_name: bool },
    /// The registration was gone on with, with something changed, as
    /// often as [`HubLimits::changes`] allows in
    /// [`HubLimits::changes_ms`].
    TooManyChanges { limit: usize, window_ms: u64 },
    /// The store could not be read or written: why, for a log. The
    /// refusal a host is answered ([`RegisterError::body`]) does not
    /// carry it.
    Storage(String),
}

/// The refusal of a registration that changes too often
/// ([`RegisterError::TooManyChanges`]): 429.
pub const CODE_TOO_MANY_CHANGES: &str = "too_many_changes";
/// The refusal of a start for a registration with as many runs that
/// have not ended as [`HubLimits::runs_per_registration`]: 429.
pub const CODE_TOO_MANY_RUNS: &str = "too_many_runs";

/// What a host or a script is told of a store that failed. Why is the
/// daemon's to log, and not theirs to read.
const STORE_FAILED: &str = "the daemon could not read or write its store: try again";

impl RegisterError {
    /// The refusal as the protocol writes it.
    #[must_use]
    pub fn body(&self) -> ErrorBody {
        let code = match self {
            RegisterError::InvalidDefinition(errors) => {
                return ErrorBody::invalid_definition(errors);
            }
            RegisterError::UnsupportedVersion(asked) => {
                return ErrorBody::unsupported_version(*asked);
            }
            RegisterError::UnknownRegistration => CODE_UNKNOWN_REGISTRATION,
            RegisterError::FreshInvalid => CODE_INTERNAL_ERROR,
            RegisterError::PromptTooLarge { .. } => CODE_PROMPT_TOO_LARGE,
            RegisterError::InvalidPrompt(_) => CODE_INVALID_PROMPT,
            RegisterError::InvalidSettings(_) => CODE_INVALID_SETTINGS,
            RegisterError::NameMismatch { .. } => CODE_NAME_MISMATCH,
            RegisterError::TooManyRegistrations { .. } => CODE_TOO_MANY_REGISTRATIONS,
            RegisterError::TooManyChanges { .. } => CODE_TOO_MANY_CHANGES,
            RegisterError::Storage(_) => {
                return ErrorBody::new(CODE_STORAGE_FAILED, STORE_FAILED);
            }
        };
        ErrorBody::new(code, self.to_string())
    }
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterError::UnsupportedVersion(v) => {
                f.write_str(&ErrorBody::unsupported_version(*v).message)
            }
            RegisterError::UnknownRegistration => f.write_str(
                "`registration` is not the ID of a registration of this surface: send the one an earlier registration of it was answered, or leave it out",
            ),
            RegisterError::FreshInvalid => {
                f.write_str("the registration could not be given an ID: register again")
            }
            RegisterError::PromptTooLarge { bytes, limit } => write!(
                f,
                "the prompt file is {bytes} bytes long, and the limit is {limit}"
            ),
            RegisterError::InvalidPrompt(m) => write!(f, "the prompt file does not read: {m}"),
            RegisterError::InvalidSettings(m) => f.write_str(m),
            RegisterError::InvalidDefinition(errors) => write!(f, "{errors}"),
            RegisterError::NameMismatch { route, definition } => write!(
                f,
                "the definition is of a surface named `{}`, and it is registered as `{}`",
                shown(definition),
                shown(route)
            ),
            RegisterError::TooManyRegistrations {
                limit,
                of_name: true,
            } => write!(
                f,
                "{limit} registrations of this name are connected, which is the limit: one makes room when its host stops serving it"
            ),
            RegisterError::TooManyRegistrations {
                limit,
                of_name: false,
            } => write!(
                f,
                "{limit} registrations are connected, which is the limit: one makes room when its host stops serving it"
            ),
            RegisterError::TooManyChanges { limit, window_ms } => write!(
                f,
                "the registration changed its definition, prompt file or settings {limit} times in {} seconds, which is the limit: register again later",
                window_ms / 1000
            ),
            RegisterError::Storage(m) => {
                write!(f, "the registration could not be recorded: {m}")
            }
        }
    }
}

impl std::error::Error for RegisterError {}

/// Why a `serve`, or anything else a host asks of a registration, was
/// refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeError {
    /// The hub holds no registration of this name with this ID: it
    /// never had, or its lease ended. The host registers again, as the
    /// registration it was.
    UnknownRegistration,
    /// The request brings more answers than
    /// [`HubLimits::answers_per_serve`]. None was taken.
    TooManyAnswers { limit: usize },
    /// The store could not be read or written: why, for a log (the
    /// refusal a host is answered does not carry it). Some of the
    /// request's answers may have been taken; the host sends them
    /// again.
    Storage(String),
}

impl ServeError {
    /// The refusal as the protocol writes it.
    #[must_use]
    pub fn body(&self) -> ErrorBody {
        let code = match self {
            ServeError::UnknownRegistration => CODE_UNKNOWN_REGISTRATION,
            ServeError::TooManyAnswers { .. } => CODE_TOO_MANY_ANSWERS,
            ServeError::Storage(_) => {
                return ErrorBody::new(CODE_STORAGE_FAILED, STORE_FAILED);
            }
        };
        ErrorBody::new(code, self.to_string())
    }
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::UnknownRegistration => f.write_str(
                "the surface has no such registration now: register again with `registration` set to this ID (PUT /api/bot/surfaces/<name>)",
            ),
            ServeError::TooManyAnswers { limit } => write!(
                f,
                "one serve brings at most {limit} answers: none of these was taken; send them in several"
            ),
            ServeError::Storage(m) => write!(f, "the calls could not be read or written: {m}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// A call for [`Hub::enqueue`].
#[derive(Clone, Debug, PartialEq)]
pub struct Enqueue {
    /// The run that makes the call.
    pub run: String,
    /// The operation, by the name the definition gives it
    /// ([`clax_surface::Operation::name`]): `shell.settings.set`.
    pub op: String,
    /// The arguments, checked against the definition
    /// ([`clax_surface::Operation::check_args`]): one for each
    /// parameter.
    pub args: Vec<Value>,
    /// The call's key: `<run>:<step>:<seq>`.
    pub key: String,
    /// When the call is settled if nothing has settled it. `None` is
    /// [`HubLimits::call_ms`] from now.
    pub deadline: Option<Millis>,
    /// How the call is sent again when an attempt fails. A call made
    /// again with its key keeps the rule and the attempts of the call
    /// the hub holds.
    pub retry: Retry,
    /// What a run a join bound the surface into is on
    /// ([`wire::Call::joined`]); `None` for a run the registration
    /// started.
    pub joined: Option<Value>,
}

/// Why a call was not opened. Nothing was sent to any host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueError {
    /// The hub holds no such registration: it never did, or the
    /// registration's lease ended.
    NoRegistration,
    /// The registration is not connected.
    NotConnected,
    /// The registration's definition has no such operation.
    UnknownOp(String),
    /// The operation is withheld from every run
    /// ([`Registered::withheld`]).
    Withheld(String),
    /// The operation is one no host performs (`@spend`).
    NotAHostCall(String),
    /// The key is held by a call that is not this one.
    KeyReused,
    /// The deadline is not after now.
    PastDeadline,
    /// The arguments are longer than [`HubLimits::args_bytes`].
    ArgsTooLarge { limit: usize },
    /// The key or the run's ID is empty or longer than
    /// [`HubLimits::id_bytes`].
    IdTooLong { limit: usize },
    /// The registration has as many open calls as
    /// [`HubLimits::calls_per_registration`] allows.
    QueueFull { limit: usize },
    /// The store could not be read or written.
    Storage(String),
}

impl EnqueueError {
    /// What the script that made the call is thrown. Of a store that
    /// failed it says that and not why.
    #[must_use]
    pub fn failure(&self) -> CallFailure {
        let message = match self {
            EnqueueError::Storage(_) => format!("{STORE_FAILED}. Nothing happened"),
            other => other.to_string(),
        };
        match self {
            EnqueueError::NoRegistration | EnqueueError::PastDeadline => {
                CallFailure::new(CODE_UNAVAILABLE, message)
            }
            EnqueueError::NotConnected
            | EnqueueError::QueueFull { .. }
            | EnqueueError::Storage(_) => CallFailure::new(CODE_UNAVAILABLE, message).retryable(),
            EnqueueError::UnknownOp(_) | EnqueueError::Withheld(_) => {
                CallFailure::new(CODE_UNKNOWN_OP, message)
            }
            EnqueueError::ArgsTooLarge { .. } => CallFailure::new(CODE_INVALID_ARGS, message),
            EnqueueError::KeyReused => CallFailure::new(CODE_KEY_REUSED, message),
            EnqueueError::NotAHostCall(_) | EnqueueError::IdTooLong { .. } => {
                CallFailure::new(CODE_INTERNAL_ERROR, message)
            }
        }
    }
}

impl fmt::Display for EnqueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnqueueError::NoRegistration => f.write_str(
                "the surface's host is gone: its registration has ended. Nothing happened",
            ),
            EnqueueError::NotConnected => {
                f.write_str("the surface's host is not connected. Nothing happened")
            }
            EnqueueError::UnknownOp(op) => write!(f, "clax.{} is not an operation", shown(op)),
            EnqueueError::Withheld(op) => write!(
                f,
                "clax.{} is not available yet: its types name `Content` or `Image`",
                shown(op)
            ),
            EnqueueError::NotAHostCall(op) => {
                write!(f, "clax.{} is not an operation a host performs", shown(op))
            }
            EnqueueError::KeyReused => {
                f.write_str("the call's key is held by another call. Nothing happened")
            }
            EnqueueError::PastDeadline => {
                f.write_str("the call's deadline had passed before it was sent. Nothing happened")
            }
            EnqueueError::ArgsTooLarge { limit } => write!(
                f,
                "the call's arguments are too large to send: the limit is {limit} bytes of JSON"
            ),
            EnqueueError::IdTooLong { limit } => write!(
                f,
                "a call's key and its run's ID are each 1 to {limit} bytes long"
            ),
            EnqueueError::QueueFull { limit } => write!(
                f,
                "the surface's host has {limit} calls waiting, which is the limit. Nothing happened"
            ),
            EnqueueError::Storage(m) => {
                write!(f, "the call could not be recorded: {m}. Nothing happened")
            }
        }
    }
}

impl std::error::Error for EnqueueError {}

/// Why a run's end was not recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndedError {
    /// The registration's ID was never issued.
    NoRegistration,
    /// The result is longer than [`HubLimits::result_bytes`].
    ResultTooLarge { limit: usize },
    /// The run's ID is empty or longer than [`HubLimits::id_bytes`].
    IdTooLong { limit: usize },
    /// The registration has as many unacknowledged ends as
    /// [`HubLimits::ended_per_registration`] allows.
    QueueFull { limit: usize },
    /// The store could not be read or written.
    Storage(String),
}

impl fmt::Display for EndedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndedError::NoRegistration => f.write_str("no such registration was ever made"),
            EndedError::ResultTooLarge { limit } => write!(
                f,
                "the run's result is too large to send: the limit is {limit} bytes of JSON"
            ),
            EndedError::IdTooLong { limit } => {
                write!(f, "a run's ID is 1 to {limit} bytes long")
            }
            EndedError::QueueFull { limit } => write!(
                f,
                "{limit} ended runs wait for the surface's host, which is the limit"
            ),
            EndedError::Storage(m) => write!(f, "the run's end could not be recorded: {m}"),
        }
    }
}

impl std::error::Error for EndedError {}

// ---------------------------------------------------------------------------
// The hub
// ---------------------------------------------------------------------------

/// A registration, as [`Hub::registration`] shows it.
#[derive(Clone, Copy, Debug)]
pub struct Registration<'a> {
    /// The surface's name.
    pub name: &'a str,
    pub definition: &'a Definition,
    /// The SHA-256 of the definition's canonical bytes, in lowercase
    /// hex.
    pub hash: &'a str,
    /// The prompt file's text, as registered: checked for the surface
    /// when it was registered ([`crate::prompt::check_prompt_file`]).
    pub prompt: Option<&'a str>,
    /// The settings, as registered. Its allow patterns are patterns;
    /// that each setting is within the instance's own is not checked
    /// here.
    pub settings: &'a Settings,
    /// The operations no run is given, by the names the protocol
    /// writes them with ([`Withheld::op`]).
    pub withheld: &'a [Withheld],
}

/// An open `serve`: what [`Hub::open`] answers and [`Hub::close`]
/// takes.
#[derive(Debug, PartialEq, Eq)]
pub struct Serve {
    registration: String,
    hold: u64,
    until: Millis,
}

impl Serve {
    /// The ID of the registration that serves.
    #[must_use]
    pub fn registration(&self) -> &str {
        &self.registration
    }

    /// The instant the `serve`'s wait is over: it is closed then, if
    /// nothing made it ready before.
    #[must_use]
    pub fn until(&self) -> Millis {
        self.until
    }
}

/// What [`Hub::open`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    pub serve: Serve,
    /// How many of the request's answers were ignored: for an ID the
    /// store does not hold for this registration, or for a call
    /// already settled with something else.
    pub ignored: usize,
    /// The IDs those answers named, in their order: what the host is
    /// told was not taken ([`wire::ServeResponse::ignored`]). An answer
    /// not among them was taken, now or (the same answer, sent again)
    /// before.
    pub ignored_ids: Vec<String>,
    /// The IDs of the calls this settled: those the request answered,
    /// and those whose deadline had come. Their runs have an outcome
    /// to read.
    pub settled: Vec<String>,
}

/// What an open `serve` is to do now ([`Hub::ready`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    /// Nothing is new to its host, and its wait is not over: it
    /// waits, until its wake hook is called or [`Serve::until`] comes.
    Wait,
    /// Something is new to its host, or its wait is over: it is closed
    /// now, and answered with what is due.
    Deliver,
    /// It is a `serve` no longer: a later one of its registration took
    /// its place ([`HubLimits::serves_per_registration`]), or its
    /// registration is gone. It is closed now, and answered with
    /// nothing.
    Ended,
}

/// How much a [`Hub`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    pub registrations: usize,
    /// Open `serve`s.
    pub serves: usize,
    /// Definitions kept ready: those of the registrations, and those
    /// read back from the store to check answers by.
    pub definitions: usize,
}

/// A registration's request, read: everything of [`Hub::register`]
/// that needs no hub ([`read_registration`]).
#[derive(Clone, Debug)]
pub struct Reading {
    name: String,
    registration: Option<String>,
    definition: Arc<Definition>,
    canonical: String,
    hash: String,
    withheld: Vec<Withheld>,
    prompt: Option<String>,
    prompt_hash: Option<String>,
    settings: Settings,
}

impl Reading {
    /// The SHA-256 of the definition's canonical bytes, in lowercase
    /// hex.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The prompt file's text, when the request has one.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    /// The SHA-256 of the prompt file's bytes, in lowercase hex, when
    /// the request has one.
    #[must_use]
    pub fn prompt_hash(&self) -> Option<&str> {
        self.prompt_hash.as_deref()
    }

    /// The settings the request gives.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
}

/// A registration as [`Hub::snapshot`] gives it: its own to keep, so
/// that what is checked against it is checked with the hub let go.
///
/// The definition and the prompt file are those of one request to
/// register: the prompt file was checked against this definition
/// ([`read_registration`]). A run started from a snapshot is bound to
/// both by hash (`hash`, `prompt_hash`), so the prompt file a run is
/// sent is always one that was checked against the run's definition.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The surface's name.
    pub name: String,
    pub definition: Arc<Definition>,
    /// The SHA-256 of the definition's canonical bytes, in lowercase
    /// hex.
    pub hash: String,
    /// The prompt file's text, when the registration has one.
    pub prompt: Option<String>,
    /// The SHA-256 of the prompt file's bytes, in lowercase hex: what
    /// its text is kept under ([`Pending::keep_prompt`]) and a run
    /// started now names in its binding.
    pub prompt_hash: Option<String>,
    pub settings: Settings,
    pub withheld: Vec<Withheld>,
}

/// The longest a limit of a registration's settings may be, in its own
/// unit: a year of seconds or of minutes, a million steps or effects.
const SETTING_MOST: [(&str, u64); 5] = [
    ("call_timeout_s", 365 * 24 * 60 * 60),
    ("wait_limit_s", 365 * 24 * 60 * 60),
    ("max_effects_per_run", 1_000_000),
    ("runs.max_steps", 1_000_000),
    ("runs.max_run_minutes", 365 * 24 * 60),
];

/// The allow patterns `list` holds, within `limits`: no more than
/// [`Limits::list`] of them, each a pattern ([`Pattern::parse`]) of no
/// more names than a definition's operation can have
/// ([`Limits::depth`] namespaces, the surface before them and the
/// operation after) and none longer than [`Limits::name_bytes`], as a
/// kind's `@allow` is held to.
///
/// # Errors
/// What is wrong with the first that is not, in bounded words.
pub fn allow_patterns(list: &[String], limits: &Limits) -> Result<Vec<Pattern>, String> {
    if list.len() > limits.list {
        return Err(format!(
            "`allow` lists {} patterns, and the limit is {}",
            list.len(),
            limits.list
        ));
    }
    let names = limits.depth.min(Limits::MAX_DEPTH) + 2;
    // The longest a pattern within those bounds is written: its names,
    // the dots between them, and `.*`.
    let longest = names * (limits.name_bytes + 1) + 2;
    list.iter()
        .map(|pattern| {
            let dotted = pattern.strip_suffix(".*").unwrap_or(pattern);
            if pattern.len() > longest
                || dotted.split('.').count() > names
                || dotted.split('.').any(|name| name.len() > limits.name_bytes)
            {
                return Err(format!(
                    "`allow`: the pattern `{}` is too long: a pattern has {names} names at the most, each of {} bytes at the most",
                    shown(pattern),
                    limits.name_bytes
                ));
            }
            Pattern::parse(pattern).map_err(|m| format!("`allow`: {m}"))
        })
        .collect()
}

/// Holds a registration's settings to their bounds: no longer than
/// [`HubLimits::settings_bytes`] as JSON, `allow` as
/// [`allow_patterns`] takes it, and each limit `"none"` or a number in
/// its range: 1 at the least for a time or a count of steps (0 would
/// let nothing happen; `max_effects_per_run` may be 0), and no more
/// than a year or a million. `reader_view` is refused whatever it is:
/// nothing implements it yet, and a registration that relies on it is
/// told so rather than served as if it held.
fn check_settings(limits: &HubLimits, settings: &Settings) -> Result<(), String> {
    if let Some(view) = settings.reader_view {
        let view = serde_json::to_value(view).unwrap_or(Value::Null);
        return Err(format!(
            "`reader_view` ({view}) is not supported yet: nothing shows a run's readers any \
             part of a host's surface. Leave it out"
        ));
    }
    if let Some(allow) = &settings.allow {
        allow_patterns(allow, &limits.definition)?;
    }
    let bytes = json_bytes(settings);
    if bytes > limits.settings_bytes {
        return Err(format!(
            "the settings are {bytes} bytes of JSON, and the limit is {}",
            limits.settings_bytes
        ));
    }
    let runs = settings.runs.clone().unwrap_or_default();
    let given = [
        settings.call_timeout_s,
        settings.wait_limit_s,
        settings.max_effects_per_run,
        runs.max_steps,
        runs.max_run_minutes,
    ];
    for ((name, most), given) in SETTING_MOST.into_iter().zip(given) {
        let least = u64::from(name != "max_effects_per_run");
        if let Some(wire::Bound::At(n)) = given
            && !(least..=most).contains(&n)
        {
            return Err(format!(
                "`{name}` is {n}: it is \"none\" or a whole number from {least} to {most}"
            ));
        }
    }
    Ok(())
}

/// Reads a registration's request for the surface `name`: everything
/// of registering that needs no hub and no store, so that whoever
/// keeps a hub behind a lock does it before taking the lock
/// ([`Hub::register_parsed`]). The version, the prompt file's length,
/// the settings ([`HubLimits::settings_bytes`], `allow`'s patterns,
/// each limit's range), the definition as a host's within
/// [`HubLimits::definition`] and not named as `reserved` holds, its
/// name, and the prompt file against it.
///
/// # Errors
/// [`RegisterError`], but those only a hub can find.
pub fn read_registration(
    limits: &HubLimits,
    reserved: &Reserved,
    name: &str,
    request: &RegisterRequest,
) -> Result<Reading, RegisterError> {
    if request.clax_surface != wire::VERSION {
        return Err(RegisterError::UnsupportedVersion(request.clax_surface));
    }
    if let Some(prompt) = &request.prompt
        && prompt.len() > limits.prompt_bytes
    {
        return Err(RegisterError::PromptTooLarge {
            bytes: prompt.len(),
            limit: limits.prompt_bytes,
        });
    }
    check_settings(limits, &request.settings).map_err(RegisterError::InvalidSettings)?;
    let author = Author::Host { reserved };
    let definition = match request.notation {
        Notation::Dts => {
            parse(&request.definition, author, &limits.definition).map_err(Errors::from)
        }
        Notation::Manifest => parse_manifest(&request.definition, author, &limits.definition),
    }
    .map_err(RegisterError::InvalidDefinition)?;
    if definition.name() != name {
        return Err(RegisterError::NameMismatch {
            route: name.to_string(),
            definition: definition.name().to_string(),
        });
    }
    if let Some(prompt) = &request.prompt {
        crate::prompt::check_prompt_file(prompt, &definition)
            .map_err(RegisterError::InvalidPrompt)?;
    }
    let canonical = canonical_manifest(&definition);
    Ok(Reading {
        name: name.to_string(),
        registration: request.registration.clone(),
        hash: sha256_hex(canonical.as_bytes()),
        withheld: withheld(&definition, limits.content),
        definition: Arc::new(definition),
        canonical,
        prompt_hash: request.prompt.as_deref().map(|p| sha256_hex(p.as_bytes())),
        prompt: request.prompt.clone(),
        settings: request.settings.clone(),
    })
}

/// An answer that names a call whose result or error is to be judged
/// ([`Hub::checkable`]): the place of the answer in its request, and
/// what its call was made under.
#[derive(Clone, Debug)]
pub struct Checking {
    answer: usize,
    definition: Arc<Definition>,
    op: String,
}

/// An answer as [`Hub::open_checked`] takes it: as its host sent it,
/// or with its result or error already judged ([`check_answers`]).
#[derive(Clone, Debug)]
pub struct Checked {
    id: String,
    outcome: CheckedOutcome,
}

#[derive(Clone, Debug)]
enum CheckedOutcome {
    /// As it was sent.
    Raw(Outcome),
    /// What its call is settled as.
    Judged(Result<Value, CallFailure>),
}

impl From<Answer> for Checked {
    fn from(answer: Answer) -> Checked {
        Checked {
            id: answer.id,
            outcome: CheckedOutcome::Raw(answer.outcome),
        }
    }
}

/// What fills a result's references to uploads in from the store, so
/// that the script is given whole handles
/// (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
/// "Content"): the daemon's, over `clax_store::bot_content`.
pub trait FillContent {
    /// The whole handle for `reference` (`{"id": …, "meta"?: …}`, as its
    /// host sent it in the answer to the call `call`): the item's row,
    /// which must be one the call's run holds and the call's registration
    /// uploaded, with the reference's `meta`.
    ///
    /// # Errors
    /// Why it cannot be filled in, as the rest of a sentence after the
    /// place in the answer (`names no content of this run`): the call
    /// settles `invalid_result`.
    fn fill(&self, call: &str, reference: &Value) -> Result<Value, String>;
}

/// What a call of `op`, made under `definition`, is settled as by the
/// result or the error its host answered (`settling`): the result when
/// it is the operation's declared result and no longer than
/// [`HubLimits::result_bytes`]; the error when the operation declares
/// its code, or it is `unavailable`; `invalid_result` for any other,
/// and for a code only Clax's core gives ([`wire::CORE_CODES`]) even
/// when the definition declares it (one a check of this build would
/// refuse, kept from before it). `None` for an answer that settles
/// nothing.
///
/// A result whose type holds a `Content` or an `Image` has each
/// reference in it filled in by `fill` for the call `id`, and is then
/// checked as the script is to receive it
/// ([`clax_surface::Operation::check_result`]): an `Image` whose bytes
/// are no image fails it. With no `fill` such a result is
/// `invalid_result`: nothing here keeps content.
fn judge(
    limits: &HubLimits,
    definition: &Definition,
    op: &str,
    id: &str,
    settling: Outcome,
    fill: Option<&dyn FillContent>,
) -> Option<Result<Value, CallFailure>> {
    let shown_op = shown(op);
    let Some(operation) = definition.operation(op) else {
        return Some(Err(CallFailure::new(
            CODE_INVALID_RESULT,
            format!("clax.{shown_op} is not an operation of the definition it was called under"),
        )));
    };
    Some(match settling {
        Outcome::Accepted | Outcome::RetryAfter(_) => return None,
        Outcome::Result(value) => {
            let limit = limits.result_bytes;
            let checked = if json_bytes(&value) > limit {
                Err(format!(
                    "clax.{shown_op}: the answer is too large: the limit is {limit} bytes of JSON"
                ))
            } else {
                operation
                    .content_in_answer(&value)
                    .map_err(|e| e.message)
                    .and_then(|places| filled(&operation, value, &places, id, fill))
            };
            match checked {
                Ok(value) => Ok(value),
                Err(message) => Err(CallFailure::new(CODE_INVALID_RESULT, message)),
            }
        }
        Outcome::Error(error) => {
            let message = cut(&error.message, limits.message_bytes);
            if error.code == CODE_UNAVAILABLE {
                Err(CallFailure::new(error.code, message).retryable())
            } else if wire::CORE_CODES.contains(&error.code.as_str()) {
                Err(CallFailure::new(
                    CODE_INVALID_RESULT,
                    format!(
                        "clax.{shown_op}: its host answered with the error `{}`, which only Clax gives a script: {message}",
                        shown(&error.code)
                    ),
                ))
            } else if operation.op.throws.contains(&error.code) {
                let failure = CallFailure::new(error.code, message);
                Err(if error.retryable {
                    failure.retryable()
                } else {
                    failure
                })
            } else {
                Err(CallFailure::new(
                    CODE_INVALID_RESULT,
                    format!(
                        "clax.{shown_op}: its host answered with the error `{}`, which the operation does not declare: {message}",
                        shown(&error.code)
                    ),
                ))
            }
        }
    })
}

/// `value`, an answer of `operation` to the call `id` that passed its
/// check, with the references at `places` filled in by `fill` and the
/// whole checked as the script is to receive it.
fn filled(
    operation: &clax_surface::Operation<'_>,
    mut value: Value,
    places: &[clax_surface::ContentPlace],
    id: &str,
    fill: Option<&dyn FillContent>,
) -> Result<Value, String> {
    if places.is_empty() {
        return Ok(value);
    }
    let name = shown(operation.name);
    let Some(fill) = fill else {
        return Err(format!(
            "clax.{name}: the answer holds content, and this daemon keeps none"
        ));
    };
    for place in places {
        let at = |rest: &str| {
            if place.at.is_empty() {
                format!("clax.{name}: the answer {rest}")
            } else {
                format!("clax.{name}: the answer at {} {rest}", shown(&place.at))
            }
        };
        let Some(slot) = value.pointer_mut(&place.at) else {
            return Err(at("is not there"));
        };
        *slot = fill.fill(id, slot).map_err(|why| at(&why))?;
    }
    operation.check_result(&value).map_err(|e| e.message)?;
    Ok(value)
}

/// Judges the results and errors of `answers` that `checking` names
/// ([`Hub::checkable`]), with no hub: each against its own call's
/// definition, as [`Hub::open`] would. The work of one is bounded by
/// the definition's limits and [`HubLimits::result_bytes`], and is the
/// work that is not to be done under a lock every host shares. A
/// result that holds content is `invalid_result` (nothing fills it in:
/// [`check_answers_filled`]).
#[must_use]
pub fn check_answers(
    limits: &HubLimits,
    answers: Vec<Answer>,
    checking: &[Checking],
) -> Vec<Checked> {
    check_answers_filled(limits, answers, checking, None)
}

/// [`check_answers`], with each result's references to uploads filled
/// in by `fill` before the result is checked as the script is to
/// receive it.
#[must_use]
pub fn check_answers_filled(
    limits: &HubLimits,
    answers: Vec<Answer>,
    checking: &[Checking],
    fill: Option<&dyn FillContent>,
) -> Vec<Checked> {
    answers
        .into_iter()
        .enumerate()
        .map(|(n, answer)| {
            let Some(of) = checking.iter().find(|c| c.answer == n) else {
                return Checked::from(answer);
            };
            let id = answer.id;
            let judged = judge(
                limits,
                &of.definition,
                &of.op,
                &id,
                answer.outcome.clone(),
                fill,
            );
            let outcome = match judged {
                Some(judged) => CheckedOutcome::Judged(judged),
                None => CheckedOutcome::Raw(answer.outcome),
            };
            Checked { id, outcome }
        })
        .collect()
}

struct Reg {
    name: String,
    definition: Arc<Definition>,
    hash: String,
    prompt: Option<String>,
    prompt_hash: Option<String>,
    settings: Settings,
    withheld: Vec<Withheld>,
    /// Its place among the registrations, in the order they were made
    /// or gone on with.
    made: u64,
    /// The open `serve`s, each with the instant it lapses after.
    holds: BTreeMap<u64, Millis>,
    /// When it was last known to be served: registered, a `serve`
    /// closed, or an open one lapsed.
    alive: Millis,
    /// Whether its host may have lost the calls it accepted: it was
    /// gone on with (its host started again) and has not served since.
    lost: bool,
    /// The ends a `serve` was answered with.
    ends_sent: BTreeSet<String>,
    /// When it was gone on with, with something changed, of late
    /// ([`HubLimits::changes`]).
    changes: Vec<Millis>,
    /// The events no `serve` has taken yet, the oldest first
    /// ([`Hub::event`]).
    events: std::collections::VecDeque<wire::Event>,
}

/// Called with a registration's ID when there is something new to send
/// it: whoever holds a `serve` of that registration asks
/// [`Hub::ready`] again. It is called from inside [`Hub::enqueue`] and
/// [`Hub::ended`] and [`Hub::event`], under whatever lock the hub is kept under, so it
/// does not call the hub back; and a `serve` begins to listen for it
/// before it first asks [`Hub::ready`], or a call that arrives between
/// the two is not seen until the wait is over.
pub type Wake = Box<dyn Fn(&str) + Send + Sync>;

/// The registrations, and the rules of serving them. See the module's
/// documentation.
pub struct Hub {
    limits: HubLimits,
    reserved: Reserved,
    wake: Wake,
    now: Millis,
    /// The number the next registration or `serve` gets.
    next: u64,
    registrations: BTreeMap<String, Reg>,
    /// Definitions by hash: every registration's, and those read back
    /// from the store, each with when it was last used.
    definitions: BTreeMap<String, (Arc<Definition>, u64)>,
    /// How many calls it has settled and ends it has recorded
    /// ([`Hub::settles`]).
    settles: u64,
    /// The registrations whose lease ended where no store was at hand,
    /// each with when: marked as dropped in the store
    /// ([`Pending::set_dropped`]) by the next method that has one.
    unmarked: BTreeMap<String, Millis>,
}

impl fmt::Debug for Hub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hub")
            .field("held", &self.held())
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

/// `s` for a message: cut to 64 characters, with every character that
/// would not show written as its code.
fn shown(s: &str) -> String {
    let mut out: String = s.chars().take(64).flat_map(char::escape_debug).collect();
    if s.chars().nth(64).is_some() {
        out.push('…');
    }
    out
}

/// The bytes of `v` as JSON, without keeping them.
fn json_bytes<T: serde::Serialize + ?Sized>(v: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    match serde_json::to_writer(&mut count, v) {
        Ok(()) => count.0,
        // A value that cannot be written has no size to be within.
        Err(_) => usize::MAX,
    }
}

/// Whether `s` has the form of a registration's ID: 32 lowercase
/// hexadecimal digits, which is 128 bits.
#[must_use]
pub fn is_registration_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `s` cut to `limit` bytes at the most, at a character's end.
fn cut(s: &str, limit: usize) -> &str {
    if s.len() <= limit {
        return s;
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The operations of `definition` that a run would have but for their
/// parameters or result holding a `Content` or an `Image`, in the
/// definition's order: what a daemon that stores no content withholds,
/// and none for one that does (`content`, [`HubLimits::content`]).
/// The rule is `clax_surface`'s ([`Definition::run_operations`], asked
/// for each kind with nothing else narrowing it, and
/// [`Definition::joined_operations`], for a run of Clax's own a join
/// binds the surface into, which no kind's `@allow` narrows), so an
/// operation no kind allows is listed too: a joined run may have it.
#[must_use]
pub fn withheld(definition: &Definition, content: bool) -> Vec<Withheld> {
    if content {
        return Vec::new();
    }
    let prefix = format!("{}.", definition.name());
    let mut held: BTreeSet<&str> = BTreeSet::new();
    let runs = definition
        .kinds()
        .iter()
        .filter_map(|kind| definition.run_operations(&kind.name, &RunScope::default()))
        .chain([definition.joined_operations(&RunScope::default())]);
    for run in runs {
        held.extend(
            run.withheld
                .into_iter()
                .filter(|(_, why)| *why == clax_surface::Withheld::Content)
                .map(|(op, _)| op),
        );
    }
    definition
        .operations()
        .filter(|o| held.contains(o.name))
        .map(|o| Withheld {
            op: o.name.strip_prefix(&prefix).unwrap_or(o.name).to_string(),
            reason: WITHHELD_CONTENT.to_string(),
        })
        .collect()
}

impl Hub {
    /// A hub with no registration.
    ///
    /// `reserved` holds the names a host's surface may not take
    /// ([`Reserved::of`] Clax's own surfaces). `wake` is the hook
    /// ([`Wake`]).
    #[must_use]
    pub fn new(limits: HubLimits, reserved: Reserved, wake: Wake) -> Hub {
        Hub {
            limits,
            reserved,
            wake,
            now: Millis(0),
            next: 1,
            registrations: BTreeMap::new(),
            definitions: BTreeMap::new(),
            settles: 0,
            unmarked: BTreeMap::new(),
        }
    }

    /// The names a host's surface may not take, as the hub was made
    /// with them: what [`read_registration`] is given.
    #[must_use]
    pub fn reserved(&self) -> &Reserved {
        &self.reserved
    }

    /// How many calls the hub has settled (by an answer or a deadline)
    /// and ends it has recorded, so far. Whoever lends the hub out
    /// compares it before and after, to tell those that wait for a
    /// call's outcome only when there may be one.
    #[must_use]
    pub fn settles(&self) -> u64 {
        self.settles
    }

    /// The registration with this ID as its own copy, when the hub
    /// holds it: what a caller checks against after it has let the hub
    /// go.
    #[must_use]
    pub fn snapshot(&self, id: &str) -> Option<Snapshot> {
        let reg = self.registrations.get(id)?;
        Some(Snapshot {
            name: reg.name.clone(),
            definition: reg.definition.clone(),
            hash: reg.hash.clone(),
            prompt: reg.prompt.clone(),
            prompt_hash: reg.prompt_hash.clone(),
            settings: reg.settings.clone(),
            withheld: reg.withheld.clone(),
        })
    }

    /// The limits the hub was made with.
    #[must_use]
    pub fn limits(&self) -> &HubLimits {
        &self.limits
    }

    /// How much the hub holds now.
    #[must_use]
    pub fn held(&self) -> Held {
        Held {
            registrations: self.registrations.len(),
            serves: self.registrations.values().map(|r| r.holds.len()).sum(),
            definitions: self.definitions.len(),
        }
    }

    fn take_number(&mut self) -> u64 {
        let n = self.next;
        self.next = self.next.saturating_add(1);
        n
    }

    /// Brings the hub to `now`: a `serve` whose wait is over no longer
    /// holds its registration connected (it stays open, to be closed
    /// and answered), and the registrations whose lease has ended are
    /// dropped, with whatever `serve`s of theirs were never closed.
    fn advance(&mut self, now: Millis) {
        if now > self.now {
            self.now = now;
        }
        let now = self.now;
        let kept = self
            .limits
            .connected_ms
            .saturating_add(self.limits.lease_ms);
        let mut ended = Vec::new();
        self.registrations.retain(|id, reg| {
            let lapsed = reg.holds.values().filter(|until| **until < now).max();
            if let Some(until) = lapsed {
                reg.alive = reg.alive.max(*until);
            }
            let held =
                reg.holds.values().any(|until| *until >= now) || now.since(reg.alive) <= kept;
            if !held {
                ended.push(id.clone());
            }
            held
        });
        for id in ended {
            self.unmarked.insert(id, now);
        }
    }

    /// [`Hub::advance`], and then marks in `store` the registrations
    /// whose lease ended as dropped ([`Pending::set_dropped`]).
    fn advance_with<P: Pending>(&mut self, store: &mut P, now: Millis) -> Result<(), P::Error> {
        self.advance(now);
        while let Some((id, at)) = self.unmarked.pop_first() {
            if let Err(e) = store.set_dropped(&id, Some(at)) {
                self.unmarked.insert(id, at);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Whether the hub dropped the registration `id` at `now` or before
    /// (its lease ended, or it made room for another), and its host has
    /// not gone on as it since: kept in the store, so a hub that started
    /// again since says so too. Its calls then wait for a host that
    /// nothing says will come back, where a registration the hub does
    /// not hold and did not drop (the hub started again since, and its
    /// host has not registered again yet) may resume.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn dropped<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        now: Millis,
    ) -> Result<bool, String> {
        self.advance_with(store, now).map_err(|e| e.to_string())?;
        if self.registrations.contains_key(id) {
            return Ok(false);
        }
        store.is_dropped(id).map_err(|e| e.to_string())
    }

    /// Whether the registration is connected at the hub's instant,
    /// which [`Hub::advance`] has brought it to.
    fn is_connected(&self, reg: &Reg) -> bool {
        reg.holds.values().any(|until| *until >= self.now)
            || self.now.since(reg.alive) <= self.limits.connected_ms
    }

    /// Keeps `definition` ready under `hash`, and lets go of the
    /// definitions of no registration beyond
    /// [`HubLimits::definitions`], the longest unused first.
    fn remember(&mut self, hash: &str, definition: Arc<Definition>) {
        let used = self.take_number();
        self.definitions
            .insert(hash.to_string(), (definition, used));
        let in_use: BTreeSet<&str> = self
            .registrations
            .values()
            .map(|r| r.hash.as_str())
            .collect();
        let mut spare: Vec<(u64, String)> = self
            .definitions
            .iter()
            .filter(|(h, _)| !in_use.contains(h.as_str()))
            .map(|(h, (_, used))| (*used, h.clone()))
            .collect();
        spare.sort_unstable();
        let over = spare.len().saturating_sub(self.limits.definitions);
        for (_, hash) in spare.into_iter().take(over) {
            self.definitions.remove(&hash);
        }
    }

    /// The definition kept under `hash`: one the hub has ready, or the
    /// store's, read as the canonical manifest it is.
    fn definition<P: Pending>(
        &mut self,
        store: &mut P,
        hash: &str,
    ) -> Result<Option<Arc<Definition>>, P::Error> {
        if let Some((definition, used)) = self.definitions.get_mut(hash) {
            let definition = definition.clone();
            *used = self.next;
            self.next = self.next.saturating_add(1);
            return Ok(Some(definition));
        }
        let Some(text) = store.definition_text(hash)? else {
            return Ok(None);
        };
        // The text is this hub's own writing of a definition it read
        // within its limits; a manifest says in more bytes what a
        // `.d.ts` file says, so its length is not held to them again.
        let limits = Limits {
            manifest_bytes: usize::MAX,
            manifest_values: usize::MAX,
            ..self.limits.definition.clone()
        };
        let author = Author::Host {
            reserved: &self.reserved,
        };
        let Ok(definition) = parse_manifest(&text, author, &limits) else {
            return Ok(None);
        };
        let definition = Arc::new(definition);
        self.remember(hash, definition.clone());
        Ok(Some(definition))
    }

    /// The definition kept under `hash`, as a call made under it is
    /// checked by: a registration's, or one the store keeps
    /// ([`Pending::keep_definition`]). `None` for a hash nothing was
    /// registered with.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn kept_definition<P: Pending>(
        &mut self,
        store: &mut P,
        hash: &str,
    ) -> Result<Option<Arc<Definition>>, String> {
        self.definition(store, hash).map_err(|e| e.to_string())
    }

    /// The earliest instant after `now` at which time alone changes
    /// something, or the instant of something that is due and not yet
    /// done: a call's deadline ([`Pending::next_deadline`], which may
    /// be at or before `now` when [`Hub::expire`] has not settled it
    /// yet), a registration ceasing to be connected, a lease ending, a
    /// call waiting to be sent again coming due ([`Pending::next_resend`]:
    /// when whoever holds a `serve` of its registration asks
    /// [`Hub::ready`] again). `None`
    /// when nothing waits on time. Whoever keeps the hub calls
    /// [`Hub::expire`] then, and need not before.
    ///
    /// What only a `serve` can act on is never answered once its
    /// instant is past: a call its host asked for later that has come
    /// due waits for a `serve`, not for the clock. So an instant at or
    /// before `now` is answered only for a deadline, which the next
    /// [`Hub::expire`] settles.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn next_wake<P: Pending>(
        &mut self,
        store: &mut P,
        now: Millis,
    ) -> Result<Option<Millis>, String> {
        self.advance_with(store, now).map_err(|e| e.to_string())?;
        let now = self.now;
        let (connected, lease) = (self.limits.connected_ms, self.limits.lease_ms);
        let deadline = store.next_deadline().map_err(|e| e.to_string())?;
        let again = store.next_resend(now).map_err(|e| e.to_string())?;
        Ok(self
            .registrations
            .values()
            .flat_map(|reg| {
                // When it was, or will be, last served: the end of its
                // longest open wait, or the last it was known alive.
                let served = reg
                    .holds
                    .values()
                    .copied()
                    .filter(|until| *until >= now)
                    .chain([reg.alive])
                    .max()
                    .unwrap_or(reg.alive);
                let unconnected = served.after(connected).after(1);
                let dropped = served.after(connected.saturating_add(lease)).after(1);
                [unconnected, dropped]
            })
            .chain(again)
            .filter(|at| *at > now)
            .chain(deadline)
            .min())
    }

    /// The registration that makes room for one more: the one among
    /// `ids` that is not connected and was made longest ago.
    fn oldest_unconnected<'a>(&self, ids: impl Iterator<Item = &'a String>) -> Option<String> {
        ids.filter_map(|id| Some((id, self.registrations.get(id)?)))
            .filter(|(_, reg)| !self.is_connected(reg))
            .min_by_key(|(_, reg)| reg.made)
            .map(|(id, _)| id.clone())
    }

    /// Registers the surface `name` as `request` gives it, at `now`:
    /// reads the definition as a host's, within
    /// [`HubLimits::definition`], names it by the SHA-256 of its
    /// canonical bytes, and keeps those bytes in the store. The
    /// registration is connected from `now` as if a `serve` of it had
    /// just closed.
    ///
    /// A request that names no registration makes a new one, with the
    /// ID `fresh`. The caller draws `fresh` for each request: 128 bits
    /// from a source of randomness fit for secrets, as 32 lowercase
    /// hexadecimal digits ([`is_registration_id`]). Whoever holds the
    /// daemon's token and knows an ID can go on as that registration,
    /// so an ID must not be one that can be guessed. One more than a
    /// limit allows drops the oldest registration that is not
    /// connected (of this name, for
    /// [`HubLimits::registrations_per_name`]; of any, for
    /// [`HubLimits::registrations`]), and is refused when there is
    /// none.
    ///
    /// A request that names a registration
    /// ([`RegisterRequest::registration`]) goes on as it, when the ID
    /// was issued for this surface ([`Pending::registration_name`]):
    /// when the hub holds it, its definition, prompt and settings are
    /// replaced and its open `serve`s stand; when it holds none, one is
    /// made with that ID. Either way the calls and ended runs the
    /// store holds for that ID are this registration's.
    ///
    /// # Errors
    /// [`RegisterError`]: nothing was registered, replaced or dropped.
    pub fn register<P: Pending>(
        &mut self,
        store: &mut P,
        name: &str,
        request: &RegisterRequest,
        fresh: &str,
        now: Millis,
    ) -> Result<Registered, RegisterError> {
        let reading = read_registration(&self.limits, &self.reserved, name, request)?;
        self.register_parsed(store, reading, fresh, now)
    }

    /// [`Hub::register`] for a request that was read already
    /// ([`read_registration`], with this hub's limits and reserved
    /// names): the part that needs the hub and the store, which reads
    /// no definition and no prompt file.
    ///
    /// A registration that is held and gone on with, with its
    /// definition, prompt file or settings changed, may be so
    /// [`HubLimits::changes`] times in each [`HubLimits::changes_ms`].
    ///
    /// # Errors
    /// [`RegisterError`]: nothing was registered, replaced or dropped.
    pub fn register_parsed<P: Pending>(
        &mut self,
        store: &mut P,
        reading: Reading,
        fresh: &str,
        now: Millis,
    ) -> Result<Registered, RegisterError> {
        let storage = |e: P::Error| RegisterError::Storage(e.to_string());
        self.advance_with(store, now).map_err(storage)?;
        let now = self.now;
        let name = reading.name.as_str();
        if !is_registration_id(fresh) || self.registrations.contains_key(fresh) {
            return Err(RegisterError::FreshInvalid);
        }
        let (id, held) = match reading.registration.as_deref() {
            None => (fresh, false),
            Some(id) => {
                let issued_for = match self.registrations.get(id) {
                    Some(reg) => Some(reg.name.clone()),
                    None if is_registration_id(id) => {
                        store.registration_name(id).map_err(storage)?
                    }
                    None => None,
                };
                if issued_for.as_deref() != Some(name) {
                    return Err(RegisterError::UnknownRegistration);
                }
                (id, self.registrations.contains_key(id))
            }
        };
        // Whether it changes a registration that is held, and how
        // often that has happened of late.
        let window = self.limits.changes_ms;
        let mut changes: Vec<Millis> = Vec::new();
        if let Some(reg) = self.registrations.get(id) {
            changes = reg
                .changes
                .iter()
                .copied()
                .filter(|at| now.since(*at) < window)
                .collect();
            let changed = reg.hash != reading.hash
                || reg.prompt_hash != reading.prompt_hash
                || reg.settings != reading.settings;
            if changed {
                if changes.len() >= self.limits.changes {
                    return Err(RegisterError::TooManyChanges {
                        limit: self.limits.changes,
                        window_ms: window,
                    });
                }
                changes.push(now);
            }
        }
        // What must go to make room, when the registration is one more.
        let mut evicted: Vec<String> = Vec::new();
        if !held {
            let of_name: Vec<&String> = self
                .registrations
                .iter()
                .filter(|(_, r)| r.name == name)
                .map(|(id, _)| id)
                .collect();
            if of_name.len() >= self.limits.registrations_per_name {
                let limit = self.limits.registrations_per_name;
                evicted.push(self.oldest_unconnected(of_name.into_iter()).ok_or(
                    RegisterError::TooManyRegistrations {
                        limit,
                        of_name: true,
                    },
                )?);
            }
            if self.registrations.len() - evicted.len() >= self.limits.registrations {
                let limit = self.limits.registrations;
                evicted.push(
                    self.oldest_unconnected(
                        self.registrations.keys().filter(|id| !evicted.contains(id)),
                    )
                    .ok_or(RegisterError::TooManyRegistrations {
                        limit,
                        of_name: false,
                    })?,
                );
            }
        }
        let hash = reading.hash.clone();
        store
            .keep_definition(&hash, &reading.canonical, now)
            .map_err(storage)?;
        // The prompt file is kept by its hash as the definition is: a
        // run started with it is sent it, whatever is registered later.
        if let (Some(prompt_hash), Some(text)) = (&reading.prompt_hash, &reading.prompt) {
            store.keep_prompt(prompt_hash, text, now).map_err(storage)?;
        }
        if reading.registration.is_some() {
            store.change_registration(id, &hash, now).map_err(storage)?;
        } else if !store
            .issue_registration(id, name, &hash, now)
            .map_err(storage)?
        {
            return Err(RegisterError::FreshInvalid);
        }
        for gone in &evicted {
            store.set_dropped(gone, Some(now)).map_err(storage)?;
        }
        if reading.registration.is_some() {
            store.set_dropped(id, None).map_err(storage)?;
        }
        for gone in evicted {
            self.registrations.remove(&gone);
        }
        let made = self.take_number();
        let old = self.registrations.remove(id);
        self.registrations.insert(
            id.to_string(),
            Reg {
                name: name.to_string(),
                definition: reading.definition.clone(),
                hash: hash.clone(),
                prompt: reading.prompt,
                prompt_hash: reading.prompt_hash,
                settings: reading.settings,
                withheld: reading.withheld.clone(),
                made,
                holds: old.as_ref().map(|r| r.holds.clone()).unwrap_or_default(),
                alive: now,
                // Going on as a registration is a host that started
                // again, and may not have the calls it accepted.
                lost: reading.registration.is_some(),
                ends_sent: old
                    .as_ref()
                    .map(|r| r.ends_sent.clone())
                    .unwrap_or_default(),
                changes,
                events: old.map(|r| r.events).unwrap_or_default(),
            },
        );
        self.remember(&hash, reading.definition);
        Ok(Registered {
            registration: id.to_string(),
            hash,
            clax_surface: wire::VERSION,
            withheld: reading.withheld,
        })
    }

    /// Whether the hub holds, at `now`, the registration `registration`
    /// of the surface `name`: what a request that names one is checked
    /// by.
    ///
    /// # Errors
    /// [`ServeError::UnknownRegistration`].
    pub fn find(&mut self, name: &str, registration: &str, now: Millis) -> Result<(), ServeError> {
        self.advance(now);
        match self.registrations.get(registration) {
            Some(reg) if reg.name == name => Ok(()),
            _ => Err(ServeError::UnknownRegistration),
        }
    }

    /// The registration with this ID, when the hub holds it.
    #[must_use]
    pub fn registration(&self, id: &str) -> Option<Registration<'_>> {
        let reg = self.registrations.get(id)?;
        Some(Registration {
            name: &reg.name,
            definition: &reg.definition,
            hash: &reg.hash,
            prompt: reg.prompt.as_deref(),
            settings: &reg.settings,
            withheld: &reg.withheld,
        })
    }

    /// Whether the registration is connected at `now` (the module's
    /// documentation, "Registrations"). False for one the hub does not
    /// hold.
    pub fn connected(&mut self, id: &str, now: Millis) -> bool {
        self.advance(now);
        self.registrations
            .get(id)
            .is_some_and(|reg| self.is_connected(reg))
    }

    /// Brings everything to `now`: settles the calls whose deadline
    /// has come, each with [`unanswered`], and answers their IDs (their
    /// runs have an outcome to read); forgets the ended runs recorded
    /// more than [`HubLimits::ended_ms`] ago; lapses `serve`s and ends
    /// leases.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn expire<P: Pending>(
        &mut self,
        store: &mut P,
        now: Millis,
    ) -> Result<Vec<String>, String> {
        self.advance_with(store, now).map_err(|e| e.to_string())?;
        let now = self.now;
        let settled = store
            .expire_calls(now, &settled_at_deadline)
            .map_err(|e| e.to_string())?;
        self.settles = self.settles.saturating_add(settled.len() as u64);
        if let Some(before) = now.0.checked_sub(self.limits.ended_ms) {
            store
                .expire_ends(Millis(before))
                .map_err(|e| e.to_string())?;
        }
        Ok(settled)
    }

    /// Opens a `serve` of the registration `registration` of the
    /// surface `name` at `now`: settles the calls whose deadline has
    /// come, takes `answers` in their order, and holds the
    /// registration connected for `wait_ms`
    /// ([`HubLimits::serve_wait_ms`] at the most), or until
    /// [`Hub::close`]. One more open `serve` than
    /// [`HubLimits::serves_per_registration`] ends the oldest
    /// ([`Ready::Ended`]).
    ///
    /// Of each answer that names an open call of this registration:
    ///
    /// - a result settles it as its value when it is the operation's
    ///   declared result in the definition the call was made under, and
    ///   as `invalid_result` when it is not, or is longer than
    ///   [`HubLimits::result_bytes`], or that definition no longer reads;
    /// - an error settles it as that error when its code is one the
    ///   operation declares (`@throws`) or `unavailable`, and as
    ///   `invalid_result`, with the code and the message in its
    ///   message, for any other; an error that may be tried again sends
    ///   the call again instead, as its rule allows (the module's
    ///   documentation, "Sending a call again");
    /// - `accepted` marks it as one its host has, which is not sent
    ///   again unless its host may have lost it;
    /// - `retry_after_ms` leaves it out of what is sent until that long
    ///   from `now`, and sends a call its host had accepted again then;
    ///   an accepted call of an `@once` operation, which is never sent
    ///   again, it settles `unavailable`.
    ///
    /// The last two settle nothing else, so a result or an error for the
    /// same call may follow either in the same request. An answer that
    /// names an ended run of this registration acknowledges it. Every
    /// other answer is ignored ([`Opened::ignored`]): for an ID that is
    /// neither, and any answer for a call an earlier answer of this
    /// request settled; but a result or an error for a call that was
    /// settled before with that same answer is taken: it stands.
    ///
    /// A registration that was not connected, or that its host went on
    /// as (it started again), may have lost the calls it had accepted:
    /// once the answers are taken, those it neither settled nor accepted
    /// again are planned to be sent again, each as its rule allows.
    ///
    /// # Errors
    /// [`ServeError`]. No `serve` is open. For
    /// [`ServeError::Storage`] some answers may have been taken.
    pub fn open<P: Pending>(
        &mut self,
        store: &mut P,
        name: &str,
        registration: &str,
        wait_ms: u64,
        answers: Vec<Answer>,
        now: Millis,
    ) -> Result<Opened, ServeError> {
        let answers = answers.into_iter().map(Checked::from).collect();
        self.open_checked(store, name, registration, wait_ms, answers, now)
    }

    /// The answers of a `serve` whose result or error is to be judged
    /// before the `serve` is opened: those that name an open call of
    /// the registration, each with the definition its call was made
    /// under. It reads the registration's calls without their
    /// arguments and settles nothing. Whoever keeps the hub behind a
    /// lock asks this under it, judges with the lock let go
    /// ([`check_answers`]), and opens the `serve` with what that gave
    /// ([`Hub::open_checked`]), so that no host's results are checked
    /// while every other waits.
    ///
    /// # Errors
    /// [`ServeError`], as [`Hub::open`] would refuse the request.
    pub fn checkable<P: Pending>(
        &mut self,
        store: &mut P,
        name: &str,
        registration: &str,
        answers: &[Answer],
        now: Millis,
    ) -> Result<Vec<Checking>, ServeError> {
        self.find(name, registration, now)?;
        if answers.len() > self.limits.answers_per_serve {
            return Err(ServeError::TooManyAnswers {
                limit: self.limits.answers_per_serve,
            });
        }
        let settles = |a: &Answer| matches!(a.outcome, Outcome::Result(_) | Outcome::Error(_));
        if !answers.iter().any(settles) {
            return Ok(Vec::new());
        }
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        let calls = store.pending_for(registration).map_err(storage)?.calls;
        let mut out = Vec::new();
        for (n, answer) in answers.iter().enumerate() {
            if !settles(answer) || answer.id.len() > self.limits.id_bytes {
                continue;
            }
            let Some(call) = calls.iter().find(|c| c.id == answer.id) else {
                continue;
            };
            // One whose definition does not read is settled when the
            // serve is opened.
            if let Some(definition) = self.definition(store, &call.definition).map_err(storage)? {
                out.push(Checking {
                    answer: n,
                    definition,
                    op: call.op.clone(),
                });
            }
        }
        Ok(out)
    }

    /// [`Hub::open`] with answers that may have been judged already
    /// ([`check_answers`]). A judged answer settles its call, when the
    /// call is still open, as it was judged; one that was not judged
    /// is judged here.
    ///
    /// # Errors
    /// As [`Hub::open`].
    pub fn open_checked<P: Pending>(
        &mut self,
        store: &mut P,
        name: &str,
        registration: &str,
        wait_ms: u64,
        answers: Vec<Checked>,
        now: Millis,
    ) -> Result<Opened, ServeError> {
        self.find(name, registration, now)?;
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        self.advance_with(store, now).map_err(storage)?;
        let now = self.now;
        if answers.len() > self.limits.answers_per_serve {
            return Err(ServeError::TooManyAnswers {
                limit: self.limits.answers_per_serve,
            });
        }
        // A host that comes back after it was not connected, or that
        // started again, may have lost the calls it had accepted: told
        // before this serve holds it connected, acted on once its
        // answers are taken.
        let lost = self
            .registrations
            .get(registration)
            .is_some_and(|reg| reg.lost || !self.is_connected(reg));
        store.served(registration, now).map_err(storage)?;
        let mut settled = store
            .expire_calls(now, &settled_at_deadline)
            .map_err(storage)?;
        self.settles = self.settles.saturating_add(settled.len() as u64);
        let mut ignored = Vec::new();
        let mut accepted_here: BTreeSet<String> = BTreeSet::new();
        if !answers.is_empty() {
            // The registration's open calls as each answer finds them:
            // what an earlier answer of this request did to a call (it
            // settled it, its host accepted it, or its attempts moved)
            // is what a later one for the same call is taken by.
            let mut calls = store.pending_for(registration).map_err(storage)?.calls;
            let mut settled_here: BTreeSet<String> = BTreeSet::new();
            for answer in answers {
                if answer.id.len() > self.limits.id_bytes {
                    ignored.push(answer.id);
                    continue;
                }
                let took = match calls.iter_mut().find(|c| c.id == answer.id) {
                    Some(call) if settled_here.contains(&call.id) => false,
                    Some(call) => {
                        let accepts =
                            matches!(answer.outcome, CheckedOutcome::Raw(Outcome::Accepted));
                        match self.answer(store, registration, call, answer.outcome, now)? {
                            Took::Settled => {
                                settled_here.insert(call.id.clone());
                                settled.push(call.id.clone());
                                self.settles = self.settles.saturating_add(1);
                                true
                            }
                            Took::Noted => {
                                if accepts {
                                    accepted_here.insert(call.id.clone());
                                }
                                true
                            }
                            Took::Same => {
                                settled_here.insert(call.id.clone());
                                true
                            }
                            Took::Ignored => false,
                        }
                    }
                    None => self.settled_before(store, registration, &answer, now)?,
                };
                if !took {
                    ignored.push(answer.id);
                }
            }
        }
        if lost {
            // The mark is cleared once the plan is kept: a serve whose
            // plan could not be written leaves it, and the host's next
            // serve plans again (a call planned already is not planned
            // twice).
            self.reoffer(store, registration, now, &accepted_here)?;
            if let Some(reg) = self.registrations.get_mut(registration) {
                reg.lost = false;
            }
        }
        let hold = self.take_number();
        let until = now.after(wait_ms.min(self.limits.serve_wait_ms));
        let most = self.limits.serves_per_registration.max(1);
        if let Some(reg) = self.registrations.get_mut(registration) {
            while reg.holds.len() >= most {
                reg.holds.pop_first();
            }
            reg.holds.insert(hold, until);
            reg.alive = reg.alive.max(now);
        }
        Ok(Opened {
            serve: Serve {
                registration: registration.to_string(),
                hold,
                until,
            },
            ignored: ignored.len(),
            ignored_ids: ignored,
            settled,
        })
    }

    /// Takes an answer that names no open call of registration `id`: it
    /// acknowledges an ended run of it, or, for a result or an error,
    /// is taken when it is the answer the call was settled with before
    /// (its host sends it again, its reply to the first lost). Whether it
    /// was taken.
    fn settled_before<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        answer: &Checked,
        now: Millis,
    ) -> Result<bool, ServeError> {
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        if store
            .acknowledge_end(id, &answer.id, now)
            .map_err(storage)?
        {
            return Ok(true);
        }
        // The call is not open, so the store settles nothing with this:
        // it only tells whether the call was settled with it. A result or
        // an error no definition judged here is compared as its host
        // wrote it (a result that held content was settled with the
        // content filled in, and so is not this).
        let outcome = match &answer.outcome {
            CheckedOutcome::Judged(outcome) => outcome.clone(),
            CheckedOutcome::Raw(Outcome::Result(value)) => Ok(value.clone()),
            CheckedOutcome::Raw(Outcome::Error(error)) => {
                let failure =
                    CallFailure::new(&error.code, cut(&error.message, self.limits.message_bytes));
                Err(if error.retryable || error.code == CODE_UNAVAILABLE {
                    failure.retryable()
                } else {
                    failure
                })
            }
            CheckedOutcome::Raw(Outcome::Accepted | Outcome::RetryAfter(_)) => return Ok(false),
        };
        Ok(store
            .answer_call(id, &answer.id, outcome, now)
            .map_err(storage)?
            == Answered::AlreadySettled)
    }

    /// Takes one answer for the open call `call` of registration `id`,
    /// and leaves `call` as the store has it after.
    fn answer<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        call: &mut CallSummary,
        outcome: CheckedOutcome,
        now: Millis,
    ) -> Result<Took, ServeError> {
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        let outcome = match outcome {
            CheckedOutcome::Raw(Outcome::Accepted) => {
                // Its host has it again: it is not sent again, and the
                // failure before is no longer what is known of it.
                let took = store.accept_call(id, &call.id, now).map_err(storage)?;
                if took {
                    call.accepted = true;
                    call.sent = true;
                    call.tries = Tries {
                        resend: Resend::None,
                        later: false,
                        last: None,
                        ..call.tries.clone()
                    };
                }
                return Ok(if took { Took::Noted } else { Took::Ignored });
            }
            // The host's own account: no attempt and none of the run's
            // sendings again, only a wait no shorter than the rule's
            // first, so that a host that asks for it again at once is
            // not sent it again at once. An accepted call is sent again
            // then, but one of an `@once` operation, which is never sent
            // again once accepted: that is settled, as its host did not
            // take it.
            CheckedOutcome::Raw(Outcome::RetryAfter(ms)) => {
                if !self.registrations.contains_key(id) {
                    return Ok(Took::Ignored);
                }
                if call.accepted && self.once(store, call)? {
                    return self.settle(store, id, call, Err(not_taken(call)), now);
                } else {
                    let rule = call.tries.rule.unwrap_or(self.limits.retry_kept);
                    let asked = now.after(ms.max(rule.wait_after(1)));
                    let mut tries = call.tries.clone();
                    tries.not_before = if tries.resend == Resend::Planned {
                        tries.not_before.max(asked)
                    } else {
                        asked
                    };
                    tries.later = true;
                    if call.accepted && tries.resend == Resend::None {
                        tries.resend = Resend::Due;
                    }
                    tries.last = Some(not_taken(call));
                    return self.keep(store, id, call, tries);
                }
            }
            CheckedOutcome::Judged(outcome) => outcome,
            CheckedOutcome::Raw(settling) => match self.definition_of(store, call)? {
                None => Err(CallFailure::new(
                    CODE_INVALID_RESULT,
                    format!(
                        "clax.{}: the definition it was called under no longer reads, so its answer cannot be checked",
                        shown(&call.op)
                    ),
                )),
                Some(definition) => match judge(
                    &self.limits,
                    &definition,
                    &call.op,
                    &call.id,
                    settling,
                    None,
                ) {
                    Some(outcome) => outcome,
                    None => return Ok(Took::Ignored),
                },
            },
        };
        if let Err(failure) = &outcome
            && failure.retryable
            && self.registrations.contains_key(id)
        {
            // A failure of an attempt whose next one is planned and not
            // sent yet takes no attempt more; it is what the call is
            // settled as if its deadline comes first, unless a failure
            // is known of it already.
            if call.tries.resend == Resend::Planned {
                if call.tries.last.is_some() {
                    return Ok(Took::Noted);
                }
                let tries = Tries {
                    last: Some(failure.clone()),
                    ..call.tries.clone()
                };
                return self.keep(store, id, call, tries);
            }
            if let Some(tries) = self.try_again(store, id, call, failure, now)? {
                return self.keep(store, id, call, tries);
            }
        }
        self.settle(store, id, call, outcome, now)
    }

    /// Settles the open call `call` of registration `id` with `outcome`.
    fn settle<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        call: &CallSummary,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<Took, ServeError> {
        Ok(
            match store
                .answer_call(id, &call.id, outcome, now)
                .map_err(|e| ServeError::Storage(e.to_string()))?
            {
                Answered::Settled => Took::Settled,
                Answered::AlreadySettled => Took::Same,
                Answered::Ignored => Took::Ignored,
            },
        )
    }

    /// Keeps `tries` with the open call `call` of registration `id`, and
    /// in `call`: [`Took::Noted`], or [`Took::Ignored`] when the store
    /// holds no such open call.
    fn keep<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        call: &mut CallSummary,
        tries: Tries,
    ) -> Result<Took, ServeError> {
        let kept = store
            .keep_tries(id, &call.id, &tries)
            .map_err(|e| ServeError::Storage(e.to_string()))?;
        if !kept {
            return Ok(Took::Ignored);
        }
        call.tries = tries;
        Ok(Took::Noted)
    }

    /// The definition the call was made under; `None` when it is not
    /// kept or no longer reads (a build whose definitions read otherwise
    /// than the one that kept it): a property of the call, which is then
    /// settled `invalid_result` when it is answered and never sent again.
    fn definition_of<P: Pending>(
        &mut self,
        store: &mut P,
        call: &CallSummary,
    ) -> Result<Option<Arc<Definition>>, ServeError> {
        self.definition(store, &call.definition)
            .map_err(|e| ServeError::Storage(e.to_string()))
    }

    /// Whether the call is never to be sent again once accepted: its
    /// operation is tagged `@once`, or its definition no longer reads
    /// ([`Hub::definition_of`]), so nothing says it may be.
    fn once<P: Pending>(&mut self, store: &mut P, call: &CallSummary) -> Result<bool, ServeError> {
        Ok(self.definition_of(store, call)?.is_none_or(|definition| {
            definition
                .operation(&call.op)
                .is_none_or(|operation| operation.op.once)
        }))
    }

    /// Whether `run` has one of its sendings again of `registration` left
    /// ([`HubLimits::resends_per_run`]), as the store counts them. Each
    /// registration's are its own, so that one host's failures do not
    /// spend another's in a run both serve.
    fn resend_left<P: Pending>(
        &self,
        store: &mut P,
        run: &str,
        registration: &str,
    ) -> Result<bool, ServeError> {
        let spent = store
            .resends(run, registration)
            .map_err(|e| ServeError::Storage(e.to_string()))?;
        Ok(spent < self.limits.resends_per_run)
    }

    /// Where the open call `call` of registration `id`, whose attempt
    /// came to `failure` (one that sending it again may get past), goes
    /// to when it is to be sent again: its rule has another attempt, its
    /// run has a sending left, its operation is not `@once` with the
    /// call accepted, and the wait before the next attempt ends before
    /// the call's deadline. Its next attempt is then planned, and
    /// `failure` is what it is settled as if its deadline comes first.
    /// `None` when it is not.
    fn try_again<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        call: &CallSummary,
        failure: &CallFailure,
        now: Millis,
    ) -> Result<Option<Tries>, ServeError> {
        if call.accepted && self.once(store, call)? {
            return Ok(None);
        }
        let rule = call.tries.rule.unwrap_or(self.limits.retry_kept);
        let failures = call.tries.failures.saturating_add(1);
        let at = now.after(rule.wait_after(failures));
        if !rule.another(failures)
            || at >= call.deadline
            || !self.resend_left(store, &call.run, id)?
        {
            return Ok(None);
        }
        Ok(Some(Tries {
            failures,
            // A wait its host asked for stands too.
            not_before: if call.tries.later {
                call.tries.not_before.max(at)
            } else {
                at
            },
            resend: Resend::Planned,
            last: Some(failure.clone()),
            ..call.tries.clone()
        }))
    }

    /// Plans the calls of registration `id` its host accepted and may
    /// have lost (it was not connected, or started again) to be sent
    /// again, each as its rule allows, but those `skip` names (this
    /// `serve` accepted them again), those already to be sent again, and
    /// those never to be sent again once accepted ([`Hub::once`]).
    fn reoffer<P: Pending>(
        &mut self,
        store: &mut P,
        id: &str,
        now: Millis,
        skip: &BTreeSet<String>,
    ) -> Result<(), ServeError> {
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        let pending = store.pending_for(id).map_err(storage)?;
        let accepted = pending.calls.into_iter().filter(|c| {
            c.accepted
                && c.deadline > now
                && c.tries.resend == Resend::None
                && !skip.contains(&c.id)
        });
        for call in accepted {
            if self.once(store, &call)? {
                continue;
            }
            let rule = call.tries.rule.unwrap_or(self.limits.retry_kept);
            let failures = call.tries.failures.saturating_add(1);
            if !rule.another(failures) {
                continue;
            }
            let at = now.after(rule.wait_after(failures));
            if at >= call.deadline || !self.resend_left(store, &call.run, id)? {
                continue;
            }
            let tries = Tries {
                failures,
                not_before: at,
                resend: Resend::Planned,
                ..call.tries.clone()
            };
            store.keep_tries(id, &call.id, &tries).map_err(storage)?;
        }
        Ok(())
    }

    /// What a `serve` of the registration would be answered with at
    /// `now`, in the store's order: the ID and size of each open call
    /// that is not accepted (or is to be sent again), not waiting to be
    /// sent again and not past its deadline, and of each unacknowledged
    /// end, each with whether it is new to its host.
    fn due(&mut self, id: &str, pending: &PendingSummary, now: Millis) -> Vec<Due> {
        let Some(reg) = self.registrations.get_mut(id) else {
            return Vec::new();
        };
        reg.ends_sent
            .retain(|end| pending.ends.iter().any(|e| e.id == *end));
        let mut out = Vec::new();
        for call in &pending.calls {
            let tries = &call.tries;
            if (call.accepted && tries.resend == Resend::None)
                || call.deadline <= now
                || tries.not_before > now
            {
                continue;
            }
            out.push(Due {
                // A call its host asked for again, or that is sent
                // again after a failed attempt, is new to it once more.
                new: !call.sent || tries.later || tries.resend == Resend::Planned,
                call: true,
                id: call.id.clone(),
                bytes: call.args_bytes,
            });
        }
        for end in &pending.ends {
            out.push(Due {
                new: !reg.ends_sent.contains(&end.id),
                call: false,
                id: end.id.clone(),
                bytes: end.result_bytes,
            });
        }
        out
    }

    /// What an open `serve` is to do at `now` ([`Ready`]). It reads no
    /// call's arguments.
    ///
    /// # Errors
    /// [`ServeError::Storage`].
    pub fn ready<P: Pending>(
        &mut self,
        store: &mut P,
        serve: &Serve,
        now: Millis,
    ) -> Result<Ready, ServeError> {
        self.advance(now);
        let now = self.now;
        let open = self
            .registrations
            .get(&serve.registration)
            .is_some_and(|reg| reg.holds.contains_key(&serve.hold));
        if !open {
            return Ok(Ready::Ended);
        }
        let pending = store
            .pending_for(&serve.registration)
            .map_err(|e| ServeError::Storage(e.to_string()))?;
        let due = self.due(&serve.registration, &pending, now);
        let events = self
            .registrations
            .get(&serve.registration)
            .is_some_and(|reg| !reg.events.is_empty());
        Ok(
            if now >= serve.until || events || due.iter().any(|d| d.new) {
                Ready::Deliver
            } else {
                Ready::Wait
            },
        )
    }

    /// Closes a `serve` at `now` and answers what its request is
    /// answered with: the registration's open calls (but those its
    /// host accepted, and those it asked for later) and its
    /// unacknowledged ended runs, those new to the host first and each
    /// group oldest first, [`HubLimits::deliveries_per_serve`] at the
    /// most and no more than [`HubLimits::deliveries_bytes`] together
    /// (but one at the least); then every event kept for it
    /// ([`Hub::event`]), oldest first, which those limits do not count
    /// and which go once. Each call is marked as sent: the
    /// caller is about to write them to its host, and calls
    /// [`Hub::abandon`] instead when it will not. A `serve` is answered
    /// so whether or not its wait is over; one that has ended
    /// ([`Ready::Ended`]) is answered with nothing.
    ///
    /// # Errors
    /// [`ServeError::Storage`]: the `serve` is closed, and nothing was
    /// sent.
    pub fn close<P: Pending>(
        &mut self,
        store: &mut P,
        serve: Serve,
        now: Millis,
    ) -> Result<Vec<Delivery>, ServeError> {
        self.advance(now);
        let now = self.now;
        let Some(reg) = self.registrations.get_mut(&serve.registration) else {
            return Ok(Vec::new());
        };
        if reg.holds.remove(&serve.hold).is_none() {
            return Ok(Vec::new());
        }
        reg.alive = reg.alive.max(now);
        let prefix = format!("{}.", reg.name);
        let storage = |e: P::Error| ServeError::Storage(e.to_string());
        let pending = store.pending_for(&serve.registration).map_err(storage)?;
        let mut due = self.due(&serve.registration, &pending, now);
        // Stable: within the new and within the rest, the oldest first.
        due.sort_by_key(|d| !d.new);
        due.truncate(self.limits.deliveries_per_serve);
        let mut bytes = 0usize;
        let mut sent = 0;
        for d in &due {
            bytes = bytes.saturating_add(d.bytes);
            if sent > 0 && bytes > self.limits.deliveries_bytes {
                break;
            }
            sent += 1;
        }
        due.truncate(sent);
        let ids = |calls: bool| -> Vec<String> {
            due.iter()
                .filter(|d| d.call == calls)
                .map(|d| d.id.clone())
                .collect()
        };
        let call_ids = ids(true);
        let mut calls: BTreeMap<String, PendingCall> = store
            .calls_for(&serve.registration, &call_ids)
            .map_err(storage)?
            .into_iter()
            .map(|c| (c.summary.id.clone(), c))
            .collect();
        let mut ends: BTreeMap<String, PendingEnd> = store
            .ends_for(&serve.registration, &ids(false))
            .map_err(storage)?
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        // A run's events come before its calls and its end: each was
        // told before what follows it, and none of a run comes after
        // its `ended`.
        let mut deliveries = Vec::with_capacity(due.len());
        if let Some(reg) = self.registrations.get_mut(&serve.registration) {
            deliveries.extend(reg.events.drain(..).map(Delivery::Event));
        }
        for d in &due {
            if d.call {
                // One that was settled since it was listed is not sent.
                let Some(call) = calls.remove(&d.id) else {
                    continue;
                };
                deliveries.push(Delivery::Call(wire::Call {
                    op: call
                        .summary
                        .op
                        .strip_prefix(&prefix)
                        .unwrap_or(&call.summary.op)
                        .to_string(),
                    expires_in_ms: Some(call.summary.deadline.since(now)),
                    id: call.summary.id,
                    call: call.summary.kind,
                    run: call.run,
                    args: call.args,
                    key: call.key,
                    joined: call.joined,
                }));
            } else if let Some(end) = ends.remove(&d.id) {
                deliveries.push(Delivery::Ended(wire::Ended {
                    id: end.id,
                    run: end.run,
                    end: end.end,
                    result: end.result,
                    undelivered: end.undelivered,
                }));
            }
        }
        // Sent: a planned attempt is made, and its last failure is no
        // longer what is known of it ([`Pending::sent_calls`]).
        store.sent_calls(&call_ids).map_err(storage)?;
        if let Some(reg) = self.registrations.get_mut(&serve.registration) {
            for delivery in &deliveries {
                if let Delivery::Ended(e) = delivery {
                    reg.ends_sent.insert(e.id.clone());
                }
            }
        }
        Ok(deliveries)
    }

    /// Closes a `serve` whose request went away before it could be
    /// answered, at `now`: its registration was served until now, and
    /// nothing is marked as sent, so a call no host was sent is still
    /// one no host was sent when its deadline comes (`unavailable`,
    /// and not `expired` or `unknown_outcome`).
    pub fn abandon(&mut self, serve: Serve, now: Millis) {
        self.advance(now);
        let now = self.now;
        if let Some(reg) = self.registrations.get_mut(&serve.registration)
            && reg.holds.remove(&serve.hold).is_some()
        {
            reg.alive = reg.alive.max(now);
        }
    }

    /// A `serve` that waits for nothing: [`Hub::open`] and then
    /// [`Hub::close`], at `now`. A caller that holds a request open
    /// calls the two itself, with [`Hub::ready`] and its wake hook
    /// between them.
    ///
    /// # Errors
    /// As [`Hub::open`] and [`Hub::close`].
    pub fn serve<P: Pending>(
        &mut self,
        store: &mut P,
        name: &str,
        registration: &str,
        answers: Vec<Answer>,
        now: Millis,
    ) -> Result<Vec<Delivery>, ServeError> {
        let opened = self.open(store, name, registration, 0, answers, now)?;
        self.close(store, opened.serve, now)
    }

    /// Opens a call for the registration to serve, at `now`, under the
    /// definition the registration has now, and wakes the
    /// registration. What the call is to its surface (`read`, `write`
    /// or `effect`) is read from that definition, so it cannot differ
    /// from what the host declared.
    ///
    /// A call whose key the store holds already, for this
    /// registration, run, operation and arguments, is the call it
    /// holds: its ID is answered and nothing else changes.
    ///
    /// # Errors
    /// [`EnqueueError`]: nothing of the call is kept, and no host was
    /// sent it. [`EnqueueError::failure`] is what its script is
    /// thrown.
    pub fn enqueue<P: Pending>(
        &mut self,
        store: &mut P,
        registration: &str,
        call: Enqueue,
        now: Millis,
    ) -> Result<String, EnqueueError> {
        self.enqueue_as(store, registration, None, call, now)
    }

    /// [`Hub::enqueue`], under the definition kept under `definition`
    /// in place of the one the registration has now: for a run that
    /// was bound to the registration when it had that definition, so
    /// that its calls are read and their answers checked by the
    /// definition the run was made with, whatever its host has
    /// registered since.
    ///
    /// # Errors
    /// As [`Hub::enqueue`]; [`EnqueueError::Storage`] also for a hash
    /// no definition is kept under.
    pub fn enqueue_under<P: Pending>(
        &mut self,
        store: &mut P,
        registration: &str,
        definition: &str,
        call: Enqueue,
        now: Millis,
    ) -> Result<String, EnqueueError> {
        self.enqueue_as(store, registration, Some(definition), call, now)
    }

    fn enqueue_as<P: Pending>(
        &mut self,
        store: &mut P,
        registration: &str,
        under: Option<&str>,
        call: Enqueue,
        now: Millis,
    ) -> Result<String, EnqueueError> {
        self.advance_with(store, now)
            .map_err(|e| EnqueueError::Storage(e.to_string()))?;
        let now = self.now;
        let current = &self
            .registrations
            .get(registration)
            .ok_or(EnqueueError::NoRegistration)?
            .hash;
        // An earlier definition of the registration's, with what it
        // withholds.
        let earlier = match under {
            Some(hash) if hash != current => {
                let definition = self
                    .definition(store, hash)
                    .map_err(|e| EnqueueError::Storage(e.to_string()))?
                    .ok_or_else(|| {
                        EnqueueError::Storage(format!(
                            "the definition clax.{} is called under is not kept",
                            shown(&call.op)
                        ))
                    })?;
                let withheld = withheld(&definition, self.limits.content);
                Some((definition, hash.to_string(), withheld))
            }
            _ => None,
        };
        let reg = self
            .registrations
            .get(registration)
            .ok_or(EnqueueError::NoRegistration)?;
        let (definition, hash, held_back) = match &earlier {
            Some((definition, hash, withheld)) => (&**definition, hash, &withheld[..]),
            None => (&*reg.definition, &reg.hash, &reg.withheld[..]),
        };
        let Some(operation) = definition.operation(&call.op) else {
            return Err(EnqueueError::UnknownOp(call.op));
        };
        let Some(kind) = CallKind::of(operation.op.access) else {
            return Err(EnqueueError::NotAHostCall(call.op));
        };
        let below = call
            .op
            .strip_prefix(reg.name.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(&call.op);
        if held_back.iter().any(|w| w.op == below) {
            return Err(EnqueueError::Withheld(call.op));
        }
        let ids = self.limits.id_bytes;
        if call.key.is_empty()
            || call.key.len() > ids
            || call.run.is_empty()
            || call.run.len() > ids
        {
            return Err(EnqueueError::IdTooLong { limit: ids });
        }
        // A call that may be tried again waits for a host that is away,
        // within its deadline; one that may not is refused at once.
        if !self.is_connected(reg) && !call.retry.waits_for_host() {
            return Err(EnqueueError::NotConnected);
        }
        let deadline = call.deadline.unwrap_or(now.after(self.limits.call_ms));
        if deadline <= now {
            return Err(EnqueueError::PastDeadline);
        }
        let args_bytes = json_bytes(&call.args);
        if args_bytes > self.limits.args_bytes {
            return Err(EnqueueError::ArgsTooLarge {
                limit: self.limits.args_bytes,
            });
        }
        let rule = call.retry;
        let new = NewCall {
            registration: registration.to_string(),
            definition: hash.clone(),
            run: call.run,
            op: call.op,
            args: call.args,
            args_bytes,
            key: call.key,
            kind,
            deadline,
            joined: call.joined,
            retry: rule,
        };
        let limit = self.limits.calls_per_registration;
        let storage = |e: P::Error| EnqueueError::Storage(e.to_string());
        match store.open_call(new, limit, now).map_err(storage)? {
            OpenedCall::New(id) => {
                (self.wake)(registration);
                Ok(id)
            }
            // Made again (its step run again): the attempts go on where
            // they were, under the run's rule when none was kept.
            OpenedCall::Same(id) => {
                let kept = store
                    .pending_for(registration)
                    .map_err(storage)?
                    .calls
                    .into_iter()
                    .find(|c| c.id == id);
                if let Some(call) = kept
                    && call.tries.rule.is_none()
                {
                    let tries = Tries {
                        rule: Some(rule),
                        ..call.tries
                    };
                    store
                        .keep_tries(registration, &id, &tries)
                        .map_err(storage)?;
                }
                Ok(id)
            }
            OpenedCall::KeyReused => Err(EnqueueError::KeyReused),
            OpenedCall::Full => Err(EnqueueError::QueueFull { limit }),
        }
    }

    /// Calls the wake hook for the registration, when the hub holds it:
    /// for whoever put something in the store for its host by another
    /// way than the hub (a run's end kept with the run's terminal
    /// commit).
    pub fn notify(&self, registration: &str) {
        if self.registrations.contains_key(registration) {
            (self.wake)(registration);
        }
    }

    /// Keeps `event`, of how the run `run` the registration started is
    /// going, for the registration's next `serve`, and wakes it.
    /// Events are not kept anywhere else: one for a registration the
    /// hub does not hold is dropped (false), and so is the oldest of a
    /// registration that holds [`HubLimits::events_per_registration`]
    /// already. A host that was away reads where the run is from the
    /// run itself (`GET …/runs/<id>`). Whoever calls this decides what
    /// the registration may see of the run; the hub reads none of
    /// `event`.
    pub fn event(&mut self, registration: &str, run: &str, event: Value, now: Millis) -> bool {
        self.advance(now);
        let most = self.limits.events_per_registration;
        let Some(reg) = self.registrations.get_mut(registration) else {
            return false;
        };
        if most == 0 {
            return false;
        }
        while reg.events.len() >= most {
            reg.events.pop_front();
        }
        reg.events.push_back(wire::Event {
            run: run.to_string(),
            event,
        });
        (self.wake)(registration);
        true
    }

    /// Records that `run` ended with `end`, for the registration that
    /// started it, at `now`, and wakes the registration when the hub
    /// holds it. `result` is the kind's result, for a run that
    /// finished. The end is delivered with every `serve` of the
    /// registration until an answer names it, for
    /// [`HubLimits::ended_ms`]. A registration the hub does not hold
    /// is no error, when its ID was issued: the end waits for its host
    /// to register again.
    ///
    /// # Errors
    /// [`EndedError`]: nothing was recorded.
    pub fn ended<P: Pending>(
        &mut self,
        store: &mut P,
        registration: &str,
        run: &str,
        end: End,
        result: Option<Value>,
        now: Millis,
    ) -> Result<String, EndedError> {
        self.advance(now);
        let storage = |e: P::Error| EndedError::Storage(e.to_string());
        let ids = self.limits.id_bytes;
        if run.is_empty() || run.len() > ids {
            return Err(EndedError::IdTooLong { limit: ids });
        }
        let issued = self.registrations.contains_key(registration)
            || (is_registration_id(registration)
                && store
                    .registration_name(registration)
                    .map_err(storage)?
                    .is_some());
        if !issued {
            return Err(EndedError::NoRegistration);
        }
        let result_bytes = result.as_ref().map_or(0, json_bytes);
        if result_bytes > self.limits.result_bytes {
            return Err(EndedError::ResultTooLarge {
                limit: self.limits.result_bytes,
            });
        }
        let limit = self.limits.ended_per_registration;
        let new = NewEnd {
            registration: registration.to_string(),
            run: run.to_string(),
            end,
            result,
            result_bytes,
        };
        match store.record_end(new, limit, self.now).map_err(storage)? {
            RecordedEnd::New(id) => {
                self.settles = self.settles.saturating_add(1);
                if self.registrations.contains_key(registration) {
                    (self.wake)(registration);
                }
                Ok(id)
            }
            RecordedEnd::Same(id) => Ok(id),
            RecordedEnd::Full => Err(EndedError::QueueFull { limit }),
        }
    }
}

/// One thing a `serve` would be answered with.
struct Due {
    new: bool,
    /// A call, or an end.
    call: bool,
    id: String,
    bytes: usize,
}

/// What became of one answer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Took {
    Ignored,
    /// It changed something, and settled nothing.
    Noted,
    Settled,
    /// The call was settled before with this same answer: it stands.
    Same,
}

// ---------------------------------------------------------------------------
// In memory
// ---------------------------------------------------------------------------

struct MemoryCall {
    registration: String,
    call: PendingCall,
    /// The arguments it was opened with, kept when it is settled.
    opened_with: Vec<Value>,
    outcome: Option<Result<Value, CallFailure>>,
}

struct MemoryEnd {
    registration: String,
    end: PendingEnd,
    result_bytes: usize,
    recorded: Millis,
}

/// A [`Pending`] that keeps everything in memory, for tests. A settled
/// call stays until [`MemoryPending::take_answer`] takes it, as a
/// store's stays until its run has read it.
#[derive(Default)]
pub struct MemoryPending {
    next: u64,
    registrations: BTreeMap<String, String>,
    /// The registrations marked as dropped ([`Pending::set_dropped`]).
    dropped: BTreeSet<String>,
    /// The failures of every call it was given, by run and registration,
    /// those [`MemoryPending::take_answer`] took among them: a run's
    /// sendings again ([`Pending::resends`]).
    failures: BTreeMap<(String, String), u32>,
    definitions: BTreeMap<String, String>,
    prompts: BTreeMap<String, String>,
    calls: BTreeMap<u64, MemoryCall>,
    by_key: BTreeMap<String, u64>,
    ends: BTreeMap<u64, MemoryEnd>,
}

impl fmt::Debug for MemoryPending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryPending")
            .field("held", &self.held())
            .finish_non_exhaustive()
    }
}

impl MemoryPending {
    /// With nothing in it.
    #[must_use]
    pub fn new() -> MemoryPending {
        MemoryPending::default()
    }

    fn call_number(id: &str) -> Option<u64> {
        id.strip_prefix('c')?.parse().ok()
    }

    fn end_number(id: &str) -> Option<u64> {
        id.strip_prefix('e')?.parse().ok()
    }

    fn open_of(&mut self, registration: &str, id: &str) -> Option<&mut MemoryCall> {
        Self::call_number(id)
            .and_then(|n| self.calls.get_mut(&n))
            .filter(|c| c.outcome.is_none() && c.registration == registration)
    }

    /// Whether it holds the call, settled or not.
    #[must_use]
    pub fn holds(&self, id: &str) -> bool {
        Self::call_number(id).is_some_and(|n| self.calls.contains_key(&n))
    }

    /// Whether it holds the call and the call is not settled.
    #[must_use]
    pub fn is_open(&self, id: &str) -> bool {
        Self::call_number(id)
            .and_then(|n| self.calls.get(&n))
            .is_some_and(|c| c.outcome.is_none())
    }

    /// A settled call's outcome, once: the call is then forgotten, and
    /// its key. `None` for a call that is open, or not held.
    pub fn take_answer(&mut self, id: &str) -> Option<Result<Value, CallFailure>> {
        let n = Self::call_number(id)?;
        self.calls.get(&n)?.outcome.as_ref()?;
        let call = self.calls.remove(&n)?;
        self.by_key.remove(&call.call.key);
        call.outcome
    }

    /// A settled call's outcome, left where it is: `None` for a call
    /// that is open, or not held.
    #[must_use]
    pub fn outcome_of(&self, id: &str) -> Option<&Result<Value, CallFailure>> {
        self.calls.get(&Self::call_number(id)?)?.outcome.as_ref()
    }

    /// How many calls it holds, settled ones among them, and how many
    /// unacknowledged ends.
    #[must_use]
    pub fn held(&self) -> (usize, usize) {
        (self.calls.len(), self.ends.len())
    }

    /// Where the open call `id` is with its attempts: `None` for a call
    /// that is settled, or not held.
    #[must_use]
    pub fn tries_of(&self, id: &str) -> Option<&Tries> {
        Self::call_number(id)
            .and_then(|n| self.calls.get(&n))
            .filter(|c| c.outcome.is_none())
            .map(|c| &c.call.summary.tries)
    }
}

impl Pending for MemoryPending {
    type Error = std::convert::Infallible;

    fn issue_registration(
        &mut self,
        id: &str,
        name: &str,
        _hash: &str,
        _now: Millis,
    ) -> Result<bool, Self::Error> {
        if self.registrations.contains_key(id) {
            return Ok(false);
        }
        self.registrations.insert(id.to_string(), name.to_string());
        Ok(true)
    }

    fn registration_name(&mut self, id: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.registrations.get(id).cloned())
    }

    fn keep_definition(
        &mut self,
        hash: &str,
        canonical: &str,
        _now: Millis,
    ) -> Result<(), Self::Error> {
        self.definitions
            .entry(hash.to_string())
            .or_insert_with(|| canonical.to_string());
        Ok(())
    }

    fn definition_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.definitions.get(hash).cloned())
    }

    fn keep_prompt(&mut self, hash: &str, text: &str, _now: Millis) -> Result<(), Self::Error> {
        self.prompts
            .entry(hash.to_string())
            .or_insert_with(|| text.to_string());
        Ok(())
    }

    fn prompt_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.prompts.get(hash).cloned())
    }

    fn open_call(
        &mut self,
        call: NewCall,
        limit: usize,
        _now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        if let Some(&n) = self.by_key.get(&call.key) {
            let same = self.calls.get(&n).is_some_and(|held| {
                held.registration == call.registration
                    && held.call.run == call.run
                    && held.call.summary.op == call.op
                    && held.opened_with == call.args
            });
            return Ok(if same {
                OpenedCall::Same(format!("c{n}"))
            } else {
                OpenedCall::KeyReused
            });
        }
        let open = self
            .calls
            .values()
            .filter(|c| c.outcome.is_none() && c.registration == call.registration)
            .count();
        if open >= limit {
            return Ok(OpenedCall::Full);
        }
        self.next += 1;
        let n = self.next;
        self.by_key.insert(call.key.clone(), n);
        self.calls.insert(
            n,
            MemoryCall {
                registration: call.registration,
                opened_with: call.args.clone(),
                call: PendingCall {
                    summary: CallSummary {
                        id: format!("c{n}"),
                        run: call.run.clone(),
                        definition: call.definition,
                        op: call.op,
                        kind: call.kind,
                        deadline: call.deadline,
                        args_bytes: call.args_bytes,
                        sent: false,
                        accepted: false,
                        tries: Tries {
                            rule: Some(call.retry),
                            ..Tries::default()
                        },
                    },
                    run: call.run,
                    args: call.args,
                    key: call.key,
                    joined: call.joined,
                },
                outcome: None,
            },
        );
        Ok(OpenedCall::New(format!("c{n}")))
    }

    fn pending_for(&mut self, registration: &str) -> Result<PendingSummary, Self::Error> {
        Ok(PendingSummary {
            calls: self
                .calls
                .values()
                .filter(|c| c.outcome.is_none() && c.registration == registration)
                .map(|c| c.call.summary.clone())
                .collect(),
            ends: self
                .ends
                .values()
                .filter(|e| e.registration == registration)
                .map(|e| EndSummary {
                    id: e.end.id.clone(),
                    result_bytes: e.result_bytes,
                })
                .collect(),
        })
    }

    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error> {
        Ok(self
            .calls
            .values()
            .filter(|c| {
                c.outcome.is_none()
                    && c.registration == registration
                    && ids.contains(&c.call.summary.id)
            })
            .map(|c| c.call.clone())
            .collect())
    }

    fn sent_calls(&mut self, ids: &[String]) -> Result<(), Self::Error> {
        for id in ids {
            if let Some(call) = Self::call_number(id)
                .and_then(|n| self.calls.get_mut(&n))
                .filter(|c| c.outcome.is_none())
            {
                let summary = &mut call.call.summary;
                summary.sent = true;
                if summary.tries.resend == Resend::Planned {
                    summary.tries.resend = Resend::Due;
                }
                summary.tries.later = false;
                summary.tries.last = None;
            }
        }
        Ok(())
    }

    fn accept_call(
        &mut self,
        registration: &str,
        id: &str,
        _now: Millis,
    ) -> Result<bool, Self::Error> {
        Ok(self
            .open_of(registration, id)
            .map(|call| {
                let summary = &mut call.call.summary;
                summary.sent = true;
                summary.accepted = true;
                summary.tries.resend = Resend::None;
                summary.tries.later = false;
                summary.tries.last = None;
            })
            .is_some())
    }

    fn answer_call(
        &mut self,
        registration: &str,
        id: &str,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<Answered, Self::Error> {
        let settled_before = Self::call_number(id)
            .and_then(|n| self.calls.get(&n))
            .filter(|c| c.registration == registration)
            .and_then(|c| c.outcome.as_ref());
        if let Some(before) = settled_before {
            return Ok(if *before == outcome {
                Answered::AlreadySettled
            } else {
                Answered::Ignored
            });
        }
        Ok(self
            .open_of(registration, id)
            .filter(|call| call.call.summary.deadline > now)
            .map_or(Answered::Ignored, |call| {
                call.call.args = Vec::new();
                call.outcome = Some(outcome);
                Answered::Settled
            }))
    }

    fn keep_tries(
        &mut self,
        registration: &str,
        id: &str,
        tries: &Tries,
    ) -> Result<bool, Self::Error> {
        let Some(call) = self.open_of(registration, id) else {
            return Ok(false);
        };
        let (before, run) = (call.call.summary.tries.failures, call.call.run.clone());
        call.call.summary.tries = tries.clone();
        let count = self
            .failures
            .entry((run, registration.to_string()))
            .or_insert(0);
        *count = count.saturating_sub(before).saturating_add(tries.failures);
        Ok(true)
    }

    fn resends(&mut self, run: &str, registration: &str) -> Result<u32, Self::Error> {
        Ok(self
            .failures
            .get(&(run.to_string(), registration.to_string()))
            .copied()
            .unwrap_or(0))
    }

    fn next_resend(&mut self, after: Millis) -> Result<Option<Millis>, Self::Error> {
        Ok(self
            .calls
            .values()
            .filter(|c| c.outcome.is_none())
            .map(|c| c.call.summary.tries.not_before)
            .filter(|at| *at > after)
            .min())
    }

    fn set_dropped(&mut self, id: &str, at: Option<Millis>) -> Result<(), Self::Error> {
        if at.is_some() {
            self.dropped.insert(id.to_string());
        } else {
            self.dropped.remove(id);
        }
        Ok(())
    }

    fn is_dropped(&mut self, id: &str) -> Result<bool, Self::Error> {
        Ok(self.dropped.contains(id))
    }

    fn expire_calls(
        &mut self,
        now: Millis,
        failure: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error> {
        let mut expired = Vec::new();
        for call in self.calls.values_mut() {
            if call.outcome.is_none() && call.call.summary.deadline <= now {
                call.outcome = Some(Err(failure(&call.call.summary)));
                call.call.args = Vec::new();
                expired.push(call.call.summary.id.clone());
            }
        }
        Ok(expired)
    }

    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
        Ok(self
            .calls
            .values()
            .filter(|c| c.outcome.is_none())
            .map(|c| c.call.summary.deadline)
            .min())
    }

    fn record_end(
        &mut self,
        end: NewEnd,
        limit: usize,
        now: Millis,
    ) -> Result<RecordedEnd, Self::Error> {
        let mut of_registration = self
            .ends
            .values()
            .filter(|e| e.registration == end.registration);
        if let Some(held) = of_registration.clone().find(|e| e.end.run == end.run) {
            return Ok(RecordedEnd::Same(held.end.id.clone()));
        }
        if of_registration.by_ref().count() >= limit {
            return Ok(RecordedEnd::Full);
        }
        self.next += 1;
        let n = self.next;
        self.ends.insert(
            n,
            MemoryEnd {
                registration: end.registration,
                end: PendingEnd {
                    id: format!("e{n}"),
                    run: end.run,
                    end: end.end,
                    result: end.result,
                    undelivered: Vec::new(),
                },
                result_bytes: end.result_bytes,
                recorded: now,
            },
        );
        Ok(RecordedEnd::New(format!("e{n}")))
    }

    fn ends_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingEnd>, Self::Error> {
        Ok(self
            .ends
            .values()
            .filter(|e| e.registration == registration && ids.contains(&e.end.id))
            .map(|e| e.end.clone())
            .collect())
    }

    fn acknowledge_end(
        &mut self,
        registration: &str,
        id: &str,
        _now: Millis,
    ) -> Result<bool, Self::Error> {
        let Some(n) = Self::end_number(id) else {
            return Ok(false);
        };
        if self
            .ends
            .get(&n)
            .is_some_and(|e| e.registration == registration)
        {
            self.ends.remove(&n);
            return Ok(true);
        }
        Ok(false)
    }

    fn expire_ends(&mut self, before: Millis) -> Result<usize, Self::Error> {
        let held = self.ends.len();
        self.ends.retain(|_, e| e.recorded >= before);
        Ok(held - self.ends.len())
    }
}
