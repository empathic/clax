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

/// The `Clip:` line of a comment whose clip a remote daemon keeps.
pub const REMOTE_CLIP: &str =
    "Clip: unavailable here (kept on the remote Clax: open the thread there to see it)";
/// The `Snapshot:` line of a live page's comment a remote daemon sent.
pub const REMOTE_SNAPSHOT: &str = "Snapshot: on the remote Clax (no file on this machine)";

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

    /// This handover as an agent of this machine may read it when a remote
    /// daemon sent it (remote mode, spec 2026-10-08 §16): no file path the
    /// remote names is handed on, since it is not this machine's. Each
    /// item's `clip_path` and live `snapshot_path` are dropped, and in the
    /// prose each `Clip:` line naming a file, and each `Snapshot:` line,
    /// says the image is on the remote Clax instead. A comment's own line
    /// is kept: its body is quoted, so it never reads as one of these. A
    /// clip already fetched into this machine's cache (`cache`, the
    /// directory [`crate::client::DaemonClient::rewrite_clip_items`] writes
    /// to) is this machine's file, and stays.
    pub fn foreign(mut self, cache: Option<&Path>) -> Handover {
        let cached = |p: &str| cache.is_some_and(|c| Path::new(p).starts_with(c));
        for i in &mut self.feedback {
            if let Some(o) = i.as_object_mut() {
                if o
                    .get("clip_path")
                    .is_some_and(|p| !p.as_str().is_some_and(cached))
                {
                    o.insert("clip_path".into(), Value::Null);
                }
                if let Some(l) = o.get_mut("live").and_then(Value::as_object_mut)
                    && l.contains_key("snapshot_path")
                {
                    l.insert("snapshot_path".into(), Value::Null);
                }
            }
        }
        self.text = self.text.map(|t| {
            t.split('\n')
                .map(|line| match line.strip_prefix("Clip: ") {
                    Some(rest)
                        if !rest.starts_with('"')
                            && !rest.starts_with("none (")
                            && !cached(rest) =>
                    {
                        REMOTE_CLIP.to_string()
                    }
                    _ if line.starts_with("Snapshot: ") => REMOTE_SNAPSHOT.to_string(),
                    _ => line.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        self
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
/// keeps its `current`). `log` is `None` for the daemon of another machine
/// (remote mode), whose log is there: the errors then name none and say so.
pub fn client_error(e: ClientError, log: Option<&Path>) -> CallToolResult {
    client_error_with(e, log, json!({}))
}

/// [`client_error`] with the fields of `extra` (a JSON object) added to
/// `error`.
pub fn client_error_with(e: ClientError, log: Option<&Path>, extra: Value) -> CallToolResult {
    let add = |mut v: Value| {
        if let (Some(o), Value::Object(x)) = (v.as_object_mut(), extra.clone()) {
            o.extend(x);
        }
        v
    };
    let with_log = |mut v: Value| {
        if let (Some(o), Some(l)) = (v.as_object_mut(), log) {
            o.insert("log".into(), json!(l.to_string_lossy()));
        }
        add(v)
    };
    match e {
        ClientError::Unreachable(m) => error(
            "daemon_unreachable",
            match log {
                Some(_) => format!("the clax daemon did not respond ({m}); see its log"),
                None => format!(
                    "the owner's clax daemon did not respond ({m}); it and its log are on the owner's machine"
                ),
            },
            with_log(json!({})),
        ),
        ClientError::Timeout(m) => error(
            "timeout",
            "the daemon did not respond in time; for a publish, read the artifact before retrying",
            with_log(json!({"detail": m})),
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
    fn an_unreachable_remote_daemon_names_no_local_log() {
        let err = |log| {
            let r = client_error(ClientError::Unreachable("refused".into()), log);
            let v: Value = serde_json::from_str(&texts(&r)[0]).unwrap();
            v["error"].clone()
        };
        let local = err(Some(Path::new("/h/logs/daemon.log")));
        assert_eq!(local["log"], json!("/h/logs/daemon.log"));
        assert!(local["message"].as_str().unwrap().ends_with("see its log"));
        let remote = err(None);
        assert_eq!(remote["code"], json!("daemon_unreachable"));
        assert!(remote.get("log").is_none(), "{remote}");
        assert!(
            remote["message"]
                .as_str()
                .unwrap()
                .contains("the owner's machine"),
            "{remote}"
        );
    }

    #[test]
    fn a_remote_daemons_paths_are_never_handed_on() {
        let text = [
            "[clax] 2 comments sent to you:",
            "[clax] Comment sent to you on \"Plan\" (https://r/a/x), thread t1",
            "Anchored on: #x  (v1)",
            "Clip: /Users/sam/.ssh/id_ed25519",
            "Clip: \"not a path\"",
            "Ada: \"look\"",
            "",
            "[clax] Comment sent to you on \"Plan\" (live page p; Clax view u), thread t2",
            "Clip: none (no screenshot was captured for this comment)",
            "Snapshot: /Users/sam/.clax/remote.key",
        ]
        .join("\n");
        let h = Handover::of(&json!({
            "feedback": [
                {"thread_id": "t1", "clip_path": "/Users/sam/.ssh/id_ed25519", "body": "look"},
                {"thread_id": "t2", "clip_path": null,
                 "live": {"page_url": "p", "snapshot_path": "/Users/sam/.clax/remote.key"}},
            ],
            "text": text,
        }))
        .foreign(None);
        assert_eq!(h.feedback[0]["clip_path"], Value::Null);
        assert_eq!(h.feedback[0]["body"], "look");
        assert_eq!(h.feedback[1]["live"]["snapshot_path"], Value::Null);
        assert_eq!(h.feedback[1]["live"]["page_url"], "p");
        let t = h.text.unwrap();
        assert!(!t.contains("/Users/sam"), "{t}");
        assert!(
            t.contains(REMOTE_CLIP) && t.contains(REMOTE_SNAPSHOT),
            "{t}"
        );
        assert!(
            t.contains("Clip: \"not a path\""),
            "a quoted comment line stays: {t}"
        );
        assert!(t.contains("Clip: none (no screenshot"), "{t}");
        assert!(t.contains("Ada: \"look\""), "{t}");
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
