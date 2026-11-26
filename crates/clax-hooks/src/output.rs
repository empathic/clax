//! Hook output as a harness reads it from stdout.

use serde_json::{Value, json};

/// What a hook prints: nothing, or one JSON object.
#[derive(Debug, PartialEq)]
pub struct HookOutput(Option<Value>);

impl HookOutput {
    /// Print nothing.
    pub fn none() -> HookOutput {
        HookOutput(None)
    }

    /// Context the harness adds to the conversation for `event`.
    pub fn additional_context(event: &str, text: &str) -> HookOutput {
        HookOutput(Some(json!({"hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": text,
        }})))
    }

    /// Ask the harness to block the action, giving `reason`.
    pub fn block(reason: &str) -> HookOutput {
        HookOutput(Some(json!({"decision": "block", "reason": reason})))
    }

    /// This output with `message` shown to the person (`systemMessage`).
    pub fn with_system_message(self, message: &str) -> HookOutput {
        let mut v = self.0.unwrap_or_else(|| json!({}));
        v["systemMessage"] = json!(message);
        HookOutput(Some(v))
    }

    /// PreToolUse: allow the call with `updated_input` replacing its input.
    pub fn allow_with_input(updated_input: Value) -> HookOutput {
        HookOutput(Some(json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": updated_input,
        }})))
    }

    /// PreToolUse: deny the call; `reason` is shown to the agent.
    pub fn deny(reason: &str) -> HookOutput {
        HookOutput(Some(json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }})))
    }

    /// This output as Antigravity reads it: a block becomes
    /// `{"decision":"continue","reason":…}`, added context becomes
    /// `{"injectSteps":[{"systemMessage":{"systemMessage":…}}]}`, and
    /// anything else prints nothing.
    pub fn for_antigravity(self) -> HookOutput {
        let Some(v) = self.0 else {
            return HookOutput::none();
        };
        if v["decision"] == "block" {
            return HookOutput(Some(json!({"decision": "continue", "reason": v["reason"]})));
        }
        match v["hookSpecificOutput"]["additionalContext"].as_str() {
            Some(text) => HookOutput(Some(
                json!({"injectSteps": [{"systemMessage": {"systemMessage": text}}]}),
            )),
            None => HookOutput::none(),
        }
    }

    pub fn value(&self) -> Option<&Value> {
        self.0.as_ref()
    }

    /// The single line to print, or None when there is nothing to say.
    pub fn to_line(&self) -> Option<String> {
        self.0.as_ref().map(Value::to_string)
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(HookOutput::none().to_line(), None);
        assert_eq!(
            HookOutput::additional_context("SessionStart", "hi").value(),
            Some(
                &json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "hi"}})
            )
        );
        assert_eq!(
            HookOutput::block("no").value(),
            Some(&json!({"decision": "block", "reason": "no"}))
        );
        assert_eq!(
            HookOutput::none().with_system_message("m").value(),
            Some(&json!({"systemMessage": "m"}))
        );
        assert_eq!(
            HookOutput::additional_context("SessionStart", "hi")
                .with_system_message("m")
                .value(),
            Some(&json!({
                "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "hi"},
                "systemMessage": "m",
            }))
        );
        assert_eq!(
            HookOutput::allow_with_input(json!({"a": 1})).value(),
            Some(
                &json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow", "updatedInput": {"a": 1}}})
            )
        );
        assert_eq!(
            HookOutput::deny("no").value(),
            Some(
                &json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "no"}})
            )
        );
    }

    #[test]
    fn a_block_becomes_a_continue() {
        assert_eq!(
            HookOutput::block("r").for_antigravity().value(),
            Some(&json!({"decision": "continue", "reason": "r"}))
        );
    }

    #[test]
    fn added_context_becomes_an_injected_system_message() {
        assert_eq!(
            HookOutput::additional_context("UserPromptSubmit", "t")
                .for_antigravity()
                .value(),
            Some(&json!({"injectSteps": [{"systemMessage": {"systemMessage": "t"}}]}))
        );
    }

    #[test]
    fn nothing_else_is_printed_for_antigravity() {
        assert_eq!(HookOutput::none().for_antigravity().value(), None);
        assert_eq!(HookOutput::deny("x").for_antigravity().value(), None);
    }
}
