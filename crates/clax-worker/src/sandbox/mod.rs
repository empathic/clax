//! ClaxBot's code-mode sandbox on the Worker: [`LoaderSandbox`] runs each
//! script in a dynamic Worker of its own, loaded through the Worker Loader
//! binding `LOADER` (`worker_loaders` in `wrangler.jsonc`), behind the
//! contract of [`clax_bot_sandbox`].
//!
//! # A run
//!
//! The script's code passes [`check_body`] first, as on every sandbox. The
//! dynamic Worker is then loaded (`LOADER.load`, a fresh isolate per
//! script, never cached) with three modules: the driver (`driver.js`, its
//! main module), the shared [`PRELUDE`] as `clax-prelude.js`, and the
//! script, a CommonJS module whose export makes [`wrap`]`(code)`: CommonJS
//! is compiled as script code, so the body is sloppy-mode code with
//! HTML-like comments allowed, as the native sandbox compiles it, and line
//! `N` of the code is line `N + 1` of the module, the wrapper's own first
//! line subtracted as natively. Its module name is the script's title
//! ([`module_name`]). The isolate has no network (`globalOutbound: null`),
//! no bindings (`env` is empty), the compatibility date
//! [`COMPATIBILITY_DATE`], and `limits: {cpuMs}` from [`Limits::cpu_ms`].
//!
//! The instance calls the driver's `run(job, host)` once over RPC. `host`
//! is a function of the instance's own, passed as an RPC stub: the
//! script's only way out, and the instance's only door into it. Each host
//! call crosses it as one message; the instance checks the call itself
//! (its operation, the call count, the arguments' size and JSON) before
//! [`Host::call`] sees it, whatever the isolate claims, and answers it, so
//! calls reach the host one at a time in the order the script made them
//! (the driver sends the next only once the script has nothing left to
//! run). How the script ended arrives as a last message; the driver then
//! aborts its isolate, so nothing the script queued runs after its end
//! (the aborted request's failure may reach the instance first: the run
//! waits a moment for the last message before reading the failure).
//!
//! # Limits and metering
//!
//! - **Calls, logs, the result and arguments** are bounded as natively: in
//!   the driver, by the same rules, and checked again here.
//! - **CPU**: the platform's `cpuMs` limit on the isolate; it ends a script
//!   past it, which the run reads as [`ErrorKind::CpuLimit`]. A watchdog
//!   here bounds the time the isolate is busy (wall time not waiting on the
//!   host) to `cpu_ms × 4`, at least half a second, as the native
//!   sandbox's does; it can only fire while the isolate yields.
//!   `wrangler dev --local` (workerd) enforces no CPU limit: there a
//!   script that never yields holds the whole runtime.
//! - **Memory**: the isolate has the platform's own memory limit
//!   ([`ISOLATE_MEMORY_BYTES`], 128 MiB), which no option lowers; a script
//!   past it ends with [`ErrorKind::MemoryLimit`]. [`Limits::memory_bytes`]
//!   bounds nothing here. workerd enforces no memory limit.
//! - **Metering**: the platform reports no CPU time to the caller, so
//!   [`ScriptOutcome::cpu_us`] is [`ScriptOutcome::busy_us`], the wall time
//!   the isolate was busy, as the native sandbox's wall-time fallback is.
//!   Memory is not measured: every run that loaded an isolate reports
//!   `mem_peak_bytes` as the larger of [`ISOLATE_MEMORY_BYTES`] and
//!   [`Limits::memory_bytes`], what the script could have held, and billing
//!   charges the larger of that and the limit (`clax_bot_sandbox`'s crate
//!   docs, "Cancellation and metering").
//!
//! # Cancellation
//!
//! Cancelling ends the run at once with what it did; the instance answers
//! any later message from the isolate with a stop, which aborts it. A
//! script busy without calling the host runs on until the platform's CPU
//! limit ends it.

use std::collections::HashSet;

