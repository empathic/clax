//! Definitions built as a second notation's reader would build them,
//! without the `.d.ts` reader: every rule a definition keeps is checked
//! on the definition, so nothing the `.d.ts` lexer happens to refuse is
//! let through here.

use super::host;
use crate::check::{Decl, Notation, Problem, Site, build};
use crate::{
    Access, Author, Definition, Field, Interface, Kind, Limits, Member, Namespace, Op, Param,
    Pattern, Reserved, Ty, declaration, print,
};

fn none<T>() -> Vec<T> {
    Vec::new()
}

fn op(name: &str) -> Op {
    Op {
        name: name.to_string(),
        doc: "Does it.".to_string(),
        access: Access::Read,
        waits: false,
        once: false,
        throws: Vec::new(),
        params: Vec::new(),
        result: Ty::Null,
    }
}

fn param(name: &str, ty: Ty) -> Param {
    Param {
        name: name.to_string(),
        optional: false,
        ty,
    }
}

fn field(name: &str, ty: Ty) -> Field {
    Field {
        name: name.to_string(),
        optional: false,
        ty,
        doc: None,
    }
}

fn interface(name: &str, fields: Vec<Field>) -> Interface {
    Interface {
        name: name.to_string(),
        doc: None,
        extends: Vec::new(),
        fields,
    }
}

fn kind(name: &str) -> Kind {
    Kind {
        name: name.to_string(),
        doc: "Start.".to_string(),
        allow: Some(Vec::new()),
        request: Ty::Str,
        message: None,
        result: Ty::Object(Vec::new()),
    }
}

fn surface(members: Vec<Member>) -> Namespace {
    Namespace {
        name: "demo".to_string(),
        rules: None,
        members,
    }
}

fn made(
    root: Namespace,
    interfaces: Vec<Interface>,
    kinds: Vec<Kind>,
) -> Result<Definition, Problem> {
    let reserved = Reserved::of(&[]);
    build(
        root,
        interfaces,
        kinds,
        Author::Host {
            reserved: &reserved,
        },
        &Limits::default(),
        Notation::Dts,
    )
}

/// A definition with one operation, changed by `change`, is refused at
/// `site` with a message that holds `needle`.
#[track_caller]
fn op_refused(change: impl FnOnce(&mut Op), site: impl Into<Site>, needle: &str) {
    let mut o = op("x");
    change(&mut o);
    let e = made(surface(vec![Member::Op(o)]), Vec::new(), Vec::new()).unwrap_err();
    assert!(
        e.message.contains(needle),
        "`{needle}` is not in: {}",
        e.message
    );
    assert_eq!(e.site, site.into(), "{}", e.message);
}

#[track_caller]
fn refused(parts: (Namespace, Vec<Interface>, Vec<Kind>), site: impl Into<Site>, needle: &str) {
    let e = made(parts.0, parts.1, parts.2).unwrap_err();
    assert!(
        e.message.contains(needle),
        "`{needle}` is not in: {}",
        e.message
    );
    assert_eq!(e.site, site.into(), "{}", e.message);
}

#[test]
fn a_definition_built_by_hand_is_one_the_reader_would_read() {
    let mut set = op("set");
    set.access = Access::Write;
    set.once = true;
    set.throws = vec!["invalid".into()];
    set.doc = "Sets it.\n  At once.".into();
    set.params = vec![param(
        "patch",
        Ty::Object(vec![field(
            "theme",
            Ty::union(Ty::StrLit("light".into()), [Ty::StrLit("dark".into())]),
        )]),
    )];
    set.result = Ty::Named("Settings".into());
    let mut go = kind("go");
    go.allow = Some(vec![Pattern::parse("demo.*").unwrap()]);
    go.message = Some(Ty::Instruction);
    let root = surface(vec![
        Member::Op(op("get")),
        Member::Namespace(Namespace {
            name: "settings".into(),
            rules: Some("Ask before you change one.".into()),
            members: vec![Member::Op(set)],
        }),
    ]);
    let d = made(
        root,
        vec![interface("Settings", vec![field("theme", Ty::Str)])],
        vec![go],
    )
    .unwrap();
    assert_eq!(
        d.operations().map(|o| o.name).collect::<Vec<_>>(),
        ["demo.get", "demo.settings.set"]
    );
    let printed = print(&d);
    assert_eq!(host(&printed).unwrap(), d, "{printed}");
    assert!(
        declaration(&[&d], &|_| true)
            .unwrap()
            .contains("set(patch: { theme: \"light\" | \"dark\" }): Promise<demo.Settings>;")
    );
}

