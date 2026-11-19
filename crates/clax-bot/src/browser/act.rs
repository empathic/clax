//! Actions, `expect` and the page's own actions in a live session (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §6.5,
//! §6.10): [`perform`] for a locator's action, [`expect`] for a matcher,
//! [`page`] for a page action, each answering an [`Answered`]: a value,
//! or a failure on the page with Playwright's message. A failure is an
//! answer, never an error: only what ends the call (the call's deadline
//! or the session's lifetime, the run's cancellation, the page lost) is
//! a [`CdpFail`].
//!
//! # Waiting
//!
//! An action resolves its locator ([`super::locate::resolve`]) again and
//! again until exactly one element matches and is actionable, then acts.
//! Between two attempts it waits in the page ([`FRAME_WAIT_JS`]: the
//! first DOM mutation or two animation frames, awaited through
//! `Runtime.evaluate`), and its time is the browser source's deadline: no
//! sleep in Clax. More than one match is a strict-mode violation,
//! answered at once.
//!
//! | Action | Waits for |
//! |---|---|
//! | `click`, `dblclick`, `check`, `uncheck`, `hover` | attached, visible, stable (the same box across a frame), enabled (not `hover`), receiving events (`DOM.getNodeForLocation` at the point is the element or inside it) |
//! | `fill`, `pressSequentially`, `press`, `focus`, `selectOption` | attached, visible, enabled, editable (`fill`, `pressSequentially`) |
//! | `textContent`, `innerText`, `inputValue`, `getAttribute`, `isEnabled`, `isChecked` | attached |
//! | `scrollIntoViewIfNeeded`, `screenshot` | attached, visible |
//! | `waitFor` | its `state` (`visible` by default) |
//! | `count`, `isVisible`, `isHidden`, `allTextContents` | nothing |
//!
//! `expect` checks its condition the same way, again and again until it
//! holds (or, with `not`, stops holding) or its time passes, and answers
//! once: one call however long it retried.
//!
//! # Timeouts
//!
//! A call's `timeout` in milliseconds, else 10 s for an action, 30 s for
//! a navigation (`goto`, `reload`, `goBack`, `goForward`,
//! `waitForLoadState`, `waitForURL`) and 5 s for `expect`, each at most
//! the call's cap (`browser.call_seconds`); `timeout: 0` is the cap. The
//! prelude puts the page's `setDefaultTimeout` in `timeout`.
//!
//! # Arguments
//!
//! A method's positional parameters by their names in spec §6.1, its
//! options spread beside them: `fill` `{value}`, `press` `{key}`,
//! `pressSequentially` `{text}`, `getAttribute` `{name}`, `selectOption`
//! `{values}`, `click` `{button, clickCount, position}`, `waitFor`
//! `{state}`, `toHaveText` and `toContainText` `{expected}`, `toHaveValue`
//! `{value}`, `toHaveAttribute` `{name, value}`, `toHaveCount` `{count}`,
//! `toBeChecked` `{checked}`, `toHaveURL` `{url}`, `toHaveTitle`
//! `{title}`, and `not: true` for a negated matcher; `keyboard.*` `{key}`
//! or `{text}`, `mouse.move` `{x, y, steps}`, `mouse.click` `{x, y,
//! button, clickCount}`, `mouse.wheel` `{deltaX, deltaY}`,
//! `setViewportSize` `{size: {width, height}}`, `waitForLoadState`
//! `{state}`, `waitForURL` `{url}`, `screenshot` `{fullPage}`. A string
//! or `RegExp` matcher's value is a string or `{regex, flags}`.
//!
//! # Approximations (risk `7bda2d4c`)
//!
//! - Visible is a box with a width and a height (`DOM.getBoxModel`):
//!   `visibility: hidden` and `opacity: 0` count as visible.
//! - Enabled is no `disabled` on the element or a `fieldset` around it
//!   and no `aria-disabled="true"` on it or around it; editable adds no
//!   `readonly`, on an `input`, a `textarea`, a `select` or a
//!   `contenteditable` element.
//! - Stable is the same box on two attempts' measures across one wait in
//!   the page, not on two consecutive animation frames.
//! - Receiving events is asked at one point, the box's centre (or
//!   `position`).
//! - `waitForLoadState('networkidle')` is the load and a settle.

use super::cdp::CdpFail;
use super::live::{Answered, SessionTimes};
use super::locate::{
    LocatorStep, Resolved, TextMatch, describe, normalized, parse_locator, resolve,
};
use super::session::{Driver, SessionLimits};
use super::{BrowserSource, Deadline};
use chrono::{DateTime, Duration, Utc};
use clax_core::working::Clock;
use serde_json::{Value, json};

/// An action's default timeout.
pub const ACTION_MS: i64 = 10_000;
/// A navigation's default timeout.
pub const NAVIGATION_MS: i64 = 30_000;
/// `expect`'s default timeout.
pub const EXPECT_MS: i64 = 5_000;
/// The most characters of `innerText` and `textContent`.
pub const MAX_TEXT: usize = 20_000;
/// The most console messages, page errors and request entries answered.
pub const MAX_CONSOLE_ANSWERED: usize = 100;
pub const MAX_ERRORS_ANSWERED: usize = 50;

/// What a wait in the page evaluates: the first DOM mutation or two
/// animation frames, whichever comes first.
pub const FRAME_WAIT_JS: &str = "/* clax:frame */ new Promise((r) => { let done = false; const o = new MutationObserver(() => end()); const end = () => { if (!done) { done = true; o.disconnect(); r(true); } }; o.observe(document, {subtree: true, childList: true, attributes: true, characterData: true}); requestAnimationFrame(() => requestAnimationFrame(end)); })";

/// An element's checked state.
pub const CHECKED_FN: &str = "function() { return this.checked !== undefined ? !!this.checked : this.getAttribute('aria-checked') === 'true'; }";
/// An element's value.
pub const VALUE_FN: &str = "function() { if (!(this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement || this instanceof HTMLSelectElement)) throw new Error('Not an input element'); return String(this.value); }";
/// An element's rendered text.
pub const INNER_TEXT_FN: &str = "function() { return String(this.innerText); }";
/// Selects an element's text, before `fill` types over it.
pub const SELECT_TEXT_FN: &str = "function() { if (typeof this.select === 'function') { this.select(); return; } const r = document.createRange(); r.selectNodeContents(this); const s = getSelection(); s.removeAllRanges(); s.addRange(r); }";
/// Whether the element has the focus.
pub const FOCUSED_FN: &str = "function() { return this === document.activeElement; }";
/// Selects the options of a `<select>` its argument names (`{value?,
/// label?, index?}` each), answering the values selected, as
/// Playwright's `selectOption`.
pub const SELECT_OPTION_FN: &str = "function(wanted) { /* clax:selectOption */ if (!(this instanceof HTMLSelectElement)) throw new Error('Element is not a <select> element'); const opts = Array.from(this.options); const picked = []; for (const w of wanted) { const o = opts.find((o, i) => (w.value !== undefined && o.value === w.value) || (w.label !== undefined && o.label === w.label) || (w.index !== undefined && i === w.index)); if (!o) throw new Error('did not find some options'); picked.push(o); if (!this.multiple) break; } for (const o of opts) o.selected = picked.includes(o); this.dispatchEvent(new Event('input', {bubbles: true})); this.dispatchEvent(new Event('change', {bubbles: true})); return picked.map((o) => o.value); }";

/// How the call's time is kept: its clock, the source's deadlines, the
/// call's own end and cap.
pub(super) struct Timing<'t> {
    pub clock: &'t dyn Clock,
    pub source: &'t dyn BrowserSource,
    /// The call's deadline, at most the session's lifetime.
    pub call_until: DateTime<Utc>,
    pub times: SessionTimes,
    pub limits: SessionLimits,
}

impl Timing<'_> {
    /// The call's timeout from `args`, `default` when it names none, and
    /// the instant it ends.
    fn until(&self, args: &Value, default: i64) -> (i64, DateTime<Utc>) {
        let cap = i64::from(self.times.call_seconds) * 1_000;
        let ms = match args.get("timeout").and_then(Value::as_i64) {
            Some(0) => cap,
            Some(t) if t > 0 => t.min(cap),
            _ => default.min(cap),
        };
        let until = (self.clock.now() + Duration::milliseconds(ms)).min(self.call_until);
        (ms, until)
    }

    fn deadline(&self, at: DateTime<Utc>) -> Deadline {
        self.source.deadline(at)
    }
}

fn ok(value: Value) -> Answered {
    Answered {
        ok: true,
        value,
        error: None,
        timeout: false,
        url: String::new(),
        at_ms: 0,
        shots: Vec::new(),
        kind: None,
    }
}

fn fail(error: String, timeout: bool) -> Answered {
    Answered {
        ok: false,
        value: Value::Null,
        error: Some(error),
        timeout,
        url: String::new(),
        at_ms: 0,
        shots: Vec::new(),
        kind: None,
    }
}

/// What one attempt at an element found.
enum Attempt {
    /// The one element, actionable: its index in the tree, its backend
    /// node, the point to act at.
    Got(Target),
    /// More than one element: the message.
    Strict(String),
    /// Not yet: why, in the call log's words.
    NotYet(String),
}

/// The element an action acts on.
#[derive(Clone, Debug)]
struct Target {
    resolved: Resolved,
    index: usize,
    backend: i64,
    point: (f64, f64),
}

/// What an action needs of its element before it acts.
#[derive(Clone, Copy, Debug, Default)]
struct Needs {
    visible: bool,
    stable: bool,
    enabled: bool,
    editable: bool,
    events: bool,
}

const POINTER: Needs = Needs {
    visible: true,
    stable: true,
    enabled: true,
    editable: false,
    events: true,
};
const HOVER: Needs = Needs {
    enabled: false,
    ..POINTER
};
const INPUT: Needs = Needs {
    visible: true,
    stable: false,
    enabled: true,
    editable: false,
    events: false,
};
const EDIT: Needs = Needs {
    editable: true,
    ..INPUT
};
const ATTACHED: Needs = Needs {
    visible: false,
    stable: false,
    enabled: false,
    editable: false,
    events: false,
};
const VISIBLE: Needs = Needs {
    visible: true,
    ..ATTACHED
};

