//! The `.d.ts` notation's reader: a lexer and a parser for the format's
//! subset of TypeScript, which remember the line of each declaration so
//! that every refusal, the checks' too, names one.
//!
//! The reader refuses what the notation cannot say and builds the parts
//! of a definition; the rules a definition keeps are
//! [`crate::check::build`]'s. It nests no deeper than the limit lets a
//! definition nest, so it cannot be made to exhaust the stack.

use crate::allow::Pattern;
use crate::check::{self, Author, Decl, Notation, Site};
use crate::error::{Error, Place};
use crate::limits::Limits;
use crate::model::{Access, Definition, Field, Interface, Kind, Member, Namespace, Op, Param, Ty};

/// A mistake in a `.d.ts` file and its line, as the reader passes it
/// along: no larger than a line and a message, since the reader's stack
/// holds one for each level a type nests. [`parse`] gives it as an
/// [`Error`].
#[derive(Debug)]
pub(crate) struct Mistake {
    pub line: usize,
    pub message: String,
}

impl From<Mistake> for Error {
    fn from(m: Mistake) -> Error {
        Error {
            at: Place::Line(m.line),
            message: m.message,
        }
    }
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, Mistake> {
    Err(Mistake {
        line,
        message: message.into(),
    })
}

/// Reads a surface definition from its `.d.ts` notation and checks it as
/// `author`'s, within `limits`. The crate's documentation gives the
/// format and the checks. The work is proportional to the file's size,
/// which `limits` bounds before any of it is read.
///
/// # Errors
/// The first mistake in `src`, with its line.
pub fn parse(src: &str, author: Author<'_>, limits: &Limits) -> Result<Definition, Error> {
    Ok(read(src, author, limits)?)
}

fn read(src: &str, author: Author<'_>, limits: &Limits) -> Result<Definition, Mistake> {
    if src.len() > limits.file_bytes {
        return err(
            1,
            format!(
                "the file is {} bytes long, and the limit is {}",
                src.len(),
                limits.file_bytes
            ),
        );
    }
    let mut p = Parser {
        toks: lex(src)?,
        at: 0,
        lines: Lines::default(),
        inner: Vec::new(),
        depth: 0,
        max_depth: limits.depth.min(Limits::MAX_DEPTH),
    };
    let (root, interfaces, kinds) = p.file()?;
    check::build(root, interfaces, kinds, author, limits, Notation::Dts).map_err(|problem| {
        Mistake {
            line: p.lines.of(problem.site),
            message: problem.message,
        }
    })
}

// ----------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok<'a> {
    Ident(&'a str),
    Str(String),
    /// A doc comment's text, between `/**` and `*/`, its lines ending in
    /// line feeds.
    Doc(String),
    /// Any other character.
    P(char),
}

fn line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The text being read, and the line the reader is on.
struct Cursor<'a> {
    rest: &'a str,
    line: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }
    fn starts(&self, s: &str) -> bool {
        self.rest.starts_with(s)
    }
    /// The next character, as the file's text has it: a line end of any
    /// form is one `\n`. A control character other than a tab or a line
    /// end is refused wherever it is, and so is a character that shows
    /// nothing or changes the direction of text ([`check::hidden`]): in
    /// a comment too, which a reviewer reads with the declarations
    /// around it.
    fn bump(&mut self) -> Result<Option<char>, Mistake> {
        let mut chars = self.rest.chars();
        let Some(c) = chars.next() else {
            return Ok(None);
        };
        self.rest = chars.as_str();
        if line_end(c) {
            if c == '\r' && self.rest.starts_with('\n') {
                self.rest = &self.rest[1..];
            }
            self.line += 1;
            return Ok(Some('\n'));
        }
        if c.is_control() && c != '\t' {
            return err(
                self.line,
                format!(
                    "the control character U+{:04X} is not part of a surface file: it holds text, tabs and line ends",
                    c as u32
                ),
            );
        }
        if check::hidden(c) {
            return err(
                self.line,
                format!(
                    "the character U+{:04X} is not part of a surface file, in a comment either: it shows nothing or changes the direction of the text around it, so a reader of the file would not see what it holds",
                    c as u32
                ),
            );
        }
        Ok(Some(c))
    }
    fn skip(&mut self, n: usize) {
        self.rest = &self.rest[n..];
    }
}

