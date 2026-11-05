use async_trait::async_trait;
use clax_bot_js::{Abandoned, CpuClock, CpuMeter, QuickJs, Watchdog};
use clax_bot_sandbox::conformance::{self, OPS};
use clax_bot_sandbox::{
    CODE_SCRIPT_FINISHED, CancelToken, ErrorKind, Host, HostError, Limits, Sandbox, Script,
    ScriptOutcome,
};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A bound on waiting for an engine thread to end: generous, as only a
/// condition ends the wait.
const THREAD_END: Duration = Duration::from_secs(60);

/// A clock whose every reading is `step` later than the one before, so a
/// script's CPU time is the number of times the engine looked at it.
struct TickClock {
    step: Duration,
}

struct TickMeter {
    step: Duration,
    reads: Cell<u32>,
}

impl CpuClock for TickClock {
    fn start(&self) -> Box<dyn CpuMeter> {
        Box::new(TickMeter {
            step: self.step,
            reads: Cell::new(0),
        })
    }
}

impl CpuMeter for TickMeter {
    fn elapsed(&self) -> Duration {
        self.reads.set(self.reads.get() + 1);
        self.step * self.reads.get()
    }
}

fn still() -> Arc<dyn CpuClock> {
    Arc::new(TickClock {
        step: Duration::ZERO,
    })
}

/// A clock that never advances and holds the engine at its first reading
/// until opened: an engine stuck where no poll can reach it.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}

struct GateMeter(Gate);

impl CpuClock for Gate {
    fn start(&self) -> Box<dyn CpuMeter> {
        Box::new(GateMeter(self.clone()))
    }
}

impl CpuMeter for GateMeter {
    fn elapsed(&self) -> Duration {
        let (open, cv) = &*self.0.0;
        let guard = open.lock().unwrap();
        let _open = cv.wait_while(guard, |open| !*open).unwrap();
        Duration::ZERO
    }
}

/// Answers every call with its arguments, and remembers the calls.
#[derive(Default)]
struct EchoHost {
    seen: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl Host for EchoHost {
    async fn call(&self, op: &str, args: Value) -> Result<Value, HostError> {
        self.seen
            .lock()
            .unwrap()
            .push((op.to_string(), args.clone()));
        Ok(args)
    }
}

async fn run(sandbox: &QuickJs, code: &str, limits: Limits) -> ScriptOutcome {
    sandbox
        .run(
            &Script::new("test", code),
            &EchoHost::default(),
            &limits,
            &CancelToken::new(),
        )
        .await
}

fn kind(outcome: &ScriptOutcome) -> Option<ErrorKind> {
    outcome.error.as_ref().map(|e| e.kind)
}

fn sandbox() -> QuickJs {
    QuickJs::new(OPS.iter().copied())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_conformance_suite_passes() {
    let sandbox = sandbox();
    let failures: Vec<String> = conformance::run_all(&sandbox)
        .await
        .into_iter()
        .filter_map(|(name, r)| r.err().map(|e| format!("{name}: {e}")))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn an_infinite_loop_runs_out_of_cpu_on_an_injected_clock() {
    let sandbox = sandbox().with_clock(Arc::new(TickClock {
        step: Duration::from_millis(1),
    }));
    let outcome = run(
        &sandbox,
        "for (;;) {}",
        Limits {
            cpu_ms: 50,
            ..Limits::default()
        },
    )
    .await;
    assert_eq!(kind(&outcome), Some(ErrorKind::CpuLimit), "{outcome:?}");
    assert!(outcome.error.unwrap().message.contains("50 ms"));
    assert!(outcome.cpu_us > 50_000, "{}", outcome.cpu_us);
}

#[tokio::test]
async fn a_clock_that_stands_still_never_stops_a_script() {
    let sandbox = sandbox().with_clock(still()).with_watchdog(Watchdog {
        factor: 1,
        floor: Duration::from_secs(3600),
    });
    let outcome = run(
        &sandbox,
        "let n = 0; for (let i = 0; i < 300000; i++) n += i; return n",
        Limits {
            cpu_ms: 1,
            ..Limits::default()
        },
    )
    .await;
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.value, Some(json!(44_999_850_000u64)));
}

#[tokio::test(start_paused = true)]
async fn the_watchdog_ends_a_run_whose_engine_does_not_stop_and_abandons_it() {
    // A clock that stands still: the engine's own CPU check never fires,
    // as for a builtin that runs between two polls. Paused time runs the
    // watchdog's timer out as soon as the runtime is idle.
    let sandbox = sandbox().with_clock(still());
    let outcome = run(
        &sandbox,
        "for (;;) {}",
        Limits {
            cpu_ms: 100,
            ..Limits::default()
        },
    )
    .await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::CpuLimit);
    assert!(
        error.message.contains("stopped from outside"),
        "{}",
        error.message
    );
    assert!(outcome.busy_us >= 500_000, "{}", outcome.busy_us);
    // Billing takes the engine's own figure: a clock that stands still.
    assert_eq!(outcome.cpu_us, 0);
    assert_eq!(sandbox.abandoned().total, 1);
    assert!(sandbox.wait_abandoned_at_most(0, THREAD_END));
    assert_eq!(sandbox.abandoned(), Abandoned { live: 0, total: 1 });
}

/// The patched engine's 100-step poll, not the watchdog, stops a loop whose
/// body is two heavy builtin calls: stock QuickJS polls every 10,000 steps,
/// and ran this loop to some 19 s of CPU.
#[tokio::test]
async fn the_engine_poll_stops_a_loop_of_heavy_builtins() {
    let sandbox = sandbox().with_watchdog(Watchdog {
        factor: 1,
        floor: Duration::from_secs(3600),
    });
    let code = "const a = Array.from({length: 400000}, (_, i) => String((i * 7919) % 400000));\nfor (let k = 0; k < 300; k++) { a.sort(); a.reverse() }\nreturn 1";
    let outcome = run(
        &sandbox,
        code,
        Limits {
            cpu_ms: 100,
            ..Limits::default()
        },
    )
    .await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::CpuLimit);
    assert!(
        !error.message.contains("stopped from outside"),
        "{}",
        error.message
    );
    assert!(outcome.cpu_us < 2_000_000, "{}", outcome.cpu_us);
}

