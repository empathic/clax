//! Marking a draft's changed elements on the server (spec
//! 2026-10-11-clax-dev-hosted-design §16.4), so a draft page carries no
//! text, and no hash of text, from the version it is based on.
//!
//! Both files are read with one small HTML tokenizer that keeps a stack of
//! open elements (void elements, raw-text elements, and the end tags HTML
//! lets a page leave out for `p`, `li`, `dt`/`dd`, `option`, `tr` and
//! `td`/`th`). An element's own text is its direct text, entities decoded,
//! whitespace runs collapsed and the ends trimmed. A draft element is
//! changed when its own text is not empty and is not the own text of any
//! element of the base, so an element that only moved is not marked.
//! Changed elements get the [`CHANGED_ATTR`] attribute in their start tag,
//! which the tint script ([`crate::drafts::tint_script`]) styles. Text a
//! script writes into the page after it loads is not compared.
//!
//! The work is linear in the input: the open stack is at most
//! [`MAX_DEPTH`] deep (deeper start tags are not opened), an end tag for a
//! name with no open element is skipped by a per-name count, the searches
//! for an end tag HTML lets a page leave out look at most [`SCOPE_SCAN`]
//! elements down, and output is built in one pass. Past [`MAX_MARKS`]
//! changed elements nothing is marked and only the count is reported.

use std::collections::{HashMap, HashSet};

/// The attribute marking a changed element in a served draft.
pub const CHANGED_ATTR: &str = "data-clax-draft-changed";
/// Deepest open-element stack; start tags past it are read as text holders
/// of the deepest element.
pub const MAX_DEPTH: usize = 512;
/// Most elements marked; past it, none are and the count is reported.
pub const MAX_MARKS: usize = 5000;
/// Most open elements an implied end tag's search looks through.
pub const SCOPE_SCAN: usize = 16;

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
/// Elements whose content is text up to their own end tag (`plaintext`: to
/// the end of the file).
const RAW: &[&str] = &[
    "script",
    "style",
    "textarea",
    "title",
    "xmp",
    "noscript",
    "template",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
];
/// Raw-text elements whose text is not page text.
const UNSHOWN: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "noembed", "noframes",
];
/// Start tags that close an open `p`.
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
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
    "hgroup",
    "hr",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "ul",
];
const P_SCOPE: &[&str] = &[
    "div",
    "section",
    "article",
    "td",
    "th",
    "li",
    "blockquote",
    "body",
];
const LIST_SCOPE: &[&str] = &["ul", "ol", "menu"];
const DL_SCOPE: &[&str] = &["dl"];
const DT_DD: &[&str] = &["dt", "dd"];
const SELECT_SCOPE: &[&str] = &["select", "datalist"];
const TABLE_SCOPE: &[&str] = &["table", "tbody", "thead", "tfoot"];
const ROW_SCOPE: &[&str] = &["tr", "table"];
const CELLS: &[&str] = &["td", "th"];

/// A count of the tokenizer's steps (bytes, tags and stack moves), for
/// bounding its work in tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub ops: u64,
}

struct Open {
    name: String,
    text: String,
    /// Where an attribute goes in the start tag (just before `>` or `/>`).
    insert_at: usize,
}

/// The tokenizer's state over one file.
struct Reader<'a> {
    stack: Vec<Open>,
    open_count: HashMap<String, usize>,
    out: Vec<(String, usize)>,
    stats: &'a mut DiffStats,
}

impl Reader<'_> {
    fn push(&mut self, o: Open) {
        *self.open_count.entry(o.name.clone()).or_default() += 1;
        self.stack.push(o);
    }

    fn pop(&mut self) {
        let Some(o) = self.stack.pop() else { return };
        if let Some(c) = self.open_count.get_mut(&o.name) {
            *c -= 1;
        }
        self.stats.ops += 1;
        let t = collapse(&o.text);
        if !t.is_empty() && !UNSHOWN.contains(&o.name.as_str()) {
            self.out.push((t, o.insert_at));
        }
    }

    fn pop_to(&mut self, i: usize) {
        while self.stack.len() > i {
            self.pop();
        }
    }

    /// The deepest open element named in `names` within `limit` of the top
    /// and above any of `stop_at`.
    fn find(&mut self, names: &[&str], stop_at: &[&str], limit: usize) -> Option<usize> {
        let has = names
            .iter()
            .any(|n| self.open_count.get(*n).copied().unwrap_or(0) > 0);
        if !has {
            return None;
        }
        for (seen, i) in (0..self.stack.len()).rev().enumerate() {
            if seen >= limit {
                break;
            }
            self.stats.ops += 1;
            let n = self.stack[i].name.as_str();
            if names.contains(&n) {
                return Some(i);
            }
            if stop_at.contains(&n) {
                return None;
            }
        }
        None
    }

    fn text(&mut self, s: &str) {
        self.stats.ops += s.len() as u64;
        if let Some(top) = self.stack.last_mut() {
            top.text.push_str(&decode_entities(s));
        }
    }
}

