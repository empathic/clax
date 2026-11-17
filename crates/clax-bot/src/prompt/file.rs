//! A prompt file: the words a surface supplies for runs of its kinds
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "The
//! prompt"), as text with slots that a run fills. This module reads the
//! format and renders it; it knows no surface's nouns.
//!
//! # The format
//!
//! The file is UTF-8 text of at most [`MAX_BYTES`] (64 KiB, the limit of
//! a surface file), with `\n` line endings, ending in one, and no byte
//! order mark before its first line. [`PromptFile::parse`] refuses a
//! longer text before it reads any of it. It is given text, though:
//! whoever loads a file's bytes (a host's, when a host registers one)
//! must refuse more than [`MAX_BYTES`] of them, and bytes that are not
//! UTF-8, before there is a text to parse.
//!
//! The file holds no character that shows as nothing or changes the
//! direction of text, as a surface file holds none
//! ([`clax_surface::hidden`]: zero-width characters, marks of text
//! direction, variation selectors, tag characters and the rest), no
//! control character but the tab, and no line or paragraph separator,
//! so that what a model reads is what whoever reviewed the file saw.
//! Each is refused on its line, by its code point. Clax's own files
//! (`src/clax.prompt.md`, `src/standard.prompt.md`) are held to it as a
//! host's is.
//!
//! A line that is exactly `--- <name> ---` is a **header**: it starts a
//! section, which runs to the next header or the end of the file. A
//! section's text is its lines without the last one's line ending. Lines
//! before the first header are a note for people and no part of any
//! prompt.
//!
//! A header matches exactly, wherever it stands: no section can hold a
//! line that is a header, and neither can the note, which the first
//! header ends. A line that a reader cannot tell from a header is
//! refused, on its own line: one that is a header but for white space
//! before or after it, for a character that shows as nothing (a
//! zero-width space, a byte order mark) or for a space that is not the
//! plain one (a no-break space) in the place of one. Any other line is
//! text, `---system---` and `---` too.
//!
//! | Header | Its section is | How many |
//! |---|---|---|
//! | `--- system ---` | The system prompt | One |
//! | `--- request ---` | The first user message, around the request | One |
//! | `--- opener <kind> ---` | The sentence that opens the request message of a run of `<kind>` | One for each kind of the surface when the request section has `{opener}`, else none |
//!
//! A **slot** is `{name}`. A brace that is text is written twice: `{{`
//! is `{` and `}}` is `}`. Any other brace is an error. Whose a slot is
//! shows in its name, so that no surface's slot can stand where a reader
//! sees the core's:
//!
//! - **A name with no `.`** is the format's: lowercase letters and `_`.
//! - **`core.<name>` and `rules.<namespace>`** are the core's. The part
//!   after `rules.` is a namespace's name, an identifier as a surface
//!   file has them (ASCII letters of either case, digits, `_` and `$`).
//!   No prompt file is read for a surface named `core` or `rules`.
//! - **Any other name with a `.`** is `<surface>.<name>`, in lowercase
//!   letters, digits, `_` and `.`, for the surface the file is read for
//!   ([`Known::surface`]), and is supplied by code that ships with that
//!   surface ([`Known::own`]). A host that registers its surface has no
//!   code in the daemon, so its prompt file has none.
//!
//! | Section | Slot | Becomes | How many |
//! |---|---|---|---|
//! | system | `{kind}` | The kind's paragraph: its doc comment in the surface file | Any |
//! | system | `{declarations}` | The one `declare const clax` as the model reads it, without its last line ending | One |
//! | system | `{surfaces}` | For each other binding of the run, an empty line, `# <name>`, an empty line and its rules, ending a line ([`surfaces`]); nothing when there is none | One: the section's last line, alone on it |
//! | system | `{rules}` | The rules of the surface itself that the run has and the file places nowhere else, each after an empty line ([`rules`]): the surface's own (its `declare const`'s doc comment), then, in declaration order, those of each of its namespaces (one inside another too) that the run has an operation of, but for a namespace the file places by name. Nothing when there is none | At most one |
//! | system | `{rules.<namespace>}` | The rules of a namespace directly on the surface (its doc comment in the surface file) in a run that has one of its operations; nothing in a run that has none | Any |
//! | system | `{core.code_mode}`, `{core.data}`, `{core.messages}`, `{core.images}`, `{core.finish}` | The core's standard paragraphs ([`super::standard`]); `{core.messages}` is nothing in a run whose kind takes no messages, `{core.images}` in a run that can be shown no image | Any |
//! | system | `{<surface>.<name>}` | What the surface's own code gives it | Any |
//! | request | `{opener}` | The opener section of the run's kind | Any |
//! | request | `{instructions}` | Which of the request's strings are the words of the person the run acts for, in the core's words ([`super::standard::instructions`]): the JSON Pointer of each, in a fenced block before the request, or that there is none | At most one, on a line before `{request}`'s |
//! | request | `{fence}` | A run of backticks longer than any in the request's JSON and in what `{instructions}` lists, and at least three: the fence of a code block that holds them | Any |
//! | request | `{request}` | The run's request, as the text of a JSON document, written by what renders the section | One |
//! | opener | none | | |
//!
//! These rules hold for every file:
//!
//! - **What a prompt cannot do without is required.** A system section
//!   with no `{declarations}` or no `{surfaces}`, or with no words of
//!   its own, and a request section with no `{request}`, are refused
//!   when the file is read: the API, the rules of a surface that joined
//!   the run, or the request itself would be left out of what a model
//!   is sent with nobody the wiser.
//! - **Every rule of the surface has a place.** A file with `{rules}`
//!   has one for all of them. A file without it is refused when the
//!   surface has rules of its own, a namespace inside another that has
//!   rules, or a namespace directly on it whose rules the file does not
//!   place with `{rules.<namespace>}`. This module does not know the
//!   surface's rules; what reads a file for a surface holds the rule
//!   (`Prompt::read` in `prompt.rs`).
//! - **A slot nothing supplies is an error when the file is read**,
//!   naming the slot and its line; so is a section that is missing, given
//!   twice or not one of the three, an opener for a kind the surface does
//!   not have, one that is empty or only white space, and a kind without
//!   one.
//! - **A line whose slots are all empty is left out, with its line
//!   ending** (a line with no slot is always kept). So `- {rules.web}`
//!   is a bullet in runs with a web operation and nothing in the others.
//!   A section's last line has no line ending of its own: when it is
//!   left out, the line ending of the line before it stays, and the text
//!   ends with it.
//! - **`{surfaces}` is the system section's last line, alone on it.**
//!   What it becomes begins with an empty line and ends its last line.
//!   So after the section's last line of text it adds whole paragraphs
//!   to a run with other bindings, and nothing to a run with none (the
//!   rule above), and the system prompt ends with a line ending either
//!   way. Between two paragraphs no placement is right in both runs:
//!   one of them would lose the empty line between the paragraphs or
//!   have it twice.
//! - **`{rules}` stands on a line of its own after a line of text**,
//!   where an empty line would otherwise follow: what it becomes begins
//!   with the empty line that sets it apart, so a run with no such rules
//!   has no empty line too many. That is advice, not checked.
//! - **Whose words a request holds is said before it.** `{instructions}`
//!   stands on a line before `{request}`'s, so what the request holds
//!   comes after what is said of it. A file read for a surface a host
//!   registered must have it (`check_prompt_file` in `prompt.rs`): without
//!   it the person's words would reach the model unmarked, as data.
//! - **A slot is replaced once, in the file's own text.** What a slot
//!   becomes is written as it is: a brace in it is a brace, and text in
//!   it that looks like a slot stays that text.

