//! The contract between ClaxBot and a code-mode sandbox.
//!
//! A [`Sandbox`] runs one [`Script`] under [`Limits`], and the script's only
//! way out is the [`Host`]: the script sees one global, `clax`, whose
//! methods are generated from a list of dotted operation names
//! (`files.read` becomes `clax.files.read`; [`validate_ops`] says which
//! names are allowed). Each method returns a Promise that settles with the
//! result of [`Host::call`] for that operation. Everything the run did
//! comes back as a [`ScriptOutcome`]; a [`CancelToken`] stops it.
//!
//! The observable behaviour every implementation shares (the QuickJS
//! sandbox natively, a Worker Loader isolate on the hosted Worker) is
//! pinned by [`conformance`], whose cases any implementation is run
//! against, and much of it is the shared JavaScript of [`PRELUDE`].
//!
//! # What a script is
//!
//! [`Script::code`] is the body of an async function: it may `await`, and
//! the value it `return`s is the outcome's [`ScriptOutcome::value`],
//! serialised as `JSON.stringify` does (`undefined`, a function or a
//! symbol give no value). The code must parse as such a body on its own
//! ([`check_body`], which every sandbox runs first), so no text can close
//! the function and run beside it; it is then compiled as [`wrap`] writes
//! it, and line `N` of the code is line `N` in every error message (the
//! wrapper's own first line is subtracted). Before any of it runs, code generation from strings is turned
//! off: `eval`, `Function` and the async and generator function
//! constructors throw `EvalError`, as they do in a Worker. There is no
//! `fetch`, no timer and no module loading: code that uses `import()` or
//! `import.meta` fails [`check_body`].
//!
//! Once the async body settles (returns or throws), the script is over:
//! queued jobs no longer run, host calls it made and did not await never
//! reach the host (their records say [`CODE_SCRIPT_FINISHED`]), and any
//! later call rejects with a host error of that code.
//!
//! # Host calls
//!
//! A call `clax.<op>(a, b, …)` sends `op` and the JSON array of its
//! arguments (`[a, b, …]`, serialised by the `JSON.stringify` the prelude
//! captured, so `undefined` becomes `null`) to [`Host::call`]. Arguments
//! whose JSON is not an array, does not parse back (deeper than 128
//! levels, say), or is over [`Limits::max_call_args_bytes`] are refused:
//! the call rejects with a `TypeError`, never reaches the host, and its
//! record says [`CODE_INVALID_ARGS`]. Calls reach the host one at a time,
//! in the order the script made them, and each settles its Promise:
//! resolved with the returned value, or rejected with an `Error` whose
//! `name` is `"HostError"`, whose `code` is [`HostError::code`] and whose
//! `message` is [`HostError::message`]. The script may catch it; a host
//! error it does not catch ends the run with [`ErrorKind::HostError`]
//! (only the sandbox's own host errors count: a value made to look like one
//! is a thrown value like any other).
//!
//! What an operation did to which target (for Toolpath) is the host's to
//! record: the host knows each operation's meaning, the sandbox only its
//! name and arguments.
//!
//! # Logs
//!
//! `console.log(…)` writes one line: its arguments joined by a space, each
//! a string as it is and anything else as `JSON.stringify` writes it (or
//! `String(x)` where that gives nothing, as for `undefined`), with lone
//! surrogates replaced by U+FFFD. Lines are kept whole while their bytes
//! add up to at most [`Limits::max_log_bytes`]; the first line that does
//! not fit is replaced by [`LOGS_TRUNCATED`] and every later line is
//! dropped. Running out of log room is not an error.
//!
//! # Limits
//!
//! [`Limits::validate`] says which limits a sandbox accepts; others end the
//! run at once with [`ErrorKind::InvalidLimits`]. Exceeding the CPU,
//! memory or call limit ends the script at once with that limit's
//! [`ErrorKind`]; the script cannot catch it. The call that goes over
//! [`Limits::max_calls`] never reaches the host. A result over
//! [`Limits::max_result_bytes`] is not returned.
//!
//! # Cancellation and metering
//!
//! Cancelling the run's [`CancelToken`] ends it promptly with
//! [`ErrorKind::Cancelled`] and an outcome that still carries what the run
//! did and used (logs, calls, CPU and wall time), so it can be billed and
//! audited. Dropping the run's future stops the script too, but returns
//! nothing to bill: cancel instead.
//!
//! Billing charges the CPU the engine measured
//! ([`ScriptOutcome::cpu_us`]) and memory at the larger of
//! [`Limits::memory_bytes`] and [`ScriptOutcome::mem_peak_bytes`] for the
//! run. A sandbox that enforces the limit reports a peak at most the limit
//! (a sample, for information), so it charges the limit; one whose isolate
//! has a fixed memory limit of its own, over the configured one (the
//! Worker's 128 MiB), reports that limit as the peak, since the script
//! could hold that much. [`ScriptOutcome::busy_us`] is wall time, for
//! information.
//!
//! A sandbox bounds one run; how many runs one person (or one instance) has
//! at once is the driver's to limit, before it calls [`Sandbox::run`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

