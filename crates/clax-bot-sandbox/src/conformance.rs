//! The conformance suite: scripts with the host calls they must make, the
//! replies they get, and the outcome they must reach. Every [`Sandbox`]
//! implementation passes all of it, built with the operations in [`OPS`]:
//!
//! ```ignore
//! let sandbox = MySandbox::new(conformance::OPS);
//! for (name, result) in conformance::run_all(&sandbox).await {
//!     result.unwrap_or_else(|e| panic!("{name}: {e}"));
//! }
//! ```
//!
//! Timing (`cpu_us`, `wall_us`, each call's `us`) and memory figures are
//! not compared: they differ by machine. Everything else is, the call
//! records included (by operation, outcome and code).

use super::{
    CODE_INVALID_ARGS, CODE_SCRIPT_FINISHED, CancelToken, ErrorKind, Host, HostError,
    LOGS_TRUNCATED, Limits, Sandbox, Script, ScriptOutcome,
};
use serde_json::{Value, json};
use std::sync::Mutex;

/// The operations a sandbox under test is built with.
pub const OPS: &[&str] = &["echo", "files.read", "files.write", "db.get"];

/// One host call a case makes, and the host's reply.
#[derive(Clone, Debug)]
pub struct Exchange {
    pub op: &'static str,
    /// The JSON array of the call's arguments.
    pub args: Value,
    pub reply: Result<Value, HostError>,
}

/// The call records a case must end with.
#[derive(Clone, Debug)]
pub enum Records {
    /// One per [`Case::calls`] exchange, in order: its op, whether the
    /// reply is a value, and the reply's error code.
    FromCalls,
    /// Exactly these (op, ok, code).
    Exactly(Vec<(&'static str, bool, Option<&'static str>)>),
    /// Not compared.
    Unchecked,
}

/// The outcome a case must reach.
#[derive(Clone, Debug)]
pub struct Expect {
    /// The value returned (checked when `error` is `None`; with an error
    /// there must be no value).
    pub value: Option<Value>,
    /// The log lines, when they are checked.
    pub logs: Option<Vec<String>>,
    /// The error's kind and a text its message contains.
    pub error: Option<(ErrorKind, &'static str)>,
}

/// One script, the calls it must make, and what it must end with.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: &'static str,
    pub code: &'static str,
    pub limits: Limits,
    /// Whether the run's token is cancelled before the run starts.
    pub cancelled: bool,
    /// The script's [`crate::Determinism`], when it has one.
    pub determinism: Option<crate::Determinism>,
    pub calls: Vec<Exchange>,
    pub records: Records,
    pub expect: Expect,
}

fn ok(value: Value) -> Expect {
    Expect {
        value: Some(value),
        logs: None,
        error: None,
    }
}

fn none() -> Expect {
    Expect {
        value: None,
        logs: None,
        error: None,
    }
}

fn fails(kind: ErrorKind, contains: &'static str) -> Expect {
    Expect {
        value: None,
        logs: None,
        error: Some((kind, contains)),
    }
}

fn case(name: &'static str, code: &'static str, expect: Expect) -> Case {
    Case {
        name,
        code,
        limits: Limits::default(),
        cancelled: false,
        determinism: None,
        calls: Vec::new(),
        records: Records::FromCalls,
        expect,
    }
}

fn call(op: &'static str, args: Value, reply: Value) -> Exchange {
    Exchange {
        op,
        args,
        reply: Ok(reply),
    }
}

fn call_err(op: &'static str, args: Value, code: &str, message: &str) -> Exchange {
    Exchange {
        op,
        args,
        reply: Err(HostError::new(code, message)),
    }
}

fn cpu(ms: u64) -> Limits {
    Limits {
        cpu_ms: ms,
        ..Limits::default()
    }
}

fn refused(op: &'static str) -> (&'static str, bool, Option<&'static str>) {
    (op, false, Some(CODE_INVALID_ARGS))
}