use clax_bot_sandbox::{
    CODE_INVALID_ARGS, CODE_SCRIPT_FINISHED, CallRecord, ErrorKind, LOGS_TRUNCATED, Limits,
    LogBuffer, PRELUDE, ScriptError, ScriptOutcome, Thrown, wrap,
};
#[cfg(target_arch = "wasm32")]
use clax_bot_sandbox::{CancelToken, Host, Sandbox, Script, check_body};
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(target_arch = "wasm32")]
pub mod checks;
#[cfg(target_arch = "wasm32")]
mod loader;

/// Conformance cases this sandbox ends otherwise than the native one, by
/// how V8 differs from QuickJS: the case, the error kind it ends with
/// here, and why.
pub const PLATFORM_DIFFERENCES: &[(&str, ErrorKind, &str)] = &[
    (
        "a_deeply_nested_json_parse_is_thrown",
        ErrorKind::ResultTooLarge,
        "V8's JSON.parse is not recursive: a million levels parse, and the result is over its limit",
    ),
    (
        "the_memory_limit_ends_a_deep_structure",
        ErrorKind::ResultTooLarge,
        "the isolate's memory limit is the platform's 128 MB, which two million nested arrays stay under; the result is over its limit",
    ),
];

/// Conformance cases that need a limit only Cloudflare enforces on the
/// isolate (`limits.cpuMs`, the 128 MB isolate): workerd under `wrangler
/// dev --local` enforces neither, so each would hold the runtime (or the
/// machine's memory) and is not run there.
pub const PLATFORM_ONLY: &[(&str, &str)] = &[
    ("the_cpu_limit_ends_a_loop", "workerd enforces no CPU limit"),
    (
        "the_cpu_limit_cannot_be_caught",
        "workerd enforces no CPU limit",
    ),
    (
        "the_cpu_limit_ends_a_loop_of_heavy_sorts",
        "workerd enforces no CPU limit",
    ),
    (
        "the_cpu_limit_ends_a_loop_of_heavy_string_builtins",
        "workerd enforces no CPU limit",
    ),
    (
        "the_cpu_limit_ends_a_backtracking_regexp",
        "workerd enforces no CPU limit",
    ),
    (
        "the_cpu_limit_ends_a_microtask_loop_beside_a_host_call",
        "workerd enforces no CPU limit",
    ),
    (
        "the_memory_limit_ends_an_allocation_bomb",
        "workerd enforces no memory limit",
    ),
    (
        "the_memory_limit_cannot_be_caught",
        "workerd enforces no memory limit",
    ),
];

/// The memory limit Cloudflare puts on every isolate, a dynamic Worker's
/// included, in bytes: what a run reports as its peak (see "Limits and
/// metering").
pub const ISOLATE_MEMORY_BYTES: u64 = 128 << 20;

/// The binding the sandbox loads dynamic Workers through.
pub const LOADER_BINDING: &str = "LOADER";
/// The compatibility date of every dynamic Worker (the Worker's own,
/// `wrangler.jsonc`).
pub const COMPATIBILITY_DATE: &str = "2026-09-25";
/// The driver module's name.
pub const DRIVER_MODULE: &str = "clax-driver.js";
/// The prelude module's name.
pub const PRELUDE_MODULE: &str = "clax-prelude.js";
const DRIVER_JS: &str = include_str!("driver.js");
/// What the script's CommonJS module puts before [`wrap`]`(code)`, on the
/// wrapper's first line: shadows the CommonJS wrapper's own arguments.
const SCRIPT_PREFIX: &str =
    "module.exports = function (require, module, exports, __filename, __dirname) { return ";
const SCRIPT_SUFFIX: &str = "; };";

/// How long the isolate may be busy before the run gives up on it: the
/// larger of `cpu_ms × factor` and `floor_ms`, in wall time while it is not
/// waiting on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watchdog {
    pub factor: u32,
    pub floor_ms: u64,
}

