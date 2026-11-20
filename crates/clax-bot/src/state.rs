//! A run's `state` (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §17.1):
//! the one value a script leaves the next, kept with the script step
//! that wrote it.
//!
//! - **Stored** as the step's payload's `state`, `{value, handles}`: the
//!   value, each of Clax's handles in it as `{"$clax": i}`, and the handle
//!   entries (`clax_bot_sandbox::PRELUDE`, "State"). Written by the step's
//!   commit when it differs from the one its script started from; a
//!   script stopped before it could write one leaves none; either way the
//!   last one kept stands ([`last_committed`], [`empty`] before any).
//! - **Capped** at [`CAP_BYTES`] of JSON ([`fit`]): over it, the value's
//!   top-level members are left out, largest first, until it fits, each
//!   listed. Handle entries no marker of what is kept names are left out
//!   too, so a session a member left out held is not held ([`pages`]).
//! - **Told** to the model only when something was not kept ([`Fitted`]).
//! - **Shown** to people with each handle as `"<browser page>"` and the
//!   like, and no handle entry ([`shown`]): the entries hold session
//!   capabilities (§17.3), which never leave the store.

use clax_bot_sandbox::{Dropped, StateOut};
use clax_store::bot_runs::{Step, StepKind};
use serde_json::{Map, Value, json};

/// The most bytes a run's stored state may take as JSON: 64 KiB.
pub const CAP_BYTES: usize = 64 << 10;

/// The state a run's next script starts from: the one the last committed
/// script step of `steps` kept, if any did.
#[must_use]
pub fn last_committed(steps: &[Step]) -> Option<Value> {
    steps
        .iter()
        .rev()
        .filter(|s| s.kind == StepKind::Script && s.committed_at.is_some())
        .find_map(|s| {
            s.payload
                .as_ref()
                .and_then(|p| p.get("state"))
                .filter(|v| v["value"].is_object())
                .cloned()
        })
}

/// The state of a run no script has changed: `{}` with no handles.
#[must_use]
pub fn empty() -> Value {
    json!({"value": {}, "handles": []})
}

/// What a step keeps and tells of the state its script left.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    /// `{value, handles}` to store, within [`CAP_BYTES`]; `None` when the
    /// script wrote none.
    pub saved: Option<Value>,
    /// The tool result's `state`, when there is something to tell.
    pub told: Option<Value>,
}

/// The marker `{"$clax": i}`'s index, when `v` is one.
fn marker(v: &Value) -> Option<usize> {
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    usize::try_from(o.get("$clax")?.as_u64()?).ok()
}

/// Every marker's index in `v`, as they are written out.
fn markers(v: &Value, out: &mut Vec<usize>) {
    if let Some(i) = marker(v) {
        out.push(i);
        return;
    }
    match v {
        Value::Array(items) => items.iter().for_each(|x| markers(x, out)),
        Value::Object(o) => o.values().for_each(|x| markers(x, out)),
        _ => {}
    }
}

/// `v` with each marker given by `f` instead.
fn remark(v: &Value, f: &dyn Fn(usize) -> Value) -> Value {
    if let Some(i) = marker(v) {
        return f(i);
    }
    match v {
        Value::Array(items) => Value::Array(items.iter().map(|x| remark(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), remark(x, f))).collect()),
        other => other.clone(),
    }
}

/// `value` and only the entries of `handles` its markers name, its
/// markers numbered again to match.
fn prune(value: Value, handles: &[Value]) -> Value {
    let mut used = Vec::new();
    markers(&value, &mut used);
    let mut kept: Vec<usize> = Vec::new();
    for i in used {
        if i < handles.len() && !kept.contains(&i) {
            kept.push(i);
        }
    }
    let value = remark(&value, &|i| match kept.iter().position(|k| *k == i) {
        Some(at) => json!({"$clax": at}),
        None => Value::Null,
    });
    let handles: Vec<Value> = kept.iter().map(|i| handles[*i].clone()).collect();
    json!({"value": value, "handles": handles})
}