/// Code that closes the async function it is the body of and goes on: each
/// must end the run with [`ErrorKind::Syntax`] before any of it runs
/// (`crate::check_body` refuses them all). By case name.
pub const BREAKOUTS: &[(&str, &str)] = &[
    (
        "breakout_reopening_a_function",
        "return 1}); globalThis.leak = 7; (async function () { return globalThis.leak",
    ),
    (
        "breakout_reopening_an_arrow",
        "return 1}), 5, (async () => {",
    ),
    (
        "breakout_into_a_comma_expression",
        "return 1}, function () {",
    ),
    (
        "breakout_into_an_object_literal",
        "return 1}); globalThis.leak = 7; ({a: 1",
    ),
    ("breakout_shortest", "}),({"),
    (
        "breakout_after_a_regexp_holding_a_brace",
        "/}/; }); globalThis.leak = 7; ({",
    ),
    (
        "breakout_after_a_template_holding_a_brace",
        "`${'}'}`; }); globalThis.leak = 7; ({",
    ),
    (
        "breakout_into_parameters",
        "}), (async function (a = globalThis.leak = 7) {",
    ),
];

/// Code that is a body on its own, braces in strings, comments, regexps
/// and templates included (`crate::check_body` passes them all).
pub const BODIES: &[&str] = &[
    "return 1",
    "--> an HTML-like comment line\nreturn 1",
    "const re = /}/; return re.test('}')",
    "return `${'}'}`",
    "// }\nreturn {a: {b: 1}}",
    "/* }) */ return 2",
    "'use strict'; return 1",
    "",
];

/// Every case.
#[must_use]
pub fn cases() -> Vec<Case> {
    let mut all = basics();
    for (name, code) in BREAKOUTS {
        all.push(Case {
            records: Records::Exactly(vec![]),
            ..case(name, code, fails(ErrorKind::Syntax, "SyntaxError"))
        });
    }
    all.extend(host_calls());
    all.extend(errors());
    all.extend(limits());
    all.extend(confinement());
    all.extend(determinism());
    all
}

/// The frozen clock and seeded random numbers of a script with a
/// [`crate::Determinism`].
fn determinism() -> Vec<Case> {
    let det = Some(crate::Determinism {
        now_ms: 1_700_000_000_000,
        seed: 42,
    });
    let r = crate::seeded_random(42, 3);
    vec![
        Case {
            determinism: det,
            ..case(
                "a_determined_script_sees_a_frozen_clock",
                "const a = Date.now(); for (let i = 0; i < 100000; i++) {}\n\
                 return [a, Date.now(), new Date().getTime(), new Date(5).getTime(),\n\
                 typeof Date(), new Date() instanceof Date, Date.UTC(2020, 0, 1)];",
                ok(json!([
                    1_700_000_000_000i64,
                    1_700_000_000_000i64,
                    1_700_000_000_000i64,
                    5,
                    "string",
                    true,
                    1_577_836_800_000i64
                ])),
            )
        },
        Case {
            determinism: det,
            ..case(
                "a_determined_script_draws_seeded_random_numbers",
                "return [Math.random(), Math.random(), Math.random()];",
                ok(json!(r)),
            )
        },
        case(
            "an_undetermined_script_has_a_running_clock",
            "const n = Date.now(); return typeof n === 'number' && n > 1600000000000;",
            ok(json!(true)),
        ),
    ]
}

fn basics() -> Vec<Case> {
    vec![
        case(
            "returns_a_value",
            r#"return {a: 1, b: [true, null, "x"], c: 1.5}"#,
            ok(json!({"a": 1, "b": [true, null, "x"], "c": 1.5})),
        ),
        case("returns_nothing", "const x = 1;", none()),
        case("returns_undefined_or_a_function", "return () => 1", none()),
        case(
            "awaits_microtasks",
            "const v = await Promise.resolve(41);\nreturn v + 1",
            ok(json!(42)),
        ),
        Case {
            expect: Expect {
                logs: Some(vec![
                    r#"a 1 {"b":2} [1] null undefined true"#.into(),
                    String::new(),
                    "a\u{FFFD}b".into(),
                ]),
                ..ok(json!(0))
            },
            ..case(
                "logs_are_formatted",
                r#"console.log("a", 1, {b: 2}, [1], null, undefined, true); console.log(); console.log("a\ud800b"); return 0"#,
                none(),
            )
        },
        Case {
            limits: Limits {
                max_log_bytes: 64,
                ..Limits::default()
            },
            expect: Expect {
                logs: Some(vec!["a".repeat(30), "b".repeat(30), LOGS_TRUNCATED.into()]),
                ..ok(json!("done"))
            },
            ..case(
                "logs_are_capped",
                r#"for (const c of ["a", "b", "c", "d"]) console.log(c.repeat(30)); return "done""#,
                none(),
            )
        },
        Case {
            expect: Expect {
                logs: Some(vec!["before".into()]),
                ..fails(ErrorKind::Thrown, "after")
            },
            ..case(
                "logs_survive_an_error",
                r#"console.log("before"); throw new Error("after")"#,
                none(),
            )
        },
        case(
            "the_end_is_the_body_settling",
            "(async () => { for (;;) await null })(); return 1",
            ok(json!(1)),
        ),
        Case {
            records: Records::Exactly(vec![("echo", false, Some(CODE_SCRIPT_FINISHED))]),
            ..case(
                "a_call_not_awaited_never_reaches_the_host",
                r#"clax.echo("fire"); return 0"#,
                ok(json!(0)),
            )
        },
        Case {
            records: Records::Exactly(vec![]),
            ..case(
                "a_call_after_the_end_never_reaches_the_host",
                "Promise.resolve().then(() => 0).then(() => 0).then(() => 0).then(() => clax.echo(1)); return 0",
                ok(json!(0)),
            )
        },
        case(
            "a_replaced_then_cannot_change_the_result",
            r#"Promise.prototype.then = function (a) { a("forged"); a("again") }; return "real""#,
            ok(json!("real")),
        ),
    ]
}

