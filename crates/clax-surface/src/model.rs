//! A surface definition: what both notations read into, and everything
//! else works from.

use crate::allow::Pattern;
use crate::limits::Limits;
use crate::plan::Plan;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Deref;

/// The type of a parameter, a result or a field.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ty {
    /// `string`.
    Str,
    /// `number`: any JSON number.
    Num,
    /// `Int`: a whole number. The model reads it as `number`.
    Int,
    /// `boolean`.
    Bool,
    /// `null`.
    Null,
    /// `unknown`: any JSON value.
    Unknown,
    /// `Instruction`: a string that is the run's person's own words.
    Instruction,
    /// `Content`: a handle to stored bytes.
    Content,
    /// `Image`: a `Content` that is an image.
    Image,
    /// A string literal: that string and no other.
    StrLit(String),
    /// `true` or `false`.
    BoolLit(bool),
    /// An interface of the definition, by name.
    Named(String),
    /// `T[]`.
    Array(Box<Ty>),
    /// `Record<string, T>`: an object of any keys, each value a `T`.
    Record(Box<Ty>),
    /// An object type written where it is used. A value with a field it
    /// does not list is refused.
    Object(Vec<Field>),
    /// `A | B` ([`Ty::union`] makes one).
    Union(Alts),
}

/// A union's alternatives: two or more, each once, none of them a union
/// itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Alts(Vec<Ty>);

impl Deref for Alts {
    type Target = [Ty];
    fn deref(&self) -> &[Ty] {
        &self.0
    }
}

impl Ty {
    /// The union of `first` and `rest`, flat: an alternative that is a
    /// union gives its own alternatives, and `first` alone is itself.
    #[must_use]
    pub fn union(first: Ty, rest: impl IntoIterator<Item = Ty>) -> Ty {
        let mut flat = Vec::new();
        for t in std::iter::once(first).chain(rest) {
            match t {
                Ty::Union(inner) => flat.extend(inner.0),
                other => flat.push(other),
            }
        }
        if flat.len() == 1 {
            flat.remove(0)
        } else {
            Ty::Union(Alts(flat))
        }
    }

    /// The types written directly inside this one: an array's element, a
    /// `Record`'s value, an object type's fields', a union's
    /// alternatives.
    pub(crate) fn parts(&self) -> impl Iterator<Item = &Ty> {
        let (one, fields, alts): (Option<&Ty>, &[Field], &[Ty]) = match self {
            Ty::Array(t) | Ty::Record(t) => (Some(t), &[], &[]),
            Ty::Object(fields) => (None, fields, &[]),
            Ty::Union(ts) => (None, &[], ts),
            _ => (None, &[], &[]),
        };
        one.into_iter()
            .chain(fields.iter().map(|f| &f.ty))
            .chain(alts)
    }

    /// Calls `f` with this type and every type inside it, not following
    /// interface names, without recursing: a type of any depth is safe
    /// to walk.
    pub(crate) fn each<'a>(&'a self, f: &mut dyn FnMut(&'a Ty)) {
        let mut todo = vec![self];
        while let Some(t) = todo.pop() {
            f(t);
            let from = todo.len();
            todo.extend(t.parts());
            todo[from..].reverse();
        }
    }

    /// Whether a value of this type may be `null`.
    pub(crate) fn admits_null(&self) -> bool {
        match self {
            Ty::Null => true,
            Ty::Union(ts) => ts.contains(&Ty::Null),
            _ => false,
        }
    }
}

/// One field of an interface or of an object type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Field {
    pub name: String,
    /// May be left out, or `null`.
    pub optional: bool,
    pub ty: Ty,
    /// Its doc comment, which the model reads. Only an interface's fields
    /// have one.
    pub doc: Option<String>,
}

/// One positional parameter of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// May be left out, or passed as `null`. Only trailing parameters are
    /// optional. A required parameter is never `null`, so its type does
    /// not admit it.
    pub optional: bool,
    pub ty: Ty,
}

/// What performing an operation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// `@read`: no effect.
    Read,
    /// `@write`: changes the provider's state.
    Write,
    /// `@effect`: the person sees it, or it leaves the machine.
    Effect,
    /// `@spend`: charged each time. Clax's own surfaces only.
    Spend,
}

impl Access {
    /// The tag that says it in a `.d.ts` file, without its `@`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
            Access::Effect => "effect",
            Access::Spend => "spend",
        }
    }
}