#[test]
fn names_are_identifiers() {
    let breaking = "x */ declare const clax: { evil(): void }; /*";
    op_refused(
        |o| o.name = "a b".into(),
        Decl::Op(0),
        "`a b` is not a name a script can call",
    );
    op_refused(
        |o| o.name = breaking.into(),
        Decl::Op(0),
        "is not a name a script can call",
    );
    op_refused(
        |o| o.name = String::new(),
        Decl::Op(0),
        "`` is not a name a script can call",
    );
    op_refused(
        |o| o.name = "then".into(),
        Decl::Op(0),
        "`then` is not a name a script can call",
    );
    op_refused(
        |o| o.name = "caf\u{e9}".into(),
        Decl::Op(0),
        "is not a name a script can call",
    );
    op_refused(
        |o| o.params = vec![param("ok", Ty::Str), param("a-b", Ty::Str)],
        Decl::Param(0, 1),
        "`a-b` is not a name for a parameter of operation `demo.x`",
    );
    op_refused(
        |o| o.params = vec![param("a): Promise<null>; evil(", Ty::Str)],
        Decl::Param(0, 0),
        "is not a name for a parameter",
    );
    // A field of an object type, by where it is written in the type.
    op_refused(
        |o| {
            o.result = Ty::Array(Box::new(Ty::Object(vec![
                field("ok", Ty::Object(vec![field("fine", Ty::Str)])),
                field("__proto__", Ty::Str),
            ])));
        },
        Site {
            decl: Decl::Result(0),
            inner: Some(2),
        },
        "operation `demo.x` has an object type with a field named `__proto__`",
    );
    op_refused(
        |o| o.params = vec![param("a", Ty::Object(vec![field("b c", Ty::Str)]))],
        Site {
            decl: Decl::Param(0, 0),
            inner: Some(0),
        },
        "has an object type with a field named `b c`",
    );
    refused(
        (
            surface(none()),
            vec![interface("A {} interface B", none())],
            none(),
        ),
        Decl::Interface(0),
        "`A {} interface B` is not a name for an interface",
    );
    refused(
        (
            surface(none()),
            vec![interface("A", none()), interface("prototype", none())],
            none(),
        ),
        Decl::Interface(1),
        "`prototype` is not a name for an interface",
    );
    refused(
        (
            surface(none()),
            vec![interface(
                "A",
                vec![field("ok", Ty::Str), field("a\nb", Ty::Str)],
            )],
            none(),
        ),
        Decl::Field(0, 1),
        "`a\\nb` is not a name for a field of interface `A`",
    );
    refused(
        (surface(none()), none(), vec![kind("go"), kind("go now")]),
        Decl::Kind(1),
        "`go now` is not a name for a kind",
    );
    refused(
        (
            Namespace {
                name: "demo".into(),
                rules: None,
                members: vec![Member::Namespace(Namespace {
                    name: "a.b".into(),
                    rules: None,
                    members: none(),
                })],
            },
            none(),
            none(),
        ),
        Decl::Namespace(0),
        "`a.b` is not a name a script can call",
    );
    let mut root = surface(none());
    root.name = "demo: {}; declare const clax".into();
    refused(
        (root, none(), none()),
        Decl::Surface,
        "is not one lowercase word",
    );
    // A long name is cut short in the message.
    let e = made(
        surface(vec![Member::Op(op(&"x ".repeat(5_000)))]),
        none(),
        none(),
    )
    .unwrap_err();
    assert!(e.message.len() < 400, "{}", e.message.len());
    // TypeScript's words, however the definition was made.
    op_refused(
        |o| o.name = "new".into(),
        Decl::Op(0),
        "an operation cannot be named `new`",
    );
    op_refused(
        |o| o.params = vec![param("ok", Ty::Str), param("this", Ty::Str)],
        Decl::Param(0, 1),
        "a parameter cannot be named `this`",
    );
    op_refused(
        |o| o.params = vec![param("class", Ty::Str)],
        Decl::Param(0, 0),
        "`class` is one of TypeScript's reserved words, which cannot name a parameter",
    );
    refused(
        (surface(none()), vec![interface("typeof", none())], none()),
        Decl::Interface(0),
        "`typeof` is one of TypeScript's reserved words, which cannot name an interface",
    );
    let mut root = surface(none());
    root.name = "delete".into();
    refused(
        (root, none(), none()),
        Decl::Surface,
        "`delete` is one of TypeScript's reserved words, which cannot name a surface",
    );
    // And a name's length.
    op_refused(
        |o| o.name = "n".repeat(65),
        Decl::Op(0),
        "is 65 bytes long, and the limit for a name is 64",
    );
    op_refused(
        |o| o.throws = vec!["c".repeat(65)],
        Decl::Code(0, 0),
        "is 65 bytes long, and the limit for a name is 64",
    );
    // A union's alternatives, each once: `Ty::union` does not say so.
    op_refused(
        |o| o.result = Ty::union(Ty::Str, [Ty::Null, Ty::Str]),
        Decl::Result(0),
        "has a union that lists `string` twice",
    );
    // What a reviewer would not see, in a literal and in a comment.
    op_refused(
        |o| o.result = Ty::StrLit("a\u{202e}b".into()),
        Decl::Result(0),
        "has a string literal that holds the character U+202E",
    );
    op_refused(
        |o| o.doc = "Reads\u{e0041} it.".into(),
        Decl::Op(0),
        "the documentation of operation `demo.x` holds the character U+E0041, which shows nothing or changes the direction of the text around it",
    );
    // And what shows nothing without being a format character: a
    // variation selector, a filler.
    op_refused(
        |o| o.doc = "Lists the files\u{fe0f}\u{e0159}.".into(),
        Decl::Op(0),
        "the documentation of operation `demo.x` holds the character U+FE0F, which shows nothing or changes the direction of the text around it",
    );
    op_refused(
        |o| o.result = Ty::StrLit("a\u{3164}b".into()),
        Decl::Result(0),
        "has a string literal that holds the character U+3164",
    );
}

