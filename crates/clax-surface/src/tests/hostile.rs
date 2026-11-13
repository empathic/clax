//! Input made to be too deep or too large: each is refused, with its
//! line, on a small stack, and none is refused by a clock. Where the
//! question is how much work something takes, the work is counted.

use super::{
    DEEP_STACK, VALUE_STACK, file, held as check_held, host, host_within, on_a_small_stack,
    on_a_stack, refused,
};
use crate::check::{Decl, Notation, error_codes, inherited, interface_names};
use crate::dts::read_doc;
use crate::model::Index;
use crate::value::measured;
use crate::{
    Access, Author, Definition, Interface, Limits, Op, Reserved, Ty, ValueErrorKind, declaration,
    print, print_within,
};
use serde_json::{Value, json};

/// A type nested so many levels deep.
type Nested = fn(usize) -> String;

/// A value one level deeper than the one given.
type Wrap = fn(Value) -> Value;

/// A file in which the given name names one kind of thing.
type Named = fn(&str) -> String;

/// The order a chain's interfaces are declared in: the place of each.
type Order = Box<dyn Fn(usize) -> usize>;

/// A parameter of this type, in a file whose members start on line 2.
fn with_type(ty: &str) -> String {
    file(&format!("/** D. @read */\nx(a?: {ty}): Promise<null>;"), "")
}

#[test]
fn types_nested_without_bound_are_refused_and_not_followed() {
    let deep: [(&str, Nested); 6] = [
        ("object types", |d| {
            format!("{}string{}", "{a:".repeat(d), "}".repeat(d))
        }),
        ("parentheses", |d| {
            format!("{}string{}", "(".repeat(d), ")".repeat(d))
        }),
        ("unions in parentheses", |d| {
            let open: String = (0..d).map(|i| format!("(\"l{i}\"|")).collect();
            format!("{open}string{}", ")".repeat(d))
        }),
        ("records", |d| {
            format!("{}string{}", "Record<string,".repeat(d), ">".repeat(d))
        }),
        ("arrays", |d| format!("string{}", "[]".repeat(d))),
        ("arrays of object types", |d| {
            format!("{}string{}", "{a:".repeat(d), "}[]".repeat(d))
        }),
    ];
    for (what, make) in deep {
        // Far past the limit: 10,000 levels fit a file of 64 KiB.
        for levels in [33, 100, 10_000] {
            let src = with_type(&make(levels));
            if src.len() > Limits::default().file_bytes {
                continue;
            }
            let e = on_a_small_stack(move || host(&src)).unwrap_err();
            assert_eq!(
                (e.line().unwrap(), e.message.as_str()),
                (
                    3,
                    "nested more than 32 deep, the limit for namespaces and for types"
                ),
                "{what}, {levels} deep"
            );
        }
        // At the limit a type is read, printed, shown to a model and
        // dropped, all on the same small stack. Parentheses count only
        // while they are read; an array of object types is two levels
        // a pair.
        let levels = if what == "arrays of object types" {
            16
        } else {
            32
        };
        let src = with_type(&make(levels));
        on_a_small_stack(move || {
            let d = host(&src).unwrap_or_else(|e| panic!("{what}, {levels} deep: {e}"));
            let printed = print(&d);
            assert_eq!(host(&printed).unwrap(), d, "{what}");
            assert!(declaration(&[&d], &|_| true).is_ok());
        });
    }
}

#[test]
fn namespaces_nested_without_bound_are_refused() {
    for levels in [33, 10_000] {
        let src = format!(
            "declare const demo: {}{{}}{};",
            "{\na:".repeat(levels),
            "}".repeat(levels)
        );
        let e = on_a_small_stack(move || host(&src)).unwrap_err();
        assert_eq!(
            (e.line().unwrap(), e.message.as_str()),
            (
                34,
                "nested more than 32 deep, the limit for namespaces and for types"
            )
        );
    }
    let src = format!(
        "declare const demo: {}{{ /** D. @read */ x(): Promise<null>; }}{};",
        "{a:".repeat(32),
        "}".repeat(32)
    );
    on_a_small_stack(move || {
        let d = host(&src).unwrap();
        assert_eq!(d.operations().len(), 1);
        assert_eq!(host(&print(&d)).unwrap(), d);
        assert!(
            declaration(&[&d], &|_| true)
                .unwrap()
                .contains("x(): Promise<null>;")
        );
    });
}

#[test]
fn a_file_larger_than_the_limit_is_refused_before_it_is_read() {
    let limits = Limits::default();
    assert_eq!(limits.file_bytes, 64 * 1024);
    // Nothing in it is looked at: not its nesting, not its characters.
    for filler in ["{", "\0", "/** unclosed "] {
        let src = filler.repeat(10 * 1024 * 1024 / filler.len());
        let e = host(&src).unwrap_err();
        assert_eq!(
            (e.line().unwrap(), e.message),
            (
                1,
                format!(
                    "the file is {} bytes long, and the limit is 65536",
                    src.len()
                )
            )
        );
    }
    // One byte over, and exactly at the limit.
    let fill = |bytes: usize| {
        let head = "declare const demo: {};\n//";
        format!("{head}{}", "x".repeat(bytes - head.len()))
    };
    assert!(host(&fill(64 * 1024)).is_ok());
    refused(
        &fill(64 * 1024 + 1),
        1,
        "the file is 65537 bytes long, and the limit is 65536",
    );
    // The limit is the environment's to change.
    let small = Limits {
        file_bytes: 30,
        ..Limits::default()
    };
    assert!(host_within("declare const demo: {};", &small).is_ok());
    assert!(host_within("declare const demo: {};        ", &small).is_err());
    let large = Limits {
        file_bytes: 1024 * 1024,
        ..Limits::default()
    };
    assert!(host_within(&fill(64 * 1024 + 1), &large).is_ok());
}

#[test]
fn rules_and_comments_have_a_longest_length() {
    let rules = |bytes: usize| format!("/** {} */\ndeclare const demo: {{}};", "r".repeat(bytes));
    assert!(host(&rules(4096)).is_ok());
    refused(
        &rules(4097),
        2,
        "the surface's rules is 4097 bytes long, and the limit is 4096",
    );
    let ns = |bytes: usize| file(&format!("/** {} */\nns: {{}};", "r".repeat(bytes)), "");
    assert!(host(&ns(4096)).is_ok());
    refused(
        &ns(4097),
        3,
        "the rules of `demo.ns` is 4097 bytes long, and the limit is 4096",
    );
    let doc = |bytes: usize| {
        file(
            &format!("/** {} @read */\nx(): Promise<null>;", "d".repeat(bytes)),
            "",
        )
    };
    assert!(host(&doc(4096)).is_ok());
    refused(
        &doc(4097),
        3,
        "the documentation of operation `demo.x` is 4097 bytes long, and the limit is 4096",
    );
    let tight = Limits {
        rules_bytes: 8,
        ..Limits::default()
    };
    assert!(host_within(&rules(8), &tight).is_ok());
    assert!(host_within(&rules(9), &tight).is_err());
}

