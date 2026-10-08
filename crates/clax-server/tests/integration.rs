//! The integration tests of `clax-server` in one test binary, a module per file
//! in `tests/` (but for those of `tests/web_dist.rs`): every test binary costs
//! a link and, on macOS, an assessment of the new executable on its first run.

mod api_artifacts;
mod api_assets;
mod api_attention;
mod api_auth;
mod api_batch;
mod api_caching;
mod api_changelog;
mod api_content;
mod api_docs;
mod api_events;
mod api_extension;
mod api_feedback;
mod api_gateway;
mod api_host;
mod api_inbox;
mod api_live;
mod api_live_site;
mod api_live_sites;
mod api_live_watch;
mod api_mcp;
mod api_notices;
mod api_owner;
mod api_presence;
mod api_push;
mod api_questions;
mod api_room;
mod api_sample;
mod api_sessions;
mod api_stream;
mod api_threads;
mod api_timeout;
mod api_viewers;
mod api_watches;
mod api_working;
mod api_working_auto;
mod common;
mod daemon;
mod sample_anthropic;

/// Cargo's test autodiscovery is off for this crate (`autotests = false` in
/// its manifest), so a file in `tests/` runs only once a test binary's root
/// declares it as a module.
#[test]
fn every_test_file_is_a_module() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let roots = ["integration", "web_dist"];
    let declared: Vec<String> = roots
        .iter()
        .map(|r| std::fs::read_to_string(tests.join(format!("{r}.rs"))).unwrap())
        .collect();
    for entry in std::fs::read_dir(&tests).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_str().unwrap();
        if roots.contains(&stem) {
            continue;
        }
        let decl = format!("mod {stem};");
        assert!(
            declared.iter().any(|d| d.lines().any(|l| l.trim() == decl)),
            "tests/{stem}.rs is not a module of any test binary: declare it in tests/{}.rs",
            roots[0]
        );
    }
}
