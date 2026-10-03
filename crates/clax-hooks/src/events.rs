//! Hook events: join and end the session the MCP shim registered, and hand
//! pending feedback to the agent from the Stop and prompt-submit hooks.

use crate::input::HookInput;
use crate::output::HookOutput;
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// The pending-feedback request `session_start` makes after joining is
/// abandoned after this long, so the join context is printed within the
/// hook's deadline.
pub const START_FEEDBACK_TIMEOUT: Duration = Duration::from_secs(1);

/// The daemon operations hooks need.
pub trait Daemon {
    fn browser_url(&self, path: &str) -> String;
    fn get(&self, path: &str) -> anyhow::Result<Value>;
    /// [`Daemon::get`], abandoned with an error after `timeout`.
    fn get_with_timeout(&self, path: &str, timeout: Duration) -> anyhow::Result<Value>;
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
    fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value>;
}

/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`,
/// nearest first, cover a wrapper shell between the hook and the harness),
/// filling `cwd` and recording `codex_home` when given. Returns the
/// daemon's answer, `{"session": …}`.
///
/// # Errors
/// When the input has no session ID or the daemon request fails.
pub fn join(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
) -> anyhow::Result<Value> {
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
    if let Some(h) = codex_home {
        body["codex_home"] = json!(h);
    }
    daemon.post("/api/sessions/join", &body)
}

/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`, nearest
/// first, cover a wrapper shell between the hook and the harness). The
/// context names the daemon and, when the joined session has pending
/// `prompt_hook` feedback, appends its rendered text. That feedback request is
/// bounded by [`START_FEEDBACK_TIMEOUT`]; when it fails or times out the
/// context is returned without it. `codex_home`, when given, is recorded for
/// the session so the daemon can run `codex queue` against that Codex home.
pub fn session_start(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    let joined = join(
        harness,
        parent_pid,
        ancestor_pids,
        input,
        codex_home,
        daemon,
    )?;
    let mut context = format!(
        "Clax daemon at {}; artifacts publish with the `publish` tool.",
        daemon.browser_url("/")
    );
    if let Some(sid) = joined["session"]["id"].as_str()
        && let Ok(res) = daemon.get_with_timeout(
            &format!("/api/sessions/{sid}/feedback?tier=prompt_hook"),
            START_FEEDBACK_TIMEOUT,
        )
        && let Some(text) = rendered_text(&res)
    {
        context.push_str("\n\n");
        context.push_str(&text);
    }
    Ok(HookOutput::additional_context("SessionStart", &context))
}

