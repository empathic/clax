//! HTML as Markdown, made where the page was fetched: the daemon's
//! conversion of a fetched HTML page ([`html_to_markdown`]), so reading
//! one needs no outside service and the page goes nowhere else.

use super::PublicUrl;
use super::page::{MAX_TARGET_BYTES, STORE_BYTES};
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// `html` as Markdown (CommonMark, with GitHub's tables and
/// strikethrough): headings, paragraphs, lists (nested, ordered from their
/// `start`), block quotes, links and images with their targets resolved
/// against the document's base URL, emphasis, strikethrough, inline code,
/// preformatted blocks as fenced code, rules and tables as pipe tables.
///
/// - **The base URL** is the document's first `<base href>`, wherever it
///   is, when it is at most four bytes per character of
///   [`MAX_URL_CHARS`](super::url::MAX_URL_CHARS) long, else `base`: the
///   rule [`links_of_html`](super::page::links_of_html) reads a page's
///   links by, so the Markdown's targets are the page's links.
/// - **Dropped**: what a reader does not see: `script`, `style`, `title`,
///   `noscript`, `template` (but a declarative shadow root's, which a
///   browser shows), `svg` and `math` (up to their end, or the next tag
///   that ends foreign content in a browser, such as `<p>` or `<div>`),
///   `select`, `textarea`, `iframe`, `object`, `noembed`, `noframes`,
///   `xmp` and comments. A link or image whose target is not an `http` or `https`
///   URL, or whose resolved target is longer than
///   [`MAX_URL_CHARS`](super::url::MAX_URL_CHARS), keeps only its text;
///   an image without an `alt` is dropped.
/// - **Inside a link or a table cell** blocks are spaces: the text of a
///   card link is one line, and a cell's content stays in its cell.
/// - **Escaped**: text Markdown would read as markup (`\`, `*`, `_`,
///   `` ` ``, `[`, `]`, `~`, `<`, an `&` that would start a character
///   reference, a `!` before a link, and a heading, quote, list or rule
///   marker that starts a line). Code is written as it is, between
///   backtick runs longer than any inside it.
///
/// A tokenizer pass, not a tree: where HTML implies an end the converter
/// reads one only roughly (a new `<a>` ends the open link, as does the end
/// of the block it opened in; a new item ends the item before it, a new
/// cell the cell, a new paragraph the paragraph; a block ends the inline
/// elements open in it, and inline code; inline elements nested more than
/// [`MAX_SPANS`] deep, and table rows wider than [`MAX_COLUMNS`] cells, are
/// read flatter; only the innermost [`SCAN`] open blocks are looked through
/// for the one an end closes), so a malformed
/// page may read differently from a browser's rendering, with its visible
/// text all there. Linear in `html`'s length, and at most [`STORE_BYTES`]
/// of output, as much of a page's text as the page store keeps.
#[must_use]
pub fn html_to_markdown(html: &str, base: &PublicUrl) -> String {
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tok = Tokenizer::new(
        Sink(RefCell::new(Writer::new(base))),
        TokenizerOpts::default(),
    );
    let _ = tok.feed(&input);
    tok.end();
    tok.sink.0.into_inner().finish()
}

/// The most levels of block quotes and list items a line's prefix shows:
/// deeper nesting reads as this deep, so a line costs at most this much
/// prefix however deep a page nests.
pub const MAX_DEPTH: usize = 8;

/// The most inline elements (links, emphasis, strikethrough) open at
/// once: one opened inside more is read as text.
pub const MAX_SPANS: usize = 16;

/// The most cells of a table's row: those after are joined into the last.
pub const MAX_COLUMNS: usize = 32;

/// How many of the innermost open blocks an end tag, or an implied end, is
/// looked for among: one opened further out is not ended by it, so an end
/// costs at most this much however deep a page nests.
pub const SCAN: usize = 64;

/// The largest list number written: CommonMark reads at most nine digits.
const MAX_NUMBER: u64 = 999_999_999;

/// The tags that end foreign content (`svg`, `math`) in a browser's tree
/// builder: one of these opens HTML again.
const BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
];

struct Sink(RefCell<Writer>);

impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut w = self.0.borrow_mut();
        // The tokenizer splits text at character references: a run of it
        // is read whole, so what it escapes sees all of it.
        if let Token::CharacterTokens(t) = &token {
            w.run.push_str(t);
            return TokenSinkResult::Continue;
        }
        let run = std::mem::take(&mut w.run);
        w.text(&run);
        match token {
            Token::TagToken(tag) => {
                if let Some(kind) = w.tag(&tag) {
                    return TokenSinkResult::RawData(kind);
                }
            }
            Token::CharacterTokens(_)
            | Token::NullCharacterToken
            | Token::CommentToken(_)
            | Token::DoctypeToken(_)
            | Token::EOFToken
            | Token::ParseError(_) => {}
        }
        TokenSinkResult::Continue
    }
}

/// A block a line is inside of, in the order they opened: each adds to
/// the prefix of every line written inside it.
enum Container {
    /// A block quote: `> `, once its first line is written (a blank line
    /// before it is outside it).
    Quote { started: bool },
    /// A list item: its marker (`- `, `3. `) on its first line, then as
    /// many spaces as the marker is wide. `pending` until the item's first
    /// line is written.
    Item { marker: String, pending: bool },
}

/// A list being written: its items' kind, the next item's number, and
/// how many containers were open outside it.
struct List {
    ordered: bool,
    next: u64,
    outside: usize,
}

/// An inline element whose mark is written before its first text, and
/// closed after its last.
struct Span {
    /// The element that opened it (`b`, `em`, `a`).
    name: &'static str,
    kind: SpanKind,
    /// Its opening mark is written.
    written: bool,
    /// How many blocks and containers were open, and tables flattened,
    /// when it opened: a link ends when the block it opened in does, and
    /// takes the tables it flattened with it.
    at: (usize, usize, usize),
}

enum SpanKind {
    /// Emphasis or strikethrough: the same mark opens and closes it.
    Mark(&'static str),
    /// A link to its resolved target.
    Link(String),
}

/// A table being written: its rows, each a list of cells, the row being
/// read, the cell being read (where it starts in the output, and how many
/// spans were open outside it), and the breaks owed and whether the line
/// was empty where the table began, which its rows start from.
#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<(usize, usize)>,
    before: (u8, bool),
}

/// Content dropped until its element ends.
struct Hidden {
    name: String,
    /// How many elements of that name opened inside it.
    nested: usize,
}

