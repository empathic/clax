//! An app's launcher icon: the `icon.svg` among a version's files, a
//! square full-bleed design in a small, safe subset of SVG.
//!
//! [`problem`] is the rule every publish runs, from every client. The app's
//! agent checks the same rules early (`apps/clax_app/lib/agent/icon_svg.dart`,
//! `iconProblem`), with the same codes and sentences; the vectors in
//! `tests/fixtures/icons` (each file's answer in `expect.json`) are run by
//! both, so the two cannot drift.

/// The path of an app's icon among its files.
pub const ICON_PATH: &str = "icon.svg";

/// The most bytes of an icon.
pub const MAX_ICON_BYTES: usize = 32 * 1024;

/// The elements an icon may use: shapes, paths, groups, gradients, clips,
/// masks, references within the file, and text for a single glyph.
pub const ICON_ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
    "use",
    "symbol",
    "text",
    "tspan",
];

/// The most `<use>` copies an icon may draw, every expansion counted.
pub const MAX_ICON_USES: usize = 64;

/// The deepest a `<use>` may reach through other `<use>`s.
pub const MAX_ICON_USE_DEPTH: usize = 3;

/// The most levels of elements an icon may nest, its root `<svg>` the
/// first: far below what a recursive walk of the tree (the app's drawing
/// and cleaning among them) can take.
pub const MAX_ICON_DEPTH: usize = 64;

/// Why an `icon.svg` is not a usable icon. [`IconProblem::code`] names the
/// rule; the `Display` form is the sentence the agent acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconProblem {
    /// More than [`MAX_ICON_BYTES`]: the file's size.
    TooLarge(usize),
    /// Not well-formed XML: the parser's account of why.
    NotXml(String),
    /// A `<!DOCTYPE>`.
    Doctype,
    /// Elements nested more than [`MAX_ICON_DEPTH`] levels deep.
    Nesting,
    /// A root element other than `<svg>`: its name.
    NotSvg(String),
    /// No `viewBox` on the root.
    ViewBoxMissing,
    /// A `viewBox` that is not four numbers: its value.
    ViewBoxNotNumbers(String),
    /// A `viewBox` that is not square: its value.
    ViewBoxNotSquare(String),
    /// An element outside [`ICON_ELEMENTS`]: its name.
    Element(String),
    /// An `on*` attribute: its name.
    EventAttribute(String),
    /// An `href` that is not a `#` reference within the file: its value.
    ExternalHref(String),
    /// A `filter` attribute.
    Filter,
    /// A `url()` that is not `url(#…)`: the attribute's name and value.
    ExternalUrl { name: String, value: String },
    /// A `style` with animation, filters or at-rules: its value.
    Style(String),
    /// `<use>`s that expand to more than [`MAX_ICON_USES`] copies.
    UseTooMany,
    /// `<use>`s that nest deeper than [`MAX_ICON_USE_DEPTH`].
    UseTooDeep,
}

impl IconProblem {
    /// The rule broken, as the API's `invalid_icon` error names it.
    pub fn code(&self) -> &'static str {
        match self {
            IconProblem::TooLarge(_) => "too_large",
            IconProblem::NotXml(_) => "not_xml",
            IconProblem::Doctype => "doctype",
            IconProblem::Nesting => "nesting",
            IconProblem::NotSvg(_) => "not_svg",
            IconProblem::ViewBoxMissing
            | IconProblem::ViewBoxNotNumbers(_)
            | IconProblem::ViewBoxNotSquare(_) => "view_box",
            IconProblem::Element(_) => "element",
            IconProblem::EventAttribute(_) => "event_attribute",
            IconProblem::ExternalHref(_) => "external_href",
            IconProblem::Filter => "filter",
            IconProblem::ExternalUrl { .. } => "external_url",
            IconProblem::Style(_) => "style",
            IconProblem::UseTooMany => "use_too_many",
            IconProblem::UseTooDeep => "use_too_deep",
        }
    }
}

