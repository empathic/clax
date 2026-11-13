//! The bounds a definition is read and used under.

/// How large a definition, and a value checked against it, may be. A
/// reader of either notation takes these and a [`crate::Definition`]
/// keeps the ones it was read under, so every loop and every recursion
/// over a definition or a value has a bound, whoever wrote the input.
///
/// The numbers are the environment's to choose; [`Limits::default`] gives
/// the design's (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
/// "Limits") and, where the design names none, figures several times
/// what its example files use. Two of them have a hard maximum, which
/// holds whatever is asked for: `depth` ([`Limits::MAX_DEPTH`]) and
/// `value_depth` ([`Limits::MAX_VALUE_DEPTH`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The bytes of a definition's text as a `.d.ts` file, checked
    /// before any of it is read. Default 64 KiB.
    pub file_bytes: usize,
    /// The bytes of a definition's text as a manifest, checked before
    /// any of it is read. A manifest says in more bytes what a `.d.ts`
    /// file says: about two and a half times as many for the surfaces
    /// in this repository, laid out as [`crate::print_manifest`] lays
    /// one out. Default 256 KiB, four times `file_bytes`.
    pub manifest_bytes: usize,
    /// The values of a manifest: its numbers, strings, `true`s,
    /// `false`s and `null`s, its arrays and its objects, counted as
    /// they are read. A manifest's reader keeps each as it reads, so
    /// this bounds what it holds where a text of nothing but short
    /// values would be many of them. A manifest that reads has no more
    /// than one value for each four of its bytes (all but a few of the
    /// values of a list are names and literals, each at least a
    /// character between quotes and a comma). Default 65,536, a quarter
    /// of `manifest_bytes`.
    pub manifest_values: usize,
    /// How many mistakes of a manifest are said: the first of each
    /// entry that cannot be read, up to this many, and then that the
    /// rest was not looked at. At least 1. Default 64.
    pub manifest_errors: usize,
    /// The bytes of a surface's rules, and of a namespace's. Default
    /// 4 KiB.
    pub rules_bytes: usize,
    /// The bytes of any other doc comment: an operation's, a kind's, an
    /// interface's, a field's. Default 4 KiB.
    pub doc_bytes: usize,
    /// The bytes of any one name: a surface's, a namespace's, an
    /// operation's, a parameter's, an interface's, a field's, a kind's,
    /// an error code, and each name of an allow pattern. Default 64.
    pub name_bytes: usize,
    /// How deep namespaces nest in a surface, and how deep a type nests:
    /// an array's element, a `Record`'s value and an object type's field
    /// are each one level below their type (in the `.d.ts` notation,
    /// parentheses count too). Default 32, and never more than
    /// [`Limits::MAX_DEPTH`]: what reads, checks and prints a
    /// definition recurses this deep, at up to 8 KiB of stack a level
    /// in a build without optimisation.
    pub depth: usize,
    /// The operations of a surface. Default 256.
    pub operations: usize,
    /// The interfaces of a definition. Default 256.
    pub interfaces: usize,
    /// The kinds of a definition. Default 64.
    pub kinds: usize,
    /// The items of any one list: a namespace's members, an operation's
    /// parameters and its error codes, an object type's fields, an
    /// interface's fields with those it inherits, the interfaces one
    /// extends, a union's alternatives, a kind's allow patterns. Default
    /// 128.
    pub list: usize,
    /// How long a chain of `extends` may be. Default 16.
    pub extends_depth: usize,
    /// How deep a value checked against the definition may nest. Default
    /// 128, what `serde_json` reads from text, and never more than
    /// [`Limits::MAX_VALUE_DEPTH`]. A check keeps its place in a value
    /// on the heap, a few dozen bytes a level, so this bounds memory and
    /// not the stack.
    pub value_depth: usize,
    /// How many units of work one check of a value may do. One unit is
    /// one of: comparing one value (an array, an object, a string, a
    /// number and so on, not counting the values inside it) with one
    /// type; looking one key of an object up among a type's fields; or
    /// looking at what an object holds under one name that tells a
    /// union's alternatives apart (`kind`). A value that fits a type
    /// with no union in it costs one unit for each of its values and
    /// one for each key of its objects; a union whose alternatives a
    /// value does not tell apart is tried alternative by alternative,
    /// each at its own cost. It bounds the memory of a check as well:
    /// what a check remembers of the values it tries against several
    /// alternatives is one entry for each unit at most, about 21 MiB
    /// in the worst case found under the default. Default 1,000,000.
    pub value_nodes: usize,
    /// How many steps may be taken to put a definition's types in the
    /// form a check of a value walks, which is kept with the
    /// definition: one step for each type written in it, one for each
    /// field of each interface with those it inherits, one for each
    /// object alternative of each union, and, for each list of object
    /// alternatives that unions have, the steps of working out which of
    /// their required fields of literal type tell them apart. A step
    /// keeps a bounded number of bytes (about a hundred and a name, and
    /// `list` bits where a set of a union's alternatives is kept), so
    /// this bounds the memory a definition holds beyond its own text;
    /// a definition that needs more is refused. A type handed to
    /// [`crate::check_held`] or [`crate::check_sent`] is under the same
    /// limit. Default 100,000.
    pub plan_steps: usize,
}

impl Limits {
    /// The most `depth` can be: a larger number is read as this one.
    pub const MAX_DEPTH: usize = 64;
    /// The most `value_depth` can be: a larger number is read as this
    /// one.
    pub const MAX_VALUE_DEPTH: usize = 1024;
    /// How deep the arrays and objects of a manifest's JSON may nest
    /// under these limits: `4 × depth + 8`, with `depth` no more than
    /// [`Limits::MAX_DEPTH`] (136 by default, 264 at the most). It is
    /// the deepest a manifest of a definition within `depth` is written:
    /// a parameter's type sits 6 arrays and objects deep (the document,
    /// `operations`, the operation, `params`, the parameter, the type);
    /// each level a type nests costs 4 at the most, when it is a field
    /// of an object type that is an alternative of a union (`anyOf`,
    /// the alternative, `properties`, the field's type); and the
    /// innermost type may itself be a union (`anyOf` and an
    /// alternative: 2 more). So every definition has a manifest that a
    /// reader under its own limits takes, and a text nested deeper is
    /// refused as it is read.
    #[must_use]
    pub fn manifest_nesting(&self) -> usize {
        4 * self.depth.min(Self::MAX_DEPTH) + 8
    }

    /// These limits, with `depth` and `value_depth` no more than their
    /// hard maximums: what a definition keeps and is read under.
    #[must_use]
    pub fn clamped(&self) -> Limits {
        Limits {
            depth: self.depth.min(Self::MAX_DEPTH),
            value_depth: self.value_depth.min(Self::MAX_VALUE_DEPTH),
            ..self.clone()
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            file_bytes: 64 * 1024,
            manifest_bytes: 256 * 1024,
            manifest_values: 64 * 1024,
            manifest_errors: 64,
            rules_bytes: 4 * 1024,
            doc_bytes: 4 * 1024,
            name_bytes: 64,
            depth: 32,
            operations: 256,
            interfaces: 256,
            kinds: 64,
            list: 128,
            extends_depth: 16,
            value_depth: 128,
            value_nodes: 1_000_000,
            plan_steps: 100_000,
        }
    }
}
