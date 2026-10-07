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
/// to the session's questions (`answers`) and the prose for both (`text`).
#[derive(Debug, Default)]
pub struct Handover {
    pub feedback: Vec<Value>,
    pub answers: Vec<Value>,
    pub text: Option<String>,
}

impl Handover {
    /// The handover in a `GET /api/sessions/<sid>/feedback` response.
    pub fn of(res: &Value) -> Handover {
        let list = |k: &str| res[k].as_array().cloned().unwrap_or_default();
        Handover {
            feedback: list("feedback"),
            answers: list("answers"),
            text: res["text"].as_str().map(str::to_string),
        }
    }

    /// Whether nothing was handed over.
    pub fn is_empty(&self) -> bool {
        self.feedback.is_empty() && self.answers.is_empty()
    }
}

/// A success result for `value`, which must be a JSON object, with an empty
/// `feedback` array.
pub fn success(value: Value) -> CallToolResult {
    success_with(value, Handover::default())
}

/// A success result for `value` (a JSON object) whose `feedback` array is
/// `h.feedback`, with `answers` (the late answers) when there are any. When
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
/// keeps its `current`).
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
