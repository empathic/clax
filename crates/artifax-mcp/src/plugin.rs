//! The installed plugin an MCP server or a doctor check runs against: its
//! manifest version, and the version and tool count its skill states.

use std::path::{Path, PathBuf};

/// The manifests that carry a plugin's version, relative to its root: Codex,
/// Claude Code, and the Pi package.
pub const MANIFESTS: [&str; 3] = [
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    "package.json",
];

/// The `version` of the first manifest under `root` that has one.
pub fn manifest_version(root: &Path) -> Option<String> {
    MANIFESTS.iter().find_map(|m| {
        let text = std::fs::read_to_string(root.join(m)).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v["version"].as_str().map(str::to_string)
    })
}

/// The plugin root of a shim: `CLAUDE_PLUGIN_ROOT`, else `PLUGIN_ROOT` (empty
/// counts as unset), else `cwd` when it holds a plugin manifest (Codex starts
/// the server in the plugin root and exports neither variable).
pub fn root_from_env(
    env: impl Fn(&str) -> Option<String>,
    cwd: Option<PathBuf>,
) -> Option<PathBuf> {
    ["CLAUDE_PLUGIN_ROOT", "PLUGIN_ROOT"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()).map(PathBuf::from))
        .or_else(|| cwd.filter(|d| MANIFESTS[..2].iter().any(|m| d.join(m).is_file())))
}

/// The plugin version and tool count stated by a skill's generated tools
/// block: "This is Artifax plugin <version>. It provides <n> tools".
pub fn skill_block(text: &str) -> Option<(String, usize)> {
    let rest = &text[text.find("This is Artifax plugin ")? + "This is Artifax plugin ".len()..];
    let version = rest.split_whitespace().next()?.trim_end_matches('.');
    let rest = &rest[rest.find("It provides ")? + "It provides ".len()..];
    let n = rest.split_whitespace().next()?.parse().ok()?;
    Some((version.to_string(), n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_manifest_with_a_version_wins() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(manifest_version(dir.path()), None);
        std::fs::write(dir.path().join("package.json"), r#"{"version": "0.3.0"}"#).unwrap();
        assert_eq!(manifest_version(dir.path()).as_deref(), Some("0.3.0"));
        std::fs::create_dir_all(dir.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".codex-plugin/plugin.json"),
            r#"{"name": "artifax", "version": "0.1.0"}"#,
        )
        .unwrap();
        assert_eq!(manifest_version(dir.path()).as_deref(), Some("0.1.0"));
    }

    #[test]
    fn the_root_comes_from_the_environment_then_a_plugin_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            root_from_env(
                env(&[("CLAUDE_PLUGIN_ROOT", "/c"), ("PLUGIN_ROOT", "/p")]),
                None
            ),
            Some(PathBuf::from("/c"))
        );
        assert_eq!(
            root_from_env(
                env(&[("CLAUDE_PLUGIN_ROOT", ""), ("PLUGIN_ROOT", "/p")]),
                None
            ),
            Some(PathBuf::from("/p"))
        );
        assert_eq!(root_from_env(env(&[]), Some(dir.path().into())), None);
        std::fs::create_dir_all(dir.path().join(".codex-plugin")).unwrap();
        std::fs::write(dir.path().join(".codex-plugin/plugin.json"), "{}").unwrap();
        assert_eq!(
            root_from_env(env(&[]), Some(dir.path().into())),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn the_skill_block_states_the_version_and_tool_count() {
        let text = "# Artifax\n\n<!-- tools:begin -->\nThis is Artifax plugin 0.2.0. It provides 22 tools as\n<!-- tools:end -->";
        assert_eq!(skill_block(text), Some(("0.2.0".into(), 22)));
        assert_eq!(skill_block("# Artifax\nThe tools are exposed as"), None);
    }
}