pub mod conformance;

/// `Send + Sync` natively, where a run is a `Send` future driven on any
/// thread; nothing on wasm32, where the Worker has one thread and its
/// JavaScript values are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}
/// `Send + Sync` natively; nothing on wasm32 (see the native definition).
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// One script: a title (used as its file name in stack traces), the
/// body of the async function it runs as, and, when given, what makes its
/// run repeatable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub title: String,
    pub code: String,
    #[serde(default)]
    pub determinism: Option<Determinism>,
}

/// What a script's clock and random numbers are, so a script run again
/// (a step resumed after a crash) sees what it saw before: `Date.now()`
/// and `new Date()` with no arguments give `now_ms` (milliseconds since
/// the epoch) throughout the run, and `Math.random()` is the sequence of
/// [`seeded_random`] from `seed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Determinism {
    pub now_ms: i64,
    pub seed: u32,
}

impl Script {
    pub fn new(title: impl Into<String>, code: impl Into<String>) -> Self {
        Script {
            title: title.into(),
            code: code.into(),
            determinism: None,
        }
    }

    /// The same script, its clock frozen at `now_ms` and its random
    /// numbers seeded from `seed` ([`Determinism`]).
    #[must_use]
    pub fn with_determinism(self, now_ms: i64, seed: u32) -> Self {
        Script {
            determinism: Some(Determinism { now_ms, seed }),
            ..self
        }
    }
}

/// The first `n` numbers `Math.random()` gives a script seeded with `seed`
/// (mulberry32, as the prelude computes it), for implementations' tests.
#[must_use]
pub fn seeded_random(seed: u32, n: usize) -> Vec<f64> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            state = state.wrapping_add(0x6D2B_79F5);
            let mut t = state;
            t = (t ^ (t >> 15)).wrapping_mul(t | 1);
            t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
            f64::from(t ^ (t >> 14)) / 4_294_967_296.0
        })
        .collect()
}

/// What one script may use. Every limit ends the script when exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// CPU time the script's own execution may take, in milliseconds
    /// (measured with the thread's CPU clock where there is one, else wall
    /// time while the script runs; time waiting on the host never counts).
    pub cpu_ms: u64,
    /// Memory the script's JavaScript heap may hold, in bytes.
    pub memory_bytes: u64,
    /// Host calls the script may make.
    pub max_calls: u32,
    /// Bytes of `console.log` output kept (see the crate docs, "Logs").
    pub max_log_bytes: u64,
    /// Bytes the JSON of the returned value may take.
    pub max_result_bytes: u64,
    /// Bytes the JSON of one host call's arguments may take.
    pub max_call_args_bytes: u64,
}

