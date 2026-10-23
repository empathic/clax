//! The integration tests of `clax-server` in one test binary, a module per file
//! in `tests/`: every test binary costs a link and, on macOS, an assessment of
//! the new executable on its first run.
//!
//! Bazel builds each module as its own test target instead (`clax_split`,
//! `bazel/cargo.bzl`), so an edit recompiles, relinks and reruns each module
//! on its own, in parallel; `clax_split = "root"` holds this file's own tests.
//! Gate every new test module the same way.

// In every test target: it reruns, on deferred content, the content
// modules the target holds.
#[cfg(any(not(clax_split), clax_split = "api_agent_auth"))]
mod api_agent_auth;
#[cfg(any(not(clax_split), clax_split = "api_agents"))]
mod api_agents;
#[cfg(any(not(clax_split), clax_split = "api_app_bundles"))]
mod api_app_bundles;
#[cfg(any(not(clax_split), clax_split = "api_app_drafts"))]
mod api_app_drafts;
#[cfg(any(not(clax_split), clax_split = "api_app_session"))]
mod api_app_session;
#[cfg(any(not(clax_split), clax_split = "api_approvals"))]
mod api_approvals;
#[cfg(any(not(clax_split), clax_split = "api_artifacts"))]
mod api_artifacts;
#[cfg(any(not(clax_split), clax_split = "api_assets"))]
mod api_assets;
#[cfg(any(not(clax_split), clax_split = "api_attention"))]
mod api_attention;
#[cfg(any(not(clax_split), clax_split = "api_auth"))]
mod api_auth;
#[cfg(any(not(clax_split), clax_split = "api_batch"))]
mod api_batch;
#[cfg(any(not(clax_split), clax_split = "api_briefs"))]
mod api_briefs;
#[cfg(any(not(clax_split), clax_split = "api_caching"))]
mod api_caching;
#[cfg(any(not(clax_split), clax_split = "api_canary"))]
mod api_canary;
#[cfg(any(not(clax_split), clax_split = "api_changelog"))]
mod api_changelog;
#[cfg(any(not(clax_split), clax_split = "api_client_imports"))]
mod api_client_imports;
#[cfg(any(not(clax_split), clax_split = "api_content"))]
mod api_content;
#[cfg(any(not(clax_split), clax_split = "api_db_read"))]
mod api_db_read;
#[cfg(any(not(clax_split), clax_split = "api_docs"))]
mod api_docs;
#[cfg(any(not(clax_split), clax_split = "api_drafts"))]
mod api_drafts;
#[cfg(any(not(clax_split), clax_split = "api_events"))]
mod api_events;
#[cfg(any(not(clax_split), clax_split = "api_extension"))]
mod api_extension;
#[cfg(any(not(clax_split), clax_split = "api_feedback"))]
mod api_feedback;
#[cfg(any(not(clax_split), clax_split = "api_forks"))]
mod api_forks;
#[cfg(any(not(clax_split), clax_split = "api_gateway"))]
mod api_gateway;
#[cfg(any(not(clax_split), clax_split = "api_git_tokens"))]
mod api_git_tokens;
#[cfg(any(not(clax_split), clax_split = "api_grants"))]
mod api_grants;
#[cfg(any(not(clax_split), clax_split = "api_host"))]
mod api_host;
#[cfg(any(not(clax_split), clax_split = "api_hosted"))]
mod api_hosted;
#[cfg(any(not(clax_split), clax_split = "api_hosted_content"))]
mod api_hosted_content;
#[cfg(any(not(clax_split), clax_split = "api_hosted_github"))]
mod api_hosted_github;
#[cfg(any(not(clax_split), clax_split = "api_inbox"))]
mod api_inbox;
#[cfg(any(not(clax_split), clax_split = "api_jobs"))]
mod api_jobs;
#[cfg(any(not(clax_split), clax_split = "api_linked"))]
mod api_linked;
#[cfg(any(not(clax_split), clax_split = "api_live"))]
mod api_live;
#[cfg(any(not(clax_split), clax_split = "api_live_layers"))]
mod api_live_layers;
#[cfg(any(not(clax_split), clax_split = "api_live_site"))]
mod api_live_site;
#[cfg(any(not(clax_split), clax_split = "api_live_sites"))]
mod api_live_sites;
#[cfg(any(not(clax_split), clax_split = "api_live_watch"))]
mod api_live_watch;
#[cfg(any(not(clax_split), clax_split = "api_locked"))]
mod api_locked;
#[cfg(any(not(clax_split), clax_split = "api_mcp"))]
mod api_mcp;
#[cfg(any(not(clax_split), clax_split = "api_mcp_http"))]
mod api_mcp_http;
#[cfg(any(not(clax_split), clax_split = "api_net"))]
mod api_net;
#[cfg(any(not(clax_split), clax_split = "api_notices"))]
mod api_notices;
#[cfg(any(not(clax_split), clax_split = "api_owner"))]
mod api_owner;
#[cfg(any(not(clax_split), clax_split = "api_people"))]
mod api_people;
#[cfg(any(not(clax_split), clax_split = "api_perf"))]
mod api_perf;
#[cfg(any(not(clax_split), clax_split = "api_presence"))]
mod api_presence;
#[cfg(any(not(clax_split), clax_split = "api_push"))]
mod api_push;
#[cfg(any(not(clax_split), clax_split = "api_pushes"))]
mod api_pushes;
#[cfg(any(not(clax_split), clax_split = "api_questions"))]
mod api_questions;
#[cfg(any(not(clax_split), clax_split = "api_remotes"))]
mod api_remotes;
#[cfg(any(not(clax_split), clax_split = "api_remotes_app"))]
mod api_remotes_app;
#[cfg(any(not(clax_split), clax_split = "api_restore"))]
mod api_restore;
#[cfg(any(not(clax_split), clax_split = "api_room"))]
mod api_room;
#[cfg(any(not(clax_split), clax_split = "api_sample"))]
mod api_sample;
#[cfg(any(not(clax_split), clax_split = "api_seats"))]
mod api_seats;
#[cfg(any(not(clax_split), clax_split = "api_sessions"))]
mod api_sessions;
#[cfg(any(not(clax_split), clax_split = "api_stream"))]
mod api_stream;
#[cfg(any(not(clax_split), clax_split = "api_tags"))]
mod api_tags;
#[cfg(any(not(clax_split), clax_split = "api_threads"))]
mod api_threads;
#[cfg(any(not(clax_split), clax_split = "api_timeout"))]
mod api_timeout;
#[cfg(any(not(clax_split), clax_split = "api_tool_call_ids"))]
mod api_tool_call_ids;
#[cfg(any(not(clax_split), clax_split = "api_toolpath"))]
mod api_toolpath;
#[cfg(any(not(clax_split), clax_split = "api_viewers"))]
mod api_viewers;
#[cfg(any(not(clax_split), clax_split = "api_visibility"))]
mod api_visibility;
#[cfg(any(not(clax_split), clax_split = "api_watches"))]
mod api_watches;
#[cfg(any(not(clax_split), clax_split = "api_where_to_ask"))]
mod api_where_to_ask;
#[cfg(any(not(clax_split), clax_split = "api_working"))]
mod api_working;
#[cfg(any(not(clax_split), clax_split = "api_working_auto"))]
mod api_working_auto;
#[cfg(any(not(clax_split), clax_split = "app_push_pull_e2e"))]
mod app_push_pull_e2e;
#[cfg(any(not(clax_split), clax_split = "bridge_dev"))]
mod bridge_dev;
#[cfg(any(not(clax_split), clax_split = "core_start"))]
mod core_start;
#[cfg(any(not(clax_split), clax_split = "daemon"))]
mod daemon;
mod deferred;
#[cfg(any(not(clax_split), clax_split = "get_page"))]
mod get_page;
#[cfg(any(not(clax_split), clax_split = "golden_single_owner"))]
mod golden_single_owner;
#[cfg(any(not(clax_split), clax_split = "harness_ids"))]
mod harness_ids;
#[cfg(any(not(clax_split), clax_split = "hosted_pages"))]
mod hosted_pages;
#[cfg(any(not(clax_split), clax_split = "sample_anthropic"))]
mod sample_anthropic;
#[cfg(any(not(clax_split), clax_split = "shell_boot"))]
mod shell_boot;
#[cfg(any(not(clax_split), clax_split = "shell_entries"))]
mod shell_entries;

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
