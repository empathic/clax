//! Three rules of a definition, each held in both notations and for
//! both authors: a kind's `@allow` absent and empty are two things, and
//! a host's kind says one; no text of a definition holds a character
//! that shows nothing; a host's request has no required field whose
//! type admits `null`. Whatever the `.d.ts` reader refuses, the
//! manifest's refuses too, through the same checks, in its own words.

use super::manifest::manifest;
use super::{OWN_DTS, REVIEW_DTS, SHELL_DTS, file, host, own, refused, review, shell};
use crate::check::unshowable;
use crate::{
    Author, Definition, Error, Limits, Pattern, Place, Reserved, canonical_manifest, parse,
    parse_manifest, print, print_manifest,
};

fn clax_dts(src: &str) -> Result<Definition, Error> {
    parse(src, Author::Clax, &Limits::default())
}

fn clax_manifest(src: &str) -> Result<Definition, Error> {
    parse_manifest(src, Author::Clax, &Limits::default()).map_err(|e| e.first().clone())
}

/// `src`, a host's manifest, is refused at `pointer` with `message`,
/// which shows every character it holds.
#[track_caller]
fn manifest_refused(src: &str, pointer: &str, message: &str) {
    let e = manifest(src).expect_err(src);
    assert_eq!(
        (&e.at, e.message.as_str()),
        (&Place::Pointer(pointer.into()), message),
        "{src}"
    );
    assert!(!e.to_string().chars().any(unshowable), "{e:?}");
}

/// A host's `.d.ts` file with one kind, `go`, of this doc comment and
/// this request.
fn dts_kind(doc: &str, request: &str) -> String {
    file(
        "",
        &format!("interface Kinds {{\n/** {doc} */\ngo(request: {request}): {{}};\n}}"),
    )
}

/// A manifest with one kind, `go`, of these members beside its name,
/// description and result.
fn manifest_kind(members: &str) -> String {
    format!(
        r#"{{"clax_surface":1,"name":"demo","kinds":[{{"name":"go","description":"K."{members},"result":{{"type":"object","properties":{{}},"additionalProperties":false}}}}]}}"#
    )
}

// ------------------------------------------------------------ @allow

