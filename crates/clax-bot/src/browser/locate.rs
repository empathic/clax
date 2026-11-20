//! Locators (spec `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`
//! §6.5): a chain of [`LocatorStep`]s, plain data the prelude's `Locator`
//! carries, parsed here ([`parse_locator`]) and resolved in the page at
//! each call ([`resolve`]), never kept as a live element between calls.
//!
//! # Resolution
//!
//! One attempt reads the page's document whole (`DOM.getDocument`,
//! `depth: -1`, `pierce: true`: open shadow roots included) and narrows
//! it step by step, from the document, each step within the elements the
//! chain has so far:
//!
//! | Step | Resolved by |
//! |---|---|
//! | `{css}` | `DOM.querySelectorAll` on each scope element and each open shadow root inside it |
//! | `{role, name?, exact?, checked?, level?, includeHidden?}` | `Accessibility.queryAXTree` of each scope element by role (Chrome's computed roles and names); names, `checked` and `level` matched here; nodes the tree ignores only with `includeHidden` |
//! | `{text, exact?}` | the smallest elements whose text (its text nodes', whitespace-normalised) matches |
//! | `{label, exact?}` | controls labelled by a `<label>` (wrapping, or `for`), `aria-labelledby` or `aria-label` that matches |
//! | `{placeholder, exact?}` | `input` and `textarea` whose `placeholder` matches |
//! | `{testId}` | elements whose `data-testid` matches (a string exactly) |
//! | `{nth}` | the one element at that index (from the end when negative) |
//! | `{hasText}`, `{hasNotText}` | those whose text does or does not contain it |
//!
//! A string matches case-insensitively as a substring of the
//! whitespace-normalised text, or, with `exact`, the whole of it with its
//! case. A `RegExp`, sent as `{regex, flags}`, is matched in the page: the
//! candidates' texts are tested there by `new RegExp(regex, flags)`, so a
//! pattern means what JavaScript says it means.
//!
//! # Approximations (risk `c2b4608a`)
//!
//! - Roles and names are Chrome's accessibility tree's, not Playwright's
//!   own engine's: the two disagree on generic containers, hidden
//!   subtrees and names built from `title`.
//! - An element's text is its text nodes' (as `textContent`, `<script>`
//!   and `<style>` left out), not its rendered `innerText`: hidden text
//!   counts.
//! - CSS pierces open shadow roots by querying each root on its own, so a
//!   selector whose parts span a shadow boundary matches nothing.
//! - Elements inside iframes are not reached.

use super::cdp::CdpFail;
use super::session::Driver;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The most steps of a chain.
pub const MAX_STEPS: usize = 16;
/// The most characters of a step's text, selector or pattern.
pub const MAX_STEP_TEXT: usize = 1_000;

/// What a text-matching step matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextMatch {
    /// A string: a case-insensitive substring, or the whole text with its
    /// case when `exact`.
    Text { text: String, exact: bool },
    /// A JavaScript regular expression, matched in the page.
    Regex { source: String, flags: String },
}

/// One step of a locator chain (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocatorStep {
    Css(String),
    Role {
        role: String,
        name: Option<TextMatch>,
        checked: Option<bool>,
        level: Option<u32>,
        include_hidden: bool,
    },
    Text(TextMatch),
    Label(TextMatch),
    Placeholder(TextMatch),
    TestId(TextMatch),
    Nth(i64),
    HasText(TextMatch),
    HasNotText(TextMatch),
}

/// The ARIA roles `getByRole` takes (WAI-ARIA 1.2's, abstract ones left
/// out); another is `invalid_args`.
pub const ROLES: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "meter",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
];

fn text_of_field(v: &Value, field: &str, exact: bool) -> Result<TextMatch, String> {
    match v {
        Value::String(s) if s.chars().count() <= MAX_STEP_TEXT => Ok(TextMatch::Text {
            text: s.clone(),
            exact,
        }),
        Value::Object(o) if o.contains_key("regex") => {
            let source = o["regex"]
                .as_str()
                .ok_or_else(|| format!("{field}.regex must be a string"))?;
            let flags = o.get("flags").and_then(Value::as_str).unwrap_or("");
            if source.chars().count() > MAX_STEP_TEXT {
                return Err(format!("{field}.regex is over {MAX_STEP_TEXT} characters"));
            }
            if !flags.chars().all(|c| "dgimsuyv".contains(c)) {
                return Err(format!("{field}.flags holds a flag RegExp does not have"));
            }
            Ok(TextMatch::Regex {
                source: source.to_string(),
                flags: flags.to_string(),
            })
        }
        _ => Err(format!(
            "{field} must be a string of at most {MAX_STEP_TEXT} characters or {{regex, flags}}"
        )),
    }
}

