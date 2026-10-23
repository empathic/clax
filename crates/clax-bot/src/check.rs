//! `draft.check()`: static checks of a draft (spec §6.2, §6.6), with no
//! rendering. Every HTML file must tokenize without errors, its inline
//! classic scripts must compile, and the files it references (scripts,
//! stylesheets, images, media, links) must be in the draft; the page
//! (`index.html`) must also follow the page contract (`docs/contract.md`,
//! "Page contract"): a non-empty `<title>`, a viewport `<meta>`, tokens on
//! `:root`, both dark-mode blocks and an explicit `body` background, read
//! from its inline styles and the draft stylesheets it links.

use crate::draft::Draft;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use serde::Serialize;
use std::cell::RefCell;

/// One problem `draft.check` found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    /// `html_parse`, `script_syntax`, `missing_file`, `absolute_path`,
    /// `title`, `viewport`, `tokens`, `dark_mode`, `body_background`.
    pub rule: &'static str,
    pub message: String,
}

/// What `draft.check` reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub ok: bool,
    pub problems: Vec<Problem>,
}

/// The most problems reported for one file and rule.
const MAX_PER_RULE: usize = 10;

/// What one HTML file holds, as the checks need it.
#[derive(Debug, Default)]
struct Scan {
    errors: Vec<(u64, String)>,
    /// `(line, text)` of each inline classic script.
    scripts: Vec<(u64, String)>,
    styles: Vec<String>,
    /// `(line, attribute value)` of each reference.
    refs: Vec<(u64, String)>,
    /// Linked stylesheets' `href`s.
    stylesheets: Vec<String>,
    title: Option<String>,
    viewport: bool,
}

#[derive(Default)]
struct Sink {
    scan: RefCell<Scan>,
    /// The raw-text element open, with the line it started on and its
    /// text so far; `script` only when it is an inline classic script.
    open: RefCell<Option<(String, u64, String, bool)>>,
}

fn attr<'a>(tag: &'a Tag, name: &str) -> Option<&'a str> {
    tag.attrs
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| &*a.value)
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, line: u64) -> TokenSinkResult<()> {
        match token {
            Token::ParseError(e) => self.scan.borrow_mut().errors.push((line, e.to_string())),
            Token::CharacterTokens(t) => {
                if let Some((_, _, text, _)) = self.open.borrow_mut().as_mut() {
                    text.push_str(&t);
                }
            }
            Token::TagToken(tag) => {
                let name = tag.name.to_string();
                if tag.kind == TagKind::EndTag {
                    let open = self.open.borrow_mut().take();
                    if let Some((open_name, start, text, keep)) = open {
                        if open_name != name {
                            *self.open.borrow_mut() = Some((open_name, start, text, keep));
                            return TokenSinkResult::Continue;
                        }
                        let mut scan = self.scan.borrow_mut();
                        match name.as_str() {
                            "script" if keep => scan.scripts.push((start, text)),
                            "style" => scan.styles.push(text),
                            "title" => scan.title = Some(text.trim().to_string()),
                            _ => {}
                        }
                    }
                    return TokenSinkResult::Continue;
                }
                let mut scan = self.scan.borrow_mut();
                for key in ["src", "href"] {
                    if let Some(v) = attr(&tag, key) {
                        let counts = matches!(
                            (name.as_str(), key),
                            (
                                "script" | "img" | "source" | "video" | "audio" | "iframe",
                                "src"
                            ) | ("link" | "a", "href")
                        );
                        if counts {
                            scan.refs.push((line, v.to_string()));
                        }
                    }
                }
                if name == "link"
                    && attr(&tag, "rel").is_some_and(|r| {
                        r.split_ascii_whitespace()
                            .any(|w| w.eq_ignore_ascii_case("stylesheet"))
                    })
                    && let Some(h) = attr(&tag, "href")
                {
                    scan.stylesheets.push(h.to_string());
                }
                if name == "meta"
                    && attr(&tag, "name").is_some_and(|n| n.eq_ignore_ascii_case("viewport"))
                {
                    scan.viewport = true;
                }
                drop(scan);
                if tag.self_closing {
                    return TokenSinkResult::Continue;
                }
                let raw = match name.as_str() {
                    "script" => Some(RawKind::ScriptData),
                    "style" => Some(RawKind::Rawtext),
                    "title" | "textarea" => Some(RawKind::Rcdata),
                    _ => None,
                };
                if let Some(kind) = raw {
                    let classic = name == "script"
                        && attr(&tag, "src").is_none()
                        && attr(&tag, "type").is_none_or(|t| {
                            let t = t.trim().to_ascii_lowercase();
                            t.is_empty() || t == "text/javascript" || t == "application/javascript"
                        });
                    *self.open.borrow_mut() = Some((name, line, String::new(), classic));
                    return TokenSinkResult::RawData(kind);
                }
                return TokenSinkResult::Continue;
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn scan(html: &str) -> Scan {
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tok = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
    let Sink { scan, .. } = tok.sink;
    scan.into_inner()
}

/// The draft path `reference` (in file `from`) names, or why it is not
/// checked: `Ok(None)` for one outside the draft (a URL with a scheme, a
/// fragment, `data:`), `Err` for an absolute path.
fn resolve(from: &str, reference: &str) -> Result<Option<String>, ()> {
    let r = reference.trim();
    let r = r.split(['#', '?']).next().unwrap_or("");
    if r.is_empty() || r.starts_with("//") {
        return Ok(None);
    }
    if let Some(colon) = r.find(':')
        && r[..colon]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return Ok(None);
    }
    if r.starts_with('/') {
        return Err(());
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in r.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    Ok(Some(parts.join("/")))
}

/// The CSS rules of `css` whose selector satisfies `sel`, their bodies.
fn rule_bodies(css: &str, sel: impl Fn(&str) -> bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let selector = rest[..open].rsplit(['}', ';']).next().unwrap_or("").trim();
        let after = &rest[open + 1..];
        let close = after.find('}').unwrap_or(after.len());
        if sel(selector) {
            out.push(&after[..close]);
        }
        rest = &rest[open + 1..];
    }
    out
}

fn without_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..]
            .find("*/")
            .map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    out.push_str(rest);
    out
}

