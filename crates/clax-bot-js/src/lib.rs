//! ClaxBot's native code-mode sandbox: [`QuickJs`] runs each script in a
//! QuickJS runtime of its own (embedded through `rquickjs`, over the
//! patched `rquickjs-sys` in `third_party/`), behind the contract of
//! [`clax_bot_sandbox`].
//!
//! Each run gets a thread of its own, which owns the runtime and its one
//! context from start to end. The script's host calls cross to the
//! caller's task through channels: the engine thread queues each call, and
//! once the script has nothing left to run it sends the first one and
//! waits; the caller awaits [`Host::call`] and hands the result back, and
//! the engine settles the call's Promise. Calls therefore reach the host
//! one at a time, in the order the script made them; a call still queued
//! when the script's body settles is never sent. Time spent waiting on the
//! host is not the script's CPU time.
//!
//! The caller's side needs a tokio runtime with its timer
//! (`tokio::time`): it bounds the engine with a watchdog.
//!
//! # Budgets
//!
//! - **CPU**: QuickJS (patched) calls an interrupt handler every 100
//!   back-edges and calls; the handler reads a [`CpuMeter`] started on the
//!   engine thread and stops the script once [`Limits::cpu_ms`] is spent.
//!   The default clock, [`ThreadCpuClock`], is the thread's CPU clock where
//!   the platform has one and wall time otherwise; tests inject their own.
//!   A single builtin call (a sort of a large array, say) runs to its end
//!   between two polls, so the [`Watchdog`] bounds the run from outside:
//!   once the engine has been busy (not waiting on the host) for
//!   `cpu_ms × factor` of wall time, or its floor, the run returns
//!   [`ErrorKind::CpuLimit`] at once and the engine thread is abandoned,
//!   told to stop at its next poll. Abandoned threads still running are
//!   counted ([`QuickJs::abandoned`]); past a cap
//!   ([`QuickJs::with_abandon_cap`]) the sandbox refuses new scripts with
//!   [`ErrorKind::Unavailable`] until they end.
//! - **Memory**: the runtime's own limit (`JS_SetMemoryLimit`). The patch
//!   makes an allocation over it end the script: its error cannot be
//!   caught and every later poll interrupts. Whether that happened is
//!   asked of the runtime itself (a probe that only the patched
//!   out-of-memory state interrupts without calling the handler), never
//!   read from a thrown value. The peak is the most the runtime's
//!   allocator held, plus the arguments of queued host calls, at the points
//!   the engine measures it (before each wait on the host and at the end),
//!   or the limit itself once it was hit.
//! - **Host calls**: the call past [`Limits::max_calls`] never reaches the
//!   host and stops the script.
//!
//! A stop for CPU, calls or cancellation is QuickJS's uncatchable
//! interrupt: no `catch` or `finally` in the script runs.
//!
//! Nothing but `clax` and `console` is added to a standard QuickJS
//! context, and the prelude's lockdown removes code generation from
//! strings: no `fetch`, timers, module loader or `eval`.

use clax_bot_sandbox::{
    CODE_CANCELLED, CODE_INVALID_ARGS, CODE_SCRIPT_FINISHED, CallRecord, CancelToken, ErrorKind,
    Host, HostError, Limits, LogBuffer, PRELUDE, Sandbox, Script, ScriptError, ScriptOutcome,
    Thrown, check_body, validate_ops, wrap,
};
use futures_channel::mpsc;
use futures_util::StreamExt;
use rquickjs::context::EvalOptions;
use rquickjs::{CaughtError, Context, Ctx, Function, Object, Persistent, Runtime, Value};
use serde_json::Value as Json;
use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Starts a [`CpuMeter`] for one script, on the thread that runs it.
pub trait CpuClock: Send + Sync + 'static {
    fn start(&self) -> Box<dyn CpuMeter>;
}

/// The CPU time spent since its clock started it.
pub trait CpuMeter {
    fn elapsed(&self) -> Duration;
}

/// The calling thread's CPU clock (`cpu-time`'s `ThreadTime`), or wall time
/// on a platform that has none.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadCpuClock;

impl CpuClock for ThreadCpuClock {
    fn start(&self) -> Box<dyn CpuMeter> {
        match cpu_time::ThreadTime::try_now() {
            Ok(start) => Box::new(ThreadMeter(start)),
            Err(_) => Box::new(WallMeter(Instant::now())),
        }
    }
}

struct ThreadMeter(cpu_time::ThreadTime);

impl CpuMeter for ThreadMeter {
    fn elapsed(&self) -> Duration {
        self.0.try_elapsed().unwrap_or_default()
    }
}

struct WallMeter(Instant);

impl CpuMeter for WallMeter {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

/// How long the engine may be busy before the run gives up on it: the
/// larger of `cpu_ms × factor` and `floor`, in wall time while the engine
/// is not waiting on the host (measured with `tokio::time`, so paused test
/// time applies).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watchdog {
    pub factor: u32,
    pub floor: Duration,
}

impl Default for Watchdog {
    /// Four times the CPU budget, and at least half a second.
    fn default() -> Self {
        Watchdog {
            factor: 4,
            floor: Duration::from_millis(500),
        }
    }
}

