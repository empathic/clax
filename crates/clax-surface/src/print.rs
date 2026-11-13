//! The two printers: a definition as a `.d.ts` file again, and what a
//! run's model reads of the surfaces it has.
//!
//! Both recurse over a definition's types and namespaces, which nest no
//! deeper than its [`crate::Limits`] allowed.

use crate::limits::Limits;
use crate::model::{Access, CLAX, Definition, Field, Interface, Member, Namespace, Op, Ty};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// How a type is printed.
#[derive(Clone, Copy)]
struct Style<'a> {
    /// For the model: `Int` is `number`.
    model: bool,
    /// The namespace an interface's name is written with (`review.Page`),
    /// when it is not in scope where the type is printed.
    qualify: Option<&'a str>,
}

const FILE: Style<'static> = Style {
    model: false,
    qualify: None,
};

fn ty(t: &Ty, s: Style<'_>) -> String {
    match t {
        Ty::Str => "string".into(),
        Ty::Num => "number".into(),
        Ty::Int => if s.model { "number" } else { "Int" }.into(),
        Ty::Bool => "boolean".into(),
        Ty::Null => "null".into(),
        Ty::Unknown => "unknown".into(),
        Ty::Instruction => "Instruction".into(),
        Ty::Content => "Content".into(),
        Ty::Image => "Image".into(),
        Ty::StrLit(lit) => format!("\"{}\"", lit.replace('\\', "\\\\").replace('"', "\\\"")),
        Ty::BoolLit(b) => b.to_string(),
        Ty::Named(n) => match s.qualify {
            Some(q) => format!("{q}.{n}"),
            None => n.clone(),
        },
        Ty::Array(x) => match **x {
            Ty::Union(_) => format!("({})[]", ty(x, s)),
            _ => format!("{}[]", ty(x, s)),
        },
        Ty::Record(x) => format!("Record<string, {}>", ty(x, s)),
        Ty::Object(fields) if fields.is_empty() => "{}".into(),
        Ty::Object(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|x| named(&x.name, x.optional, &x.ty, s))
                .collect();
            format!("{{ {} }}", inner.join("; "))
        }
        Ty::Union(ts) => ts.iter().map(|x| ty(x, s)).collect::<Vec<_>>().join(" | "),
    }
}

/// A type as the model reads it, whole.
#[cfg(test)]
pub(crate) fn model_text(t: &Ty) -> String {
    ty(
        t,
        Style {
            model: true,
            qualify: None,
        },
    )
}

/// `name: T` or `name?: T`.
fn named(name: &str, optional: bool, t: &Ty, s: Style<'_>) -> String {
    format!("{name}{}: {}", if optional { "?" } else { "" }, ty(t, s))
}

fn signature(o: &Op, s: Style<'_>) -> String {
    let params: Vec<String> = o
        .params
        .iter()
        .map(|p| named(&p.name, p.optional, &p.ty, s))
        .collect();
    format!(
        "{}({}): Promise<{}>;",
        o.name,
        params.join(", "),
        ty(&o.result, s)
    )
}

/// A doc comment of the file notation: on one line when its prose is one
/// line, else a block with the tags on its last line.
fn comment(out: &mut String, ind: &str, prose: &str, tags: &str) {
    if !prose.contains('\n') {
        let all = [prose, tags]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "{ind}/** {all} */");
    } else {
        let _ = writeln!(out, "{ind}/**");
        for l in prose.split('\n').chain((!tags.is_empty()).then_some(tags)) {
            let _ = writeln!(out, "{ind} *{}{l}", if l.is_empty() { "" } else { " " });
        }
        let _ = writeln!(out, "{ind} */");
    }
}

/// A doc comment as the model reads it: always one line.
fn model_comment(out: &mut String, ind: &str, text: &str) {
    let one: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let _ = writeln!(out, "{ind}/** {} */", one.join(" "));
}

fn doc(out: &mut String, ind: &str, text: Option<&String>, s: Style<'_>) {
    match text {
        Some(d) if s.model => model_comment(out, ind, d),
        Some(d) => comment(out, ind, d, ""),
        None => {}
    }
}

fn interface(out: &mut String, ind: &str, i: &Interface, s: Style<'_>) {
    doc(out, ind, i.doc.as_ref(), s);
    let ext = if i.extends.is_empty() {
        String::new()
    } else {
        format!(" extends {}", i.extends.join(", "))
    };
    let _ = writeln!(out, "{ind}interface {}{ext} {{", i.name);
    for x in &i.fields {
        field(out, ind, x, s);
    }
    let _ = writeln!(out, "{ind}}}");
}

