//! Comment anchors: where on a page a thread points, as the bridge records it.
//! A version may hold several HTML pages; an anchor names its page in `file`.
//! An `artifact` anchor points at nothing on any page: its thread is about the
//! whole artifact, and its `file` is empty.
//! DOM resolution happens in the browser; the daemon only validates, stores,
//! and summarises anchors.

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// Longest accepted CSS selector, in characters.
pub const MAX_SELECTOR: usize = 1024;
/// Longest accepted quote, in characters.
pub const MAX_QUOTE: usize = 2000;
/// Longest accepted prefix or suffix, in characters.
pub const MAX_AFFIX: usize = 64;
/// Longest accepted `name`, in characters.
pub const MAX_NAME: usize = 120;
/// Characters of the quote shown by [`Anchor::summary`].
const SUMMARY_QUOTE: usize = 120;
/// Longest accepted `file`, in bytes.
pub const MAX_FILE: usize = 512;
/// The page an anchor is on when it names none: the version's index.
pub const INDEX_FILE: &str = crate::publish::INDEX;

fn index_file() -> String {
    INDEX_FILE.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorKind {
    Element,
    Range,
    Custom,
    Area,
    /// The whole artifact: no page, element, text, or area. Every target
    /// field is null and `file` is empty ([`WHOLE_FILE`]).
    Artifact,
}

/// The `file` of an `artifact` anchor: it is on no page.
pub const WHOLE_FILE: &str = "";
/// What [`Anchor::summary`] says for an `artifact` anchor.
pub const WHOLE_SUMMARY: &str = "the whole artifact";

/// A drawn rectangle as fractions (0 to 1) of its element's border box: `x`
/// and `y` from its top left corner, `w` and `h` of its width and height.
/// `tag`, `text`, and `children` fingerprint that element at draw time (its
/// local name, the first characters of its text with whitespace collapsed,
/// its child element count), so re-anchoring by selector alone can tell
/// another element at the same path; each is optional and serialized only
/// when present.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorArea {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<u32>,
}

/// Longest area fingerprint `tag` or `text`, in characters.
pub const MAX_FINGERPRINT: usize = 64;

impl AnchorArea {
    /// Every fraction is finite, the rectangle has some width and height,
    /// and it lies within the element's box.
    fn in_range(&self) -> bool {
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        // Fractions are rounded, so their sums may pass 1 by a rounding step.
        let fits = |a: f64, b: f64| a + b <= 1.0 + 1e-4;
        [self.x, self.y, self.w, self.h].into_iter().all(unit)
            && self.w > 0.0
            && self.h > 0.0
            && fits(self.x, self.w)
            && fits(self.y, self.h)
    }

    /// The fingerprint's `tag` and `text` are at most [`MAX_FINGERPRINT`]
    /// characters with no control characters or line separators.
    fn fingerprint_ok(&self) -> bool {
        [&self.tag, &self.text].into_iter().flatten().all(|s| {
            s.chars().count() <= MAX_FINGERPRINT
                && !s
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        })
    }
}

/// The anchored region at pick time, in viewport pixels, with the page's
/// scroll offsets and viewport width.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(rename = "scrollX")]
    pub scroll_x: f64,
    #[serde(rename = "scrollY")]
    pub scroll_y: f64,
    #[serde(rename = "viewportW")]
    pub viewport_w: f64,
}

/// Spec §9 "Anchors". Every field but `kind` and `file` may be null. `file` is
/// the published path of the page the anchor is on, `index.html` when absent;
/// an `artifact` anchor is on no page, and its `file` is empty.
/// An `area` anchor is a rectangle the viewer drew: `selector` names the
/// smallest element containing it, `area` places it within that element, and
/// `rect` holds it in viewport pixels at draw time; `area` is serialized only
/// when present.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub kind: AnchorKind,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub html_hash: Option<String>,
    #[serde(default)]
    pub rect: Option<AnchorRect>,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<AnchorArea>,
    #[serde(default = "index_file")]
    pub file: String,
    /// The route a live page's thread was made at (spec 2026-10-05 §5.2):
    /// the query and hash route, set by the daemon; absent elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The quote as a person reads it: its text with a space where it runs
    /// from one element into another, which the quote (kept as the page's
    /// text nodes join, for re-anchoring) lacks. Shown in place of the quote;
    /// absent on anchors made without one, which show the quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What the element is called, as the bridge read it when it was picked:
    /// its accessible name, else the start of its text, else its kind with
    /// the nearest heading ("Chart under ‘p95 by four-hour window’"). Shown
    /// as a thread's title where the anchor quotes nothing; absent on
    /// anchors made without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_anchor", message)
}