impl Default for Watchdog {
    /// Four times the CPU budget, and at least half a second (as the native
    /// sandbox's).
    fn default() -> Self {
        Watchdog {
            factor: 4,
            floor_ms: 500,
        }
    }
}

impl Watchdog {
    #[must_use]
    pub fn deadline_ms(&self, cpu_ms: u64) -> u64 {
        cpu_ms
            .saturating_mul(u64::from(self.factor))
            .max(self.floor_ms)
    }
}

/// The [`clax_bot_sandbox::Sandbox`] over the Worker Loader binding (see
/// the module docs), whose `clax` has the operations it was built with.
/// Without the binding every run ends with [`ErrorKind::Unavailable`].
// Read by the wasm32 run only.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub struct LoaderSandbox {
    ops: Vec<String>,
    op_set: HashSet<String>,
    watchdog: Watchdog,
    /// The `LOADER` binding, when the Worker has one.
    loader: Option<worker::send::SendWrapper<wasm_bindgen::JsValue>>,
}

impl LoaderSandbox {
    /// A sandbox over `env`'s `LOADER` binding (none: every run is
    /// [`ErrorKind::Unavailable`]), whose `clax` has `ops`; or why `ops`
    /// are not usable ([`clax_bot_sandbox::validate_ops`]).
    pub fn from_env<I, S>(env: &worker::Env, ops: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let binding = js_sys::Reflect::get(
            env.as_ref(),
            &wasm_bindgen::JsValue::from_str(LOADER_BINDING),
        )
        .ok()
        .filter(|b| !b.is_undefined() && !b.is_null());
        LoaderSandbox::with_binding(binding, ops)
    }

    /// A sandbox over `binding` (a Worker Loader binding, or `None`).
    pub fn with_binding<I, S>(
        binding: Option<wasm_bindgen::JsValue>,
        ops: I,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ops: Vec<String> = ops.into_iter().map(Into::into).collect();
        clax_bot_sandbox::validate_ops(&ops)?;
        Ok(LoaderSandbox {
            op_set: ops.iter().cloned().collect(),
            ops,
            watchdog: Watchdog::default(),
            loader: binding.map(worker::send::SendWrapper::new),
        })
    }