#[tokio::test(start_paused = true)]
async fn past_its_cap_of_abandoned_threads_the_sandbox_refuses_scripts() {
    let gate = Gate::default();
    let sandbox = sandbox()
        .with_clock(Arc::new(gate.clone()))
        .with_abandon_cap(1);
    let stuck = run(&sandbox, "for (;;) {}", Limits::default()).await;
    assert_eq!(kind(&stuck), Some(ErrorKind::CpuLimit));
    assert_eq!(sandbox.abandoned(), Abandoned { live: 1, total: 1 });
    let refused = run(&sandbox, "return 1", Limits::default()).await;
    assert_eq!(kind(&refused), Some(ErrorKind::Unavailable), "{refused:?}");
    gate.open();
    assert!(sandbox.wait_abandoned_at_most(0, THREAD_END));
    // Real time again, so the watchdog gives the next run its due.
    tokio::time::resume();
    let fine = run(&sandbox, "return 1", Limits::default()).await;
    assert_eq!(fine.value, Some(json!(1)), "{fine:?}");
}

/// Cancels the run's token from inside its first host call.
struct CancelOnCall(CancelToken);

#[async_trait]
impl Host for CancelOnCall {
    async fn call(&self, _op: &str, args: Value) -> Result<Value, HostError> {
        self.0.cancel();
        Ok(args)
    }
}

#[tokio::test]
async fn a_cancelled_run_returns_what_it_did_so_far() {
    let sandbox = sandbox().with_clock(still()).with_watchdog(Watchdog {
        factor: 1,
        floor: Duration::from_secs(3600),
    });
    let cancel = CancelToken::new();
    let host = CancelOnCall(cancel.clone());
    let code = "console.log('before'); await clax.echo(1); for (;;) {}";
    let outcome = sandbox
        .run(&Script::new("t", code), &host, &Limits::default(), &cancel)
        .await;
    assert_eq!(kind(&outcome), Some(ErrorKind::Cancelled), "{outcome:?}");
    assert_eq!(outcome.logs, vec!["before".to_string()]);
    assert_eq!(outcome.calls.len(), 1);
    assert!(outcome.calls[0].ok);
    assert!(outcome.wall_us > 0);
    assert!(sandbox.wait_abandoned_at_most(0, THREAD_END));
}

#[tokio::test(start_paused = true)]
async fn dropping_a_run_stops_its_engine() {
    let sandbox = sandbox().with_clock(still()).with_watchdog(Watchdog {
        factor: 1,
        floor: Duration::from_secs(3600),
    });
    let host = EchoHost::default();
    let limits = Limits::default();
    let cancel = CancelToken::new();
    let script = Script::new("t", "for (;;) {}");
    let run = sandbox.run(&script, &host, &limits, &cancel);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .is_err()
    );
    assert_eq!(sandbox.abandoned().total, 1);
    assert!(sandbox.wait_abandoned_at_most(0, THREAD_END));
}