fn field(out: &mut String, ind: &str, x: &Field, s: Style<'_>) {
    let ind = format!("{ind}  ");
    doc(out, &ind, x.doc.as_ref(), s);
    let _ = writeln!(out, "{ind}{};", named(&x.name, x.optional, &x.ty, s));
}

fn file_members(out: &mut String, members: &[Member], depth: usize) {
    let ind = "  ".repeat(depth);
    for m in members {
        match m {
            Member::Op(o) => {
                let mut tags = vec![format!("@{}", o.access.tag())];
                if o.waits {
                    tags.push("@waits".into());
                }
                if o.once {
                    tags.push("@once".into());
                }
                tags.extend(o.throws.iter().map(|code| format!("@throws {code}")));
                comment(out, &ind, &o.doc, &tags.join(" "));
                let _ = writeln!(out, "{ind}{}", signature(o, FILE));
            }
            Member::Namespace(n) => {
                if let Some(r) = &n.rules {
                    comment(out, &ind, r, "");
                }
                let _ = writeln!(out, "{ind}{}: {{", n.name);
                file_members(out, &n.members, depth + 1);
                let _ = writeln!(out, "{ind}}};");
            }
        }
    }
}

/// The definition as a `.d.ts` file, in the one way this crate writes
/// one: the surface, then its interfaces, then its kinds. Reading the
/// file back gives an equal definition, and printing that gives the
/// same file. `//` comments and the layout of the file a definition was
/// read from are not part of it.
///
/// The print may be longer than the file the definition was read from,
/// and so longer than [`Limits::file_bytes`] lets a file be read: it
/// puts each member, each field and each line of a doc comment on a
/// line of its own, indented two spaces for each namespace around it,
/// and puts spaces around its punctuation, where a source may be
/// written with none. It is never longer than `2 × depth + 8` times its
/// source (`depth` being [`Limits::depth`]: 72 times, by default; the
/// most a line of the print holds for one byte of source is a blank
/// line of documentation at the deepest indentation). [`print_within`]
/// says whether a print can be read back under given limits.
#[must_use]
pub fn print(def: &Definition) -> String {
    let mut out = String::new();
    if let Some(r) = def.rules() {
        comment(&mut out, "", r, "");
    }
    let _ = writeln!(out, "declare const {}: {{", def.name());
    file_members(&mut out, def.members(), 1);
    out.push_str("};\n");
    if !def.interfaces().is_empty() {
        out.push('\n');
    }
    for i in def.interfaces() {
        interface(&mut out, "", i, FILE);
    }
    if !def.kinds().is_empty() {
        out.push_str("\ninterface Kinds {\n");
        for k in def.kinds() {
            let tags = match &k.allow {
                // An empty list is `@allow` alone.
                Some(allow) => std::iter::once("@allow".to_string())
                    .chain(allow.iter().map(ToString::to_string))
                    .collect::<Vec<_>>()
                    .join(" "),
                None => String::new(),
            };
            comment(&mut out, "  ", &k.doc, &tags);
            let message = k
                .message
                .as_ref()
                .map(|m| format!(", {}", named("message", true, m, FILE)))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {}({}{message}): {};",
                k.name,
                named("request", false, &k.request, FILE),
                ty(&k.result, FILE)
            );
        }
        out.push_str("}\n");
    }
    out
}

/// The definition's print ([`print`]), when a reader under `limits`
/// would take it: a tool that writes the print to a file uses this, so
/// as not to write one that reading refuses.
///
/// # Errors
/// That the print is longer than `limits.file_bytes`, with both
/// numbers.
pub fn print_within(def: &Definition, limits: &Limits) -> Result<String, String> {
    let printed = print(def);
    if printed.len() > limits.file_bytes {
        return Err(format!(
            "the definition prints as a file of {} bytes, and the limit is {}",
            printed.len(),
            limits.file_bytes
        ));
    }
    Ok(printed)
}

/// What the model reads of some members and their namespaces.
struct ModelMembers<'a> {
    has: &'a dyn Fn(&str) -> bool,
    style: Style<'a>,
    /// The dotted name of the namespace being printed, with its `.`.
    path: String,
}

