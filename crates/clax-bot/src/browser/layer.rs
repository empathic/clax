//! The browser layer's one table (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §6.9,
//! §6.11): each method of `Page`, `Locator`, `Keyboard`, `Mouse`,
//! `LocatorAssertions` and `PageAssertions` that reaches the browser,
//! with its flat operation (`browser.page`, `browser.act`,
//! `browser.expect`, `browser.close`), its action, its parameter names
//! and the check of the `args` its call sends ([`check`]).
//!
//! The prelude's own table (`clax_bot_sandbox::browser_layer_table`) is
//! this one's class, method, operation, action and parameter names, row
//! for row, which a test holds; the flat operations' action unions in
//! `src/clax.d.ts` are this table's actions, which another holds.
//!
//! # Arguments
//!
//! A call's `args` hold the method's positional parameters by their
//! names and its options beside them (`clax_bot_sandbox::PRELUDE`, "The
//! browser layer"), and nothing else: an option Clax does not have is
//! `invalid_args`, naming it. Every row with options takes `timeout`
//! (milliseconds, 0 or more); every matcher takes `not`. A text matcher
//! is a string or `{regex, flags}`.

use crate::capability::Capability;
use serde_json::Value;

/// A flat session operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Page,
    Act,
    Expect,
    Close,
}

impl Op {
    /// Its name in the prelude's table: `page`, `act`, `expect`, `close`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Op::Page => "page",
            Op::Act => "act",
            Op::Expect => "expect",
            Op::Close => "close",
        }
    }

    /// The operation of Clax's surface: `browser.<name>`.
    #[must_use]
    pub fn op(self) -> &'static str {
        match self {
            Op::Page => OP_PAGE,
            Op::Act => OP_ACT,
            Op::Expect => OP_EXPECT,
            Op::Close => OP_CLOSE,
        }
    }
}

/// The flat operations' names on Clax's surface.
pub const OP_OPEN: &str = "browser.open";
pub const OP_PAGE: &str = "browser.page";
pub const OP_ACT: &str = "browser.act";
pub const OP_EXPECT: &str = "browser.expect";
pub const OP_CLOSE: &str = "browser.close";

/// What an argument must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Str,
    /// An integer, 0 or more.
    Int,
    /// A number.
    Num,
    Bool,
    /// A string or `{regex, flags?}`.
    Text,
    /// A [`Kind::Text`] or a list of them.
    Texts,
    /// A JSON object.
    Object,
    /// Anything: the driver reads it.
    Any,
    /// One of these strings.
    OneOf(&'static [&'static str]),
}

/// One field a call's `args` may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arg {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
}

const fn req(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: true,
    }
}

const fn opt(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: false,
    }
}

const TIMEOUT: Arg = opt("timeout", Kind::Int);
const NOT: Arg = opt("not", Kind::Bool);
const WAIT_UNTIL: Arg = opt(
    "waitUntil",
    Kind::OneOf(&["load", "domcontentloaded", "networkidle"]),
);
const BUTTON: Arg = opt("button", Kind::OneOf(&["left", "right", "middle"]));

/// One method of the layer (see the module docs).
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub class: &'static str,
    pub method: &'static str,
    pub op: Op,
    /// The action it sends; `None` for `close`, which has none.
    pub action: Option<&'static str>,
    /// Its positional parameters' names, `opts` for its options.
    pub params: &'static [&'static str],
    /// What its call's `args` may hold.
    pub args: &'static [Arg],
    /// The capability it needs beyond the session's own.
    pub needs: Option<Capability>,
}

const fn page(
    method: &'static str,
    action: &'static str,
    params: &'static [&'static str],
    args: &'static [Arg],
) -> Row {
    Row {
        class: "Page",
        method,
        op: Op::Page,
        action: Some(action),
        params,
        args,
        needs: None,
    }
}

const fn of(
    class: &'static str,
    method: &'static str,
    op: Op,
    action: &'static str,
    params: &'static [&'static str],
    args: &'static [Arg],
) -> Row {
    Row {
        class,
        method,
        op,
        action: Some(action),
        params,
        args,
        needs: None,
    }
}

const fn act(method: &'static str, params: &'static [&'static str], args: &'static [Arg]) -> Row {
    of("Locator", method, Op::Act, method, params, args)
}

const fn matcher(
    method: &'static str,
    params: &'static [&'static str],
    args: &'static [Arg],
) -> Row {
    of(
        "LocatorAssertions",
        method,
        Op::Expect,
        method,
        params,
        args,
    )
}

