//! What a run of a kind is given (the design's "A run"): the operations
//! it may call, and the places in its request that hold the person's
//! own words or content. Each is a pure function of a definition and,
//! for the places, of a request that passes the kind's check.

use crate::allow::Pattern;
use crate::check::shown;
use crate::model::{Definition, Kind};
use crate::plan::{Node, NodeId};
use crate::value::{Choices, ValueError, ValueErrorKind, check_choosing, fits};
use serde_json::Value;
use std::fmt;

/// A node holds an `Instruction` somewhere inside it.
pub(crate) const INSTRUCTION: u8 = 1;
/// A node holds a `Content` or an `Image` somewhere inside it.
pub(crate) const CONTENT: u8 = 2;
/// Every value of the node holds a `Content` or an `Image`: one is
/// reached through required fields alone, and through every
/// alternative of each union on the way.
pub(crate) const CONTENT_REQUIRED: u8 = 4;

/// For each of `nodes`, what it holds ([`INSTRUCTION`], [`CONTENT`],
/// [`CONTENT_REQUIRED`]), through arrays, records, fields, unions and
/// interfaces, in time and memory proportional to the nodes and the
/// types written inside them. An interface that reaches itself only
/// through required fields has no value, and requires nothing.
pub(crate) fn reaches(nodes: &[Node]) -> Vec<u8> {
    // Each edge goes from a node to one written directly inside it;
    // `required` is set when every value of the node has a value of
    // the inner one (a required field, a union's alternative, the
    // interface a name names).
    fn edges(node: &Node, mut edge: impl FnMut(NodeId, bool)) {
        match node {
            Node::Named(i) => edge(*i, true),
            Node::Array(x) | Node::Record(x) => edge(*x, false),
            Node::Object(o) => o.fields.iter().for_each(|f| edge(f.ty, !f.optional)),
            Node::Union(u) => u.alts.iter().for_each(|&a| edge(a, true)),
            _ => {}
        }
    }
    let n = nodes.len();
    // The nodes each node is written inside, one list after another.
    let mut start = vec![0usize; n + 1];
    for node in nodes {
        edges(node, |inner, _| start[inner as usize + 1] += 1);
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut outer: Vec<(NodeId, bool)> = vec![(0, false); start[n]];
    for (at, node) in nodes.iter().enumerate() {
        edges(node, |inner, required| {
            outer[fill[inner as usize]] = (at as NodeId, required);
            fill[inner as usize] += 1;
        });
    }
    let mut bits = vec![0u8; n];
    let mut todo = Vec::new();
    for (at, node) in nodes.iter().enumerate() {
        bits[at] = match node {
            Node::Instruction => INSTRUCTION,
            Node::Content | Node::Image => CONTENT | CONTENT_REQUIRED,
            _ => 0,
        };
        if bits[at] != 0 {
            todo.push(at);
        }
    }
    let mut required: Vec<usize> = todo
        .iter()
        .copied()
        .filter(|&at| bits[at] & CONTENT_REQUIRED != 0)
        .collect();
    // What a node holds, its outer nodes hold: a node is looked at
    // again only when it gains a bit, twice at most.
    while let Some(at) = todo.pop() {
        let held = bits[at] & (INSTRUCTION | CONTENT);
        for &(up, _) in &outer[start[at]..start[at + 1]] {
            let up = up as usize;
            if bits[up] | held != bits[up] {
                bits[up] |= held;
                todo.push(up);
            }
        }
    }
    // A union requires what all of its alternatives do, anything else
    // what one inner node it requires does: counted down, so each edge
    // is followed once.
    let mut need: Vec<usize> = nodes
        .iter()
        .map(|node| match node {
            Node::Union(u) => u.alts.len(),
            _ => 1,
        })
        .collect();
    while let Some(at) = required.pop() {
        for &(up, edge_required) in &outer[start[at]..start[at + 1]] {
            let up = up as usize;
            if !edge_required || bits[up] & CONTENT_REQUIRED != 0 {
                continue;
            }
            need[up] -= 1;
            if need[up] == 0 {
                bits[up] |= CONTENT_REQUIRED;
                required.push(up);
            }
        }
    }
    bits
}

/// What narrows a run's operations beside its kind's `@allow`. The
/// default is an operator who allows `*`, a starter who narrows
/// nothing, and no content.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunScope<'a> {
    /// The operator's allow patterns, from Clax's configuration or a
    /// registration's settings. `None` is `*`: every operation.
    pub operator: Option<&'a [Pattern]>,
    /// The starter's allow patterns, the `allow` member of the request
    /// that starts the run. `None` narrows nothing; an empty list
    /// allows no operation.
    pub starter: Option<&'a [Pattern]>,
    /// Whether Clax stores content. Until it does (`false`), an
    /// operation whose parameters or result hold a `Content` or an
    /// `Image` is withheld ([`Withheld::Content`]).
    pub content: bool,
}

