//! What a run of a kind is given (the design's "A run"): the operations
//! it may call, and the places in its request that hold the person's
//! own words or content. Each is a pure function of a definition and,
//! for the places, of a request that passes the kind's check.

use crate::allow::Pattern;
use crate::check::shown;
use crate::model::{Definition, Kind};
use crate::plan::{Node, NodeId};
use crate::value::{ValueError, ValueErrorKind, fits_sent};
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
    /// written there or in an interface reached from there, and Clax
    /// stores no content yet.
    Content,
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
            let reason = if !k.allows(o.name) {
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
        let elsewhere = k
            .allow
            .iter()
            .flatten()
            .filter(|p| !p.names_any(&[self]))
            .collect();
        let request = reaches[self.index.plan.kinds[n].0 as usize];
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
    /// It does no more work than the check of the value
    /// ([`crate::Limits::value_nodes`]), and once more again: a unit
    /// for each value it looks at, and what checking a value against
    /// a union's alternatives costs, where the value is an array or an
    /// object under a union that holds an `Instruction`.
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
        places(def, self, v, INSTRUCTION)
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
        places(def, self, v, CONTENT)
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

/// The places in `v`, a request of `kind`, of the values whose type
/// holds `want` itself.
fn places(def: &Definition, kind: &Kind, v: &Value, want: u8) -> Result<Vec<String>, ValueError> {
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
    kind.check_request(def, v)?;
    let what = format!("kind {name}: the request");
    let nodes = &def.index.plan.nodes;
    let reaches = &def.index.reaches;
    let mut left = def.limits.value_nodes;
    let too_large = || ValueError {
        kind: ValueErrorKind::TooLarge,
        message: format!(
            "{what} is too large to read: the limit on the work of one check is {}",
            def.limits.value_nodes
        ),
    };
    let mut out = Vec::new();
    let mut todo: Vec<(NodeId, &Value, String)> =
        vec![(def.index.plan.kinds[n].0, v, String::new())];
    'values: while let Some((mut id, v, at)) = todo.pop() {
        // The node the value is read as: through names and unions.
        loop {
            if reaches[id as usize] & want == 0 {
                continue 'values;
            }
            left = left.checked_sub(1).ok_or_else(too_large)?;
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
                    } else {
                        for &alt in alts {
                            if fits_sent(def, alt, v, &mut left, &what)? {
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
            Node::Instruction if want == INSTRUCTION && v.is_string() => out.push(at),
            Node::Content | Node::Image if want == CONTENT && v.is_object() => out.push(at),
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
    Ok(out)
}
