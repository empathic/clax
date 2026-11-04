//! HTML as Markdown, made where the page was fetched: the daemon's
//! conversion of a fetched HTML page ([`html_to_markdown`]), so reading
//! one needs no outside service and the page goes nowhere else.

use super::PublicUrl;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use std::cell::RefCell;

/// `html` as Markdown: headings, paragraphs, lists (nested, ordered from
/// their `start`), links and images with their targets resolved against
/// the document's base URL (its first `<base href>`, else `base`),
/// emphasis, inline code, preformatted blocks as fenced code, block quotes,
/// rules and tables as pipe tables. What a reader does not see is dropped:
/// `head`, scripts, styles, `template`, `noscript`, `svg`, forms' controls
/// and comments. A link or image whose target is not an `http` or `https`
/// URL keeps only its text; an image without an `alt` is dropped. Text
/// that Markdown would read as markup (`*`, `_`, `` ` ``, `[`, `]`, `\`, a
/// leading `#`, `>` or list marker) is escaped.
///
/// A tokenizer pass, not a tree: unclosed elements end where HTML's
/// implied ends would put them only roughly (a paragraph at the next
/// block, a list item at the next item of its list, the rest at the end
/// of the page), so what a malformed page reads as may differ from a
/// browser's rendering; its text is all there. Linear in `html`'s length.
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

/// The most bytes of a link or image target kept: a URL of
/// [`MAX_URL_CHARS`](super::url::MAX_URL_CHARS) characters is at most four
/// bytes each. A longer one (a `data:` image, say) keeps only its text.
const MAX_TARGET_BYTES: usize = super::url::MAX_URL_CHARS * 4;

/// The most levels of block quotes and of lists a line's prefix shows:
/// deeper nesting reads as this deep, so a line costs at most this much
/// prefix however deep a page nests.
const MAX_DEPTH: usize = 8;

struct Sink(RefCell<Writer>);

impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut w = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => {
                let raw = w.tag(&tag);
                if let Some(kind) = raw {
                    return TokenSinkResult::RawData(kind);
                }
            }
            Token::CharacterTokens(t) => w.text(&t),
            Token::NullCharacterToken
            | Token::CommentToken(_)
            | Token::DoctypeToken(_)
            | Token::EOFToken
            | Token::ParseError(_) => {}
        }
        TokenSinkResult::Continue
    }
}

/// A list being written: its items' marker and the next item's number.
struct List {
    ordered: bool,
    next: u64,
}

/// A table being written: its rows, each a list of cells, the row being
/// read, the cell being read (where it starts in the output), and whether
/// the first row was a header row.
#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<usize>,
}

struct Writer {
    base: url::Url,
    base_seen: bool,
    out: String,
    /// Inside an element whose content is dropped, how deep.
    hidden: usize,
    /// The element that opened the hidden content, so only its own end
    /// closes it (its name, and how many of it are open inside it).
    hidden_by: Option<(String, usize)>,
    lists: Vec<List>,
    quotes: usize,
    /// Inside `pre`: its text so far, kept verbatim.
    pre: Option<String>,
    pre_depth: usize,
    /// An open link's target, and where its text starts in `out`.
    link: Option<(String, usize)>,
    tables: Vec<Table>,
    /// Tables opened inside a cell, read as that cell's text, how deep.
    flat_tables: usize,
    /// A space is owed before the next text on this line.
    space: bool,
    /// Nothing has been written on this line yet (its prefix not either).
    line_start: bool,
    /// The blank lines owed before the next text: 1 a line break, 2 a
    /// paragraph break.
    breaks: u8,
}

impl Writer {
    fn new(base: &PublicUrl) -> Writer {
        Writer {
            base: base.as_url().clone(),
            base_seen: false,
            out: String::new(),
            hidden: 0,
            hidden_by: None,
            lists: Vec::new(),
            quotes: 0,
            pre: None,
            pre_depth: 0,
            link: None,
            tables: Vec::new(),
            flat_tables: 0,
            space: false,
            line_start: true,
            breaks: 0,
        }
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
        matches!(u.scheme(), "http" | "https").then(|| u.to_string())
    }

    /// Whether a cell is being read: inside it, line and paragraph breaks
    /// are spaces.
    fn in_cell(&self) -> bool {
        self.tables.last().is_some_and(|t| t.cell.is_some())
    }

    /// Owes a break of `n` (1 a line, 2 a paragraph) before the next text.
    fn block(&mut self, n: u8) {
        if self.in_cell() {
            self.space = true;
            return;
        }
        if !self.out.is_empty() {
            self.breaks = self.breaks.max(n);
        }
        self.space = false;
    }

