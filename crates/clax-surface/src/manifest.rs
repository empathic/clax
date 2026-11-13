//! The manifest notation: a definition as one JSON document, for a
//! program to build and send. The crate's documentation gives its shape.
//!
//! The reader ([`parse_manifest`]) reads the text into a tree that keeps
//! every object's members in the order written, refuses what the
//! notation cannot say, and builds the parts of a definition; the rules
//! a definition keeps are [`crate::check::build`]'s, as they are for the
//! `.d.ts` reader. It remembers where each declaration is written, so
//! that every refusal, the checks' too, names a value of the document.
//! It reads a manifest to its end and says the first mistake of each
//! entry it cannot read. It recurses as deep as the text nests its
//! arrays and objects, which is no deeper than
//! [`Limits::manifest_nesting`] (`serde_json` reads the text with no
//! bound of its own; the bound is this reader's), and builds nothing
//! that nests deeper than the limit lets a definition nest, so it
//! cannot be made to exhaust the stack.
//!
//! The writer is the one way this crate writes a definition as a
//! manifest: laid out for a reader ([`print_manifest`]), and without
//! the layout, as the bytes that name a definition
//! ([`canonical_manifest`]).

use crate::allow::Pattern;
use crate::check::{self, Author, Decl, Notation, Site, shown};
use crate::error::{Error, Errors, Place};
use crate::limits::Limits;
use crate::model::{Access, Definition, Field, Interface, Kind, Member, Namespace, Op, Param, Ty};
use crate::value::IMAGE_MEDIA_TYPES;
use serde::de::{DeserializeSeed, Deserializer, Error as _, MapAccess, SeqAccess, Visitor};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

/// The version of the manifest notation this crate reads and writes: the
/// number a manifest gives as `clax_surface`.
pub const MANIFEST_VERSION: u64 = 1;

// ------------------------------------------------------------------ tree

/// A JSON value as the document has it: an object's members in the
/// order written.
pub(crate) enum Node {
    Null,
    Bool(bool),
    /// A number: itself, when it is a whole number from 0 up.
    Num(Option<u64>),
    Str(String),
    Arr(Vec<Node>),
    Obj(Vec<(String, Node)>),
}

/// Reads one JSON value that has `depth` arrays and objects around it,
/// in a text that may nest them `max` deep and hold `values` values, of
/// which `left` are still to come.
#[derive(Clone, Copy)]
struct Tree<'a> {
    depth: usize,
    max: usize,
    values: usize,
    left: &'a Cell<usize>,
}

impl Tree<'_> {
    /// What reads a value an array or object here holds.
    fn inside<E: serde::de::Error>(self) -> Result<Self, E> {
        if self.depth == self.max {
            return Err(E::custom(format_args!(
                "arrays and objects nested more than {} deep, the limit for a manifest",
                self.max
            )));
        }
        Ok(Tree {
            depth: self.depth + 1,
            ..self
        })
    }

    /// Counts the value being read: a number, a string, `true`,
    /// `false`, `null`, an array or an object.
    fn counted<E: serde::de::Error>(self, node: Node) -> Result<Node, E> {
        match self.left.get().checked_sub(1) {
            Some(left) => {
                self.left.set(left);
                Ok(node)
            }
            None => Err(E::custom(format_args!(
                "more than {} values, the limit for a manifest",
                self.values
            ))),
        }
    }
}

