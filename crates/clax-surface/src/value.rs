//! Checking JSON values against a definition's types: a call's
//! arguments, a provider's answer, a kind's request, message and result.
//!
//! A value and the definition it is checked against may both come from
//! outside. A check walks the value against the definition's
//! [`crate::plan`] with a stack of its own, so however deep the value
//! nests the check uses a fixed amount of the thread's stack, and it
//! descends no deeper than [`crate::Limits::value_depth`]. Everything it
//! does is counted against [`crate::Limits::value_nodes`], one unit for
//! each step whose cost has a small bound: comparing one value with one
//! type, looking one key up among a type's fields, looking at one field
//! that tells a union's alternatives apart. Which alternatives of a
//! union a value can be is a lookup in what the plan worked out, and an
//! alternative that does not fit costs what was spent finding that out
//! and nothing else: a refusal is a place and a type until the one that
//! is returned is put into words, once, in a message of bounded length
//! (but for an unknown field's name in a call's own arguments,
//! [`Operation::check_args`]).

use crate::allow::identifier;
#[cfg(test)]
use crate::check;
use crate::check::shown;
#[cfg(test)]
use crate::model::Ty;
use crate::model::{Definition, Kind, Operation};
use crate::plan::{self, Among, Lit, Node, NodeId, Object, has_bit, next_bit, set_bit};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

/// The media types of an `Image`.
pub const IMAGE_MEDIA_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

const ONE_OF_THE_IMAGE_TYPES: &str = "one of image/png, image/jpeg, image/webp, image/gif";

/// The fields of a `Content` a script holds, and of one as it is sent.
const HELD: &[&str] = &[
    "id",
    "media_type",
    "bytes",
    "sha256",
    "name",
    "width",
    "height",
    "meta",
];
const SENT: &[&str] = &["id", "meta"];

/// Why a check of a value did not pass. More reasons may be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueErrorKind {
    /// The value is not of the type: the message says where and how.
    Wrong,
    /// Checking the value would take more work than
    /// [`crate::Limits::value_nodes`] allows: for a call's arguments,
    /// checking them all. It was not checked whole.
    TooLarge,
    /// The value nests deeper than [`crate::Limits::value_depth`]. It
    /// was not checked whole.
    TooDeep,
    /// The kind is not one of the definition's: it is another
    /// definition's, whose types this one does not have. Nothing was
    /// checked.
    ForeignType,
}

/// A value that did not pass a check: why, and the message a script
/// reads, which is what the error displays as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueError {
    pub kind: ValueErrorKind,
    /// What is wrong, after what was checked and the place in it:
    /// `clax.files.list: the result[0].bytes must be an integer`.
    pub message: String,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValueError {}

impl ValueError {
    fn wrong(message: String) -> ValueError {
        ValueError {
            kind: ValueErrorKind::Wrong,
            message,
        }
    }
}

/// Which form of `Content` a check expects: the two sides of Clax
/// filling a handle in from its store.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    /// What a script holds: the whole handle.
    Held,
    /// What a provider or a starter sent: a reference to an upload.
    Sent,
}

/// Checks a value a script holds against `ty`, a type built for a test
/// that is not one of `def`'s own: each call prepares it, in steps
/// counted as a definition's are and charged to the check's budget.
/// Every `Content` and `Image` in the value is a whole handle.
#[cfg(test)]
pub(crate) fn check_held(def: &Definition, ty: &Ty, v: &Value) -> Result<(), ValueError> {
    free(def, ty, v, Side::Held, "value")
}

/// [`check_held`] of a value as a provider or a starter sent it, before
/// Clax fills in its `Content`: each is a reference to an upload.
#[cfg(test)]
pub(crate) fn check_sent(def: &Definition, ty: &Ty, v: &Value) -> Result<(), ValueError> {
    free(def, ty, v, Side::Sent, "value")
}

/// Checks `v` against a type that may not be one of `def`'s own. The
/// type is put in the form a check walks each time, in steps that are
/// counted ([`crate::Limits::plan_steps`] bounds them) and charged to
/// the check's budget, a unit a step.
#[cfg(test)]
fn free(def: &Definition, ty: &Ty, v: &Value, side: Side, what: &str) -> Result<(), ValueError> {
    if let Some(m) = check::foreign(def, ty) {
        let what = if what == "value" { "the value" } else { what };
        return Err(ValueError {
            kind: ValueErrorKind::ForeignType,
            message: format!("{what} cannot be checked: {m}"),
        });
    }
    let limit = def.limits.plan_steps;
    let (extra, root, steps) = plan::beside(
        &def.index.plan.nodes,
        &def.index.interface_by_name,
        ty,
        limit,
    );
    if steps > limit {
        let what = if what == "value" { "the value" } else { what };
        return Err(ValueError {
            kind: ValueErrorKind::ForeignType,
            message: format!(
                "{what} cannot be checked: its type is beyond the surface's limits: it takes {}",
                check::plan_too_large(&def.limits)
            ),
        });
    }
    let mut checker = Checker::new(def, &extra, side);
    // Preparing the type is part of this check's work.
    checker.left = checker.left.saturating_sub(steps);
    checker.check(root, v).map_err(|miss| miss.of(what))
}

