//! The plugins this binary was built with, and writing them out as a
//! marketplace directory (`clax init`). The layout matches the repository:
//! `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json`,
//! `.grok-plugin/marketplace.json` and `plugins/{claude-code,clax,clax-grok,pi}`,
//! so the manifests' `./plugins/<name>` sources resolve. The Pi package carries what its `package.json` ships.
//! `build.rs` rebuilds the binary when any of it changes.

use rust_embed::RustEmbed;
use std::path::Path;

/// One embed per embedded directory: a debug build reads embeds from disk on
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
plugin_dir!(ClaudeCode, "../../plugins/claude-code/", "claude-code/");
plugin_dir!(Codex, "../../plugins/clax/", "clax/");
plugin_dir!(Grok, "../../plugins/clax-grok/", "clax-grok/");
plugin_dir!(PiSrc, "../../plugins/pi/src/", "pi/src/");
plugin_dir!(PiSkills, "../../plugins/pi/skills/", "pi/skills/");

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

/// Every file of the marketplace tree: (path relative to its root, contents).
pub fn files() -> Vec<(String, Vec<u8>)> {
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
    out.sort();
    out
}

/// Writes the tree to `root`, replacing what is there. It is built in a
/// sibling temporary directory and renamed into place, so a harness never
/// reads a half-written plugin. Scripts (`*.sh`) are 0755, other files 0644.
///
/// The caller holds the init lock, so any `.marketplace.*.tmp` or `.old`
/// sibling left by an interrupted run is stale and is removed first. On an
/// error the temporary tree is removed too, and the previous tree is put
/// back; if even that fails, it is kept as `.marketplace.<pid>.old` and the
/// error says so.
pub fn materialize(root: &Path) -> std::io::Result<()> {
    materialize_with(root, |from, to| std::fs::rename(from, to))
}

fn materialize_with(
    root: &Path,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
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
    let result = write_tree(&tmp).and_then(|()| swap(root, &tmp, &old, &rename));
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Moves `root` aside to `old` and `tmp` into its place. When the second
/// rename fails, `old` goes back to `root`, or is kept where it is.
fn swap(
    root: &Path,
    tmp: &Path,
    old: &Path,
    rename: &impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let moved = root.exists();
    if moved {
        rename(root, old)?;
    }
    if let Err(e) = rename(tmp, root) {
        if moved && rename(old, root).is_err() {
            return Err(std::io::Error::new(
                e.kind(),
                format!("{e}; the previous marketplace is kept at {}", old.display()),
            ));
        }
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(old);
    Ok(())
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

    /// `materialize` writes the whole tree, with the wrappers executable
    /// and everything else not.
    #[test]
    fn materialize_writes_the_grok_plugin_and_marketplace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("marketplace");
        materialize(&root).unwrap();
        let mode = |rel: &str| {
            std::fs::metadata(root.join(rel))
                .unwrap_or_else(|e| panic!("{rel}: {e}"))
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("plugins/clax-grok/scripts/ensure-clax.sh"), 0o755);
        assert_eq!(mode("plugins/clax-grok/.grok-plugin/plugin.json"), 0o644);
        assert_eq!(mode("plugins/clax-grok/.mcp.json"), 0o644);
        assert_eq!(mode("plugins/clax-grok/hooks/hooks.json"), 0o644);
        let index = std::fs::read_to_string(root.join(".grok-plugin/marketplace.json")).unwrap();
        assert!(index.contains("./plugins/clax-grok"), "{index}");
    }

    /// A failed final rename puts the previous tree back; when that fails
    /// too, the previous tree survives as `.old`.
    #[test]
    fn a_failed_swap_keeps_the_previous_marketplace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("marketplace");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("previous"), "x").unwrap();
        let fail_into = |target: &'static str| {
            move |from: &Path, to: &Path| {
                if to.file_name().is_some_and(|n| n == "marketplace")
                    && from.to_string_lossy().ends_with(target)
                {
                    Err(std::io::Error::other("injected"))
                } else {
                    std::fs::rename(from, to)
                }
            }
        };
        // Only the new tree's rename fails: the previous one is put back.
        assert!(materialize_with(&root, fail_into(".tmp")).is_err());
        assert!(root.join("previous").is_file());
        // Putting it back fails too: it survives as `.old`.
        let e = materialize_with(&root, |from: &Path, to: &Path| {
            if to.file_name().is_some_and(|n| n == "marketplace") {
                Err(std::io::Error::other("injected"))
            } else {
                std::fs::rename(from, to)
            }
        })
        .unwrap_err();
        let old = dir
            .path()
            .join(format!(".marketplace.{}.old", std::process::id()));
        assert!(old.join("previous").is_file(), "{e}");
        assert!(e.to_string().contains("kept at"), "{e}");
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
