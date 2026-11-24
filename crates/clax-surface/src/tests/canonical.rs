//! A definition's canonical bytes: the same for every way a manifest
//! can spell it, another for every definition that says another thing,
//! and fixed, so that a change to them fails here.

use super::manifest::{Family, Rng, manifest};
use super::{host, own, review, shell};
use crate::manifest::{Node, tree};
use crate::{Definition, Limits, canonical_manifest, print_manifest};
use std::collections::BTreeSet;
use std::fmt::Write as _;

// ------------------------------------------------------------ other spellings

fn object(pairs: &[(&str, Node)]) -> Node {
    Node::Obj(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), copy(v)))
            .collect(),
    )
}

fn copy(node: &Node) -> Node {
    match node {
        Node::Null => Node::Null,
        Node::Bool(b) => Node::Bool(*b),
        Node::Num(n) => Node::Num(*n),
        Node::Str(s) => Node::Str(s.clone()),
        Node::Arr(items) => Node::Arr(items.iter().map(copy).collect()),
        Node::Obj(pairs) => Node::Obj(pairs.iter().map(|(k, v)| (k.clone(), copy(v))).collect()),
    }
}

fn text(s: &str) -> Node {
    Node::Str(s.to_string())
}

fn shuffle<T>(r: &mut Rng, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, r.below(i + 1));
    }
}

/// The members of an object the notation names, by name.
struct Members(Vec<(String, Node)>);

impl Members {
    fn of(node: &Node) -> Members {
        match node {
            Node::Obj(pairs) => Members(pairs.iter().map(|(k, v)| (k.clone(), copy(v))).collect()),
            _ => panic!("an object"),
        }
    }
    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
    fn get(&self, key: &str) -> Option<&Node> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    fn take(&mut self, key: &str) -> Option<Node> {
        let at = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(at).1)
    }
    fn set(&mut self, key: &str, value: Node) {
        self.take(key);
        self.0.push((key.to_string(), value));
    }
    /// The object again, its members in another order.
    fn shuffled(mut self, r: &mut Rng) -> Node {
        shuffle(r, &mut self.0);
        Node::Obj(self.0)
    }
}

/// The type of every literal of `values`, when they have one.
fn literal_type(values: &[Node]) -> Option<&'static str> {
    if values.iter().all(|v| matches!(v, Node::Str(_))) {
        Some("string")
    } else if values.iter().all(|v| matches!(v, Node::Bool(_))) {
        Some("boolean")
    } else {
        None
    }
}

/// Another spelling of a type: what the reader takes and the writer
/// does not write.
fn respelled_type(r: &mut Rng, schema: &Node) -> Node {
    let mut m = Members::of(schema);
    let description = m.take("description");
    if let Some(Node::Arr(alternatives)) = m.take("anyOf") {
        let mut alternatives: Vec<Node> =
            alternatives.iter().map(|a| respelled_type(r, a)).collect();
        // The last of them as a union of their own.
        if alternatives.len() > 2 && r.one_in(2) {
            let k = 1 + r.below(alternatives.len() - 2);
            let tail = alternatives.split_off(k);
            alternatives.push(object(&[("anyOf", Node::Arr(tail))]));
        }
        m.set("anyOf", Node::Arr(alternatives));
    } else if let Some(Node::Arr(values)) = m.take("enum") {
        match r.below(3) {
            0 => {
                let consts = values.iter().map(|v| object(&[("const", copy(v))]));
                m.set("anyOf", Node::Arr(consts.collect()));
            }
            1 => {
                if let Some(t) = literal_type(&values) {
                    m.set("type", text(t));
                }
                m.set("enum", Node::Arr(values));
            }
            _ => m.set("enum", Node::Arr(values)),
        }
    } else if let Some(value) = m.take("const") {
        match r.below(3) {
            0 => m.set("enum", Node::Arr(vec![value])),
            1 => {
                let t = literal_type(std::slice::from_ref(&value));
                m.set("type", text(t.unwrap_or("string")));
                m.set("const", value);
            }
            _ => m.set("const", value),
        }
    } else if matches!(m.get("type"), Some(Node::Str(t)) if t == "integer") && r.one_in(2) {
        m.take("type");
        m.set("$ref", text("clax:Int"));
    } else if let Some(items) = m.take("items") {
        m.set("items", respelled_type(r, &items));
    } else if matches!(m.get("additionalProperties"), Some(Node::Obj(_))) {
        let value = m.take("additionalProperties").unwrap_or(Node::Null);
        m.set("additionalProperties", respelled_type(r, &value));
    } else if m.has("additionalProperties") {
        respelled_fields(r, &mut m);
    }
    if let Some(d) = description {
        m.set("description", d);
    }
    m.shuffled(r)
}

