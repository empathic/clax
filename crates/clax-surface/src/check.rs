//! The checks every definition passes, whichever notation it was read
//! from. A reader hands [`build`] the parts it read; `build` checks them
//! and makes the [`Definition`], or says what is wrong and where, as a
//! [`Site`] the reader turns into a place in its own input.
//!
//! Nothing here trusts a reader to have refused anything: every rule of
//! the format that a definition can break is checked here, and the
//! checks that bound size and depth run first and do not recurse, so
//! what runs after them, here and in the rest of the crate, recurses
//! only as deep as [`Limits`] allow.

use crate::allow::{callable, identifier, property};
use crate::limits::Limits;
use crate::model::{
    Access, CLAX, Definition, Field, Index, Interface, Kind, Member, Namespace, Op, Ty, op_at,
};
use crate::plan;
use std::collections::{BTreeMap, BTreeSet};

/// The surfaces the core itself provides, beside `clax`.
const CORE_SURFACES: &[&str] = &["runs", "content"];

/// The names no surface may take, Clax's own neither: what a prompt
/// file's slots of the core's begin with (`{core.data}`,
/// `{rules.web}`), which a surface's own slots, named
/// `<surface>.<name>`, must not be taken for.
const PROMPT_PREFIXES: &[&str] = &["core", "rules"];

/// The names a run's steps keep fields under, which a host's surface
/// may not take, in alphabetical order. A run's step keeps what a
/// surface other than `clax` adds to it under the surface's name,
/// beside the run loop's own fields and `clax`'s, so a surface named as
/// one of them could not add a field to any step. They are the run
/// loop's fields of a step's payload (`calls`, `code`, `error`,
/// `images`, `outcome`, `reason`, `result`, `summary`, `title`,
/// `tool_use_id`) and of what a model is told of a script (`calls`,
/// `error`, `images`, `logs`, `replay`, `value`), and the fields Clax's own surface adds (`base`,
/// `changed`, `created`, `notice`, `posted`, `published`, `replied`,
/// `sources`). The run loop holds this list equal to its own
/// (`clax-bot`, `surface.rs`).
pub const STEP_FIELDS: &[&str] = &[
    "base",
    "calls",
    "changed",
    "code",
    "created",
    "error",
    "images",
    "logs",
    "notice",
    "outcome",
    "posted",
    "published",
    "reason",
    "replay",
    "replied",
    "result",
    "sources",
    "summary",
    "title",
    "tool_use_id",
    "value",
];

/// The names a host's surface may not take: `clax`, the core's own
/// surfaces (`runs`, `content`), and every name Clax's surfaces put on
/// the `clax` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reserved(BTreeSet<String>);

impl Reserved {
    /// The reserved names, given Clax's own surfaces: the one named
    /// `clax`, whose namespaces a host may not shadow, and any other
    /// Clax provides.
    #[must_use]
    pub fn of(clax: &[&Definition]) -> Reserved {
        let mut names: BTreeSet<String> = CORE_SURFACES.iter().map(|s| (*s).to_string()).collect();
        names.insert(CLAX.to_string());
        for def in clax {
            names.insert(def.name().to_string());
            names.extend(def.top_level_names().into_iter().map(str::to_string));
        }
        Reserved(names)
    }

    /// Whether `name` is reserved.
    #[must_use]
    pub fn holds(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// The reserved names, in alphabetical order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

/// Whose surface a definition is.
#[derive(Clone, Copy, Debug)]
pub enum Author<'a> {
    /// Clax itself: its surface may be named anything, and may charge
    /// (`@spend`).
    Clax,
    /// A host. Its surface may not take a [`Reserved`] name, and may not
    /// use `@spend`.
    Host { reserved: &'a Reserved },
}

/// A declaration of a definition. Indexes count in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decl {
    /// The surface's own declaration: its name.
    Surface,
    /// The surface's rules.
    Rules,
    /// The surface's members, as one list.
    Members,
    /// A namespace, counting each where it is declared, before its
    /// members.
    Namespace(usize),
    /// A namespace's rules: the namespace, counted as [`Decl::Namespace`]
    /// counts them.
    NamespaceRules(usize),
    /// An operation, as [`Definition::operations`] lists them.
    Op(usize),
    /// A parameter of an operation: the operation, then the parameter.
    Param(usize, usize),
    /// An error code of an operation: the operation, then the code.
    Code(usize, usize),
    /// An operation's result.
    Result(usize),
    Interface(usize),
    /// One of the interfaces an interface extends: the interface, then
    /// its place in the list.
    Extends(usize, usize),
    /// A field of an interface: the interface, then the field.
    Field(usize, usize),
    Kind(usize),
    /// A kind's request, message and result.
    Request(usize),
    Message(usize),
    KindResult(usize),
}

/// Where in a definition a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    pub decl: Decl,
    /// Within the type of `decl`, the field of an object type written
    /// inline that the problem is at or inside: fields count from 0 in
    /// the order they are written, a field before the fields inside its
    /// own type.
    pub inner: Option<usize>,
}

impl From<Decl> for Site {
    fn from(decl: Decl) -> Site {
        Site { decl, inner: None }
    }
}

/// One step of reading a type in the order [`Site::inner`] numbers the
/// fields of the object types written inline in it ([`visit_inline`]).
pub(crate) enum Visit<'a> {
    /// A type, inside the inline field of this number, if any.
    Type(&'a Ty, Option<usize>),
    /// Field `k` of an object type written inline, whose fields are
    /// `siblings`, numbered `n`: before the types inside it.
    Field {
        n: usize,
        field: &'a Field,
        siblings: &'a [Field],
        k: usize,
    },
    /// Alternative `k` of the union `alts` and all inside it have been
    /// read; the union is inside the inline field of this number, if
    /// any.
    Alternative {
        alts: &'a [Ty],
        k: usize,
        inside: Option<usize>,
    },
}

