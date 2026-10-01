//! The plugins this binary was built with, and writing them out as a
//! marketplace directory (`clax init`). The layout matches the repository:
//! `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json` and
//! `plugins/{claude-code,clax,pi}`, so both manifests' `./plugins/<name>`
//! sources resolve. The Pi package carries what its `package.json` ships.
//! `build.rs` rebuilds the binary when any of it changes.

use rust_embed::RustEmbed;
use std::path::Path;

#[derive(RustEmbed)]
#[folder = "../../plugins/"]
#[include = "claude-code/**"]
#[include = "clax/**"]
#[include = "pi/package.json"]
#[include = "pi/README.md"]
#[include = "pi/src/**"]
#[include = "pi/skills/**"]
#[exclude = "**/.DS_Store"]
#[exclude = "**/*.swp"]
#[exclude = "**/*~"]
struct Plugins;

const CLAUDE_MARKETPLACE: &str = include_str!("../../../.claude-plugin/marketplace.json");
const CODEX_MARKETPLACE: &str = include_str!("../../../.agents/plugins/marketplace.json");

/// Every file of the marketplace tree: (path relative to its root, contents).
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![
        (
            ".claude-plugin/marketplace.json".to_string(),
            CLAUDE_MARKETPLACE.as_bytes().to_vec(),
        ),
        (
            ".agents/plugins/marketplace.json".to_string(),
            CODEX_MARKETPLACE.as_bytes().to_vec(),
        ),
    ];
    for p in Plugins::iter() {
        let f = Plugins::get(&p).expect("an embedded file lists itself");
        out.push((format!("plugins/{p}"), f.data.into_owned()));
    }
    out.sort();
    out
}

/// Writes the tree to `root`, replacing what is there. It is built in a
/// sibling temporary directory and renamed into place, so a harness never
/// reads a half-written plugin. Scripts (`*.sh`) are 0755, other files 0644.
///
/// The caller holds the init lock, so any `.marketplace.*.tmp` or `.old`
/// sibling left by an interrupted run is stale and is removed first. On an
/// error the temporary tree is removed too.
pub fn materialize(root: &Path) -> std::io::Result<()> {
    let parent = root.parent().expect("the marketplace root has a parent");
    std::fs::create_dir_all(parent)?;
    for e in std::fs::read_dir(parent)?.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(".marketplace.") && (name.ends_with(".tmp") || name.ends_with(".old")) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let pid = std::process::id();
    let tmp = parent.join(format!(".marketplace.{pid}.tmp"));
    let old = parent.join(format!(".marketplace.{pid}.old"));
    let result = write_tree(&tmp).and_then(|()| {
        if root.exists() {
            std::fs::rename(root, &old)?;
        }
        std::fs::rename(&tmp, root)
    });
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&old);
    result
}

fn write_tree(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for (rel, data) in files() {
        let path = dir.join(&rel);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))?;
        std::fs::write(&path, data)?;
        let mode = if rel.ends_with(".sh") { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build.rs` names every directory a file is embedded from, so adding a
    /// file anywhere in the tree rebuilds the binary.
    #[test]
    fn the_build_script_watches_every_embedded_directory() {
        let watched: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/plugin-dirs.txt"))
            .lines()
            .collect();
        // `plugins` and `plugins/pi` hold files that are not embedded
        // (`node_modules`, tests), so they are not watched as a whole.
        let unwatched = [Path::new("plugins"), Path::new("plugins/pi")];
        for (rel, _) in files() {
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
        let names: Vec<String> = files().into_iter().map(|(n, _)| n).collect();
        for want in [
            "plugins/claude-code/.claude-plugin/plugin.json",
            "plugins/claude-code/.mcp.json",
            "plugins/clax/.codex-plugin/plugin.json",
            "plugins/clax/.mcp.json",
            "plugins/clax/scripts/ensure-clax.sh",
            "plugins/pi/package.json",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} missing: {names:?}");
        }
    }
}