impl Operation<'_> {
    /// Checks the arguments of a call, as the script passed them, and
    /// returns them, one value for each parameter, `null` for an
    /// optional one left out. A `Content` among them is a whole handle
    /// (the crate's documentation, "Values"): the script passes what it
    /// holds, and nothing is filled in between it and the provider.
    ///
    /// A refusal that names an unknown field of an argument quotes the
    /// name whole, as Rust's `{:?}` writes a string: the arguments are
    /// the caller's own, and the refusal goes back to that caller, so it
    /// is as long as the caller made the name. (Every other check cuts
    /// such a name short: what it refuses was sent by someone else.)
    ///
    /// `null`, like a missing argument, means "left out": it is accepted
    /// for an optional parameter whatever its type, and refused for a
    /// required one (whose type, by the definition's checks, never
    /// admits `null`; `unknown` is any value but `null` there). Trailing
    /// `null`s beyond the required parameters do not count as passed.
    /// The arguments of one call share one budget of work.
    ///
    /// # Errors
    /// A wrong count, or a value of the wrong type, naming the operation
    /// and the parameter: `clax.files.write: argument 2 (text) is
    /// missing`. When the budget runs out, at whichever argument, that
    /// the call's arguments are together too large
    /// ([`ValueErrorKind::TooLarge`]).
    pub fn check_args(&self, args: &[Value]) -> Result<Vec<Value>, ValueError> {
        let params = &self.op.params;
        let bad = |m: String| ValueError::wrong(format!("clax.{}: {m}", self.name));
        let required = params.iter().filter(|p| !p.optional).count();
        let mut passed = args.len();
        while passed > required && args[passed - 1].is_null() {
            passed -= 1;
        }
        if passed > params.len() {
            return Err(bad(format!(
                "takes at most {} argument{}, got {passed}",
                params.len(),
                if params.len() == 1 { "" } else { "s" }
            )));
        }
        let roots = &self.def.index.plan.ops[self.at].0;
        let mut checker = Checker::new(self.def, &[], Side::Held);
        checker.own = true;
        let mut out = Vec::with_capacity(params.len());
        for (i, (param, &root)) in params.iter().zip(roots).enumerate() {
            let v = args.get(i).unwrap_or(&Value::Null);
            if v.is_null() && param.optional {
                out.push(Value::Null);
                continue;
            }
            if v.is_null() || i >= passed {
                return Err(bad(format!(
                    "argument {} ({}) is missing",
                    i + 1,
                    param.name
                )));
            }
            checker.check(root, v).map_err(|miss| match miss.kind {
                // The budget is the call's, not this argument's.
                ValueErrorKind::TooLarge => ValueError {
                    kind: miss.kind,
                    message: format!(
                        "clax.{}: the call's arguments are together too large to check: the limit on the work of one check is {}",
                        self.name, self.def.limits.value_nodes
                    ),
                },
                _ => miss.of(&format!(
                    "clax.{}: argument {} ({})",
                    self.name,
                    i + 1,
                    param.name
                )),
            })?;
            out.push(v.clone());
        }
        Ok(out)
    }

    /// Checks what a provider answered a call with, before Clax fills in
    /// its `Content`: each is a reference to an upload (the crate's
    /// documentation, "Values").
    ///
    /// # Errors
    /// What is wrong with it, naming the operation: `clax.shell.windows:
    /// the answer[0].id must be a string`.
    pub fn check_answer(&self, v: &Value) -> Result<(), ValueError> {
        let root = self.def.index.plan.ops[self.at].1;
        Checker::new(self.def, &[], Side::Sent)
            .check(root, v)
            .map_err(|miss| miss.of(&format!("clax.{}: the answer", self.name)))
    }

    /// Checks a call's result as the script is about to receive it, after
    /// Clax has filled in its `Content`: each is a whole handle (the
    /// crate's documentation, "Values").
    ///
    /// # Errors
    /// What is wrong with it, naming the operation: `clax.files.list: the
    /// result[0].bytes must be an integer`.
    pub fn check_result(&self, v: &Value) -> Result<(), ValueError> {
        let root = self.def.index.plan.ops[self.at].1;
        Checker::new(self.def, &[], Side::Held)
            .check(root, v)
            .map_err(|miss| miss.of(&format!("clax.{}: the result", self.name)))
    }
}

/// One of a kind's three types.
#[derive(Clone, Copy)]
enum Part {
    Request,
    Message,
    Result,
}

impl Kind {
    /// Checks the request a run of this kind is started with, as its
    /// starter sent it, before Clax fills in its `Content`: each is a
    /// reference to an upload (the crate's documentation, "Values").
    /// `def` is the definition the kind is one of.
    ///
    /// # Errors
    /// What is wrong with it, naming the kind: `kind triage: the
    /// request.ask must be a string`.
    pub fn check_request(&self, def: &Definition, v: &Value) -> Result<(), ValueError> {
        self.check(def, Part::Request, v)
    }

    /// Checks a message sent to a run of this kind after it started, as
    /// its sender sent it, before Clax fills in its `Content`
    /// (a reference to an upload: the crate's documentation, "Values").
    /// `def` is the definition the kind is one of.
    ///
    /// # Errors
    /// What is wrong with it, naming the kind; or that the kind takes no
    /// messages.
    pub fn check_message(&self, def: &Definition, v: &Value) -> Result<(), ValueError> {
        self.check(def, Part::Message, v)
    }

    /// Checks the value a run of this kind finishes with. It is the
    /// script's own value, so a `Content` in it is a whole handle the
    /// script holds (the crate's documentation, "Values"); nothing is
    /// filled in. `def` is the definition the kind is one of.
    ///
    /// # Errors
    /// What is wrong with it, naming the kind: `kind triage: the
    /// result.summary is missing`.
    pub fn check_result(&self, def: &Definition, v: &Value) -> Result<(), ValueError> {
        self.check(def, Part::Result, v)
    }

