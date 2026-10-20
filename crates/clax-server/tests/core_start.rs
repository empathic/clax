//! The core `serve` runs, started without a listener (`clax_server::core`).

use clax_server::core::{Core, CoreConfig};
use tower::ServiceExt;

fn cfg(home: &std::path::Path) -> CoreConfig {
    CoreConfig {
        home: clax_core::Home::at(home.to_path_buf()),
        version: "test",
        reap_interval: std::time::Duration::from_secs(60),
        codex: clax_realtime::push::CodexPush::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web: clax_testkit::web_ui(),
        hosted: None,
        require_credential: true,
        bind: std::net::Ipv4Addr::LOCALHOST.into(),
        port: 0,
        embed: None,
    }
}

#[tokio::test]
async fn a_core_answers_its_router_with_no_listener_and_writes_no_daemon_json() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
    let req = axum::http::Request::get("/healthz")
        .header("host", "localhost")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = core.router().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(!dir.path().join("daemon.json").exists());
    core.stop().await;
}

#[tokio::test]
async fn stopping_a_core_closes_its_store_so_another_opens_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
    core.stop().await;
    let again = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
    again.stop().await;
}

#[tokio::test]
async fn a_daemon_has_no_trim_route() {
    let ts = clax_testkit::TestServer::spawn().await;
    let res = ts.post_json("/api/core/trim", serde_json::json!({})).await;
    assert_eq!(res.status(), 404);
}