/// Why a run is not given an operation of its surface: the first of
/// these, in this order, that holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Withheld {
    /// The kind's `@allow` names it not.
    Kind,
    /// The operator's allow patterns name it not.
    Operator,
    /// The starter's allow patterns name it not.
    Starter,
    /// Its parameters or its result hold a `Content` or an `Image`,
    /// written there or in an interface reached from there, and the
    /// daemon's store keeps no content ([`RunScope::content`]).
    Content,
}

impl Withheld {
    /// The word a registration's answer gives for an operation withheld
    /// for this reason ([`crate::wire::WithheldOp::reason`]): `None` for
    /// a reason that is a run's alone, since a registration withholds
    /// an operation from every run only for [`Withheld::Content`].
    #[must_use]
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Withheld::Content => Some(crate::wire::WITHHELD_CONTENT),
            Withheld::Kind | Withheld::Operator | Withheld::Starter => None,
        }
    }
}

impl fmt::Display for Withheld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Withheld::Kind => "the kind's allow list does not name it",
            Withheld::Operator => "the operator's allow list does not name it",
            Withheld::Starter => "the starter's allow list does not name it",
            Withheld::Content => "content is not available yet",
        })
    }
}

/// Where a kind's request holds a `Content` or an `Image`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentUse {
    /// Nowhere.
    None,
    /// Somewhere a request may leave out: an optional field, an array's
    /// element, a `Record`'s value, one alternative of a union of
    /// several. A request that carries one there is refused while Clax
    /// stores no content ([`Kind::content_places`] finds them).
    Optional,
    /// Somewhere every request has one: a run of the kind cannot be
    /// started while Clax stores no content.
    Required,
}

/// What a run of one kind of a definition is given
/// ([`Definition::run_operations`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOperations<'a> {
    /// The operations the run may call, by their dotted names
    /// ([`crate::Operation::name`]), in declaration order: those of
    /// the definition that the kind's `@allow`, the operator's patterns
    /// and the starter's all name, less those withheld for content.
    pub granted: Vec<&'a str>,
    /// Every other operation of the definition, in declaration order,
    /// with the first reason it is withheld for.
    pub withheld: Vec<(&'a str, Withheld)>,
    /// The kind's allow patterns that name no operation of this
    /// definition, in the order the kind lists them: what it wants of
    /// other surfaces (`threads.open`, `runs.wait`), which the caller
    /// that holds a run's other surfaces gives it or not. No mistake of
    /// the definition's, though a pattern under the surface's own name
    /// here names nothing anywhere.
    pub elsewhere: Vec<&'a Pattern>,
    /// Where the kind's request holds a `Content` or an `Image`.
    pub request_content: ContentUse,
}

