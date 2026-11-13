//! The manifest notation: it reads to the definitions the `.d.ts`
//! notation reads to, both ways and stably; what it refuses, with the
//! JSON Pointer of each refusal; input made to be too deep or too large;
//! and an MCP server's tool list as a manifest.

use super::{
    DEEP_STACK, OWN_DTS, REVIEW_DTS, SHELL_DTS, host, host_within, on_a_small_stack, on_a_stack,
    own, review, shell,
};
use crate::check::{Notation, build, unshowable};
use crate::{
    Access, Author, Definition, Error, Errors, Field, Interface, Kind, Limits, MANIFEST_VERSION,
    Member, Namespace, Op, Param, Pattern, Place, Reserved, Ty, canonical_manifest, declaration,
    parse, parse_manifest, print, print_manifest, print_manifest_within, print_within,
};
use serde_json::{Value, json};

/// Reads `src` as a host's manifest, beside the fixture named `clax`,
/// within `limits`, and answers with every mistake.
fn manifest_all(src: &str, limits: &Limits) -> Result<Definition, Errors> {
    let reserved = Reserved::of(&[&own()]);
    parse_manifest(
        src,
        Author::Host {
            reserved: &reserved,
        },
        limits,
    )
}

/// The same, with the first mistake.
fn manifest_within(src: &str, limits: &Limits) -> Result<Definition, Error> {
    manifest_all(src, limits).map_err(|e| e.first().clone())
}

pub(super) fn manifest(src: &str) -> Result<Definition, Error> {
    manifest_within(src, &Limits::default())
}

/// A host's manifest with these entries of `operations`, and these
/// members after them (each with its leading comma).
fn document(operations: &str, rest: &str) -> String {
    format!(r#"{{"clax_surface":1,"name":"demo","operations":[{operations}]{rest}}}"#)
}

/// An operation `x` with these members after its name (each with its
/// leading comma) in place of the ones it needs.
fn operation(members: &str) -> String {
    format!(r#"{{"name":"x"{members}}}"#)
}

/// A manifest whose one operation has an optional parameter of this type.
fn with_type(schema: &str) -> String {
    document(
        &operation(&format!(
            r#","description":"D.","access":"read","params":[{{"name":"a","optional":true,"schema":{schema}}}],"result":{{"type":"null"}}"#
        )),
        "",
    )
}

/// The type of that parameter.
fn type_of(schema: &str) -> Ty {
    let d = manifest(&with_type(schema)).unwrap_or_else(|e| panic!("{schema}: {e}"));
    d.operation("demo.x").unwrap().op.params[0].ty.clone()
}

/// `src` is refused at the value `pointer` names, with a message that
/// holds `needle`, and what is said shows every character it holds.
#[track_caller]
fn refused_at(src: &str, pointer: &str, needle: &str) {
    match manifest(src) {
        Ok(_) => panic!("accepted:\n{src}"),
        Err(e) => {
            assert!(
                e.message.contains(needle),
                "`{needle}` is not in: {e}\n{src}"
            );
            assert_eq!(e.at, Place::Pointer(pointer.to_string()), "{e}\n{src}");
            assert_eq!(e.pointer(), Some(pointer));
            assert_eq!(e.line(), None);
            let shown = e.to_string();
            if pointer.is_empty() {
                assert_eq!(shown, e.message);
            } else {
                assert_eq!(shown, format!("{pointer}: {}", e.message));
            }
            assert!(!shown.chars().any(unshowable), "{shown:?}");
            assert!(shown.len() < 2048, "{} bytes: {shown}", shown.len());
        }
    }
}

/// `src` is refused on `line`, as text that is not the JSON a manifest
/// is, with a message that holds `needle`.
#[track_caller]
fn refused_on(src: &str, line: usize, needle: &str) {
    match manifest(src) {
        Ok(_) => panic!("accepted:\n{src}"),
        Err(e) => {
            assert!(e.message.contains(needle), "`{needle}` is not in: {e}");
            assert_eq!(e.at, Place::Line(line), "{e}");
            assert_eq!(e.to_string(), format!("line {line}: {}", e.message));
            assert!(!e.to_string().chars().any(unshowable), "{e:?}");
            assert!(e.to_string().len() < 512, "{e}");
        }
    }
}

/// Everything the two notations promise of one definition, read as
/// `read_dts` and `read_manifest` read it.
#[track_caller]
fn round_trips(
    what: &str,
    def: &Definition,
    read_dts: &dyn Fn(&str) -> Result<Definition, Error>,
    read_manifest: &dyn Fn(&str) -> Result<Definition, Error>,
) {
    let written = print_manifest(def);
    // A definition, as a manifest, reads to an equal one.
    let from_manifest = read_manifest(&written)
        .unwrap_or_else(|e| panic!("{what}, as a manifest, is refused: {e}\n{written}"));
    assert_eq!(&from_manifest, def, "{what}, as a manifest:\n{written}");
    // Writing is stable, and does not depend on the notation read.
    assert_eq!(print_manifest(&from_manifest), written, "{what}");
    let file = print(&from_manifest);
    assert_eq!(file, print(def), "{what}");
    // What came from a manifest, as a file, reads to an equal one.
    let from_file =
        read_dts(&file).unwrap_or_else(|e| panic!("{what}, as a file, is refused: {e}\n{file}"));
    assert_eq!(from_file, from_manifest, "{what}, as a file:\n{file}");
    assert_eq!(print_manifest(&from_file), written, "{what}");
    // The model reads the same bytes, whichever notation it came from.
    let all = |_: &str| true;
    assert_eq!(
        declaration(&[&from_manifest], &all),
        declaration(&[def], &all),
        "{what}"
    );
    assert_eq!(
        declaration(&[&from_file], &all),
        declaration(&[def], &all),
        "{what}"
    );
    assert_eq!(
        print_manifest_within(def, &Limits::default()).as_deref(),
        Ok(written.as_str()),
        "{what}"
    );
}

#[test]
fn the_example_files_round_trip_through_the_manifest() {
    let as_clax = |src: &str| parse(src, Author::Clax, &Limits::default());
    let manifest_as_clax = |src: &str| {
        parse_manifest(src, Author::Clax, &Limits::default()).map_err(|e| e.first().clone())
    };
    round_trips("clax", &own(), &as_clax, &manifest_as_clax);
    round_trips("shell", &shell(), &host, &manifest);
    round_trips("review", &review(), &host, &manifest);
    // Each file itself, not only its print, reads to what its manifest
    // reads to.
    for (src, def) in [(SHELL_DTS, shell()), (REVIEW_DTS, review())] {
        assert_eq!(manifest(&print_manifest(&host(src).unwrap())).unwrap(), def);
    }
    assert_eq!(
        manifest_as_clax(&print_manifest(&as_clax(OWN_DTS).unwrap())).unwrap(),
        own()
    );
}

/// The start of `shell.d.ts` as its manifest: the surface, its first two
/// operations in order, an interface with a documented field, and the
/// kind.
#[test]
fn the_writer_writes_shell_as_documented() {
    let written = print_manifest(&shell());
    assert!(
        written.starts_with(
            r##"{
  "clax_surface": 1,
  "name": "shell",
  "rules": "You are working on a ClaxShell desktop. Each app is an artifact; the\nbar, the dock and the launcher are parts of the shell, each an artifact\ntoo. A part of the shell must call `claxos.call('shown')` once it has\ndrawn. Read `clax.shell.bridge()` before you write against the bridge: use\nonly the calls and topics it lists. After you publish a part, look at\nit with `clax.shell.screenshot` before you say it is done.",
  "operations": [
    {
      "name": "parts",
      "description": "Which artifact fills each part of the shell, and which is being worked on.",
      "access": "read",
      "params": [],
      "result": {"type": "array", "items": {"$ref": "#/interfaces/Part"}}
    },
    {
      "name": "windows",
      "description": "The windows open on the desktop.",
      "access": "read",
      "params": [],
      "result": {"type": "array", "items": {"$ref": "#/interfaces/Window"}}
    },
"##
        ),
        "{written}"
    );
    for part in [
        r#"
    {
      "name": "screenshot",
      "description": "A part or a window as it draws now.",
      "access": "read",
      "throws": ["not_found"],
      "params": [
        {"name": "of", "schema": {"anyOf": [{"type": "object", "properties": {"part": {"type": "string"}}, "required": ["part"], "additionalProperties": false}, {"type": "object", "properties": {"window": {"type": "string"}}, "required": ["window"], "additionalProperties": false}]}}
      ],
      "result": {"$ref": "clax:Image"}
    },
    {
      "name": "settings.get",
"#,
        r#"
    {
      "name": "ask",
      "description": "Asks the person in the Comment box, and waits for their answer.",
      "access": "effect",
      "waits": true,
      "once": true,
      "throws": ["declined"],
      "params": [
        {"name": "question", "schema": {"type": "string"}},
        {"name": "choices", "optional": true, "schema": {"type": "array", "items": {"type": "string"}}}
      ],
      "result": {"type": "string"}
    }
  ],
  "interfaces": {
    "Part": {
      "type": "object",
      "properties": {
        "name": {"type": "string", "description": "`bar`, `dock`, `launcher`, …"},
        "artifact": {"type": "string"},
        "title": {"type": "string"},
        "working": {"type": "boolean", "description": "A ClaxBot run is editing it now."}
      },
      "required": ["name", "artifact", "title", "working"],
      "additionalProperties": false
    },
"#,
        r#"
    "SettingsPatch": {
      "type": "object",
      "properties": {
        "theme": {"enum": ["light", "dark"]},
        "accent": {"type": "string"},
        "corners": {"type": "integer"}
      },
      "additionalProperties": false
    },
"#,
        r##"
  "kinds": [
    {
      "name": "triage",
      "description": "The request is a comment the person drew across several apps. Decide\nwhat each app it touches should be asked, and ask each with\n`clax.threads.open`. Wait for those runs with `clax.runs.wait`, then\nfinish with what each did. Change no artifact yourself.",
      "allow": ["shell.parts", "shell.windows", "shell.settings.get", "shell.screenshot", "shell.ask", "threads.open", "runs.wait"],
      "request": {"$ref": "#/interfaces/Triage"},
      "result": {"$ref": "#/interfaces/TriageResult"}
    }
  ]
}
"##,
    ] {
        assert!(written.contains(part), "{part}\nis not in\n{written}");
    }
    // `review.d.ts` has interfaces that extend others.
    assert!(print_manifest(&review()).contains(
        r#"
    "Listed": {
      "type": "object",
      "extends": ["Named"],
      "properties": {
        "next_cursor": {"anyOf": [{"type": "string"}, {"type": "null"}]},
        "truncated": {"type": "boolean"}
      },
      "required": ["next_cursor", "truncated"],
      "additionalProperties": false
    },
"#
    ));
}

/// The crate's documentation shows one definition twice, as a file and
/// as a manifest: they are one definition, and the manifest is what the
/// writer writes of it.
#[test]
fn the_documentations_two_examples_are_one_definition() {
    let docs = include_str!("../lib.rs");
    let example = |fence: &str| {
        let from = docs.find(fence).expect("the example") + fence.len();
        let until = from + docs[from..].find("//! ```").expect("its end");
        docs[from..until]
            .lines()
            .map(|l| {
                l.strip_prefix("//! ")
                    .or(l.strip_prefix("//!"))
                    .unwrap_or(l)
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    let file = host(&example("//! ```ts\n")).unwrap();
    let written = example("//! ```json\n");
    assert_eq!(manifest(&written).unwrap(), file);
    assert_eq!(print_manifest(&file), written);
    // And the start of its canonical bytes, as shown.
    let shown = example("//! ```text\n");
    assert!(shown.len() > 200, "{shown}");
    assert!(canonical_manifest(&file).starts_with(shown.trim_end()));
}

/// Every form the writer writes, each only where it says something.
#[test]
fn the_writer_writes_each_form() {
    let def = host(
        "/** Rules. */\ndeclare const demo: {\n  /**\n   * Does:\n   *     \"an\" \\example\\\n   * @write @once @waits @throws a @throws b\n   */\n  x(a: \"q\\\"\" | \"b\\\\\", b?: (A | null)[][], c?: unknown, d?: true | \"t\" | Int, e?: {}): Promise<Record<string, { n: Int; m?: number }>>;\n  empty: {};\n  /** Ns. */\n  ruled: {\n    inner: {\n      /** D. @read */\n      y(): Promise<Content>;\n    };\n  };\n  plain: {\n    /** D. @effect @waits */\n    z(): Promise<Instruction | false>;\n  };\n};\n/** An A. */\ninterface A extends B, C { /** Doc. */ a?: Int }\ninterface B {}\ninterface C { c: boolean }\ninterface Kinds {\n  /** Go. @allow demo.* * */\n  go(request: A, message?: string): B;\n  /** Stop. @allow */\n  stop(request: Image): C;\n}",
    )
    .unwrap();
    assert_eq!(
        print_manifest(&def),
        r##"{
  "clax_surface": 1,
  "name": "demo",
  "rules": "Rules.",
  "operations": [
    {
      "name": "x",
      "description": "Does:\n    \"an\" \\example\\",
      "access": "write",
      "waits": true,
      "once": true,
      "throws": ["a", "b"],
      "params": [
        {"name": "a", "schema": {"enum": ["q\"", "b\\"]}},
        {"name": "b", "optional": true, "schema": {"type": "array", "items": {"type": "array", "items": {"anyOf": [{"$ref": "#/interfaces/A"}, {"type": "null"}]}}}},
        {"name": "c", "optional": true, "schema": {}},
        {"name": "d", "optional": true, "schema": {"anyOf": [{"const": true}, {"const": "t"}, {"type": "integer"}]}},
        {"name": "e", "optional": true, "schema": {"type": "object", "properties": {}, "additionalProperties": false}}
      ],
      "result": {"type": "object", "additionalProperties": {"type": "object", "properties": {"n": {"type": "integer"}, "m": {"type": "number"}}, "required": ["n"], "additionalProperties": false}}
    },
    {
      "namespace": "empty"
    },
    {
      "namespace": "ruled",
      "rules": "Ns."
    },
    {
      "name": "ruled.inner.y",
      "description": "D.",
      "access": "read",
      "params": [],
      "result": {"$ref": "clax:Content"}
    },
    {
      "name": "plain.z",
      "description": "D.",
      "access": "effect",
      "waits": true,
      "params": [],
      "result": {"anyOf": [{"$ref": "clax:Instruction"}, {"const": false}]}
    }
  ],
  "interfaces": {
    "A": {
      "type": "object",
      "description": "An A.",
      "extends": ["B", "C"],
      "properties": {
        "a": {"type": "integer", "description": "Doc."}
      },
      "additionalProperties": false
    },
    "B": {
      "type": "object",
      "properties": {},
      "additionalProperties": false
    },
    "C": {
      "type": "object",
      "properties": {
        "c": {"type": "boolean"}
      },
      "required": ["c"],
      "additionalProperties": false
    }
  },
  "kinds": [
    {
      "name": "go",
      "description": "Go.",
      "allow": ["demo.*", "*"],
      "request": {"$ref": "#/interfaces/A"},
      "message": {"type": "string"},
      "result": {"$ref": "#/interfaces/B"}
    },
    {
      "name": "stop",
      "description": "Stop.",
      "allow": [],
      "request": {"$ref": "clax:Image"},
      "result": {"$ref": "#/interfaces/C"}
    }
  ]
}
"##
    );
    assert_eq!(
        print_manifest(&host("declare const demo: {};").unwrap()),
        "{\n  \"clax_surface\": 1,\n  \"name\": \"demo\",\n  \"operations\": []\n}\n"
    );
    assert_eq!(MANIFEST_VERSION, 1);
}

// ------------------------------------------------------- generated family

/// Numbers that follow from a seed and nothing else.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as usize
    }
    pub(super) fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len())]
    }
}