    fn check(&self, def: &Definition, part: Part, v: &Value) -> Result<(), ValueError> {
        let name = shown(&self.name);
        let (side, what, takes) = match part {
            Part::Request => (Side::Sent, "request", true),
            Part::Message => (Side::Sent, "message", self.message.is_some()),
            Part::Result => (Side::Held, "result", true),
        };
        let what = format!("kind {name}: the {what}");
        if !takes {
            return Err(ValueError::wrong(format!(
                "kind {name} takes no messages after it starts"
            )));
        }
        // The kind's place in `def`, where its types have nodes already.
        let at = def
            .kinds
            .iter()
            .position(|k| std::ptr::eq(k, self))
            .or_else(|| def.kinds.iter().position(|k| k == self));
        let root = at.and_then(|n| {
            let (request, message, result) = def.index.plan.kinds[n];
            match part {
                Part::Request => Some(request),
                Part::Message => message,
                Part::Result => Some(result),
            }
        });
        match root {
            Some(root) => Checker::new(def, &[], side)
                .check(root, v)
                .map_err(|miss| miss.of(&what)),
            None => Err(ValueError {
                kind: ValueErrorKind::ForeignType,
                message: format!(
                    "kind {name} is not one of the kinds of surface `{}`",
                    shown(def.name())
                ),
            }),
        }
    }
}

/// A key, as a place in a value: `.name`, or `["…"]` for one that is
/// not a plain name, cut short.
fn key(out: &mut String, k: &str) {
    if identifier(k) {
        out.push('.');
        out.push_str(&shown(k));
    } else {
        out.push_str("[\"");
        out.push_str(&shown(k));
        out.push_str("\"]");
    }
}

/// A list of names for a message, cut short.
fn listed<'x>(names: impl Iterator<Item = &'x str>) -> String {
    const MAX: usize = 200;
    let mut out = String::new();
    for n in names {
        if out.len() > MAX {
            out.push_str(", …");
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&shown(n));
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out
    }
}

/// The nodes a check walks: a definition's own, then those of a type
/// built beside them.
#[derive(Clone, Copy)]
struct Types<'a> {
    def: &'a Definition,
    extra: &'a [Node],
}

impl<'a> Types<'a> {
    fn node(self, id: NodeId) -> &'a Node {
        let fixed = &self.def.index.plan.nodes;
        let at = id as usize;
        match at.checked_sub(fixed.len()) {
            None => &fixed[at],
            Some(mine) => &self.extra[mine],
        }
    }

    /// The alternatives the literal `v` is one of, if any: a search
    /// among a telling field's literals, which are in order.
    fn takes(self, table: &[(Lit, Among)], v: &Value) -> Option<Among> {
        use std::cmp::Ordering;
        let found = table.binary_search_by(|(lit, _)| match (lit, v) {
            (Lit::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Lit::Str(node, at), Value::String(b)) => self.text(*node, *at).cmp(b.as_str()),
            (Lit::Bool(_), Value::String(_)) => Ordering::Less,
            (Lit::Str(..), Value::Bool(_)) => Ordering::Greater,
            // Nothing else is a literal: it comes after them all.
            _ => Ordering::Less,
        });
        found.ok().map(|at| table[at].1)
    }

    /// The string a [`Lit::Str`] names.
    fn text(self, node: NodeId, at: u32) -> &'a str {
        match self.node(node) {
            Node::StrLit(s) => s,
            Node::Union(u) => u.strings.get(at as usize).map_or("", |s| s),
            _ => "",
        }
    }

    /// A type as the model reads it, for a message: `Int` is `number`.
    /// It stops when `out` is full, and recurses no deeper than a type
    /// nests.
    fn write(self, id: NodeId, out: &mut Cut) -> Result<(), Full> {
        match self.node(id) {
            Node::Unknown => out.put("unknown"),
            Node::Str => out.put("string"),
            Node::Instruction => out.put("Instruction"),
            Node::Num | Node::Int => out.put("number"),
            Node::Bool => out.put("boolean"),
            Node::Null => out.put("null"),
            Node::Content => out.put("Content"),
            Node::Image => out.put("Image"),
            Node::Never => out.put("never"),
            Node::StrLit(s) => {
                out.put("\"")?;
                for c in s.chars() {
                    if matches!(c, '"' | '\\') {
                        out.put("\\")?;
                    }
                    out.put(c.encode_utf8(&mut [0; 4]))?;
                }
                out.put("\"")
            }
            Node::BoolLit(b) => out.put(if *b { "true" } else { "false" }),
            Node::Named(i) => out.put(&self.def.interfaces[*i as usize].name),
            Node::Array(x) => {
                let union = matches!(self.node(*x), Node::Union(_));
                if union {
                    out.put("(")?;
                }
                self.write(*x, out)?;
                out.put(if union { ")[]" } else { "[]" })
            }
            Node::Record(x) => {
                out.put("Record<string, ")?;
                self.write(*x, out)?;
                out.put(">")
            }
            Node::Object(o) if o.fields.is_empty() => out.put("{}"),
            Node::Object(o) => {
                // An interface is written by its name, at a `Named`
                // node: no check is handed an interface's own node.
                debug_assert!(id as usize >= self.def.interfaces.len());
                out.put("{ ")?;
                for (n, f) in o.fields.iter().enumerate() {
                    if n > 0 {
                        out.put("; ")?;
                    }
                    out.put(&f.name)?;
                    out.put(if f.optional { "?: " } else { ": " })?;
                    self.write(f.ty, out)?;
                }
                out.put(" }")
            }
            Node::Union(u) => {
                for (n, alt) in u.alts.iter().enumerate() {
                    if n > 0 {
                        out.put(" | ")?;
                    }
                    self.write(*alt, out)?;
                }
                Ok(())
            }
        }
    }

    /// A type for a message, no longer than a message should carry.
    fn shown(self, id: NodeId) -> String {
        const MAX: usize = 200;
        let mut out = Cut {
            text: String::new(),
            left: MAX,
        };
        if self.write(id, &mut out).is_err() {
            out.text.push('…');
        }
        out.text
    }
}

/// Text that takes so many characters and no more.
struct Cut {
    text: String,
    left: usize,
}

/// A [`Cut`] has all it takes.
struct Full;

impl Cut {
    fn put(&mut self, s: &str) -> Result<(), Full> {
        for c in s.chars() {
            if self.left == 0 {
                return Err(Full);
            }
            self.left -= 1;
            self.text.push(c);
        }
        Ok(())
    }
}