impl<'de> DeserializeSeed<'de> for Tree<'_> {
    type Value = Node;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Tree<'_> {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Node, E> {
        self.counted(Node::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Node, E> {
        self.counted(Node::Bool(v))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Node, E> {
        self.counted(Node::Num(Some(v)))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Node, E> {
        self.counted(Node::Num(u64::try_from(v).ok()))
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Node, E> {
        self.counted(Node::Num(None))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Node, E> {
        self.counted(Node::Str(v.to_string()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Node, E> {
        self.counted(Node::Str(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let inside = self.inside()?;
        self.counted(Node::Null)?;
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(inside)? {
            items.push(item);
        }
        Ok(Node::Arr(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let inside = self.inside()?;
        self.counted(Node::Null)?;
        let mut pairs = Vec::new();
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(A::Error::custom(format_args!(
                    "the key `{}` is given twice in one object",
                    shown(&key)
                )));
            }
            pairs.push((key, map.next_value_seed(inside)?));
        }
        Ok(Node::Obj(pairs))
    }
}

/// Why the text is not the JSON a manifest is, as `serde_json` or
/// [`Tree`] said it, with its line.
fn not_json(e: &serde_json::Error) -> Error {
    let text = e.to_string();
    // `serde_json` ends what it says with where: ` at line 3 column 7`.
    let said = text
        .rsplit_once(" at line ")
        .map_or(text.as_str(), |(said, _)| said);
    let message = if e.is_data() {
        format!("{said} (column {})", e.column())
    } else {
        format!("the manifest is not JSON: {said} (column {})", e.column())
    };
    Error {
        at: Place::Line(e.line().max(1)),
        message,
    }
}

/// The text as a tree, when it is JSON that nests its arrays and
/// objects no more than `nesting` deep and holds no more than `values`
/// values. `serde_json` reads it, with no limit of its own on how deep:
/// the limits are [`Tree`]'s.
pub(crate) fn tree(src: &str, nesting: usize, values: usize) -> Result<Node, Error> {
    let mut de = serde_json::Deserializer::from_str(src);
    de.disable_recursion_limit();
    let left = Cell::new(values);
    let read = Tree {
        depth: 0,
        max: nesting,
        values,
        left: &left,
    };
    let node = read.deserialize(&mut de).map_err(|e| not_json(&e))?;
    de.end().map_err(|e| not_json(&e))?;
    Ok(node)
}

// ---------------------------------------------------------------- places

/// One step from a value of the document to a value inside it.
enum Seg {
    /// A key the notation names.
    Word(&'static str),
    /// A key the document chose: a name.
    Key(String),
    Index(usize),
}

/// The way to each value the reader went into: its parent, and the step
/// from there. A value is known by its position here.
struct Trail(Vec<(usize, Seg)>);

impl Trail {
    /// The document itself.
    const ROOT: usize = usize::MAX;

    fn step(&mut self, parent: usize, seg: Seg) -> usize {
        self.0.push((parent, seg));
        self.0.len() - 1
    }
    fn word(&mut self, parent: usize, key: &'static str) -> usize {
        self.step(parent, Seg::Word(key))
    }
    fn key(&mut self, parent: usize, key: &str) -> usize {
        self.step(parent, Seg::Key(key.to_string()))
    }
    fn index(&mut self, parent: usize, i: usize) -> usize {
        self.step(parent, Seg::Index(i))
    }

    /// The JSON Pointer of the value `at`, as a message shows it.
    fn pointer(&self, mut at: usize) -> String {
        let mut segs = Vec::new();
        while let Some((parent, seg)) = self.0.get(at) {
            segs.push(seg);
            at = *parent;
        }
        let mut out = String::new();
        for seg in segs.into_iter().rev() {
            out.push('/');
            match seg {
                Seg::Word(w) => out.push_str(w),
                Seg::Key(k) => out.push_str(&shown(k).replace('~', "~0").replace('/', "~1")),
                Seg::Index(i) => {
                    let _ = write!(out, "{i}");
                }
            }
        }
        out
    }
}

/// Where a type is written: the value a problem with it is named at, and
/// each field of the object types written in it, in the order written.
#[derive(Default)]
struct TyPlace {
    at: usize,
    inner: Vec<usize>,
}

struct OpPlace {
    at: usize,
    /// Its `throws`, or the operation when it has none.
    throws: usize,
    codes: Vec<usize>,
    params: Vec<TyPlace>,
    result: TyPlace,
}

struct KindPlace {
    at: usize,
    request: TyPlace,
    message: Option<TyPlace>,
    result: TyPlace,
}

struct InterfacePlace {
    at: usize,
    /// Its `extends`, or the interface when it has none, and each name
    /// there.
    extends_at: usize,
    extends: Vec<usize>,
    fields: Vec<TyPlace>,
}

/// The value of each declaration, by its [`Site`].
struct Places {
    surface: usize,
    rules: usize,
    /// `operations`, the surface's members.
    members: usize,
    /// Each namespace: the entry or name that opened it, and its rules
    /// (the same, when it has none).
    namespaces: Vec<(usize, usize)>,
    ops: Vec<OpPlace>,
    /// `interfaces` itself.
    interfaces_at: usize,
    interfaces: Vec<InterfacePlace>,
    kinds_at: usize,
    kinds: Vec<KindPlace>,
}

impl Places {
    fn of(&self, site: Site) -> usize {
        let ty = |t: Option<&TyPlace>| {
            t.map(|t| {
                site.inner
                    .and_then(|i| t.inner.get(i).copied())
                    .unwrap_or(t.at)
            })
        };
        let found = match site.decl {
            Decl::Surface => Some(self.surface),
            Decl::Rules => Some(self.rules),
            Decl::Members => Some(self.members),
            Decl::Namespace(n) => self.namespaces.get(n).map(|ns| ns.0),
            Decl::NamespaceRules(n) => self.namespaces.get(n).map(|ns| ns.1),
            Decl::Op(n) => self.ops.get(n).map(|o| o.at),
            Decl::Code(n, k) => self
                .ops
                .get(n)
                .map(|o| o.codes.get(k).copied().unwrap_or(o.throws)),
            Decl::Param(n, k) => ty(self.ops.get(n).and_then(|o| o.params.get(k))),
            Decl::Result(n) => ty(self.ops.get(n).map(|o| &o.result)),
            Decl::Interface(n) => Some(self.interfaces.get(n).map_or(self.interfaces_at, |i| i.at)),
            Decl::Extends(n, k) => self
                .interfaces
                .get(n)
                .map(|i| i.extends.get(k).copied().unwrap_or(i.extends_at)),
            Decl::Field(n, k) => ty(self.interfaces.get(n).and_then(|i| i.fields.get(k))),
            Decl::Kind(n) => Some(self.kinds.get(n).map_or(self.kinds_at, |k| k.at)),
            Decl::Request(n) => ty(self.kinds.get(n).map(|k| &k.request)),
            Decl::Message(n) => ty(self.kinds.get(n).and_then(|k| k.message.as_ref())),
            Decl::KindResult(n) => ty(self.kinds.get(n).map(|k| &k.result)),
        };
        found.unwrap_or(Trail::ROOT)
    }
}

// ---------------------------------------------------------------- reader

/// Reads a surface definition from its manifest notation and checks it
/// as `author`'s, within `limits`: the checks, and the definition, are
/// those of [`crate::parse`]. The crate's documentation gives the
/// notation. The text is at most [`Limits::manifest_bytes`] long, which
/// is checked before any of it is read, holds at most
/// [`Limits::manifest_values`] values, and nests its arrays and objects
/// at most [`Limits::manifest_nesting`] deep; one byte-order mark at
/// its start is skipped. The work is proportional to its size,
/// but for looking a key up among the members of its object, and a name
/// among those of its list, which is the size times the logarithm of
/// the size at most.
///
/// # Errors
/// The mistakes in `src`, in the order the text has them. When `src`
/// is not JSON, gives an object a key twice, nests too deep, holds more
/// values than [`Limits::manifest_values`], or is not a manifest of the
/// version this crate reads: that one, with its line or its JSON
/// Pointer. Else the first mistake of notation in each entry of
/// `operations`, of `interfaces` and of `kinds`, and each member of the
/// document the notation does not name, each with the JSON Pointer of
/// the value that is wrong: [`Limits::manifest_errors`] of them at the
/// most, and then one more that says how many entries were not looked
/// at ([`Errors::unread`]), so that what is answered is bounded
/// whatever the text. Else, when every entry is read, the first rule
/// the definition breaks ([`crate::check`]), at the value that breaks
/// it.
pub fn parse_manifest(
    src: &str,
    author: Author<'_>,
    limits: &Limits,
) -> Result<Definition, Errors> {
    if src.len() > limits.manifest_bytes {
        return Err(Errors::from(Error {
            at: Place::Pointer(String::new()),
            message: format!(
                "the manifest is {} bytes long, and the limit is {}",
                src.len(),
                limits.manifest_bytes
            ),
        }));
    }
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let document = tree(src, limits.manifest_nesting(), limits.manifest_values)?;
    let mut r = Reader {
        trail: Trail(Vec::new()),
        places: Places {
            surface: Trail::ROOT,
            rules: Trail::ROOT,
            members: Trail::ROOT,
            namespaces: Vec::new(),
            ops: Vec::new(),
            interfaces_at: Trail::ROOT,
            interfaces: Vec::new(),
            kinds_at: Trail::ROOT,
            kinds: Vec::new(),
        },
        inner: Vec::new(),
        max_depth: limits.depth.min(Limits::MAX_DEPTH),
        errors: Vec::new(),
        max_errors: limits.manifest_errors.max(1),
        unread: 0,
    };
    let (root, interfaces, kinds) = r.document(&document)?;
    let mut errors = std::mem::take(&mut r.errors).into_iter();
    if let Some(first) = errors.next() {
        let mut rest: Vec<Error> = errors.collect();
        if r.unread > 0 {
            rest.push(Error {
                at: Place::Pointer(String::new()),
                message: format!(
                    "{} said, and the {} of the manifest after {} not looked at: put {} right, and read it again",
                    if r.max_errors == 1 { "1 mistake is".to_owned() } else { format!("{} mistakes are", r.max_errors) },
                    if r.unread == 1 { "1 entry".to_owned() } else { format!("{} entries", r.unread) },
                    match (r.max_errors == 1, r.unread == 1) {
                        (true, true) => "it was",
                        (true, false) => "it were",
                        (false, true) => "them was",
                        (false, false) => "them were",
                    },
                    if r.max_errors == 1 { "this" } else { "these" },
                ),
            });
        }
        return Err(Errors::new(first, rest, r.unread));
    }
    check::build(root, interfaces, kinds, author, limits, Notation::Manifest).map_err(|problem| {
        Errors::from(Error {
            at: Place::Pointer(r.trail.pointer(r.places.of(problem.site))),
            message: problem.message,
        })
    })
}

const KEYWORDS: &str = "a type is written with `type` (\"string\", \"number\", \"integer\", \"boolean\", \"null\", \"array\", \"object\"), `enum` and `const` (literals), `items`, `properties`, `required` and `additionalProperties`, `anyOf` (a union), `$ref` and `description`, each where it applies, and with no other keyword of JSON Schema; `{}` is `unknown`";

const INTERFACE: &str = "an entry of `interfaces` is an interface, `{\"type\": \"object\", \"properties\": {…}, \"additionalProperties\": false}` with `required`, `description` and `extends` where it has them; any other type is written where it is used";

/// The parts of a definition, as [`check::build`] takes them.
type Parts = (Namespace, Vec<Interface>, Vec<Kind>);

/// The members of an object, each taken by the reader that knows its
/// key; what is left is refused.
struct Members<'n> {
    at: usize,
    pairs: &'n [(String, Node)],
    taken: Vec<bool>,
}

impl<'n> Members<'n> {
    fn new(at: usize, pairs: &'n [(String, Node)]) -> Self {
        Members {
            at,
            pairs,
            taken: vec![false; pairs.len()],
        }
    }

    /// The member under `key`, and where it is.
    fn take(&mut self, trail: &mut Trail, key: &'static str) -> Option<(usize, &'n Node)> {
        let i = self.pairs.iter().position(|(k, _)| k == key)?;
        self.taken[i] = true;
        Some((trail.word(self.at, key), &self.pairs[i].1))
    }

    /// The key of the first member nothing took.
    fn left(&self) -> Option<&'n str> {
        let i = self.taken.iter().position(|t| !t)?;
        Some(&self.pairs[i].0)
    }
}

/// A surface as a flat list of dotted names fills it: the surface, the
/// namespaces now open below it, outermost first, and with each the
/// names of the namespaces of its own that are closed.
struct Open {
    surface: Namespace,
    closed: BTreeSet<String>,
    below: Vec<(Namespace, BTreeSet<String>)>,
}

impl Open {
    /// Makes the namespace at `path` below the surface the open one,
    /// closing and opening what that takes, and says how many it
    /// opened, and the dotted name of the first namespace on the way
    /// that was closed before, whose members are therefore not listed
    /// together. Such a one is opened again all the same: the mistake
    /// is the entry's that breaks the rule, and not of each that
    /// follows it there.
    fn enter(&mut self, path: &[&str]) -> (usize, Option<String>) {
        let common = self
            .below
            .iter()
            .zip(path)
            .take_while(|((ns, _), name)| ns.name == **name)
            .count();
        while self.below.len() > common {
            self.close();
        }
        let mut again = None;
        for name in &path[common..] {
            let closed = match self.below.last_mut() {
                Some((_, closed)) => closed,
                None => &mut self.closed,
            };
            if closed.remove(*name) && again.is_none() {
                again = Some(path[..=self.below.len()].join("."));
            }
            self.below.push((
                Namespace {
                    name: (*name).to_string(),
                    rules: None,
                    members: Vec::new(),
                },
                BTreeSet::new(),
            ));
        }
        (path.len() - common, again)
    }

    /// Closes the innermost open namespace: it is a member of the one
    /// around it.
    fn close(&mut self) {
        let Some((ns, _)) = self.below.pop() else {
            return;
        };
        let (parent, closed) = match self.below.last_mut() {
            Some((parent, closed)) => (parent, closed),
            None => (&mut self.surface, &mut self.closed),
        };
        closed.insert(ns.name.clone());
        parent.members.push(Member::Namespace(ns));
    }

    /// The namespace that is open: the surface, when none is.
    fn current(&mut self) -> &mut Namespace {
        match self.below.last_mut() {
            Some((ns, _)) => ns,
            None => &mut self.surface,
        }
    }

    /// The surface, every namespace closed.
    fn finish(mut self) -> Namespace {
        while !self.below.is_empty() {
            self.close();
        }
        self.surface
    }
}

/// An object type's two forms.
enum Shape<'n> {
    /// `additionalProperties: false`: its `properties` and `required`.
    Closed {
        properties: Option<(usize, &'n Node)>,
        required: Option<(usize, &'n Node)>,
    },
    /// `additionalProperties` as a type: where it is, and the type.
    Record(usize, &'n Node),
}

/// What a type's keywords say it is.
enum Form<'n> {
    /// A type that holds no other: a name, a literal, a reference.
    Is(Ty),
    /// A union: where its `anyOf` is, its first member and the others,
    /// one at least.
    Union(usize, &'n Node, &'n [Node]),
    /// An array: where its `items` is, and the type.
    Array(usize, &'n Node),
    Object(Shape<'n>),
}

struct Reader {
    trail: Trail,
    places: Places,
    /// The fields of the object types of the type being read.
    inner: Vec<usize>,
    max_depth: usize,
    /// The first mistake of each entry that could not be read, no more
    /// than `max_errors` of them.
    errors: Vec<Error>,
    max_errors: usize,
    /// How many entries were not looked at, once the mistakes were as
    /// many as are said.
    unread: usize,
}

impl Reader {
    fn refuse<T>(&self, at: usize, message: impl Into<String>) -> Result<T, Error> {
        Err(Error {
            at: Place::Pointer(self.trail.pointer(at)),
            message: message.into(),
        })
    }

    fn text<'n>(&self, at: usize, node: &'n Node, what: &str) -> Result<&'n str, Error> {
        match node {
            Node::Str(s) => Ok(s),
            _ => self.refuse(at, format!("{what} is a string")),
        }
    }

    fn list<'n>(&self, at: usize, node: &'n Node, what: &str) -> Result<&'n [Node], Error> {
        match node {
            Node::Arr(items) => Ok(items),
            _ => self.refuse(at, format!("{what} is an array")),
        }
    }

    fn object<'n>(
        &self,
        at: usize,
        node: &'n Node,
        what: &str,
    ) -> Result<&'n [(String, Node)], Error> {
        match node {
            Node::Obj(pairs) => Ok(pairs),
            _ => self.refuse(at, format!("{what} is an object")),
        }
    }

    /// An optional member that is `true` or `false`; `false` when it is
    /// left out.
    fn flag(&self, found: Option<(usize, &Node)>, key: &str) -> Result<bool, Error> {
        match found {
            None => Ok(false),
            Some((_, Node::Bool(b))) => Ok(*b),
            Some((at, _)) => self.refuse(at, format!("`{key}` is `true` or `false`")),
        }
    }

    /// An optional member that is a string.
    fn optional_text(
        &self,
        found: Option<(usize, &Node)>,
        what: &str,
    ) -> Result<Option<String>, Error> {
        found
            .map(|(at, node)| self.text(at, node, what).map(str::to_string))
            .transpose()
    }

    /// Refuses the first member of `m` that no reader took. `what` says
    /// what the object is and which fields it has.
    fn done(&mut self, m: &Members<'_>, what: &str) -> Result<(), Error> {
        match m.left() {
            None => Ok(()),
            Some(key) => {
                let at = self.trail.key(m.at, key);
                self.refuse(at, format!("`{}` is not a field of {what}", shown(key)))
            }
        }
    }

    fn too_deep<T>(&self, at: usize) -> Result<T, Error> {
        self.refuse(
            at,
            format!(
                "nested more than {} deep, the limit for namespaces and for types",
                self.max_depth
            ),
        )
    }

    fn document(&mut self, document: &Node) -> Result<Parts, Error> {
        let pairs = self.object(Trail::ROOT, document, "a manifest")?;
        let mut m = Members::new(Trail::ROOT, pairs);
        match m.take(&mut self.trail, "clax_surface") {
            None => {
                return self.refuse(
                    Trail::ROOT,
                    format!(
                        "the manifest does not say which version of the notation it is written in: add `\"clax_surface\": {MANIFEST_VERSION}`"
                    ),
                );
            }
            Some((_, Node::Num(Some(MANIFEST_VERSION)))) => {}
            Some((at, _)) => {
                return self.refuse(
                    at,
                    format!(
                        "this build reads version {MANIFEST_VERSION} of the manifest notation (`\"clax_surface\": {MANIFEST_VERSION}`), and no other"
                    ),
                );
            }
        }
        let Some((name_at, name)) = m.take(&mut self.trail, "name") else {
            return self.refuse(
                Trail::ROOT,
                "the manifest names no surface: add `\"name\"`, one lowercase word",
            );
        };
        self.places.surface = name_at;
        let name = self.text(name_at, name, "the surface's `name`")?;
        let rules = m.take(&mut self.trail, "rules");
        if let Some((rules_at, _)) = rules {
            self.places.rules = rules_at;
        }
        let rules = self.optional_text(rules, "`rules`")?;
        let mut open = Open {
            surface: Namespace {
                name: name.to_string(),
                rules,
                members: Vec::new(),
            },
            closed: BTreeSet::new(),
            below: Vec::new(),
        };
        // From here on a mistake is one of several: each entry that
        // cannot be read says its first, and the rest are read, in the
        // order the text has them.
        let mut interfaces = Vec::new();
        let mut kinds = Vec::new();
        for (key, node) in pairs {
            if matches!(key.as_str(), "clax_surface" | "name" | "rules") {
                continue;
            }
            if self.full() {
                self.unread += match node {
                    Node::Arr(entries) => entries.len(),
                    Node::Obj(entries) => entries.len(),
                    _ => 1,
                };
                continue;
            }
            let read = match key.as_str() {
                "operations" => {
                    let at = self.trail.word(Trail::ROOT, "operations");
                    self.places.members = at;
                    self.operations(at, node, &mut open)
                }
                "interfaces" => {
                    let at = self.trail.word(Trail::ROOT, "interfaces");
                    self.interfaces(at, node, &mut interfaces)
                }
                "kinds" => {
                    let at = self.trail.word(Trail::ROOT, "kinds");
                    self.kinds(at, node, &mut kinds)
                }
                other => {
                    let at = self.trail.key(Trail::ROOT, other);
                    self.refuse(
                        at,
                        format!(
                            "`{}` is not a field of a manifest, which has `clax_surface`, `name`, `rules`, `operations`, `interfaces` and `kinds`",
                            shown(other)
                        ),
                    )
                }
            };
            self.noted(read);
        }
        Ok((open.finish(), interfaces, kinds))
    }

    /// Keeps the mistake of something that could not be read, to be
    /// said with the others.
    fn noted(&mut self, read: Result<(), Error>) {
        if let Err(e) = read {
            self.errors.push(e);
        }
    }

    /// Whether as many mistakes are kept as are said: nothing more is
    /// looked at then.
    fn full(&self) -> bool {
        self.errors.len() >= self.max_errors
    }

    /// Whether the entries of a list from the `i`th on, of `len`, are
    /// left unread: they are counted, and no more.
    fn stop(&mut self, i: usize, len: usize) -> bool {
        if self.full() {
            self.unread += len - i;
        }
        self.full()
    }

    fn operations(&mut self, at: usize, node: &Node, open: &mut Open) -> Result<(), Error> {
        let entries = self.list(at, node, "`operations`")?;
        for (i, entry) in entries.iter().enumerate() {
            if self.stop(i, entries.len()) {
                break;
            }
            let at = self.trail.index(at, i);
            let read = self.entry(at, entry, open);
            self.noted(read);
        }
        Ok(())
    }

    /// One entry of `operations`: an operation, or a namespace.
    fn entry(&mut self, at: usize, entry: &Node, open: &mut Open) -> Result<(), Error> {
        let pairs = self.object(
            at,
            entry,
            "an entry of `operations` (an operation, or a namespace as `{\"namespace\": \"settings\", \"rules\": \"…\"}`)",
        )?;
        let mut m = Members::new(at, pairs);
        if let Some((name_at, name)) = m.take(&mut self.trail, "namespace") {
            let name = self.text(name_at, name, "a namespace's dotted name")?;
            let path: Vec<&str> = name.split('.').collect();
            let opened = self.enter(open, &path, name_at, at)?;
            if opened == 0 {
                return self.refuse(
                    at,
                    format!(
                        "namespace `{}` has its entry after one of its members, or a second time: a namespace's entry comes once, before its members",
                        shown(name)
                    ),
                );
            }
            let rules = m.take(&mut self.trail, "rules");
            if let (Some((rules_at, _)), Some(place)) = (rules, self.places.namespaces.last_mut()) {
                place.1 = rules_at;
            }
            open.current().rules = self.optional_text(rules, "`rules`")?;
            self.done(&m, "a namespace's entry, which has `namespace` and `rules`")
        } else {
            let Some((name_at, name)) = m.take(&mut self.trail, "name") else {
                return self.refuse(
                    at,
                    "an entry of `operations` has a `name` (an operation, by its dotted name below the surface) or a `namespace`",
                );
            };
            let name = self.text(name_at, name, "an operation's dotted name")?;
            let mut path: Vec<&str> = name.split('.').collect();
            let own = path.pop().unwrap_or_default();
            self.enter(open, &path, name_at, name_at)?;
            let op = self.operation(&mut m, at, own, name)?;
            self.done(
                &m,
                "an operation, which has `name`, `description`, `access`, `waits`, `once`, `throws`, `params` and `result`",
            )?;
            open.current().members.push(Member::Op(op));
            Ok(())
        }
    }

    /// Opens the namespace at `path` for the entry whose name is at
    /// `name_at`; each namespace it opens is known by `opened_at`.
    fn enter(
        &mut self,
        open: &mut Open,
        path: &[&str],
        name_at: usize,
        opened_at: usize,
    ) -> Result<usize, Error> {
        // No deeper than a definition nests, so that nothing built here
        // can be made to nest without bound.
        if path.len() > self.max_depth {
            return self.too_deep(name_at);
        }
        let (opened, again) = open.enter(path);
        self.places
            .namespaces
            .extend(std::iter::repeat_n((opened_at, opened_at), opened));
        match again {
            None => Ok(opened),
            Some(dotted) => self.refuse(
                name_at,
                format!(
                    "namespace `{}` is listed in two places: the members of one namespace follow one another in `operations`",
                    shown(&dotted)
                ),
            ),
        }
    }

    /// One operation: `own` is its own name, `dotted` the name its entry
    /// gives it.
    fn operation(
        &mut self,
        m: &mut Members<'_>,
        at: usize,
        own: &str,
        dotted: &str,
    ) -> Result<Op, Error> {
        let name = shown(dotted);
        let Some((doc_at, doc)) = m.take(&mut self.trail, "description") else {
            return self.refuse(
                at,
                format!(
                    "operation `{name}` has no `description`: say what it does, for the model to read"
                ),
            );
        };
        let doc = self.text(doc_at, doc, "an operation's `description`")?;
        let Some((access_at, access)) = m.take(&mut self.trail, "access") else {
            return self.refuse(
                at,
                format!(
                    "operation `{name}` has no `access`: it is one of \"read\", \"write\", \"effect\" and \"spend\""
                ),
            );
        };
        let access = match self.text(access_at, access, "an operation's `access`")? {
            "read" => Access::Read,
            "write" => Access::Write,
            "effect" => Access::Effect,
            "spend" => Access::Spend,
            other => {
                return self.refuse(
                    access_at,
                    format!(
                        "`{}` is not an access: an operation's `access` is one of \"read\", \"write\", \"effect\" and \"spend\"",
                        shown(other)
                    ),
                );
            }
        };
        let waits = m.take(&mut self.trail, "waits");
        let waits = self.flag(waits, "waits")?;
        let once = m.take(&mut self.trail, "once");
        let once = self.flag(once, "once")?;
        let mut throws = Vec::new();
        let mut codes = Vec::new();
        let mut throws_at = at;
        if let Some((list_at, list)) = m.take(&mut self.trail, "throws") {
            throws_at = list_at;
            for (k, code) in self.list(list_at, list, "`throws`")?.iter().enumerate() {
                let code_at = self.trail.index(list_at, k);
                throws.push(self.text(code_at, code, "an error code")?.to_string());
                codes.push(code_at);
            }
        }
        let mut params = Vec::new();
        let mut param_places = Vec::new();
        if let Some((list_at, list)) = m.take(&mut self.trail, "params") {
            for (k, param) in self.list(list_at, list, "`params`")?.iter().enumerate() {
                let param_at = self.trail.index(list_at, k);
                let (param, place) = self.param(param_at, param)?;
                params.push(param);
                param_places.push(place);
            }
        }
        let Some((result_at, result)) = m.take(&mut self.trail, "result") else {
            return self.refuse(
                at,
                format!(
                    "operation `{name}` has no `result`: give its type, `{{\"type\": \"null\"}}` for one that answers nothing"
                ),
            );
        };
        let (result, result_place) = self.ty_at(result_at, result, result_at)?;
        self.places.ops.push(OpPlace {
            at,
            throws: throws_at,
            codes,
            params: param_places,
            result: result_place,
        });
        Ok(Op {
            name: own.to_string(),
            doc: doc.to_string(),
            access,
            waits,
            once,
            throws,
            params,
            result,
        })
    }

    fn param(&mut self, at: usize, node: &Node) -> Result<(Param, TyPlace), Error> {
        let pairs = self.object(at, node, "a parameter")?;
        let mut m = Members::new(at, pairs);
        let Some((name_at, name)) = m.take(&mut self.trail, "name") else {
            return self.refuse(at, "a parameter has a `name`");
        };
        let name = self.text(name_at, name, "a parameter's `name`")?;
        let optional = m.take(&mut self.trail, "optional");
        let optional = self.flag(optional, "optional")?;
        let Some((schema_at, schema)) = m.take(&mut self.trail, "schema") else {
            return self.refuse(
                at,
                format!("parameter `{}` has no `schema`: give its type", shown(name)),
            );
        };
        let (ty, place) = self.ty_at(schema_at, schema, at)?;
        self.done(&m, "a parameter, which has `name`, `optional` and `schema`")?;
        let param = Param {
            name: name.to_string(),
            optional,
            ty,
        };
        Ok((param, place))
    }

    fn interfaces(
        &mut self,
        at: usize,
        node: &Node,
        out: &mut Vec<Interface>,
    ) -> Result<(), Error> {
        self.places.interfaces_at = at;
        let entries = self.object(at, node, "`interfaces`")?;
        for (i, (name, entry)) in entries.iter().enumerate() {
            if self.stop(i, entries.len()) {
                break;
            }
            let at = self.trail.key(at, name);
            match self.interface(at, name, entry) {
                Ok(interface) => out.push(interface),
                Err(e) => self.errors.push(e),
            }
        }
        Ok(())
    }

    /// One member of `interfaces`.
    fn interface(&mut self, at: usize, name: &str, entry: &Node) -> Result<Interface, Error> {
        {
            let Node::Obj(pairs) = entry else {
                return self.refuse(at, INTERFACE);
            };
            let mut m = Members::new(at, pairs);
            match m.take(&mut self.trail, "type") {
                Some((_, Node::Str(t))) if t == "object" => {}
                _ => return self.refuse(at, INTERFACE),
            }
            let doc = m.take(&mut self.trail, "description");
            let doc = self.optional_text(doc, "an interface's `description`")?;
            let mut extends = Vec::new();
            let mut extends_at = at;
            let mut extends_places = Vec::new();
            if let Some((list_at, list)) = m.take(&mut self.trail, "extends") {
                extends_at = list_at;
                for (k, base) in self.list(list_at, list, "`extends`")?.iter().enumerate() {
                    let base_at = self.trail.index(list_at, k);
                    extends_places.push(base_at);
                    extends.push(
                        self.text(base_at, base, "an interface's name in `extends`")?
                            .to_string(),
                    );
                }
            }
            let Shape::Closed {
                properties,
                required,
            } = self.shape(&mut m, at)?
            else {
                return self.refuse(at, INTERFACE);
            };
            let (fields, places) = self.fields(properties, required, None)?;
            self.done(
                &m,
                "an interface, which has `type`, `description`, `extends`, `properties`, `required` and `additionalProperties`",
            )?;
            self.places.interfaces.push(InterfacePlace {
                at,
                extends_at,
                extends: extends_places,
                fields: places,
            });
            Ok(Interface {
                name: name.to_string(),
                doc,
                extends,
                fields,
            })
        }
    }

    fn kinds(&mut self, at: usize, node: &Node, out: &mut Vec<Kind>) -> Result<(), Error> {
        self.places.kinds_at = at;
        let entries = self.list(at, node, "`kinds`")?;
        for (i, entry) in entries.iter().enumerate() {
            if self.stop(i, entries.len()) {
                break;
            }
            let at = self.trail.index(at, i);
            match self.kind(at, entry) {
                Ok(kind) => out.push(kind),
                Err(e) => self.errors.push(e),
            }
        }
        Ok(())
    }

    /// One entry of `kinds`.
    fn kind(&mut self, at: usize, entry: &Node) -> Result<Kind, Error> {
        {
            let pairs = self.object(at, entry, "a kind")?;
            let mut m = Members::new(at, pairs);
            let Some((name_at, name)) = m.take(&mut self.trail, "name") else {
                return self.refuse(at, "a kind has a `name`");
            };
            let name = self.text(name_at, name, "a kind's `name`")?;
            let Some((doc_at, doc)) = m.take(&mut self.trail, "description") else {
                return self.refuse(
                    at,
                    format!(
                        "kind `{}` has no `description`: it is the prompt's paragraph about the request",
                        shown(name)
                    ),
                );
            };
            let doc = self.text(doc_at, doc, "a kind's `description`")?;
            let mut allow = None;
            if let Some((list_at, list)) = m.take(&mut self.trail, "allow") {
                let mut patterns = Vec::new();
                for (k, pattern) in self.list(list_at, list, "`allow`")?.iter().enumerate() {
                    let pattern_at = self.trail.index(list_at, k);
                    let pattern = self.text(pattern_at, pattern, "an allow pattern")?;
                    match Pattern::parse(pattern) {
                        Ok(p) => patterns.push(p),
                        Err(message) => return self.refuse(pattern_at, message),
                    }
                }
                allow = Some(patterns);
            }
            let mut ty = |r: &mut Self, key: &'static str| match m.take(&mut r.trail, key) {
                Some((ty_at, node)) => r.ty_at(ty_at, node, ty_at).map(Some),
                None => Ok(None),
            };
            let request = ty(self, "request")?;
            let message = ty(self, "message")?;
            let result = ty(self, "result")?;
            let (Some((request, request_place)), Some((result, result_place))) = (request, result)
            else {
                return self.refuse(
                    at,
                    format!(
                        "kind `{}` has a `request` and a `result`, each a type, and a `message` when it takes messages after it starts",
                        shown(name)
                    ),
                );
            };
            self.done(
                &m,
                "a kind, which has `name`, `description`, `allow`, `request`, `message` and `result`",
            )?;
            let (message, message_place) = message.unzip();
            self.places.kinds.push(KindPlace {
                at,
                request: request_place,
                message: message_place,
                result: result_place,
            });
            Ok(Kind {
                name: name.to_string(),
                doc: doc.to_string(),
                allow,
                request,
                message,
                result,
            })
        }
    }

    /// A type where a declaration has one: a problem with it is named at
    /// `named_at`, or at the field of an object type inside it.
    fn ty_at(&mut self, at: usize, node: &Node, named_at: usize) -> Result<(Ty, TyPlace), Error> {
        let outer = std::mem::take(&mut self.inner);
        let (ty, _) = self.schema(at, node, 0, false)?;
        let inner = std::mem::replace(&mut self.inner, outer);
        Ok((
            ty,
            TyPlace {
                at: named_at,
                inner,
            },
        ))
    }

    /// Which form an object type has, by its `additionalProperties`.
    fn shape<'n>(&mut self, m: &mut Members<'n>, at: usize) -> Result<Shape<'n>, Error> {
        let properties = m.take(&mut self.trail, "properties");
        let required = m.take(&mut self.trail, "required");
        match m.take(&mut self.trail, "additionalProperties") {
            None => self.refuse(
                at,
                "an object type says `\"additionalProperties\": false` (it has the fields `properties` lists, and a value with any other is refused) or gives `additionalProperties` a type (it has any keys, as `Record<string, T>`): one that says neither would take fields the format cannot name",
            ),
            Some((_, Node::Bool(false))) => Ok(Shape::Closed {
                properties,
                required,
            }),
            Some((extra_at, Node::Bool(true))) => self.refuse(
                extra_at,
                "`\"additionalProperties\": true` is not a type of the format: an object type lists every field it has (`false`), or has any keys with values of one type (a type here; `{}` for any value)",
            ),
            Some((extra_at, node @ Node::Obj(_))) => {
                if let Some((other_at, _)) = properties.or(required) {
                    return self.refuse(
                        other_at,
                        "`properties` and `required` do not go with `additionalProperties` as a type: an object type lists its fields or has any keys, not both",
                    );
                }
                Ok(Shape::Record(extra_at, node))
            }
            Some((extra_at, _)) => self.refuse(
                extra_at,
                "`additionalProperties` is `false` or a type",
            ),
        }
    }

    /// An object's fields from its `properties` and `required`, in the
    /// order `properties` has them. `depth` is that of an object type
    /// written in a type; `None` for an interface, whose fields are
    /// declarations with places of their own.
    fn fields(
        &mut self,
        properties: Option<(usize, &Node)>,
        required: Option<(usize, &Node)>,
        depth: Option<usize>,
    ) -> Result<(Vec<Field>, Vec<TyPlace>), Error> {
        let mut fields = Vec::new();
        let mut places = Vec::new();
        if let Some((at, node)) = properties {
            for (name, schema) in self.object(at, node, "`properties`")? {
                let field_at = self.trail.key(at, name);
                let (ty, doc) = match depth {
                    Some(depth) => {
                        self.inner.push(field_at);
                        self.schema(field_at, schema, depth + 1, true)?
                    }
                    None => {
                        let outer = std::mem::take(&mut self.inner);
                        let read = self.schema(field_at, schema, 0, true)?;
                        places.push(TyPlace {
                            at: field_at,
                            inner: std::mem::replace(&mut self.inner, outer),
                        });
                        read
                    }
                };
                fields.push(Field {
                    name: name.clone(),
                    // Until `required` names it.
                    optional: true,
                    ty,
                    doc,
                });
            }
        }
        if let Some((at, node)) = required {
            self.require(&mut fields, at, node)?;
        }
        Ok((fields, places))
    }

    /// Makes the fields that `required` names required: each is a field
    /// of `fields`, named once.
    fn require(&mut self, fields: &mut [Field], at: usize, node: &Node) -> Result<(), Error> {
        let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
        for (k, f) in fields.iter().enumerate() {
            // The first of a name, should `properties` hold it twice.
            by_name.entry(f.name.as_str()).or_insert(k);
        }
        let mut named = Vec::new();
        for (k, name) in self.list(at, node, "`required`")?.iter().enumerate() {
            let name_at = self.trail.index(at, k);
            let name = self.text(name_at, name, "a field's name in `required`")?;
            match by_name.remove(name) {
                Some(k) => named.push(k),
                None => {
                    return self.refuse(
                        name_at,
                        format!(
                            "`required` names `{}`, which is not a field `properties` lists, or which it has named already",
                            shown(name)
                        ),
                    );
                }
            }
        }
        for k in named {
            fields[k].optional = false;
        }
        Ok(())
    }

    /// One literal of `enum` or `const`.
    fn literal(&self, at: usize, node: &Node, key: &str) -> Result<Ty, Error> {
        match node {
            Node::Str(s) => Ok(Ty::StrLit(s.clone())),
            Node::Bool(b) => Ok(Ty::BoolLit(*b)),
            _ => self.refuse(
                at,
                format!(
                    "`{key}` holds strings, `true` and `false`, the format's literals: `null` is the type `{{\"type\": \"null\"}}`, and no other value has a literal type"
                ),
            ),
        }
    }

    /// The `type` beside `enum` or `const`, which may say what the
    /// literals are and nothing else.
    fn literal_type(&mut self, m: &mut Members<'_>, literals: &[&Ty]) -> Result<(), Error> {
        let Some((at, node)) = m.take(&mut self.trail, "type") else {
            return Ok(());
        };
        let fits = match node {
            Node::Str(t) if t == "string" => literals.iter().all(|l| matches!(l, Ty::StrLit(_))),
            Node::Str(t) if t == "boolean" => literals.iter().all(|l| matches!(l, Ty::BoolLit(_))),
            _ => false,
        };
        if fits {
            return Ok(());
        }
        self.refuse(
            at,
            "`type` beside `enum` or `const` is \"string\" when every literal is a string, or \"boolean\" when every one is `true` or `false`; it may be left out",
        )
    }

    /// A type, with `depth` types around it, and its `description`,
    /// which only a field takes (`describes`). This and what it calls
    /// to read the types inside a type ([`Reader::fields`],
    /// [`Reader::union`]) are all that recurses here: what decides and
    /// refuses is apart, so that a level costs little stack.
    fn schema(
        &mut self,
        at: usize,
        node: &Node,
        depth: usize,
        describes: bool,
    ) -> Result<(Ty, Option<String>), Error> {
        let (mut m, description) = self.begin(at, node, depth, describes)?;
        let ty = match self.form(&mut m, at)? {
            Form::Is(ty) => ty,
            Form::Union(any_at, first, rest) => self.union(any_at, first, rest, depth)?,
            Form::Array(items_at, items) => {
                Ty::Array(Box::new(self.schema(items_at, items, depth + 1, false)?.0))
            }
            Form::Object(Shape::Record(value_at, value)) => {
                Ty::Record(Box::new(self.schema(value_at, value, depth + 1, false)?.0))
            }
            Form::Object(Shape::Closed {
                properties,
                required,
            }) => {
                // As in a `.d.ts` file, where an object type's braces
                // are a level whether or not a field is between them.
                if depth == self.max_depth {
                    return self.too_deep(at);
                }
                Ty::Object(self.fields(properties, required, Some(depth))?.0)
            }
        };
        self.end(&m, at)?;
        Ok((ty, description))
    }

    /// The members of a type's object, and its `description`.
    fn begin<'n>(
        &mut self,
        at: usize,
        node: &'n Node,
        depth: usize,
        describes: bool,
    ) -> Result<(Members<'n>, Option<String>), Error> {
        if depth > self.max_depth {
            return self.too_deep(at);
        }
        let Node::Obj(pairs) = node else {
            return self.refuse(at, format!("a type is an object: {KEYWORDS}"));
        };
        let mut m = Members::new(at, pairs);
        let description = match m.take(&mut self.trail, "description") {
            Some((doc_at, _)) if !describes => {
                return self.refuse(
                    doc_at,
                    "`description` documents an interface, or a field of one: no other type takes one, since nothing would read it",
                );
            }
            found => self.optional_text(found, "`description`")?,
        };
        Ok((m, description))
    }

    /// Refuses what no keyword of the type's form took.
    fn end(&mut self, m: &Members<'_>, at: usize) -> Result<(), Error> {
        match m.left() {
            None => Ok(()),
            Some(key) => {
                let key_at = self.trail.key(at, key);
                self.refuse(
                    key_at,
                    format!("`{}` is not read here: {KEYWORDS}", shown(key)),
                )
            }
        }
    }

    /// The alternatives of an `anyOf`, each at the union's own depth.
    fn union(&mut self, at: usize, first: &Node, rest: &[Node], depth: usize) -> Result<Ty, Error> {
        let first_at = self.trail.index(at, 0);
        let (first, _) = self.schema(first_at, first, depth, false)?;
        let mut alternatives = Vec::with_capacity(rest.len());
        for (k, member) in rest.iter().enumerate() {
            let member_at = self.trail.index(at, k + 1);
            alternatives.push(self.schema(member_at, member, depth, false)?.0);
        }
        Ok(Ty::union(first, alternatives))
    }

    /// What a type's keywords say it is: the type itself, when it holds
    /// no other, else where the types it holds are.
    fn form<'n>(&mut self, m: &mut Members<'n>, at: usize) -> Result<Form<'n>, Error> {
        if let Some((ref_at, target)) = m.take(&mut self.trail, "$ref") {
            let target = self.text(ref_at, target, "`$ref`")?;
            return match (target, target.strip_prefix("#/interfaces/")) {
                (_, Some(name)) if name.is_empty() || name.contains('/') => self.refuse(
                    ref_at,
                    format!(
                        "`$ref` names a whole interface, `#/interfaces/<Name>`, and `{}` does not: a reference cannot point inside an interface or at `interfaces` itself",
                        shown(target)
                    ),
                ),
                (_, Some(name)) => Ok(Form::Is(Ty::Named(name.to_string()))),
                ("clax:Content", _) => Ok(Form::Is(Ty::Content)),
                ("clax:Image", _) => Ok(Form::Is(Ty::Image)),
                ("clax:Instruction", _) => Ok(Form::Is(Ty::Instruction)),
                ("clax:Int", _) => Ok(Form::Is(Ty::Int)),
                _ => self.refuse(
                    ref_at,
                    format!(
                        "`$ref` names an interface of the manifest (`#/interfaces/<Name>`) or a type of the core's (`clax:Content`, `clax:Image`, `clax:Instruction`, `clax:Int`), and `{}` is neither",
                        shown(target)
                    ),
                ),
            };
        }
        if let Some((any_at, members)) = m.take(&mut self.trail, "anyOf") {
            return match self.list(any_at, members, "`anyOf`")? {
                [first, rest @ ..] if !rest.is_empty() => Ok(Form::Union(any_at, first, rest)),
                _ => self.refuse(
                    any_at,
                    "`anyOf` is a union, of two types or more: write one type as itself",
                ),
            };
        }
        if let Some((const_at, literal)) = m.take(&mut self.trail, "const") {
            let literal = self.literal(const_at, literal, "const")?;
            self.literal_type(m, &[&literal])?;
            return Ok(Form::Is(literal));
        }
        if let Some((enum_at, literals)) = m.take(&mut self.trail, "enum") {
            let mut read = Vec::new();
            for (k, literal) in self.list(enum_at, literals, "`enum`")?.iter().enumerate() {
                let literal_at = self.trail.index(enum_at, k);
                read.push(self.literal(literal_at, literal, "enum")?);
            }
            self.literal_type(m, &read.iter().collect::<Vec<_>>())?;
            let mut read = read.into_iter();
            return match read.next() {
                Some(first) => Ok(Form::Is(Ty::union(first, read))),
                None => self.refuse(
                    enum_at,
                    "`enum` lists no literal: a type with no value is not a type of the format",
                ),
            };
        }
        let Some((type_at, name)) = m.take(&mut self.trail, "type") else {
            return Ok(Form::Is(Ty::Unknown));
        };
        let name = match name {
            Node::Str(name) => name.as_str(),
            Node::Arr(_) => {
                return self.refuse(
                    type_at,
                    "`type` names one type: write a union as `anyOf`, `{\"anyOf\": [{\"type\": \"string\"}, {\"type\": \"null\"}]}`",
                );
            }
            _ => return self.refuse(type_at, "`type` is a string"),
        };
        match name {
            "string" => Ok(Form::Is(Ty::Str)),
            "number" => Ok(Form::Is(Ty::Num)),
            "integer" => Ok(Form::Is(Ty::Int)),
            "boolean" => Ok(Form::Is(Ty::Bool)),
            "null" => Ok(Form::Is(Ty::Null)),
            "array" => match m.take(&mut self.trail, "items") {
                Some((items_at, items)) => Ok(Form::Array(items_at, items)),
                None => self.refuse(
                    at,
                    "an array type gives its elements' type as `items`; `{}` there is `unknown`",
                ),
            },
            "object" => Ok(Form::Object(self.shape(m, at)?)),
            other => self.refuse(
                type_at,
                format!(
                    "`{}` is not a `type` of the format, which has \"string\", \"number\", \"integer\", \"boolean\", \"null\", \"array\" and \"object\"",
                    shown(other)
                ),
            ),
        }
    }
}