    /// The same sandbox, bounded by `watchdog`.
    #[must_use]
    pub fn with_watchdog(self, watchdog: Watchdog) -> Self {
        LoaderSandbox { watchdog, ..self }
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl Sandbox for LoaderSandbox {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let started = js_sys::Date::now();
        if let Err(e) = limits.validate() {
            return ScriptOutcome::failed(ErrorKind::InvalidLimits, e);
        }
        let Some(binding) = &self.loader else {
            return ScriptOutcome::failed(
                ErrorKind::Unavailable,
                "the Worker has no LOADER binding (worker_loaders in wrangler.jsonc)",
            );
        };
        if let Err(e) = check_body(&script.code) {
            return ScriptOutcome {
                error: Some(e.into_error()),
                wall_us: micros_since(started),
                ..ScriptOutcome::default()
            };
        }
        if cancel.is_cancelled() {
            return ScriptOutcome::failed(ErrorKind::Cancelled, "the run was cancelled");
        }
        let state = RunState::new(&self.op_set, *limits);
        loader::run(
            binding,
            script,
            &self.ops,
            limits,
            state,
            host,
            cancel,
            self.watchdog.deadline_ms(limits.cpu_ms),
            started,
        )
        .await
    }
}

#[cfg(target_arch = "wasm32")]
fn micros_since(start_ms: f64) -> u64 {
    ms_to_us(js_sys::Date::now() - start_ms)
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn ms_to_us(ms: f64) -> u64 {
    if ms.is_finite() && ms > 0.0 {
        (ms * 1000.0) as u64
    } else {
        0
    }
}

/// The script's module name: its title with every character but ASCII
/// letters, digits, space, `_`, `-` and `.` replaced by `_`, no leading
/// `.` (a relative specifier), and `script` for one left empty or naming
/// the driver's own modules. It is the file name in the script's stack
/// traces, as the title is natively.
#[must_use]
pub fn module_name(title: &str) -> String {
    let mut name: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while name.starts_with('.') {
        name.replace_range(..1, "_");
    }
    let name = name.trim().to_string();
    if name.is_empty() || name == DRIVER_MODULE || name == PRELUDE_MODULE {
        "script".into()
    } else {
        name
    }
}

/// The script's CommonJS module: [`wrap`]`(code)` on the module's first
/// line onwards, so code line `N` is module line `N + 1`.
#[must_use]
pub fn script_module(code: &str) -> String {
    format!("{SCRIPT_PREFIX}{}{SCRIPT_SUFFIX}", wrap(code))
}

/// The prelude's CommonJS module, exporting [`PRELUDE`]'s function.
#[must_use]
pub fn prelude_module() -> String {
    format!("module.exports = {PRELUDE}")
}

/// The driver module, importing the script's module `name`.
#[must_use]
pub fn driver_module(name: &str) -> String {
    format!(
        "import make from {};\n{DRIVER_JS}",
        Value::String(name.to_string())
    )
}

/// The job the driver's `run` takes, as JSON: with `determinism`, the
/// frozen clock and the random seed the prelude takes (`nowMs`, `seed`),
/// as the native sandbox hands them.
#[must_use]
pub fn job_json(
    ops: &[String],
    limits: &Limits,
    file: &str,
    determinism: Option<clax_bot_sandbox::Determinism>,
) -> String {
    let mut job = json!({
        "ops": ops,
        "maxCalls": limits.max_calls,
        "maxCallArgsBytes": limits.max_call_args_bytes,
        "maxLogBytes": limits.max_log_bytes,
        "truncated": LOGS_TRUNCATED,
        "file": file,
    });
    if let Some(d) = determinism {
        job["nowMs"] = json!(d.now_ms);
        job["seed"] = json!(d.seed);
    }
    job.to_string()
}

/// A call the driver says never reached the host.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Refused {
    pub op: String,
    pub code: String,
    #[serde(default)]
    pub bytes: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CallMessage {
    pub op: String,
    pub args: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Done {
    pub ok: bool,
    #[serde(default)]
    pub payload: Option<String>,
}

/// How the script ended, as the driver reports it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Final {
    #[serde(default)]
    pub records: Vec<Refused>,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub done: Option<Done>,
    #[serde(default)]
    pub stop: Option<String>,
    #[serde(default)]
    pub never: bool,
    #[serde(default)]
    pub gone: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Message {
    #[serde(default)]
    records: Vec<Refused>,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default)]
    call: Option<CallMessage>,
    #[serde(default, rename = "final")]
    end: Option<Final>,
}

/// What one message from the isolate asks of the run.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// A call for the host; answer it with [`RunState::answer`].
    Call {
        op: String,
        args: Value,
        args_bytes: u64,
    },
    /// A call refused here: send this answer.
    Reply(String),
    /// The script ended.
    End(End),
}

/// Why a run ended.
#[derive(Debug, PartialEq)]
pub enum End {
    /// The driver's last message.
    Final(Box<Final>),
    /// The isolate went over its calls past the driver's count.
    Calls,
    /// The isolate's request failed with this message, with no last
    /// message first.
    Failed(String),
    /// No isolate was loaded: the loader refused the script's code with
    /// this message.
    NotLoaded(String),
    /// The message could not be read.
    Garbled(String),
    /// The isolate reported a record the driver cannot have made.
    Forged(String),
    Cancelled,
    Watchdog {
        busy_ms: u64,
    },
}

/// The most bytes a record may say a call's arguments had: past the
/// longest string V8 makes.
const MAX_RECORD_BYTES: f64 = (1u64 << 30) as f64;

/// The answer that stops the isolate: the run has ended.
pub const STOP_REPLY: &str = r#"{"stop":"the run has ended"}"#;

