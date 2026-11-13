//! A definition's types as a check of a value walks them: one node for
//! each type written in the definition, named by its place in a list,
//! with what can be worked out once worked out when the definition is
//! built. An object type knows its fields by name; a union knows, for
//! each form a value can take, which of its alternatives the value can
//! be, and which fields tell its object alternatives apart.
//!
//! The nodes of a definition are part of it ([`Plan`]). The crate's
//! tests build a type that is not one of the definition's own beside
//! them, for one check (`beside`).
//!
//! Building either is counted in steps, and stops when it has taken
//! more than it was allowed ([`crate::Limits::plan_steps`]). A step
//! keeps a bounded number of bytes, so the count bounds what a plan
//! holds: a node is 24 bytes, and an object type or a union 100 or so
//! more; a field is its name again and 30 bytes; and what tells a
//! union's alternatives apart ([`Tells`]) is some 100 bytes for each
//! name, 24 for each type a name has, 32 for each literal, and one set
//! of the union's alternatives (a bit each) for each of those that
//! more than one alternative shares. Unions that list the same object
//! alternatives share one [`Tells`], and a literal is held once, in
//! its node.

use crate::model::{Index, Interface, Kind, Namespace, Ty, op_at};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A node, by its place: a definition's own come first, in
/// [`Plan::nodes`], and the first of those are its interfaces, in
/// declaration order.
pub(crate) type NodeId = u32;

/// How many words hold `bits` bits.
pub(crate) fn words(bits: usize) -> usize {
    bits.div_ceil(64)
}

pub(crate) fn set_bit(bits: &mut [u64], i: usize) {
    bits[i / 64] |= 1 << (i % 64);
}

pub(crate) fn has_bit(bits: &[u64], i: usize) -> bool {
    bits[i / 64] & (1 << (i % 64)) != 0
}

/// The first bit set in `bits` at `from` or after it.
pub(crate) fn next_bit(bits: &[u64], from: usize) -> Option<usize> {
    let mut word = from / 64;
    let mut rest = *bits.get(word)? & (u64::MAX << (from % 64));
    loop {
        if rest != 0 {
            return Some(word * 64 + rest.trailing_zeros() as usize);
        }
        word += 1;
        rest = *bits.get(word)?;
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Unknown,
    Str,
    Instruction,
    Num,
    Int,
    Bool,
    Null,
    Content,
    Image,
    StrLit(Box<str>),
    BoolLit(bool),
    /// An interface, by its place among the definition's: the node at
    /// that place is its [`Object`].
    Named(u32),
    Array(NodeId),
    Record(NodeId),
    Object(Box<Object>),
    Union(Box<Union>),
    /// A type no value has: a name no interface answers to. The
    /// definition's checks refuse one before a plan is built.
    Never,
}

/// An object type, or an interface with the fields it inherits.
#[derive(Clone, Debug)]
pub(crate) struct Object {
    /// In declaration order, inherited ones first.
    pub fields: Vec<FieldNode>,
    /// The places of `fields`, in the order of their names.
    by_name: Vec<u32>,
    /// One bit a field: set for a required one.
    pub required: Vec<u64>,
    /// The places of the required fields of literal type, which tell
    /// this type from the other alternatives of a union.
    tells: Vec<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct FieldNode {
    pub name: Box<str>,
    pub optional: bool,
    pub ty: NodeId,
}

impl Object {
    /// The place of the field named `name`: a search among at most
    /// `Limits::list` names.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name
            .binary_search_by(|&at| (*self.fields[at as usize].name).cmp(name))
            .ok()
            .map(|i| self.by_name[i] as usize)
    }
}

/// A union, with which of its alternatives a value of each form can be.
#[derive(Clone, Debug)]
pub(crate) struct Union {
    /// Every alternative, as written.
    pub alts: Vec<NodeId>,
    /// `unknown` is one: every value fits.
    pub any: bool,
    pub null: bool,
    /// `true` fits, and `false` does.
    pub yes: bool,
    pub no: bool,
    /// Every number fits; a whole number does.
    pub number: bool,
    pub int: bool,
    /// Every string fits.
    pub string: bool,
    /// The string literals, in order.
    pub strings: Vec<Box<str>>,
    /// Every alternative is a literal; every one is a string literal.
    pub literal: bool,
    pub only_strings: bool,
    /// The alternatives an array can be, as written.
    pub arrays: Vec<NodeId>,
    /// The alternatives an object can be, as written.
    pub objects: Vec<NodeId>,
    /// What tells `objects` apart, when anything does: one for every
    /// union of the same object alternatives in the same order.
    pub tells: Option<Arc<Tells>>,
}

/// What tells the object alternatives of a union apart: the names
/// under which some of them have a required field of literal type, and
/// the sets of alternatives those names speak of.
#[derive(Debug, Default)]
pub(crate) struct Tells {
    pub tells: Vec<Tell>,
    /// The sets of more than one alternative, one after another: each
    /// has one bit for each of the union's `objects`.
    sets: Vec<u64>,
}

/// Some of a union's object alternatives.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Among {
    /// This one, by its place among the union's `objects`.
    One(usize),
    /// Several: the set that starts at this word of [`Tells`]' sets.
    Set(usize),
}

