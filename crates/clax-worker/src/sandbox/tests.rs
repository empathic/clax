use super::*;
use clax_bot_sandbox::{CODE_SCRIPT_FINISHED, HostError, check_body, conformance};

fn state(limits: Limits) -> RunState {
    let ops: HashSet<String> = conformance::OPS.iter().map(|s| s.to_string()).collect();
    RunState::new(&ops, limits)
}

fn call_msg(op: &str, args: &str) -> String {
    json!({"records": [], "logs": [], "call": {"op": op, "args": args, "bytes": args.len()}})
        .to_string()
}

#[test]
fn a_title_becomes_a_module_name_that_is_no_specifier_of_another_module() {
    assert_eq!(module_name("my script"), "my script");
    assert_eq!(module_name("node:fs"), "node_fs");
    assert_eq!(module_name("../x/y"), "_._x_y");
    assert_eq!(module_name(".hidden"), "_hidden");
    assert_eq!(module_name("  "), "script");
    assert_eq!(module_name(DRIVER_MODULE), "script");
    assert_eq!(module_name(PRELUDE_MODULE), "script");
    assert_eq!(module_name("a\u{e9}b\0"), "a_b_");
}

#[test]
fn the_script_module_puts_code_line_n_on_module_line_n_plus_one() {
    let code = "const a = 1;\nthrow new Error('x')";
    let module = script_module(code);
    assert!(module.contains(&wrap(code)), "{module}");
    let lines: Vec<&str> = module.lines().collect();
    assert_eq!(lines[1], "const a = 1;");
    assert_eq!(lines[2], "throw new Error('x')");
    assert!(lines[0].ends_with(clax_bot_sandbox::WRAP_PREFIX.trim_end()));
}

#[test]
fn the_driver_imports_the_script_module_by_its_quoted_name() {
    let driver = driver_module("my \"script\"");
    assert!(driver.starts_with("import make from \"my \\\"script\\\"\";\n"));
    assert!(prelude_module().starts_with("module.exports = "));
    assert!(prelude_module().ends_with(PRELUDE));
}

#[test]
fn the_drivers_depth_limit_is_serde_jsons() {
    let depth: usize = DRIVER_JS
        .lines()
        .find_map(|l| l.strip_prefix("const MAX_DEPTH = "))
        .and_then(|v| v.trim_end_matches(';').parse().ok())
        .expect("driver.js names MAX_DEPTH");
    let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(serde_json::from_str::<Value>(&nested(depth)).is_ok());
    assert!(serde_json::from_str::<Value>(&nested(depth + 1)).is_err());
}