impl Watchdog {
    fn deadline(&self, cpu_ms: u64) -> Duration {
        Duration::from_millis(cpu_ms)
            .saturating_mul(self.factor)
            .max(self.floor)
    }
}

/// Engine threads a sandbox gave up on that are still running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Abandoned {
    /// Still running now.
    pub live: usize,
    /// Abandoned since the sandbox was made.
    pub total: u64,
}

#[derive(Default)]
struct Pool {
    state: Mutex<Abandoned>,
    ended: Condvar,
}

/// The QuickJS [`Sandbox`], whose `clax` has the operations it was built
/// with. Clones share their abandoned-thread count and cap.
#[derive(Clone)]
pub struct QuickJs {
    ops: Arc<[String]>,
    op_set: Arc<HashSet<String>>,
    clock: Arc<dyn CpuClock>,
    watchdog: Watchdog,
    abandon_cap: usize,
    pool: Arc<Pool>,
}

impl std::fmt::Debug for QuickJs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickJs")
            .field("ops", &self.ops)
            .field("watchdog", &self.watchdog)
            .field("abandon_cap", &self.abandon_cap)
            .finish_non_exhaustive()
    }
}

impl QuickJs {
    /// A sandbox whose `clax` has `ops` (dotted names: `files.read` is
    /// `clax.files.read`), timed by [`ThreadCpuClock`], with the default
    /// [`Watchdog`] and a cap of 8 abandoned threads.
    ///
    /// # Panics
    ///
    /// When [`validate_ops`] refuses `ops` (a fixed list in the caller's
    /// code); [`QuickJs::try_new`] returns the error instead.
    pub fn new<I, S>(ops: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        QuickJs::try_new(ops).unwrap_or_else(|e| panic!("clax-bot-js: {e}"))
    }

    /// [`QuickJs::new`], or why `ops` are not usable.
    pub fn try_new<I, S>(ops: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ops: Vec<String> = ops.into_iter().map(Into::into).collect();
        validate_ops(&ops)?;
        Ok(QuickJs {
            op_set: Arc::new(ops.iter().cloned().collect()),
            ops: ops.into(),
            clock: Arc::new(ThreadCpuClock),
            watchdog: Watchdog::default(),
            abandon_cap: 8,
            pool: Arc::default(),
        })
    }

    /// The same sandbox, timed by `clock`.
    #[must_use]
    pub fn with_clock(self, clock: Arc<dyn CpuClock>) -> Self {
        QuickJs { clock, ..self }
    }

    /// The same sandbox, bounded by `watchdog`.
    #[must_use]
    pub fn with_watchdog(self, watchdog: Watchdog) -> Self {
        QuickJs { watchdog, ..self }
    }

    /// The same sandbox, refusing new scripts while `cap` abandoned engine
    /// threads are still running.
    #[must_use]
    pub fn with_abandon_cap(self, cap: usize) -> Self {
        QuickJs {
            abandon_cap: cap,
            ..self
        }
    }

    /// Whether the QuickJS this crate was built with is the patched one
    /// its guarantees rest on (`third_party/rquickjs-sys.patch`): the
    /// `Function` constructors refuse a body that closes the function,
    /// without running it; running out of memory cannot be caught; and the
    /// interrupt handler is polled every 100 steps. Run once per process;
    /// while it fails, every run ends with [`ErrorKind::Unavailable`] and
    /// its message, rather than running on an engine that would not hold.
    pub fn self_test() -> Result<(), String> {
        static RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        RESULT.get_or_init(|| verdict(probe_engine())).clone()
    }

    /// The engine threads this sandbox (and its clones) gave up on: how
    /// many are still running (`live`, which past the cap refuses new
    /// scripts) and how many there have been. How many runs one person may
    /// have at once is the driver's to limit.
    #[must_use]
    pub fn abandoned(&self) -> Abandoned {
        *lock(&self.pool.state)
    }

    /// Waits until at most `live` abandoned threads are still running, or
    /// `timeout` passes; whether they were.
    #[must_use]
    pub fn wait_abandoned_at_most(&self, live: usize, timeout: Duration) -> bool {
        let state = lock(&self.pool.state);
        let (state, _) = self
            .pool
            .ended
            .wait_timeout_while(state, timeout, |s| s.live > live)
            .unwrap_or_else(|e| e.into_inner());
        state.live <= live
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the engine thread shares with the run's caller.
struct RunShared {
    cancel: AtomicBool,
    cpu_us: AtomicU64,
    peak: AtomicU64,
    logs: Mutex<LogBuffer>,
    /// (abandoned, exited), changed under the pool's lock.
    fate: Mutex<(bool, bool)>,
}

/// Gives up on a run's engine: tells it to stop, and counts it while it
/// still runs.
fn abandon(pool: &Pool, run: &RunShared) {
    run.cancel.store(true, Ordering::SeqCst);
    let mut state = lock(&pool.state);
    let mut fate = lock(&run.fate);
    if !fate.0 && !fate.1 {
        fate.0 = true;
        state.live += 1;
        state.total += 1;
    }
}

/// Marks the engine thread ended, whichever way it ends.
struct ExitGuard {
    pool: Arc<Pool>,
    run: Arc<RunShared>,
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.pool.state);
        let mut fate = lock(&self.run.fate);
        fate.1 = true;
        if fate.0 {
            state.live -= 1;
            self.pool.ended.notify_all();
        }
    }
}