impl ModelMembers<'_> {
    /// The members of a namespace that the run has; nothing when it has
    /// none of them.
    fn members(&mut self, members: &[Member], depth: usize) -> String {
        let ind = "  ".repeat(depth);
        let mut out = String::new();
        for m in members {
            match m {
                Member::Op(o) => {
                    let before = self.path.len();
                    self.path.push_str(&o.name);
                    let has = (self.has)(&self.path);
                    self.path.truncate(before);
                    if !has {
                        continue;
                    }
                    let mut d = o.doc.clone();
                    match o.access {
                        Access::Write | Access::Effect => d.push_str(" (writes)"),
                        Access::Spend => d.push_str(" (charged)"),
                        Access::Read => {}
                    }
                    if o.waits {
                        d.push_str(" (may take long)");
                    }
                    if !o.throws.is_empty() {
                        let _ = write!(d, " Errors: {}.", o.throws.join(", "));
                    }
                    model_comment(&mut out, &ind, &d);
                    let _ = writeln!(out, "{ind}{}", signature(o, self.style));
                }
                Member::Namespace(n) => out.push_str(&self.namespace(n, depth)),
            }
        }
        out
    }

    fn namespace(&mut self, n: &Namespace, depth: usize) -> String {
        let before = self.path.len();
        self.path.push_str(&n.name);
        self.path.push('.');
        let body = self.members(&n.members, depth + 1);
        self.path.truncate(before);
        if body.is_empty() {
            return String::new();
        }
        let ind = "  ".repeat(depth);
        format!("{ind}{}: {{\n{body}{ind}}};\n", n.name)
    }
}

/// How the model is told what `Content` and `Image` are, after the
/// surfaces' interfaces, when an operation the run has names one
/// ([`declaration`]).
pub const CONTENT_DECLARATION: &str = "\
/** What Clax keeps of some bytes, named by a handle: an operation answers or takes one where its type says `Content`. Pass a handle on as it was given. Return an `Image` from a script, anywhere in its value, to see it. */
interface Content { id: string; media_type: string; bytes: number; sha256: string; name?: string; width?: number; height?: number; meta?: Record<string, unknown> }
/** A `Content` of an image: its `media_type` is image/png, image/jpeg, image/webp or image/gif. */
type Image = Content;
";

/// What a run's model reads of its surfaces: the interfaces its
/// operations reach, then one `declare const clax: { … };` that holds
/// the operations themselves. When an operation the run has names
/// `Content` or `Image`, [`CONTENT_DECLARATION`] comes after the
/// interfaces. `has` says whether the run has an
/// operation, by its dotted name ([`crate::Operation::name`]).
///
/// - The surface named `clax` gives its members directly, and its
///   interfaces as they are. Every other surface gives one namespace of
///   `clax`, of its own name, and its interfaces inside
///   `declare namespace <name> { … }`, named `<name>.<Interface>` in its
///   operations: no two surfaces' types can meet. Surfaces come in the
///   order of `surfaces`.
/// - A namespace none of whose operations the run has is left out, and so
///   is an interface no remaining operation reaches. Kinds and rules are
///   never printed (rules belong to the prompt).
/// - An operation's doc comment is its documentation, then "(writes)" for
///   `@write` and `@effect` or "(charged)" for `@spend`, "(may take long)"
///   for `@waits`, and "Errors: a, b." for `@throws a @throws b`. `@once` says
///   nothing to the model. `Int` is `number`. Interfaces keep their
///   `extends` and their doc comments. A doc comment is one line, its
///   lines joined by spaces.
///
/// # Errors
/// When two of `surfaces` put one name on `clax`: two of one name, or
/// one named as a namespace of the surface named `clax`.
pub fn declaration(surfaces: &[&Definition], has: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let mut names = BTreeSet::new();
    for def in surfaces {
        for n in def.top_level_names() {
            if !names.insert(n) {
                return Err(format!(
                    "two surfaces put `{n}` on `clax`: `{}` is one of them",
                    def.name()
                ));
            }
        }
    }
    let mut out = String::new();
    for def in surfaces {
        let reached = def.reach(
            def.operations()
                .filter(|o| has(o.name))
                .flat_map(|o| o.op.types()),
        );
        let mine = def
            .interfaces()
            .iter()
            .zip(&reached)
            .filter(|(_, r)| **r)
            .map(|(i, _)| i);
        let style = Style {
            model: true,
            qualify: None,
        };
        if def.name() == CLAX {
            mine.for_each(|i| interface(&mut out, "", i, style));
        } else if reached.contains(&true) {
            let _ = writeln!(out, "declare namespace {} {{", def.name());
            mine.for_each(|i| interface(&mut out, "  ", i, style));
            out.push_str("}\n");
        }
    }
    // The core's types, after the surfaces' own, for a run that has an
    // operation that names one.
    if surfaces
        .iter()
        .any(|def| def.operations().any(|o| has(o.name) && o.holds_content()))
    {
        out.push_str(CONTENT_DECLARATION);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("declare const clax: {\n");
    for def in surfaces {
        let own = def.name() == CLAX;
        let mut members = ModelMembers {
            has,
            style: Style {
                model: true,
                qualify: (!own).then_some(def.name()),
            },
            path: String::new(),
        };
        out.push_str(&if own {
            members.members(def.members(), 1)
        } else {
            members.namespace(&def.root, 1)
        });
    }
    out.push_str("};\n");
    Ok(out)
}
