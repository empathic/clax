//! The tool that ends a run, `finish`: what it takes, and what the run
//! loop makes of a call of it.
//!
//! A run's kind is a function of its surface's file
//! (`name(request: Req): Result`), and `finish` carries its value: the
//! tool's input schema is the kind's result
//! ([`clax_surface::Kind::result_schema`]), and its description is
//! [`ENDS`] followed by the doc comment of the result's interface, when
//! it is one that has one. For Clax's own kinds (`thread`, `brief` in
//! `src/clax.d.ts`) the result is `Done`, `{summary: string}`, whose doc
//! comment completes the description the tool has always had.
//!
//! What a call is read as depends on the surface that declared the kind:
//!
//! - **`clax`**: as before kinds had types. A `summary` that is a string
//!   finishes the run with `{summary}` (other keys of the input are not
//!   read); any other input is answered that it does not match the
//!   tool's schema, with nothing counted and no bound but the run's own.
//! - **Any other surface**: the input is checked with
//!   [`clax_surface::Kind::check_result`]. A value that passes is the
//!   run's result: the run finishes with it, and the terminal step keeps
//!   it as `result`. One that fails is no ending: the model is answered
//!   [`INVALID_RESULT`] with the checker's message, as the call's tool
//!   result, and the run goes on. The turn's `next` counts such answers
//!   in a row (`invalid_results`): any other answer to a turn (another
//!   tool's call, one whose input does not match its tool's schema, a
//!   turn with no call that is nudged) and any other step (a script)
//!   ends the row. When [`crate::policy::RunLimits::max_invalid_results`]
//!   answers have been given in a row, the next result that fails gives
//!   the run up, with the code [`INVALID_RESULT`].
//!
//! A run whose kind its surface's definition does not declare stops at
//! its first turn (`request_gone`), since its request cannot be rendered;
//! should one reach `finish`, no value passes.

use crate::ending::Ending;
use crate::policy;
use crate::surface::{Fields, RunInfo};
use clax_sample::messages::Block;
use clax_surface::{CLAX, Definition, Kind, Ty};
use serde_json::{Value, json};

/// The code of a result that failed its kind's check: the tool result
/// the model is answered with starts with it, and a run whose model sent
/// one too many in a row gives up with it.
pub const INVALID_RESULT: &str = "invalid_result";

/// What the description of `finish` starts with, for every kind.
pub const ENDS: &str = "Ends the run: the work is done.";

/// How a run's `finish` is offered and read: see the module
/// documentation.
pub(crate) struct Finish<'a> {
    rule: Rule<'a>,
    /// How many results in a row the model was answered
    /// [`INVALID_RESULT`] before this turn.
    invalid: u64,
    /// [`crate::policy::RunLimits::max_invalid_results`].
    limit: Option<u32>,
}

enum Rule<'a> {
    /// A kind of Clax's own surface.
    Clax(&'a Definition, &'a Kind),
    /// A kind of another surface.
    Typed(&'a Definition, &'a Kind),
    /// The run's kind (`<surface>.<kind>`), which its surface's
    /// definition does not declare.
    Undeclared(&'a str),
}

impl<'a> Finish<'a> {
    /// The `finish` of a run that `info` describes, whose kind's surface
    /// is defined by `def` (the definition of the kind's provider,
    /// `RunSurfaces::kind_provider`, which a run has whether or not its
    /// surface is among the run's bindings). `limit` is
    /// [`crate::policy::RunLimits::max_invalid_results`] and `last` the
    /// step the last committed turn asked for (`Control::next`), which
    /// counts the results already refused in a row.
    pub(crate) fn of(
        info: &'a RunInfo,
        def: &'a Definition,
        limit: Option<u32>,
        last: &Value,
    ) -> Finish<'a> {
        let (surface, name) = info.kind.split_once('.').unwrap_or((&info.kind, ""));
        let rule = match def.kind(name) {
            Some(k) if surface == CLAX => Rule::Clax(def, k),
            Some(k) => Rule::Typed(def, k),
            None => Rule::Undeclared(&info.kind),
        };
        Finish {
            rule,
            invalid: last["invalid_results"].as_u64().unwrap_or(0),
            limit,
        }
    }

    /// The tool's description and input schema.
    pub(crate) fn tool(&self) -> (String, Value) {
        let (def, kind) = match self.rule {
            Rule::Clax(d, k) | Rule::Typed(d, k) => (d, k),
            Rule::Undeclared(_) => return (ENDS.to_string(), json!({"type": "object"})),
        };
        let doc = match &kind.result {
            Ty::Named(n) => def.interface(n).and_then(|i| i.doc.as_deref()),
            _ => None,
        };
        let description = match doc {
            Some(doc) => format!("{ENDS} {doc}"),
            None => ENDS.to_string(),
        };
        // Every kind of a definition has a schema (its reader refuses a
        // result that is not an object): none is a kind made otherwise,
        // and nothing passes its check.
        match kind.result_schema(def) {
            Some(schema) => (description, schema),
            None => (ENDS.to_string(), json!({"type": "object"})),
        }
    }

    /// What a call of `finish` with `input` (its ID `id`) asks for next:
    /// the run's `next`. `None` for an input of a `clax` kind without a
    /// string `summary`, which the run loop answers as any input that
    /// does not match its tool's schema.
    pub(crate) fn read(&self, id: &str, input: &Value) -> Option<Value> {
        let refused = match self.rule {
            Rule::Clax(..) => {
                return input["summary"]
                    .as_str()
                    .map(|summary| json!({"kind": "finish", "summary": summary}));
            }
            Rule::Typed(def, kind) => match kind.check_result(def, input) {
                Ok(()) => return Some(json!({"kind": "finish", "result": input})),
                Err(e) => e.message,
            },
            Rule::Undeclared(kind) => {
                format!("the run's kind {kind} is not declared by its surface")
            }
        };
        let why = policy::error_text(&refused);
        if self
            .limit
            .is_some_and(|limit| self.invalid >= u64::from(limit))
        {
            return Some(
                json!({"kind": "give_up", "code": INVALID_RESULT, "reason": format!(
                    "The model's result did not match the run's kind {} times in a row: {why}",
                    self.invalid + 1
                )}),
            );
        }
        Some(json!({
            "kind": "turn",
            "invalid_results": self.invalid + 1,
            "user": [Block::tool_result(id, format!("{INVALID_RESULT}: {why}"), true).to_json()],
        }))
    }
}

/// The ending of a run whose model's `finish` asked for `next`
/// ([`Finish::read`]): the kind's result, which the terminal step keeps
/// as `result`; or, for a kind of `clax`, `{summary}`, which it keeps as
/// `summary`, as it always has.
pub(crate) fn ending(next: &Value) -> Ending {
    match next.get("result") {
        Some(result) => {
            let mut core = Fields::new();
            core.insert("result".into(), result.clone());
            Ending {
                result: Some(result.clone()),
                core,
                ..Ending::finished("")
            }
        }
        None => Ending::finished(next["summary"].as_str().unwrap_or("")),
    }
}