/// Abandons the engine if the run's future is dropped before it ends.
struct AbandonOnDrop {
    pool: Arc<Pool>,
    run: Arc<RunShared>,
    armed: bool,
}

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if self.armed {
            abandon(&self.pool, &self.run);
        }
    }
}

enum ToHost {
    Call {
        id: u32,
        op: String,
        args: Json,
        args_bytes: u64,
    },
    /// A call that will not reach the host: its record.
    Refused(CallRecord),
    Done(Box<ScriptOutcome>),
}

type Reply = (u32, Result<Json, HostError>);

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

enum Event {
    Cancel,
    Watchdog,
    Message(Option<ToHost>),
}

#[async_trait::async_trait]
impl Sandbox for QuickJs {
    async fn run(
        &self,
        script: &Script,
        host: &dyn Host,
        limits: &Limits,
        cancel: &CancelToken,
    ) -> ScriptOutcome {
        let started = Instant::now();
        if let Err(e) = limits.validate() {
            return ScriptOutcome::failed(ErrorKind::InvalidLimits, e);
        }
        if let Err(e) = QuickJs::self_test() {
            return ScriptOutcome::failed(ErrorKind::Unavailable, e);
        }
        if let Err(e) = check_body(&script.code) {
            return ScriptOutcome {
                error: Some(e.into_error()),
                wall_us: micros(started.elapsed()),
                ..ScriptOutcome::default()
            };
        }
        if cancel.is_cancelled() {
            return ScriptOutcome::failed(ErrorKind::Cancelled, "the run was cancelled");
        }
        {
            let state = lock(&self.pool.state);
            if state.live >= self.abandon_cap {
                return ScriptOutcome::failed(
                    ErrorKind::Unavailable,
                    format!(
                        "the sandbox has {} abandoned scripts still running; try again later",
                        state.live
                    ),
                );
            }
        }
        let (to_host, mut requests) = mpsc::unbounded::<ToHost>();
        let (replies, from_host) = std::sync::mpsc::channel::<Reply>();
        let run = Arc::new(RunShared {
            cancel: AtomicBool::new(false),
            cpu_us: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            logs: Mutex::new(LogBuffer::new(limits.max_log_bytes)),
            fate: Mutex::new((false, false)),
        });
        let engine = Engine {
            script: script.clone(),
            limits: *limits,
            ops: self.ops.clone(),
            op_set: self.op_set.clone(),
            clock: self.clock.clone(),
            run: run.clone(),
        };
        let exit = ExitGuard {
            pool: self.pool.clone(),
            run: run.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("clax-bot-js".into())
            .spawn(move || {
                let _exit = exit;
                let outcome = engine.run(&to_host, &from_host);
                let _ = to_host.unbounded_send(ToHost::Done(Box::new(outcome)));
            });
        if let Err(e) = spawned {
            return ScriptOutcome::failed(
                ErrorKind::Unavailable,
                format!("the sandbox could not start a thread: {e}"),
            );
        }
        let mut guard = AbandonOnDrop {
            pool: self.pool.clone(),
            run: run.clone(),
            armed: true,
        };
        let deadline = self.watchdog.deadline(limits.cpu_ms);
        let mut busy = Duration::ZERO;
        let mut calls = Vec::new();
        let stopped = |kind: ErrorKind, message: String, busy: Duration, calls: Vec<CallRecord>| {
            abandon(&self.pool, &run);
            ScriptOutcome {
                value: None,
                logs: lock(&run.logs).lines().to_vec(),
                calls,
                error: Some(ScriptError::new(kind, message)),
                cpu_us: run.cpu_us.load(Ordering::SeqCst),
                busy_us: micros(busy),
                mem_peak_bytes: run.peak.load(Ordering::SeqCst),
                wall_us: micros(started.elapsed()),
            }
        };
        let mut outcome = loop {
            let left = deadline.saturating_sub(busy);
            let waited = tokio::time::Instant::now();
            let event = tokio::select! {
                biased;
                () = cancel.cancelled() => Event::Cancel,
                m = requests.next() => Event::Message(m),
                () = tokio::time::sleep(left) => Event::Watchdog,
            };
            busy += waited.elapsed();
            match event {
                Event::Cancel => {
                    guard.armed = false;
                    return stopped(
                        ErrorKind::Cancelled,
                        "the run was cancelled".into(),
                        busy,
                        calls,
                    );
                }
                Event::Watchdog => {
                    guard.armed = false;
                    return stopped(
                        ErrorKind::CpuLimit,
                        format!(
                            "the script used more than its {} ms of CPU time (stopped from outside after {} ms busy)",
                            limits.cpu_ms,
                            busy.as_millis()
                        ),
                        busy,
                        calls,
                    );
                }
                Event::Message(Some(ToHost::Call {
                    id,
                    op,
                    args,
                    args_bytes,
                })) => {
                    let t = Instant::now();
                    let result = tokio::select! {
                        biased;
                        () = cancel.cancelled() => None,
                        r = host.call(&op, args) => Some(r),
                    };
                    let Some(result) = result else {
                        calls.push(CallRecord {
                            op,
                            ok: false,
                            code: Some(CODE_CANCELLED.into()),
                            args_bytes,
                            us: micros(t.elapsed()),
                        });
                        guard.armed = false;
                        return stopped(
                            ErrorKind::Cancelled,
                            "the run was cancelled".into(),
                            busy,
                            calls,
                        );
                    };
                    calls.push(CallRecord {
                        op,
                        ok: result.is_ok(),
                        code: result.as_ref().err().map(|e| e.code.clone()),
                        args_bytes,
                        us: micros(t.elapsed()),
                    });
                    // A send fails only once the engine has ended, when its
                    // Done is already queued.
                    let _ = replies.send((id, result));
                }
                Event::Message(Some(ToHost::Refused(record))) => calls.push(record),
                Event::Message(Some(ToHost::Done(outcome))) => break *outcome,
                Event::Message(None) => {
                    break ScriptOutcome::failed(
                        ErrorKind::Thrown,
                        "the sandbox's engine thread ended without an outcome",
                    );
                }
            }
        };
        guard.armed = false;
        outcome.calls = calls;
        outcome.busy_us = micros(busy);
        outcome.wall_us = micros(started.elapsed());
        outcome
    }
}

/// Why the engine stopped the script from outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Cpu,
    Calls,
    Cancelled,
}