use super::standard::CorePiece;
use std::fmt;

/// The longest prompt file that is read, in bytes: 64 KiB, the default
/// limit of a surface file (`clax_surface::Limits::file_bytes`).
pub const MAX_BYTES: usize = 64 * 1024;

/// A slot of a prompt file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// `{kind}`.
    Kind,
    /// `{declarations}`.
    Declarations,
    /// `{surfaces}`.
    Surfaces,
    /// `{rules}`.
    Rules,
    /// `{rules.<namespace>}`: the namespace's name.
    RulesOf(String),
    /// `{core.<piece>}`.
    Core(CorePiece),
    /// A slot of the surface's own, by its whole name,
    /// `<surface>.<name>`.
    Own(String),
    /// `{opener}`.
    Opener,
    /// `{instructions}`.
    Instructions,
    /// `{fence}`.
    Fence,
    /// `{request}`.
    Request,
}

impl Slot {
    /// The slot as a file writes it, without its braces.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Slot::Kind => "kind".into(),
            Slot::Declarations => "declarations".into(),
            Slot::Surfaces => "surfaces".into(),
            Slot::Rules => "rules".into(),
            Slot::RulesOf(namespace) => format!("rules.{namespace}"),
            Slot::Core(piece) => format!("core.{}", piece.name()),
            Slot::Own(name) => name.clone(),
            Slot::Opener => "opener".into(),
            Slot::Instructions => "instructions".into(),
            Slot::Fence => "fence".into(),
            Slot::Request => "request".into(),
        }
    }
}

/// Why a prompt file was refused: what is wrong, and the line (from 1)
/// it is wrong on, when it is one line's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

fn error<T>(line: usize, message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line: Some(line),
        message: message.into(),
    })
}

/// An error of the file as a whole, or of what it is read for.
fn refuse<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line: None,
        message: message.into(),
    })
}

/// What a prompt file may name: the surface it is read for.
#[derive(Clone, Copy, Debug)]
pub struct Known<'a> {
    /// The surface's name: not `core` and not `rules`, whose slots are
    /// the core's.
    pub surface: &'a str,
    /// The surface's kinds.
    pub kinds: &'a [&'a str],
    /// The namespaces directly on the surface that have rules.
    pub ruled_namespaces: &'a [&'a str],
    /// The slots that code shipped with the surface supplies, by their
    /// whole names, each `<surface>.<name>`. None for a surface a host
    /// registered.
    pub own: &'a [&'a str],
}

/// The names of surfaces no prompt file is read for: the prefixes of the
/// core's own slots.
const CORE_PREFIXES: [&str; 2] = ["core", "rules"];

/// Whether `s` is lowercase letters, digits, `_` and `.`, and not empty.
fn lowercase_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Whether `s` is an identifier as a surface file has them, which a
/// namespace's name is: ASCII letters, digits, `_` and `$`, not
/// starting with a digit.
fn identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

impl Known<'_> {
    /// Refuses a surface no prompt file is read for, and an own slot
    /// that is not the surface's by its name.
    fn check(&self) -> Result<(), Error> {
        let surface = self.surface;
        if CORE_PREFIXES.contains(&surface) {
            return refuse(format!(
                "no prompt file is read for a surface named `{surface}`: `{{core.<name>}}` and \
                 `{{rules.<namespace>}}` are the core's slots, so a surface named `core` or \
                 `rules` could have none of its own"
            ));
        }
        for own in self.own {
            let named = own
                .strip_prefix(surface)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(lowercase_name);
            if !named {
                return refuse(format!(
                    "the surface `{surface}` cannot supply a slot named `{{{own}}}`: a surface's \
                     own slots are `{{{surface}.<name>}}`, in lowercase letters, digits, `_` and \
                     `.`"
                ));
            }
        }
        Ok(())
    }
}

/// One piece of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Slot(Slot),
}

/// A section's text: its lines, each in pieces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    lines: Vec<Vec<Part>>,
}

impl Template {
    /// The text with each slot replaced by what `value` gives for it.
    /// `value` is asked once for each place a slot is written, in the
    /// text's order, and what it gives is written as it is. A line whose
    /// slots are all empty is left out with its line ending; when that
    /// is the last line, which has none, the line ending before it
    /// stays.
    pub fn render(&self, mut value: impl FnMut(&Slot) -> String) -> String {
        let mut out = String::new();
        let last = self.lines.len().saturating_sub(1);
        for (i, line) in self.lines.iter().enumerate() {
            let (mut text, mut slots, mut filled) = (String::new(), 0, 0);
            for part in line {
                match part {
                    Part::Text(t) => text.push_str(t),
                    Part::Slot(slot) => {
                        let v = value(slot);
                        slots += 1;
                        filled += usize::from(!v.is_empty());
                        text.push_str(&v);
                    }
                }
            }
            if slots > 0 && filled == 0 {
                continue;
            }
            out.push_str(&text);
            if i != last {
                out.push('\n');
            }
        }
        out
    }

    /// Every slot the text has, in its order, one for each place.
    pub fn slots(&self) -> impl Iterator<Item = &Slot> {
        self.lines.iter().flatten().filter_map(|p| match p {
            Part::Slot(s) => Some(s),
            Part::Text(_) => None,
        })
    }

    /// Whether the text has `slot`.
    #[must_use]
    pub fn has(&self, slot: &Slot) -> bool {
        self.slots().any(|s| s == slot)
    }

    /// The lines (from 0) that have `slot`, one for each place.
    fn lines_with(&self, slot: &Slot) -> Vec<usize> {
        let places = |line: &Vec<Part>| {
            line.iter()
                .filter(|p| **p == Part::Slot(slot.clone()))
                .count()
        };
        self.lines
            .iter()
            .enumerate()
            .flat_map(|(i, line)| std::iter::repeat_n(i, places(line)))
            .collect()
    }

    /// Whether any of the text outside its slots is more than white
    /// space.
    fn has_words(&self) -> bool {
        self.lines.iter().flatten().any(|p| match p {
            Part::Text(t) => !t.trim().is_empty(),
            Part::Slot(_) => false,
        })
    }
}

/// Which section a line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    System,
    Request,
    Opener,
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::System => "system",
            Section::Request => "request",
            Section::Opener => "opener",
        }
    }
}