const O: &[&str] = &["opts"];
const T: &[Arg] = &[TIMEOUT];
const M: &[Arg] = &[TIMEOUT, NOT];

/// Every method of the layer that reaches the browser, in the prelude's
/// order.
pub const ROWS: &[Row] = &[
    page(
        "goto",
        "goto",
        &["url", "opts"],
        &[req("url", Kind::Str), WAIT_UNTIL, TIMEOUT],
    ),
    page("reload", "reload", O, &[WAIT_UNTIL, TIMEOUT]),
    page("goBack", "goBack", O, T),
    page("goForward", "goForward", O, T),
    page("title", "title", &[], &[]),
    page("content", "content", &[], &[]),
    page(
        "setViewportSize",
        "setViewportSize",
        &["size"],
        &[req("size", Kind::Object)],
    ),
    page(
        "waitForLoadState",
        "waitForLoadState",
        &["state", "opts"],
        &[
            opt(
                "state",
                Kind::OneOf(&["load", "domcontentloaded", "networkidle"]),
            ),
            TIMEOUT,
        ],
    ),
    page(
        "waitForURL",
        "waitForURL",
        &["url", "opts"],
        &[req("url", Kind::Text), TIMEOUT],
    ),
    page(
        "evaluate",
        "evaluate",
        &["fn", "arg"],
        &[
            opt("source", Kind::Str),
            opt("expression", Kind::Str),
            opt("arg", Kind::Any),
        ],
    ),
    page(
        "screenshot",
        "screenshot",
        O,
        &[opt("fullPage", Kind::Bool), TIMEOUT],
    ),
    page("consoleMessages", "consoleMessages", &[], &[]),
    page("pageErrors", "pageErrors", &[], &[]),
    page("failedRequests", "failedRequests", &[], &[]),
    Row {
        needs: Some(Capability::OpenArtifact),
        ..page(
            "runTests",
            "runTests",
            &["paths"],
            &[opt("paths", Kind::Any)],
        )
    },
    Row {
        class: "Page",
        method: "close",
        op: Op::Close,
        action: None,
        params: &[],
        args: &[],
        needs: None,
    },
    of(
        "Keyboard",
        "down",
        Op::Page,
        "keyboard.down",
        &["key"],
        &[req("key", Kind::Str)],
    ),
    of(
        "Keyboard",
        "up",
        Op::Page,
        "keyboard.up",
        &["key"],
        &[req("key", Kind::Str)],
    ),
    of(
        "Keyboard",
        "press",
        Op::Page,
        "keyboard.press",
        &["key"],
        &[req("key", Kind::Str)],
    ),
    of(
        "Keyboard",
        "type",
        Op::Page,
        "keyboard.type",
        &["text"],
        &[req("text", Kind::Str)],
    ),
    of(
        "Keyboard",
        "insertText",
        Op::Page,
        "keyboard.insertText",
        &["text"],
        &[req("text", Kind::Str)],
    ),
    of(
        "Mouse",
        "move",
        Op::Page,
        "mouse.move",
        &["x", "y", "opts"],
        &[
            req("x", Kind::Num),
            req("y", Kind::Num),
            opt("steps", Kind::Int),
            TIMEOUT,
        ],
    ),
    of(
        "Mouse",
        "down",
        Op::Page,
        "mouse.down",
        O,
        &[BUTTON, TIMEOUT],
    ),
    of("Mouse", "up", Op::Page, "mouse.up", O, &[BUTTON, TIMEOUT]),
    of(
        "Mouse",
        "click",
        Op::Page,
        "mouse.click",
        &["x", "y", "opts"],
        &[
            req("x", Kind::Num),
            req("y", Kind::Num),
            BUTTON,
            opt("clickCount", Kind::Int),
            TIMEOUT,
        ],
    ),
    of(
        "Mouse",
        "dblclick",
        Op::Page,
        "mouse.dblclick",
        &["x", "y", "opts"],
        &[req("x", Kind::Num), req("y", Kind::Num), BUTTON, TIMEOUT],
    ),
    of(
        "Mouse",
        "wheel",
        Op::Page,
        "mouse.wheel",
        &["deltaX", "deltaY"],
        &[req("deltaX", Kind::Num), req("deltaY", Kind::Num)],
    ),
    act(
        "click",
        O,
        &[
            BUTTON,
            opt("clickCount", Kind::Int),
            opt("position", Kind::Object),
            TIMEOUT,
        ],
    ),
    act(
        "dblclick",
        O,
        &[BUTTON, opt("position", Kind::Object), TIMEOUT],
    ),
    act(
        "fill",
        &["value", "opts"],
        &[req("value", Kind::Str), TIMEOUT],
    ),
    act("press", &["key", "opts"], &[req("key", Kind::Str), TIMEOUT]),
    act(
        "pressSequentially",
        &["text", "opts"],
        &[req("text", Kind::Str), TIMEOUT],
    ),
    act("hover", O, &[opt("position", Kind::Object), TIMEOUT]),
    act("focus", O, T),
    act("check", O, T),
    act("uncheck", O, T),
    act(
        "selectOption",
        &["values", "opts"],
        &[req("values", Kind::Any), TIMEOUT],
    ),
    act("textContent", O, T),
    act("innerText", O, T),
    act("inputValue", O, T),
    act(
        "getAttribute",
        &["name", "opts"],
        &[req("name", Kind::Str), TIMEOUT],
    ),
    act("allTextContents", &[], &[]),
    act("isVisible", &[], &[]),
    act("isHidden", &[], &[]),
    act("isEnabled", O, T),
    act("isChecked", O, T),
    act("count", &[], &[]),
    act(
        "waitFor",
        O,
        &[
            opt(
                "state",
                Kind::OneOf(&["attached", "detached", "visible", "hidden"]),
            ),
            TIMEOUT,
        ],
    ),
    act("scrollIntoViewIfNeeded", O, T),
    act("screenshot", O, T),
    matcher("toBeVisible", O, M),
    matcher("toBeHidden", O, M),
    matcher("toBeEnabled", O, M),
    matcher("toBeDisabled", O, M),
    matcher(
        "toBeChecked",
        O,
        &[opt("checked", Kind::Bool), TIMEOUT, NOT],
    ),
    matcher("toBeFocused", O, M),
    matcher(
        "toHaveText",
        &["expected", "opts"],
        &[req("expected", Kind::Texts), TIMEOUT, NOT],
    ),
    matcher(
        "toContainText",
        &["expected", "opts"],
        &[req("expected", Kind::Texts), TIMEOUT, NOT],
    ),
    matcher(
        "toHaveValue",
        &["value", "opts"],
        &[req("value", Kind::Text), TIMEOUT, NOT],
    ),
    matcher(
        "toHaveAttribute",
        &["name", "value", "opts"],
        &[
            req("name", Kind::Str),
            opt("value", Kind::Text),
            TIMEOUT,
            NOT,
        ],
    ),
    matcher(
        "toHaveCount",
        &["count", "opts"],
        &[req("count", Kind::Int), TIMEOUT, NOT],
    ),
    of(
        "PageAssertions",
        "toHaveURL",
        Op::Expect,
        "toHaveURL",
        &["url", "opts"],
        &[req("url", Kind::Text), TIMEOUT, NOT],
    ),
    of(
        "PageAssertions",
        "toHaveTitle",
        Op::Expect,
        "toHaveTitle",
        &["title", "opts"],
        &[req("title", Kind::Text), TIMEOUT, NOT],
    ),
];