#[test]
fn lists_have_a_longest_length() {
    let ops = |n: usize| {
        let body: String = (0..n)
            .map(|i| format!("/** D. @read */ o{i}(): Promise<null>;\n"))
            .collect();
        file(&format!("a: {{\n{body}}};"), "")
    };
    assert!(host(&ops(128)).is_ok());
    refused(&ops(129), 2, "`a` has 129 members, and the limit is 128");
    let fields = |n: usize| {
        let body: String = (0..n).map(|i| format!("f{i}: string;\n")).collect();
        file("", &format!("interface A {{\n{body}}}"))
    };
    assert!(host(&fields(128)).is_ok());
    refused(
        &fields(129),
        4,
        "interface `A` has 129 fields, and the limit is 128",
    );
    let bases = |n: usize| {
        let declared: String = (0..n).map(|i| format!("interface B{i} {{}}\n")).collect();
        let named: Vec<String> = (0..n).map(|i| format!("B{i}")).collect();
        file(
            "",
            &format!("{declared}interface A extends {} {{}}", named.join(", ")),
        )
    };
    assert!(host(&bases(128)).is_ok());
    refused(
        &bases(129),
        133,
        "interface `A` extends 129 interfaces, and the limit is 128",
    );
    let alts = |n: usize| {
        with_type(
            &(0..n)
                .map(|i| format!("\"v{i}\""))
                .collect::<Vec<_>>()
                .join("|"),
        )
    };
    assert!(host(&alts(128)).is_ok());
    refused(
        &alts(129),
        3,
        "has a union of 129 alternatives, where the limit is 128",
    );
    // One line a code: the refusal is on the line of the first too many.
    let codes = |n: usize| {
        let codes: String = (0..n).map(|i| format!("\n * @throws c{i}")).collect();
        file(
            &format!("/** D.\n * @read{codes}\n */\nx(): Promise<null>;"),
            "",
        )
    };
    assert!(host(&codes(128)).is_ok());
    refused(
        &codes(129),
        132,
        "operation `demo.x` names 129 error codes, and the limit is 128",
    );
    let params = |n: usize| {
        let params: Vec<String> = (0..n).map(|i| format!("p{i}?: null")).collect();
        file(
            &format!("/** D. @read */\nx({}): Promise<null>;", params.join(", ")),
            "",
        )
    };
    assert!(host(&params(128)).is_ok());
    refused(
        &params(129),
        3,
        "operation `demo.x` has 129 parameters, and the limit is 128",
    );
    let chain = |n: usize| {
        let body: String = (1..n)
            .map(|i| format!("interface I{i} extends I{} {{}}\n", i - 1))
            .collect();
        file("", &format!("interface I0 {{}}\n{body}"))
    };
    assert!(host(&chain(17)).is_ok());
    refused(
        &chain(18),
        21,
        "interface `I17` is 17 steps of `extends` from the end of its chain, and the limit is 16",
    );
    // An environment that allows more gets more.
    let more = Limits {
        list: 200,
        extends_depth: 20,
        ..Limits::default()
    };
    assert!(host_within(&ops(129), &more).is_ok());
    assert!(host_within(&chain(18), &more).is_ok());
}

/// The longest chain of `extends` a file of 64 KiB can hold is checked in
/// steps proportional to its length, whatever order it is declared in.
#[test]
fn a_chain_of_extends_is_checked_in_one_walk() {
    let chain = |order: &dyn Fn(usize) -> usize, n: usize| -> Vec<Interface> {
        (0..n)
            .map(|at| {
                let i = order(at);
                Interface {
                    name: format!("a{i}"),
                    doc: None,
                    extends: if i == 0 {
                        Vec::new()
                    } else {
                        vec![format!("a{}", i - 1)]
                    },
                    fields: Vec::new(),
                }
            })
            .collect()
    };
    let limits = Limits {
        interfaces: 10_000,
        extends_depth: 10_000,
        ..Limits::default()
    };
    for n in [2_185, 8_740] {
        let orders: [(&str, Order); 2] = [
            ("base first", Box::new(|at| at)),
            ("base last", Box::new(move |at| n - 1 - at)),
        ];
        for (order, f) in orders {
            let interfaces = chain(&*f, n);
            let mut index = Index::default();
            interface_names(&interfaces, &limits, &mut index, Notation::Dts).unwrap();
            let steps = inherited(&interfaces, &limits, &mut index, Notation::Dts).unwrap();
            assert!(steps <= 3 * n, "{n} interfaces, {order}: {steps} steps");
        }
    }
    // And the whole reader takes such a file, when its limits allow one.
    let src: String = std::iter::once("declare const d:{};interface a0{}".to_string())
        .chain((1..2_185).map(|i| format!("interface a{i} extends a{}{{}}", i - 1)))
        .collect();
    assert!(src.len() <= 64 * 1024);
    assert_eq!(
        host_within(&src, &limits).unwrap().interfaces().len(),
        2_185
    );
    assert!(
        host(&src)
            .unwrap_err()
            .message
            .contains("the file declares 2185 interfaces, and the limit is 256")
    );
}

/// `{"c": {"c": … leaf … }}`, `depth` deep, built without recursion.
fn nested(key: &str, depth: usize, leaf: Value) -> Value {
    (0..depth).fold(leaf, |v, _| {
        let mut o = serde_json::Map::new();
        o.insert(key.to_string(), v);
        Value::Object(o)
    })
}

/// A definition with these interfaces, and the type of field `v` of `T`,
/// with `nodes` comparisons to spend on one check.
fn types(interfaces: &str, nodes: usize) -> (crate::Definition, Ty) {
    let limits = Limits {
        value_nodes: nodes,
        ..Limits::default()
    };
    let d = host_within(&file("", interfaces), &limits).unwrap();
    let ty = d.interface("T").unwrap().fields[0].ty.clone();
    (d, ty)
}

#[test]
fn a_union_is_checked_in_work_proportional_to_the_value() {
    // Told apart by a literal field, wrong at the bottom, 100 deep: each
    // level says which alternative it is, so each is looked at once.
    let (d, ty) = types(
        "interface T { v: N | L }\ninterface N { k: \"n\"; c: N | L }\ninterface L { k: \"l\" }",
        1_000,
    );
    let bad = (0..100).fold(json!({"k": "x"}), |v, _| json!({"k": "n", "c": v}));
    let path = ".c".repeat(100);
    assert_eq!(
        check_held(&d, &ty, &bad),
        Err(format!("value{path} must be N | L"))
    );
    let good = (0..100).fold(json!({"k": "l"}), |v, _| json!({"k": "n", "c": v}));
    assert_eq!(check_held(&d, &ty, &good), Ok(()));
    let wrong_field = (0..100).fold(
        json!({"k": "l", "more": 1}),
        |v, _| json!({"k": "n", "c": v}),
    );
    assert_eq!(
        check_held(&d, &ty, &wrong_field),
        Err(format!("value{path} has no field \"more\" (fields: k)"))
    );

    // No literal field: two alternatives that both recurse. Each object
    // is compared with each type once, so 100 levels fit in 1,000 units
    // of work, where trying every path through them would take 2^100.
    let (d, ty) = types(
        "interface T { v: A | B }\ninterface A { c?: A | B; a?: null }\ninterface B { c?: A | B; b?: null }",
        1_000,
    );
    assert_eq!(
        check_held(&d, &ty, &nested("c", 100, json!(5))),
        Err("value must be A | B".to_string())
    );
    assert_eq!(
        check_held(&d, &ty, &nested("c", 100, json!({"b": null}))),
        Ok(())
    );
    // The same through interfaces both alternatives share.
    let (d, ty) = types(
        "interface T { v: A | B }\ninterface A { c?: X; a?: null }\ninterface B { c?: X; b?: null }\ninterface X { c?: A | B }",
        1_000,
    );
    assert_eq!(
        check_held(&d, &ty, &nested("c", 100, json!(5))),
        Err("value must be A | B".to_string())
    );
}