fn check_len(name: &str, v: &Option<String>, max: usize) -> Result<()> {
    match v {
        Some(s) if s.chars().count() > max => {
            Err(bad(format!("{name} is longer than {max} characters")))
        }
        _ => Ok(()),
    }
}

/// Whitespace runs collapsed to one space, trimmed.
pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `s` without whitespace, control characters or U+FEFF: what a label and
/// its quote must share.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| !(c.is_whitespace() || c.is_control() || *c == '\u{FEFF}'))
        .collect()
}

/// At most `n` characters of `s`, with `…` appended when cut.
pub fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// `f` (a fraction) as whole percent with `%`, clamped to 0 to 100; `<1%`
/// for a non-zero share that rounds to 0.
fn pct(f: f64) -> String {
    // Clamped to 0..=100 first, so the cast cannot truncate or wrap.
    #[allow(clippy::cast_possible_truncation)]
    let p = (f.clamp(0.0, 1.0) * 100.0).round() as i64;
    if p == 0 && f > 0.0 {
        "<1%".to_string()
    } else {
        format!("{p}%")
    }
}

impl Anchor {
    /// Element, range, and area anchors need a selector, custom anchors a
    /// `custom_name`; an area anchor needs an `area` whose fractions lie in 0
    /// to 1 with some width and height, within the element's box, and only area
    /// anchors carry one; selectors and names hold no control characters and no
    /// U+2028 or U+2029 line or paragraph separators, and a `name` is not
    /// blank; lengths are capped by [`MAX_SELECTOR`], [`MAX_QUOTE`],
    /// [`MAX_AFFIX`], and [`MAX_NAME`].
    /// `file` is a safe relative path ([`crate::publish::check_path`]) of at
    /// most [`MAX_FILE`] bytes; whether
    /// the version holds it is the store's check.
    ///
    /// An `artifact` anchor carries no selector, quote, prefix, suffix,
    /// `html_hash`, `rect`, `custom_name`, `area`, `route`, `label`, or `name`, and its `file`
    /// is empty ([`Anchor::normalized`] empties the index default).
    ///
    /// # Errors
    /// `Invalid { code: "invalid_anchor" }` naming the first problem.
    pub fn validate(&self) -> Result<()> {
        if self.kind == AnchorKind::Artifact {
            return self.validate_whole();
        }
        match self.kind {
            AnchorKind::Element | AnchorKind::Range | AnchorKind::Area
                if self.selector.as_deref().is_none_or(str::is_empty) =>
            {
                return Err(bad("element, range, and area anchors need a selector"));
            }
            AnchorKind::Custom if self.custom_name.as_deref().is_none_or(str::is_empty) => {
                return Err(bad("custom anchors need a custom_name"));
            }
            _ => {}
        }
        match (&self.kind, &self.area) {
            (AnchorKind::Area, None) => return Err(bad("area anchors need an area")),
            (AnchorKind::Area, Some(a)) if !a.in_range() => {
                return Err(bad(
                    "area fractions must lie in 0 to 1, with some width and height, within the element",
                ));
            }
            (AnchorKind::Area, Some(a)) if !a.fingerprint_ok() => {
                return Err(bad(format!(
                    "area tag and text are at most {MAX_FINGERPRINT} characters without control characters"
                )));
            }
            (AnchorKind::Area, Some(_)) | (_, None) => {}
            (_, Some(_)) => return Err(bad("only area anchors carry an area")),
        }
        if self.file.len() > MAX_FILE {
            return Err(bad(format!("file is longer than {MAX_FILE} bytes")));
        }
        if crate::publish::check_path(&self.file).is_err() {
            return Err(bad("file is not a safe relative path"));
        }
        for (name, v) in [
            ("selector", &self.selector),
            ("custom_name", &self.custom_name),
            ("html_hash", &self.html_hash),
            ("name", &self.name),
        ] {
            if v.as_deref().is_some_and(|s| {
                s.chars()
                    .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
            }) {
                return Err(bad(format!(
                    "{name} contains control characters or line breaks"
                )));
            }
        }
        check_len("selector", &self.selector, MAX_SELECTOR)?;
        check_len("custom_name", &self.custom_name, MAX_SELECTOR)?;
        check_len("html_hash", &self.html_hash, 80)?;
        check_len("name", &self.name, MAX_NAME)?;
        if self.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
            return Err(bad("name is empty"));
        }
        check_len("quote", &self.quote, MAX_QUOTE)?;
        check_len("prefix", &self.prefix, MAX_AFFIX)?;
        check_len("suffix", &self.suffix, MAX_AFFIX)?;
        self.validate_label()?;
        if let Some(r) = &self.route
            && (r.len() > crate::live::MAX_ROUTE
                || !(r.starts_with('?') || r.starts_with('#'))
                || r.chars()
                    .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')))
        {
            return Err(bad(format!(
                "route must start with ? or #, be at most {} bytes, and hold no control characters",
                crate::live::MAX_ROUTE
            )));
        }
        Ok(())
    }

