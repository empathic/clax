//! Allow patterns: which operations a kind's `@allow`, or an operator's
//! `allow`, names.

use crate::check::shown;
use crate::model::Definition;
use std::fmt;

/// One allow pattern over dotted operation names
/// ([`crate::Operation::name`]):
///
/// - `shell.windows` is that operation;
/// - `web.*`, `shell.settings.*` and `review.*` are every operation below
///   that namespace or surface, at any depth;
/// - `*` is every operation.
///
/// Clax's own operations are named without a surface (`files.read`,
/// `files.*`), and no other surface may be named `clax`, so a pattern
/// whose first name is `clax` could match nothing, and is refused.
///
/// A list of patterns allows an operation one of them matches, so an
/// empty list allows none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern(Form);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Form {
    All,
    /// Every name that starts with this, which ends in `.`.
    Under(String),
    Exact(String),
}

/// Whether `s` is an identifier of ASCII letters, digits, `_` and `$`.
pub(crate) fn identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Whether `s` can name a property of an object a script or a provider
/// holds: an identifier, and none of the names every object has already.
pub(crate) fn property(s: &str) -> bool {
    identifier(s) && !matches!(s, "__proto__" | "constructor" | "prototype")
}

/// The names no namespace or operation may have: the names every object
/// has already (`__proto__`, `constructor`, `prototype`); `then`, which
/// would make the object that holds it a thenable that `await` calls;
/// `toJSON`, which `JSON.stringify` calls when a script logs or returns
/// a namespace; and the other methods of `Object.prototype`, which
/// JavaScript itself calls (`toString` and `valueOf` when a namespace
/// is made a string) or a script expects to find on any object. A
/// namespace of `clax` has no prototype, so an operation of one of these
/// names would be the only property of that name, and a call of it a
/// call no script wrote. The sandbox refuses the same names
/// (`clax_bot_sandbox::RESERVED_SEGMENTS`, which a test of `clax-bot`
/// holds equal to this list).
pub const RESERVED_CALLABLE: &[&str] = &[
    "__proto__",
    "constructor",
    "prototype",
    "then",
    "toJSON",
    "toString",
    "toLocaleString",
    "valueOf",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// Whether `s` can name something a script reaches through `clax`, a
/// namespace or an operation: a [`property`], and none of
/// [`RESERVED_CALLABLE`].
pub(crate) fn callable(s: &str) -> bool {
    property(s) && !RESERVED_CALLABLE.contains(&s)
}

impl Pattern {
    /// Reads a pattern.
    ///
    /// # Errors
    /// What is wrong with `s`, when it is not `*`, a dotted name, or a
    /// dotted name followed by `.*`, or when its first name is `clax`
    /// (`clax` itself included).
    /// The message shows `s` cut short and escaped.
    pub fn parse(s: &str) -> Result<Pattern, String> {
        if s == "*" {
            return Ok(Pattern(Form::All));
        }
        let (name, under) = match s.strip_suffix(".*") {
            Some(name) => (name, true),
            None => (s, false),
        };
        if !name.split('.').all(callable) {
            return Err(format!(
                "`{}` is not an allow pattern: write an operation (`shell.windows`), everything below a surface or namespace (`shell.*`, `shell.settings.*`), or `*`",
                shown(s)
            ));
        }
        // The one name `clax` is no operation of Clax's own (they are
        // named without it) and no other surface's (none takes the name).
        if s == "clax" {
            return Err(
                "`clax` is not an allow pattern: Clax's own operations are named without `clax.` \
                 (`files.read`, `files.*`)"
                    .to_string(),
            );
        }
        if let Some(rest) = s.strip_prefix("clax.") {
            return Err(format!(
                "`{}` is not an allow pattern: Clax's own operations are named without `clax.` (`{}`)",
                shown(s),
                shown(rest)
            ));
        }
        Ok(Pattern(if under {
            Form::Under(format!("{name}."))
        } else {
            Form::Exact(name.to_string())
        }))
    }

    /// Whether the pattern names the operation `op`, by its dotted name.
    #[must_use]
    pub fn matches(&self, op: &str) -> bool {
        match &self.0 {
            Form::All => true,
            Form::Under(prefix) => op.starts_with(prefix.as_str()),
            Form::Exact(name) => op == name,
        }
    }

    /// The name the pattern is, when it is one name with no `.`: what
    /// names an operation directly on `clax`, and nothing else.
    pub(crate) fn single_name(&self) -> Option<&str> {
        match &self.0 {
            Form::Exact(name) if !name.contains('.') => Some(name),
            _ => None,
        }
    }

    /// How many names the pattern is made of, and the bytes of the
    /// longest.
    pub(crate) fn names(&self) -> (usize, usize) {
        let dotted = match &self.0 {
            Form::All => return (0, 0),
            Form::Under(prefix) => prefix.trim_end_matches('.'),
            Form::Exact(name) => name,
        };
        let longest = dotted.split('.').map(str::len).max().unwrap_or(0);
        (dotted.split('.').count(), longest)
    }

    /// Whether the pattern names an operation of any of `surfaces`. One
    /// that names none is most likely a mistake, which only a caller
    /// that holds every surface of a run can tell: a kind may allow
    /// another surface's operations.
    #[must_use]
    pub fn names_any(&self, surfaces: &[&Definition]) -> bool {
        surfaces
            .iter()
            .any(|d| d.operations().any(|o| self.matches(o.name)))
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Form::All => f.write_str("*"),
            Form::Under(prefix) => write!(f, "{prefix}*"),
            Form::Exact(name) => f.write_str(name),
        }
    }
}