/// Prose that a doc comment may hold: several lines, blank lines,
/// indentation, bullets, a tag in backticks, what JSON escapes.
const PROSE: &[&str] = &[
    "Does it.",
    "Two lines,\nthe second.",
    "Para one.\n\nPara two.",
    "Does:\n* one\n*\n\n    clax.demo.x()",
    "Globs like a/* and // are prose.",
    "A `@read` in backticks, and `@throws gone`.",
    "A tab\there, \"quotes\", a back\\slash and {\"json\": [1]}.",
    "Not ASCII: é, —, ✓, 日本.",
];

const LITERALS: &[&str] = &[
    "a",
    "b",
    "",
    " ",
    "a\"b",
    "c\\d",
    "*/",
    "@read",
    "é ✓",
    "#/interfaces/T0",
    "clax:Int",
    "null",
];

/// Field names in an order that is not the alphabet's.
const FIELDS: &[&str] = &["zed", "kind", "a", "m_2", "$b", "_", "type", "delete"];

pub(super) struct Family {
    rng: Rng,
    interfaces: usize,
}

impl Family {
    fn prose(&mut self) -> String {
        self.rng.pick(PROSE).to_string()
    }

    fn maybe_prose(&mut self) -> Option<String> {
        self.rng.one_in(2).then(|| self.prose())
    }

    /// Fields of distinct names, some optional; `documented` for an
    /// interface's.
    fn fields(&mut self, depth: usize, documented: bool) -> Vec<Field> {
        let from = self.rng.below(FIELDS.len());
        (0..self.rng.below(4))
            .map(|k| Field {
                name: FIELDS[(from + k) % FIELDS.len()].to_string(),
                optional: self.rng.one_in(3),
                ty: self.ty(depth),
                doc: if documented { self.maybe_prose() } else { None },
            })
            .collect()
    }

    /// A type that nests at most `depth` levels, with no `Instruction`.
    fn ty(&mut self, depth: usize) -> Ty {
        let leaf = |g: &mut Self| match g.rng.below(13) {
            0 | 1 => Ty::Str,
            2 => Ty::Num,
            3 => Ty::Int,
            4 => Ty::Bool,
            5 => Ty::Null,
            6 => Ty::Unknown,
            7 => Ty::Content,
            8 => Ty::Image,
            9 => Ty::BoolLit(g.rng.one_in(2)),
            10 | 11 if g.interfaces > 0 => Ty::Named(format!("T{}", g.rng.below(g.interfaces))),
            _ => Ty::StrLit(g.rng.pick(LITERALS).to_string()),
        };
        if depth == 0 {
            return leaf(self);
        }
        match self.rng.below(10) {
            0 => Ty::Array(Box::new(self.ty(depth - 1))),
            1 => Ty::Record(Box::new(self.ty(depth - 1))),
            2 | 3 => Ty::Object(self.fields(depth - 1, false)),
            // A union of literals alone.
            4 => self.union(|g| match g.rng.below(4) {
                0 => Ty::BoolLit(g.rng.one_in(2)),
                _ => Ty::StrLit(g.rng.pick(LITERALS).to_string()),
            }),
            // Object types that a literal field tells apart.
            5 => self.union(|g| {
                let mut fields = vec![Field {
                    name: "kind".into(),
                    optional: false,
                    ty: Ty::StrLit(g.rng.pick(LITERALS).to_string()),
                    doc: None,
                }];
                fields.extend(
                    g.fields(depth - 1, false)
                        .into_iter()
                        .filter(|f| f.name != "kind"),
                );
                Ty::Object(fields)
            }),
            // Anything with anything.
            6 | 7 => self.union(|g| g.ty(depth - 1)),
            _ => leaf(self),
        }
    }

    /// A union of two to four alternatives that `one` makes, each once.
    fn union(&mut self, mut one: impl FnMut(&mut Self) -> Ty) -> Ty {
        let want = 2 + self.rng.below(3);
        let mut alternatives: Vec<Ty> = Vec::new();
        for _ in 0..12 {
            let t = one(self);
            if !matches!(t, Ty::Union(_)) && !alternatives.contains(&t) {
                alternatives.push(t);
            }
            if alternatives.len() == want {
                break;
            }
        }
        let mut alternatives = alternatives.into_iter();
        let first = alternatives.next().unwrap_or(Ty::Str);
        Ty::union(first, alternatives)
    }

    /// A type that may hold the person's words.
    fn with_instruction(&mut self) -> Ty {
        match self.rng.below(4) {
            0 => Ty::Instruction,
            1 => Ty::union(Ty::Instruction, [Ty::Null]),
            2 => Ty::Object(vec![
                Field {
                    name: "ask".into(),
                    optional: false,
                    ty: Ty::Instruction,
                    doc: None,
                },
                Field {
                    name: "more".into(),
                    optional: true,
                    ty: self.ty(2),
                    doc: None,
                },
            ]),
            _ => self.ty(3),
        }
    }

    fn op(&mut self, n: usize) -> Op {
        let access = [Access::Read, Access::Write, Access::Effect][self.rng.below(3)];
        let waits = self.rng.one_in(3);
        let codes = ["not_found", "invalid", "a1", "b_2"];
        let from = self.rng.below(codes.len());
        let mut params: Vec<Param> = (0..self.rng.below(4))
            .map(|k| {
                let ty = self.ty(3);
                Param {
                    name: format!("p{k}"),
                    // `null` leaves a parameter out, so one that admits
                    // it is optional.
                    optional: ty.admits_null() || self.rng.one_in(3),
                    ty,
                }
            })
            .collect();
        params.sort_by_key(|p| p.optional);
        Op {
            name: format!("op{n}"),
            doc: self.prose(),
            access,
            waits,
            once: access != Access::Read && self.rng.one_in(3),
            throws: (0..self.rng.below(4))
                .map(|k| codes[(from + k) % codes.len()].to_string())
                .collect(),
            params,
            result: if waits {
                self.with_instruction()
            } else {
                self.ty(3)
            },
        }
    }

    /// The members of a namespace with `depth` namespaces to go: empty
    /// namespaces and ruled ones among the operations, anywhere.
    fn members(&mut self, depth: usize) -> Vec<Member> {
        (0..self.rng.below(5))
            .map(|n| {
                if depth > 0 && self.rng.one_in(3) {
                    Member::Namespace(Namespace {
                        name: format!("ns{n}"),
                        rules: self.maybe_prose(),
                        members: self.members(depth - 1),
                    })
                } else {
                    Member::Op(self.op(n))
                }
            })
            .collect()
    }

    pub(super) fn definition(seed: u64) -> Definition {
        let mut g = Family {
            rng: Rng(seed),
            interfaces: 0,
        };
        g.interfaces = g.rng.below(5);
        let root = Namespace {
            name: "demo".into(),
            rules: g.maybe_prose(),
            members: g.members(3),
        };
        // Each interface extends some of those before it, and its fields
        // have names no other's have.
        let interfaces: Vec<Interface> = (0..g.interfaces)
            .map(|i| Interface {
                name: format!("T{i}"),
                doc: g.maybe_prose(),
                extends: (0..i)
                    .rev()
                    .filter(|_| g.rng.one_in(3))
                    .map(|j| format!("T{j}"))
                    .collect(),
                fields: g
                    .fields(3, true)
                    .into_iter()
                    .map(|f| Field {
                        name: format!("{}{i}", f.name),
                        ..f
                    })
                    .collect(),
            })
            .collect();
        // Which interfaces have a required field whose type admits
        // `null`, with those they inherit: one extends only those
        // before it.
        let mut nullable: Vec<bool> = Vec::new();
        for i in &interfaces {
            let own = i.fields.iter().any(|f| !f.optional && f.ty.admits_null());
            let inherited = i
                .extends
                .iter()
                .any(|e| nullable[e[1..].parse::<usize>().unwrap()]);
            nullable.push(own || inherited);
        }
        // A host's request has no such field of its own: one written
        // inline is made optional, and a request that would have one
        // otherwise is asked for in an array.
        let request = |t: Ty| {
            let has = |t: &Ty| match t {
                Ty::Object(fields) => fields.iter().any(|f| !f.optional && f.ty.admits_null()),
                Ty::Named(n) => nullable[n[1..].parse::<usize>().unwrap()],
                _ => false,
            };
            match t {
                Ty::Union(alts) if alts.iter().any(has) => Ty::Array(Box::new(Ty::Union(alts))),
                Ty::Object(fields) => Ty::Object(
                    fields
                        .into_iter()
                        .map(|f| Field {
                            optional: f.optional || f.ty.admits_null(),
                            ..f
                        })
                        .collect(),
                ),
                t if has(&t) => Ty::Array(Box::new(t)),
                t => t,
            }
        };
        let patterns = ["*", "demo.*", "demo.ns0.*", "files.read", "demo.op1"];
        let kinds: Vec<Kind> = (0..g.rng.below(3))
            .map(|n| Kind {
                name: format!("k{n}"),
                doc: g.prose(),
                // A host's kind always says; an empty list allows none.
                allow: Some(
                    (0..g.rng.below(4))
                        .map(|_| Pattern::parse(g.rng.pick(&patterns)).unwrap())
                        .collect(),
                ),
                request: request(g.with_instruction()),
                message: g.rng.one_in(2).then(|| g.with_instruction()),
                // A kind's result is an object: the type, as its one field.
                result: Ty::Object(vec![Field {
                    name: "value".to_string(),
                    optional: false,
                    ty: g.ty(3),
                    doc: None,
                }]),
            })
            .collect();
        let reserved = Reserved::of(&[&own()]);
        let author = Author::Host {
            reserved: &reserved,
        };
        build(
            root,
            interfaces,
            kinds,
            author,
            &Limits::default(),
            Notation::Dts,
        )
        .unwrap_or_else(|p| panic!("seed {seed} does not make a definition: {}", p.message))
    }
}

/// Definitions made from a seed, which between them hold every form:
/// interfaces that extend others, unions with and without a literal
/// that tells their alternatives apart, object types written inline,
/// arrays, records, literals, optional fields and parameters, kinds with
/// messages, prose of several lines, every tag, empty namespaces.
#[test]
fn a_family_of_generated_definitions_round_trips() {
    let mut seen = [false; 13];
    for seed in 0..400 {
        let def = Family::definition(seed);
        round_trips(&format!("seed {seed}"), &def, &host, &manifest);
        let written = print_manifest(&def);
        for (k, form) in [
            "\"extends\": [",
            "\"enum\": [",
            "\"anyOf\": [",
            "\"properties\": {\"kind\": {\"const\": ",
            "\"additionalProperties\": {",
            "\"items\": {",
            "\"optional\": true",
            "\"message\": {",
            "\"namespace\": ",
            "\"once\": true",
            "\"allow\": [\"",
            "\"allow\": []",
            "\"$ref\": \"clax:Instruction\"",
        ]
        .iter()
        .enumerate()
        {
            seen[k] |= written.contains(form);
        }
    }
    assert_eq!(seen, [true; 13]);
}

// ------------------------------------------------------------ the notation