#[test]
fn a_hosts_kind_says_what_it_allows_and_an_empty_list_allows_nothing() {
    // `.d.ts`: no tag is refused, on the kind's line, with what to write.
    refused(
        &dts_kind("K.", "null"),
        6,
        "kind `go` has no @allow: a host's kind says which operations its runs may have, so write `@allow demo.*` for every operation of the surface, `@allow demo.<operation> …` for some, or `@allow` alone for none",
    );
    // The manifest: no member is refused, at the kind, in its words.
    manifest_refused(
        &manifest_kind(r#","request":{"type":"null"}"#),
        "/kinds/0",
        "kind `go` has no `allow`: a host's kind lists the operations its runs may have, so write `\"allow\": [\"demo.*\"]` for every operation of the surface, `[\"demo.<operation>\", …]` for some, or `[]` for none",
    );
    // `@allow` alone and `"allow": []` are one definition: a kind that
    // allows no operation, not even one of its own surface's.
    let src = file(
        "/** D. @read */\nx(): Promise<null>;",
        "interface Kinds {\n/** K. @allow */\ngo(request: null): {};\n}",
    );
    let empty = host(&src).unwrap();
    let go = empty.kind("go").unwrap();
    assert_eq!(go.allow, Some(Vec::new()));
    assert!(!go.allows("demo.x") && !go.allows("files.read"));
    let from_manifest = manifest(
        r#"{"clax_surface":1,"name":"demo","operations":[{"name":"x","description":"D.","access":"read","result":{"type":"null"}}],
            "kinds":[{"name":"go","description":"K.","allow":[],"request":{"type":"null"},"result":{"type":"object","properties":{},"additionalProperties":false}}]}"#,
    )
    .unwrap();
    assert_eq!(from_manifest, empty);
    // Each writes it in its own words, and reads it back.
    assert!(
        print(&empty).contains("/** K. @allow */"),
        "{}",
        print(&empty)
    );
    assert!(print_manifest(&empty).contains("\"allow\": [],"));
    assert_eq!(host(&print(&empty)).unwrap(), empty);
    assert_eq!(manifest(&print_manifest(&empty)).unwrap(), empty);
    assert!(canonical_manifest(&empty).contains(r#""description":"K.","allow":[],"request""#));
    // `@allow` alone beside a list is the list.
    let both = host(&dts_kind("K.\n@allow\n@allow demo.x", "null")).unwrap();
    assert_eq!(
        both.kind("go").unwrap().allow,
        Some(vec![Pattern::parse("demo.x").unwrap()])
    );
    // A list longer than the limit is still refused.
    let long = vec!["demo.x"; 129].join(" ");
    refused(
        &dts_kind(&format!("K. @allow {long}"), "null"),
        6,
        "kind `go` allows 129 patterns, and the limit is 128",
    );
}

/// Everything after a kind's first tag is tags, so prose written after
/// `@allow` would be patterns: a host's pattern that is one name no
/// operation directly on `clax` has is refused, in both notations, and
/// the `.d.ts` reader's refusals of a pattern say where prose goes. A
/// name Clax puts on `clax` is still a pattern, and a kind of Clax's
/// own may name its own operations by one name.
#[test]
fn prose_after_allow_is_not_read_as_patterns() {
    refused(
        &dts_kind("Triage.\n * @allow demo.x\n * Only for windows", "null"),
        8,
        "kind `go` allows `Only`, which names no operation: an allow pattern names the surface too (`demo.Only`), and Clax's own operations by their namespace (`threads.open`). Everything after a doc comment's first tag is tags: prose goes before them",
    );
    manifest_refused(
        &manifest_kind(r#","allow":["demo.x","Only"],"request":{"type":"null"}"#),
        "/kinds/0",
        "kind `go` allows `Only`, which names no operation: an allow pattern names the surface too (`demo.Only`), and Clax's own operations by their namespace (`threads.open`)",
    );
    refused(
        &dts_kind("Triage.\n * @allow demo.x\n * for demo.windows.", "null"),
        7,
        "`demo.windows.` is not an allow pattern: write an operation (`shell.windows`), everything below a surface or namespace (`shell.*`, `shell.settings.*`), or `*`. Everything after a doc comment's first tag is tags: prose goes before them",
    );
    // The one name `clax` names no operation either, in both notations.
    for e in [
        host(&dts_kind("K. @allow clax", "null"))
            .unwrap_err()
            .to_string(),
        manifest(&manifest_kind(
            r#","allow":["clax"],"request":{"type":"null"}"#,
        ))
        .unwrap_err()
        .to_string(),
    ] {
        assert!(e.contains("`clax` is not an allow pattern"), "{e}");
    }
    // One name of those Clax puts on `clax`, and `*`, are patterns.
    let reserved = Reserved::of(&[&own()]);
    let clax_name = reserved
        .names()
        .find(|n| !["clax", "runs", "content"].contains(n))
        .unwrap()
        .to_string();
    let d = host(&dts_kind(&format!("K. @allow * {clax_name}"), "null")).unwrap();
    assert_eq!(
        d.kind("go").unwrap().allow,
        Some(vec![
            Pattern::parse("*").unwrap(),
            Pattern::parse(&clax_name).unwrap()
        ])
    );
}

#[test]
fn clax_s_own_kind_may_say_nothing_which_narrows_nothing() {
    let dts = "declare const clax: {\n  /** D. @read */\n  x(): Promise<null>;\n};\ninterface Kinds {\n  /** Says nothing. */\n  all(request: null): {};\n  /** Allows none. @allow */\n  none(request: null): {};\n  /** Allows one. @allow x */\n  one(request: null): {};\n}\n";
    let def = clax_dts(dts).unwrap();
    let allow = |k: &str| def.kind(k).unwrap().allow.clone();
    assert_eq!(allow("all"), None);
    assert_eq!(allow("none"), Some(Vec::new()));
    assert_eq!(allow("one"), Some(vec![Pattern::parse("x").unwrap()]));
    let allows = |k: &str, op: &str| def.kind(k).unwrap().allows(op);
    assert!(allows("all", "x") && allows("all", "anything.else"));
    assert!(!allows("none", "x"));
    assert!(allows("one", "x") && !allows("one", "y"));
    // Both notations keep the three apart, both ways.
    let printed = print(&def);
    assert!(
        printed.contains("  /** Says nothing. */\n  all(")
            && printed.contains("  /** Allows none. @allow */\n"),
        "{printed}"
    );
    assert_eq!(clax_dts(&printed).unwrap(), def);
    let written = print_manifest(&def);
    let from_manifest = clax_manifest(&written).unwrap();
    assert_eq!(from_manifest, def);
    assert_eq!(clax_dts(&print(&from_manifest)).unwrap(), def);
    let canonical = canonical_manifest(&def);
    assert!(canonical.contains(r#"{"name":"all","description":"Says nothing.","request""#));
    assert!(
        canonical.contains(r#"{"name":"none","description":"Allows none.","allow":[],"request""#)
    );
    assert!(
        canonical.contains(r#"{"name":"one","description":"Allows one.","allow":["x"],"request""#)
    );
    // The fixture of Clax's own shape says nothing of its two kinds, as
    // ClaxBot's file does, and reads as it did.
    assert!(own().kinds().iter().all(|k| k.allow.is_none()));
    assert!(clax_dts(OWN_DTS).is_ok());
}

// ------------------------------------------------- what a reviewer sees

/// Characters that show nothing or change the direction of text, the
/// joiners among them.
const UNSEEN: &[char] = &[
    '\u{200c}',
    '\u{200d}',
    '\u{200b}',
    '\u{202e}',
    '\u{2066}',
    '\u{61c}',
    '\u{ad}',
    '\u{fe0f}',
    '\u{e0041}',
    '\u{3164}',
    '\u{2060}',
    '\u{feff}',
];

/// The places of a manifest a model reads text from, each holding `c`:
/// the pointer it is refused at, and what the refusal says of it.
fn manifest_places(c: char) -> Vec<(String, &'static str, String)> {
    let code = format!("U+{:04X}", c as u32);
    let holds = format!(
        "holds the character {code}, which shows nothing or changes the direction of the text around it: documentation holds text, tabs and line feeds"
    );
    let literal = format!(
        "has a string literal that holds the character {code}: a literal holds no control character, no line end, and no character that shows nothing or changes the direction of text"
    );
    let op = |members: &str| {
        format!(
            r#"{{"clax_surface":1,"name":"demo","operations":[{{"name":"x","description":"D.","access":"read"{members},"result":{{"type":"null"}}}}]}}"#
        )
    };
    let param = |schema: &str| {
        op(&format!(
            r#","params":[{{"name":"a","optional":true,"schema":{schema}}}]"#
        ))
    };
    vec![
        (
            format!(r#"{{"clax_surface":1,"name":"demo","rules":"R{c}."}}"#),
            "/rules",
            format!("the surface's rules {holds}"),
        ),
        (
            format!(
                r#"{{"clax_surface":1,"name":"demo","operations":[{{"namespace":"ns","rules":"R{c}."}}]}}"#
            ),
            "/operations/0/rules",
            format!("the rules of `demo.ns` {holds}"),
        ),
        (
            format!(
                r#"{{"clax_surface":1,"name":"demo","operations":[{{"name":"x","description":"D{c}.","access":"read","result":{{"type":"null"}}}}]}}"#
            ),
            "/operations/0",
            format!("the `description` of operation `demo.x` {holds}"),
        ),
        (
            format!(
                r#"{{"clax_surface":1,"name":"demo","interfaces":{{"A":{{"type":"object","description":"A{c}.","additionalProperties":false}}}}}}"#
            ),
            "/interfaces/A",
            format!("the `description` of interface `A` {holds}"),
        ),
        (
            format!(
                r#"{{"clax_surface":1,"name":"demo","interfaces":{{"A":{{"type":"object","properties":{{"a":{{"type":"null","description":"F{c}."}}}},"additionalProperties":false}}}}}}"#
            ),
            "/interfaces/A/properties/a",
            format!("the `description` of field `a` of interface `A` {holds}"),
        ),
        (
            manifest_kind(r#","allow":[],"request":{"type":"null"}"#)
                .replace("\"K.\"", &format!("\"K{c}.\"")),
            "/kinds/0",
            format!("the `description` of kind `go` {holds}"),
        ),
        (
            param(&format!(r#"{{"const":"ad{c}min"}}"#)),
            "/operations/0/params/0",
            format!("operation `demo.x` {literal}"),
        ),
        (
            param(&format!(r#"{{"enum":["a","b{c}"]}}"#)),
            "/operations/0/params/0",
            format!("operation `demo.x` {literal}"),
        ),
    ]
}

#[test]
fn no_text_of_a_definition_holds_a_character_that_shows_nothing_in_either_notation_for_either_author()
 {
    let reserved = Reserved::of(&[&own()]);
    let authors = [
        Author::Host {
            reserved: &reserved,
        },
        Author::Clax,
    ];
    for &c in UNSEEN {
        for author in authors {
            // A manifest, where JSON holds the character itself, and
            // where it escapes it.
            for (src, pointer, message) in manifest_places(c) {
                let escaped = src.replace(c, &format!("\\u{:04x}", c as u32));
                let escaped = if (c as u32) > 0xFFFF {
                    let n = c as u32 - 0x10000;
                    src.replace(
                        c,
                        &format!(
                            "\\u{:04x}\\u{:04x}",
                            0xD800 + (n >> 10),
                            0xDC00 + (n & 0x3FF)
                        ),
                    )
                } else {
                    escaped
                };
                for text in [&src, &escaped] {
                    let e = parse_manifest(text, author, &Limits::default())
                        .map_err(|e| e.first().clone())
                        .expect_err(text);
                    assert_eq!(
                        (&e.at, e.message.as_str()),
                        (&Place::Pointer(pointer.into()), message.as_str()),
                        "{author:?} {text}"
                    );
                    assert!(!e.to_string().chars().any(unshowable), "{e:?}");
                }
            }
            // A `.d.ts` file, which holds none anywhere.
            for (what, src) in [
                (
                    "documentation",
                    file(
                        &format!("/** Reads{c} it. @read */\nx(): Promise<null>;"),
                        "",
                    ),
                ),
                (
                    "rules",
                    format!("/** Rules{c}. */\ndeclare const demo: {{}};"),
                ),
                (
                    "a literal",
                    file(
                        &format!("/** D. @read */\nx(a?: \"a{c}\" | \"b\"): Promise<null>;"),
                        "",
                    ),
                ),
                ("a comment", format!("// A{c}\ndeclare const demo: {{}};")),
            ] {
                let e = parse(&src, author, &Limits::default()).expect_err(what);
                assert!(
                    e.message.contains(&format!("U+{:04X}", c as u32))
                        && e.message.contains("shows nothing or changes the direction"),
                    "{what}: {e}"
                );
                assert!(!e.to_string().chars().any(unshowable), "{e:?}");
            }
        }
    }
}

/// The code points of the crate documentation's table of characters
/// that show nothing: the first column of each of its rows.
fn documented() -> Vec<(u32, u32)> {
    let docs = include_str!("../lib.rs");
    let code = |s: &str| u32::from_str_radix(s.trim().trim_start_matches("U+"), 16).unwrap();
    let mut ranges = Vec::new();
    for line in docs.lines() {
        let Some(row) = line.strip_prefix("//!   | U+") else {
            continue;
        };
        let first = row.split('|').next().unwrap();
        for part in format!("U+{first}").split(',') {
            match part.split_once(" to ") {
                Some((from, to)) => ranges.push((code(from), code(to))),
                None => ranges.push((code(part), code(part))),
            }
        }
    }
    ranges
}

#[test]
fn the_documented_table_is_what_is_refused() {
    let table = documented();
    assert_eq!(table.len(), 32, "{table:x?}");
    let listed = |c: char| table.iter().any(|&(a, b)| (a..=b).contains(&(c as u32)));
    assert!(('\0'..=char::MAX).all(|c| crate::check::hidden(c) == listed(c)));
    assert!(listed('\u{200c}') && listed('\u{200d}'));
}

// ------------------------------------------------- `null` and `?`

#[test]
fn a_hosts_request_has_no_required_field_that_admits_null() {
    let dts_says = |field: &str| {
        format!(
            "field `{field}` of the request of kind `go` is required and its type admits `null`: write `{field}?:` and leave `| null` out of its type: an optional field may be left out or be `null`"
        )
    };
    let manifest_says = |field: &str| {
        format!(
            "field `{field}` of the request of kind `go` is required and its type admits `null`: leave `{field}` out of `required`, and `null` out of its type: an optional field may be left out or be `null`"
        )
    };
    let with_kind = |request: &str, rest: &str| {
        file(
            "",
            &format!(
                "{rest}interface Kinds {{\n/** K. @allow */\ngo(request:\n{request}): {{}};\n}}"
            ),
        )
    };
    // Written inline, in an interface, inherited, among a union's
    // alternatives: each refused where the field is written.
    refused(
        &with_kind("{ a: string;\nb: string | null }", ""),
        8,
        &dts_says("b"),
    );
    refused(
        &with_kind("R", "interface R {\n  ok: Int;\n  for: null | string;\n}\n"),
        6,
        &dts_says("for"),
    );
    refused(
        &with_kind(
            "R",
            "interface R extends B { ok: Int }\ninterface B {\n  x: null;\n}\n",
        ),
        6,
        &dts_says("x"),
    );
    refused(
        &with_kind("{ a: Int } |\n{ c: { d: null }; b: Int | null }", ""),
        8,
        &dts_says("b"),
    );
    // What is written to be left out, and what is inside a field, is
    // not the request's own.
    for request in [
        "{ a?: string | null }",
        "{ a?: string }",
        "{ a: unknown }",
        "{ a: { b: string | null } }",
        "{ a: (string | null)[] }",
        "string | null",
        "Record<string, Int | null>",
    ] {
        host(&with_kind(request, "")).unwrap_or_else(|e| panic!("{request}: {e}"));
    }
    // The manifest refuses the same, at the same places, in its words.
    let request = |schema: &str, interfaces: &str| {
        format!(
            r#"{{"clax_surface":1,"name":"demo"{interfaces},"kinds":[{{"name":"go","description":"K.","allow":[],"request":{schema},"result":{{"type":"object","properties":{{}},"additionalProperties":false}}}}]}}"#
        )
    };
    let nullable = r#"{"anyOf":[{"type":"string"},{"type":"null"}]}"#;
    manifest_refused(
        &request(
            &format!(
                r#"{{"type":"object","properties":{{"a":{{"type":"string"}},"b":{nullable}}},"required":["a","b"],"additionalProperties":false}}"#
            ),
            "",
        ),
        "/kinds/0/request/properties/b",
        &manifest_says("b"),
    );
    manifest_refused(
        &request(
            r##"{"$ref":"#/interfaces/R"}"##,
            r#","interfaces":{"R":{"type":"object","properties":{"ok":{"type":"integer"},"for":{"type":"null"}},"required":["ok","for"],"additionalProperties":false}}"#,
        ),
        "/interfaces/R/properties/for",
        &manifest_says("for"),
    );
    manifest_refused(
        &request(
            &format!(
                r#"{{"anyOf":[{{"type":"object","properties":{{"a":{{"type":"integer"}}}},"required":["a"],"additionalProperties":false}},{{"type":"object","properties":{{"c":{{"type":"object","properties":{{"d":{{"type":"null"}}}},"required":["d"],"additionalProperties":false}},"b":{nullable}}},"required":["c","b"],"additionalProperties":false}}]}}"#
            ),
            "",
        ),
        "/kinds/0/request/anyOf/1/properties/b",
        &manifest_says("b"),
    );
    assert!(
        manifest(&request(
            &format!(
                r#"{{"type":"object","properties":{{"b":{nullable}}},"additionalProperties":false}}"#
            ),
            ""
        ))
        .is_ok()
    );
    // Clax's own surface keeps what it has (`for: string | null`).
    for src in [
        "declare const clax: {};\ninterface Kinds {\n/** K. */\ngo(request: { a: string | null }): {};\n}",
        "declare const clax: {};\ninterface R { for: string | null }\ninterface Kinds {\n/** K. */\ngo(request: R): {};\n}",
    ] {
        let def = clax_dts(src).unwrap();
        assert_eq!(clax_manifest(&print_manifest(&def)).unwrap(), def);
    }
}

#[test]
fn a_required_parameter_that_admits_null_is_refused_for_either_author() {
    let reserved = Reserved::of(&[&own()]);
    for author in [
        Author::Host {
            reserved: &reserved,
        },
        Author::Clax,
    ] {
        let e = parse(
            &file("/** D. @read */\nx(a: string | null): Promise<null>;", ""),
            author,
            &Limits::default(),
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "line 3: parameter `a` of operation `demo.x` is required and its type admits `null`, which a call cannot pass for a required parameter (`null` means left out): make it optional, `a?:`"
        );
        let e = parse_manifest(
            r#"{"clax_surface":1,"name":"demo","operations":[{"name":"x","description":"D.","access":"read","params":[{"name":"a","schema":{"type":"null"}}],"result":{"type":"null"}}]}"#,
            author,
            &Limits::default(),
        )
        .unwrap_err();
        assert_eq!(
            e.first().to_string(),
            "/operations/0/params/0: parameter `a` of operation `demo.x` is required and its type admits `null`, which a call cannot pass for a required parameter (`null` means left out): make it optional, `\"optional\": true`"
        );
    }
}

/// The three files of the tree still read, and convert both ways, under
/// the rules.
#[test]
fn the_example_files_keep_the_rules() {
    for (what, def, text) in [
        ("shell.d.ts", shell(), SHELL_DTS),
        ("review.d.ts", review(), REVIEW_DTS),
    ] {
        assert!(
            def.kinds()
                .iter()
                .all(|k| k.allow.as_ref().is_some_and(|a| !a.is_empty())),
            "{what}"
        );
        assert_eq!(manifest(&print_manifest(&def)).unwrap(), def, "{what}");
        assert_eq!(host(&print(&def)).unwrap(), def, "{what}");
        assert_eq!(host(text).unwrap(), def, "{what}");
    }
}