/// A host call the script made that has not been sent.
enum Queued {
    Call {
        id: u32,
        op: String,
        args: Json,
        args_bytes: u64,
    },
    Refused(CallRecord),
}

/// What the script's natives share with the engine loop.
#[derive(Default)]
struct Shared {
    calls_made: u32,
    next_id: u32,
    queue: VecDeque<Queued>,
    queued_bytes: u64,
    /// `done(ok, payload)`, once the script's body has settled.
    done: Option<(bool, Option<String>)>,
}

/// The engine's own state the interrupt handler reads and writes.
#[derive(Default)]
struct Interrupt {
    stop: Cell<Option<Stop>>,
    /// Set while the engine probes the runtime for its out-of-memory
    /// state; `hit` records that the handler ran during the probe.
    probing: Cell<bool>,
    hit: Cell<bool>,
}

struct Engine {
    script: Script,
    limits: Limits,
    ops: Arc<[String]>,
    op_set: Arc<HashSet<String>>,
    clock: Arc<dyn CpuClock>,
    run: Arc<RunShared>,
}

/// The file name a script is compiled under: its title, or `script`.
fn file_name(title: &str) -> String {
    let name: String = title.chars().filter(|c| *c != '\0').collect();
    if name.trim().is_empty() {
        "script".into()
    } else {
        name
    }
}

fn eval_options(file: &str) -> EvalOptions {
    let mut options = EvalOptions::default();
    options.filename = Some(file.to_string());
    options.strict = false;
    options
}

/// The code's line named first in `stack` (`file:LINE:COL`): the source's
/// line less the wrapper's own first line (`WRAP_PREFIX`).
fn line_in(stack: &str, file: &str) -> Option<u64> {
    source_line_in(stack, file).and_then(|line: u64| line.checked_sub(1).filter(|l| *l > 0))
}