struct Writer {
    /// Text read since the last tag.
    run: String,
    base: url::Url,
    base_seen: bool,
    out: String,
    hidden: Option<Hidden>,
    /// The block elements open, innermost last, by name.
    blocks: Vec<&'static str>,
    containers: Vec<Container>,
    /// How many of `containers` are quotes.
    quotes: usize,
    lists: Vec<List>,
    spans: Vec<Span>,
    /// Inline elements opened past [`MAX_SPANS`] and read as text, by
    /// name: as many of their ends are skipped.
    deep_spans: HashMap<&'static str, usize>,
    /// Inside `pre`: its text so far, kept verbatim, and how many `pre`
    /// opened inside it.
    pre: Option<(String, usize)>,
    /// Inside inline code: its text so far, and the element that opened
    /// it.
    code: Option<(String, String)>,
    tables: Vec<Table>,
    /// Tables opened inside a cell or a link, read as text, how deep.
    flat_tables: usize,
    /// A heading's level, written before the line's first text.
    heading: Option<usize>,
    /// A space is owed before the next text on this line.
    space: bool,
    /// Nothing has been written on this line yet but its prefix.
    line_empty: bool,
    /// Where the line's own text starts in the output, after its prefix:
    /// a word written there could start a block.
    line_at: usize,
    /// The line breaks owed before the next text: 1 a new line, 2 a blank
    /// line between.
    breaks: u8,
}

impl Writer {
    fn new(base: &PublicUrl) -> Writer {
        Writer {
            run: String::new(),
            base: base.as_url().clone(),
            base_seen: false,
            out: String::new(),
            hidden: None,
            blocks: Vec::new(),
            containers: Vec::new(),
            quotes: 0,
            lists: Vec::new(),
            spans: Vec::new(),
            deep_spans: HashMap::new(),
            pre: None,
            code: None,
            tables: Vec::new(),
            flat_tables: 0,
            heading: None,
            space: false,
            line_empty: true,
            line_at: 0,
            breaks: 0,
        }
    }

    fn full(&self) -> bool {
        self.out.len() >= STORE_BYTES
    }