impl std::fmt::Display for IconProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconProblem::TooLarge(bytes) => write!(
                f,
                "icon.svg is {bytes} bytes; an icon may be at most 32 KB \
                 ({MAX_ICON_BYTES} bytes): simplify its paths."
            ),
            IconProblem::NotXml(why) => write!(f, "icon.svg is not well-formed XML: {why}."),
            IconProblem::Doctype => write!(f, "icon.svg has a <!DOCTYPE>; leave it out."),
            IconProblem::Nesting => write!(
                f,
                "icon.svg nests its elements more than {MAX_ICON_DEPTH} deep: flatten \
                 its groups."
            ),
            IconProblem::NotSvg(name) => write!(
                f,
                "icon.svg must have <svg> as its root element, not <{name}>."
            ),
            IconProblem::ViewBoxMissing => write!(
                f,
                "icon.svg needs a square viewBox, such as viewBox=\"0 0 1024 1024\"."
            ),
            IconProblem::ViewBoxNotNumbers(v) => {
                write!(f, "icon.svg's viewBox \"{v}\" is not four numbers.")
            }
            IconProblem::ViewBoxNotSquare(v) => write!(
                f,
                "icon.svg's viewBox \"{v}\" is not square: give it equal width \
                 and height, such as \"0 0 1024 1024\"."
            ),
            IconProblem::Element(name) => write!(
                f,
                "icon.svg uses <{name}>, which an icon may not: use only {}. \
                 No scripts, foreignObject, images, filters, styles or animation.",
                ICON_ELEMENTS.join(", ")
            ),
            IconProblem::EventAttribute(name) => {
                write!(f, "icon.svg: the event attribute {name} is not allowed.")
            }
            IconProblem::ExternalHref(v) => write!(
                f,
                "icon.svg: href \"{v}\" points outside the file; only #references \
                 within it are allowed."
            ),
            IconProblem::Filter => write!(f, "icon.svg: filters are not allowed."),
            IconProblem::ExternalUrl { name, value } => write!(
                f,
                "icon.svg: {name}=\"{value}\" points outside the file; only url(#…) \
                 is allowed."
            ),
            IconProblem::Style(v) => write!(
                f,
                "icon.svg: the style \"{v}\" uses animation, filters or at-rules, \
                 which an icon may not."
            ),
            IconProblem::UseTooMany | IconProblem::UseTooDeep => write!(
                f,
                "icon.svg's <use> elements expand to more than {MAX_ICON_USES} \
                 copies, or nest more than {MAX_ICON_USE_DEPTH} deep: draw the \
                 shapes directly instead."
            ),
        }
    }
}

/// Why `svg` is not a usable icon; `None` when it is one. The rules run in
/// this order and the first broken one is named: size, UTF-8, nesting at
/// most [`MAX_ICON_DEPTH`] deep (read from the text, before the parser,
/// which recurses once per level), well-formed XML, no DOCTYPE, an `<svg>`
/// root, a square `viewBox`, then each element (the root's descendants in
/// document order, the root last) by its name and its attributes in source
/// order, then the `<use>` expansion.
pub fn problem(svg: &[u8]) -> Option<IconProblem> {
    if svg.len() > MAX_ICON_BYTES {
        return Some(IconProblem::TooLarge(svg.len()));
    }
    let Ok(text) = std::str::from_utf8(svg) else {
        return Some(IconProblem::NotXml("it is not UTF-8".into()));
    };
    if too_deep(text) {
        return Some(IconProblem::Nesting);
    }
    let doc = match parse(text, false) {
        Ok(doc) => doc,
        Err(roxmltree::Error::DtdDetected) => {
            return Some(match parse(text, true) {
                Err(e) if !named_after_doctype(&e) => IconProblem::NotXml(e.to_string()),
                _ => IconProblem::Doctype,
            });
        }
        Err(e) => return Some(IconProblem::NotXml(e.to_string())),
    };
    let root = doc.root_element();
    let name = root.tag_name().name();
    if name != "svg" {
        return Some(IconProblem::NotSvg(name.into()));
    }
    if let Some(p) = square_view_box(&attributes(text, root)) {
        return Some(p);
    }
    let elements = root.descendants().skip(1).filter(|n| n.is_element());
    for e in elements.chain([root]) {
        let name = e.tag_name().name();
        if !ICON_ELEMENTS.contains(&name) {
            return Some(IconProblem::Element(name.into()));
        }
        if let Some(p) = attributes(text, e).iter().find_map(attribute_problem) {
            return Some(p);
        }
    }
    use_problem(text, root)
}

