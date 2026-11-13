//! The messages of the host protocol
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "A
//! host's surface on a daemon"; `docs/contract.md`, "ClaxBot surfaces"):
//! what a host that registers a surface at run time sends a daemon and
//! is sent back, as plain data. Nothing here reads a socket or a clock:
//! a host written in Rust, the daemon's routes and the run loop's
//! provider for an outside surface all build and read these types, and
//! `docs/claxbot-surfaces/contract.json` holds examples of every one,
//! which this crate's tests read.
//!
//! | Message | Sent by | With |
//! |---|---|---|
//! | [`RegisterRequest`] | The host | `PUT /api/bot/surfaces/<name>` |
//! | [`Registered`] | The daemon | Its answer |
//! | [`ServeRequest`] | The host | `POST /api/bot/surfaces/<name>/serve` |
//! | [`ServeResponse`] | The daemon | Its answer |
//! | [`StartRunRequest`] | The host | `POST /api/bot/surfaces/<name>/runs` |
//! | [`StartRunResponse`] | The daemon | Its answer |
//! | [`MessageRequest`] | The host | `POST /api/bot/surfaces/<name>/runs/<id>/messages` |
//! | [`MessageResponse`] | The daemon | Its answer |
//! | [`RunState`] | The daemon | `GET /api/bot/surfaces/<name>/runs/<id>?registration=<id>` |
//! | [`ErrorResponse`] | The daemon | Any refusal |
//!
//! # Members a reader does not know
//!
//! - **What a host sends is read strictly.** A member the message does
//!   not have is refused, never ignored: a host that misspells
//!   `wait_ms` or sends a setting this daemon does not have is told so
//!   and is not served as if it had said nothing. Every type the host
//!   sends says `deny_unknown_fields`.
//! - **What a daemon sends is read leniently.** A member a host does
//!   not know is ignored, a delivery whose `call` it does not know is
//!   kept as [`Delivery::Other`] and skipped, and an end it does not
//!   know is kept as [`End::Other`] and read as the run having ended,
//!   so a daemon may say more in a later version of Clax and a host
//!   built today still reads it.
//!
//! # The version
//!
//! The protocol has a version, [`VERSION`], named as a manifest names
//! its notation's: `clax_surface`. A host states it once, when it
//! registers ([`RegisterRequest::clax_surface`]), and the daemon
//! answers with the one it speaks ([`Registered::clax_surface`]);
//! another version is refused ([`CODE_UNSUPPORTED_VERSION`]). `serve`
//! and `runs` carry none: they belong to a registration, which has one.
//! A version changes only when a message changes meaning; a new
//! optional member of a host's message, a new member of a daemon's, a
//! new kind of delivery and a new way for a run to end do not change
//! it.

use crate::model::Access;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The version of the protocol these types are: what
/// [`RegisterRequest::clax_surface`] holds.
pub const VERSION: u64 = 1;