// ---------------------------------------------------------------- writer

/// A JSON string.
fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A list of JSON strings on one line.
fn strings<S: AsRef<str>>(out: &mut String, items: impl IntoIterator<Item = S>) {
    out.push('[');
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        quote(out, item.as_ref());
    }
    out.push(']');
}

fn is_literal(t: &Ty) -> bool {
    matches!(t, Ty::StrLit(_) | Ty::BoolLit(_))
}

fn literal(out: &mut String, t: &Ty) {
    match t {
        Ty::StrLit(s) => quote(out, s),
        Ty::BoolLit(b) => out.push_str(if *b { "true" } else { "false" }),
        _ => {}
    }
}

/// How a type's schema names what it does not write out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refs {
    /// The manifest's: an interface as `#/interfaces/<name>`, and the
    /// core's types as `clax:<name>`.
    Manifest,
    /// A tool's input, as the model APIs read JSON Schema: an interface
    /// as `#/$defs/<name>`, and the core's types written out as the
    /// values a script holds ([`crate::check_held`]): `Instruction` a
    /// string, `Content` and `Image` a whole handle.
    Tool,
}

/// What says `t`, without the braces around it: empty for `unknown`.
fn keywords(out: &mut String, t: &Ty, refs: Refs) {
    let simple = |out: &mut String, name: &str| {
        let _ = write!(out, "\"type\": \"{name}\"");
    };
    let core = |out: &mut String, name: &str| {
        let _ = write!(out, "\"$ref\": \"clax:{name}\"");
    };
    match t {
        Ty::Str => simple(out, "string"),
        Ty::Num => simple(out, "number"),
        Ty::Int => simple(out, "integer"),
        Ty::Bool => simple(out, "boolean"),
        Ty::Null => simple(out, "null"),
        Ty::Unknown => {}
        Ty::Instruction if refs == Refs::Tool => simple(out, "string"),
        Ty::Content if refs == Refs::Tool => handle(out, false),
        Ty::Image if refs == Refs::Tool => handle(out, true),
        Ty::Instruction => core(out, "Instruction"),
        Ty::Content => core(out, "Content"),
        Ty::Image => core(out, "Image"),
        Ty::StrLit(_) | Ty::BoolLit(_) => {
            out.push_str("\"const\": ");
            literal(out, t);
        }
        Ty::Named(n) => {
            out.push_str("\"$ref\": ");
            match refs {
                Refs::Manifest => quote(out, &format!("#/interfaces/{n}")),
                Refs::Tool => quote(out, &format!("#/$defs/{n}")),
            }
        }
        Ty::Array(x) => {
            out.push_str("\"type\": \"array\", \"items\": ");
            schema(out, x, None, refs);
        }
        Ty::Record(x) => {
            out.push_str("\"type\": \"object\", \"additionalProperties\": ");
            schema(out, x, None, refs);
        }
        Ty::Object(fields) => object(out, fields.iter(), refs),
        Ty::Union(ts) if ts.iter().all(is_literal) => {
            out.push_str("\"enum\": [");
            for (i, x) in ts.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                literal(out, x);
            }
            out.push(']');
        }
        Ty::Union(ts) => {
            out.push_str("\"anyOf\": [");
            for (i, x) in ts.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                schema(out, x, None, refs);
            }
            out.push(']');
        }
    }
}