/// One operation of a surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    /// Its own name, the last segment of its dotted name.
    pub name: String,
    /// Its documentation, which the model reads.
    pub doc: String,
    pub access: Access,
    /// `@waits`: may take as long as a person does.
    pub waits: bool,
    /// `@once`: repeating it is not safe, so it is never sent again once a
    /// provider has accepted it. Never set on a read.
    pub once: bool,
    /// `@throws`: the error codes it may fail with, each once, in the
    /// order its tags give them.
    pub throws: Vec<String>,
    /// Its positional parameters, the optional ones last.
    pub params: Vec<Param>,
    pub result: Ty,
}

/// A member of a surface or of a namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    Op(Op),
    Namespace(Namespace),
}

impl Member {
    /// The member's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Member::Op(o) => &o.name,
            Member::Namespace(n) => &n.name,
        }
    }
}

/// A namespace: named operations and namespaces under one name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Namespace {
    pub name: String,
    /// Its doc comment: rules for runs that have one of its operations.
    pub rules: Option<String>,
    pub members: Vec<Member>,
}

/// A named object type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub doc: Option<String>,
    /// The interfaces whose fields it also has.
    pub extends: Vec<String>,
    /// Its own fields, without those it inherits
    /// ([`Definition::fields_of`] gives both).
    pub fields: Vec<Field>,
}

/// A request kind: a function from a typed request to a typed result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub name: String,
    /// The prompt's paragraph about the request.
    pub doc: String,
    /// `@allow`: the operations a run of this kind may have. `Some` of
    /// an empty list (`@allow` alone, `"allow": []`) allows none. `None`
    /// is a kind with no `@allow` at all, which only Clax's own surface
    /// may have ([`crate::Author::Clax`]) and which narrows nothing; a
    /// host's kind always has a list. Both notations keep the two apart,
    /// and [`Kind::allows`] reads it.
    pub allow: Option<Vec<Pattern>>,
    /// The type of the request a run starts with.
    pub request: Ty,
    /// The type of a message the run takes after it starts; `None` for a
    /// kind that takes none.
    pub message: Option<Ty>,
    /// The type of the value the run finishes with.
    pub result: Ty,
}

impl Kind {
    /// Whether the kind lets its runs have the operation `op`, by its
    /// dotted name: any operation when it has no `@allow` (a kind of
    /// Clax's own surface), none when its list is empty, else one that
    /// a pattern of its list names. What else narrows a run (the
    /// operator's list, the starter's) is asked of each of those
    /// patterns ([`Pattern::matches`]); [`Definition::run_operations`]
    /// asks all of them at once.
    #[must_use]
    pub fn allows(&self, op: &str) -> bool {
        self.allow
            .as_ref()
            .is_none_or(|patterns| patterns.iter().any(|p| p.matches(op)))
    }
}

/// The name of the surface whose operations sit directly on the `clax`
/// object; every other surface is one namespace of it.
pub const CLAX: &str = "clax";

/// A surface's definition, which has passed every check of the format
/// (the crate's documentation lists them) within its [`Limits`]: only a
/// reader of this crate makes one. Two are equal when they say the same,
/// whatever limits each was read under. It holds its types a second
/// time in the form a check of a value walks
/// ([`Limits::plan_steps`] bounds that form), so cloning one copies
/// both. As `{:?}` it shows what it says and its limits, not that
/// second form.
#[derive(Clone)]
pub struct Definition {
    pub(crate) root: Namespace,
    pub(crate) interfaces: Vec<Interface>,
    pub(crate) kinds: Vec<Kind>,
    pub(crate) index: Index,
    pub(crate) limits: Limits,
}

impl fmt::Debug for Definition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Definition")
            .field("root", &self.root)
            .field("interfaces", &self.interfaces)
            .field("kinds", &self.kinds)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Definition {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.interfaces == other.interfaces && self.kinds == other.kinds
    }
}

impl Eq for Definition {}

/// What is worked out once about a definition, when it is checked.
#[derive(Clone, Debug, Default)]
pub(crate) struct Index {
    /// Every operation in declaration order: its dotted name, and the
    /// position of each member on the way to it from the surface.
    pub ops: Vec<(String, Vec<usize>)>,
    pub op_by_name: BTreeMap<String, usize>,
    pub interface_by_name: BTreeMap<String, usize>,
    /// For each interface, every field a value of it has, as (the
    /// interface that declares it, its position there): inherited ones
    /// first.
    pub fields: Vec<Vec<(usize, usize)>>,
    /// The types as a check of a value walks them.
    pub plan: Plan,
    /// For each node of `plan`, what it holds at any depth, through
    /// interfaces ([`crate::run_ops::reaches`]).
    pub reaches: Vec<u8>,
}

