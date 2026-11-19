//! The arguments of `clax.browser.test(target?, opts?)` and
//! `clax.browser.browse(url, opts?)` (spec §5.1, §5.2): [`BrowserTarget`],
//! [`BrowserOpts`], [`Step`] and [`Viewport`], parsed from the call's
//! JSON and checked against every cap, and the JavaScript each step
//! evaluates in the page.
//!
//! # Checks
//!
//! A value that fails its check is refused
//! [`api::CODE_BAD_ARGS`](crate::api::CODE_BAD_ARGS), the message naming
//! the field (`opts.steps[3]`, `opts.viewport.width`, …) and what it must
//! be. The caps (spec §5, the plan's Global Constraints): at most
//! [`MAX_STEPS`] steps, of which [`MAX_SCREENSHOTS`] screenshots; a
//! `wait` of at most [`MAX_WAIT_MS`]; a custom viewport 200–3,840 wide,
//! 200–4,000 high, scale 1–3; `tests` only under `test`, at most
//! [`MAX_TEST_FILES`] paths, each `tests/<name>.test.js`. A `timeout_s`
//! above the grant's per-test time is lowered to it. So that no frame
//! the driver sends is unbounded, a selector is at most
//! [`MAX_SELECTOR`] characters, and `type` text, an `eval` or `expect`
//! expression and a `goto` at most [`MAX_TEXT`].
//!
//! # The JavaScript of steps
//!
//! Each step that acts in the page evaluates one expression
//! (`Runtime.evaluate` with `awaitPromise` and `returnByValue`), built by
//! the functions here, each of which answers a JSON object: `click`'s
//! ([`click_js`]) the centre of the element after scrolling it into view
//! and whether `document.elementFromPoint` there is it or inside it
//! (else a description of what covers it); `wait`'s ([`wait_js`]) a
//! promise over a `MutationObserver` and animation frames, bounded in the
//! page by its own timer; `expect`'s ([`expect_js`]) the checks of spec
//! §5.2; `eval`'s ([`eval_js`]) the value as JSON text. Strings from the
//! script are embedded as JSON string literals; an expression is embedded
//! as written, inside parentheses and on lines of its own.

use super::BrowserError;
use crate::api::CODE_BAD_ARGS;
use serde_json::{Map, Value};

/// The most steps one call may have.
pub const MAX_STEPS: usize = 50;
/// The most screenshots one test may take.
pub const MAX_SCREENSHOTS: usize = 6;
/// The longest one `wait` step may wait, in milliseconds.
pub const MAX_WAIT_MS: u32 = 10_000;
/// The most saved test files one call may name.
pub const MAX_TEST_FILES: usize = 50;
/// The longest selector, in characters.
pub const MAX_SELECTOR: usize = 1_000;
/// The longest `type` text, expression or `goto`, in characters.
pub const MAX_TEXT: usize = 10_000;
/// The most bytes of an `eval` value as JSON.
pub const MAX_EVAL_BYTES: usize = 16 * 1024;

/// Which operation the arguments are of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Test,
    Browse,
}

/// What a test loads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserTarget {
    /// A published version of the run's artifact; the run's draft when
    /// `None`.
    pub version: Option<u32>,
    /// The page to open, a path of the artifact: `index.html` by default.
    pub path: String,
}

/// The browser's window.
#[derive(Clone, Debug, PartialEq)]
pub struct Viewport {
    /// `desktop`, `tablet` or `phone`; `None` for a custom one.
    pub preset: Option<&'static str>,
    pub width: u32,
    pub height: u32,
    pub mobile: bool,
    pub scale: f64,
}

impl Viewport {
    /// 1280×800 at scale 1, the default.
    #[must_use]
    pub fn desktop() -> Viewport {
        Viewport {
            preset: Some("desktop"),
            width: 1280,
            height: 800,
            mobile: false,
            scale: 1.0,
        }
    }

    /// 820×1180 at scale 2, mobile, touch.
    #[must_use]
    pub fn tablet() -> Viewport {
        Viewport {
            preset: Some("tablet"),
            width: 820,
            height: 1180,
            mobile: true,
            scale: 2.0,
        }
    }

    /// 390×844 at scale 3, mobile, touch.
    #[must_use]
    pub fn phone() -> Viewport {
        Viewport {
            preset: Some("phone"),
            width: 390,
            height: 844,
            mobile: true,
            scale: 3.0,
        }
    }

    /// Its name, or `<width>x<height>` for a custom one.
    #[must_use]
    pub fn label(&self) -> String {
        self.preset
            .map_or_else(|| format!("{}x{}", self.width, self.height), str::to_string)
    }
}

/// Where a `scroll` step scrolls to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollTo {
    Top,
    Bottom,
}

/// What a `wait` step waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitFor {
    /// That many milliseconds.
    Ms(u32),
    /// An element: present (and rendered, with `visible`), or gone with
    /// `gone`; at most `ms`.
    Selector {
        selector: String,
        visible: bool,
        gone: bool,
        ms: u32,
    },
}

/// The checks of an `expect` step (spec §5.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expect {
    pub selector: Option<String>,
    pub text: Option<String>,
    pub visible: Option<bool>,
    pub count: Option<u32>,
    pub eval: Option<String>,
}

/// One step of a call (spec §5.2).
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// `test`: a path of the artifact; `browse`: a URL.
    Goto(String),
    Click(String),
    Type {
        selector: String,
        text: String,
    },
    Press(Key),
    Scroll {
        selector: Option<String>,
        y: Option<i64>,
        to: Option<ScrollTo>,
    },
    Wait(WaitFor),
    Eval(String),
    Expect(Expect),
    ExpectNoErrors,
    Screenshot {
        full_page: bool,
        selector: Option<String>,
    },
}