fn lex(src: &str) -> Result<Vec<(Tok<'_>, usize)>, Mistake> {
    let mut cur = Cursor {
        rest: src.strip_prefix('\u{feff}').unwrap_or(src),
        line: 1,
    };
    let mut toks = Vec::new();
    while let Some(c) = cur.peek() {
        let line = cur.line;
        if cur.starts("//") {
            while cur.peek().is_some_and(|c| !line_end(c)) {
                cur.bump()?;
            }
        } else if cur.starts("/*") {
            let doc = cur.starts("/**") && !cur.starts("/**/");
            cur.skip(if doc { 3 } else { 2 });
            let mut text = String::new();
            loop {
                if cur.starts("*/") {
                    cur.skip(2);
                    break;
                }
                match cur.bump()? {
                    Some(c) => text.push(c),
                    None => return err(line, "this comment is not closed: end it with `*/`"),
                }
            }
            if doc {
                toks.push((Tok::Doc(text), line));
            }
        } else if c == '"' {
            cur.skip(1);
            let mut s = String::new();
            loop {
                match cur.peek() {
                    None => {
                        return err(
                            line,
                            "this string is not closed: end it with `\"` on the same line",
                        );
                    }
                    Some(c) if line_end(c) => {
                        return err(
                            line,
                            "this string is not closed: end it with `\"` on the same line",
                        );
                    }
                    Some('"') => {
                        cur.skip(1);
                        break;
                    }
                    Some('\\') => {
                        cur.skip(1);
                        match cur.peek() {
                            Some(e @ ('"' | '\\')) => {
                                s.push(e);
                                cur.skip(1);
                            }
                            _ => {
                                return err(
                                    line,
                                    "a string literal may escape only `\\\"` and `\\\\`",
                                );
                            }
                        }
                    }
                    Some(_) => {
                        if let Some(c) = cur.bump()? {
                            s.push(c);
                        }
                    }
                }
            }
            toks.push((Tok::Str(s), line));
        } else if c == '\'' || c == '`' {
            return err(
                line,
                format!("a string literal is written in double quotes, not with `{c}`"),
            );
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let len = cur
                .rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
                .unwrap_or(cur.rest.len());
            toks.push((Tok::Ident(&cur.rest[..len]), line));
            cur.skip(len);
        } else if c.is_whitespace() {
            cur.bump()?;
        } else if let Some(c) = cur.bump()? {
            toks.push((Tok::P(c), line));
        }
    }
    Ok(toks)
}

// ------------------------------------------------------------ doc comments

/// A tag, the words after it, and the line each is on.
struct Tag {
    name: String,
    words: Vec<(String, usize)>,
    line: usize,
}

/// A doc comment: its prose, then its tags.
pub(crate) struct Doc {
    pub prose: String,
    tags: Vec<Tag>,
}

const TAGS: &[&str] = &[
    "read", "write", "effect", "spend", "waits", "once", "throws", "allow",
];

/// Reads a doc comment that starts on `start`.
///
/// A line after the first that begins, after its indentation, with `* `
/// (or is `*` alone) loses that much, and keeps the indentation it has
/// after it; any other line is taken as it stands, without the white
/// space around it. Blank lines at either end are dropped. A word that
/// begins with `@` begins a tag, and everything after the first tag
/// belongs to tags: the words up to the next tag are the tag's own.
///
/// It answers with the comment and how many steps reading it took: one
/// for each of its lines, and one for each blank line dropped from
/// either end.
pub(crate) fn read_doc(raw: &str, start: usize) -> Result<(Doc, usize), Mistake> {
    let mut prose: Vec<&str> = Vec::new();
    let mut tags: Vec<Tag> = Vec::new();
    let mut steps = 0;
    for (n, l) in raw.split('\n').enumerate() {
        steps += 1;
        let trimmed = l.trim();
        let l = if n == 0 {
            trimmed
        } else if trimmed == "*" {
            ""
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            rest
        } else {
            trimmed
        };
        let in_prose = tags.is_empty();
        let mut prose_end = l.len();
        let mut at = 0;
        for w in l.split_whitespace() {
            // Where this word starts in `l`.
            at += l[at..].find(w).unwrap_or(0);
            if let Some(name) = w.strip_prefix('@') {
                if tags.is_empty() {
                    prose_end = at;
                }
                if !TAGS.contains(&name) {
                    return err(
                        start + n,
                        format!(
                            "`{}` is not a tag of the format, which has @read, @write, @effect, @spend, @waits, @once, @throws and @allow; write a word that starts with `@` in backticks to keep it as prose",
                            check::shown(w)
                        ),
                    );
                }
                tags.push(Tag {
                    name: name.to_string(),
                    words: Vec::new(),
                    line: start + n,
                });
            } else if let Some(t) = tags.last_mut() {
                t.words.push((w.to_string(), start + n));
            }
            at += w.len();
        }
        if in_prose {
            prose.push(l[..prose_end].trim_end());
        }
    }
    // The lines between the first and the last that are not blank.
    let from = prose.iter().take_while(|l| l.is_empty()).count();
    let kept = &mut prose[from..];
    let blank_after = kept.iter().rev().take_while(|l| l.is_empty()).count();
    let until = kept.len() - blank_after;
    let kept = &mut kept[..until];
    steps += from + blank_after;
    if let Some(first) = kept.first_mut() {
        *first = first.trim_start();
    }
    let doc = Doc {
        prose: kept.join("\n"),
        tags,
    };
    Ok((doc, steps))
}