/// A name under which some object alternatives of a union have a
/// required field of literal type (`kind`): an object in which it is
/// missing is none of them, and what it holds there says which of them
/// the object can be.
#[derive(Debug)]
pub(crate) struct Tell {
    /// One alternative that has the field (an [`Object`]'s node), and
    /// the field's place in it: where its name is.
    pub object: NodeId,
    pub field: u32,
    /// The alternatives that have the field.
    pub alts: Among,
    /// The alternatives that have it, by its type there: those that
    /// inherit one declaration share one type.
    pub types: Vec<(NodeId, Among)>,
    /// For each literal one of `types` takes, in order, the
    /// alternatives it is one of: there when the types take few
    /// literals each, which is when it costs no more to build than
    /// `types` did.
    pub by_literal: Option<Vec<(Lit, Among)>>,
}

/// A literal a telling field can hold, by where the definition has it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Lit {
    Bool(bool),
    /// The string literal at this node; or, when the node is a union of
    /// literals, the one at this place among its `strings`.
    Str(NodeId, u32),
}

impl Tells {
    /// Word `n` of the set `among` is.
    pub fn word(&self, among: Among, n: usize) -> u64 {
        match among {
            Among::One(alt) if alt / 64 == n => 1 << (alt % 64),
            Among::One(_) => 0,
            Among::Set(at) => self.sets[at + n],
        }
    }

    /// Where the set `among` is starts, after making it one: a set of
    /// its own, when it was one alternative.
    fn set_of(sets: &mut Vec<u64>, width: usize, among: &mut Among) -> usize {
        match *among {
            Among::One(only) => {
                let at = sets.len();
                sets.resize(at + width, 0);
                set_bit(&mut sets[at..], only);
                *among = Among::Set(at);
                at
            }
            Among::Set(at) => at,
        }
    }

    /// Adds alternative `alt` to `among`, which is no set that anything
    /// else names.
    fn add(sets: &mut Vec<u64>, width: usize, among: &mut Among, alt: usize) {
        if !matches!(*among, Among::One(only) if only == alt) {
            let at = Tells::set_of(sets, width, among);
            set_bit(&mut sets[at..], alt);
        }
    }

    /// Adds the alternatives of `more` to `among`, which is no set
    /// that anything else names.
    fn join(sets: &mut Vec<u64>, width: usize, among: &mut Among, more: Among) {
        match more {
            Among::One(alt) => Tells::add(sets, width, among, alt),
            Among::Set(from) => {
                let into = Tells::set_of(sets, width, among);
                for n in 0..width {
                    sets[into + n] |= sets[from + n];
                }
            }
        }
    }

    /// The bytes this holds, counting each list at its length.
    #[cfg(test)]
    fn bytes(&self) -> usize {
        use std::mem::size_of;
        let tells: usize = self
            .tells
            .iter()
            .map(|t| {
                size_of::<Tell>()
                    + t.types.len() * size_of::<(NodeId, Among)>()
                    + t.by_literal
                        .as_ref()
                        .map_or(0, |l| l.len() * size_of::<(Lit, Among)>())
            })
            .sum();
        size_of::<Tells>() + tells + self.sets.len() * 8
    }
}