/// No registration of the surface's name is the one the request names,
/// or the name has none: the host registers again.
pub const CODE_UNKNOWN_REGISTRATION: &str = "unknown_registration";
/// The request's body is not the message its route takes.
pub const CODE_INVALID_REQUEST: &str = "invalid_request";
/// The definition is not one: [`ErrorBody::errors`] says why.
pub const CODE_INVALID_DEFINITION: &str = "invalid_definition";
/// The registration is of another version of the protocol.
pub const CODE_UNSUPPORTED_VERSION: &str = "unsupported_version";
/// The definition's surface is not named as the route names it.
pub const CODE_NAME_MISMATCH: &str = "name_mismatch";
/// A setting is not one the daemon takes.
pub const CODE_INVALID_SETTINGS: &str = "invalid_settings";
/// The prompt file is longer than the daemon takes.
pub const CODE_PROMPT_TOO_LARGE: &str = "prompt_too_large";
/// The prompt file is not one for the surface: the message says why
/// (the format is `crates/clax-bot/src/prompt/file.rs`'s).
pub const CODE_INVALID_PROMPT: &str = "invalid_prompt";
/// The daemon holds as many registrations as it takes.
pub const CODE_TOO_MANY_REGISTRATIONS: &str = "too_many_registrations";
/// A `serve` brought more answers than the daemon takes in one.
pub const CODE_TOO_MANY_ANSWERS: &str = "too_many_answers";
/// The request asks for something of the protocol that this daemon
/// does not do yet: a start that names surfaces to bind
/// ([`StartRunRequest::bind`]). Nothing was done.
pub const CODE_NOT_YET: &str = "not_yet";
/// The surface has no run of this ID that the registration started:
/// none is sent a message, and none is shown, for another
/// registration.
pub const CODE_UNKNOWN_RUN: &str = "unknown_run";
/// The run's kind takes no messages after it starts: its kind has one
/// parameter, or is one of Clax's own.
pub const CODE_TAKES_NO_MESSAGES: &str = "takes_no_messages";
/// The run has ended, and takes no message.
pub const CODE_RUN_ENDED: &str = "run_ended";
/// The run holds a message under this key with another body.
pub const CODE_KEY_CONFLICT: &str = "key_conflict";
/// The run holds as many messages it has not been given yet as it may:
/// a later one is taken once it has taken some.
pub const CODE_TOO_MANY_MESSAGES: &str = "too_many_messages";

/// The request's body is longer than the daemon takes.
pub const CODE_TOO_LARGE: &str = "too_large";
/// The daemon could not do what was asked, through no fault of the
/// request's.
pub const CODE_INTERNAL_ERROR: &str = "internal_error";
/// The daemon could not read or write its store. Some of a `serve`'s
/// answers may have been taken: the host sends them again.
pub const CODE_STORAGE_FAILED: &str = "storage_failed";

/// The HTTP status a daemon answers a refusal with, by its code: 400
/// for a request that is wrong as it stands, 404 for a registration
/// the daemon does not hold or a run it did not start, 409 for a run
/// that has ended and a key given to another message, 413 for a body
/// that is too long, 429 for a limit that waiting lifts, 501 for
/// [`CODE_NOT_YET`], and 500 for
/// [`CODE_INTERNAL_ERROR`], [`CODE_STORAGE_FAILED`] and any code that
/// is not one of this module's.
#[must_use]
pub fn status(code: &str) -> u16 {
    match code {
        CODE_INVALID_REQUEST
        | CODE_INVALID_DEFINITION
        | CODE_UNSUPPORTED_VERSION
        | CODE_NAME_MISMATCH
        | CODE_INVALID_SETTINGS
        | CODE_PROMPT_TOO_LARGE
        | CODE_INVALID_PROMPT
        | CODE_TOO_MANY_ANSWERS
        | CODE_TAKES_NO_MESSAGES => 400,
        CODE_UNKNOWN_REGISTRATION | CODE_UNKNOWN_RUN => 404,
        CODE_RUN_ENDED | CODE_KEY_CONFLICT => 409,
        CODE_TOO_LARGE => 413,
        CODE_TOO_MANY_REGISTRATIONS | CODE_TOO_MANY_MESSAGES => 429,
        CODE_NOT_YET => 501,
        _ => 500,
    }
}

/// What a script is thrown when no host took its call: nothing
/// happened.
pub const CODE_UNAVAILABLE: &str = "unavailable";
/// What a script is thrown when a read was sent to its host and not
/// answered in time.
pub const CODE_EXPIRED: &str = "expired";
/// What a script is thrown when a write or an effect was sent to its
/// host and not answered in time: it may have taken effect.
pub const CODE_UNKNOWN_OUTCOME: &str = "unknown_outcome";
/// What a script is thrown when its host answered a call with a value
/// that is not the operation's result.
pub const CODE_INVALID_RESULT: &str = "invalid_result";