/// The slot named `name` on line `line` of a `section`, if `known` lets
/// that section have it. The core's prefixes are matched before the
/// surface's, so no name of [`Known::own`] stands for one of the core's.
fn slot(name: &str, section: Section, known: &Known<'_>, line: usize) -> Result<Slot, Error> {
    let unknown = |why: &str| error(line, format!("unknown slot `{{{name}}}`: {why}"));
    let elsewhere = |other: Section| {
        error(
            line,
            format!(
                "the slot `{{{name}}}` is the {} section's, not the {} section's",
                other.name(),
                section.name()
            ),
        )
    };
    let in_system = |slot: Slot| match section {
        Section::System => Ok(slot),
        _ => elsewhere(Section::System),
    };
    let in_request = |slot: Slot| match section {
        Section::Request => Ok(slot),
        _ => elsewhere(Section::Request),
    };
    let surface = known.surface;
    let Some((prefix, rest)) = name.split_once('.') else {
        return match name {
            "kind" => in_system(Slot::Kind),
            "declarations" => in_system(Slot::Declarations),
            "surfaces" => in_system(Slot::Surfaces),
            "rules" => in_system(Slot::Rules),
            "opener" => in_request(Slot::Opener),
            "instructions" => in_request(Slot::Instructions),
            "fence" => in_request(Slot::Fence),
            "request" => in_request(Slot::Request),
            _ => unknown(
                "the slots with no `.` are `{kind}`, `{declarations}`, `{surfaces}`, `{rules}`, \
                 `{opener}`, `{instructions}`, `{fence}` and `{request}`",
            ),
        };
    };
    match prefix {
        "core" => match CorePiece::named(rest) {
            Some(piece) => in_system(Slot::Core(piece)),
            None => unknown(&format!(
                "the core's paragraphs are {}",
                CorePiece::ALL
                    .map(|p| format!("`{{core.{}}}`", p.name()))
                    .join(", ")
            )),
        },
        "rules" if known.ruled_namespaces.contains(&rest) => {
            in_system(Slot::RulesOf(rest.to_string()))
        }
        "rules" => unknown(&format!("the surface has no namespace `{rest}` with rules")),
        _ if prefix != surface => unknown(&format!(
            "a slot with a `.` is the core's (`{{core.<name>}}`, `{{rules.<namespace>}}`) or the \
             surface's own, and this file is read for the surface `{surface}`, whose own are \
             `{{{surface}.<name>}}`"
        )),
        _ if known.own.contains(&name) => in_system(Slot::Own(name.to_string())),
        _ if known.own.is_empty() => unknown(&format!(
            "the surface `{surface}` has no slots of its own: only code that ships with a \
             surface supplies one"
        )),
        _ => unknown(&format!(
            "the slots of the surface `{surface}`'s own are {}",
            known
                .own
                .iter()
                .map(|o| format!("`{{{o}}}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// One line of a section's text, in pieces.
fn parts(text: &str, section: Section, known: &Known<'_>, line: usize) -> Result<Vec<Part>, Error> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        plain.push_str(&rest[..at]);
        let brace = &rest[at..=at];
        rest = &rest[at + 1..];
        if let Some(after) = rest.strip_prefix(brace) {
            plain.push_str(brace);
            rest = after;
            continue;
        }
        if brace == "}" {
            return error(
                line,
                "a `}` that closes no slot: a brace that is text is written twice, `}}`",
            );
        }
        let Some(end) = rest
            .find(['{', '}'])
            .filter(|&e| rest[e..].starts_with('}'))
        else {
            return error(
                line,
                "a `{` that opens no slot: a brace that is text is written twice, `{{`",
            );
        };
        let name = &rest[..end];
        let well_formed = match name.strip_prefix("rules.") {
            Some(namespace) => identifier(namespace),
            None => lowercase_name(name),
        };
        if !well_formed {
            return error(
                line,
                format!(
                    "`{{{name}}}` is no slot (a slot's name is lowercase letters, digits, `_` \
                     and `.`, or `rules.` and a namespace's name): a brace that is text is \
                     written twice, `{{{{` and `}}}}`"
                ),
            );
        }
        if !plain.is_empty() {
            out.push(Part::Text(std::mem::take(&mut plain)));
        }
        out.push(Part::Slot(slot(name, section, known, line)?));
        rest = &rest[end + 1..];
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Part::Text(plain));
    }
    Ok(out)
}

/// The name a header line gives, if `line` is one.
fn header(line: &str) -> Option<&str> {
    line.strip_prefix("--- ")?.strip_suffix(" ---")
}

/// Whether `c` shows as nothing on a line, or changes the direction of
/// the text around it: what a surface file may not hold either
/// ([`clax_surface::hidden`]), among them the zero-width characters,
/// the byte order mark, the soft hyphen, the marks of text direction,
/// the variation selectors and the tag characters.
fn shows_as_nothing(c: char) -> bool {
    clax_surface::hidden(c)
}

/// Why a file may not hold `c`, if it may not: it shows as nothing or
/// changes the direction of text ([`shows_as_nothing`]), it is a
/// control character other than the tab (a line ending ends a line,
/// and a carriage return is refused as such), or it ends a line where
/// no line ends (U+2028, U+2029).
fn unseen(c: char) -> Option<&'static str> {
    if shows_as_nothing(c) {
        Some("which shows as nothing or changes the direction of text")
    } else if c.is_control() && c != '\t' {
        Some("a control character")
    } else if matches!(c, '\u{2028}' | '\u{2029}') {
        Some("a line or paragraph separator")
    } else {
        None
    }
}

/// Why `line` is no header and cannot be told from one by a reader, if
/// that is so: it is a header once its characters that show as nothing
/// are taken out, its spaces that are not the plain one are read as
/// that, and the white space around it is dropped, and it has such a
/// character or such white space. (So a header's name holds none.)
fn like_a_header(line: &str) -> Option<String> {
    let plain: String = line
        .chars()
        .filter(|c| !shows_as_nothing(*c))
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    header(plain.trim())?;
    const EXACT: &str = "a header is `--- <name> ---`, with plain spaces, and nothing else on \
                         its line";
    let odd = |c: &char| shows_as_nothing(*c) || (c.is_whitespace() && !matches!(c, ' ' | '\t'));
    if let Some((at, c)) = line.chars().enumerate().find(|(_, c)| odd(c)) {
        let what = if shows_as_nothing(c) {
            "which shows as nothing"
        } else {
            "a space that is not the plain one"
        };
        return Some(format!(
            "a header but for U+{:04X}, {what}, as character {} of the line: {EXACT}",
            u32::from(c),
            at + 1
        ));
    }
    if header(line).is_some() {
        return None;
    }
    let around = if line.trim_end() == line {
        "before"
    } else if line.trim_start() == line {
        "after"
    } else {
        "around"
    };
    Some(format!("a header with white space {around} it: {EXACT}"))
}

/// The first line (from 1) of `lines` that is the header `wanted` but
/// for its white space and the case of its letters, and so is no header:
/// what a writer who meant that header most likely wrote.
fn near_miss(lines: &[&str], wanted: &str) -> Option<usize> {
    let squashed = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted = squashed(wanted);
    lines
        .iter()
        .position(|l| header(l).is_none() && squashed(l) == wanted)
        .map(|i| i + 1)
}

/// The error of a file with no header `wanted`, for `why`: on the line
/// that nearly is that header ([`near_miss`]) when there is one, else on
/// the file's last line.
fn no_section<T>(lines: &[&str], wanted: &str, why: &str) -> Result<T, Error> {
    match near_miss(lines, wanted) {
        Some(line) => error(
            line,
            format!(
                "no `{wanted}` section{why}: this line, `{}`, is not that header, which is \
                 written `{wanted}` exactly",
                lines[line - 1]
            ),
        ),
        None => error(lines.len(), format!("no `{wanted}` section{why}")),
    }
}

/// A section as the file has it: what it is, the kind an opener is for,
/// the line of its header, and its lines in pieces.
struct Read<'a> {
    section: Section,
    kind: &'a str,
    header: usize,
    lines: Vec<Vec<Part>>,
}

/// A prompt file, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptFile {
    /// The system prompt.
    pub system: Template,
    /// The first user message.
    pub request: Template,
    /// Each kind's opener, in the file's order.
    openers: Vec<(String, String)>,
}

impl PromptFile {
    /// Reads `source` as the prompt file of the surface `known`
    /// describes.
    ///
    /// # Errors
    /// Whatever the format refuses (the module's documentation lists
    /// it), naming the line when it is one line's; and a `known` that
    /// names a surface no prompt file is read for, or an own slot that
    /// is not that surface's by its name.
    pub fn parse(source: &str, known: &Known<'_>) -> Result<PromptFile, Error> {
        if source.len() > MAX_BYTES {
            return refuse(format!(
                "the file is {} bytes long, and the limit is {MAX_BYTES}",
                source.len()
            ));
        }
        known.check()?;
        if source.starts_with('\u{feff}') {
            return error(
                1,
                "the file begins with a byte order mark (U+FEFF), which shows as nothing: a \
                 prompt file is UTF-8 with none",
            );
        }
        if let Some(at) = source.find('\r') {
            let line = source[..at].matches('\n').count() + 1;
            return error(line, "a carriage return: lines end in `\\n` alone");
        }
        let Some(body) = source.strip_suffix('\n') else {
            let lines = source.split('\n').count();
            return error(lines, "the file does not end with a line ending");
        };
        let lines: Vec<&str> = body.split('\n').collect();

        let mut sections: Vec<Read<'_>> = Vec::new();
        for (i, text) in lines.iter().enumerate() {
            let line = i + 1;
            if let Some(why) = like_a_header(text) {
                return error(line, why);
            }
            if let Some((at, (c, what))) = text
                .chars()
                .enumerate()
                .find_map(|(at, c)| unseen(c).map(|what| (at, (c, what))))
            {
                return error(
                    line,
                    format!(
                        "U+{:04X}, {what}, as character {} of the line: a prompt file holds \
                         no character a reader would not see, and no control character but \
                         the tab, so that what a model reads is what its reviewer saw",
                        u32::from(c),
                        at + 1
                    ),
                );
            }
            if let Some(name) = header(text) {
                let (section, kind) = match name {
                    "system" => (Section::System, ""),
                    "request" => (Section::Request, ""),
                    _ => match name.strip_prefix("opener ") {
                        Some(kind) => (Section::Opener, kind),
                        None => {
                            return error(
                                line,
                                format!(
                                    "unknown section `{name}`: a section is `system`, `request` \
                                     or `opener <kind>`"
                                ),
                            );
                        }
                    },
                };
                sections.push(Read {
                    section,
                    kind,
                    header: line,
                    lines: Vec::new(),
                });
            } else if let Some(read) = sections.last_mut() {
                read.lines.push(parts(text, read.section, known, line)?);
            }
        }

        let one = |wanted: Section| -> Result<(usize, Template), Error> {
            let mut found = sections.iter().filter(|s| s.section == wanted);
            let Some(first) = found.next() else {
                return no_section(&lines, &format!("--- {} ---", wanted.name()), "");
            };
            if let Some(second) = found.next() {
                return error(
                    second.header,
                    format!("a second `--- {} ---` section", wanted.name()),
                );
            }
            Ok((
                first.header,
                Template {
                    lines: first.lines.clone(),
                },
            ))
        };
        let (system_at, system) = one(Section::System)?;
        let (request_at, request) = one(Section::Request)?;
        // The line of the file that line `i` of the section under the
        // header on line `header` is.
        let line_of = |header: usize, i: usize| header + 1 + i;

        // What a prompt cannot do without, each once.
        let once = |template: &Template, at: usize, slot: Slot, section: Section, without: &str| {
            let name = slot.name();
            match template.lines_with(&slot)[..] {
                [_] => Ok(()),
                [] => error(
                    at,
                    format!(
                        "the {} section has no `{{{name}}}` slot: without it {without}",
                        section.name()
                    ),
                ),
                [_, second, ..] => error(
                    line_of(at, second),
                    format!(
                        "a second `{{{name}}}`: the {} section has it once",
                        section.name()
                    ),
                ),
            }
        };
        once(
            &system,
            system_at,
            Slot::Declarations,
            Section::System,
            "a run would not be told what its API is",
        )?;
        once(
            &system,
            system_at,
            Slot::Surfaces,
            Section::System,
            "a run would not be told the rules of the other surfaces it has; it is the \
             section's last line",
        )?;
        if system.lines.last().map(Vec::as_slice) != Some(&[Part::Slot(Slot::Surfaces)]) {
            return error(
                line_of(system_at, system.lines_with(&Slot::Surfaces)[0]),
                "`{surfaces}` is the system section's last line, alone on it: anywhere else, a \
                 run with other surfaces or a run with none would lose an empty line between \
                 two paragraphs or have one too many",
            );
        }
        if let [_, second, ..] = system.lines_with(&Slot::Rules)[..] {
            return error(
                line_of(system_at, second),
                "a second `{rules}`: the system section has it once at most",
            );
        }
        if !system.has_words() {
            return error(
                system_at,
                "the system section has no words of its own, only slots",
            );
        }
        once(
            &request,
            request_at,
            Slot::Request,
            Section::Request,
            "a run would not be sent its request",
        )?;
        match request.lines_with(&Slot::Instructions)[..] {
            [] => {}
            [at] if at < request.lines_with(&Slot::Request)[0] => {}
            [at] => {
                return error(
                    line_of(request_at, at),
                    "`{instructions}` must stand on a line before `{request}`'s: which strings are \
                     the person's words is said before the request, so nothing in the request \
                     can be read as saying it",
                );
            }
            [_, second, ..] => {
                return error(
                    line_of(request_at, second),
                    "a second `{instructions}`: the request section has it once at most",
                );
            }
        }

        let takes_opener = request.has(&Slot::Opener);
        let mut openers: Vec<(String, String)> = Vec::new();
        for read in sections.iter().filter(|s| s.section == Section::Opener) {
            let (kind, line) = (read.kind, read.header);
            if !takes_opener {
                return error(
                    line,
                    format!(
                        "an opener for `{kind}`, and the request section has no `{{opener}}` \
                         slot to put it in"
                    ),
                );
            }
            if !known.kinds.contains(&kind) {
                return error(
                    line,
                    format!("an opener for `{kind}`, which is no kind of the surface"),
                );
            }
            if openers.iter().any(|(k, _)| k == kind) {
                return error(line, format!("a second opener for `{kind}`"));
            }
            let text = Template {
                lines: read.lines.clone(),
            }
            .render(|_| unreachable!("an opener has no slot"));
            if text.trim().is_empty() {
                return error(
                    line,
                    format!(
                        "the opener for `{kind}` is {}",
                        if text.is_empty() {
                            "empty"
                        } else {
                            "only white space"
                        }
                    ),
                );
            }
            openers.push((kind.to_string(), text));
        }
        if takes_opener
            && let Some(kind) = known
                .kinds
                .iter()
                .find(|k| openers.iter().all(|(o, _)| o != *k))
        {
            return no_section(
                &lines,
                &format!("--- opener {kind} ---"),
                ": the request section has `{opener}`, so each kind of the surface has one",
            );
        }
        Ok(PromptFile {
            system,
            request,
            openers,
        })
    }

    /// The opener of `kind`: `None` when the file has no openers, or
    /// `kind` is no kind of its surface.
    #[must_use]
    pub fn opener(&self, kind: &str) -> Option<&str> {
        self.openers
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, text)| text.as_str())
    }
}