impl Union {
    /// Whether the string `s` fits one of the alternatives.
    pub fn takes_str(&self, s: &str) -> bool {
        self.any || self.string || self.strings.binary_search_by(|l| (**l).cmp(s)).is_ok()
    }

    pub fn takes_bool(&self, b: bool) -> bool {
        self.any || if b { self.yes } else { self.no }
    }
}

/// The nodes of a definition's types.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub nodes: Vec<Node>,
    /// For each operation, in the order the definition lists them: its
    /// parameters' types, and its result's.
    pub ops: Vec<(Vec<NodeId>, NodeId)>,
    /// For each kind: its request's type, its message's and its
    /// result's.
    pub kinds: Vec<(NodeId, Option<NodeId>, NodeId)>,
    /// How many steps building it took ([`build`]).
    pub steps: usize,
}

impl Plan {
    /// The bytes the plan holds, counting each list at its length and
    /// each [`Tells`] once.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        use std::mem::size_of;
        let mut seen = std::collections::BTreeSet::new();
        let nodes: usize = self
            .nodes
            .iter()
            .map(|node| {
                size_of::<Node>()
                    + match node {
                        Node::StrLit(s) => s.len(),
                        Node::Object(o) => {
                            size_of::<Object>()
                                + o.fields
                                    .iter()
                                    .map(|f| size_of::<FieldNode>() + f.name.len())
                                    .sum::<usize>()
                                + o.by_name.len() * 4
                                + o.required.len() * 8
                                + o.tells.len() * 4
                        }
                        Node::Union(u) => {
                            size_of::<Union>()
                                + (u.alts.len() + u.arrays.len() + u.objects.len()) * 4
                                + u.strings.iter().map(|s| 16 + s.len()).sum::<usize>()
                                + u.tells
                                    .as_ref()
                                    .filter(|t| seen.insert(Arc::as_ptr(t)))
                                    .map_or(0, |t| t.bytes())
                        }
                        _ => 0,
                    }
            })
            .sum();
        let ops: usize = self.ops.iter().map(|(p, _)| 32 + p.len() * 4).sum();
        nodes + ops + self.kinds.len() * 16
    }
}

struct Builder<'a> {
    by_name: &'a BTreeMap<String, usize>,
    /// The nodes that are there already, which the new ones follow.
    fixed: &'a [Node],
    nodes: Vec<Node>,
    /// The unions among `nodes`, whose `tells` are worked out last.
    unions: Vec<NodeId>,
    /// What tells apart the object alternatives worked out so far, by
    /// the object each alternative is (`NodeId::MAX` for one that is
    /// none).
    shared: BTreeMap<Vec<NodeId>, Option<Arc<Tells>>>,
    steps: usize,
    /// The most steps to take: past it, nothing more is worked out.
    limit: usize,
}