fn contract_problems(path: &str, s: &Scan, css: &str, out: &mut Vec<Problem>) {
    let mut add = |rule: &'static str, message: &str| {
        out.push(Problem {
            path: path.to_string(),
            line: None,
            rule,
            message: message.to_string(),
        });
    };
    if s.title.as_deref().is_none_or(str::is_empty) {
        add(
            "title",
            "the page needs a non-empty <title> (a short name, two to four words)",
        );
    }
    if !s.viewport {
        add(
            "viewport",
            "the page needs <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        );
    }
    let css = without_comments(css);
    let compact: String = css.chars().filter(|c| !c.is_whitespace()).collect();
    let root_tokens = rule_bodies(&css, |s| s.split(',').any(|p| p.trim() == ":root"))
        .iter()
        .any(|b| b.contains("--"));
    if !root_tokens {
        add(
            "tokens",
            "define colors as CSS custom properties (--name: …) on :root",
        );
    }
    let media = compact.contains("prefers-color-scheme:dark");
    let guarded = compact.contains(":root:not([data-theme=\"light\"])")
        || compact.contains(":root:not([data-theme='light'])")
        || compact.contains(":root:not([data-theme=light])");
    let explicit = compact.contains(":root[data-theme=\"dark\"]")
        || compact.contains(":root[data-theme='dark']")
        || compact.contains(":root[data-theme=dark]");
    if !(media && guarded) {
        add(
            "dark_mode",
            "give dark mode under @media (prefers-color-scheme: dark), guarded by :root:not([data-theme=\"light\"])",
        );
    }
    if !explicit {
        add(
            "dark_mode",
            "give dark mode again under :root[data-theme=\"dark\"]",
        );
    }
    let body_bg = rule_bodies(&css, |s| {
        s.split(',')
            .any(|p| matches!(p.trim(), "body" | "html body"))
    })
    .iter()
    .any(|b| b.contains("background"));
    if !body_bg {
        add(
            "body_background",
            "give body an explicit background (and color) from the tokens",
        );
    }
}