/// The least [`Limits::memory_bytes`] a sandbox accepts: below it an
/// engine cannot set up the script's context.
pub const MIN_MEMORY_BYTES: u64 = 1 << 20;
/// The most [`Limits::cpu_ms`] a sandbox accepts (an hour).
pub const MAX_CPU_MS: u64 = 3_600_000;
/// The least each byte limit on logs, results and arguments may be.
pub const MIN_IO_BYTES: u64 = 64;

impl Default for Limits {
    /// CPU 2 s, memory 64 MiB, 200 host calls, logs 32 KiB, result 64 KiB,
    /// arguments 256 KiB a call.
    fn default() -> Self {
        Limits {
            cpu_ms: 2_000,
            memory_bytes: 64 << 20,
            max_calls: 200,
            max_log_bytes: 32 << 10,
            max_result_bytes: 64 << 10,
            max_call_args_bytes: 256 << 10,
        }
    }
}

impl Limits {
    /// Whether a sandbox can run under these limits: CPU from 1 ms to
    /// [`MAX_CPU_MS`], memory at least [`MIN_MEMORY_BYTES`], at least one
    /// host call, and at least [`MIN_IO_BYTES`] for logs, the result and
    /// each call's arguments. No limit means "unlimited": a zero is
    /// refused, not read as no limit.
    pub fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.cpu_ms == 0 || self.cpu_ms > MAX_CPU_MS {
            problems.push(format!(
                "cpu_ms is {}; it must be 1 to {MAX_CPU_MS}",
                self.cpu_ms
            ));
        }
        if self.memory_bytes < MIN_MEMORY_BYTES {
            problems.push(format!(
                "memory_bytes is {}; it must be at least {MIN_MEMORY_BYTES}",
                self.memory_bytes
            ));
        }
        if self.max_calls == 0 {
            problems.push("max_calls is 0; it must be at least 1".to_string());
        }
        for (name, value) in [
            ("max_log_bytes", self.max_log_bytes),
            ("max_result_bytes", self.max_result_bytes),
            ("max_call_args_bytes", self.max_call_args_bytes),
        ] {
            if value < MIN_IO_BYTES {
                problems.push(format!(
                    "{name} is {value}; it must be at least {MIN_IO_BYTES}"
                ));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }
}

/// Why the operation names a sandbox is built with are not usable, if they
/// are not: each is one or more segments joined by `.`, each segment an
/// identifier (`[A-Za-z_$][A-Za-z0-9_$]*`) other than `__proto__`,
/// `constructor` and `prototype`; no name is listed twice, and none is
/// also the namespace of another (`files` beside `files.read`).
pub fn validate_ops<S: AsRef<str>>(ops: &[S]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for op in ops {
        let op = op.as_ref();
        for segment in op.split('.') {
            let mut chars = segment.chars();
            let first = chars.next();
            let ident = first.is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
            if !ident || matches!(segment, "__proto__" | "constructor" | "prototype") {
                return Err(format!("{op:?} is not a usable operation name"));
            }
        }
        if !seen.insert(op.to_string()) {
            return Err(format!("{op:?} is listed twice"));
        }
    }
    for op in &seen {
        let prefix = format!("{op}.");
        if seen.iter().any(|other| other.starts_with(&prefix)) {
            return Err(format!("{op:?} is both an operation and a namespace"));
        }
    }
    Ok(())
}

/// A host call that failed, as the script sees it: a stable machine code
/// (`not_found`, `forbidden`, …) and a message for the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostError {
    pub code: String,
    pub message: String,
}

impl HostError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        HostError {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HostError {}

/// The code of a call the sandbox refused for its arguments.
pub const CODE_INVALID_ARGS: &str = "invalid_args";
/// The code of a call made after (or not awaited before) the script's end.
pub const CODE_SCRIPT_FINISHED: &str = "script_finished";
/// The code of the host call a cancellation interrupted.
pub const CODE_CANCELLED: &str = "cancelled";

/// Stops a run. Cheap to clone; every clone is the same token. Works on
/// every target (no runtime needed): [`CancelToken::cancelled`] is a plain
/// future woken by [`CancelToken::cancel`].
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<CancelState>);