/// What is wrong with a value, as what it takes to say so: nothing is
/// put into words unless the check returns it.
#[derive(Clone, Copy)]
enum What<'a, 'v> {
    /// It ` must be` this: `a string`.
    Form(&'static str),
    /// It ` must be` of this type.
    Type(NodeId),
    /// It lacks this field.
    Lacks(&'a str),
    /// It has this key, which is none of these fields.
    Extra(&'v str, Names<'a>),
    /// Its field of this name ` must be` this.
    Field(&'static str, &'static str),
}

#[derive(Clone, Copy)]
enum Names<'a> {
    Of(&'a Object),
    Fixed(&'static [&'static str]),
}

/// A check that did not pass: why, and what is wrong after the name of
/// what was checked.
struct Miss {
    kind: ValueErrorKind,
    rest: String,
}

impl Miss {
    /// The error about `what` (`value`, `clax.files.read: argument 1
    /// (path)`).
    fn of(self, what: &str) -> ValueError {
        ValueError {
            kind: self.kind,
            message: format!("{what}{}", self.rest),
        }
    }
}

/// What a check has found out, under a union it had to try alternative
/// by alternative: whether an array or an object fits a union or an
/// interface, so that it is compared with one type once however many
/// alternatives lead there. A type is named by its node; a value by
/// where it is in memory, which is its own for as long as this holds
/// the borrow `'v` that every value it is asked about is behind.
struct Known<'v> {
    fits: BTreeMap<(NodeId, usize), bool>,
    values: PhantomData<&'v Value>,
}

impl<'v> Known<'v> {
    fn get(&self, node: NodeId, v: &'v Value) -> Option<bool> {
        self.fits
            .get(&(node, std::ptr::from_ref(v) as usize))
            .copied()
    }

    fn set(&mut self, node: NodeId, v: &'v Value, fits: bool) {
        self.fits
            .insert((node, std::ptr::from_ref(v) as usize), fits);
    }
}

/// A value part of the way through its check.
struct Frame<'a, 'v> {
    value: &'v Value,
    /// The node to remember the outcome under, when it is remembered.
    remember: Option<NodeId>,
    /// How long `Checker::bits` was before this frame took any.
    bits: usize,
    at: At<'a, 'v>,
}

/// Where a frame is in its value.
enum At<'a, 'v> {
    /// The elements before `next` fit.
    Array {
        items: &'v [Value],
        of: NodeId,
        next: usize,
    },
    /// The entries before `key` fit.
    Record {
        entries: serde_json::map::Iter<'v>,
        of: NodeId,
        key: &'v str,
    },
    /// The fields to look at are the bits this frame took, one a field:
    /// those before `next` are done, and `field` is the one being looked
    /// at.
    Object {
        object: &'a Object,
        map: &'v Map<String, Value>,
        next: usize,
        field: usize,
    },
    /// A union one of whose alternatives the value can be: the frame
    /// above is that alternative's.
    One,
    /// A union several of whose alternatives the value can be: they are
    /// tried in order from `next`. With `chosen`, the candidates are the
    /// bits this frame took, one for each of `alts`; without, all of
    /// `alts`.
    Trial {
        union: NodeId,
        alts: &'a [NodeId],
        chosen: bool,
        next: usize,
    },
}

/// What entering a value found.
#[derive(Clone, Copy)]
enum Entered<'a, 'v> {
    /// It fits, and nothing is left to look at.
    Fits,
    /// It may fit: a frame is on the stack for what is left.
    Open,
    Miss(What<'a, 'v>),
}

/// One check of values against one definition, with what it has left to
/// spend and what it has found out.
struct Checker<'a, 'v> {
    types: Types<'a>,
    side: Side,
    /// How many more units of work it may do.
    left: usize,
    frames: Vec<Frame<'a, 'v>>,
    /// The places in `frames` of the unions being tried alternative by
    /// alternative.
    trials: Vec<usize>,
    /// How many arrays and objects enclose the value being looked at.
    depth: usize,
    /// The bit sets of the frames, each frame's after those below it.
    bits: Vec<u64>,
    known: Known<'v>,
    /// How many times a refusal was put into words.
    worded: usize,
    /// The values are their reader's own (a call's arguments): a refusal
    /// quotes an unknown field's name whole, not cut short.
    own: bool,
    /// When kept, the alternative each union was read through for an
    /// array or an object ([`Choices`]).
    choices: Option<BTreeMap<(NodeId, usize), NodeId>>,
}

impl<'a, 'v> Checker<'a, 'v> {
    fn new(def: &'a Definition, extra: &'a [Node], side: Side) -> Self {
        Checker {
            types: Types { def, extra },
            side,
            left: def.limits.value_nodes,
            frames: Vec::new(),
            trials: Vec::new(),
            depth: 0,
            bits: Vec::new(),
            known: Known {
                fits: BTreeMap::new(),
                values: PhantomData,
            },
            worded: 0,
            own: false,
            choices: None,
        }
    }

    /// Keeps, when choices are kept, that the union `union` read `v`
    /// through its alternative `alt`.
    fn chose(&mut self, union: NodeId, v: &'v Value, alt: NodeId) {
        if let Some(choices) = &mut self.choices {
            choices.insert((union, std::ptr::from_ref(v) as usize), alt);
        }
    }

    /// One unit of work, if there is one left.
    fn spend(&mut self) -> Result<(), Miss> {
        if self.left == 0 {
            return Err(Miss {
                kind: ValueErrorKind::TooLarge,
                rest: format!(
                    " is too large to check: the limit on the work of one check is {}",
                    self.types.def.limits.value_nodes
                ),
            });
        }
        self.left -= 1;
        Ok(())
    }

