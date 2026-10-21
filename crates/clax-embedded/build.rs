//! Names the directories the crate embeds, each from the environment when
//! set (Bazel sets them to its builds and copies) or else the checkout's:
//! `CLAX_WEB_DIST_DIR` (`web/dist`, `src/web.rs`), `CLAX_EXTENSION_DIST_DIR`
//! (`web/dist-extension`, `src/extension.rs`) and `CLAX_PLUGINS_DIR`
//! (`plugins/`, `src/plugins.rs`).
//!
//! It also names every embedded directory (the plugin trees, the extension
//! and the web UI, each where it is embedded from), so adding a file
//! rebuilds the crate: rust-embed's `include_bytes!` makes Cargo track the
//! files that existed at the last build, but not new ones. The list, by
//! each directory's place in the checkout, is also written to
//! `$OUT_DIR/plugin-dirs.txt` for a test to check.

use std::path::{Path, PathBuf};

/// The embedded plugin roots, relative to the plugins directory, as in
/// `src/plugins.rs`.
const PLUGIN_DIRS: &[&str] = &[
    "claude-code",
    "clax",
    "clax-grok",
    "pi/src",
    "pi/skills",
    "pi/scripts",
];
const FILES: &[&str] = &[
    "plugins/pi/package.json",
    "plugins/pi/README.md",
    ".claude-plugin/marketplace.json",
    ".agents/plugins/marketplace.json",
    ".grok-plugin/marketplace.json",
];

/// Every directory under `root` (itself included), each as `(its name
/// under `name`, where it is)`.
fn walk(root: &Path, name: PathBuf, out: &mut Vec<(PathBuf, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    out.push((name.clone(), root.to_path_buf()));
    let mut subs: Vec<std::ffi::OsString> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name())
        .collect();
    subs.sort();
    for s in subs {
        walk(&root.join(&s), name.join(&s), out);
    }
}

/// The value of `var`, else `default`, as `cargo` is told to rebuild on.
fn root(var: &str, default: String) -> String {
    println!("cargo::rerun-if-env-changed={var}");
    std::env::var(var).unwrap_or(default)
}

/// [`walk`]s `sub` of `root`, the directory embedded from; when that is not
/// there yet (Bazel places its plugins copy only for the compile), `sub` of
/// the checkout's `fallback` instead.
fn walk_root(
    root: &str,
    fallback: &Path,
    sub: &str,
    name: PathBuf,
    out: &mut Vec<(PathBuf, PathBuf)>,
) {
    let at = Path::new(root).join(sub);
    if at.is_dir() {
        walk(&at, name, out);
    } else {
        walk(&fallback.join(sub), name, out);
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let repo = manifest.join("../..");
    println!("cargo:rerun-if-changed=build.rs");
    let plugins = root("CLAX_PLUGINS_DIR", format!("{}/plugins", repo.display()));
    let dist = root(
        "CLAX_EXTENSION_DIST_DIR",
        format!("{}/web/dist-extension/", repo.display()),
    );
    let web = root("CLAX_WEB_DIST_DIR", format!("{}/web/dist/", repo.display()));
    let mut dirs = Vec::new();
    for d in PLUGIN_DIRS {
        let name = Path::new("plugins").join(d);
        walk_root(&plugins, &repo.join("plugins"), d, name, &mut dirs);
    }
    let checkout_web = repo.join("web");
    walk_root(
        &dist,
        &checkout_web.join("dist-extension"),
        "",
        PathBuf::from("web/dist-extension"),
        &mut dirs,
    );
    walk_root(
        &web,
        &checkout_web.join("dist"),
        "",
        PathBuf::from("web/dist"),
        &mut dirs,
    );
    for (_, at) in &dirs {
        println!("cargo:rerun-if-changed={}", at.display());
    }
    for f in FILES {
        println!("cargo:rerun-if-changed={}", repo.join(f).display());
    }
    let list: String = dirs
        .iter()
        .map(|(name, _)| format!("{}\n", name.display()))
        .collect();
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    std::fs::write(out.join("plugin-dirs.txt"), list).expect("OUT_DIR is writable");
    println!("cargo::rustc-env=CLAX_PLUGINS_DIR={plugins}");
    println!("cargo::rustc-env=CLAX_EXTENSION_DIST_DIR={dist}");
    println!("cargo::rustc-env=CLAX_WEB_DIST_DIR={web}");
}
