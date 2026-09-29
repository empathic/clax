//! Lifecycle events: join and end the session the MCP shim registered.

use crate::input::HookInput;
use crate::output::HookOutput;
use anyhow::{Context, bail};
use serde_json::{Value, json};

/// The daemon operations hooks need.
pub trait Daemon {
    fn browser_url(&self, path: &str) -> String;
    fn get(&self, path: &str) -> anyhow::Result<Value>;
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
    fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
}

/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`, nearest
/// first, cover a wrapper shell between the hook and the harness).
pub fn session_start(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    let Some(session_id) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let mut body = json!({
        "harness": harness,
        "parent_pid": parent_pid,
        "harness_session_id": session_id,
    });
    if !ancestor_pids.is_empty() {
        body["ancestor_pids"] = json!(ancestor_pids);
    }
    if let Some(cwd) = &input.cwd {
        body["cwd"] = json!(cwd);
    }
    daemon.post("/api/sessions/join", &body)?;
    Ok(HookOutput::additional_context(
        "SessionStart",
        &format!(
            "Artifax daemon at {}; artifacts publish with the `publish` tool.",
            daemon.browser_url("/")
        ),
    ))
}

/// Ends the live session for `(harness, session_id)`, if there is one.
pub fn session_end(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    let Some(session_id) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let listed = daemon.get("/api/sessions?live=true")?;
    let found = listed["sessions"]
        .as_array()
        .context("sessions listing is not an array")?
        .iter()
        .filter(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(session_id))
        .filter_map(|s| s["id"].as_str());
    for id in found {
        daemon.patch(&format!("/api/sessions/{id}"), &json!({"ended": true}))?;
    }
    Ok(HookOutput::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<(String, String, Value)>>,
    }
    impl Daemon for Fake {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("GET".into(), path.into(), Value::Null));
            Ok(json!({"sessions": [
                {"id": "a", "harness": "claude", "harness_session_id": "s1"},
                {"id": "b", "harness": "codex", "harness_session_id": "s1"},
                {"id": "c", "harness": "claude", "harness_session_id": "s2"},
            ]}))
        }
        fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("POST".into(), path.into(), body.clone()));
            Ok(json!({}))
        }
        fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
            self.calls
                .borrow_mut()
                .push(("PATCH".into(), path.into(), body.clone()));
            Ok(json!({}))
        }
    }

    fn input(id: &str) -> HookInput {
        HookInput::parse(&format!(r#"{{"session_id":"{id}","cwd":"/w"}}"#))
    }

    #[test]
    fn start_joins_and_reports_url() {
        let d = Fake::default();
        let out = session_start("claude", 42, &[7, 1], &input("s1"), &d).unwrap();
        let calls = d.calls.borrow();
        assert_eq!(calls[0].1, "/api/sessions/join");
        assert_eq!(
            calls[0].2,
            json!({"harness": "claude", "parent_pid": 42, "ancestor_pids": [7, 1], "harness_session_id": "s1", "cwd": "/w"})
        );
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("http://h:1/"), "{text}");
    }

    #[test]
    fn start_without_session_id_errors() {
        let d = Fake::default();
        assert!(session_start("claude", 1, &[], &HookInput::default(), &d).is_err());
        assert!(d.calls.borrow().is_empty());
    }

    #[test]
    fn end_ends_only_the_matching_session() {
        let d = Fake::default();
        let out = session_end("claude", &input("s1"), &d).unwrap();
        assert_eq!(out, HookOutput::none());
        let calls = d.calls.borrow();
        let patches: Vec<_> = calls.iter().filter(|c| c.0 == "PATCH").collect();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].1, "/api/sessions/a");
    }
}