impl Definition {
    /// The operations a run of the kind named `kind` may call: the
    /// definition's operations that the kind's `@allow`, the operator's
    /// patterns and the starter's all name, in declaration order, less
    /// those withheld for content while `scope.content` is `false`.
    /// The rest are listed as withheld, each with the first reason that
    /// holds of [`Withheld`]'s, and the patterns of the kind's
    /// `@allow` that name none of this definition's operations as
    /// wanted elsewhere. A pattern names operations as
    /// [`Pattern::matches`] says: `*` every one, `a.b.*` every one
    /// whose dotted name starts with `a.b.` (at any depth), `a.b` that
    /// one alone. A kind with no `@allow` (only Clax's own surface has
    /// one) narrows nothing, and one whose list is empty allows none.
    /// `None` when the definition has no such kind. It does no I/O, and
    /// takes time proportional to the operations times the patterns.
    #[must_use]
    pub fn run_operations(&self, kind: &str, scope: &RunScope<'_>) -> Option<RunOperations<'_>> {
        let n = self.kinds.iter().position(|k| k.name == kind)?;
        let k = &self.kinds[n];
        let (granted, withheld) = self.operations_for(Some(k), scope);
        let elsewhere = k
            .allow
            .iter()
            .flatten()
            .filter(|p| !p.names_any(&[self]))
            .collect();
        let request = self.index.reaches[self.index.plan.kinds[n].0 as usize];
        let request_content = if request & CONTENT_REQUIRED != 0 {
            ContentUse::Required
        } else if request & CONTENT != 0 {
            ContentUse::Optional
        } else {
            ContentUse::None
        };
        Some(RunOperations {
            granted,
            withheld,
            elsewhere,
            request_content,
        })
    }

    /// The operations a run of another surface's kind that this surface
    /// joins may call (the design's "A run": a surface bound into a run it
    /// did not declare the kind of, by a standing join): as
    /// [`Definition::run_operations`] gives a kind's, with no kind's
    /// `@allow` to narrow them. `scope.starter` is the join's own allow
    /// patterns. Nothing is wanted elsewhere, and there is no request.
    #[must_use]
    pub fn joined_operations(&self, scope: &RunScope<'_>) -> RunOperations<'_> {
        let (granted, withheld) = self.operations_for(None, scope);
        RunOperations {
            granted,
            withheld,
            elsewhere: Vec::new(),
            request_content: ContentUse::None,
        }
    }

    /// Each operation, in declaration order, granted or withheld with the
    /// first reason that holds, for a run of `kind` (`None`: a run this
    /// surface joins, which no kind's `@allow` narrows).
    fn operations_for(
        &self,
        kind: Option<&Kind>,
        scope: &RunScope<'_>,
    ) -> (Vec<&str>, Vec<(&str, Withheld)>) {
        let names = |list: Option<&[Pattern]>, op: &str| {
            list.is_none_or(|patterns| patterns.iter().any(|p| p.matches(op)))
        };
        let reaches = &self.index.reaches;
        let holds_content = |at: usize| {
            let (params, result) = &self.index.plan.ops[at];
            params
                .iter()
                .chain([result])
                .any(|&node| reaches[node as usize] & CONTENT != 0)
        };
        let (mut granted, mut withheld) = (Vec::new(), Vec::new());
        for o in self.operations() {
            let reason = if kind.is_some_and(|k| !k.allows(o.name)) {
                Some(Withheld::Kind)
            } else if !names(scope.operator, o.name) {
                Some(Withheld::Operator)
            } else if !names(scope.starter, o.name) {
                Some(Withheld::Starter)
            } else if !scope.content && holds_content(o.at) {
                Some(Withheld::Content)
            } else {
                None
            };
            match reason {
                Some(why) => withheld.push((o.name, why)),
                None => granted.push(o.name),
            }
        }
        (granted, withheld)
    }
}

impl Kind {
    /// The places in a request of this kind whose declared type is
    /// `Instruction`: the person's own words, which a prompt can name
    /// without marking them inside the JSON. Each is a JSON Pointer
    /// (RFC 6901) into `v` (`""` for the request itself, `/ask`,
    /// `/turns/0/text`, `/notes/a~1b` for the key `a/b`), in the
    /// order the value is written out: an array's elements in order,
    /// an object's members in the order its `serde_json::Map` keeps
    /// them. `def` is the definition the kind is one of.
    ///
    /// The request is checked first ([`Kind::check_request`]), and read
    /// as that check reads it: through optional fields (one that is
    /// absent or `null` holds nothing), array elements, `Record`
    /// values and interfaces; a union by the first of its alternatives
    /// the value is one of, in the order the check tries them, so a
    /// field of type `Instruction` in an alternative the value is not
    /// is no place. A string that a union reads as `Instruction` and as
    /// another of its alternatives too (`string`, a literal equal to
    /// it) is not one: the person's words are those that cannot be
    /// read as anything else. Under `unknown` nothing is a place.
    ///
    /// It does the work of the check of the value
    /// ([`crate::Limits::value_nodes`]), which keeps which alternative
    /// of each union it read an array or an object through, and then a
    /// unit for each value it looks at, within a budget of the same
    /// size: a union is not tried again, however deep unions nest.
    ///
    /// # Errors
    /// What [`Kind::check_request`] refuses `v` for; that the kind is
    /// not one of `def`'s ([`ValueErrorKind::ForeignType`]); or that
    /// reading it would take more work than the budget
    /// ([`ValueErrorKind::TooLarge`]).
    pub fn instruction_places(
        &self,
        def: &Definition,
        v: &Value,
    ) -> Result<Vec<String>, ValueError> {
        places(def, self, v, INSTRUCTION, false)
    }

    /// [`Kind::instruction_places`] of a message sent to a run of this
    /// kind after it started: the places in `v` whose declared type, in
    /// the kind's message type, is `Instruction`, found and ordered as
    /// they are in a request. The message is checked first
    /// ([`Kind::check_message`]).
    ///
    /// # Errors
    /// What [`Kind::check_message`] refuses `v` for (a kind that takes
    /// no messages among it), and as [`Kind::instruction_places`].
    pub fn message_instruction_places(
        &self,
        def: &Definition,
        v: &Value,
    ) -> Result<Vec<String>, ValueError> {
        places(def, self, v, INSTRUCTION, true)
    }

    /// The places in a request of this kind that hold a `Content` or an
    /// `Image`, as [`Kind::instruction_places`] finds and orders them:
    /// a JSON Pointer to each reference to an upload. A caller refuses
    /// a request that has any while Clax stores no content
    /// ([`ContentUse::Optional`]).
    ///
    /// # Errors
    /// As [`Kind::instruction_places`].
    pub fn content_places(&self, def: &Definition, v: &Value) -> Result<Vec<String>, ValueError> {
        places(def, self, v, CONTENT, false)
    }

    /// [`Kind::content_places`] of a message sent to a run of this kind
    /// after it started ([`Kind::message_instruction_places`]).
    ///
    /// # Errors
    /// As [`Kind::message_instruction_places`].
    pub fn message_content_places(
        &self,
        def: &Definition,
        v: &Value,
    ) -> Result<Vec<String>, ValueError> {
        places(def, self, v, CONTENT, true)
    }
}

/// `token` as one step of a JSON Pointer, after `/`.
fn pointer_step(out: &mut String, token: &str) {
    out.push('/');
    for c in token.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            c => out.push(c),
        }
    }
}