#[test]
fn a_check_stops_at_its_depth_and_at_its_budget() {
    let (d, ty) = types("interface T { v: N }\ninterface N { c?: N }", 1_000_000);
    assert_eq!(check_held(&d, &ty, &nested("c", 128, json!({}))), Ok(()));
    for depth in [129, 20_000] {
        let v = nested("c", depth, json!({}));
        let (d, ty) = (d.clone(), ty.clone());
        let r = on_a_small_stack(move || {
            let r = check_held(&d, &ty, &v);
            // Dropping a value this deep is its owner's to do with care.
            std::mem::forget(v);
            r
        });
        assert_eq!(
            r,
            Err("value is nested more than 128 deep, the limit: it was not checked".to_string()),
            "{depth}"
        );
    }
    // `unknown` looks no further, however deep the value.
    let (d, ty) = types("interface T { v: unknown }", 10);
    let v = nested("c", 20_000, json!(5));
    assert_eq!(check_held(&d, &ty, &v), Ok(()));
    std::mem::forget(v);

    // A value with more parts than the budget. A type handed to
    // `check_held` is prepared for the call, a unit for each of its
    // steps: two here, for `Int` and its array.
    let (d, ty) = types("interface T { v: Int[] }", 1_000);
    let numbers = |n: u64| Value::Array((0..n).map(Value::from).collect());
    assert_eq!(check_held(&d, &ty, &numbers(997)), Ok(()));
    assert_eq!(
        check_held(&d, &ty, &numbers(998)),
        Err("value is too large to check: the limit on the work of one check is 1000".to_string())
    );
    // The budget is one check's: the next starts afresh.
    assert_eq!(check_held(&d, &ty, &numbers(997)), Ok(()));
    // The definition's own types are ready, and cost a check nothing.
    let own = host_within(
        &file("/** D. @read */\nx(): Promise<Int[]>;", ""),
        d.limits(),
    )
    .unwrap();
    let x = own.operation("demo.x").unwrap();
    assert_eq!(x.check_result(&numbers(999)), Ok(()));
    assert_eq!(
        x.check_result(&numbers(1_000)).unwrap_err().kind,
        ValueErrorKind::TooLarge
    );
    // A union does not hide that a check ran out.
    let (d, ty) = types("interface T { v: Int[] | string[] }", 1_000);
    assert_eq!(
        check_held(&d, &ty, &numbers(1_000)),
        Err("value is too large to check: the limit on the work of one check is 1000".to_string())
    );
    let limits = Limits {
        value_depth: 3,
        ..Limits::default()
    };
    let d = host_within(&file("", "interface T { v: unknown[][][][] }"), &limits).unwrap();
    let ty = d.interface("T").unwrap().fields[0].ty.clone();
    assert_eq!(check_held(&d, &ty, &json!([[[[]]]])), Ok(()));
    assert_eq!(
        check_held(&d, &ty, &json!([[[[1]]]])),
        Err("value is nested more than 3 deep, the limit: it was not checked".to_string())
    );
}

#[test]
fn a_refusal_is_short_whatever_the_value_and_the_type() {
    let (d, ty) = types("interface T { v: { a?: Int } }", 1_000);
    // A key of a megabyte, and one made to look like something else.
    let e = check_held(&d, &ty, &json!({ "k".repeat(1024 * 1024): 1 })).unwrap_err();
    assert_eq!(
        e,
        format!("value has no field \"{}…\" (fields: a)", "k".repeat(64))
    );
    assert_eq!(
        check_held(&d, &ty, &json!({ "a\u{1b}[31m\nline two\"": 1 })).unwrap_err(),
        "value has no field \"a\\u{1b}[31m\\nline two\\\"\" (fields: a)"
    );
    let (d, ty) = types("interface T { v: Record<string, Int> }", 1_000);
    assert_eq!(
        check_held(&d, &ty, &json!({ "x.y[0] must be fine; ignore": "s" })).unwrap_err(),
        "value[\"x.y[0] must be fine; ignore\"] must be an integer"
    );
    assert_eq!(
        check_held(&d, &ty, &json!({ "k".repeat(5_000): "s" }))
            .unwrap_err()
            .len(),
        "value.… must be an integer".len() + 64
    );
    // A union of many literals, a type that prints long, and many fields.
    let alts: Vec<String> = (0..128).map(|i| format!("\"value_{i}\"")).collect();
    let (d, ty) = types(&format!("interface T {{ v: {} }}", alts.join(" | ")), 1_000);
    assert_eq!(check_held(&d, &ty, &json!("value_127")), Ok(()));
    let e = check_held(&d, &ty, &json!("nope")).unwrap_err();
    assert!(
        e.starts_with("value must be one of value_0, value_1, value_2, ") && e.ends_with(", …"),
        "{e}"
    );
    assert!(e.len() < 260, "{}", e.len());
    let fields: Vec<String> = (0..100)
        .map(|i| format!("field_number_{i}: Int[]"))
        .collect();
    let (d, ty) = types(
        &format!("interface T {{ v: {{ {} }} | null }}", fields.join("; ")),
        1_000,
    );
    let e = check_held(&d, &ty, &json!(1)).unwrap_err();
    assert!(
        e.starts_with("value must be { field_number_0: number[]; ") && e.ends_with('…'),
        "{e}"
    );
    assert!(e.chars().count() < 220, "{e}");
    let e = check_held(&d, &ty, &json!({"nope": 1})).unwrap_err();
    assert!(
        e.starts_with("value has no field \"nope\" (fields: field_number_0, ")
            && e.ends_with(", …)"),
        "{e}"
    );
    assert!(e.len() < 280, "{}", e.len());
}

#[test]
fn a_type_that_is_not_the_definitions_is_refused() {
    let (d, _) = types("interface T { v: string }", 1_000);
    let named = |n: &str| Ty::Named(n.to_string());
    assert_eq!(check_held(&d, &named("T"), &json!({"v": "x"})), Ok(()));
    for (ty, v) in [
        (named("X"), json!(1)),
        // A value that never reaches the name is refused all the same.
        (Ty::Array(Box::new(named("X"))), json!([])),
        (Ty::union(Ty::Null, [named("X")]), json!(null)),
        (
            Ty::Object(vec![crate::Field {
                name: "a".into(),
                optional: true,
                ty: named("X"),
                doc: None,
            }]),
            json!({}),
        ),
    ] {
        assert_eq!(
            check_held(&d, &ty, &v),
            Err("the value cannot be checked: its type names `X`, which the surface does not declare".to_string()),
            "{ty:?}"
        );
        assert!(super::sent(&d, &ty, &v).is_err());
    }
    // A type deeper than the definition's limit, built outside it.
    let deep = (0..10_000).fold(Ty::Str, |t, _| Ty::Array(Box::new(t)));
    let r = check_held(&d, &deep, &json!([]));
    // Dropping a type this deep is its maker's to do with care.
    std::mem::forget(deep);
    assert_eq!(
        r,
        Err("the value cannot be checked: its type is beyond the surface's limits: a type nested more than 32 deep, the limit".to_string())
    );
}

/// The definition of `shape`, and the type of field `v` of its `T`.
fn shaped(shape: &str) -> (Definition, Ty) {
    let d = host(shape).unwrap_or_else(|e| panic!("{e}"));
    let ty = d.interface("T").unwrap().fields[0].ty.clone();
    (d, ty)
}

/// 127 interfaces that extend one whose field `a` is a union of 77
/// object types of 81 fields each, and one whose `a` is a number: an
/// object `{"a": 1}` is the last of 128 alternatives, and each of the
/// others fails on a type that prints as 55 KB.
fn long_printing_alternatives() -> String {
    let big: Vec<String> = (0..77)
        .map(|i| {
            let fields: Vec<String> = (0..80).map(|f| format!("f{f}:null")).collect();
            format!("{{k{i}:null;{}}}", fields.join(";"))
        })
        .collect();
    let alts: Vec<String> = (0..127).map(|i| format!("W{i}")).collect();
    let heirs: String = alts
        .iter()
        .map(|a| format!("interface {a} extends W{{}}"))
        .collect();
    format!(
        "declare const d:{{}};interface T{{v:({}|Y)[]}}interface W{{a:{}}}interface Y{{a:number}}{heirs}",
        alts.join("|"),
        big.join("|")
    )
}