/// The codes only Clax's core throws a script, whatever surface its call
/// was for: no host may declare one in an operation's `@throws` (a
/// host's definition that does is refused) or answer a call with one
/// (its answer is settled as [`CODE_INVALID_RESULT`]). They are the
/// daemon's refusals of a call before its host is sent it (`unknown_op`,
/// `invalid_args`, `key_reused`, `effect_limit`, `call_limit`), its words
/// for a call its host did not answer ([`CODE_EXPIRED`],
/// [`CODE_UNKNOWN_OUTCOME`]) or answered wrongly
/// ([`CODE_INVALID_RESULT`], `provider_error`), what a script is thrown
/// for a step it could not go on with (`replay_diverged`,
/// `run_not_claimed`, `step_mismatch`, `script_finished`, `cancelled`,
/// `waiting`, `park_limit`, `parked_runs_limit`), and
/// [`CODE_INTERNAL_ERROR`]. A run's count of its effects reads several of
/// them as "nothing was done", which a host could otherwise claim of a
/// call it performed. [`CODE_UNAVAILABLE`] is not one: a host may say
/// with it that nothing happened.
pub const CORE_CODES: [&str; 18] = [
    "call_limit",
    "cancelled",
    "effect_limit",
    CODE_EXPIRED,
    CODE_INTERNAL_ERROR,
    "invalid_args",
    CODE_INVALID_RESULT,
    "key_reused",
    "park_limit",
    "parked_runs_limit",
    "provider_error",
    "replay_diverged",
    "run_not_claimed",
    "script_finished",
    "step_mismatch",
    "unknown_op",
    CODE_UNKNOWN_OUTCOME,
    "waiting",
];

/// Which notation a definition's text is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notation {
    /// A `.d.ts` file ([`crate::parse`]).
    Dts,
    /// A manifest ([`crate::parse_manifest`]).
    Manifest,
}

/// Registers a surface: the body of `PUT /api/bot/surfaces/<name>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    /// The protocol's version: [`VERSION`].
    pub clax_surface: u64,
    /// The notation `definition` is written in.
    pub notation: Notation,
    /// The definition's text. Its surface is named as the route names
    /// it.
    pub definition: String,
    /// The ID an earlier registration of this surface was answered
    /// ([`Registered::registration`]), to go on as that registration:
    /// the calls and ended runs that wait for it are delivered to
    /// this one, whether or not the daemon still holds it (it does not
    /// after a restart, or once the host has been away too long). An
    /// ID this daemon did not give a registration of this surface is
    /// refused ([`CODE_UNKNOWN_REGISTRATION`]). Left out, the
    /// registration is a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<String>,
    /// The surface's prompt file, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The registration's settings: each one left out is the daemon's.
    #[serde(default, skip_serializing_if = "Settings::is_empty")]
    pub settings: Settings,
}

/// A registration's settings, by the names the daemon's configuration
/// gives them (`[bot.surfaces.<name>]`). Each is within the instance's
/// own: a registration narrows and never widens. A setting is left out
/// or given: `null` is no value of any of them, and is refused as a
/// value of the wrong type is (no limit is spelled `"none"`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The operations offered at all, as allow patterns
    /// ([`crate::Pattern`]). Left out: all of them.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub allow: Option<Vec<String>>,
    /// What someone who only reads a run's artifact may see of this
    /// surface in the run. Left out: nothing. A daemon that does not
    /// support it yet refuses a registration that gives it
    /// ([`CODE_INVALID_SETTINGS`]); this one does not.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub reader_view: Option<ReaderView>,
    /// How many effects of this surface one run may have.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_effects_per_run: Option<Bound>,
    /// How long, in seconds, a call may go unanswered.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub call_timeout_s: Option<Bound>,
    /// How long, in seconds, a call of an operation that waits may go
    /// unanswered. The configuration's `wait_limit` is the same figure
    /// written for a person (`"24h"`).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub wait_limit_s: Option<Bound>,
    /// Limits for runs of this surface's kinds.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub runs: Option<RunSettings>,
}

impl Settings {
    /// Whether no setting is given.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Settings::default()
    }
}

/// Limits for runs of a surface's kinds ([`Settings::runs`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSettings {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_steps: Option<Bound>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_run_minutes: Option<Bound>,
}

/// A limit: a whole number, or `"none"` for no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// No limit: `"none"`.
    None,
    /// This many.
    At(u64),
}

