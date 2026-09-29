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

/// A success result for `value`, which must be a JSON object, with an empty
/// `feedback` array.
pub fn success(value: Value) -> CallToolResult {
    success_with(value, Vec::new(), None)
}

/// A success result for `value` (a JSON object) whose `feedback` array is
/// `feedback`. When `feedback` is not empty and `text` is given, a second text
/// block `---\n<text>` follows: the trailing block agents read as prose.
pub fn success_with(value: Value, feedback: Vec<Value>, text: Option<String>) -> CallToolResult {
    let Value::Object(mut obj) = value else {
        panic!("tool results are JSON objects");
    };
    let trailing = text
        .filter(|_| !feedback.is_empty())
        .map(|t| ContentBlock::text(format!("---\n{t}")));
    obj.insert("feedback".into(), Value::Array(feedback));
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
    match e {
        ClientError::Unreachable(m) => error(
            "daemon_unreachable",
            format!("the artifax daemon did not respond ({m}); see its log"),
            json!({"log": log.to_string_lossy()}),
        ),
        ClientError::Timeout(m) => error(
            "timeout",
            "the daemon did not respond in time; for a publish, read the artifact before retrying",
            json!({"detail": m, "log": log.to_string_lossy()}),
        ),
        ClientError::BadResponse(m) => error(
            "bad_response",
            format!("the daemon's response could not be read: {m}"),
            json!({}),
        ),
        ClientError::Api { error, .. } => error_object(error),
    }
}
