//! A fetched page's text: how a body is handled by its type
//! ([`handling`]), decoding a body by its declared charset,
//! pretty-printing JSON, the links a page holds (spec §5.2), the window a
//! script reads (spec §6.1) and search snippets.

use super::PublicUrl;
use encoding_rs::{Encoding, UTF_8};
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use std::cell::RefCell;
use std::collections::HashSet;

/// The most characters one window holds.
pub const PAGE_CHARS: usize = 20_000;
/// The most bytes of a page's text the page store keeps (1 MiB).
pub const STORE_BYTES: usize = 1 << 20;
/// The most characters of a search result's snippet.
pub const SNIPPET_CHARS: usize = 600;
/// The most links kept of one page.
pub const MAX_LINKS: usize = 1_000;
/// The most link targets of one page resolved and checked, kept or not:
/// past this many, a page's later links are not read, so a page of
/// repeated or refused targets costs at most this many URL parses.
pub const MAX_CANDIDATES: usize = 10 * MAX_LINKS;

/// `body` as text: in the charset `content_type` declares when
/// `encoding_rs` knows its label, otherwise UTF-8 with invalid sequences
/// replaced. A byte-order mark wins over the declared charset, as in a
/// browser.
#[must_use]
pub fn decode(body: &[u8], content_type: &str) -> String {
    let declared = content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']))
    });
    let encoding = declared
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .unwrap_or(UTF_8);
    encoding.decode(body).0.into_owned()
}