/// The chain `v` (an array of steps), or what is wrong with it, naming
/// the step.
///
/// # Errors
/// A step that does not parse, an unknown role, more than [`MAX_STEPS`].
pub fn parse_locator(v: &Value) -> Result<Vec<LocatorStep>, String> {
    let Some(steps) = v.as_array() else {
        return Err("locator must be an array of steps".into());
    };
    if steps.len() > MAX_STEPS {
        return Err(format!("locator has over {MAX_STEPS} steps"));
    }
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| parse_step(s).map_err(|e| format!("locator[{i}]: {e}")))
        .collect()
}

fn parse_step(s: &Value) -> Result<LocatorStep, String> {
    let Some(o) = s.as_object() else {
        return Err("a step must be an object".into());
    };
    let exact = o.get("exact").and_then(Value::as_bool).unwrap_or(false);
    if let Some(css) = o.get("css") {
        let css = css
            .as_str()
            .filter(|c| !c.trim().is_empty() && c.chars().count() <= MAX_STEP_TEXT)
            .ok_or("css must be a selector")?;
        return Ok(LocatorStep::Css(css.to_string()));
    }
    if let Some(role) = o.get("role") {
        let role = role.as_str().ok_or("role must be a string")?;
        if !ROLES.contains(&role) {
            return Err(format!("{role} is not an ARIA role"));
        }
        let name = o
            .get("name")
            .map(|n| text_of_field(n, "name", exact))
            .transpose()?;
        let checked = match o.get("checked") {
            None => None,
            Some(c) => Some(c.as_bool().ok_or("checked must be a boolean")?),
        };
        let level = match o.get("level") {
            None => None,
            Some(l) => Some(
                l.as_u64()
                    .and_then(|l| u32::try_from(l).ok())
                    .ok_or("level must be a whole number")?,
            ),
        };
        let include_hidden = o
            .get("includeHidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return Ok(LocatorStep::Role {
            role: role.to_string(),
            name,
            checked,
            level,
            include_hidden,
        });
    }
    for (key, make) in [
        ("text", LocatorStep::Text as fn(TextMatch) -> LocatorStep),
        ("label", LocatorStep::Label),
        ("placeholder", LocatorStep::Placeholder),
        ("hasText", LocatorStep::HasText),
        ("hasNotText", LocatorStep::HasNotText),
    ] {
        if let Some(t) = o.get(key) {
            return Ok(make(text_of_field(t, key, exact)?));
        }
    }
    if let Some(t) = o.get("testId") {
        // A test ID string matches exactly, as Playwright's.
        return Ok(LocatorStep::TestId(text_of_field(t, "testId", true)?));
    }
    if let Some(n) = o.get("nth") {
        return Ok(LocatorStep::Nth(
            n.as_i64().ok_or("nth must be a whole number")?,
        ));
    }
    Err(
        "a step names one of css, role, text, label, placeholder, testId, nth, hasText, hasNotText"
            .into(),
    )
}

/// `s` as a JavaScript string literal in single quotes.
fn quoted(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn shown(t: &TextMatch) -> String {
    match t {
        TextMatch::Text { text, .. } => quoted(text),
        TextMatch::Regex { source, flags } => format!("/{source}/{flags}"),
    }
}

fn exact_opt(t: &TextMatch) -> &'static str {
    match t {
        TextMatch::Text { exact: true, .. } => ", { exact: true }",
        _ => "",
    }
}