/// What a step keeps and tells of `left`, the state its script left
/// ([`clax_bot_sandbox::ScriptOutcome::state`]); `stopped` says why there
/// is none, for a script the sandbox stopped before it could write one
/// (`None` for one that never ran, which has nothing to tell).
#[must_use]
pub fn fit(left: Option<StateOut>, stopped: Option<&str>) -> Fitted {
    let Some(out) = left else {
        return Fitted {
            saved: None,
            told: stopped.map(|why| {
                json!({"saved": false, "why": format!(
                    "{why}, before its state could be written: the state the last script left stands"
                )})
            }),
        };
    };
    let mut dropped: Vec<Dropped> = out.dropped;
    let mut more = out.more;
    let mut members = match out.value {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let size = |m: &Map<String, Value>| prune(Value::Object(m.clone()), &out.handles).to_string().len();
    while size(&members) > CAP_BYTES && !members.is_empty() {
        let (key, bytes) = members
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string().len()))
            .max_by_key(|(_, n)| *n)
            .expect("a member");
        members.remove(&key);
        let path = if key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        {
            format!("state.{key}")
        } else {
            format!("state[{}]", json!(key))
        };
        let d = Dropped {
            path,
            why: format!("too large ({bytes} bytes; state keeps {CAP_BYTES} bytes at most)"),
        };
        if dropped.len() < 20 {
            dropped.push(d);
        } else {
            more += 1;
        }
    }
    let saved = prune(Value::Object(members), &out.handles);
    let told = (!dropped.is_empty() || more > 0).then(|| {
        let mut t = json!({"dropped": dropped});
        if more > 0 {
            t["more"] = json!(more);
        }
        t
    });
    Fitted {
        saved: Some(saved),
        told,
    }
}

/// The page capabilities a stored state holds (each handle entry's
/// `page`), each once: the sessions it keeps open (spec §17.4).
#[must_use]
pub fn pages(saved: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in saved["handles"].as_array().into_iter().flatten() {
        if let Some(p) = h["page"].as_str()
            && !out.iter().any(|o| o == p)
        {
            out.push(p.to_string());
        }
    }
    out
}

/// A stored state as people are shown it: its value, each handle as
/// `"<browser page>"`, `"<browser locator>"` and the like, and nothing of
/// its handle entries.
#[must_use]
pub fn shown(saved: &Value) -> Value {
    let handles = saved["handles"].as_array().cloned().unwrap_or_default();
    remark(&saved["value"], &|i| {
        let of = handles.get(i).and_then(|h| h["of"].as_str()).unwrap_or("");
        json!(match of {
            "Page" => "<browser page>",
            "Locator" => "<browser locator>",
            "Keyboard" => "<browser keyboard>",
            "Mouse" => "<browser mouse>",
            _ => "<browser assertion>",
        })
    })
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn out(value: Value, handles: Value) -> StateOut {
        StateOut {
            value,
            handles: serde_json::from_value(handles).unwrap(),
            ..StateOut::default()
        }
    }

    #[test]
    fn a_state_kept_whole_tells_nothing() {
        let f = fit(Some(out(json!({"a": 1}), json!([]))), None);
        assert_eq!(f.saved, Some(json!({"value": {"a": 1}, "handles": []})));
        assert_eq!(f.told, None);
        assert_eq!(fit(None, None), Fitted { saved: None, told: None });
        let stopped = fit(None, Some("the script ran out of memory"));
        assert_eq!(stopped.told.unwrap()["saved"], false);
    }

    /// Over the cap, the largest members go first, each listed, and a
    /// handle only they held goes with them.
    #[test]
    fn a_state_over_its_cap_leaves_out_its_largest_members() {
        let big = "x".repeat(CAP_BYTES);
        let left = out(
            json!({"big": big, "p": {"$clax": 1}, "huge list": [big, {"$clax": 0}], "n": 1}),
            json!([{"page": "a", "of": "Page"}, {"page": "b", "of": "Page"}]),
        );
        let f = fit(Some(left), None);
        let saved = f.saved.unwrap();
        assert_eq!(
            saved,
            json!({"value": {"p": {"$clax": 0}, "n": 1}, "handles": [{"page": "b", "of": "Page"}]})
        );
        assert_eq!(pages(&saved), ["b"]);
        let told = f.told.unwrap();
        let paths: Vec<&str> = told["dropped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["state[\"huge list\"]", "state.big"]);
        assert!(saved.to_string().len() <= CAP_BYTES);
    }

    #[test]
    fn a_shown_state_has_no_capability() {
        let saved = json!({"value": {"p": {"$clax": 0}, "l": [{"$clax": 1}], "n": 1},
                           "handles": [{"page": "secret", "of": "Page"},
                                       {"page": "secret", "of": "Locator"}]});
        let s = shown(&saved);
        assert_eq!(s, json!({"p": "<browser page>", "l": ["<browser locator>"], "n": 1}));
        assert!(!s.to_string().contains("secret"));
    }
}