#[tokio::test]
async fn invalid_limits_run_nothing() {
    let host = EchoHost::default();
    for limits in [
        Limits {
            memory_bytes: 0,
            ..Limits::default()
        },
        Limits {
            memory_bytes: 50_000,
            ..Limits::default()
        },
        Limits {
            cpu_ms: 0,
            ..Limits::default()
        },
        Limits {
            max_calls: 0,
            ..Limits::default()
        },
        Limits {
            max_call_args_bytes: 1,
            ..Limits::default()
        },
    ] {
        let outcome = sandbox()
            .run(
                &Script::new("t", "await clax.echo(1)"),
                &host,
                &limits,
                &CancelToken::new(),
            )
            .await;
        assert_eq!(kind(&outcome), Some(ErrorKind::InvalidLimits), "{limits:?}");
    }
    assert!(host.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_allocation_bomb_runs_out_of_memory() {
    let limits = Limits {
        memory_bytes: 8 << 20,
        ..Limits::default()
    };
    let outcome = run(
        &sandbox(),
        "const keep = []; for (;;) keep.push({n: keep.length, s: 'x' + keep.length})",
        limits,
    )
    .await;
    assert_eq!(kind(&outcome), Some(ErrorKind::MemoryLimit), "{outcome:?}");
    assert!(outcome.mem_peak_bytes >= 8 << 20);
}

#[tokio::test]
async fn memory_is_measured_with_arguments_in_flight() {
    let outcome = run(
        &sandbox(),
        "const a = new Array(100000).fill(1); await clax.echo('y'.repeat(200000)); return a.length",
        Limits::default(),
    )
    .await;
    assert_eq!(outcome.value, Some(json!(100000)));
    assert!(
        outcome.mem_peak_bytes > 1_000_000,
        "{}",
        outcome.mem_peak_bytes
    );
    assert_eq!(outcome.calls[0].args_bytes, 200_004);
}

#[tokio::test]
async fn the_call_past_the_limit_never_reaches_the_host() {
    let host = EchoHost::default();
    let limits = Limits {
        max_calls: 3,
        ..Limits::default()
    };
    let outcome = sandbox()
        .run(
            &Script::new("calls", "for (let i = 0; ; i++) await clax.echo(i)"),
            &host,
            &limits,
            &CancelToken::new(),
        )
        .await;
    assert_eq!(kind(&outcome), Some(ErrorKind::CallLimit), "{outcome:?}");
    assert_eq!(outcome.calls.len(), 3);
    let seen = host.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2], ("echo".to_string(), json!([2])));
}

#[tokio::test]
async fn refused_arguments_never_reach_the_host() {
    let host = EchoHost::default();
    let code = "let a = [0]; for (let i = 0; i < 17; i++) a = [a, a];\ntry { await clax.echo(a) } catch (e) { return [e.name, e.message] }";
    let outcome = sandbox()
        .run(
            &Script::new("t", code),
            &host,
            &Limits::default(),
            &CancelToken::new(),
        )
        .await;
    let value = outcome
        .value
        .clone()
        .unwrap_or_else(|| panic!("{outcome:?}"));
    assert_eq!(value[0], "TypeError");
    assert!(
        value[1].as_str().unwrap().contains("limit is 262144"),
        "{value}"
    );
    assert!(host.seen.lock().unwrap().is_empty());
    assert_eq!(outcome.calls[0].code.as_deref(), Some("invalid_args"));
}

#[tokio::test]
async fn a_call_left_queued_at_the_end_is_recorded_not_made() {
    let host = EchoHost::default();
    let outcome = sandbox()
        .run(
            &Script::new("t", "clax.echo('a'); clax.echo('b'); return 0"),
            &host,
            &Limits::default(),
            &CancelToken::new(),
        )
        .await;
    assert_eq!(outcome.value, Some(json!(0)));
    assert!(host.seen.lock().unwrap().is_empty());
    let codes: Vec<_> = outcome.calls.iter().map(|c| c.code.as_deref()).collect();
    assert_eq!(
        codes,
        vec![Some(CODE_SCRIPT_FINISHED), Some(CODE_SCRIPT_FINISHED)]
    );
}

#[tokio::test]
async fn a_result_too_large_is_refused() {
    let limits = Limits {
        max_result_bytes: 1000,
        ..Limits::default()
    };
    let outcome = run(&sandbox(), "return {big: 'y'.repeat(2000)}", limits).await;
    assert_eq!(kind(&outcome), Some(ErrorKind::ResultTooLarge));
    assert_eq!(outcome.value, None);
}

#[tokio::test]
async fn a_syntax_error_names_its_line() {
    let outcome = run(&sandbox(), "const a = 1;\n\nlet = = 3;", Limits::default()).await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert!(
        error.message.starts_with("SyntaxError: "),
        "{}",
        error.message
    );
    assert!(error.message.ends_with("(line 3)"), "{}", error.message);
}