/// An object type with `fields`, in their order, without the braces
/// around it: `type`, `properties` (each with its `description` last),
/// `required` and `additionalProperties`.
fn object<'f>(out: &mut String, fields: impl Iterator<Item = &'f Field> + Clone, refs: Refs) {
    out.push_str("\"type\": \"object\", \"properties\": {");
    for (i, f) in fields.clone().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        quote(out, &f.name);
        out.push_str(": ");
        schema(out, &f.ty, f.doc.as_deref(), refs);
    }
    out.push('}');
    required(out, fields, ", ");
    out.push_str(", \"additionalProperties\": false");
}

/// A whole handle to stored bytes, as a script holds one
/// ([`crate::check_held`]), without the braces around it: an `Image`'s
/// `media_type` is one of [`IMAGE_MEDIA_TYPES`].
fn handle(out: &mut String, image: bool) {
    out.push_str(
        "\"type\": \"object\", \"properties\": {\"id\": {\"type\": \"string\"}, \"media_type\": ",
    );
    if image {
        out.push_str("{\"enum\": ");
        strings(out, IMAGE_MEDIA_TYPES);
        out.push('}');
    } else {
        out.push_str("{\"type\": \"string\"}");
    }
    out.push_str(
        ", \"bytes\": {\"type\": \"integer\"}, \"sha256\": {\"type\": \"string\"}, \
         \"name\": {\"type\": \"string\"}, \"width\": {\"type\": \"integer\"}, \
         \"height\": {\"type\": \"integer\"}, \"meta\": {\"type\": \"object\"}}, \
         \"required\": [\"id\", \"media_type\", \"bytes\", \"sha256\"], \"additionalProperties\": false",
    );
}