/// One run's records, logs and checks: everything about a run that is not
/// JavaScript, so that natively testable.
pub struct RunState {
    ops: HashSet<String>,
    limits: Limits,
    calls: Vec<CallRecord>,
    logs: LogBuffer,
    /// Calls the instance was sent.
    received: u32,
    /// Calls the driver reported never reached it.
    reported: u32,
}

impl RunState {
    #[must_use]
    pub fn new(op_set: &HashSet<String>, limits: Limits) -> Self {
        RunState {
            ops: op_set.clone(),
            limits,
            calls: Vec::new(),
            logs: LogBuffer::new(limits.max_log_bytes),
            received: 0,
            reported: 0,
        }
    }

    fn refuse(&mut self, op: String, args_bytes: u64, message: String) -> Step {
        self.calls.push(CallRecord {
            op,
            ok: false,
            code: Some(CODE_INVALID_ARGS.into()),
            args_bytes,
            us: 0,
        });
        Step::Reply(answer_json(&Err(clax_bot_sandbox::HostError::new(
            CODE_INVALID_ARGS,
            message,
        ))))
    }

    /// Keeps the records and log lines the isolate sent, after checking
    /// each record against what the driver can report: a call it refused
    /// ([`CODE_INVALID_ARGS`]) or one the script's end left unsent
    /// ([`CODE_SCRIPT_FINISHED`]), of one of the sandbox's operations, with
    /// a byte count a JavaScript string can have, and no more of them, with
    /// the calls that reached the instance, than [`Limits::max_calls`].
    fn keep(&mut self, records: Vec<Refused>, logs: Vec<String>) -> Result<(), End> {
        for r in records {
            let known = r.code == CODE_INVALID_ARGS || r.code == CODE_SCRIPT_FINISHED;
            let bytes_ok = r.bytes.is_finite()
                && r.bytes >= 0.0
                && r.bytes.fract() == 0.0
                && r.bytes <= MAX_RECORD_BYTES;
            if !self.ops.contains(&r.op) || !known || !bytes_ok {
                return Err(End::Forged(format!(
                    "a record of {:?} with code {:?} and {} bytes",
                    r.op, r.code, r.bytes
                )));
            }
            if self.reported + self.received >= self.limits.max_calls {
                return Err(End::Calls);
            }
            self.reported += 1;
            self.calls.push(CallRecord {
                op: r.op,
                ok: false,
                code: Some(r.code),
                args_bytes: r.bytes as u64,
                us: 0,
            });
        }
        for line in logs {
            self.logs.push(line);
        }
        Ok(())
    }

