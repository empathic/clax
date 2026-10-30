//! The integration tests of `clax-mcp` in one test binary, a module per file in
//! `tests/`: every test binary costs a link and, on macOS, an assessment of
//! the new executable on its first run.
//!
//! Bazel builds each module as its own test target instead (`clax_split`,
//! `bazel/cargo.bzl`), so an edit recompiles, relinks and reruns each module
//! on its own, in parallel; `clax_split = "root"` holds this file's own tests.
//! Gate every new test module the same way.

#[cfg(any(not(clax_split), clax_split = "calls"))]
mod calls;
#[cfg(any(not(clax_split), clax_split = "channel"))]
mod channel;
#[cfg(any(not(clax_split), clax_split = "comments"))]
mod comments;
#[cfg(any(not(clax_split), clax_split = "connect_instance"))]
mod connect_instance;
#[cfg(any(not(clax_split), clax_split = "db"))]
mod db;
#[cfg(any(not(clax_split), clax_split = "live"))]
mod live;
#[cfg(any(not(clax_split), clax_split = "open"))]
mod open;
#[cfg(any(not(clax_split), clax_split = "open_status"))]
mod open_status;
#[cfg(any(not(clax_split), clax_split = "people"))]
mod people;
#[cfg(any(not(clax_split), clax_split = "publish_public"))]
mod publish_public;
#[cfg(any(not(clax_split), clax_split = "remote"))]
mod remote;
#[cfg(any(not(clax_split), clax_split = "shim"))]
mod shim;
#[cfg(any(not(clax_split), clax_split = "site_access"))]
mod site_access;
#[cfg(any(not(clax_split), clax_split = "tools"))]
mod tools;

/// Cargo's test autodiscovery is off for this crate (`autotests = false` in
/// its manifest), so a file in `tests/`, or a directory there with a
/// `main.rs` or `mod.rs`, runs only once a test binary's root declares it as
/// a module.
#[cfg(any(not(clax_split), clax_split = "root"))]
#[test]
fn every_test_file_is_a_module() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let roots = ["integration"];
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