#[tokio::test]
async fn a_thrown_error_names_its_line() {
    let outcome = run(
        &sandbox(),
        "await clax.echo(1);\nfunction f() {\n  throw new RangeError('deep')\n}\nf()",
        Limits::default(),
    )
    .await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::Thrown);
    assert_eq!(error.message, "RangeError: deep (line 3)");
}

#[tokio::test]
async fn a_host_error_is_a_rejection_the_script_can_catch() {
    struct Failing;
    #[async_trait]
    impl Host for Failing {
        async fn call(&self, _op: &str, _args: Value) -> Result<Value, HostError> {
            Err(HostError::new("forbidden", "not yours"))
        }
    }
    let code = "let caught; try { await clax.files.read('x') } catch (e) { caught = [e.name, e.code, e.message] }\nawait clax.db.get('c', 'd')";
    let outcome = sandbox()
        .run(
            &Script::new("t", code),
            &Failing,
            &Limits::default(),
            &CancelToken::new(),
        )
        .await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::HostError);
    assert_eq!(error.message, "HostError forbidden: not yours");
    let records: Vec<_> = outcome
        .calls
        .iter()
        .map(|c| (c.op.as_str(), c.ok, c.code.as_deref()))
        .collect();
    assert_eq!(
        records,
        vec![
            ("files.read", false, Some("forbidden")),
            ("db.get", false, Some("forbidden"))
        ]
    );
}

#[tokio::test]
async fn logs_are_capped() {
    let limits = Limits {
        max_log_bytes: 100,
        ..Limits::default()
    };
    let outcome = run(
        &sandbox(),
        "for (let i = 0; i < 1000; i++) console.log('line ' + i); return 1",
        limits,
    )
    .await;
    assert_eq!(outcome.value, Some(json!(1)));
    assert_eq!(
        outcome.logs.last().unwrap(),
        clax_bot_sandbox::LOGS_TRUNCATED
    );
    let kept: usize = outcome.logs[..outcome.logs.len() - 1]
        .iter()
        .map(String::len)
        .sum();
    assert!(kept <= 100, "{kept}");
    assert_eq!(outcome.logs.len(), 16, "{:?}", outcome.logs);
}

#[tokio::test]
async fn a_promise_that_never_settles_ends_the_script() {
    let outcome = run(&sandbox(), "await new Promise(() => {})", Limits::default()).await;
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, ErrorKind::Thrown);
    assert!(error.message.contains("never settles"));
}

/// Holds every call until two scripts are both waiting on one.
struct Meeting {
    barrier: tokio::sync::Barrier,
}

#[async_trait]
impl Host for Meeting {
    async fn call(&self, _op: &str, args: Value) -> Result<Value, HostError> {
        self.barrier.wait().await;
        Ok(args)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_scripts_are_isolated() {
    let sandbox = sandbox();
    let host = Meeting {
        barrier: tokio::sync::Barrier::new(2),
    };
    let code = |name: &str| {
        format!(
            "const before = typeof globalThis.mark;\nglobalThis.mark = '{name}';\nObject.prototype.polluted = '{name}';\nawait clax.echo('{name}');\nreturn [before, globalThis.mark, ({{}}).polluted]"
        )
    };
    let (a, b) = (Script::new("a", code("a")), Script::new("b", code("b")));
    let limits = Limits::default();
    let (ca, cb) = (CancelToken::new(), CancelToken::new());
    let (ra, rb) = tokio::join!(
        sandbox.run(&a, &host, &limits, &ca),
        sandbox.run(&b, &host, &limits, &cb)
    );
    assert_eq!(ra.value, Some(json!(["undefined", "a", "a"])), "{ra:?}");
    assert_eq!(rb.value, Some(json!(["undefined", "b", "b"])), "{rb:?}");
}

#[tokio::test]
async fn an_outcome_reports_its_figures() {
    let outcome = run(
        &sandbox(),
        "await clax.echo(1); return 2",
        Limits::default(),
    )
    .await;
    assert_eq!(outcome.value, Some(json!(2)), "{outcome:?}");
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(outcome.calls[0].args_bytes, 3);
    assert!(outcome.mem_peak_bytes > 0);
    assert!(outcome.wall_us > 0);
}

#[tokio::test]
async fn a_huge_argument_never_reaches_the_host() {
    let host = EchoHost::default();
    let outcome = sandbox()
        .run(
            &Script::new("t", "await clax.echo('x'.repeat(25e6)); return 0"),
            &host,
            &Limits::default(),
            &CancelToken::new(),
        )
        .await;
    assert!(
        matches!(
            kind(&outcome),
            Some(ErrorKind::MemoryLimit | ErrorKind::Thrown)
        ),
        "{outcome:?}"
    );
    assert!(host.seen.lock().unwrap().is_empty());
}
