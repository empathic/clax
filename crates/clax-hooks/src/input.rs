//! Hook input as a harness writes it to stdin.

use serde::Deserialize;
use serde_json::{Map, Value};

/// The fields Clax reads from hook stdin; everything else is kept in `rest`.
#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub hook_event_name: Option<String>,
    pub transcript_path: Option<String>,
    pub stop_hook_active: Option<bool>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl HookInput {
    /// Parses leniently: anything that is not a JSON object of the expected
    /// shape yields the default (all fields absent). Grok Build's camelCase
    /// keys (`sessionId`, `stopHookActive`, `transcriptPath`,
    /// `hookEventName`) fill the fields whose snake_case key is absent; they
    /// are copied after deserialising, not declared as serde aliases,
    /// because Grok sends both spellings of some keys and an alias would
    /// make that a duplicate-field error. The camelCase keys stay in `rest`.
    pub fn parse(stdin: &str) -> HookInput {
        let mut i: HookInput = serde_json::from_str(stdin).unwrap_or_default();
        let s = |i: &HookInput, k: &str| i.rest.get(k).and_then(Value::as_str).map(str::to_string);
        if i.session_id.is_none() {
            i.session_id = s(&i, "sessionId");
        }
        if i.transcript_path.is_none() {
            i.transcript_path = s(&i, "transcriptPath");
        }
        if i.hook_event_name.is_none() {
            i.hook_event_name = s(&i, "hookEventName");
        }
        if i.stop_hook_active.is_none() {
            i.stop_hook_active = i.rest.get("stopHookActive").and_then(Value::as_bool);
        }
        i
    }

    /// Why Grok Build's Stop fired: `end_turn` at the end of a turn,
    /// `channel_closed` or `shutdown` at session end. `None` when the input
    /// has no `reason` string.
    pub fn stop_reason(&self) -> Option<&str> {
        self.rest.get("reason").and_then(Value::as_str)
    }

    /// Whether this is Grok Build's envelope, which always carries the
    /// camelCase `hookEventName`; Claude Code and Codex send only
    /// `hook_event_name`.
    pub fn is_grok_envelope(&self) -> bool {
        self.rest.contains_key("hookEventName")
    }

    /// Whether Claude Code's Stop input lists a subagent still running in
    /// the background: a `background_tasks` entry with `type` `"subagent"`
    /// and `status` `"running"`. Background shells and monitors, such as
    /// Clax's own comment follower, do not count. Grok Build's
    /// `backgroundTasks` is not read: its subagents' tool calls do not
    /// renew the parent's working records, so keeping them would only
    /// delay their end.
    pub fn has_running_subagent(&self) -> bool {
        self.rest
            .get("background_tasks")
            .and_then(Value::as_array)
            .is_some_and(|tasks| {
                tasks
                    .iter()
                    .any(|t| t["type"] == "subagent" && t["status"] == "running")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_extra_fields() {
        let i = HookInput::parse(r#"{"session_id":"s","cwd":"/w","source":"startup"}"#);
        assert_eq!(i.session_id.as_deref(), Some("s"));
        assert_eq!(i.cwd.as_deref(), Some("/w"));
        assert_eq!(i.rest["source"], "startup");
    }

    #[test]
    fn grok_camel_case_keys_fill_the_known_fields() {
        let i = HookInput::parse(
            r#"{"hookEventName":"Stop","sessionId":"g1","cwd":"/w","stopHookActive":true,"transcriptPath":"/t","reason":"end_turn"}"#,
        );
        assert_eq!(i.session_id.as_deref(), Some("g1"));
        assert_eq!(i.stop_hook_active, Some(true));
        assert_eq!(i.transcript_path.as_deref(), Some("/t"));
        assert_eq!(i.hook_event_name.as_deref(), Some("Stop"));
        assert_eq!(i.stop_reason(), Some("end_turn"));
        assert!(i.is_grok_envelope());
    }

    #[test]
    fn both_spellings_together_parse_and_snake_case_wins() {
        let i = HookInput::parse(
            r#"{"session_id":"s","sessionId":"g","stop_hook_active":false,"stopHookActive":true}"#,
        );
        assert_eq!(i.session_id.as_deref(), Some("s"));
        assert_eq!(i.stop_hook_active, Some(false));
    }

    #[test]
    fn claude_and_codex_input_is_not_a_grok_envelope() {
        let i = HookInput::parse(
            r#"{"session_id":"s","hook_event_name":"Stop","stop_hook_active":true}"#,
        );
        assert!(!i.is_grok_envelope());
        assert_eq!(i.stop_reason(), None);
        assert_eq!(i.stop_hook_active, Some(true));
    }

    #[test]
    fn a_running_subagent_is_a_running_subagent_entry_of_background_tasks() {
        let has = |j: &str| HookInput::parse(j).has_running_subagent();
        assert!(has(
            r#"{"background_tasks":[{"id":"a","type":"subagent","status":"running","agent_type":"general-purpose"}]}"#
        ));
        assert!(has(
            r#"{"background_tasks":[{"id":"b","type":"shell","status":"running"},{"id":"a","type":"subagent","status":"running"}]}"#
        ));
        assert!(
            !has(
                r#"{"background_tasks":[{"id":"b","type":"shell","status":"running","command":"clax feedback follow"}]}"#
            ),
            "a background shell, such as the comment follower, is not a subagent"
        );
        assert!(!has(
            r#"{"background_tasks":[{"id":"a","type":"subagent","status":"completed"}]}"#
        ));
        assert!(
            !has(
                r#"{"backgroundTasks":[{"id":"a","type":"subagent","status":"running","agentType":"explore"}]}"#
            ),
            "Grok Build's key is not read"
        );
        assert!(!has(r#"{"background_tasks":[]}"#));
        assert!(!has(r#"{"background_tasks":"x"}"#));
        assert!(!has(r#"{"background_tasks":["subagent"]}"#));
        assert!(!has(r#"{"session_id":"s"}"#));
    }

    #[test]
    fn bad_input_is_default() {
        assert!(HookInput::parse("{nope").session_id.is_none());
        assert!(HookInput::parse("").session_id.is_none());
        assert!(HookInput::parse("[1]").session_id.is_none());
    }
}