    /// `Ok` when `v` is of the type at `root`.
    fn check(&mut self, root: NodeId, v: &'v Value) -> Result<(), Miss> {
        self.frames.clear();
        self.trials.clear();
        self.bits.clear();
        self.depth = 0;
        match self.enter(root, v)? {
            Entered::Fits => {
                debug_assert!(self.settled());
                return Ok(());
            }
            Entered::Open => {}
            Entered::Miss(what) => self.missed(what, true)?,
        }
        while let Some(top) = self.frames.last_mut() {
            // What the top frame looks at next: a value inside its own,
            // or nothing more.
            let next = match &mut top.at {
                At::Array { items, of, next } => items.get(*next).map(|x| {
                    *next += 1;
                    Ok((*of, x))
                }),
                At::Record { entries, of, key } => entries.next().map(|(k, x)| {
                    *key = k;
                    Ok((*of, x))
                }),
                At::Object {
                    object,
                    map,
                    next,
                    field,
                } => next_bit(&self.bits[top.bits..], *next).map(|at| {
                    *next = at + 1;
                    *field = at;
                    let f = &object.fields[at];
                    match map.get(&*f.name) {
                        Some(x) => Ok((f.ty, x)),
                        None => Err(What::Lacks(&f.name)),
                    }
                }),
                At::One => None,
                At::Trial {
                    alts, chosen, next, ..
                } => {
                    let candidate = if *chosen {
                        next_bit(&self.bits[top.bits..], *next)
                    } else {
                        (*next < alts.len()).then_some(*next)
                    };
                    candidate.map(|at| {
                        *next = at + 1;
                        Ok((alts[at], top.value))
                    })
                }
            };
            match next {
                Some(Ok((node, x))) => match self.enter(node, x)? {
                    Entered::Fits => self.fitted(),
                    Entered::Open => {}
                    Entered::Miss(what) => self.missed(what, true)?,
                },
                // The frame's own value lacks a field.
                Some(Err(what)) => self.missed(what, false)?,
                None => {
                    let spent = match self.frames.last().map(|f| &f.at) {
                        Some(At::Trial { union, .. }) => Some(*union),
                        _ => None,
                    };
                    match spent {
                        // No alternative fit.
                        Some(union) => {
                            self.pop(false);
                            self.missed(What::Type(union), true)?;
                        }
                        None => {
                            self.pop(true);
                            self.fitted();
                        }
                    }
                }
            }
        }
        debug_assert!(self.settled());
        Ok(())
    }

    /// Whether nothing is left of a check: what one that passed leaves
    /// behind.
    fn settled(&self) -> bool {
        self.frames.is_empty() && self.trials.is_empty() && self.bits.is_empty() && self.depth == 0
    }

    /// The value the top frame was looking at fits: a union that was
    /// waiting on an alternative is done too, and whatever waited on it.
    fn fitted(&mut self) {
        while matches!(
            self.frames.last().map(|f| &f.at),
            Some(At::One | At::Trial { .. })
        ) {
            self.pop(true);
        }
    }