impl Serialize for Bound {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Bound::None => s.serialize_str("none"),
            Bound::At(n) => s.serialize_u64(*n),
        }
    }
}

impl<'de> Deserialize<'de> for Bound {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Bound, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) if s == "none" => Ok(Bound::None),
            Value::Number(n) if n.is_u64() => Ok(Bound::At(n.as_u64().unwrap_or(0))),
            _ => Err(D::Error::custom(
                "a limit is a whole number, zero or more, or \"none\"",
            )),
        }
    }
}

/// What a reader of a run's artifact may see of a surface in the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderView {
    /// Nothing of it.
    None,
    /// The names of its operations and their outcomes.
    Names,
}

/// A registration: the daemon's answer to a [`RegisterRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// The registration's ID, which the host's later requests name
    /// ([`ServeRequest::registration`]): 32 lowercase hexadecimal
    /// digits, drawn at random. A host keeps it for as long as it may
    /// have calls waiting, across its own restarts.
    pub registration: String,
    /// The SHA-256 of the definition's canonical bytes
    /// ([`crate::canonical_manifest`]), in lowercase hex: the same for
    /// one definition in either notation.
    pub hash: String,
    /// The protocol's version the daemon speaks.
    pub clax_surface: u64,
    /// The operations of the definition that no run is given yet, each
    /// with why. A script that calls one is refused as for an
    /// operation its run does not have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<Withheld>,
}

/// An operation a registration's runs are not given
/// ([`Registered::withheld`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withheld {
    /// The operation, by its dotted name below the surface
    /// ([`Call::op`]).
    pub op: String,
    /// Why, as a word: [`WITHHELD_CONTENT`]. A later daemon may give
    /// others; a host reads one it does not know as "withheld".
    pub reason: String,
}

/// Why an operation whose parameters or result name `Content` or
/// `Image` is withheld: this daemon carries no content yet.
pub const WITHHELD_CONTENT: &str = "content";

/// A refusal: the body of every answer that is not a success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

/// Why a request was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// One of the codes of this module (`CODE_…`), or another of the
    /// daemon's.
    pub code: String,
    /// The refusal, for a person.
    pub message: String,
    /// For [`CODE_INVALID_DEFINITION`]: the definition's mistakes, in
    /// the text's order, as [`crate::Errors`] holds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<DefinitionError>,
    /// For [`CODE_INVALID_DEFINITION`] of a manifest: how many of its
    /// entries were not looked at ([`crate::Errors::unread`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unread: u64,
    /// For [`CODE_UNSUPPORTED_VERSION`]: the versions of the protocol
    /// the daemon speaks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported: Vec<u64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl ErrorBody {
    /// A refusal with a code and a message alone.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> ErrorBody {
        ErrorBody {
            code: code.into(),
            message: message.into(),
            errors: Vec::new(),
            unread: 0,
            supported: Vec::new(),
        }
    }

    /// The refusal of a registration of version `asked`
    /// ([`CODE_UNSUPPORTED_VERSION`]), with the versions this crate's
    /// types are.
    #[must_use]
    pub fn unsupported_version(asked: u64) -> ErrorBody {
        ErrorBody {
            supported: vec![VERSION],
            ..ErrorBody::new(
                CODE_UNSUPPORTED_VERSION,
                format!(
                    "`clax_surface` is {asked}, and this Clax speaks version {VERSION} of the protocol"
                ),
            )
        }
    }

    /// The refusal of a definition that has `errors`
    /// ([`CODE_INVALID_DEFINITION`]).
    #[must_use]
    pub fn invalid_definition(errors: &crate::Errors) -> ErrorBody {
        ErrorBody {
            code: CODE_INVALID_DEFINITION.to_string(),
            message: errors.to_string(),
            errors: errors.iter().map(DefinitionError::from).collect(),
            unread: errors.unread() as u64,
            supported: Vec::new(),
        }
    }
}

