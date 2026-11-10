//! Names the web UI the Worker embeds (`src/web.rs`): `CLAX_WEB_DIST_DIR`
//! when set (Bazel sets it to its build of `web/dist`), else the checkout's
//! `web/dist`, and keeps the crate in step with it.
//!
//! rust-embed's `include_bytes!` makes Cargo track the files that existed
//! when the crate last compiled, but not files added since: a Worker built
//! before the web UI keeps an empty tree after the UI is built. Naming the
//! directory here makes Cargo rebuild the crate when anything under it
//! changes.
//!
//! A wasm32 build, the Worker itself, refuses a `web/dist` without an
//! `index.html` (a checkout holds only `.gitkeep` there until
//! `scripts/build-web.sh` runs): such a Worker answers every shell page with
//! `503 ui_not_built`. Native builds, which only run the tests and clippy,
//! do not need it.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=CLAX_WEB_DIST_DIR");
    let dist = std::env::var_os("CLAX_WEB_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../web/dist"));
    println!("cargo::rustc-env=CLAX_WEB_DIST_DIR={}", dist.display());
    println!("cargo::rerun-if-changed={}", dist.display());
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
    if wasm && !dist.join("index.html").is_file() {
        println!(
            "cargo::error={} has no index.html: build the web UI first (scripts/build-web.sh); \
             a Worker without it answers every page with 503 ui_not_built",
            dist.display()
        );
    }
}
