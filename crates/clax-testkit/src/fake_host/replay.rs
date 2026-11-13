//! The contract fixture's script (`docs/claxbot-surfaces/contract.json`,
//! `script`), replayed from the host's side through a [`HostTransport`],
//! with a [`ScriptWorld`] doing what its run steps say.
//!
//! The fixture's placeholders: `$file:<name>` is the text of
//! `files[<name>]`; `$<name>` is an ID the daemon chose, bound when it is
//! first seen in an answer, the same throughout, and never the value of
//! another name; `$_` is any value.

use super::HostTransport;
use clax_surface::wire::{RegisterRequest, ServeRequest};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// What the script's `$` names stand for, and what differed so far.
#[derive(Debug, Default)]
pub struct Names {
    files: Map<String, Value>,
    names: BTreeMap<String, Value>,
    differences: Vec<String>,
}

impl Names {
    /// With the fixture's `files`.
    #[must_use]
    pub fn new(files: &Value) -> Names {
        Names {
            files: files.as_object().cloned().unwrap_or_default(),
            ..Names::default()
        }
    }

    /// What `$name` stands for, once bound.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.names.get(name)
    }

    /// `v` with its files and bound names filled in. A name not bound
    /// yet is a difference, and is left as it is.
    pub fn fill(&mut self, v: &Value, step: &Value) -> Value {
        match v {
            Value::String(s) => {
                if let Some(file) = s.strip_prefix("$file:") {
                    self.files.get(file).cloned().unwrap_or_else(|| {
                        self.differ(&format!("the fixture has no file {file}"), step);
                        v.clone()
                    })
                } else if s.starts_with('$') {
                    self.names.get(s).cloned().unwrap_or_else(|| {
                        self.differ(&format!("{s} is used before the daemon gave it"), step);
                        v.clone()
                    })
                } else {
                    v.clone()
                }
            }
            Value::Array(items) => items.iter().map(|i| self.fill(i, step)).collect(),
            Value::Object(o) => o
                .iter()
                .map(|(k, i)| (k.clone(), self.fill(i, step)))
                .collect::<Map<_, _>>()
                .into(),
            _ => v.clone(),
        }
    }

    /// Holds `got` to `want`: equal, but that `$_` is anything and a
    /// `$name` is whatever it first was and no other name's value.
    /// Members are compared as sets, not in order.
    pub fn hold(&mut self, want: &Value, got: &Value, step: &Value) {
        match (want, got) {
            (Value::String(s), _) if s == "$_" => {}
            (Value::String(s), _) if s.starts_with('$') && !s.starts_with("$file:") => {
                match self.names.get(s) {
                    Some(known) if known == got => {}
                    Some(known) => self.differ(&format!("{s} is {known}, and here {got}"), step),
                    None if self.names.values().any(|v| v == got) => {
                        self.differ(&format!("{s} is {got}, which another name is"), step);
                    }
                    None => {
                        self.names.insert(s.clone(), got.clone());
                    }
                }
            }
            (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
                for (w, g) in w.iter().zip(g) {
                    self.hold(w, g, step);
                }
            }
            (Value::Object(w), Value::Object(g))
                if w.keys().collect::<BTreeSet<_>>() == g.keys().collect::<BTreeSet<_>>() =>
            {
                for (key, w) in w {
                    self.hold(w, &g[key], step);
                }
            }
            _ => {
                let want = self.fill(want, step);
                if want != *got {
                    self.differ(&format!("expected {want}, got {got}"), step);
                }
            }
        }
    }

    /// Records a difference at `step` unless `holds`.
    pub fn expect(&mut self, holds: bool, what: &str, step: &Value) {
        if !holds {
            self.differ(what, step);
        }
    }

    /// Records a difference at `step`.
    pub fn differ(&mut self, what: &str, step: &Value) {
        let at = &step["at_ms"];
        let note = step["note"].as_str().unwrap_or("");
        self.differences
            .push(format!("at_ms {at} ({note}): {what}"));
    }

    /// What differed, in the script's order.
    #[must_use]
    pub fn differences(&self) -> &[String] {
        &self.differences
    }
}

/// The runs' side of the script, and its clock: whatever a test drives
/// runs and time with (in process, [`super::MemoryHub`]).
#[async_trait::async_trait]
pub trait ScriptWorld: Send {
    /// Moves the daemon's clock to `ms` of the script's, before the step
    /// at that instant.
    async fn at(&mut self, ms: u64);

    /// Does or checks a step with `run` (`call`, `outcome`, `waiting`,
    /// `ended`), binding and holding names through `names`.
    async fn run_step(&mut self, step: &Value, names: &mut Names);
}

/// Replays the fixture's script: each `host` step through `transport`,
/// its answer held to the step's `status` and `response`, and each `run`
/// step by `world`. A step's `held_ms` is the transport's to honour.
/// Answers the names as the script bound them, with what differed
/// ([`Names::differences`]): nothing, when the daemon does what the
/// script says.
pub async fn replay(
    fixture: &Value,
    transport: &impl HostTransport,
    world: &mut impl ScriptWorld,
) -> Names {
    let mut names = Names::new(&fixture["files"]);
    let script = fixture["script"].as_array().cloned().unwrap_or_default();
    for step in &script {
        world.at(step["at_ms"].as_u64().unwrap_or(0)).await;
        if step["run"].is_string() {
            world.run_step(step, &mut names).await;
            continue;
        }
        let surface = step["surface"].as_str().unwrap_or("");
        let asked = names.fill(&step["request"], step);
        let (status, body) = match step["host"].as_str().unwrap_or("") {
            "register" => match serde_json::from_value::<RegisterRequest>(asked) {
                Ok(asked) => transport.put_surface(surface, &asked).await,
                Err(e) => {
                    names.differ(&format!("not a RegisterRequest: {e}"), step);
                    continue;
                }
            },
            "serve" => match serde_json::from_value::<ServeRequest>(asked) {
                Ok(asked) => transport.serve(surface, &asked).await,
                Err(e) => {
                    names.differ(&format!("not a ServeRequest: {e}"), step);
                    continue;
                }
            },
            other => {
                names.differ(&format!("a step this replay does not know: {other}"), step);
                continue;
            }
        };
        if step["status"] != status {
            names.differ(&format!("status {status}: {body}"), step);
        }
        names.hold(&step["response"], &body, step);
    }
    names
}