fn host_calls() -> Vec<Case> {
    vec![
        Case {
            calls: vec![call(
                "files.read",
                json!(["index.html"]),
                json!("<p>hi</p>"),
            )],
            ..case(
                "a_host_call_resolves",
                r#"const t = await clax.files.read("index.html"); return t.length"#,
                ok(json!(9)),
            )
        },
        Case {
            calls: vec![call(
                "files.write",
                json!(["a.txt", "x", null, {"n": 1}]),
                Value::Null,
            )],
            ..case(
                "arguments_go_as_a_json_array",
                r#"const r = await clax.files.write("a.txt", "x", undefined, {n: 1}); return [r, "ok"]"#,
                ok(json!([null, "ok"])),
            )
        },
        Case {
            calls: vec![
                call("echo", json!([1]), json!("one")),
                call("db.get", json!(["c", "d"]), json!({"v": 2})),
            ],
            ..case(
                "calls_reach_the_host_in_the_order_made",
                r#"const [a, b] = await Promise.all([clax.echo(1), clax.db.get("c", "d")]); return [a, b]"#,
                ok(json!(["one", {"v": 2}])),
            )
        },
        case(
            "clax_has_exactly_its_ops",
            "return [typeof clax.files.read, typeof clax.db.get, typeof clax.files.nope, typeof clax.nope]",
            ok(json!(["function", "function", "undefined", "undefined"])),
        ),
        case(
            "clax_namespaces_have_no_prototype",
            "return [Object.getPrototypeOf(clax) === null, Object.getPrototypeOf(clax.files) === null, typeof ({}).read, typeof ({}).files, typeof ({}).echo]",
            ok(json!([true, true, "undefined", "undefined", "undefined"])),
        ),
        Case {
            calls: vec![call_err(
                "files.read",
                json!(["missing"]),
                "not_found",
                "no such file: missing",
            )],
            ..case(
                "a_host_error_is_a_rejection_the_script_can_catch",
                r#"try { await clax.files.read("missing"); return "read" }
catch (e) { return [e.name, e.code, e.message, e instanceof Error] }"#,
                ok(json!([
                    "HostError",
                    "not_found",
                    "no such file: missing",
                    true
                ])),
            )
        },
        Case {
            calls: vec![call_err(
                "files.read",
                json!(["missing"]),
                "not_found",
                "no such file: missing",
            )],
            ..case(
                "an_uncaught_host_error_ends_the_script",
                r#"await clax.files.read("missing"); return 1"#,
                fails(
                    ErrorKind::HostError,
                    "HostError not_found: no such file: missing",
                ),
            )
        },
        Case {
            records: Records::Exactly(vec![refused("echo"), refused("echo")]),
            ..case(
                "arguments_whose_json_is_not_an_array_are_refused",
                r#"Array.prototype.toJSON = function () { return "not an array" };
const out = [];
try { await clax.echo(1) } catch (e) { out.push(e.name) }
Array.prototype.toJSON = function () { return undefined };
try { await clax.echo(1) } catch (e) { out.push(e.name) }
delete Array.prototype.toJSON;
return out"#,
                ok(json!(["TypeError", "TypeError"])),
            )
        },
        Case {
            records: Records::Exactly(vec![refused("echo"), refused("echo"), refused("echo")]),
            ..case(
                "arguments_that_do_not_parse_back_are_refused",
                r#"const out = [];
let deep = 0; for (let i = 0; i < 300; i++) deep = [deep];
for (const arg of [deep, "\ud800", 10n]) {
  try { await clax.echo(arg); out.push("sent") } catch (e) { out.push(e.name) }
}
return out"#,
                ok(json!(["TypeError", "TypeError", "TypeError"])),
            )
        },
        Case {
            limits: Limits {
                max_call_args_bytes: 64,
                ..Limits::default()
            },
            calls: vec![call("echo", json!(["x".repeat(58)]), json!(1))],
            records: Records::Exactly(vec![refused("echo"), refused("echo"), ("echo", true, None)]),
            ..case(
                "arguments_over_their_limit_are_refused",
                r#"const out = [];
for (const arg of ["x".repeat(100), "é".repeat(40), "x".repeat(58)]) {
  try { await clax.echo(arg); out.push("sent") } catch (e) { out.push(e.name) }
}
return out"#,
                ok(json!(["TypeError", "TypeError", "sent"])),
            )
        },
        Case {
            records: Records::Exactly(vec![refused("echo")]),
            ..case(
                "a_shared_subtree_counts_every_time_it_is_serialised",
                r#"let a = [0]; for (let i = 0; i < 18; i++) a = [a, a];
try { await clax.echo(a); return "sent" } catch (e) { return e.name }"#,
                ok(json!("TypeError")),
            )
        },
    ]
}