impl Step {
    /// Whether it is an assertion (`expect`, `expectNoErrors`), whose
    /// failure does not end the list.
    #[must_use]
    pub fn is_assertion(&self) -> bool {
        matches!(self, Step::Expect(_) | Step::ExpectNoErrors)
    }

    /// Its name as a script writes it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Step::Goto(_) => "goto",
            Step::Click(_) => "click",
            Step::Type { .. } => "type",
            Step::Press(_) => "press",
            Step::Scroll { .. } => "scroll",
            Step::Wait(_) => "wait",
            Step::Eval(_) => "eval",
            Step::Expect(_) => "expect",
            Step::ExpectNoErrors => "expectNoErrors",
            Step::Screenshot { .. } => "screenshot",
        }
    }
}

/// Which saved tests to run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Tests {
    #[default]
    None,
    /// Every `tests/*.test.js` of the target.
    All,
    /// These paths.
    Paths(Vec<String>),
}

/// A call's options, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserOpts {
    pub viewport: Viewport,
    pub steps: Vec<Step>,
    pub tests: Tests,
    pub html: bool,
    pub text: bool,
    /// The call's time cap, in seconds: its `timeout_s`, at most the
    /// grant's per-test time, which it is when absent.
    pub timeout_s: u32,
}

/// One key `press` sends (`Input.dispatchKeyEvent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// As the script named it.
    pub name: String,
    /// The DOM `key`.
    pub key: String,
    /// The DOM `code`.
    pub code: String,
    /// `windowsVirtualKeyCode`.
    pub key_code: u32,
    /// The text it types, if any.
    pub text: Option<String>,
}

/// The key `name` names: one of the named keys (`Enter`, `Tab`,
/// `Escape`, `Backspace`, `Delete`, `Space`, the arrows, `Home`, `End`,
/// `PageUp`, `PageDown`) or one printable character.
#[must_use]
pub fn key(name: &str) -> Option<Key> {
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| Key {
        name: name.to_string(),
        key: key.to_string(),
        code: code.to_string(),
        key_code,
        text: text.map(str::to_string),
    };
    let k = match name {
        "Enter" => named("Enter", "Enter", 13, Some("\r")),
        "Tab" => named("Tab", "Tab", 9, None),
        "Escape" => named("Escape", "Escape", 27, None),
        "Backspace" => named("Backspace", "Backspace", 8, None),
        "Delete" => named("Delete", "Delete", 46, None),
        "Space" | " " => named(" ", "Space", 32, Some(" ")),
        "ArrowUp" => named("ArrowUp", "ArrowUp", 38, None),
        "ArrowDown" => named("ArrowDown", "ArrowDown", 40, None),
        "ArrowLeft" => named("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" => named("ArrowRight", "ArrowRight", 39, None),
        "Home" => named("Home", "Home", 36, None),
        "End" => named("End", "End", 35, None),
        "PageUp" => named("PageUp", "PageUp", 33, None),
        "PageDown" => named("PageDown", "PageDown", 34, None),
        _ => {
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() || c.is_control() {
                return None;
            }
            let upper = c.to_ascii_uppercase();
            let (code, key_code) = if c.is_ascii_alphabetic() {
                (format!("Key{upper}"), u32::from(upper))
            } else if c.is_ascii_digit() {
                (format!("Digit{c}"), u32::from(c))
            } else {
                (String::new(), 0)
            };
            let text = c.to_string();
            Key {
                name: name.to_string(),
                key: text.clone(),
                code,
                key_code,
                text: Some(text),
            }
        }
    };
    Some(k)
}

fn bad(field: &str, what: impl std::fmt::Display) -> BrowserError {
    BrowserError::new(CODE_BAD_ARGS, format!("{field}: {what}"), false)
}

/// `v` as an object, `null` as an empty one.
fn object<'a>(v: &'a Value, field: &str) -> Result<Option<&'a Map<String, Value>>, BrowserError> {
    match v {
        Value::Null => Ok(None),
        Value::Object(o) => Ok(Some(o)),
        _ => Err(bad(field, "must be an object")),
    }
}

/// Refuses a member of `o` not among `known`.
fn only(o: &Map<String, Value>, field: &str, known: &[&str]) -> Result<(), BrowserError> {
    match o.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(bad(
            &format!("{field}.{k}"),
            format!("is not an option (one of {})", known.join(", ")),
        )),
        None => Ok(()),
    }
}

fn int(v: &Value, field: &str, min: i64, max: i64) -> Result<i64, BrowserError> {
    match v.as_i64() {
        Some(n) if (min..=max).contains(&n) && !v.is_f64() => Ok(n),
        _ => Err(bad(
            field,
            format!("must be a whole number from {min} to {max}"),
        )),
    }
}

fn boolean(v: Option<&Value>, field: &str) -> Result<bool, BrowserError> {
    match v {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(field, "must be true or false")),
    }
}

fn string(v: &Value, field: &str, max: usize) -> Result<String, BrowserError> {
    match v.as_str() {
        Some("") => Err(bad(field, "must not be empty")),
        Some(s) if s.chars().count() > max => {
            Err(bad(field, format!("must be at most {max} characters")))
        }
        Some(s) => Ok(s.to_string()),
        None => Err(bad(field, "must be a string")),
    }
}

fn opt_string(v: Option<&Value>, field: &str, max: usize) -> Result<Option<String>, BrowserError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => string(v, field, max).map(Some),
    }
}