    fn attr<'t>(tag: &'t Tag, name: &str) -> Option<&'t str> {
        tag.attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| &*a.value)
    }

    /// `href` resolved against the document's base, when it is an `http`
    /// or `https` URL of a length worth keeping.
    fn target(&self, href: &str) -> Option<String> {
        if href.len() > MAX_TARGET_BYTES {
            return None;
        }
        let u = self.base.join(href.trim()).ok()?;
        (matches!(u.scheme(), "http" | "https")
            && u.as_str().chars().count() <= super::url::MAX_URL_CHARS)
            .then(|| u.to_string())
    }

    /// Whether blocks are spaces here: inside a cell or a link.
    fn inline_only(&self) -> bool {
        self.tables.last().is_some_and(|t| t.cell.is_some())
            || self
                .spans
                .iter()
                .any(|s| matches!(s.kind, SpanKind::Link(_)))
    }

    /// Where the open cell starts in the output, else 0: nothing before it
    /// is taken back.
    fn floor(&self) -> usize {
        self.tables
            .last()
            .and_then(|t| t.cell)
            .map_or(0, |(at, _)| at)
    }

    /// How many spans were open outside the open cell: those it does not
    /// close.
    fn cell_spans(&self) -> usize {
        self.tables
            .last()
            .and_then(|t| t.cell)
            .map_or(0, |(_, spans)| spans)
    }

    /// Owes a break of `n` (1 a new line, 2 a blank line) before the next
    /// text, and ends the open spans: none crosses a block. Before a list
    /// item's first text no more is owed than its item asked for, so an
    /// item that starts with a block keeps its marker on its first line.
    fn block(&mut self, n: u8) {
        if self.inline_only() {
            self.space = true;
            return;
        }
        self.close_spans(0);
        self.deep_spans.clear();
        let pending = matches!(
            self.containers.last(),
            Some(Container::Item { pending: true, .. })
        );
        if !self.out.is_empty() && !pending {
            self.breaks = self.breaks.max(n);
        }
        self.space = false;
    }

    /// The prefix of a line inside the open containers, of at most
    /// [`MAX_DEPTH`] of them and the innermost. For a blank line (`blank`)
    /// without the quotes not yet started and its trailing spaces;
    /// otherwise the quotes are started and a pending item's marker is
    /// written (and no longer pending).
    fn prefix(&mut self, blank: bool) -> String {
        let n = self.containers.len();
        let last = (n > MAX_DEPTH).then(|| n - 1);
        let mut p = String::new();
        for i in (0..n.min(MAX_DEPTH)).chain(last) {
            match &mut self.containers[i] {
                Container::Quote { started } if blank && !*started => {}
                Container::Quote { started } => {
                    p.push_str("> ");
                    if !blank {
                        *started = true;
                    }
                }
                Container::Item { marker, pending } if *pending && !blank => {
                    p.push_str(marker);
                    *pending = false;
                }
                Container::Item { marker, .. } => {
                    p.extend(std::iter::repeat_n(' ', marker.len()));
                }
            }
        }
        if blank {
            let end = p.trim_end().len();
            p.truncate(end);
        }
        p
    }

    /// Pays the breaks owed and the line's prefix (and heading marker), or
    /// the space owed, then the open spans' marks, before text.
    fn start_text(&mut self) {
        let pending = matches!(
            self.containers.last(),
            Some(Container::Item { pending: true, .. })
        );
        if self.breaks > 0 || (self.line_empty && (pending || self.out.is_empty())) {
            if !self.out.is_empty() {
                let floor = self.floor();
                trim_spaces(&mut self.out, floor);
                self.out.push('\n');
                for _ in 1..self.breaks {
                    let blank = self.prefix(true);
                    self.out.push_str(&blank);
                    self.out.push('\n');
                }
            }
            self.breaks = 0;
            let p = self.prefix(false);
            self.out.push_str(&p);
            self.line_empty = true;
            self.line_at = self.out.len();
        } else if self.space && !self.line_empty {
            self.out.push(' ');
        }
        self.space = false;
        if let Some(level) = self.heading.take() {
            self.out.push_str(&"#".repeat(level));
            self.out.push(' ');
        }
        self.write_spans();
    }

    /// Writes the opening marks of the open spans not yet written.
    fn write_spans(&mut self) {
        let floor = self.floor();
        for i in 0..self.spans.len() {
            if self.spans[i].written {
                continue;
            }
            match &self.spans[i].kind {
                SpanKind::Mark(m) => self.out.push_str(m),
                SpanKind::Link(_) => {
                    // A `!` before it would make it an image.
                    if self.out.len() > floor
                        && self.out.ends_with('!')
                        && !self.out.ends_with("\\!")
                    {
                        self.out.pop();
                        self.out.push_str("\\!");
                    }
                    self.out.push('[');
                }
            }
            self.spans[i].written = true;
        }
    }

    /// Closes the spans above the first `keep`, innermost first.
    fn close_spans(&mut self, keep: usize) {
        while self.spans.len() > keep {
            let s = self.spans.pop().expect("a span is open");
            if matches!(s.kind, SpanKind::Link(_)) {
                self.flat_tables = s.at.2;
            }
            if !s.written {
                continue;
            }
            let floor = self.floor();
            if trim_spaces(&mut self.out, floor) {
                self.space = true;
            }
            match s.kind {
                SpanKind::Mark(m) => self.out.push_str(m),
                SpanKind::Link(t) => {
                    self.out.push_str("](");
                    self.out.push_str(&link_url(&t));
                    self.out.push(')');
                }
            }
        }
    }

    fn open_span(&mut self, name: &'static str, kind: SpanKind) {
        if self.spans.len() >= MAX_SPANS {
            *self.deep_spans.entry(name).or_default() += 1;
            return;
        }
        self.spans.push(Span {
            name,
            kind,
            written: false,
            at: (self.blocks.len(), self.containers.len(), self.flat_tables),
        });
    }

    /// Ends the links whose block or container has ended, and the spans
    /// inside them.
    fn end_links_outside(&mut self) {
        let (blocks, containers) = (self.blocks.len(), self.containers.len());
        if let Some(i) = self.spans.iter().position(|s| {
            matches!(s.kind, SpanKind::Link(_)) && (s.at.0 > blocks || s.at.1 > containers)
        }) {
            self.close_spans(i);
        }
    }

    /// Whether the next word written starts the line's own text, where it
    /// could start a block.
    fn at_line(&self) -> bool {
        self.line_empty && self.out.len() == self.line_at
    }

    /// Ends the innermost of the open blocks named `names`, with the blocks
    /// inside it, unless one of `stops` (the element's scope in HTML: the
    /// table or list it belongs to) is open inside it, or it is not among
    /// the innermost [`SCAN`]; whether one was.
    fn end_block(&mut self, names: &[&str], stops: &[&str]) -> bool {
        let n = self.blocks.len();
        let from = n.saturating_sub(SCAN);
        match self.blocks[from..]
            .iter()
            .rposition(|b| names.contains(b) || stops.contains(b))
        {
            Some(i) if names.contains(&self.blocks[from + i]) => {
                self.blocks.truncate(from + i);
                true
            }
            _ => false,
        }
    }

    /// Keeps the stack of open blocks for a block element's start or end
    /// tag, with the ends HTML implies within their scope (a new item ends
    /// the open item of its list, a new cell or row the open one of its
    /// table, a block the paragraph it opens in), and ends the links whose
    /// block ended. A block's `/>` is ignored, as in HTML.
    fn track_block(&mut self, name: &str, start: bool) {
        let Some(name) = block_name(name) else {
            return;
        };
        if start {
            // A block opened in a paragraph ends it (no block opens in
            // one, so it is the innermost when it is open).
            if name != "td" && name != "th" && name != "tr" && self.blocks.last() == Some(&"p") {
                self.blocks.pop();
                // A link opened in that paragraph goes on in the block, as
                // a browser carries it into the blocks that follow.
                let depth = self.blocks.len();
                for sp in &mut self.spans {
                    if matches!(sp.kind, SpanKind::Link(_)) && sp.at.0 > depth {
                        sp.at.0 = depth;
                    }
                }
            }
            match name {
                "li" => {
                    self.end_block(&["li"], &["ul", "ol", "menu"]);
                }
                "dt" | "dd" => {
                    self.end_block(&["dt", "dd"], &["dl"]);
                }
                "td" | "th" => {
                    self.end_block(&["td", "th"], &["table"]);
                }
                "tr" => {
                    self.end_block(&["tr"], &["table"]);
                }
                _ => {}
            }
            self.blocks.push(name);
        } else {
            let stops: &[&str] = match name {
                "table" => &[],
                "td" | "th" | "tr" => &["table"],
                "li" => &["ul", "ol", "menu", "table", "td", "th"],
                _ => &["table", "td", "th"],
            };
            self.end_block(&[name], stops);
        }
        self.end_links_outside();
    }

    /// Ends the innermost open span `name` opened, and the spans inside
    /// it; an end with no such span open (or one outside the open cell) is
    /// skipped.
    fn end_span(&mut self, name: &str) {
        if let Some(n) = self.deep_spans.get_mut(name)
            && *n > 0
        {
            *n -= 1;
            return;
        }
        let keep = self.cell_spans();
        if let Some(i) = self.spans.iter().rposition(|s| s.name == name)
            && i >= keep
        {
            self.close_spans(i);
        }
    }

    fn text(&mut self, t: &str) {
        if self.hidden.is_some() || self.full() {
            return;
        }
        if let Some((pre, _)) = self.pre.as_mut() {
            if pre.len() < STORE_BYTES {
                pre.push_str(t);
            }
            return;
        }
        if let Some((code, _)) = self.code.as_mut() {
            if code.len() < STORE_BYTES {
                code.push_str(t);
            }
            return;
        }
        if t.starts_with(|c: char| c.is_ascii_whitespace()) {
            self.space = true;
        }
        let mut words = t.split_ascii_whitespace().peekable();
        while let Some(w) = words.next() {
            self.start_text();
            let at_line = self.at_line();
            escape_into(&mut self.out, w, at_line);
            self.line_empty = false;
            if words.peek().is_some() {
                self.space = true;
            }
        }
        if t.ends_with(|c: char| c.is_ascii_whitespace()) {
            self.space = true;
        }
    }

    /// Handles `tag`, answering the raw kind of the text that follows when
    /// it is not markup.
    fn tag(&mut self, tag: &Tag) -> Option<RawKind> {
        let name = &*tag.name;
        let start = tag.kind == TagKind::StartTag;
        let raw = match name {
            "script" => Some(RawKind::ScriptData),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(RawKind::Rawtext),
            "title" | "textarea" => Some(RawKind::Rcdata),
            _ => None,
        };
        // The first base with an `href` counts, wherever it is, hidden or
        // not, as for the page's links.
        if name == "base" && start && !self.base_seen {
            if let Some(h) = Writer::attr(tag, "href") {
                self.base_seen = true;
                if h.len() <= MAX_TARGET_BYTES
                    && let Ok(b) = self.base.join(h.trim())
                {
                    self.base = b;
                }
            }
            return None;
        }
        if let Some(h) = self.hidden.as_mut() {
            let foreign = h.name == "svg" || h.name == "math";
            if h.name == name {
                if start && !tag.self_closing {
                    h.nested += 1;
                } else if !start {
                    if h.nested == 0 {
                        self.hidden = None;
                    } else {
                        h.nested -= 1;
                    }
                }
                return if start { raw } else { None };
            }
            if !(foreign && start && BREAKOUT.contains(&name)) {
                return if start { raw } else { None };
            }
            // HTML again: the foreign content has ended.
            self.hidden = None;
        }
        if start && hides(name, tag) {
            // A self-closing `<svg/>` hides nothing; a raw element is
            // never self-closing (HTML ignores the `/`).
            if !tag.self_closing || raw.is_some() {
                self.hidden = Some(Hidden {
                    name: name.to_string(),
                    nested: 0,
                });
            }
            return raw;
        }
        if self.full() {
            return None;
        }
        if self.pre.is_some() {
            self.in_pre(name, start, tag.self_closing);
            return None;
        }
        if self.code.is_some() {
            if !start && self.code.as_ref().is_some_and(|(_, by)| by == name) {
                self.end_code();
                return None;
            }
            if name == "br" {
                self.code.as_mut().expect("in code").0.push(' ');
                return None;
            }
            let link = if start {
                Writer::attr(tag, "href")
                    .and_then(|h| self.target(h))
                    .is_some()
            } else {
                self.spans
                    .iter()
                    .any(|s| matches!(s.kind, SpanKind::Link(_)))
            };
            if name == "a" && link {
                // A link in code (an API reference's type): the code so far,
                // the link, and the code after it, each a span.
                let by = self.code.as_ref().map(|(_, by)| by.clone());
                self.end_code();
                self.track_block(name, start);
                self.element(name, start, tag);
                self.code = by.map(|by| (String::new(), by));
                return None;
            }
            // Other inline markup in code (an anchor without a target too)
            // is skipped; a block ends it.
            block_name(name)?;
            self.end_code();
        }
        self.track_block(name, start);
        self.element(name, start, tag);
        None
    }

    fn in_pre(&mut self, name: &str, start: bool, self_closing: bool) {
        let (text, depth) = self.pre.as_mut().expect("in pre");
        match (name, start) {
            ("pre" | "listing", true) if !self_closing => *depth += 1,
            ("pre" | "listing", false) if *depth > 0 => *depth -= 1,
            ("pre" | "listing", false) => self.end_pre(),
            ("br", _) => text.push('\n'),
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn element(&mut self, name: &str, start: bool, tag: &Tag) {
        match (name, start) {
            (
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
                | "nav" | "figure" | "figcaption" | "address" | "details" | "summary" | "form"
                | "fieldset" | "dl" | "center" | "body" | "html" | "head",
                _,
            ) => self.block(2),
            ("dt" | "dd", _) => self.block(1),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                self.block(2);
                if !self.inline_only() {
                    self.heading = Some(usize::from(name.as_bytes()[1] - b'0'));
                }
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                self.heading = None;
                self.block(2);
            }
            ("br", _) => self.block(1),
            ("hr", true) => {
                self.block(2);
                if !self.inline_only() {
                    self.start_text();
                    self.out.push_str("---");
                    self.line_empty = false;
                    self.block(2);
                }
            }
            ("blockquote", true) => {
                self.block(2);
                if !self.inline_only() {
                    self.containers.push(Container::Quote { started: false });
                    self.quotes += 1;
                }
            }
            ("blockquote", false) => {
                if self.inline_only() {
                    self.space = true;
                } else if self.quotes > 0
                    && let Some(i) = self
                        .containers
                        .iter()
                        .rposition(|c| matches!(c, Container::Quote { .. }))
                {
                    self.block(2);
                    self.truncate_containers(i);
                }
            }
            ("ul" | "ol" | "menu", true) => {
                if self.inline_only() {
                    self.space = true;
                    return;
                }
                self.block(if self.containers.is_empty() { 2 } else { 1 });
                let next = Writer::attr(tag, "start")
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(1)
                    .min(MAX_NUMBER);
                self.lists.push(List {
                    ordered: name == "ol",
                    next,
                    outside: self.containers.len(),
                });
            }
            ("ul" | "ol" | "menu", false) => {
                if self.inline_only() {
                    self.space = true;
                    return;
                }
                if let Some(l) = self.lists.pop() {
                    self.block(1);
                    self.truncate_containers(l.outside);
                    if self.containers.is_empty() {
                        self.block(2);
                    }
                }
            }
            ("li", true) => {
                if self.inline_only() {
                    self.space = true;
                    return;
                }
                self.block(1);
                let (marker, outside) = match self.lists.last_mut() {
                    Some(l) => {
                        let m = if l.ordered {
                            let m = format!("{}. ", l.next);
                            l.next = l.next.saturating_add(1).min(MAX_NUMBER);
                            m
                        } else {
                            "- ".to_string()
                        };
                        (m, Some(l.outside))
                    }
                    None => ("- ".to_string(), None),
                };
                // The item before it in its list ends here; outside a list,
                // the innermost open item does.
                match outside {
                    Some(outside) => self.truncate_containers(outside),
                    None => {
                        let n = self.containers.len();
                        let from = n.saturating_sub(SCAN);
                        if let Some(i) = self.containers[from..]
                            .iter()
                            .rposition(|c| matches!(c, Container::Item { .. }))
                        {
                            self.truncate_containers(from + i);
                        }
                    }
                }
                self.end_links_outside();
                self.containers.push(Container::Item {
                    marker,
                    pending: true,
                });
            }
            ("li", false) => {
                if self.inline_only() {
                    self.space = true;
                    return;
                }
                self.block(1);
                let outside = self.lists.last().map_or(0, |l| l.outside);
                if self.containers.len() > outside
                    && matches!(self.containers.last(), Some(Container::Item { .. }))
                {
                    self.containers.pop();
                }
            }
            ("pre" | "listing", true) => {
                if self.inline_only() {
                    self.code = Some((String::new(), name.to_string()));
                    return;
                }
                self.block(2);
                self.pre = Some((String::new(), 0));
            }
            ("a", true) => {
                // A new link ends the open one, as in HTML.
                self.end_span("a");
                if let Some(t) = Writer::attr(tag, "href").and_then(|h| self.target(h)) {
                    self.open_span("a", SpanKind::Link(t));
                }
            }
            ("img", true) => self.image(tag),
            ("strong" | "b", true) => self.open_span(static_name(name), SpanKind::Mark("**")),
            ("em" | "i", true) => self.open_span(static_name(name), SpanKind::Mark("*")),
            ("del" | "s" | "strike", true) => {
                self.open_span(static_name(name), SpanKind::Mark("~~"));
            }
            ("a" | "strong" | "b" | "em" | "i" | "del" | "s" | "strike", false) => {
                self.end_span(name);
            }
            ("code" | "kbd" | "samp" | "tt", true) => {
                self.code = Some((String::new(), name.to_string()));
            }
            ("table", true) if self.inline_only() || self.flat_tables > 0 => {
                // A table in a cell or a link is its text, so nesting costs
                // nothing more.
                self.flat_tables += 1;
                self.space = true;
            }
            ("table", false) if self.flat_tables > 0 => {
                self.flat_tables -= 1;
                self.space = true;
            }
            ("tr" | "td" | "th" | "caption", _) if self.flat_tables > 0 => self.space = true,
            ("table", true) => {
                self.block(2);
                self.tables.push(Table {
                    before: (self.breaks, self.line_empty),
                    ..Table::default()
                });
            }
            ("table", false) => self.end_table(),
            ("tr", _) => self.end_row(),
            ("td" | "th", true) => {
                if self.tables.is_empty() {
                    self.space = true;
                    return;
                }
                self.end_cell();
                // Spans opened in the table outside any cell have no text
                // in this one.
                self.spans.retain(|s| s.written);
                self.space = false;
                self.line_empty = false;
                self.breaks = 0;
                let (at, spans) = (self.out.len(), self.spans.len());
                if let Some(t) = self.tables.last_mut() {
                    t.cell = Some((at, spans));
                }
            }
            ("td" | "th", false) => self.end_cell(),
            _ => {}
        }
    }

    /// Ends the containers from the `keep`th on.
    fn truncate_containers(&mut self, keep: usize) {
        while self.containers.len() > keep {
            if let Some(Container::Quote { .. }) = self.containers.pop() {
                self.quotes -= 1;
            }
        }
    }

    fn image(&mut self, tag: &Tag) {
        let alt = collapse(Writer::attr(tag, "alt").unwrap_or(""));
        if alt.is_empty() {
            return;
        }
        let src = Writer::attr(tag, "src").and_then(|s| self.target(s));
        self.start_text();
        match src {
            Some(src) => {
                self.out.push_str("![");
                escape_into(&mut self.out, &alt, false);
                self.out.push_str("](");
                self.out.push_str(&link_url(&src));
                self.out.push(')');
            }
            None => {
                let at_line = self.at_line();
                escape_into(&mut self.out, &alt, at_line);
            }
        }
        self.line_empty = false;
    }

    fn end_code(&mut self) {
        let Some((code, _)) = self.code.take() else {
            return;
        };
        // White space at its edges is a space owed outside it.
        if code.starts_with(|c: char| c.is_ascii_whitespace()) {
            self.space = true;
        }
        let trailing = code.ends_with(|c: char| c.is_ascii_whitespace());
        let code = collapse(&code);
        if code.is_empty() {
            return;
        }
        let fence = "`".repeat(longest_run(&code, '`') + 1);
        // A space each side when the code starts or ends with a backtick,
        // which CommonMark strips.
        let pad = if code.starts_with('`') || code.ends_with('`') {
            " "
        } else {
            ""
        };
        self.start_text();
        self.out.push_str(&fence);
        self.out.push_str(pad);
        self.out.push_str(&code);
        self.out.push_str(pad);
        self.out.push_str(&fence);
        self.line_empty = false;
        self.space = trailing;
    }

    fn end_pre(&mut self) {
        let Some((code, _)) = self.pre.take() else {
            return;
        };
        let code = code.strip_prefix('\n').unwrap_or(&code);
        let code = code.trim_end_matches(['\n', ' ', '\t']);
        if code.is_empty() {
            return;
        }
        // A fence longer than any run of backticks in the code.
        let fence = "`".repeat(longest_run(code, '`').max(2) + 1);
        self.block(2);
        self.start_text();
        self.out.push_str(&fence);
        let lead = self.prefix(false);
        for line in code.split('\n') {
            if self.full() {
                break;
            }
            self.out.push('\n');
            self.out.push_str(&lead);
            self.out.push_str(line);
        }
        self.out.push('\n');
        self.out.push_str(&lead);
        self.out.push_str(&fence);
        self.line_empty = false;
        self.block(2);
    }

    fn end_cell(&mut self) {
        let keep = self.cell_spans();
        let Some((at, _)) = self.tables.last().and_then(|t| t.cell) else {
            return;
        };
        self.close_spans(keep);
        let at = at.min(self.out.len());
        let cell = collapse(&self.out[at..]).replace('|', "\\|");
        self.out.truncate(at);
        if let Some(t) = self.tables.last_mut() {
            t.cell = None;
            t.row.push(cell);
        }
        self.flat_tables = 0;
        self.space = false;
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(t) = self.tables.last_mut()
            && !t.row.is_empty()
        {
            let mut row = std::mem::take(&mut t.row);
            if row.len() > MAX_COLUMNS {
                let rest = row.split_off(MAX_COLUMNS - 1).join(" ");
                row.push(rest);
            }
            t.rows.push(row);
        }
    }

    fn end_table(&mut self) {
        self.end_row();
        let Some(t) = self.tables.pop() else {
            return;
        };
        let Some(width) = t.rows.iter().map(Vec::len).max() else {
            return;
        };
        // The rows start where the table began: its cells took nothing
        // from the line.
        (self.breaks, self.line_empty) = t.before;
        for (i, row) in t.rows.iter().enumerate() {
            if self.full() {
                break;
            }
            self.block(1);
            self.start_text();
            self.out.push_str("| ");
            self.out.push_str(&row.join(" | "));
            // Only the header row is padded to the table's width: GitHub
            // fills a short body row itself.
            if i == 0 {
                for _ in row.len()..width {
                    self.out.push_str(" | ");
                }
            }
            self.out.push_str(" |");
            self.line_empty = false;
            if i == 0 {
                self.block(1);
                self.start_text();
                self.out.push('|');
                self.out.push_str(&" --- |".repeat(width));
            }
        }
        self.block(2);
    }

    fn finish(mut self) -> String {
        let run = std::mem::take(&mut self.run);
        self.text(&run);
        self.hidden = None;
        if self.code.is_some() {
            self.end_code();
        }
        if self.pre.is_some() {
            self.end_pre();
        }
        while !self.tables.is_empty() {
            self.end_table();
        }
        self.close_spans(0);
        let mut end = self.out.trim_end().len().min(STORE_BYTES);
        while !self.out.is_char_boundary(end) {
            end -= 1;
        }
        self.out.truncate(end);
        self.out
    }
}