#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        CancelToken::default()
    }

    /// Cancels: every run given this token (or a clone) ends.
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::SeqCst) {
            let wakers =
                std::mem::take(&mut *self.0.wakers.lock().unwrap_or_else(|e| e.into_inner()));
            for waker in wakers {
                waker.wake();
            }
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled.
    #[must_use]
    pub fn cancelled(&self) -> Cancelled<'_> {
        Cancelled(self)
    }
}

/// The future of [`CancelToken::cancelled`].
#[derive(Debug)]
pub struct Cancelled<'a>(&'a CancelToken);

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.is_cancelled() {
            return Poll::Ready(());
        }
        {
            let mut wakers = self.0.0.wakers.lock().unwrap_or_else(|e| e.into_inner());
            if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                wakers.push(cx.waker().clone());
            }
        }
        if self.0.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// What a script can reach: the operations behind `clax`.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Host: MaybeSendSync {
    /// Performs `op` (a dotted name the sandbox was built with) on `args`,
    /// the JSON array of the script's arguments.
    async fn call(&self, op: &str, args: Value) -> Result<Value, HostError>;
}

/// Runs scripts.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Sandbox: MaybeSendSync {
    /// Runs `script` to its end under `limits`, its host calls through
    /// `host`, until it ends or `cancel` is cancelled. Never panics on
    /// anything the script does: every failure is in the outcome's
    /// [`ScriptOutcome::error`].
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome;
}

/// One host call the script made, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    pub op: String,
    /// Whether the host returned a value. False for a host error, and for
    /// a call that never reached the host.
    pub ok: bool,
    /// The [`HostError::code`] of a failed call: the host's, or the
    /// sandbox's own ([`CODE_INVALID_ARGS`], [`CODE_SCRIPT_FINISHED`],
    /// [`CODE_CANCELLED`]) for a call the host never answered.
    pub code: Option<String>,
    /// Bytes of the call's arguments as JSON.
    pub args_bytes: u64,
    /// Wall time the host took, in microseconds (0 when it was not
    /// reached).
    pub us: u64,
}

/// Why a script ended without a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The code does not compile as an async function body.
    Syntax,
    /// The script threw (or returned a rejected promise, or awaited one
    /// that can never settle) and did not catch it.
    Thrown,
    /// [`Limits::cpu_ms`] ran out.
    CpuLimit,
    /// [`Limits::memory_bytes`] ran out.
    MemoryLimit,
    /// The script tried more than [`Limits::max_calls`] host calls.
    CallLimit,
    /// The returned value's JSON is over [`Limits::max_result_bytes`].
    ResultTooLarge,
    /// A host call failed and the script did not catch it.
    HostError,
    /// The limits fail [`Limits::validate`]; nothing ran.
    InvalidLimits,
    /// The run's [`CancelToken`] was cancelled.
    Cancelled,
    /// The sandbox could not take the script (out of threads, or too many
    /// earlier runs it had to abandon still running); nothing ran.
    Unavailable,
}

/// The error a script ended with, for the model to read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptError {
    pub kind: ErrorKind,
    /// What happened, with the script's line number where there is one
    /// (`… (line 3)`).
    pub message: String,
}

impl ScriptError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ScriptError {
            kind,
            message: message.into(),
        }
    }
}