/// `s` pretty-printed with two-space indents, when it is JSON. Only the
/// whitespace between tokens changes: keys keep their order and numbers
/// and strings their text.
#[must_use]
pub fn pretty_json(s: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(s).ok()?;
    let mut out = String::with_capacity(s.len() * 2);
    let mut depth = 0usize;
    let (mut in_string, mut escaped) = (false, false);
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n("  ", depth));
    };
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                while chars.peek().is_some_and(char::is_ascii_whitespace) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('}' | ']')) {
                    out.extend(chars.next());
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_ascii_whitespace() => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// The `href`s of `html`'s `a`, `area` and `link` elements, resolved
/// against the document's base URL (its first `<base href>`, which applies
/// to the whole document as in a browser, else `base`), kept when they are
/// public URLs ([`PublicUrl::parse`]): normalised, each once in the order
/// first seen, at most [`MAX_LINKS`], from the first [`MAX_CANDIDATES`]
/// `href`s. Linear in the page's length.
#[must_use]
pub fn links_of_html(html: &str, base: &PublicUrl) -> Vec<String> {
    #[derive(Default)]
    struct Hrefs {
        base: RefCell<Option<String>>,
        hrefs: RefCell<Vec<String>>,
    }
    impl TokenSink for Hrefs {
        type Handle = ();
        fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
            let Token::TagToken(tag) = token else {
                return TokenSinkResult::Continue;
            };
            if tag.kind == TagKind::EndTag {
                return TokenSinkResult::Continue;
            }
            let href = tag
                .attrs
                .iter()
                .find(|a| &*a.name.local == "href")
                .map(|a| a.value.to_string());
            match (&*tag.name, href) {
                ("base", Some(h)) => {
                    let mut base = self.base.borrow_mut();
                    if base.is_none() {
                        *base = Some(h);
                    }
                }
                ("a" | "area" | "link", Some(h)) => self.hrefs.borrow_mut().push(h),
                _ => {}
            }
            // Text in these is not markup (an `href` in a script is not a
            // link), whether or not the tag is written self-closing: HTML
            // ignores `/>` on them, so `<script/>` still opens a script.
            match &*tag.name {
                "script" => TokenSinkResult::RawData(RawKind::ScriptData),
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => {
                    TokenSinkResult::RawData(RawKind::Rawtext)
                }
                "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
                _ => TokenSinkResult::Continue,
            }
        }
    }
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tok = Tokenizer::new(Hrefs::default(), TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
    let Hrefs {
        base: page_base,
        hrefs,
    } = tok.sink;
    // Resolved once: a long `<base href>` costs its length, not its length
    // per link.
    let doc_base = page_base
        .into_inner()
        .filter(|b| b.len() <= MAX_TARGET_BYTES)
        .and_then(|b| base.as_url().join(b.trim()).ok())
        .unwrap_or_else(|| base.as_url().clone());
    let mut links = Links::new();
    for href in hrefs.into_inner() {
        if !links.add_from(&doc_base, &href) {
            break;
        }
    }
    links.into_vec()
}

/// The most bytes of a link target worth resolving: a URL of
/// [`MAX_URL_CHARS`](super::url::MAX_URL_CHARS) characters is at most four
/// bytes each.
const MAX_TARGET_BYTES: usize = super::url::MAX_URL_CHARS * 4;

/// The links of Markdown `md`: inline links and images (`[t](u "title")`),
/// reference definitions (`[r]: u`), autolinks (`<u>`) and bare `http`
/// and `https` URLs, resolved against `base` and kept as
/// [`links_of_html`] keeps them: at most [`MAX_LINKS`], from the first
/// [`MAX_CANDIDATES`] targets in the page's order, whichever pass found
/// them.
///
/// A scanner, not a CommonMark parser, linear in `md`'s length: every
/// pass moves past what it read. It reads code spans and blocks like any
/// other text, and takes the target of any `](`, so it may count a string
/// as a link that a renderer would not; a link it counts is still text the
/// page holds. A target with no end before the next whitespace (an
/// unclosed `](`) is read to there and not looked at again.
#[must_use]
pub fn links_of_markdown(md: &str, base: &PublicUrl) -> Vec<String> {
    let mut found: Vec<(usize, &str)> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    // Inline links and images: `](target)` or `](<target>)`, with an
    // optional title after the target.
    let mut from = 0;
    while let Some(i) = md[from..].find("](") {
        let start = from + i + 2;
        let rest = &md[start..];
        let (target, len) = if let Some(r) = rest.strip_prefix('<') {
            match r.find(['>', '\n']) {
                Some(j) if r[j..].starts_with('>') => (&r[..j], j + 2),
                Some(j) => ("", j + 1),
                None => ("", rest.len()),
            }
        } else {
            let mut depth = 0usize;
            let mut end = rest.len();
            for (j, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        end = j;
                        break;
                    }
                    ')' => depth -= 1,
                    c if c.is_whitespace() => {
                        end = j;
                        break;
                    }
                    _ => {}
                }
            }
            (&rest[..end], end)
        };
        if !target.is_empty() {
            found.push((start, target));
            spans.push((start, start + len));
        }
        // Past the target: nothing in it is read again.
        from = start + len;
    }
    // Autolinks: `<scheme://…>`. Each scan stops at the next `<`.
    let mut from = 0;
    while let Some(i) = md[from..].find('<') {
        let start = from + i + 1;
        let rest = &md[start..];
        let j = rest.find(['>', '<', ' ', '\n']).unwrap_or(rest.len());
        if rest[j..].starts_with('>') && rest[..j].contains("://") {
            found.push((start, &rest[..j]));
            spans.push((start, start + j));
        }
        from = start + j;
    }
    // Reference definitions: a line `[label]: target`.
    let mut at = 0;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent < 4
            && trimmed.starts_with('[')
            && let Some(j) = trimmed.find("]:")
        {
            let after = &trimmed[j + 2..];
            let target = after.split_whitespace().next().unwrap_or("");
            let target = target
                .strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .unwrap_or(target);
            if !target.is_empty() {
                let start = at + indent + j + 2 + (after.len() - after.trim_start().len());
                found.push((start, target));
                spans.push((start, start + target.len()));
            }
        }
        at += line.len();
    }
    // Bare URLs, outside the links above: the spans, sorted and merged, are
    // searched by halves.
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let inside = |p: usize| {
        let k = merged.partition_point(|&(s, _)| s <= p);
        k > 0 && p < merged[k - 1].1
    };
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(i) = md[from..].find(scheme) {
            let start = from + i;
            let rest = &md[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "<>()[]\"'`".contains(c))
                .unwrap_or(rest.len());
            from = start + end.max(scheme.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if !inside(start) && url.len() > scheme.len() {
                found.push((start, url));
            }
        }
    }
    found.sort_by_key(|&(at, _)| at);
    let mut links = Links::new();
    for (_, target) in found {
        if !links.add_from(base.as_url(), target) {
            break;
        }
    }
    links.into_vec()
}