/// A path of the artifact a test may open: relative or from `/`, no
/// scheme, no `..`.
fn artifact_path(v: &Value, field: &str) -> Result<String, BrowserError> {
    let s = string(v, field, MAX_TEXT)?;
    let has_scheme = s
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.is_empty() && !scheme.contains('/'));
    if has_scheme || s.starts_with("//") || s.contains('\\') {
        return Err(bad(
            field,
            "must be a path of the artifact (`about.html`, `/a/b.html`), not a URL",
        ));
    }
    if s.split(['/', '?', '#']).any(|seg| seg == "..") {
        return Err(bad(field, "must not leave the artifact (`..`)"));
    }
    Ok(s)
}

/// The target of `clax.browser.test`: `v` is its first argument (`null`
/// or absent for the draft's `index.html`).
///
/// # Errors
/// [`CODE_BAD_ARGS`], naming the field.
pub fn parse_target(v: &Value) -> Result<BrowserTarget, BrowserError> {
    let Some(o) = object(v, "target")? else {
        return Ok(BrowserTarget {
            version: None,
            path: clax_core::publish::INDEX.into(),
        });
    };
    only(o, "target", &["version", "path"])?;
    let version = match o.get("version") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            u32::try_from(int(v, "target.version", 1, i64::from(u32::MAX))?)
                .map_err(|_| bad("target.version", "is too large"))?,
        ),
    };
    let path = match o.get("path") {
        None | Some(Value::Null) => clax_core::publish::INDEX.into(),
        Some(v) => artifact_path(v, "target.path")?
            .trim_start_matches('/')
            .to_string(),
    };
    let path = if path.is_empty() {
        clax_core::publish::INDEX.into()
    } else {
        path
    };
    Ok(BrowserTarget { version, path })
}

fn viewport(v: &Value) -> Result<Viewport, BrowserError> {
    const F: &str = "opts.viewport";
    match v {
        Value::Null => return Ok(Viewport::desktop()),
        Value::String(s) => {
            return match s.as_str() {
                "desktop" => Ok(Viewport::desktop()),
                "tablet" => Ok(Viewport::tablet()),
                "phone" => Ok(Viewport::phone()),
                other => Err(bad(
                    F,
                    format!(
                        "`{other}` is not a viewport (desktop, tablet, phone, or {{width, height}})"
                    ),
                )),
            };
        }
        _ => {}
    }
    let o = object(v, F)?.ok_or_else(|| bad(F, "must be a name or an object"))?;
    only(o, F, &["width", "height", "mobile", "scale"])?;
    let dim = |name: &str, max: i64| -> Result<u32, BrowserError> {
        let field = format!("{F}.{name}");
        let v = o.get(name).ok_or_else(|| bad(&field, "is required"))?;
        Ok(u32::try_from(int(v, &field, 200, max)?).unwrap_or(200))
    };
    let width = dim("width", 3840)?;
    let height = dim("height", 4000)?;
    let mobile = boolean(o.get("mobile"), &format!("{F}.mobile"))?;
    let scale = match o.get("scale") {
        None | Some(Value::Null) => 1.0,
        Some(v) => match v.as_f64() {
            Some(s) if (1.0..=3.0).contains(&s) => s,
            _ => return Err(bad(&format!("{F}.scale"), "must be a number from 1 to 3")),
        },
    };
    Ok(Viewport {
        preset: None,
        width,
        height,
        mobile,
        scale,
    })
}

