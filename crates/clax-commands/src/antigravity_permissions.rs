//! The rule in Antigravity's settings that approves every Clax tool, and
//! adding it.
//!
//! Antigravity asks before every MCP tool call. [`PERMISSION_RULE`] in `permissions.allow` of
//! `~/.gemini/antigravity-cli/settings.json` approves every tool of Clax's
//! server. Antigravity applies `deny` rules over `ask` rules over `allow`
//! rules, so an `ask` or `deny` rule that names the server wins over it.

use clax_core::antigravity::{PERMISSION_RULE, SERVER};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The command that shows the rule and adds it on confirmation.
pub const SETUP_COMMAND: &str = "clax init --agent antigravity";

/// `<home dir>/.gemini/antigravity-cli/settings.json`.
pub fn settings_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".gemini/antigravity-cli/settings.json")
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    /// `permissions.allow` has the rule (or `mcp(*)`).
    Allowed,
    Missing,
    /// `ask` or `deny` rules that name this server: `mcp(*)`,
    /// `mcp(clax-antigravity_clax/*)` or `mcp(clax-antigravity_clax/<tool>)`.
    Blocked(Vec<String>),
}

/// Of the settings file's text; `Err` when it is not a JSON object or
/// `permissions`, `allow`, `ask` or `deny` has the wrong type.
pub fn assess(text: &str) -> Result<State, String> {
    let settings = parse(text)?;
    let blocking: Vec<String> = [rules(&settings, "ask")?, rules(&settings, "deny")?]
        .concat()
        .into_iter()
        .filter(|r| names_server(r))
        .map(str::to_string)
        .collect();
    if !blocking.is_empty() {
        return Ok(State::Blocked(blocking));
    }
    let allow = rules(&settings, "allow")?;
    Ok(
        if allow
            .iter()
            .any(|r| *r == PERMISSION_RULE || *r == "mcp(*)")
        {
            State::Allowed
        } else {
            State::Missing
        },
    )
}

/// Whether `permissions.allow` in the settings file's text has the rule
/// itself; false when the text does not parse.
pub fn has_rule(text: &str) -> bool {
    parse(text)
        .and_then(|s| rules(&s, "allow").map(|a| a.contains(&PERMISSION_RULE)))
        .unwrap_or(false)
}

/// The text with the rule appended to `permissions.allow`, every other
/// key kept; an empty or missing file becomes `{"permissions":{"allow":[…]}}`.
pub fn with_rule(text: &str) -> Result<String, String> {
    let mut settings = parse(text)?;
    let permissions = settings
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("`permissions` is not an object")?;
    let allow = permissions
        .entry("allow")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or("`permissions.allow` is not an array")?;
    if !allow.iter().any(|r| r == PERMISSION_RULE) {
        allow.push(Value::from(PERMISSION_RULE));
    }
    let text = serde_json::to_string_pretty(&Value::Object(settings)).map_err(|e| e.to_string())?;
    Ok(text + "\n")
}

/// The settings object, with `permissions` and its rule lists checked; an
/// empty text is an empty object.
fn parse(text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let Value::Object(settings) = serde_json::from_str(text).map_err(|e| e.to_string())? else {
        return Err("the settings are not a JSON object".into());
    };
    for key in ["allow", "ask", "deny"] {
        rules(&settings, key)?;
    }
    Ok(settings)
}

/// The rules in `permissions.<key>`: none when either is absent, an error
/// when `permissions` is not an object or the list not an array of strings.
fn rules<'a>(settings: &'a Map<String, Value>, key: &str) -> Result<Vec<&'a str>, String> {
    let Some(permissions) = settings.get("permissions") else {
        return Ok(Vec::new());
    };
    let permissions = permissions
        .as_object()
        .ok_or("`permissions` is not an object")?;
    let Some(list) = permissions.get(key) else {
        return Ok(Vec::new());
    };
    let wrong = || format!("`permissions.{key}` is not an array of strings");
    list.as_array()
        .ok_or_else(wrong)?
        .iter()
        .map(|r| r.as_str().ok_or_else(wrong))
        .collect()
}