/// The row of `action` of the flat operation `op`, when it has one. An
/// `expect` matcher on the page (an empty locator) is `PageAssertions`'.
#[must_use]
pub fn row(op: Op, action: &str, on_page: bool) -> Option<&'static Row> {
    ROWS.iter().find(|r| {
        r.op == op
            && r.action == Some(action)
            && (op != Op::Expect || (r.class == "PageAssertions") == on_page)
    })
}

/// The actions of `op`, in the table's order, each once.
#[must_use]
pub fn actions(op: Op) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for r in ROWS.iter().filter(|r| r.op == op) {
        if let Some(a) = r.action
            && !out.contains(&a)
        {
            out.push(a);
        }
    }
    out
}

fn is_text(v: &Value) -> bool {
    match v {
        Value::String(_) => true,
        Value::Object(o) => {
            o.get("regex").is_some_and(Value::is_string)
                && o.get("flags").is_none_or(Value::is_string)
                && o.keys().all(|k| k == "regex" || k == "flags")
        }
        _ => false,
    }
}

fn fits(kind: Kind, v: &Value) -> Result<(), String> {
    let ok = match kind {
        Kind::Str => v.is_string(),
        Kind::Int => v.as_u64().is_some(),
        Kind::Num => v.is_number(),
        Kind::Bool => v.is_boolean(),
        Kind::Text => is_text(v),
        Kind::Texts => is_text(v) || v.as_array().is_some_and(|a| a.iter().all(is_text)),
        Kind::Object => v.is_object(),
        Kind::Any => true,
        Kind::OneOf(names) => v.as_str().is_some_and(|s| names.contains(&s)),
    };
    if ok {
        return Ok(());
    }
    Err(match kind {
        Kind::Str => "must be a string".into(),
        Kind::Int => "must be a whole number, 0 or more".into(),
        Kind::Num => "must be a number".into(),
        Kind::Bool => "must be true or false".into(),
        Kind::Text => "must be a string or a RegExp".into(),
        Kind::Texts => "must be a string, a RegExp or a list of them".into(),
        Kind::Object => "must be an object".into(),
        Kind::Any => String::new(),
        Kind::OneOf(names) => format!("must be one of {}", names.join(", ")),
    })
}