/// `"required": […]` after `before`, when a field is required.
fn required<'f>(out: &mut String, fields: impl Iterator<Item = &'f Field> + Clone, before: &str) {
    if fields.clone().any(|f| !f.optional) {
        out.push_str(before);
        out.push_str("\"required\": ");
        strings(out, fields.filter(|f| !f.optional).map(|f| &f.name));
    }
}

/// A type on one line, with a field's `description` last.
fn schema(out: &mut String, t: &Ty, description: Option<&str>, refs: Refs) {
    out.push('{');
    let from = out.len();
    keywords(out, t, refs);
    if let Some(d) = description {
        if out.len() > from {
            out.push_str(", ");
        }
        out.push_str("\"description\": ");
        quote(out, d);
    }
    out.push('}');
}

/// Writes the members of an object or the items of an array, one a
/// line, between the brackets that are already and will be written
/// around them: `line` writes what follows the indentation.
struct Lines<'a> {
    out: &'a mut String,
    indent: usize,
    any: bool,
}

impl Lines<'_> {
    fn open(out: &mut String, bracket: char, indent: usize) -> Lines<'_> {
        out.push(bracket);
        Lines {
            out,
            indent,
            any: false,
        }
    }

    /// Starts a line, and gives what to write it to.
    fn line(&mut self) -> &mut String {
        if std::mem::replace(&mut self.any, true) {
            self.out.push(',');
        }
        self.out.push('\n');
        self.out.push_str(&"  ".repeat(self.indent));
        self.out
    }

    /// Starts a line with `"key": `.
    fn key(&mut self, key: &str) -> &mut String {
        let out = self.line();
        quote(out, key);
        out.push_str(": ");
        out
    }

    fn text(&mut self, key: &str, value: &str) {
        quote(self.key(key), value);
    }

    fn close(self, bracket: char) {
        if self.any {
            self.out.push('\n');
            self.out.push_str(&"  ".repeat(self.indent - 1));
        }
        self.out.push(bracket);
    }
}