#[test]
fn doc_comments_print_as_one_comment_and_read_back_the_same() {
    for (doc, needle) in [
        ("", "the documentation of operation `demo.x` is empty"),
        (
            "x */ declare const clax: { evil(): void }; /*",
            "the documentation of operation `demo.x` holds `*/`, which would end the comment it is printed in",
        ),
        ("a\0b", "holds the character U+0000"),
        ("a\rb", "holds the character U+000D"),
        ("a\u{1b}[31mb", "holds the character U+001B"),
        ("a\u{2028}b", "holds the character U+2028"),
        (" a", "starts or ends with white space"),
        ("a\n", "starts or ends with white space"),
        ("a \nb", "one of its lines ends with it"),
        (
            "Reads it. @write",
            "holds the word `@write`, which would be read as a tag",
        ),
        ("See\n@allow", "holds the word `@allow`"),
    ] {
        op_refused(|o| o.doc = doc.into(), Decl::Op(0), needle);
    }
    op_refused(
        |o| o.doc = "x".repeat(4097),
        Decl::Op(0),
        "the documentation of operation `demo.x` is 4097 bytes long, and the limit is 4096",
    );
    // Every other comment keeps the same rules.
    let mut root = surface(none());
    root.rules = Some("Rules. */".into());
    refused(
        (root, none(), none()),
        Decl::Rules,
        "the surface's rules holds `*/`",
    );
    let mut root = surface(none());
    root.rules = Some("r".repeat(4097));
    refused(
        (root, none(), none()),
        Decl::Rules,
        "the surface's rules is 4097 bytes long, and the limit is 4096",
    );
    let ns = |rules: &str| {
        surface(vec![Member::Namespace(Namespace {
            name: "ns".into(),
            rules: Some(rules.into()),
            members: none(),
        })])
    };
    refused(
        (ns(""), none(), none()),
        Decl::NamespaceRules(0),
        "the rules of `demo.ns` is empty",
    );
    refused(
        (ns("a @b"), none(), none()),
        Decl::NamespaceRules(0),
        "holds the word `@b`",
    );
    let mut a = interface("A", vec![field("a", Ty::Str)]);
    a.doc = Some("An A.\t".into());
    refused(
        (surface(none()), vec![a], none()),
        Decl::Interface(0),
        "the comment of interface `A` starts or ends with white space",
    );
    let mut a = interface("A", vec![field("a", Ty::Str)]);
    a.fields[0].doc = Some("*/".into());
    refused(
        (surface(none()), vec![a], none()),
        Decl::Field(0, 0),
        "the comment of field `a` of interface `A` holds `*/`",
    );
    let mut go = kind("go");
    go.doc = String::new();
    refused(
        (surface(none()), none(), vec![go]),
        Decl::Kind(0),
        "the documentation of kind `go` is empty",
    );
    // What is allowed: several lines, indentation inside, a tab, a backticked tag.
    let mut o = op("x");
    o.doc = "One.\n\n    indented\ta\n* a `@read` bullet".into();
    let d = made(surface(vec![Member::Op(o)]), none(), none()).unwrap();
    assert_eq!(host(&print(&d)).unwrap(), d);
}