#[test]
fn an_alternative_that_does_not_fit_costs_what_was_spent_finding_out() {
    let src = long_printing_alternatives();
    assert!(
        src.len() > 59_000 && src.len() <= 64 * 1024,
        "{}",
        src.len()
    );
    let (d, ty) = shaped(&src);
    let elements = |n: usize, a: Value| Value::Array((0..n).map(|_| json!({ "a": a })).collect());
    // Each element: the union, then for each of 128 alternatives the
    // object, its one key, and the value under it. Nothing is put into
    // words, and nothing is remembered: a number is looked at in one
    // step.
    for n in [1, 10, 100] {
        let (r, work) = measured(&d, &ty, &elements(n, json!(1)));
        assert_eq!(r, Ok(()));
        assert_eq!(work.units, 1 + n * (1 + 128 * 3), "{n} elements");
        assert_eq!((work.worded, work.remembered), (0, 0), "{n} elements");
    }
    // A value that fits no alternative is refused in one message, which
    // names the type in a few words however long it prints.
    let (r, work) = measured(&d, &ty, &elements(100, json!("x")));
    let e = r.unwrap_err();
    assert_eq!(e.kind, ValueErrorKind::Wrong);
    assert!(
        e.message.starts_with("value[0] must be W0 | W1 | W2 | ") && e.message.ends_with('…'),
        "{e}"
    );
    assert!(e.message.chars().count() < 220, "{e}");
    assert_eq!(work.units, 1 + 1 + 128 * 3);
    assert_eq!(work.worded, 1);
    // The type that prints long is itself what a value must be.
    let big = d.interface("W").unwrap().fields[0].ty.clone();
    let (r, work) = measured(&d, &big, &json!(1));
    let e = r.unwrap_err().message;
    assert!(
        e.starts_with("value must be { k0: null; f0: null; ") && e.chars().count() < 220,
        "{e}"
    );
    assert_eq!((work.units, work.worded), (1, 1));
    // The value the budget stops is stopped by the count, not by a clock.
    let (r, work) = measured(&d, &ty, &elements(2_598, json!(1)));
    assert_eq!(r.unwrap_err().kind, ValueErrorKind::TooLarge);
    assert_eq!(work.units, 1_000_000);
    assert_eq!(measured(&d, &ty, &elements(2_597, json!(1))).0, Ok(()));
}

#[test]
fn telling_alternatives_apart_is_a_lookup_and_is_counted() {
    // Alternatives that each say which they are, under one name: one
    // look at that name, however many alternatives there are.
    let told = |alts: usize| {
        let interfaces: String = (0..alts)
            .map(|i| format!("interface A{i}{{kind:\"k{i}\";n:Int}}"))
            .collect();
        let names: Vec<String> = (0..alts).map(|i| format!("A{i}")).collect();
        format!(
            "declare const d:{{}};interface T{{v:({})[]}}{interfaces}",
            names.join("|")
        )
    };
    let values = |n: usize, kind: &str| {
        Value::Array((0..n).map(|_| json!({"kind": kind, "n": 1})).collect())
    };
    for alts in [2, 128] {
        let (d, ty) = shaped(&told(alts));
        let last = format!("k{}", alts - 1);
        let (r, work) = measured(&d, &ty, &values(50, &last));
        assert_eq!(r, Ok(()), "{alts} alternatives");
        // The union and its one telling name; the alternative, its two
        // keys and their two values.
        assert_eq!(work.units, 1 + 50 * (2 + 5), "{alts} alternatives");
        assert_eq!(work.remembered, 0);
        // A value that names no alternative is refused for the union,
        // and one that names one for what is wrong with that one.
        let e = check_held(&d, &ty, &values(1, "nope")).unwrap_err();
        assert!(e.starts_with("value[0] must be A0 | A1"), "{e}");
        assert_eq!(
            check_held(&d, &ty, &json!([{"kind": last, "n": "x"}])),
            Err("value[0].n must be an integer".to_string())
        );
    }

    // 128 interfaces that inherit the same 100 required fields, each a
    // union of 100 literals: no field tells one from another, and every
    // look is counted. Each element: the union and 100 names; the first
    // alternative, its 100 keys and their 100 values.
    let literals: Vec<String> = (0..100).map(|i| format!("\"{i}\"")).collect();
    let fields: String = (0..100)
        .map(|f| format!("f{f}:{};", literals.join("|")))
        .collect();
    let names: Vec<String> = (0..128).map(|i| format!("A{i}")).collect();
    let heirs: String = names
        .iter()
        .map(|a| format!("interface {a} extends B{{}}"))
        .collect();
    let (d, ty) = shaped(&format!(
        "declare const d:{{}};interface T{{v:({})[]}}interface B{{{fields}}}{heirs}",
        names.join("|")
    ));
    let one: serde_json::Map<String, Value> =
        (0..100).map(|f| (format!("f{f}"), json!("99"))).collect();
    let values = |n: usize| Value::Array((0..n).map(|_| Value::Object(one.clone())).collect());
    for n in [1, 20] {
        let (r, work) = measured(&d, &ty, &values(n));
        assert_eq!(r, Ok(()));
        assert_eq!(work.units, 1 + n * (101 + 201), "{n} elements");
        assert_eq!((work.worded, work.remembered), (0, 0));
    }
    // 4,950 of them were inside the budget when only values were
    // counted; counted in full, 3,311 are and 3,312 are not.
    assert_eq!(measured(&d, &ty, &values(3_311)).0, Ok(()));
    let (r, work) = measured(&d, &ty, &values(3_312));
    assert_eq!(r.unwrap_err().kind, ValueErrorKind::TooLarge);
    assert_eq!(work.units, 1_000_000);
    // One that holds another literal in its last field is none of them.
    let mut odd = one.clone();
    odd.insert("f99".into(), json!("100"));
    let (r, work) = measured(&d, &ty, &Value::Array(vec![Value::Object(odd)]));
    assert!(
        r.unwrap_err()
            .message
            .starts_with("value[0] must be A0 | A1 | ")
    );
    assert_eq!(work.units, 1 + 101);
}

#[test]
fn an_object_is_checked_in_steps_proportional_to_its_keys() {
    // 128 optional fields: an object costs one unit, and one for each
    // key it has and each value under one, whatever the type could hold.
    let fields: String = (0..128).map(|f| format!("f{f}?:Int;")).collect();
    let (d, ty) = shaped(&format!(
        "declare const d:{{}};interface T{{v:N[]}}interface N{{{fields}}}"
    ));
    for keys in [0, 1, 128] {
        let one: serde_json::Map<String, Value> =
            (0..keys).map(|f| (format!("f{f}"), json!(f))).collect();
        let v = Value::Array((0..10).map(|_| Value::Object(one.clone())).collect());
        let (r, work) = measured(&d, &ty, &v);
        assert_eq!(r, Ok(()));
        assert_eq!(work.units, 1 + 10 * (1 + 2 * keys), "{keys} keys");
    }
    // A key that is no field is found at once, among any number.
    let mut one: serde_json::Map<String, Value> =
        (0..128).map(|f| (format!("f{f}"), json!(f))).collect();
    one.insert("a".into(), json!(1));
    let (r, work) = measured(&d, &ty, &Value::Array(vec![Value::Object(one)]));
    assert!(
        r.unwrap_err()
            .message
            .starts_with("value[0] has no field \"a\" (fields: f0, f1, ")
    );
    assert_eq!(work.units, 3);
}

/// `{"c": … {"b": null} … }`, `depth` deep.
fn deep_union_value(depth: usize) -> Value {
    nested("c", depth, json!({"b": null}))
}

const UNION_OF_INTERFACES: &str = "interface T { v: A | B }\ninterface A { c?: A | B; a?: null }\ninterface B { c?: A | B; b?: null }";