/// Checks `args`, the `args` of a call of `row` (`null` for none): an
/// object holding only `row`'s fields, each of its kind, the required
/// ones present.
///
/// # Errors
/// What is wrong, naming the field (`args.url: must be a string`).
pub fn check(row: &Row, args: &Value) -> Result<(), String> {
    let empty = serde_json::Map::new();
    let o = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err("args: must be an object".into()),
    };
    let name = format!("{}.{}", row.class, row.method);
    for (k, v) in o {
        let Some(a) = row.args.iter().find(|a| a.name == k) else {
            return Err(format!("args.{k}: is not an option of {name}"));
        };
        if v.is_null() && !a.required {
            continue;
        }
        fits(a.kind, v).map_err(|why| format!("args.{k}: {why}"))?;
    }
    for a in row.args.iter().filter(|a| a.required) {
        if o.get(a.name).is_none_or(Value::is_null) {
            return Err(format!("args.{}: is required by {name}", a.name));
        }
    }
    Ok(())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_prelude_table_equals_the_layer_table() {
        let prelude = clax_bot_sandbox::browser_layer_table();
        let prelude = prelude.as_array().expect("an array");
        assert_eq!(prelude.len(), ROWS.len());
        for (p, r) in prelude.iter().zip(ROWS) {
            let mine = json!([r.class, r.method, r.op.name(), r.action, r.params]);
            assert_eq!(p, &mine);
        }
    }

    #[test]
    fn every_row_with_options_takes_a_timeout_and_every_matcher_not() {
        for r in ROWS {
            if r.params.contains(&"opts") {
                assert!(r.args.contains(&TIMEOUT), "{}.{}", r.class, r.method);
            }
            if r.op == Op::Expect {
                assert!(r.args.contains(&NOT), "{}.{}", r.class, r.method);
            }
        }
    }

    #[test]
    fn args_are_checked_by_their_row() {
        let goto = row(Op::Page, "goto", false).unwrap();
        assert_eq!(
            check(
                goto,
                &json!({"url": "/a", "waitUntil": "load", "timeout": 5})
            ),
            Ok(())
        );
        assert_eq!(
            check(goto, &json!({})),
            Err("args.url: is required by Page.goto".into())
        );
        assert_eq!(
            check(goto, &json!({"url": 1})),
            Err("args.url: must be a string".into())
        );
        assert_eq!(
            check(goto, &json!({"url": "/a", "force": true})),
            Err("args.force: is not an option of Page.goto".into())
        );
        let text = row(Op::Expect, "toHaveText", false).unwrap();
        assert_eq!(
            check(
                text,
                &json!({"expected": ["a", {"regex": "b", "flags": "i"}], "not": true})
            ),
            Ok(())
        );
        assert!(check(text, &json!({"expected": [1]})).is_err());
        let title = row(Op::Expect, "toHaveTitle", true).unwrap();
        assert_eq!(title.class, "PageAssertions");
        assert!(row(Op::Expect, "toHaveTitle", false).is_none());
        assert!(row(Op::Page, "click", false).is_none());
        assert_eq!(
            check(row(Op::Act, "count", false).unwrap(), &Value::Null),
            Ok(())
        );
    }
}
