//! The integration tests of `clax-mcp` in one test binary, a module per file in
//! `tests/`: every test binary costs a link and, on macOS, an assessment of
//! the new executable on its first run.

mod calls;
mod channel;
mod comments;
mod common;
mod db;
mod live;
mod open;
mod open_status;
mod shim;
mod tools;

/// Cargo's test autodiscovery is off for this crate (`autotests = false` in
/// its manifest), so a file in `tests/`, or a directory there with a
/// `main.rs` or `mod.rs`, runs only once a test binary's root declares it as
/// a module.
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