#[test]
fn the_deepest_value_is_checked_on_a_stack_of_64_kib() {
    // Every level an object reached through a union of interfaces, as
    // deep as the default limits allow, and one deeper.
    let (d, ty) = types(UNION_OF_INTERFACES, 1_000_000);
    assert_eq!(d.limits().value_depth, 128);
    let checked = move |depth: usize, d: Definition, ty: Ty| {
        let v = deep_union_value(depth);
        on_a_stack(VALUE_STACK, move || {
            let r = crate::check_held(&d, &ty, &v);
            // Dropping a value this deep is its owner's to do with care.
            std::mem::forget(v);
            r
        })
    };
    assert_eq!(checked(128, d.clone(), ty.clone()), Ok(()));
    let e = checked(129, d.clone(), ty.clone()).unwrap_err();
    assert_eq!(e.kind, ValueErrorKind::TooDeep);
    assert_eq!(
        e.message,
        "value is nested more than 128 deep, the limit: it was not checked"
    );

    // No limit asks for more than the hard maximum, and a value that
    // deep is checked on the same stack: a check keeps its place on the
    // heap.
    let unbounded = Limits {
        value_depth: usize::MAX,
        ..Limits::default()
    };
    let d = host_within(&file("", UNION_OF_INTERFACES), &unbounded).unwrap();
    assert_eq!(d.limits().value_depth, Limits::MAX_VALUE_DEPTH);
    assert_eq!(Limits::MAX_VALUE_DEPTH, 1_024);
    assert_eq!(checked(1_024, d.clone(), ty.clone()), Ok(()));
    for depth in [1_025, 100_000] {
        let e = checked(depth, d.clone(), ty.clone()).unwrap_err();
        assert_eq!(e.kind, ValueErrorKind::TooDeep, "{depth}");
        assert_eq!(
            e.message,
            "value is nested more than 1024 deep, the limit: it was not checked"
        );
    }
    // A value wrong at the bottom fits no alternative at any level, and
    // finding that out takes a few units a level.
    let v = nested("c", 1_023, json!({"b": 5}));
    let (r, work) = measured(&d, &ty, &v);
    std::mem::forget(v);
    assert_eq!(r.unwrap_err().message, "value must be A | B");
    assert!(work.units < 8 * 1_024, "{}", work.units);
    assert_eq!(work.worded, 1);
    // One whose levels say which alternative they are is refused at its
    // place, however deep.
    let told = host_within(
        &file(
            "",
            "interface T { v: N | L }\ninterface N { k: \"n\"; c: N | L }\ninterface L { k: \"l\" }",
        ),
        &unbounded,
    )
    .unwrap();
    let ty = told.interface("T").unwrap().fields[0].ty.clone();
    let v = (0..1_000).fold(json!({"k": "x"}), |v, _| json!({"k": "n", "c": v}));
    let e = check_held(&told, &ty, &v).unwrap_err();
    std::mem::forget(v);
    assert_eq!(e, format!("value{} must be N | L", ".c".repeat(1_000)));
}

#[test]
fn a_definition_as_deep_as_the_hard_maximum_fits_a_stack_of_1_mib() {
    assert_eq!(Limits::MAX_DEPTH, 64);
    let unbounded = Limits {
        depth: usize::MAX,
        value_depth: usize::MAX,
        ..Limits::default()
    };
    assert_eq!(unbounded.clamped().depth, 64);
    let shapes: [(&str, Nested, Wrap); 5] = [
        (
            "object types",
            |d| format!("{}string{}", "{a:".repeat(d), "}".repeat(d)),
            |v| json!({ "a": v }),
        ),
        (
            "parentheses",
            |d| format!("{}string{}", "(".repeat(d), ")".repeat(d)),
            |v| v,
        ),
        (
            "unions in parentheses",
            |d| {
                let open: String = (0..d).map(|i| format!("(\"l{i}\"|")).collect();
                format!("{open}string{}", ")".repeat(d))
            },
            |v| v,
        ),
        (
            "records",
            |d| format!("{}string{}", "Record<string,".repeat(d), ">".repeat(d)),
            |v| json!({ "k": v }),
        ),
        (
            "arrays of unions of object types",
            |d| {
                format!(
                    "{}string{}",
                    "({a:".repeat(d / 2),
                    "}|null)[]".repeat(d / 2)
                )
            },
            |v| json!([{ "a": v }]),
        ),
    ];
    for (what, make, wrap) in shapes {
        let levels = if what == "arrays of unions of object types" {
            32
        } else {
            64
        };
        let ty = make(64);
        let src = format!(
            "declare const demo: {{\n/** D. @read */\nx(a?: {ty}): Promise<null>;\n}};\ninterface T {{ v: {ty} }}\ninterface Kinds {{\n/** K. @allow */\nk(request: {ty}): {{}};\n}}"
        );
        let limits = unbounded.clone();
        on_a_stack(DEEP_STACK, move || {
            let d = host_within(&src, &limits).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(d.limits().depth, 64);
            let printed = print(&d);
            assert_eq!(host_within(&printed, &limits).unwrap(), d, "{what}");
            assert!(declaration(&[&d], &|_| true).is_ok());
            let t = d.interface("T").unwrap().fields[0].ty.clone();
            let v = (0..levels).fold(json!("s"), |v, _| wrap(v));
            assert_eq!(check_held(&d, &t, &v), Ok(()), "{what}");
            assert!(check_held(&d, &t, &json!(5)).unwrap_err().len() < 300);
            let x = d.operation("demo.x").unwrap();
            assert!(x.check_args(std::slice::from_ref(&v)).is_ok(), "{what}");
            assert_eq!(d.kind("k").unwrap().check_request(&d, &v), Ok(()));
        });
        // One level more is refused, whatever limit was asked for.
        let over = with_type(&make(if levels == 32 { 66 } else { 65 }));
        let limits = unbounded.clone();
        let e = on_a_small_stack(move || host_within(&over, &limits)).unwrap_err();
        assert_eq!(
            e.message, "nested more than 64 deep, the limit for namespaces and for types",
            "{what}"
        );
    }
    let namespaces = |levels: usize| {
        format!(
            "declare const demo: {}{{ /** D. @read */ x(): Promise<null>; }}{};",
            "{a:".repeat(levels),
            "}".repeat(levels)
        )
    };
    let (src, limits) = (namespaces(64), unbounded.clone());
    on_a_stack(DEEP_STACK, move || {
        let d = host_within(&src, &limits).unwrap();
        assert_eq!(host_within(&print(&d), &limits).unwrap(), d);
        assert!(declaration(&[&d], &|_| true).is_ok());
    });
    let src = namespaces(65);
    assert!(
        on_a_small_stack(move || host_within(&src, &unbounded))
            .unwrap_err()
            .message
            .contains("nested more than 64 deep")
    );
}

/// The widest diamond a file of 64 KiB holds: one base of 128 fields,
/// 127 interfaces that extend it, and as many again as fit that extend
/// all of those. Every one of the last inherits each field 127 times
/// over, and is given it once.
#[test]
fn a_diamond_of_extends_is_checked_in_counted_steps() {
    let field = |name: String| crate::Field {
        name,
        optional: true,
        ty: Ty::Null,
        doc: None,
    };
    let interface = |name: String, extends: Vec<String>, fields: Vec<crate::Field>| Interface {
        name,
        doc: None,
        extends,
        fields,
    };
    let (fields, middles, tops) = (128, 127, 111);
    let mut interfaces = vec![interface(
        "B".into(),
        Vec::new(),
        (0..fields).map(|f| field(format!("f{f}"))).collect(),
    )];
    interfaces
        .extend((0..middles).map(|i| interface(format!("M{i}"), vec!["B".into()], Vec::new())));
    let all: Vec<String> = (0..middles).map(|i| format!("M{i}")).collect();
    interfaces.extend((0..tops).map(|i| interface(format!("N{i}"), all.clone(), Vec::new())));
    let limits = Limits::default();
    let mut index = Index::default();
    interface_names(&interfaces, &limits, &mut index, Notation::Dts).unwrap();
    let steps = inherited(&interfaces, &limits, &mut index, Notation::Dts).unwrap();
    // One step an interface, one for each `extends`, and one for each
    // field of each interface extended.
    let extends = middles + tops * middles;
    assert_eq!(
        steps,
        interfaces.len() + extends + extends * fields,
        "{} interfaces",
        interfaces.len()
    );
    assert!(steps < 2_000_000);
    assert!(index.fields.iter().all(|f| f.len() == fields));
    // The same as a file, which the reader takes whole.
    let written: String = interfaces
        .iter()
        .map(|i| {
            let fields: String = i
                .fields
                .iter()
                .map(|f| format!("{}?:null;", f.name))
                .collect();
            let extends = if i.extends.is_empty() {
                String::new()
            } else {
                format!(" extends {}", i.extends.join(","))
            };
            format!("interface {}{extends}{{{fields}}}", i.name)
        })
        .collect();
    let src = format!("declare const d:{{}};{written}");
    assert!(src.len() <= 64 * 1024, "{}", src.len());
    let d = host(&src).unwrap();
    assert_eq!(d.fields_of("N110").unwrap().len(), 128);
}

