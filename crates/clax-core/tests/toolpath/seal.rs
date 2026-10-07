//! A test-only reader of `.path.jsonl` streams: the JSONL RFC's "Reading
//! JSONL" algorithm (Toolpath `docs/RFC-jsonl.md` at commit `77dc16a5`),
//! sealing a stream into the single-path `Graph` the RFC says a file is at
//! its boundary, so it can be validated against the Toolpath schema.
//!
//! Fatal, as the RFC requires: a first line that is not a valid
//! `PathOpen`, malformed JSON on any line, a line that is not newline
//! terminated, a step `Signature` before its `Step`, and an ambiguous or
//! missing head at EOF with no `Head` line. Unknown variants, and an
//! unknown `PathOpen` version, are skipped (and counted in `warnings`).

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// A sealed stream: the single-path graph, and the warnings the RFC says a
/// reader should raise.
#[derive(Debug)]
pub struct Sealed {
    pub graph: Value,
    pub warnings: Vec<String>,
}

/// Reads `text` as a `.path.jsonl` stream and seals it into a `Graph`
/// whose `graph.id` is the path's `graph_ref`, or else its ID.
pub fn seal(text: &str) -> Result<Sealed, String> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("the final line is not newline-terminated".into());
    }
    let mut lines = text.split_terminator('\n').enumerate();
    let (_, first) = lines.next().ok_or("the stream is empty")?;
    let first: Value = serde_json::from_str(first).map_err(|e| format!("line 1: {e}"))?;
    let open = first
        .get("PathOpen")
        .and_then(Value::as_object)
        .ok_or("line 1 is not a PathOpen")?;
    let mut warnings = Vec::new();
    let version = open
        .get("version")
        .and_then(Value::as_str)
        .ok_or("PathOpen has no version")?;
    if version != "1" {
        warnings.push(format!("unknown PathOpen version {version}"));
    }
    let path_id = open
        .get("id")
        .and_then(Value::as_str)
        .ok_or("PathOpen has no id")?
        .to_string();
    let base = open.get("base").cloned();
    let graph_ref = open.get("graph_ref").cloned();
    let mut path_meta: Map<String, Value> = match open.get("meta") {
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err("PathOpen meta is not an object".into()),
        None => Map::new(),
    };
    let mut actors: BTreeMap<String, Value> = BTreeMap::new();
    let mut path_sigs: Vec<Value> = Vec::new();
    let mut steps: Vec<Value> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut head: Option<String> = None;

    for (i, line) in lines {
        let n = i + 1;
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {n}: {e}"))?;
        let obj = v
            .as_object()
            .filter(|o| o.len() == 1)
            .ok_or(format!("line {n} is not one externally tagged object"))?;
        let (variant, body) = obj.iter().next().expect("one entry");
        match variant.as_str() {
            "Step" => {
                let id = body
                    .pointer("/step/id")
                    .and_then(Value::as_str)
                    .ok_or(format!("line {n}: a Step without step.id"))?
                    .to_string();
                index.insert(id, steps.len());
                steps.push(body.clone());
            }
            "ActorDef" => {
                let actor = body
                    .get("actor")
                    .and_then(Value::as_str)
                    .ok_or(format!("line {n}: an ActorDef without actor"))?;
                let def = body
                    .get("definition")
                    .ok_or(format!("line {n}: an ActorDef without definition"))?;
                actors.insert(actor.to_string(), def.clone());
            }
            "Signature" => {
                let target = body
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or(format!("line {n}: a Signature without target"))?;
                let sig = body
                    .get("signature")
                    .cloned()
                    .ok_or(format!("line {n}: a Signature without signature"))?;
                if target == "path" {
                    path_sigs.push(sig);
                } else if let Some(id) = target.strip_prefix("step:") {
                    let at = *index
                        .get(id)
                        .ok_or(format!("line {n}: a Signature for step {id} before it"))?;
                    let meta = steps[at]
                        .as_object_mut()
                        .expect("a step is an object")
                        .entry("meta")
                        .or_insert_with(|| json!({}));
                    meta.as_object_mut()
                        .ok_or(format!("line {n}: step {id} meta is not an object"))?
                        .entry("signatures")
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .ok_or(format!("line {n}: step {id} signatures is not an array"))?
                        .push(sig);
                } else {
                    return Err(format!("line {n}: an unknown Signature target"));
                }
            }
            "PathMeta" => {
                let patch = body
                    .get("patch")
                    .and_then(Value::as_object)
                    .ok_or(format!("line {n}: a PathMeta without a patch object"))?;
                for (k, v) in patch {
                    path_meta.insert(k.clone(), v.clone());
                }
            }
            "Head" => {
                head = Some(
                    body.get("step_id")
                        .and_then(Value::as_str)
                        .ok_or(format!("line {n}: a Head without step_id"))?
                        .to_string(),
                );
            }
            "PathClose" => {}
            other => warnings.push(format!("line {n}: unknown variant {other}")),
        }
    }

    let head = match head {
        Some(h) => h,
        None => {
            let parents: std::collections::BTreeSet<&str> = steps
                .iter()
                .filter_map(|s| s.pointer("/step/parents").and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let tips: Vec<&str> = steps
                .iter()
                .filter_map(|s| s.pointer("/step/id").and_then(Value::as_str))
                .filter(|id| !parents.contains(id))
                .collect();
            match tips.as_slice() {
                [one] => one.to_string(),
                _ => return Err("ambiguous or missing head".into()),
            }
        }
    };

    if !actors.is_empty() {
        path_meta.insert("actors".into(), json!(actors));
    }
    if !path_sigs.is_empty() {
        path_meta.insert("signatures".into(), Value::Array(path_sigs));
    }
    let mut identity = json!({"id": path_id, "head": head});
    if let Some(b) = base {
        identity["base"] = b;
    }
    if let Some(g) = &graph_ref {
        identity["graph_ref"] = g.clone();
    }
    let graph_id = graph_ref
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or(&path_id)
        .to_string();
    let mut path = json!({"path": identity, "steps": steps});
    if !path_meta.is_empty() {
        path["meta"] = Value::Object(path_meta);
    }
    Ok(Sealed {
        graph: json!({"graph": {"id": graph_id}, "paths": [path]}),
        warnings,
    })
}
