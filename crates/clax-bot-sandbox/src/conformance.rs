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
//! A case runs one script ([`cases`]); a [`Sequence`] runs several, one
//! after the other on the same sandbox ([`sequences`]), and holds that
//! nothing of one script (its operations among it) reaches the next.
//!
//! Timing (`cpu_us`, `wall_us`, each call's `us`) and memory figures are
//! not compared: they differ by machine. Everything else is, the call
//! records included (by operation, outcome and code).

use super::{
    CODE_CANCELLED, CODE_INVALID_ARGS, CODE_SCRIPT_FINISHED, CODE_WAITING, CancelToken, ErrorKind,
    Host, HostError, HostReply, LOGS_TRUNCATED, Limits, MAX_OP_BYTES, MAX_OPS, MAX_OPS_BYTES,
    Sandbox, Script, ScriptOutcome,
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
    /// What the host answers ([`Host::ask`]): a value, a host error, or
    /// that it has no answer yet.
    pub reply: HostReply,
}

impl Exchange {
    /// The record the call ends with: whether the reply is a value, and
    /// its code (a host error's, or [`CODE_WAITING`]).
    #[must_use]
    pub fn record(&self) -> (&'static str, bool, Option<&str>) {
        match &self.reply {
            HostReply::Answer(Ok(_)) => (self.op, true, None),
            HostReply::Answer(Err(e)) => (self.op, false, Some(e.code.as_str())),
            HostReply::Waiting => (self.op, false, Some(CODE_WAITING)),
        }
    }
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
    /// Whether the host cancels the run's token when it takes the case's
    /// first call, and never answers it.
    pub cancel_in_call: bool,
    /// The script's [`crate::Determinism`], when it has one.
    pub determinism: Option<crate::Determinism>,
    /// The script's own operations ([`Script::ops`]); `None` for the
    /// sandbox's, [`OPS`].
    pub ops: Option<Vec<String>>,
    pub calls: Vec<Exchange>,
    pub records: Records,
    pub expect: Expect,
}

impl Case {
    /// The script the case runs.
    #[must_use]
    pub fn script(&self) -> Script {
        Script {
            title: self.name.into(),
            code: self.code.into(),
            determinism: self.determinism,
            ops: self.ops.clone(),
        }
    }
}

/// Scripts run one after the other on one sandbox, each checked as a
/// [`Case`] is.
#[derive(Clone, Debug)]
pub struct Sequence {
    pub name: &'static str,
    pub steps: Vec<Case>,
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
        cancel_in_call: false,
        determinism: None,
        ops: None,
        calls: Vec::new(),
        records: Records::FromCalls,
        expect,
    }
}

fn call(op: &'static str, args: Value, reply: Value) -> Exchange {
    Exchange {
        op,
        args,
        reply: HostReply::Answer(Ok(reply)),
    }
}

/// A call the host has no answer to yet: the run parks on it.
fn waits(op: &'static str, args: Value) -> Exchange {
    Exchange {
        op,
        args,
        reply: HostReply::Waiting,
    }
}