    /// Reads one message from the isolate.
    pub fn take(&mut self, text: &str) -> Step {
        let message: Message = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => return Step::End(End::Garbled(e.to_string())),
        };
        if let Err(end) = self.keep(message.records, message.logs) {
            return Step::End(end);
        }
        if let Some(mut end) = message.end {
            let records = std::mem::take(&mut end.records);
            let logs = std::mem::take(&mut end.logs);
            if let Err(end) = self.keep(records, logs) {
                return Step::End(end);
            }
            return Step::End(End::Final(Box::new(end)));
        }
        let Some(call) = message.call else {
            return Step::End(End::Garbled("a message with no call".into()));
        };
        if self.reported + self.received >= self.limits.max_calls {
            return Step::End(End::Calls);
        }
        self.received += 1;
        let args_bytes = call.args.len() as u64;
        if !self.ops.contains(&call.op) {
            let message = format!("no operation {:?}", call.op);
            return self.refuse(call.op, args_bytes, message);
        }
        if args_bytes > self.limits.max_call_args_bytes {
            let message = format!(
                "the arguments of {} are {args_bytes} bytes of JSON; the limit is {}",
                call.op, self.limits.max_call_args_bytes
            );
            return self.refuse(call.op, args_bytes, message);
        }
        match serde_json::from_str::<Value>(&call.args) {
            Ok(args) if args.is_array() => Step::Call {
                op: call.op,
                args,
                args_bytes,
            },
            Ok(_) => {
                let message = format!("the arguments of {} do not serialise as an array", call.op);
                self.refuse(call.op, args_bytes, message)
            }
            Err(e) => {
                let message = format!(
                    "the arguments of {} do not parse back from their JSON: {e}",
                    call.op
                );
                self.refuse(call.op, args_bytes, message)
            }
        }
    }

    /// Records the host's answer to a [`Step::Call`]; the answer for the
    /// isolate.
    pub fn answer(
        &mut self,
        op: String,
        args_bytes: u64,
        us: u64,
        result: &Result<Value, clax_bot_sandbox::HostError>,
    ) -> String {
        self.calls.push(CallRecord {
            op,
            ok: result.is_ok(),
            code: result.as_ref().err().map(|e| e.code.clone()),
            args_bytes,
            us,
        });
        answer_json(result)
    }

    /// Records a host call a cancellation interrupted.
    pub fn interrupted(&mut self, op: String, args_bytes: u64, us: u64) {
        self.calls.push(CallRecord {
            op,
            ok: false,
            code: Some(clax_bot_sandbox::CODE_CANCELLED.into()),
            args_bytes,
            us,
        });
    }

    /// The run's outcome for `end`, the isolate busy for `busy_us` of the
    /// run's `wall_us`.
    #[must_use]
    pub fn outcome(mut self, end: End, busy_us: u64, wall_us: u64) -> ScriptOutcome {
        // A run that loaded no isolate held no memory.
        let loaded = !matches!(end, End::NotLoaded(_));
        let ended: Result<Option<Value>, ScriptError> = match end {
            End::Final(mut end) => {
                let records = std::mem::take(&mut end.records);
                let logs = std::mem::take(&mut end.logs);
                match self.keep(records, logs) {
                    Ok(()) => self.ended(*end),
                    Err(End::Calls) => Err(self.call_limit()),
                    Err(End::Forged(what)) => Err(forged(&what)),
                    Err(_) => Err(forged("an unreadable report")),
                }
            }
            End::Forged(what) => Err(forged(&what)),
            End::Calls => Err(self.call_limit()),
            End::Failed(message) | End::NotLoaded(message) => {
                Err(platform_error(&message, &self.limits))
            }
            End::Garbled(e) => Err(ScriptError::new(
                ErrorKind::Thrown,
                format!("the sandbox could not read its isolate's message: {e}"),
            )),
            End::Cancelled => Err(ScriptError::new(
                ErrorKind::Cancelled,
                "the run was cancelled",
            )),
            End::Watchdog { busy_ms } => Err(ScriptError::new(
                ErrorKind::CpuLimit,
                format!(
                    "the script used more than its {} ms of CPU time (stopped from outside after {busy_ms} ms busy)",
                    self.limits.cpu_ms
                ),
            )),
        };
        let mem_peak_bytes = if loaded {
            ISOLATE_MEMORY_BYTES.max(self.limits.memory_bytes)
        } else {
            0
        };
        let (value, error) = match ended {
            Ok(value) => (value, None),
            Err(e) => (None, Some(e)),
        };
        ScriptOutcome {
            value,
            logs: self.logs.into_lines(),
            calls: self.calls,
            error,
            cpu_us: busy_us,
            busy_us,
            mem_peak_bytes,
            wall_us,
        }
    }

    /// What the driver's last message says the script ended with.
    fn ended(&self, end: Final) -> Result<Option<Value>, ScriptError> {
        if end.stop.is_some() {
            Err(self.call_limit())
        } else if let Some(done) = end.done {
            if done.ok {
                value_of(done.payload, &self.limits)
            } else {
                let thrown: Thrown = done
                    .payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p).ok())
                    .unwrap_or_default();
                Err(thrown.into_error())
            }
        } else if end.never {
            Err(ScriptError::new(
                ErrorKind::Thrown,
                "the script awaited a promise that never settles",
            ))
        } else {
            Err(ScriptError::new(
                ErrorKind::Thrown,
                format!("the script lost its host: {}", end.gone.unwrap_or_default()),
            ))
        }
    }

    fn call_limit(&self) -> ScriptError {
        ScriptError::new(
            ErrorKind::CallLimit,
            format!(
                "the script made more than its {} host calls",
                self.limits.max_calls
            ),
        )
    }
}