impl Builder<'_> {
    fn node(&self, id: NodeId) -> &Node {
        let at = id as usize;
        match at.checked_sub(self.fixed.len()) {
            None => &self.fixed[at],
            Some(mine) => &self.nodes[mine],
        }
    }

    fn push(&mut self, node: Node) -> NodeId {
        self.steps += 1;
        self.nodes.push(node);
        (self.fixed.len() + self.nodes.len() - 1) as NodeId
    }

    fn is_literal(&self, id: NodeId) -> bool {
        match self.node(id) {
            Node::StrLit(_) | Node::BoolLit(_) => true,
            Node::Union(u) => u.literal,
            _ => false,
        }
    }

    /// The node of `t`, after those of the types inside it. It recurses
    /// as deep as `t` nests, which the definition's checks have bounded.
    fn ty(&mut self, t: &Ty) -> NodeId {
        let node = match t {
            Ty::Str => Node::Str,
            Ty::Num => Node::Num,
            Ty::Int => Node::Int,
            Ty::Bool => Node::Bool,
            Ty::Null => Node::Null,
            Ty::Unknown => Node::Unknown,
            Ty::Instruction => Node::Instruction,
            Ty::Content => Node::Content,
            Ty::Image => Node::Image,
            Ty::StrLit(s) => Node::StrLit(s.as_str().into()),
            Ty::BoolLit(b) => Node::BoolLit(*b),
            Ty::Named(n) => match self.by_name.get(n) {
                Some(&i) => Node::Named(i as u32),
                None => Node::Never,
            },
            Ty::Array(x) => Node::Array(self.ty(x)),
            Ty::Record(x) => Node::Record(self.ty(x)),
            Ty::Object(fields) => {
                let fields = fields
                    .iter()
                    .map(|f| FieldNode {
                        name: f.name.as_str().into(),
                        optional: f.optional,
                        ty: self.ty(&f.ty),
                    })
                    .collect();
                Node::Object(Box::new(self.object(fields)))
            }
            Ty::Union(alts) => {
                let alts: Vec<NodeId> = alts.iter().map(|a| self.ty(a)).collect();
                let id = self.push(Node::Union(Box::new(self.union(alts))));
                self.unions.push(id);
                return id;
            }
        };
        self.push(node)
    }

    fn object(&self, fields: Vec<FieldNode>) -> Object {
        let mut by_name: Vec<u32> = (0..fields.len() as u32).collect();
        by_name.sort_by(|&a, &b| fields[a as usize].name.cmp(&fields[b as usize].name));
        let mut required = vec![0; words(fields.len())];
        let mut tells = Vec::new();
        for (at, f) in fields.iter().enumerate() {
            if !f.optional {
                set_bit(&mut required, at);
                if self.is_literal(f.ty) {
                    tells.push(at as u32);
                }
            }
        }
        Object {
            fields,
            by_name,
            required,
            tells,
        }
    }

    /// The union of `alts`, without its `tells`.
    fn union(&self, alts: Vec<NodeId>) -> Union {
        let mut u = Union {
            alts: Vec::new(),
            any: false,
            null: false,
            yes: false,
            no: false,
            number: false,
            int: false,
            string: false,
            strings: Vec::new(),
            literal: true,
            only_strings: true,
            arrays: Vec::new(),
            objects: Vec::new(),
            tells: None,
        };
        for &alt in &alts {
            let node = self.node(alt);
            u.literal &= matches!(node, Node::StrLit(_) | Node::BoolLit(_));
            u.only_strings &= matches!(node, Node::StrLit(_));
            match node {
                Node::Unknown => u.any = true,
                Node::Null => u.null = true,
                Node::Bool => (u.yes, u.no) = (true, true),
                Node::BoolLit(true) => u.yes = true,
                Node::BoolLit(false) => u.no = true,
                Node::Num => u.number = true,
                Node::Int => u.int = true,
                Node::Str | Node::Instruction => u.string = true,
                Node::StrLit(s) => u.strings.push(s.clone()),
                Node::Array(_) => u.arrays.push(alt),
                Node::Record(_)
                | Node::Content
                | Node::Image
                | Node::Object(_)
                | Node::Named(_) => u.objects.push(alt),
                // A union is flat, and nothing is a `Never`.
                Node::Union(_) | Node::Never => {}
            }
        }
        u.strings.sort();
        u.strings.dedup();
        u.alts = alts;
        u
    }

    /// The object an alternative of a union is, if it is one with
    /// fields: its node, and itself.
    fn object_of(&self, alt: NodeId) -> Option<(NodeId, &Object)> {
        let at = match self.node(alt) {
            Node::Named(i) => *i,
            Node::Object(_) => alt,
            _ => return None,
        };
        match self.node(at) {
            Node::Object(o) => Some((at, o)),
            _ => None,
        }
    }

    /// The literals the type at `id` takes, when it is a literal or a
    /// union of them: each as it is ordered, and as it is kept.
    fn literals(&self, id: NodeId) -> Vec<(Key<'_>, Lit)> {
        match self.node(id) {
            Node::StrLit(s) => vec![(Key::Str(s), Lit::Str(id, 0))],
            Node::BoolLit(b) => vec![(Key::Bool(*b), Lit::Bool(*b))],
            Node::Union(u) => {
                let strings = u.strings.iter().zip(0..);
                let mut all: Vec<(Key<'_>, Lit)> = strings
                    .map(|(s, at)| (Key::Str(s), Lit::Str(id, at)))
                    .collect();
                all.extend(u.no.then_some((Key::Bool(false), Lit::Bool(false))));
                all.extend(u.yes.then_some((Key::Bool(true), Lit::Bool(true))));
                all
            }
            _ => Vec::new(),
        }
    }

    /// The object each object alternative of the union at `id` is, by
    /// its node: what tells them apart depends on nothing else.
    fn objects_of(&self, id: NodeId) -> Vec<NodeId> {
        let Node::Union(u) = self.node(id) else {
            return Vec::new();
        };
        let object = |&alt| self.object_of(alt).map_or(NodeId::MAX, |(at, _)| at);
        u.objects.iter().map(object).collect()
    }

    /// What tells the object alternatives of the union at `id` apart,
    /// and how many steps working it out took: the names of their
    /// required fields of literal type, and under each name the types
    /// the field has, each once however many alternatives inherit it.
    fn tells(&self, id: NodeId) -> (Tells, usize) {
        let Node::Union(u) = self.node(id) else {
            return (Tells::default(), 0);
        };
        let width = words(u.objects.len());
        let mut tells: Vec<Tell> = Vec::new();
        let mut sets: Vec<u64> = Vec::new();
        let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
        let mut by_type: BTreeMap<(usize, NodeId), usize> = BTreeMap::new();
        let mut steps = 0;
        for (n, &alt) in u.objects.iter().enumerate() {
            let Some((object, o)) = self.object_of(alt) else {
                continue;
            };
            for &field in &o.tells {
                steps += 1;
                let f = &o.fields[field as usize];
                let Some(&at) = by_name.get(&*f.name) else {
                    by_name.insert(&f.name, tells.len());
                    by_type.insert((tells.len(), f.ty), 0);
                    tells.push(Tell {
                        object,
                        field,
                        alts: Among::One(n),
                        types: vec![(f.ty, Among::One(n))],
                        by_literal: None,
                    });
                    continue;
                };
                let tell = &mut tells[at];
                Tells::add(&mut sets, width, &mut tell.alts, n);
                match by_type.get(&(at, f.ty)) {
                    Some(&ty) => Tells::add(&mut sets, width, &mut tell.types[ty].1, n),
                    None => {
                        by_type.insert((at, f.ty), tell.types.len());
                        tell.types.push((f.ty, Among::One(n)));
                    }
                }
            }
        }
        for tell in &mut tells {
            // A table of every literal, when that is no more work
            // than a few steps for each type.
            let literals: Vec<_> = tell
                .types
                .iter()
                .map(|(ty, _)| self.literals(*ty))
                .collect();
            let count: usize = literals.iter().map(Vec::len).sum();
            steps += tell.types.len();
            if count > 4 * tell.types.len() {
                continue;
            }
            steps += count;
            // Each literal with the alternatives it is one of, and
            // whether that set is the table's own to add to: a literal
            // one type takes has that type's alternatives, as they are.
            let mut table: BTreeMap<Key<'_>, (Lit, Among, bool)> = BTreeMap::new();
            for (lits, (_, alts)) in literals.into_iter().zip(&tell.types) {
                for (key, lit) in lits {
                    let Some((_, among, own)) = table.get_mut(&key) else {
                        table.insert(key, (lit, *alts, false));
                        continue;
                    };
                    if let (Among::Set(at), false) = (*among, *own) {
                        let copy = sets.len();
                        sets.extend_from_within(at..at + width);
                        *among = Among::Set(copy);
                    }
                    *own = true;
                    Tells::join(&mut sets, width, among, *alts);
                }
            }
            let table = table.into_values();
            tell.by_literal = Some(table.map(|(lit, among, _)| (lit, among)).collect());
        }
        tells.iter_mut().for_each(|t| t.types.shrink_to_fit());
        tells.shrink_to_fit();
        sets.shrink_to_fit();
        (Tells { tells, sets }, steps)
    }

    /// Works out what tells each new union's object alternatives apart,
    /// once for the unions that list the same ones, until the steps
    /// are past their limit.
    fn finish(&mut self) {
        for at in 0..self.unions.len() {
            if self.steps > self.limit {
                return;
            }
            let id = self.unions[at];
            let objects = self.objects_of(id);
            self.steps += objects.len();
            let tells = match self.shared.get(&objects) {
                Some(tells) => tells.clone(),
                None => {
                    let (tells, steps) = self.tells(id);
                    self.steps += steps;
                    let tells = (!tells.tells.is_empty()).then(|| Arc::new(tells));
                    self.shared.insert(objects, tells.clone());
                    tells
                }
            };
            let mine = id as usize - self.fixed.len();
            if let Node::Union(u) = &mut self.nodes[mine] {
                u.tells = tells;
            }
        }
    }
}