fn call_err(op: &'static str, args: Value, code: &str, message: &str) -> Exchange {
    Exchange {
        op,
        args,
        reply: HostReply::Answer(Err(HostError::new(code, message))),
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
    all.extend(script_ops());
    all.extend(refused_ops());
    all.extend(parking());
    all
}

/// Runs that park on a call the host has no answer to, and scripts that
/// try to look as if they had.
fn parking() -> Vec<Case> {
    vec![
        Case {
            calls: vec![
                call("echo", json!([1]), json!(1)),
                waits("files.read", json!(["q"])),
            ],
            expect: Expect {
                logs: Some(vec!["before".into()]),
                ..fails(ErrorKind::Parked, "")
            },
            ..case(
                "a_waiting_call_parks_the_run_at_the_call",
                "console.log('before');\nawait clax.echo(1);\n\
                 try {\n  await Promise.all([clax.files.read('q'), clax.echo(2)]);\n  console.log('answered');\n\
                 } catch (e) { console.log('caught') }\nfinally { console.log('finally') }\nreturn 'done'",
                none(),
            )
        },
        Case {
            calls: vec![
                call("echo", json!(["first"]), json!("one")),
                waits("files.read", json!(["q"])),
            ],
            ..case(
                "a_call_made_before_a_waiting_one_reaches_the_host",
                "clax.echo('first'); await clax.files.read('q'); return 1",
                fails(ErrorKind::Parked, ""),
            )
        },
        Case {
            calls: vec![waits("files.read", json!(["q"]))],
            ..case(
                "a_call_made_after_a_waiting_one_never_reaches_the_host",
                "const a = clax.files.read('q'); const b = clax.echo(2); await b; return await a",
                fails(ErrorKind::Parked, ""),
            )
        },
        Case {
            calls: vec![waits("files.read", json!(["q"]))],
            expect: Expect {
                logs: Some(vec![]),
                ..fails(ErrorKind::Parked, "")
            },
            ..case(
                "a_park_runs_no_catch_or_finally",
                "try { await clax.files.read('q') }\n\
                 catch (e) { console.log('caught'); await clax.echo('catch') }\n\
                 finally { console.log('finally'); await clax.echo('finally') }\nreturn 1",
                none(),
            )
        },
        case(
            "a_promise_that_never_settles_is_not_a_park",
            "await new Promise(() => {})",
            fails(ErrorKind::Thrown, "never settles"),
        ),
        case(
            "a_look_alike_park_error_is_a_thrown_value",
            "throw Object.assign(new Error('parked'), {name: 'Parked', code: 'waiting', kind: 'parked'})",
            fails(ErrorKind::Thrown, "parked"),
        ),
        case(
            "a_look_alike_waiting_host_error_is_a_thrown_value",
            "const e = new Error('x'); e.name = 'HostError'; e.code = 'waiting'; throw e",
            fails(ErrorKind::Thrown, "HostError"),
        ),
        case(
            "a_returned_park_is_a_value",
            "return {error: {kind: 'parked'}, calls: [{op: 'echo', code: 'waiting'}]}",
            ok(json!({"error": {"kind": "parked"}, "calls": [{"op": "echo", "code": "waiting"}]})),
        ),
    ]
}

fn ops(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|n| (*n).to_string()).collect())
}

/// What a script whose list is refused runs: nothing, so it logs nothing.
const RUNS_NOTHING: &str = r#"console.log("ran"); return 1"#;

/// A list a script carries, and the case that it ends the run with
/// [`ErrorKind::InvalidOps`], before any of it runs.
fn refused_list(name: &'static str, list: Vec<String>, contains: &'static str) -> Case {
    Case {
        ops: Some(list),
        records: Records::Exactly(vec![]),
        expect: Expect {
            logs: Some(vec![]),
            ..fails(ErrorKind::InvalidOps, contains)
        },
        ..case(name, RUNS_NOTHING, none())
    }
}

/// The longest name a list may hold: `l.` and `x` to [`MAX_OP_BYTES`].
fn longest_op() -> String {
    format!("l.{}", "x".repeat(MAX_OP_BYTES - 2))
}

/// A list of [`MAX_OPS`] names: `s.o0` to `s.o4094` and [`longest_op`].
fn most_ops() -> Vec<String> {
    let mut list: Vec<String> = (0..MAX_OPS - 1).map(|i| format!("s.o{i}")).collect();
    list.push(longest_op());
    list
}

/// `count` names of [`MAX_OP_BYTES`] each: `b000.xxx…` onwards.
fn heaviest_ops(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("b{i:03}.{}", "x".repeat(MAX_OP_BYTES - 5)))
        .collect()
}