/// One mistake of a definition ([`crate::Error`]): its place, a line of
/// a `.d.ts` file or a JSON Pointer into a manifest, and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionError {
    /// The line of the text, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    /// The value of the manifest, as a JSON Pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    pub message: String,
}

impl From<&crate::Error> for DefinitionError {
    fn from(e: &crate::Error) -> DefinitionError {
        DefinitionError {
            line: e.line().map(|l| l as u64),
            pointer: e.pointer().map(str::to_string),
            message: e.message.clone(),
        }
    }
}

/// One long-poll: the body of `POST /api/bot/surfaces/<name>/serve`. It
/// brings the host's answers and is answered with what the host is to
/// do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeRequest {
    /// The registration that serves ([`Registered::registration`]).
    pub registration: String,
    /// How long, in milliseconds, the daemon may hold the request
    /// while it has nothing new to deliver. The daemon holds it no
    /// longer than it holds any (12 s), whatever is asked.
    pub wait_ms: u64,
    /// Answers to deliveries, taken in their order. Left out: none.
    #[serde(default)]
    pub answers: Vec<Answer>,
}

/// What a host says of one delivery, matched to it by ID.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// The delivery's ID ([`Call::id`], [`Ended::id`]).
    pub id: String,
    pub outcome: Outcome,
}

/// The four things a host says of a delivery.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The call's result: `{"id": …, "result": …}`. For an `ended`
    /// delivery, which has no result, any value: it says the host has
    /// it.
    Result(Value),
    /// The call failed: `{"id": …, "error": {"code": …, "message": …}}`.
    Error(CallError),
    /// The host has the call and is working on it:
    /// `{"id": …, "accepted": true}`. Its result follows in a later
    /// `serve`.
    Accepted,
    /// The host cannot take the call now and asks to be sent it again
    /// no sooner than this many milliseconds from now:
    /// `{"id": …, "retry_after_ms": …}`. Any whole number: 0 is "with
    /// the next `serve`". The call's deadline stands.
    RetryAfter(u64),
}

/// A call's failure, as its host states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallError {
    /// One of the error codes the operation declares (`@throws`), or
    /// [`CODE_UNAVAILABLE`] for "nothing happened".
    pub code: String,
    pub message: String,
    /// Whether the same call may succeed if it is sent again:
    /// `{"code": …, "message": …, "retryable": true}`. `true` lets the
    /// daemon send it again under its key, as the run's policy allows,
    /// before its script is thrown the error. Left out (`false`), the
    /// error is final: the script is thrown it at once. An error of
    /// [`CODE_UNAVAILABLE`] may be tried again whatever this says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retryable: bool,
}

impl CallError {
    /// A final error: one sending the call again would not change.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> CallError {
        CallError {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    /// The same error, saying that the call may succeed if sent again.
    #[must_use]
    pub fn retryable(self) -> CallError {
        CallError {
            retryable: true,
            ..self
        }
    }
}

/// An [`Answer`] as it is written.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRepr {
    id: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    result: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    error: Option<CallError>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    accepted: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    retry_after_ms: Option<u64>,
}

/// A member that is present, whatever its value: `null` is a value
/// where the member's type takes it, and a mistake where it does not.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl Serialize for Answer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut repr = AnswerRepr {
            id: self.id.clone(),
            result: None,
            error: None,
            accepted: None,
            retry_after_ms: None,
        };
        match &self.outcome {
            Outcome::Result(v) => repr.result = Some(v.clone()),
            Outcome::Error(e) => repr.error = Some(e.clone()),
            Outcome::Accepted => repr.accepted = Some(true),
            Outcome::RetryAfter(ms) => repr.retry_after_ms = Some(*ms),
        }
        repr.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Answer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Answer, D::Error> {
        let repr = AnswerRepr::deserialize(d)?;
        let outcome = match (repr.result, repr.error, repr.accepted, repr.retry_after_ms) {
            (Some(v), None, None, None) => Outcome::Result(v),
            (None, Some(e), None, None) => Outcome::Error(e),
            (None, None, Some(true), None) => Outcome::Accepted,
            (None, None, None, Some(ms)) => Outcome::RetryAfter(ms),
            (None, None, Some(false), None) => {
                return Err(D::Error::custom(
                    "`accepted` is `true`: an answer that has nothing to say is left out",
                ));
            }
            _ => {
                return Err(D::Error::custom(
                    "an answer has exactly one of `result`, `error`, `accepted` and `retry_after_ms`",
                ));
            }
        };
        Ok(Answer {
            id: repr.id,
            outcome,
        })
    }
}

