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

    pub fn value(&self) -> Option<&Value> {
        self.0.as_ref()
    }

    /// The single line to print, or None when there is nothing to say.
    pub fn to_line(&self) -> Option<String> {
        self.0.as_ref().map(Value::to_string)
    }
}

#[cfg(test)]
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
}