fn step(v: &Value, field: &str, mode: Mode) -> Result<Step, BrowserError> {
    let o = match v {
        Value::Object(o) if o.len() == 1 => o,
        Value::Object(o) if o.len() > 1 => {
            return Err(bad(field, "a step has exactly one member, its name"));
        }
        _ => {
            return Err(bad(
                field,
                "must be an object naming one step, like {click: \"#go\"}",
            ));
        }
    };
    let (name, arg) = o.iter().next().expect("one member");
    let at = format!("{field}.{name}");
    let sel = |v: &Value, f: &str| string(v, f, MAX_SELECTOR);
    Ok(match name.as_str() {
        "goto" => Step::Goto(match mode {
            Mode::Test => artifact_path(arg, &at)?,
            Mode::Browse => string(arg, &at, MAX_TEXT)?,
        }),
        "click" => Step::Click(sel(arg, &at)?),
        "type" => {
            let o = object(arg, &at)?.ok_or_else(|| bad(&at, "must be {selector, text}"))?;
            only(o, &at, &["selector", "text"])?;
            let selector = sel(
                o.get("selector").unwrap_or(&Value::Null),
                &format!("{at}.selector"),
            )?;
            let text = match o.get("text") {
                Some(Value::String(s)) if s.chars().count() <= MAX_TEXT => s.clone(),
                Some(Value::String(_)) => {
                    return Err(bad(
                        &format!("{at}.text"),
                        format!("must be at most {MAX_TEXT} characters"),
                    ));
                }
                _ => return Err(bad(&format!("{at}.text"), "must be a string")),
            };
            Step::Type { selector, text }
        }
        "press" => {
            let name = string(arg, &at, 32)?;
            Step::Press(key(&name).ok_or_else(|| {
                bad(
                    &at,
                    format!("`{name}` is not a key (Enter, Tab, Escape, Backspace, Delete, Space, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, Home, End, PageUp, PageDown, or one character)"),
                )
            })?)
        }
        "scroll" => {
            let o = object(arg, &at)?.ok_or_else(|| bad(&at, "must be {selector?, y?, to?}"))?;
            only(o, &at, &["selector", "y", "to"])?;
            let selector = opt_string(o.get("selector"), &format!("{at}.selector"), MAX_SELECTOR)?;
            let y = match o.get("y") {
                None | Some(Value::Null) => None,
                Some(v) => Some(int(v, &format!("{at}.y"), -1_000_000, 1_000_000)?),
            };
            let to = match o
                .get("to")
                .and_then(|v| if v.is_null() { None } else { Some(v) })
            {
                None => None,
                Some(v) => Some(match v.as_str() {
                    Some("top") => ScrollTo::Top,
                    Some("bottom") => ScrollTo::Bottom,
                    _ => return Err(bad(&format!("{at}.to"), "must be \"top\" or \"bottom\"")),
                }),
            };
            if selector.is_none() && y.is_none() && to.is_none() {
                return Err(bad(&at, "names a selector, a y or a to"));
            }
            Step::Scroll { selector, y, to }
        }
        "wait" => Step::Wait(match arg {
            Value::Object(o) => {
                only(o, &at, &["selector", "visible", "gone", "ms"])?;
                let ms = match o.get("ms") {
                    None | Some(Value::Null) => MAX_WAIT_MS,
                    Some(v) => {
                        u32::try_from(int(v, &format!("{at}.ms"), 0, i64::from(MAX_WAIT_MS))?)
                            .unwrap_or(MAX_WAIT_MS)
                    }
                };
                let visible = boolean(o.get("visible"), &format!("{at}.visible"))?;
                let gone = boolean(o.get("gone"), &format!("{at}.gone"))?;
                if visible && gone {
                    return Err(bad(&at, "waits for visible or gone, not both"));
                }
                WaitFor::Selector {
                    selector: sel(
                        o.get("selector").unwrap_or(&Value::Null),
                        &format!("{at}.selector"),
                    )?,
                    visible,
                    gone,
                    ms,
                }
            }
            v => WaitFor::Ms(
                u32::try_from(int(v, &at, 0, i64::from(MAX_WAIT_MS))?).unwrap_or(MAX_WAIT_MS),
            ),
        }),
        "eval" => Step::Eval(string(arg, &at, MAX_TEXT)?),
        "expect" => {
            let o = object(arg, &at)?.ok_or_else(|| bad(&at, "must be an object of checks"))?;
            only(o, &at, &["selector", "text", "visible", "count", "eval"])?;
            let e = Expect {
                selector: opt_string(o.get("selector"), &format!("{at}.selector"), MAX_SELECTOR)?,
                text: match o.get("text") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if s.chars().count() <= MAX_TEXT => Some(s.clone()),
                    Some(_) => {
                        return Err(bad(
                            &format!("{at}.text"),
                            format!("must be a string of at most {MAX_TEXT} characters"),
                        ));
                    }
                },
                visible: match o.get("visible") {
                    None | Some(Value::Null) => None,
                    v => Some(boolean(v, &format!("{at}.visible"))?),
                },
                count: match o.get("count") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        u32::try_from(int(v, &format!("{at}.count"), 0, 1_000_000)?).unwrap_or(0),
                    ),
                },
                eval: opt_string(o.get("eval"), &format!("{at}.eval"), MAX_TEXT)?,
            };
            if e.selector.is_none() && e.eval.is_none() {
                return Err(bad(&at, "names a selector or an eval"));
            }
            if e.selector.is_none()
                && (e.text.is_some() || e.visible.is_some() || e.count.is_some())
            {
                return Err(bad(&at, "text, visible and count need a selector"));
            }
            Step::Expect(e)
        }
        "expectNoErrors" => match arg {
            Value::Bool(true) => Step::ExpectNoErrors,
            _ => return Err(bad(&at, "must be true")),
        },
        "screenshot" => {
            let o = match arg {
                Value::Bool(true) | Value::Null => None,
                v => object(v, &at)?,
            };
            match o {
                None => Step::Screenshot {
                    full_page: false,
                    selector: None,
                },
                Some(o) => {
                    only(o, &at, &["fullPage", "selector"])?;
                    let full_page = boolean(o.get("fullPage"), &format!("{at}.fullPage"))?;
                    let selector =
                        opt_string(o.get("selector"), &format!("{at}.selector"), MAX_SELECTOR)?;
                    if full_page && selector.is_some() {
                        return Err(bad(&at, "takes the full page or an element, not both"));
                    }
                    Step::Screenshot {
                        full_page,
                        selector,
                    }
                }
            }
        }
        other => {
            return Err(bad(
                field,
                format!(
                    "`{other}` is not a step (goto, click, type, press, scroll, wait, eval, expect, expectNoErrors, screenshot)"
                ),
            ));
        }
    })
}

/// Whether `path` names a saved test file: `tests/<name>.test.js`, its
/// name not empty and holding no `..` (it may hold `/`, as the Clax
/// app's inspect view allows: `web/shell/src/app/inspect.ts`, `TEST_FILE`).
#[must_use]
pub fn is_test_file(path: &str) -> bool {
    path.strip_prefix("tests/")
        .and_then(|n| n.strip_suffix(".test.js"))
        .is_some_and(|n| !n.is_empty() && !n.contains(".."))
}