/// Scripts that carry their own operations.
fn script_ops() -> Vec<Case> {
    vec![
        Case {
            ops: ops(&["shell.windows", "echo"]),
            calls: vec![call("shell.windows", json!([]), json!([{"id": 1}]))],
            ..case(
                "a_scripts_own_ops_replace_the_sandboxs",
                "const w = await clax.shell.windows();\n\
                 return [w, typeof clax.files, typeof clax.db, typeof clax.echo, Object.keys(clax).sort()]",
                ok(json!([[{"id": 1}], "undefined", "undefined", "function", ["echo", "shell"]])),
            )
        },
        Case {
            ops: ops(&["shell.windows"]),
            records: Records::Exactly(vec![]),
            ..case(
                "a_call_outside_the_scripts_ops_is_not_made",
                "try { await clax.files.read('x'); return 'read' } catch (e) { return e.name }",
                ok(json!("TypeError")),
            )
        },
        Case {
            ops: ops(&[
                "shell.windows.panes.list",
                "shell.windows.panes.focus",
                "shell.windows.count",
                "shell.tabs",
            ]),
            calls: vec![call("shell.windows.panes.list", json!([2]), json!(3))],
            ..case(
                "a_scripts_namespaces_nest_three_deep",
                "const n = await clax.shell.windows.panes.list(2);\n\
                 return [n, typeof clax.shell.windows.panes.focus, typeof clax.shell.windows.count,\n\
                 typeof clax.shell.tabs, typeof clax.shell.windows.panes.nope,\n\
                 Object.getPrototypeOf(clax.shell.windows.panes) === null, Object.keys(clax.shell.windows).sort()]",
                ok(json!([
                    3,
                    "function",
                    "function",
                    "function",
                    "undefined",
                    true,
                    ["count", "panes"]
                ])),
            )
        },
        Case {
            ops: Some(vec![]),
            ..case(
                "a_script_with_no_ops_has_an_empty_clax",
                "return [typeof clax, Object.keys(clax).length, typeof clax.echo]",
                ok(json!(["object", 0, "undefined"])),
            )
        },
        Case {
            ops: Some(most_ops()),
            calls: vec![call("s.o4094", json!([1]), json!("last"))],
            ..case(
                "a_script_may_have_the_most_ops",
                "const r = await clax.s.o4094(1);\n\
                 return [r, Object.keys(clax.s).length, typeof clax.l['x'.repeat(4094)], typeof clax.s.o0]",
                ok(json!(["last", 4095, "function", "function"])),
            )
        },
        Case {
            ops: Some(heaviest_ops(MAX_OPS_BYTES / MAX_OP_BYTES)),
            ..case(
                "a_script_may_have_ops_of_the_most_bytes",
                "return [Object.keys(clax).length, typeof clax.b063['x'.repeat(4091)]]",
                ok(json!([64, "function"])),
            )
        },
    ]
}