    /// Takes the top frame off, remembering whether its value fit.
    fn pop(&mut self, fits: bool) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        // The top frame's bits are the last there are, and all of its
        // own: one a field, or one a candidate.
        debug_assert_eq!(
            self.bits.len() - frame.bits,
            match frame.at {
                At::Object { object, .. } => plan::words(object.fields.len()),
                At::Trial {
                    alts, chosen: true, ..
                } => plan::words(alts.len()),
                _ => 0,
            }
        );
        self.bits.truncate(frame.bits);
        // A union tried alternative by alternative fits by the one it
        // tried last.
        if fits
            && let At::Trial {
                union, alts, next, ..
            } = &frame.at
        {
            self.chose(*union, frame.value, alts[*next - 1]);
        }
        match frame.at {
            At::Array { .. } | At::Record { .. } | At::Object { .. } => self.depth -= 1,
            At::Trial { .. } => {
                // The innermost union being tried is this one.
                debug_assert_eq!(self.trials.last(), Some(&self.frames.len()));
                self.trials.pop();
            }
            At::One => {}
        }
        if let Some(node) = frame.remember {
            self.known.set(node, frame.value, fits);
        }
    }

    /// A value does not fit: the one the top frame was looking at
    /// (`inside`), or the top frame's own. Under a union that is being
    /// tried, that alternative is over and the union goes on to its
    /// next; under none, this is the check's answer.
    fn missed(&mut self, what: What<'a, 'v>, inside: bool) -> Result<(), Miss> {
        match self.trials.last() {
            Some(&trial) => {
                while self.frames.len() > trial + 1 {
                    self.pop(false);
                }
                Ok(())
            }
            None => {
                let frames = self.frames.len() - usize::from(!inside && !self.frames.is_empty());
                Err(Miss {
                    kind: ValueErrorKind::Wrong,
                    rest: self.words(frames, what),
                })
            }
        }
    }

    /// The place the first `frames` frames are at, and what is wrong
    /// there.
    fn words(&mut self, frames: usize, what: What<'a, 'v>) -> String {
        self.worded += 1;
        let mut out = String::new();
        for frame in &self.frames[..frames] {
            match &frame.at {
                At::Array { next, .. } => {
                    out.push_str(&format!("[{}]", next - 1));
                }
                At::Record { key: k, .. } => key(&mut out, k),
                At::Object { object, field, .. } => {
                    out.push('.');
                    out.push_str(&object.fields[*field].name);
                }
                At::One | At::Trial { .. } => {}
            }
        }
        match what {
            What::Form(form) => {
                out.push_str(" must be ");
                out.push_str(form);
            }
            What::Type(id) => match self.types.node(id) {
                Node::Union(u) if u.only_strings => {
                    let names = u.alts.iter().filter_map(|&a| match self.types.node(a) {
                        Node::StrLit(s) => Some(&**s),
                        _ => None,
                    });
                    out.push_str(" must be one of ");
                    out.push_str(&listed(names));
                }
                _ => {
                    out.push_str(" must be ");
                    out.push_str(&self.types.shown(id));
                }
            },
            What::Lacks(name) => {
                out.push('.');
                out.push_str(name);
                out.push_str(" is missing");
            }
            What::Extra(k, names) => {
                let fields = match names {
                    Names::Of(object) => listed(object.fields.iter().map(|f| &*f.name)),
                    Names::Fixed(names) => listed(names.iter().copied()),
                };
                if self.own {
                    out.push_str(&format!(" has no field {k:?} (fields: {fields})"));
                } else {
                    out.push_str(&format!(
                        " has no field \"{}\" (fields: {fields})",
                        shown(k)
                    ));
                }
            }
            What::Field(name, form) => {
                out.push_str(&format!(".{name} must be {form}"));
            }
        }
        out
    }

    /// Looks at `v` as a value of the type at `id`: one unit, and one
    /// more for each key or telling field it looks up. What is inside
    /// `v` is left to a frame.
    fn enter(&mut self, id: NodeId, v: &'v Value) -> Result<Entered<'a, 'v>, Miss> {
        let limits = &self.types.def.limits;
        if self.depth > limits.value_depth {
            return Err(Miss {
                kind: ValueErrorKind::TooDeep,
                rest: format!(
                    " is nested more than {} deep, the limit: it was not checked",
                    limits.value_depth
                ),
            });
        }
        self.spend()?;
        let node = self.types.node(id);
        let remember = match node {
            Node::Named(i) => Some(*i),
            Node::Union(_) => Some(id),
            _ => None,
        }
        .filter(|_| self.may_come_again(v));
        if let Some(node) = remember
            && let Some(fits) = self.known.get(node, v)
        {
            return Ok(if fits {
                Entered::Fits
            } else {
                Entered::Miss(What::Type(id))
            });
        }
        let entered = self.begin(id, node, v, remember)?;
        if let Some(node) = remember {
            match entered {
                Entered::Fits => self.known.set(node, v, true),
                Entered::Miss(_) => self.known.set(node, v, false),
                Entered::Open => {}
            }
        }
        Ok(entered)
    }

    /// Whether a check may come to `v` with the same union or interface
    /// again: an array or an object (anything else is looked at in one
    /// step, so there is nothing to save) inside a value that a union
    /// is being tried against alternative by alternative, where two
    /// alternatives may lead to the same type. The alternatives
    /// themselves are each tried once, unless a union further out is
    /// being tried too.
    fn may_come_again(&self, v: &'v Value) -> bool {
        (v.is_object() || v.is_array())
            && match self.trials.as_slice() {
                [] => false,
                [only] => !std::ptr::eq(self.frames[*only].value, v),
                _ => true,
            }
    }

    fn begin(
        &mut self,
        id: NodeId,
        node: &'a Node,
        v: &'v Value,
        remember: Option<NodeId>,
    ) -> Result<Entered<'a, 'v>, Miss> {
        let is = |fits: bool, form: &'static str| {
            if fits {
                Entered::Fits
            } else {
                Entered::Miss(What::Form(form))
            }
        };
        Ok(match node {
            Node::Unknown => Entered::Fits,
            Node::Never => Entered::Miss(What::Type(id)),
            Node::Str | Node::Instruction => is(v.is_string(), "a string"),
            Node::Int => is(v.is_i64() || v.is_u64(), "an integer"),
            Node::Num => is(v.is_number(), "a number"),
            Node::Bool => is(v.is_boolean(), "a boolean"),
            Node::Null => is(v.is_null(), "null"),
            Node::StrLit(s) if v.as_str() == Some(&**s) => Entered::Fits,
            Node::BoolLit(b) if v.as_bool() == Some(*b) => Entered::Fits,
            Node::StrLit(_) | Node::BoolLit(_) => Entered::Miss(What::Type(id)),
            Node::Content => self.content(v, false),
            Node::Image => self.content(v, true),
            Node::Array(of) => match v.as_array() {
                None => Entered::Miss(What::Form("an array")),
                Some(items) => self.open(
                    v,
                    remember,
                    At::Array {
                        items,
                        of: *of,
                        next: 0,
                    },
                ),
            },
            Node::Record(of) => match v.as_object() {
                None => Entered::Miss(What::Form("an object")),
                Some(map) => self.open(
                    v,
                    remember,
                    At::Record {
                        entries: map.iter(),
                        of: *of,
                        key: "",
                    },
                ),
            },
            Node::Named(i) => match self.types.node(*i) {
                Node::Object(object) => self.object(object, v, remember)?,
                _ => Entered::Miss(What::Type(id)),
            },
            Node::Object(object) => self.object(object, v, remember)?,
            Node::Union(u) => self.union(id, u, v, remember)?,
        })
    }

    /// Puts a frame on the stack for what is inside `v`.
    fn open(&mut self, v: &'v Value, remember: Option<NodeId>, at: At<'a, 'v>) -> Entered<'a, 'v> {
        if matches!(at, At::Array { .. } | At::Record { .. } | At::Object { .. }) {
            self.depth += 1;
        }
        self.frames.push(Frame {
            value: v,
            remember,
            bits: self.bits.len(),
            at,
        });
        Entered::Open
    }

    /// An object against an object type: every key is one of the type's
    /// fields, and the frame it opens goes through the fields to look
    /// at, in the order the type declares them: those the object has
    /// (but an optional one that is `null`), and the required ones it
    /// lacks.
    fn object(
        &mut self,
        object: &'a Object,
        v: &'v Value,
        remember: Option<NodeId>,
    ) -> Result<Entered<'a, 'v>, Miss> {
        let Some(map) = v.as_object() else {
            return Ok(Entered::Miss(What::Form("an object")));
        };
        let start = self.bits.len();
        self.bits.extend_from_slice(&object.required);
        for (k, x) in map {
            if let Err(miss) = self.spend() {
                self.bits.truncate(start);
                return Err(miss);
            }
            match object.find(k) {
                None => {
                    self.bits.truncate(start);
                    return Ok(Entered::Miss(What::Extra(k, Names::Of(object))));
                }
                Some(at) if object.fields[at].optional && x.is_null() => {}
                Some(at) => set_bit(&mut self.bits[start..], at),
            }
        }
        self.depth += 1;
        self.frames.push(Frame {
            value: v,
            remember,
            bits: start,
            at: At::Object {
                object,
                map,
                next: 0,
                field: 0,
            },
        });
        Ok(Entered::Open)
    }

    /// A value against a union: which alternatives it can be is looked
    /// up by its form, and for an object by what it holds under each
    /// name that tells the alternatives apart, one unit a name (or one
    /// for each type the field has among the alternatives, where those
    /// types take too many literals for a table).
    fn union(
        &mut self,
        id: NodeId,
        u: &'a plan::Union,
        v: &'v Value,
        remember: Option<NodeId>,
    ) -> Result<Entered<'a, 'v>, Miss> {
        let none = Entered::Miss(What::Type(id));
        let is = |fits: bool| if fits { Entered::Fits } else { none };
        if u.any {
            return Ok(Entered::Fits);
        }
        let (alts, chosen): (&'a [NodeId], bool) = match v {
            Value::Null => return Ok(is(u.null)),
            Value::Bool(b) => return Ok(is(u.takes_bool(*b))),
            Value::Number(n) => return Ok(is(u.number || (u.int && (n.is_i64() || n.is_u64())))),
            Value::String(s) => return Ok(is(u.takes_str(s))),
            Value::Array(_) => (&u.arrays, false),
            Value::Object(map) => {
                let start = self.bits.len();
                self.bits
                    .resize(start + plan::words(u.objects.len()), u64::MAX);
                let tells = u.tells.as_deref();
                for tell in tells.iter().flat_map(|t| &t.tells) {
                    if let Err(miss) = self.spend() {
                        self.bits.truncate(start);
                        return Err(miss);
                    }
                    let held = match self.types.node(tell.object) {
                        Node::Object(o) => map.get(&*o.fields[tell.field as usize].name),
                        _ => None,
                    };
                    // Word `n` of some of the alternatives.
                    let word = |among, n| tells.map_or(0, |t| t.word(among, n));
                    let mine = start..self.bits.len();
                    match (held, &tell.by_literal) {
                        // None of those that require it.
                        (None, _) => {
                            for (n, mine) in self.bits[mine].iter_mut().enumerate() {
                                *mine &= !word(tell.alts, n);
                            }
                        }
                        // Of those that require it, the ones this
                        // literal is one of.
                        (Some(x), Some(table)) => {
                            let takes = self.types.takes(table, x);
                            for (n, mine) in self.bits[mine].iter_mut().enumerate() {
                                *mine &= !word(tell.alts, n) | takes.map_or(0, |t| word(t, n));
                            }
                        }
                        // The same, a type at a time: one more unit for
                        // each type after the first.
                        (Some(x), None) => {
                            for (at, (ty, alts)) in tell.types.iter().enumerate() {
                                if at > 0
                                    && let Err(miss) = self.spend()
                                {
                                    self.bits.truncate(start);
                                    return Err(miss);
                                }
                                if !self.literal(*ty, x) {
                                    for (n, mine) in self.bits[mine.clone()].iter_mut().enumerate()
                                    {
                                        *mine &= !word(*alts, n);
                                    }
                                }
                            }
                        }
                    }
                }
                (&u.objects, true)
            }
        };
        // The candidates: every one of `alts`, or those whose bit is
        // set.
        let start = self.bits.len() - if chosen { plan::words(alts.len()) } else { 0 };
        let candidate = |from: usize, bits: &[u64]| {
            if chosen {
                next_bit(bits, from).filter(|&at| at < alts.len())
            } else {
                (from < alts.len()).then_some(from)
            }
        };
        let first = candidate(0, &self.bits[start..]);
        let second = first.and_then(|at| candidate(at + 1, &self.bits[start..]));
        let Some(first) = first else {
            self.bits.truncate(start);
            return Ok(none);
        };
        if second.is_some() {
            // Several: each is tried, and what one finds out about the
            // values inside is remembered for the next.
            if chosen {
                // The bits past the last alternative are not candidates.
                for at in alts.len()..(self.bits.len() - start) * 64 {
                    if has_bit(&self.bits[start..], at) {
                        self.bits[start + at / 64] &= !(1 << (at % 64));
                    }
                }
            }
            self.trials.push(self.frames.len());
            self.frames.push(Frame {
                value: v,
                remember,
                bits: start,
                at: At::Trial {
                    union: id,
                    alts,
                    chosen,
                    next: 0,
                },
            });
            return Ok(Entered::Open);
        }
        // The value says which alternative it means: what is wrong with
        // that one is what is wrong. Kept as the union's reading whether
        // or not it fits: one that does not is on no path a value that
        // passed is read along.
        self.bits.truncate(start);
        self.chose(id, v, alts[first]);
        if remember.is_none() {
            return self.enter(alts[first], v);
        }
        self.open(v, remember, At::One);
        let entered = self.enter(alts[first], v)?;
        if !matches!(entered, Entered::Open) {
            self.frames.pop();
        }
        Ok(entered)
    }

    /// Whether `v` is the literal, or one of the literals, at `id`.
    fn literal(&self, id: NodeId, v: &Value) -> bool {
        match (self.types.node(id), v) {
            (Node::StrLit(s), Value::String(x)) => **s == **x,
            (Node::BoolLit(b), Value::Bool(x)) => b == x,
            (Node::Union(u), Value::String(x)) => u.takes_str(x),
            (Node::Union(u), Value::Bool(x)) => u.takes_bool(*x),
            _ => false,
        }
    }

    /// A `Content` or an `Image`, in the form this check's side has it.
    fn content(&self, v: &'v Value, image: bool) -> Entered<'a, 'v> {
        let Some(o) = v.as_object() else {
            return Entered::Miss(What::Form("an object"));
        };
        let held = self.side == Side::Held;
        let names = if held { HELD } else { SENT };
        // A handle has eight fields at most, so this stops at the ninth
        // key at the latest.
        if let Some(k) = o.keys().find(|k| !names.contains(&k.as_str())) {
            return Entered::Miss(What::Extra(k, Names::Fixed(names)));
        }
        for &name in names {
            let required = match name {
                "id" => true,
                "media_type" | "bytes" | "sha256" => held,
                _ => false,
            };
            let form = match o.get(name) {
                None | Some(Value::Null) if !required => continue,
                None => return Entered::Miss(What::Lacks(name)),
                Some(x) => match name {
                    "bytes" | "width" | "height" => {
                        (!x.is_u64()).then_some("a whole number, zero or more")
                    }
                    "meta" => (!x.is_object()).then_some("an object"),
                    _ => (!x.is_string()).then_some("a string"),
                },
            };
            if let Some(form) = form {
                return Entered::Miss(What::Field(name, form));
            }
        }
        if image && held {
            let media_type = o
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !IMAGE_MEDIA_TYPES.contains(&media_type) {
                return Entered::Miss(What::Field("media_type", ONE_OF_THE_IMAGE_TYPES));
            }
        }
        Entered::Fits
    }
}