/// An object type's or an interface's `properties` and `required`: the
/// fields in their order, each type respelled; `required` in another
/// order, or there though empty; `properties` left out when empty.
fn respelled_fields(r: &mut Rng, m: &mut Members) {
    let fields = match m.take("properties") {
        Some(Node::Obj(fields)) => fields,
        _ => Vec::new(),
    };
    if !fields.is_empty() || r.one_in(2) {
        let fields = fields
            .iter()
            .map(|(k, v)| (k.clone(), respelled_type(r, v)));
        m.set("properties", Node::Obj(fields.collect()));
    }
    match m.take("required") {
        Some(Node::Arr(mut names)) => {
            shuffle(r, &mut names);
            m.set("required", Node::Arr(names));
        }
        _ if r.one_in(3) => m.set("required", Node::Arr(Vec::new())),
        _ => {}
    }
}

/// Another spelling of a whole manifest, as text: members of the
/// notation in another order, what may be left out written out and the
/// reverse, a namespace's entry where none is needed, each type
/// respelled, and the text laid out another way. The order of
/// operations, interfaces, fields, alternatives and kinds is kept: it
/// is the definition's.
fn respelled(r: &mut Rng, canonical: &str) -> String {
    let document = tree(canonical, usize::MAX, usize::MAX).unwrap();
    let mut m = Members::of(&document);
    let Some(Node::Arr(entries)) = m.take("operations") else {
        panic!("operations");
    };
    let explicit: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match Members::of(e).get("namespace") {
            Some(Node::Str(n)) => Some(n.clone()),
            _ => None,
        })
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut operations = Vec::new();
    for entry in &entries {
        let mut e = Members::of(entry);
        let (name, own) = match (e.get("namespace"), e.get("name")) {
            (Some(Node::Str(n)), _) => (n.clone(), true),
            (_, Some(Node::Str(n))) => (n.clone(), false),
            _ => panic!("an entry"),
        };
        // An entry for each namespace on the way that needs none.
        let segments: Vec<&str> = name.split('.').collect();
        for k in 1..segments.len() {
            let namespace = segments[..k].join(".");
            if seen.insert(namespace.clone()) && !explicit.contains(&namespace) && r.one_in(3) {
                operations.push(object(&[("namespace", text(&namespace))]));
            }
        }
        if own {
            seen.insert(name);
            operations.push(e.shuffled(r));
            continue;
        }
        for flag in ["waits", "once"] {
            if !e.has(flag) && r.one_in(3) {
                e.set(flag, Node::Bool(false));
            }
        }
        if !e.has("throws") && r.one_in(3) {
            e.set("throws", Node::Arr(Vec::new()));
        }
        let Some(Node::Arr(params)) = e.take("params") else {
            panic!("params");
        };
        if !params.is_empty() || r.one_in(2) {
            let params = params.iter().map(|p| {
                let mut p = Members::of(p);
                let schema = p.take("schema").unwrap_or(Node::Null);
                p.set("schema", respelled_type(r, &schema));
                if !p.has("optional") && r.one_in(3) {
                    p.set("optional", Node::Bool(false));
                }
                p.shuffled(r)
            });
            e.set("params", Node::Arr(params.collect()));
        }
        let result = e.take("result").unwrap_or(Node::Null);
        e.set("result", respelled_type(r, &result));
        operations.push(e.shuffled(r));
    }
    if !operations.is_empty() || r.one_in(2) {
        m.set("operations", Node::Arr(operations));
    }
    match m.take("interfaces") {
        Some(Node::Obj(interfaces)) => {
            let interfaces = interfaces.iter().map(|(name, i)| {
                let mut i = Members::of(i);
                respelled_fields(r, &mut i);
                if !i.has("extends") && r.one_in(3) {
                    i.set("extends", Node::Arr(Vec::new()));
                }
                (name.clone(), i.shuffled(r))
            });
            m.set("interfaces", Node::Obj(interfaces.collect()));
        }
        _ if r.one_in(2) => m.set("interfaces", Node::Obj(Vec::new())),
        _ => {}
    }
    match m.take("kinds") {
        Some(Node::Arr(kinds)) => {
            let kinds = kinds.iter().map(|k| {
                let mut k = Members::of(k);
                for part in ["request", "message", "result"] {
                    if let Some(t) = k.take(part) {
                        k.set(part, respelled_type(r, &t));
                    }
                }
                k.shuffled(r)
            });
            m.set("kinds", Node::Arr(kinds.collect()));
        }
        _ if r.one_in(2) => m.set("kinds", Node::Arr(Vec::new())),
        _ => {}
    }
    let layout = match r.below(4) {
        0 => Layout {
            indent: None,
            ascii: true,
            around: ("", ""),
        },
        1 => Layout {
            indent: Some(4),
            ascii: false,
            around: ("", "\n"),
        },
        2 => Layout {
            indent: Some(1),
            ascii: true,
            around: ("\u{feff}  \n\t", "\r\n \n"),
        },
        _ => Layout {
            indent: None,
            ascii: false,
            around: ("", ""),
        },
    };
    let mut out = String::from(layout.around.0);
    layout.write(&mut out, &m.shuffled(r), 0);
    out.push_str(layout.around.1);
    out
}