/// The source's line of `file` named first in `stack`.
fn source_line_in(stack: &str, file: &str) -> Option<u64> {
    let key = format!("{file}:");
    let mut from = 0;
    while let Some(at) = stack[from..].find(&key) {
        let rest = &stack[from + at + key.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse() {
            return Some(line);
        }
        from += at + 1;
    }
    None
}

/// A caught exception, described as the prelude would describe it.
fn thrown(err: CaughtError<'_>, file: &str) -> Thrown {
    match err {
        CaughtError::Exception(e) => {
            let obj = e.as_object();
            let line = e.stack().and_then(|s| line_in(&s, file));
            Thrown {
                name: obj
                    .get::<_, Option<String>>("name")
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                message: e.message().unwrap_or_default(),
                line,
                ..Thrown::default()
            }
        }
        CaughtError::Value(v) => Thrown {
            message: v
                .as_string()
                .and_then(|s| s.to_string().ok())
                .unwrap_or_else(|| format!("{v:?}")),
            ..Thrown::default()
        },
        CaughtError::Error(e) => Thrown {
            message: e.to_string(),
            ..Thrown::default()
        },
    }
}

/// How the script ended, before the result's checks.
type Ended = Result<Option<String>, ScriptError>;

impl Engine {
    fn run(
        self,
        to_host: &mpsc::UnboundedSender<ToHost>,
        from_host: &std::sync::mpsc::Receiver<Reply>,
    ) -> ScriptOutcome {
        let meter: Rc<dyn CpuMeter> = self.clock.start().into();
        let interrupt = Rc::new(Interrupt::default());
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut peak: u64 = 0;
        let ended = self.execute(to_host, from_host, &meter, &interrupt, &shared, &mut peak);
        // Calls still queued never reach the host.
        for queued in std::mem::take(&mut shared.borrow_mut().queue) {
            let record = match queued {
                Queued::Call { op, args_bytes, .. } => CallRecord {
                    op,
                    ok: false,
                    code: Some(CODE_SCRIPT_FINISHED.into()),
                    args_bytes,
                    us: 0,
                },
                Queued::Refused(record) => record,
            };
            let _ = to_host.unbounded_send(ToHost::Refused(record));
        }
        let cpu_us = micros(meter.elapsed());
        let ended = ended.and_then(|value| match value {
            Some(json) if json.len() as u64 > self.limits.max_result_bytes => {
                Err(ScriptError::new(
                    ErrorKind::ResultTooLarge,
                    format!(
                        "the returned value is {} bytes of JSON; the limit is {} bytes",
                        json.len(),
                        self.limits.max_result_bytes
                    ),
                ))
            }
            Some(json) => serde_json::from_str(&json).map(Some).map_err(|e| {
                ScriptError::new(
                    ErrorKind::Thrown,
                    format!("the returned value does not parse back from its JSON: {e}"),
                )
            }),
            None => Ok(None),
        });
        if matches!(&ended, Err(e) if e.kind == ErrorKind::MemoryLimit) {
            peak = peak.max(self.limits.memory_bytes);
        }
        let (value, error) = match ended {
            Ok(value) => (value, None),
            Err(e) => (None, Some(e)),
        };
        ScriptOutcome {
            value,
            logs: lock(&self.run.logs).lines().to_vec(),
            calls: Vec::new(),
            error,
            cpu_us,
            busy_us: 0,
            mem_peak_bytes: peak,
            wall_us: 0,
        }
    }

    fn limit_error(&self, stop: Stop) -> ScriptError {
        match stop {
            Stop::Cpu => ScriptError::new(
                ErrorKind::CpuLimit,
                format!(
                    "the script used more than its {} ms of CPU time",
                    self.limits.cpu_ms
                ),
            ),
            Stop::Calls => ScriptError::new(
                ErrorKind::CallLimit,
                format!(
                    "the script made more than its {} host calls",
                    self.limits.max_calls
                ),
            ),
            Stop::Cancelled => ScriptError::new(ErrorKind::Cancelled, "the run was cancelled"),
        }
    }

    fn memory_error(&self) -> ScriptError {
        ScriptError::new(
            ErrorKind::MemoryLimit,
            format!(
                "the script needed more than its {} bytes of memory",
                self.limits.memory_bytes
            ),
        )
    }

    /// Whether an allocation has failed in this runtime: the patched
    /// QuickJS then interrupts every poll before calling the handler, so a
    /// loop the handler would stop at once is stopped without it.
    fn out_of_memory(&self, ctx: &Context, interrupt: &Interrupt) -> bool {
        interrupt.probing.set(true);
        interrupt.hit.set(false);
        ctx.with(|ctx| {
            let _ = ctx.eval::<(), _>("for (;;) {}");
            let _ = ctx.catch();
        });
        interrupt.probing.set(false);
        !interrupt.hit.get()
    }

    /// The error a failure ends the run with: a stop the engine made, else
    /// the runtime having run out of memory, else what the script threw.
    fn failure(&self, ctx: &Context, interrupt: &Interrupt, t: Thrown) -> ScriptError {
        if let Some(stop) = interrupt.stop.get() {
            return self.limit_error(stop);
        }
        if self.out_of_memory(ctx, interrupt) {
            return self.memory_error();
        }
        t.into_error()
    }

    /// Runs the script; the JSON of its value (`None` for no value) or the
    /// error it ended with.
    fn execute(
        &self,
        to_host: &mpsc::UnboundedSender<ToHost>,
        from_host: &std::sync::mpsc::Receiver<Reply>,
        meter: &Rc<dyn CpuMeter>,
        interrupt: &Rc<Interrupt>,
        shared: &Rc<RefCell<Shared>>,
        peak: &mut u64,
    ) -> Ended {
        let start_err = |what: &str, e: rquickjs::Error| {
            ScriptError::new(
                ErrorKind::Unavailable,
                format!("the sandbox could not {what}: {e}"),
            )
        };
        let rt = Runtime::new().map_err(|e| start_err("start QuickJS", e))?;
        rt.set_memory_limit(usize::try_from(self.limits.memory_bytes).unwrap_or(usize::MAX));
        let budget = Duration::from_millis(self.limits.cpu_ms);
        {
            let meter = meter.clone();
            let interrupt = interrupt.clone();
            let run = self.run.clone();
            rt.set_interrupt_handler(Some(Box::new(move || {
                if interrupt.probing.get() {
                    interrupt.hit.set(true);
                    return true;
                }
                if interrupt.stop.get().is_some() {
                    return true;
                }
                if run.cancel.load(Ordering::Relaxed) {
                    interrupt.stop.set(Some(Stop::Cancelled));
                    return true;
                }
                let spent = meter.elapsed();
                run.cpu_us.store(micros(spent), Ordering::Relaxed);
                if spent > budget {
                    interrupt.stop.set(Some(Stop::Cpu));
                    return true;
                }
                false
            })));
        }
        let ctx = Context::full(&rt).map_err(|e| start_err("make a context", e))?;
        let file = file_name(&self.script.title);

        // The prelude, the body check, the script compiled, the lockdown,
        // the start.
        let settle = ctx.with(|ctx| -> Result<Persistent<Function<'static>>, Thrown> {
            let caught = |e| thrown(CaughtError::from_error(&ctx, e), &file);
            let (call, log, done) = self.natives(&ctx, interrupt, shared).map_err(caught)?;
            let prelude: Function = ctx
                .eval_with_options(PRELUDE, eval_options("clax:prelude"))
                .map_err(caught)?;
            let options = Object::new(ctx.clone()).map_err(caught)?;
            options
                .set("maxCallArgsBytes", self.limits.max_call_args_bytes as f64)
                .map_err(caught)?;
            if let Some(d) = self.script.determinism {
                options.set("nowMs", d.now_ms as f64).map_err(caught)?;
                options.set("seed", f64::from(d.seed)).map_err(caught)?;
            }
            let ops: Vec<String> = self.ops.to_vec();
            let api: Object = prelude
                .call((ops, call, log, done, options))
                .map_err(caught)?;
            let check: Function = api.get("check").map_err(caught)?;
            let lockdown: Function = api.get("lockdown").map_err(caught)?;
            let start: Function = api.get("start").map_err(caught)?;
            let settle: Function = api.get("settle").map_err(caught)?;
            if let Err(e) = check.call::<_, ()>((self.script.code.as_str(),)) {
                let mut t = caught(e);
                if t.name == "SyntaxError" {
                    // The body's own line, from the same code compiled as
                    // the script is: a body that closes the function
                    // compiles there, and keeps the check's message.
                    let wrapped: rquickjs::Result<Function> =
                        ctx.eval_with_options(wrap(&self.script.code), eval_options(&file));
                    t.line = match wrapped {
                        Err(e) => caught(e).line,
                        Ok(_) => None,
                    };
                    t.code = Some("syntax".into());
                }
                return Err(t);
            }
            let compiled: Function = ctx
                .eval_with_options(wrap(&self.script.code), eval_options(&file))
                .map_err(caught)?;
            lockdown.call::<_, ()>(()).map_err(caught)?;
            start
                .call::<_, ()>((compiled, file.as_str()))
                .map_err(caught)?;
            Ok(Persistent::save(&ctx, settle))
        });
        let settle = match settle {
            Ok(settle) => settle,
            Err(t) if t.code.as_deref() == Some("syntax") && interrupt.stop.get().is_none() => {
                let mut message = format!("SyntaxError: {}", t.message);
                if let Some(line) = t.line {
                    message.push_str(&format!(" (line {line})"));
                }
                return Err(ScriptError::new(ErrorKind::Syntax, message));
            }
            Err(t) => return Err(self.failure(&ctx, interrupt, t)),
        };

        loop {
            // Run the script's queued jobs until none is left or its body
            // has settled.
            loop {
                if shared.borrow().done.is_some() {
                    break;
                }
                match rt.execute_pending_job() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(job) => {
                        let t = job.0.with(|ctx| {
                            thrown(
                                CaughtError::from_error(&ctx, rquickjs::Error::Exception),
                                &file,
                            )
                        });
                        return Err(self.failure(&ctx, interrupt, t));
                    }
                }
            }
            // Measuring walks the whole heap, so it happens once per wait
            // on the host and at the end, not after every job.
            let queued_bytes = shared.borrow().queued_bytes;
            *peak = (*peak).max(heap_bytes(&rt).saturating_add(queued_bytes));
            self.run.peak.store(*peak, Ordering::SeqCst);
            self.run
                .cpu_us
                .store(micros(meter.elapsed()), Ordering::SeqCst);
            if let Some(stop) = interrupt.stop.get() {
                return Err(self.limit_error(stop));
            }
            if meter.elapsed() > budget {
                return Err(self.limit_error(Stop::Cpu));
            }
            if self.out_of_memory(&ctx, interrupt) {
                return Err(self.memory_error());
            }
            let done = shared.borrow_mut().done.take();
            if let Some((ok, payload)) = done {
                if ok {
                    return Ok(payload);
                }
                let t: Thrown = payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p).ok())
                    .unwrap_or_default();
                return Err(t.into_error());
            }
            // Send what the script queued, up to its first call.
            let next = loop {
                let item = shared.borrow_mut().queue.pop_front();
                match item {
                    Some(Queued::Refused(record)) => {
                        let _ = to_host.unbounded_send(ToHost::Refused(record));
                    }
                    Some(Queued::Call {
                        id,
                        op,
                        args,
                        args_bytes,
                    }) => break Some((id, op, args, args_bytes)),
                    None => break None,
                }
            };
            let Some((id, op, args, args_bytes)) = next else {
                return Err(ScriptError::new(
                    ErrorKind::Thrown,
                    "the script awaited a promise that never settles",
                ));
            };
            shared.borrow_mut().queued_bytes -= args_bytes;
            if to_host
                .unbounded_send(ToHost::Call {
                    id,
                    op,
                    args,
                    args_bytes,
                })
                .is_err()
            {
                return Err(self.limit_error(Stop::Cancelled));
            }
            let Ok((id, result)) = from_host.recv() else {
                return Err(self.limit_error(Stop::Cancelled));
            };
            let (ok, json) = match result {
                Ok(v) => (true, v.to_string()),
                Err(e) => (
                    false,
                    serde_json::json!({"code": e.code, "message": e.message}).to_string(),
                ),
            };
            let settled = ctx.with(|ctx| -> Result<(), Thrown> {
                let caught = |e| thrown(CaughtError::from_error(&ctx, e), &file);
                let settle = settle.clone().restore(&ctx).map_err(caught)?;
                settle.call::<_, ()>((id, ok, json)).map_err(caught)
            });
            if let Err(t) = settled {
                return Err(self.failure(&ctx, interrupt, t));
            }
        }
    }

    /// The natives the prelude takes: `call`, `log` and `done`.
    fn natives<'js>(
        &self,
        ctx: &Ctx<'js>,
        interrupt: &Rc<Interrupt>,
        shared: &Rc<RefCell<Shared>>,
    ) -> rquickjs::Result<(Function<'js>, Function<'js>, Function<'js>)> {
        let call = {
            let shared = shared.clone();
            let interrupt = interrupt.clone();
            let op_set = self.op_set.clone();
            let max_calls = self.limits.max_calls;
            let max_bytes = self.limits.max_call_args_bytes;
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, op: String, args: Value<'js>| -> rquickjs::Result<u32> {
                    let mut shared = shared.borrow_mut();
                    if !op_set.contains(&op) {
                        return Err(rquickjs::Exception::throw_type(
                            &ctx,
                            &format!("no operation {op:?}"),
                        ));
                    }
                    if shared.done.is_some() {
                        return Err(rquickjs::Exception::throw_type(
                            &ctx,
                            "the script has finished; this call was not made",
                        ));
                    }
                    if shared.calls_made >= max_calls {
                        interrupt.stop.set(Some(Stop::Calls));
                        return Err(rquickjs::Exception::throw_internal(
                            &ctx,
                            &format!("the script made more than its {max_calls} host calls"),
                        ));
                    }
                    shared.calls_made += 1;
                    let text = args.as_string().map(rquickjs::String::to_string);
                    let checked: Result<(Json, u64), (String, u64)> = match text {
                        Some(Ok(text)) => {
                            let bytes = text.len() as u64;
                            if bytes > max_bytes {
                                Err((
                                    format!(
                                        "the arguments of {op} are {bytes} bytes of JSON; the limit is {max_bytes}"
                                    ),
                                    bytes,
                                ))
                            } else {
                                match serde_json::from_str::<Json>(&text) {
                                    Ok(v) if v.is_array() => Ok((v, bytes)),
                                    Ok(_) => Err((
                                        format!(
                                            "the arguments of {op} do not serialise as an array"
                                        ),
                                        bytes,
                                    )),
                                    Err(e) => Err((
                                        format!(
                                            "the arguments of {op} do not parse back from their JSON: {e}"
                                        ),
                                        bytes,
                                    )),
                                }
                            }
                        }
                        Some(Err(_)) => {
                            Err((format!("the arguments of {op} are not valid text"), 0))
                        }
                        None => match args.as_number() {
                            Some(n) => Err((
                                format!(
                                    "the arguments of {op} are at least {n} bytes of JSON; the limit is {max_bytes}"
                                ),
                                n as u64,
                            )),
                            None => Err((format!("the arguments of {op} have no JSON"), 0)),
                        },
                    };
                    match checked {
                        Ok((args, args_bytes)) => {
                            let id = shared.next_id;
                            shared.next_id += 1;
                            shared.queued_bytes += args_bytes;
                            shared.queue.push_back(Queued::Call {
                                id,
                                op,
                                args,
                                args_bytes,
                            });
                            Ok(id)
                        }
                        Err((message, args_bytes)) => {
                            shared.queue.push_back(Queued::Refused(CallRecord {
                                op,
                                ok: false,
                                code: Some(CODE_INVALID_ARGS.into()),
                                args_bytes,
                                us: 0,
                            }));
                            Err(rquickjs::Exception::throw_type(&ctx, &message))
                        }
                    }
                },
            )?
        };
        let log = {
            let run = self.run.clone();
            Function::new(ctx.clone(), move |line: Value<'js>| {
                let line = line
                    .as_string()
                    .and_then(|s| s.to_string().ok())
                    .unwrap_or_else(|| "[unprintable]".into());
                lock(&run.logs).push(line);
            })?
        };
        let done = {
            let shared = shared.clone();
            Function::new(ctx.clone(), move |ok: bool, payload: Option<String>| {
                let mut shared = shared.borrow_mut();
                if shared.done.is_none() {
                    shared.done = Some((ok, payload));
                }
            })?
        };
        Ok((call, log, done))
    }
}