/// What the writer never writes and the reader reads all the same: each
/// says what its other spelling says.
#[test]
fn a_manifest_written_by_hand_reads_as_its_other_spellings_do() {
    let literal = |s: &str| Ty::StrLit(s.to_string());
    for (schema, ty) in [
        ("{}", Ty::Unknown),
        (r#"{"type":"integer"}"#, Ty::Int),
        (r#"{"$ref":"clax:Int"}"#, Ty::Int),
        (r#"{"$ref":"clax:Content"}"#, Ty::Content),
        (r#"{"$ref":"clax:Image"}"#, Ty::Image),
        (r#"{"const":"a"}"#, literal("a")),
        (r#"{"type":"string","const":"a"}"#, literal("a")),
        (r#"{"enum":["a"]}"#, literal("a")),
        (r#"{"type":"boolean","const":false}"#, Ty::BoolLit(false)),
        (
            r#"{"type":"string","enum":["a","b"]}"#,
            Ty::union(literal("a"), [literal("b")]),
        ),
        (
            r#"{"enum":[true,"a"]}"#,
            Ty::union(Ty::BoolLit(true), [literal("a")]),
        ),
        // A union inside a union is the one union.
        (
            r#"{"anyOf":[{"enum":["a","b"]},{"anyOf":[{"type":"null"},{"type":"number"}]}]}"#,
            Ty::union(literal("a"), [literal("b"), Ty::Null, Ty::Num]),
        ),
        (
            r#"{"additionalProperties":false,"type":"object"}"#,
            Ty::Object(Vec::new()),
        ),
        (
            r#"{"type":"object","additionalProperties":{}}"#,
            Ty::Record(Box::new(Ty::Unknown)),
        ),
    ] {
        assert_eq!(type_of(schema), ty, "{schema}");
    }
    // The members of an object the notation names come in any order, an
    // operation's `params` may be left out, and what is false may be said.
    let d = manifest(
        r#"{"kinds":[{"result":{"type":"object","properties":{},"additionalProperties":false},"request":{"$ref":"clax:Instruction"},"description":"K.","name":"k","allow":[]}],
            "operations":[{"result":{"type":"null"},"access":"write","waits":false,"once":false,"throws":[],
                           "description":"D.","name":"x"}],
            "name":"demo","clax_surface":1}"#,
    )
    .unwrap();
    assert_eq!(
        d,
        host("declare const demo: {\n/** D. @write */\nx(): Promise<null>;\n};\ninterface Kinds {\n/** K. @allow */\nk(request: Instruction): {};\n}").unwrap()
    );
    // A manifest of the least there is.
    assert_eq!(
        manifest(r#"{"clax_surface":1,"name":"demo"}"#).unwrap(),
        host("declare const demo: {};").unwrap()
    );
}

/// The order of an object's members is the definition's order wherever
/// the definition has one: the fields of an interface and of an object
/// type, and the interfaces.
#[test]
fn property_order_is_document_order() {
    let d = manifest(&document(
        &operation(
            r#","description":"D.","access":"read","result":{"type":"object","properties":{"zed":{},"a":{},"m":{}},"additionalProperties":false,"required":["m","zed"]}"#,
        ),
        r##","interfaces":{"Zed":{"type":"object","properties":{"z":{"type":"string"},"b":{"$ref":"#/interfaces/Able"},"a":{"type":"null"}},"additionalProperties":false},
                         "Able":{"type":"object","additionalProperties":false}}"##,
    ))
    .unwrap();
    assert_eq!(
        print(&d),
        "declare const demo: {\n  /** D. @read */\n  x(): Promise<{ zed: unknown; a?: unknown; m: unknown }>;\n};\n\ninterface Zed {\n  z?: string;\n  b?: Able;\n  a?: null;\n}\ninterface Able {\n}\n"
    );
    // `serde_json::Value`, as this crate is built, would have sorted them.
    let sorted: Value = serde_json::from_str(r#"{"zed":1,"a":2}"#).unwrap();
    assert_eq!(sorted.to_string(), r#"{"a":2,"zed":1}"#);
}

/// A flat list of dotted names is the surface's tree: a namespace
/// stands where its first member does, and has an entry of its own
/// where it has rules or holds nothing.
#[test]
fn dotted_names_make_namespaces() {
    let op = |name: &str| {
        format!(
            r#"{{"name":"{name}","description":"D.","access":"read","result":{{"type":"null"}}}}"#
        )
    };
    let d = manifest(&document(
        &[
            op("a"),
            op("n.m.b"),
            op("n.m.c"),
            r#"{"namespace":"n.empty"}"#.to_string(),
            op("n.d"),
            r#"{"namespace":"r","rules":"R."}"#.to_string(),
            r#"{"namespace":"r.s","rules":"S."}"#.to_string(),
            op("r.s.e"),
            op("f"),
            r#"{"namespace":"deep.er.still"}"#.to_string(),
        ]
        .join(","),
        "",
    ))
    .unwrap();
    assert_eq!(
        print(&d),
        "declare const demo: {\n  /** D. @read */\n  a(): Promise<null>;\n  n: {\n    m: {\n      /** D. @read */\n      b(): Promise<null>;\n      /** D. @read */\n      c(): Promise<null>;\n    };\n    empty: {\n    };\n    /** D. @read */\n    d(): Promise<null>;\n  };\n  /** R. */\n  r: {\n    /** S. */\n    s: {\n      /** D. @read */\n      e(): Promise<null>;\n    };\n  };\n  /** D. @read */\n  f(): Promise<null>;\n  deep: {\n    er: {\n      still: {\n      };\n    };\n  };\n};\n"
    );
    assert_eq!(manifest(&print_manifest(&d)).unwrap(), d);
    let entries = |list: &[String]| document(&list.join(","), "");
    // The members of one namespace follow one another.
    refused_at(
        &entries(&[op("n.a"), op("b"), op("n.c")]),
        "/operations/2/name",
        "namespace `n` is listed in two places: the members of one namespace follow one another",
    );
    refused_at(
        &entries(&[op("n.m.a"), op("n.b"), op("n.m.c")]),
        "/operations/2/name",
        "namespace `n.m` is listed in two places",
    );
    // A namespace's entry comes once, before its members.
    for late in [
        vec![op("n.a"), r#"{"namespace":"n","rules":"R."}"#.to_string()],
        vec![
            r#"{"namespace":"n"}"#.to_string(),
            r#"{"namespace":"n"}"#.to_string(),
        ],
        vec![
            r#"{"namespace":"n.m"}"#.to_string(),
            r#"{"namespace":"n","rules":"R."}"#.to_string(),
        ],
    ] {
        refused_at(
            &entries(&late),
            "/operations/1",
            "namespace `n` has its entry after one of its members, or a second time",
        );
    }
    // What the definition's own checks say of names is said of these.
    refused_at(
        &entries(&[op("n"), op("n.a")]),
        "/operations/1/name",
        "`demo.n` is declared twice: a surface or namespace holds each name once",
    );
    refused_at(
        &entries(&[op("a"), op("a")]),
        "/operations/1",
        "`demo.a` is declared twice",
    );
    refused_at(
        &entries(&[op("n..a")]),
        "/operations/0/name",
        "`` is not a name a script can call",
    );
    refused_at(
        &entries(&[op("")]),
        "/operations/0",
        "`` is not a name a script can call",
    );
    refused_at(
        &entries(&[r#"{"namespace":"then"}"#.to_string()]),
        "/operations/0",
        "`then` is not a name a script can call",
    );
    // The names JavaScript calls on an object by itself, as an
    // operation's name and as a namespace's.
    for &name in crate::RESERVED_CALLABLE {
        let refusal = format!("`{name}` is not a name a script can call");
        refused_at(&entries(&[op(name)]), "/operations/0", &refusal);
        refused_at(
            &entries(&[format!(r#"{{"namespace":"{name}"}}"#)]),
            "/operations/0",
            &refusal,
        );
    }
    refused_at(
        &entries(&[r#"{"namespace":"n","rules":""}"#.to_string()]),
        "/operations/0/rules",
        "the rules of `demo.n` is empty",
    );
}

/// What is not the notation is refused where it is written.
#[test]
fn what_the_notation_cannot_say_is_refused_with_its_pointer() {
    let full = r#","description":"D.","access":"read","result":{"type":"null"}"#;
    for (src, pointer, needle) in [
        // The document.
        ("[]", "", "a manifest is an object"),
        (
            r#"{"name":"demo"}"#,
            "",
            "the manifest does not say which version of the notation it is written in: add `\"clax_surface\": 1`",
        ),
        (
            r#"{"clax_surface":2,"name":"demo","future":true}"#,
            "/clax_surface",
            "this build reads version 1 of the manifest notation (`\"clax_surface\": 1`), and no other",
        ),
        (
            r#"{"clax_surface":"1","name":"demo"}"#,
            "/clax_surface",
            "this build reads version 1",
        ),
        (
            r#"{"clax_surface":1.0,"name":"demo"}"#,
            "/clax_surface",
            "this build reads version 1",
        ),
        (r#"{"clax_surface":1}"#, "", "the manifest names no surface"),
        (
            r#"{"clax_surface":1,"name":7}"#,
            "/name",
            "the surface's `name` is a string",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","version":3}"#,
            "/version",
            "`version` is not a field of a manifest, which has `clax_surface`, `name`, `rules`, `operations`, `interfaces` and `kinds`",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","$schema":"https://json-schema.org/draft/2020-12/schema"}"#,
            "/$schema",
            "`$schema` is not a field of a manifest",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","rules":["a"]}"#,
            "/rules",
            "`rules` is a string",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","operations":{}}"#,
            "/operations",
            "`operations` is an array",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","interfaces":[]}"#,
            "/interfaces",
            "`interfaces` is an object",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","kinds":{}}"#,
            "/kinds",
            "`kinds` is an array",
        ),
        // An operation.
        (
            &document("7", ""),
            "/operations/0",
            "an entry of `operations` (an operation, or a namespace as",
        ),
        (
            &document("{}", ""),
            "/operations/0",
            "an entry of `operations` has a `name`",
        ),
        (
            &document(&operation(""), ""),
            "/operations/0",
            "operation `x` has no `description`",
        ),
        (
            &document(&operation(r#","description":"D.""#), ""),
            "/operations/0",
            "operation `x` has no `access`",
        ),
        (
            &document(&operation(r#","description":"D.","access":"read""#), ""),
            "/operations/0",
            "operation `x` has no `result`",
        ),
        (
            &document(
                &operation(r#","description":"D.","access":"Read","result":{}"#),
                "",
            ),
            "/operations/0/access",
            "`Read` is not an access: an operation's `access` is one of \"read\", \"write\", \"effect\" and \"spend\"",
        ),
        (
            &document(&operation(&format!(r#"{full},"waits":"yes""#)), ""),
            "/operations/0/waits",
            "`waits` is `true` or `false`",
        ),
        (
            &document(&operation(&format!(r#"{full},"once":1"#)), ""),
            "/operations/0/once",
            "`once` is `true` or `false`",
        ),
        (
            &document(&operation(&format!(r#"{full},"throws":"gone""#)), ""),
            "/operations/0/throws",
            "`throws` is an array",
        ),
        (
            &document(&operation(&format!(r#"{full},"throws":["a",2]"#)), ""),
            "/operations/0/throws/1",
            "an error code is a string",
        ),
        (
            &document(&operation(&format!(r#"{full},"kind":"read""#)), ""),
            "/operations/0/kind",
            "`kind` is not a field of an operation, which has `name`, `description`, `access`, `waits`, `once`, `throws`, `params` and `result`",
        ),
        (
            &document(&operation(&format!(r#"{full},"inputSchema":{{}}"#)), ""),
            "/operations/0/inputSchema",
            "`inputSchema` is not a field of an operation",
        ),
        (
            &document(&operation(&format!(r#"{full},"params":{{"a":{{}}}}"#)), ""),
            "/operations/0/params",
            "`params` is an array",
        ),
        (
            &document(
                &operation(&format!(r#"{full},"params":[{{"schema":{{}}}}]"#)),
                "",
            ),
            "/operations/0/params/0",
            "a parameter has a `name`",
        ),
        (
            &document(
                &operation(&format!(r#"{full},"params":[{{"name":"a"}}]"#)),
                "",
            ),
            "/operations/0/params/0",
            "parameter `a` has no `schema`",
        ),
        (
            &document(
                &operation(&format!(
                    r#"{full},"params":[{{"name":"a","schema":{{}},"required":true}}]"#
                )),
                "",
            ),
            "/operations/0/params/0/required",
            "`required` is not a field of a parameter, which has `name`, `optional` and `schema`",
        ),
        (
            &document(r#"{"namespace":"n","description":"D."}"#, ""),
            "/operations/0/description",
            "`description` is not a field of a namespace's entry, which has `namespace` and `rules`",
        ),
        // An interface.
        (
            &document("", r#","interfaces":{"A":{"type":"string"}}"#),
            "/interfaces/A",
            "an entry of `interfaces` is an interface",
        ),
        (
            &document("", r#","interfaces":{"A":{"enum":["a","b"]}}"#),
            "/interfaces/A",
            "any other type is written where it is used",
        ),
        (
            &document(
                "",
                r#","interfaces":{"A":{"type":"object","additionalProperties":{}}}"#,
            ),
            "/interfaces/A",
            "an entry of `interfaces` is an interface",
        ),
        (
            &document(
                "",
                r#","interfaces":{"A":{"type":"object","properties":{}}}"#,
            ),
            "/interfaces/A",
            "an object type says `\"additionalProperties\": false`",
        ),
        (
            &document(
                "",
                r#","interfaces":{"A":{"type":"object","additionalProperties":false,"extends":"B"}}"#,
            ),
            "/interfaces/A/extends",
            "`extends` is an array",
        ),
        (
            &document(
                "",
                r##","interfaces":{"A":{"type":"object","additionalProperties":false,"allOf":[{"$ref":"#/interfaces/B"}]}}"##,
            ),
            "/interfaces/A/allOf",
            "`allOf` is not a field of an interface, which has `type`, `description`, `extends`, `properties`, `required` and `additionalProperties`",
        ),
        // A kind.
        (
            &document("", r#","kinds":[{"name":"k","request":{},"result":{}}]"#),
            "/kinds/0",
            "kind `k` has no `description`",
        ),
        (
            &document(
                "",
                r#","kinds":[{"name":"k","description":"K.","request":{}}]"#,
            ),
            "/kinds/0",
            "kind `k` has a `request` and a `result`",
        ),
        (
            &document(
                "",
                r#","kinds":[{"name":"k","description":"K.","request":{},"result":{},"allow":["a b"]}]"#,
            ),
            "/kinds/0/allow/0",
            "`a b` is not an allow pattern",
        ),
        (
            &document(
                "",
                r#","kinds":[{"name":"k","description":"K.","request":{},"result":{},"access":"read"}]"#,
            ),
            "/kinds/0/access",
            "`access` is not a field of a kind",
        ),
    ] {
        refused_at(src, pointer, needle);
    }
}

/// The types are a subset of JSON Schema, and the rest of JSON Schema is
/// refused by its keyword.
#[test]
fn json_schema_outside_the_subset_is_refused_by_keyword() {
    let at = |rest: &str| format!("/operations/0/params/0/schema{rest}");
    for (schema, rest, needle) in [
        ("7", "", "a type is an object"),
        ("true", "", "a type is an object"),
        (
            r#"{"type":"integer","minimum":1}"#,
            "/minimum",
            "`minimum` is not read here: a type is written with `type`",
        ),
        (
            r#"{"type":"string","pattern":"^a"}"#,
            "/pattern",
            "`pattern` is not read here",
        ),
        (
            r#"{"type":"string","format":"date-time"}"#,
            "/format",
            "`format` is not read here",
        ),
        (
            r#"{"type":"string","maxLength":3}"#,
            "/maxLength",
            "`maxLength` is not read here",
        ),
        (
            r#"{"oneOf":[{"type":"string"},{"type":"null"}]}"#,
            "/oneOf",
            "`oneOf` is not read here",
        ),
        (
            r#"{"allOf":[{"type":"string"}]}"#,
            "/allOf",
            "`allOf` is not read here",
        ),
        (r#"{"not":{}}"#, "/not", "`not` is not read here"),
        (
            r#"{"type":"string","default":"a"}"#,
            "/default",
            "`default` is not read here",
        ),
        (
            r#"{"type":"string","title":"A"}"#,
            "/title",
            "`title` is not read here",
        ),
        (
            r#"{"type":"string","nullable":true}"#,
            "/nullable",
            "`nullable` is not read here",
        ),
        (
            r#"{"type":["string","null"]}"#,
            "/type",
            "`type` names one type: write a union as `anyOf`",
        ),
        (r#"{"type":7}"#, "/type", "`type` is a string"),
        (
            r#"{"type":"any"}"#,
            "/type",
            "`any` is not a `type` of the format, which has \"string\", \"number\", \"integer\", \"boolean\", \"null\", \"array\" and \"object\"",
        ),
        (
            r#"{"type":"string","items":{}}"#,
            "/items",
            "`items` is not read here",
        ),
        (
            r#"{"type":"array"}"#,
            "",
            "an array type gives its elements' type as `items`",
        ),
        (
            r#"{"type":"array","items":[{"type":"string"}]}"#,
            "/items",
            "a type is an object",
        ),
        (
            r#"{"type":"array","items":{},"minItems":1}"#,
            "/minItems",
            "`minItems` is not read here",
        ),
        (
            r#"{"type":"object"}"#,
            "",
            "an object type says `\"additionalProperties\": false`",
        ),
        (
            r#"{"type":"object","properties":{"a":{}}}"#,
            "",
            "one that says neither would take fields the format cannot name",
        ),
        (
            r#"{"type":"object","additionalProperties":true}"#,
            "/additionalProperties",
            "`\"additionalProperties\": true` is not a type of the format",
        ),
        (
            r#"{"type":"object","additionalProperties":"no"}"#,
            "/additionalProperties",
            "`additionalProperties` is `false` or a type",
        ),
        (
            r#"{"type":"object","properties":{"a":{}},"additionalProperties":{}}"#,
            "/properties",
            "`properties` and `required` do not go with `additionalProperties` as a type",
        ),
        (
            r#"{"type":"object","required":["a"],"additionalProperties":{}}"#,
            "/required",
            "do not go with `additionalProperties` as a type",
        ),
        (
            r#"{"type":"object","properties":[],"additionalProperties":false}"#,
            "/properties",
            "`properties` is an object",
        ),
        (
            r#"{"type":"object","properties":{"a":{}},"required":"a","additionalProperties":false}"#,
            "/required",
            "`required` is an array",
        ),
        (
            r#"{"type":"object","properties":{"a":{}},"required":["a","b"],"additionalProperties":false}"#,
            "/required/1",
            "`required` names `b`, which is not a field `properties` lists, or which it has named already",
        ),
        (
            r#"{"type":"object","properties":{"a":{}},"required":["a","a"],"additionalProperties":false}"#,
            "/required/1",
            "`required` names `a`",
        ),
        (
            r#"{"type":"object","properties":{"a":{}},"additionalProperties":false,"patternProperties":{}}"#,
            "/patternProperties",
            "`patternProperties` is not read here",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","minLength":1}},"additionalProperties":false}"#,
            "/properties/a/minLength",
            "`minLength` is not read here",
        ),
        (
            r##"{"$ref":"#/$defs/A"}"##,
            "/$ref",
            "`$ref` names an interface of the manifest (`#/interfaces/<Name>`) or a type of the core's (`clax:Content`, `clax:Image`, `clax:Instruction`, `clax:Int`), and `#/$defs/A` is neither",
        ),
        (
            r#"{"$ref":"https://example.com/schema.json"}"#,
            "/$ref",
            "is neither",
        ),
        (r#"{"$ref":"clax:String"}"#, "/$ref", "is neither"),
        (
            r##"{"$ref":"#/interfaces/A/properties/x"}"##,
            "/$ref",
            "`$ref` names a whole interface, `#/interfaces/<Name>`, and `#/interfaces/A/properties/x` does not: a reference cannot point inside an interface or at `interfaces` itself",
        ),
        (
            r##"{"$ref":"#/interfaces/"}"##,
            "/$ref",
            "`$ref` names a whole interface, `#/interfaces/<Name>`, and `#/interfaces/` does not",
        ),
        (r#"{"$ref":7}"#, "/$ref", "`$ref` is a string"),
        (
            r##"{"$ref":"#/interfaces/A","type":"object"}"##,
            "/type",
            "`type` is not read here",
        ),
        (
            r#"{"anyOf":[{"type":"string"}]}"#,
            "/anyOf",
            "`anyOf` is a union, of two types or more",
        ),
        (r#"{"anyOf":{}}"#, "/anyOf", "`anyOf` is an array"),
        (
            r#"{"anyOf":[{"type":"string"},{"type":"number","minimum":0}]}"#,
            "/anyOf/1/minimum",
            "`minimum` is not read here",
        ),
        (r#"{"enum":[]}"#, "/enum", "`enum` lists no literal"),
        (
            r#"{"enum":["a",1]}"#,
            "/enum/1",
            "`enum` holds strings, `true` and `false`, the format's literals",
        ),
        (
            r#"{"enum":["a",null]}"#,
            "/enum/1",
            "`null` is the type `{\"type\": \"null\"}`",
        ),
        (r#"{"const":3}"#, "/const", "`const` holds strings"),
        (r#"{"const":{"a":1}}"#, "/const", "`const` holds strings"),
        (
            r#"{"type":"number","enum":["a"]}"#,
            "/type",
            "`type` beside `enum` or `const` is \"string\" when every literal is a string",
        ),
        (
            r#"{"type":"string","enum":["a",true]}"#,
            "/type",
            "`type` beside `enum` or `const`",
        ),
        (
            r#"{"const":"a","enum":["a"]}"#,
            "/enum",
            "`enum` is not read here",
        ),
        (
            r#"{"type":"string","description":"A."}"#,
            "/description",
            "`description` documents an interface, or a field of one",
        ),
        (
            r#"{"type":"array","items":{"type":"string","description":"A."}}"#,
            "/items/description",
            "`description` documents an interface, or a field of one",
        ),
        (
            r#"{"type":"object","properties":{"a":{"description":7}},"additionalProperties":false}"#,
            "/properties/a/description",
            "`description` is a string",
        ),
    ] {
        refused_at(&with_type(schema), &at(rest), needle);
    }
}

/// The rules of a definition are checked on a manifest's as on a file's,
/// in the same words, at the value that breaks them.
#[test]
fn the_definitions_rules_hold_for_a_manifest() {
    let full = r#","description":"D.","access":"read","result":{"type":"null"}"#;
    let interface = |body: &str| {
        document(
            "",
            &format!(
                r#","interfaces":{{"A":{{"type":"object",{body}"additionalProperties":false}}}}"#
            ),
        )
    };
    for (src, pointer, needle) in [
        (
            r#"{"clax_surface":1,"name":"files"}"#.to_string(),
            "/name",
            "`files` is a reserved name: a host's surface may not be named `clax` or as one of Clax's own namespaces",
        ),
        (
            r#"{"clax_surface":1,"name":"calls"}"#.to_string(),
            "/name",
            "`calls` is a reserved name: a host's surface may not be named as a field of a run's step",
        ),
        (
            r#"{"clax_surface":1,"name":"published"}"#.to_string(),
            "/name",
            "`published` is a reserved name: a host's surface may not be named as a field of a run's step",
        ),
        (
            r#"{"clax_surface":1,"name":"Demo"}"#.to_string(),
            "/name",
            "the surface's name `Demo` is not one lowercase word",
        ),
        (
            r#"{"clax_surface":1,"name":"demo","rules":"R. */"}"#.to_string(),
            "/rules",
            "the surface's rules holds `*/`",
        ),
        (
            document(
                &operation(r#","description":"D.","access":"spend","result":{}"#),
                "",
            ),
            "/operations/0",
            "operation `demo.x` has the access \"spend\", which only Clax's own surface may use",
        ),
        (
            document(
                &operation(r#","description":"D.","access":"read","once":true,"result":{}"#),
                "",
            ),
            "/operations/0",
            "operation `demo.x` says `once` and has the access \"read\": repeating a read is harmless",
        ),
        (
            document(
                &operation(r#","description":"D. @read","access":"read","result":{}"#),
                "",
            ),
            "/operations/0",
            "holds the word `@read`, which the definition's `.d.ts` file would read as a tag: write it in backticks",
        ),
        (
            document(
                &operation(r#","description":"","access":"read","result":{}"#),
                "",
            ),
            "/operations/0",
            "the `description` of operation `demo.x` is empty",
        ),
        (
            document(
                &operation(&format!(r#"{full},"throws":["gone","Not Found"]"#)),
                "",
            ),
            "/operations/0/throws/1",
            "`Not Found` is not an error code of operation `demo.x`",
        ),
        (
            document(&operation(&format!(r#"{full},"throws":["a","b","a"]"#)), ""),
            "/operations/0/throws/2",
            "operation `demo.x` names the error code `a` twice",
        ),
        (
            document(
                &operation(&format!(r#"{full},"throws":["busy","effect_limit"]"#)),
                "",
            ),
            "/operations/0/throws/1",
            "operation `demo.x` names `effect_limit` in its `throws`, a code only Clax gives a script",
        ),
        (
            document(
                &operation(&format!(r#"{full},"throws":["invalid_args"]"#)),
                "",
            ),
            "/operations/0/throws/0",
            "operation `demo.x` names `invalid_args` in its `throws`, a code only Clax gives a script",
        ),
        (
            document(
                &operation(&format!(
                    r#"{full},"params":[{{"name":"a","optional":true,"schema":{{}}}},{{"name":"b","schema":{{}}}}]"#
                )),
                "",
            ),
            "/operations/0/params/1",
            "parameter `b` of operation `demo.x` is required and follows an optional one",
        ),
        (
            document(
                &operation(&format!(
                    r#"{full},"params":[{{"name":"a","schema":{{"anyOf":[{{"type":"string"}},{{"type":"null"}}]}}}}]"#
                )),
                "",
            ),
            "/operations/0/params/0",
            "is required and its type admits `null`, which a call cannot pass for a required parameter (`null` means left out): make it optional, `\"optional\": true`",
        ),
        (
            document(
                &operation(&format!(
                    r#"{full},"params":[{{"name":"class","schema":{{}}}}]"#
                )),
                "",
            ),
            "/operations/0/params/0",
            "`class` is one of TypeScript's reserved words",
        ),
        (
            document(
                &operation(&format!(
                    r#"{full},"params":[{{"name":"a","schema":{{"$ref":"clax:Instruction"}}}}]"#
                )),
                "",
            ),
            "/operations/0/params/0",
            "parameter `a` of operation `demo.x` holds `Instruction`",
        ),
        (
            document(
                &operation(
                    r#","description":"D.","access":"read","result":{"type":"array","items":{"$ref":"clax:Instruction"}}"#,
                ),
                "",
            ),
            "/operations/0/result",
            "the result of operation `demo.x`, which does not say `waits`, holds `Instruction`: `Instruction` (the person's own words) may appear only in a kind's request, in a kind's message and in the result of an operation that says `waits`",
        ),
        (
            document(
                &operation(
                    r##","description":"D.","access":"read","result":{"$ref":"#/interfaces/Missing"}"##,
                ),
                "",
            ),
            "/operations/0/result",
            "operation `demo.x` uses the type `Missing`, which `interfaces` does not list: add an interface of that name there, or use one of the format's types",
        ),
        // Inside a type, the field of the object type that is wrong.
        (
            with_type(
                r#"{"anyOf":[{"type":"null"},{"type":"object","properties":{"ok":{},"deep":{"type":"array","items":{"type":"object","properties":{"a":{},"b c":{}},"additionalProperties":false}}},"additionalProperties":false}]}"#,
            ),
            "/operations/0/params/0/schema/anyOf/1/properties/deep/items/properties/b c",
            "operation `demo.x` has an object type with a field named `b c`",
        ),
        (
            with_type(
                r#"{"type":"object","properties":{"a":{"type":"string","description":"A."}},"additionalProperties":false}"#,
            ),
            "/operations/0/params/0/schema/properties/a",
            "has an object type whose field `a` has a `description`: only an interface's fields take one",
        ),
        (
            with_type(r#"{"anyOf":[{"type":"string"},{"type":"string"}]}"#),
            "/operations/0/params/0",
            "has a union that lists `string` twice",
        ),
        (
            with_type(r#"{"enum":["a","b","a"]}"#),
            "/operations/0/params/0",
            "has a union that lists the literal \"a\" twice",
        ),
        (
            interface(r#""properties":{"a":{"type":"string","description":"A.\t"}},"#),
            "/interfaces/A/properties/a",
            "the `description` of field `a` of interface `A` starts or ends with white space",
        ),
        (
            interface(r##""properties":{"a":{"$ref":"#/interfaces/B"}},"##),
            "/interfaces/A/properties/a",
            "field `a` of interface `A` uses the type `B`, which `interfaces` does not list",
        ),
        (
            interface(r#""extends":["B"],"#),
            "/interfaces/A/extends/0",
            "interface `A` extends `B`, which `interfaces` does not list: an interface extends interfaces of the same manifest",
        ),
        (
            interface(r#""extends":["A"],"#),
            "/interfaces/A",
            "interface `A` extends itself",
        ),
        (
            interface(r#""description":"","#),
            "/interfaces/A",
            "the `description` of interface `A` is empty",
        ),
        (
            document(
                "",
                r#","interfaces":{"string":{"type":"object","additionalProperties":false}}"#,
            ),
            "/interfaces/string",
            "an interface may not be named `string`",
        ),
        (
            document(
                "",
                r#","interfaces":{"Kinds":{"type":"object","additionalProperties":false}}"#,
            ),
            "/interfaces/Kinds",
            "an interface may not be named `Kinds`",
        ),
        (
            document(
                "",
                r#","interfaces":{"A":{"type":"object","properties":{"a":{}},"additionalProperties":false},
                                  "B":{"type":"object","extends":["A"],"properties":{"a":{}},"additionalProperties":false}}"#,
            ),
            "/interfaces/B/properties/a",
            "interface `B` declares the field `a`, which it already has from `A`",
        ),
        (
            document(
                "",
                r#","kinds":[{"name":"k","description":"K.","request":{},"result":{"type":"object","properties":{},"additionalProperties":false}},{"name":"k","description":"K.","request":{},"result":{"type":"object","properties":{},"additionalProperties":false}}]"#,
            ),
            "/kinds/1",
            "kind `k` is declared twice",
        ),
        (
            document(
                "",
                r#","kinds":[{"name":"k","description":"K.","allow":["demo.*"],"request":{},"result":{"type":"object","properties":{},"additionalProperties":false}},{"name":"l","description":"L.","request":{},"result":{"type":"object","properties":{},"additionalProperties":false}}]"#,
            ),
            "/kinds/1",
            "kind `l` has no `allow`: a host's kind lists the operations its runs may have, so write `\"allow\": [\"demo.*\"]` for every operation of the surface, `[\"demo.<operation>\", …]` for some, or `[]` for none",
        ),
        (
            document(
                "",
                r#","kinds":[{"name":"k","description":"K.","request":{},"result":{"type":"object","properties":{"a":{"$ref":"clax:Instruction"}},"required":["a"],"additionalProperties":false}}]"#,
            ),
            "/kinds/0/result",
            "the result of kind `k` holds `Instruction`",
        ),
        (
            document(
                "",
                r##","kinds":[{"name":"k","description":"K.","request":{},"message":{"$ref":"#/interfaces/M"},"result":{"type":"object","properties":{},"additionalProperties":false}}]"##,
            ),
            "/kinds/0/message",
            "kind `k` uses the type `M`, which `interfaces` does not list",
        ),
    ] {
        refused_at(&src, pointer, needle);
    }
    // As Clax's own, a manifest may be named `clax` and may charge.
    let own_manifest = r#"{"clax_surface":1,"name":"clax","operations":[{"name":"web.search","description":"D.","access":"spend","result":{}}]}"#;
    let d = parse_manifest(own_manifest, Author::Clax, &Limits::default()).unwrap();
    assert_eq!(d.operation("web.search").unwrap().op.access, Access::Spend);
    refused_at(own_manifest, "/name", "`clax` is a reserved name");
}

/// Text that is not JSON, or not the JSON a manifest is, is refused
/// with its line.
#[test]
fn text_that_is_not_json_is_refused_with_its_line() {
    for (src, line, needle) in [
        ("", 1, "the manifest is not JSON: EOF while parsing a value"),
        (
            "{\n  \"clax_surface\": 1,\n  \"name\": \"demo\",\n}",
            4,
            "the manifest is not JSON: trailing comma (column 1)",
        ),
        (
            "{\"clax_surface\": 1, \"name\": \"demo\"} x",
            1,
            "the manifest is not JSON: trailing characters",
        ),
        (
            "{\"clax_surface\": 1, 'name': \"demo\"}",
            1,
            "the manifest is not JSON: key must be a string",
        ),
        (
            "// a comment\n{}",
            1,
            "the manifest is not JSON: expected value",
        ),
        (
            "\u{feff}\u{feff}{\"clax_surface\": 1, \"name\": \"demo\"}",
            1,
            "the manifest is not JSON: expected value",
        ),
        (
            "{\"clax_surface\": 1,\n \"name\": \"de\u{1}mo\"}",
            2,
            "the manifest is not JSON: control character",
        ),
        (
            "{\"clax_surface\": 1, \"name\": \"\\ud800\"}",
            1,
            "the manifest is not JSON: ",
        ),
        (
            "{\"clax_surface\": 1e999, \"name\": \"demo\"}",
            1,
            "the manifest is not JSON: number out of range",
        ),
        // An object gives each key once: which of two values a reader
        // took would be its own choice.
        (
            "{\"clax_surface\": 1,\n \"name\": \"demo\",\n \"name\": \"other\"}",
            3,
            "the key `name` is given twice in one object (column ",
        ),
        (
            "{\"clax_surface\": 1, \"name\": \"demo\", \"interfaces\": {\"A\": {\"type\": \"object\", \"additionalProperties\": false,\n\"properties\": {\"a\": {}, \"a\": {\"type\": \"string\"}}}}}",
            2,
            "the key `a` is given twice in one object",
        ),
        (
            "{\"clax_surface\": 1, \"\u{202e}\": 1, \"\u{202e}\": 2}",
            1,
            "the key `\\u{202e}` is given twice",
        ),
    ] {
        refused_on(src, line, needle);
    }
}

// ----------------------------------------------------------------- hostile

/// JSON nested `depth` arrays deep.
fn arrays(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

/// The type `leaf` inside `levels` unions, each of a literal and the
/// next: two arrays or objects a level, and no deeper as a type.
fn unions(levels: usize, leaf: &str) -> String {
    let open: String = (0..levels)
        .map(|i| format!(r#"{{"anyOf":[{{"const":"l{i}"}},"#))
        .collect();
    format!("{open}{leaf}{}", "]}".repeat(levels))
}

/// A `.d.ts` file whose one operation has a parameter of the type
/// `{ a: T } | string` nested `levels` deep around `leaf`: what costs a
/// manifest the most arrays and objects for each level a type nests,
/// four.
fn objects_in_unions(levels: usize, leaf: &str) -> String {
    format!(
        "declare const demo: {{\n/** D. @read */\nx(a: {}{leaf}{}): Promise<null>;\n}};",
        "{ a: ".repeat(levels),
        " } | string".repeat(levels)
    )
}

#[test]
fn json_nested_without_bound_is_refused_and_not_followed() {
    let limit = Limits::default().manifest_nesting();
    assert_eq!(limit, 4 * 32 + 8);
    let most = Limits {
        depth: usize::MAX,
        ..Limits::default()
    };
    assert_eq!(most.manifest_nesting(), 4 * 64 + 8);
    // Far past the limit, and past where `serde_json` would stop by
    // itself (127): 120,000 levels fit a manifest of 256 KiB.
    for depth in [limit + 1, 200, 1_000, 120_000] {
        let src = arrays(depth);
        let e = on_a_small_stack(move || manifest(&src)).unwrap_err();
        assert_eq!(e.at, Place::Line(1), "{depth}");
        assert!(
            e.message.starts_with(
                "arrays and objects nested more than 136 deep, the limit for a manifest (column "
            ),
            "{depth}: {e}"
        );
    }
    // The bound follows the limit on how deep types nest.
    let shallow = Limits {
        depth: 2,
        ..Limits::default()
    };
    assert!(
        manifest_within(&arrays(17), &shallow)
            .unwrap_err()
            .message
            .starts_with("arrays and objects nested more than 16 deep")
    );
    // A type sits six deep, in a parameter: 65 unions reach the limit,
    // and are read, as the one union they are.
    let string = r#"{"type":"string"}"#;
    let at_limit = with_type(&unions(65, string));
    assert_eq!(crate::manifest::nesting(&at_limit), limit);
    let d = on_a_small_stack(move || manifest(&at_limit)).unwrap();
    let Ty::Union(alternatives) = &d.operation("demo.x").unwrap().op.params[0].ty else {
        panic!("a union");
    };
    assert_eq!(alternatives.len(), 66);
    let over = with_type(&unions(
        65,
        &format!(r#"{{"type":"array","items":{string}}}"#),
    ));
    assert_eq!(crate::manifest::nesting(&over), limit + 1);
    let e = on_a_small_stack(move || manifest(&over)).unwrap_err();
    assert!(
        e.message
            .starts_with("arrays and objects nested more than 136 deep")
    );
    // The bound is on the text as it is written: a spelling the writer
    // does not write may nest deeper than the writer's, and a type at
    // the deepest level that is spelled so is refused where the
    // writer's spelling of it reads.
    let deepest = |innermost: &str| {
        let open = r#"{"anyOf":[{"type":"object","properties":{"a":"#;
        let close = r#"},"required":["a"],"additionalProperties":false},{"type":"string"}]}"#;
        with_type(&format!(
            "{}{innermost}{}",
            open.repeat(32),
            close.repeat(32)
        ))
    };
    let written = deepest(r#"{"anyOf":[{"type":"number"},{"const":"a"},{"const":"b"}]}"#);
    assert_eq!(crate::manifest::nesting(&written), limit);
    let read = on_a_small_stack(move || manifest(&written)).unwrap();
    for respelled in [
        r#"{"anyOf":[{"type":"number"},{"anyOf":[{"const":"a"},{"const":"b"}]}]}"#,
        r#"{"anyOf":[{"type":"number"},{"enum":["a","b"]}]}"#,
    ] {
        let src = deepest(respelled);
        assert!(crate::manifest::nesting(&src) > limit);
        let e = on_a_small_stack(move || manifest(&src)).unwrap_err();
        assert!(
            e.message
                .starts_with("arrays and objects nested more than 136 deep"),
            "{e}"
        );
        // One level up, each reads as the writer's spelling does.
        let open = r#"{"anyOf":[{"type":"object","properties":{"a":"#;
        let close = r#"},"required":["a"],"additionalProperties":false},{"type":"string"}]}"#;
        let shallower = |innermost: &str| {
            manifest(&with_type(&format!(
                "{}{innermost}{}",
                open.repeat(31),
                close.repeat(31)
            )))
            .unwrap()
        };
        assert_eq!(
            shallower(respelled),
            shallower(r#"{"anyOf":[{"type":"number"},{"const":"a"},{"const":"b"}]}"#)
        );
    }
    assert_eq!(manifest(&print_manifest(&read)).unwrap(), read);
    // Types nest no deeper in a manifest than in a file, and say so in
    // the same words.
    for (what, open, close) in [
        ("arrays", r#"{"type":"array","items":"#, "}"),
        (
            "records",
            r#"{"type":"object","additionalProperties":"#,
            "}",
        ),
        (
            "object types",
            r#"{"type":"object","properties":{"a":"#,
            r#"},"additionalProperties":false}"#,
        ),
    ] {
        let nested = |levels: usize| {
            with_type(&format!(
                "{}{string}{}",
                open.repeat(levels),
                close.repeat(levels)
            ))
        };
        let src = nested(32);
        let d = on_a_small_stack(move || manifest(&src)).unwrap_or_else(|e| panic!("{what}: {e}"));
        round_trips(what, &d, &host, &manifest);
        let src = nested(33);
        let e = on_a_small_stack(move || manifest(&src)).unwrap_err();
        assert_eq!(
            e.message, "nested more than 32 deep, the limit for namespaces and for types",
            "{what}"
        );
        assert!(
            e.pointer()
                .is_some_and(|p| p.starts_with("/operations/0/params/0/schema/")),
            "{what}: {e}"
        );
    }
    // Namespaces too.
    let op = |levels: usize| {
        document(
            &format!(
                r#"{{"name":"{}x","description":"D.","access":"read","result":{{"type":"null"}}}}"#,
                "a.".repeat(levels)
            ),
            "",
        )
    };
    let d = manifest(&op(32)).unwrap();
    round_trips("namespaces", &d, &host, &manifest);
    refused_at(
        &op(33),
        "/operations/0/name",
        "nested more than 32 deep, the limit for namespaces and for types",
    );
    refused_at(
        &document(
            &format!(r#"{{"namespace":"{}a"}}"#, "a.".repeat(20_000)),
            "",
        ),
        "/operations/0/namespace",
        "nested more than 32 deep",
    );
}

/// Every definition a `.d.ts` file gives has a manifest, however deep
/// its types nest: the smallest that `serde_json`'s own bound of 127
/// would refuse, and the deepest the default limits allow, each read on
/// the stack the crate's documentation names.
#[test]
fn every_definition_has_a_manifest_however_deep_its_types_nest() {
    // `{ a: T } | string` nested 30 times: a manifest 126 deep.
    let d = host(&objects_in_unions(30, "string")).unwrap();
    let written = print_manifest_within(&d, &Limits::default()).unwrap();
    assert_eq!(crate::manifest::nesting(&written), 6 + 4 * 30);
    let read = on_a_small_stack(move || manifest(&written)).unwrap();
    assert_eq!(read, d);
    round_trips("30 unions of object types", &d, &host, &manifest);
    // The deepest: 32 times, around a union.
    let d = host(&objects_in_unions(32, "number | null")).unwrap();
    let written = print_manifest_within(&d, &Limits::default()).unwrap();
    assert_eq!(
        crate::manifest::nesting(&written),
        Limits::default().manifest_nesting()
    );
    let (again, canonical) = (d.clone(), canonical_manifest(&d));
    on_a_small_stack(move || {
        assert_eq!(manifest(&written).unwrap(), again);
        assert_eq!(manifest(&canonical).unwrap(), again);
        assert_eq!(print_manifest(&again), written);
        assert_eq!(host(&print(&again)).unwrap(), again);
    });
    round_trips(
        "the deepest type of the default limits",
        &d,
        &host,
        &manifest,
    );
    // One level more is no definition, in either notation.
    let limits = Limits {
        depth: usize::MAX,
        ..Limits::default()
    };
    let deeper = host_within(&objects_in_unions(33, "string"), &limits).unwrap();
    let written = print_manifest(&deeper);
    assert_eq!(
        host(&objects_in_unions(33, "string")).unwrap_err().message,
        "nested more than 32 deep, the limit for namespaces and for types"
    );
    assert!(
        manifest(&written)
            .unwrap_err()
            .message
            .starts_with("arrays and objects nested more than 136 deep")
    );
    // A definition read under a greater depth has no manifest within a
    // lesser one, and the writer's check says so.
    assert_eq!(
        print_manifest_within(&deeper, &Limits::default()),
        Err("the definition's manifest nests arrays and objects 138 deep, and the limit is 136 for a manifest whose types nest 32 deep".to_string())
    );
}

/// The writers' checks say what a reader under the limits says: a
/// definition read under the default limits has no manifest and no
/// print within limits that take fewer operations, or fewer values,
/// than it has, though its text is short and shallow enough.
#[test]
fn a_print_within_limits_is_one_a_reader_under_them_takes() {
    let d = super::shell();
    let fewer_ops = Limits {
        operations: 1,
        ..Limits::default()
    };
    let fewer_values = Limits {
        manifest_values: 16,
        ..Limits::default()
    };
    for limits in [&fewer_ops, &fewer_values] {
        let printed = print_manifest(&d);
        assert!(printed.len() <= limits.manifest_bytes);
        let refused = manifest_within(&printed, limits).unwrap_err().to_string();
        assert_eq!(
            print_manifest_within(&d, limits),
            Err(format!(
                "a reader under these limits refuses the definition's manifest: {refused}"
            ))
        );
    }
    let refused = host_within(&print(&d), &fewer_ops).unwrap_err().to_string();
    assert_eq!(
        print_within(&d, &fewer_ops),
        Err(format!(
            "a reader under these limits refuses the definition's print: {refused}"
        ))
    );
    assert_eq!(print_within(&d, &fewer_values), Ok(print(&d)));
}

/// With `depth` at its hard maximum, the deepest manifest there is fits
/// the stack the crate's documentation names for that.
#[test]
fn the_deepest_manifest_of_all_fits_a_stack_of_1_mib() {
    let limits = Limits {
        depth: usize::MAX,
        ..Limits::default()
    };
    let src = objects_in_unions(64, "number | null");
    on_a_stack(DEEP_STACK, move || {
        let d = host_within(&src, &limits).unwrap();
        assert_eq!(d.limits().depth, 64);
        let written = print_manifest_within(&d, &limits).unwrap();
        assert_eq!(crate::manifest::nesting(&written), 264);
        assert_eq!(manifest_within(&written, &limits).unwrap(), d);
        assert_eq!(
            manifest_within(&canonical_manifest(&d), &limits).unwrap(),
            d
        );
        // Under the default limits it is too deep, as its file is.
        assert!(manifest(&written).unwrap_err().message.starts_with(
            "arrays and objects nested more than 136 deep, the limit for a manifest (column "
        ));
    });
}

#[test]
fn a_manifest_larger_than_the_limit_is_refused_before_it_is_read() {
    // Not JSON at all: nothing looked.
    let src = "x".repeat(256 * 1024 + 1);
    refused_at(
        &src,
        "",
        "the manifest is 262145 bytes long, and the limit is 262144",
    );
    // A manifest's bound is its own: four times a file's.
    let limits = Limits::default();
    assert_eq!(limits.manifest_bytes, 4 * limits.file_bytes);
    let padded = format!(
        r#"{{"clax_surface":1,"name":"demo"{}}}"#,
        " ".repeat(limits.file_bytes)
    );
    assert!(manifest(&padded).is_ok());
    let small = Limits {
        manifest_bytes: 20,
        ..Limits::default()
    };
    let e = manifest_within(r#"{"clax_surface":1,"name":"demo"}"#, &small).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the manifest is 32 bytes long, and the limit is 20"
    );
    // What a definition writes may be more than it was read from, and
    // the writer's check says so.
    let d = manifest(&with_type(&format!(
        r#"{{"enum":[{}]}}"#,
        (0..120)
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    )))
    .unwrap();
    let tight = Limits {
        manifest_bytes: 600,
        ..Limits::default()
    };
    let written = print_manifest(&d);
    assert_eq!(
        print_manifest_within(&d, &tight),
        Err(format!(
            "the definition's manifest is {} bytes long, and the limit is 600",
            written.len()
        ))
    );
    assert!(manifest_within(&written, &tight).is_err());
}

/// A copy of `t` whose interfaces are those of the copy numbered `k`.
fn renamed(t: &Ty, k: usize) -> Ty {
    let field = |f: &Field| Field {
        ty: renamed(&f.ty, k),
        ..f.clone()
    };
    match t {
        Ty::Named(n) => Ty::Named(format!("{n}{k}")),
        Ty::Array(x) => Ty::Array(Box::new(renamed(x, k))),
        Ty::Record(x) => Ty::Record(Box::new(renamed(x, k))),
        Ty::Object(fields) => Ty::Object(fields.iter().map(field).collect()),
        Ty::Union(alternatives) => {
            let mut alternatives = alternatives.iter().map(|x| renamed(x, k));
            let first = alternatives.next().unwrap_or(Ty::Null);
            Ty::union(first, alternatives)
        }
        other => other.clone(),
    }
}

/// `review.d.ts` so many times over as one surface: each copy's
/// operations in a namespace of its own, its interfaces under names of
/// their own.
fn review_times(copies: usize) -> Definition {
    let review = review();
    let members = |members: &[Member], k: usize| -> Vec<Member> {
        fn copy(m: &Member, k: usize) -> Member {
            match m {
                Member::Op(o) => Member::Op(Op {
                    params: o
                        .params
                        .iter()
                        .map(|p| Param {
                            ty: renamed(&p.ty, k),
                            ..p.clone()
                        })
                        .collect(),
                    result: renamed(&o.result, k),
                    ..o.clone()
                }),
                Member::Namespace(n) => Member::Namespace(Namespace {
                    members: n.members.iter().map(|m| copy(m, k)).collect(),
                    ..n.clone()
                }),
            }
        }
        members.iter().map(|m| copy(m, k)).collect()
    };
    let root = Namespace {
        name: "review".into(),
        rules: review.rules().map(str::to_string),
        members: (0..copies)
            .map(|k| {
                Member::Namespace(Namespace {
                    name: format!("copy{k}"),
                    rules: None,
                    members: members(review.members(), k),
                })
            })
            .collect(),
    };
    let interfaces = (0..copies)
        .flat_map(|k| {
            review.interfaces().iter().map(move |i| Interface {
                name: format!("{}{k}", i.name),
                extends: i.extends.iter().map(|e| format!("{e}{k}")).collect(),
                fields: i
                    .fields
                    .iter()
                    .map(|f| Field {
                        ty: renamed(&f.ty, k),
                        ..f.clone()
                    })
                    .collect(),
                ..i.clone()
            })
        })
        .collect();
    let reserved = Reserved::of(&[&own()]);
    let author = Author::Host {
        reserved: &reserved,
    };
    let unbounded = Limits {
        file_bytes: usize::MAX,
        manifest_bytes: usize::MAX,
        ..Limits::default()
    };
    build(
        root,
        interfaces,
        Vec::new(),
        author,
        &unbounded,
        Notation::Dts,
    )
    .unwrap_or_else(|p| panic!("{copies} copies: {}", p.message))
}

/// A `.d.ts` file as large as a file may be has a manifest a reader
/// takes, when it is written as surfaces are: the surfaces in the tree,
/// and one of them repeated up to the limit on a file.
#[test]
fn a_file_at_its_limit_has_a_manifest_within_the_manifests_limit() {
    let limits = Limits::default();
    for (what, def) in [("clax", own()), ("shell", shell()), ("review", review())] {
        let (file, written) = (print(&def), print_manifest(&def));
        assert!(
            written.len() < 3 * file.len() && written.len() <= limits.manifest_bytes,
            "{what}: {} and {} bytes",
            file.len(),
            written.len()
        );
    }
    // The most copies of `review.d.ts` whose file is within the limit:
    // more operations and interfaces than any surface in the tree has.
    let copies = (1..)
        .find(|&n| print(&review_times(n + 1)).len() > limits.file_bytes)
        .unwrap();
    let def = review_times(copies);
    let file = print_within(&def, &limits).unwrap();
    assert!(file.len() > limits.file_bytes * 9 / 10, "{}", file.len());
    assert_eq!(host(&file).unwrap(), def);
    let written = print_manifest_within(&def, &limits).unwrap();
    assert_eq!(manifest(&written).unwrap(), def);
    assert_eq!(manifest(&canonical_manifest(&def)).unwrap(), def);
    // Laid out, it is under three times its file; its canonical bytes
    // are fewer.
    assert!(written.len() < 3 * file.len(), "{}", written.len());
    assert!(canonical_manifest(&def).len() < written.len());
}

/// The files that cost a manifest the most bytes for each of their own.
fn dense_files() -> Vec<(&'static str, String)> {
    let names: Vec<String> = ('a'..='z')
        .chain('A'..='Z')
        .map(String::from)
        .chain(('a'..='z').flat_map(|a| ('a'..='c').map(move |b| format!("{a}{b}"))))
        .take(128)
        .collect();
    let limit = Limits::default().file_bytes;
    // Interfaces whose fields are empty object types.
    let mut fields = String::from("declare const demo:{}");
    for i in 0..256 {
        let body: String = names.iter().map(|n| format!("{n}:{{}},")).collect();
        let next = format!("interface I{i}{{{body}}}");
        if fields.len() + next.len() > limit {
            break;
        }
        fields.push_str(&next);
    }
    // Empty namespaces, as deep as namespaces go.
    let mut namespaces = String::from("declare const demo:{");
    let mut groups = 0;
    loop {
        let body: String = names.iter().map(|n| format!("{n}:{{}},")).collect();
        let next = format!(
            "g{groups}:{}{{{body}}}{},",
            "{a:".repeat(30),
            "}".repeat(30)
        );
        if namespaces.len() + next.len() + 2 > limit || groups == 128 {
            break;
        }
        namespaces.push_str(&next);
        groups += 1;
    }
    namespaces.push_str("};");
    // Object types nested as deep as types go, each of one field.
    let chain = |n: &String| format!("{n}:{}{{}}{},", "{a:".repeat(31), "}".repeat(31));
    let mut chains = String::from("declare const demo:{}");
    for i in 0..256 {
        let room = limit.saturating_sub(chains.len() + 16) / chain(&names[127]).len();
        if room == 0 {
            break;
        }
        let body: String = names.iter().take(room).map(chain).collect();
        chains.push_str(&format!("interface I{i}{{{body}}}"));
    }
    vec![
        ("fields of empty object types", fields),
        ("empty namespaces", namespaces),
        ("object types of one field, nested", chains),
    ]
}

/// A file written to be as dense as the notation allows (no
/// documentation, one-letter names, empty object types) has a manifest
/// of fifteen to twenty-two times its size, which is more than a
/// manifest may be: it has none under the default limits, the writer's
/// check says so, and nothing else stands between it and one.
#[test]
fn a_file_made_dense_has_a_manifest_many_times_its_size() {
    let limits = Limits::default();
    let unbounded = Limits {
        manifest_bytes: usize::MAX,
        manifest_values: usize::MAX,
        ..Limits::default()
    };
    let mut ratios = Vec::new();
    for (what, file) in dense_files() {
        assert!(
            file.len() > limits.file_bytes * 9 / 10 && file.len() <= limits.file_bytes,
            "{what}: {}",
            file.len()
        );
        let d = host(&file).unwrap_or_else(|e| panic!("{what}: {e}"));
        let written = print_manifest(&d);
        assert_eq!(
            print_manifest_within(&d, &limits),
            Err(format!(
                "the definition's manifest is {} bytes long, and the limit is 262144",
                written.len()
            )),
            "{what}"
        );
        assert_eq!(manifest_within(&written, &unbounded).unwrap(), d, "{what}");
        ratios.push((
            written.len() / file.len(),
            canonical_manifest(&d).len() / file.len(),
        ));
    }
    // Laid out, and as canonical bytes: whole multiples of the file.
    assert_eq!(ratios, [(15, 12), (15, 12), (22, 20)]);
}

/// The lists of a manifest are bounded as a file's are, by the
/// definition's own checks.
#[test]
fn lists_longer_than_the_limit_are_refused() {
    let many = |n: usize| {
        (0..n)
            .map(|i| format!("\"l{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    assert!(manifest(&with_type(&format!(r#"{{"enum":[{}]}}"#, many(128)))).is_ok());
    refused_at(
        &with_type(&format!(r#"{{"enum":[{}]}}"#, many(129))),
        "/operations/0/params/0",
        "operation `demo.x` has a union of 129 alternatives, where the limit is 128",
    );
    // As many as a manifest of 64 KiB holds, all the same.
    let same = vec!["\"a\""; 15_000].join(",");
    refused_at(
        &with_type(&format!(r#"{{"enum":[{same}]}}"#)),
        "/operations/0/params/0",
        "a union of 15000 alternatives, where the limit is 128",
    );
    let fields = (0..129)
        .map(|i| format!("\"f{i}\":{{}}"))
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &with_type(&format!(
            r#"{{"type":"object","properties":{{{fields}}},"additionalProperties":false}}"#
        )),
        "/operations/0/params/0",
        "an object type with 129 fields, where the limit is 128",
    );
    refused_at(
        &document(
            "",
            &format!(
                r#","interfaces":{{"A":{{"type":"object","properties":{{{fields}}},"additionalProperties":false}}}}"#
            ),
        ),
        "/interfaces/A",
        "interface `A` has 129 fields, and the limit is 128",
    );
    let codes = (0..129)
        .map(|i| format!("\"c{i}\""))
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &document(
            &operation(&format!(
                r#","description":"D.","access":"read","result":{{}},"throws":[{codes}]"#
            )),
            "",
        ),
        "/operations/0/throws/128",
        "operation `demo.x` names 129 error codes, and the limit is 128",
    );
    let interfaces = (0..257)
        .map(|i| format!(r#""I{i}":{{"type":"object","additionalProperties":false}}"#))
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &document("", &format!(r#","interfaces":{{{interfaces}}}"#)),
        "/interfaces/I256",
        "the manifest lists 257 interfaces, and the limit is 256",
    );
    let kinds = (0..65)
        .map(|i| format!(r#"{{"name":"k{i}","description":"K.","request":{{}},"result":{{}}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &document("", &format!(r#","kinds":[{kinds}]"#)),
        "/kinds/64",
        "the manifest lists 65 kinds, and the limit is 64",
    );
    let ops = (0..257)
        .map(|i| {
            format!(
                r#"{{"name":"n{}.o{i}","description":"D.","access":"read","result":{{}}}}"#,
                i / 100
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &document(&ops, ""),
        "/operations/256",
        "the surface has more operations than the limit, 256",
    );
    let members = (0..129)
        .map(|i| format!(r#"{{"name":"o{i}","description":"D.","access":"read","result":{{}}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    refused_at(
        &document(&members, ""),
        "/operations",
        "`demo` has 129 members, and the limit is 128",
    );
    let long = "n".repeat(65);
    refused_at(
        &document(&format!(r#"{{"namespace":"{long}"}}"#), ""),
        "/operations/0",
        "is 65 bytes long, and the limit for a name is 64",
    );
    refused_at(
        &document(
            &operation(&format!(
                r#","description":"{}","access":"read","result":{{}}"#,
                "d".repeat(4097)
            )),
            "",
        ),
        "/operations/0",
        "the `description` of operation `demo.x` is 4097 bytes long, and the limit is 4096",
    );
    refused_at(
        &format!(
            r#"{{"clax_surface":1,"name":"demo","rules":"{}"}}"#,
            "r".repeat(4097)
        ),
        "/rules",
        "the surface's rules is 4097 bytes long, and the limit is 4096",
    );
}

/// Characters a reader would not see are refused in a manifest as in a
/// file, wherever a string of it reaches a definition, and no refusal
/// shows one or grows with its input.
#[test]
fn hidden_characters_and_long_keys_are_refused_and_not_shown() {
    let full = r#","description":"D.","access":"read","result":{"type":"null"}"#;
    // As the text has them, and as JSON escapes them.
    for hidden in [
        "\u{202e}",
        "\\u202e",
        "\\u200b",
        "\\u0000",
        "\\u001b",
        "\\ufeff",
        "\\udb40\\udc01",
    ] {
        let read: String = serde_json::from_str(&format!("\"{hidden}\"")).unwrap();
        let code = read.chars().next().unwrap() as u32;
        let name = format!("\\u{{{code:x}}}");
        for (src, pointer, needle) in [
            (
                format!(r#"{{"clax_surface":1,"name":"de{hidden}mo"}}"#),
                "/name".to_string(),
                format!("the surface's name `de{name}mo` is not one lowercase word"),
            ),
            (
                format!(r#"{{"clax_surface":1,"name":"demo","rules":"R{hidden}."}}"#),
                "/rules".to_string(),
                format!("the surface's rules holds the character U+{code:04X}"),
            ),
            (
                format!(r#"{{"clax_surface":1,"name":"demo","ru{hidden}les":"R."}}"#),
                format!("/ru{name}les"),
                format!("`ru{name}les` is not a field of a manifest"),
            ),
            (
                document(&format!(r#"{{"name":"x{hidden}"{full}}}"#), ""),
                "/operations/0".to_string(),
                format!("`x{name}` is not a name a script can call"),
            ),
            (
                document(
                    &operation(&format!(
                        r#","description":"D{hidden}.","access":"read","result":{{}}"#
                    )),
                    "",
                ),
                "/operations/0".to_string(),
                format!("holds the character U+{code:04X}"),
            ),
            (
                document(
                    &operation(&format!(
                        r#","description":"D.","access":"re{hidden}ad","result":{{}}"#
                    )),
                    "",
                ),
                "/operations/0/access".to_string(),
                format!("`re{name}ad` is not an access"),
            ),
            (
                document(
                    &operation(&format!(r#"{full},"throws":["go{hidden}ne"]"#)),
                    "",
                ),
                "/operations/0/throws/0".to_string(),
                format!("`go{name}ne` is not an error code"),
            ),
            (
                with_type(&format!(r#"{{"const":"a{hidden}b"}}"#)),
                "/operations/0/params/0".to_string(),
                format!("has a string literal that holds the character U+{code:04X}"),
            ),
            (
                with_type(&format!(r#"{{"$ref":"clax:{hidden}"}}"#)),
                "/operations/0/params/0/schema/$ref".to_string(),
                format!("and `clax:{name}` is neither"),
            ),
            (
                with_type(&format!(r##"{{"$ref":"#/interfaces/A{hidden}"}}"##)),
                "/operations/0/params/0".to_string(),
                format!("uses the type `A{name}`, which `interfaces` does not list"),
            ),
            (
                with_type(&format!(r#"{{"type":"str{hidden}ing"}}"#)),
                "/operations/0/params/0/schema/type".to_string(),
                format!("`str{name}ing` is not a `type` of the format"),
            ),
            (
                with_type(&format!(r#"{{"mini{hidden}mum":1}}"#)),
                format!("/operations/0/params/0/schema/mini{name}mum"),
                format!("`mini{name}mum` is not read here"),
            ),
            (
                with_type(&format!(
                    r#"{{"type":"object","properties":{{"f{hidden}":{{}}}},"additionalProperties":false}}"#
                )),
                format!("/operations/0/params/0/schema/properties/f{name}"),
                format!("has an object type with a field named `f{name}`"),
            ),
            (
                document(
                    "",
                    &format!(
                        r#","interfaces":{{"A{hidden}":{{"type":"object","additionalProperties":false}}}}"#
                    ),
                ),
                format!("/interfaces/A{name}"),
                format!("`A{name}` is not a name for an interface"),
            ),
            (
                document(
                    "",
                    &format!(
                        r#","interfaces":{{"A":{{"type":"object","extends":["B{hidden}"],"additionalProperties":false}}}}"#
                    ),
                ),
                "/interfaces/A/extends/0".to_string(),
                format!("interface `A` extends `B{name}`, which `interfaces` does not list"),
            ),
            (
                document(
                    "",
                    &format!(
                        r#","kinds":[{{"name":"k","description":"K.","allow":["a.{hidden}"],"request":{{}},"result":{{}}}}]"#
                    ),
                ),
                "/kinds/0/allow/0".to_string(),
                format!("`a.{name}` is not an allow pattern"),
            ),
        ] {
            refused_at(&src, &pointer, &needle);
        }
    }
    // A key is shown cut, with `~` and `/` as a JSON Pointer writes them.
    let long = "k".repeat(5_000);
    let cut = format!("{}…", "k".repeat(64));
    refused_at(
        &format!(r#"{{"clax_surface":1,"name":"demo","{long}":1}}"#),
        &format!("/{cut}"),
        &format!("`{cut}` is not a field of a manifest"),
    );
    refused_at(
        &document(
            "",
            &format!(
                r#","interfaces":{{"{long}":{{"type":"object","additionalProperties":false}}}}"#
            ),
        ),
        &format!("/interfaces/{cut}"),
        "is 5000 bytes long, and the limit for a name is 64",
    );
    refused_at(
        r#"{"clax_surface":1,"name":"demo","a/b~c":1}"#,
        "/a~1b~0c",
        "`a/b~c` is not a field of a manifest",
    );
}

/// An interface may name itself and those that name it; `extends` may
/// not go round. Neither is followed while a manifest is read.
#[test]
fn references_that_go_round_are_names_and_not_followed() {
    let interfaces = |a: &str, b: &str| {
        document(
            &operation(
                r##","description":"D.","access":"read","result":{"$ref":"#/interfaces/A"}"##,
            ),
            &format!(
                r##","interfaces":{{"A":{{"type":"object",{a}"additionalProperties":false}},"B":{{"type":"object",{b}"additionalProperties":false}}}}"##
            ),
        )
    };
    let d = manifest(&interfaces(
        r##""properties":{"b":{"$ref":"#/interfaces/B"},"self":{"type":"array","items":{"$ref":"#/interfaces/A"}}},"##,
        r##""properties":{"a":{"anyOf":[{"$ref":"#/interfaces/A"},{"type":"null"}]}},"required":["a"],"##,
    ))
    .unwrap();
    assert_eq!(
        d,
        host("declare const demo: {\n/** D. @read */\nx(): Promise<A>;\n};\ninterface A { b?: B; self?: A[] }\ninterface B { a: A | null }").unwrap()
    );
    round_trips("interfaces that name each other", &d, &host, &manifest);
    refused_at(
        &interfaces(r#""extends":["B"],"#, r#""extends":["A"],"#),
        "/interfaces/A",
        "interface `A` extends itself, through `B`: `extends` may not go round in a circle",
    );
}

// ------------------------------------------------------------- many mistakes

/// A manifest is read to its end: each entry of `operations`, of
/// `interfaces` and of `kinds` that cannot be read says its first
/// mistake, and the rules of a definition are checked once every entry
/// reads.
#[test]
fn every_entry_that_cannot_be_read_says_its_first_mistake() {
    let ok = r#"{"name":"b","description":"D.","access":"read","result":{}}"#;
    let src = document(
        &[
            r#"{"name":"a","description":"D.","access":"Read","result":{}}"#,
            ok,
            // Two mistakes in one entry: the first is said.
            r#"{"name":"c","description":"D.","access":"read","params":[{"name":"n","schema":{"type":"integer","minimum":1}}],"result":{},"kind":"x"}"#,
            r#"{"namespace":"n","rules":7}"#,
            "7",
        ]
        .join(","),
        r#","interfaces":{"A":{"type":"string"},
                         "B":{"type":"object","additionalProperties":false},
                         "C":{"type":"object","properties":{"x":{"type":"str"}},"additionalProperties":false}},
            "kinds":[{"name":"k"},{"name":"l","description":"K.","request":{},"result":{}}],
            "extra":1"#,
    );
    let errors = manifest_all(&src, &Limits::default()).unwrap_err();
    let said: Vec<(&str, &str)> = errors
        .iter()
        .map(|e| (e.pointer().unwrap(), e.message.as_str()))
        .collect();
    let expected = [
        ("/operations/0/access", "`Read` is not an access"),
        (
            "/operations/2/params/0/schema/minimum",
            "`minimum` is not read here",
        ),
        ("/operations/3/rules", "`rules` is a string"),
        ("/operations/4", "an entry of `operations` ("),
        ("/interfaces/A", "an entry of `interfaces` is an interface"),
        (
            "/interfaces/C/properties/x/type",
            "`str` is not a `type` of the format",
        ),
        ("/kinds/0", "kind `k` has no `description`"),
        ("/extra", "`extra` is not a field of a manifest"),
    ];
    assert_eq!(said.len(), expected.len(), "{errors}");
    for ((pointer, message), (at, needle)) in said.iter().zip(expected) {
        assert_eq!(*pointer, at, "{errors}");
        assert!(message.starts_with(needle), "{message}");
    }
    assert_eq!(errors.count(), 8);
    assert_eq!(errors.first().pointer(), Some("/operations/0/access"));
    // They print one a line, in the manifest's order.
    let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(errors.to_string(), lines.join("\n"));
    assert_eq!(errors.clone().into_vec().len(), 8);
    // A section of the wrong form is one mistake, and the others are
    // still read.
    let errors = manifest_all(
        r#"{"clax_surface":1,"name":"demo","operations":{},"interfaces":[],"kinds":[7]}"#,
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        errors.to_string(),
        "/operations: `operations` is an array\n/interfaces: `interfaces` is an object\n/kinds/0: a kind is an object"
    );
    // What the document itself gets wrong is said alone: nothing else
    // can be read by it.
    for (src, said) in [
        (
            r#"{"clax_surface":2,"name":7,"operations":[7]}"#,
            "/clax_surface: this build reads version 1 of the manifest notation (`\"clax_surface\": 1`), and no other",
        ),
        (
            r#"{"clax_surface":1,"name":7,"operations":[7]}"#,
            "/name: the surface's `name` is a string",
        ),
    ] {
        assert_eq!(
            manifest_all(src, &Limits::default())
                .unwrap_err()
                .to_string(),
            said
        );
    }
    let cut = manifest_all(
        "{\"clax_surface\":1,\"name\":\"demo\",\"operations\":[7,",
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!((cut.count(), cut.first().line()), (1, Some(1)));
    // The rules of a definition are checked when its entries all read,
    // and the first that is broken is said: here the surface's name is
    // reserved, and an operation charges.
    let reserved = |operation: &str| {
        format!(r#"{{"clax_surface":1,"name":"files","operations":[{operation}]}}"#)
    };
    let spends = r#"{"name":"x","description":"D.","access":"spend","result":{}}"#;
    assert_eq!(
        manifest_all(&reserved(&format!("{spends},7")), &Limits::default())
            .unwrap_err()
            .to_string(),
        "/operations/1: an entry of `operations` (an operation, or a namespace as `{\"namespace\": \"settings\", \"rules\": \"…\"}`) is an object"
    );
    let errors = manifest_all(&reserved(spends), &Limits::default()).unwrap_err();
    assert_eq!(errors.count(), 1);
    assert!(
        errors
            .first()
            .message
            .starts_with("`files` is a reserved name")
    );
}

/// What a manifest's reader answers and holds is bounded whatever the
/// text: it says so many mistakes and then that it stopped, and it
/// reads so many values and no more.
#[test]
fn the_mistakes_said_and_the_values_read_are_bounded() {
    let limits = Limits::default();
    assert_eq!(
        (limits.manifest_errors, limits.manifest_values),
        (64, 65_536)
    );
    // A hundred entries, each wrong: 64 are said, and that 36 are left.
    let wrong = |n: usize| document(&vec!["7"; n].join(","), "");
    let errors = manifest_all(&wrong(100), &limits).unwrap_err();
    assert_eq!((errors.count(), errors.unread()), (65, 36));
    let all: Vec<&Error> = errors.iter().collect();
    assert_eq!(all[63].pointer(), Some("/operations/63"));
    assert_eq!(
        all[64].to_string(),
        "64 mistakes are said, and the 36 entries of the manifest after them were not looked at: put these right, and read it again"
    );
    // As many as are said, and no more: nothing was left.
    let errors = manifest_all(&wrong(64), &limits).unwrap_err();
    assert_eq!((errors.count(), errors.unread()), (64, 0));
    // What is left is counted across the document, not read: entries of
    // `operations`, of `interfaces` and of `kinds`, and members the
    // notation does not name.
    let src = format!(
        r#"{{"clax_surface":1,"name":"demo","zzz":1,"kinds":[{}],"interfaces":{{"A":1,"B":2}},"operations":[7,7,7],"yyy":2}}"#,
        vec!["7"; 70].join(",")
    );
    let errors = manifest_all(&src, &limits).unwrap_err();
    assert_eq!(errors.first().pointer(), Some("/zzz"));
    assert_eq!((errors.count(), errors.unread()), (65, 7 + 2 + 3 + 1));
    let few = Limits {
        manifest_errors: 2,
        ..Limits::default()
    };
    assert_eq!(
        manifest_all(&wrong(5), &few).unwrap_err().to_string(),
        "/operations/0: an entry of `operations` (an operation, or a namespace as `{\"namespace\": \"settings\", \"rules\": \"…\"}`) is an object\n/operations/1: an entry of `operations` (an operation, or a namespace as `{\"namespace\": \"settings\", \"rules\": \"…\"}`) is an object\n2 mistakes are said, and the 3 entries of the manifest after them were not looked at: put these right, and read it again"
    );
    // One of each is said as one.
    let one = Limits {
        manifest_errors: 1,
        ..Limits::default()
    };
    assert!(
        manifest_all(&wrong(2), &one).unwrap_err().to_string().ends_with(
            "\n1 mistake is said, and the 1 entry of the manifest after it was not looked at: put this right, and read it again"
        )
    );
    assert!(
        manifest_all(&wrong(3), &one).unwrap_err().to_string().ends_with(
            "\n1 mistake is said, and the 2 entries of the manifest after it were not looked at: put this right, and read it again"
        )
    );
    // A limit of none is a limit of one: a refusal says something.
    let none = Limits {
        manifest_errors: 0,
        ..Limits::default()
    };
    assert_eq!(manifest_all(&wrong(5), &none).unwrap_err().count(), 2);
    // Manifests as long as a manifest may be, of nothing but entries
    // that are wrong: the answer is a few kilobytes, not megabytes.
    let filled = |open: &str, entry: &dyn Fn(usize) -> String, close: &str| {
        let mut src = format!(r#"{{"clax_surface":1,"name":"demo",{open}"#);
        let mut n = 0;
        loop {
            let next = entry(n);
            if src.len() + next.len() + close.len() + 1 > limits.manifest_bytes {
                break;
            }
            if n > 0 {
                src.push(',');
            }
            src.push_str(&next);
            n += 1;
        }
        src.push_str(close);
        (src, n)
    };
    for (what, (src, entries)) in [
        (
            "numbers",
            filled(r#""operations":["#, &|_| "1".into(), "]}"),
        ),
        (
            "empty operations",
            filled(r#""operations":["#, &|_| "{}".into(), "]}"),
        ),
        (
            "empty kinds",
            filled(r#""kinds":["#, &|_| "{}".into(), "]}"),
        ),
        (
            "interfaces that are numbers",
            filled(r#""interfaces":{"#, &|n| format!("\"{n}\":1"), "}}"),
        ),
        // Under the limit on values, so that every entry is one read.
        (
            "long operations",
            filled(
                r#""operations":["#,
                &|n| format!(r#"{{"name":"o{n}","description":"{}"}}"#, "d".repeat(40)),
                "]}",
            ),
        ),
    ] {
        assert!(src.len() > limits.manifest_bytes - 80, "{what}");
        let errors = on_a_small_stack(move || manifest_all(&src, &Limits::default())).unwrap_err();
        let said = errors.to_string();
        assert!(errors.count() <= 65, "{what}: {}", errors.count());
        assert!(said.len() < 16 * 1024, "{what}: {} bytes", said.len());
        if what == "long operations" || what == "interfaces that are numbers" {
            assert_eq!(
                (errors.count(), errors.unread()),
                (65, entries - 64),
                "{what}"
            );
        } else {
            // More values than a manifest may hold: one mistake, and
            // nothing kept of the text.
            assert_eq!(errors.count(), 1, "{what}");
            assert!(
                said.contains("more than 65536 values, the limit for a manifest (column "),
                "{what}: {said}"
            );
            assert_eq!(errors.first().line(), Some(1), "{what}");
        }
    }
    // The limit on values is far from a manifest that reads: one as
    // long as a manifest may be and as full of values as the notation
    // lets one be (unions of 128 short literals) has fewer.
    let literals: Vec<String> = ('a'..='z')
        .chain('A'..='Z')
        .map(String::from)
        .chain(('a'..='z').flat_map(|a| ('a'..='c').map(move |b| format!("{a}{b}"))))
        .take(128)
        .map(|l| format!("\"{l}\""))
        .collect();
    let field = |n: &String| format!("{n}:{{\"enum\":[{}]}}", literals.join(","));
    let (src, interfaces) = filled(
        r#""interfaces":{"#,
        &|n| {
            format!(
                r#""I{n}":{{"type":"object","properties":{{{}}},"additionalProperties":false}}"#,
                literals.iter().map(field).collect::<Vec<_>>().join(",")
            )
        },
        "}}",
    );
    assert!(interfaces >= 3 && src.len() > limits.manifest_bytes * 3 / 4);
    let d = manifest(&src).unwrap();
    assert_eq!(d.interfaces().len(), interfaces);
    let values = src.matches(['"', '{', '[']).count();
    assert!(values < 2 * limits.manifest_values, "{values}");
    let tight = Limits {
        manifest_values: 1_000,
        ..Limits::default()
    };
    assert_eq!(
        manifest_within(&src, &tight).unwrap_err().message,
        "more than 1000 values, the limit for a manifest (column 4673)"
    );
}

/// The mistakes are said in the order the text has them, and one
/// mistake is said once.
#[test]
fn mistakes_come_in_the_texts_order_and_each_once() {
    let errors = manifest_all(
        r#"{"clax_surface":1,"name":"demo","zzz":1,"kinds":[{}],"operations":[{}]}"#,
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        errors
            .iter()
            .map(|e| e.pointer().unwrap())
            .collect::<Vec<_>>(),
        ["/zzz", "/kinds/0", "/operations/0"]
    );
    // A namespace listed in two places is the mistake of the entry that
    // comes back to it, and not of each one after.
    let op = |name: &str| {
        format!(r#"{{"name":"{name}","description":"D.","access":"read","result":{{}}}}"#)
    };
    let entries = |names: &[&str]| {
        document(
            &names.iter().map(|n| op(n)).collect::<Vec<_>>().join(","),
            "",
        )
    };
    let errors = manifest_all(
        &entries(&["a.x", "y", "a.z1", "a.z2", "a.b.z3"]),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        errors.to_string(),
        "/operations/2/name: namespace `a` is listed in two places: the members of one namespace follow one another in `operations`"
    );
    // Each time it is come back to is a mistake of its own.
    let errors = manifest_all(
        &entries(&["a.x", "y", "a.z1", "a.z2", "w", "a.b.z3", "a.b.z4"]),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        errors
            .iter()
            .map(|e| e.pointer().unwrap())
            .collect::<Vec<_>>(),
        ["/operations/2/name", "/operations/5/name"]
    );
}

/// One byte-order mark before a manifest is skipped, as one before a
/// `.d.ts` file is.
#[test]
fn a_byte_order_mark_before_a_manifest_is_skipped() {
    let plain = r#"{"clax_surface":1,"name":"demo","operations":[]}"#;
    assert_eq!(
        manifest(&format!("\u{feff}{plain}")).unwrap(),
        manifest(plain).unwrap()
    );
    assert_eq!(
        host("\u{feff}declare const demo: {};").unwrap(),
        manifest(plain).unwrap()
    );
    // One, and at the start.
    refused_on(
        &format!("\u{feff}\u{feff}{plain}"),
        1,
        "the manifest is not JSON: expected value",
    );
    refused_on(
        &plain.replace("1,", "1,\u{feff}"),
        1,
        "the manifest is not JSON: key must be a string",
    );
    // Its line is still its line.
    refused_on(
        "\u{feff}{\n\"clax_surface\":1,\n\"name\":\"demo\",\n}",
        4,
        "the manifest is not JSON: trailing comma",
    );
}

/// A script reaches a surface's members below the surface's name, so a
/// namespace directly on the surface does not take that name again: in
/// a manifest, an operation's `name` does not start with it.
#[test]
fn a_namespace_on_a_surface_is_not_named_as_the_surface() {
    let op = |name: &str| {
        format!(
            r#"{{"name":"{name}","description":"D.","access":"read","result":{{"type":"null"}}}}"#
        )
    };
    let said = "`demo` is the surface's own name, and names nothing below it: an operation and a namespace are named below their surface (`f`, which a script calls as `clax.demo.f`, not `demo.f`)";
    refused_at(&document(&op("demo.f"), ""), "/operations/0/name", said);
    refused_at(
        &document(&[op("a"), op("demo.deep.f")].join(","), ""),
        "/operations/1/name",
        said,
    );
    refused_at(
        &document(r#"{"namespace":"demo","rules":"R."}"#, ""),
        "/operations/0",
        said,
    );
    super::refused(
        "declare const demo: {\n  demo: {\n    /** D. @read */\n    f(): Promise<null>;\n  };\n};",
        2,
        "namespace `demo` has the name of the surface it is on: a script would reach its operations as `clax.demo.demo.…`, and the surface's members are below `demo` already",
    );
    // An operation may have its surface's name, and so may a namespace
    // that is not directly on it.
    let d = manifest(&document(&[op("demo"), op("n.demo.f")].join(","), "")).unwrap();
    assert_eq!(
        d.operations().map(|o| o.name).collect::<Vec<_>>(),
        ["demo.demo", "demo.n.demo.f"]
    );
    round_trips("names of the surface", &d, &host, &manifest);
    // Clax's own surface keeps the rule.
    let own = r#"{"clax_surface":1,"name":"clax","operations":[{"name":"clax.x","description":"D.","access":"read","result":{}}]}"#;
    let e = parse_manifest(own, Author::Clax, &Limits::default()).unwrap_err();
    assert!(
        e.first()
            .message
            .starts_with("`clax` is the surface's own name")
    );
}

/// No surface is named `core` or `rules`, a host's or Clax's own, in
/// either notation: a prompt file's slots of the core's begin so.
#[test]
fn no_surface_is_named_as_the_cores_slots_begin() {
    for name in ["core", "rules"] {
        let said = format!(
            "`{name}` cannot name a surface: in a prompt file the slots `{{core.…}}` and `{{rules.…}}` are the core's, so a surface named `core` or `rules` could have no slot of its own"
        );
        let (file, written) = (
            format!("declare const {name}: {{}};"),
            format!(r#"{{"clax_surface":1,"name":"{name}"}}"#),
        );
        super::refused(&file, 1, &said);
        refused_at(&written, "/name", &said);
        let limits = Limits::default();
        assert_eq!(
            parse(&file, Author::Clax, &limits).unwrap_err().message,
            said
        );
        assert_eq!(
            parse_manifest(&written, Author::Clax, &limits)
                .unwrap_err()
                .first()
                .message,
            said
        );
    }
    // Longer names that begin so are names like any other.
    assert!(host("declare const cores: {};").is_ok());
    assert!(manifest(r#"{"clax_surface":1,"name":"rules2"}"#).is_ok());
}

// --------------------------------------------------------------------- MCP

/// An MCP server's answer to `tools/list`, as servers write them: one
/// from zod (`$schema`, a constraint, a `type` that is a list, a nested
/// object left open), one from pydantic (`title` on everything, `$defs`
/// and a `$ref` into them, `anyOf` with `null`, defaults), a tool with
/// no arguments and a name that is no identifier, one with no
/// description, a property that is no identifier and an `enum` of
/// numbers, and one with an `outputSchema` and a word in its
/// description that starts with `@`.
const MCP_TOOLS: &str = r##"{
  "tools": [
    {
      "name": "create_issue",
      "description": "Create a new issue in a GitHub repository.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "owner": {"type": "string", "description": "Repository owner"},
          "title": {"type": "string", "minLength": 1},
          "labels": {"type": "array", "items": {"type": "string"}, "default": []},
          "milestone": {"type": "number"},
          "assignee": {"type": ["string", "null"]},
          "options": {
            "type": "object",
            "properties": {"draft": {"type": "boolean", "description": "Open as a draft", "default": false}}
          }
        },
        "required": ["owner", "title"],
        "additionalProperties": false,
        "$schema": "http://json-schema.org/draft-07/schema#"
      }
    },
    {
      "name": "search",
      "description": "Search the web.\n\nArgs:\n    query: what to look for\n",
      "inputSchema": {
        "$defs": {
          "Filter": {
            "properties": {
              "site": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null, "title": "Site"}
            },
            "title": "Filter",
            "type": "object"
          }
        },
        "properties": {
          "query": {"title": "Query", "type": "string"},
          "limit": {"default": 10, "maximum": 50, "minimum": 1, "title": "Limit", "type": "integer"},
          "filter": {"anyOf": [{"$ref": "#/$defs/Filter"}, {"type": "null"}], "default": null}
        },
        "required": ["query"],
        "title": "searchArguments",
        "type": "object"
      }
    },
    {
      "name": "list-windows",
      "description": "List open windows",
      "inputSchema": {"type": "object", "properties": {}},
      "annotations": {"readOnlyHint": true}
    },
    {
      "name": "http.request",
      "inputSchema": {
        "type": "object",
        "properties": {
          "url": {"type": "string", "format": "uri"},
          "content-type": {"type": "string"},
          "retries": {"enum": [0, 1, 3]},
          "headers": {"type": "object", "additionalProperties": {"type": "string"}},
          "body": {}
        },
        "required": ["url"]
      }
    },
    {
      "name": "notify",
      "description": "Notify a user. Use @here to reach everyone.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "text": {"type": "string"},
          "kind": {"oneOf": [{"const": "info"}, {"const": "warn"}]}
        },
        "required": ["text"]
      },
      "outputSchema": {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]}
    }
  ]
}"##;

/// Whether `name` can name a field or an operation.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !["__proto__", "constructor", "prototype", "then"].contains(&name)
}

/// A tool's words as a definition holds documentation: on one line, a
/// word that starts with `@` in backticks, nothing that would end a
/// comment or that does not show, and not too long.
fn bridged_text(text: Option<&str>) -> Option<String> {
    let words: Vec<String> = text?
        .replace("*/", "* /")
        .split_whitespace()
        .map(|w| {
            if w.starts_with('@') {
                format!("`{w}`")
            } else {
                w.to_string()
            }
        })
        .collect();
    let line: String = words
        .join(" ")
        .chars()
        .filter(|&c| !unshowable(c))
        .take(1000)
        .collect();
    Some(line.trim().to_string()).filter(|l| !l.is_empty())
}

/// A union of `alternatives`, when they are two or more types that
/// differ and none of them is `unknown`; the one, when one; else
/// `unknown`.
fn bridged_union(alternatives: Vec<Value>) -> Value {
    let known = alternatives.iter().all(|a| a != &json!({}));
    let distinct = (1..alternatives.len()).all(|i| !alternatives[..i].contains(&alternatives[i]));
    match alternatives.len() {
        1 => alternatives.into_iter().next().unwrap_or_default(),
        n if n > 1 && known && distinct => json!({"anyOf": alternatives}),
        _ => json!({}),
    }
}

/// A type of the manifest for a JSON Schema: what the subset says is
/// kept, every object type is closed, and what it does not say is
/// `unknown` (`{}`), which takes any value. `defs` is the schema's
/// `$defs`; `documented` whether the type is an interface's, whose
/// fields keep their descriptions; `budget` how many more schemas may be
/// gone into, since `$defs` can name each other in a circle.
fn bridged_type(schema: &Value, defs: &Value, documented: bool, budget: usize) -> Value {
    let (Some(s), Some(budget)) = (schema.as_object(), budget.checked_sub(1)) else {
        return json!({});
    };
    let inner = |schema: &Value| bridged_type(schema, defs, false, budget);
    if let Some(target) = s.get("$ref").and_then(Value::as_str) {
        return inner(&defs[target.rsplit('/').next().unwrap_or_default()]);
    }
    if let Some(types) = s.get("type").and_then(Value::as_array) {
        return bridged_union(types.iter().map(|t| inner(&json!({"type": t}))).collect());
    }
    if let Some(alternatives) = ["anyOf", "oneOf"]
        .iter()
        .find_map(|key| s.get(*key).and_then(Value::as_array))
    {
        return bridged_union(alternatives.iter().map(inner).collect());
    }
    let literals = match (s.get("enum").and_then(Value::as_array), s.get("const")) {
        (Some(values), _) => Some(values.clone()),
        (None, Some(value)) => Some(vec![value.clone()]),
        (None, None) => None,
    };
    if let Some(values) = literals {
        let fits = !values.is_empty() && values.iter().all(|v| v.is_string() || v.is_boolean());
        return if fits {
            json!({"enum": values})
        } else {
            json!({})
        };
    }
    match s.get("type").and_then(Value::as_str) {
        Some(t @ ("string" | "number" | "integer" | "boolean" | "null")) => json!({"type": t}),
        Some("array") => {
            json!({"type": "array", "items": inner(s.get("items").unwrap_or(&json!({})))})
        }
        Some("object") => {
            if let Some(values) = s.get("additionalProperties").filter(|v| v.is_object()) {
                return json!({"type": "object", "additionalProperties": inner(values)});
            }
            let none = serde_json::Map::new();
            let properties = s
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&none);
            // A field a definition cannot name: any keys, any values.
            if !properties.keys().all(|k| is_identifier(k)) {
                return json!({"type": "object", "additionalProperties": {}});
            }
            let mut fields = serde_json::Map::new();
            for (name, property) in properties {
                let mut field = inner(property);
                let text = bridged_text(property["description"].as_str());
                if let (true, Some(text)) = (documented, text) {
                    field["description"] = json!(text);
                }
                fields.insert(name.clone(), field);
            }
            let required: Vec<&Value> = s
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|n| n.as_str().is_some_and(|n| fields.contains_key(n)))
                .collect();
            json!({"type": "object", "properties": fields, "required": required, "additionalProperties": false})
        }
        _ => json!({}),
    }
}

/// The manifest of a surface named `name` for an MCP tool list: what a
/// bridge has to do, and no more.
///
/// - A tool is an operation. Its name is the tool's with every
///   character an identifier cannot hold written `_`; its description
///   the tool's ([`bridged_text`]), or a sentence that names the tool
///   when it has none; its access `read` when the server says the tool
///   only reads, else `write`.
/// - A tool takes one object of named arguments and an operation
///   positional parameters, so the operation has one parameter, `args`:
///   an interface made of the tool's `inputSchema` ([`bridged_type`]),
///   which keeps its fields' descriptions, or, when a field of it
///   cannot be named, an object of any keys.
/// - Its result is `unknown`: a server's `outputSchema` leaves its
///   objects open, and a result checked against a closed type would be
///   refused for a field the schema did not list.
///
/// What is lost: every range, length, pattern, format and default (the
/// server still checks them), the descriptions of fields below the
/// first level, the names of `$defs`, and the shape of anything the
/// subset cannot say. This bridge read the list through a map that
/// sorts its keys, so each interface's fields are in the alphabet's
/// order; one that keeps the list's order gives the model the server's.
fn manifest_of_mcp(name: &str, listed: &Value) -> Value {
    let mut operations = Vec::new();
    let mut interfaces = serde_json::Map::new();
    for tool in listed["tools"].as_array().into_iter().flatten() {
        let listed_name = tool["name"].as_str().unwrap_or_default();
        let operation: String = listed_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let input = &tool["inputSchema"];
        let args = bridged_type(input, &input["$defs"], true, 64);
        let schema = if args.get("properties").is_some() {
            // `list_windows` gives `ListWindowsArgs`.
            let interface: String = operation
                .split('_')
                .filter(|w| !w.is_empty())
                .map(|w| format!("{}{}", w[..1].to_uppercase(), &w[1..]))
                .chain(["Args".to_string()])
                .collect();
            interfaces.insert(interface.clone(), args);
            json!({"$ref": format!("#/interfaces/{interface}")})
        } else {
            args
        };
        let reads = tool["annotations"]["readOnlyHint"] == json!(true);
        operations.push(json!({
            "name": operation,
            "description": bridged_text(tool["description"].as_str())
                .unwrap_or_else(|| format!("The tool `{listed_name}`.")),
            "access": if reads { "read" } else { "write" },
            "params": [{"name": "args", "schema": schema}],
            "result": {},
        }));
    }
    json!({
        "clax_surface": MANIFEST_VERSION,
        "name": name,
        "operations": operations,
        "interfaces": interfaces,
    })
}

#[test]
fn a_bridge_makes_a_manifest_of_an_mcp_tool_list() {
    let listed: Value = serde_json::from_str(MCP_TOOLS).unwrap();
    let written = manifest_of_mcp("tracker", &listed).to_string();
    let d = manifest(&written).unwrap_or_else(|e| panic!("{e}\n{written}"));
    assert_eq!(
        declaration(&[&d], &|_| true).unwrap(),
        "declare namespace tracker {\n  interface CreateIssueArgs {\n    assignee?: string | null;\n    labels?: string[];\n    milestone?: number;\n    options?: { draft?: boolean };\n    /** Repository owner */\n    owner: string;\n    title: string;\n  }\n  interface ListWindowsArgs {\n  }\n  interface NotifyArgs {\n    kind?: \"info\" | \"warn\";\n    text: string;\n  }\n  interface SearchArgs {\n    filter?: { site?: string | null } | null;\n    limit?: number;\n    query: string;\n  }\n}\n\ndeclare const clax: {\n  tracker: {\n    /** Create a new issue in a GitHub repository. (writes) */\n    create_issue(args: tracker.CreateIssueArgs): Promise<unknown>;\n    /** Search the web. Args: query: what to look for (writes) */\n    search(args: tracker.SearchArgs): Promise<unknown>;\n    /** List open windows */\n    list_windows(args: tracker.ListWindowsArgs): Promise<unknown>;\n    /** The tool `http.request`. (writes) */\n    http_request(args: Record<string, unknown>): Promise<unknown>;\n    /** Notify a user. Use `@here` to reach everyone. (writes) */\n    notify(args: tracker.NotifyArgs): Promise<unknown>;\n  };\n};\n"
    );
    round_trips("the MCP tool list", &d, &host, &manifest);
    let create = d.operation("tracker.create_issue").unwrap();
    assert!(
        create
            .check_args(&[
                json!({"owner": "o", "title": "t", "assignee": null, "options": {"draft": true}})
            ])
            .is_ok()
    );
    assert!(
        create
            .check_args(&[json!({"owner": "o", "title": "t", "options": {"public": true}})])
            .is_err()
    );
    let request = d.operation("tracker.http_request").unwrap();
    assert!(
        request
            .check_args(&[
                json!({"url": "https://example.com", "content-type": "text/plain", "retries": 3})
            ])
            .is_ok()
    );
    // As the server lists them, with its schemas as they are, none of
    // the five tools is an operation: each is refused for its own
    // reason, all in one reading.
    let as_listed: Vec<Value> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            let mut operation = json!({
                "name": tool["name"],
                "access": "write",
                "params": [{"name": "args", "schema": tool["inputSchema"]}],
                "result": tool.get("outputSchema").cloned().unwrap_or_else(|| json!({})),
            });
            if let Some(description) = tool.get("description") {
                operation["description"] = description.clone();
            }
            operation
        })
        .collect();
    let as_listed =
        json!({"clax_surface": 1, "name": "tracker", "operations": as_listed}).to_string();
    let errors = manifest_all(&as_listed, &Limits::default()).unwrap_err();
    let said: Vec<(&str, &str)> = errors
        .iter()
        .map(|e| (e.pointer().unwrap(), e.message.as_str()))
        .collect();
    let expected = [
        (
            "/operations/0/params/0/schema/properties/assignee/type",
            "`type` names one type: write a union as `anyOf`",
        ),
        (
            "/operations/1/params/0/schema",
            "an object type says `\"additionalProperties\": false`",
        ),
        (
            "/operations/2/params/0/schema",
            "an object type says `\"additionalProperties\": false`",
        ),
        (
            "/operations/3",
            "operation `http.request` has no `description`",
        ),
        (
            "/operations/4/params/0/schema",
            "an object type says `\"additionalProperties\": false`",
        ),
    ];
    assert_eq!(said.len(), expected.len(), "{errors}");
    for ((pointer, message), (at, needle)) in said.iter().zip(expected) {
        assert_eq!(*pointer, at, "{errors}");
        assert!(message.starts_with(needle), "{message}");
    }
}