#[test]
fn serde_json_refuses_exactly_the_escapes_the_driver_refuses() {
    for bad in [r#"["\ud800"]"#, r#"["\udc00"]"#, r#"["\ud800A"]"#] {
        assert!(serde_json::from_str::<Value>(bad).is_err(), "{bad}");
    }
    assert!(serde_json::from_str::<Value>(r#"["😀"]"#).is_ok());
}

#[test]
fn a_call_reaches_the_host_and_its_answer_is_recorded() {
    let mut s = state(Limits::default());
    let Step::Call {
        op,
        args,
        args_bytes,
    } = s.take(&call_msg("files.read", r#"["a"]"#))
    else {
        panic!("not a call");
    };
    assert_eq!(
        (op.as_str(), &args, args_bytes),
        ("files.read", &json!(["a"]), 5)
    );
    let answer = s.answer(op, args_bytes, 7, &Err(HostError::new("not_found", "no")));
    let answer: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["ok"], false);
    assert_eq!(
        serde_json::from_str::<Value>(answer["json"].as_str().unwrap()).unwrap(),
        json!({"code": "not_found", "message": "no"})
    );
    let outcome = s.outcome(End::Cancelled, 3, 9);
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(outcome.calls[0].code.as_deref(), Some("not_found"));
    assert_eq!(outcome.calls[0].us, 7);
    assert_eq!(
        (outcome.cpu_us, outcome.busy_us, outcome.wall_us),
        (3, 3, 9)
    );
}

#[test]
fn the_instance_checks_every_call_whatever_the_isolate_claims() {
    let limits = Limits {
        max_calls: 4,
        max_call_args_bytes: 64,
        ..Limits::default()
    };
    let mut s = state(limits);
    for (op, args) in [
        ("nope", "[1]"),
        ("echo", &format!("[\"{}\"]", "x".repeat(70))),
        ("echo", "{}"),
        ("echo", "[1"),
    ] {
        assert!(
            matches!(s.take(&call_msg(op, args)), Step::Reply(_)),
            "{op} {args}"
        );
    }
    assert_eq!(s.take(&call_msg("echo", "[1]")), Step::End(End::Calls));
    let outcome = s.outcome(End::Calls, 0, 0);
    assert!(
        outcome
            .calls
            .iter()
            .all(|c| c.code.as_deref() == Some(CODE_INVALID_ARGS))
    );
    assert_eq!(outcome.calls.len(), 4);
    assert_eq!(outcome.error.unwrap().kind, ErrorKind::CallLimit);
}

#[test]
fn records_and_logs_keep_the_order_the_isolate_sent_them_in() {
    let mut s = state(Limits {
        max_log_bytes: 64,
        ..Limits::default()
    });
    let first = json!({
        "records": [{"op": "echo", "code": "invalid_args", "bytes": 84}],
        "logs": ["a".repeat(40)],
        "call": {"op": "echo", "args": "[1]", "bytes": 3},
    })
    .to_string();
    assert!(matches!(s.take(&first), Step::Call { .. }));
    let _ = s.answer("echo".into(), 3, 1, &Ok(json!(1)));
    let end = json!({"final": {
        "records": [{"op": "db.get", "code": CODE_SCRIPT_FINISHED, "bytes": 2}],
        "logs": ["b".repeat(40), "c"],
        "done": {"ok": true, "payload": "[1,2]"},
    }})
    .to_string();
    let Step::End(end) = s.take(&end) else {
        panic!("not the end")
    };
    let outcome = s.outcome(end, 0, 0);
    assert_eq!(outcome.value, Some(json!([1, 2])));
    let made: Vec<_> = outcome
        .calls
        .iter()
        .map(|c| (c.op.as_str(), c.ok, c.code.as_deref(), c.args_bytes))
        .collect();
    assert_eq!(
        made,
        [
            ("echo", false, Some("invalid_args"), 84),
            ("echo", true, None, 3),
            ("db.get", false, Some(CODE_SCRIPT_FINISHED), 2),
        ]
    );
    assert_eq!(outcome.logs, ["a".repeat(40), LOGS_TRUNCATED.to_string()]);
}

fn ended(final_: Value, limits: Limits) -> ScriptOutcome {
    let mut s = state(limits);
    let Step::End(end) = s.take(&json!({ "final": final_ }).to_string()) else {
        panic!("not the end")
    };
    s.outcome(end, 0, 0)
}

#[test]
fn the_end_reads_as_the_native_sandbox_reads_it() {
    let thrown = json!({"name": "Error", "message": "boom", "line": 2}).to_string();
    let out = ended(
        json!({"done": {"ok": false, "payload": thrown}}),
        Limits::default(),
    );
    let e = out.error.unwrap();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (ErrorKind::Thrown, "Error: boom (line 2)")
    );

    let out = ended(
        json!({"done": {"ok": true, "payload": null}}),
        Limits::default(),
    );
    assert_eq!((out.value, out.error), (None, None));

    let small = Limits {
        max_result_bytes: 64,
        ..Limits::default()
    };
    let out = ended(
        json!({"done": {"ok": true, "payload": format!("\"{}\"", "x".repeat(70))}}),
        small,
    );
    assert_eq!(out.error.unwrap().kind, ErrorKind::ResultTooLarge);

    let deep = format!("{}{}", "[".repeat(150), "]".repeat(150));
    let out = ended(
        json!({"done": {"ok": true, "payload": deep}}),
        Limits::default(),
    );
    assert_eq!(out.error.unwrap().kind, ErrorKind::Thrown);

    let out = ended(json!({"stop": "calls"}), Limits::default());
    assert_eq!(out.error.unwrap().kind, ErrorKind::CallLimit);

    let out = ended(json!({"never": true}), Limits::default());
    assert!(out.error.unwrap().message.contains("never settles"));
}

#[test]
fn platform_failures_map_to_their_error_kinds() {
    let l = Limits::default();
    assert_eq!(
        platform_error("Error: Worker exceeded CPU time limit.", &l).kind,
        ErrorKind::CpuLimit
    );
    assert_eq!(
        platform_error("Worker exceeded memory limit.", &l).kind,
        ErrorKind::MemoryLimit
    );
    let syntax = platform_error(
        "Error: Failed to start Worker:\nUncaught SyntaxError: Unexpected token '='\n  at my script:3:6",
        &l,
    );
    assert_eq!(syntax.kind, ErrorKind::Syntax);
    assert_eq!(syntax.message, "SyntaxError: Unexpected token '=' (line 2)");
    assert_eq!(
        platform_error("Error: Failed to start Worker: too many", &l).kind,
        ErrorKind::Unavailable
    );
    assert_eq!(
        platform_error("Error: something else", &l).kind,
        ErrorKind::Thrown
    );
}

#[test]
fn the_documented_cases_are_conformance_cases() {
    let names: HashSet<&str> = conformance::cases().iter().map(|c| c.name).collect();
    for (name, _, _) in PLATFORM_DIFFERENCES {
        assert!(names.contains(name), "{name}");
    }
    for (name, _) in PLATFORM_ONLY {
        assert!(names.contains(name), "{name}");
    }
}

#[test]
fn every_breakout_is_refused_before_the_isolate_sees_it() {
    for (name, code) in conformance::BREAKOUTS {
        assert!(check_body(code).is_err(), "{name}");
    }
}

#[test]
fn the_watchdog_is_a_multiple_of_the_budget_with_a_floor() {
    let w = Watchdog::default();
    assert_eq!(w.deadline_ms(100), 500);
    assert_eq!(w.deadline_ms(2_000), 8_000);
}

#[test]
fn the_job_carries_the_limits_the_driver_applies() {
    let job: Value = serde_json::from_str(&job_json(
        &["echo".to_string()],
        &Limits::default(),
        "my script",
        None,
    ))
    .unwrap();
    assert_eq!(job["ops"], json!(["echo"]));
    assert_eq!(job["maxCalls"], 200);
    assert_eq!(job["maxCallArgsBytes"], 256 << 10);
    assert_eq!(job["maxLogBytes"], 32 << 10);
    assert_eq!(job["truncated"], LOGS_TRUNCATED);
    assert_eq!(job["file"], "my script");
    assert!(job.get("nowMs").is_none() && job.get("seed").is_none());
    let det = clax_bot_sandbox::Determinism {
        now_ms: 1_700_000_000_000,
        seed: 7,
    };
    let job: Value =
        serde_json::from_str(&job_json(&[], &Limits::default(), "s", Some(det))).unwrap();
    assert_eq!(
        (&job["nowMs"], &job["seed"]),
        (&json!(1_700_000_000_000i64), &json!(7))
    );
}

fn report(records: Value) -> String {
    json!({"final": {"records": records, "logs": [], "done": {"ok": true, "payload": "1"}}})
        .to_string()
}

#[test]
fn records_the_driver_cannot_have_made_end_the_run() {
    for forged in [
        json!([{"op": "files.delete", "code": CODE_INVALID_ARGS, "bytes": 3}]),
        json!([{"op": "echo", "code": "not_found", "bytes": 3}]),
        json!([{"op": "echo", "code": CODE_INVALID_ARGS, "bytes": 1e15}]),
        json!([{"op": "echo", "code": CODE_INVALID_ARGS, "bytes": -1}]),
        json!([{"op": "echo", "code": CODE_INVALID_ARGS, "bytes": 2.5}]),
    ] {
        let mut s = state(Limits::default());
        let Step::End(end) = s.take(&report(forged.clone())) else {
            panic!("not the end")
        };
        assert!(matches!(end, End::Forged(_)), "{forged}: {end:?}");
        let out = s.outcome(end, 0, 0);
        let e = out.error.unwrap();
        assert_eq!(e.kind, ErrorKind::Thrown, "{forged}");
        assert!(e.message.contains("tampered"), "{}", e.message);
        assert!(out.calls.is_empty(), "{forged}");
        assert_eq!(out.value, None);
    }
}

#[test]
fn more_records_and_calls_than_the_call_limit_end_the_run() {
    let limits = Limits {
        max_calls: 2,
        ..Limits::default()
    };
    let one = json!({"op": "echo", "code": CODE_SCRIPT_FINISHED, "bytes": 3});
    let mut s = state(limits);
    assert_eq!(
        s.take(&report(json!([one, one, one]))),
        Step::End(End::Calls)
    );
    let out = s.outcome(End::Calls, 0, 0);
    assert_eq!(out.error.unwrap().kind, ErrorKind::CallLimit);
    assert_eq!(out.calls.len(), 2);

    let mut s = state(limits);
    assert!(matches!(
        s.take(&call_msg("echo", "[1]")),
        Step::Call { .. }
    ));
    let _ = s.answer("echo".into(), 3, 1, &Ok(json!(1)));
    let refused = json!({"records": [one], "logs": [], "call": {"op": "echo", "args": "[2]"}});
    assert_eq!(s.take(&refused.to_string()), Step::End(End::Calls));
}

#[test]
fn a_run_reports_the_isolates_memory_limit_as_its_peak() {
    let out = ended(
        json!({"done": {"ok": true, "payload": "1"}}),
        Limits::default(),
    );
    assert_eq!(out.mem_peak_bytes, ISOLATE_MEMORY_BYTES);
    let big = Limits {
        memory_bytes: 256 << 20,
        ..Limits::default()
    };
    let out = ended(json!({"done": {"ok": true, "payload": "1"}}), big);
    assert_eq!(out.mem_peak_bytes, 256 << 20);
}

#[test]
fn a_run_that_loaded_no_isolate_reports_no_memory() {
    let out = state(Limits::default()).outcome(
        End::NotLoaded("Failed to start Worker: the loader has no load".into()),
        0,
        0,
    );
    assert_eq!(out.error.unwrap().kind, ErrorKind::Unavailable);
    assert_eq!(out.mem_peak_bytes, 0);
}