/// The prose of a doc comment that takes no tags; none when it is empty.
fn plain(doc: Option<Doc>, what: &str) -> Result<Option<String>, Mistake> {
    match doc {
        None => Ok(None),
        Some(d) => match d.tags.first() {
            Some(t) => err(
                t.line,
                format!(
                    "`@{}` does not belong in {what}: only operations and kinds take tags",
                    t.name
                ),
            ),
            None => Ok(Some(d.prose).filter(|p| !p.is_empty())),
        },
    }
}

/// A tag that says something by being there.
fn flag(tag: &Tag, seen: bool) -> Result<bool, Mistake> {
    if seen {
        return err(tag.line, format!("`@{}` is given twice", tag.name));
    }
    match tag.words.first() {
        Some((w, line)) => err(
            *line,
            format!(
                "`@{}` takes no words, and `{}` follows it: tags come after the prose",
                tag.name,
                check::shown(w)
            ),
        ),
        None => Ok(true),
    }
}

// ---------------------------------------------------------------- lines

/// Where a type is written: its first line, and the line of each field
/// of the object types written in it, in the order written.
#[derive(Default)]
struct TyLines {
    line: usize,
    inner: Vec<usize>,
}

struct OpLines {
    line: usize,
    /// The line of each error code.
    codes: Vec<usize>,
    params: Vec<TyLines>,
    result: TyLines,
}

struct KindLines {
    line: usize,
    request: TyLines,
    message: Option<TyLines>,
    result: TyLines,
}

/// The line of each declaration, by its [`Site`].
#[derive(Default)]
struct Lines {
    surface: usize,
    namespaces: Vec<usize>,
    ops: Vec<OpLines>,
    interfaces: Vec<(usize, Vec<TyLines>)>,
    kinds: Vec<KindLines>,
}

impl Lines {
    fn of(&self, site: Site) -> usize {
        let (line, ty): (Option<usize>, Option<&TyLines>) = match site.decl {
            Decl::Surface | Decl::Rules | Decl::Members => (Some(self.surface), None),
            Decl::Namespace(n) | Decl::NamespaceRules(n) => (self.namespaces.get(n).copied(), None),
            Decl::Op(n) => (self.ops.get(n).map(|o| o.line), None),
            Decl::Code(n, k) => (
                self.ops
                    .get(n)
                    .map(|o| o.codes.get(k).copied().unwrap_or(o.line)),
                None,
            ),
            Decl::Param(n, k) => (None, self.ops.get(n).and_then(|o| o.params.get(k))),
            Decl::Result(n) => (None, self.ops.get(n).map(|o| &o.result)),
            Decl::Interface(n) | Decl::Extends(n, _) => (self.interfaces.get(n).map(|i| i.0), None),
            Decl::Field(n, k) => (None, self.interfaces.get(n).and_then(|i| i.1.get(k))),
            Decl::Kind(n) => (self.kinds.get(n).map(|k| k.line), None),
            Decl::Request(n) => (None, self.kinds.get(n).map(|k| &k.request)),
            Decl::Message(n) => (None, self.kinds.get(n).and_then(|k| k.message.as_ref())),
            Decl::KindResult(n) => (None, self.kinds.get(n).map(|k| &k.result)),
        };
        let inner = ty.and_then(|t| site.inner.and_then(|i| t.inner.get(i).copied()));
        inner
            .or(ty.map(|t| t.line))
            .or(line)
            .unwrap_or(self.surface)
    }
}