/// Reads `t` in the order its text is written, and so the order the
/// readers keep the lines or pointers of its inline fields in: a type,
/// then what is inside it; an object type's fields in order, each
/// numbered before its own type is read; a union's alternatives in
/// order. It is the one numbering of [`Site::inner`]. It recurses as
/// deep as `t` nests, which [`shape`] has bounded.
pub(crate) fn visit_inline<'a, E>(
    t: &'a Ty,
    f: &mut dyn FnMut(Visit<'a>) -> Result<(), E>,
) -> Result<(), E> {
    fn go<'a, E>(
        t: &'a Ty,
        inside: Option<usize>,
        next: &mut usize,
        f: &mut dyn FnMut(Visit<'a>) -> Result<(), E>,
    ) -> Result<(), E> {
        f(Visit::Type(t, inside))?;
        match t {
            Ty::Array(x) | Ty::Record(x) => go(x, inside, next, f),
            Ty::Union(ts) => {
                for (k, x) in ts.iter().enumerate() {
                    go(x, inside, next, f)?;
                    f(Visit::Alternative {
                        alts: ts,
                        k,
                        inside,
                    })?;
                }
                Ok(())
            }
            Ty::Object(fields) => {
                for (k, field) in fields.iter().enumerate() {
                    let n = *next;
                    *next += 1;
                    f(Visit::Field {
                        n,
                        field,
                        siblings: fields,
                        k,
                    })?;
                    go(&field.ty, Some(n), next, f)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    go(t, None, &mut 0, f)
}

/// The notation a definition was read from. A rule a definition breaks
/// is the same rule whichever it was, and is said in the words its
/// author wrote it in: a tag of a `.d.ts` file, a member of a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notation {
    Dts,
    Manifest,
}

impl Notation {
    /// What an operation's or a kind's documentation is called.
    fn documentation(self) -> &'static str {
        match self {
            Notation::Dts => "the documentation",
            Notation::Manifest => "the `description`",
        }
    }

    /// What an interface's or a field's doc comment is called.
    fn comment(self) -> &'static str {
        match self {
            Notation::Dts => "the comment",
            Notation::Manifest => "the `description`",
        }
    }

    /// What documentation holds, after the character it may not.
    fn holds(self) -> &'static str {
        match self {
            Notation::Dts => "a doc comment holds text, tabs and line feeds",
            Notation::Manifest => "documentation holds text, tabs and line feeds",
        }
    }

    /// Why a word that starts with `@` is refused in documentation.
    fn tag_word(self) -> &'static str {
        match self {
            Notation::Dts => "which would be read as a tag",
            Notation::Manifest => "which the definition's `.d.ts` file would read as a tag",
        }
    }

    /// A host's operation that charges.
    fn spend(self, full: &str) -> String {
        match self {
            Notation::Dts => format!(
                "operation `{full}` is tagged @spend, which only Clax's own surface may use: a host's operation is @read, @write or @effect"
            ),
            Notation::Manifest => format!(
                "operation `{full}` has the access \"spend\", which only Clax's own surface may use: a host's operation is \"read\", \"write\" or \"effect\""
            ),
        }
    }

    /// A host's operation that declares a code only Clax's core gives
    /// ([`crate::wire::CORE_CODES`]).
    fn core_code(self, full: &str, code: &str) -> String {
        let declares = match self {
            Notation::Dts => format!("is tagged @throws {code}"),
            Notation::Manifest => format!("names `{code}` in its `throws`"),
        };
        format!(
            "operation `{full}` {declares}, a code only Clax gives a script: a host's operation declares codes of its own, and `unavailable` needs no declaring"
        )
    }

    /// A read that runs once.
    fn once_read(self, full: &str) -> String {
        match self {
            Notation::Dts => format!(
                "operation `{full}` is tagged @once and @read: repeating a read is harmless, so @once belongs only with @write, @effect or @spend"
            ),
            Notation::Manifest => format!(
                "operation `{full}` says `once` and has the access \"read\": repeating a read is harmless, so `once` belongs only with \"write\", \"effect\" or \"spend\""
            ),
        }
    }

    /// How to make the parameter `name` optional.
    fn make_optional(self, name: &str) -> String {
        match self {
            Notation::Dts => format!("make it optional, `{name}?:`"),
            Notation::Manifest => "make it optional, `\"optional\": true`".to_string(),
        }
    }

    /// That a host's kind `kind`, of the surface `surface`, does not say
    /// which operations its runs may have.
    fn no_allow(self, kind: &str, surface: &str) -> String {
        match self {
            Notation::Dts => format!(
                "kind `{kind}` has no @allow: a host's kind says which operations its runs may have, so write `@allow {surface}.*` for every operation of the surface, `@allow {surface}.<operation> …` for some, or `@allow` alone for none"
            ),
            Notation::Manifest => format!(
                "kind `{kind}` has no `allow`: a host's kind lists the operations its runs may have, so write `\"allow\": [\"{surface}.*\"]` for every operation of the surface, `[\"{surface}.<operation>\", …]` for some, or `[]` for none"
            ),
        }
    }

    /// That a host's kind `kind`, of the surface `surface`, allows
    /// `pattern`, one name that no operation has: a host's operation is
    /// named with its surface, and one of Clax's with its namespace.
    fn names_nothing(self, kind: &str, surface: &str, pattern: &str) -> String {
        let why = format!(
            "kind `{kind}` allows `{pattern}`, which names no operation: an allow pattern names the surface too (`{surface}.{pattern}`), and Clax's own operations by their namespace (`threads.open`)"
        );
        match self {
            Notation::Dts => format!(
                "{why}. Everything after a doc comment's first tag is tags: prose goes before them"
            ),
            Notation::Manifest => why,
        }
    }

    /// That the result of kind `kind`, whose type is `ty`, is not an
    /// object: a run's value is the input of the tool that ends it, and
    /// a tool's input is an object.
    fn result_not_object(self, kind: &str, ty: &Ty) -> String {
        let (dts, manifest) = match ty {
            Ty::Union(_) => ("a union", "an `anyOf` or an `enum`"),
            Ty::Array(_) => ("an array", "an array"),
            Ty::Record(_) => (
                "a `Record`",
                "an object with `additionalProperties` and no `properties`",
            ),
            Ty::Str | Ty::StrLit(_) => ("a string", "a string"),
            Ty::Num | Ty::Int => ("a number", "a number"),
            Ty::Bool | Ty::BoolLit(_) => ("a boolean", "a boolean"),
            Ty::Null => ("`null`", "`null`"),
            Ty::Unknown => ("`unknown`", "`{}`"),
            _ => ("a type of Clax's", "a `clax:` type"),
        };
        match self {
            Notation::Dts => format!(
                "the result of kind `{kind}` is {dts}, and a run finishes by calling a tool whose input is an object: make the result an interface or an object type, wrapping the value in a field (`{{ value: … }}`)"
            ),
            Notation::Manifest => format!(
                "the `result` of kind `{kind}` is {manifest}, and a run finishes by calling a tool whose input is an object: make it an object type or a `$ref` to an interface, wrapping the value in a property (`{{\"type\": \"object\", \"properties\": {{\"value\": …}}, \"required\": [\"value\"], \"additionalProperties\": false}}`)"
            ),
        }
    }

    /// How to make a required field `name` of a request optional
    /// instead of letting it be `null`.
    fn optional_field(self, name: &str) -> String {
        match self {
            Notation::Dts => format!(
                "write `{name}?:` and leave `| null` out of its type: an optional field may be left out or be `null`"
            ),
            Notation::Manifest => format!(
                "leave `{name}` out of `required`, and `null` out of its type: an optional field may be left out or be `null`"
            ),
        }
    }

    /// That there are more interfaces or kinds (`what`) than the limit.
    fn too_many(self, count: usize, what: &str, limit: usize) -> String {
        match self {
            Notation::Dts => {
                format!("the file declares {count} {what}, and the limit is {limit}")
            }
            Notation::Manifest => {
                format!("the manifest lists {count} {what}, and the limit is {limit}")
            }
        }
    }

    /// That `what` uses a type no interface has the name of.
    fn unknown_type(self, what: &str, name: &str) -> String {
        match self {
            Notation::Dts => format!(
                "{what} uses the type `{name}`, which the file does not declare: declare `interface {name} {{ … }}`, or use one of the format's types"
            ),
            Notation::Manifest => format!(
                "{what} uses the type `{name}`, which `interfaces` does not list: add an interface of that name there, or use one of the format's types"
            ),
        }
    }

    /// That `interface` extends a name no interface has.
    fn unknown_base(self, interface: &str, base: &str) -> String {
        match self {
            Notation::Dts => format!(
                "interface `{interface}` extends `{base}`, which the file does not declare: an interface extends interfaces of the same file"
            ),
            Notation::Manifest => format!(
                "interface `{interface}` extends `{base}`, which `interfaces` does not list: an interface extends interfaces of the same manifest"
            ),
        }
    }

    /// That a field of an object type written in `what` is documented.
    fn documented_field(self, what: &str, field: &str) -> String {
        match self {
            Notation::Dts => format!(
                "{what} has an object type whose field `{field}` has a doc comment: only an interface's fields are documented"
            ),
            Notation::Manifest => format!(
                "{what} has an object type whose field `{field}` has a `description`: only an interface's fields take one"
            ),
        }
    }

    /// Where `Instruction` may appear.
    fn instruction_places(self) -> &'static str {
        match self {
            Notation::Dts => {
                "`Instruction` (the person's own words) may appear only in a kind's request, in a kind's message and in the result of an operation tagged @waits; every other string is data"
            }
            Notation::Manifest => {
                "`Instruction` (the person's own words) may appear only in a kind's request, in a kind's message and in the result of an operation that says `waits`; every other string is data"
            }
        }
    }

    /// An operation that does not wait, in the middle of a sentence.
    fn does_not_wait(self) -> &'static str {
        match self {
            Notation::Dts => "which is not tagged @waits",
            Notation::Manifest => "which does not say `waits`",
        }
    }

    /// That a namespace directly on the surface has the surface's name.
    fn repeats_surface(self, name: &str) -> String {
        match self {
            Notation::Dts => format!(
                "namespace `{name}` has the name of the surface it is on: a script would reach its operations as `clax.{name}.{name}.…`, and the surface's members are below `{name}` already"
            ),
            Notation::Manifest => format!(
                "`{name}` is the surface's own name, and names nothing below it: an operation and a namespace are named below their surface (`f`, which a script calls as `clax.{name}.f`, not `{name}.f`)"
            ),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub site: Site,
    pub message: String,
}

fn problem<T>(site: impl Into<Site>, message: String) -> Result<T, Problem> {
    Err(Problem {
        site: site.into(),
        message,
    })
}

/// The names a file may not give an interface, beside
/// [`RESERVED_WORDS`] and [`TS_TYPES_LEFT_OUT`]: the format's own types.
const TYPE_WORDS: &[&str] = &[
    "string",
    "number",
    "boolean",
    "unknown",
    "Int",
    "Instruction",
    "Content",
    "Image",
    "Record",
    "Kinds",
    "Promise",
    "Array",
];

/// TypeScript's types that the format leaves out, which no interface may
/// be named either: the type reader refuses each as a type
/// (`crate::dts`).
const TS_TYPES_LEFT_OUT: &[&str] = &["any", "never", "object", "symbol", "bigint"];

/// The words TypeScript keeps for itself: ECMAScript's reserved words,
/// `undefined`, and the type operators. None can name a surface (which
/// the model reads as `declare namespace <name>`), an interface or a
/// parameter; a namespace, an operation, a kind and a field are
/// properties, which any word can name, except that a method named
/// `new` is a constructor.
pub(crate) const RESERVED_WORDS: &[&str] = &[
    "null",
    "true",
    "false",
    "void",
    "undefined",
    "class",
    "function",
    "new",
    "delete",
    "typeof",
    "this",
    "import",
    "export",
    "default",
    "enum",
    "var",
    "const",
    "if",
    "in",
    "do",
    "for",
    "while",
    "with",
    "return",
    "switch",
    "case",
    "try",
    "catch",
    "throw",
    "else",
    "break",
    "continue",
    "super",
    "debugger",
    "instanceof",
    "finally",
    "extends",
    "readonly",
    "keyof",
    "infer",
    "unique",
];

/// The global types of TypeScript's standard library without the DOM:
/// every interface, type alias and namespace that `lib.es2022.d.ts` and
/// the libraries it references declare at the top level (TypeScript
/// 5.9.3), the utility types (`Partial`, `Pick`, `Awaited`) among them,
/// in order. The surface named `clax` gives none of these names to an
/// interface: what the model reads declares that surface's interfaces
/// at the top level, where one of these names would merge with the
/// library's declaration (`interface String`) or clash with it
/// (`interface Map`).
#[rustfmt::skip]
pub(crate) const ES2022_TYPES: &[&str] = &[
    "AggregateError", "AggregateErrorConstructor", "Array", "ArrayBuffer",
    "ArrayBufferConstructor", "ArrayBufferLike", "ArrayBufferTypes",
    "ArrayBufferView", "ArrayConstructor", "ArrayIterator", "ArrayLike",
    "AsyncGenerator", "AsyncGeneratorFunction",
    "AsyncGeneratorFunctionConstructor", "AsyncIterable",
    "AsyncIterableIterator", "AsyncIterator", "AsyncIteratorObject", "Atomics",
    "Awaited", "BigInt", "BigInt64Array", "BigInt64ArrayConstructor",
    "BigIntConstructor", "BigIntToLocaleStringOptions", "BigUint64Array",
    "BigUint64ArrayConstructor", "Boolean", "BooleanConstructor",
    "BuiltinIteratorReturn", "CallableFunction", "Capitalize",
    "ClassAccessorDecoratorContext", "ClassAccessorDecoratorResult",
    "ClassAccessorDecoratorTarget", "ClassDecorator", "ClassDecoratorContext",
    "ClassFieldDecoratorContext", "ClassGetterDecoratorContext",
    "ClassMemberDecoratorContext", "ClassMethodDecoratorContext",
    "ClassSetterDecoratorContext", "ConcatArray", "ConstructorParameters",
    "DataView", "DataViewConstructor", "Date", "DateConstructor",
    "DecoratorContext", "DecoratorMetadata", "DecoratorMetadataObject",
    "Error", "ErrorConstructor", "ErrorOptions", "EvalError",
    "EvalErrorConstructor", "Exclude", "Extract", "FinalizationRegistry",
    "FinalizationRegistryConstructor", "FlatArray", "Float32Array",
    "Float32ArrayConstructor", "Float64Array", "Float64ArrayConstructor",
    "Function", "FunctionConstructor", "Generator", "GeneratorFunction",
    "GeneratorFunctionConstructor", "IArguments", "ImportAssertions",
    "ImportAttributes", "ImportCallOptions", "ImportMeta", "InstanceType",
    "Int16Array", "Int16ArrayConstructor", "Int32Array",
    "Int32ArrayConstructor", "Int8Array", "Int8ArrayConstructor", "Intl",
    "Iterable", "IterableIterator", "Iterator", "IteratorObject",
    "IteratorResult", "IteratorReturnResult", "IteratorYieldResult", "JSON",
    "Lowercase", "Map", "MapConstructor", "MapIterator", "Math",
    "MethodDecorator", "NewableFunction", "NoInfer", "NonNullable", "Number",
    "NumberConstructor", "Object", "ObjectConstructor", "Omit",
    "OmitThisParameter", "ParameterDecorator", "Parameters", "Partial", "Pick",
    "Promise", "PromiseConstructor", "PromiseConstructorLike",
    "PromiseFulfilledResult", "PromiseLike", "PromiseRejectedResult",
    "PromiseSettledResult", "PropertyDecorator", "PropertyDescriptor",
    "PropertyDescriptorMap", "PropertyKey", "ProxyConstructor", "ProxyHandler",
    "RangeError", "RangeErrorConstructor", "Readonly", "ReadonlyArray",
    "ReadonlyMap", "ReadonlySet", "Record", "ReferenceError",
    "ReferenceErrorConstructor", "Reflect", "RegExp", "RegExpConstructor",
    "RegExpExecArray", "RegExpIndicesArray", "RegExpMatchArray",
    "RegExpStringIterator", "Required", "ReturnType", "Set", "SetConstructor",
    "SetIterator", "SharedArrayBuffer", "SharedArrayBufferConstructor",
    "String", "StringConstructor", "StringIterator", "Symbol",
    "SymbolConstructor", "SyntaxError", "SyntaxErrorConstructor",
    "TemplateStringsArray", "ThisParameterType", "ThisType", "TypeError",
    "TypeErrorConstructor", "TypedPropertyDescriptor", "URIError",
    "URIErrorConstructor", "Uint16Array", "Uint16ArrayConstructor",
    "Uint32Array", "Uint32ArrayConstructor", "Uint8Array",
    "Uint8ArrayConstructor", "Uint8ClampedArray",
    "Uint8ClampedArrayConstructor", "Uncapitalize", "Uppercase", "WeakKey",
    "WeakKeyTypes", "WeakMap", "WeakMapConstructor", "WeakRef",
    "WeakRefConstructor", "WeakSet", "WeakSetConstructor",
];

/// The words ECMAScript reserves in strict mode, beside
/// [`RESERVED_WORDS`]. None can name a surface, which the model reads
/// as `declare namespace <name>`; anything else may be named with one.
pub(crate) const STRICT_WORDS: &[&str] = &[
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
];

const NAME_RULE: &str = "a name is letters, digits, `_` and `$`, does not start with a digit, and is not `__proto__`, `constructor` or `prototype`";

/// Whether `c` shows nothing, or changes the direction of the text
/// around it: the bidirectional controls, the zero-width and joining
/// characters, the soft hyphen, the tag characters and the rest of
/// Unicode's general category Cf (format characters); and what
/// Unicode says a renderer shows nothing for though it is not a format
/// character (its Default_Ignorable_Code_Point property): the
/// variation selectors, the combining grapheme joiner, the Hangul
/// fillers, two Khmer vowels and the Mongolian variation selectors,
/// and the code points it keeps unassigned for more of them. The
/// joiners U+200C and U+200D are among them, whatever script needs
/// them. A surface file holds none, so that what a model reads of a
/// definition is what a reviewer saw; nor does a host's prompt file
/// (`crates/clax-bot/src/prompt/file.rs`). The crate's documentation ("The
/// checks") lists these ranges as a table, which is this function: a
/// test holds the two to one set of code points.
pub fn hidden(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x034F
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

/// Whether a message names `c` by its code instead of showing it: a
/// control character, a line or paragraph separator, or a [`hidden`]
/// one.
pub(crate) fn unshowable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || hidden(c)
}

/// A name, or other text from outside, as a message shows it: cut short,
/// and with nothing in it that could break out of the message or that
/// its reader would not see.
pub(crate) fn shown(s: &str) -> String {
    const MAX: usize = 64;
    let mut out = String::new();
    let mut chars = s.chars();
    for c in chars.by_ref().take(MAX) {
        if unshowable(c) && !matches!(c, '\n' | '\r' | '\t') {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else if c == '\'' {
            // As `{:?}` writes a string: an apostrophe is itself.
            out.push(c);
        } else {
            out.extend(c.escape_debug());
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// What a name names, which decides what it may be.
#[derive(Clone, Copy)]
enum NameOf<'a> {
    Namespace,
    Operation,
    /// Of the operation with this dotted name.
    Parameter(&'a str),
    Interface,
    /// Of the interface with this name.
    Field(&'a str),
    /// Of an object type written in this declaration.
    InlineField(&'a str),
    Kind,
}

/// What is wrong with `name` as the name of an `of`, if anything: its
/// form, its length, and the words TypeScript keeps.
fn name_problem(name: &str, of: NameOf<'_>, limits: &Limits) -> Option<String> {
    let shown_name = shown(name);
    let form = match of {
        NameOf::Namespace | NameOf::Operation => (!callable(name)).then(|| {
            format!(
                "`{shown_name}` is not a name a script can call: {NAME_RULE}, nor `then`, \
                 `toJSON` or a method of `Object.prototype` (`toString`, `valueOf`, …), which \
                 JavaScript calls by itself"
            )
        }),
        NameOf::Parameter(op) => (!identifier(name)).then(|| {
            format!(
                "`{shown_name}` is not a name for a parameter of operation `{}`: use letters, digits, `_` and `$`",
                shown(op)
            )
        }),
        NameOf::Interface => (!property(name))
            .then(|| format!("`{shown_name}` is not a name for an interface: {NAME_RULE}")),
        NameOf::Field(interface) => (!property(name)).then(|| {
            format!(
                "`{shown_name}` is not a name for a field of interface `{}`: {NAME_RULE}",
                shown(interface)
            )
        }),
        NameOf::InlineField(what) => (!property(name)).then(|| {
            format!("{what} has an object type with a field named `{shown_name}`: {NAME_RULE}")
        }),
        NameOf::Kind => (!property(name))
            .then(|| format!("`{shown_name}` is not a name for a kind: {NAME_RULE}")),
    };
    if form.is_some() {
        return form;
    }
    if let Some(m) = too_long(name, limits) {
        return Some(m);
    }
    let reserved = RESERVED_WORDS.contains(&name);
    match of {
        NameOf::Operation | NameOf::Kind if name == "new" => Some(format!(
            "{} cannot be named `new`: TypeScript reads `new(…)` as a constructor, not as a method",
            if matches!(of, NameOf::Kind) {
                "a kind"
            } else {
                "an operation"
            }
        )),
        NameOf::Parameter(_) if name == "this" => Some(
            "a parameter cannot be named `this`: TypeScript reads `this: T` as the type of `this`, and the model would read an operation of one parameter fewer".to_string(),
        ),
        NameOf::Parameter(_) if reserved => Some(reserved_word(name, "a parameter")),
        NameOf::Interface if reserved => Some(reserved_word(name, "an interface")),
        NameOf::Interface if TS_TYPES_LEFT_OUT.contains(&name) => Some(format!(
            "an interface may not be named `{shown_name}`: that is one of TypeScript's types, which the format leaves out"
        )),
        NameOf::Interface if TYPE_WORDS.contains(&name) => Some(format!(
            "an interface may not be named `{shown_name}`: that is one of the format's own type names"
        )),
        _ => None,
    }
}

fn reserved_word(name: &str, what: &str) -> String {
    format!(
        "`{name}` is one of TypeScript's reserved words, which cannot name {what}: what the model reads of the surface would not be TypeScript"
    )
}

/// What is wrong with the length of `name`, if anything.
fn too_long(name: &str, limits: &Limits) -> Option<String> {
    (name.len() > limits.name_bytes).then(|| {
        format!(
            "the name `{}` is {} bytes long, and the limit for a name is {}",
            shown(name),
            name.len(),
            limits.name_bytes
        )
    })
}

/// What is said of types that take more steps than the limit to put in
/// the form a check of a value walks: it follows what has the types.
pub(crate) fn plan_too_large(limits: &Limits) -> String {
    format!(
        "more than {} steps to prepare for checking values, the limit (`Limits::plan_steps`): what costs the most is a union of many interfaces that each have many required fields of literal type",
        limits.plan_steps
    )
}

/// Checks the parts of a definition as `author`'s, within `limits`, and
/// makes the definition. What it refuses it says in the words of
/// `notation`, which the parts were read from.
pub(crate) fn build(
    root: Namespace,
    interfaces: Vec<Interface>,
    kinds: Vec<Kind>,
    author: Author<'_>,
    limits: &Limits,
    notation: Notation,
) -> Result<Definition, Problem> {
    let limits = limits.clamped();
    surface(&root, author, &limits, notation)?;
    let mut index = Index::default();
    members(&root, author, &limits, &mut index, notation)?;
    interface_names(&interfaces, &limits, &mut index, notation)?;
    library_names(&root, &interfaces)?;
    interface_comments(&interfaces, &limits, notation)?;
    interface_fields(&interfaces, &limits)?;
    inherited(&interfaces, &limits, &mut index, notation)?;
    kind_rules(&root.name, &kinds, author, &limits, notation)?;
    types(&root, &interfaces, &kinds, &limits, &index, notation)?;
    results_are_objects(&kinds, notation)?;
    if let Author::Host { .. } = author {
        request_fields(&interfaces, &kinds, &index, notation)?;
    }
    let plan = plan::build(&root, &interfaces, &kinds, &index, limits.plan_steps);
    if plan.steps > limits.plan_steps {
        return problem(
            Decl::Surface,
            format!("the surface's types take {}", plan_too_large(&limits)),
        );
    }
    index.reaches = crate::run_ops::reaches(&plan.nodes);
    index.plan = plan;
    let def = Definition {
        root,
        interfaces,
        kinds,
        index,
        limits,
    };
    instructions(&def, notation)?;
    if let Author::Host { .. } = author {
        allow_said(&def, notation)?;
    }
    Ok(def)
}

/// A kind's result is an object: an interface or an object type
/// written where it is used. The value a run finishes with is the input
/// of the tool that ends it (`Kind::result_schema`), and the model APIs
/// take only an object as a tool's input. So a union (a nullable object
/// too), an array, a primitive, a literal, `null`, `unknown`, a core
/// type and a `Record<string, T>` are refused, for Clax's surface and a
/// host's alike. (A `Record`'s schema is an object with
/// `additionalProperties` and no `properties`, which not every model API
/// takes as a tool's input.)
fn results_are_objects(kinds: &[Kind], notation: Notation) -> Result<(), Problem> {
    for (n, k) in kinds.iter().enumerate() {
        if !matches!(k.result, Ty::Named(_) | Ty::Object(_)) {
            return problem(
                Decl::KindResult(n),
                notation.result_not_object(&shown(&k.name), &k.result),
            );
        }
    }
    Ok(())
}

/// A host's kind says which operations its runs may have: only Clax's
/// own may leave its `@allow` out, which narrows nothing. It is checked
/// after every other rule, so that a definition's other mistakes are
/// said first.
fn allow_said(def: &Definition, notation: Notation) -> Result<(), Problem> {
    match def.kinds.iter().position(|k| k.allow.is_none()) {
        Some(n) => problem(
            Decl::Kind(n),
            notation.no_allow(&shown(&def.kinds[n].name), &shown(def.name())),
        ),
        None => Ok(()),
    }
}

/// What is wrong with the text of a doc comment (rules, documentation
/// or a comment on an interface or a field), if anything: `what` names
/// it. The rules are those that make it print as one doc comment and
/// read back the same, and that make it what its reader sees.
fn doc_comment(doc: &str, max: usize, what: &str, notation: Notation) -> Option<String> {
    if doc.len() > max {
        return Some(format!(
            "{what} is {} bytes long, and the limit is {max}",
            doc.len()
        ));
    }
    if doc.is_empty() {
        return Some(format!("{what} is empty: leave it out, or say something"));
    }
    if doc.contains("*/") {
        return Some(format!(
            "{what} holds `*/`, which would end the comment it is printed in"
        ));
    }
    if let Some(c) = doc
        .chars()
        .find(|&c| unshowable(c) && !matches!(c, '\n' | '\t'))
    {
        return Some(format!(
            "{what} holds the character U+{:04X}{}: {}",
            c as u32,
            if hidden(c) {
                ", which shows nothing or changes the direction of the text around it"
            } else {
                ""
            },
            notation.holds()
        ));
    }
    if doc.trim() != doc || doc.split('\n').any(|l| l.trim_end() != l) {
        return Some(format!(
            "{what} starts or ends with white space, or one of its lines ends with it"
        ));
    }
    if let Some(w) = doc.split_whitespace().find(|w| w.starts_with('@')) {
        return Some(format!(
            "{what} holds the word `{}`, {}: write it in backticks",
            shown(w),
            notation.tag_word()
        ));
    }
    None
}

fn surface(
    root: &Namespace,
    author: Author<'_>,
    limits: &Limits,
    notation: Notation,
) -> Result<(), Problem> {
    let name = &root.name;
    let word = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !word || !callable(name) {
        return problem(
            Decl::Surface,
            format!(
                "the surface's name `{}` is not one lowercase word (letters, then letters and digits; not `constructor`, `prototype` or `then`)",
                shown(name)
            ),
        );
    }
    if let Some(m) = too_long(name, limits) {
        return problem(Decl::Surface, m);
    }
    if RESERVED_WORDS.contains(&name.as_str()) {
        return problem(Decl::Surface, reserved_word(name, "a surface"));
    }
    if STRICT_WORDS.contains(&name.as_str()) {
        return problem(
            Decl::Surface,
            format!(
                "`{name}` is one of ECMAScript's reserved words in strict mode, which cannot name a surface: what the model reads of the surface would not be TypeScript"
            ),
        );
    }
    if PROMPT_PREFIXES.contains(&name.as_str()) {
        return problem(
            Decl::Surface,
            format!(
                "`{name}` cannot name a surface: in a prompt file the slots `{{core.…}}` and `{{rules.…}}` are the core's, so a surface named `core` or `rules` could have no slot of its own"
            ),
        );
    }
    if let Author::Host { .. } = author
        && STEP_FIELDS.contains(&name.as_str())
    {
        return problem(
            Decl::Surface,
            format!(
                "`{name}` is a reserved name: a host's surface may not be named as a field of a run's step ({})",
                STEP_FIELDS.join(", ")
            ),
        );
    }
    if let Author::Host { reserved } = author
        && reserved.holds(name)
    {
        return problem(
            Decl::Surface,
            format!(
                "`{name}` is a reserved name: a host's surface may not be named `clax` or as one of Clax's own namespaces ({})",
                reserved.names().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    if let Some(rules) = &root.rules
        && let Some(m) = doc_comment(rules, limits.rules_bytes, "the surface's rules", notation)
    {
        return problem(Decl::Rules, m);
    }
    Ok(())
}

/// Checks every namespace and operation, and lists the operations. It
/// walks the namespaces with a stack of its own and one path, so their
/// depth costs nothing but the stack's entries.
fn members(
    root: &Namespace,
    author: Author<'_>,
    limits: &Limits,
    index: &mut Index,
    notation: Notation,
) -> Result<(), Problem> {
    struct Level<'a> {
        ns: &'a Namespace,
        site: Decl,
        next: usize,
        /// The length of the path before this namespace's own name.
        path: usize,
        names: BTreeSet<&'a str>,
    }
    let mut path = if root.name == CLAX {
        String::new()
    } else {
        format!("{}.", root.name)
    };
    let mut stack = vec![Level {
        ns: root,
        site: Decl::Members,
        next: 0,
        path: 0,
        names: BTreeSet::new(),
    }];
    let (mut ops, mut namespaces) = (0, 0);
    while let Some(level) = stack.last_mut() {
        if level.next == 0 && level.ns.members.len() > limits.list {
            return problem(
                level.site,
                format!(
                    "`{}` has {} members, and the limit is {}",
                    shown(&level.ns.name),
                    level.ns.members.len(),
                    limits.list
                ),
            );
        }
        let ns: &Namespace = level.ns;
        let Some(m) = ns.members.get(level.next) else {
            path.truncate(level.path);
            stack.pop();
            continue;
        };
        level.next += 1;
        let (site, of) = match m {
            Member::Op(_) => (Decl::Op(ops), NameOf::Operation),
            Member::Namespace(_) => (Decl::Namespace(namespaces), NameOf::Namespace),
        };
        let name = m.name();
        if let Some(m) = name_problem(name, of, limits) {
            return problem(site, m);
        }
        if !level.names.insert(name) {
            return problem(
                site,
                format!(
                    "`{}` is declared twice: a surface or namespace holds each name once",
                    shown(&format!("{path}{name}"))
                ),
            );
        }
        match m {
            Member::Op(op) => {
                if ops == limits.operations {
                    return problem(
                        site,
                        format!(
                            "the surface has more operations than the limit, {}",
                            limits.operations
                        ),
                    );
                }
                let full = format!("{path}{name}");
                operation(op, &full, ops, author, limits, notation)?;
                index.op_by_name.insert(full.clone(), ops);
                index
                    .ops
                    .push((full, stack.iter().map(|l| l.next - 1).collect()));
                ops += 1;
            }
            Member::Namespace(inner) => {
                namespaces += 1;
                if stack.len() == 1 && name == root.name {
                    return problem(site, notation.repeats_surface(name));
                }
                if stack.len() > limits.depth {
                    return problem(
                        site,
                        format!(
                            "namespace `{}` is nested more than {} deep, the limit",
                            shown(&format!("{path}{name}")),
                            limits.depth
                        ),
                    );
                }
                if let Some(rules) = &inner.rules
                    && let Some(m) = doc_comment(
                        rules,
                        limits.rules_bytes,
                        &format!("the rules of `{}`", shown(&format!("{path}{name}"))),
                        notation,
                    )
                {
                    return problem(Decl::NamespaceRules(namespaces - 1), m);
                }
                let before = path.len();
                path.push_str(name);
                path.push('.');
                stack.push(Level {
                    ns: inner,
                    site,
                    next: 0,
                    path: before,
                    names: BTreeSet::new(),
                });
            }
        }
    }
    Ok(())
}

/// Whether `code` has the form of an error code.
pub(crate) fn error_code(code: &str) -> bool {
    code.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The rules one operation keeps by itself. Its types are checked with
/// every other type, in [`types`].
fn operation(
    op: &Op,
    full: &str,
    n: usize,
    author: Author<'_>,
    limits: &Limits,
    notation: Notation,
) -> Result<(), Problem> {
    let site = Decl::Op(n);
    let full = shown(full);
    if let Some(m) = doc_comment(
        &op.doc,
        limits.doc_bytes,
        &format!("{} of operation `{full}`", notation.documentation()),
        notation,
    ) {
        return problem(site, m);
    }
    if op.access == Access::Spend && matches!(author, Author::Host { .. }) {
        return problem(site, notation.spend(&full));
    }
    if op.once && op.access == Access::Read {
        return problem(site, notation.once_read(&full));
    }
    error_codes(op, &full, n, author, limits, notation)?;
    parameters(op, &full, n, limits, notation)
}

/// An operation's error codes: how many, the form of each, each once,
/// and, for a host's operation, none of the codes only Clax's core gives
/// ([`crate::wire::CORE_CODES`]). It looks at none when there are more
/// than the limit, and returns how many it looked at.
pub(crate) fn error_codes(
    op: &Op,
    full: &str,
    n: usize,
    author: Author<'_>,
    limits: &Limits,
    notation: Notation,
) -> Result<usize, Problem> {
    if op.throws.len() > limits.list {
        return problem(
            Decl::Code(n, limits.list),
            format!(
                "operation `{full}` names {} error codes, and the limit is {}",
                op.throws.len(),
                limits.list
            ),
        );
    }
    let mut codes = BTreeSet::new();
    for (k, code) in op.throws.iter().enumerate() {
        let site = Decl::Code(n, k);
        if !error_code(code) {
            return problem(
                site,
                format!(
                    "`{}` is not an error code of operation `{full}`: a code is lowercase letters, digits and `_`, and starts with a letter",
                    shown(code)
                ),
            );
        }
        if let Some(m) = too_long(code, limits) {
            return problem(site, m);
        }
        if matches!(author, Author::Host { .. }) && crate::wire::CORE_CODES.contains(&code.as_str())
        {
            return problem(site, notation.core_code(full, code));
        }
        if !codes.insert(code.as_str()) {
            return problem(
                site,
                format!(
                    "operation `{full}` names the error code `{}` twice",
                    shown(code)
                ),
            );
        }
    }
    Ok(op.throws.len())
}

/// An operation's parameters: how many, their names, each name once,
/// the optional ones last, and none required that could only be left
/// out.
fn parameters(
    op: &Op,
    full: &str,
    n: usize,
    limits: &Limits,
    notation: Notation,
) -> Result<(), Problem> {
    if op.params.len() > limits.list {
        return problem(
            Decl::Op(n),
            format!(
                "operation `{full}` has {} parameters, and the limit is {}",
                op.params.len(),
                limits.list
            ),
        );
    }
    let mut names = BTreeSet::new();
    for (k, p) in op.params.iter().enumerate() {
        let site = Decl::Param(n, k);
        if let Some(m) = name_problem(&p.name, NameOf::Parameter(full), limits) {
            return problem(site, m);
        }
        let name = shown(&p.name);
        if !names.insert(p.name.as_str()) {
            return problem(
                site,
                format!("operation `{full}` has two parameters named `{name}`"),
            );
        }
        if !p.optional && k > 0 && op.params[k - 1].optional {
            return problem(
                site,
                format!(
                    "parameter `{name}` of operation `{full}` is required and follows an optional one: only trailing parameters may be optional"
                ),
            );
        }
        if !p.optional && p.ty.admits_null() {
            return problem(
                site,
                format!(
                    "parameter `{name}` of operation `{full}` is required and its type admits `null`, which a call cannot pass for a required parameter (`null` means left out): {}",
                    notation.make_optional(&name)
                ),
            );
        }
    }
    Ok(())
}

/// How many interfaces there are, the name of each, and each declared
/// once: it fills in the index of interfaces by name.
pub(crate) fn interface_names(
    interfaces: &[Interface],
    limits: &Limits,
    index: &mut Index,
    notation: Notation,
) -> Result<(), Problem> {
    if interfaces.len() > limits.interfaces {
        return problem(
            Decl::Interface(limits.interfaces),
            notation.too_many(interfaces.len(), "interfaces", limits.interfaces),
        );
    }
    for (n, i) in interfaces.iter().enumerate() {
        let site = Decl::Interface(n);
        if let Some(m) = name_problem(&i.name, NameOf::Interface, limits) {
            return problem(site, m);
        }
        if index.interface_by_name.insert(i.name.clone(), n).is_some() {
            return problem(
                site,
                format!("interface `{}` is declared twice", shown(&i.name)),
            );
        }
    }
    Ok(())
}

/// The interfaces of the surface named `clax` are declared at the top
/// level of what the model reads ([`crate::declaration`]), beside
/// TypeScript's own: none takes the name of a global type of its
/// library ([`ES2022_TYPES`]), nor `globalThis`. Any other surface's
/// are declared in a namespace of the surface's name, where every name
/// is its own.
fn library_names(root: &Namespace, interfaces: &[Interface]) -> Result<(), Problem> {
    if root.name != CLAX {
        return Ok(());
    }
    for (n, i) in interfaces.iter().enumerate() {
        let name = i.name.as_str();
        if ES2022_TYPES.binary_search(&name).is_ok() || name == "globalThis" {
            return problem(
                Decl::Interface(n),
                format!(
                    "an interface of the surface named `clax` may not be named `{name}`: its interfaces are declared at the top level of what the model reads, where TypeScript's standard library declares `{name}` already"
                ),
            );
        }
    }
    Ok(())
}

/// The doc comments of the interfaces and of their fields.
fn interface_comments(
    interfaces: &[Interface],
    limits: &Limits,
    notation: Notation,
) -> Result<(), Problem> {
    for (n, i) in interfaces.iter().enumerate() {
        let name = shown(&i.name);
        if let Some(doc) = &i.doc
            && let Some(m) = doc_comment(
                doc,
                limits.doc_bytes,
                &format!("{} of interface `{name}`", notation.comment()),
                notation,
            )
        {
            return problem(Decl::Interface(n), m);
        }
        for (k, f) in i.fields.iter().enumerate() {
            if let Some(doc) = &f.doc
                && let Some(m) = doc_comment(
                    doc,
                    limits.doc_bytes,
                    &format!(
                        "{} of field `{}` of interface `{name}`",
                        notation.comment(),
                        shown(&f.name)
                    ),
                    notation,
                )
            {
                return problem(Decl::Field(n, k), m);
            }
        }
    }
    Ok(())
}

/// Each interface's own lists: how many interfaces it extends, how many
/// fields it declares, their names, and each name once. What it
/// inherits is [`inherited`]'s.
fn interface_fields(interfaces: &[Interface], limits: &Limits) -> Result<(), Problem> {
    for (n, i) in interfaces.iter().enumerate() {
        let name = shown(&i.name);
        if i.extends.len() > limits.list {
            return problem(
                Decl::Interface(n),
                format!(
                    "interface `{name}` extends {} interfaces, and the limit is {}",
                    i.extends.len(),
                    limits.list
                ),
            );
        }
        if i.fields.len() > limits.list {
            return problem(
                Decl::Interface(n),
                format!(
                    "interface `{name}` has {} fields, and the limit is {}",
                    i.fields.len(),
                    limits.list
                ),
            );
        }
        let mut names = BTreeSet::new();
        for (k, f) in i.fields.iter().enumerate() {
            let site = Decl::Field(n, k);
            if let Some(m) = name_problem(&f.name, NameOf::Field(&i.name), limits) {
                return problem(site, m);
            }
            if !names.insert(f.name.as_str()) {
                return problem(
                    site,
                    format!(
                        "interface `{name}` declares the field `{}` twice",
                        shown(&f.name)
                    ),
                );
            }
        }
    }
    Ok(())
}

/// Resolves `extends`, refuses a circle and a chain that is too long,
/// and works out each interface's fields with those it inherits: one
/// walk, each interface finished once. It answers how many steps the
/// walk took: one for each interface, each `extends` and each inherited
/// field looked at.
pub(crate) fn inherited(
    interfaces: &[Interface],
    limits: &Limits,
    index: &mut Index,
    notation: Notation,
) -> Result<usize, Problem> {
    let mut work = 0usize;
    let mut bases: Vec<Vec<usize>> = Vec::with_capacity(interfaces.len());
    for (n, i) in interfaces.iter().enumerate() {
        let mut mine = Vec::with_capacity(i.extends.len());
        let mut seen = BTreeSet::new();
        for (k, e) in i.extends.iter().enumerate() {
            let Some(&b) = index.interface_by_name.get(e) else {
                return problem(
                    Decl::Extends(n, k),
                    notation.unknown_base(&shown(&i.name), &shown(e)),
                );
            };
            if !seen.insert(b) {
                return problem(
                    Decl::Extends(n, k),
                    format!(
                        "interface `{}` extends `{}` twice",
                        shown(&i.name),
                        shown(e)
                    ),
                );
            }
            mine.push(b);
        }
        bases.push(mine);
    }
    // 0: not reached; 1: on the walk's stack; 2: finished.
    let mut state = vec![0u8; interfaces.len()];
    let mut depth = vec![0usize; interfaces.len()];
    index.fields = vec![Vec::new(); interfaces.len()];
    for start in 0..interfaces.len() {
        if state[start] != 0 {
            continue;
        }
        // Each entry: an interface, and which of its bases comes next.
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some(&mut (at, ref mut next)) = stack.last_mut() {
            work += 1;
            if let Some(&base) = bases[at].get(*next) {
                *next += 1;
                match state[base] {
                    0 => {
                        state[base] = 1;
                        stack.push((base, 0));
                    }
                    1 => {
                        let from = stack.iter().position(|&(i, _)| i == base).unwrap_or(0);
                        let through: Vec<String> = stack[from + 1..]
                            .iter()
                            .take(8)
                            .map(|&(i, _)| shown(&interfaces[i].name))
                            .collect();
                        return problem(
                            Decl::Interface(base),
                            format!(
                                "interface `{}` extends itself{}: `extends` may not go round in a circle",
                                shown(&interfaces[base].name),
                                if through.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        ", through `{}`{}",
                                        through.join("`, `"),
                                        if stack.len() - from - 1 > through.len() {
                                            " and more"
                                        } else {
                                            ""
                                        }
                                    )
                                }
                            ),
                        );
                    }
                    _ => {}
                }
                continue;
            }
            // Every base is finished: this one's depth and fields follow.
            stack.pop();
            state[at] = 2;
            let i = &interfaces[at];
            let name = shown(&i.name);
            depth[at] = bases[at].iter().map(|&b| depth[b] + 1).max().unwrap_or(0);
            if depth[at] > limits.extends_depth {
                return problem(
                    Decl::Interface(at),
                    format!(
                        "interface `{name}` is {} steps of `extends` from the end of its chain, and the limit is {}",
                        depth[at], limits.extends_depth
                    ),
                );
            }
            let mut all: Vec<(usize, usize)> = Vec::new();
            let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
            let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
            for &b in &bases[at] {
                for &(owner, k) in &index.fields[b] {
                    work += 1;
                    if !seen.insert((owner, k)) {
                        continue;
                    }
                    let field = interfaces[owner].fields[k].name.as_str();
                    if let Some(&other) = by_name.get(field) {
                        return problem(
                            Decl::Interface(at),
                            format!(
                                "interface `{name}` gets the field `{}` from both `{}` and `{}`",
                                shown(field),
                                shown(&interfaces[other].name),
                                shown(&interfaces[owner].name)
                            ),
                        );
                    }
                    by_name.insert(field, owner);
                    all.push((owner, k));
                }
            }
            for (k, f) in i.fields.iter().enumerate() {
                if let Some(&owner) = by_name.get(f.name.as_str()) {
                    return problem(
                        Decl::Field(at, k),
                        format!(
                            "interface `{name}` declares the field `{}`, which it already has from `{}`: an interface adds fields to those it extends and changes none",
                            shown(&f.name),
                            shown(&interfaces[owner].name)
                        ),
                    );
                }
                all.push((at, k));
            }
            if all.len() > limits.list {
                return problem(
                    Decl::Interface(at),
                    format!(
                        "interface `{name}` has {} fields with those it inherits, and the limit is {}",
                        all.len(),
                        limits.list
                    ),
                );
            }
            index.fields[at] = all;
        }
    }
    Ok(work)
}

/// What is wrong with the shape of `t`, without recursing: how deep it
/// nests and how long its lists are.
fn shape(t: &Ty, limits: &Limits) -> Option<String> {
    let mut todo = vec![(t, 0usize)];
    while let Some((t, depth)) = todo.pop() {
        if depth > limits.depth {
            return Some(format!(
                "a type nested more than {} deep, the limit",
                limits.depth
            ));
        }
        match t {
            Ty::Object(fields) if fields.len() > limits.list => {
                return Some(format!(
                    "an object type with {} fields, where the limit is {}",
                    fields.len(),
                    limits.list
                ));
            }
            Ty::Union(ts) if ts.len() > limits.list => {
                return Some(format!(
                    "a union of {} alternatives, where the limit is {}",
                    ts.len(),
                    limits.list
                ));
            }
            Ty::Union(ts) if ts.len() < 2 || ts.iter().any(|x| matches!(x, Ty::Union(_))) => {
                return Some(
                    "a union that is not two or more alternatives, none of them a union".into(),
                );
            }
            _ => {}
        }
        // A union's alternatives are at its own level; the rest, one below.
        let below = depth + usize::from(!matches!(t, Ty::Union(_)));
        todo.extend(t.parts().map(|p| (p, below)));
    }
    None
}

/// An alternative of a union, as a message about the union names it.
fn alternative(t: &Ty) -> String {
    match t {
        Ty::Str => "`string`".into(),
        Ty::Num => "`number`".into(),
        Ty::Int => "`Int`".into(),
        Ty::Bool => "`boolean`".into(),
        Ty::Null => "`null`".into(),
        Ty::Unknown => "`unknown`".into(),
        Ty::Instruction => "`Instruction`".into(),
        Ty::Content => "`Content`".into(),
        Ty::Image => "`Image`".into(),
        Ty::StrLit(s) => format!("the literal \"{}\"", shown(s)),
        Ty::BoolLit(b) => format!("`{b}`"),
        Ty::Named(n) => format!("`{}`", shown(n)),
        Ty::Array(_) => "one array type".into(),
        Ty::Record(_) => "one `Record`".into(),
        Ty::Object(_) => "one object type".into(),
        Ty::Union(_) => "one union".into(),
    }
}

/// Checks one type where it is written: `decl` and `what` say where.
/// After [`shape`] it recurses, no deeper than the limit.
fn ty(
    t: &Ty,
    decl: Decl,
    what: &str,
    limits: &Limits,
    index: &Index,
    notation: Notation,
) -> Result<(), Problem> {
    if let Some(m) = shape(t, limits) {
        return problem(decl, format!("{what} has {m}"));
    }
    let at = |inner| Site { decl, inner };
    visit_inline(t, &mut |step| match step {
        Visit::Type(Ty::Named(n), inside) if !index.interface_by_name.contains_key(n) => {
            problem(at(inside), notation.unknown_type(what, &shown(n)))
        }
        Visit::Type(Ty::StrLit(s), inside) => match s.chars().find(|&c| unshowable(c)) {
            Some(c) => problem(
                at(inside),
                format!(
                    "{what} has a string literal that holds the character U+{:04X}: a literal holds no control character, no line end, and no character that shows nothing or changes the direction of text",
                    c as u32
                ),
            ),
            None => Ok(()),
        },
        Visit::Type(..) => Ok(()),
        Visit::Alternative { alts, k, inside } => {
            let x = &alts[k];
            if alts[..k].contains(x) {
                return problem(
                    at(inside),
                    format!(
                        "{what} has a union that lists {} twice: a union lists each alternative once",
                        alternative(x)
                    ),
                );
            }
            Ok(())
        }
        Visit::Field {
            n,
            field: f,
            siblings,
            k,
        } => {
            let site = at(Some(n));
            if let Some(m) = name_problem(&f.name, NameOf::InlineField(what), limits) {
                return problem(site, m);
            }
            if siblings[..k].iter().any(|g| g.name == f.name) {
                return problem(
                    site,
                    format!(
                        "{what} has an object type that lists the field `{}` twice",
                        shown(&f.name)
                    ),
                );
            }
            if f.doc.is_some() {
                return problem(site, notation.documented_field(what, &shown(&f.name)));
            }
            Ok(())
        }
    })
}

/// Every type of the definition is well formed and names interfaces the
/// file declares.
fn types(
    root: &Namespace,
    interfaces: &[Interface],
    kinds: &[Kind],
    limits: &Limits,
    index: &Index,
    notation: Notation,
) -> Result<(), Problem> {
    for (n, (name, path)) in index.ops.iter().enumerate() {
        let Some(op) = op_at(root, path) else {
            continue;
        };
        let what = format!("operation `{}`", shown(name));
        for (k, p) in op.params.iter().enumerate() {
            ty(&p.ty, Decl::Param(n, k), &what, limits, index, notation)?;
        }
        ty(&op.result, Decl::Result(n), &what, limits, index, notation)?;
    }
    for (n, i) in interfaces.iter().enumerate() {
        for (k, f) in i.fields.iter().enumerate() {
            let what = format!(
                "field `{}` of interface `{}`",
                shown(&f.name),
                shown(&i.name)
            );
            ty(&f.ty, Decl::Field(n, k), &what, limits, index, notation)?;
        }
    }
    for (n, k) in kinds.iter().enumerate() {
        let what = format!("kind `{}`", shown(&k.name));
        ty(&k.request, Decl::Request(n), &what, limits, index, notation)?;
        if let Some(m) = &k.message {
            ty(m, Decl::Message(n), &what, limits, index, notation)?;
        }
        ty(
            &k.result,
            Decl::KindResult(n),
            &what,
            limits,
            index,
            notation,
        )?;
    }
    Ok(())
}

/// The rules a kind keeps by itself, and how many kinds there are.
/// Their types are checked with every other type, in [`types`].
///
/// A host's kind allows no pattern that is one name with no `.` but
/// one [`Reserved`] holds: such a name is an operation directly on
/// `clax` or nothing, and a word of prose written after `@allow` reads
/// as one.
fn kind_rules(
    surface: &str,
    kinds: &[Kind],
    author: Author<'_>,
    limits: &Limits,
    notation: Notation,
) -> Result<(), Problem> {
    if kinds.len() > limits.kinds {
        return problem(
            Decl::Kind(limits.kinds),
            notation.too_many(kinds.len(), "kinds", limits.kinds),
        );
    }
    let mut names = BTreeSet::new();
    for (n, k) in kinds.iter().enumerate() {
        let site = Decl::Kind(n);
        if let Some(m) = name_problem(&k.name, NameOf::Kind, limits) {
            return problem(site, m);
        }
        let name = shown(&k.name);
        if !names.insert(k.name.as_str()) {
            return problem(site, format!("kind `{name}` is declared twice"));
        }
        if let Some(m) = doc_comment(
            &k.doc,
            limits.doc_bytes,
            &format!("{} of kind `{name}`", notation.documentation()),
            notation,
        ) {
            return problem(site, m);
        }
        let Some(allow) = &k.allow else { continue };
        if allow.len() > limits.list {
            return problem(
                site,
                format!(
                    "kind `{name}` allows {} patterns, and the limit is {}",
                    allow.len(),
                    limits.list
                ),
            );
        }
        for pattern in allow {
            // A surface, the namespaces inside it, and an operation.
            let (names, longest) = pattern.names();
            if names > limits.depth + 2 || longest > limits.name_bytes {
                return problem(
                    site,
                    format!(
                        "kind `{name}` allows `{}`, which no operation's name can be: a name is {} bytes at most, and an operation has {} namespaces around it at most",
                        shown(&pattern.to_string()),
                        limits.name_bytes,
                        limits.depth + 1
                    ),
                );
            }
            if let (Author::Host { reserved }, Some(one)) = (author, pattern.single_name())
                && !reserved.holds(one)
            {
                return problem(
                    site,
                    notation.names_nothing(&name, &shown(surface), &shown(one)),
                );
            }
        }
    }
    Ok(())
}

/// A host's kind's request names no required field whose type admits
/// `null`: a field that may be absent is optional (`name?:`), which
/// takes `null` too. The fields are the request's own: those of the
/// object type or interface it is, or of each object type and interface
/// among the alternatives of a union it is; a field further inside is
/// data, and not this rule's.
fn request_fields(
    interfaces: &[Interface],
    kinds: &[Kind],
    index: &Index,
    notation: Notation,
) -> Result<(), Problem> {
    for (n, k) in kinds.iter().enumerate() {
        let alts: &[Ty] = match &k.request {
            Ty::Union(alts) => alts,
            one => std::slice::from_ref(one),
        };
        let kind = shown(&k.name);
        let refuse = |site: Site, field: &str| {
            let field = shown(field);
            problem(
                site,
                format!(
                    "field `{field}` of the request of kind `{kind}` is required and its type admits `null`: {}",
                    notation.optional_field(&field)
                ),
            )
        };
        // The number `Site::inner` gives each of the request's own
        // fields written inline: those of an object type it is, or that
        // is one of its alternatives.
        let mut own: Vec<(&Field, usize)> = Vec::new();
        let _ = visit_inline(&k.request, &mut |step| {
            if let Visit::Field {
                n, field, siblings, ..
            } = step
                && alts
                    .iter()
                    .any(|a| matches!(a, Ty::Object(fs) if std::ptr::eq(fs.as_slice(), siblings)))
            {
                own.push((field, n));
            }
            Ok::<(), ()>(())
        });
        for alt in alts {
            match alt {
                Ty::Object(fields) => {
                    for f in fields {
                        if !f.optional && f.ty.admits_null() {
                            let inner = own
                                .iter()
                                .find(|(g, _)| std::ptr::eq(*g, f))
                                .map(|&(_, at)| at);
                            return refuse(
                                Site {
                                    decl: Decl::Request(n),
                                    inner,
                                },
                                &f.name,
                            );
                        }
                    }
                }
                Ty::Named(name) => {
                    let Some(&i) = index.interface_by_name.get(name) else {
                        continue;
                    };
                    for &(owner, at) in &index.fields[i] {
                        let f = &interfaces[owner].fields[at];
                        if !f.optional && f.ty.admits_null() {
                            return refuse(Decl::Field(owner, at).into(), &f.name);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// For each interface, the interface an `Instruction` is written in that
/// it reaches through fields and `extends` (itself, when it is written
/// there), if any.
fn instruction_holders(def: &Definition) -> Vec<Option<usize>> {
    let n = def.interfaces.len();
    // `users[j]`: the interfaces that name `j` in a field or extend it.
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut holder: Vec<Option<usize>> = vec![None; n];
    let mut todo = Vec::new();
    for (i, interface) in def.interfaces.iter().enumerate() {
        let mut direct = false;
        let mut uses = |t: &Ty| {
            t.each(&mut |x| match x {
                Ty::Instruction => direct = true,
                Ty::Named(name) => {
                    if let Some(&j) = def.index.interface_by_name.get(name) {
                        users[j].push(i);
                    }
                }
                _ => {}
            });
        };
        interface.fields.iter().for_each(|f| uses(&f.ty));
        for e in &interface.extends {
            if let Some(&j) = def.index.interface_by_name.get(e) {
                users[j].push(i);
            }
        }
        if direct {
            holder[i] = Some(i);
            todo.push(i);
        }
    }
    while let Some(j) = todo.pop() {
        for &i in &users[j] {
            if holder[i].is_none() {
                holder[i] = holder[j];
                todo.push(i);
            }
        }
    }
    holder
}

fn instructions(def: &Definition, notation: Notation) -> Result<(), Problem> {
    let places = notation.instruction_places();
    let holder = instruction_holders(def);
    // How `t` holds an `Instruction`, when it does: `""` when it is
    // written in `t` itself, else `, in interface X`.
    let holds = |t: &Ty| -> Option<String> {
        let mut found = None;
        t.each(&mut |x| match x {
            Ty::Instruction if found.is_none() => found = Some(String::new()),
            Ty::Named(n) if found.is_none() => {
                if let Some(h) = def.index.interface_by_name.get(n).and_then(|&i| holder[i]) {
                    found = Some(format!(
                        ", in interface `{}`",
                        shown(&def.interfaces[h].name)
                    ));
                }
            }
            _ => {}
        });
        found
    };
    for (n, o) in def.operations().enumerate() {
        for (k, p) in o.op.params.iter().enumerate() {
            if let Some(within) = holds(&p.ty) {
                return problem(
                    Decl::Param(n, k),
                    format!(
                        "parameter `{}` of operation `{}` holds `Instruction`{within}: {places}",
                        shown(&p.name),
                        shown(o.name)
                    ),
                );
            }
        }
        if !o.op.waits
            && let Some(within) = holds(&o.op.result)
        {
            return problem(
                Decl::Result(n),
                format!(
                    "the result of operation `{}`, {}, holds `Instruction`{within}: {places}",
                    shown(o.name),
                    notation.does_not_wait()
                ),
            );
        }
    }
    for (n, k) in def.kinds.iter().enumerate() {
        if let Some(within) = holds(&k.result) {
            return problem(
                Decl::KindResult(n),
                format!(
                    "the result of kind `{}` holds `Instruction`{within}: {places}",
                    shown(&k.name)
                ),
            );
        }
    }
    Ok(())
}

/// Why `t` is not a type of `def`, if it is not: it nests deeper or
/// lists more than `def`'s limits allow, or names an interface `def`
/// lacks. It does not recurse, so a type built anywhere is safe to ask
/// about.
#[cfg(test)]
pub(crate) fn foreign(def: &Definition, t: &Ty) -> Option<String> {
    if let Some(m) = shape(t, &def.limits) {
        return Some(format!("its type is beyond the surface's limits: {m}"));
    }
    let mut missing = None;
    t.each(&mut |x| {
        if let Ty::Named(n) = x
            && missing.is_none()
            && !def.index.interface_by_name.contains_key(n)
        {
            missing = Some(n);
        }
    });
    missing.map(|n| {
        format!(
            "its type names `{}`, which the surface does not declare",
            shown(n)
        )
    })
}