/// Whether `text`'s start tags nest more than [`MAX_ICON_DEPTH`] deep, by a
/// scan of the text that needs no parse (so it is the same for text that is
/// not well-formed, and the app's `iconProblem` makes it alike): a start
/// tag not closed by `/>` opens a level and an end tag closes one; a
/// comment, `<![CDATA[`, `<?…?>` and `<!…>` open none, and a quoted value
/// in a tag hides its `>`.
fn too_deep(text: &str) -> bool {
    let s = text.as_bytes();
    // Where `end` ends, searching from `from`; the text's end without one.
    let past = |end: &[u8], from: usize| {
        s.get(from..)
            .and_then(|rest| rest.windows(end.len()).position(|w| w == end))
            .map_or(s.len(), |at| from + at + end.len())
    };
    let mut depth = 0usize;
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with(b"<!--") {
            i = past(b"-->", i + 4);
        } else if rest.starts_with(b"<![CDATA[") {
            i = past(b"]]>", i + 9);
        } else if rest.starts_with(b"<?") {
            i = past(b"?>", i + 2);
        } else if rest.starts_with(b"<!") {
            i = past(b">", i + 2);
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = past(b">", i + 2);
        } else if rest[0] == b'<' {
            let mut j = i + 1;
            let mut closed = false;
            while j < s.len() && s[j] != b'>' {
                if s[j] == b'"' || s[j] == b'\'' {
                    j = past(&s[j..=j], j + 1) - 1;
                }
                j += 1;
            }
            if j < s.len() && s[j - 1] == b'/' {
                closed = true;
            }
            if !closed {
                depth += 1;
                if depth > MAX_ICON_DEPTH {
                    return true;
                }
            }
            i = j + 1;
        } else {
            i = s[i..]
                .iter()
                .position(|&c| c == b'<')
                .map_or(s.len(), |at| i + at);
        }
    }
    false
}

fn parse(text: &str, allow_dtd: bool) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    let options = roxmltree::ParsingOptions {
        allow_dtd,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(text, options)
}

/// Whether a document with a DOCTYPE that fails to parse is named for its
/// DOCTYPE rather than as not well-formed: the app's parser reads entity
/// references, namespaces, duplicate attributes, characters and comments
/// loosely and names the DOCTYPE before it checks them, and it expands no
/// entity.
fn named_after_doctype(e: &roxmltree::Error) -> bool {
    use roxmltree::Error as E;
    matches!(
        e,
        E::UnknownEntityReference(..)
            | E::MalformedEntityReference(..)
            | E::EntityReferenceLoop(..)
            | E::UnexpectedEntityCloseTag(..)
            | E::UnknownNamespace(..)
            | E::DuplicatedAttribute(..)
            | E::DuplicatedNamespace(..)
            | E::InvalidAttributeValue(..)
            | E::NonXmlChar(..)
            | E::InvalidComment(..)
            | E::InvalidCharacterData(..)
            | E::UnknownToken(..)
    )
}

/// An attribute as written: its qualified name and its value with the
/// predefined and character references decoded, as the app's parser
/// reads it. Namespace declarations (`xmlns`, `xmlns:…`) are attributes
/// here too.
struct Attr<'a> {
    qname: &'a str,
    value: String,
}

impl Attr<'_> {
    /// The name after the prefix, or the whole name without one.
    fn local(&self) -> &str {
        match self.qname.split_once(':') {
            Some((prefix, local)) if !prefix.is_empty() => local,
            _ => self.qname,
        }
    }
}