/// The strict-mode message for `r`'s matches of `desc`.
fn strict(name: &str, desc: &str, r: &Resolved) -> String {
    let listed: Vec<String> = r
        .found
        .iter()
        .take(10)
        .enumerate()
        .map(|(n, &i)| format!("    {}) {}", n + 1, r.tree.shown(i)))
        .collect();
    format!(
        "{name}: Error: strict mode violation: {desc} resolved to {} elements:\n{}",
        r.found.len(),
        listed.join("\n")
    )
}

/// Whether element `i` is disabled (see the module docs).
fn disabled(r: &Resolved, i: usize) -> bool {
    let t = &r.tree;
    let control = |n: &super::locate::Node| {
        matches!(
            n.name.as_str(),
            "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
        )
    };
    if control(&t.nodes[i]) && t.nodes[i].attr("disabled").is_some() {
        return true;
    }
    let mut at = Some(i);
    while let Some(a) = at {
        let n = &t.nodes[a];
        if n.attr("aria-disabled") == Some("true") {
            return true;
        }
        if a != i && n.name == "fieldset" && n.attr("disabled").is_some() && control(&t.nodes[i]) {
            return true;
        }
        at = n.parent;
    }
    false
}

/// Whether element `i` is editable (see the module docs).
fn editable(r: &Resolved, i: usize) -> bool {
    let n = &r.tree.nodes[i];
    let kind = match n.name.as_str() {
        "input" => !matches!(
            n.attr("type")
                .unwrap_or("text")
                .to_ascii_lowercase()
                .as_str(),
            "checkbox"
                | "radio"
                | "button"
                | "submit"
                | "reset"
                | "image"
                | "file"
                | "hidden"
                | "range"
                | "color"
        ),
        "textarea" | "select" => true,
        _ => n.attr("contenteditable").is_some_and(|v| v != "false"),
    };
    kind && n.attr("readonly").is_none() && !disabled(r, i)
}

/// The box of `backend`: `(x, y, width, height)`, or why it has none.
async fn rect(
    d: &mut Driver<'_>,
    backend: i64,
) -> Result<Result<(f64, f64, f64, f64), String>, CdpFail> {
    match d
        .page_call("DOM.getBoxModel", json!({"backendNodeId": backend}))
        .await
    {
        Ok(m) => {
            let b = &m["model"]["border"];
            let n = |k: usize| b[k].as_f64().unwrap_or(0.0);
            let (w, h) = (
                m["model"]["width"].as_f64().unwrap_or(0.0),
                m["model"]["height"].as_f64().unwrap_or(0.0),
            );
            if w <= 0.0 || h <= 0.0 {
                Ok(Err("element is not visible".into()))
            } else {
                Ok(Ok((n(0), n(1), w, h)))
            }
        }
        Err(CdpFail::Protocol(_)) => Ok(Err("element is not visible".into())),
        Err(e) => Err(e),
    }
}

/// Waits in the page (see the module docs).
async fn frame_wait(d: &mut Driver<'_>) -> Result<(), CdpFail> {
    d.evaluate(FRAME_WAIT_JS).await.map(|_| ())
}

/// One attempt at `steps`' element under `needs`, acting at `position`
/// (relative to its box) when given.
async fn attempt(
    d: &mut Driver<'_>,
    name: &str,
    steps: &[LocatorStep],
    needs: Needs,
    position: Option<(f64, f64)>,
) -> Result<Attempt, CdpFail> {
    let desc = describe(steps);
    let r = resolve(d, steps).await?;
    match r.found.len() {
        0 => return Ok(Attempt::NotYet(format!("waiting for {desc}"))),
        1 => {}
        _ => return Ok(Attempt::Strict(strict(name, &desc, &r))),
    }
    let index = r.found[0];
    let backend = r.tree.nodes[index].backend;
    if needs.enabled && disabled(&r, index) {
        return Ok(Attempt::NotYet("element is not enabled".into()));
    }
    if needs.editable && !editable(&r, index) {
        return Ok(Attempt::NotYet("element is not editable".into()));
    }
    let mut rect_now = None;
    if needs.visible {
        if needs.events || needs.stable {
            // Scrolled into view first, so the box and the point are the
            // viewport's.
            match d
                .page_call(
                    "DOM.scrollIntoViewIfNeeded",
                    json!({"backendNodeId": backend}),
                )
                .await
            {
                Ok(_) | Err(CdpFail::Protocol(_)) => {}
                Err(e) => return Err(e),
            }
        }
        match rect(d, backend).await? {
            Ok(b) => rect_now = Some(b),
            Err(why) => return Ok(Attempt::NotYet(why)),
        }
    }
    if needs.stable {
        frame_wait(d).await?;
        match rect(d, backend).await? {
            Ok(b) if Some(b) == rect_now => {}
            Ok(_) => return Ok(Attempt::NotYet("element is not stable".into())),
            Err(why) => return Ok(Attempt::NotYet(why)),
        }
    }
    let point = match (rect_now, position) {
        (Some((x, y, _, _)), Some((px, py))) => (x + px, y + py),
        (Some((x, y, w, h)), None) => (x + w / 2.0, y + h / 2.0),
        (None, _) => (0.0, 0.0),
    };
    if needs.events {
        let hit = match d
            .page_call(
                "DOM.getNodeForLocation",
                json!({"x": point.0.round() as i64, "y": point.1.round() as i64, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true}),
            )
            .await
        {
            Ok(h) => h["backendNodeId"].as_i64(),
            Err(CdpFail::Protocol(_)) => None,
            Err(e) => return Err(e),
        };
        let inside = hit
            .and_then(|b| r.tree.by_backend.get(&b).copied())
            .is_some_and(|h| r.tree.contains(index, h));
        if !inside {
            let by = hit
                .and_then(|b| r.tree.by_backend.get(&b).copied())
                .map_or_else(|| "another element".to_string(), |h| r.tree.shown(h));
            return Ok(Attempt::NotYet(format!("element is covered by {by}")));
        }
    }
    Ok(Attempt::Got(Target {
        resolved: r,
        index,
        backend,
        point,
    }))
}

/// Why an action could not get its element.
enum Missed {
    Strict(String),
    /// Its time ran out; the last reason it waited for.
    Timeout(String),
}

/// `steps`' element under `needs`, waited for until `until` (see the
/// module docs).
async fn wait_for_target(
    d: &mut Driver<'_>,
    timing: &Timing<'_>,
    until: DateTime<Utc>,
    name: &str,
    steps: &[LocatorStep],
    needs: Needs,
    position: Option<(f64, f64)>,
) -> Result<Result<Target, Missed>, CdpFail> {
    d.cdp.set_deadline(timing.deadline(until));
    let mut last = format!("waiting for {}", describe(steps));
    loop {
        match attempt(d, name, steps, needs, position).await {
            Ok(Attempt::Got(t)) => return Ok(Ok(t)),
            Ok(Attempt::Strict(m)) => return Ok(Err(Missed::Strict(m))),
            Ok(Attempt::NotYet(why)) => last = why,
            Err(CdpFail::Deadline) => return Ok(Err(Missed::Timeout(last))),
            Err(e) => return Err(e),
        }
        match frame_wait(d).await {
            Ok(()) => {}
            Err(CdpFail::Deadline) => return Ok(Err(Missed::Timeout(last))),
            Err(e) => return Err(e),
        }
    }
}

/// The failure of `name` that missed its element.
fn missed(name: &str, ms: i64, desc: &str, m: Missed) -> Answered {
    match m {
        Missed::Strict(msg) => fail(msg, false),
        Missed::Timeout(last) => {
            let mut log =
                format!("{name}: Timeout {ms}ms exceeded.\nCall log:\n  - waiting for {desc}");
            if last != format!("waiting for {desc}") {
                log.push_str(&format!("\n  - {last}"));
            }
            fail(log, true)
        }
    }
}

/// One mouse event at `(x, y)`.
async fn mouse_event(
    d: &mut Driver<'_>,
    ty: &str,
    (x, y): (f64, f64),
    button: &str,
    click_count: i64,
) -> Result<(), CdpFail> {
    let buttons = match (ty, button) {
        ("mousePressed", "left") => 1,
        ("mousePressed", "right") => 2,
        ("mousePressed", "middle") => 4,
        _ => 0,
    };
    let mut p = json!({"type": ty, "x": x, "y": y});
    if ty != "mouseMoved" {
        p["button"] = json!(button);
        p["buttons"] = json!(buttons);
        p["clickCount"] = json!(click_count);
    }
    d.page_call("Input.dispatchMouseEvent", p).await?;
    d.mouse = (x, y);
    Ok(())
}

/// A click of `count` presses at `point`.
async fn click_at(
    d: &mut Driver<'_>,
    point: (f64, f64),
    button: &str,
    count: i64,
) -> Result<(), CdpFail> {
    mouse_event(d, "mouseMoved", point, "none", 0).await?;
    for n in 1..=count.max(1) {
        mouse_event(d, "mousePressed", point, button, n).await?;
        mouse_event(d, "mouseReleased", point, button, n).await?;
    }
    Ok(())
}

fn button_of(args: &Value) -> Result<&str, String> {
    match args.get("button").and_then(Value::as_str).unwrap_or("left") {
        b @ ("left" | "right" | "middle") => Ok(b),
        other => Err(format!("button must be left, right or middle, not {other}")),
    }
}

fn position_of(args: &Value) -> Option<(f64, f64)> {
    let p = args.get("position")?;
    Some((p["x"].as_f64()?, p["y"].as_f64()?))
}

/// A key of Playwright's names (`Enter`, `a`, `F5`, `Shift`), with its
/// DOM `key`, `code`, key code and text.
fn key_of(name: &str) -> Option<super::opts::Key> {
    let named = |key: &str, code: &str, key_code: u32| super::opts::Key {
        name: name.to_string(),
        key: key.to_string(),
        code: code.to_string(),
        key_code,
        text: None,
    };
    Some(match name {
        "Shift" => named("Shift", "ShiftLeft", 16),
        "Control" => named("Control", "ControlLeft", 17),
        "Alt" => named("Alt", "AltLeft", 18),
        "Meta" | "ControlOrMeta" => named("Meta", "MetaLeft", 91),
        "Insert" => named("Insert", "Insert", 45),
        f if f.len() >= 2
            && f.starts_with('F')
            && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: u32 = f[1..].parse().unwrap_or(1);
            named(f, f, 111 + n)
        }
        other => return super::opts::key(other),
    })
}

/// The modifier bit of a modifier key.
fn modifier_bit(key: &str) -> i64 {
    match key {
        "Alt" => 1,
        "Control" => 2,
        "Meta" | "ControlOrMeta" => 4,
        "Shift" => 8,
        _ => 0,
    }
}