/// A literal, as a table of them is ordered: `false`, `true`, then the
/// strings.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key<'a> {
    Bool(bool),
    Str(&'a str),
}

/// The plan of a definition whose parts have passed the checks of
/// their names, their lists and their types. It counts its steps
/// ([`Plan::steps`]): one for each node, one for each interface and
/// each of its fields with those it inherits, one for each object alternative
/// of each union, and for each list of object alternatives that unions
/// have, once however many have it, one for each telling field of each
/// alternative, one for each type such a field has, and one for each
/// literal put in a table. Past `limit` steps it works out no more of
/// what tells unions apart, and the plan is not one to check with: a
/// count above `limit` says so.
pub(crate) fn build(
    root: &Namespace,
    interfaces: &[Interface],
    kinds: &[Kind],
    index: &Index,
    limit: usize,
) -> Plan {
    let mut b = Builder {
        by_name: &index.interface_by_name,
        fixed: &[],
        // The interfaces' places, filled in once their fields' types
        // have nodes.
        nodes: vec![Node::Never; interfaces.len()],
        unions: Vec::new(),
        shared: BTreeMap::new(),
        steps: 0,
        limit,
    };
    let declared: Vec<Vec<NodeId>> = interfaces
        .iter()
        .map(|i| i.fields.iter().map(|f| b.ty(&f.ty)).collect())
        .collect();
    for (i, all) in index.fields.iter().enumerate() {
        let fields = all
            .iter()
            .map(|&(owner, k)| {
                let f = &interfaces[owner].fields[k];
                FieldNode {
                    name: f.name.as_str().into(),
                    optional: f.optional,
                    ty: declared[owner][k],
                }
            })
            .collect();
        b.steps += 1 + all.len();
        b.nodes[i] = Node::Object(Box::new(b.object(fields)));
    }
    let ops = index
        .ops
        .iter()
        .filter_map(|(_, path)| op_at(root, path))
        .map(|op| {
            let params = op.params.iter().map(|p| b.ty(&p.ty)).collect();
            (params, b.ty(&op.result))
        })
        .collect();
    let kinds = kinds
        .iter()
        .map(|k| {
            (
                b.ty(&k.request),
                k.message.as_ref().map(|m| b.ty(m)),
                b.ty(&k.result),
            )
        })
        .collect();
    b.finish();
    b.nodes.shrink_to_fit();
    Plan {
        nodes: b.nodes,
        ops,
        kinds,
        steps: b.steps,
    }
}

/// The nodes of `t`, a type that is not one of the definition's own,
/// numbered after the definition's (`fixed`), the node of `t` itself,
/// and how many steps building them took, counted and stopped as
/// [`build`]'s are. `t` nests no deeper than the definition's limits
/// allow.
#[cfg(test)]
pub(crate) fn beside(
    fixed: &[Node],
    by_name: &BTreeMap<String, usize>,
    t: &Ty,
    limit: usize,
) -> (Vec<Node>, NodeId, usize) {
    let mut b = Builder {
        by_name,
        fixed,
        nodes: Vec::new(),
        unions: Vec::new(),
        shared: BTreeMap::new(),
        steps: 0,
        limit,
    };
    let root = b.ty(t);
    b.finish();
    (b.nodes, root, b.steps)
}