/// `node`'s attributes in source order, read from its start tag in the
/// (well-formed, DTD-free) `text`.
fn attributes<'a>(text: &'a str, node: roxmltree::Node<'_, '_>) -> Vec<Attr<'a>> {
    let s = &text[node.range().start..];
    let b = s.as_bytes();
    let space = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n');
    let mut i = 1;
    while !space(b[i]) && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let mut out = Vec::new();
    loop {
        while space(b[i]) {
            i += 1;
        }
        if b[i] == b'/' || b[i] == b'>' {
            return out;
        }
        let start = i;
        while !space(b[i]) && b[i] != b'=' {
            i += 1;
        }
        let qname = &s[start..i];
        while b[i] != b'\'' && b[i] != b'"' {
            i += 1;
        }
        let quote = b[i];
        let open = i + 1;
        i = open;
        while b[i] != quote {
            i += 1;
        }
        out.push(Attr {
            qname,
            value: decode_references(&s[open..i]),
        });
        i += 1;
    }
}

/// `raw` with `&lt;`, `&gt;`, `&amp;`, `&apos;`, `&quot;` and character
/// references replaced by their characters (the parser has checked each).
fn decode_references(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(';').map_or(rest.len(), |e| at + e);
        let entity = &rest[at + 1..end];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "apos" => Some('\''),
            "quot" => Some('"'),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[(end + 1).min(rest.len())..];
            }
            None => {
                out.push('&');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of the attribute written `qname`.
fn attribute<'b>(attrs: &'b [Attr<'_>], qname: &str) -> Option<&'b str> {
    attrs
        .iter()
        .find(|a| a.qname == qname)
        .map(|a| a.value.as_str())
}

/// Whitespace as the app's `String.trim` removes it.
fn trim_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Whitespace as the app's regular expressions match `\s`.
fn pattern_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// A number as SVG writes one: `[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?`.
fn svg_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let digits = |mut i: usize| {
        let from = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        (i, i - from)
    };
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let (after, whole) = digits(i);
    i = after;
    let mut fraction = 0;
    if b.get(i) == Some(&b'.') {
        (i, fraction) = digits(i + 1);
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let (after, exponent) = digits(i);
        if exponent == 0 {
            return None;
        }
        i = after;
    }
    if i != b.len() {
        return None;
    }
    s.parse().ok()
}

fn square_view_box(root: &[Attr<'_>]) -> Option<IconProblem> {
    let Some(value) = attribute(root, "viewBox") else {
        return Some(IconProblem::ViewBoxMissing);
    };
    // Split at each run of whitespace and commas, as `[\s,]+` splits: a
    // leading or trailing comma leaves an empty part.
    let mut parts = vec![String::new()];
    let mut in_run = false;
    for c in value.trim_matches(trim_space).chars() {
        if pattern_space(c) || c == ',' {
            if !in_run {
                parts.push(String::new());
            }
            in_run = true;
        } else {
            parts.last_mut().unwrap().push(c);
            in_run = false;
        }
    }
    let numbers: Vec<f64> = parts.iter().filter_map(|p| svg_number(p)).collect();
    if parts.len() != 4 || numbers.len() != 4 {
        return Some(IconProblem::ViewBoxNotNumbers(value.into()));
    }
    let (w, h) = (numbers[2], numbers[3]);
    if w <= 0.0 || h <= 0.0 || (w - h).abs() > 1e-6 * w {
        return Some(IconProblem::ViewBoxNotSquare(value.into()));
    }
    None
}

fn attribute_problem(a: &Attr<'_>) -> Option<IconProblem> {
    let name = a.local();
    let value = a.value.trim_matches(trim_space);
    if name.to_lowercase().starts_with("on") {
        return Some(IconProblem::EventAttribute(name.into()));
    }
    if name == "href" {
        return (!value.starts_with('#')).then(|| IconProblem::ExternalHref(value.into()));
    }
    if name == "filter" {
        return Some(IconProblem::Filter);
    }
    let lower = value.to_lowercase();
    // Every url() must name an element of the file, as fill="url(#g)".
    let mut from = 0;
    while let Some(at) = lower[from..].find("url(").map(|i| from + i) {
        let Some(close) = lower[at..].find(')').map(|i| at + i) else {
            break;
        };
        let inside = lower[at + 4..close].trim_start_matches(pattern_space);
        if !inside.starts_with('#') {
            return Some(IconProblem::ExternalUrl {
                name: name.into(),
                value: value.into(),
            });
        }
        from = close + 1;
    }
    if name == "style"
        && ["animation", "@", "expression", "filter"]
            .iter()
            .any(|w| lower.contains(w))
    {
        return Some(IconProblem::Style(value.into()));
    }
    None
}

/// Refuses `<use>`s that expand past [`MAX_ICON_USES`] copies or nest
/// deeper than [`MAX_ICON_USE_DEPTH`] (a reference cycle among them).
fn use_problem(text: &str, root: roxmltree::Node<'_, '_>) -> Option<IconProblem> {
    let mut by_id = std::collections::HashMap::new();
    for e in root.descendants().skip(1).filter(|n| n.is_element()) {
        if let Some(id) = attribute(&attributes(text, e), "id") {
            by_id.insert(id.to_owned(), e);
        }
    }
    let mut copies = 0;
    walk(text, root, 0, &by_id, &mut copies).err()
}

/// Counts the copies `e`'s subtree draws, `e` itself among them.
fn walk(
    text: &str,
    e: roxmltree::Node<'_, '_>,
    depth: usize,
    by_id: &std::collections::HashMap<String, roxmltree::Node<'_, '_>>,
    copies: &mut usize,
) -> Result<(), IconProblem> {
    for u in e.descendants().filter(|n| n.is_element()) {
        if u.tag_name().name() != "use" {
            continue;
        }
        if depth >= MAX_ICON_USE_DEPTH {
            return Err(IconProblem::UseTooDeep);
        }
        *copies += 1;
        if *copies > MAX_ICON_USES {
            return Err(IconProblem::UseTooMany);
        }
        let attrs = attributes(text, u);
        let href = attribute(&attrs, "href").or_else(|| attribute(&attrs, "xlink:href"));
        let target = href
            .filter(|h| !h.is_empty())
            .and_then(|h| by_id.get(h.chars().skip(1).collect::<String>().as_str()));
        if let Some(&t) = target {
            walk(text, t, depth + 1, by_id, copies)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    const CODES: &[&str] = &[
        "too_large",
        "not_xml",
        "doctype",
        "nesting",
        "not_svg",
        "view_box",
        "element",
        "event_attribute",
        "external_href",
        "filter",
        "external_url",
        "style",
        "use_too_many",
        "use_too_deep",
    ];

    /// Each vector's answer: its code and sentence, or no problem. A
    /// `not_xml` sentence is compared up to the parser's own account, which
    /// the two parsers word differently.
    #[test]
    fn every_fixture_gets_its_expected_answer() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/icons");
        let expect: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("expect.json")).unwrap()).unwrap();
        let files: BTreeSet<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".svg"))
            .collect();
        assert_eq!(files, expect.keys().cloned().collect::<BTreeSet<_>>());
        let codes: BTreeSet<&str> = expect
            .values()
            .filter_map(|w| w["problem"]["code"].as_str())
            .collect();
        assert_eq!(codes, CODES.iter().copied().collect::<BTreeSet<_>>());
        assert!(expect.keys().filter(|k| k.starts_with("ok-")).count() >= 4);
        let mut wrong = Vec::new();
        for (name, want) in &expect {
            let got = problem(&std::fs::read(dir.join(name)).unwrap());
            let want = &want["problem"];
            let ok = match &got {
                None => want.is_null(),
                Some(p) => {
                    let message = p.to_string();
                    let expected = want["message"].as_str().unwrap_or_default();
                    want["code"] == p.code()
                        && if p.code() == "not_xml" {
                            message.starts_with(expected)
                        } else {
                            message == expected
                        }
                }
            };
            if !ok {
                wrong.push(format!(
                    "{name}: expected {want}, got {:?}",
                    got.map(|p| (p.code(), p.to_string()))
                ));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn bytes_that_are_not_utf8_are_not_xml() {
        let p = problem(b"<svg viewBox=\"0 0 1 1\">\xff</svg>").unwrap();
        assert_eq!(p.code(), "not_xml");
    }
}