/// `s` with whitespace runs collapsed to one space and the ends trimmed.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// The first `needle` (ASCII, lowercase) in `h` at or after `from`,
/// compared ASCII case-blind.
fn find_ci(h: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > h.len() || needle.len() > h.len() - from {
        return None;
    }
    (from..=h.len() - needle.len())
        .find(|&at| h[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

/// The first `b` in `h` at or after `from`.
fn find_byte(h: &[u8], from: usize, b: u8) -> Option<usize> {
    h.get(from..)?
        .iter()
        .position(|&c| c == b)
        .map(|p| from + p)
}

/// The first `needle` in `h` at or after `from`.
fn find_bytes(h: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > h.len() || needle.len() > h.len() - from {
        return None;
    }
    (from..=h.len() - needle.len()).find(|&at| &h[at..at + needle.len()] == needle)
}

/// Each element's own text (non-empty only) and the byte offset its start
/// tag takes an attribute at.
fn own_texts(html: &str, stats: &mut DiffStats) -> Vec<(String, usize)> {
    let h = html.as_bytes();
    let n = h.len();
    let mut r = Reader {
        stack: Vec::new(),
        open_count: HashMap::new(),
        out: Vec::new(),
        stats,
    };
    let mut i = 0;
    while i < n {
        r.stats.ops += 1;
        let Some(lt) = find_byte(h, i, b'<') else {
            r.text(&html[i..]);
            break;
        };
        if lt > i {
            r.text(&html[i..lt]);
        }
        if h[lt..].starts_with(b"<!--") {
            // `<!-->` and `<!--->` are empty comments.
            i = if h[lt..].starts_with(b"<!-->") {
                lt + 5
            } else if h[lt..].starts_with(b"<!--->") {
                lt + 6
            } else {
                find_bytes(h, lt + 4, b"-->").map_or(n, |e| e + 3)
            };
            r.stats.ops += (i - lt) as u64;
            continue;
        }
        if matches!(h.get(lt + 1), Some(b'!' | b'?')) {
            i = find_byte(h, lt, b'>').map_or(n, |e| e + 1);
            r.stats.ops += (i - lt) as u64;
            continue;
        }
        let end_tag = h.get(lt + 1) == Some(&b'/');
        let name_start = lt + if end_tag { 2 } else { 1 };
        if !h.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
            r.text("<");
            i = lt + 1;
            continue;
        }
        let mut j = name_start + 1;
        while j < n && (h[j].is_ascii_alphanumeric() || h[j] == b'-') {
            j += 1;
        }
        let name = html[name_start..j].to_ascii_lowercase();
        // The tag ends at the first `>` outside an attribute value quoted
        // right after `=`.
        let mut quote = 0u8;
        let mut after_eq = false;
        while j < n {
            r.stats.ops += 1;
            let c = h[j];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
                j += 1;
                continue;
            }
            if c == b'>' {
                break;
            }
            if after_eq && (c == b'"' || c == b'\'') {
                quote = c;
                after_eq = false;
            } else if c == b'=' {
                after_eq = true;
            } else if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c) {
                after_eq = false;
            }
            j += 1;
        }
        let tag_end = j.min(n);
        i = tag_end + 1;
        if end_tag {
            if let Some(at) = r.find(&[name.as_str()], &[], usize::MAX) {
                r.pop_to(at);
            }
            continue;
        }
        if CLOSES_P.contains(&name.as_str())
            && let Some(p) = r.find(&["p"], P_SCOPE, SCOPE_SCAN)
        {
            r.pop_to(p);
        }
        let implied = match name.as_str() {
            "li" => r.find(&["li"], LIST_SCOPE, SCOPE_SCAN),
            "dt" | "dd" => r.find(DT_DD, DL_SCOPE, SCOPE_SCAN),
            "option" => r.find(&["option"], SELECT_SCOPE, SCOPE_SCAN),
            "tr" => r.find(&["tr"], TABLE_SCOPE, SCOPE_SCAN),
            "td" | "th" => r.find(CELLS, ROW_SCOPE, SCOPE_SCAN),
            _ => None,
        };
        if let Some(at) = implied {
            r.pop_to(at);
        }
        let insert_at = if tag_end > 0 && h.get(tag_end - 1) == Some(&b'/') {
            tag_end - 1
        } else {
            tag_end
        };
        if VOID.contains(&name.as_str()) {
            continue;
        }
        let raw = RAW.contains(&name.as_str());
        let mut open = Open {
            name,
            text: String::new(),
            insert_at,
        };
        if raw {
            let from = i.min(n);
            let (end, after) = if open.name == "plaintext" {
                (n, n)
            } else {
                let needle = format!("</{}", open.name);
                match find_ci(h, from, needle.as_bytes()) {
                    Some(at) => (at, find_byte(h, at, b'>').map_or(n, |gt| gt + 1)),
                    None => (n, n),
                }
            };
            r.stats.ops += (after - from) as u64;
            if !UNSHOWN.contains(&open.name.as_str()) {
                open.text = decode_entities(&html[from..end]);
            }
            if r.stack.len() < MAX_DEPTH {
                r.push(open);
                r.pop();
            }
            i = after;
            continue;
        }
        if r.stack.len() < MAX_DEPTH {
            r.push(open);
        }
    }
    r.pop_to(0);
    r.out
}