    /// What starts each line: the open lists' indentation and the block
    /// quotes' markers.
    fn prefix(&self) -> String {
        let enclosing = self.lists.len().saturating_sub(1);
        let mut p = self.quote_marks();
        p.push_str(&" ".repeat(indent_of(&self.lists[..enclosing])));
        p
    }

    /// The block quotes' markers, at most [`MAX_DEPTH`] of them.
    fn quote_marks(&self) -> String {
        "> ".repeat(self.quotes.min(MAX_DEPTH))
    }

    /// Pays the breaks owed and the line's prefix, before text.
    fn start_text(&mut self) {
        if self.breaks > 0 {
            while self.out.ends_with(' ') {
                self.out.pop();
            }
            let quote = self.quote_marks();
            for i in 0..self.breaks {
                self.out.push('\n');
                // A blank line inside a block quote stays inside it.
                if i + 1 < self.breaks && self.quotes > 0 {
                    self.out.push_str(quote.trim_end());
                }
            }
            self.breaks = 0;
            self.line_start = true;
            self.space = false;
        }
        if self.line_start {
            let p = self.prefix();
            // A list item's continuation lines line up under its text.
            let item = if self.lists.is_empty() { "" } else { "  " };
            if !self.out.is_empty() && !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push_str(&p);
            if !self.out.ends_with("- ") && !ends_with_number(&self.out) {
                self.out.push_str(item);
            }
            self.line_start = false;
        } else if self.space {
            self.out.push(' ');
        }
        self.space = false;
    }

    fn push_markup(&mut self, s: &str) {
        self.start_text();
        self.out.push_str(s);
    }