/// The places in `v`, a request of `kind` (a message of it, with
/// `message`), of the values whose type holds `want` itself.
fn places(
    def: &Definition,
    kind: &Kind,
    v: &Value,
    want: u8,
    message: bool,
) -> Result<Vec<String>, ValueError> {
    let name = shown(&kind.name);
    let Some(n) = def
        .kinds
        .iter()
        .position(|k| std::ptr::eq(k, kind))
        .or_else(|| def.kinds.iter().position(|k| k == kind))
    else {
        return Err(ValueError {
            kind: ValueErrorKind::ForeignType,
            message: format!(
                "kind {name} is not one of the kinds of surface `{}`",
                shown(def.name())
            ),
        });
    };
    let root = if message {
        def.index.plan.kinds[n].1
    } else {
        Some(def.index.plan.kinds[n].0)
    };
    let Some(root) = root else {
        // A kind that takes no messages: its check says so.
        kind.check_message(def, v)?;
        return Ok(Vec::new());
    };
    let what = format!(
        "kind {name}: the {}",
        if message { "message" } else { "request" }
    );
    // The kind's check, as `Kind::check_request` and
    // `Kind::check_message` make it, keeping which alternative of each
    // union it read the value through, so that the walk reads it so too
    // without trying them again.
    let choices = check_choosing(def, root, v, false, &what)?;
    let mut left = def.limits.value_nodes;
    let mut out = Vec::new();
    places_in(
        def,
        Walk {
            root,
            v,
            at: String::new(),
            want,
            held: false,
            what: &what,
            choices: Some(&choices),
        },
        &mut left,
        &mut out,
    )?;
    Ok(out.into_iter().map(|(at, _)| at).collect())
}