/// Whether `v` is a value of the definition's type at `node`, as a
/// script holds it (`held`: whole handles) or as a starter or a
/// provider sends it (references to uploads), spending from
/// `left`, which it lowers by the units it spent: a union's reading of
/// a value is the first of its alternatives that this says it fits, in
/// the order the check tries them.
///
/// # Errors
/// That `left` ran out ([`ValueErrorKind::TooLarge`]), or that the
/// value nests too deep ([`ValueErrorKind::TooDeep`]), after `what`.
pub(crate) fn fits(
    def: &Definition,
    node: NodeId,
    v: &Value,
    held: bool,
    left: &mut usize,
    what: &str,
) -> Result<bool, ValueError> {
    let side = if held { Side::Held } else { Side::Sent };
    let mut checker = Checker::new(def, &[], side);
    checker.left = *left;
    let r = checker.check(node, v);
    *left = checker.left;
    match r {
        Ok(()) => Ok(true),
        Err(miss) if miss.kind == ValueErrorKind::Wrong => Ok(false),
        Err(miss) => Err(miss.of(what)),
    }
}

/// The alternative a check read each union through, for every array
/// and object it read through a union of several of them: the first
/// that fits, in the order the check tries them, which is the union's
/// reading of the value wherever it meets it. A value is named by where
/// it is in memory, so these are of the value checked, behind `'v`.
pub(crate) struct Choices<'v> {
    chosen: BTreeMap<(NodeId, usize), NodeId>,
    values: PhantomData<&'v Value>,
}

