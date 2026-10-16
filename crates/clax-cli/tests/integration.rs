//! The integration tests of `clax-cli` in one test binary, a module per file in
//! `tests/` (but for `tests/descriptors.rs`, a binary of its own): every test
//! binary costs a link and, on macOS, an assessment of the new executable on
//! its first run.
//!
//! Bazel builds each module as its own test target instead (`clax_split`,
//! `bazel/cargo.bzl`), so an edit recompiles, relinks and reruns each module
//! on its own, in parallel; `clax_split = "root"` holds this file's own tests.
//! Gate every new test module the same way.
//!
//! The tests talk to their test daemons directly, as browsers and scripts
//! do; the rule that commands reach the daemon only through the marked
//! clients (`clippy.toml`) is for the crate's own code.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

#[cfg(any(not(clax_split), clax_split = "bin"))]
mod bin;
#[cfg(any(not(clax_split), clax_split = "clean_break"))]
mod clean_break;
#[cfg(any(not(clax_split), clax_split = "cli"))]
mod cli;
#[cfg(any(not(clax_split), clax_split = "comments"))]
mod comments;
#[cfg_attr(clax_split, allow(dead_code, unused_imports, unused_macros))]
mod common;
#[cfg(any(not(clax_split), clax_split = "connect"))]
mod connect;
#[cfg(any(not(clax_split), clax_split = "extension"))]
mod extension;
#[cfg(any(not(clax_split), clax_split = "first_start"))]
mod first_start;
#[cfg(any(not(clax_split), clax_split = "follow"))]
mod follow;
#[cfg(any(not(clax_split), clax_split = "grok_dedupe"))]
mod grok_dedupe;
#[cfg(any(not(clax_split), clax_split = "hook_audit"))]
mod hook_audit;
#[cfg(any(not(clax_split), clax_split = "hook_call_ids"))]
mod hook_call_ids;
#[cfg(any(not(clax_split), clax_split = "init"))]
mod init;
#[cfg(any(not(clax_split), clax_split = "journal"))]
mod journal;
#[cfg(any(not(clax_split), clax_split = "mcp_preflight"))]
mod mcp_preflight;
#[cfg(any(not(clax_split), clax_split = "native_host"))]
mod native_host;
#[cfg(any(not(clax_split), clax_split = "people"))]
mod people;
#[cfg(any(not(clax_split), clax_split = "standdown"))]
mod standdown;
#[cfg(any(not(clax_split), clax_split = "switch"))]
mod switch;
#[cfg(any(not(clax_split), clax_split = "verify_harnesses"))]
mod verify_harnesses;
#[cfg(any(not(clax_split), clax_split = "version"))]
mod version;

/// Cargo's test autodiscovery is off for this crate (`autotests = false` in
/// its manifest), so a file in `tests/`, or a directory there with a
/// `main.rs` or `mod.rs`, runs only once a test binary's root declares it as
/// a module.
#[cfg(any(not(clax_split), clax_split = "root"))]
#[test]
fn every_test_file_is_a_module() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let roots = ["integration", "descriptors"];
    let declared: Vec<String> = roots
        .iter()
        .map(|r| std::fs::read_to_string(tests.join(format!("{r}.rs"))).unwrap())
        .collect();
    for entry in std::fs::read_dir(&tests).unwrap() {
        let path = entry.unwrap().path();
        let (stem, what) = if path.is_dir() {
            if !path.join("main.rs").exists() && !path.join("mod.rs").exists() {
                continue;
            }
            (path.file_name().unwrap().to_str().unwrap(), "/")
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            (path.file_stem().unwrap().to_str().unwrap(), ".rs")
        } else {
            continue;
        };
        if roots.contains(&stem) {
            continue;
        }
        let decl = format!("mod {stem};");
        assert!(
            declared.iter().any(|d| d.lines().any(|l| l.trim() == decl)),
            "tests/{stem}{what} is not a module of any test binary: declare it in tests/{}.rs",
            roots[0]
        );
    }
}