#[test]
fn a_doc_comment_is_read_in_steps_proportional_to_its_lines() {
    // Blank lines at either end are dropped: each line is looked at
    // once, and once more when it is one of those.
    for (what, raw, lines) in [
        (
            "blank lines first",
            format!("{}x ", "\n".repeat(65_000)),
            65_001,
        ),
        (
            "blank lines last",
            format!(" x{}", "\n".repeat(65_000)),
            65_001,
        ),
        ("only blank lines", "\n".repeat(65_000), 65_001),
        (
            "blank lines between",
            format!(" a{}b ", "\n".repeat(65_000)),
            65_001,
        ),
    ] {
        let (doc, steps) = read_doc(&raw, 1).unwrap();
        assert!(steps <= 2 * lines, "{what}: {steps} steps");
        match what {
            "only blank lines" => assert_eq!(doc.prose, ""),
            "blank lines between" => assert_eq!(doc.prose.len(), 65_002),
            _ => assert_eq!(doc.prose, "x", "{what}"),
        }
    }
    // And the reader takes such a file.
    let src = format!("/**{}x */\ndeclare const demo: {{}};", "\n".repeat(65_000));
    assert_eq!(host(&src).unwrap().rules(), Some("x"));
}

#[test]
fn error_codes_are_looked_at_once_each_and_not_at_all_past_their_limit() {
    let op = |codes: usize| Op {
        name: "x".into(),
        doc: "D.".into(),
        access: Access::Read,
        waits: false,
        once: false,
        throws: (0..codes).map(|i| format!("c{i}")).collect(),
        params: Vec::new(),
        result: Ty::Null,
    };
    let many = Limits {
        list: 16_000,
        ..Limits::default()
    };
    let reserved = Reserved::of(&[]);
    let as_host = Author::Host {
        reserved: &reserved,
    };
    assert_eq!(
        error_codes(&op(16_000), "demo.x", 0, as_host, &many, Notation::Dts).map_err(|_| ()),
        Ok(16_000)
    );
    // More than the limit: refused by their number, with none compared.
    let e = error_codes(
        &op(16_000),
        "demo.x",
        0,
        as_host,
        &Limits::default(),
        Notation::Dts,
    )
    .unwrap_err();
    assert_eq!(e.site, Decl::Code(0, 128).into());
    assert_eq!(
        e.message,
        "operation `demo.x` names 16000 error codes, and the limit is 128"
    );
    // As a file: as many `@throws` as 64 KiB hold, on the line of the
    // first too many.
    let tags: String = (0..4_000).map(|i| format!("\n@throws c{i}")).collect();
    let src = format!("declare const demo: {{\n/** D. @read{tags} */\nx(): Promise<null>;\n}};");
    assert!(src.len() <= 64 * 1024);
    refused(
        &src,
        131,
        "operation `demo.x` names 4000 error codes, and the limit is 128",
    );
}

#[test]
fn a_name_has_a_longest_length() {
    assert_eq!(Limits::default().name_bytes, 64);
    let (fits, long) = ("n".repeat(64), "n".repeat(65));
    let too_long = format!(
        "the name `{}…` is 65 bytes long, and the limit for a name is 64",
        "n".repeat(64)
    );
    // Each kind of name, where it is written.
    let cases: [(&str, usize, Named); 9] = [
        ("a surface", 1, |n| format!("declare const {n}: {{}};")),
        ("a namespace", 2, |n| file(&format!("{n}: {{}};"), "")),
        ("an operation", 3, |n| {
            file(&format!("/** D. @read */\n{n}(): Promise<null>;"), "")
        }),
        ("a parameter", 3, |n| {
            file(
                &format!("/** D. @read */\nx({n}?: null): Promise<null>;"),
                "",
            )
        }),
        ("an error code", 2, |n| {
            file(
                &format!("/** D. @read @throws {n} */\nx(): Promise<null>;"),
                "",
            )
        }),
        ("an interface", 4, |n| {
            file("", &format!("interface {n} {{}}"))
        }),
        ("a field", 4, |n| {
            file("", &format!("interface A {{ {n}: null }}"))
        }),
        ("a field of an object type", 3, |n| {
            file(
                &format!("/** D. @read */\nx(a?: {{ {n}: null }}): Promise<null>;"),
                "",
            )
        }),
        ("a kind", 6, |n| {
            file(
                "",
                &format!("interface Kinds {{\n/** K. @allow */\n{n}(request: null): {{}};\n}}"),
            )
        }),
    ];
    for (what, line, make) in cases {
        assert!(host(&make(&fits)).is_ok(), "{what}");
        refused(&make(&long), line, &too_long);
    }
    // An allow pattern's names, each.
    let allow = |pattern: &str| {
        file(
            "",
            &format!("interface Kinds {{\n/** K. @allow {pattern} */\nk(request: null): {{}};\n}}"),
        )
    };
    assert!(host(&allow(&format!("{fits}.{fits}.*"))).is_ok());
    refused(
        &allow(&format!("demo.{long}")),
        6,
        "which no operation's name can be: a name is 64 bytes at most",
    );
    refused(
        &allow("a.".repeat(40).trim_end_matches('.')),
        6,
        "an operation has 33 namespaces around it at most",
    );
    // The limit is the environment's, and a message about a name is
    // short whatever the name: 60,000 letters, and one name twice.
    let short = Limits {
        name_bytes: 3,
        ..Limits::default()
    };
    assert!(host_within("declare const abc: {};", &short).is_ok());
    assert!(host_within("declare const abcd: {};", &short).is_err());
    let e = host(&file(
        &format!("/** D. @read */\n{}(): Promise<null>;", "a".repeat(60_000)),
        "",
    ))
    .unwrap_err();
    assert!(e.message.len() < 200, "{}", e.message.len());
    let twice = "a".repeat(64);
    let e = host(&file(
        &format!(
            "/** D. @read */\n{twice}(): Promise<null>;\n/** D. @read */\n{twice}(): Promise<null>;"
        ),
        "",
    ))
    .unwrap_err();
    assert_eq!(
        (e.line().unwrap(), e.message),
        (
            5,
            format!(
                "`demo.{}…` is declared twice: a surface or namespace holds each name once",
                "a".repeat(59)
            )
        )
    );
}