#[test]
fn the_rest_of_an_operation_is_checked() {
    for code in ["Bad", "not-found", "", "a b", "_a", "9a", "a\nb"] {
        op_refused(
            |o| o.throws = vec!["fine".into(), code.into()],
            Decl::Code(0, 1),
            "is not an error code of operation `demo.x`: a code is lowercase letters, digits and `_`, and starts with a letter",
        );
    }
    op_refused(
        |o| o.throws = vec!["gone".into(), "late".into(), "gone".into()],
        Decl::Code(0, 2),
        "operation `demo.x` names the error code `gone` twice",
    );
    op_refused(
        |o| o.access = Access::Spend,
        Decl::Op(0),
        "operation `demo.x` is tagged @spend",
    );
    op_refused(|o| o.once = true, Decl::Op(0), "is tagged @once and @read");
    op_refused(
        |o| o.params = vec![param("a", Ty::Str), param("a", Ty::Str)],
        Decl::Param(0, 1),
        "two parameters named `a`",
    );
    op_refused(
        |o| {
            o.params = vec![param("a", Ty::Str), param("b", Ty::Str)];
            o.params[0].optional = true;
        },
        Decl::Param(0, 1),
        "is required and follows an optional one",
    );
    op_refused(
        |o| o.params = vec![param("a", Ty::union(Ty::Str, [Ty::Int, Ty::Null]))],
        Decl::Param(0, 0),
        "is required and its type admits `null`",
    );
    op_refused(
        |o| o.params = vec![param("a", Ty::Instruction)],
        Decl::Param(0, 0),
        "parameter `a` of operation `demo.x` holds `Instruction`",
    );
    op_refused(
        |o| o.result = Ty::Object(vec![field("a", Ty::Array(Box::new(Ty::Instruction)))]),
        Decl::Result(0),
        "which is not tagged @waits, holds `Instruction`",
    );
}

#[test]
fn types_are_checked() {
    op_refused(
        |o| o.result = Ty::Named("Gone".into()),
        Decl::Result(0),
        "operation `demo.x` uses the type `Gone`",
    );
    op_refused(
        |o| o.result = Ty::Named("A {} declare const clax".into()),
        Decl::Result(0),
        "uses the type `A {} declare const clax`, which the file does not declare",
    );
    for (lit, code) in [
        ("a\nb", "U+000A"),
        ("a\0", "U+0000"),
        ("\t", "U+0009"),
        ("a\u{2029}", "U+2029"),
    ] {
        op_refused(
            |o| o.result = Ty::union(Ty::StrLit("ok".into()), [Ty::StrLit(lit.into())]),
            Decl::Result(0),
            &format!("has a string literal that holds the character {code}"),
        );
    }
    // A quote and a backslash are a literal's to hold: they print escaped.
    let mut o = op("x");
    o.result = Ty::StrLit("a\"b\\ */ c".into());
    let d = made(surface(vec![Member::Op(o)]), Vec::new(), Vec::new()).unwrap();
    assert!(print(&d).contains("x(): Promise<\"a\\\"b\\\\ */ c\">;"));
    assert_eq!(host(&print(&d)).unwrap(), d);

    op_refused(
        |o| {
            o.result = Ty::Object(vec![
                field("a", Ty::Str),
                field("b", Ty::Str),
                field("a", Ty::Str),
            ])
        },
        Site {
            decl: Decl::Result(0),
            inner: Some(2),
        },
        "lists the field `a` twice",
    );
    op_refused(
        |o| {
            let mut f = field("a", Ty::Str);
            f.doc = Some("Documented.".into());
            o.result = Ty::Object(vec![f]);
        },
        Site {
            decl: Decl::Result(0),
            inner: Some(0),
        },
        "has an object type whose field `a` has a doc comment",
    );
    // A union is flat and never empty, however it is made.
    assert_eq!(Ty::union(Ty::Str, []), Ty::Str);
    let nested = Ty::union(
        Ty::union(Ty::Str, [Ty::Null]),
        [Ty::union(Ty::Int, [Ty::Bool])],
    );
    assert!(matches!(&nested, Ty::Union(alts) if **alts == [Ty::Str, Ty::Null, Ty::Int, Ty::Bool]));
}