/// The named entities decoded; any other is left as written.
fn named_entity(name: &str) -> Option<char> {
    match name.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => None,
    }
}

/// The entity at the start of `s` (which starts with `&`): its length and
/// what it decodes to, `None` for text that is not an entity reference of
/// the forms `&#x<1–8 hex>;`, `&#<1–8 digits>;` or `&<1–8 letters>;`. A
/// reference that names no known character decodes to itself.
fn entity_at(s: &[u8]) -> Option<(usize, Option<char>)> {
    let run = |from: usize, ok: fn(&u8) -> bool| {
        let k = s
            .get(from..)
            .map_or(0, |t| t.iter().take(9).take_while(|c| ok(c)).count());
        (1..=8).contains(&k).then_some(k)
    };
    let semi = |at: usize| s.get(at) == Some(&b';');
    if s.get(1) == Some(&b'#') {
        if matches!(s.get(2), Some(b'x' | b'X'))
            && let Some(k) = run(3, u8::is_ascii_hexdigit)
            && semi(3 + k)
        {
            let digits = std::str::from_utf8(&s[3..3 + k]).ok()?;
            let ch = u32::from_str_radix(digits, 16)
                .ok()
                .filter(|&cp| cp > 0)
                .and_then(char::from_u32);
            return Some((4 + k, ch));
        }
        if let Some(k) = run(2, u8::is_ascii_digit)
            && semi(2 + k)
        {
            let digits = std::str::from_utf8(&s[2..2 + k]).ok()?;
            let ch = digits
                .parse::<u32>()
                .ok()
                .filter(|&cp| cp > 0)
                .and_then(char::from_u32);
            return Some((3 + k, ch));
        }
        return None;
    }
    let k = run(1, u8::is_ascii_alphabetic)?;
    if !semi(1 + k) {
        return None;
    }
    let name = std::str::from_utf8(&s[1..1 + k]).ok()?;
    Some((2 + k, named_entity(name)))
}

/// `s` with its character references decoded (see [`entity_at`]).
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut from = 0;
    let mut i = 0;
    while let Some(amp) = find_byte(b, i, b'&') {
        match entity_at(&b[amp..]) {
            Some((len, Some(ch))) => {
                out.push_str(&s[from..amp]);
                out.push(ch);
                from = amp + len;
                i = from;
            }
            Some((len, None)) => i = amp + len,
            None => i = amp + 1,
        }
    }
    out.push_str(&s[from..]);
    out
}

/// What [`mark_changed`] answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marked {
    /// The draft with its changed elements marked, or as sent when not `marked`.
    pub html: String,
    /// How many elements of the draft changed.
    pub changed: usize,
    /// Whether they are marked: false past [`MAX_MARKS`].
    pub marked: bool,
}

/// `draft` with [`CHANGED_ATTR`] on every element whose own text is not the
/// own text of any element of `base`; with `base` `None` (a new file, or a
/// base file withheld from the drafting agent), on every element with text.
pub fn mark_changed(draft: &str, base: Option<&str>) -> Marked {
    mark_changed_counting(draft, base, &mut DiffStats::default())
}