/// The entries of `operations` for `members`, whose namespace has the
/// dotted name `path` below the surface (with its `.`, when it has one).
fn entries(list: &mut Lines<'_>, members: &[Member], path: &mut String) {
    for m in members {
        match m {
            Member::Op(o) => {
                let indent = list.indent + 1;
                let mut e = Lines::open(list.line(), '{', indent);
                e.text("name", &format!("{path}{}", o.name));
                e.text("description", &o.doc);
                e.text("access", o.access.tag());
                if o.waits {
                    e.key("waits").push_str("true");
                }
                if o.once {
                    e.key("once").push_str("true");
                }
                if !o.throws.is_empty() {
                    strings(e.key("throws"), &o.throws);
                }
                let mut params = Lines::open(e.key("params"), '[', indent + 1);
                for p in &o.params {
                    let out = params.line();
                    out.push_str("{\"name\": ");
                    quote(out, &p.name);
                    if p.optional {
                        out.push_str(", \"optional\": true");
                    }
                    out.push_str(", \"schema\": ");
                    schema(out, &p.ty, None, Refs::Manifest);
                    out.push('}');
                }
                params.close(']');
                schema(e.key("result"), &o.result, None, Refs::Manifest);
                e.close('}');
            }
            Member::Namespace(n) => {
                let before = path.len();
                path.push_str(&n.name);
                // A namespace says itself where its members do not.
                if n.rules.is_some() || n.members.is_empty() {
                    let indent = list.indent + 1;
                    let mut e = Lines::open(list.line(), '{', indent);
                    e.text("namespace", path);
                    if let Some(r) = &n.rules {
                        e.text("rules", r);
                    }
                    e.close('}');
                }
                path.push('.');
                entries(list, &n.members, path);
                path.truncate(before);
            }
        }
    }
}