/// A page's links as they are kept: public, normalised, each once, at
/// most [`MAX_LINKS`], of at most [`MAX_CANDIDATES`] tried.
struct Links {
    tried: usize,
    seen: HashSet<String>,
    links: Vec<String>,
}

impl Links {
    fn new() -> Links {
        Links {
            tried: 0,
            seen: HashSet::new(),
            links: Vec::new(),
        }
    }

    /// Tries `href`, resolved against `base`, and keeps it when it is a
    /// public URL not kept yet; false once [`MAX_LINKS`] are kept or
    /// [`MAX_CANDIDATES`] tried, when the caller stops. A target longer than
    /// any URL kept is dropped unread, and counts as tried.
    fn add_from(&mut self, base: &::url::Url, href: &str) -> bool {
        if self.links.len() >= MAX_LINKS || self.tried >= MAX_CANDIDATES {
            return false;
        }
        self.tried += 1;
        if href.len() <= MAX_TARGET_BYTES
            && let Ok(abs) = base.join(href.trim())
            && let Ok(u) = PublicUrl::parse(abs.as_str())
            && self.seen.insert(u.normalized().to_string())
        {
            self.links.push(u.normalized().to_string());
        }
        self.links.len() < MAX_LINKS && self.tried < MAX_CANDIDATES
    }

    fn into_vec(self) -> Vec<String> {
        self.links
    }
}

/// A window of a page's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub text: String,
    /// Where `text` starts, in characters.
    pub offset: usize,
    /// Where the next window starts; `None` when this one reaches the end.
    pub next_offset: Option<usize>,
    /// The whole text's length, in characters.
    pub total_chars: usize,
}

/// The window of `text` from character `offset`: at most `page_chars`
/// characters (at least one), ending just after the last line break in
/// the window's last tenth when it does not reach the end and has one
/// there. An offset at or past the end gives an empty window at
/// `total_chars` with no next one.
#[must_use]
pub fn window(text: &str, offset: usize, page_chars: usize) -> Window {
    let page_chars = page_chars.max(1);
    let total_chars = text.chars().count();
    if offset >= total_chars {
        return Window {
            text: String::new(),
            offset: total_chars,
            next_offset: None,
            total_chars,
        };
    }
    let byte_at = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(i, _)| i)
    };
    let mut end = offset.saturating_add(page_chars).min(total_chars);
    if end < total_chars {
        // The window's last tenth, where a line break ends it early.
        let tail = end - (end - offset) / 10;
        let (tail_byte, end_byte) = (byte_at(tail), byte_at(end));
        if let Some(i) = text[tail_byte..end_byte].rfind('\n') {
            end = tail + text[tail_byte..tail_byte + i].chars().count() + 1;
        }
    }
    let start_byte = byte_at(offset);
    let end_byte = byte_at(end);
    Window {
        text: text[start_byte..end_byte].to_string(),
        offset,
        next_offset: (end < total_chars).then_some(end),
        total_chars,
    }
}

/// `s` cut to at most `max` characters at a word boundary, ending with `…`
/// (which counts) when cut; `s` whole when it fits, and empty when `max`
/// is 0.
#[must_use]
pub fn snippet(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep = max.saturating_sub(1);
    let prefix_end = s.char_indices().nth(keep).map_or(s.len(), |(i, _)| i);
    let prefix = &s[..prefix_end];
    let at_word = s[prefix_end..].starts_with(char::is_whitespace);
    let cut = if at_word {
        prefix
    } else {
        prefix
            .rfind(char::is_whitespace)
            .map_or(prefix, |i| &prefix[..i])
    };
    let cut = cut.trim_end();
    let cut = if cut.is_empty() { prefix } else { cut };
    format!("{cut}…")
}

/// A page the run fetched, as the reply cites it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The page's first `# ` heading, as page text (unescaped), if any.
    pub title: Option<String>,
    /// The page's final URL, or the one asked for when that is not
    /// `http(s)`.
    pub url: String,
}

/// The longest title a Sources list shows, in characters.
const TITLE_CHARS: usize = 200;