fn errors() -> Vec<Case> {
    vec![
        case(
            "a_thrown_error_names_its_line",
            "const x = 1;\nthrow new Error(\"boom\")",
            fails(ErrorKind::Thrown, "Error: boom (line 2)"),
        ),
        case(
            "a_thrown_value",
            r#"throw "plain""#,
            fails(ErrorKind::Thrown, "plain"),
        ),
        case(
            "a_thrown_null_is_a_thrown_value",
            "throw null",
            fails(ErrorKind::Thrown, "null"),
        ),
        case(
            "a_rejected_await",
            r#"await Promise.reject(new TypeError("bad"))"#,
            fails(ErrorKind::Thrown, "TypeError: bad"),
        ),
        case(
            "a_syntax_error_names_its_line",
            "const a = 1;\nconst = 2;",
            fails(ErrorKind::Syntax, "line 2"),
        ),
        case(
            "a_first_line_may_open_with_an_html_like_comment",
            "--> a comment, as at the start of any line\nreturn 1",
            ok(json!(1)),
        ),
        case(
            "an_unserialisable_result_is_thrown",
            "const a = {}; a.self = a; return a",
            fails(ErrorKind::Thrown, ""),
        ),
        case(
            "a_result_that_does_not_parse_back_is_thrown",
            r#"return "\ud800""#,
            fails(ErrorKind::Thrown, ""),
        ),
        case(
            "a_result_nested_too_deep_is_thrown",
            "let a = 0; for (let i = 0; i < 150; i++) a = [a]; return a",
            fails(ErrorKind::Thrown, ""),
        ),
        case(
            "unbounded_recursion_is_thrown",
            "function f() { return f() + 1 } f()",
            fails(ErrorKind::Thrown, "Maximum call stack size exceeded"),
        ),
        case(
            "a_deeply_nested_json_parse_is_thrown",
            "return JSON.parse('['.repeat(1000000) + ']'.repeat(1000000))",
            fails(ErrorKind::Thrown, "Maximum call stack size exceeded"),
        ),
        case(
            "a_look_alike_host_error_is_a_thrown_value",
            r#"const e = new Error("x"); e.name = "HostError"; e.code = "not_found";
throw new Proxy(e, { get(t, k) { return typeof k === "symbol" ? true : t[k] } })"#,
            fails(ErrorKind::Thrown, ""),
        ),
        case(
            "a_look_alike_out_of_memory_error_is_a_thrown_value",
            r#"const e = new Error("out of memory"); e.name = "InternalError"; throw e"#,
            fails(ErrorKind::Thrown, "InternalError: out of memory"),
        ),
        Case {
            calls: vec![call("echo", json!([1]), json!(1))],
            ..case(
                "an_unhandled_rejection_does_not_end_the_script",
                r#"Promise.reject(null); await clax.echo(1); return "fine""#,
                ok(json!("fine")),
            )
        },
    ]
}