/// Whether a rule covers some tool of Clax's server: `mcp(*)`,
/// `mcp(<server>/*)` or `mcp(<server>/<tool>)`.
fn names_server(rule: &str) -> bool {
    rule == "mcp(*)"
        || rule
            .strip_prefix("mcp(")
            .and_then(|r| r.strip_suffix(')'))
            .and_then(|r| r.strip_prefix(SERVER))
            .and_then(|r| r.strip_prefix('/'))
            .is_some_and(|tool| !tool.is_empty())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_rule_or_a_rule_for_every_server_allows() {
        let with = format!(r#"{{"permissions":{{"allow":["command(ls)","{PERMISSION_RULE}"]}}}}"#);
        assert_eq!(assess(&with), Ok(State::Allowed));
        assert!(has_rule(&with));
        let every = r#"{"permissions":{"allow":["mcp(*)"]}}"#;
        assert_eq!(assess(every), Ok(State::Allowed));
        assert!(!has_rule(every));
    }

    #[test]
    fn without_the_rule_it_is_missing() {
        for text in [
            "",
            "{}",
            r#"{"model":"m","trustedWorkspaces":["/w"]}"#,
            r#"{"permissions":{}}"#,
            r#"{"permissions":{"allow":["mcp(other/*)","command(git status)"]}}"#,
            r#"{"permissions":{"ask":["mcp(other/delete)"],"deny":["command(rm)"]}}"#,
        ] {
            assert_eq!(assess(text), Ok(State::Missing), "{text}");
        }
    }

    #[test]
    fn an_ask_or_deny_rule_naming_the_server_blocks_and_is_named() {
        let ask = r#"{"permissions":{"ask":["mcp(clax-antigravity_clax/delete)"]}}"#;
        assert_eq!(
            assess(ask),
            Ok(State::Blocked(vec![
                "mcp(clax-antigravity_clax/delete)".into()
            ]))
        );
        let both = format!(
            r#"{{"permissions":{{"allow":["{PERMISSION_RULE}"],"ask":["command(ls)","mcp(*)"],"deny":["{PERMISSION_RULE}"]}}}}"#
        );
        assert_eq!(
            assess(&both),
            Ok(State::Blocked(vec![
                "mcp(*)".into(),
                PERMISSION_RULE.into()
            ])),
            "deny and ask win over allow"
        );
        let other = r#"{"permissions":{"deny":["mcp(clax-antigravity_claxx/*)","mcp(clax-antigravity_clax/)"]}}"#;
        assert_eq!(assess(other), Ok(State::Missing));
    }

    #[test]
    fn text_that_is_not_a_settings_object_is_an_error() {
        for text in [
            "[]",
            "{",
            "null",
            r#"{"permissions":{"allow":"x"}}"#,
            r#"{"permissions":[]}"#,
            r#"{"permissions":{"ask":{}}}"#,
            r#"{"permissions":{"deny":[1]}}"#,
        ] {
            assert!(assess(text).is_err(), "{text}");
            assert!(with_rule(text).is_err(), "{text}");
            assert!(!has_rule(text), "{text}");
        }
    }

    #[test]
    fn the_rule_added_to_an_empty_file_is_the_whole_file() {
        assert_eq!(
            with_rule("").unwrap(),
            format!(
                "{{\n  \"permissions\": {{\n    \"allow\": [\n      \"{PERMISSION_RULE}\"\n    ]\n  }}\n}}\n"
            )
        );
    }

    #[test]
    fn adding_the_rule_keeps_every_other_key() {
        let out = with_rule(r#"{"model":"m","trustedWorkspaces":["/w"]}"#).unwrap();
        assert_eq!(
            parsed(&out),
            serde_json::json!({
                "model": "m",
                "trustedWorkspaces": ["/w"],
                "permissions": {"allow": [PERMISSION_RULE]},
            })
        );
        assert!(out.ends_with("}\n"), "{out}");
        assert!(
            out.contains("\n  \"model\": \"m\""),
            "two-space indent: {out}"
        );
        assert_eq!(assess(&out), Ok(State::Allowed));
    }

    #[test]
    fn the_rule_is_appended_to_the_allow_rules_there() {
        let text = r#"{"permissions":{"allow":["command(git status)","mcp(other/*)"],"deny":["command(rm)"]}}"#;
        let out = with_rule(text).unwrap();
        assert_eq!(
            parsed(&out)["permissions"],
            serde_json::json!({
                "allow": ["command(git status)", "mcp(other/*)", PERMISSION_RULE],
                "deny": ["command(rm)"],
            })
        );
    }

    #[test]
    fn adding_the_rule_twice_leaves_one() {
        let once = with_rule("{}").unwrap();
        let twice = with_rule(&once).unwrap();
        assert_eq!(twice, once);
        assert_eq!(
            parsed(&twice)["permissions"]["allow"],
            serde_json::json!([PERMISSION_RULE])
        );
    }

    #[test]
    fn the_settings_file_is_under_the_gemini_directory() {
        assert_eq!(
            settings_path(Path::new("/h")),
            PathBuf::from("/h/.gemini/antigravity-cli/settings.json")
        );
        assert_eq!(SERVER, "clax-antigravity_clax");
    }
}
