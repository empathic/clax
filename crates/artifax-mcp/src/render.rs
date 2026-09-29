//! Tool result rendering: every result is one text block holding a pretty-printed
//! JSON object that carries a `feedback` array (empty until comments exist).

use crate::client::ClientError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value, json};
use std::path::Path;

fn text(mut obj: Map<String, Value>) -> ContentBlock {
    obj.insert("feedback".into(), json!([]));
    ContentBlock::text(
        serde_json::to_string_pretty(&Value::Object(obj)).expect("JSON values serialise"),
    )
}

/// A success result for `value`, which must be a JSON object.
pub fn success(value: Value) -> CallToolResult {
    let Value::Object(obj) = value else {
        panic!("tool results are JSON objects");
    };
    CallToolResult::success(vec![text(obj)])
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
/// `daemon_unreachable` naming `log`; an API error passes the daemon's `error`
/// object through unchanged (so a conflict keeps its `current`).
pub fn client_error(e: ClientError, log: &Path) -> CallToolResult {
    match e {
        ClientError::Unreachable(m) => error(
            "daemon_unreachable",
            format!("the artifax daemon did not respond ({m}); see its log"),
            json!({"log": log.to_string_lossy()}),
        ),
        ClientError::Api { error, .. } => error_object(error),
    }
}