/// One value [`places_in`] reads: its type, where it is, what is sought
/// and on which side of Clax filling in its `Content` it is.
struct Walk<'a> {
    root: NodeId,
    v: &'a Value,
    /// The value's place, as a JSON Pointer.
    at: String,
    want: u8,
    /// Whether the value is one a script holds (its `Content` whole
    /// handles), not one a sender sent.
    held: bool,
    what: &'a str,
    /// Which alternative of each union the check of `v` read it
    /// through, when the walk follows such a check: without, each
    /// union's alternatives are tried again.
    choices: Option<&'a Choices<'a>>,
}

/// The places under `w.v`, a value that passed its check, of the values
/// whose type holds `w.want` itself, each with whether that type is
/// `Image`, added to `out` in the order the value is written out;
/// spending from `left`.
fn places_in(
    def: &Definition,
    w: Walk<'_>,
    left: &mut usize,
    out: &mut Vec<(String, bool)>,
) -> Result<(), ValueError> {
    let Walk {
        root,
        v,
        at,
        want,
        held,
        what,
        choices,
    } = w;
    let nodes = &def.index.plan.nodes;
    let reaches = &def.index.reaches;
    let too_large = || ValueError {
        kind: ValueErrorKind::TooLarge,
        message: format!(
            "{what} is too large to read: the limit on the work of one check is {}",
            def.limits.value_nodes
        ),
    };
    let mut todo: Vec<(NodeId, &Value, String)> = vec![(root, v, at)];
    'values: while let Some((mut id, v, at)) = todo.pop() {
        // The node the value is read as: through names and unions.
        loop {
            if reaches[id as usize] & want == 0 {
                continue 'values;
            }
            *left = left.checked_sub(1).ok_or_else(too_large)?;
            match &nodes[id as usize] {
                Node::Named(i) => id = *i,
                Node::Union(u) => {
                    let alts: &[NodeId] = match v {
                        _ if u.any => continue 'values,
                        Value::String(s) => {
                            let instruction = u
                                .alts
                                .iter()
                                .find(|&&a| matches!(nodes[a as usize], Node::Instruction));
                            let also = u.alts.iter().any(|&a| match &nodes[a as usize] {
                                Node::Str => true,
                                Node::StrLit(l) => **l == **s,
                                _ => false,
                            });
                            match instruction {
                                Some(&a) if !also => {
                                    id = a;
                                    continue;
                                }
                                _ => continue 'values,
                            }
                        }
                        Value::Array(_) => &u.arrays,
                        Value::Object(_) => &u.objects,
                        _ => continue 'values,
                    };
                    // The check takes the first alternative that fits;
                    // one alone is the one, the value having passed.
                    let mut chosen = None;
                    if let [only] = alts {
                        chosen = Some(*only);
                    } else if let Some(alt) = choices.and_then(|c| c.of(id, v)) {
                        chosen = Some(alt);
                    } else {
                        for &alt in alts {
                            if fits(def, alt, v, held, left, what)? {
                                chosen = Some(alt);
                                break;
                            }
                        }
                    }
                    match chosen {
                        Some(alt) => id = alt,
                        None => continue 'values,
                    }
                }
                _ => break,
            }
        }
        let step = |k: &str| {
            let mut p = at.clone();
            pointer_step(&mut p, k);
            p
        };
        match &nodes[id as usize] {
            Node::Instruction if want == INSTRUCTION && v.is_string() => out.push((at, false)),
            Node::Content if want == CONTENT && v.is_object() => out.push((at, false)),
            Node::Image if want == CONTENT && v.is_object() => out.push((at, true)),
            Node::Array(x) => {
                let items = v.as_array().map_or(&[][..], Vec::as_slice);
                for (i, item) in items.iter().enumerate().rev() {
                    todo.push((*x, item, step(&i.to_string())));
                }
            }
            Node::Record(x) => {
                for (k, item) in v.as_object().iter().flat_map(|m| m.iter().rev()) {
                    todo.push((*x, item, step(k)));
                }
            }
            Node::Object(o) => {
                for (k, item) in v.as_object().iter().flat_map(|m| m.iter().rev()) {
                    let Some(f) = o.find(k).map(|at| &o.fields[at]) else {
                        continue;
                    };
                    if !(f.optional && item.is_null()) {
                        todo.push((f.ty, item, step(k)));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The places in a value of an operation that hold a `Content` or an
/// `Image` ([`crate::Operation::content_in_answer`],
/// [`crate::Operation::content_in_args`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentPlace {
    /// A JSON Pointer to the handle or the reference.
    pub at: String,
    /// The type there is `Image`, not `Content`.
    pub image: bool,
}

impl crate::model::Operation<'_> {
    /// Whether the operation's parameters or result hold a `Content` or an
    /// `Image`, written there or in an interface reached from there.
    #[must_use]
    pub fn holds_content(&self) -> bool {
        let (params, result) = &self.def.index.plan.ops[self.at];
        params
            .iter()
            .chain([result])
            .any(|&node| self.def.index.reaches[node as usize] & CONTENT != 0)
    }

    /// The places in `v`, an answer of this operation as its provider
    /// sent it, that hold a reference to an upload (`{"id", "meta"?}`):
    /// what Clax fills in from its store before the script is given the
    /// result. Each is a JSON Pointer into `v` (`""` for the answer
    /// itself), read as [`Kind::instruction_places`] reads a request,
    /// in the order the value is written out, with whether its type is
    /// `Image`.
    ///
    /// # Errors
    /// What [`crate::Operation::check_answer`] refuses `v` for, or that
    /// reading it would take more work than the definition's budget
    /// ([`ValueErrorKind::TooLarge`]).
    pub fn content_in_answer(&self, v: &Value) -> Result<Vec<ContentPlace>, ValueError> {
        let root = self.def.index.plan.ops[self.at].1;
        let what = format!("clax.{}: the answer", self.name);
        // The answer's check, as `check_answer` makes it, keeping the
        // alternatives it read the value through for the walk.
        let choices = check_choosing(self.def, root, v, false, &what)?;
        if self.def.index.reaches[root as usize] & CONTENT == 0 {
            return Ok(Vec::new());
        }
        let mut left = self.def.limits.value_nodes;
        let mut out = Vec::new();
        places_in(
            self.def,
            Walk {
                root,
                v,
                at: String::new(),
                want: CONTENT,
                held: false,
                what: &what,
                choices: Some(&choices),
            },
            &mut left,
            &mut out,
        )?;
        Ok(out
            .into_iter()
            .map(|(at, image)| ContentPlace { at, image })
            .collect())
    }

    /// The places in `args`, a call's arguments as
    /// [`crate::Operation::check_args`] answered them (one for each
    /// parameter), that hold a whole handle a script passed: each a JSON
    /// Pointer into the arguments as an array (`/0`, `/1/shot`), in
    /// order, with whether its type is `Image`.
    ///
    /// # Errors
    /// That reading them would take more work than the definition's
    /// budget ([`ValueErrorKind::TooLarge`]).
    pub fn content_in_args(&self, args: &[Value]) -> Result<Vec<ContentPlace>, ValueError> {
        let roots = &self.def.index.plan.ops[self.at].0;
        let what = format!("clax.{}: the arguments", self.name);
        let mut left = self.def.limits.value_nodes;
        let mut out = Vec::new();
        for (i, (&root, v)) in roots.iter().zip(args).enumerate() {
            if v.is_null() || self.def.index.reaches[root as usize] & CONTENT == 0 {
                continue;
            }
            places_in(
                self.def,
                Walk {
                    root,
                    v,
                    at: format!("/{i}"),
                    want: CONTENT,
                    held: true,
                    what: &what,
                    choices: None,
                },
                &mut left,
                &mut out,
            )?;
        }
        Ok(out
            .into_iter()
            .map(|(at, image)| ContentPlace { at, image })
            .collect())
    }
}