/// The definition as a manifest laid out for a person to read: the
/// surface, its operations, its interfaces, its kinds, each in the
/// definition's order, a declaration and a field a line, indented two
/// spaces, lines ended by line feeds. Reading the text back gives an
/// equal definition, and the same definition always gives the same
/// text, whichever notation it was read from.
///
/// The members, their order and their spelling are those of the
/// definition's canonical bytes ([`canonical_manifest`], which lists
/// them), and are fixed. The layout is not: the white space between
/// them may change from one version of this crate to the next, so
/// nothing should be keyed on this text. Hash the canonical bytes.
///
/// The text may be longer than [`Limits::manifest_bytes`] lets a
/// manifest be read: [`print_manifest_within`] says whether a reader
/// would take it.
#[must_use]
pub fn print_manifest(def: &Definition) -> String {
    let mut out = String::new();
    let mut top = Lines::open(&mut out, '{', 1);
    let _ = write!(top.key("clax_surface"), "{MANIFEST_VERSION}");
    top.text("name", def.name());
    if let Some(r) = def.rules() {
        top.text("rules", r);
    }
    let mut operations = Lines::open(top.key("operations"), '[', 2);
    entries(&mut operations, def.members(), &mut String::new());
    operations.close(']');
    if !def.interfaces().is_empty() {
        let mut interfaces = Lines::open(top.key("interfaces"), '{', 2);
        for i in def.interfaces() {
            let mut e = Lines::open(interfaces.key(&i.name), '{', 3);
            e.text("type", "object");
            if let Some(d) = &i.doc {
                e.text("description", d);
            }
            if !i.extends.is_empty() {
                strings(e.key("extends"), &i.extends);
            }
            let mut properties = Lines::open(e.key("properties"), '{', 4);
            for f in &i.fields {
                schema(
                    properties.key(&f.name),
                    &f.ty,
                    f.doc.as_deref(),
                    Refs::Manifest,
                );
            }
            properties.close('}');
            if i.fields.iter().any(|f| !f.optional) {
                required(e.line(), i.fields.iter(), "");
            }
            e.key("additionalProperties").push_str("false");
            e.close('}');
        }
        interfaces.close('}');
    }
    if !def.kinds().is_empty() {
        let mut kinds = Lines::open(top.key("kinds"), '[', 2);
        for k in def.kinds() {
            let mut e = Lines::open(kinds.line(), '{', 3);
            e.text("name", &k.name);
            e.text("description", &k.doc);
            if let Some(allow) = &k.allow {
                strings(e.key("allow"), allow.iter().map(ToString::to_string));
            }
            schema(e.key("request"), &k.request, None, Refs::Manifest);
            if let Some(message) = &k.message {
                schema(e.key("message"), message, None, Refs::Manifest);
            }
            schema(e.key("result"), &k.result, None, Refs::Manifest);
            e.close('}');
        }
        kinds.close(']');
    }
    top.close('}');
    out.push('\n');
    out
}

