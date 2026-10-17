//! The plugins this binary was built with, and writing them out as a
//! marketplace directory (`clax init`). The layout matches the repository:
//! `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json`,
//! `.grok-plugin/marketplace.json` and `plugins/{claude-code,clax,clax-grok,pi}`,
//! so the manifests' `./plugins/<name>` sources resolve. The files are the
//! binary's ([`clax_core::embedded::Bundle::plugins`]).

use std::path::Path;

/// Every file of the marketplace tree: (path relative to its root,
/// contents), sorted.
pub fn files() -> Vec<(String, Vec<u8>)> {
    clax_core::embedded::files_of(crate::embedded().plugins())
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
    materialize_with(root, &files(), |from, to| std::fs::rename(from, to))
}

/// [`materialize`] of `files`, renaming with `rename`.
fn materialize_with(
    root: &Path,
    files: &[(String, Vec<u8>)],
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
    let result = write_tree(&tmp, files).and_then(|()| swap(root, &tmp, &old, &rename));
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

fn write_tree(dir: &Path, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for (rel, data) in files {
        let path = dir.join(rel);
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

    /// A small tree laid out as the marketplace is.
    fn tree() -> Vec<(String, Vec<u8>)> {
        [
            ("plugins/clax-grok/scripts/ensure-clax.sh", "#!/bin/sh\n"),
            ("plugins/clax-grok/.grok-plugin/plugin.json", "{}"),
            ("plugins/clax-grok/.mcp.json", "{}"),
            ("plugins/clax-grok/hooks/hooks.json", "{}"),
            (
                ".grok-plugin/marketplace.json",
                r#"{"plugins": [{"source": "./plugins/clax-grok"}]}"#,
            ),
        ]
        .into_iter()
        .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
        .collect()
    }

    /// `materialize` writes the whole tree, with the wrappers executable
    /// and everything else not.
    #[test]
    fn materialize_writes_the_grok_plugin_and_marketplace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("marketplace");
        materialize_with(&root, &tree(), |from, to| std::fs::rename(from, to)).unwrap();
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
        assert!(materialize_with(&root, &tree(), fail_into(".tmp")).is_err());
        assert!(root.join("previous").is_file());
        // Putting it back fails too: it survives as `.old`.
        let e = materialize_with(&root, &tree(), |from: &Path, to: &Path| {
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
}
