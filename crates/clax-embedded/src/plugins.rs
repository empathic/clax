//! The plugin trees and marketplaces `clax init` writes out: the layout
//! matches the repository (`.claude-plugin/marketplace.json`,
//! `.agents/plugins/marketplace.json`, `.grok-plugin/marketplace.json` and
//! `plugins/{claude-code,clax,clax-grok,pi}`), so the manifests'
//! `./plugins/<name>` sources resolve. The Pi package carries what its
//! `package.json` ships.

use rust_embed::RustEmbed;

/// One embed per embedded directory, under `CLAX_PLUGINS_DIR` (set by
/// `build.rs`: the checkout's `plugins/`, or Bazel's copy of it). A debug
/// build reads embeds from disk on
/// every call, so one embed of all of `plugins/` would walk
/// `plugins/pi/node_modules` each time.
macro_rules! plugin_dir {
    ($name:ident, $folder:literal, $prefix:literal) => {
        #[derive(RustEmbed)]
        #[folder = $folder]
        #[prefix = $prefix]
        #[exclude = "**/.DS_Store"]
        #[exclude = "**/*.swp"]
        #[exclude = "**/*~"]
        struct $name;
    };
}
plugin_dir!(ClaudeCode, "$CLAX_PLUGINS_DIR/claude-code/", "claude-code/");
plugin_dir!(Codex, "$CLAX_PLUGINS_DIR/clax/", "clax/");
plugin_dir!(Grok, "$CLAX_PLUGINS_DIR/clax-grok/", "clax-grok/");
plugin_dir!(PiSrc, "$CLAX_PLUGINS_DIR/pi/src/", "pi/src/");
plugin_dir!(PiSkills, "$CLAX_PLUGINS_DIR/pi/skills/", "pi/skills/");
plugin_dir!(PiScripts, "$CLAX_PLUGINS_DIR/pi/scripts/", "pi/scripts/");

const CLAUDE_MARKETPLACE: &str = include_str!("../../../.claude-plugin/marketplace.json");
const CODEX_MARKETPLACE: &str = include_str!("../../../.agents/plugins/marketplace.json");
const GROK_MARKETPLACE: &str = include_str!("../../../.grok-plugin/marketplace.json");
const PI_PACKAGE: &[u8] = include_bytes!("../../../plugins/pi/package.json");
const PI_README: &[u8] = include_bytes!("../../../plugins/pi/README.md");

/// Every file of an embed: (path under `plugins/`, contents).
fn embedded<E: RustEmbed>() -> impl Iterator<Item = (String, Vec<u8>)> {
    E::iter().map(|p| {
        let f = E::get(&p).expect("an embedded file lists itself");
        (format!("plugins/{p}"), f.data.into_owned())
    })
}

/// Every file of the marketplace tree: (path relative to its root,
/// contents), sorted.
pub fn plugin_files() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![
        (
            ".grok-plugin/marketplace.json".to_string(),
            GROK_MARKETPLACE.as_bytes().to_vec(),
        ),
        (
            ".claude-plugin/marketplace.json".to_string(),
            CLAUDE_MARKETPLACE.as_bytes().to_vec(),
        ),
        (
            ".agents/plugins/marketplace.json".to_string(),
            CODEX_MARKETPLACE.as_bytes().to_vec(),
        ),
        ("plugins/pi/package.json".to_string(), PI_PACKAGE.to_vec()),
        ("plugins/pi/README.md".to_string(), PI_README.to_vec()),
    ];
    out.extend(embedded::<ClaudeCode>());
    out.extend(embedded::<Codex>());
    out.extend(embedded::<Grok>());
    out.extend(embedded::<PiSrc>());
    out.extend(embedded::<PiSkills>());
    out.extend(embedded::<PiScripts>());
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// `build.rs` names every directory a file is embedded from, so adding a
    /// file anywhere in the tree rebuilds the crate.
    #[test]
    fn the_build_script_watches_every_embedded_directory() {
        let watched: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/plugin-dirs.txt"))
            .lines()
            .collect();
        // `plugins` and `plugins/pi` hold files that are not embedded
        // (`node_modules`, tests), so they are not watched as a whole.
        let unwatched = [Path::new("plugins"), Path::new("plugins/pi")];
        for (rel, _) in plugin_files() {
            if !rel.starts_with("plugins/") {
                continue;
            }
            for d in Path::new(&rel).ancestors().skip(1) {
                if d.as_os_str().is_empty() || unwatched.contains(&d) {
                    continue;
                }
                let d = d.to_str().unwrap();
                assert!(
                    watched.contains(&d),
                    "{d} (for {rel}) is not watched: {watched:?}"
                );
            }
        }
        assert!(watched.contains(&"plugins/clax/scripts"), "{watched:?}");
    }

    /// Release builds embed the tree at compile time; the plugin manifests
    /// and MCP configurations are dotfiles, which must be included.
    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_embeds_the_dotfiles() {
        let names: Vec<String> = plugin_files().into_iter().map(|(n, _)| n).collect();
        for want in [
            "plugins/claude-code/.claude-plugin/plugin.json",
            "plugins/claude-code/.mcp.json",
            "plugins/clax/.codex-plugin/plugin.json",
            "plugins/clax/.mcp.json",
            "plugins/clax/scripts/ensure-clax.sh",
            "plugins/clax-grok/.grok-plugin/plugin.json",
            "plugins/clax-grok/.mcp.json",
            "plugins/clax-grok/hooks/hooks.json",
            "plugins/clax-grok/scripts/ensure-clax.sh",
            "plugins/pi/package.json",
            ".grok-plugin/marketplace.json",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} missing: {names:?}");
        }
    }
}