/// Everything one run did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ScriptOutcome {
    /// The returned value, when the script returned one and ended without
    /// an error.
    pub value: Option<Value>,
    /// Its `console.log` lines (see the crate docs, "Logs").
    pub logs: Vec<String>,
    /// The host calls it made, in order.
    pub calls: Vec<CallRecord>,
    pub error: Option<ScriptError>,
    /// CPU time the engine measured for the script's execution, in
    /// microseconds: what is billed. For a run the sandbox had to stop from
    /// outside (cancelled, or past its CPU budget without the engine
    /// noticing) it is the last figure the engine reported, and
    /// [`ScriptOutcome::busy_us`] says how long it was busy.
    pub cpu_us: u64,
    /// Wall time the engine was busy (running the script, not waiting on
    /// the host), in microseconds: what a watchdog bounds. Not billed.
    #[serde(default)]
    pub busy_us: u64,
    /// The most memory its heap (and its arguments in flight) held at the
    /// points the sandbox sampled it, in bytes, or a fixed limit of the
    /// isolate's own that is over the configured one. Billing charges the
    /// larger of this and [`Limits::memory_bytes`] (the crate docs,
    /// "Cancellation and metering").
    pub mem_peak_bytes: u64,
    /// Wall time of the whole run, host calls included, in microseconds.
    pub wall_us: u64,
}

impl ScriptOutcome {
    /// An outcome with only an error: nothing ran.
    #[must_use]
    pub fn failed(kind: ErrorKind, message: impl Into<String>) -> Self {
        ScriptOutcome {
            error: Some(ScriptError::new(kind, message)),
            ..ScriptOutcome::default()
        }
    }
}

/// The line that stands for every log line dropped once
/// [`Limits::max_log_bytes`] is reached.
pub const LOGS_TRUNCATED: &str = "[logs truncated]";

/// The JavaScript every sandbox runs before the script, so that `clax`,
/// `console.log`, arguments, errors and the body rule behave identically
/// everywhere.
///
/// It is one function expression, called once as
/// `prelude(ops, call, log, done, options)` with:
///
/// - `ops`: the array of dotted operation names (it throws a `TypeError`
///   for one [`validate_ops`] refuses);
/// - `call(op, args)`: the implementation's native function that takes one
///   host call and returns its call ID (an integer), or throws (a
///   `TypeError` for arguments it refuses). `args` is the arguments' JSON
///   text; or, when that is longer than `options.maxCallArgsBytes`
///   characters, its length (a number); or `undefined` when they have no
///   JSON (`JSON.stringify` threw or gave nothing);
/// - `log(line)`: the native function that keeps one log line;
/// - `done(ok, payload)`: the native function told how the script ended:
///   `done(true, json)` with the returned value's JSON (or `undefined`
///   when there is none), or `done(false, errorJson)` with the JSON of
///   [`Thrown`] for what it threw;
/// - `options`: `{maxCallArgsBytes}`, from [`Limits`], and `nowMs` and
///   `seed` from the script's [`Determinism`] when it has one (else left
///   out: the engine's own clock and random numbers).
///
/// It defines the globals `clax` and `console` and returns an object of
/// four functions, which capture every built-in they use before the
/// script runs:
///
/// - `check(code)` throws the `SyntaxError` of code that does not compile
///   as an async function body on its own, as `new AsyncFunction(code)`
///   compiles it. That is the rule only in an engine whose `Function`
///   constructors parse the body on its own, as the specification says and
///   V8 does: stock QuickJS does not (it parses, and runs, the concatenated
///   source), which is why `clax-bot-js` builds a patched QuickJS. A
///   sandbox runs [`check_body`] first in any case;
/// - `lockdown()` turns off code generation from strings (`eval`,
///   `Function` and the async and generator function constructors throw
///   `EvalError`) and removes QuickJS's `InternalError` global; run it
///   after compiling the script and before starting it;
/// - `start(fn, file)` runs the script's compiled async function
///   (compiled under the file name `file`, which it looks for in stack
///   traces to find the line) and reports its end through `done`;
/// - `settle(id, ok, json)` settles a host call: `ok` true with the
///   result's JSON, false with the JSON of a [`HostError`].
pub const PRELUDE: &str = include_str!("prelude.js");

/// What [`wrap`] puts before the code: its own line, as the `Function`
/// constructors' source does, so a first line that opens with `-->` (an
/// HTML-like comment there) reads the same in [`check_body`], the
/// prelude's `check` and the compiled script.
pub const WRAP_PREFIX: &str = "(async function () {\n";
/// What [`wrap`] puts after the code.
pub const WRAP_SUFFIX: &str = "\n})";

