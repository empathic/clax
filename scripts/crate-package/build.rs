//! The build script of the generated `clax` package (scripts/crate-package.py
//! writes the package; this file is copied in unchanged). It stands in for
//! the build scripts of the crates the package bundles, whose paths assume
//! the workspace:
//!
//! - `CLAX_BUILD_COMMIT` (clax-cli): the environment's when set, else the
//!   commit the package was assembled from, in `BUILD_COMMIT`.
//! - `CLAX_EXTENSION_PUBLIC_KEY` (clax-core): `web/extension/key/key.pub.b64`
//!   when the package carries it.
//! - `CLAX_WEB_DIST_DIR`, `CLAX_EXTENSION_DIST_DIR` and `CLAX_PLUGINS_DIR`
//!   (clax-embedded): the package's own copies.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    println!("cargo::rerun-if-changed=build.rs");

    println!("cargo::rerun-if-env-changed=CLAX_BUILD_COMMIT");
    println!("cargo::rerun-if-changed=BUILD_COMMIT");
    let commit = std::env::var("CLAX_BUILD_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| std::fs::read_to_string(root.join("BUILD_COMMIT")).ok())
        .map(|c| c.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    assert!(
        commit == "unknown" || is_commit(&commit),
        "CLAX_BUILD_COMMIT must be 7 to 64 lowercase hex digits or `unknown`"
    );
    println!("cargo::rustc-env=CLAX_BUILD_COMMIT={commit}");

    let key = root.join("web/extension/key/key.pub.b64");
    if let Ok(text) = std::fs::read_to_string(&key) {
        let text = text.trim();
        if !text.is_empty() {
            println!("cargo::rustc-env=CLAX_EXTENSION_PUBLIC_KEY={text}");
        }
    }

    for (var, rel) in [
        ("CLAX_WEB_DIST_DIR", "web/dist/"),
        ("CLAX_EXTENSION_DIST_DIR", "web/dist-extension/"),
        ("CLAX_PLUGINS_DIR", "plugins"),
    ] {
        println!("cargo::rustc-env={var}={}", root.join(rel).display());
    }
}

/// Whether `c` is a commit name: 7 to 64 lowercase hex digits.
fn is_commit(c: &str) -> bool {
    (7..=64).contains(&c.len())
        && c.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