// ---------------------------------------------------------------- parser

const TYPES: &str = "the format's types are string, number, Int, boolean, null, unknown, string literals, true, false, arrays (`T[]`), `Record<string, T>`, object types, unions, the file's interfaces, and Instruction, Content and Image";

/// The parts of a definition, as [`check::build`] takes them.
type Parts = (Namespace, Vec<Interface>, Vec<Kind>);

struct Parser<'a> {
    toks: Vec<(Tok<'a>, usize)>,
    at: usize,
    lines: Lines,
    /// The lines of the object-type fields of the type being read.
    inner: Vec<usize>,
    /// How many namespaces, or how many types, enclose what is being
    /// read.
    depth: usize,
    max_depth: usize,
}

impl Parser<'_> {
    /// The line of the next token, or of the last when none is left.
    fn line(&self) -> usize {
        self.toks
            .get(self.at)
            .or(self.toks.last())
            .map_or(1, |t| t.1)
    }
    fn peek(&self) -> Option<&Tok<'_>> {
        self.toks.get(self.at).map(|t| &t.0)
    }
    fn is(&self, c: char) -> bool {
        self.peek() == Some(&Tok::P(c))
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if *s == w)
    }
    fn eat(&mut self, c: char) -> bool {
        let yes = self.is(c);
        if yes {
            self.at += 1;
        }
        yes
    }
    fn found(&self) -> String {
        match self.peek() {
            None => "the end of the file".into(),
            Some(Tok::Ident(s)) => format!("`{}`", check::shown(s)),
            Some(Tok::Str(s)) => format!("\"{}\"", check::shown(s)),
            Some(Tok::Doc(_)) => "a doc comment".into(),
            // What a reader of the message would not see is named.
            Some(Tok::P(c)) if c.is_whitespace() || check::unshowable(*c) => {
                format!("the character U+{:04X}", *c as u32)
            }
            Some(Tok::P(c)) => format!("`{c}`"),
        }
    }
    fn want(&mut self, c: char) -> Result<(), Mistake> {
        if self.eat(c) {
            Ok(())
        } else {
            err(
                self.line(),
                format!("expected `{c}`, found {}", self.found()),
            )
        }
    }
    fn ident(&mut self, what: &str) -> Result<String, Mistake> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = (*s).to_string();
                self.at += 1;
                Ok(s)
            }
            _ => err(
                self.line(),
                format!("expected {what}, found {}", self.found()),
            ),
        }
    }
    fn word(&mut self, w: &str) -> Result<(), Mistake> {
        if self.is_word(w) {
            self.at += 1;
            Ok(())
        } else {
            err(
                self.line(),
                format!("expected `{w}`, found {}", self.found()),
            )
        }
    }
    /// The doc comment before a declaration, if there is one.
    fn doc(&mut self) -> Result<Option<Doc>, Mistake> {
        let Some(Tok::Doc(raw)) = self.peek() else {
            return Ok(None);
        };
        let (doc, _) = read_doc(raw, self.line())?;
        self.at += 1;
        if matches!(self.peek(), Some(Tok::Doc(_))) {
            return err(
                self.line(),
                "two doc comments in a row: a declaration takes one (a `//` comment is not part of a definition)",
            );
        }
        Ok(Some(doc))
    }
    /// The end of a member: `;` or `,`, or nothing before `}` or before
    /// a line break, as TypeScript ends a member of a type: the next
    /// token starts on a later line than the member's last.
    fn end_member(&mut self) -> Result<(), Mistake> {
        let after_a_break = match (self.toks.get(self.at), self.at.checked_sub(1)) {
            (Some(next), Some(last)) => next.1 > self.toks[last].1,
            _ => false,
        };
        if self.eat(';') || self.eat(',') || self.is('}') || after_a_break {
            Ok(())
        } else {
            err(
                self.line(),
                format!(
                    "expected `;`, `,`, a line break or `}}` after a member, found {}",
                    self.found()
                ),
            )
        }
    }
    fn no_generics(&self, name: &str) -> Result<(), Mistake> {
        if self.is('<') {
            return err(
                self.line(),
                format!(
                    "`{}<…>`: the format has no generics but `Record<string, T>`",
                    check::shown(name)
                ),
            );
        }
        Ok(())
    }
    /// One level further in: refused past the limit, so that neither this
    /// parser nor anything that walks what it builds can be made to nest
    /// without bound. `more` counts levels a type has above what is being
    /// read (its `[]`s).
    fn deeper(&mut self, more: usize) -> Result<(), Mistake> {
        if self.depth + more > self.max_depth {
            return err(
                self.line(),
                format!(
                    "nested more than {} deep, the limit for namespaces and for types",
                    self.max_depth
                ),
            );
        }
        Ok(())
    }

    fn file(&mut self) -> Result<Parts, Mistake> {
        let mut root = None;
        let mut interfaces = Vec::new();
        let mut kinds = None;
        while self.peek().is_some() {
            let doc = self.doc()?;
            let line = self.line();
            match self.ident("`declare const` or `interface`")?.as_str() {
                "declare" => {
                    if root.is_some() {
                        return err(
                            line,
                            "a second `declare const`: a surface file declares one surface",
                        );
                    }
                    self.word("const")?;
                    self.lines.surface = self.line();
                    let name = self.ident("the surface's name")?;
                    self.want(':')?;
                    let members = self.members()?;
                    self.eat(';');
                    let rules = plain(doc, "a surface's rules")?;
                    root = Some(Namespace {
                        name,
                        rules,
                        members,
                    });
                }
                "interface" => {
                    let name_line = self.line();
                    let name = self.ident("the interface's name")?;
                    self.no_generics(&name)?;
                    if name == "Kinds" {
                        if kinds.is_some() {
                            return err(
                                name_line,
                                "a second `interface Kinds`: a surface file declares its kinds once",
                            );
                        }
                        if doc.is_some() {
                            return err(
                                line,
                                "`interface Kinds` takes no doc comment: each kind has its own",
                            );
                        }
                        if self.is_word("extends") {
                            return err(
                                name_line,
                                "`interface Kinds` extends nothing: it holds the surface's kinds",
                            );
                        }
                        kinds = Some(self.kinds()?);
                        continue;
                    }
                    let mut extends = Vec::new();
                    if self.is_word("extends") {
                        self.at += 1;
                        loop {
                            let base = self.ident("an interface's name")?;
                            self.no_generics(&base)?;
                            extends.push(base);
                            if !self.eat(',') {
                                break;
                            }
                        }
                    }
                    let (fields, lines, _) = self.fields(true)?;
                    self.lines.interfaces.push((name_line, lines));
                    let doc = plain(doc, "an interface's comment")?;
                    interfaces.push(Interface {
                        name,
                        doc,
                        extends,
                        fields,
                    });
                }
                "type" => {
                    return err(
                        line,
                        "`type` aliases are not part of the format: declare an `interface`, or write the type where it is used",
                    );
                }
                other => {
                    return err(
                        line,
                        format!(
                            "`{}` is not part of a surface file, which holds one `declare const <name>: {{ … }}` and `interface`s",
                            check::shown(other)
                        ),
                    );
                }
            }
        }
        let Some(root) = root else {
            return err(
                self.line(),
                "the file declares no surface: write `declare const <name>: { … };`",
            );
        };
        Ok((root, interfaces, kinds.unwrap_or_default()))
    }

    fn members(&mut self) -> Result<Vec<Member>, Mistake> {
        self.want('{')?;
        let mut out = Vec::new();
        while !self.eat('}') {
            let doc = self.doc()?;
            let line = self.line();
            let name = self.ident("an operation's or a namespace's name")?;
            if self.is('?') {
                return err(
                    line,
                    format!(
                        "`{}?`: a surface's members are not optional; a run has an operation or does not, by its allow lists",
                        check::shown(&name)
                    ),
                );
            }
            if self.eat(':') {
                if !self.is('{') {
                    return err(
                        line,
                        format!(
                            "`{0}:` must open a namespace (`{0}: {{ … }}`); an operation is written as a method, `{0}(…): Promise<T>;`",
                            check::shown(&name)
                        ),
                    );
                }
                self.lines.namespaces.push(line);
                let rules = plain(doc, "a namespace's rules")?;
                self.depth += 1;
                self.deeper(0)?;
                let members = self.members()?;
                self.depth -= 1;
                out.push(Member::Namespace(Namespace {
                    name,
                    rules,
                    members,
                }));
            } else {
                // Its place is kept before its types are read, so that the
                // operations' lines stay in declaration order.
                let at = self.lines.ops.len();
                self.lines.ops.push(OpLines {
                    line,
                    codes: Vec::new(),
                    params: Vec::new(),
                    result: TyLines::default(),
                });
                let (op, lines) = self.op(name, line, doc)?;
                self.lines.ops[at] = lines;
                out.push(Member::Op(op));
            }
            self.end_member()?;
        }
        Ok(out)
    }

    fn op(
        &mut self,
        name: String,
        line: usize,
        doc: Option<Doc>,
    ) -> Result<(Op, OpLines), Mistake> {
        let shown = check::shown(&name);
        let (params, param_lines) = self.params()?;
        self.want(':')?;
        if !self.is_word("Promise") {
            return err(
                self.line(),
                format!(
                    "operation `{shown}` must return `Promise<T>` (every operation is async), found {}",
                    self.found()
                ),
            );
        }
        self.at += 1;
        self.want('<')?;
        let (result, result_lines) = self.ty_at()?;
        self.want('>')?;
        let Some(doc) = doc.filter(|d| !d.prose.is_empty()) else {
            return err(
                line,
                format!(
                    "operation `{shown}` has no documentation: write a doc comment above it that says what it does, then its tags (`/** What it does. @read */`)"
                ),
            );
        };
        let mut access = Vec::new();
        let (mut waits, mut once, mut throws): (bool, bool, Vec<String>) =
            (false, false, Vec::new());
        let mut codes = Vec::new();
        for t in &doc.tags {
            match t.name.as_str() {
                "read" | "write" | "effect" | "spend" => {
                    flag(t, false)?;
                    access.push(match t.name.as_str() {
                        "read" => Access::Read,
                        "write" => Access::Write,
                        "effect" => Access::Effect,
                        _ => Access::Spend,
                    });
                }
                "waits" => waits = flag(t, waits)?,
                "once" => once = flag(t, once)?,
                // One code a tag. Whether an operation names a code
                // twice, or too many, is the definition's to check.
                "throws" => match t.words.as_slice() {
                    [] => {
                        return err(
                            t.line,
                            "`@throws` names no error code: write `@throws not_found`, and `@throws` again for each further code",
                        );
                    }
                    [(w, line), rest @ ..] => {
                        if !check::error_code(w) {
                            return err(
                                *line,
                                format!(
                                    "`{}` is not an error code: a code is lowercase letters, digits and `_`, and starts with a letter. `@throws` takes one code, and prose goes before the tags",
                                    check::shown(w)
                                ),
                            );
                        }
                        if let Some((next, line)) = rest.first() {
                            return err(
                                *line,
                                format!(
                                    "`@throws` takes one code, and `{}` follows it: write `@throws` again for each further code (`@throws not_found @throws forbidden`); prose goes before the tags",
                                    check::shown(next)
                                ),
                            );
                        }
                        throws.push(w.clone());
                        codes.push(*line);
                    }
                },
                other => {
                    return err(
                        t.line,
                        format!("`@{other}` belongs on a kind, not on an operation"),
                    );
                }
            }
        }
        let [access] = access.as_slice() else {
            let has = match access.as_slice() {
                [] => "none".to_string(),
                many => many
                    .iter()
                    .map(|a| format!("@{}", a.tag()))
                    .collect::<Vec<_>>()
                    .join(" and "),
            };
            return err(
                line,
                format!(
                    "operation `{shown}` needs exactly one of @read, @write, @effect and @spend, and has {has}"
                ),
            );
        };
        let op = Op {
            name,
            doc: doc.prose,
            access: *access,
            waits,
            once,
            throws,
            params,
            result,
        };
        let lines = OpLines {
            line,
            codes,
            params: param_lines,
            result: result_lines,
        };
        Ok((op, lines))
    }

    fn params(&mut self) -> Result<(Vec<Param>, Vec<TyLines>), Mistake> {
        self.want('(')?;
        let mut out: Vec<Param> = Vec::new();
        let mut lines = Vec::new();
        while !self.eat(')') {
            let line = self.line();
            let name = self.ident("a parameter's name")?;
            let optional = self.eat('?');
            self.want(':')?;
            let (ty, mut ty_lines) = self.ty_at()?;
            ty_lines.line = line;
            out.push(Param { name, optional, ty });
            lines.push(ty_lines);
            if !self.eat(',') && !self.is(')') {
                return err(
                    self.line(),
                    format!("expected `,` or `)`, found {}", self.found()),
                );
            }
        }
        Ok((out, lines))
    }

    fn kinds(&mut self) -> Result<Vec<Kind>, Mistake> {
        self.want('{')?;
        let mut out = Vec::new();
        while !self.eat('}') {
            let doc = self.doc()?;
            let line = self.line();
            let name = self.ident("a kind's name")?;
            let shown = check::shown(&name);
            if !self.is('(') {
                return err(
                    line,
                    format!(
                        "`{shown}` is not a kind: `interface Kinds` holds one method a kind, `{shown}(request: T): R;`"
                    ),
                );
            }
            let shape = format!(
                "`{shown}(request: T): R`, or `{shown}(request: T, message?: M): R` for a kind that takes messages after it starts"
            );
            let (params, lines) = self.params()?;
            let (mut params, mut lines) = (params.into_iter(), lines.into_iter());
            let (request, message) = match (params.next(), params.next(), params.next()) {
                (Some(request), message, None) => (request, message),
                (None, ..) => {
                    return err(
                        line,
                        format!("kind `{shown}` takes no request: a kind is {shape}"),
                    );
                }
                _ => {
                    return err(
                        line,
                        format!("kind `{shown}` takes more than two parameters: a kind is {shape}"),
                    );
                }
            };
            if request.optional || request.name != "request" {
                return err(
                    line,
                    format!(
                        "the first parameter of kind `{shown}` must be `request`, and not optional: a kind is {shape}"
                    ),
                );
            }
            if let Some(m) = &message
                && (!m.optional || m.name != "message")
            {
                return err(
                    line,
                    format!(
                        "the second parameter of kind `{shown}` must be `message?`, optional because a run starts with its request alone: a kind is {shape}"
                    ),
                );
            }
            self.want(':')?;
            let (result, result_lines) = self.ty_at()?;
            self.end_member()?;
            let Some(doc) = doc.filter(|d| !d.prose.is_empty()) else {
                return err(
                    line,
                    format!(
                        "kind `{shown}` has no documentation: its doc comment is the prompt's paragraph about the request"
                    ),
                );
            };
            let mut allow: Option<Vec<Pattern>> = None;
            for t in &doc.tags {
                if t.name != "allow" {
                    return err(
                        t.line,
                        format!(
                            "`@{}` belongs on an operation; a kind takes only @allow",
                            t.name
                        ),
                    );
                }
                // `@allow` alone is a list that allows nothing.
                let list = allow.get_or_insert_default();
                for (w, line) in &t.words {
                    list.push(Pattern::parse(w).map_err(|message| Mistake {
                        line: *line,
                        message: format!(
                            "{message}. Everything after a doc comment's first tag is tags: prose goes before them"
                        ),
                    })?);
                }
            }
            self.lines.kinds.push(KindLines {
                line,
                request: lines.next().unwrap_or_default(),
                message: lines.next(),
                result: result_lines,
            });
            out.push(Kind {
                name,
                doc: doc.prose,
                allow,
                request: request.ty,
                message: message.map(|m| m.ty),
                result,
            });
        }
        Ok(out)
    }

    /// An object's fields, and how many levels the deepest of their
    /// types nests. `interface`: whether they are an interface's, which
    /// may have doc comments and whose lines are returned; an inline
    /// object type's go to the type being read.
    fn fields(&mut self, interface: bool) -> Result<(Vec<Field>, Vec<TyLines>, usize), Mistake> {
        self.want('{')?;
        let mut out = Vec::new();
        let mut lines = Vec::new();
        let mut height = 0;
        while !self.eat('}') {
            let doc_line = self.line();
            let doc = self.doc()?;
            let line = self.line();
            let name = self.ident("a field's name")?;
            if self.is('(') {
                return err(
                    line,
                    format!(
                        "`{}(…)` is a method: only the surface (`declare const`) and `interface Kinds` hold methods",
                        check::shown(&name)
                    ),
                );
            }
            let optional = self.eat('?');
            self.want(':')?;
            let ty = if interface {
                let (ty, mut ty_lines) = self.ty_at()?;
                ty_lines.line = line;
                lines.push(ty_lines);
                ty
            } else {
                self.inner.push(line);
                let (ty, h) = self.ty()?;
                height = height.max(h);
                ty
            };
            let doc = plain(doc, "a field's comment")?;
            if doc.is_some() && !interface {
                return err(
                    doc_line,
                    "a field of an object type written inline takes no doc comment: declare an `interface` to document its fields",
                );
            }
            out.push(Field {
                name,
                optional,
                ty,
                doc,
            });
            self.end_member()?;
        }
        Ok((out, lines, height))
    }

    /// A type where a declaration has one, with the lines it is written
    /// on.
    fn ty_at(&mut self) -> Result<(Ty, TyLines), Mistake> {
        let line = self.line();
        let outer_inner = std::mem::take(&mut self.inner);
        let outer_depth = std::mem::replace(&mut self.depth, 0);
        let ty = self.ty()?.0;
        self.depth = outer_depth;
        let inner = std::mem::replace(&mut self.inner, outer_inner);
        Ok((ty, TyLines { line, inner }))
    }

    /// A type, and how many levels it nests.
    fn ty(&mut self) -> Result<(Ty, usize), Mistake> {
        self.eat('|');
        let (first, mut height) = self.postfix()?;
        let mut rest = Vec::new();
        while self.eat('|') {
            let (t, h) = self.postfix()?;
            height = height.max(h);
            rest.push(t);
        }
        Ok((Ty::union(first, rest), height))
    }

    fn postfix(&mut self) -> Result<(Ty, usize), Mistake> {
        let (mut t, mut height) = self.primary()?;
        while self.is('[') {
            height += 1;
            self.deeper(height)?;
            self.at += 1;
            self.want(']')?;
            t = Ty::Array(Box::new(t));
        }
        Ok((t, height))
    }

    /// What a type inside this one is read with: one level in.
    fn inside<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, Mistake>,
    ) -> Result<T, Mistake> {
        self.depth += 1;
        self.deeper(0)?;
        let out = read(self)?;
        self.depth -= 1;
        Ok(out)
    }

    fn primary(&mut self) -> Result<(Ty, usize), Mistake> {
        let line = self.line();
        match self.peek() {
            Some(Tok::P('{')) => {
                let (fields, _, height) = self.inside(|p| p.fields(false))?;
                Ok((Ty::Object(fields), height + 1))
            }
            Some(Tok::P('(')) => {
                self.at += 1;
                let (t, height) = self.inside(Self::ty)?;
                self.want(')')?;
                Ok((t, height))
            }
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.at += 1;
                Ok((Ty::StrLit(s), 0))
            }
            Some(Tok::Ident(name)) => {
                let name = (*name).to_string();
                self.at += 1;
                if name != "Record" {
                    self.no_generics(&name)?;
                }
                let t = match name.as_str() {
                    "string" => Ty::Str,
                    "number" => Ty::Num,
                    "boolean" => Ty::Bool,
                    "null" => Ty::Null,
                    "unknown" => Ty::Unknown,
                    "true" => Ty::BoolLit(true),
                    "false" => Ty::BoolLit(false),
                    "Int" => Ty::Int,
                    "Instruction" => Ty::Instruction,
                    "Content" => Ty::Content,
                    "Image" => Ty::Image,
                    "Record" => {
                        self.want('<')?;
                        if !self.is_word("string") {
                            return err(
                                self.line(),
                                format!(
                                    "a `Record`'s keys are `string` (`Record<string, T>`), found {}",
                                    self.found()
                                ),
                            );
                        }
                        self.at += 1;
                        self.want(',')?;
                        let (v, height) = self.inside(Self::ty)?;
                        self.want('>')?;
                        return Ok((Ty::Record(Box::new(v)), height + 1));
                    }
                    "any" | "never" | "void" | "undefined" | "object" | "symbol" | "bigint"
                    | "Array" | "Promise" | "Kinds" | "keyof" | "typeof" | "readonly" => {
                        return err(
                            line,
                            format!("`{name}` is not a type of the format: {TYPES}"),
                        );
                    }
                    _ => Ty::Named(name),
                };
                Ok((t, 0))
            }
            _ => err(
                line,
                format!("expected a type, found {}: {TYPES}", self.found()),
            ),
        }
    }
}