/// The daemon's answer to a [`ServeRequest`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServeResponse {
    /// What the host is to do: what is new to it first, and each
    /// group oldest first. A call and an ended run have an `id`, which
    /// the host's answer names; a delivery of a kind a host does not
    /// know may have none, needs no answer, and is not sent again for
    /// want of one. Left out: nothing.
    #[serde(default)]
    pub calls: Vec<Delivery>,
}

/// One thing a `serve` delivers, told apart by its `call` member.
#[derive(Clone, Debug, PartialEq)]
pub enum Delivery {
    /// A call of one of the surface's operations: `call` is `"read"`,
    /// `"write"` or `"effect"`.
    Call(Call),
    /// A run of the registration's has ended: `call` is `"ended"`.
    Ended(Ended),
    /// How a run the registration started is going: `call` is
    /// `"event"`. It has no ID and needs no answer.
    Event(Event),
    /// A delivery of a kind these types do not know, whole. A host
    /// skips it.
    Other(Map<String, Value>),
}

impl Delivery {
    /// The delivery's ID, when it has one.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Delivery::Call(c) => Some(&c.id),
            Delivery::Ended(e) => Some(&e.id),
            Delivery::Event(_) => None,
            Delivery::Other(m) => m.get("id").and_then(Value::as_str),
        }
    }
}

/// What a call is to its surface: the operation's access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Read,
    Write,
    Effect,
}

impl CallKind {
    /// The kind of a call of an operation with `access`. `None` for
    /// [`Access::Spend`], which no host's operation is.
    #[must_use]
    pub fn of(access: Access) -> Option<CallKind> {
        match access {
            Access::Read => Some(CallKind::Read),
            Access::Write => Some(CallKind::Write),
            Access::Effect => Some(CallKind::Effect),
            Access::Spend => None,
        }
    }
}

/// A call of one of the surface's operations, for the host to perform
/// and answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// The delivery's ID. A call that is delivered again has the same.
    /// It is delivered with every `serve` until the host answers it
    /// or says `accepted`.
    pub id: String,
    pub call: CallKind,
    /// The run that makes the call.
    pub run: String,
    /// The operation, by its dotted name below the surface, as a
    /// manifest lists it: `settings.set`, not `shell.settings.set`.
    pub op: String,
    /// One value for each parameter, `null` for an optional one left
    /// out, checked against the definition.
    pub args: Vec<Value>,
    /// The call's key: the same whenever the same call is made again.
    /// A write or an effect made twice with one key takes effect once.
    pub key: String,
    /// How long from now, in milliseconds, an answer is still taken.
    /// This daemon gives every call a deadline; a host that is sent a
    /// call without one waits for as long as it likes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_ms: Option<u64>,
    /// For a call of a run of another surface's kind that a join bound
    /// this surface into (`PUT /api/bot/surfaces/<name>/joins`): what
    /// the run is on, each surface's scope as [`StartRunRequest::bind`]
    /// names it, with the threads the run was started for: `{"clax":
    /// {"artifact": "<ID>", "thread_ids": ["<ID>", …]}}`. So the host
    /// knows which of the artifacts it joined the call concerns. Left
    /// out for a call of a run the registration started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined: Option<Value>,
}

/// A run has ended: one the registration started, or one of another
/// surface's kind that a join bound the surface into. Of a run it was
/// joined into a host is told how it ended and nothing a person wrote:
/// no `result`, and a `gave_up`'s `reason` is empty.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Ended {
    /// The delivery's ID. It is delivered until an answer names it.
    pub id: String,
    pub run: String,
    pub end: End,
    /// The kind's result, for a run that finished.
    #[serde(default, deserialize_with = "present")]
    pub result: Option<Value>,
    /// The `seq` of each message the run took ([`MessageResponse`]) and
    /// never gave its model: its last step was not a turn. Left out
    /// when there is none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undelivered: Vec<u64>,
}