#[test]
fn interfaces_and_kinds_are_checked() {
    let mut a = interface("A", none());
    a.extends = vec!["B".into()];
    refused(
        (surface(none()), vec![a.clone()], none()),
        Decl::Extends(0, 0),
        "interface `A` extends `B`, which the file does not declare",
    );
    let mut b = interface("B", none());
    b.extends = vec!["A".into()];
    refused(
        (surface(none()), vec![a, b], none()),
        Decl::Interface(0),
        "interface `A` extends itself, through `B`",
    );
    refused(
        (surface(none()), vec![interface("Int", none())], none()),
        Decl::Interface(0),
        "an interface may not be named `Int`",
    );
    // A host's kind says what its runs may have, after every other
    // rule.
    let mut go = kind("go");
    go.allow = None;
    refused(
        (surface(none()), none(), vec![kind("ok"), go]),
        Decl::Kind(1),
        "kind `go` has no @allow: a host's kind says which operations its runs may have, so write `@allow demo.*` for every operation of the surface, `@allow demo.<operation> …` for some, or `@allow` alone for none",
    );
    // A kind's result is an object.
    let mut go = kind("go");
    go.result = Ty::Null;
    refused(
        (surface(none()), none(), vec![go]),
        Decl::KindResult(0),
        "the result of kind `go` is `null`, and a run finishes by calling a tool whose input is an object: make the result an interface or an object type, wrapping the value in a field (`{ value: … }`)",
    );
    let mut go = kind("go");
    go.request = Ty::Named("Gone".into());
    refused(
        (surface(none()), none(), vec![go]),
        Decl::Request(0),
        "kind `go` uses the type `Gone`",
    );
    let mut go = kind("go");
    go.message = Some(Ty::Object(vec![field("a", Ty::Named("Gone".into()))]));
    refused(
        (surface(none()), none(), vec![go]),
        Site {
            decl: Decl::Message(0),
            inner: Some(0),
        },
        "kind `go` uses the type `Gone`",
    );
    let mut go = kind("go");
    go.result = Ty::Object(vec![field("said", Ty::Instruction)]);
    refused(
        (surface(none()), none(), vec![go]),
        Decl::KindResult(0),
        "the result of kind `go` holds `Instruction`",
    );
    refused(
        (surface(none()), none(), vec![kind("go"), kind("go")]),
        Decl::Kind(1),
        "kind `go` is declared twice",
    );
}