/// The error of a run whose isolate reported what the driver cannot have:
/// the script tampered with the report.
fn forged(what: &str) -> ScriptError {
    ScriptError::new(
        ErrorKind::Thrown,
        format!("the sandbox refused its isolate's report ({what}): the script tampered with it"),
    )
}

/// The isolate's answer to a host call: `{ok, json}`.
#[must_use]
pub fn answer_json(result: &Result<Value, clax_bot_sandbox::HostError>) -> String {
    match result {
        Ok(v) => json!({"ok": true, "json": v.to_string()}).to_string(),
        Err(e) => json!({
            "ok": false,
            "json": json!({"code": e.code, "message": e.message}).to_string(),
        })
        .to_string(),
    }
}

/// The returned value from its JSON, as natively: over
/// [`Limits::max_result_bytes`] it is [`ErrorKind::ResultTooLarge`], and
/// JSON that does not parse back is thrown.
fn value_of(json: Option<String>, limits: &Limits) -> Result<Option<Value>, ScriptError> {
    match json {
        Some(json) if json.len() as u64 > limits.max_result_bytes => Err(ScriptError::new(
            ErrorKind::ResultTooLarge,
            format!(
                "the returned value is {} bytes of JSON; the limit is {} bytes",
                json.len(),
                limits.max_result_bytes
            ),
        )),
        Some(json) => serde_json::from_str(&json).map(Some).map_err(|e| {
            ScriptError::new(
                ErrorKind::Thrown,
                format!("the returned value does not parse back from its JSON: {e}"),
            )
        }),
        None => Ok(None),
    }
}

/// The error a failed request to the isolate ends the run with, by the
/// platform's message: past the CPU or memory limit, a module that does
/// not compile (an early error the body check leaves to the engine), or a
/// Worker that could not start.
#[must_use]
pub fn platform_error(message: &str, limits: &Limits) -> ScriptError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("exceeded cpu") || lower.contains("cpu time limit") {
        return ScriptError::new(
            ErrorKind::CpuLimit,
            format!(
                "the script used more than its {} ms of CPU time",
                limits.cpu_ms
            ),
        );
    }
    if lower.contains("exceeded memory") || lower.contains("memory limit") {
        return ScriptError::new(
            ErrorKind::MemoryLimit,
            format!(
                "the script needed more than the memory its isolate may hold (charged at {} bytes)",
                limits.memory_bytes
            ),
        );
    }
    if let Some(syntax) = message.find("SyntaxError: ") {
        let rest = &message[syntax..];
        let first = rest.lines().next().unwrap_or(rest);
        let line = message
            .lines()
            .filter_map(|l| l.trim().strip_prefix("at "))
            .find_map(|at| {
                let mut parts = at.rsplitn(3, ':');
                let _col = parts.next()?;
                parts.next()?.parse::<u64>().ok()
            })
            .and_then(|l| l.checked_sub(1))
            .filter(|l| *l > 0);
        let mut text = first.to_string();
        if let Some(line) = line {
            text.push_str(&format!(" (line {line})"));
        }
        return ScriptError::new(ErrorKind::Syntax, text);
    }
    if lower.contains("failed to start worker") || lower.contains("too many") {
        return ScriptError::new(
            ErrorKind::Unavailable,
            format!("the sandbox could not start the script's isolate: {message}"),
        );
    }
    ScriptError::new(
        ErrorKind::Thrown,
        format!("the script's isolate ended: {message}"),
    )
}

#[cfg(test)]
mod tests;