/// One key event of `k` (`keyDown`, `keyUp`) with `modifiers`.
async fn key_event(
    d: &mut Driver<'_>,
    ty: &str,
    k: &super::opts::Key,
    modifiers: i64,
) -> Result<(), CdpFail> {
    let mut p = json!({
        "type": if ty == "keyDown" && k.text.is_none() { "rawKeyDown" } else { ty },
        "key": k.key,
        "code": k.code,
        "windowsVirtualKeyCode": k.key_code,
        "modifiers": modifiers,
    });
    if ty == "keyDown"
        && let Some(t) = &k.text
        && modifiers & !8 == 0
    {
        p["text"] = json!(t);
    }
    d.page_call("Input.dispatchKeyEvent", p).await.map(|_| ())
}

/// `keyboard.press` of `combo` (`Control+a`): the modifiers down, the
/// key down and up, the modifiers up.
async fn press_combo(d: &mut Driver<'_>, combo: &str) -> Result<Result<(), String>, CdpFail> {
    let parts: Vec<&str> = if combo == "+" {
        vec!["+"]
    } else {
        combo.split('+').collect()
    };
    let mut keys = Vec::new();
    for p in &parts {
        match key_of(p) {
            Some(k) => keys.push(k),
            None => return Ok(Err(format!("Unknown key: \"{p}\""))),
        }
    }
    let Some((last, mods)) = keys.split_last() else {
        return Ok(Err("Unknown key: \"\"".into()));
    };
    let mut bits = 0;
    for m in mods {
        bits |= modifier_bit(&m.name);
        key_event(d, "keyDown", m, bits).await?;
    }
    key_event(d, "keyDown", last, bits).await?;
    key_event(d, "keyUp", last, bits).await?;
    for m in mods.iter().rev() {
        bits &= !modifier_bit(&m.name);
        key_event(d, "keyUp", m, bits).await?;
    }
    Ok(Ok(()))
}

/// Types `text` a key at a time: a key Playwright names by its character
/// as its key events, any other character inserted.
async fn type_text(d: &mut Driver<'_>, text: &str) -> Result<(), CdpFail> {
    for c in text.chars() {
        let s = c.to_string();
        match super::opts::key(&s).filter(|k| k.text.is_some()) {
            Some(k) => {
                key_event(d, "keyDown", &k, 0).await?;
                key_event(d, "keyUp", &k, 0).await?;
            }
            None => {
                d.page_call("Input.insertText", json!({"text": s})).await?;
            }
        }
    }
    Ok(())
}

/// Calls `function` on element `backend` with `args`: its value, or what
/// it threw.
async fn on_element(
    d: &mut Driver<'_>,
    backend: i64,
    function: &str,
    args: &[Value],
) -> Result<Result<Value, String>, CdpFail> {
    let o = match d
        .page_call("DOM.resolveNode", json!({"backendNodeId": backend}))
        .await
    {
        Ok(o) => o,
        Err(CdpFail::Protocol(m)) => return Ok(Err(m)),
        Err(e) => return Err(e),
    };
    let Some(object) = o["object"]["objectId"].as_str() else {
        return Ok(Err("the element is gone".into()));
    };
    let arguments: Vec<Value> = args.iter().map(|a| json!({"value": a})).collect();
    let r = match d
        .page_call(
            "Runtime.callFunctionOn",
            json!({"objectId": object, "functionDeclaration": function, "arguments": arguments, "awaitPromise": true, "returnByValue": true}),
        )
        .await
    {
        Ok(r) => r,
        Err(CdpFail::Protocol(m)) => return Ok(Err(m)),
        Err(e) => return Err(e),
    };
    if let Some(e) = r.get("exceptionDetails") {
        let what = e["exception"]["description"]
            .as_str()
            .or_else(|| e["text"].as_str())
            .unwrap_or("it threw");
        let first = what.lines().next().unwrap_or(what);
        return Ok(Err(first.trim_start_matches("Error: ").to_string()));
    }
    Ok(Ok(r["result"].get("value").cloned().unwrap_or(Value::Null)))
}