/// The source an implementation compiles for `code` once [`check_body`]
/// (and the prelude's `check`) have passed it: an async function expression
/// whose body is `code`. Line `N` of `code` is line `N + 1` of this source;
/// error messages give `N`.
#[must_use]
pub fn wrap(code: &str) -> String {
    format!("{WRAP_PREFIX}{code}{WRAP_SUFFIX}")
}

/// Why [`check_body`] refused code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyError {
    pub message: String,
    /// The code's line the parser stopped at, when it names one.
    pub line: Option<u64>,
}

impl BodyError {
    /// The [`ScriptError`] of [`ErrorKind::Syntax`] a run ends with.
    #[must_use]
    pub fn into_error(self) -> ScriptError {
        let mut message = format!("SyntaxError: {}", self.message);
        if let Some(line) = self.line {
            message.push_str(&format!(" (line {line})"));
        }
        ScriptError::new(ErrorKind::Syntax, message)
    }
}

/// Whether `code` parses as the body of an async function on its own, by a
/// parser that is no engine's (oxc's, pure Rust, on every target), so every
/// sandbox applies the same rule whatever its engine's `Function`
/// constructor does: [`wrap`]`(code)` must parse as a script (sloppy mode,
/// HTML-like comments allowed, as the engines' scripts) whose only
/// statement is the parenthesised async function, spanning the whole source
/// but its parentheses. Code that closes the function and goes on fails,
/// whatever it closes it with, as does code that loads a module (`import()`,
/// `import.meta`). The parser leaves some early errors to later
/// analysis; the engine still refuses those when it compiles.
pub fn check_body(code: &str) -> Result<(), BodyError> {
    use oxc_ast::ast::{Expression, Statement};
    let source = wrap(code);
    let allocator = oxc_allocator::Allocator::default();
    let options = oxc_parser::ParseOptions {
        allow_return_outside_function: false,
        preserve_parens: true,
        ..oxc_parser::ParseOptions::default()
    };
    let parsed = oxc_parser::Parser::new(&allocator, &source, oxc_span::SourceType::script())
        .with_options(options)
        .parse();
    let line_of = |offset: usize| {
        let line = source[..offset.min(source.len())].matches('\n').count() as u64;
        (line >= 1).then_some(line)
    };
    if let Some(diagnostic) = parsed.diagnostics.first() {
        let offset = diagnostic
            .labels
            .first()
            .map(|label| label.offset() as usize);
        return Err(BodyError {
            message: diagnostic.message.to_string(),
            line: offset.and_then(line_of),
        });
    }
    if parsed.panicked {
        return Err(BodyError {
            message: "the code does not parse".into(),
            line: None,
        });
    }
    // A script loads no module: an engine with a module loader of its own
    // (a Worker's built-in modules) would hand it what `clax` does not.
    let module_use = parsed
        .module_record
        .dynamic_imports
        .first()
        .map(|i| i.span.start)
        .or_else(|| parsed.module_record.import_metas.first().map(|s| s.start));
    if let Some(offset) = module_use {
        return Err(BodyError {
            message: "import() and import.meta are not allowed: a script loads no module".into(),
            line: line_of(offset as usize),
        });
    }
    let program = &parsed.program;
    let whole = program.directives.is_empty()
        && program.body.len() == 1
        && match &program.body[0] {
            Statement::ExpressionStatement(statement) => match &statement.expression {
                Expression::ParenthesizedExpression(parens) => match &parens.expression {
                    Expression::FunctionExpression(function) => {
                        function.r#async
                            && !function.generator
                            && function.span.start == 1
                            && function.span.end as usize == source.len() - 1
                    }
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        };
    if whole {
        Ok(())
    } else {
        Err(BodyError {
            message: "the code closes the function it is the body of".into(),
            line: None,
        })
    }
}

/// What the prelude reports a script threw, through `done(false, …)`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Default)]
pub struct Thrown {
    /// The error's `name` (`Error`, `TypeError`, `HostError`, …), empty for
    /// a thrown value that is not an `Error`.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub message: String,
    /// A [`HostError::code`], for a host error.
    #[serde(default)]
    pub code: Option<String>,
    /// The script line it was thrown at, when known.
    #[serde(default)]
    pub line: Option<u64>,
    /// Whether it is one of the sandbox's own host errors (known by
    /// identity, not by its properties).
    #[serde(default)]
    pub host: bool,
}

impl Thrown {
    /// The [`ScriptError`] the run ends with.
    #[must_use]
    pub fn into_error(self) -> ScriptError {
        let kind = if self.host {
            ErrorKind::HostError
        } else {
            ErrorKind::Thrown
        };
        let mut message = match (self.name.as_str(), &self.code) {
            ("", _) => self.message,
            (name, Some(code)) => format!("{name} {code}: {}", self.message),
            (name, None) if self.message.is_empty() => name.to_string(),
            (name, None) => format!("{name}: {}", self.message),
        };
        if let Some(line) = self.line {
            message.push_str(&format!(" (line {line})"));
        }
        ScriptError::new(kind, message)
    }
}

/// Keeps log lines under [`Limits::max_log_bytes`] by the rule of the crate
/// docs ("Logs"), for implementations to share.
#[derive(Debug, Default, Clone)]
pub struct LogBuffer {
    lines: Vec<String>,
    bytes: u64,
    cap: u64,
    full: bool,
}

impl LogBuffer {
    #[must_use]
    pub fn new(cap: u64) -> Self {
        LogBuffer {
            cap,
            ..LogBuffer::default()
        }
    }

    /// Keeps `line` if it fits; the first that does not becomes
    /// [`LOGS_TRUNCATED`], and later ones are dropped.
    pub fn push(&mut self, line: String) {
        if self.full {
            return;
        }
        let len = line.len() as u64;
        if self.bytes + len > self.cap {
            self.full = true;
            self.lines.push(LOGS_TRUNCATED.to_string());
            return;
        }
        self.bytes += len;
        self.lines.push(line);
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    #[must_use]
    pub fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_keep_whole_lines_until_one_does_not_fit() {
        let mut logs = LogBuffer::new(10);
        logs.push("12345".into());
        logs.push("12345".into());
        logs.push("1".into());
        logs.push("".into());
        assert_eq!(logs.into_lines(), vec!["12345", "12345", LOGS_TRUNCATED]);
    }

    #[test]
    fn wrapping_puts_the_code_on_lines_of_its_own() {
        assert_eq!(wrap("return 1"), "(async function () {\nreturn 1\n})");
    }

    #[test]
    fn a_body_that_closes_its_function_is_refused() {
        for (_, code) in conformance::BREAKOUTS {
            assert!(check_body(code).is_err(), "{code:?}");
        }
        for code in conformance::BODIES {
            assert_eq!(check_body(code), Ok(()), "{code:?}");
        }
    }

    #[test]
    fn a_body_that_loads_a_module_is_refused() {
        for (code, line) in [
            ("return import('x')", Some(1)),
            ("const a = 1;\nawait import(`node:${'fs'}`)", Some(2)),
            ("return import.meta", Some(1)),
            ("return import.source('x')", Some(1)),
        ] {
            let err = check_body(code).unwrap_err();
            assert_eq!(err.line, line, "{code:?}: {err:?}");
        }
        assert_eq!(
            check_body("const o = { import: 1 }; return o.import"),
            Ok(())
        );
        assert_eq!(check_body("return 'import(\"x\")'"), Ok(()));
    }

    #[test]
    fn a_syntax_error_names_the_code_line() {
        let err = check_body("const a = 1;\nconst = 2;").unwrap_err();
        assert_eq!(err.line, Some(2), "{err:?}");
        assert!(err.into_error().message.starts_with("SyntaxError: "));
    }

    #[test]
    fn a_thrown_value_reads_as_its_name_message_and_line() {
        let host = Thrown {
            name: "HostError".into(),
            message: "no such file".into(),
            code: Some("not_found".into()),
            line: Some(2),
            host: true,
        };
        assert_eq!(
            host.into_error(),
            ScriptError::new(
                ErrorKind::HostError,
                "HostError not_found: no such file (line 2)"
            )
        );
        let plain = Thrown {
            message: "oops".into(),
            ..Thrown::default()
        };
        assert_eq!(
            plain.into_error(),
            ScriptError::new(ErrorKind::Thrown, "oops")
        );
    }

    #[test]
    fn default_limits_are_valid_and_zeros_are_not() {
        assert_eq!(Limits::default().validate(), Ok(()));
        let zero = Limits {
            cpu_ms: 0,
            memory_bytes: 0,
            max_calls: 0,
            max_log_bytes: 0,
            max_result_bytes: 0,
            max_call_args_bytes: 0,
        };
        let err = zero.validate().unwrap_err();
        for field in [
            "cpu_ms",
            "memory_bytes",
            "max_calls",
            "max_log_bytes",
            "max_result_bytes",
            "max_call_args_bytes",
        ] {
            assert!(err.contains(field), "{err}");
        }
        let tiny = Limits {
            memory_bytes: 50_000,
            ..Limits::default()
        };
        assert!(tiny.validate().unwrap_err().contains("memory_bytes"));
        let long = Limits {
            cpu_ms: MAX_CPU_MS + 1,
            ..Limits::default()
        };
        assert!(long.validate().unwrap_err().contains("cpu_ms"));
        let least = Limits {
            cpu_ms: 1,
            memory_bytes: MIN_MEMORY_BYTES,
            max_calls: 1,
            max_log_bytes: MIN_IO_BYTES,
            max_result_bytes: MIN_IO_BYTES,
            max_call_args_bytes: MIN_IO_BYTES,
        };
        assert_eq!(least.validate(), Ok(()));
    }

    #[test]
    fn op_names_are_identifiers_without_prototype_segments() {
        assert_eq!(
            validate_ops(&["echo", "files.read", "db.get", "$a._b1"]),
            Ok(())
        );
        for bad in [
            "",
            "files.",
            ".read",
            "__proto__.x",
            "a.constructor",
            "prototype",
            "1a",
            "a-b",
            "a b",
        ] {
            assert!(validate_ops(&[bad]).is_err(), "{bad:?}");
        }
        assert!(validate_ops(&["a", "a"]).is_err());
        assert!(validate_ops(&["files", "files.read"]).is_err());
    }

    #[test]
    fn a_cancel_token_wakes_its_waiters_once_cancelled() {
        use std::sync::atomic::AtomicUsize;
        use std::task::Wake;
        struct Count(AtomicUsize);
        impl Wake for Count {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let count = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(count.clone());
        let mut cx = Context::from_waker(&waker);
        let token = CancelToken::new();
        let clone = token.clone();
        let mut fut = Box::pin(clone.cancelled());
        assert_eq!(fut.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(fut.as_mut().poll(&mut cx), Poll::Pending);
        token.cancel();
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
        assert_eq!(fut.as_mut().poll(&mut cx), Poll::Ready(()));
        assert!(clone.is_cancelled());
        token.cancel();
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_outcome_round_trips_through_json() {
        let outcome = ScriptOutcome {
            value: Some(serde_json::json!({"a": 1})),
            logs: vec!["hi".into()],
            calls: vec![CallRecord {
                op: "files.read".into(),
                ok: false,
                code: Some("not_found".into()),
                args_bytes: 9,
                us: 3,
            }],
            error: Some(ScriptError::new(ErrorKind::InvalidLimits, "x")),
            cpu_us: 1,
            busy_us: 5,
            mem_peak_bytes: 2,
            wall_us: 4,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["error"]["kind"], "invalid_limits");
        assert_eq!(
            serde_json::from_value::<ScriptOutcome>(json).unwrap(),
            outcome
        );
    }
}