/// The chain in Playwright's words, as a script writes it:
/// `getByRole('button', { name: 'Save' }).first()`.
#[must_use]
pub fn describe(steps: &[LocatorStep]) -> String {
    let parts: Vec<String> = steps
        .iter()
        .map(|s| match s {
            LocatorStep::Css(c) => format!("locator({})", quoted(c)),
            LocatorStep::Role {
                role,
                name,
                checked,
                level,
                include_hidden,
            } => {
                let mut opts = Vec::new();
                if let Some(n) = name {
                    opts.push(format!("name: {}", shown(n)));
                    if matches!(n, TextMatch::Text { exact: true, .. }) {
                        opts.push("exact: true".into());
                    }
                }
                if let Some(c) = checked {
                    opts.push(format!("checked: {c}"));
                }
                if let Some(l) = level {
                    opts.push(format!("level: {l}"));
                }
                if *include_hidden {
                    opts.push("includeHidden: true".into());
                }
                if opts.is_empty() {
                    format!("getByRole({})", quoted(role))
                } else {
                    format!("getByRole({}, {{ {} }})", quoted(role), opts.join(", "))
                }
            }
            LocatorStep::Text(t) => format!("getByText({}{})", shown(t), exact_opt(t)),
            LocatorStep::Label(t) => format!("getByLabel({}{})", shown(t), exact_opt(t)),
            LocatorStep::Placeholder(t) => {
                format!("getByPlaceholder({}{})", shown(t), exact_opt(t))
            }
            LocatorStep::TestId(t) => format!("getByTestId({})", shown(t)),
            LocatorStep::Nth(0) => "first()".into(),
            LocatorStep::Nth(-1) => "last()".into(),
            LocatorStep::Nth(n) => format!("nth({n})"),
            LocatorStep::HasText(t) => format!("filter({{ hasText: {} }})", shown(t)),
            LocatorStep::HasNotText(t) => format!("filter({{ hasNotText: {} }})", shown(t)),
        })
        .collect();
    if parts.is_empty() {
        "page".into()
    } else {
        parts.join(".")
    }
}

/// `s` with each run of whitespace one space, trimmed.
#[must_use]
pub fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `text` matches the string `t` (see the module docs); a regex
/// is the page's to test.
fn matches_text(t: &str, exact: bool, text: &str) -> bool {
    let text = normalized(text);
    let t = normalized(t);
    if exact {
        text == t
    } else {
        text.to_lowercase().contains(&t.to_lowercase())
    }
}

/// One node of the page's document, as `DOM.getDocument` gave it.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub node_id: i64,
    pub backend: i64,
    /// Lowercase: `div`, `#text`, `#document`.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub value: Option<String>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// The open shadow roots it hosts.
    pub shadow: Vec<usize>,
}

impl Node {
    /// Whether it is an element.
    #[must_use]
    pub fn is_element(&self) -> bool {
        !self.name.starts_with('#')
    }

    /// Its attribute `name`.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The page's document at one attempt.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub by_node: BTreeMap<i64, usize>,
    pub by_backend: BTreeMap<i64, usize>,
}

impl Tree {
    /// The tree of `DOM.getDocument`'s answer `v`.
    #[must_use]
    pub fn of(v: &Value) -> Tree {
        let mut t = Tree::default();
        t.add(&v["root"], None);
        t
    }

