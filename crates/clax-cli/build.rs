//! Rebuilds the binary when the plugin tree it embeds (`src/plugins.rs`)
//! changes, including when a file is added. rust-embed's `include_bytes!`
//! makes cargo track the files that existed at the last build, but not new
//! ones, so this script names every embedded directory as well. The list is
//! also written to `$OUT_DIR/plugin-dirs.txt` for a test to check.

use std::path::{Path, PathBuf};

/// The embedded roots, relative to the repository, as in `src/plugins.rs`.
const DIRS: &[&str] = &[
    "plugins/claude-code",
    "plugins/clax",
    "plugins/pi/src",
    "plugins/pi/skills",
];
const FILES: &[&str] = &[
    "plugins/pi/package.json",
    "plugins/pi/README.md",
    ".claude-plugin/marketplace.json",
    ".agents/plugins/marketplace.json",
];

fn walk(repo: &Path, rel: PathBuf, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(repo.join(&rel)) else {
        return;
    };
    out.push(rel.clone());
    let mut subs: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| rel.join(e.file_name()))
        .collect();
    subs.sort();
    for s in subs {
        walk(repo, s, out);
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let repo = manifest.join("../..");
    println!("cargo:rerun-if-changed=build.rs");
    let mut dirs = Vec::new();
    for d in DIRS {
        walk(&repo, PathBuf::from(d), &mut dirs);
    }
    for d in &dirs {
        println!("cargo:rerun-if-changed={}", repo.join(d).display());
    }
    for f in FILES {
        println!("cargo:rerun-if-changed={}", repo.join(f).display());
    }
    let list: String = dirs.iter().map(|d| format!("{}\n", d.display())).collect();
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    std::fs::write(out.join("plugin-dirs.txt"), list).expect("OUT_DIR is writable");
}