    /// A `label` is at most [`MAX_QUOTE`] characters of plain text (no control
    /// characters or line separators) and reads as the quote, or the start of
    /// it, with only its whitespace changed; an anchor without a quote has none.
    fn validate_label(&self) -> Result<()> {
        let Some(label) = &self.label else {
            return Ok(());
        };
        check_len("label", &self.label, MAX_QUOTE)?;
        if label
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            return Err(bad("label contains control characters or line breaks"));
        }
        let shown = squash(label);
        match &self.quote {
            Some(q) if !shown.is_empty() && squash(q).starts_with(&shown) => Ok(()),
            _ => Err(bad(
                "label is the quote, or its start, with only its whitespace changed",
            )),
        }
    }

    /// An `artifact` anchor names nothing on a page.
    fn validate_whole(&self) -> Result<()> {
        let targets = self.selector.is_some()
            || self.quote.is_some()
            || self.prefix.is_some()
            || self.suffix.is_some()
            || self.html_hash.is_some()
            || self.rect.is_some()
            || self.custom_name.is_some()
            || self.area.is_some()
            || self.route.is_some()
            || self.label.is_some()
            || self.name.is_some();
        if targets {
            return Err(bad(
                "artifact anchors carry no selector, quote, prefix, suffix, html_hash, rect, custom_name, area, route, label, or name",
            ));
        }
        if self.file != WHOLE_FILE {
            return Err(bad("artifact anchors name no page: file is empty"));
        }
        Ok(())
    }

    /// Whether the anchor is about the whole artifact.
    pub fn is_whole(&self) -> bool {
        self.kind == AnchorKind::Artifact
    }

    /// The anchor as stored: an `artifact` anchor sent without a `file` (read
    /// as the index) or with `index.html` has its `file` emptied, since it is
    /// on no page. Any other anchor is unchanged.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        if self.is_whole() && self.file == INDEX_FILE {
            self.file = WHOLE_FILE.to_string();
        }
        self
    }

    /// One line naming the anchor: `the whole artifact` for an `artifact`
    /// anchor; otherwise the route and ` › ` when there is one,
    /// the file and ` › ` when it is not `index.html`, the selector (or `custom:<name>`, or for an area
    /// `area in <selector> (<w>% × <h>%)`, its share of the element's width
    /// and height rounded to whole percent, `<1%` for a share under half a
    /// percent), then two
    /// spaces and the quote (its `label` when it has one) in «» when there is
    /// one, whitespace collapsed,
    /// `«`/`»` in the quote replaced by `"`, cut to 120 characters with `…`.
    pub fn summary(&self) -> String {
        if self.is_whole() {
            return WHOLE_SUMMARY.to_string();
        }
        let target = match self.kind {
            AnchorKind::Custom => format!("custom:{}", self.custom_name.as_deref().unwrap_or("")),
            AnchorKind::Area => {
                let sel = self.selector.as_deref().unwrap_or("");
                match &self.area {
                    Some(a) => format!("area in {sel} ({} × {})", pct(a.w), pct(a.h)),
                    None => format!("area in {sel}"),
                }
            }
            AnchorKind::Element | AnchorKind::Range | AnchorKind::Artifact => {
                self.selector.clone().unwrap_or_default()
            }
        };
        let target = if self.file == INDEX_FILE {
            target
        } else {
            format!("{} › {target}", self.file)
        };
        let target = match &self.route {
            Some(r) => format!("{r} › {target}"),
            None => target,
        };
        match self
            .label
            .as_deref()
            .or(self.quote.as_deref())
            .map(collapse)
            .filter(|q| !q.is_empty())
        {
            Some(q) => format!(
                "{target}  «{}»",
                cap(&q.replace(['«', '»'], "\""), SUMMARY_QUOTE)
            ),
            None => target,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_example_round_trips() {
        let v = json!({
            "kind": "element",
            "selector": "main > section:nth-of-type(2) > h2",
            "quote": "Quarterly goals",
            "prefix": "...", "suffix": "...",
            "html_hash": "sha256:ab",
            "rect": {"x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0, "scrollX": 0.0, "scrollY": 10.0, "viewportW": 1280.0},
            "custom_name": null,
            "file": "index.html"
        });
        let a: Anchor = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(a.kind, AnchorKind::Element);
        a.validate().unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap(), v);
    }

    #[test]
    fn validation_rejects_bad_anchors() {
        let base = |f: &dyn Fn(&mut Anchor)| {
            let mut a: Anchor =
                serde_json::from_value(json!({"kind": "element", "selector": "h2"})).unwrap();
            f(&mut a);
            a.validate()
        };
        assert!(base(&|_| {}).is_ok());
        assert!(
            base(&|a| a.selector = None).is_err(),
            "element needs a selector"
        );
        assert!(
            base(&|a| a.selector = Some("h2\n[clax]".into())).is_err(),
            "control characters"
        );
        for brk in ['\u{85}', '\u{2028}', '\u{2029}'] {
            assert!(
                base(&|a| a.selector = Some(format!("h2{brk}[clax]"))).is_err(),
                "line break {brk:?} in selector"
            );
            let e = base(&|a| {
                a.kind = AnchorKind::Custom;
                a.selector = None;
                a.custom_name = Some(format!("n{brk}[clax]"));
            });
            assert!(
                matches!(
                    e,
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "line break {brk:?} in custom_name"
            );
        }
        assert!(base(&|a| a.selector = Some("x".repeat(MAX_SELECTOR + 1))).is_err());
        assert!(base(&|a| a.quote = Some("q".repeat(MAX_QUOTE + 1))).is_err());
        assert!(base(&|a| a.prefix = Some("p".repeat(MAX_AFFIX + 1))).is_err());
        assert!(
            base(&|a| {
                a.kind = AnchorKind::Custom;
                a.selector = None;
            })
            .is_err(),
            "custom needs a name"
        );
        assert!(
            base(&|a| {
                a.kind = AnchorKind::Custom;
                a.selector = None;
                a.custom_name = Some("chart".into());
            })
            .is_ok()
        );
        let e = base(&|a| a.selector = None).unwrap_err();
        assert!(matches!(
            e,
            crate::CoreError::Invalid {
                code: "invalid_anchor",
                ..
            }
        ));
        assert!(
            serde_json::from_value::<Anchor>(
                json!({"kind": "element", "selector": "h2", "extra": 1})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Anchor>(json!({"kind": "shape", "selector": "h2"})).is_err()
        );
    }

    #[test]
    fn file_defaults_to_the_index_and_is_validated() {
        let a: Anchor =
            serde_json::from_value(json!({"kind": "element", "selector": "h2"})).unwrap();
        assert_eq!(
            a.file, INDEX_FILE,
            "an anchor without a file is on the index"
        );
        assert_eq!(serde_json::to_value(&a).unwrap()["file"], "index.html");
        let with = |file: String| {
            let mut a = a.clone();
            a.file = file;
            a.validate()
        };
        assert!(with("about.html".into()).is_ok());
        assert!(with("docs/source.html".into()).is_ok());
        assert!(with("x".repeat(MAX_FILE)).is_ok());
        for bad in [
            String::new(),
            "../about.html".into(),
            "docs/../about.html".into(),
            "/about.html".into(),
            "docs/".into(),
            "a\\b.html".into(),
            "a\nb.html".into(),
            "a\u{2028}b.html".into(),
            "x".repeat(MAX_FILE + 1),
        ] {
            assert!(
                matches!(
                    with(bad.clone()),
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_route_leads_the_summary() {
        let a: Anchor = serde_json::from_value(json!({
            "kind": "element", "selector": "main > button", "quote": "Save", "route": "?tab=billing"
        }))
        .unwrap();
        assert!(a.summary().starts_with("?tab=billing › main > button"));
    }

    #[test]
    fn summary_names_a_file_other_than_the_index() {
        let mut a: Anchor = serde_json::from_value(
            json!({"kind": "element", "selector": "main > h2", "quote": "Sources", "file": "source.html"}),
        )
        .unwrap();
        assert_eq!(a.summary(), "source.html › main > h2  «Sources»");
        a.file = INDEX_FILE.into();
        assert_eq!(a.summary(), "main > h2  «Sources»");
    }

    #[test]
    fn summary_collapses_whitespace_and_caps_the_quote() {
        let mut a: Anchor = serde_json::from_value(
            json!({"kind": "range", "selector": "body > p", "quote": "  two\n\n  words «x» "}),
        )
        .unwrap();
        assert_eq!(a.summary(), "body > p  «two words \"x\"»");
        a.quote = Some("w".repeat(200));
        let s = a.summary();
        assert!(s.ends_with("…»"), "{s}");
        assert_eq!(s.chars().filter(|c| *c == 'w').count(), 120);
        a.quote = None;
        assert_eq!(a.summary(), "body > p");
        let c: Anchor =
            serde_json::from_value(json!({"kind": "custom", "custom_name": "chart-1"})).unwrap();
        assert_eq!(c.summary(), "custom:chart-1");
    }

    #[test]
    fn a_label_respaces_the_quote_for_display() {
        let v = json!({"kind": "element", "selector": "body > div", "quote": "Mon5 km", "label": "Mon 5 km"});
        let a: Anchor = serde_json::from_value(v.clone()).unwrap();
        a.validate().unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap()["label"], "Mon 5 km");
        assert_eq!(a.summary(), "body > div  «Mon 5 km»");
        let old: Anchor = serde_json::from_value(
            json!({"kind": "element", "selector": "body > div", "quote": "Mon5 km"}),
        )
        .unwrap();
        old.validate().unwrap();
        assert!(serde_json::to_value(&old).unwrap().get("label").is_none());
        assert_eq!(old.summary(), "body > div  «Mon5 km»");
        let with = |label: String| {
            let mut a = a.clone();
            a.label = Some(label);
            a.validate()
        };
        // A label may stop short of a long quote's end; one with U+FEFF or a
        // control character dropped from the quote still matches.
        assert!(with("Mon 5".into()).is_ok());
        let mut q = a.clone();
        q.quote = Some("Mon\u{FEFF}\u{85}5 km".into());
        q.validate().unwrap();
        let long = "w ".repeat(MAX_QUOTE);
        let mut big = a.clone();
        big.quote = Some("w".repeat(MAX_QUOTE));
        big.label = Some(long.trim_end().into());
        for bad in [
            with(String::new()),
            with("Tue 6 km".into()),
            with("Mon 5 km more".into()),
            with("Mon\n5 km".into()),
            with("Mon\u{2028}5 km".into()),
            big.validate(),
        ] {
            assert!(
                matches!(
                    bad,
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "{bad:?}"
            );
        }
        let mut no_quote = a.clone();
        no_quote.quote = None;
        assert!(no_quote.validate().is_err(), "a label needs a quote");
        let whole: Anchor =
            serde_json::from_value(json!({"kind": "artifact", "file": "", "label": "x"})).unwrap();
        assert!(whole.validate().is_err());
    }

    #[test]
    fn a_name_is_kept_as_plain_short_text() {
        let v = json!({"kind": "area", "selector": "main > canvas", "area": {"x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5},
            "name": "Chart under ‘p95 by four-hour window’"});
        let a: Anchor = serde_json::from_value(v).unwrap();
        a.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap()["name"],
            "Chart under ‘p95 by four-hour window’"
        );
        let old: Anchor =
            serde_json::from_value(json!({"kind": "element", "selector": "body > div"})).unwrap();
        old.validate().unwrap();
        assert!(serde_json::to_value(&old).unwrap().get("name").is_none());
        let with = |name: &str| {
            let mut a = a.clone();
            a.name = Some(name.into());
            a.validate()
        };
        assert!(with(&"n".repeat(MAX_NAME)).is_ok());
        for bad in [
            with(&"n".repeat(MAX_NAME + 1)),
            with("Save\nnow"),
            with("Save\u{2028}now"),
            with("  "),
        ] {
            assert!(
                matches!(
                    bad,
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "{bad:?}"
            );
        }
        let whole: Anchor =
            serde_json::from_value(json!({"kind": "artifact", "file": "", "name": "x"})).unwrap();
        assert!(whole.validate().is_err());
    }

    fn area_anchor() -> Anchor {
        serde_json::from_value(json!({
            "kind": "area",
            "selector": "main > section:nth-of-type(2)",
            "area": {"x": 0.1, "y": 0.25, "w": 0.4213, "h": 0.1788},
            "rect": {"x": 10.0, "y": 20.0, "w": 300.0, "h": 90.0, "scrollX": 0.0, "scrollY": 400.0, "viewportW": 1280.0}
        }))
        .unwrap()
    }

    #[test]
    fn area_anchors_validate_their_fractions_and_need_a_selector() {
        let a = area_anchor();
        a.validate().unwrap();
        let with = |f: &dyn Fn(&mut Anchor)| {
            let mut a = area_anchor();
            f(&mut a);
            a.validate()
        };
        let area = |x: f64, y: f64, w: f64, h: f64| {
            Some(AnchorArea {
                x,
                y,
                w,
                h,
                ..Default::default()
            })
        };
        assert!(with(&|a| a.area = area(0.0, 0.0, 1.0, 1.0)).is_ok());
        for (name, bad) in [
            ("no area", None),
            ("negative x", area(-0.1, 0.0, 0.5, 0.5)),
            ("past the right edge", area(0.6, 0.0, 0.5, 0.5)),
            ("past the bottom edge", area(0.0, 0.7, 0.5, 0.4)),
            ("zero width", area(0.0, 0.0, 0.0, 0.5)),
            ("zero height", area(0.0, 0.0, 0.5, 0.0)),
            ("not finite", area(f64::NAN, 0.0, 0.5, 0.5)),
            ("over one", area(0.0, 0.0, 1.5, 0.5)),
        ] {
            assert!(
                matches!(
                    with(&|a| a.area = bad.clone()),
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "{name}"
            );
        }
        assert!(
            with(&|a| a.selector = None).is_err(),
            "area needs a selector"
        );
        assert!(
            with(&|a| a.selector = Some("x".repeat(MAX_SELECTOR + 1))).is_err(),
            "the selector limit holds"
        );
        let mut e: Anchor =
            serde_json::from_value(json!({"kind": "element", "selector": "h2"})).unwrap();
        e.area = area(0.0, 0.0, 1.0, 1.0);
        assert!(e.validate().is_err(), "only area anchors carry an area");
        assert!(
            serde_json::from_value::<Anchor>(json!({
                "kind": "area", "selector": "h2", "area": {"x": 0, "y": 0, "w": 1, "h": 1, "z": 1}
            }))
            .is_err(),
            "unknown area fields"
        );
    }

    #[test]
    fn area_fingerprints_round_trip_and_are_bounded() {
        let v = json!({"kind": "area", "selector": "main > section", "area": {"x": 0.1, "y": 0.2, "w": 0.3, "h": 0.4, "tag": "section", "text": "Quarterly goals", "children": 3}});
        let a: Anchor = serde_json::from_value(v.clone()).unwrap();
        a.validate().unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap()["area"], v["area"]);
        for (name, patch) in [
            ("long tag", json!({"tag": "x".repeat(MAX_FINGERPRINT + 1)})),
            (
                "long text",
                json!({"text": "t".repeat(MAX_FINGERPRINT + 1)}),
            ),
            ("control text", json!({"text": "a\nb"})),
            ("separator tag", json!({"tag": "a\u{2028}b"})),
        ] {
            let mut bad = v.clone();
            for (k, val) in patch.as_object().unwrap() {
                bad["area"][k] = val.clone();
            }
            let a: Anchor = serde_json::from_value(bad).unwrap();
            assert!(a.validate().is_err(), "{name}");
        }
        assert!(
            serde_json::from_value::<Anchor>(json!({"kind": "area", "selector": "h2", "area": {"x": 0, "y": 0, "w": 1, "h": 1, "children": -1}})).is_err(),
            "children is a count"
        );
    }

    #[test]
    fn area_anchors_round_trip_and_other_anchors_carry_no_area_field() {
        let a = area_anchor();
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["kind"], "area");
        assert_eq!(
            v["area"],
            json!({"x": 0.1, "y": 0.25, "w": 0.4213, "h": 0.1788})
        );
        assert_eq!(serde_json::from_value::<Anchor>(v).unwrap(), a);
        let e: Anchor =
            serde_json::from_value(json!({"kind": "element", "selector": "h2"})).unwrap();
        assert!(serde_json::to_value(&e).unwrap().get("area").is_none());
    }

    #[test]
    fn area_summary_names_the_element_and_the_share_it_covers() {
        let mut a = area_anchor();
        assert_eq!(
            a.summary(),
            "area in main > section:nth-of-type(2) (42% × 18%)"
        );
        a.file = "source.html".into();
        assert_eq!(
            a.summary(),
            "source.html › area in main > section:nth-of-type(2) (42% × 18%)"
        );
        a.area = Some(AnchorArea {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 0.004,
            ..Default::default()
        });
        assert!(a.summary().ends_with("(100% × <1%)"), "{}", a.summary());
    }

    #[test]
    fn artifact_anchors_name_no_page_and_summarise_as_the_whole_artifact() {
        let a: Anchor = serde_json::from_value(json!({"kind": "artifact"})).unwrap();
        assert_eq!(
            a.file, INDEX_FILE,
            "serde reads a missing file as the index"
        );
        a.validate()
            .expect_err("the index default is emptied before validation");
        let a = a.normalized();
        assert_eq!(a.file, WHOLE_FILE);
        a.validate().unwrap();
        assert_eq!(a.summary(), "the whole artifact");
        assert!(a.is_whole());
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["kind"], "artifact");
        assert_eq!(v["file"], "");
        assert_eq!(serde_json::from_value::<Anchor>(v).unwrap(), a);
        let explicit: Anchor =
            serde_json::from_value(json!({"kind": "artifact", "file": ""})).unwrap();
        explicit.validate().unwrap();
        let elsewhere: Anchor =
            serde_json::from_value(json!({"kind": "artifact", "file": "about.html"})).unwrap();
        assert!(
            elsewhere.normalized().validate().is_err(),
            "an artifact anchor is on no page"
        );
        for (name, extra) in [
            ("selector", json!({"selector": "h2"})),
            ("quote", json!({"quote": "Goals"})),
            ("prefix", json!({"prefix": "a"})),
            ("suffix", json!({"suffix": "b"})),
            ("html_hash", json!({"html_hash": "sha256:ab"})),
            ("custom_name", json!({"custom_name": "chart"})),
            ("route", json!({"route": "?a=1"})),
            (
                "rect",
                json!({"rect": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0, "scrollX": 0.0, "scrollY": 0.0, "viewportW": 1.0}}),
            ),
            (
                "area",
                json!({"area": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}}),
            ),
        ] {
            let mut v = json!({"kind": "artifact"});
            for (k, val) in extra.as_object().unwrap() {
                v[k] = val.clone();
            }
            let a: Anchor = serde_json::from_value(v).unwrap();
            assert!(
                matches!(
                    a.normalized().validate(),
                    Err(CoreError::Invalid {
                        code: "invalid_anchor",
                        ..
                    })
                ),
                "{name}"
            );
        }
        let e: Anchor =
            serde_json::from_value(json!({"kind": "element", "selector": "h2"})).unwrap();
        assert_eq!(e.clone().normalized(), e, "other anchors are unchanged");
    }
}
