//! A fetched page's text: decoding a body by its declared charset,
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
/// against `base` (or the page's own `<base href>`), kept when they are
/// public URLs ([`PublicUrl::parse`]): normalised, each once in the order
/// first seen, at most [`MAX_LINKS`].
#[must_use]
pub fn links_of_html(html: &str, base: &PublicUrl) -> Vec<String> {
    #[derive(Default)]
    struct Hrefs {
        base: RefCell<Option<String>>,
        hrefs: RefCell<Vec<(Option<String>, String)>>,
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
                ("a" | "area" | "link", Some(h)) => {
                    let base = self.base.borrow().clone();
                    self.hrefs.borrow_mut().push((base, h));
                }
                _ => {}
            }
            // Text in these is not markup: an `href` in a script is not a link.
            if tag.self_closing {
                return TokenSinkResult::Continue;
            }
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
    let hrefs = tok.sink.hrefs.into_inner();
    let mut links = Links::new();
    for (page_base, href) in hrefs {
        let resolved_base = page_base
            .and_then(|b| base.as_url().join(b.trim()).ok())
            .unwrap_or_else(|| base.as_url().clone());
        if !links.add_from(&resolved_base, &href) {
            break;
        }
    }
    links.into_vec()
}

/// The links of Markdown `md`: inline links and images (`[t](u "title")`),
/// reference definitions (`[r]: u`), autolinks (`<u>`) and bare `http`
/// and `https` URLs, resolved against `base` and kept as
/// [`links_of_html`] keeps them.
#[must_use]
pub fn links_of_markdown(md: &str, base: &PublicUrl) -> Vec<String> {
    let mut found: Vec<(usize, &str)> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    // Inline links and images: `](target)` or `](<target>)`, with an
    // optional title after the target.
    let mut from = 0;
    while let Some(i) = md[from..].find("](") {
        let start = from + i + 2;
        from = start;
        let rest = &md[start..];
        let (target, len) = if let Some(r) = rest.strip_prefix('<') {
            match r.find(['>', '\n']) {
                Some(j) if r[j..].starts_with('>') => (&r[..j], j + 2),
                _ => continue,
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
    }
    // Autolinks: `<scheme://…>`.
    let mut from = 0;
    while let Some(i) = md[from..].find('<') {
        let start = from + i + 1;
        from = start;
        let rest = &md[start..];
        if let Some(j) = rest.find(['>', '<', ' ', '\n'])
            && rest[j..].starts_with('>')
            && rest[..j].contains("://")
        {
            found.push((start, &rest[..j]));
            spans.push((start, start + j));
        }
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
    // Bare URLs, outside the links above.
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(i) = md[from..].find(scheme) {
            let start = from + i;
            let rest = &md[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "<>()[]\"'`".contains(c))
                .unwrap_or(rest.len());
            from = start + end.max(scheme.len());
            let inside = spans.iter().any(|&(s, e)| s <= start && start < e);
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if !inside && url.len() > scheme.len() {
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
/// most [`MAX_LINKS`].
struct Links {
    seen: HashSet<String>,
    links: Vec<String>,
}

impl Links {
    fn new() -> Links {
        Links {
            seen: HashSet::new(),
            links: Vec::new(),
        }
    }

    /// Adds `href`, resolved against `base`, when it is a public URL;
    /// false once [`MAX_LINKS`] are kept.
    fn add_from(&mut self, base: &::url::Url, href: &str) -> bool {
        if self.links.len() >= MAX_LINKS {
            return false;
        }
        if let Ok(abs) = base.join(href.trim())
            && let Ok(u) = PublicUrl::parse(abs.as_str())
            && self.seen.insert(u.normalized().to_string())
        {
            self.links.push(u.normalized().to_string());
        }
        self.links.len() < MAX_LINKS
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
/// (which counts) when cut; `s` whole when it fits.
#[must_use]
pub fn snippet(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_snippet_cuts_at_a_word() {
        assert_eq!(snippet("short text", 600), "short text");
        assert_eq!(snippet("alpha beta gamma", 12), "alpha beta…");
        assert_eq!(snippet("alpha beta gamma", 11), "alpha beta…");
        assert_eq!(snippet("alpha beta gamma", 10), "alpha…");
        assert_eq!(snippet("abcdefghij", 5), "abcd…");
        let long = "word ".repeat(200);
        let s = snippet(&long, SNIPPET_CHARS);
        assert!(s.chars().count() <= SNIPPET_CHARS);
        assert!(s.ends_with("word…"));
    }
}