    fn add(&mut self, v: &Value, parent: Option<usize>) -> usize {
        let at = self.nodes.len();
        let attrs = v["attributes"]
            .as_array()
            .map(|a| {
                a.chunks(2)
                    .map(|kv| {
                        (
                            kv[0].as_str().unwrap_or_default().to_string(),
                            kv.get(1)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let node = Node {
            node_id: v["nodeId"].as_i64().unwrap_or(0),
            backend: v["backendNodeId"].as_i64().unwrap_or(0),
            name: v["nodeName"].as_str().unwrap_or_default().to_lowercase(),
            attrs,
            value: v["nodeValue"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            parent,
            children: Vec::new(),
            shadow: Vec::new(),
        };
        self.by_node.insert(node.node_id, at);
        self.by_backend.insert(node.backend, at);
        self.nodes.push(node);
        for c in v["children"].as_array().into_iter().flatten() {
            let ci = self.add(c, Some(at));
            self.nodes[at].children.push(ci);
        }
        for r in v["shadowRoots"].as_array().into_iter().flatten() {
            if r["shadowRootType"] == "open" {
                let ri = self.add(r, Some(at));
                self.nodes[at].shadow.push(ri);
            }
        }
        at
    }

    /// The text of node `i`: its text nodes', in order, `<script>`,
    /// `<style>` and closed content left out (as `textContent`).
    #[must_use]
    pub fn text(&self, i: usize) -> String {
        let mut out = String::new();
        self.text_into(i, &mut out);
        out
    }

    fn text_into(&self, i: usize, out: &mut String) {
        let n = &self.nodes[i];
        if n.name == "#text" {
            out.push_str(n.value.as_deref().unwrap_or_default());
            return;
        }
        if matches!(
            n.name.as_str(),
            "script" | "style" | "noscript" | "template" | "head"
        ) {
            return;
        }
        for &c in n.children.iter().chain(&n.shadow) {
            self.text_into(c, out);
        }
    }

    /// Whether `inner` is `outer` or inside it (shadow roots included).
    #[must_use]
    pub fn contains(&self, outer: usize, inner: usize) -> bool {
        let mut at = Some(inner);
        while let Some(i) = at {
            if i == outer {
                return true;
            }
            at = self.nodes[i].parent;
        }
        false
    }

    /// Every element inside `i` (not `i`), in document order.
    fn descendants(&self, i: usize, out: &mut Vec<usize>) {
        for &c in self.nodes[i].children.iter().chain(&self.nodes[i].shadow) {
            if self.nodes[c].is_element() {
                out.push(c);
            }
            self.descendants(c, out);
        }
    }

    /// The element with ID `id`, if any.
    fn by_id(&self, id: &str) -> Option<usize> {
        (0..self.nodes.len())
            .find(|&i| self.nodes[i].is_element() && self.nodes[i].attr("id") == Some(id))
    }

    /// How element `i` is shown in a message: `<div class="overlay">`.
    #[must_use]
    pub fn shown(&self, i: usize) -> String {
        let n = &self.nodes[i];
        let mut s = format!("<{}", n.name);
        for key in ["id", "class", "data-testid", "role", "type", "name"] {
            if let Some(v) = n.attr(key) {
                let v: String = v.chars().take(60).collect();
                s.push_str(&format!(" {key}=\"{v}\""));
            }
        }
        s.push('>');
        let text = normalized(&self.text(i));
        if !text.is_empty() {
            let t: String = text.chars().take(40).collect();
            s.push_str(&t);
            if text.chars().count() > 40 {
                s.push('…');
            }
            s.push_str(&format!("</{}>", n.name));
        }
        s
    }

    /// The root (the document).
    fn root(&self) -> usize {
        0
    }
}

/// One attempt's resolution: the page's document and the elements the
/// chain matches, in document order.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub tree: Tree,
    pub found: Vec<usize>,
}

/// Which of `texts` the regex `source`/`flags` matches, tested in the
/// page (see the module docs).
async fn regex_test(
    d: &mut Driver<'_>,
    source: &str,
    flags: &str,
    texts: &[String],
) -> Result<Vec<bool>, CdpFail> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let expr = format!(
        "/* clax:regex */ ((a) => {{ const re = new RegExp(a[0], a[1]); return a[2].map((t) => {{ re.lastIndex = 0; return re.test(t); }}); }})({})",
        json!([source, flags, texts])
    );
    Ok(match d.evaluate(&expr).await? {
        Ok(Value::Array(a)) => a.iter().map(|b| b.as_bool() == Some(true)).collect(),
        _ => vec![false; texts.len()],
    })
}

/// Which of `texts` match `t`.
pub(super) async fn text_matches(
    d: &mut Driver<'_>,
    t: &TextMatch,
    texts: &[String],
) -> Result<Vec<bool>, CdpFail> {
    match t {
        TextMatch::Text { text, exact } => Ok(texts
            .iter()
            .map(|s| matches_text(text, *exact, s))
            .collect()),
        TextMatch::Regex { source, flags } => {
            let normalised: Vec<String> = texts.iter().map(|s| normalized(s)).collect();
            regex_test(d, source, flags, &normalised).await
        }
    }
}

/// `items` where `keep` holds.
fn kept(items: Vec<usize>, keep: &[bool]) -> Vec<usize> {
    items
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| **k)
        .map(|(i, _)| i)
        .collect()
}

/// Resolves `steps` in the page once (see the module docs).
///
/// # Errors
/// What ended the wait; the browser's refusal of a command (a selector it
/// cannot parse among them).
pub(super) async fn resolve(
    d: &mut Driver<'_>,
    steps: &[LocatorStep],
) -> Result<Resolved, CdpFail> {
    let doc = d
        .page_call("DOM.getDocument", json!({"depth": -1, "pierce": true}))
        .await?;
    let tree = Tree::of(&doc);
    let mut scope: Vec<usize> = vec![tree.root()];
    for step in steps {
        scope = step_once(d, &tree, &scope, step).await?;
    }
    // The document itself is no match.
    let found = scope
        .into_iter()
        .filter(|&i| tree.nodes[i].is_element())
        .collect();
    Ok(Resolved { tree, found })
}

/// Every element within `scope` (each scope element's descendants, and
/// the element itself unless it is the document), deduplicated, in
/// document order.
fn within(tree: &Tree, scope: &[usize], with_self: bool) -> Vec<usize> {
    let mut out = BTreeSet::new();
    for &s in scope {
        if with_self && tree.nodes[s].is_element() {
            out.insert(s);
        }
        let mut v = Vec::new();
        tree.descendants(s, &mut v);
        out.extend(v);
    }
    out.into_iter().collect()
}

async fn step_once(
    d: &mut Driver<'_>,
    tree: &Tree,
    scope: &[usize],
    step: &LocatorStep,
) -> Result<Vec<usize>, CdpFail> {
    Ok(match step {
        LocatorStep::Css(sel) => {
            let mut roots: Vec<usize> = Vec::new();
            for &s in scope {
                roots.push(s);
                // Each open shadow root inside the scope, queried on its
                // own.
                let mut inside = Vec::new();
                tree.descendants(s, &mut inside);
                for i in std::iter::once(s).chain(inside) {
                    roots.extend(tree.nodes[i].shadow.iter().copied());
                }
            }
            let mut found = BTreeSet::new();
            for r in roots {
                let a = d
                    .page_call(
                        "DOM.querySelectorAll",
                        json!({"nodeId": tree.nodes[r].node_id, "selector": sel}),
                    )
                    .await?;
                for id in a["nodeIds"].as_array().into_iter().flatten() {
                    if let Some(&i) = id.as_i64().and_then(|id| tree.by_node.get(&id)) {
                        found.insert(i);
                    }
                }
            }
            found.into_iter().collect()
        }
        LocatorStep::Role {
            role,
            name,
            checked,
            level,
            include_hidden,
        } => {
            let mut found = BTreeSet::new();
            let mut named: Vec<(usize, String)> = Vec::new();
            for &s in scope {
                let a = d
                    .page_call(
                        "Accessibility.queryAXTree",
                        json!({"backendNodeId": tree.nodes[s].backend, "role": role}),
                    )
                    .await?;
                for n in a["nodes"].as_array().into_iter().flatten() {
                    if n["ignored"] == true && !include_hidden {
                        continue;
                    }
                    let Some(&i) = n["backendDOMNodeId"]
                        .as_i64()
                        .and_then(|b| tree.by_backend.get(&b))
                    else {
                        continue;
                    };
                    let prop = |p: &str| {
                        n["properties"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|x| x["name"] == p)
                            .map(|x| x["value"]["value"].clone())
                    };
                    if let Some(c) = checked {
                        let is = prop("checked").is_some_and(|v| v == "true" || v == true);
                        if is != *c {
                            continue;
                        }
                    }
                    if let Some(l) = level
                        && prop("level").and_then(|v| v.as_u64()) != Some(u64::from(*l))
                    {
                        continue;
                    }
                    named.push((
                        i,
                        n["name"]["value"].as_str().unwrap_or_default().to_string(),
                    ));
                }
            }
            match name {
                None => found.extend(named.iter().map(|(i, _)| *i)),
                Some(t) => {
                    let texts: Vec<String> = named.iter().map(|(_, n)| n.clone()).collect();
                    let keep = text_matches(d, t, &texts).await?;
                    found.extend(
                        named
                            .iter()
                            .zip(keep)
                            .filter(|(_, k)| *k)
                            .map(|((i, _), _)| *i),
                    );
                }
            }
            found.into_iter().collect()
        }
        LocatorStep::Text(t) => {
            let all = within(tree, scope, false);
            let texts: Vec<String> = all.iter().map(|&i| tree.text(i)).collect();
            let keep = text_matches(d, t, &texts).await?;
            let hits: Vec<usize> = kept(all, &keep);
            // The smallest: no other hit inside it.
            hits.iter()
                .copied()
                .filter(|&h| !hits.iter().any(|&o| o != h && tree.contains(h, o)))
                .collect()
        }
        LocatorStep::Label(t) => {
            let all = within(tree, scope, false);
            let mut labelled: Vec<(usize, String)> = Vec::new();
            for &i in &all {
                let n = &tree.nodes[i];
                if let Some(l) = n.attr("aria-label") {
                    labelled.push((i, l.to_string()));
                }
                if let Some(ids) = n.attr("aria-labelledby") {
                    let text = ids
                        .split_whitespace()
                        .filter_map(|id| tree.by_id(id))
                        .map(|l| tree.text(l))
                        .collect::<Vec<_>>()
                        .join(" ");
                    labelled.push((i, text));
                }
                if n.name == "label" {
                    let control = match n.attr("for") {
                        Some(id) => tree.by_id(id),
                        None => {
                            let mut inside = Vec::new();
                            tree.descendants(i, &mut inside);
                            inside.into_iter().find(|&c| {
                                matches!(
                                    tree.nodes[c].name.as_str(),
                                    "input"
                                        | "textarea"
                                        | "select"
                                        | "button"
                                        | "meter"
                                        | "output"
                                        | "progress"
                                )
                            })
                        }
                    };
                    if let Some(c) = control {
                        labelled.push((c, tree.text(i)));
                    }
                }
            }
            let texts: Vec<String> = labelled.iter().map(|(_, s)| s.clone()).collect();
            let keep = text_matches(d, t, &texts).await?;
            let inside: BTreeSet<usize> = all.into_iter().collect();
            let found: BTreeSet<usize> = labelled
                .iter()
                .zip(keep)
                .filter(|(_, k)| *k)
                .map(|((i, _), _)| *i)
                .filter(|i| inside.contains(i))
                .collect();
            found.into_iter().collect()
        }
        LocatorStep::Placeholder(t) => {
            let all: Vec<usize> = within(tree, scope, false)
                .into_iter()
                .filter(|&i| {
                    matches!(tree.nodes[i].name.as_str(), "input" | "textarea")
                        && tree.nodes[i].attr("placeholder").is_some()
                })
                .collect();
            let texts: Vec<String> = all
                .iter()
                .map(|&i| {
                    tree.nodes[i]
                        .attr("placeholder")
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let keep = text_matches(d, t, &texts).await?;
            kept(all, &keep)
        }
        LocatorStep::TestId(t) => {
            let all: Vec<usize> = within(tree, scope, false)
                .into_iter()
                .filter(|&i| tree.nodes[i].attr("data-testid").is_some())
                .collect();
            let texts: Vec<String> = all
                .iter()
                .map(|&i| {
                    tree.nodes[i]
                        .attr("data-testid")
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let keep = text_matches(d, t, &texts).await?;
            kept(all, &keep)
        }
        LocatorStep::Nth(n) => {
            let elements: Vec<usize> = scope
                .iter()
                .copied()
                .filter(|&i| tree.nodes[i].is_element())
                .collect();
            let len = i64::try_from(elements.len()).unwrap_or(i64::MAX);
            let at = if *n < 0 { len + n } else { *n };
            usize::try_from(at)
                .ok()
                .and_then(|a| elements.get(a).copied())
                .into_iter()
                .collect()
        }
        LocatorStep::HasText(t) | LocatorStep::HasNotText(t) => {
            let elements: Vec<usize> = scope
                .iter()
                .copied()
                .filter(|&i| tree.nodes[i].is_element())
                .collect();
            let texts: Vec<String> = elements.iter().map(|&i| tree.text(i)).collect();
            // A filter's string is a substring, whatever `exact`.
            let t = match t {
                TextMatch::Text { text, .. } => TextMatch::Text {
                    text: text.clone(),
                    exact: false,
                },
                r => r.clone(),
            };
            let mut keep = text_matches(d, &t, &texts).await?;
            if matches!(step, LocatorStep::HasNotText(_)) {
                for k in &mut keep {
                    *k = !*k;
                }
            }
            kept(elements, &keep)
        }
    })
}
