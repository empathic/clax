//! The core `serve` runs, started without a listener (`clax_server::core`).

use clax_server::core::{Core, CoreConfig};
use tower::ServiceExt;

fn cfg(
    home: &std::path::Path,
    web: std::sync::Arc<dyn clax_core::embedded::FileTree>,
    quickstart: bool,
) -> CoreConfig {
    CoreConfig {
        home: clax_core::Home::at(home.to_path_buf()),
        version: "test",
        reap_interval: std::time::Duration::from_secs(60),
        codex: clax_realtime::push::CodexPush::default(),
        sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
        web,
        hosted: None,
        require_credential: true,
        bind: std::net::Ipv4Addr::LOCALHOST.into(),
        port: 0,
        embed: None,
        quickstart,
    }
}

#[tokio::test]
async fn a_core_answers_its_router_with_no_listener_and_writes_no_daemon_json() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), clax_testkit::web_ui(), false), &mut |_| {})
        .await
        .unwrap();
    let req = axum::http::Request::get("/healthz")
        .header("host", "localhost")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = core.router().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["version"], "test");
    // A daemon is given no build identity: `build` is null, not absent.
    assert_eq!(health.get("build"), Some(&serde_json::Value::Null));
    assert!(health.get("commit").is_some(), "{health}");
    assert!(!dir.path().join("daemon.json").exists());
    core.stop().await;
}

#[tokio::test]
async fn stopping_a_core_closes_its_store_and_ends_its_streams() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), clax_testkit::web_ui(), false), &mut |_| {})
        .await
        .unwrap();
    let store = core.state().store.clone();
    let mut shutdown = core.shutdown_requested();
    assert!(store.call(|_| Ok(())).await.is_ok(), "the store answers");
    core.stop().await;
    assert!(
        store.call(|_| Ok(())).await.is_err(),
        "a stopped core's store refuses calls"
    );
    assert!(
        *shutdown.borrow_and_update(),
        "its streams were told to end"
    );
    let again = Core::start(cfg(dir.path(), clax_testkit::web_ui(), false), &mut |_| {})
        .await
        .unwrap();
    again.stop().await;
}

#[tokio::test]
async fn a_daemon_has_no_trim_route() {
    let ts = clax_testkit::TestServer::spawn().await;
    let res = ts.post_json("/api/core/trim", serde_json::json!({})).await;
    assert_eq!(res.status(), 404);
}

fn tree(mark: &str) -> std::sync::Arc<dyn clax_core::embedded::FileTree> {
    std::sync::Arc::new(clax_testkit::quickstart_tree(mark))
}

async fn get(core: &Core, path: &str) -> serde_json::Value {
    let req = axum::http::Request::get(path)
        .header("host", "localhost")
        .header("authorization", format!("Bearer {}", core.state().token))
        .body(axum::body::Body::empty())
        .unwrap();
    let res = core.router().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200, "{path}");
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn artifacts(core: &Core) -> Vec<serde_json::Value> {
    let list = get(core, "/api/artifacts").await;
    list.get("artifacts")
        .unwrap_or(&list)
        .as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .clone()
}

#[tokio::test]
async fn a_fresh_home_starts_with_the_quickstart_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), tree("a"), true), &mut |_| {})
        .await
        .unwrap();
    let list = artifacts(&core).await;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["title"], "Clax Quickstart");
    assert_eq!(list[0]["pinned"], true);
    core.stop().await;
}

#[tokio::test]
async fn a_second_start_seeds_nothing_more() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), tree("a"), true), &mut |_| {})
        .await
        .unwrap();
    core.stop().await;
    let core = Core::start(cfg(dir.path(), tree("a"), true), &mut |_| {})
        .await
        .unwrap();
    assert_eq!(artifacts(&core).await.len(), 1);
    core.stop().await;
}

#[tokio::test]
async fn a_new_release_reaches_an_untouched_quickstart_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), tree("a"), true), &mut |_| {})
        .await
        .unwrap();
    core.stop().await;
    let b = clax_core::quickstart::Release::from_tree(&clax_testkit::quickstart_tree("b")).unwrap();
    let core = Core::start(cfg(dir.path(), tree("b"), true), &mut |_| {})
        .await
        .unwrap();
    let list = artifacts(&core).await;
    assert_eq!(list.len(), 1);
    let id = list[0]["id"].as_str().unwrap();
    let versions = get(&core, &format!("/api/artifacts/{id}/versions")).await;
    let versions = versions.get("versions").unwrap_or(&versions);
    let versions = versions.as_array().unwrap_or_else(|| panic!("{versions}"));
    assert_eq!(versions.len(), 2, "{versions:?}");
    let labels: Vec<_> = versions.iter().map(|v| v["label"].clone()).collect();
    assert!(
        labels.contains(&serde_json::json!(format!("Quickstart {}", b.id))),
        "{labels:?}"
    );
    core.stop().await;
}

#[tokio::test]
async fn the_switch_off_seeds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(cfg(dir.path(), tree("a"), false), &mut |_| {})
        .await
        .unwrap();
    assert!(artifacts(&core).await.is_empty());
    assert!(core.state().store.quickstart_of(None).unwrap().is_none());
    core.stop().await;
}

#[tokio::test]
async fn a_web_ui_without_the_quickstart_seeds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let bare: std::sync::Arc<dyn clax_core::embedded::FileTree> =
        std::sync::Arc::new(clax_core::embedded::MemTree(
            [("index.html".to_string(), b"<p>".to_vec())]
                .into_iter()
                .collect(),
        ));
    let core = Core::start(cfg(dir.path(), bare, true), &mut |_| {})
        .await
        .unwrap();
    assert!(artifacts(&core).await.is_empty());
    assert!(core.state().store.quickstart_of(None).unwrap().is_none());
    core.stop().await;
    let core = Core::start(cfg(dir.path(), tree("a"), true), &mut |_| {})
        .await
        .unwrap();
    assert_eq!(artifacts(&core).await.len(), 1);
    core.stop().await;
}

#[tokio::test]
async fn a_hosted_start_seeds_no_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg(dir.path(), tree("a"), true);
    c.hosted = clax_http::hosted::Hosted::new("http://localhost:1", Some("the setup secret"))
        .map(std::sync::Arc::new);
    assert!(c.hosted.is_some());
    let core = Core::start(c, &mut |_| {}).await.unwrap();
    let store = &core.state().store;
    assert!(store.list_artifacts().unwrap().is_empty());
    assert!(store.quickstart_of(None).unwrap().is_none());
    core.stop().await;
}
