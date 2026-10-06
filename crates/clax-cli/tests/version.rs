//! `clax --version` is exactly `clax <version>`, which the plugin wrappers
//! and installers compare; `clax version --verbose` names the commit the
//! binary was built from.
use assert_cmd::Command;
use std::path::Path;

/// The commit this checkout's build embeds: the `CLAX_BUILD_COMMIT` override
/// the build saw, else `HEAD` of the repository whose root is the workspace,
/// else `unknown`.
fn expected_commit() -> String {
    if let Some(c) = option_env!("CLAX_BUILD_COMMIT").filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let ours = git(&["rev-parse", "--show-toplevel"])
        .is_some_and(|t| std::fs::canonicalize(t).ok() == std::fs::canonicalize(&root).ok());
    ours.then(|| git(&["rev-parse", "HEAD"]))
        .flatten()
        .unwrap_or_else(|| "unknown".into())
}

fn clax(args: &[&str]) -> String {
    let out = Command::cargo_bin("clax")
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn version_is_exactly_clax_and_the_version() {
    let want = format!("clax {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(clax(&["--version"]), want);
    assert_eq!(clax(&["version"]), want);
}

#[test]
fn version_verbose_names_commit() {
    let commit = expected_commit();
    assert_eq!(
        clax(&["version", "--verbose"]),
        format!("clax {}\ncommit {commit}\n", env!("CARGO_PKG_VERSION"))
    );
    let v: serde_json::Value = serde_json::from_str(&clax(&["version", "--json"])).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "commit": commit})
    );
}