#[test]
fn a_print_may_be_longer_than_its_source_and_says_whether_it_can_be_read() {
    let limits = Limits::default();
    let bound = |src: &str, limits: &Limits| src.len() * (2 * limits.depth + 8);
    // Dense files of exactly 64 KiB: 256 interfaces of two fields, each
    // a union of 24 one-digit literals; and as many blank lines of
    // documentation as fit, at the deepest indentation.
    let pad = |mut src: String| {
        assert!(src.len() + 3 <= 64 * 1024, "{}", src.len());
        src.push_str("\n//");
        while src.len() < 64 * 1024 {
            src.push('x');
        }
        src
    };
    let union: Vec<String> = (0..24u8)
        .map(|i| format!("\"{}\"", (b'a' + i) as char))
        .collect();
    let union = union.join("|");
    let wide: String = (0..256)
        .map(|i| format!("interface I{i}{{f0?:{union};f1?:{union};}}"))
        .collect();
    let wide = pad(format!("declare const d:{{}};{wide}"));
    // A doc comment holds 4 KiB: fifteen operations' worth.
    let operations: String = (0..15)
        .map(|i| format!("/**a{}b @read*/x{i}():Promise<null>;", "\n".repeat(4_094)))
        .collect();
    let deep_docs = pad(format!(
        "declare const d:{}{{{operations}}}{};",
        "{n:".repeat(32),
        "}".repeat(32)
    ));
    for (what, src, least) in [
        ("wide interfaces", wide, 80_000),
        ("blank lines of documentation", deep_docs, 4_000_000),
    ] {
        let d = host(&src).unwrap_or_else(|e| panic!("{what}: {e}"));
        let printed = print(&d);
        assert!(printed.len() > least, "{what}: {} bytes", printed.len());
        assert!(
            printed.len() <= bound(&src, &limits),
            "{what}: {} bytes",
            printed.len()
        );
        // Under the limits it was read under the print is not a file,
        // and says so; under limits that allow it, it reads back equal.
        assert_eq!(
            print_within(&d, &limits),
            Err(format!(
                "the definition prints as a file of {} bytes, and the limit is 65536",
                printed.len()
            )),
            "{what}"
        );
        assert!(host(&printed).is_err(), "{what}");
        let room = Limits {
            file_bytes: printed.len(),
            ..Limits::default()
        };
        assert_eq!(print_within(&d, &room).as_deref(), Ok(printed.as_str()));
        assert_eq!(host_within(&printed, &room).unwrap(), d, "{what}");
    }
    // A file as it is usually written prints within the limit.
    for d in [super::own(), super::shell(), super::review()] {
        assert_eq!(print_within(&d, &limits), Ok(print(&d)));
    }
}

#[test]
fn a_refusal_says_whether_the_value_is_wrong_too_large_or_too_deep() {
    let (d, ty) = types("interface T { v: Int[][] }", 10);
    let kind = |v: Value| crate::check_held(&d, &ty, &v).map_err(|e| e.kind);
    assert_eq!(kind(json!([[1, 2]])), Ok(()));
    assert_eq!(kind(json!([[1, "2"]])), Err(ValueErrorKind::Wrong));
    assert_eq!(
        kind(json!([[1, 2, 3, 4, 5, 6, 7, 8, 9]])),
        Err(ValueErrorKind::TooLarge)
    );
    let e = crate::check_held(&d, &Ty::Named("Gone".into()), &json!(1)).unwrap_err();
    assert_eq!(e.kind, ValueErrorKind::ForeignType);
    assert_eq!(e.to_string(), e.message);
    let shallow = Limits {
        value_depth: 1,
        ..Limits::default()
    };
    let d = host_within(&file("", "interface T { v: Int[][] }"), &shallow).unwrap();
    let e = crate::check_held(&d, &ty, &json!([[1]])).unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (
            ValueErrorKind::TooDeep,
            "value is nested more than 1 deep, the limit: it was not checked"
        )
    );
    // The same from the checks an operation and a kind have.
    let d = host_within(
        &file(
            "/** D. @read */\nx(a: Int[]): Promise<Int[]>;",
            "interface Kinds {\n/** K. @allow */\nk(request: Int[], message?: Int[]): { v: Int[] };\n}",
        ),
        &Limits {
            value_nodes: 3,
            ..Limits::default()
        },
    )
    .unwrap();
    let (x, k) = (d.operation("demo.x").unwrap(), d.kind("k").unwrap());
    let (fits, large, wrong) = (json!([1, 2]), json!([1, 2, 3]), json!([1, "2"]));
    let kinds = |check: &dyn Fn(&Value) -> Result<(), crate::ValueError>| {
        [&fits, &large, &wrong].map(|v| check(v).map_err(|e| e.kind))
    };
    let expected = [
        Ok(()),
        Err(ValueErrorKind::TooLarge),
        Err(ValueErrorKind::Wrong),
    ];
    assert_eq!(
        kinds(&|v| x.check_args(std::slice::from_ref(v)).map(|_| ())),
        expected
    );
    assert_eq!(kinds(&|v| x.check_answer(v)), expected);
    assert_eq!(kinds(&|v| x.check_result(v)), expected);
    assert_eq!(kinds(&|v| k.check_request(&d, v)), expected);
    assert_eq!(kinds(&|v| k.check_message(&d, v)), expected);
    // A result is an object, whose one field holds the same values:
    // the object around them takes what is left of the budget.
    assert_eq!(
        kinds(&|v| k.check_result(&d, &json!({"v": v})).map(|_| ())),
        [
            Err(ValueErrorKind::TooLarge),
            Err(ValueErrorKind::TooLarge),
            Err(ValueErrorKind::TooLarge)
        ]
    );
    assert_eq!(
        x.check_args(std::slice::from_ref(&large))
            .unwrap_err()
            .message,
        "clax.demo.x: the call's arguments are together too large to check: the limit on the work of one check is 3"
    );
    // The budget is the call's: an argument that would fit by itself
    // does not after another has spent from it, and the refusal blames
    // neither.
    let d = host_within(
        &file("/** D. @read */\nx(a: Int[], b: Int[]): Promise<null>;", ""),
        &Limits {
            value_nodes: 5,
            ..Limits::default()
        },
    )
    .unwrap();
    let x = d.operation("demo.x").unwrap();
    assert!(x.check_args(&[json!([]), json!([1, 2, 3])]).is_ok());
    assert!(x.check_args(&[json!([1, 2, 3]), json!([])]).is_ok());
    let e = x.check_args(&[json!([1]), json!([1, 2, 3])]).unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (
            ValueErrorKind::TooLarge,
            "clax.demo.x: the call's arguments are together too large to check: the limit on the work of one check is 5"
        )
    );
}

/// The `i`th name of two characters: `A0`, `A1`, … `Az`, `B0`, ….
fn short(i: usize) -> String {
    let (a, b) = ((b'A' + (i / 36) as u8) as char, (i % 36) as u32);
    format!("{a}{}", char::from_digit(b, 36).unwrap())
}

/// 128 interfaces of `per` required fields, each of a literal type of
/// its own, and one interface whose fields are `unions` unions of all
/// 128: in one order when `same`, else each starting at another. Names
/// are two characters, so the file is as dense as one can be.
fn told_apart(per: usize, unions: usize, same: bool) -> String {
    let name = short;
    let mut src = String::from("declare const d:{};");
    for i in 0..128 {
        let fields: String = (0..per)
            .map(|f| format!("{}:\"{i}\";", name(f).to_lowercase()))
            .collect();
        src += &format!("interface {}{{{fields}}}", name(i));
    }
    let fields: String = (0..unions)
        .map(|u| {
            let alts: Vec<String> = (0..128)
                .map(|i| name(if same { i } else { (i + u) % 128 }))
                .collect();
            format!("{}:{};", name(u).to_lowercase(), alts.join("|"))
        })
        .collect();
    src + &format!("interface T{{{fields}}}")
}

/// What a plan may hold for the steps it took: 128 bytes a step, and
/// the text of the file's string literals twice.
fn plan_bound(d: &Definition, src: &str) -> usize {
    128 * d.index.plan.steps + 2 * src.len()
}

