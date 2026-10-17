//! The content tests that drive the server only through its routes, run
//! again on [`Deferred`](clax_testkit::Deferred) content
//! (`tests/api_hosted_content.rs` tests it). A module of `common`, so that
//! each test target Bazel deals `tests/integration.rs`'s modules into
//! (`clax_split`, `bazel/cargo.bzl`) runs it over the modules it holds.

use clax_testkit::{CONTENT_MODE_VAR, deferred_content};

/// The modules whose tests run again on deferred content.
const MODULES: &[&str] = &[
    "api_artifacts",
    "api_content",
    "api_assets",
    "api_threads",
    "api_live",
    "api_live_site",
    "api_live_sites",
    "api_live_watch",
];

/// Whether this test binary holds `module`: every one without `clax_split`.
fn holds(module: &str) -> bool {
    if !cfg!(clax_split) {
        return true;
    }
    match module {
        "api_artifacts" => cfg!(clax_split = "api_artifacts"),
        "api_content" => cfg!(clax_split = "api_content"),
        "api_assets" => cfg!(clax_split = "api_assets"),
        "api_threads" => cfg!(clax_split = "api_threads"),
        "api_live" => cfg!(clax_split = "api_live"),
        "api_live_site" => cfg!(clax_split = "api_live_site"),
        "api_live_sites" => cfg!(clax_split = "api_live_sites"),
        "api_live_watch" => cfg!(clax_split = "api_live_watch"),
        _ => false,
    }
}

/// The content tests that drive the server only through its routes pass
/// on deferred content: this test binary runs them again with
/// [`CONTENT_MODE_VAR`] set, so every test daemon runs on one connection
/// with content that completes only when awaited. Left out are the tests
/// that read or change the home's files (the default content store's
/// layout, local paths handed to agents) or call a synchronous store
/// method themselves. Each module this binary holds must have run.
#[test]
fn the_content_tests_pass_when_content_io_needs_an_executor() {
    if deferred_content() || !module_path!().starts_with("integration::") {
        return;
    }
    let held: Vec<&str> = MODULES.iter().copied().filter(|m| holds(m)).collect();
    if held.is_empty() {
        return;
    }
    const FILES: &[&str] = &[
        "api_content::wrapped_pages_are_cached_per_file_until_the_artifact_is_deleted",
        "api_live::a_live_pages_blobs_and_snapshots_route_are_hidden_from_the_lan",
        "api_live::a_live_pages_blob_is_refused_before_its_bytes_are_read",
        "api_live_site::a_failed_move_leaves_no_page_made_for_it",
        "api_live_watch::a_scope_watch_whose_page_cannot_be_made_is_not_kept",
        "api_live_watch::thread_views_carry_the_page_url_and_snapshot_path_of_a_live_page",
        "api_threads::every_thread_of_every_artifact_needs_the_token",
        "api_threads::create_stores_the_clip_and_serves_it_sandboxed",
        "api_threads::a_thread_view_names_the_clip_file_to_local_agents_alone",
        "api_threads::thread_events_never_carry_the_clip_path",
        "api_threads::viewers_reopen_and_delete_threads_with_events",
    ];
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    for m in &held {
        cmd.arg(format!("{m}::"));
    }
    for t in FILES {
        cmd.args(["--skip", t]);
    }
    let out = cmd.env(CONTENT_MODE_VAR, "deferred").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let passed: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("test result: ok. "))
        .and_then(|l| l.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    if !cfg!(clax_split) {
        assert!(passed > 100, "{passed} passed:\n{stdout}");
    }
    for m in &held {
        let prefix = format!("test {m}::");
        assert!(
            stdout
                .lines()
                .any(|l| l.starts_with(&prefix) && l.ends_with(" ok")),
            "no test of {m} passed:\n{stdout}"
        );
    }
}