fn limits() -> Vec<Case> {
    vec![
        Case {
            limits: Limits {
                memory_bytes: 0,
                ..Limits::default()
            },
            ..case(
                "limits_that_fail_validation_run_nothing",
                "return 1",
                fails(ErrorKind::InvalidLimits, "memory_bytes"),
            )
        },
        Case {
            cancelled: true,
            ..case(
                "a_cancelled_token_runs_nothing",
                "return 1",
                fails(ErrorKind::Cancelled, ""),
            )
        },
        Case {
            limits: Limits {
                max_calls: 2,
                ..Limits::default()
            },
            calls: vec![
                call("echo", json!([0]), json!(0)),
                call("echo", json!([1]), json!(1)),
            ],
            ..case(
                "the_call_limit_ends_the_script",
                r#"for (let i = 0; i < 5; i++) await clax.echo(i); return "done""#,
                fails(ErrorKind::CallLimit, "2"),
            )
        },
        Case {
            limits: Limits {
                max_calls: 1,
                ..Limits::default()
            },
            calls: vec![call("echo", json!([0]), json!(0))],
            ..case(
                "the_call_limit_cannot_be_caught",
                r#"try { for (;;) await clax.echo(0) } catch (e) {} return "escaped""#,
                fails(ErrorKind::CallLimit, ""),
            )
        },
        Case {
            limits: Limits {
                max_result_bytes: 100,
                ..Limits::default()
            },
            ..case(
                "a_result_over_its_limit",
                r#"return "x".repeat(200)"#,
                fails(ErrorKind::ResultTooLarge, "100"),
            )
        },
        Case {
            limits: Limits {
                max_result_bytes: 64,
                ..Limits::default()
            },
            ..case(
                "a_result_at_its_limit",
                r#"return "x".repeat(62)"#,
                ok(json!("x".repeat(62))),
            )
        },
        Case {
            limits: cpu(100),
            ..case(
                "the_cpu_limit_ends_a_loop",
                "for (;;) {}",
                fails(ErrorKind::CpuLimit, "100 ms"),
            )
        },
        Case {
            limits: cpu(100),
            ..case(
                "the_cpu_limit_cannot_be_caught",
                r#"try { for (;;) {} } catch (e) {} return "escaped""#,
                fails(ErrorKind::CpuLimit, ""),
            )
        },
        Case {
            limits: cpu(100),
            ..case(
                "the_cpu_limit_ends_a_loop_of_heavy_sorts",
                "const a = Array.from({length: 400000}, (_, i) => String((i * 7919) % 400000));\nfor (let k = 0; k < 300; k++) { a.sort(); a.reverse() }\nreturn 1",
                fails(ErrorKind::CpuLimit, ""),
            )
        },
        Case {
            limits: cpu(100),
            ..case(
                "the_cpu_limit_ends_a_loop_of_heavy_string_builtins",
                "let n = 0; for (let k = 0; k < 2000; k++) { n += 'ab'.repeat(5e6).indexOf('ba', 9e6) }\nreturn n",
                fails(ErrorKind::CpuLimit, ""),
            )
        },
        Case {
            limits: cpu(100),
            ..case(
                "the_cpu_limit_ends_a_backtracking_regexp",
                "return /(a+)+$/.test('a'.repeat(40) + 'b')",
                fails(ErrorKind::CpuLimit, ""),
            )
        },
        Case {
            limits: cpu(100),
            records: Records::Unchecked,
            ..case(
                "the_cpu_limit_ends_a_microtask_loop_beside_a_host_call",
                "(async () => { for (;;) await null })(); await clax.echo(1); return 1",
                fails(ErrorKind::CpuLimit, ""),
            )
        },
        Case {
            limits: Limits {
                memory_bytes: 16 << 20,
                ..Limits::default()
            },
            ..case(
                "the_memory_limit_ends_an_allocation_bomb",
                "const a = []; for (;;) a.push(new Array(10000).fill(a.length))",
                fails(ErrorKind::MemoryLimit, ""),
            )
        },
        case(
            "the_memory_limit_ends_a_deep_structure",
            "let a = []; for (let i = 0; i < 2000000; i++) a = [a]; return a",
            fails(ErrorKind::MemoryLimit, ""),
        ),
        Case {
            limits: Limits {
                memory_bytes: 16 << 20,
                ..Limits::default()
            },
            ..case(
                "the_memory_limit_cannot_be_caught",
                "let n = 0; try { const a = []; for (;;) a.push(new Array(10000).fill(1)) } catch (e) { n = 1 } return ['continued', n]",
                fails(ErrorKind::MemoryLimit, ""),
            )
        },
    ]
}