fn tests(v: Option<&Value>, mode: Mode) -> Result<Tests, BrowserError> {
    const F: &str = "opts.tests";
    let t = match v {
        None | Some(Value::Null | Value::Bool(false)) => return Ok(Tests::None),
        Some(Value::Bool(true)) => Tests::All,
        Some(Value::Array(paths)) => {
            if paths.len() > MAX_TEST_FILES {
                return Err(bad(F, format!("names at most {MAX_TEST_FILES} files")));
            }
            let mut out = Vec::with_capacity(paths.len());
            for (i, p) in paths.iter().enumerate() {
                let field = format!("{F}[{i}]");
                let s = string(p, &field, 1_000)?;
                if !is_test_file(&s) {
                    return Err(bad(&field, "must be a path tests/<name>.test.js"));
                }
                out.push(s);
            }
            Tests::Paths(out)
        }
        Some(_) => {
            return Err(bad(
                F,
                "must be true, false or a list of tests/<name>.test.js paths",
            ));
        }
    };
    if mode == Mode::Browse {
        return Err(bad(
            F,
            "saved tests run under clax.browser.test only, not browse",
        ));
    }
    Ok(t)
}

/// The options of a call of `mode`: `v` is its `opts` argument (`null` or
/// absent for none), `grant_seconds` the grant's per-test time.
///
/// # Errors
/// [`CODE_BAD_ARGS`], naming the field.
pub fn parse_opts(v: &Value, mode: Mode, grant_seconds: u32) -> Result<BrowserOpts, BrowserError> {
    let empty = Map::new();
    let o = object(v, "opts")?.unwrap_or(&empty);
    only(
        o,
        "opts",
        &["viewport", "steps", "tests", "html", "text", "timeout_s"],
    )?;
    let viewport = viewport(o.get("viewport").unwrap_or(&Value::Null))?;
    let steps = match o.get("steps") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(steps)) => {
            if steps.len() > MAX_STEPS {
                return Err(bad(
                    "opts.steps",
                    format!("has {} steps; at most {MAX_STEPS}", steps.len()),
                ));
            }
            steps
                .iter()
                .enumerate()
                .map(|(i, s)| step(s, &format!("opts.steps[{i}]"), mode))
                .collect::<Result<Vec<_>, _>>()?
        }
        Some(_) => return Err(bad("opts.steps", "must be a list of steps")),
    };
    let shots = steps
        .iter()
        .filter(|s| matches!(s, Step::Screenshot { .. }))
        .count();
    if shots > MAX_SCREENSHOTS {
        return Err(bad(
            "opts.steps",
            format!("takes {shots} screenshots; at most {MAX_SCREENSHOTS} a test"),
        ));
    }
    let timeout_s = match o.get("timeout_s") {
        None | Some(Value::Null) => grant_seconds,
        Some(v) => {
            let n = int(v, "opts.timeout_s", 1, i64::from(u32::MAX))?;
            u32::try_from(n).unwrap_or(u32::MAX).min(grant_seconds)
        }
    };
    Ok(BrowserOpts {
        viewport,
        steps,
        tests: tests(o.get("tests"), mode)?,
        html: boolean(o.get("html"), "opts.html")?,
        text: boolean(o.get("text"), "opts.text")?,
        timeout_s,
    })
}

/// A string as a JavaScript string literal.
fn lit(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// The page's description of an element, `describe(el)`: its tag, ID and
/// first classes, as a selector would name it.
const DESCRIBE: &str = "const describe = (el) => { if (!el) return 'nothing'; let d = el.tagName.toLowerCase(); if (el.id) d += '#' + el.id; if (typeof el.className === 'string' && el.className.trim()) d += '.' + el.className.trim().split(/\\s+/).slice(0, 2).join('.'); return d; };";

/// The expression `click` evaluates (see the module docs): `{found,
/// x, y, ok, top}`.
#[must_use]
pub fn click_js(selector: &str) -> String {
    format!(
        "(() => {{ {DESCRIBE} const el = document.querySelector({sel}); if (!el) return {{found: false}}; el.scrollIntoView({{block: 'center', inline: 'center', behavior: 'instant'}}); const r = el.getBoundingClientRect(); if (r.width === 0 || r.height === 0) return {{found: true, ok: false, top: 'it has no size (hidden or not rendered)'}}; const x = r.left + r.width / 2, y = r.top + r.height / 2; const hit = document.elementFromPoint(x, y); return {{found: true, x, y, ok: hit === el || el.contains(hit), top: describe(hit)}}; }})()",
        sel = lit(selector)
    )
}

/// The expression that focuses the element `type` types into: `{found}`.
#[must_use]
pub fn focus_js(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return {{found: false}}; el.scrollIntoView({{block: 'center', behavior: 'instant'}}); el.focus(); return {{found: true, focused: document.activeElement === el}}; }})()",
        sel = lit(selector)
    )
}

/// The expression `scroll` evaluates: `{found}`.
#[must_use]
pub fn scroll_js(selector: Option<&str>, y: Option<i64>, to: Option<ScrollTo>) -> String {
    let target = selector.map_or_else(
        || "document.scrollingElement || document.documentElement".to_string(),
        |s| format!("document.querySelector({})", lit(s)),
    );
    let top = match (to, y) {
        (Some(ScrollTo::Top), _) => "0".to_string(),
        (Some(ScrollTo::Bottom), _) => "el.scrollHeight".to_string(),
        (None, Some(y)) => format!("el.scrollTop + ({y})"),
        (None, None) => "el.scrollTop".to_string(),
    };
    let into_view = if selector.is_some() && y.is_none() && to.is_none() {
        "el.scrollIntoView({block: 'center', behavior: 'instant'});"
    } else {
        ""
    };
    format!(
        "(() => {{ const el = {target}; if (!el) return {{found: false}}; {into_view} el.scrollTo({{top: {top}, behavior: 'instant'}}); return {{found: true}}; }})()"
    )
}