/// Checks `draft`. Inline classic scripts are parsed by the sandbox
/// contract's engine-independent check ([`clax_bot_sandbox::check_body`]),
/// as the body a script would be: it runs nothing.
pub fn check(draft: &Draft) -> Report {
    let mut problems = Vec::new();
    let files = draft.files();
    for (path, entry) in files {
        let html = path.ends_with(".html") || path.ends_with(".htm");
        if !html {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&entry.content) else {
            problems.push(Problem {
                path: path.clone(),
                line: None,
                rule: "html_parse",
                message: "the file is not UTF-8".into(),
            });
            continue;
        };
        let s = scan(text);
        for (line, e) in s.errors.iter().take(MAX_PER_RULE) {
            problems.push(Problem {
                path: path.clone(),
                line: Some(*line),
                rule: "html_parse",
                message: e.clone(),
            });
        }
        let mut missing = 0;
        for (line, r) in &s.refs {
            match resolve(path, r) {
                Ok(Some(target)) if !files.contains_key(&target) => {
                    missing += 1;
                    if missing <= MAX_PER_RULE {
                        problems.push(Problem {
                            path: path.clone(),
                            line: Some(*line),
                            rule: "missing_file",
                            message: format!("{r:?} names {target}, which is not in the draft"),
                        });
                    }
                }
                Err(()) => problems.push(Problem {
                    path: path.clone(),
                    line: Some(*line),
                    rule: "absolute_path",
                    message: format!(
                        "{r:?} is an absolute path; reference supporting files by relative path"
                    ),
                }),
                _ => {}
            }
        }
        for (line, code) in &s.scripts {
            if let Err(e) = clax_bot_sandbox::check_body(code) {
                problems.push(Problem {
                    path: path.clone(),
                    line: Some(line + e.line.unwrap_or(1) - 1),
                    rule: "script_syntax",
                    message: e.message,
                });
            }
        }
        if path == clax_core::publish::INDEX {
            let mut css = s.styles.join("\n");
            for href in &s.stylesheets {
                if let Ok(Some(p)) = resolve(path, href)
                    && let Some(e) = files.get(&p)
                {
                    css.push('\n');
                    css.push_str(&String::from_utf8_lossy(&e.content));
                }
            }
            contract_problems(path, &s, &css, &mut problems);
        }
    }
    Report {
        ok: problems.is_empty(),
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::DraftEntry;
    use std::collections::BTreeMap;

    pub const GOOD: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Example Page</title>
<link rel="stylesheet" href="style.css">
<style>
  :root { --bg: #fff; --fg: #111; }
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) { --bg: #111; --fg: #eee; }
  }
</style>
</head>
<body>
<img src="img/logo.png" alt="">
<a href="#top">Top</a> <a href="https://example.com">x</a>
<script>console.log(1 < 2);</script>
<script type="module">import x from "./x.js";</script>
</body>
</html>
"##;

    fn draft(files: &[(&str, &str)]) -> Draft {
        let files: BTreeMap<String, DraftEntry> = files
            .iter()
            .map(|(p, c)| {
                (
                    p.to_string(),
                    DraftEntry {
                        content: c.as_bytes().to_vec(),
                        content_type: clax_core::publish::content_type_for(p),
                    },
                )
            })
            .collect();
        Draft::of(1, files)
    }

    fn rules(r: &Report) -> Vec<&'static str> {
        r.problems.iter().map(|p| p.rule).collect()
    }

    #[tokio::test]
    async fn a_page_that_follows_the_contract_passes() {
        let d = draft(&[
            ("index.html", GOOD),
            (
                "style.css",
                ":root[data-theme=\"dark\"] { --bg: #111; }\nbody { background: var(--bg); color: var(--fg); }",
            ),
            ("img/logo.png", "png"),
        ]);
        let r = check(&d);
        assert_eq!(r.problems, vec![]);
        assert!(r.ok);
    }

    #[tokio::test]
    async fn contract_parse_and_reference_problems_are_named() {
        let d = draft(&[
            (
                "index.html",
                "<html><head></head><body><img src=\"/abs.png\"><script src=\"app.js\"></script><div class=\"a\"\"></div>\n<script>\nlet a = ;\n</script></body></html>",
            ),
            (
                "about.html",
                "<a href=\"index.html\">home</a><link rel=stylesheet href=\"x/missing.css\">",
            ),
        ]);
        let r = check(&d);
        let got = rules(&r);
        for rule in [
            "html_parse",
            "script_syntax",
            "missing_file",
            "absolute_path",
            "title",
            "viewport",
            "tokens",
            "dark_mode",
            "body_background",
        ] {
            assert!(got.contains(&rule), "{rule} missing from {got:?}");
        }
        assert!(!r.ok);
        let about: Vec<_> = r
            .problems
            .iter()
            .filter(|p| p.path == "about.html")
            .collect();
        assert_eq!(about.len(), 1, "{about:?}");
        assert!(about[0].message.contains("x/missing.css"));
    }

    #[test]
    fn references_resolve_relative_to_their_file() {
        assert_eq!(resolve("a/b.html", "../c.css"), Ok(Some("c.css".into())));
        assert_eq!(
            resolve("a/b.html", "./d/e.js?v=1"),
            Ok(Some("a/d/e.js".into()))
        );
        assert_eq!(resolve("index.html", "https://x.y/z"), Ok(None));
        assert_eq!(resolve("index.html", "data:image/png;base64,AA"), Ok(None));
        assert_eq!(resolve("index.html", "#x"), Ok(None));
        assert_eq!(resolve("index.html", "//cdn.example/x.js"), Ok(None));
        assert_eq!(resolve("index.html", "/x.js"), Err(()));
    }
}