/// Lists a script may not carry, each refused before it runs.
fn refused_ops() -> Vec<Case> {
    let list = |names: &[&str]| ops(names).unwrap_or_default();
    let mut too_many = most_ops();
    too_many.push("s.more".into());
    vec![
        refused_list(
            "ops_with_an_empty_name_are_refused",
            list(&[""]),
            "not a usable",
        ),
        refused_list(
            "ops_with_an_empty_segment_are_refused",
            list(&["shell..windows"]),
            "not a usable",
        ),
        refused_list(
            "ops_ending_in_a_dot_are_refused",
            list(&["shell."]),
            "not a usable",
        ),
        refused_list(
            "ops_starting_with_a_dot_are_refused",
            list(&[".shell"]),
            "not a usable",
        ),
        refused_list(
            "ops_that_are_not_identifiers_are_refused",
            list(&["echo", "shell.win-dows"]),
            "not a usable",
        ),
        refused_list(
            "ops_that_are_not_ascii_are_refused",
            list(&["shell.w\u{EF}ndows"]),
            "not a usable",
        ),
        refused_list(
            "ops_through_proto_are_refused",
            list(&["__proto__.polluted"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_constructor_are_refused",
            list(&["shell.constructor"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_prototype_are_refused",
            list(&["prototype"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_then_are_refused",
            list(&["shell.then"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_tojson_are_refused",
            list(&["shell.toJSON"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_tostring_are_refused",
            list(&["shell.toString"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_tolocalestring_are_refused",
            list(&["toLocaleString"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_valueof_are_refused",
            list(&["shell.valueOf"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_hasownproperty_are_refused",
            list(&["shell.hasOwnProperty"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_isprototypeof_are_refused",
            list(&["shell.isPrototypeOf"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_propertyisenumerable_are_refused",
            list(&["shell.propertyIsEnumerable"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_definegetter_are_refused",
            list(&["shell.__defineGetter__"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_definesetter_are_refused",
            list(&["shell.__defineSetter__"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_lookupgetter_are_refused",
            list(&["shell.__lookupGetter__"]),
            "not a usable",
        ),
        refused_list(
            "ops_named_lookupsetter_are_refused",
            list(&["shell.__lookupSetter__"]),
            "not a usable",
        ),
        refused_list(
            "namespaces_named_tojson_are_refused",
            list(&["toJSON.read"]),
            "not a usable",
        ),
        refused_list(
            "ops_listed_twice_are_refused",
            list(&["echo", "shell.windows", "echo"]),
            "listed twice",
        ),
        refused_list(
            "an_op_that_is_also_a_namespace_is_refused",
            list(&["shell", "shell.windows"]),
            "both an operation and a namespace",
        ),
        refused_list(
            "a_namespace_that_is_also_an_op_is_refused",
            list(&["shell.windows.list", "shell.windows"]),
            "both an operation and a namespace",
        ),
        refused_list(
            "more_than_the_most_ops_are_refused",
            too_many,
            "operations are listed",
        ),
        refused_list(
            "an_op_name_over_its_bytes_is_refused",
            vec![format!("{}x", longest_op())],
            "bytes; an operation name is at most",
        ),
        refused_list(
            "ops_over_their_bytes_together_are_refused",
            heaviest_ops(MAX_OPS_BYTES / MAX_OP_BYTES + 1),
            "bytes together",
        ),
    ]
}

/// What a script on the sandbox's own operations sees once an earlier one
/// had `shell.windows`: none of it.
fn sees_only_the_sandboxs(name: &'static str) -> Case {
    case(
        name,
        "return [typeof clax.shell, typeof globalThis.leak, typeof clax.files.read, Object.keys(clax).sort()]",
        ok(json!([
            "undefined",
            "undefined",
            "function",
            ["db", "echo", "files"]
        ])),
    )
}

/// Every sequence.
#[must_use]
pub fn sequences() -> Vec<Sequence> {
    let shell = || ops(&["shell.windows"]);
    vec![
        Sequence {
            name: "each_script_sees_only_its_own_ops",
            steps: vec![
                Case {
                    ops: shell(),
                    calls: vec![call("shell.windows", json!([]), json!(2))],
                    ..case(
                        "each_script_sees_only_its_own_ops_1",
                        "globalThis.leak = clax; return [await clax.shell.windows(), typeof clax.echo]",
                        ok(json!([2, "undefined"])),
                    )
                },
                Case {
                    ops: ops(&["echo"]),
                    ..case(
                        "each_script_sees_only_its_own_ops_2",
                        "return [typeof clax.shell, typeof clax.echo, typeof clax.files, typeof globalThis.leak]",
                        ok(json!(["undefined", "function", "undefined", "undefined"])),
                    )
                },
                sees_only_the_sandboxs("each_script_sees_only_its_own_ops_3"),
            ],
        },
        Sequence {
            name: "a_scripts_ops_do_not_outlive_its_error",
            steps: vec![
                Case {
                    ops: shell(),
                    ..case(
                        "a_scripts_ops_do_not_outlive_its_error_1",
                        "globalThis.leak = clax; throw new Error('boom')",
                        fails(ErrorKind::Thrown, "boom"),
                    )
                },
                sees_only_the_sandboxs("a_scripts_ops_do_not_outlive_its_error_2"),
            ],
        },
        Sequence {
            name: "a_scripts_ops_do_not_outlive_its_cancel",
            steps: vec![
                Case {
                    ops: shell(),
                    cancel_in_call: true,
                    calls: vec![call("shell.windows", json!([]), Value::Null)],
                    records: Records::Exactly(vec![("shell.windows", false, Some(CODE_CANCELLED))]),
                    ..case(
                        "a_scripts_ops_do_not_outlive_its_cancel_1",
                        "globalThis.leak = clax; await clax.shell.windows(); return 1",
                        fails(ErrorKind::Cancelled, ""),
                    )
                },
                sees_only_the_sandboxs("a_scripts_ops_do_not_outlive_its_cancel_2"),
            ],
        },
        Sequence {
            name: "a_scripts_ops_do_not_outlive_its_limit",
            steps: vec![
                Case {
                    ops: shell(),
                    limits: Limits {
                        max_calls: 1,
                        ..Limits::default()
                    },
                    calls: vec![call("shell.windows", json!([]), json!(1))],
                    ..case(
                        "a_scripts_ops_do_not_outlive_its_limit_1",
                        "globalThis.leak = clax; for (;;) await clax.shell.windows()",
                        fails(ErrorKind::CallLimit, ""),
                    )
                },
                sees_only_the_sandboxs("a_scripts_ops_do_not_outlive_its_limit_2"),
            ],
        },
        Sequence {
            name: "a_park_leaves_the_sandbox_as_it_was",
            steps: vec![
                Case {
                    ops: shell(),
                    calls: vec![waits("shell.windows", json!([]))],
                    ..case(
                        "a_park_leaves_the_sandbox_as_it_was_1",
                        "globalThis.leak = clax; await clax.shell.windows(); return 1",
                        fails(ErrorKind::Parked, ""),
                    )
                },
                sees_only_the_sandboxs("a_park_leaves_the_sandbox_as_it_was_2"),
            ],
        },
        Sequence {
            name: "a_refused_list_leaves_the_sandbox_as_it_was",
            steps: vec![
                refused_list(
                    "a_refused_list_leaves_the_sandbox_as_it_was_1",
                    vec!["shell.windows".into(), "shell".into()],
                    "both an operation and a namespace",
                ),
                sees_only_the_sandboxs("a_refused_list_leaves_the_sandbox_as_it_was_2"),
            ],
        },
    ]
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
            calls: vec![
                Exchange {
                    op: "echo",
                    args: json!([1]),
                    reply: HostReply::Answer(Err(HostError::new(
                        "web_unavailable",
                        "the source failed",
                    )
                    .retryable())),
                },
                call_err("echo", json!([2]), "web_cap", "the run used its searches"),
            ],
            ..case(
                "a_host_error_says_whether_it_is_retryable",
                r#"const out = [];
for (const n of [1, 2]) {
  try { await clax.echo(n) } catch (e) { out.push([e.code, e.retryable]) }
}
return out"#,
                ok(json!([["web_unavailable", true], ["web_cap", false]])),
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
    /// Cancelled when the first expected call arrives, which is then never
    /// answered.
    cancel: Option<CancelToken>,
}

impl ScriptedHost {
    #[must_use]
    pub fn new(calls: Vec<Exchange>) -> Self {
        ScriptedHost {
            calls,
            state: Mutex::new((0, Vec::new())),
            cancel: None,
        }
    }

    /// The same host, cancelling `token` when it takes its first expected
    /// call, and never answering that call.
    #[must_use]
    pub fn cancelling(self, token: CancelToken) -> Self {
        ScriptedHost {
            cancel: Some(token),
            ..self
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
        match self.ask(op, args).await {
            HostReply::Answer(result) => result,
            HostReply::Waiting => Err(HostError::new(
                "conformance",
                "the scripted host has no answer to this call yet",
            )),
        }
    }

    async fn ask(&self, op: &str, args: Value) -> HostReply {
        let reply = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = state.0;
            match self.calls.get(next) {
                Some(want) if want.op == op && want.args == args => {
                    state.0 += 1;
                    want.reply.clone()
                }
                want => {
                    let want =
                        want.map_or("no call".to_string(), |w| format!("{} {}", w.op, w.args));
                    let shown: String = args.to_string().chars().take(200).collect();
                    state
                        .1
                        .push(format!("call {next}: got {op} {shown}, expected {want}"));
                    return HostReply::Answer(Err(HostError::new(
                        "conformance",
                        "unexpected call",
                    )));
                }
            }
        };
        if let Some(token) = &self.cancel {
            token.cancel();
            std::future::pending::<()>().await;
        }
        reply
    }
}

/// Runs `case` on `sandbox` and checks its outcome; the outcome on success.
pub async fn check(sandbox: &dyn Sandbox, case: &Case) -> Result<ScriptOutcome, String> {
    let cancel = CancelToken::new();
    let mut host = ScriptedHost::new(case.calls.clone());
    if case.cancel_in_call {
        host = host.cancelling(cancel.clone());
    }
    let script = case.script();
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
        Records::FromCalls => Some(case.calls.iter().map(Exchange::record).collect()),
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

/// Runs `sequence`'s steps on `sandbox`, one after the other, checking
/// each as [`check`] does; the first step's problems, if any.
pub async fn check_sequence(sandbox: &dyn Sandbox, sequence: &Sequence) -> Result<(), String> {
    for (i, step) in sequence.steps.iter().enumerate() {
        check(sandbox, step)
            .await
            .map_err(|e| format!("step {} ({}): {e}", i + 1, step.name))?;
    }
    Ok(())
}

/// Runs every case, then every sequence, on `sandbox`, one at a time: each
/// one's name and its result.
pub async fn run_all(sandbox: &dyn Sandbox) -> Vec<(&'static str, Result<(), String>)> {
    let mut results = Vec::new();
    for case in cases() {
        let result = check(sandbox, &case).await.map(|_| ());
        results.push((case.name, result));
    }
    for sequence in sequences() {
        let result = check_sequence(sandbox, &sequence).await;
        results.push((sequence.name, result));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every case, the steps of every sequence among them.
    fn every_case() -> Vec<Case> {
        let mut all = cases();
        all.extend(sequences().into_iter().flat_map(|s| s.steps));
        all
    }

    #[test]
    fn case_names_are_unique() {
        let mut names: Vec<_> = every_case().iter().map(|c| c.name).collect();
        names.extend(sequences().iter().map(|s| s.name));
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn every_scripted_call_uses_a_listed_op() {
        assert_eq!(crate::validate_ops(OPS), Ok(()));
        for case in every_case() {
            let listed: Vec<String> = case
                .ops
                .clone()
                .unwrap_or_else(|| OPS.iter().map(|o| (*o).to_string()).collect());
            for call in &case.calls {
                assert!(
                    listed.iter().any(|o| o == call.op),
                    "{}: {}",
                    case.name,
                    call.op
                );
                assert!(call.args.is_array(), "{}: args are an array", case.name);
            }
        }
    }

    #[test]
    fn a_case_expects_invalid_ops_exactly_when_its_list_is_refused() {
        for case in every_case() {
            let refused = case
                .ops
                .as_ref()
                .is_some_and(|o| crate::validate_ops(o).is_err());
            let expected = matches!(case.expect.error, Some((ErrorKind::InvalidOps, _)));
            assert_eq!(refused, expected, "{}", case.name);
        }
    }

    #[test]
    fn the_largest_lists_are_at_their_limits() {
        let most = most_ops();
        assert_eq!(most.len(), MAX_OPS);
        assert_eq!(most[MAX_OPS - 2], "s.o4094");
        assert_eq!(longest_op(), format!("l.{}", "x".repeat(4094)));
        assert_eq!(longest_op().len(), MAX_OP_BYTES);
        let heaviest = heaviest_ops(MAX_OPS_BYTES / MAX_OP_BYTES);
        assert_eq!(
            heaviest.iter().map(String::len).sum::<usize>(),
            MAX_OPS_BYTES
        );
        assert_eq!(heaviest[63], format!("b063.{}", "x".repeat(4091)));
        assert_eq!(crate::validate_ops(&most), Ok(()));
        assert_eq!(crate::validate_ops(&heaviest), Ok(()));
    }

    #[test]
    fn every_case_but_the_invalid_one_has_valid_limits() {
        for case in every_case() {
            let valid = case.limits.validate().is_ok();
            let wants_invalid = matches!(case.expect.error, Some((ErrorKind::InvalidLimits, _)));
            assert_eq!(valid, !wants_invalid, "{}", case.name);
        }
    }
}