#[test]
fn limits_hold_however_a_definition_is_made() {
    // A type 33 deep, where the limit is 32.
    let deep = |levels: usize| (0..levels).fold(Ty::Str, |t, _| Ty::Array(Box::new(t)));
    let mut o = op("x");
    o.result = deep(32);
    assert!(made(surface(vec![Member::Op(o)]), none(), none()).is_ok());
    op_refused(
        |o| o.result = deep(33),
        Decl::Result(0),
        "operation `demo.x` has a type nested more than 32 deep, the limit",
    );
    op_refused(
        |o| {
            o.result = (0..33).fold(Ty::Str, |t, n| match n % 3 {
                0 => Ty::Record(Box::new(t)),
                1 => Ty::Object(vec![field("a", t)]),
                _ => Ty::union(Ty::Array(Box::new(t)), [Ty::Null]),
            });
        },
        Decl::Result(0),
        "has a type nested more than 32 deep",
    );
    // Namespaces 33 deep.
    let nest = |levels: usize| {
        (0..levels).fold(vec![Member::Op(op("x"))], |members, _| {
            vec![Member::Namespace(Namespace {
                name: "n".into(),
                rules: None,
                members,
            })]
        })
    };
    let d = made(surface(nest(32)), none(), none()).unwrap();
    assert_eq!(
        d.operations().next().unwrap().name.len(),
        "demo.".len() + 32 * 2 + 1
    );
    refused(
        (surface(nest(33)), none(), none()),
        Decl::Namespace(32),
        "is nested more than 32 deep, the limit",
    );
    // Lists.
    op_refused(
        |o| o.result = Ty::Object((0..129).map(|n| field(&format!("f{n}"), Ty::Str)).collect()),
        Decl::Result(0),
        "has an object type with 129 fields, where the limit is 128",
    );
    op_refused(
        |o| o.result = Ty::union(Ty::Str, (0..128).map(|n| Ty::StrLit(n.to_string()))),
        Decl::Result(0),
        "has a union of 129 alternatives, where the limit is 128",
    );
    op_refused(
        |o| o.params = (0..129).map(|n| param(&format!("p{n}"), Ty::Str)).collect(),
        Decl::Op(0),
        "operation `demo.x` has 129 parameters, and the limit is 128",
    );
    op_refused(
        |o| o.throws = (0..129).map(|n| format!("c{n}")).collect(),
        Decl::Code(0, 128),
        "operation `demo.x` names 129 error codes, and the limit is 128",
    );
    let ops = |n: usize| {
        (0..n)
            .map(|i| Member::Op(op(&format!("o{i}"))))
            .collect::<Vec<_>>()
    };
    refused(
        (surface(ops(129)), none(), none()),
        Decl::Members,
        "`demo` has 129 members, and the limit is 128",
    );
    let spread = |n: usize| {
        vec![
            Member::Namespace(Namespace {
                name: "a".into(),
                rules: None,
                members: ops(128),
            }),
            Member::Namespace(Namespace {
                name: "b".into(),
                rules: None,
                members: ops(n - 128),
            }),
        ]
    };
    assert!(made(surface(spread(256)), none(), none()).is_ok());
    let mut more = spread(256);
    more.push(Member::Op(op("last")));
    refused(
        (surface(more), none(), none()),
        Decl::Op(256),
        "the surface has more operations than the limit, 256",
    );
    let many = |n: usize| {
        (0..n)
            .map(|i| interface(&format!("I{i}"), none()))
            .collect::<Vec<_>>()
    };
    assert!(made(surface(none()), many(256), none()).is_ok());
    refused(
        (surface(none()), many(257), none()),
        Decl::Interface(256),
        "the file declares 257 interfaces, and the limit is 256",
    );
    refused(
        (
            surface(none()),
            none(),
            (0..65).map(|i| kind(&format!("k{i}"))).collect(),
        ),
        Decl::Kind(64),
        "the file declares 65 kinds, and the limit is 64",
    );
    // A chain of `extends` 17 long, and fields inherited past the limit.
    let chain = |n: usize| {
        (0..n)
            .map(|i| {
                let mut x = interface(&format!("I{i}"), vec![field(&format!("f{i}"), Ty::Str)]);
                if i > 0 {
                    x.extends = vec![format!("I{}", i - 1)];
                }
                x
            })
            .collect::<Vec<_>>()
    };
    assert!(made(surface(none()), chain(17), none()).is_ok());
    refused(
        (surface(none()), chain(18), none()),
        Decl::Interface(17),
        "interface `I17` is 17 steps of `extends` from the end of its chain, and the limit is 16",
    );
    let wide = |n: usize| {
        interface(
            "Wide",
            (0..n).map(|i| field(&format!("w{i}"), Ty::Str)).collect(),
        )
    };
    let mut heir = interface("Heir", vec![field("own", Ty::Str)]);
    heir.extends = vec!["Wide".into()];
    assert!(made(surface(none()), vec![wide(127), heir.clone()], none()).is_ok());
    refused(
        (surface(none()), vec![wide(128), heir], none()),
        Decl::Interface(1),
        "interface `Heir` has 129 fields with those it inherits, and the limit is 128",
    );
}