/// [`mark_changed`], counting its steps in `stats`.
pub fn mark_changed_counting(draft: &str, base: Option<&str>, stats: &mut DiffStats) -> Marked {
    let known: HashSet<String> = match base {
        Some(b) => own_texts(b, stats).into_iter().map(|(t, _)| t).collect(),
        None => HashSet::new(),
    };
    let mut marks: Vec<usize> = own_texts(draft, stats)
        .into_iter()
        .filter(|(t, _)| !known.contains(t))
        .map(|(_, at)| at)
        .collect();
    stats.ops += marks.len() as u64;
    if marks.len() > MAX_MARKS {
        return Marked {
            html: draft.to_string(),
            changed: marks.len(),
            marked: false,
        };
    }
    marks.sort_unstable();
    let attr = format!(" {CHANGED_ATTR}");
    let mut html = String::with_capacity(draft.len() + marks.len() * attr.len());
    let mut from = 0;
    for &at in &marks {
        html.push_str(&draft[from..at]);
        html.push_str(&attr);
        from = at;
    }
    html.push_str(&draft[from..]);
    Marked {
        html,
        changed: marks.len(),
        marked: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(draft: &str, base: Option<&str>) -> Marked {
        mark_changed(draft, base)
    }

    #[test]
    fn marks_elements_whose_own_text_is_new_and_not_ones_that_only_moved() {
        let base = "<body><h1>Plan</h1><p>One</p><p>Two</p></body>";
        let r = marked(
            "<body><h1>Plan</h1><p>Two</p><p>Three</p><p>One</p></body>",
            Some(base),
        );
        assert_eq!(
            r.html,
            "<body><h1>Plan</h1><p>Two</p><p data-clax-draft-changed>Three</p><p>One</p></body>"
        );
        assert_eq!(r.changed, 1);
    }

    #[test]
    fn compares_own_text_only_so_a_parent_of_a_changed_child_is_not_marked() {
        let r = marked(
            r#"<ul class="x"><li><b>Lead</b> plans</li></ul>"#,
            Some(r#"<ul class="x"><li><b>Lead</b> runs</li></ul>"#),
        );
        assert_eq!(
            r.html,
            r#"<ul class="x"><li data-clax-draft-changed><b>Lead</b> plans</li></ul>"#
        );
    }

    #[test]
    fn handles_void_and_self_closed_tags_quoted_gt_omitted_end_tags_entities_comments_and_scripts()
    {
        let base = r#"<p>a &amp; b<p>kept<br/><img alt="x > y"><li>one<li>two<!-- <p>gone</p> --><script>var s="<p>no</p>"</script>"#;
        let draft = r#"<p>a &amp; b<p>kept<br/><img alt="x > y"><li>one<li>three<!-- <p>gone</p> --><script>var s="<p>new</p>"</script>"#;
        assert_eq!(
            marked(draft, Some(base)).html,
            r#"<p>a &amp; b<p>kept<br/><img alt="x > y"><li>one<li data-clax-draft-changed>three<!-- <p>gone</p> --><script>var s="<p>new</p>"</script>"#
        );
    }

    #[test]
    fn marks_every_element_with_text_when_there_is_no_base_to_compare_with() {
        assert_eq!(
            marked("<h1>A</h1><div><p>B</p></div>", None).html,
            "<h1 data-clax-draft-changed>A</h1><div><p data-clax-draft-changed>B</p></div>"
        );
    }

    #[test]
    fn reads_script_text_up_to_its_end_tag_so_a_quoted_end_p_in_it_is_not_markup() {
        let base = r#"<p>keep</p><script>var s = "</p><p>fake";</script><p>last</p>"#;
        assert_eq!(
            marked(
                r#"<p>keep</p><script>var s = "</p><p>other";</script><p>new</p>"#,
                Some(base)
            )
            .html,
            r#"<p>keep</p><script>var s = "</p><p>other";</script><p data-clax-draft-changed>new</p>"#
        );
    }

    #[test]
    fn ends_a_tag_at_gt_inside_no_quote_taking_quotes_only_right_after_eq() {
        assert_eq!(
            marked(r#"<p data-x="a > b">one</p>"#, None).html,
            r#"<p data-x="a > b" data-clax-draft-changed>one</p>"#
        );
        assert_eq!(
            marked("<p data-x = 'a>b'>two</p>", None).html,
            "<p data-x = 'a>b' data-clax-draft-changed>two</p>"
        );
        // A quote that does not start a value is part of the name or value,
        // so the tag ends at the first >.
        assert_eq!(
            marked(r#"<p title=a"b>three</p>"#, None).html,
            r#"<p title=a"b data-clax-draft-changed>three</p>"#
        );
        assert_eq!(
            marked(r#"<p a"b>four</p>"#, None).html,
            r#"<p a"b data-clax-draft-changed>four</p>"#
        );
    }

    #[test]
    fn reads_empty_comment_forms_as_empty_comments() {
        assert_eq!(
            marked("<!--><p>a</p><!---><p>b</p>", None).html,
            "<!--><p data-clax-draft-changed>a</p><!---><p data-clax-draft-changed>b</p>"
        );
    }

    #[test]
    fn treats_iframe_noembed_and_noframes_as_unshown_raw_text_and_plaintext_as_text_to_the_end() {
        assert_eq!(
            marked(
                "<iframe><p>x</p></iframe><noembed><p>y</p></noembed><noframes><p>z</p></noframes><p>w</p>",
                None
            )
            .html,
            "<iframe><p>x</p></iframe><noembed><p>y</p></noembed><noframes><p>z</p></noframes><p data-clax-draft-changed>w</p>"
        );
        assert_eq!(
            marked("<plaintext><p>all text</p>", None).html,
            "<plaintext data-clax-draft-changed><p>all text</p>"
        );
    }

    #[test]
    fn decodes_entities_before_comparing_and_keeps_multibyte_text_whole() {
        // The same text written with and without references is not a change.
        let r = marked(
            "<p>caf&#xe9; &lt;b&gt; &nbsp;x</p><p>naïve – ok</p>",
            Some("<p>café <b>\u{a0}x</p><p>naïve – ok</p>"),
        );
        assert!(r.marked);
        assert_eq!(r.changed, 1, "{}", r.html);
        // `<b>` in the base is a tag, so only the base's p reads `café`.
        let same = marked("<p>caf&eacute;</p>", Some("<p>caf&eacute;</p>"));
        assert_eq!(same.changed, 0);
        assert_eq!(
            marked(
                "<p>&bogus; &#0; &#x110000;</p>",
                Some("<p>&bogus; &#0; &#x110000;</p>")
            )
            .changed,
            0
        );
        assert_eq!(
            marked("<h1>Ünïcödé</h1>", None).html,
            "<h1 data-clax-draft-changed>Ünïcödé</h1>"
        );
    }

    #[test]
    fn an_unterminated_tag_or_raw_element_ends_the_file_without_a_panic() {
        assert_eq!(marked("<p>a</p><div class=\"x", None).changed, 1);
        assert_eq!(marked("<p>a<script>never closed", None).changed, 1);
        assert_eq!(marked("<p>a</p><script", None).changed, 1);
        assert_eq!(
            marked("<p>tail/", None).html,
            "<p data-clax-draft-changed>tail/"
        );
        assert_eq!(marked("a < b <3", None).changed, 0);
    }

    const MIB: usize = 1024 * 1024;

    /// Steps taken for `draft` over `base`, which must stay within 8 steps
    /// per input byte.
    fn work(draft: &str, base: Option<&str>) -> Marked {
        let mut stats = DiffStats::default();
        let result = mark_changed_counting(draft, base, &mut stats);
        let size = draft.len() + base.map_or(0, str::len);
        assert!(
            stats.ops <= 8 * size as u64 + 1000,
            "{} steps for {size} bytes",
            stats.ops
        );
        result
    }

    #[test]
    fn bounds_many_marks_and_marks_nothing_past_max_marks_but_reports_the_count() {
        let same = "<b>x</b>".repeat(131_072);
        assert_eq!(same.len(), MIB);
        let r = work(&same, None);
        assert_eq!((r.changed, r.marked), (131_072, false));
        assert_eq!(r.html, same);
        let unique: String = (0..87_000).map(|i| format!("<b>{i}</b>")).collect();
        let r = work(&unique, Some(&unique));
        assert_eq!((r.changed, r.marked), (0, true));
        let r = work(&unique, None);
        assert_eq!((r.changed, r.marked), (87_000, false));
    }

    #[test]
    fn bounds_unclosed_elements_past_the_depth_cap_and_end_tags_for_names_never_opened() {
        assert!(work(&"<span>z".repeat(150_000), None).marked);
        let opened = "<a>".repeat(174_000);
        let r = work(
            &format!("{opened}{}", "</x>".repeat(131_000)),
            Some(&opened),
        );
        assert_eq!(r.changed, 0);
    }

    #[test]
    fn bounds_implied_end_tags_searched_under_a_deep_stack() {
        work(
            &format!("<ul><li>{}{}", "<b>".repeat(600), "<li>n".repeat(100_000)),
            None,
        );
        work(
            &format!("<p>{}{}", "<i>".repeat(600), "<div>d".repeat(100_000)),
            None,
        );
    }
}
