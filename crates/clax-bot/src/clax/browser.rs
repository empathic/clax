//! `browser.test` and `browser.browse` in a session of Clax's own
//! surface (the session's browser part, spec 2026-11-19-claxbot-browser
//! §5, §8).
//!
//! A browser operation the run's binding lacks is refused by the run
//! loop before the session sees it
//! ([`crate::surface::SurfaceProvider::undeclared`], [`disabled`]), and
//! refused the same here by a session handed one all the same: charged
//! nothing, journaled by the run loop, `browser_disabled`.

use super::session::ClaxSession;
use crate::browser::{CODE_BROWSER_DISABLED, OP_BROWSE, OP_TEST};
use crate::surface::{Answer, Call, Fields};
use clax_bot_sandbox::HostError;
use serde_json::{Value, json};

/// What the run shows of a browser call beside its answer: `browser`,
/// its summary.
fn showing(browser: Value) -> Fields {
    Fields::from_iter([("browser".to_string(), browser)])
}

/// What the run shows of a call to `op` on `args` before it is
/// performed: the target as the script gave it, or the URL. `None` for
/// an operation that is not the browser's.
fn shown(op: &str, args: &[Value]) -> Option<Value> {
    match op {
        OP_TEST => Some(json!({"target": args.first().cloned().unwrap_or(Value::Null)})),
        OP_BROWSE => Some(json!({"url": args.first().cloned().unwrap_or(Value::Null)})),
        _ => None,
    }
}

/// What a script is answered for a browser operation its run's binding
/// does not have, and for one it has on a host with no browser source:
/// refused, charged nothing, with what the run shows of the call, for
/// the run loop to journal; `None` for an operation that is not the
/// browser's.
pub(super) fn disabled(op: &str, args: &[Value]) -> Option<Answer> {
    let browser = shown(op, args)?;
    let refusal = HostError::new(
        CODE_BROWSER_DISABLED,
        format!("clax.{op} is not available to this run"),
    );
    Some(
        Answer::refused(refusal)
            .uncharged()
            .showing(showing(browser)),
    )
}

impl ClaxSession<'_> {
    /// Performs the browser call `call`: no host performs one yet, so
    /// each is refused as on a host with no browser source.
    pub(super) async fn browser(&self, call: &Call<'_>) -> Answer {
        disabled(call.op, call.args).unwrap_or_else(|| {
            Answer::refused(HostError::new(
                crate::api::CODE_UNKNOWN_OP,
                format!("clax.{} is not a browser operation", call.op),
            ))
        })
    }
}