/// One operation of a definition, with the name a script calls it by.
#[derive(Clone, Copy, Debug)]
pub struct Operation<'a> {
    /// Dotted, below the `clax` global: `files.read` in the surface named
    /// `clax`, `shell.settings.get` in the surface named `shell`.
    pub name: &'a str,
    pub op: &'a Op,
    pub(crate) def: &'a Definition,
    /// Its place among the definition's operations.
    pub(crate) at: usize,
}

/// The operation at `path` below `root`: the position of each member
/// on the way to it.
pub(crate) fn op_at<'a>(root: &'a Namespace, path: &[usize]) -> Option<&'a Op> {
    let mut members = &root.members;
    let mut found = None;
    for &at in path {
        match members.get(at)? {
            Member::Namespace(ns) => members = &ns.members,
            Member::Op(op) => found = Some(op),
        }
    }
    found
}

impl Definition {
    /// The surface's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.root.name
    }

    /// The surface's rules: the doc comment of its `declare const`.
    #[must_use]
    pub fn rules(&self) -> Option<&str> {
        self.root.rules.as_deref()
    }

    /// The surface's operations and namespaces, in declaration order.
    #[must_use]
    pub fn members(&self) -> &[Member] {
        &self.root.members
    }

    /// The interfaces, in declaration order.
    #[must_use]
    pub fn interfaces(&self) -> &[Interface] {
        &self.interfaces
    }

    /// The request kinds, in declaration order.
    #[must_use]
    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }

    /// The limits the definition was read under, which also bound a
    /// check of a value against it: `depth` and `value_depth` are no
    /// more than their hard maximums, whatever was asked for
    /// ([`Limits::clamped`]).
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The interface named `name`.
    #[must_use]
    pub fn interface(&self, name: &str) -> Option<&Interface> {
        self.index
            .interface_by_name
            .get(name)
            .map(|&i| &self.interfaces[i])
    }

    /// The kind named `name`.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<&Kind> {
        self.kinds.iter().find(|k| k.name == name)
    }

    /// The names this surface puts on the `clax` object: its top-level
    /// members' for the surface named `clax`, its own name otherwise.
    #[must_use]
    pub fn top_level_names(&self) -> Vec<&str> {
        if self.name() == CLAX {
            self.root.members.iter().map(Member::name).collect()
        } else {
            vec![self.name()]
        }
    }

    fn operation_at(&self, n: usize) -> Operation<'_> {
        let (name, path) = &self.index.ops[n];
        Operation {
            name,
            op: op_at(&self.root, path).expect("the index names an operation"),
            def: self,
            at: n,
        }
    }

    /// Every operation, in declaration order (a namespace's where the
    /// namespace is declared).
    pub fn operations(&self) -> impl ExactSizeIterator<Item = Operation<'_>> {
        (0..self.index.ops.len()).map(|n| self.operation_at(n))
    }

    /// The operation with the dotted name `name` (see [`Operation::name`]).
    #[must_use]
    pub fn operation(&self, name: &str) -> Option<Operation<'_>> {
        self.index
            .op_by_name
            .get(name)
            .map(|&n| self.operation_at(n))
    }

    /// Every field a value of the interface named `name` has: those of
    /// the interfaces it extends, in the order it names them, then its
    /// own. `None` when the definition has no such interface.
    #[must_use]
    pub fn fields_of(&self, name: &str) -> Option<Vec<&Field>> {
        let &i = self.index.interface_by_name.get(name)?;
        Some(
            self.index.fields[i]
                .iter()
                .map(|&(owner, k)| &self.interfaces[owner].fields[k])
                .collect(),
        )
    }

    /// Which interfaces `roots` reach, through fields and `extends`: one
    /// flag for each interface, in declaration order.
    pub(crate) fn reach<'a>(&'a self, roots: impl Iterator<Item = &'a Ty>) -> Vec<bool> {
        let index = &self.index.interface_by_name;
        let note = |t: &'a Ty, todo: &mut Vec<usize>| {
            t.each(&mut |x| {
                if let Ty::Named(n) = x
                    && let Some(&i) = index.get(n.as_str())
                {
                    todo.push(i);
                }
            });
        };
        let mut todo = Vec::new();
        roots.for_each(|t| note(t, &mut todo));
        let mut seen = vec![false; self.interfaces.len()];
        while let Some(i) = todo.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let interface = &self.interfaces[i];
            interface.fields.iter().for_each(|f| note(&f.ty, &mut todo));
            todo.extend(
                interface
                    .extends
                    .iter()
                    .filter_map(|e| index.get(e.as_str())),
            );
        }
        seen
    }
}

impl Op {
    /// The types of its parameters, then of its result.
    pub(crate) fn types(&self) -> impl Iterator<Item = &Ty> {
        self.params.iter().map(|p| &p.ty).chain([&self.result])
    }
}
