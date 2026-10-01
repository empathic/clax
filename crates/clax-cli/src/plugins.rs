//! The plugins this binary was built with, and writing them out as a
//! marketplace directory (`clax init`). The layout matches the repository:
//! `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json` and
//! `plugins/{claude-code,clax,pi}`, so both manifests' `./plugins/<name>`
//! sources resolve. The Pi package carries what its `package.json` ships.

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
pub fn materialize(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = root.parent().expect("the marketplace root has a parent");
    std::fs::create_dir_all(parent)?;
    let pid = std::process::id();
    let tmp = parent.join(format!(".marketplace.{pid}.tmp"));
    let old = parent.join(format!(".marketplace.{pid}.old"));
    let _ = std::fs::remove_dir_all(&tmp);
    for (rel, data) in files() {
        let path = tmp.join(&rel);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))?;
        std::fs::write(&path, data)?;
        let mode = if rel.ends_with(".sh") { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    if root.exists() {
        std::fs::rename(root, &old)?;
    }
    std::fs::rename(&tmp, root)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}
