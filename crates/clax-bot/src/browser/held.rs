//! A call of a page exported to a machine grant (spec
//! 2026-11-19-claxbot-browser-design §18.5, option A's transport): what
//! the grant's holder may ask of the page, read from its request the way
//! a script's call is read ([`layer`]'s rows, the locator's steps), and
//! the answer it is given.
//!
//! - **What it may ask**: a page action, a locator action or an `expect`
//!   matcher of the layer's table, with the `args` its row takes, or
//!   `close`. `share` and `revoke` are a run's own (a holder narrows
//!   nothing: the person exports again); `record` and `runTests` keep
//!   what they make in the run's content and its tests, which a holder
//!   does not reach, and are refused `browser_disabled`.
//! - **Its class** ([`Class`]) is its row's, which the capability's
//!   `actions` caveat and the export's grant row hold; a `goto`, `reload`,
//!   `goBack` or `goForward` names where it goes ([`Nav`]) for the
//!   `origin` caveat.
//! - **The answer** is the script's (`ok`, `url`, `ms`, `cost`, then
//!   `value`, or `error` and `timeout`), but for a screenshot, which a
//!   script gets as an item of the run's content and a holder as the
//!   image itself, `{"media_type": "image/png", "data": <base64>}`.

use super::cap::Class;
use super::layer::{self, Op};
use super::live::{Answered, Nav, SessionCall};
use super::{BrowserError, CODE_BROWSER_DISABLED};
use crate::api::CODE_BAD_ARGS;
use base64::Engine as _;
use serde_json::{Value, json};

/// A holder's call, read ([`parse`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HeldCall {
    /// The call to perform; `None` for `close`.
    pub call: Option<SessionCall>,
    /// The class its capability must allow.
    pub class: Class,
    /// Where it goes, for an `origin` caveat.
    pub nav: Nav,
}

fn bad(why: impl Into<String>) -> BrowserError {
    BrowserError::new(CODE_BAD_ARGS, why, false)
}

/// `v` as an action's `args`: an object (`{}` for none).
fn args_of(v: Option<&Value>) -> Result<Value, BrowserError> {
    match v {
        None | Some(Value::Null) => Ok(json!({})),
        Some(o @ Value::Object(_)) => Ok(o.clone()),
        Some(_) => Err(bad("args: must be an object")),
    }
}

/// The call of operation `op` (`page`, `act`, `expect` or `close`), its
/// `action` (a matcher for `expect`), `locator` and `args`, as a holder
/// sends it (see the module docs).
///
/// # Errors
/// `invalid_args` for an operation, action, locator or option the layer
/// does not take, and for `share` and `revoke`; `browser_disabled` for
/// `record` and `runTests`.
pub fn parse(
    op: &str,
    action: Option<&str>,
    locator: Option<&Value>,
    args: Option<&Value>,
) -> Result<HeldCall, BrowserError> {
    let op = match op {
        "page" => Op::Page,
        "act" => Op::Act,
        "expect" => Op::Expect,
        "close" => {
            return Ok(HeldCall {
                call: None,
                class: Class::Close,
                nav: Nav::None,
            });
        }
        _ => return Err(bad("op: must be one of page, act, expect, close")),
    };
    let action = action.ok_or_else(|| bad("action: must be a string"))?;
    let locator_of = |v: Option<&Value>| -> Result<Value, BrowserError> {
        let v = v.cloned().unwrap_or(Value::Null);
        super::locate::parse_locator(&v).map_err(|e| bad(format!("locator: {e}")))?;
        Ok(v)
    };
    let on_page = op == Op::Expect
        && locator.is_none_or(|l| l.is_null() || l.as_array().is_some_and(Vec::is_empty));
    let row = layer::row(op, action, on_page).ok_or_else(|| {
        bad(format!(
            "action: `{action}` is not {} action",
            match op {
                Op::Page => "a page",
                Op::Act => "a locator",
                _ => "an expect",
            }
        ))
    })?;
    if matches!(action, "share" | "revoke") {
        return Err(bad(format!(
            "action: `{action}` is a run's own; ask the person to export the page again"
        )));
    }
    if matches!(action, "record" | "runTests") {
        return Err(BrowserError::new(
            CODE_BROWSER_DISABLED,
            format!("page.{action} is not served to an exported page"),
            false,
        ));
    }
    let args = args_of(args)?;
    layer::check(row, &args).map_err(bad)?;
    let class = layer::class(row).ok_or_else(|| bad(format!("action: `{action}`")))?;
    let call = match op {
        Op::Page => SessionCall::Page {
            action: action.to_string(),
            args,
        },
        Op::Act => SessionCall::Act {
            locator: locator_of(locator)?,
            action: action.to_string(),
            args,
        },
        _ => SessionCall::Expect {
            locator: if on_page {
                None
            } else {
                Some(locator_of(locator)?)
            },
            matcher: action.to_string(),
            args,
        },
    };
    let nav = match &call {
        SessionCall::Page { action, args } => match action.as_str() {
            "goto" => Nav::Goto(args["url"].as_str().unwrap_or_default().to_string()),
            "reload" => Nav::Reload,
            "goBack" | "goForward" => Nav::History,
            _ => Nav::None,
        },
        _ => Nav::None,
    };
    Ok(HeldCall {
        call: Some(call),
        class,
        nav,
    })
}