/// What `{surfaces}` becomes for `bindings`, each a surface's name and
/// its rules: for each, an empty line, `# <name>`, an empty line and the
/// rules, ending a line. Empty for none.
#[must_use]
pub fn surfaces(bindings: &[(&str, &str)]) -> String {
    bindings
        .iter()
        .map(|(name, rules)| format!("\n# {name}\n\n{rules}\n"))
        .collect()
}

/// What `{rules}` becomes for `rules`, the rules it stands for in their
/// order: each after an empty line, the last without its line ending.
/// Empty for none.
#[must_use]
pub fn rules(rules: &[&str]) -> String {
    rules
        .iter()
        .map(|rule| format!("\n{rule}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: Known<'static> = Known {
        surface: "clax",
        kinds: &["thread", "brief"],
        ruled_namespaces: &["web", "webSearch$_1"],
        own: &["clax.page_contract"],
    };

    fn read(source: &str) -> PromptFile {
        PromptFile::parse(source, &KNOWN).unwrap_or_else(|e| panic!("{e}\n{source}"))
    }

    fn refused(source: &str) -> String {
        let shown: String = source.chars().take(400).collect();
        PromptFile::parse(source, &KNOWN)
            .expect_err(&format!(
                "this file is to be refused, and was read (its first 400 characters):\n{shown}\n"
            ))
            .to_string()
    }

    /// The error of `source`, which is to be refused for `why`.
    fn refused_as(source: &str, why: &str) -> String {
        PromptFile::parse(source, &KNOWN)
            .expect_err(&format!("{why} is to be refused, and was read"))
            .to_string()
    }

    /// What every system section must end with, after its own lines.
    const TAIL: &str = "Words.\n{declarations}\n{surfaces}";

    /// A file whose system section is the lines of `system`, from line
    /// 2, and then [`TAIL`], and whose request section takes no opener.
    fn with_system(system: &str) -> String {
        format!("--- system ---\n{system}\n{TAIL}\n--- request ---\n{{request}}\n")
    }

    /// `text` as a system section's lines, with none of what a whole
    /// file must have: what [`Template::render`] is tested on.
    fn system(text: &str) -> Template {
        let lines = text.split('\n').enumerate();
        Template {
            lines: lines
                .map(|(i, l)| parts(l, Section::System, &KNOWN, i + 1).unwrap())
                .collect(),
        }
    }

    /// Each slot named by itself, so a rendering shows where each went.
    fn by_name(slot: &Slot) -> String {
        format!("<{}>", slot.name())
    }

    #[test]
    fn a_file_is_sections_of_text_with_slots() {
        let file = read(
            "A note, with {anything} in it.\n\
             --- system ---\n\
             You are {kind}.\n\
             \n\
             {core.code_mode} {core.data} {core.images} {core.finish}\n\
             {rules.web}{clax.page_contract}{rules.webSearch$_1}\n\
             {rules}\n\
             {declarations}\n\
             {surfaces}\n\
             --- request ---\n\
             {opener} Then:\n\
             {request}\n\
             --- opener brief ---\n\
             Build it.\n\
             --- opener thread ---\n\
             Do {{what}} they ask.\n",
        );
        assert_eq!(
            file.system.render(by_name),
            "You are <kind>.\n\n<core.code_mode> <core.data> <core.images> <core.finish>\n\
             <rules.web><clax.page_contract><rules.webSearch$_1>\n<rules>\n<declarations>\n\
             <surfaces>"
        );
        assert_eq!(file.request.render(by_name), "<opener> Then:\n<request>");
        assert_eq!(file.opener("thread"), Some("Do {what} they ask."));
        assert_eq!(file.opener("brief"), Some("Build it."));
        assert_eq!(file.opener("triage"), None);
        assert_eq!(
            file.system.slots().map(Slot::name).collect::<Vec<_>>(),
            [
                "kind",
                "core.code_mode",
                "core.data",
                "core.images",
                "core.finish",
                "rules.web",
                "clax.page_contract",
                "rules.webSearch$_1",
                "rules",
                "declarations",
                "surfaces"
            ]
        );
        assert!(file.system.has(&Slot::RulesOf("web".into())));
        assert!(!file.system.has(&Slot::RulesOf("db".into())));
    }

    /// A section's text is its lines without the last line ending,
    /// whatever follows it.
    #[test]
    fn a_section_ends_before_its_last_line_ending() {
        let file =
            read("--- request ---\n{request}\n\n--- system ---\nb\n{declarations}\n{surfaces}\n");
        assert_eq!(file.request.render(by_name), "<request>\n");
        assert_eq!(file.system.render(by_name), "b\n<declarations>\n<surfaces>");
        assert_eq!(system("").render(by_name), "");
        assert_eq!(system("\n").render(by_name), "\n");
    }

    #[test]
    fn a_doubled_brace_is_a_brace() {
        let text = system("a {{b}} c{{{kind}}}{{{{ }}");
        assert_eq!(text.render(|_| "K".into()), "a {b} c{K}{{ }");
    }

    /// What a slot becomes is written as it is: braces, doubled braces
    /// and whole slots in it are text.
    #[test]
    fn a_slot_is_replaced_once() {
        let text = system("{kind}|{declarations}|{kind}");
        let rendered = text.render(|slot| match slot {
            Slot::Kind => "{declarations} {{x}} {kind} }{".into(),
            _ => "{kind}".into(),
        });
        assert_eq!(
            rendered,
            "{declarations} {{x}} {kind} }{|{kind}|{declarations} {{x}} {kind} }{"
        );
    }

    #[test]
    fn a_line_whose_slots_are_all_empty_is_left_out() {
        let text = system("first\n- {rules.web}\nkept: {kind}{rules.web}\n\n{surfaces}");
        let empty_but_kind = |slot: &Slot| match slot {
            Slot::Kind => "K".to_string(),
            _ => String::new(),
        };
        // The bullet's line goes with its line ending; the last line
        // has none, so the one before it stays; the empty line, which
        // has no slot, stays.
        assert_eq!(text.render(empty_but_kind), "first\nkept: K\n\n");
        assert_eq!(
            text.render(by_name),
            "first\n- <rules.web>\nkept: <kind><rules.web>\n\n<surfaces>"
        );
        // Every line left out leaves nothing.
        assert_eq!(system("{kind}\n{surfaces}").render(|_| String::new()), "");
    }

    #[test]
    fn a_slot_nothing_supplies_is_refused_by_name_and_line() {
        assert_eq!(
            refused(&with_system("fine\n- {kinds}")),
            "line 3: unknown slot `{kinds}`: the slots with no `.` are `{kind}`, \
             `{declarations}`, `{surfaces}`, `{rules}`, `{opener}`, `{instructions}`, `{fence}` \
             and `{request}`"
        );
        assert_eq!(
            refused(&with_system("{core.safety}")),
            "line 2: unknown slot `{core.safety}`: the core's paragraphs are `{core.code_mode}`, \
             `{core.data}`, `{core.messages}`, `{core.images}`, `{core.finish}`"
        );
        assert_eq!(
            refused(&with_system("{rules.db}")),
            "line 2: unknown slot `{rules.db}`: the surface has no namespace `db` with rules"
        );
        assert_eq!(
            refused(&with_system("{clax.contract}")),
            "line 2: unknown slot `{clax.contract}`: the slots of the surface `clax`'s own are \
             `{clax.page_contract}`"
        );
        for request_slot in ["request", "instructions", "fence"] {
            assert_eq!(
                refused(&with_system(&format!("{{{request_slot}}}"))),
                format!(
                    "line 2: the slot `{{{request_slot}}}` is the request section's, not the \
                     system section's"
                )
            );
        }
        assert_eq!(
            refused(&format!(
                "--- system ---\n{TAIL}\n--- request ---\n\n{{kind}}\n"
            )),
            "line 7: the slot `{kind}` is the system section's, not the request section's"
        );
        assert_eq!(
            refused(&format!(
                "--- system ---\n{TAIL}\n--- request ---\n{{opener}}{{request}}\n\
                 --- opener thread ---\n{{rules}}\n--- opener brief ---\n"
            )),
            "line 8: the slot `{rules}` is the system section's, not the opener section's"
        );
    }

    /// Whose a slot is shows in its name: the core's prefixes are the
    /// core's whatever the surface's code supplies, and every other
    /// name with a `.` is the surface's that the file is read for.
    #[test]
    fn a_slot_with_a_dot_is_the_cores_or_the_surfaces_own() {
        assert_eq!(
            refused(&with_system("{shell.windows}")),
            "line 2: unknown slot `{shell.windows}`: a slot with a `.` is the core's \
             (`{core.<name>}`, `{rules.<namespace>}`) or the surface's own, and this file is \
             read for the surface `clax`, whose own are `{clax.<name>}`"
        );
        let parse = |known: Known<'_>, system: &str| {
            PromptFile::parse(&with_system(system), &known).map_err(|e| e.to_string())
        };
        // A surface a host registered has no slot of its own.
        let host = Known {
            surface: "shell",
            own: &[],
            ..KNOWN
        };
        assert_eq!(
            parse(host, "{shell.windows}").unwrap_err(),
            "line 2: unknown slot `{shell.windows}`: the surface `shell` has no slots of its \
             own: only code that ships with a surface supplies one"
        );
        // No surface's code supplies a slot under a name of the core's,
        // of another surface's or of the format's.
        for (surface, own) in [
            ("clax", "core.data"),
            ("clax", "rules.web"),
            ("clax", "shell.windows"),
            ("clax", "clax"),
            ("clax", "clax."),
            ("clax", "clax.Page"),
            ("clax", "claxon.page"),
            ("clax", "kind"),
        ] {
            let known = Known {
                surface,
                own: &[own],
                ..KNOWN
            };
            assert_eq!(
                parse(known, "{core.data}").unwrap_err(),
                format!(
                    "the surface `{surface}` cannot supply a slot named `{{{own}}}`: a \
                     surface's own slots are `{{{surface}.<name>}}`, in lowercase letters, \
                     digits, `_` and `.`"
                )
            );
        }
        // And no prompt file is read for a surface named as a prefix of
        // the core's.
        for surface in ["core", "rules"] {
            let known = Known {
                surface,
                own: &[],
                ..KNOWN
            };
            assert_eq!(
                parse(known, "{core.data}").unwrap_err(),
                format!(
                    "no prompt file is read for a surface named `{surface}`: `{{core.<name>}}` \
                     and `{{rules.<namespace>}}` are the core's slots, so a surface named \
                     `core` or `rules` could have none of its own"
                )
            );
        }
        // So `{core.data}` and `{rules.web}` are the core's in every
        // file that is read.
        let file = read(&with_system("{core.data}{rules.web}{clax.page_contract}"));
        assert_eq!(
            file.system.slots().take(3).collect::<Vec<_>>(),
            [
                &Slot::Core(CorePiece::Data),
                &Slot::RulesOf("web".into()),
                &Slot::Own("clax.page_contract".into())
            ]
        );
    }

    /// The part after `rules.` is any name a namespace may have; every
    /// other slot's name is lowercase.
    #[test]
    fn a_namespace_of_any_name_has_a_slot() {
        let file = read(&with_system("{rules.webSearch$_1}"));
        assert!(file.system.has(&Slot::RulesOf("webSearch$_1".into())));
        assert_eq!(
            refused(&with_system("{rules.Web}")),
            "line 2: unknown slot `{rules.Web}`: the surface has no namespace `Web` with rules"
        );
        for name in [
            "rules.",
            "rules.1x",
            "rules.a.b",
            "rules.a-b",
            "Rules.web",
            "core.Data",
        ] {
            assert_eq!(
                refused(&with_system(&format!("{{{name}}}"))),
                format!(
                    "line 2: `{{{name}}}` is no slot (a slot's name is lowercase letters, \
                     digits, `_` and `.`, or `rules.` and a namespace's name): a brace that is \
                     text is written twice, `{{{{` and `}}}}`"
                )
            );
        }
        assert!(identifier("$") && identifier("_a1") && identifier("aB$"));
        assert!(!identifier("") && !identifier("1a") && !identifier("a.b") && !identifier("é"));
    }

    #[test]
    fn a_brace_that_is_neither_a_slot_nor_doubled_is_refused() {
        const NO_SLOT: &str = "is no slot (a slot's name is lowercase letters, digits, `_` and \
                               `.`, or `rules.` and a namespace's name): a brace that is text \
                               is written twice, `{{` and `}}`";
        assert_eq!(
            refused(&with_system("publish({note, addresses})")),
            format!("line 2: `{{note, addresses}}` {NO_SLOT}")
        );
        assert_eq!(
            refused(&with_system("a { b")),
            "line 2: a `{` that opens no slot: a brace that is text is written twice, `{{`"
        );
        assert_eq!(
            refused(&with_system("a {kind {kind}")),
            "line 2: a `{` that opens no slot: a brace that is text is written twice, `{{`"
        );
        assert_eq!(
            refused(&with_system("a } b")),
            "line 2: a `}` that closes no slot: a brace that is text is written twice, `}}`"
        );
        assert_eq!(
            refused(&with_system("{}")),
            format!("line 2: `{{}}` {NO_SLOT}")
        );
        assert_eq!(
            refused(&with_system("{Kind}")),
            format!("line 2: `{{Kind}}` {NO_SLOT}")
        );
    }

    #[test]
    fn sections_are_the_three_each_as_often_as_it_must_be() {
        let system = format!("--- system ---\n{TAIL}\n");
        assert_eq!(
            refused("--- request ---\n{request}\n"),
            "line 2: no `--- system ---` section"
        );
        assert_eq!(refused(&system), "line 4: no `--- request ---` section");
        assert_eq!(
            refused(&format!("{system}--- request ---\n--- system ---\n")),
            "line 6: a second `--- system ---` section"
        );
        assert_eq!(
            refused(&format!("{system}--- request ---\n--- examples ---\n")),
            "line 6: unknown section `examples`: a section is `system`, `request` or \
             `opener <kind>`"
        );
        let both =
            format!("{system}--- request ---\n{{opener}}{{request}}\n--- opener thread ---\na\n");
        assert_eq!(
            refused(&both),
            "line 8: no `--- opener brief ---` section: the request section has `{opener}`, so \
             each kind of the surface has one"
        );
        assert_eq!(
            refused(&format!("{both}--- opener triage ---\n")),
            "line 9: an opener for `triage`, which is no kind of the surface"
        );
        assert_eq!(
            refused(&format!("{both}--- opener thread ---\n")),
            "line 9: a second opener for `thread`"
        );
        assert_eq!(
            refused(&format!("{both}--- opener brief ---\n")),
            "line 9: the opener for `brief` is empty"
        );
        assert_eq!(
            refused(&format!("{both}--- opener brief ---\n \t\n\n")),
            "line 9: the opener for `brief` is only white space"
        );
        assert_eq!(
            refused(&format!(
                "{system}--- request ---\n{{request}}\n--- opener thread ---\n"
            )),
            "line 7: an opener for `thread`, and the request section has no `{opener}` slot to \
             put it in"
        );
    }

    /// A line that is a header but for white space around it is
    /// refused, in the note too; a line that only resembles a header is
    /// text, and is named when the header it resembles is missing.
    #[test]
    fn a_header_is_exact() {
        let file = with_system("x");
        for (from, to, where_) in [
            ("--- request ---\n", "--- request --- \n", "after"),
            ("--- request ---\n", "--- request ---\t\n", "after"),
            ("--- request ---\n", " --- request ---\n", "before"),
            ("--- system ---\n", "\t--- system --- \n", "around"),
        ] {
            assert_eq!(file.matches(from).count(), 1);
            let line = if from.contains("system") { 1 } else { 6 };
            assert_eq!(
                refused(&file.replace(from, to)),
                format!(
                    "line {line}: a header with white space {where_} it: a header is \
                     `--- <name> ---`, with plain spaces, and nothing else on its line"
                )
            );
        }
        assert_eq!(
            refused(&format!("A note.\n--- system --- \n{file}")),
            "line 2: a header with white space after it: a header is `--- <name> ---`, with \
             plain spaces, and nothing else on its line"
        );
        // Text that only resembles a header is text.
        let text = read(&with_system(
            "---\n---system---\n--- system---\n-- system --",
        ));
        assert_eq!(
            text.system.render(by_name),
            "---\n---system---\n--- system---\n-- system --\nWords.\n<declarations>\n<surfaces>"
        );
        // The missing section's error names the line that resembles its
        // header, not the file's last.
        assert_eq!(
            refused(&file.replace("--- system ---", "---system---")),
            "line 1: no `--- system ---` section: this line, `---system---`, is not that \
             header, which is written `--- system ---` exactly"
        );
        assert_eq!(
            refused(&format!(
                "A note.\n---Request  ---\n{{request}}\n--- system ---\n{TAIL}\n"
            )),
            "line 2: no `--- request ---` section: this line, `---Request  ---`, is not that \
             header, which is written `--- request ---` exactly"
        );
        let openers = format!(
            "--- system ---\n{TAIL}\n--- request ---\n{{opener}}{{request}}\n\
             --- opener thread ---\na\n---opener brief---\nb\n"
        );
        assert_eq!(
            refused(&openers),
            "line 9: no `--- opener brief ---` section: the request section has `{opener}`, so \
             each kind of the surface has one: this line, `---opener brief---`, is not that \
             header, which is written `--- opener brief ---` exactly"
        );
    }

    /// A line that is a header but for a character a reader does not
    /// see, or sees as a space, is refused on its own line, by that
    /// character; so is a byte order mark before the file's first line.
    #[test]
    fn a_header_with_a_character_that_does_not_show_is_refused_on_its_line() {
        const EXACT: &str = "a header is `--- <name> ---`, with plain spaces, and nothing else \
                             on its line";
        let file = with_system("x");
        let changed = |from: &str, to: &str| {
            assert_eq!(file.matches(from).count(), 1, "{from}");
            refused_as(
                &file.replace(from, to),
                &format!("a file with the header {to:?}"),
            )
        };
        // A byte order mark, as an editor may write one: before the
        // first header, and before a note's first line.
        const MARK: &str = "line 1: the file begins with a byte order mark (U+FEFF), which shows \
                            as nothing: a prompt file is UTF-8 with none";
        assert_eq!(refused(&format!("\u{feff}{file}")), MARK);
        assert_eq!(refused(&format!("\u{feff}A note.\n{file}")), MARK);
        // Inside the file, a mark or a zero-width space on a header.
        assert_eq!(
            changed("--- request ---", "--- request ---\u{200b}"),
            format!(
                "line 6: a header but for U+200B, which shows as nothing, as character 16 of \
                 the line: {EXACT}"
            )
        );
        assert_eq!(
            changed("--- request ---", "\u{feff}--- request ---"),
            format!(
                "line 6: a header but for U+FEFF, which shows as nothing, as character 1 of \
                 the line: {EXACT}"
            )
        );
        assert_eq!(
            changed("--- request ---", "--- req\u{200d}uest ---"),
            format!(
                "line 6: a header but for U+200D, which shows as nothing, as character 8 of \
                 the line: {EXACT}"
            )
        );
        // A space that is not the plain one, in the place of one and
        // after the header.
        assert_eq!(
            changed("--- request ---", "---\u{a0}request\u{a0}---"),
            format!(
                "line 6: a header but for U+00A0, a space that is not the plain one, as \
                 character 4 of the line: {EXACT}"
            )
        );
        assert_eq!(
            changed("--- system ---", "--- system ---\u{3000}"),
            format!(
                "line 1: a header but for U+3000, a space that is not the plain one, as \
                 character 15 of the line: {EXACT}"
            )
        );
        // A space that is not the plain one in a line that is no header
        // is text.
        let text = read(&with_system("a\u{a0}b\u{3000}c ---\td"));
        assert!(
            text.system
                .render(by_name)
                .starts_with("a\u{a0}b\u{3000}c ---\td\n")
        );
    }

    /// A character a reader would not see, a control character but the
    /// tab, and a line or paragraph separator are refused on any line,
    /// a note's too, naming the line and the code point: a file with
    /// none is what its reviewer read.
    #[test]
    fn a_character_a_reader_would_not_see_is_refused_on_its_line() {
        const HIDDEN: &str = "which shows as nothing or changes the direction of text";
        const CONTROL: &str = "a control character";
        const SEPARATOR: &str = "a line or paragraph separator";
        let why = |code: u32, what: &str, at: usize| {
            format!(
                "U+{code:04X}, {what}, as character {at} of the line: a prompt file holds no \
                 character a reader would not see, and no control character but the tab, so \
                 that what a model reads is what its reviewer saw"
            )
        };
        for (c, what) in [
            ('\u{202e}', HIDDEN),
            ('\u{200b}', HIDDEN),
            ('\u{2066}', HIDDEN),
            ('\u{fe0f}', HIDDEN),
            ('\u{e0041}', HIDDEN),
            ('\u{e0001}', HIDDEN),
            ('\u{ad}', HIDDEN),
            ('\u{1b}', CONTROL),
            ('\u{0}', CONTROL),
            ('\u{7f}', CONTROL),
            ('\u{85}', CONTROL),
            ('\u{2028}', SEPARATOR),
            ('\u{2029}', SEPARATOR),
        ] {
            let code = u32::from(c);
            assert_eq!(
                refused(&with_system(&format!("Tidy{c} it."))),
                format!("line 2: {}", why(code, what, 5)),
                "U+{code:04X} in the system section"
            );
            assert_eq!(
                refused(&format!("A note{c}.\n{}", with_system("x"))),
                format!("line 1: {}", why(code, what, 7)),
                "U+{code:04X} in the note"
            );
            let request = with_system("x").replace("{request}\n", &format!("{{request}}{c}\n"));
            assert_eq!(
                refused(&request),
                format!("line 7: {}", why(code, what, 10)),
                "U+{code:04X} in the request section"
            );
        }
        // Clax's own files hold none, as a host's may not.
        for (name, source) in [
            ("src/clax.prompt.md", super::super::SOURCE),
            ("src/standard.prompt.md", super::super::standard::STANDARD),
        ] {
            assert!(
                source.chars().all(|c| unseen(c).is_none() || c == '\n'),
                "{name} holds a character a reader would not see"
            );
        }
    }

    /// The system section declares the API once and ends with the other
    /// surfaces' rules, the request section holds the request once, and
    /// the system section has words.
    #[test]
    fn what_a_prompt_cannot_do_without_is_required() {
        let file = with_system("x");
        let changed = |from: &str, to: &str| {
            assert_eq!(file.matches(from).count(), 1, "{from}");
            refused_as(
                &file.replace(from, to),
                &format!("a file with {to:?} where a file that is read has {from:?}"),
            )
        };
        assert_eq!(
            changed("{declarations}\n", ""),
            "line 1: the system section has no `{declarations}` slot: without it a run would \
             not be told what its API is"
        );
        assert_eq!(
            changed("{declarations}\n", "{declarations}\n\n{declarations}\n"),
            "line 6: a second `{declarations}`: the system section has it once"
        );
        assert_eq!(
            changed("{surfaces}\n", ""),
            "line 1: the system section has no `{surfaces}` slot: without it a run would not \
             be told the rules of the other surfaces it has; it is the section's last line"
        );
        assert_eq!(
            changed("{surfaces}\n", "{surfaces}{surfaces}\n"),
            "line 5: a second `{surfaces}`: the system section has it once"
        );
        const PLACE: &str = "`{surfaces}` is the system section's last line, alone on it: \
                             anywhere else, a run with other surfaces or a run with none would \
                             lose an empty line between two paragraphs or have one too many";
        assert_eq!(
            changed(
                "{declarations}\n{surfaces}\n",
                "{surfaces}\n{declarations}\n"
            ),
            format!("line 4: {PLACE}")
        );
        assert_eq!(
            changed("{surfaces}\n", "{surfaces} \n"),
            format!("line 5: {PLACE}")
        );
        assert_eq!(
            changed("{surfaces}\n", "{surfaces}\n\n"),
            format!("line 5: {PLACE}")
        );
        assert_eq!(
            changed("{request}\n", "Go.\n"),
            "line 6: the request section has no `{request}` slot: without it a run would not \
             be sent its request"
        );
        assert_eq!(
            changed("{request}\n", "{request}\n{request}\n"),
            "line 8: a second `{request}`: the request section has it once"
        );
        assert_eq!(
            changed("x\nWords.\n", "{rules}\n{kind}{rules}\n"),
            "line 3: a second `{rules}`: the system section has it once at most"
        );
        assert_eq!(
            changed("x\nWords.\n", " \n{kind}\n"),
            "line 1: the system section has no words of its own, only slots"
        );
        assert_eq!(
            refused("--- system ---\n--- request ---\n{request}\n"),
            "line 1: the system section has no `{declarations}` slot: without it a run would \
             not be told what its API is"
        );
    }

    /// `{instructions}` is said before the request, once at most, and
    /// `{fence}` stands wherever the file wants a fence.
    #[test]
    fn whose_words_a_request_holds_is_said_before_it() {
        let file = |request: &str| format!("--- system ---\n{TAIL}\n--- request ---\n{request}\n");
        let read_file = read(&file("{instructions}\n\n{fence}json\n{request}\n{fence}"));
        assert_eq!(
            read_file.request.render(by_name),
            "<instructions>\n\n<fence>json\n<request>\n<fence>"
        );
        assert_eq!(
            read_file
                .request
                .slots()
                .map(Slot::name)
                .collect::<Vec<_>>(),
            ["instructions", "fence", "request", "fence"]
        );
        assert_eq!(
            refused(&file("{request}\n{instructions}")),
            "line 7: `{instructions}` must stand on a line before `{request}`'s: which strings \
             are the person's words is said before the request, so nothing in the request can be \
             read as saying it"
        );
        assert_eq!(
            refused(&file("{instructions} {request}")),
            "line 6: `{instructions}` must stand on a line before `{request}`'s: which strings \
             are the person's words is said before the request, so nothing in the request can be \
             read as saying it"
        );
        assert_eq!(
            refused(&file("{instructions}\n{instructions}\n{request}")),
            "line 7: a second `{instructions}`: the request section has it once at most"
        );
        // A file without it is read: whether it must have it is the
        // caller's to say.
        read(&file("{request}"));
    }

    #[test]
    fn line_endings_are_newlines_and_the_file_ends_with_one() {
        assert_eq!(
            refused("--- system ---\r\n--- request ---\r\n"),
            "line 1: a carriage return: lines end in `\\n` alone"
        );
        assert_eq!(
            refused("--- system ---\n--- request ---\nx"),
            "line 3: the file does not end with a line ending"
        );
    }

    /// A file over the limit is refused before any of it is read: what
    /// else is wrong with it is not looked for.
    #[test]
    fn a_file_has_a_longest_length() {
        assert_eq!(MAX_BYTES, clax_surface::Limits::default().file_bytes);
        let file = with_system("x");
        let filled = |bytes: usize| format!("{}\n{file}", "n".repeat(bytes - file.len() - 1));
        assert_eq!(filled(MAX_BYTES).len(), MAX_BYTES);
        read(&filled(MAX_BYTES));
        assert_eq!(
            refused_as(
                &filled(MAX_BYTES + 1),
                "a file of one byte more than `MAX_BYTES`"
            ),
            "the file is 65537 bytes long, and the limit is 65536"
        );
        assert_eq!(
            refused_as(
                &"{\r".repeat(MAX_BYTES),
                "a file of twice `MAX_BYTES`, wrong in every line"
            ),
            "the file is 131072 bytes long, and the limit is 65536"
        );
    }

    /// Text before the first header is read by nobody: braces and
    /// unknown slots in it are not errors.
    #[test]
    fn the_note_before_the_first_section_is_no_part_of_a_prompt() {
        let file = read(&format!("{{unknown}} {{ }}}} \n\n{}", with_system("s")));
        assert_eq!(
            file.system.render(by_name),
            "s\nWords.\n<declarations>\n<surfaces>"
        );
        assert_eq!(file.request.render(by_name), "<request>");
    }

    #[test]
    fn surfaces_are_a_heading_and_rules_each() {
        assert_eq!(surfaces(&[]), "");
        assert_eq!(
            surfaces(&[("shell", "Draw first.\nThen call."), ("mail", "{kind}")]),
            "\n# shell\n\nDraw first.\nThen call.\n\n# mail\n\n{kind}\n"
        );
        // After a prompt's last line, where every file has the slot:
        // the prompt ends with a line ending with and without another
        // surface.
        let text = system("```\n{surfaces}");
        assert_eq!(
            text.render(|_| surfaces(&[("shell", "Draw first.")])),
            "```\n\n# shell\n\nDraw first.\n"
        );
        assert_eq!(text.render(|_| surfaces(&[])), "```\n");
    }

    /// `{rules}` on its own line after a line of text, before an empty
    /// one: each rule is a paragraph, and no rules leave no empty line
    /// too many.
    #[test]
    fn rules_are_a_paragraph_each() {
        assert_eq!(rules(&[]), "");
        assert_eq!(rules(&["One."]), "\nOne.");
        assert_eq!(
            rules(&["One.\nStill one.", "{kind}"]),
            "\nOne.\nStill one.\n\n{kind}"
        );
        let text = system("- last\n{rules}\n\n# Next");
        assert_eq!(text.render(|_| rules(&[])), "- last\n\n# Next");
        assert_eq!(
            text.render(|_| rules(&["One.", "Two."])),
            "- last\n\nOne.\n\nTwo.\n\n# Next"
        );
    }
}