/// [`session_start`] for a harness that ignores `SessionStart` output
/// (Grok Build): joins and prints nothing. It does not ask for
/// `prompt_hook` feedback: that request marks comments delivered, and
/// they would never reach the agent.
///
/// # Errors
/// When the input has no session ID or the join request fails.
pub fn session_start_quiet(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    join(harness, parent_pid, ancestor_pids, input, None, daemon)?;
    Ok(HookOutput::none())
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

/// The ID of the live Clax session for `(harness, input.session_id)`, if any.
///
/// # Errors
/// When the input has no `session_id` or the daemon cannot be asked.
fn live_session(
    harness: &str,
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<Option<String>> {
    let Some(hsid) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let listed = daemon.get("/api/sessions?live=true")?;
    Ok(listed["sessions"]
        .as_array()
        .context("sessions listing is not an array")?
        .iter()
        .find(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(hsid))
        .and_then(|s| s["id"].as_str())
        .map(str::to_string))
}

/// The rendered feedback the daemon hands over for `query`, if any. The text
/// is the daemon's, passed through unchanged.
fn feedback_text(daemon: &dyn Daemon, sid: &str, query: &str) -> anyhow::Result<Option<String>> {
    let res = daemon.get(&format!("/api/sessions/{sid}/feedback?{query}"))?;
    Ok(rendered_text(&res))
}

/// The non-empty `text` of a feedback response.
fn rendered_text(res: &Value) -> Option<String> {
    res["text"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Tier 2. Blocks the stop with the pending feedback as the reason; allows it
/// (prints nothing) when nothing is pending. Only watches with replies armed
/// count. While `stop_hook_active` is set, only never-delivered rows can block,
/// so a stop is blocked at most once per new comment. When it allows the
/// stop, it ends the session's working records (the turn is over). For Grok
/// Build, a Stop whose reason is not `end_turn` allows the stop without asking
/// the daemon.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn stop(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    // Grok Build fires Stop again at session end (`channel_closed`,
    // `shutdown`) and ignores that run's output; only `end_turn` (or an
    // input without a reason) is the end of a turn.
    if harness == "grok" && input.stop_reason().is_some_and(|r| r != "end_turn") {
        return Ok(HookOutput::none());
    }
    let Some(sid) = live_session(harness, input, daemon)? else {
        return Ok(HookOutput::none());
    };
    let resends = !input.stop_hook_active.unwrap_or(false);
    match feedback_text(daemon, &sid, &format!("tier=stop_hook&resends={resends}"))? {
        Some(text) => Ok(HookOutput::block(&text)),
        None => {
            // The turn really ends: its working records end with it.
            daemon.post(&format!("/api/sessions/{sid}/working/end"), &json!({}))?;
            Ok(HookOutput::none())
        }
    }
}

/// `PostToolUse`: renews the session's working records. Prints nothing.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    if let Some(sid) = live_session(harness, input, daemon)? {
        daemon.post(&format!("/api/sessions/{sid}/working/renew"), &json!({}))?;
    }
    Ok(HookOutput::none())
}

/// Tier 3. Adds pending feedback to the prompt as additional context.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn prompt(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    let Some(sid) = live_session(harness, input, daemon)? else {
        return Ok(HookOutput::none());
    };
    Ok(match feedback_text(daemon, &sid, "tier=prompt_hook")? {
        Some(text) => HookOutput::additional_context("UserPromptSubmit", &text),
        None => HookOutput::none(),
    })
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
                {"id": "g", "harness": "grok", "harness_session_id": "s1"},
            ]}))
        }
        fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
            self.get(path)
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
        let out = session_start("claude", 42, &[7, 1], &input("s1"), None, &d).unwrap();
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
    fn a_quiet_start_joins_and_prints_nothing() {
        let d = Fake::default();
        let out = session_start_quiet("grok", 42, &[7], &input("s1"), &d).unwrap();
        assert_eq!(out, HookOutput::none());
        let calls = d.calls.borrow();
        assert_eq!(calls.len(), 1, "no prompt_hook request: {calls:?}");
        assert_eq!(calls[0].1, "/api/sessions/join");
        assert_eq!(calls[0].2["harness"], "grok");
    }

    #[test]
    fn start_without_session_id_errors() {
        let d = Fake::default();
        assert!(session_start("claude", 1, &[], &HookInput::default(), None, &d).is_err());
        assert!(d.calls.borrow().is_empty());
    }

    #[test]
    fn start_sends_codex_home_when_known() {
        let d = Fake::default();
        session_start("codex", 42, &[], &input("s1"), Some("/cx"), &d).unwrap();
        assert_eq!(d.calls.borrow()[0].2["codex_home"], "/cx");
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

    struct FeedbackFake {
        text: Option<&'static str>,
        seen: RefCell<Vec<String>>,
        posts: RefCell<Vec<String>>,
    }
    impl Daemon for FeedbackFake {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.seen.borrow_mut().push(path.to_string());
            if path.starts_with("/api/sessions?") {
                return Ok(json!({"sessions": [
                    {"id": "S", "harness": "claude", "harness_session_id": "s1"},
                    {"id": "G", "harness": "grok", "harness_session_id": "s1"},
                ]}));
            }
            Ok(
                json!({"feedback": if self.text.is_some() { json!([{}]) } else { json!([]) }, "text": self.text, "waited_s": 0}),
            )
        }
        fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
            self.get(path)
        }
        fn post(&self, path: &str, _: &Value) -> anyhow::Result<Value> {
            self.posts.borrow_mut().push(path.to_string());
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }
    fn fake(text: Option<&'static str>) -> FeedbackFake {
        FeedbackFake {
            text,
            seen: RefCell::new(vec![]),
            posts: RefCell::new(vec![]),
        }
    }

    #[test]
    fn stop_ends_the_turn_only_when_it_allows_the_stop() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        stop("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap();
        assert!(
            d.posts.borrow().is_empty(),
            "a blocked stop continues the turn"
        );
        let d = fake(None);
        assert_eq!(
            stop(
                "claude",
                &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#),
                &d
            )
            .unwrap(),
            HookOutput::none()
        );
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/end"]);
    }

    #[test]
    fn tool_renews_and_prints_nothing() {
        let d = fake(None);
        assert_eq!(
            tool("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/renew"]);
        let d = fake(None);
        assert_eq!(
            tool("claude", &HookInput::parse(r#"{"session_id":"other"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            d.posts.borrow().is_empty(),
            "no live session: nothing to renew"
        );
    }

    #[test]
    fn stop_blocks_with_the_payload_and_excludes_resends_when_active() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        let out = stop(
            "claude",
            &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":false}"#),
            &d,
        )
        .unwrap();
        assert_eq!(out, HookOutput::block("[clax] 1 comment sent to you:\nX"));
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=stop_hook&resends=true")
        );
        let d = fake(None);
        let out = stop(
            "claude",
            &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#),
            &d,
        )
        .unwrap();
        assert_eq!(out, HookOutput::none());
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=stop_hook&resends=false")
        );
    }

    #[test]
    fn unknown_sessions_and_other_harnesses_print_nothing() {
        let d = fake(Some("x"));
        assert_eq!(
            stop("codex", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(
            prompt("claude", &HookInput::parse(r#"{"session_id":"nope"}"#), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            stop("claude", &HookInput::default(), &d).is_err(),
            "no session_id"
        );
    }

    #[test]
    fn a_grok_stop_acts_only_at_the_end_of_a_turn() {
        let d = fake(Some("[clax] hi"));
        let grok = |reason: &str| {
            HookInput::parse(&format!(r#"{{"sessionId":"s1","reason":"{reason}"}}"#))
        };
        assert_eq!(
            stop("grok", &grok("channel_closed"), &d).unwrap(),
            HookOutput::none()
        );
        assert_eq!(
            stop("grok", &grok("shutdown"), &d).unwrap(),
            HookOutput::none()
        );
        assert!(
            d.seen.borrow().is_empty(),
            "no daemon request at session end"
        );
        assert_eq!(
            stop("grok", &grok("end_turn"), &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
        let no_reason = HookInput::parse(r#"{"sessionId":"s1"}"#);
        assert_eq!(
            stop("grok", &no_reason, &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
    }

    #[test]
    fn a_reason_does_not_filter_other_harnesses() {
        let d = fake(Some("[clax] hi"));
        let i = HookInput::parse(r#"{"session_id":"s1","reason":"anything"}"#);
        assert_eq!(
            stop("claude", &i, &d).unwrap(),
            HookOutput::block("[clax] hi")
        );
    }

    #[test]
    fn grok_stop_hook_active_turns_resends_off() {
        let d = fake(Some("[clax] hi"));
        let i = HookInput::parse(r#"{"sessionId":"s1","reason":"end_turn","stopHookActive":true}"#);
        stop("grok", &i, &d).unwrap();
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/G/feedback?tier=stop_hook&resends=false"),
            "{:?}",
            d.seen.borrow()
        );
    }

    #[test]
    fn prompt_adds_context() {
        let d = fake(Some("P"));
        assert_eq!(
            prompt("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::additional_context("UserPromptSubmit", "P")
        );
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=prompt_hook")
        );
    }

    #[test]
    fn session_start_appends_pending_feedback() {
        let d = fake(Some("PENDING"));
        let out = session_start("claude", 1, &[], &input("s1"), None, &d).unwrap();
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.ends_with("\n\nPENDING"), "{text}");
    }

    /// A daemon whose feedback route answers only after `FEEDBACK_DELAY`; like
    /// the real client, a bounded request gives up after its timeout.
    struct SlowFeedback;
    const FEEDBACK_DELAY: Duration = Duration::from_secs(5);
    impl Daemon for SlowFeedback {
        fn browser_url(&self, path: &str) -> String {
            format!("http://h:1{path}")
        }
        fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.get_with_timeout(path, Duration::MAX)
        }
        fn get_with_timeout(&self, _: &str, timeout: Duration) -> anyhow::Result<Value> {
            std::thread::sleep(timeout.min(FEEDBACK_DELAY));
            if timeout < FEEDBACK_DELAY {
                bail!("timed out");
            }
            Ok(json!({"feedback": [{}], "text": "LATE", "waited_s": 0}))
        }
        fn post(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({"session": {"id": "S"}}))
        }
        fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn session_start_returns_the_join_context_when_feedback_is_slow() {
        let started = std::time::Instant::now();
        let out = session_start("claude", 1, &[], &input("s1"), None, &SlowFeedback).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
        let text = out.value().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("http://h:1/"), "{text}");
        assert!(!text.contains("LATE"), "{text}");
    }
}