/// The expression `wait` evaluates (see the module docs): a promise of
/// `{ok}`, with `error` when the time ran out.
#[must_use]
pub fn wait_js(w: &WaitFor) -> String {
    match w {
        WaitFor::Ms(ms) => {
            format!("new Promise((resolve) => setTimeout(() => resolve({{ok: true}}), {ms}))")
        }
        WaitFor::Selector {
            selector,
            visible,
            gone,
            ms,
        } => {
            let sel = lit(selector);
            let check = if *gone {
                format!("!document.querySelector({sel})")
            } else if *visible {
                format!(
                    "(() => {{ const el = document.querySelector({sel}); if (!el) return false; const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; }})()"
                )
            } else {
                format!("!!document.querySelector({sel})")
            };
            let what = if *gone {
                "gone"
            } else if *visible {
                "visible"
            } else {
                "present"
            };
            format!(
                "new Promise((resolve) => {{ const check = () => {check}; if (check()) return resolve({{ok: true}}); let done = false; const finish = (v) => {{ if (done) return; done = true; obs.disconnect(); clearTimeout(timer); resolve(v); }}; const obs = new MutationObserver(() => {{ if (check()) finish({{ok: true}}); }}); obs.observe(document.documentElement, {{subtree: true, childList: true, attributes: true, characterData: true}}); const frame = () => {{ if (done) return; if (check()) return finish({{ok: true}}); requestAnimationFrame(frame); }}; requestAnimationFrame(frame); const timer = setTimeout(() => finish({{ok: false, error: {err}}}), {ms}); }})",
                err = lit(&format!("{selector} was not {what} after {ms} ms")),
            )
        }
    }
}

/// The expression `expect` evaluates (see the module docs): a promise of
/// `{ok}`, with `error` naming the first check that failed.
#[must_use]
pub fn expect_js(e: &Expect) -> String {
    let mut checks = String::new();
    if let Some(sel) = &e.selector {
        let s = lit(sel);
        checks.push_str(&format!(
            "const all = document.querySelectorAll({s}); const el = all[0];"
        ));
        match e.count {
            Some(n) => checks.push_str(&format!(
                "if (all.length !== {n}) return {{ok: false, error: {s} + ' matched ' + all.length + ', not {n}'}};"
            )),
            None => checks.push_str(&format!(
                "if (!el) return {{ok: false, error: {s} + ' matched nothing'}};"
            )),
        }
        if let Some(v) = e.visible {
            checks.push_str(&format!(
                "{{ const r = el ? el.getBoundingClientRect() : null; const st = el ? getComputedStyle(el) : null; const shown = !!el && r.width > 0 && r.height > 0 && st.visibility !== 'hidden' && st.display !== 'none'; if (shown !== {v}) return {{ok: false, error: {s} + (shown ? ' is visible' : ' is not visible')}}; }}"
            ));
        }
        if let Some(t) = &e.text {
            checks.push_str(&format!(
                "{{ const got = el ? el.innerText : ''; if (!got.includes({t})) return {{ok: false, error: {s} + ' text ' + JSON.stringify(got.slice(0, 200)) + ' does not contain ' + {t}}}; }}",
                t = lit(t)
            ));
        }
    }
    if let Some(ev) = &e.eval {
        checks.push_str(&format!(
            "{{ const v = await (\n{ev}\n); if (!v) return {{ok: false, error: 'eval was ' + String(v)}}; }}"
        ));
    }
    format!("(async () => {{ {checks} return {{ok: true}}; }})()")
}

/// The expression `eval` evaluates (see the module docs): a promise of
/// `{json}` (the value as JSON text, `null` for `undefined`), or
/// `{tooLong}` past [`MAX_EVAL_BYTES`].
#[must_use]
pub fn eval_js(expression: &str) -> String {
    format!(
        "(async () => {{ const v = await (\n{expression}\n); const s = JSON.stringify(v); if (s === undefined) return {{json: 'null'}}; if (new TextEncoder().encode(s).length > {MAX_EVAL_BYTES}) return {{tooLong: true}}; return {{json: s}}; }})()"
    )
}