    fn text(&mut self, t: &str) {
        if self.hidden > 0 {
            return;
        }
        if let Some(pre) = self.pre.as_mut() {
            pre.push_str(t);
            return;
        }
        let mut words = t.split(|c: char| c.is_ascii_whitespace()).peekable();
        if t.starts_with(|c: char| c.is_ascii_whitespace()) {
            self.space = true;
        }
        while let Some(w) = words.next() {
            if w.is_empty() {
                continue;
            }
            let at_line = self.line_start || self.breaks > 0;
            self.start_text();
            escape_into(&mut self.out, w, at_line);
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
        if let Some((by, nested)) = self.hidden_by.as_mut() {
            if by == name {
                if start && !tag.self_closing {
                    *nested += 1;
                } else if !start {
                    if *nested == 0 {
                        self.hidden_by = None;
                        self.hidden = 0;
                    } else {
                        *nested -= 1;
                    }
                }
            }
            return if start { raw } else { None };
        }
        if start
            && matches!(
                name,
                "head"
                    | "script"
                    | "style"
                    | "template"
                    | "noscript"
                    | "svg"
                    | "math"
                    | "select"
                    | "textarea"
                    | "button"
                    | "iframe"
                    | "object"
                    | "noembed"
                    | "noframes"
                    | "title"
            )
        {
            if name == "head" || !tag.self_closing || raw.is_some() {
                self.hidden = 1;
                self.hidden_by = Some((name.to_string(), 0));
            }
            return raw;
        }
        if name == "base" && start && !self.base_seen {
            self.base_seen = true;
            if let Some(b) = Writer::attr(tag, "href").and_then(|h| self.base.join(h.trim()).ok()) {
                self.base = b;
            }
            return None;
        }
        if self.pre.is_some() {
            match (name, start) {
                ("pre", true) => self.pre_depth += 1,
                ("pre", false) if self.pre_depth > 0 => self.pre_depth -= 1,
                ("pre", false) => self.end_pre(),
                ("br", _) if start => self.pre.as_mut().expect("in pre").push('\n'),
                _ => {}
            }
            return None;
        }
        match (name, start) {
            (
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
                | "nav" | "figure" | "figcaption" | "address" | "details" | "summary" | "form"
                | "fieldset" | "dl" | "center",
                _,
            ) => self.block(2),
            ("dt" | "dd", _) => self.block(1),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                self.block(2);
                if !self.in_cell() {
                    let level = usize::from(name.as_bytes()[1] - b'0');
                    self.push_markup(&format!("{} ", "#".repeat(level)));
                }
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => self.block(2),
            ("br", _) if start => self.block(1),
            ("hr", _) if start => {
                self.block(2);
                if !self.in_cell() {
                    self.push_markup("---");
                    self.block(2);
                }
            }
            ("blockquote", true) => {
                self.block(2);
                self.quotes += 1;
            }
            ("blockquote", false) => {
                self.block(2);
                self.quotes = self.quotes.saturating_sub(1);
            }
            ("ul" | "ol" | "menu", true) => {
                self.block(if self.lists.is_empty() { 2 } else { 1 });
                let next = Writer::attr(tag, "start")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(1);
                self.lists.push(List {
                    ordered: name == "ol",
                    next,
                });
            }
            ("ul" | "ol" | "menu", false) => {
                self.lists.pop();
                self.block(if self.lists.is_empty() { 2 } else { 1 });
            }
            ("li", true) => {
                self.block(1);
                if self.in_cell() {
                    return None;
                }
                let marker = match self.lists.last_mut() {
                    Some(l) if l.ordered => {
                        let m = format!("{}. ", l.next);
                        l.next += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
                if self.lists.is_empty() {
                    self.push_markup(&marker);
                } else {
                    // The marker goes at the list's own indentation.
                    self.start_marker(&marker);
                }
            }
            ("li", false) => self.block(1),
            ("pre", true) => {
                self.block(2);
                self.pre = Some(String::new());
                self.pre_depth = 0;
            }
            ("a", true) if self.link.is_none() => {
                if let Some(t) = Writer::attr(tag, "href").and_then(|h| self.target(h)) {
                    self.start_text();
                    self.out.push('[');
                    self.link = Some((t, self.out.len()));
                }
            }
            ("a", false) => {
                if let Some((t, at)) = self.link.take() {
                    if self.out.len() == at {
                        // No text: no link.
                        self.out.pop();
                    } else {
                        while self.out.ends_with(' ') {
                            self.out.pop();
                        }
                        self.out.push_str("](");
                        self.out.push_str(&link_url(&t));
                        self.out.push(')');
                    }
                }
            }
            ("img", true) => {
                let alt = Writer::attr(tag, "alt").map(str::trim).unwrap_or("");
                let src = Writer::attr(tag, "src").and_then(|s| self.target(s));
                if let (false, Some(src)) = (alt.is_empty(), src) {
                    self.start_text();
                    self.out.push_str("![");
                    escape_into(&mut self.out, &collapse(alt), false);
                    self.out.push_str("](");
                    self.out.push_str(&link_url(&src));
                    self.out.push(')');
                } else if !alt.is_empty() {
                    self.start_text();
                    escape_into(&mut self.out, &collapse(alt), false);
                }
            }
            ("strong" | "b", _) => self.inline_mark("**", start),
            ("em" | "i", _) => self.inline_mark("*", start),
            ("del" | "s" | "strike", _) => self.inline_mark("~~", start),
            ("code" | "kbd" | "samp" | "tt", _) => self.inline_mark("`", start),
            ("table", true) if self.in_cell() || self.flat_tables > 0 => {
                // A table in a cell is that cell's text, so nesting costs
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
                self.tables.push(Table::default());
            }
            ("table", false) => self.end_table(),
            ("tr", true) => self.end_row(),
            ("tr", false) => self.end_row(),
            ("td" | "th", true) => {
                self.end_cell();
                if let Some(t) = self.tables.last_mut() {
                    self.space = false;
                    self.line_start = false;
                    self.breaks = 0;
                    t.cell = Some(self.out.len());
                }
            }
            ("td" | "th", false) => self.end_cell(),
            _ => {}
        }
        None
    }

    /// Writes a list item's marker at its list's indentation.
    fn start_marker(&mut self, marker: &str) {
        if self.breaks > 0 || self.line_start {
            let saved = self.lists.pop();
            // The prefix of the enclosing lists, with this one's marker.
            let pushed = saved.is_some();
            self.breaks = self.breaks.max(1);
            if pushed {
                // Pay the breaks and the enclosing lists' prefix only.
                let breaks = self.breaks;
                self.breaks = 0;
                while self.out.ends_with(' ') {
                    self.out.pop();
                }
                if !self.out.is_empty() {
                    let quote = self.quote_marks();
                    for i in 0..breaks {
                        self.out.push('\n');
                        if i + 1 < breaks && self.quotes > 0 {
                            self.out.push_str(quote.trim_end());
                        }
                    }
                }
                let mut p = self.quote_marks();
                p.push_str(&" ".repeat(indent_of(&self.lists)));
                self.out.push_str(&p);
                self.out.push_str(marker);
                self.line_start = false;
                self.space = false;
            }
            if let Some(l) = saved {
                self.lists.push(l);
            }
        } else {
            self.push_markup(marker);
        }
    }

    fn inline_mark(&mut self, mark: &str, start: bool) {
        if start {
            self.start_text();
            self.out.push_str(mark);
        } else if self.out.ends_with(mark) {
            // Empty: write nothing.
            let n = self.out.len() - mark.len();
            self.out.truncate(n);
        } else {
            while self.out.ends_with(' ') {
                self.out.pop();
                self.space = true;
            }
            self.out.push_str(mark);
        }
    }

    fn end_pre(&mut self) {
        let Some(code) = self.pre.take() else {
            return;
        };
        let code = code.strip_prefix('\n').unwrap_or(&code);
        let code = code.trim_end_matches(['\n', ' ', '\t']);
        if code.is_empty() {
            return;
        }
        // A fence longer than any run of backticks in the code.
        let longest = longest_run(code, '`');
        let fence = "`".repeat(longest.max(2) + 1);
        self.block(2);
        self.start_text();
        let p = self.prefix();
        let item = if self.lists.is_empty() { "" } else { "  " };
        let lead = format!("{p}{item}");
        self.out.push_str(&fence);
        for line in code.split('\n') {
            self.out.push('\n');
            self.out.push_str(&lead);
            self.out.push_str(line);
        }
        self.out.push('\n');
        self.out.push_str(&lead);
        self.out.push_str(&fence);
        self.block(2);
    }

    fn end_cell(&mut self) {
        let Some(t) = self.tables.last_mut() else {
            return;
        };
        let Some(at) = t.cell.take() else {
            return;
        };
        let cell = collapse(&self.out[at..]).replace('|', "\\|");
        self.out.truncate(at);
        t.row.push(cell);
        self.space = false;
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(t) = self.tables.last_mut()
            && !t.row.is_empty()
        {
            let row = std::mem::take(&mut t.row);
            t.rows.push(row);
        }
    }

    fn end_table(&mut self) {
        self.end_row();
        let Some(t) = self.tables.pop() else {
            return;
        };
        if t.rows.is_empty() {
            return;
        }
        let width = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        self.block(2);
        for (i, row) in t.rows.iter().enumerate() {
            self.block(1);
            self.start_text();
            let cells: Vec<&str> = (0..width)
                .map(|j| row.get(j).map_or("", String::as_str))
                .collect();
            self.out.push_str("| ");
            self.out.push_str(&cells.join(" | "));
            self.out.push_str(" |");
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
        if self.pre.is_some() {
            self.end_pre();
        }
        while !self.tables.is_empty() {
            self.end_table();
        }
        if let Some((t, at)) = self.link.take()
            && self.out.len() > at
        {
            self.out.push_str("](");
            self.out.push_str(&link_url(&t));
            self.out.push(')');
        }
        let trimmed = self.out.trim_end().len();
        self.out.truncate(trimmed);
        self.out
    }
}

/// The indentation of a line inside `lists`: each list's marker width,
/// for at most [`MAX_DEPTH`] of them.
fn indent_of(lists: &[List]) -> usize {
    lists
        .iter()
        .take(MAX_DEPTH)
        .map(|l| if l.ordered { 3 } else { 2 })
        .sum()
}

/// Whether `out` ends with an ordered list marker (`12. `).
fn ends_with_number(out: &str) -> bool {
    let Some(rest) = out.strip_suffix(". ") else {
        return false;
    };
    rest.bytes().last().is_some_and(|b| b.is_ascii_digit())
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

/// Appends `word`, escaping what Markdown would read as markup; at a
/// line's start (`at_line`), also a heading, quote or list marker.
fn escape_into(out: &mut String, word: &str, at_line: bool) {
    if at_line {
        let lead = matches!(
            word.as_bytes().first(),
            Some(b'#' | b'>' | b'+' | b'-' | b'=')
        ) || ordered_marker(word);
        if lead {
            out.push('\\');
        }
    }
    for c in word.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Whether `word` would start an ordered list item (`1.` or `1)`).
fn ordered_marker(word: &str) -> bool {
    let digits = word.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && digits <= 9
        && matches!(word.as_bytes().get(digits), Some(b'.' | b')'))
        && word.len() == digits + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        html_to_markdown(
            html,
            &PublicUrl::parse("https://site.test/a/b.html").unwrap(),
        )
    }

    #[test]
    fn headings_paragraphs_and_breaks() {
        assert_eq!(
            md("<h1>Title</h1><p>One\n  two</p><p>Three<br>four</p><h3>Sub</h3>"),
            "# Title\n\nOne two\n\nThree\nfour\n\n### Sub"
        );
    }

    #[test]
    fn what_a_reader_does_not_see_is_dropped() {
        let page = "<html><head><title>T</title><style>p{}</style></head>\
            <body><script>var a = '<p>no</p>';</script><noscript>nojs</noscript>\
            <p>Seen<!-- hidden --></p><template><p>tpl</p></template>\
            <svg><text>svg</text></svg><button>Click</button></body></html>";
        assert_eq!(md(page), "Seen");
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
            md(r#"<base href="https://cdn.test/d/"><a href="e">e</a>"#),
            "[e](https://cdn.test/d/e)"
        );
        assert_eq!(
            md(r#"<img src="/i.png" alt="A cat"> <img src="/x.png">"#),
            "![A cat](https://site.test/i.png)"
        );
    }

    #[test]
    fn a_link_that_is_not_http_keeps_its_text() {
        assert_eq!(
            md(
                r#"<a href="javascript:go()">Go</a> <a href="mailto:a@b.test">mail</a> <a href="/x"></a>"#
            ),
            "Go mail"
        );
    }

    #[test]
    fn a_target_with_spaces_or_parentheses_is_bracketed() {
        assert_eq!(
            md(r#"<a href="https://w.test/A_(b)">w</a>"#),
            "[w](<https://w.test/A_(b)>)"
        );
    }

    #[test]
    fn emphasis_and_inline_code() {
        assert_eq!(
            md("<p><strong>bold</strong> <em>it</em> <code>x()</code> <del>gone</del></p>"),
            "**bold** *it* `x()` ~~gone~~"
        );
        assert_eq!(md("<p>a<b></b>b</p>"), "ab");
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
    }

    #[test]
    fn preformatted_text_is_fenced_verbatim() {
        assert_eq!(
            md("<p>x</p><pre><code>fn  a() {\n    b(`c`);\n}\n</code></pre><p>y</p>"),
            "x\n\n```\nfn  a() {\n    b(`c`);\n}\n```\n\ny"
        );
        // A fence longer than any backtick run inside.
        assert_eq!(md("<pre>a ``` b</pre>"), "````\na ``` b\n````");
        // Markup inside pre is not converted; its text is kept.
        assert_eq!(md("<pre><b>*x*</b></pre>"), "```\n*x*\n```");
    }

    #[test]
    fn block_quotes_mark_every_line() {
        assert_eq!(
            md("<blockquote><p>a</p><p>b</p></blockquote><p>c</p>"),
            "> a\n>\n> b\n\nc"
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
    fn tables_become_pipe_tables() {
        assert_eq!(
            md(
                "<table><tr><th>Name</th><th>Qty</th></tr><tr><td>a|b</td><td>2<br>pcs</td></tr><tr><td>c</td></tr></table>"
            ),
            "| Name | Qty |\n| --- | --- |\n| a\\|b | 2 pcs |\n| c |  |"
        );
    }

    #[test]
    fn text_that_reads_as_markup_is_escaped() {
        assert_eq!(
            md("<p># not a heading</p><p>a*b*c [x] _y_ `z`</p><p>1. no list</p>"),
            "\\# not a heading\n\na\\*b\\*c \\[x\\] \\_y\\_ \\`z\\`\n\n\\1. no list"
        );
    }

    #[test]
    fn character_references_are_decoded() {
        assert_eq!(
            md("<p>caf&eacute; &amp; &lt;tag&gt; &#x1F600;</p>"),
            "café & <tag> 😀"
        );
    }

    #[test]
    fn hostile_pages_are_converted_in_one_pass() {
        // Unclosed everything: still linear, still all the text.
        let n = 50_000;
        let page = "<ul><li><a href=x><b><table><tr><td><blockquote>w ".repeat(n);
        let out = md(&page);
        assert_eq!(out.matches('w').count(), n);
        // Deep quotes and lists: each line's prefix stays short.
        let deep = md(&"<blockquote><ul><li>q ".repeat(n));
        assert_eq!(deep.matches('q').count(), n);
        assert!(
            deep.lines().all(|l| l.len() < 100),
            "a prefix grew with depth"
        );
        let nested = md(&"<table><tr><td>t ".repeat(n));
        assert_eq!(nested.matches('t').count(), n);
        let pre = format!("<pre>{}", "`".repeat(n));
        assert!(md(&pre).contains(&"`".repeat(n)));
        assert_eq!(md(""), "");
    }
}