impl Kind {
    /// The JSON Schema of the value a run of this kind finishes with
    /// ([`Kind::result`]), as a model is given it for the input of the
    /// tool that ends the run. It is the manifest's schema of the type
    /// ([`print_manifest`]), with what a tool's input cannot hold
    /// written another way:
    ///
    /// - a result that is an interface is that interface's object type
    ///   itself, with every field it has, those it inherits first
    ///   ([`Definition::fields_of`]), and without its doc comment, which
    ///   the tool's own description may carry;
    /// - an interface the result reaches through its fields is a
    ///   `{"$ref": "#/$defs/<name>"}`, and `$defs` holds each one
    ///   reached, in the definition's order, as an object type with all
    ///   its fields and its doc comment as `description`;
    /// - `Instruction` is a string, and `Content` and `Image` a whole
    ///   handle, the values a script holds ([`Kind::check_result`]).
    ///
    /// So `{summary: string}` is `{"type": "object", "properties":
    /// {"summary": {"type": "string"}}, "required": ["summary"],
    /// "additionalProperties": false}`. A field's doc comment is its
    /// `description`, as in the manifest. What the schema cannot say,
    /// [`Kind::check_result`] still checks: that an optional field may
    /// be `null`, and an `Int` is written without a fraction.
    ///
    /// `def` is the definition the kind is one of. Every kind of a
    /// definition has a result that is an object, which its reader
    /// checks; `None` for a kind that is not so: one whose result is not
    /// an object type, or names an interface `def` lacks.
    #[must_use]
    pub fn result_schema(&self, def: &Definition) -> Option<serde_json::Value> {
        let mut out = String::from("{");
        let roots: Vec<&Ty> = match &self.result {
            Ty::Named(n) => {
                let fields = def.fields_of(n)?;
                object(&mut out, fields.iter().copied(), Refs::Tool);
                fields.iter().map(|&f| &f.ty).collect()
            }
            Ty::Object(fields) => {
                object(&mut out, fields.iter(), Refs::Tool);
                fields.iter().map(|f| &f.ty).collect()
            }
            _ => return None,
        };
        let reached = referenced(def, roots);
        if reached.iter().any(|r| *r) {
            out.push_str(", \"$defs\": {");
            let mut first = true;
            for (i, interface) in def.interfaces().iter().enumerate() {
                if !reached[i] {
                    continue;
                }
                if !std::mem::replace(&mut first, false) {
                    out.push_str(", ");
                }
                quote(&mut out, &interface.name);
                out.push_str(": {");
                if let Some(d) = &interface.doc {
                    out.push_str("\"description\": ");
                    quote(&mut out, d);
                    out.push_str(", ");
                }
                let fields = def.fields_of(&interface.name).unwrap_or_default();
                object(&mut out, fields.iter().copied(), Refs::Tool);
                out.push('}');
            }
            out.push('}');
        }
        out.push('}');
        // The writer's text is JSON, as deep as the type: read without
        // `serde_json`'s bound on depth, which the type's limits bound.
        let mut de = serde_json::Deserializer::from_str(&out);
        de.disable_recursion_limit();
        serde::Deserialize::deserialize(&mut de).ok()
    }
}

/// Which of `def`'s interfaces the types `roots` reference, through
/// their fields and the fields of each interface referenced: one flag
/// for each interface, in declaration order. An interface is flattened
/// where it is written out, so one only extended is not referenced.
fn referenced(def: &Definition, roots: Vec<&Ty>) -> Vec<bool> {
    let mut seen = vec![false; def.interfaces().len()];
    let mut todo = roots;
    while let Some(t) = todo.pop() {
        let mut names = Vec::new();
        t.each(&mut |x| {
            if let Ty::Named(n) = x {
                names.push(n.as_str());
            }
        });
        for n in names {
            let Some(&i) = def.index.interface_by_name.get(n) else {
                continue;
            };
            if !std::mem::replace(&mut seen[i], true) {
                todo.extend(
                    def.fields_of(n)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| &f.ty),
                );
            }
        }
    }
    seen
}

/// How deep the arrays and objects of a text this crate wrote nest.
pub(crate) fn nesting(text: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let mut in_string = false;
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'\\' if in_string => {
                bytes.next();
            }
            b'"' => in_string = !in_string,
            b'{' | b'[' if !in_string => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b'}' | b']' if !in_string => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// The definition's manifest ([`print_manifest`]), when a reader under
/// `limits` would take it: a tool that writes the manifest to a file
/// uses this, so as not to write one that reading refuses.
///
/// # Errors
/// That the manifest is longer than `limits.manifest_bytes`, with both
/// numbers, or, for a definition read under a greater `depth` than
/// `limits` has, that it nests deeper than
/// [`Limits::manifest_nesting`].
pub fn print_manifest_within(def: &Definition, limits: &Limits) -> Result<String, String> {
    let printed = print_manifest(def);
    if printed.len() > limits.manifest_bytes {
        return Err(format!(
            "the definition's manifest is {} bytes long, and the limit is {}",
            printed.len(),
            limits.manifest_bytes
        ));
    }
    let deepest = nesting(&printed);
    if deepest > limits.manifest_nesting() {
        return Err(format!(
            "the definition's manifest nests arrays and objects {deepest} deep, and the limit is {} for a manifest whose types nest {} deep",
            limits.manifest_nesting(),
            limits.depth.min(Limits::MAX_DEPTH)
        ));
    }
    Ok(printed)
}

/// The definition's canonical bytes: what names it, and what to hash.
/// Two definitions are equal exactly when these are, whichever notation
/// each was read from and however its text was laid out.
///
/// They are the definition's manifest as JSON, in UTF-8, with nothing
/// between its tokens: no space, no line end, none at the end. This
/// form is fixed for version 1 of the notation:
///
/// - The members of each object come in one order. The document:
///   `clax_surface`, `name`, `rules`, `operations`, `interfaces`,
///   `kinds`. An operation: `name`, `description`, `access`, `waits`,
///   `once`, `throws`, `params`, `result`; a parameter: `name`,
///   `optional`, `schema`; a namespace's entry: `namespace`, `rules`.
///   An interface: `type`, `description`, `extends`, `properties`,
///   `required`, `additionalProperties`. A kind: `name`, `description`,
///   `allow`, `request`, `message`, `result`. A type: `type`, then
///   `items`, or `properties`, `required` and `additionalProperties`;
///   a field's `description` last.
/// - What the definition orders is in its order: operations (a
///   namespace's where the namespace stands), parameters, error codes,
///   interfaces, fields, the interfaces one extends, a union's
///   alternatives, kinds and allow patterns. `required` names the
///   required fields in the fields' order.
/// - Each meaning has one spelling, [`print_manifest`]'s: a member is
///   left out where its absence says the same (`rules`, `waits`,
///   `once`, `throws`, `optional`, `required`, `description`,
///   `extends`, `interfaces`, `kinds`, `message`, and the entry of a
///   namespace that has no rules and holds a member), and
///   `operations`, `params` and `properties` are always there. A
///   kind's `allow` is left out when the kind has none (one of Clax's
///   own surface, which narrows nothing) and is `[]` when it allows no
///   operation: the two say different things. `Int` is
///   `{"type":"integer"}`; a literal is a `const`; a union of literals
///   alone is an `enum`, any other an `anyOf` with one member for each
///   alternative; `unknown` is `{}`.
/// - A string is written between `"` with `"` as `\"`, `\` as `\\`,
///   a line feed as `\n`, a tab as `\t`, a carriage return as `\r`,
///   any other character below U+0020 as `\u00` and its two lowercase
///   hexadecimal digits, and every other character as itself, in UTF-8:
///   `/` is not escaped, nor is anything that is not ASCII. (A definition
///   holds no control character but the line feeds and tabs of its
///   documentation.)
/// - The only number is the notation's version, in decimal.
///
/// The layout [`print_manifest`] adds for a reader is not part of this:
/// it may change, and these bytes do not.
///
/// How the form may grow: a later version of the notation that gives a
/// definition something more to say writes it as a member that is left
/// out when the definition does not say it (or says what leaving it
/// out means), and a definition is written in the lowest version of
/// the notation that can say it, which is the number `clax_surface`
/// then holds. So a definition that uses nothing new keeps these bytes
/// under every later version, and what was stored under a hash of them
/// is still found.
#[must_use]
pub fn canonical_manifest(def: &Definition) -> String {
    let laid_out = print_manifest(def);
    let mut out = String::with_capacity(laid_out.len());
    let mut in_string = false;
    let mut chars = laid_out.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_string => {
                out.push(c);
                out.extend(chars.next());
            }
            '"' => {
                in_string = !in_string;
                out.push(c);
            }
            ' ' | '\n' if !in_string => {}
            c => out.push(c),
        }
    }
    out
}