/// What a holder's call answers: `a`, the page's answer (none for a
/// close), charged `ms` milliseconds of browser time for `cost_milli`
/// milli-credits (see the module docs).
#[must_use]
pub fn answer(a: Option<&Answered>, ms: i64, cost_milli: i64) -> Value {
    let cost = cost_milli as f64 / 1000.0;
    let Some(a) = a else {
        return json!({"ok": true, "ms": ms, "cost": cost});
    };
    let mut v = json!({"ok": a.ok, "url": a.url, "ms": ms, "cost": cost});
    if a.ok {
        let mut value = a.value.clone();
        if let Some(i) = value["screenshot"].as_u64()
            && let Some(png) = a.shots.get(usize::try_from(i).unwrap_or(usize::MAX))
        {
            value = json!({
                "media_type": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
            });
        }
        v["value"] = value;
    } else {
        v["error"] = json!(a.error.clone().unwrap_or_default());
        if a.timeout {
            v["timeout"] = json!(true);
        }
    }
    v
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_read_by_the_layers_rows_and_classed() {
        let c = parse("page", Some("title"), None, None).unwrap();
        assert_eq!((c.class, c.nav), (Class::Read, Nav::None));
        let c = parse(
            "page",
            Some("goto"),
            None,
            Some(&json!({"url": "https://example.com/"})),
        )
        .unwrap();
        assert_eq!(
            (c.class, c.nav),
            (Class::Navigate, Nav::Goto("https://example.com/".into()))
        );
        let c = parse(
            "act",
            Some("click"),
            Some(&json!([{"css": "button"}])),
            None,
        )
        .unwrap();
        assert_eq!(c.class, Class::Act);
        let c = parse("close", None, None, None).unwrap();
        assert_eq!((c.call, c.class), (None, Class::Close));
    }

    #[test]
    fn what_a_holder_may_not_ask_is_refused() {
        for (op, action, code) in [
            ("page", "share", CODE_BAD_ARGS),
            ("page", "revoke", CODE_BAD_ARGS),
            ("page", "record", CODE_BROWSER_DISABLED),
            ("page", "runTests", CODE_BROWSER_DISABLED),
            ("page", "nope", CODE_BAD_ARGS),
            ("drive", "title", CODE_BAD_ARGS),
        ] {
            let e = parse(op, Some(action), None, None).unwrap_err();
            assert_eq!(e.code, code, "{op} {action}");
        }
        let e = parse("page", Some("title"), None, Some(&json!([1]))).unwrap_err();
        assert_eq!(e.code, CODE_BAD_ARGS);
    }

    #[test]
    fn a_screenshot_is_answered_as_the_image() {
        let a = Answered {
            ok: true,
            value: json!({"screenshot": 0}),
            error: None,
            timeout: false,
            url: "https://example.com/".into(),
            at_ms: 10,
            shots: vec![vec![1, 2, 3]],
            kind: None,
        };
        let v = answer(Some(&a), 10, 5);
        assert_eq!(v["value"]["media_type"], "image/png");
        assert_eq!(v["value"]["data"], "AQID");
        assert_eq!(v["cost"], 0.005);
        assert_eq!(
            answer(None, 3, 0),
            json!({"ok": true, "ms": 3, "cost": 0.0})
        );
    }
}
