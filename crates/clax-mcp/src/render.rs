//! Tool result rendering: every result's first text block holds a pretty-printed
//! JSON object that carries a `feedback` array; a success that hands over
//! feedback adds a second, trailing text block that describes it as prose.

use crate::client::ClientError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value, json};
use std::path::Path;

/// The error block: `obj` with an empty `feedback` array.
fn text(mut obj: Map<String, Value>) -> ContentBlock {
    obj.insert("feedback".into(), json!([]));
    ContentBlock::text(
        serde_json::to_string_pretty(&Value::Object(obj)).expect("JSON values serialise"),
    )
}

/// What a feedback poll handed over: comments (`feedback`), late answers
/// to the session's questions (`answers`), placeholders for comments held
/// because their bodies are withheld from the session's person
/// (`withheld`), and the prose for all of them (`text`).
#[derive(Debug, Default)]
pub struct Handover {
    pub feedback: Vec<Value>,
    pub answers: Vec<Value>,
    pub withheld: Vec<Value>,
    /// Notes from Clax itself, such as what a restored session's end
    /// released (spec 2026-10-08 §6.7).
    pub notes: Vec<Value>,
    pub text: Option<String>,
}

impl Handover {
    /// The handover in a `GET /api/sessions/<sid>/feedback` response.
    pub fn of(res: &Value) -> Handover {
        let list = |k: &str| res[k].as_array().cloned().unwrap_or_default();
        Handover {
            feedback: list("feedback"),
            answers: list("answers"),
            withheld: list("withheld"),
            notes: list("notes"),
            text: res["text"].as_str().map(str::to_string),
        }
    }

    /// Whether nothing was handed over.
    pub fn is_empty(&self) -> bool {
        self.feedback.is_empty()
            && self.answers.is_empty()
            && self.withheld.is_empty()
            && self.notes.is_empty()
    }
}

/// A success result for `value`, which must be a JSON object, with an empty
/// `feedback` array.
pub fn success(value: Value) -> CallToolResult {
    success_with(value, Handover::default())
}

/// A success result for `value` (a JSON object) whose `feedback` array is
/// `h.feedback`, with `answers` (the late answers), `withheld` (the
/// placeholders) and `notes` when there are any. When
/// something was handed over and `h.text` is given, a second text block
/// `---\n<text>` follows: the trailing block agents read as prose.
pub fn success_with(value: Value, h: Handover) -> CallToolResult {
    let Value::Object(mut obj) = value else {
        panic!("tool results are JSON objects");
    };
    let trailing = h
        .text
        .clone()
        .filter(|_| !h.is_empty())
        .map(|t| ContentBlock::text(format!("---\n{t}")));
    obj.insert("feedback".into(), Value::Array(h.feedback));
    if !h.answers.is_empty() {
        obj.insert("answers".into(), Value::Array(h.answers));
    }
    if !h.withheld.is_empty() {
        obj.insert("withheld".into(), Value::Array(h.withheld));
    }
    if !h.notes.is_empty() {
        obj.insert("notes".into(), Value::Array(h.notes));
    }
    let mut blocks = vec![ContentBlock::text(
        serde_json::to_string_pretty(&Value::Object(obj)).expect("JSON values serialise"),
    )];
    blocks.extend(trailing);
    CallToolResult::success(blocks)
}

/// An error result `{error: {code, message, ..extra}}`, where `extra` is a JSON
/// object whose fields are added to `error`.
pub fn error(code: &str, message: impl Into<String>, extra: Value) -> CallToolResult {
    let mut err = Map::new();
    err.insert("code".into(), json!(code));
    err.insert("message".into(), json!(message.into()));
    if let Value::Object(extra) = extra {
        err.extend(extra);
    }
    error_object(Value::Object(err))
}

fn error_object(err: Value) -> CallToolResult {
    let mut obj = Map::new();
    obj.insert("error".into(), err);
    CallToolResult::error(vec![text(obj)])
}

/// The error result for a failed daemon call. An unreachable daemon is
/// `daemon_unreachable` naming `log`; a request past its deadline is `timeout`
/// (also naming `log`); an unparseable success body is `bad_response`; an API
/// error passes the daemon's `error` object through unchanged (so a conflict
/// keeps its `current`). An empty `log` names none (the log is not the
/// agent's to read).
pub fn client_error(e: ClientError, log: &Path) -> CallToolResult {
    client_error_with(e, log, json!({}))
}

/// [`client_error`] with the fields of `extra` (a JSON object) added to
/// `error`.
pub fn client_error_with(e: ClientError, log: &Path, extra: Value) -> CallToolResult {
    let add = |mut v: Value| {
        if let (Some(o), Value::Object(x)) = (v.as_object_mut(), extra.clone()) {
            o.extend(x);
        }
        if log.as_os_str().is_empty()
            && let Some(o) = v.as_object_mut()
        {
            o.remove("log");
        }
        v
    };
    match e {
        ClientError::Unreachable(m) => error(
            "daemon_unreachable",
            format!("the clax daemon did not respond ({m}); see its log"),
            add(json!({"log": log.to_string_lossy()})),
        ),
        ClientError::Timeout(m) => error(
            "timeout",
            "the daemon did not respond in time; for a publish, read the artifact before retrying",
            add(json!({"detail": m, "log": log.to_string_lossy()})),
        ),
        ClientError::BadResponse(m) => error(
            "bad_response",
            format!("the daemon's response could not be read: {m}"),
            add(json!({})),
        ),
        ClientError::Api { error, .. } => error_object(add(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(r: &CallToolResult) -> Vec<String> {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect()
    }

    #[test]
    fn an_empty_log_path_names_no_log() {
        let e = || ClientError::Unreachable("down".into());
        let named = texts(&client_error(e(), Path::new("/h/.clax/daemon.log")));
        assert!(named[0].contains("/h/.clax/daemon.log"), "{named:?}");
        let none = texts(&client_error(e(), Path::new("")));
        let v: Value = serde_json::from_str(&none[0]).unwrap();
        assert!(v["error"].get("log").is_none(), "{v}");
        assert_eq!(v["error"]["code"], "daemon_unreachable");
    }

    #[test]
    fn a_note_alone_is_handed_over_with_its_text() {
        let note = "[clax] Your session ended after 30 minutes without contact";
        let h =
            Handover::of(&json!({"feedback": [], "answers": [], "notes": [note], "text": note}));
        assert!(!h.is_empty());
        let r = success_with(json!({"ok": true}), h);
        let t = texts(&r);
        assert_eq!(t.len(), 2, "{t:?}");
        let first: Value = serde_json::from_str(&t[0]).unwrap();
        assert_eq!(first["notes"], json!([note]));
        assert_eq!(t[1], format!("---\n{note}"));
        // Without one, nothing is added.
        let r = success_with(json!({"ok": true}), Handover::of(&json!({"text": null})));
        let t = texts(&r);
        assert_eq!(t.len(), 1);
        assert!(
            serde_json::from_str::<Value>(&t[0])
                .unwrap()
                .get("notes")
                .is_none()
        );
    }
}