/// Whether an element `name` opened by `tag` hides its content.
fn hides(name: &str, tag: &Tag) -> bool {
    match name {
        "template" => Writer::attr(tag, "shadowrootmode").is_none(),
        "script" | "style" | "noscript" | "svg" | "math" | "select" | "textarea" | "iframe"
        | "object" | "noembed" | "noframes" | "title" | "xmp" => true,
        _ => false,
    }
}

/// `name` as one of the block elements [`Writer::track_block`] keeps,
/// or none for another element.
fn block_name(name: &str) -> Option<&'static str> {
    const BLOCKS: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "body",
        "center",
        "dd",
        "details",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "li",
        "listing",
        "main",
        "menu",
        "nav",
        "ol",
        "p",
        "pre",
        "section",
        "summary",
        "table",
        "td",
        "th",
        "tr",
        "ul",
    ];
    BLOCKS.iter().find(|b| **b == name).copied()
}

/// `name` as one of the inline element names [`Span`] keeps.
fn static_name(name: &str) -> &'static str {
    match name {
        "strong" => "strong",
        "b" => "b",
        "em" => "em",
        "i" => "i",
        "del" => "del",
        "s" => "s",
        _ => "strike",
    }
}

/// Removes the spaces `out` ends with, down to `floor` at most; whether
/// it removed any.
fn trim_spaces(out: &mut String, floor: usize) -> bool {
    let mut any = false;
    while out.len() > floor && out.ends_with(' ') {
        out.pop();
        any = true;
    }
    any
}