/// `s` cut to `max` characters, ending with `…` when it was.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// Performs locator action `action` of `steps` (unparsed: `locator`) with
/// `args` (see the module docs).
///
/// # Errors
/// What ended the call.
pub(super) async fn perform(
    d: &mut Driver<'_>,
    timing: &Timing<'_>,
    locator: &Value,
    action: &str,
    args: &Value,
) -> Result<Answered, CdpFail> {
    let name = format!("locator.{action}");
    let steps = match parse_locator(locator) {
        Ok(s) => s,
        Err(e) => return Ok(fail(format!("{name}: {e}"), false)),
    };
    let desc = describe(&steps);
    let (ms, until) = timing.until(args, ACTION_MS);
    // What answers at once.
    match action {
        "count" => {
            let r = resolve(d, &steps).await?;
            return Ok(ok(json!(r.found.len())));
        }
        "allTextContents" => {
            let r = resolve(d, &steps).await?;
            let texts: Vec<Value> = r
                .found
                .iter()
                .map(|&i| json!(cut(&r.tree.text(i), MAX_TEXT)))
                .collect();
            return Ok(ok(Value::Array(texts)));
        }
        "isVisible" | "isHidden" => {
            let r = resolve(d, &steps).await?;
            if r.found.len() > 1 {
                return Ok(fail(strict(&name, &desc, &r), false));
            }
            let visible = match r.found.first() {
                Some(&i) => rect(d, r.tree.nodes[i].backend).await?.is_ok(),
                None => false,
            };
            return Ok(ok(json!(if action == "isVisible" {
                visible
            } else {
                !visible
            })));
        }
        "waitFor" => return wait_for_state(d, timing, &name, &steps, args).await,
        _ => {}
    }
    let needs = match action {
        "click" | "dblclick" | "check" | "uncheck" => POINTER,
        "hover" => HOVER,
        "fill" | "pressSequentially" => EDIT,
        "press" | "focus" | "selectOption" => INPUT,
        "textContent" | "innerText" | "inputValue" | "getAttribute" | "isEnabled" | "isChecked" => {
            ATTACHED
        }
        "scrollIntoViewIfNeeded" | "screenshot" => VISIBLE,
        other => return Ok(fail(format!("locator.{other} is not an action"), false)),
    };
    let button = match button_of(args) {
        Ok(b) => b,
        Err(e) => return Ok(fail(format!("{name}: {e}"), false)),
    };
    let t = match wait_for_target(d, timing, until, &name, &steps, needs, position_of(args)).await?
    {
        Ok(t) => t,
        Err(m) => return Ok(missed(&name, ms, &desc, m)),
    };
    let node = &t.resolved.tree.nodes[t.index];
    Ok(match action {
        "click" => {
            let count = args
                .get("clickCount")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 3);
            click_at(d, t.point, button, count).await?;
            settle_after(d).await?;
            ok(Value::Null)
        }
        "dblclick" => {
            click_at(d, t.point, button, 2).await?;
            settle_after(d).await?;
            ok(Value::Null)
        }
        "hover" => {
            mouse_event(d, "mouseMoved", t.point, "none", 0).await?;
            ok(Value::Null)
        }
        "check" | "uncheck" => {
            let want = action == "check";
            match on_element(d, t.backend, CHECKED_FN, &[]).await? {
                Ok(Value::Bool(now)) if now == want => ok(Value::Null),
                Ok(Value::Bool(_)) => {
                    click_at(d, t.point, "left", 1).await?;
                    settle_after(d).await?;
                    match on_element(d, t.backend, CHECKED_FN, &[]).await? {
                        Ok(Value::Bool(now)) if now == want => ok(Value::Null),
                        _ => fail(
                            format!("{name}: Clicking the checkbox did not change its state"),
                            false,
                        ),
                    }
                }
                Ok(_) => fail(format!("{name}: Not a checkbox or radio button"), false),
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        "fill" => {
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default();
            d.page_call("DOM.focus", json!({"backendNodeId": t.backend}))
                .await?;
            if let Err(e) = on_element(d, t.backend, SELECT_TEXT_FN, &[]).await? {
                return Ok(fail(format!("{name}: {e}"), false));
            }
            if value.is_empty() {
                let del = key_of("Delete").expect("a named key");
                key_event(d, "keyDown", &del, 0).await?;
                key_event(d, "keyUp", &del, 0).await?;
            } else {
                d.page_call("Input.insertText", json!({"text": value}))
                    .await?;
            }
            ok(Value::Null)
        }
        "pressSequentially" => {
            let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
            d.page_call("DOM.focus", json!({"backendNodeId": t.backend}))
                .await?;
            type_text(d, text).await?;
            ok(Value::Null)
        }
        "press" => {
            let key = args.get("key").and_then(Value::as_str).unwrap_or_default();
            d.page_call("DOM.focus", json!({"backendNodeId": t.backend}))
                .await?;
            match press_combo(d, key).await? {
                Ok(()) => {
                    settle_after(d).await?;
                    ok(Value::Null)
                }
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        "focus" => {
            d.page_call("DOM.focus", json!({"backendNodeId": t.backend}))
                .await?;
            ok(Value::Null)
        }
        "selectOption" => {
            let wanted: Vec<Value> = match args.get("values") {
                Some(Value::Array(a)) => a.clone(),
                Some(v) => vec![v.clone()],
                None => Vec::new(),
            }
            .into_iter()
            .map(|v| match v {
                Value::String(s) => json!({"value": s, "label": s}),
                other => other,
            })
            .collect();
            match on_element(d, t.backend, SELECT_OPTION_FN, &[Value::Array(wanted)]).await? {
                Ok(v) => ok(v),
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        "textContent" => ok(json!(cut(&t.resolved.tree.text(t.index), MAX_TEXT))),
        "innerText" => match on_element(d, t.backend, INNER_TEXT_FN, &[]).await? {
            Ok(Value::String(s)) => ok(json!(cut(&s, MAX_TEXT))),
            Ok(_) => ok(json!("")),
            Err(e) => fail(format!("{name}: {e}"), false),
        },
        "inputValue" => match on_element(d, t.backend, VALUE_FN, &[]).await? {
            Ok(v) => ok(v),
            Err(e) => fail(format!("{name}: Error: {e}"), false),
        },
        "getAttribute" => {
            let n = args.get("name").and_then(Value::as_str).unwrap_or_default();
            ok(node.attr(n).map_or(Value::Null, |v| json!(v)))
        }
        "isEnabled" => ok(json!(!disabled(&t.resolved, t.index))),
        "isChecked" => match on_element(d, t.backend, CHECKED_FN, &[]).await? {
            Ok(Value::Bool(b)) => ok(json!(b)),
            _ => fail(
                format!("{name}: Error: Not a checkbox or radio button"),
                false,
            ),
        },
        "scrollIntoViewIfNeeded" => {
            d.page_call(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": t.backend}),
            )
            .await?;
            ok(Value::Null)
        }
        "screenshot" => {
            d.page_call(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": t.backend}),
            )
            .await?;
            let (x, y, w, h) = match rect(d, t.backend).await? {
                Ok(b) => b,
                Err(why) => return Ok(fail(format!("{name}: {why}"), false)),
            };
            // The box is the viewport's; the capture's clip is the page's.
            let m = d.page_call("Page.getLayoutMetrics", json!({})).await?;
            let (sx, sy) = (
                m["cssVisualViewport"]["pageX"].as_f64().unwrap_or(0.0),
                m["cssVisualViewport"]["pageY"].as_f64().unwrap_or(0.0),
            );
            match d
                .capture((x + sx, y + sy, w, h), true, timing.limits)
                .await?
            {
                Ok(()) => ok(json!({"screenshot": d.shots.len() - 1})),
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        other => fail(format!("locator.{other} is not an action"), false),
    })
}

/// Lets the page settle after an action: two animation frames.
async fn settle_after(d: &mut Driver<'_>) -> Result<(), CdpFail> {
    match d.settle().await {
        // The action is done: the time it ran out in settling is not its
        // failure.
        Err(CdpFail::Deadline) => Ok(()),
        other => other,
    }
}

/// `locator.waitFor({state})`.
async fn wait_for_state(
    d: &mut Driver<'_>,
    timing: &Timing<'_>,
    name: &str,
    steps: &[LocatorStep],
    args: &Value,
) -> Result<Answered, CdpFail> {
    let desc = describe(steps);
    let state = args
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("visible");
    if !matches!(state, "attached" | "detached" | "visible" | "hidden") {
        return Ok(fail(
            format!("{name}: state must be attached, detached, visible or hidden"),
            false,
        ));
    }
    let (ms, until) = timing.until(args, ACTION_MS);
    d.cdp.set_deadline(timing.deadline(until));
    loop {
        let r = match resolve(d, steps).await {
            Ok(r) => r,
            Err(CdpFail::Deadline) => break,
            Err(e) => return Err(e),
        };
        if r.found.len() > 1 && matches!(state, "attached" | "visible") {
            return Ok(fail(strict(name, &desc, &r), false));
        }
        let visible = match r.found.first() {
            Some(&i) if r.found.len() == 1 => match rect(d, r.tree.nodes[i].backend).await {
                Ok(v) => v.is_ok(),
                Err(CdpFail::Deadline) => break,
                Err(e) => return Err(e),
            },
            _ => false,
        };
        let done = match state {
            "attached" => !r.found.is_empty(),
            "detached" => r.found.is_empty(),
            "visible" => visible,
            _ => !visible,
        };
        if done {
            return Ok(ok(Value::Null));
        }
        match frame_wait(d).await {
            Ok(()) => {}
            Err(CdpFail::Deadline) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(fail(
        format!(
            "{name}: Timeout {ms}ms exceeded.\nCall log:\n  - waiting for {desc} to be {state}"
        ),
        true,
    ))
}

/// An expected text, value or URL: a string or `{regex, flags}`.
fn expected_of(v: &Value) -> Option<TextMatch> {
    match v {
        Value::String(s) => Some(TextMatch::Text {
            text: s.clone(),
            exact: true,
        }),
        Value::Object(o) => Some(TextMatch::Regex {
            source: o.get("regex")?.as_str()?.to_string(),
            flags: o
                .get("flags")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        _ => None,
    }
}

fn shown_expected(t: &TextMatch) -> String {
    match t {
        TextMatch::Text { text, .. } => format!("\"{text}\""),
        TextMatch::Regex { source, flags } => format!("/{source}/{flags}"),
    }
}

/// Whether each of `texts` matches `t`: a string whole (`contains`
/// false) or as a substring, with its case; a regex in the page.
async fn texts_match(
    d: &mut Driver<'_>,
    t: &TextMatch,
    texts: &[String],
    contains: bool,
) -> Result<Vec<bool>, CdpFail> {
    match t {
        TextMatch::Text { text, .. } => {
            let want = normalized(text);
            Ok(texts
                .iter()
                .map(|s| {
                    let s = normalized(s);
                    if contains {
                        s.contains(&want)
                    } else {
                        s == want
                    }
                })
                .collect())
        }
        TextMatch::Regex { .. } => super::locate::text_matches(d, t, texts).await,
    }
}

/// What one check of a matcher saw: whether its condition holds, and
/// what it received, in the failure's words. `Err` answers at once (a
/// strict-mode violation, bad arguments).
type Check = Result<(bool, String), String>;

/// One check of `matcher` on `steps` (the page when `None`).
async fn check(
    d: &mut Driver<'_>,
    steps: Option<&[LocatorStep]>,
    matcher: &str,
    args: &Value,
) -> Result<Check, CdpFail> {
    let Some(steps) = steps else {
        return Ok(match matcher {
            "toHaveURL" => {
                let Some(want) = args.get("url").and_then(expected_of) else {
                    return Ok(Err("toHaveURL needs a URL or a RegExp".into()));
                };
                let url = d.url.clone();
                let hit = texts_match(d, &want, std::slice::from_ref(&url), false).await?[0];
                Ok((hit, format!("\"{url}\"")))
            }
            "toHaveTitle" => {
                let Some(want) = args.get("title").and_then(expected_of) else {
                    return Ok(Err("toHaveTitle needs a title or a RegExp".into()));
                };
                let title = match d.evaluate("document.title").await? {
                    Ok(Value::String(t)) => t,
                    _ => String::new(),
                };
                let hit = texts_match(d, &want, std::slice::from_ref(&title), false).await?[0];
                Ok((hit, format!("\"{title}\"")))
            }
            other => Err(format!("{other} is not a page matcher")),
        });
    };
    let desc = describe(steps);
    let r = resolve(d, steps).await?;
    if matcher == "toHaveCount" {
        let Some(want) = args.get("count").and_then(Value::as_u64) else {
            return Ok(Err("toHaveCount needs a count".into()));
        };
        return Ok(Ok((
            r.found.len() as u64 == want,
            r.found.len().to_string(),
        )));
    }
    if matches!(matcher, "toHaveText" | "toContainText") {
        let contains = matcher == "toContainText";
        let texts: Vec<String> = r.found.iter().map(|&i| r.tree.text(i)).collect();
        let shown_texts = || {
            let t: Vec<String> = texts
                .iter()
                .map(|s| format!("\"{}\"", normalized(s)))
                .collect();
            if texts.len() == 1 {
                t[0].clone()
            } else {
                format!("[{}]", t.join(", "))
            }
        };
        return Ok(match args.get("expected") {
            Some(Value::Array(want)) => {
                let mut wants = Vec::new();
                for w in want {
                    match expected_of(w) {
                        Some(t) => wants.push(t),
                        None => return Ok(Err(format!("{matcher} takes strings and RegExps"))),
                    }
                }
                if wants.len() != texts.len() {
                    Ok((false, shown_texts()))
                } else {
                    let mut all = true;
                    for (w, t) in wants.iter().zip(&texts) {
                        all &= texts_match(d, w, std::slice::from_ref(t), contains).await?[0];
                    }
                    Ok((all, shown_texts()))
                }
            }
            Some(w) => match expected_of(w) {
                None => Err(format!(
                    "{matcher} takes a string, a RegExp or an array of them"
                )),
                Some(want) => {
                    if r.found.len() > 1 {
                        return Ok(Err(strict(&format!("expect.{matcher}"), &desc, &r)));
                    }
                    if texts.is_empty() {
                        Ok((false, "<element(s) not found>".into()))
                    } else {
                        let hit = texts_match(d, &want, &texts, contains).await?[0];
                        Ok((hit, shown_texts()))
                    }
                }
            },
            None => Err(format!("{matcher} needs what to expect")),
        });
    }
    if r.found.len() > 1 {
        return Ok(Err(strict(&format!("expect.{matcher}"), &desc, &r)));
    }
    let Some(&i) = r.found.first() else {
        // No element: hidden, and nothing else holds.
        return Ok(Ok((
            matcher == "toBeHidden",
            "<element(s) not found>".into(),
        )));
    };
    let backend = r.tree.nodes[i].backend;
    Ok(match matcher {
        "toBeVisible" | "toBeHidden" => {
            let visible = rect(d, backend).await?.is_ok();
            let shown = if visible { "visible" } else { "hidden" };
            Ok((visible == (matcher == "toBeVisible"), shown.into()))
        }
        "toBeEnabled" | "toBeDisabled" => {
            let enabled = !disabled(&r, i);
            let shown = if enabled { "enabled" } else { "disabled" };
            Ok((enabled == (matcher == "toBeEnabled"), shown.into()))
        }
        "toBeChecked" => {
            let want = args.get("checked").and_then(Value::as_bool).unwrap_or(true);
            match on_element(d, backend, CHECKED_FN, &[]).await? {
                Ok(Value::Bool(b)) => {
                    Ok((b == want, if b { "checked" } else { "unchecked" }.into()))
                }
                _ => Err("Not a checkbox or radio button".into()),
            }
        }
        "toBeFocused" => match on_element(d, backend, FOCUSED_FN, &[]).await? {
            Ok(Value::Bool(b)) => Ok((b, if b { "focused" } else { "inactive" }.into())),
            _ => Ok((false, "inactive".into())),
        },
        "toHaveValue" => {
            let Some(want) = args.get("value").and_then(expected_of) else {
                return Ok(Err("toHaveValue needs a value or a RegExp".into()));
            };
            match on_element(d, backend, VALUE_FN, &[]).await? {
                Ok(Value::String(v)) => {
                    let hit = match &want {
                        TextMatch::Text { text, .. } => &v == text,
                        r => super::locate::text_matches(d, r, std::slice::from_ref(&v)).await?[0],
                    };
                    Ok((hit, format!("\"{v}\"")))
                }
                Ok(_) => Ok((false, "\"\"".into())),
                Err(e) => Err(e),
            }
        }
        "toHaveAttribute" => {
            let Some(n) = args.get("name").and_then(Value::as_str) else {
                return Ok(Err("toHaveAttribute needs a name".into()));
            };
            let got = r.tree.nodes[i].attr(n).map(str::to_string);
            match (args.get("value").filter(|v| !v.is_null()), got) {
                (None, g) => Ok((
                    g.is_some(),
                    g.map_or("<no attribute>".into(), |v| format!("\"{v}\"")),
                )),
                (Some(_), None) => Ok((false, "<no attribute>".into())),
                (Some(w), Some(g)) => match expected_of(w) {
                    None => Err("toHaveAttribute's value is a string or a RegExp".into()),
                    Some(TextMatch::Text { text, .. }) => Ok((g == text, format!("\"{g}\""))),
                    Some(re) => {
                        let hit =
                            super::locate::text_matches(d, &re, std::slice::from_ref(&g)).await?[0];
                        Ok((hit, format!("\"{g}\"")))
                    }
                },
            }
        }
        other => Err(format!("{other} is not a locator matcher")),
    })
}

/// What a matcher expects, in its failure's words.
fn expectation(matcher: &str, args: &Value) -> String {
    let field = |k: &str| {
        args.get(k)
            .and_then(expected_of)
            .map_or_else(|| args[k].to_string(), |t| shown_expected(&t))
    };
    match matcher {
        "toBeVisible" => "visible".into(),
        "toBeHidden" => "hidden".into(),
        "toBeEnabled" => "enabled".into(),
        "toBeDisabled" => "disabled".into(),
        "toBeChecked" => {
            if args.get("checked").and_then(Value::as_bool) == Some(false) {
                "unchecked".into()
            } else {
                "checked".into()
            }
        }
        "toBeFocused" => "focused".into(),
        "toHaveText" | "toContainText" => match args.get("expected") {
            Some(Value::Array(a)) => format!(
                "[{}]",
                a.iter()
                    .map(|w| expected_of(w).map_or_else(|| w.to_string(), |t| shown_expected(&t)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => field("expected"),
        },
        "toHaveValue" => field("value"),
        "toHaveAttribute" => match args.get("value").filter(|v| !v.is_null()) {
            Some(_) => format!("{}={}", args["name"].as_str().unwrap_or(""), field("value")),
            None => format!("attribute {}", args["name"].as_str().unwrap_or("")),
        },
        "toHaveCount" => args["count"].to_string(),
        "toHaveURL" => field("url"),
        "toHaveTitle" => field("title"),
        _ => String::new(),
    }
}

/// `expect(locator)` or `expect(page)` (when `locator` is null) with
/// `matcher` and `args`, retried until it holds or its time passes (see
/// the module docs).
///
/// # Errors
/// What ended the call.
pub(super) async fn expect(
    d: &mut Driver<'_>,
    timing: &Timing<'_>,
    locator: Option<&Value>,
    matcher: &str,
    args: &Value,
) -> Result<Answered, CdpFail> {
    let not = args.get("not").and_then(Value::as_bool).unwrap_or(false);
    let steps = match locator.map(parse_locator).transpose() {
        Ok(s) => s,
        Err(e) => return Ok(fail(format!("expect.{matcher}: {e}"), false)),
    };
    let subject = if steps.is_some() { "locator" } else { "page" };
    let call = format!(
        "expect({subject}).{}{matcher}()",
        if not { "not." } else { "" }
    );
    let (ms, until) = timing.until(args, EXPECT_MS);
    d.cdp.set_deadline(timing.deadline(until));
    let mut received = String::from("<nothing yet>");
    loop {
        match check(d, steps.as_deref(), matcher, args).await {
            Ok(Ok((holds, got))) => {
                if holds != not {
                    return Ok(ok(Value::Null));
                }
                received = got;
            }
            Ok(Err(e)) => return Ok(fail(format!("{call} failed\n\n{e}"), false)),
            Err(CdpFail::Deadline) => break,
            Err(e) => return Err(e),
        }
        match frame_wait(d).await {
            Ok(()) => {}
            Err(CdpFail::Deadline) => break,
            Err(e) => return Err(e),
        }
    }
    let mut msg = format!("{call} failed\n\n");
    if let Some(s) = &steps {
        msg.push_str(&format!("Locator: {}\n", describe(s)));
    }
    msg.push_str(&format!(
        "Expected: {}{}\nReceived: {received}\nTimeout: {ms}ms",
        if not { "not " } else { "" },
        expectation(matcher, args)
    ));
    Ok(fail(msg, true))
}

/// A page action beyond the ones a session opens with (spec §6.1):
/// `keyboard.*`, `mouse.*`, `setViewportSize`, `waitForLoadState`,
/// `waitForURL`, `goBack`, `goForward`, `consoleMessages`, `pageErrors`,
/// `failedRequests`, `screenshot`; `None` for an action it does not have.
///
/// # Errors
/// What ended the call.
pub(super) async fn page(
    d: &mut Driver<'_>,
    timing: &Timing<'_>,
    action: &str,
    args: &Value,
) -> Result<Option<Answered>, CdpFail> {
    let name = format!("page.{action}");
    let num = |k: &str| args.get(k).and_then(Value::as_f64);
    Ok(Some(match action {
        "keyboard.down" | "keyboard.up" => {
            let k = args.get("key").and_then(Value::as_str).unwrap_or_default();
            match key_of(k) {
                Some(key) => {
                    let ty = if action == "keyboard.down" {
                        "keyDown"
                    } else {
                        "keyUp"
                    };
                    key_event(d, ty, &key, 0).await?;
                    ok(Value::Null)
                }
                None => fail(format!("{name}: Unknown key: \"{k}\""), false),
            }
        }
        "keyboard.press" => {
            let k = args.get("key").and_then(Value::as_str).unwrap_or_default();
            match press_combo(d, k).await? {
                Ok(()) => ok(Value::Null),
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        "keyboard.type" => {
            type_text(
                d,
                args.get("text").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            ok(Value::Null)
        }
        "keyboard.insertText" => {
            let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
            d.page_call("Input.insertText", json!({"text": text}))
                .await?;
            ok(Value::Null)
        }
        "mouse.move" => {
            let (Some(x), Some(y)) = (num("x"), num("y")) else {
                return Ok(Some(fail(format!("{name}: x and y are numbers"), false)));
            };
            let steps = args
                .get("steps")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 100);
            let (fx, fy) = d.mouse;
            for s in 1..=steps {
                let k = s as f64 / steps as f64;
                mouse_event(
                    d,
                    "mouseMoved",
                    (fx + (x - fx) * k, fy + (y - fy) * k),
                    "none",
                    0,
                )
                .await?;
            }
            ok(Value::Null)
        }
        "mouse.down" | "mouse.up" => {
            let button = match button_of(args) {
                Ok(b) => b,
                Err(e) => return Ok(Some(fail(format!("{name}: {e}"), false))),
            };
            let ty = if action == "mouse.down" {
                "mousePressed"
            } else {
                "mouseReleased"
            };
            let at = d.mouse;
            mouse_event(d, ty, at, button, 1).await?;
            ok(Value::Null)
        }
        "mouse.click" | "mouse.dblclick" => {
            let (Some(x), Some(y)) = (num("x"), num("y")) else {
                return Ok(Some(fail(format!("{name}: x and y are numbers"), false)));
            };
            let button = match button_of(args) {
                Ok(b) => b,
                Err(e) => return Ok(Some(fail(format!("{name}: {e}"), false))),
            };
            let count = if action == "mouse.dblclick" {
                2
            } else {
                args.get("clickCount")
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    .clamp(1, 3)
            };
            click_at(d, (x, y), button, count).await?;
            ok(Value::Null)
        }
        "mouse.wheel" => {
            let (dx, dy) = (num("deltaX").unwrap_or(0.0), num("deltaY").unwrap_or(0.0));
            let (x, y) = d.mouse;
            d.page_call(
                "Input.dispatchMouseEvent",
                json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}),
            )
            .await?;
            ok(Value::Null)
        }
        "setViewportSize" => {
            let size = &args["size"];
            let (Some(w), Some(h)) = (size["width"].as_u64(), size["height"].as_u64()) else {
                return Ok(Some(fail(
                    format!("{name}: size is {{width, height}}"),
                    false,
                )));
            };
            if !(200..=3_840).contains(&w) || !(200..=4_000).contains(&h) {
                return Ok(Some(fail(
                    format!("{name}: a viewport is 200 to 3840 wide and 200 to 4000 high"),
                    false,
                )));
            }
            let (w, h) = (w as u32, h as u32);
            d.viewport.width = w;
            d.viewport.height = h;
            d.viewport.preset = None;
            let vp = d.viewport.clone();
            d.page_call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width": vp.width, "height": vp.height, "deviceScaleFactor": vp.scale, "mobile": vp.mobile}),
            )
            .await?;
            ok(Value::Null)
        }
        "waitForLoadState" => {
            let state = args.get("state").and_then(Value::as_str).unwrap_or("load");
            if !matches!(state, "load" | "domcontentloaded" | "networkidle") {
                return Ok(Some(fail(
                    format!("{name}: state must be load, domcontentloaded or networkidle"),
                    false,
                )));
            }
            let (ms, until) = timing.until(args, NAVIGATION_MS);
            d.cdp.set_deadline(timing.deadline(until));
            match d.wait_loaded().await {
                Ok(()) => {
                    if state == "networkidle" {
                        settle_after(d).await?;
                    }
                    ok(Value::Null)
                }
                Err(CdpFail::Deadline) => fail(format!("{name}: Timeout {ms}ms exceeded."), true),
                Err(e) => return Err(e),
            }
        }
        "waitForURL" => {
            let Some(want) = args.get("url").and_then(expected_of) else {
                return Ok(Some(fail(
                    format!("{name}: url is a string or a RegExp"),
                    false,
                )));
            };
            let (ms, until) = timing.until(args, NAVIGATION_MS);
            d.cdp.set_deadline(timing.deadline(until));
            loop {
                let url = d.url.clone();
                match texts_match(d, &want, std::slice::from_ref(&url), false).await {
                    Ok(hit) if hit[0] => break ok(Value::Null),
                    Ok(_) => {}
                    Err(CdpFail::Deadline) => {
                        break fail(
                            format!(
                                "{name}: Timeout {ms}ms exceeded.\n  waiting for navigation to {}",
                                shown_expected(&want)
                            ),
                            true,
                        );
                    }
                    Err(e) => return Err(e),
                }
                match frame_wait(d).await {
                    Ok(()) => {}
                    Err(CdpFail::Deadline) => {
                        break fail(
                            format!(
                                "{name}: Timeout {ms}ms exceeded.\n  waiting for navigation to {}",
                                shown_expected(&want)
                            ),
                            true,
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        "goBack" | "goForward" => {
            let h = d.page_call("Page.getNavigationHistory", json!({})).await?;
            let at = h["currentIndex"].as_i64().unwrap_or(0);
            let to = if action == "goBack" { at - 1 } else { at + 1 };
            let entry = usize::try_from(to)
                .ok()
                .and_then(|i| h["entries"].get(i))
                .cloned();
            match entry {
                None => ok(Value::Null),
                Some(e) => {
                    let (ms, until) = timing.until(args, NAVIGATION_MS);
                    d.cdp.set_deadline(timing.deadline(until));
                    let url = e["url"].as_str().unwrap_or_default().to_string();
                    match d.history(e["id"].as_i64().unwrap_or(0), &url).await {
                        Ok(Ok(())) => {
                            settle_after(d).await?;
                            ok(
                                json!({"url": d.url, "status": d.status, "ok": (200..300).contains(&d.status)}),
                            )
                        }
                        Ok(Err(e)) => fail(format!("{name}: {e}"), false),
                        Err(CdpFail::Deadline) => {
                            fail(format!("{name}: Timeout {ms}ms exceeded."), true)
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        "record" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(1);
            let r = super::record::record(d, timing, seconds).await?;
            let at: Vec<i64> = r.frames.iter().map(|(at, _)| *at).collect();
            d.shots.extend(r.frames.into_iter().map(|(_, jpeg)| jpeg));
            ok(
                json!({"record": {"at": at, "ms": r.ms, "width": d.viewport.width, "height": d.viewport.height}}),
            )
        }
        "consoleMessages" => {
            let from = d.console.len().saturating_sub(MAX_CONSOLE_ANSWERED);
            let msgs: Vec<Value> = d.console[from..]
                .iter()
                .map(|c| json!({"type": c["level"], "text": c["text"], "location": {"url": "", "lineNumber": 0, "columnNumber": 0}}))
                .collect();
            ok(Value::Array(msgs))
        }
        "pageErrors" => {
            let from = d.errors.len().saturating_sub(MAX_ERRORS_ANSWERED);
            ok(Value::Array(d.errors[from..].to_vec()))
        }
        "failedRequests" => {
            let last = |l: &[Value]| l[l.len().saturating_sub(MAX_CONSOLE_ANSWERED)..].to_vec();
            ok(json!({"failed": last(&d.failed), "blocked": last(&d.blocked)}))
        }
        "screenshot" => {
            let full = args
                .get("fullPage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let m = d.page_call("Page.getLayoutMetrics", json!({})).await?;
            let n = |v: &Value| v.as_f64().unwrap_or(0.0);
            let clip = if full {
                let c = &m["cssContentSize"];
                (0.0, 0.0, n(&c["width"]), n(&c["height"]))
            } else {
                let v = &m["cssLayoutViewport"];
                (
                    n(&v["pageX"]),
                    n(&v["pageY"]),
                    n(&v["clientWidth"]),
                    n(&v["clientHeight"]),
                )
            };
            match d.capture(clip, full, timing.limits).await? {
                Ok(()) => ok(json!({"screenshot": d.shots.len() - 1})),
                Err(e) => fail(format!("{name}: {e}"), false),
            }
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::browser::fake::{
        ChromeLog, Dom, El, FakeBrowserSource, FakeChrome, PageModel, el, value,
    };
    use crate::browser::live::{BrowserSessions, CallCtx, SessionCall, SessionTimes};
    use crate::browser::opts::Viewport;
    use crate::browser::serve::{Decision, Origin, Paused};
    use crate::browser::session::{RequestPolicy, SessionLimits, Start};
    use crate::browser::{BrowserError, Served};
    use base64::Engine as _;
    use clax_bot_sandbox::CancelToken;
    use clax_core::ArtifactId;
    use clax_core::working::ManualClock;
    use clax_store::bot_web::SessionKind;
    use std::sync::Arc;

    const TIMES: SessionTimes = SessionTimes {
        call_seconds: 60,
        idle_seconds: 300,
        max_seconds: 1_800,
    };

    struct Serve;

    #[async_trait::async_trait]
    impl RequestPolicy for Serve {
        async fn decide(&self, _req: &Paused) -> Decision {
            Decision::Fulfill(Served {
                status: 200,
                headers: vec![("Content-Type".into(), "text/html".into())],
                body: b"<p>hi</p>".to_vec(),
            })
        }
    }

    struct World {
        src: Arc<FakeBrowserSource>,
        clock: Arc<ManualClock>,
        sessions: BrowserSessions,
        log: ChromeLog,
    }

    /// A page whose document is `dom` and whose title is "Hello".
    fn model(dom: Dom) -> PageModel {
        PageModel::new().dom(dom).evaluate(|p| {
            if p["expression"] == "document.title" {
                value(json!("Hello"))
            } else {
                value(json!(true))
            }
        })
    }

    /// A session open on a page whose document is `dom`.
    async fn world(dom: Dom) -> World {
        world_with(model(dom)).await
    }

    async fn world_with(model: PageModel) -> World {
        world_clocked(model, Arc::new(ManualClock::at("2026-11-19T10:00:00Z"))).await
    }

    async fn world_clocked(model: PageModel, clock: Arc<ManualClock>) -> World {
        let src = Arc::new(FakeBrowserSource::new());
        let model = std::sync::Mutex::new(Some(model));
        src.opens(move || {
            Ok(FakeChrome::page(
                model.lock().unwrap().take().expect("one open"),
            ))
        });
        let sessions = BrowserSessions::new(
            src.clone(),
            clock.clone(),
            SessionLimits { image_side: 2_000 },
        );
        sessions
            .open(
                "run-1",
                "s1".into(),
                SessionKind::Artifact,
                Start::Test {
                    origin: Origin::new(&ArtifactId::parse("abcdefghjkmn").unwrap()),
                    path: "index.html".into(),
                },
                Arc::new(Serve),
                Viewport::desktop(),
                TIMES,
                clock.now() + chrono::Duration::seconds(60),
                &CancelToken::new(),
            )
            .await
            .expect("opened");
        let log = src.logs()[0].clone();
        World {
            src,
            clock,
            sessions,
            log,
        }
    }

    impl World {
        fn ctx(&self) -> CallCtx {
            CallCtx::new(
                self.clock.now() + chrono::Duration::seconds(60),
                CancelToken::new(),
            )
        }

        async fn act(&self, locator: Value, action: &str, args: Value) -> Answered {
            self.sessions
                .call(
                    "run-1",
                    "s1",
                    SessionCall::Act {
                        locator,
                        action: action.into(),
                        args,
                    },
                    self.ctx(),
                )
                .await
                .expect("answered")
        }

        async fn expect(&self, locator: Option<Value>, matcher: &str, args: Value) -> Answered {
            self.sessions
                .call(
                    "run-1",
                    "s1",
                    SessionCall::Expect {
                        locator,
                        matcher: matcher.into(),
                        args,
                    },
                    self.ctx(),
                )
                .await
                .expect("answered")
        }

        async fn page(&self, action: &str, args: Value) -> Result<Answered, BrowserError> {
            self.sessions
                .call(
                    "run-1",
                    "s1",
                    SessionCall::Page {
                        action: action.into(),
                        args,
                    },
                    self.ctx(),
                )
                .await
        }

        async fn count(&self, locator: Value) -> Value {
            self.act(locator, "count", json!({})).await.value
        }

        /// The mouse events sent since the session opened: `(type, x, y)`.
        fn mouse(&self) -> Vec<(String, f64, f64)> {
            self.log
                .sent("Input.dispatchMouseEvent")
                .iter()
                .map(|c| {
                    let p = &c["params"];
                    (
                        p["type"].as_str().unwrap().to_string(),
                        p["x"].as_f64().unwrap(),
                        p["y"].as_f64().unwrap(),
                    )
                })
                .collect()
        }

        /// The waits in the page sent so far.
        fn frame_waits(&self) -> usize {
            self.log
                .sent("Runtime.evaluate")
                .iter()
                .filter(|c| {
                    c["params"]["expression"]
                        .as_str()
                        .is_some_and(|e| e.contains("clax:frame"))
                })
                .count()
        }
    }

    fn button(name: &str, x: f64) -> El {
        el("button")
            .text(name)
            .rect(x, 0.0, 100.0, 40.0)
            .role("button", name)
    }

    fn form() -> Dom {
        Dom::new(vec![
            el("h1")
                .attr("class", "title")
                .text("Hello world")
                .rect(0.0, 0.0, 400.0, 40.0)
                .role("heading", "Hello world")
                .level(1),
            el("form")
                .rect(0.0, 100.0, 600.0, 400.0)
                .child(
                    el("label")
                        .attr("for", "email")
                        .text("Email address")
                        .rect(0.0, 100.0, 100.0, 20.0),
                )
                .child(
                    el("input")
                        .attr("id", "email")
                        .attr("placeholder", "you@example.com")
                        .rect(0.0, 120.0, 200.0, 20.0)
                        .role("textbox", "Email address")
                        .value(""),
                )
                .child(
                    el("input")
                        .attr("aria-label", "Search")
                        .attr("placeholder", "Search the site")
                        .rect(0.0, 150.0, 200.0, 20.0)
                        .role("searchbox", "Search")
                        .value(""),
                )
                .child(
                    el("button")
                        .attr("class", "primary")
                        .attr("data-testid", "go")
                        .text("Save draft")
                        .rect(0.0, 200.0, 100.0, 40.0)
                        .role("button", "Save draft"),
                ),
            el("button")
                .text("Save")
                .rect(300.0, 0.0, 100.0, 40.0)
                .role("button", "Save"),
            el("ul")
                .child(el("li").text("one apple").rect(0.0, 600.0, 100.0, 20.0))
                .child(el("li").text("two pears").rect(0.0, 620.0, 100.0, 20.0)),
        ])
    }

    #[tokio::test]
    async fn each_step_kind_resolves_as_the_spec_says() {
        let w = world(form()).await;
        for (locator, n) in [
            (json!([{"css": "button.primary"}]), 1),
            (json!([{"css": "button"}]), 2),
            (json!([{"role": "button"}]), 2),
            (json!([{"role": "heading", "level": 1}]), 1),
            (json!([{"role": "heading", "level": 2}]), 0),
            (json!([{"text": "hello"}]), 1),
            (json!([{"text": "Hello", "exact": true}]), 0),
            (json!([{"text": "Hello world", "exact": true}]), 1),
            (json!([{"label": "email"}]), 1),
            (json!([{"label": "Search"}]), 1),
            (json!([{"placeholder": "search the"}]), 1),
            (json!([{"testId": "go"}]), 1),
            (json!([{"testId": "g"}]), 0),
            (json!([{"css": "li"}, {"nth": 0}]), 1),
            (json!([{"css": "li"}, {"nth": -1}]), 1),
            (json!([{"css": "li"}, {"nth": 5}]), 0),
            (json!([{"css": "li"}, {"hasText": "PEARS"}]), 1),
            (json!([{"css": "li"}, {"hasNotText": "pears"}]), 1),
        ] {
            assert_eq!(w.count(locator.clone()).await, json!(n), "{locator}");
        }
        let label = w
            .act(
                json!([{"label": "Email"}]),
                "getAttribute",
                json!({"name": "id"}),
            )
            .await;
        assert_eq!(label.value, json!("email"));
        let last = w
            .act(
                json!([{"css": "li"}, {"nth": -1}]),
                "textContent",
                json!({}),
            )
            .await;
        assert_eq!(last.value, json!("two pears"));
        // A step that does not parse fails the call, naming it.
        let bad = w.act(json!([{"role": "buton"}]), "count", json!({})).await;
        assert_eq!(
            bad.error.as_deref(),
            Some("locator.count: locator[0]: buton is not an ARIA role")
        );
    }

    #[tokio::test]
    async fn a_chain_scopes_its_next_step() {
        let w = world(form()).await;
        assert_eq!(
            w.count(json!([{"css": "form"}, {"role": "button"}])).await,
            json!(1)
        );
        assert_eq!(
            w.count(json!([{"css": "form"}, {"text": "Hello"}])).await,
            json!(0)
        );
        let name = w
            .act(
                json!([{"css": "form"}, {"role": "button"}]),
                "innerText",
                json!({}),
            )
            .await;
        assert_eq!(name.value, json!("Save draft"));
    }

    #[tokio::test]
    async fn role_names_match_as_substrings_unless_exact() {
        let w = world(form()).await;
        assert_eq!(
            w.count(json!([{"role": "button", "name": "save"}])).await,
            json!(2)
        );
        assert_eq!(
            w.count(json!([{"role": "button", "name": "Save", "exact": true}]))
                .await,
            json!(1)
        );
        assert_eq!(
            w.count(json!([{"role": "button", "name": "save", "exact": true}]))
                .await,
            json!(0)
        );
        assert_eq!(
            describe(
                &parse_locator(
                    &json!([{"role": "button", "name": "Save", "exact": true}, {"nth": 0}])
                )
                .unwrap()
            ),
            "getByRole('button', { name: 'Save', exact: true }).first()"
        );
    }

    #[tokio::test]
    async fn two_matches_are_a_strict_mode_violation() {
        let w = world(form()).await;
        let a = w
            .act(
                json!([{"role": "button", "name": "Save"}]),
                "click",
                json!({}),
            )
            .await;
        assert!(!a.ok && !a.timeout, "{a:?}");
        let e = a.error.unwrap();
        assert!(e.starts_with("locator.click: Error: strict mode violation: getByRole('button', { name: 'Save' }) resolved to 2 elements:"), "{e}");
        assert!(
            e.contains("1) <button class=\"primary\" data-testid=\"go\">Save draft</button>"),
            "{e}"
        );
        assert!(e.contains("2) <button>Save</button>"), "{e}");
        assert!(w.mouse().is_empty(), "nothing was clicked");
        assert_eq!(w.frame_waits(), 0, "answered at once");
        // The session stays open.
        assert!(w.page("title", json!({})).await.unwrap().ok);
    }

    fn covered(states: usize) -> Dom {
        let overlay = el("div")
            .attr("class", "overlay")
            .rect(0.0, 0.0, 1280.0, 800.0)
            .z(10);
        let mut s: Vec<Vec<El>> = (0..states)
            .map(|_| vec![button("Book", 100.0), overlay.clone()])
            .collect();
        s.push(vec![button("Book", 100.0)]);
        Dom::states(s)
    }

    #[tokio::test]
    async fn click_waits_until_the_cover_is_gone() {
        let w = world(covered(2)).await;
        let a = w
            .act(
                json!([{"role": "button", "name": "Book"}]),
                "click",
                json!({}),
            )
            .await;
        assert!(a.ok, "{a:?}");
        assert!(w.frame_waits() >= 2);
        assert_eq!(
            w.mouse(),
            [
                ("mouseMoved".to_string(), 150.0, 20.0),
                ("mousePressed".to_string(), 150.0, 20.0),
                ("mouseReleased".to_string(), 150.0, 20.0)
            ]
        );
        let pressed = &w.log.sent("Input.dispatchMouseEvent")[1]["params"];
        assert_eq!(
            (
                &pressed["button"],
                &pressed["clickCount"],
                &pressed["buttons"]
            ),
            (&json!("left"), &json!(1), &json!(1))
        );
    }

    #[tokio::test]
    async fn click_times_out_naming_the_cover() {
        let w = world_with(model(covered(1_000_000)).frame_limit(5)).await;
        let call = w.act(
            json!([{"role": "button", "name": "Book"}]),
            "click",
            json!({"timeout": 2000}),
        );
        futures::pin_mut!(call);
        let waits = |log: &ChromeLog| log.sent("Runtime.evaluate").len();
        let before = waits(&w.log);
        tokio::select! {
            a = &mut call => panic!("answered: {a:?}"),
            () = w.log.wait_sent("Runtime.evaluate", before + 6) => {}
        }
        w.clock.advance(2);
        w.src.fire_until(w.clock.now());
        let a = call.await;
        assert!(!a.ok && a.timeout, "{a:?}");
        assert_eq!(
            a.error.as_deref(),
            Some(
                "locator.click: Timeout 2000ms exceeded.\nCall log:\n  - waiting for getByRole('button', { name: 'Book' })\n  - element is covered by <div class=\"overlay\">"
            )
        );
        assert!(w.mouse().is_empty());
    }

    #[tokio::test]
    async fn an_unstable_box_is_waited_for() {
        let moving = el("button")
            .text("Go")
            .rect(0.0, 0.0, 100.0, 40.0)
            .rect(50.0, 0.0, 100.0, 40.0)
            .rect(50.0, 0.0, 100.0, 40.0)
            .role("button", "Go");
        let w = world(Dom::new(vec![moving])).await;
        let a = w.act(json!([{"role": "button"}]), "click", json!({})).await;
        assert!(a.ok, "{a:?}");
        assert_eq!(w.mouse()[0], ("mouseMoved".to_string(), 100.0, 20.0));
        assert_eq!(
            w.log.sent("DOM.getBoxModel").len(),
            4,
            "two measures an attempt, two attempts"
        );
    }

    #[tokio::test]
    async fn fill_selects_and_inserts() {
        let w = world(form()).await;
        let a = w
            .act(
                json!([{"label": "Email"}]),
                "fill",
                json!({"value": "ada@example.com"}),
            )
            .await;
        assert!(a.ok, "{a:?}");
        let methods: Vec<String> = w
            .log
            .methods()
            .into_iter()
            .filter(|m| {
                matches!(
                    m.as_str(),
                    "DOM.focus" | "Runtime.callFunctionOn" | "Input.insertText"
                )
            })
            .collect();
        assert_eq!(
            methods,
            ["DOM.focus", "Runtime.callFunctionOn", "Input.insertText"]
        );
        assert!(
            w.log.sent("Runtime.callFunctionOn")[0]["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("this.select")
        );
        assert_eq!(
            w.log.sent("Input.insertText")[0]["params"]["text"],
            "ada@example.com"
        );
        let v = w
            .act(json!([{"label": "Email"}]), "inputValue", json!({}))
            .await;
        assert_eq!(v.value, json!("ada@example.com"));
        assert!(
            w.expect(
                Some(json!([{"label": "Email"}])),
                "toHaveValue",
                json!({"value": "ada@example.com"})
            )
            .await
            .ok
        );
        // A second fill replaces the value.
        w.act(json!([{"label": "Email"}]), "fill", json!({"value": "x"}))
            .await;
        assert_eq!(
            w.act(json!([{"label": "Email"}]), "inputValue", json!({}))
                .await
                .value,
            json!("x")
        );
    }

    #[tokio::test]
    async fn count_and_is_visible_do_not_wait() {
        let w = world(form()).await;
        assert_eq!(w.count(json!([{"text": "Booked"}])).await, json!(0));
        let v = w
            .act(json!([{"text": "Booked"}]), "isVisible", json!({}))
            .await;
        assert_eq!(v.value, json!(false));
        let h = w
            .act(json!([{"text": "Booked"}]), "isHidden", json!({}))
            .await;
        assert_eq!(h.value, json!(true));
        let all = w
            .act(json!([{"css": "li"}]), "allTextContents", json!({}))
            .await;
        assert_eq!(all.value, json!(["one apple", "two pears"]));
        assert_eq!(w.frame_waits(), 0);
    }

    fn appears(after: usize) -> Dom {
        let mut s: Vec<Vec<El>> = (0..after).map(|_| vec![button("Book", 0.0)]).collect();
        s.push(vec![
            button("Book", 0.0),
            el("p").text("Booked").rect(0.0, 100.0, 100.0, 20.0),
        ]);
        Dom::states(s)
    }

    #[tokio::test]
    async fn expect_retries_in_the_browser_and_answers_once() {
        let w = world(appears(3)).await;
        let a = w
            .expect(Some(json!([{"text": "Booked"}])), "toBeVisible", json!({}))
            .await;
        assert!(a.ok, "{a:?}");
        assert_eq!(w.log.sent("DOM.getDocument").len(), 4);
        assert_eq!(w.frame_waits(), 3);
        assert!(
            w.expect(
                Some(json!([{"text": "Booked"}])),
                "toHaveText",
                json!({"expected": "Booked"})
            )
            .await
            .ok
        );
        assert!(
            w.expect(
                Some(json!([{"text": "Booked"}])),
                "toContainText",
                json!({"expected": "ook"})
            )
            .await
            .ok
        );
        assert!(
            w.expect(None, "toHaveTitle", json!({"title": "Hello"}))
                .await
                .ok
        );
    }

    #[tokio::test]
    async fn expect_not_waits_for_the_condition_to_stop() {
        let spinner = el("div")
            .attr("class", "spinner")
            .text("Loading")
            .rect(0.0, 0.0, 20.0, 20.0);
        let w = world(Dom::states(vec![
            vec![spinner.clone()],
            vec![spinner],
            vec![],
        ]))
        .await;
        let a = w
            .expect(
                Some(json!([{"css": ".spinner"}])),
                "toBeVisible",
                json!({"not": true}),
            )
            .await;
        assert!(a.ok, "{a:?}");
        assert_eq!(w.log.sent("DOM.getDocument").len(), 3);
    }

    #[tokio::test]
    async fn to_have_text_with_an_array_matches_in_order() {
        let w = world_with(model(form()).frame_limit(3)).await;
        let ok = w
            .expect(
                Some(json!([{"css": "li"}])),
                "toHaveText",
                json!({"expected": ["one apple", {"regex": "pears$", "flags": ""}]}),
            )
            .await;
        assert!(ok.ok, "{ok:?}");
        let docs = w.log.sent("DOM.getDocument").len();
        let call = w.expect(
            Some(json!([{"css": "li"}])),
            "toHaveText",
            json!({"expected": ["two pears", "one apple"], "timeout": 1000}),
        );
        futures::pin_mut!(call);
        // Four checks: three waits in the page answered, the fourth not.
        tokio::select! {
            a = &mut call => panic!("answered: {a:?}"),
            () = w.log.wait_sent("DOM.getDocument", docs + 4) => {}
        }
        w.clock.advance(1);
        w.src.fire_until(w.clock.now());
        let a = call.await;
        assert!(!a.ok && a.timeout);
        assert_eq!(
            a.error.as_deref(),
            Some(
                "expect(locator).toHaveText() failed\n\nLocator: locator('li')\nExpected: [\"two pears\", \"one apple\"]\nReceived: [\"one apple\", \"two pears\"]\nTimeout: 1000ms"
            )
        );
    }

    #[tokio::test]
    async fn timeouts_are_capped_by_call_seconds() {
        let w = world_with(model(form()).frame_limit(0)).await;
        for t in [json!(999_999), json!(0)] {
            let call = w.act(json!([{"text": "Booked"}]), "click", json!({"timeout": t}));
            futures::pin_mut!(call);
            let docs = w.log.sent("DOM.getDocument").len();
            tokio::select! {
                a = &mut call => panic!("answered: {a:?}"),
                () = w.log.wait_sent("DOM.getDocument", docs + 1) => {}
            }
            assert_eq!(
                *w.src.deadlines().last().unwrap(),
                w.clock.now() + chrono::Duration::seconds(60)
            );
            w.clock.advance(60);
            w.src.fire_until(w.clock.now());
            let a = call.await;
            assert!(
                a.error
                    .unwrap()
                    .starts_with("locator.click: Timeout 60000ms exceeded.")
            );
        }
    }

    #[tokio::test]
    async fn evaluate_calls_the_source_with_its_arg() {
        let w = world_with(PageModel::new().dom(form()).evaluate(|p| {
            if p["expression"]
                .as_str()
                .is_some_and(|e| e.starts_with("((x) => x.a * 2)("))
            {
                value(json!(42))
            } else {
                value(json!(true))
            }
        }))
        .await;
        let a = w
            .page(
                "evaluate",
                json!({"source": "(x) => x.a * 2", "arg": {"a": 21}}),
            )
            .await
            .unwrap();
        assert_eq!(a.value, json!(42));
        assert_eq!(
            w.log.sent("Runtime.evaluate").last().unwrap()["params"],
            json!({"expression": "((x) => x.a * 2)({\"a\":21})", "awaitPromise": true, "returnByValue": true})
        );
    }

    #[tokio::test]
    async fn a_regex_step_matches_in_the_page() {
        let w = world(form()).await;
        assert_eq!(
            w.count(json!([{"text": {"regex": "^hello", "flags": "i"}}]))
                .await,
            json!(1)
        );
        assert_eq!(
            w.count(json!([{"text": {"regex": "^hello", "flags": ""}}]))
                .await,
            json!(0)
        );
        let tests: Vec<String> = w
            .log
            .sent("Runtime.evaluate")
            .iter()
            .filter_map(|c| {
                c["params"]["expression"]
                    .as_str()
                    .filter(|e| e.contains("clax:regex"))
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(tests.len(), 2);
        assert!(
            tests[0].contains("new RegExp(a[0], a[1])") && tests[0].contains("[\"^hello\",\"i\",["),
            "{}",
            tests[0]
        );
        assert_eq!(
            w.count(json!([{"role": "button", "name": {"regex": "draft$", "flags": ""}}]))
                .await,
            json!(1)
        );
    }

    #[tokio::test]
    async fn page_actions_send_their_input() {
        let w = world(form()).await;
        w.page("keyboard.press", json!({"key": "Control+a"}))
            .await
            .unwrap();
        let keys: Vec<(String, String, i64)> = w
            .log
            .sent("Input.dispatchKeyEvent")
            .iter()
            .map(|c| {
                let p = &c["params"];
                (
                    p["type"].as_str().unwrap().into(),
                    p["key"].as_str().unwrap().into(),
                    p["modifiers"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("rawKeyDown".to_string(), "Control".to_string(), 2),
                ("keyDown".to_string(), "a".to_string(), 2),
                ("keyUp".to_string(), "a".to_string(), 2),
                ("keyUp".to_string(), "Control".to_string(), 0)
            ]
        );
        assert!(
            w.log.sent("Input.dispatchKeyEvent")[1]["params"]
                .get("text")
                .is_none(),
            "a chord types nothing"
        );
        w.page("mouse.click", json!({"x": 10, "y": 20}))
            .await
            .unwrap();
        w.page("mouse.wheel", json!({"deltaX": 0, "deltaY": 300}))
            .await
            .unwrap();
        let wheel = w.log.sent("Input.dispatchMouseEvent").last().unwrap()["params"].clone();
        assert_eq!(
            wheel,
            json!({"type": "mouseWheel", "x": 10.0, "y": 20.0, "deltaX": 0.0, "deltaY": 300.0})
        );
        w.page(
            "setViewportSize",
            json!({"size": {"width": 390, "height": 844}}),
        )
        .await
        .unwrap();
        let vp = w
            .log
            .sent("Emulation.setDeviceMetricsOverride")
            .last()
            .unwrap()["params"]
            .clone();
        assert_eq!((&vp["width"], &vp["height"]), (&json!(390), &json!(844)));
        let bad = w
            .page(
                "setViewportSize",
                json!({"size": {"width": 10, "height": 844}}),
            )
            .await
            .unwrap();
        assert!(!bad.ok);
        let shot = w
            .act(json!([{"testId": "go"}]), "screenshot", json!({}))
            .await;
        assert_eq!(shot.value, json!({"screenshot": 0}));
        assert_eq!(shot.shots.len(), 1);
        let clip = w.log.sent("Page.captureScreenshot").last().unwrap()["params"]["clip"].clone();
        assert_eq!(
            (&clip["x"], &clip["y"], &clip["width"]),
            (&json!(0.0), &json!(200.0), &json!(100.0))
        );
        let f = w.page("failedRequests", json!({})).await.unwrap();
        assert_eq!(f.value, json!({"failed": [], "blocked": []}));
        assert_eq!(
            w.page("consoleMessages", json!({})).await.unwrap().value,
            json!([])
        );
        assert_eq!(
            w.page("pageErrors", json!({})).await.unwrap().value,
            json!([])
        );
        let check = w
            .expect(
                None,
                "toHaveURL",
                json!({"url": {"regex": "clax-test\\.invalid/$", "flags": ""}}),
            )
            .await;
        assert!(check.ok, "{check:?}");
    }

    /// A page that sends a screencast frame when the screencast starts
    /// and after each acknowledgement, each 50 ms after the last.
    fn screencasting(clock: Arc<ManualClock>) -> PageModel {
        let mut n = 0;
        model(form()).on(move |cmd: &Value| {
            let method = cmd["method"].as_str().unwrap_or_default();
            if method != "Page.startScreencast" && method != "Page.screencastFrameAck" {
                return None;
            }
            if method == "Page.screencastFrameAck" {
                clock.advance_millis(50);
            }
            n += 1;
            let data = base64::engine::general_purpose::STANDARD.encode(format!("jpeg{n}"));
            Some(vec![
                crate::browser::fake::answer(cmd, json!({})),
                crate::browser::fake::event(
                    "Page.screencastFrame",
                    json!({"data": data, "sessionId": n, "metadata": {}}),
                    Some("S"),
                ),
            ])
        })
    }

    #[tokio::test]
    async fn every_frame_is_acknowledged() {
        let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
        let w = world_clocked(screencasting(clock.clone()), clock).await;
        let a = w.page("record", json!({"seconds": 1})).await.unwrap();
        assert!(a.ok, "{a:?}");
        let acks = w.log.sent("Page.screencastFrameAck");
        let sent = w.log.sent("Page.startScreencast");
        assert_eq!(
            sent[0]["params"],
            json!({"format": "jpeg", "quality": 70, "maxWidth": 2000, "maxHeight": 2000})
        );
        // A frame every 50 ms for a second, and the one at its end: each
        // acknowledged, by its own ID.
        assert_eq!(acks.len(), 21);
        for (i, ack) in acks.iter().enumerate() {
            assert_eq!(ack["params"]["sessionId"], json!(i + 1));
        }
        assert_eq!(w.log.sent("Page.stopScreencast").len(), 1);
        // Kept at most every 125 ms: at 0, 150, 300, ... 900.
        let r = &a.value["record"];
        assert_eq!(r["at"], json!([0, 150, 300, 450, 600, 750, 900]));
        assert_eq!(a.shots.len(), 7);
        assert_eq!(a.shots[0], b"jpeg1");
        assert_eq!(r["ms"], 1000);
    }

    #[tokio::test]
    async fn ten_seconds_is_the_most() {
        let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
        let w = world_clocked(screencasting(clock.clone()), clock).await;
        let a = w.page("record", json!({"seconds": 60})).await.unwrap();
        assert!(a.ok, "{a:?}");
        assert_eq!(a.value["record"]["ms"], 10_000);
        assert_eq!(w.log.sent("Page.screencastFrameAck").len(), 201);
    }
}