fn confinement() -> Vec<Case> {
    vec![
        case(
            "no_ambient_io",
            "return [typeof fetch, typeof setTimeout, typeof setInterval, typeof require, typeof process, typeof XMLHttpRequest, typeof WebSocket, typeof InternalError]",
            ok(json!([
                "undefined",
                "undefined",
                "undefined",
                "undefined",
                "undefined",
                "undefined",
                "undefined",
                "undefined"
            ])),
        ),
        Case {
            records: Records::Exactly(vec![]),
            ..case(
                "no_module_loading",
                r#"try { await import("x"); return "loaded" } catch (e) { return "refused" }"#,
                fails(ErrorKind::Syntax, "import()"),
            )
        },
        Case {
            records: Records::Exactly(vec![]),
            ..case(
                "no_loading_a_runtime_module",
                "const w = await import(\"cloudflare:workers\");\nreturn Object.keys(w)",
                fails(ErrorKind::Syntax, "line 1"),
            )
        },
        Case {
            records: Records::Exactly(vec![]),
            ..case(
                "no_loading_a_node_module",
                "const n = 1;\nconst fs = await import(\"node:\" + \"fs\");\nreturn Object.keys(fs).length",
                fails(ErrorKind::Syntax, "line 2"),
            )
        },
        Case {
            records: Records::Exactly(vec![]),
            ..case(
                "no_import_meta",
                "return typeof import.meta",
                fails(ErrorKind::Syntax, "SyntaxError"),
            )
        },
        Case {
            records: Records::Exactly(vec![
                refused("echo"),
                ("files.read", false, Some(CODE_SCRIPT_FINISHED)),
            ]),
            expect: Expect {
                logs: Some(vec!["real".into(), "forged".into()]),
                ..ok(json!("done"))
            },
            ..case(
                "serialisation_hooks_cannot_forge_call_records_or_logs",
                r#"console.log("real");
try { await clax.echo(10n) } catch (e) {}
clax.files.read("never sent");
const forged = [{op: "files.write", code: "not_found", bytes: 1e15}, {op: "nope", ok: true}];
Array.prototype.toJSON = function () { return forged };
Object.prototype.toJSON = function () { return forged };
Object.defineProperty(Array.prototype, "0", { set(v) {}, configurable: true });
Object.defineProperty(Array.prototype, "1", { set(v) {}, configurable: true });
console.log("forged");
return "done""#,
                none(),
            )
        },
        case(
            "no_code_generation_from_strings",
            r#"const tries = [
  () => eval("1"),
  () => (0, eval)("1"),
  () => new Function("return 1"),
  () => Function("return 1"),
  () => (() => {}).constructor("return 1"),
  () => new (Object.getPrototypeOf(async function () {}).constructor)("return 1"),
  () => new (Object.getPrototypeOf(function* () {}).constructor)("yield 1"),
  () => new (Object.getPrototypeOf(async function* () {}).constructor)("yield 1"),
];
const out = [];
for (const t of tries) { try { t(); out.push("ran") } catch (e) { out.push(e.name) } }
return [out, (() => {}) instanceof Function]"#,
            ok(json!([
                [
                    "EvalError",
                    "EvalError",
                    "EvalError",
                    "EvalError",
                    "EvalError",
                    "EvalError",
                    "EvalError",
                    "EvalError"
                ],
                true
            ])),
        ),
    ]
}

/// A [`Host`] that answers a case's calls in order and notes any call it
/// did not expect.
pub struct ScriptedHost {
    calls: Vec<Exchange>,
    state: Mutex<(usize, Vec<String>)>,
}

impl ScriptedHost {
    #[must_use]
    pub fn new(calls: Vec<Exchange>) -> Self {
        ScriptedHost {
            calls,
            state: Mutex::new((0, Vec::new())),
        }
    }