/// The bytes the runtime's allocator holds.
fn heap_bytes(rt: &Runtime) -> u64 {
    u64::try_from(rt.memory_usage().malloc_size).unwrap_or(0)
}

/// What [`QuickJs::self_test`] observed of the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EngineFacts {
    /// `new AsyncFunction("}), … ({")` threw a SyntaxError and ran nothing.
    body_check_refuses_breakouts: bool,
    /// An allocation bomb inside `try` ended the script, its `catch` unrun.
    out_of_memory_is_uncatchable: bool,
    /// How often the interrupt handler ran during a 10,000-step loop.
    polls_in_ten_thousand_steps: u32,
}

/// The engine probed in fresh runtimes of its own.
fn probe_engine() -> EngineFacts {
    let body_check_refuses_breakouts = (|| {
        let rt = Runtime::new().ok()?;
        let ctx = Context::full(&rt).ok()?;
        ctx.with(|ctx| {
            let code = r#"
                const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
                let refused = false;
                try { new AsyncFunction("}), globalThis.ran = 1, ({") } catch (e) { refused = e instanceof SyntaxError }
                refused && globalThis.ran === undefined"#;
            ctx.eval::<bool, _>(code).ok()
        })
    })()
    .unwrap_or(false);
    let out_of_memory_is_uncatchable = (|| {
        let rt = Runtime::new().ok()?;
        rt.set_memory_limit(4 << 20);
        let ctx = Context::full(&rt).ok()?;
        ctx.with(|ctx| {
            let code = "let caught = false; try { const a = []; for (;;) a.push(new Array(1000).fill(1)) } catch (e) { caught = true } caught";
            let ended = ctx.eval::<bool, _>(code).is_err();
            let _ = ctx.catch();
            Some(ended)
        })
    })()
    .unwrap_or(false);
    let polls_in_ten_thousand_steps = (|| {
        let rt = Runtime::new().ok()?;
        let polls = Rc::new(Cell::new(0u32));
        {
            let polls = polls.clone();
            rt.set_interrupt_handler(Some(Box::new(move || {
                polls.set(polls.get() + 1);
                false
            })));
        }
        let ctx = Context::full(&rt).ok()?;
        ctx.with(|ctx| ctx.eval::<(), _>("for (let i = 0; i < 10000; i++) {}").ok())?;
        Some(polls.get())
    })()
    .unwrap_or(0);
    EngineFacts {
        body_check_refuses_breakouts,
        out_of_memory_is_uncatchable,
        polls_in_ten_thousand_steps,
    }
}