/// How a run ended: an object with one member, which names the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The model called `finish`.
    Finished {},
    /// The model gave up, or the run loop did for it: `reason` is its
    /// words, empty for a run the surface was joined into ([`Ended`]).
    GaveUp { code: String, reason: String },
    /// The run was stopped: `cancelled`, `run_cap`, `credits_exhausted`
    /// and the rest.
    Stopped { reason: String },
    /// The run could not go on.
    Failed { code: String },
    /// An end these types do not know, whole. The run has ended; a
    /// host reads no more of it than that.
    Other(Map<String, Value>),
}

/// The ends these types know, as they are written.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KnownEnd {
    Finished {},
    GaveUp { code: String, reason: String },
    Stopped { reason: String },
    Failed { code: String },
}

impl Serialize for End {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.clone() {
            End::Finished {} => KnownEnd::Finished {},
            End::GaveUp { code, reason } => KnownEnd::GaveUp { code, reason },
            End::Stopped { reason } => KnownEnd::Stopped { reason },
            End::Failed { code } => KnownEnd::Failed { code },
            End::Other(map) => return map.serialize(s),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for End {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<End, D::Error> {
        let map = Map::deserialize(d)?;
        let known = map.len() == 1
            && map
                .keys()
                .all(|k| matches!(k.as_str(), "finished" | "gave_up" | "stopped" | "failed"));
        if !known {
            return Ok(End::Other(map));
        }
        Ok(
            match serde_json::from_value(Value::Object(map)).map_err(D::Error::custom)? {
                KnownEnd::Finished {} => End::Finished {},
                KnownEnd::GaveUp { code, reason } => End::GaveUp { code, reason },
                KnownEnd::Stopped { reason } => End::Stopped { reason },
                KnownEnd::Failed { code } => End::Failed { code },
            },
        )
    }
}

/// An [`Ended`] as it is written: its `call` member with the rest.
#[derive(Serialize)]
struct EndedRepr<'a> {
    id: &'a str,
    call: &'static str,
    run: &'a str,
    end: &'a End,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: &'a Option<Value>,
    #[serde(skip_serializing_if = "<[u64]>::is_empty")]
    undelivered: &'a [u64],
}

/// The `call` of an [`Ended`].
const CALL_ENDED: &str = "ended";

impl Serialize for Ended {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        EndedRepr {
            id: &self.id,
            call: CALL_ENDED,
            run: &self.run,
            end: &self.end,
            result: &self.result,
            undelivered: &self.undelivered,
        }
        .serialize(s)
    }
}

impl Serialize for Delivery {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Delivery::Call(c) => c.serialize(s),
            Delivery::Ended(e) => e.serialize(s),
            Delivery::Event(e) => e.serialize(s),
            Delivery::Other(m) => m.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Delivery {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Delivery, D::Error> {
        let map = Map::deserialize(d)?;
        let Some(call) = map.get("call").and_then(Value::as_str) else {
            return Err(D::Error::custom(
                "a delivery says what it is in `call`, a string",
            ));
        };
        match call {
            "read" | "write" | "effect" => serde_json::from_value(Value::Object(map))
                .map(Delivery::Call)
                .map_err(D::Error::custom),
            CALL_ENDED => serde_json::from_value(Value::Object(map))
                .map(Delivery::Ended)
                .map_err(D::Error::custom),
            CALL_EVENT => serde_json::from_value(Value::Object(map))
                .map(Delivery::Event)
                .map_err(D::Error::custom),
            _ => Ok(Delivery::Other(map)),
        }
    }
}

/// How a run the registration started is going, as a `serve` delivers
/// it: `{"call": "event", "run": …, "event": {"type": …, …}}`. Events
/// are not kept: one a host is away for is not sent later, and
/// [`RunState`] says where the run is now. `event` is read leniently,
/// a `type` a host does not know skipped. This daemon sends:
///
/// | `type` | With | When |
/// |---|---|---|
/// | `step` | `step`, `kind` (`turn`, `script`), and `messages`, the `seq` of each message the step gave the model, when it gave any | A step committed that did not end the run |
/// | `parked` | `step`, `waiting_on`: `{surface}`, with `op` and `deadline` when the surface is the registration's own | The run parked on a call that waits |
/// | `resumed` | `step` | The run's parked step is run again: the call it waited on was answered or its deadline came |
///
/// How the run ended is the `ended` delivery ([`Ended`]), which is
/// kept until the host acknowledges it; no event of a run comes after
/// its `ended`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Event {
    pub run: String,
    pub event: Value,
}