/// A way to write JSON that is not the writer's.
struct Layout {
    /// Spaces a level, each member on a line; all on one line when none.
    indent: Option<usize>,
    /// Whether everything but ASCII is escaped, and `/` too.
    ascii: bool,
    /// What is before and after the document.
    around: (&'static str, &'static str),
}

impl Layout {
    fn string(&self, out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '/' if self.ascii => out.push_str("\\/"),
                c if (c as u32) < 0x20 || (self.ascii && !c.is_ascii()) => {
                    let mut units = [0u16; 2];
                    for unit in c.encode_utf16(&mut units) {
                        let _ = write!(out, "\\u{unit:04X}");
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    fn line(&self, out: &mut String, depth: usize) {
        if let Some(n) = self.indent {
            out.push('\n');
            out.push_str(&" ".repeat(n * depth));
        }
    }

    fn write(&self, out: &mut String, node: &Node, depth: usize) {
        match node {
            Node::Null => out.push_str("null"),
            Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Node::Num(n) => {
                let _ = write!(out, "{}", n.unwrap_or(0));
            }
            Node::Str(s) => self.string(out, s),
            Node::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.line(out, depth + 1);
                    self.write(out, item, depth + 1);
                }
                if !items.is_empty() {
                    self.line(out, depth);
                }
                out.push(']');
            }
            Node::Obj(pairs) => {
                out.push('{');
                for (i, (key, value)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.line(out, depth + 1);
                    self.string(out, key);
                    out.push_str(if self.indent.is_some() { " : " } else { ":" });
                    self.write(out, value, depth + 1);
                }
                if !pairs.is_empty() {
                    self.line(out, depth);
                }
                out.push('}');
            }
        }
    }
}

/// Every spelling of a manifest the reader takes gives the same
/// canonical bytes: nothing of how a manifest is written is part of a
/// definition's name but what the definition says.
#[test]
fn canonical_bytes_do_not_depend_on_how_a_manifest_is_spelled() {
    let mut r = Rng(7);
    let mut seen_forms = [false; 6];
    let definitions = (0..150).map(Family::definition).chain([shell(), review()]);
    let mut respellings = 0;
    for (n, def) in definitions.enumerate() {
        let canonical = canonical_manifest(&def);
        assert_eq!(manifest(&canonical).unwrap(), def, "{n}");
        assert_eq!(manifest(&print_manifest(&def)).unwrap(), def, "{n}");
        for _ in 0..4 {
            let other = respelled(&mut r, &canonical);
            let read = manifest(&other).unwrap_or_else(|e| panic!("{n}: {e}\n{other}"));
            assert_eq!(read, def, "{n}:\n{other}");
            assert_eq!(canonical_manifest(&read), canonical, "{n}:\n{other}");
            respellings += usize::from(other != canonical);
            for (k, form) in [
                "\"$ref\":\"clax:Int\"",
                "\"required\":[]",
                "\"extends\":[]",
                "\"waits\":false",
                "\"throws\":[]",
                "\"optional\":false",
            ]
            .iter()
            .enumerate()
            {
                seen_forms[k] |= other.contains(form);
            }
        }
    }
    assert!(respellings > 590, "{respellings}");
    assert_eq!(seen_forms, [true; 6]);
    // Clax's own, which only it may write.
    let def = own();
    let canonical = canonical_manifest(&def);
    for _ in 0..8 {
        let other = respelled(&mut r, &canonical);
        let read = crate::parse_manifest(&other, crate::Author::Clax, &Limits::default()).unwrap();
        assert_eq!(canonical_manifest(&read), canonical);
    }
}

// --------------------------------------------------------- what they hold

const BASE: &str = r#"/** Rules. */
declare const demo: {
  /** Gets. @read @throws gone @throws busy */
  get(key: string, wide?: boolean): Promise<Item | null>;
  /** Ns rules. */
  jobs: {
    /** Starts. @effect */
    start(name: "a" | "b"): Promise<null>;
  };
};
/** An item. */
interface Item extends Named, Dated {
  /** Its size. */
  size: Int;
  tags?: string[];
}
interface Named { name: string }
interface Dated { at: string }
interface Kinds {
  /** Do it. @allow demo.get demo.jobs.* */
  go(request: { ask: Instruction }): { done: boolean };
}
"#;

/// The canonical bytes of [`BASE`] with `old` written as `new`.
#[track_caller]
fn changed(old: &str, new: &str) -> String {
    assert_eq!(BASE.matches(old).count(), 1, "{old}");
    let def = host(&BASE.replace(old, new)).unwrap_or_else(|e| panic!("{old} as {new}: {e}"));
    canonical_manifest(&def)
}

/// Every part of a definition is in its canonical bytes: its words for
/// the model and their order as much as its types.
#[test]
fn canonical_bytes_differ_when_any_part_of_a_definition_does() {
    let all = [
        changed("Rules.", "Rules."),
        // Documentation.
        changed("Rules.", "Other rules."),
        changed("/** Rules. */\n", ""),
        changed("Gets.", "Gets it."),
        changed("Ns rules.", "Ns rule."),
        changed("  /** Ns rules. */\n", ""),
        changed("An item.", "One item."),
        changed("/** An item. */\n", ""),
        changed("Its size.", "Its length."),
        changed("  /** Its size. */\n", ""),
        changed("Do it.", "Do."),
        // Names.
        changed("const demo", "const demi"),
        changed("get(", "got("),
        changed("jobs:", "work:"),
        changed("key:", "k:"),
        changed("size:", "length:"),
        changed("go(", "run("),
        // What an operation is.
        changed("@read", "@write"),
        changed("@read", "@read @waits"),
        changed("@effect", "@effect @once"),
        changed("@throws gone @throws busy", "@throws busy @throws gone"),
        changed(" @throws busy", ""),
        changed("@throws gone", "@throws went"),
        changed("wide?:", "wide:"),
        changed("key: string, wide?: boolean", "wide: boolean, key?: string"),
        changed(", wide?: boolean", ""),
        changed("key: string", "key: number"),
        changed("Promise<Item | null>", "Promise<null | Item>"),
        changed("Promise<Item | null>", "Promise<Item>"),
        changed("\"a\" | \"b\"", "\"b\" | \"a\""),
        changed("\"a\" | \"b\"", "\"a\" | \"c\""),
        changed("\"a\" | \"b\"", "\"a\" | true"),
        changed("\"a\" | \"b\"", "\"a\""),
        changed("\"a\" | \"b\"", "string"),
        // Where things stand.
        changed(
            "  /** Starts. @effect */\n    start(name: \"a\" | \"b\"): Promise<null>;\n",
            "",
        ),
        changed(
            "interface Named { name: string }\ninterface Dated { at: string }",
            "interface Dated { at: string }\ninterface Named { name: string }",
        ),
        // Interfaces.
        changed("extends Named, Dated", "extends Dated, Named"),
        changed("extends Named, Dated", "extends Named"),
        changed("size: Int", "size: number"),
        changed("size: Int", "size?: Int"),
        changed("tags?: string[]", "tags?: string[][]"),
        changed("tags?: string[]", "tags?: Record<string, string>"),
        changed("tags?: string[]", "tags?: unknown"),
        changed("tags?: string[];\n", ""),
        changed("{ name: string }", "{ name: string; more?: Content }"),
        changed("{ at: string }", "{ at: Image }"),
        // Kinds.
        changed(" demo.get demo.jobs.*", ""),
        changed("demo.get demo.jobs.*", "demo.jobs.* demo.get"),
        changed("demo.get demo.jobs.*", "demo.get *"),
        changed("{ ask: Instruction }", "{ ask: Instruction; n?: Int }"),
        changed(
            "{ ask: Instruction })",
            "{ ask: Instruction }, message?: string)",
        ),
        changed("{ done: boolean }", "{ done: true }"),
    ];
    assert_eq!(all[0], changed("demo.get", "demo.get"));
    let distinct: BTreeSet<&String> = all.iter().collect();
    assert_eq!(distinct.len(), all.len());
    // A field's place among the fields, and an operation's among the
    // operations, are parts like any other.
    let swapped = host(
        "declare const demo: {\n/** B. @read */\nb(): Promise<null>;\n/** A. @read */\na(): Promise<{ y: string; x: string }>;\n};",
    )
    .unwrap();
    let sorted = host(
        "declare const demo: {\n/** A. @read */\na(): Promise<{ x: string; y: string }>;\n/** B. @read */\nb(): Promise<null>;\n};",
    )
    .unwrap();
    assert_ne!(canonical_manifest(&swapped), canonical_manifest(&sorted));
}

// ------------------------------------------------------------ the form itself

/// The bytes themselves. They name definitions that are stored: a
/// change here changes every stored name, and is a new version of the
/// notation, not an edit of this test.
#[test]
fn the_canonical_form_is_fixed() {
    let canonical = |src: &str| canonical_manifest(&host(src).unwrap());
    assert_eq!(
        canonical("declare const demo: {};"),
        r#"{"clax_surface":1,"name":"demo","operations":[]}"#
    );
    assert_eq!(
        canonical_manifest(&host(BASE).unwrap()),
        concat!(
            r#"{"clax_surface":1,"name":"demo","rules":"Rules.","operations":["#,
            r#"{"name":"get","description":"Gets.","access":"read","throws":["gone","busy"],"#,
            r#""params":[{"name":"key","schema":{"type":"string"}},{"name":"wide","optional":true,"schema":{"type":"boolean"}}],"#,
            r##""result":{"anyOf":[{"$ref":"#/interfaces/Item"},{"type":"null"}]}},"##,
            r#"{"namespace":"jobs","rules":"Ns rules."},"#,
            r#"{"name":"jobs.start","description":"Starts.","access":"effect","#,
            r#""params":[{"name":"name","schema":{"enum":["a","b"]}}],"result":{"type":"null"}}],"#,
            r#""interfaces":{"Item":{"type":"object","description":"An item.","extends":["Named","Dated"],"#,
            r#""properties":{"size":{"type":"integer","description":"Its size."},"tags":{"type":"array","items":{"type":"string"}}},"#,
            r#""required":["size"],"additionalProperties":false},"#,
            r#""Named":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false},"#,
            r#""Dated":{"type":"object","properties":{"at":{"type":"string"}},"required":["at"],"additionalProperties":false}},"#,
            r#""kinds":[{"name":"go","description":"Do it.","allow":["demo.get","demo.jobs.*"],"#,
            r#""request":{"type":"object","properties":{"ask":{"$ref":"clax:Instruction"}},"required":["ask"],"additionalProperties":false},"#,
            r#""result":{"type":"object","properties":{"done":{"type":"boolean"}},"required":["done"],"additionalProperties":false}}]}"#
        )
    );
    // Every other form of a type, an operation and a kind: one with no
    // `@allow`, which only Clax's own surface may have.
    let clax_canonical = |src: &str| {
        canonical_manifest(
            &crate::parse(src, crate::Author::Clax, &crate::Limits::default()).unwrap(),
        )
    };
    assert_eq!(
        clax_canonical(
            "declare const demo: {\n  empty: {};\n  deep: {\n    er: {\n      /** D. @write @waits @once */\n      x(a: unknown, b?: Record<string, number | true>, c?: {}, d?: Content | Image): Promise<Instruction | \"no\">;\n    };\n  };\n};\ninterface Kinds {\n  /** K. */\n  k(request: null, message?: false): {};\n}"
        ),
        concat!(
            r#"{"clax_surface":1,"name":"demo","operations":[{"namespace":"empty"},"#,
            r#"{"name":"deep.er.x","description":"D.","access":"write","waits":true,"once":true,"#,
            r#""params":[{"name":"a","schema":{}},"#,
            r#"{"name":"b","optional":true,"schema":{"type":"object","additionalProperties":{"anyOf":[{"type":"number"},{"const":true}]}}},"#,
            r#"{"name":"c","optional":true,"schema":{"type":"object","properties":{},"additionalProperties":false}},"#,
            r#"{"name":"d","optional":true,"schema":{"anyOf":[{"$ref":"clax:Content"},{"$ref":"clax:Image"}]}}],"#,
            r#""result":{"anyOf":[{"$ref":"clax:Instruction"},{"const":"no"}]}}],"#,
            r#""kinds":[{"name":"k","description":"K.","request":{"type":"null"},"message":{"const":false},"result":{"type":"object","properties":{},"additionalProperties":false}}]}"#
        )
    );
    // A string's escapes: `"`, `\`, a line feed and a tab, and nothing
    // else; what is not ASCII is itself, and so is `/`.
    assert_eq!(
        canonical(
            "declare const demo: {\n  /**\n   * A \"quote\", a back\\slash, a/slash,\ta tab,\n   * é, — and 日本: {\"k\": [1]}.\n   * @read\n   */\n  x(): Promise<\"a\\\"b\" | \"c\\\\d\" | \"é/\">;\n};"
        ),
        "{\"clax_surface\":1,\"name\":\"demo\",\"operations\":[{\"name\":\"x\",\"description\":\"A \\\"quote\\\", a back\\\\slash, a/slash,\\ta tab,\\né, — and 日本: {\\\"k\\\": [1]}.\",\"access\":\"read\",\"params\":[],\"result\":{\"enum\":[\"a\\\"b\",\"c\\\\d\",\"é/\"]}}]}"
    );
    // It is the laid-out manifest without its layout, and is read back.
    for def in [own(), shell(), review()] {
        let (canonical, laid_out) = (canonical_manifest(&def), print_manifest(&def));
        assert!(!canonical.ends_with('\n') && !canonical.contains('\n'));
        let same = |text: &str| {
            let mut out = String::new();
            Layout {
                indent: None,
                ascii: false,
                around: ("", ""),
            }
            .write(&mut out, &tree(text, usize::MAX, usize::MAX).unwrap(), 0);
            out
        };
        assert_eq!(same(&laid_out), canonical);
        assert_eq!(same(&canonical), canonical);
        assert!(canonical.len() < laid_out.len());
    }
    let _: fn(&Definition) -> String = canonical_manifest;
    // What the crate's documentation says a manifest's tree costs.
    assert_eq!(std::mem::size_of::<Node>(), 32);
}

/// FNV-1a over `bytes`, 64 bits: enough to see that bytes changed, and
/// no dependency.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The canonical bytes of the surfaces in the tree, by their length,
/// their ends and a checksum. They change when one of those files does,
/// which is then said here; they do not change for any other reason:
/// a change to the form shows here as a change to all three at once,
/// and is not made by editing these numbers.
#[test]
fn the_canonical_bytes_of_the_surfaces_in_the_tree_are_these() {
    for (what, def, length, checksum, ends) in [
        (
            "own.d.ts",
            own(),
            11_459,
            348_368_148_432_450_972u64,
            "aces/BriefRequest\"},\"result\":{\"$ref\":\"#/interfaces/Done\"}}]}",
        ),
        (
            "shell.d.ts",
            shell(),
            5_728,
            15_583_094_276_542_572_955,
            "es/Triage\"},\"result\":{\"$ref\":\"#/interfaces/TriageResult\"}}]}",
        ),
        (
            "review.d.ts",
            review(),
            14_893,
            5_632_222_845_735_531_524,
            "faces/SubjectAsk\"},\"result\":{\"$ref\":\"#/interfaces/Quest\"}}]}",
        ),
    ] {
        let canonical = canonical_manifest(&def);
        let tail: String = canonical
            .chars()
            .rev()
            .take(60)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(
            (canonical.len(), fnv(canonical.as_bytes()), tail.as_str()),
            (length, checksum, ends),
            "{what}"
        );
        assert!(
            canonical.starts_with("{\"clax_surface\":1,\"name\":\""),
            "{what}"
        );
    }
}