#[test]
fn what_a_definition_keeps_to_check_values_is_counted_and_bounded() {
    // The files as they are usually written take a few hundred steps.
    for (d, src, steps) in [
        (super::own(), super::OWN_DTS, 234),
        (super::shell(), super::SHELL_DTS, 108),
        (super::review(), super::REVIEW_DTS, 446),
    ] {
        let plan = &d.index.plan;
        assert_eq!(plan.steps, steps, "{}", d.name());
        assert!(plan.bytes() <= plan_bound(&d, src), "{}", d.name());
        assert!(plan.bytes() < 32 * 1024, "{}", d.name());
    }

    // A file of 64 KiB: 128 interfaces of 32 fields that tell them
    // apart, and 78 unions of all 128. What tells the 128 apart is
    // worked out and kept once: 129 interfaces and their 4,174 fields,
    // 4,096 literals, 78 unions of 128 names with a step for each
    // alternative again, and once 4,096 telling fields, their 4,096
    // types and 4,096 literals.
    let src = told_apart(32, 78, true);
    assert!((65_000..=65_536).contains(&src.len()), "{}", src.len());
    let d = host(&src).unwrap_or_else(|e| panic!("{e}"));
    let plan = &d.index.plan;
    assert_eq!(
        plan.steps,
        129 + 4_174 + 4_096 + 78 * (1 + 128 + 128) + 3 * 4_096
    );
    assert!(plan.bytes() <= plan_bound(&d, &src));
    assert!(plan.bytes() < 1024 * 1024, "{}", plan.bytes());
    // As `{:?}` it is what the file says, not that form.
    let shown = format!("{d:?}");
    assert!(shown.starts_with("Definition { root: Namespace { name: \"d\""));
    assert!(shown.ends_with(".. }") && !shown.contains("Plan") && !shown.contains("nodes: ["));
    // And it tells them apart: one look at each of the 32 names.
    let ty = d.interface("T").unwrap().fields[77].ty.clone();
    let last = d.interface(&short(127)).unwrap();
    let one: serde_json::Map<String, Value> = last
        .fields
        .iter()
        .map(|f| (f.name.clone(), json!("127")))
        .collect();
    let (r, work) = measured(&d, &ty, &Value::Object(one.clone()));
    assert_eq!(r, Ok(()));
    assert_eq!(work.units, 1 + 32 + 1 + 2 * 32);
    let mut other = one;
    other.insert("a0".into(), json!("126"));
    let (r, work) = measured(&d, &ty, &Value::Object(other));
    assert!(
        r.unwrap_err()
            .message
            .starts_with("value must be A0 | A1 | ")
    );
    assert_eq!(work.units, 1 + 32);

    // The same unions with their alternatives each in another order
    // share nothing, and would take 78 times the steps: refused, with
    // the limit named, after no more than one union past it.
    let src = told_apart(32, 78, false);
    let e = host(&src).unwrap_err();
    assert_eq!(
        (e.line().unwrap(), e.message.as_str()),
        (
            1,
            "the surface's types take more than 100000 steps to prepare for checking values, the limit (`Limits::plan_steps`): what costs the most is a union of many interfaces that each have many required fields of literal type"
        )
    );
    // Seven of them are within the limit, and hold what their steps
    // allow; so does a definition read with no limit.
    let src = told_apart(32, 7, false);
    let d = host(&src).unwrap_or_else(|e| panic!("{e}"));
    let plan = &d.index.plan;
    assert_eq!(
        plan.steps,
        129 + 4_103 + 4_096 + 7 * (1 + 128 + 128 + 3 * 4_096)
    );
    assert!(plan.steps <= 100_000);
    assert!(plan.bytes() <= plan_bound(&d, &src));
    assert!(plan.bytes() < 3 * 1024 * 1024, "{}", plan.bytes());
    let src = told_apart(32, 78, false);
    let unbounded = Limits {
        plan_steps: usize::MAX,
        ..Limits::default()
    };
    let d = host_within(&src, &unbounded).unwrap();
    assert_eq!(
        d.index.plan.steps,
        129 + 4_174 + 4_096 + 78 * (1 + 128 + 128 + 3 * 4_096)
    );
    assert!(d.index.plan.bytes() <= plan_bound(&d, &src));

    // Names as long as they may be, inherited by as many interfaces as
    // there may be: the steps count each field of each, and bound it.
    let fields: String = (0..128)
        .map(|f| format!("{}{f:03}:Int;", "f".repeat(61)))
        .collect();
    let heirs: String = (0..255)
        .map(|i| format!("interface H{i} extends B{{}}"))
        .collect();
    let src = format!("declare const d:{{}};interface B{{{fields}}}{heirs}");
    let d = host(&src).unwrap_or_else(|e| panic!("{e}"));
    let plan = &d.index.plan;
    assert_eq!(plan.steps, 256 + 256 * 128 + 128);
    assert!(plan.bytes() <= plan_bound(&d, &src));
    assert!(
        (2 * 1024 * 1024..4 * 1024 * 1024).contains(&plan.bytes()),
        "{}",
        plan.bytes()
    );
}

#[test]
fn a_type_from_outside_is_prepared_in_counted_steps_charged_to_its_check() {
    // The 128 interfaces alone, and a union of them that the
    // definition does not have: preparing it takes a step for the
    // union, two for each alternative, and three for each of the 4,096
    // telling fields.
    let src = told_apart(32, 0, true);
    let steps = 1 + 2 * 128 + 3 * 4_096;
    let ty = Ty::union(Ty::Named(short(0)), (1..128).map(|i| Ty::Named(short(i))));
    let value: serde_json::Map<String, Value> = (0..32)
        .map(|f| (short(f).to_lowercase(), json!("5")))
        .collect();
    let value = Value::Object(value);
    // The check itself: the union and its 32 names, the alternative,
    // its 32 keys and their values.
    let units = 1 + 32 + 1 + 2 * 32;
    let within = |limits: Limits| {
        let d = host_within(&src, &limits).unwrap();
        assert_eq!(d.index.plan.steps, 129 + 2 * 4_096);
        crate::check_held(&d, &ty, &value).map_err(|e| (e.kind, e.message))
    };
    assert_eq!(within(Limits::default()), Ok(()));
    let budget = |value_nodes| Limits {
        value_nodes,
        ..Limits::default()
    };
    assert_eq!(within(budget(steps + units)), Ok(()));
    assert_eq!(
        within(budget(steps + units - 1)).unwrap_err().0,
        ValueErrorKind::TooLarge
    );
    assert_eq!(
        within(budget(steps)),
        Err((
            ValueErrorKind::TooLarge,
            format!("value is too large to check: the limit on the work of one check is {steps}")
        ))
    );
    // And it is under the definition's limit on such steps.
    let plan = |plan_steps| Limits {
        plan_steps,
        ..Limits::default()
    };
    assert_eq!(within(plan(steps)), Ok(()));
    assert_eq!(
        within(plan(steps - 1)),
        Err((
            ValueErrorKind::ForeignType,
            format!(
                "the value cannot be checked: its type is beyond the surface's limits: it takes more than {} steps to prepare for checking values, the limit (`Limits::plan_steps`): what costs the most is a union of many interfaces that each have many required fields of literal type",
                steps - 1
            )
        ))
    );
}

#[test]
fn alternatives_that_nothing_tells_apart_cost_more_units_than_the_value_has_bytes() {
    // Four object types with no field of literal type: each is tried,
    // and the last costs the union, three that fail on its key, and
    // itself with its key and its value.
    let (d, ty) =
        shaped("declare const d:{};interface T{v:({a:string}|{b:string}|{c:string}|{d:string})[]}");
    let v = Value::Array((0..100).map(|_| json!({"d": ""})).collect());
    let (r, work) = measured(&d, &ty, &v);
    assert_eq!(r, Ok(()));
    assert_eq!(work.units, 1 + 100 * (1 + 3 * 2 + 3));
    assert_eq!(v.to_string().len(), 1 + 100 * 9);
}