/// The `call` of an [`Event`].
const CALL_EVENT: &str = "event";

#[derive(Serialize)]
struct EventRepr<'a> {
    call: &'static str,
    run: &'a str,
    event: &'a Value,
}

impl Serialize for Event {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        EventRepr {
            call: CALL_EVENT,
            run: &self.run,
            event: &self.event,
        }
        .serialize(s)
    }
}

/// Starts a run of one of the surface's kinds, as the token's person:
/// the body of `POST /api/bot/surfaces/<name>/runs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRunRequest {
    /// The registration that starts the run, and serves it
    /// ([`Registered::registration`]).
    pub registration: String,
    /// The kind, by its name in the definition: `triage`.
    pub kind: String,
    /// The kind's request.
    pub request: Value,
    /// The operations the run may have, as allow patterns. Left out:
    /// all the kind allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// Other surfaces to bind, each with its scope:
    /// `{"clax": {"artifact": "…"}}`. A daemon that does not bind
    /// refuses a start that has it ([`CODE_NOT_YET`]) and
    /// starts no run: a run made without a surface its starter named
    /// would not be the run that was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<Map<String, Value>>,
}

/// The daemon's answer to a [`StartRunRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRunResponse {
    /// The run's ID.
    pub run: String,
}

/// A message to a run the registration started, of the type its kind
/// takes as its second parameter: the body of
/// `POST /api/bot/surfaces/<name>/runs/<id>/messages`. It is given to
/// the run's model between steps, in the order messages arrived; a
/// string in it typed `Instruction` is the person's words, as in a
/// request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRequest {
    /// The registration that started the run.
    pub registration: String,
    pub message: Value,
    /// Names the message once for its run: the same key and message
    /// again is answered as the first was, and nothing is sent twice;
    /// the same key with another message is [`CODE_KEY_CONFLICT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The daemon's answer to a [`MessageRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageResponse {
    /// The message's ID.
    pub message: String,
    /// Its place among the run's messages, from 1, in the order they
    /// arrived.
    pub seq: u64,
    /// The run is parked, waiting on a call (a question to its person):
    /// the message is given to its model at the first turn after the
    /// call is answered or its deadline comes, and does not end the
    /// wait. A message cannot interrupt a run that waits.
    #[serde(default)]
    pub waiting: bool,
}

/// Where a run the registration started is now: the answer to
/// `GET /api/bot/surfaces/<name>/runs/<id>?registration=<id>`, which
/// a host that was away reads in place of the events it missed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run: String,
    /// The kind, by its name in the definition.
    pub kind: String,
    /// `queued`, `running`, `parked`, `stopping`, `finished`,
    /// `gave_up`, `stopped` or `failed`.
    pub status: String,
    /// The steps committed.
    pub steps: u64,
    /// What a parked run waits on, as an event's `waiting_on`; present
    /// exactly while the run waits, and a message sent meanwhile is
    /// given after the wait ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<Value>,
    /// The messages it has not been given yet.
    pub messages_waiting: u64,
    /// For a run that has ended, the `seq` of each message it took and
    /// never gave its model, as its [`Ended`] names them. Left out when
    /// there is none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undelivered: Vec<u64>,
    /// How it ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<End>,
    /// The kind's result, for a run that finished with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}