fn is_http(u: &str) -> bool {
    let u = u.trim_start().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// The first Markdown ATX level-1 heading of `text` outside a fenced code
/// block (up to three leading spaces, optional closing `#`s), whitespace
/// collapsed.
fn heading(text: &str) -> Option<String> {
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = line.trim_start_matches(' ');
        let run = |c: char| body.chars().take_while(|&x| x == c).count();
        if indent <= 3 {
            let mark = ['`', '~'].into_iter().find(|&c| run(c) >= 3);
            if let Some(c) = mark {
                let n = run(c);
                match fence {
                    None => {
                        fence = Some((c, n));
                        continue;
                    }
                    Some((fc, fn_)) if fc == c && n >= fn_ && body[n..].trim().is_empty() => {
                        fence = None;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        if fence.is_some() || indent > 3 {
            continue;
        }
        let Some(rest) = body.strip_prefix("# ") else {
            continue;
        };
        let rest = rest.trim_end();
        let core = rest.trim_end_matches('#');
        let rest = if core.is_empty() || core.ends_with(' ') || core.ends_with('\t') {
            core
        } else {
            rest
        };
        let t: String = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        let t: String = t.chars().take(TITLE_CHARS).collect();
        return (!t.is_empty()).then_some(t);
    }
    None
}

/// The pages `pages` stored, in the order given, as [`Source`]s. A page
/// whose final URL and requested URL are both not `http(s)` is left out,
/// and a later page with the same final URL (ignoring a fragment and trailing
/// slashes) as an earlier one is not listed again.
#[must_use]
pub fn sources(pages: &[clax_store::bot_web::WebPageRow]) -> Vec<Source> {
    let mut seen = std::collections::HashSet::new();
    pages
        .iter()
        .filter_map(|p| {
            let url = [&p.final_url, &p.url].into_iter().find(|u| is_http(u))?;
            let url = url.trim();
            let key = url.split('#').next().unwrap_or(url).trim_end_matches('/');
            seen.insert(key.to_string()).then(|| Source {
                title: heading(&p.text),
                url: url.to_string(),
            })
        })
        .collect()
}

/// `s` as Markdown link text that shows `s` and nothing else: every ASCII
/// punctuation character is backslash-escaped (which also keeps a bare URL
/// in it from becoming a link), and line breaks and control characters
/// become spaces.
fn link_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.push(' ');
        } else {
            if c.is_ascii_punctuation() {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

/// `u` as a Markdown link destination: the characters that end or open it
/// are percent-encoded.
pub fn link_target(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for c in u.chars() {
        if c.is_whitespace() || c.is_control() || "()<>\\[]".contains(c) {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The Sources list a reply ends with (spec §8): `None` without pages,
/// otherwise `"\n\nSources:\n- [<title or final URL>](<final URL>)"` for
/// each page in fetch order. A title is the page's own text, so it is
/// rendered as plain text and cannot change the link's target; the
/// target is the final URL when that is `http(s)`.
#[must_use]
pub fn sources_markdown(pages: &[clax_store::bot_web::WebPageRow]) -> Option<String> {
    let all = sources(pages);
    if all.is_empty() {
        return None;
    }
    let mut out = String::from("\n\nSources:");
    for s in &all {
        let target = link_target(&s.url);
        let text = s
            .title
            .as_deref()
            .map_or_else(|| link_text(&s.url), link_text);
        out.push_str(&format!("\n- [{text}]({target})"));
    }
    Some(out)
}

/// The `User-Agent` every source sends with a fetch: `ClaxBot/1
/// (+<origin>)` with the instance's public origin (a trailing `/`
/// dropped), so a site can tell whose bot it is and where to ask; plain
/// `ClaxBot/1` with none (`None`), so an origin that is not public (a
/// fallback such as `http://localhost`, a LAN address) is never disclosed.
/// Pass `Some` only for an origin the instance is configured to publish.
#[must_use]
pub fn user_agent(public_origin: Option<&str>) -> String {
    match public_origin.map(|o| o.trim_end_matches('/')) {
        Some(o) if !o.is_empty() => format!("ClaxBot/1 (+{o})"),
        _ => "ClaxBot/1".to_string(),
    }
}

/// What a source does with a fetched body ([`handling`]). Shared by every
/// source, so a page reads the same whichever host fetched it; the
/// variants and their meaning are stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    /// The text as it came, decoded by its charset: Markdown and other
    /// text, and every textual body but HTML in `Raw`. Its links are read
    /// from it ([`links_of_markdown`]).
    Text,
    /// HTML as it came, in `Raw`; its links are read from the HTML
    /// ([`links_of_html`]).
    RawHtml,
    /// JSON, pretty-printed ([`pretty_json`]; as it came when it does not
    /// parse), in `Auto`.
    Json,
    /// HTML converted to Markdown, in `Auto`; its links are read from the
    /// HTML before conversion.
    Html,
    /// A PDF, Office or OpenDocument file converted to Markdown under a
    /// name with this extension (`document.<ext>`), in `Auto`; its links
    /// are read from the Markdown.
    Document(&'static str),
    /// Not read (`web_unsupported_content_type`): images, audio, video,
    /// fonts and other binary types, and documents in `Raw`.
    Unsupported,
}

impl Handling {
    /// Whether the body is read as text, unconverted or converted from
    /// HTML: what a 4xx or 5xx must be to be answered as a page.
    #[must_use]
    pub fn is_textual(self) -> bool {
        matches!(
            self,
            Handling::Text | Handling::RawHtml | Handling::Json | Handling::Html
        )
    }
}

/// The documents Markdown conversion takes, by media type, with the
/// extension it reads them by.
const DOCUMENTS: &[(&str, &str)] = &[
    ("application/pdf", "pdf"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    ("application/vnd.ms-excel.sheet.macroenabled.12", "xlsm"),
    (
        "application/vnd.ms-excel.sheet.binary.macroenabled.12",
        "xlsb",
    ),
    ("application/vnd.ms-excel", "xls"),
    ("application/vnd.oasis.opendocument.spreadsheet", "ods"),
    ("application/vnd.oasis.opendocument.text", "odt"),
    ("application/vnd.apple.numbers", "numbers"),
];

/// The `application/` types that are text.
const TEXT_APPLICATIONS: &[&str] = &[
    "application/xml",
    "application/javascript",
    "application/x-javascript",
    "application/ecmascript",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/x-ndjson",
    "application/sql",
    "application/graphql",
    "application/x-sh",
];

/// A `Content-Type`'s media type, lowercase, without its parameters.
#[must_use]
pub fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// The media type a body is read as: the served one ([`media_type`]),
/// or, when none or `application/octet-stream` was served, [`sniff`]'s.
#[must_use]
pub fn media_type_of(content_type: &str, body: &[u8]) -> String {
    let served = media_type(content_type);
    if served.is_empty() || served == "application/octet-stream" {
        sniff(body).to_string()
    } else {
        served
    }
}

/// A media type for a body served without a useful one, from its first
/// bytes: PDF, binary (a NUL, or control bytes past one in twenty), HTML,
/// else plain text.
#[must_use]
pub fn sniff(body: &[u8]) -> &'static str {
    if body.is_empty() {
        return "text/plain";
    }
    let head = &body[..body.len().min(1024)];
    if head.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    let control = head
        .iter()
        .filter(|b| **b < 0x20 && !matches!(**b, 9 | 10 | 12 | 13 | 27))
        .count();
    if head.contains(&0) || control * 20 > head.len() {
        return "application/octet-stream";
    }
    let text = String::from_utf8_lossy(head);
    let start = text.trim_start().to_ascii_lowercase();
    if start.starts_with("<!doctype html") || start.starts_with("<html") {
        return "text/html";
    }
    "text/plain"
}

/// What a source does with a body of `content_type` fetched in `format`
/// (see [`Handling`]). Pass the type the body is read as,
/// [`media_type_of`]`(content_type, body)`, not the raw header: an empty or
/// `application/octet-stream` type is sniffed there, and would be
/// `Unsupported` here. (Parameters such as `charset` are ignored.)
///
/// | Type | `Auto` | `Raw` |
/// |---|---|---|
/// | `text/markdown`, other `text/*`, textual `application/*`, `+xml` | `Text` | `Text` |
/// | `text/html`, `application/xhtml+xml` | `Html` | `RawHtml` |
/// | `application/json`, `text/json`, `+json` | `Json` | `Text` |
/// | PDF, Office, OpenDocument | `Document(ext)` | `Unsupported` |
/// | `image/*`, `audio/*`, `video/*`, `font/*`, anything else | `Unsupported` | `Unsupported` |
#[must_use]
pub fn handling(content_type: &str, format: super::Format) -> Handling {
    use super::Format::{Auto, Raw};
    let media = media_type(content_type);
    let media = media.as_str();
    let top = media.split('/').next().unwrap_or("");
    if matches!(top, "image" | "audio" | "video" | "font") {
        return Handling::Unsupported;
    }
    if matches!(media, "text/html" | "application/xhtml+xml") {
        return match format {
            Auto => Handling::Html,
            Raw => Handling::RawHtml,
        };
    }
    if matches!(media, "application/json" | "text/json")
        || (media.starts_with("application/") && media.ends_with("+json"))
    {
        return match format {
            Auto => Handling::Json,
            Raw => Handling::Text,
        };
    }
    if let Some((_, ext)) = DOCUMENTS.iter().find(|(m, _)| *m == media) {
        return match format {
            Auto => Handling::Document(ext),
            Raw => Handling::Unsupported,
        };
    }
    if top == "text" || TEXT_APPLICATIONS.contains(&media) || media.ends_with("+xml") {
        return Handling::Text;
    }
    Handling::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handling_follows_the_type_and_the_format() {
        use crate::web::Format::{Auto, Raw};
        for (ct, auto, raw) in [
            (
                "text/markdown; charset=utf-8",
                Handling::Text,
                Handling::Text,
            ),
            ("TEXT/HTML", Handling::Html, Handling::RawHtml),
            ("application/xhtml+xml", Handling::Html, Handling::RawHtml),
            (
                "application/pdf",
                Handling::Document("pdf"),
                Handling::Unsupported,
            ),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                Handling::Document("docx"),
                Handling::Unsupported,
            ),
            ("application/json", Handling::Json, Handling::Text),
            ("application/ld+json", Handling::Json, Handling::Text),
            ("text/csv", Handling::Text, Handling::Text),
            ("application/xml", Handling::Text, Handling::Text),
            ("application/rss+xml", Handling::Text, Handling::Text),
            ("image/png", Handling::Unsupported, Handling::Unsupported),
            (
                "image/svg+xml",
                Handling::Unsupported,
                Handling::Unsupported,
            ),
            ("audio/mpeg", Handling::Unsupported, Handling::Unsupported),
            ("video/mp4", Handling::Unsupported, Handling::Unsupported),
            ("font/woff2", Handling::Unsupported, Handling::Unsupported),
            (
                "application/zip",
                Handling::Unsupported,
                Handling::Unsupported,
            ),
            ("", Handling::Unsupported, Handling::Unsupported),
        ] {
            assert_eq!(handling(ct, Auto), auto, "{ct} auto");
            assert_eq!(handling(ct, Raw), raw, "{ct} raw");
        }
        assert!(Handling::Html.is_textual() && Handling::Json.is_textual());
        assert!(!Handling::Document("pdf").is_textual() && !Handling::Unsupported.is_textual());
    }

    #[test]
    fn an_unhelpful_type_is_sniffed() {
        assert_eq!(sniff(b""), "text/plain");
        assert_eq!(sniff(b"%PDF-1.7 ..."), "application/pdf");
        assert_eq!(sniff(b"  <!DOCTYPE html><p>x"), "text/html");
        assert_eq!(sniff(b"a\0b"), "application/octet-stream");
        assert_eq!(sniff(b"# Notes\n\nplain"), "text/plain");
        assert_eq!(
            media_type_of("application/octet-stream", b"plain"),
            "text/plain"
        );
        assert_eq!(media_type_of("", b"%PDF-1"), "application/pdf");
        assert_eq!(
            media_type_of("Text/HTML; charset=x", b"%PDF-1"),
            "text/html"
        );
    }

    #[test]
    fn the_user_agent_names_the_instance() {
        assert_eq!(
            user_agent(Some("https://clax.example.com/")),
            "ClaxBot/1 (+https://clax.example.com)"
        );
        // No public origin: nothing about the instance is disclosed.
        assert_eq!(user_agent(None), "ClaxBot/1");
        assert_eq!(user_agent(Some("")), "ClaxBot/1");
    }

    fn base(s: &str) -> PublicUrl {
        PublicUrl::parse(s).unwrap()
    }

    #[test]
    fn a_window_ends_at_a_line_break_near_its_end() {
        // 100-char windows; a line break at char 94 (in the last tenth).
        let text = format!("{}\n{}", "a".repeat(94), "b".repeat(200));
        let w = window(&text, 0, 100);
        assert_eq!(w.text, format!("{}\n", "a".repeat(94)));
        assert_eq!(w.offset, 0);
        assert_eq!(w.next_offset, Some(95));
        assert_eq!(w.total_chars, 295);
        // A line break earlier than the last tenth does not shorten it.
        let text = format!("{}\n{}", "a".repeat(50), "b".repeat(200));
        let w = window(&text, 0, 100);
        assert_eq!(w.text.chars().count(), 100);
        assert_eq!(w.next_offset, Some(100));
        // The last window runs to the end, whatever it holds.
        let w = window(&text, 200, 100);
        assert_eq!(w.text, "b".repeat(51));
        assert_eq!(w.next_offset, None);
        // Windows cover the text exactly.
        let text = "line one\nline two\n".repeat(50);
        let mut at = Some(0);
        let mut joined = String::new();
        while let Some(o) = at {
            let w = window(&text, o, 64);
            joined.push_str(&w.text);
            at = w.next_offset;
        }
        assert_eq!(joined, text);
    }

    #[test]
    fn windows_count_chars_not_bytes() {
        let text = "é".repeat(250);
        let w = window(&text, 100, 100);
        assert_eq!(w.text, "é".repeat(100));
        assert_eq!(w.offset, 100);
        assert_eq!(w.next_offset, Some(200));
        assert_eq!(w.total_chars, 250);
    }

    #[test]
    fn an_offset_past_the_end_is_an_empty_window() {
        for offset in [10, 11, 1_000_000] {
            let w = window("0123456789", offset, PAGE_CHARS);
            assert_eq!(
                w,
                Window {
                    text: String::new(),
                    offset: 10,
                    next_offset: None,
                    total_chars: 10,
                }
            );
        }
        assert_eq!(window("", 0, PAGE_CHARS).next_offset, None);
    }

    #[test]
    fn decodes_the_declared_charset() {
        assert_eq!(decode(b"caf\xe9", "text/html; charset=iso-8859-1"), "café");
        assert_eq!(
            decode(b"caf\xe9", "text/html; Charset=\"ISO-8859-1\""),
            "café"
        );
        assert_eq!(decode("café".as_bytes(), "text/html"), "café");
        assert_eq!(decode(b"caf\xe9", "text/plain"), "caf\u{fffd}");
        assert_eq!(
            decode(b"caf\xe9", "text/plain; charset=nonsense"),
            "caf\u{fffd}"
        );
        assert_eq!(decode(b"\xef\xbb\xbfhi", "text/plain"), "hi");
    }

    #[test]
    fn json_is_pretty_printed() {
        assert_eq!(
            pretty_json(r#"{"a":[1,2]}"#).as_deref(),
            Some("{\n  \"a\": [\n    1,\n    2\n  ]\n}")
        );
        // Keys keep their order, numbers and strings their text.
        let given =
            r#" {"b": 1e400, "a" : {}, "s": "x, \"y\": [z]", "n": 123456789012345678901234} "#;
        let want = "{\n  \"b\": 1e400,\n  \"a\": {},\n  \"s\": \"x, \\\"y\\\": [z]\",\n  \
                    \"n\": 123456789012345678901234\n}";
        assert_eq!(pretty_json(given).as_deref(), Some(want));
        assert_eq!(pretty_json("<html>"), None);
        assert_eq!(pretty_json("[1,"), None);
    }

    #[test]
    fn links_are_absolute_and_normalised() {
        let b = base("https://x.com/a");
        let html = r##"<a href="/b#c">b</a> <a href="/b">again</a>
            <a href="HTTPS://Y.com:443/z">y</a> <a href="mailto:m@x.com">m</a>
            <a href="http://127.0.0.1/">local</a> <area href="d">
            <script>var s = '<a href="/in-script">';</script>"##;
        assert_eq!(
            links_of_html(html, &b),
            vec!["https://x.com/b", "https://y.com/z", "https://x.com/d"]
        );
        let based = r#"<base href="https://cdn.x.com/dir/"><a href="p">p</a>"#;
        assert_eq!(links_of_html(based, &b), vec!["https://cdn.x.com/dir/p"]);
        // The first `<base href>` applies to the whole document.
        let late = r#"<a href="p">p</a><base href="https://cdn.x.com/dir/"><base href="/no/">"#;
        assert_eq!(links_of_html(late, &b), vec!["https://cdn.x.com/dir/p"]);
        // `<script/>` still opens a script.
        let script = r#"<script/><a href="/in-script"></script><a href="/out">"#;
        assert_eq!(links_of_html(script, &b), vec!["https://x.com/out"]);
        let md = "See [b](/b#c \"t\"), ![i](<img.png>), <https://y.com/q>.\n\
                  Bare https://z.com/r, and [ref]: /s\n[w]: https://w.com/";
        assert_eq!(
            links_of_markdown(md, &b),
            vec![
                "https://x.com/b",
                "https://x.com/img.png",
                "https://y.com/q",
                "https://z.com/r",
                "https://w.com/",
            ]
        );
        let many: String = (0..1_500).map(|i| format!("<a href=\"/{i}\">")).collect();
        assert_eq!(links_of_html(&many, &b).len(), MAX_LINKS);
    }

    #[test]
    fn hostile_pages_are_scanned_in_one_pass() {
        let b = base("https://x.com/a");
        // Half a million unclosed `](`: one target, read to the end.
        assert!(links_of_markdown(&"](".repeat(500_000), &b).is_empty());
        assert!(links_of_markdown(&"](<".repeat(300_000), &b).is_empty());
        assert!(links_of_markdown(&"<".repeat(1_000_000), &b).is_empty());
        // Bare-URL-like text with no separator is one candidate, too long.
        assert!(links_of_markdown(&"https://x".repeat(100_000), &b).is_empty());
        // Many real links and spans: the first MAX_LINKS are kept.
        let many: String = (0..50_000)
            .map(|i| {
                format!(
                    "[l](/{i}) <https://y.com/{i}> https://z.com/{i}
"
                )
            })
            .collect();
        let links = links_of_markdown(&many, &b);
        assert_eq!(links.len(), MAX_LINKS);
        assert_eq!(
            &links[..3],
            ["https://x.com/0", "https://y.com/0", "https://z.com/0"]
        );
        // Targets past MAX_CANDIDATES are not read, kept or not: a valid
        // link after a run of refused and repeated ones is dropped.
        let mut md = String::from("[k](/kept)\n");
        for i in 0..MAX_CANDIDATES {
            md.push_str(if i % 2 == 0 {
                "[m](mailto:a@x.com) "
            } else {
                "[r](/kept) "
            });
        }
        md.push_str("[late](/late)\n");
        assert_eq!(links_of_markdown(&md, &b), vec!["https://x.com/kept"]);
        let html = format!(
            r#"<a href="/kept">{}<a href="/late">"#,
            r#"<a href="javascript:void(0)">"#.repeat(MAX_CANDIDATES)
        );
        assert_eq!(links_of_html(&html, &b), vec!["https://x.com/kept"]);
        // The same page with one target fewer keeps the late link.
        let html = format!(
            r#"<a href="/kept">{}<a href="/late">"#,
            r#"<a href="javascript:void(0)">"#.repeat(MAX_CANDIDATES - 2)
        );
        assert_eq!(
            links_of_html(&html, &b),
            vec!["https://x.com/kept", "https://x.com/late"]
        );
        // A long `<base href>` is resolved once, not once per link.
        let html = format!(
            r#"<base href="/{}">{}"#,
            "a".repeat(5_000),
            r#"<a href="b">"#.repeat(100_000)
        );
        assert_eq!(links_of_html(&html, &b), vec!["https://x.com/b"]);
    }

    #[test]
    fn a_snippet_cuts_at_a_word() {
        assert_eq!(snippet("short text", 600), "short text");
        assert_eq!(snippet("alpha beta gamma", 12), "alpha beta…");
        assert_eq!(snippet("alpha beta gamma", 11), "alpha beta…");
        assert_eq!(snippet("alpha beta gamma", 10), "alpha…");
        assert_eq!(snippet("abcdefghij", 5), "abcd…");
        assert_eq!(snippet("abcdefghij", 1), "…");
        assert_eq!(snippet("abcdefghij", 0), "");
        assert_eq!(snippet("", 0), "");
        let long = "word ".repeat(200);
        let s = snippet(&long, SNIPPET_CHARS);
        assert!(s.chars().count() <= SNIPPET_CHARS);
        assert!(s.ends_with("word…"));
    }
}