/// Whether `facts` are those of the patched engine; what is missing if not.
fn verdict(facts: EngineFacts) -> Result<(), String> {
    let mut missing = Vec::new();
    if !facts.body_check_refuses_breakouts {
        missing.push("its Function constructors run a body that closes the function");
    }
    if !facts.out_of_memory_is_uncatchable {
        missing.push("a script can catch running out of memory");
    }
    if facts.polls_in_ten_thousand_steps < 50 {
        missing.push("it polls the interrupt handler too rarely to hold a CPU budget");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "this build of QuickJS is not the patched one the sandbox needs \
             (third_party/rquickjs-sys.patch, scripts/vendor-rquickjs.sh): {}",
            missing.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_found_after_the_file_name() {
        let stack = "    at <anonymous> (clax:prelude:3:1)\n    at f (my script:12:5)\n";
        assert_eq!(line_in(stack, "my script"), Some(11));
        assert_eq!(line_in("at f (s:1:3)", "s"), None);
        assert_eq!(line_in(stack, "other"), None);
        assert_eq!(line_in("x:y:7", "x"), None);
    }

    #[test]
    fn a_script_without_a_title_is_named_script() {
        assert_eq!(file_name("  "), "script");
        assert_eq!(file_name("a\0b"), "ab");
    }

    #[test]
    fn the_watchdog_is_a_multiple_of_the_budget_with_a_floor() {
        let w = Watchdog {
            factor: 4,
            floor: Duration::from_millis(500),
        };
        assert_eq!(w.deadline(100), Duration::from_millis(500));
        assert_eq!(w.deadline(2_000), Duration::from_millis(8_000));
    }

    #[test]
    fn this_build_passes_its_engine_self_test() {
        assert_eq!(QuickJs::self_test(), Ok(()));
        let facts = probe_engine();
        assert!(facts.polls_in_ten_thousand_steps >= 50, "{facts:?}");
    }

    #[test]
    fn an_engine_without_the_patch_fails_the_self_test_closed() {
        let stock = EngineFacts {
            body_check_refuses_breakouts: false,
            out_of_memory_is_uncatchable: false,
            polls_in_ten_thousand_steps: 1,
        };
        let err = verdict(stock).unwrap_err();
        assert!(err.contains("closes the function"), "{err}");
        assert!(err.contains("catch running out of memory"), "{err}");
        assert!(err.contains("too rarely"), "{err}");
        let patched = EngineFacts {
            body_check_refuses_breakouts: true,
            out_of_memory_is_uncatchable: true,
            polls_in_ten_thousand_steps: 200,
        };
        assert_eq!(verdict(patched), Ok(()));
    }

    #[test]
    fn unusable_op_names_are_refused() {
        assert!(QuickJs::try_new(["__proto__.polluted"]).is_err());
        assert!(QuickJs::try_new(["files", "files.read"]).is_err());
        assert!(QuickJs::try_new(["files.read", "echo"]).is_ok());
    }
}
