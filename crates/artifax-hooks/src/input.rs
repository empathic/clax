//! Hook input as a harness writes it to stdin.

use serde::Deserialize;
use serde_json::{Map, Value};

/// The fields Artifax reads from hook stdin; everything else is kept in `rest`.
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
    /// shape yields the default (all fields absent).
    pub fn parse(stdin: &str) -> HookInput {
        serde_json::from_str(stdin).unwrap_or_default()
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
    fn bad_input_is_default() {
        assert!(HookInput::parse("{nope").session_id.is_none());
        assert!(HookInput::parse("").session_id.is_none());
        assert!(HookInput::parse("[1]").session_id.is_none());
    }
}