/// `u` as a Markdown link destination: in angle brackets when it holds a
/// space or a parenthesis, which would end it otherwise.
fn link_url(u: &str) -> String {
    if u.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", u.replace('<', "%3C").replace('>', "%3E"))
    } else {
        u.to_string()
    }
}

/// `s` with each run of whitespace one space, trimmed.
fn collapse(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

/// The longest run of `c` in `s`.
fn longest_run(s: &str, c: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for ch in s.chars() {
        if ch == c {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Appends `word`, escaping what Markdown would read as markup; as a
/// line's first word (`at_line`), also a heading, quote, list or rule
/// marker.
fn escape_into(out: &mut String, word: &str, at_line: bool) {
    if at_line
        && matches!(
            word.as_bytes().first(),
            Some(b'#' | b'>' | b'+' | b'-' | b'=')
        )
    {
        out.push('\\');
    }
    // An ordered list marker's `.` or `)` is escaped: a backslash before a
    // digit is no escape, and would show.
    let marker = (at_line && ordered_marker(word)).then(|| word.len() - 1);
    let mut chars = word.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let escape = Some(i) == marker
            || match c {
                '\\' | '*' | '_' | '`' | '[' | ']' | '~' | '<' => true,
                // A character reference (`&amp;`, `&#1;`) would be decoded.
                '&' => chars
                    .peek()
                    .is_some_and(|(_, n)| n.is_ascii_alphanumeric() || *n == '#'),
                _ => false,
            };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Whether `word` would start an ordered list item (`1.` or `1)`).
fn ordered_marker(word: &str) -> bool {
    let digits = word.bytes().take_while(u8::is_ascii_digit).count();
    (1..=9).contains(&digits)
        && matches!(word.as_bytes().get(digits), Some(b'.' | b')'))
        && word.len() == digits + 1
}

#[cfg(test)]
mod tests {
    use super::super::page::links_of_html;
    use super::*;

    fn page() -> PublicUrl {
        PublicUrl::parse("https://site.test/a/b.html").unwrap()
    }

    fn md(html: &str) -> String {
        html_to_markdown(html, &page())
    }

    #[test]
    fn headings_paragraphs_and_breaks() {
        assert_eq!(
            md("<h1>Title</h1><p>One\n  two</p><p>Three<br>four</p><h3>Sub</h3>"),
            "# Title\n\nOne two\n\nThree\nfour\n\n### Sub"
        );
        // A heading with nothing to show writes no marker.
        assert_eq!(md("<h1><img src=/logo.png></h1><p>a</p>"), "a");
    }

    #[test]
    fn what_a_reader_does_not_see_is_dropped() {
        let page = "<html><head><title>T</title><style>p{}</style></head>\
            <body><script>var a = '<p>no</p>';</script><noscript>nojs</noscript>\
            <p>Seen<!-- hidden --></p><template><p>tpl</p></template>\
            <svg><text>svg</text></svg><select><option>o</select></body></html>";
        assert_eq!(md(page), "Seen");
        // A button's label and a declarative shadow root are seen.
        assert_eq!(
            md("<button>Read more</button><template shadowrootmode=open><p>Shadow</p></template>"),
            "Read more\n\nShadow"
        );
    }

    #[test]
    fn a_head_left_open_hides_nothing_but_its_own() {
        assert_eq!(
            md("<html><head><meta charset=utf-8><title>T</title><body><p>Hello</p>"),
            "Hello"
        );
        assert_eq!(md("<p>a</p><head><p>b</p>"), "a\n\nb");
    }

    #[test]
    fn foreign_content_ends_where_a_browser_ends_it() {
        assert_eq!(md(r#"<svg><path d=""/><p>Body</p>"#), "Body");
        assert_eq!(md("<math><mi>x</mi></math><p>After</p>"), "After");
        assert_eq!(md("<svg/><p>a</p>"), "a");
    }

    #[test]
    fn links_and_images_resolve_against_the_page() {
        assert_eq!(
            md(
                r#"<p>See <a href="../c?x=1">the page</a> and <a href="https://o.test/">o</a>.</p>"#
            ),
            "See [the page](https://site.test/c?x=1) and [o](https://o.test/)."
        );
        assert_eq!(
            md(r#"<img src="/i.png" alt="A cat"> <img src="/x.png">"#),
            "![A cat](https://site.test/i.png)"
        );
    }

    #[test]
    fn the_base_is_read_as_the_pages_links_read_it() {
        let html = r#"<head><base target=_blank><base href="https://cdn.test/d/"></head><a href="e">e</a>"#;
        assert_eq!(md(html), "[e](https://cdn.test/d/e)");
        assert_eq!(links_of_html(html, &page()), ["https://cdn.test/d/e"]);
        // A first base too long to read leaves the page's own, for both,
        // whatever base follows it.
        let long = format!(
            r#"<base href="https://cdn.test/{}"><base href="https://o.test/"><a href="e">e</a>"#,
            "x".repeat(MAX_TARGET_BYTES)
        );
        assert_eq!(md(&long), "[e](https://site.test/a/e)");
        assert_eq!(links_of_html(&long, &page()), ["https://site.test/a/e"]);
    }

    #[test]
    fn a_link_that_is_not_http_or_too_long_keeps_its_text() {
        assert_eq!(
            md(
                r#"<a href="javascript:go()">Go</a> <a href="mailto:a@b.test">mail</a> <a href="/x"></a>"#
            ),
            "Go mail"
        );
        let far = format!(r#"<a href="/{}">far</a>"#, "x".repeat(2_000));
        assert_eq!(md(&far), "far");
    }

    #[test]
    fn a_target_with_spaces_or_parentheses_is_bracketed() {
        assert_eq!(
            md(r#"<a href="https://w.test/A_(b)">w</a>"#),
            "[w](<https://w.test/A_(b)>)"
        );
    }

    #[test]
    fn a_new_link_ends_the_open_one() {
        assert_eq!(
            md(r#"<a href="/1">one <a href="/2">two</a> three"#),
            "[one](https://site.test/1) [two](https://site.test/2) three"
        );
    }

    #[test]
    fn a_card_link_is_one_line() {
        assert_eq!(
            md(r#"<a href="https://x.test/"><h3>Title</h3><p>Desc</p></a><p>next</p>"#),
            "[Title Desc](https://x.test/)\n\nnext"
        );
    }

    #[test]
    fn a_bang_before_a_link_is_not_an_image() {
        assert_eq!(
            md(r#"<p>Wow!<a href="https://x.test/">pic</a></p>"#),
            "Wow\\![pic](https://x.test/)"
        );
    }

    #[test]
    fn emphasis_and_strikethrough() {
        assert_eq!(
            md("<p><strong>bold</strong> <em>it</em> <del>gone</del></p>"),
            "**bold** *it* ~~gone~~"
        );
        assert_eq!(md("<p>a<b></b>b</p>"), "ab");
        assert_eq!(md("<i><b>x</b></i>"), "***x***");
        assert_eq!(md("<i>a*</i>"), "*a\\**");
        // Spaces at an element's edges go outside its marks.
        assert_eq!(
            md("<p>Hello<b> world </b>again</p>"),
            "Hello **world** again"
        );
    }

    #[test]
    fn code_is_written_as_it_is() {
        assert_eq!(md("<code>a_b*c</code>"), "`a_b*c`");
        assert_eq!(md("<code>a`b</code>"), "``a`b``");
        assert_eq!(md("<code>`x</code>"), "`` `x ``");
        assert_eq!(md("<p>run <kbd>ls  -l</kbd> now</p>"), "run `ls -l` now");
    }

    #[test]
    fn lists_nest_and_number_from_their_start() {
        assert_eq!(
            md(
                "<ul><li>one</li><li>two<ul><li>inner</li></ul></li></ul><ol start=3><li>c</li><li>d</ol>"
            ),
            "- one\n- two\n  - inner\n\n3. c\n4. d"
        );
        // Unclosed items end at the next item.
        assert_eq!(md("<ul><li>a<li>b</ul>"), "- a\n- b");
        // CommonMark reads at most nine digits as a number.
        assert_eq!(
            md("<ol start=18446744073709551615><li>a<li>b</ol>"),
            "999999999. a\n999999999. b"
        );
    }

    #[test]
    fn an_item_that_starts_with_a_block_keeps_its_marker() {
        assert_eq!(
            md("<ul><li><p>a</p><p>b</p></li><li><h3>c</h3></li></ul>"),
            "- a\n\n  b\n\n- ### c"
        );
        assert_eq!(md("<ul><li><ul><li>x</li></ul></li></ul>"), "- - x");
    }

    #[test]
    fn content_under_an_item_lines_up_with_its_text() {
        assert_eq!(md("<ol><li>a<p>b</p></li></ol>"), "1. a\n\n   b");
        assert_eq!(
            md("<ol start=10><li>a<ul><li>b</li></ul></li></ol>"),
            "10. a\n    - b"
        );
        assert_eq!(
            md("<ul><li>x<blockquote>q</blockquote></li></ul>"),
            "- x\n\n  > q"
        );
    }

    #[test]
    fn item_text_that_reads_as_a_marker_is_escaped() {
        assert_eq!(
            md("<ul><li>1. Install</li><li># Title</li><li>- x</li></ul>"),
            "- 1\\. Install\n- \\# Title\n- \\- x"
        );
    }

    #[test]
    fn preformatted_text_is_fenced_verbatim() {
        assert_eq!(
            md("<p>x</p><pre><code>fn  a() {\n    b(`c`);\n}\n</code></pre><p>y</p>"),
            "x\n\n```\nfn  a() {\n    b(`c`);\n}\n```\n\ny"
        );
        assert_eq!(md("<pre>a ``` b</pre>"), "````\na ``` b\n````");
        assert_eq!(md("<pre><b>*x*</b></pre>"), "```\n*x*\n```");
        // In a list item, its lines line up with the item's text.
        assert_eq!(
            md("<ul><li>a<pre>b\nc</pre></li></ul>"),
            "- a\n\n  ```\n  b\n  c\n  ```"
        );
    }

    #[test]
    fn block_quotes_mark_every_line() {
        assert_eq!(
            md("<blockquote><p>a</p><p>b</p></blockquote><p>c</p>"),
            "> a\n>\n> b\n\nc"
        );
    }

    #[test]
    fn tables_become_pipe_tables() {
        assert_eq!(
            md(
                "<table><tr><th>Name</th><th>Qty</th></tr><tr><td>a|b</td><td>2<br>pcs</td></tr><tr><td>c</td></tr></table>"
            ),
            "| Name | Qty |\n| --- | --- |\n| a\\|b | 2 pcs |\n| c |"
        );
        // A short header row is padded to the widest row.
        assert_eq!(
            md("<table><tr><td>a</td></tr><tr><td>b</td><td>c</td></tr></table>"),
            "| a |  |\n| --- | --- |\n| b | c |"
        );
    }

    #[test]
    fn a_table_in_a_cell_is_its_text() {
        assert_eq!(
            md(
                "<table><tr><td>a<table><tr><td>b</td><td>c</td></tr></table></td><td>d</td></tr></table>"
            ),
            "| a b c | d |\n| --- | --- |"
        );
    }

    #[test]
    fn marks_and_links_left_open_across_a_cell_end_at_it() {
        assert_eq!(md("<b><table><td></b>x"), "| x |\n| --- |");
        assert_eq!(md(r#"<a href="/x"><table><td></a>y"#), "y");
        assert_eq!(
            md("<table><td><b>x</td><td>y</b></td></table>"),
            "| **x** | y |\n| --- | --- |"
        );
    }

    #[test]
    fn text_that_reads_as_markup_is_escaped() {
        assert_eq!(
            md("<p># not a heading</p><p>a*b*c [x] _y_ `z` ~w~</p><p>1. no list</p>"),
            "\\# not a heading\n\na\\*b\\*c \\[x\\] \\_y\\_ \\`z\\` \\~w\\~\n\n1\\. no list"
        );
        // Decoded `<` and `&` stay text: no autolink, raw HTML or reference.
        assert_eq!(
            md("<p>&lt;javascript:alert(1)&gt; &amp;amp; a & b</p>"),
            "\\<javascript:alert(1)> \\&amp; a & b"
        );
    }

    #[test]
    fn a_marker_after_a_heading_or_a_mark_is_not_escaped() {
        assert_eq!(md("<h2>1. Introduction</h2>"), "## 1. Introduction");
        assert_eq!(md("<p><b>1.</b> first</p>"), "**1.** first");
    }

    #[test]
    fn a_table_first_in_an_item_or_a_quote_keeps_its_lines() {
        assert_eq!(
            md(
                "<p>intro</p><ul><li><table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table></li></ul>"
            ),
            "intro\n\n- | a | b |\n  | --- | --- |\n  | c | d |"
        );
        assert_eq!(
            md("<blockquote><table><tr><td>a</td></tr></table></blockquote>"),
            "> | a |\n> | --- |"
        );
    }

    #[test]
    fn a_link_left_open_ends_with_its_block() {
        assert_eq!(
            md(r#"<p><a href="/x">Home</p><h1>Title</h1><ul><li>a</li><li>b</li></ul>"#),
            "[Home](https://site.test/x)\n\n# Title\n\n- a\n- b"
        );
        assert_eq!(
            md("<ul><li><a href=/1>one</li><li><a href=/2>two</li></ul>"),
            "- [one](https://site.test/1)\n- [two](https://site.test/2)"
        );
        // A table it flattened ends with it.
        assert_eq!(
            md("<a href=/x><table><td></a>y<table><tr><td>a</td><td>b</td></tr></table>"),
            "y\n\n| a | b |\n| --- | --- |"
        );
    }

    #[test]
    fn inline_code_ends_at_a_block_and_keeps_its_links() {
        assert_eq!(
            md("<p><code>x</p><ul><li>a</li></ul><p>After <a href=/y>link</a></p>"),
            "`x`\n\n- a\n\nAfter [link](https://site.test/y)"
        );
        assert_eq!(
            md(r#"<code><a href="/struct.Vec.html">Vec</a>&lt;T&gt;</code>"#),
            "[`Vec`](https://site.test/struct.Vec.html)`<T>`"
        );
        assert_eq!(md("<code><pre>a\nb</pre></code>"), "```\na\nb\n```");
        // An anchor without a target does not split it; the spaces around
        // a link in it stay.
        assert_eq!(md("<code>a<a name=x>b</a>c</code>"), "`abc`");
        assert_eq!(
            md("<code><a href=/f>foo</a> bar</code>"),
            "[`foo`](https://site.test/f) `bar`"
        );
    }

    #[test]
    fn implied_ends_stay_in_their_scope() {
        // A block ends the paragraph it opens in, so an inner paragraph
        // does not end the link that opened in the block.
        assert_eq!(
            md("<p>Intro<div class=card><a href=/x><h3>T</h3><p>Desc</p></a></div></p>"),
            "Intro\n\n[T Desc](https://site.test/x)"
        );
        // A table's rows do not end a link in the cell around it.
        assert_eq!(
            md(
                "<table><tr><td><a href=/x>Card<table><tr><td>inner</td></tr></table></a></td></tr></table>"
            ),
            "| [Card inner](https://site.test/x) |\n| --- |"
        );
        // A nested list's item does not end the outer item's link.
        assert_eq!(
            md("<ul><li><a href=/x>a<ul><li>b</li></ul>c</a></li></ul>"),
            "- [a b c](https://site.test/x)"
        );
        // A card link in a paragraph goes on through its blocks.
        assert_eq!(
            md("<p><a href=/x><div>T</div><div>D</div></a></p><p>next</p>"),
            "[T D](https://site.test/x)\n\nnext"
        );
        // `/>` on a block is ignored, as in HTML.
        assert_eq!(
            md("<div><a href=/x><div/>Title</div> more</a></div>"),
            "[Title more](https://site.test/x)"
        );
    }

    #[test]
    fn marks_opened_outside_a_cell_are_not_written_in_it() {
        assert_eq!(
            md("<table><tr><b><td>x</td><td>y</td></tr></table>"),
            "| x | y |\n| --- | --- |"
        );
        assert_eq!(
            md("<table><tr><a href=/1><td><a href=/2>x</a></td></tr></table>"),
            "| [x](https://site.test/2) |\n| --- |"
        );
    }

    #[test]
    fn deep_marks_are_forgotten_at_a_block() {
        let page = format!("{}<p><b>x</b> y</p>", "<b>".repeat(MAX_SPANS + 1));
        assert_eq!(md(&page), "**x** y");
    }

    #[test]
    fn items_outside_a_list_are_siblings() {
        assert_eq!(md("<li>a<li>b<li>c"), "- a\n- b\n- c");
    }

    #[test]
    fn character_references_are_decoded() {
        assert_eq!(md("<p>caf&eacute; &#x1F600;</p>"), "café 😀");
    }

    #[test]
    fn hostile_pages_are_converted_in_one_pass() {
        let n = 20_000;
        // Unclosed everything: still all the text.
        let out = md(&"<ul><li><a href=x><b><table><tr><td><blockquote>w ".repeat(n));
        assert_eq!(out.matches('w').count(), n);
        // Deep quotes and lists: each line's prefix stays short.
        let deep = md(&"<blockquote><ul><li>q ".repeat(n));
        assert_eq!(deep.matches('q').count(), n);
        assert!(
            deep.lines().all(|l| l.len() < 100),
            "a prefix grew with depth"
        );
        // Deep inline elements and their ends.
        let spans = md(&format!("{}x{}", "<b><i>".repeat(n), "</i></b>".repeat(n)));
        assert!(spans.contains('x'));
        let nested = md(&"<table><tr><td>t ".repeat(n));
        assert_eq!(nested.matches('t').count(), n);
        // One wide row and many short ones: rows are not padded.
        let wide = format!(
            "<table><tr>{}<tr>{}</table>",
            "<td>c".repeat(n),
            "<td>r<tr>".repeat(n)
        );
        assert!(md(&wide).len() < 10 * wide.len());
        // A target is the page's URL at most, however short its href.
        let links = md(&r#"<a href="">x</a>"#.repeat(10_000));
        assert_eq!(
            links.matches("[x](https://site.test/a/b.html)").count(),
            10_000
        );
        // The output stops at what the page store keeps, however a page
        // makes its lines long.
        let big = md(&"<p>word ".repeat(STORE_BYTES / 2));
        assert!(big.len() <= STORE_BYTES);
        let lines = md(&format!(
            "{}<pre>{}",
            "<ol start=123456789><li>".repeat(9),
            "\n".repeat(STORE_BYTES)
        ));
        assert!(lines.len() <= STORE_BYTES);
        let pre = format!("<pre>{}", "`".repeat(n));
        assert!(md(&pre).contains(&"`".repeat(n)));
        assert_eq!(md(""), "");
        // Nothing here panics.
        for page in [
            "<b><table><td></b>",
            r#"<a href="/x"><table><td></a>"#,
            "<td>é</td><table><td>é<b>é</td></b>",
            "é!<table><td><a href=/x>a</a>",
            "</ul></li></blockquote></table></td></a></b></pre></code>",
        ] {
            md(page);
        }
    }
}