    /// What went wrong: unexpected calls, and expected calls never made.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut problems = state.1.clone();
        for missed in &self.calls[state.0.min(self.calls.len())..] {
            problems.push(format!(
                "expected a call {} {} that was never made",
                missed.op, missed.args
            ));
        }
        problems
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Host for ScriptedHost {
    async fn call(&self, op: &str, args: Value) -> Result<Value, HostError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let next = state.0;
        match self.calls.get(next) {
            Some(want) if want.op == op && want.args == args => {
                state.0 += 1;
                want.reply.clone()
            }
            want => {
                let want = want.map_or("no call".to_string(), |w| format!("{} {}", w.op, w.args));
                let shown: String = args.to_string().chars().take(200).collect();
                state
                    .1
                    .push(format!("call {next}: got {op} {shown}, expected {want}"));
                Err(HostError::new("conformance", "unexpected call"))
            }
        }
    }
}

/// Runs `case` on `sandbox` and checks its outcome; the outcome on success.
pub async fn check(sandbox: &dyn Sandbox, case: &Case) -> Result<ScriptOutcome, String> {
    let host = ScriptedHost::new(case.calls.clone());
    let mut script = Script::new(case.name, case.code);
    script.determinism = case.determinism;
    let cancel = CancelToken::new();
    if case.cancelled {
        cancel.cancel();
    }
    let outcome = sandbox.run(&script, &host, &case.limits, &cancel).await;
    let mut problems = host.problems();
    match (&case.expect.error, &outcome.error) {
        (None, None) => {
            if outcome.value != case.expect.value {
                problems.push(format!(
                    "value {:?}, expected {:?}",
                    outcome.value, case.expect.value
                ));
            }
        }
        (None, Some(got)) => problems.push(format!("unexpected error {got:?}")),
        (Some((kind, _)), None) => {
            problems.push(format!(
                "expected a {kind:?} error, got value {:?}",
                outcome.value
            ));
        }
        (Some((kind, contains)), Some(got)) => {
            if got.kind != *kind || !got.message.contains(contains) {
                problems.push(format!(
                    "error {got:?}, expected {kind:?} containing {contains:?}"
                ));
            }
            if outcome.value.is_some() {
                problems.push(format!("a value {:?} beside the error", outcome.value));
            }
        }
    }
    if let Some(logs) = &case.expect.logs
        && &outcome.logs != logs
    {
        problems.push(format!("logs {:?}, expected {logs:?}", outcome.logs));
    }
    let made: Vec<(&str, bool, Option<&str>)> = outcome
        .calls
        .iter()
        .map(|c| (c.op.as_str(), c.ok, c.code.as_deref()))
        .collect();
    let want: Option<Vec<(&str, bool, Option<&str>)>> = match &case.records {
        Records::FromCalls => Some(
            case.calls
                .iter()
                .map(|c| {
                    (
                        c.op,
                        c.reply.is_ok(),
                        c.reply.as_ref().err().map(|e| e.code.as_str()),
                    )
                })
                .collect(),
        ),
        Records::Exactly(records) => Some(records.clone()),
        Records::Unchecked => None,
    };
    if let Some(want) = want
        && made != want
    {
        problems.push(format!("call records {made:?}, expected {want:?}"));
    }
    if problems.is_empty() {
        Ok(outcome)
    } else {
        Err(problems.join("; "))
    }
}

/// Runs every case on `sandbox`, one at a time: each case's name and its
/// result.
pub async fn run_all(sandbox: &dyn Sandbox) -> Vec<(&'static str, Result<(), String>)> {
    let mut results = Vec::new();
    for case in cases() {
        let result = check(sandbox, &case).await.map(|_| ());
        results.push((case.name, result));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_names_are_unique() {
        let mut names: Vec<_> = cases().iter().map(|c| c.name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn every_scripted_call_uses_a_listed_op() {
        assert_eq!(crate::validate_ops(OPS), Ok(()));
        for case in cases() {
            for call in &case.calls {
                assert!(OPS.contains(&call.op), "{}: {}", case.name, call.op);
                assert!(call.args.is_array(), "{}: args are an array", case.name);
            }
        }
    }

    #[test]
    fn every_case_but_the_invalid_one_has_valid_limits() {
        for case in cases() {
            let valid = case.limits.validate().is_ok();
            let wants_invalid = matches!(case.expect.error, Some((ErrorKind::InvalidLimits, _)));
            assert_eq!(valid, !wants_invalid, "{}", case.name);
        }
    }
}