impl<'v> Choices<'v> {
    /// The alternative of `union` that `v` was read as, when the check
    /// read it so.
    pub(crate) fn of(&self, union: NodeId, v: &'v Value) -> Option<NodeId> {
        self.chosen
            .get(&(union, std::ptr::from_ref(v) as usize))
            .copied()
    }
}

/// Checks `v` against the definition's type at `root`, as a script
/// holds it (`held`) or as it was sent, and answers which alternative
/// each union read it through ([`Choices`]): the check of a kind's
/// request or message, done once for a reading of it that follows it.
///
/// # Errors
/// What is wrong with `v`, after `what`, as the check says it.
pub(crate) fn check_choosing<'v>(
    def: &Definition,
    root: NodeId,
    v: &'v Value,
    held: bool,
    what: &str,
) -> Result<Choices<'v>, ValueError> {
    let side = if held { Side::Held } else { Side::Sent };
    let mut checker = Checker::new(def, &[], side);
    checker.choices = Some(BTreeMap::new());
    checker.check(root, v).map_err(|miss| miss.of(what))?;
    Ok(Choices {
        chosen: checker.choices.take().unwrap_or_default(),
        values: PhantomData,
    })
}

/// What one check did, for the tests that count it.
#[cfg(test)]
pub(crate) struct Work {
    /// The units of work it spent.
    pub units: usize,
    /// How many times it put a refusal into words.
    pub worded: usize,
    /// How many outcomes it remembered.
    pub remembered: usize,
}

/// [`check_held`], and the work it did on the value: preparing the
/// type is not counted.
#[cfg(test)]
pub(crate) fn measured(def: &Definition, ty: &Ty, v: &Value) -> (Result<(), ValueError>, Work) {
    let (extra, root, _) = plan::beside(
        &def.index.plan.nodes,
        &def.index.interface_by_name,
        ty,
        usize::MAX,
    );
    let mut checker = Checker::new(def, &extra, Side::Held);
    let r = checker.check(root, v).map_err(|miss| miss.of("value"));
    let work = Work {
        units: def.limits.value_nodes - checker.left,
        worded: checker.worded,
        remembered: checker.known.fits.len(),
    };
    (r, work)
}

/// A type of `def` as a message about a value shows it, whole.
#[cfg(test)]
pub(crate) fn shown_whole(def: &Definition, ty: &Ty) -> String {
    let (extra, root, _) = plan::beside(
        &def.index.plan.nodes,
        &def.index.interface_by_name,
        ty,
        usize::MAX,
    );
    let types = Types { def, extra: &extra };
    let mut out = Cut {
        text: String::new(),
        left: usize::MAX,
    };
    let _ = types.write(root, &mut out);
    out.text
}