/// The expression that waits two animation frames: the page settled.
pub const SETTLE_JS: &str = "new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))";

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(v: Value) -> Result<BrowserOpts, BrowserError> {
        parse_opts(&v, Mode::Test, 60)
    }

    fn refused(v: Value) -> String {
        let e = opts(v).unwrap_err();
        assert_eq!(e.code, CODE_BAD_ARGS);
        assert!(!e.retryable);
        e.message
    }

    #[test]
    fn the_defaults_are_the_drafts_index_on_a_desktop() {
        assert_eq!(
            parse_target(&Value::Null).unwrap(),
            BrowserTarget {
                version: None,
                path: "index.html".into()
            }
        );
        assert_eq!(
            parse_target(&json!({"version": 3, "path": "about.html"})).unwrap(),
            BrowserTarget {
                version: Some(3),
                path: "about.html".into()
            }
        );
        for bad in [
            json!("index.html"),
            json!({"version": 0}),
            json!({"version": -1}),
            json!({"version": "2"}),
            json!({"path": ""}),
            json!({"path": "https://elsewhere.example/"}),
            json!({"path": "../x.html"}),
            json!({"page": "x.html"}),
        ] {
            let e = parse_target(&bad).unwrap_err();
            assert_eq!(e.code, CODE_BAD_ARGS, "{bad}");
            assert!(e.message.starts_with("target"), "{}", e.message);
        }
        let o = opts(Value::Null).unwrap();
        assert_eq!(o.viewport, Viewport::desktop());
        assert!(o.steps.is_empty());
        assert_eq!(o.tests, Tests::None);
        assert!(!o.html && !o.text);
        assert_eq!(o.timeout_s, 60);
    }

    #[test]
    fn viewport_presets_have_their_sizes() {
        let vp = |name: &str| opts(json!({"viewport": name})).unwrap().viewport;
        let d = vp("desktop");
        assert_eq!(
            (d.width, d.height, d.mobile, d.scale),
            (1280, 800, false, 1.0)
        );
        let t = vp("tablet");
        assert_eq!(
            (t.width, t.height, t.mobile, t.scale),
            (820, 1180, true, 2.0)
        );
        let p = vp("phone");
        assert_eq!(
            (p.width, p.height, p.mobile, p.scale),
            (390, 844, true, 3.0)
        );
        assert_eq!(p.label(), "phone");
        let m = refused(json!({"viewport": "watch"}));
        assert!(m.contains("opts.viewport") && m.contains("desktop"), "{m}");
    }

    #[test]
    fn a_custom_viewport_is_bounded() {
        let v = opts(json!({"viewport": {"width": 1024, "height": 768}}))
            .unwrap()
            .viewport;
        assert_eq!(
            v,
            Viewport {
                preset: None,
                width: 1024,
                height: 768,
                mobile: false,
                scale: 1.0
            }
        );
        assert_eq!(v.label(), "1024x768");
        let v =
            opts(json!({"viewport": {"width": 200, "height": 4000, "mobile": true, "scale": 2.5}}))
                .unwrap()
                .viewport;
        assert_eq!(
            (v.width, v.height, v.mobile, v.scale),
            (200, 4000, true, 2.5)
        );
        for (vp, field) in [
            (json!({"width": 199, "height": 800}), "opts.viewport.width"),
            (json!({"width": 3841, "height": 800}), "opts.viewport.width"),
            (
                json!({"width": 800, "height": 4001}),
                "opts.viewport.height",
            ),
            (json!({"width": 800}), "opts.viewport.height"),
            (
                json!({"width": 800, "height": 800, "scale": 0.5}),
                "opts.viewport.scale",
            ),
            (
                json!({"width": 800, "height": 800, "scale": 4}),
                "opts.viewport.scale",
            ),
            (
                json!({"width": 800, "height": 800, "mobile": "yes"}),
                "opts.viewport.mobile",
            ),
            (
                json!({"width": 800, "height": 800, "depth": 2}),
                "opts.viewport.depth",
            ),
        ] {
            let m = refused(json!({"viewport": vp}));
            assert!(m.starts_with(field), "{m} should name {field}");
        }
    }

    #[test]
    fn too_many_steps_is_invalid_args() {
        let steps: Vec<Value> = (0..=MAX_STEPS).map(|_| json!({"wait": 1})).collect();
        let m = refused(json!({"steps": steps}));
        assert!(m.starts_with("opts.steps") && m.contains("50"), "{m}");
        let steps: Vec<Value> = (0..MAX_STEPS).map(|_| json!({"wait": 1})).collect();
        assert_eq!(opts(json!({"steps": steps})).unwrap().steps.len(), 50);
        let shots: Vec<Value> = (0..=MAX_SCREENSHOTS)
            .map(|_| json!({"screenshot": {}}))
            .collect();
        let m = refused(json!({"steps": shots}));
        assert!(m.contains("6") && m.contains("screenshot"), "{m}");
        let m = refused(json!({"steps": [{"wait": 10_001}]}));
        assert!(m.starts_with("opts.steps[0].wait"), "{m}");
        let m = refused(json!({"steps": [{"wait": {"selector": "#a", "ms": 10_001}}]}));
        assert!(m.starts_with("opts.steps[0].wait.ms"), "{m}");
        let long = "a".repeat(MAX_SELECTOR + 1);
        let m = refused(json!({"steps": [{"click": long}]}));
        assert!(m.starts_with("opts.steps[0].click"), "{m}");
    }

    #[test]
    fn every_step_parses() {
        let o = opts(json!({"steps": [
            {"goto": "about.html"},
            {"click": "#go"},
            {"type": {"selector": "input", "text": "hi"}},
            {"press": "Enter"},
            {"scroll": {"to": "bottom"}},
            {"scroll": {"selector": "#list", "y": 200}},
            {"wait": 250},
            {"wait": {"selector": ".done", "visible": true}},
            {"eval": "document.title"},
            {"expect": {"selector": "li", "count": 3, "text": "milk"}},
            {"expectNoErrors": true},
            {"screenshot": {"fullPage": true}},
            {"screenshot": {}},
        ]}))
        .unwrap();
        let names: Vec<&str> = o.steps.iter().map(Step::name).collect();
        assert_eq!(
            names,
            [
                "goto",
                "click",
                "type",
                "press",
                "scroll",
                "scroll",
                "wait",
                "wait",
                "eval",
                "expect",
                "expectNoErrors",
                "screenshot",
                "screenshot"
            ]
        );
        assert_eq!(
            o.steps[7],
            Step::Wait(WaitFor::Selector {
                selector: ".done".into(),
                visible: true,
                gone: false,
                ms: MAX_WAIT_MS
            })
        );
        assert_eq!(
            o.steps[11],
            Step::Screenshot {
                full_page: true,
                selector: None
            }
        );
        let asserts: Vec<bool> = o.steps.iter().map(Step::is_assertion).collect();
        assert_eq!(asserts.iter().filter(|a| **a).count(), 2);
        assert!(o.steps[9].is_assertion() && o.steps[10].is_assertion());
        // A test's goto is a path of the artifact, not a URL.
        let m = refused(json!({"steps": [{"goto": "https://elsewhere.example/"}]}));
        assert!(m.starts_with("opts.steps[0].goto"), "{m}");
        // An expect checks something.
        let m = refused(json!({"steps": [{"expect": {}}]}));
        assert!(m.starts_with("opts.steps[0].expect"), "{m}");
    }

    #[test]
    fn an_unknown_step_names_itself() {
        let m = refused(json!({"steps": [{"click": "#a"}, {"hover": "#b"}]}));
        assert!(m.starts_with("opts.steps[1]") && m.contains("hover"), "{m}");
        let m = refused(json!({"steps": [{"click": "#a", "wait": 1}]}));
        assert!(m.starts_with("opts.steps[0]") && m.contains("one"), "{m}");
        let m = refused(json!({"steps": ["click"]}));
        assert!(m.starts_with("opts.steps[0]"), "{m}");
        let m = refused(json!({"steps": [{"press": "Hyper"}]}));
        assert!(
            m.starts_with("opts.steps[0].press") && m.contains("Hyper"),
            "{m}"
        );
        let m = refused(json!({"colour": "red"}));
        assert!(m.starts_with("opts.colour"), "{m}");
    }

    #[test]
    fn tests_are_refused_under_browse() {
        let o = opts(json!({"tests": true})).unwrap();
        assert_eq!(o.tests, Tests::All);
        let o = opts(json!({"tests": ["tests/form.test.js"]})).unwrap();
        assert_eq!(o.tests, Tests::Paths(vec!["tests/form.test.js".into()]));
        assert_eq!(opts(json!({"tests": false})).unwrap().tests, Tests::None);
        for bad in [
            json!(["form.test.js"]),
            json!(["tests/form.js"]),
            json!("all"),
        ] {
            let m = refused(json!({"tests": bad}));
            assert!(m.starts_with("opts.tests"), "{m}");
        }
        let many: Vec<String> = (0..=MAX_TEST_FILES)
            .map(|i| format!("tests/t{i}.test.js"))
            .collect();
        assert!(refused(json!({"tests": many})).contains("50"));
        let e = parse_opts(&json!({"tests": true}), Mode::Browse, 60).unwrap_err();
        assert_eq!(e.code, CODE_BAD_ARGS);
        assert!(e.message.starts_with("opts.tests"), "{}", e.message);
        // Browse takes no tests, but a goto there is a URL.
        let o = parse_opts(
            &json!({"steps": [{"goto": "https://example.com/next"}]}),
            Mode::Browse,
            60,
        )
        .unwrap();
        assert_eq!(o.steps[0], Step::Goto("https://example.com/next".into()));
    }

    #[test]
    fn timeout_s_is_capped_by_the_grant() {
        assert_eq!(opts(json!({"timeout_s": 30})).unwrap().timeout_s, 30);
        assert_eq!(opts(json!({"timeout_s": 600})).unwrap().timeout_s, 60);
        assert_eq!(
            parse_opts(&json!({}), Mode::Test, 15).unwrap().timeout_s,
            15
        );
        for bad in [json!(0), json!(-5), json!(2.5), json!("30")] {
            let m = refused(json!({"timeout_s": bad}));
            assert!(m.starts_with("opts.timeout_s"), "{m}");
        }
    }

    #[test]
    fn press_knows_enter_tab_escape_and_arrows() {
        let k = key("Enter").unwrap();
        assert_eq!(
            (k.key.as_str(), k.code.as_str(), k.key_code),
            ("Enter", "Enter", 13)
        );
        assert_eq!(k.text.as_deref(), Some("\r"));
        assert_eq!(key("Tab").unwrap().key_code, 9);
        assert_eq!(key("Escape").unwrap().key_code, 27);
        assert_eq!(key("Escape").unwrap().text, None);
        for (name, code) in [
            ("ArrowUp", 38),
            ("ArrowDown", 40),
            ("ArrowLeft", 37),
            ("ArrowRight", 39),
        ] {
            let k = key(name).unwrap();
            assert_eq!(
                (k.key.as_str(), k.code.as_str(), k.key_code),
                (name, name, code)
            );
        }
        let a = key("a").unwrap();
        assert_eq!(
            (a.key.as_str(), a.code.as_str(), a.key_code),
            ("a", "KeyA", 65)
        );
        assert_eq!(a.text.as_deref(), Some("a"));
        assert_eq!(key("7").unwrap().code, "Digit7");
        assert_eq!(key("Space").unwrap().text.as_deref(), Some(" "));
        assert_eq!(key("Hyper"), None);
        assert_eq!(key(""), None);
    }

    #[test]
    fn the_step_scripts_embed_their_strings_as_literals() {
        let click = click_js("button[name=\"go\"]");
        assert!(click.contains(r#""button[name=\"go\"]""#), "{click}");
        assert!(click.contains("elementFromPoint"), "{click}");
        assert!(click.contains("scrollIntoView"), "{click}");
        let wait = wait_js(&WaitFor::Selector {
            selector: ".done".into(),
            visible: true,
            gone: false,
            ms: 2500,
        });
        assert!(wait.contains("MutationObserver") && wait.contains("requestAnimationFrame"));
        assert!(wait.contains("2500"), "{wait}");
        let e = expect_js(&Expect {
            selector: Some("li".into()),
            count: Some(3),
            text: Some("milk".into()),
            ..Expect::default()
        });
        assert!(
            e.contains("querySelectorAll(\"li\")") && e.contains("innerText"),
            "{e}"
        );
        assert!(e.contains("\"milk\""), "{e}");
        // An expression ending in a line comment still closes.
        let ev = eval_js("document.title // the title");
        assert!(ev.contains("document.title // the title\n"), "{ev}");
        assert!(ev.contains("JSON.stringify") && ev.contains(&MAX_EVAL_BYTES.to_string()));
    }
}
