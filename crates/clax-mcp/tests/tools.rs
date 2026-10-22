use clax_mcp::tools::{
    AssetUploadArgs, FileArg, FileEncoding, ForkArgs, ListArgs, ListScope, PublishArgs, ReadArgs,
    StatusArgs, TagArgs, TagsArgs, TargetArgs,
};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_testkit::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::Value;
use std::collections::BTreeMap;

fn tools_for(ts: &TestServer) -> ClaxTools {
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    )
}

/// The JSON object a tool result carries, and whether it is an error result.
fn body(r: &CallToolResult) -> (Value, bool) {
    assert_eq!(r.content.len(), 1, "{r:?}");
    let text = &r.content[0].as_text().expect("text content").text;
    let v: Value = serde_json::from_str(text).expect("result text is JSON");
    (v, r.is_error == Some(true))
}

fn ok(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let (v, is_error) = body(&r.expect("tool result, not a protocol error"));
    assert!(!is_error, "{v}");
    assert_eq!(v["feedback"], serde_json::json!([]), "{v}");
    v
}

fn err(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let (v, is_error) = body(&r.expect("tool result, not a protocol error"));
    assert!(is_error, "{v}");
    v
}

fn html(title: &str, page: &str) -> PublishArgs {
    PublishArgs {
        html: Some(page.to_string()),
        title: Some(title.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn publish_then_read_round_trips_the_page_and_files() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let mut files = BTreeMap::new();
    files.insert(
        "app.js".to_string(),
        Some(FileArg {
            content: Some("console.log(1)".into()),
            ..Default::default()
        }),
    );
    let pub1 = ok(t
        .publish(Parameters(PublishArgs {
            files: Some(files),
            ..html("Hello", "<h1>hi</h1>")
        }))
        .await);
    let id = pub1["artifact_id"].as_str().unwrap().to_string();
    assert_eq!(pub1["version"], 1);
    assert_eq!(pub1["title"], "Hello");
    assert_eq!(
        pub1["url"],
        format!("http://localhost:{}/a/{id}", ts.addr.port())
    );
    assert_eq!(pub1["files"], serde_json::json!(["app.js", "index.html"]));

    let page = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: pub1["url"].as_str().unwrap().to_string(),
            ..Default::default()
        }))
        .await);
    assert_eq!(page["artifact_id"], id.as_str());
    assert_eq!(page["version"], 1);
    assert_eq!(page["path"], "index.html");
    assert_eq!(page["content"], "<h1>hi</h1>");
    assert_eq!(page["truncated"], false);
    assert_eq!(page["size"], 11);

    let js = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id.clone(),
            path: Some("app.js".into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(js["content"], "console.log(1)");

    let pub2 = ok(t
        .publish(Parameters(PublishArgs {
            id: Some(id.clone()),
            if_version: Some(1),
            ..html("Hello", "<h1>v2</h1>")
        }))
        .await);
    assert_eq!(pub2["version"], 2);
    assert_eq!(pub2["files"], serde_json::json!(["app.js", "index.html"]));
    let old = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id,
            version: Some(1),
            ..Default::default()
        }))
        .await);
    assert_eq!(old["content"], "<h1>hi</h1>");
}

#[tokio::test]
async fn stale_if_version_is_an_error_result_naming_current() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let id = ok(t.publish(Parameters(html("A", "<p>1"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = err(t
        .publish(Parameters(PublishArgs {
            id: Some(id.clone()),
            if_version: Some(7),
            ..html("A", "<p>2")
        }))
        .await);
    assert_eq!(e["error"]["code"], "conflict");
    assert_eq!(e["error"]["current"], 1);
    let cur = &e["error"]["current_version"];
    assert_eq!(cur["n"], 1, "{e}");
    assert_eq!(cur["files"], serde_json::json!(["index.html"]));
    assert!(cur["created_at"].is_string());
    assert!(cur["label"].is_null());
    assert_eq!(
        cur["url"],
        format!("http://localhost:{}/a/{id}/v/1", ts.addr.port())
    );
    assert_eq!(
        e["error"]["hint"],
        "read the current version, merge your change, and retry with if_version = 1"
    );
}

#[tokio::test]
async fn publish_requires_exactly_one_page_source() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let e = err(t.publish(Parameters(PublishArgs::default())).await);
    assert_eq!(e["error"]["code"], "invalid_args");
    let e = err(t
        .publish(Parameters(PublishArgs {
            file_path: Some("/nope.html".into()),
            ..html("A", "<p>")
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args");
}

#[tokio::test]
async fn publish_reads_local_files() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.html");
    std::fs::write(&page, "<p>from disk").unwrap();
    let png = dir.path().join("dot.png");
    std::fs::write(&png, [0x89u8, b'P', b'N', b'G', 0, 0xff]).unwrap();
    let mut files = BTreeMap::new();
    files.insert(
        "img/dot.png".to_string(),
        Some(FileArg {
            path: Some(png.to_string_lossy().into()),
            ..Default::default()
        }),
    );
    let p = ok(t
        .publish(Parameters(PublishArgs {
            file_path: Some(page.to_string_lossy().into()),
            files: Some(files),
            title: Some("Disk".into()),
            ..Default::default()
        }))
        .await);
    let id = p["artifact_id"].as_str().unwrap().to_string();
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id.clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content"], "<p>from disk");
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id,
            path: Some("img/dot.png".into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content_type"], "image/png");
    assert_eq!(r["content_base64"], "iVBORwD/");
    assert!(r.get("content").is_none(), "{r}");
    assert_eq!(r["size"], 6);
}

#[tokio::test]
async fn read_truncates_a_large_page() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let big = "x".repeat(1_000_000);
    let id = ok(t.publish(Parameters(html("Big", &big))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id.clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["truncated"], true);
    assert_eq!(r["size"], 1_000_000);
    assert_eq!(r["content"].as_str().unwrap().len(), 200_000);

    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id,
            max_bytes: Some(10),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content"], "xxxxxxxxxx");
}

#[tokio::test]
async fn binary_over_the_cap_returns_no_content() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let mut files = BTreeMap::new();
    files.insert(
        "blob.bin".to_string(),
        Some(FileArg {
            content: Some("AAAAAAAAAAAA".into()),
            encoding: Some(FileEncoding::Base64),
            ..Default::default()
        }),
    );
    let id = ok(t
        .publish(Parameters(PublishArgs {
            files: Some(files),
            ..html("B", "<p>")
        }))
        .await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: id,
            path: Some("blob.bin".into()),
            max_bytes: Some(4),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["truncated"], true);
    assert_eq!(r["size"], 9);
    assert!(r.get("content").is_none() && r.get("content_base64").is_none());
}

#[tokio::test]
async fn list_is_pinned_first_then_most_recent_and_honours_limit() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let mut ids = Vec::new();
    for title in ["one", "two", "three"] {
        let p = ok(t.publish(Parameters(html(title, "<p>"))).await);
        ids.push(p["artifact_id"].as_str().unwrap().to_string());
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let pinned = ok(t
        .pin(Parameters(TargetArgs {
            url_or_id: ids[0].clone(),
        }))
        .await);
    assert_eq!(pinned["pinned"], true);

    let l = ok(t.list(Parameters(ListArgs::default())).await);
    let got: Vec<&str> = l["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(got, ["one", "three", "two"]);
    let first = &l["artifacts"][0];
    assert_eq!(first["id"], ids[0].as_str());
    assert_eq!(
        first["url"],
        format!("http://localhost:{}/a/{}", ts.addr.port(), ids[0])
    );
    assert_eq!(first["version"], 1);
    assert_eq!(first["pinned"], true);
    assert!(first["updated_at"].is_string());
    assert!(first["owner_session_id"].is_null());

    let l = ok(t
        .list(Parameters(ListArgs {
            limit: Some(1),
            ..Default::default()
        }))
        .await);
    assert_eq!(l["artifacts"].as_array().unwrap().len(), 1);

    let u = ok(t
        .unpin(Parameters(TargetArgs {
            url_or_id: ids[0].clone(),
        }))
        .await);
    assert_eq!(u["pinned"], false);
    ok(t.delete(Parameters(TargetArgs {
        url_or_id: ids[1].clone(),
    }))
    .await);
    let l = ok(t.list(Parameters(ListArgs::default())).await);
    assert_eq!(l["artifacts"].as_array().unwrap().len(), 2);
    let e = err(t
        .read(Parameters(ReadArgs {
            url_or_id: ids[1].clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(e["error"]["code"], "not_found");
}

#[tokio::test]
async fn asset_upload_returns_blob_urls() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let id = ok(t.publish(Parameters(html("A", "<p>"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, [1u8, 2, 3]).unwrap();
    std::fs::write(&b, "hello").unwrap();
    let r = ok(t
        .asset_upload(Parameters(AssetUploadArgs {
            url_or_id: id.clone(),
            file_path: Some(a.to_string_lossy().into()),
            file_paths: Some(vec![b.to_string_lossy().into()]),
        }))
        .await);
    let assets = r["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0]["content_type"], "image/png");
    assert_eq!(assets[0]["size"], 3);
    assert_eq!(assets[1]["content_type"], "text/plain");
    // On the artifact's site host, where its pages name it.
    let url = assets[0]["url"].as_str().unwrap();
    let (origin, blob_path) = url.split_once(&format!(":{}", ts.addr.port())).unwrap();
    assert_eq!(origin, format!("http://{id}.localhost"), "{url}");
    assert!(blob_path.starts_with("/_clax/blob/"), "{url}");
    let res = ts.site_get(&id, blob_path).await;
    assert_eq!(res.bytes().await.unwrap().as_ref(), [1u8, 2, 3]);
}

/// A file the daemon would refuse for its type is refused before any file
/// is uploaded, naming it.
#[tokio::test]
async fn asset_upload_refuses_an_unsupported_file_before_uploading_any() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let id = ok(t.publish(Parameters(html("A", "<p>"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.exe");
    std::fs::write(&a, [1u8, 2, 3]).unwrap();
    std::fs::write(&b, [0u8]).unwrap();
    let b = b.to_string_lossy().to_string();
    let e = err(t
        .asset_upload(Parameters(AssetUploadArgs {
            url_or_id: id.clone(),
            file_path: None,
            file_paths: Some(vec![a.to_string_lossy().into(), b.clone()]),
        }))
        .await);
    assert_eq!(e["error"]["code"], "unsupported_type", "{e}");
    assert_eq!(e["error"]["path"], b.as_str(), "{e}");
    assert!(
        ts.store
            .list_assets(&clax_core::ArtifactId::parse(&id).unwrap())
            .unwrap()
            .is_empty(),
        "nothing was uploaded"
    );
}

/// A create past its deadline may have made the artifact, whose ID the
/// agent never saw: the timeout says to look for it with `list` before
/// publishing again, not to read an artifact it cannot name.
#[tokio::test]
async fn a_create_that_times_out_says_to_look_for_the_artifact_with_list() {
    // A server that takes every request and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let t = ClaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None)
            .with_publish_timeout(std::time::Duration::from_millis(200)),
        format!("http://localhost:{port}"),
        None,
        dir.path().join("daemon.log"),
    );
    let e = err(t.publish(Parameters(html("A", "<p>"))).await);
    assert_eq!(e["error"]["code"], "timeout", "{e}");
    let m = e["error"]["message"].as_str().unwrap();
    assert!(m.contains("call list"), "{m}");
    assert!(!m.contains("read the artifact"), "{m}");
}

/// An update without `if_version` on an artifact the daemon reports no
/// current version for is `bad_response`, and nothing is published (no
/// version 0 is sent to conflict on).
#[tokio::test]
async fn an_update_without_a_known_current_version_is_bad_response() {
    use axum::routing::{get, post};
    let publishes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = axum::Router::new()
        .route(
            "/api/artifacts/{id}",
            get(|| async { axum::Json(serde_json::json!({"artifact": {"id": "7q3k9mzx2b4t"}})) }),
        )
        .route(
            "/api/artifacts/{id}/versions",
            post({
                let publishes = publishes.clone();
                move || async move {
                    publishes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::http::StatusCode::CREATED
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let dir = tempfile::tempdir().unwrap();
    let t = ClaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None),
        format!("http://localhost:{port}"),
        None,
        dir.path().join("daemon.log"),
    );
    let e = err(t
        .publish(Parameters(PublishArgs {
            id: Some("7q3k9mzx2b4t".into()),
            ..html("A", "<p>")
        }))
        .await);
    assert_eq!(e["error"]["code"], "bad_response", "{e}");
    assert_eq!(publishes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// A daemon refusal part way through names the file it refused and the
/// assets already uploaded, so a retry sends only the rest.
#[tokio::test]
async fn asset_upload_failing_part_way_names_the_assets_already_uploaded() {
    use axum::routing::post;
    let uploads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = axum::Router::new().route(
        "/api/artifacts/{id}/assets",
        post({
            let uploads = uploads.clone();
            move || async move {
                use axum::http::StatusCode;
                let n = uploads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    (
                        StatusCode::CREATED,
                        axum::Json(serde_json::json!({
                            "asset": {"id": "01K6AB7D2E3F4G5H6J7K8M9N0P", "content_type": "image/png", "size": 3},
                            "url": "/_blob/01K6AB7D2E3F4G5H6J7K8M9N0P",
                        })),
                    )
                } else {
                    (
                        StatusCode::INSUFFICIENT_STORAGE,
                        axum::Json(serde_json::json!({
                            "error": {"code": "storage_full", "message": "no room"},
                        })),
                    )
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let dir = tempfile::tempdir().unwrap();
    let t = ClaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None),
        format!("http://localhost:{port}"),
        None,
        dir.path().join("daemon.log"),
    );
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    std::fs::write(&a, [1u8, 2, 3]).unwrap();
    std::fs::write(&b, [4u8, 5, 6]).unwrap();
    let b = b.to_string_lossy().to_string();
    let e = err(t
        .asset_upload(Parameters(AssetUploadArgs {
            url_or_id: "7q3k9mzx2b4t".into(),
            file_path: None,
            file_paths: Some(vec![a.to_string_lossy().into(), b.clone()]),
        }))
        .await);
    assert_eq!(e["error"]["code"], "storage_full", "{e}");
    assert_eq!(e["error"]["path"], b.as_str(), "{e}");
    assert_eq!(
        e["error"]["uploaded"],
        serde_json::json!([{
            "id": "01K6AB7D2E3F4G5H6J7K8M9N0P",
            "url": format!("http://localhost:{port}/_blob/01K6AB7D2E3F4G5H6J7K8M9N0P"),
            "content_type": "image/png",
            "size": 3,
        }]),
        "{e}"
    );
}

#[tokio::test]
async fn status_reports_the_daemon() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let s = ok(t.status(Parameters(StatusArgs::default())).await);
    assert_eq!(
        s["daemon_url"],
        format!("http://localhost:{}", ts.addr.port())
    );
    assert_eq!(s["version"], "test");
    assert!(s["session"].is_null());
    assert!(s["harness"].is_null());
    assert_eq!(s["watches"], serde_json::json!([]));
    assert!(s["push"].is_null(), "no session, no push");
}

#[tokio::test]
async fn unreachable_daemon_is_an_error_result_naming_the_log() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("logs").join("daemon.log");
    let t = ClaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None),
        format!("http://localhost:{port}"),
        None,
        log.clone(),
    );
    for r in [
        t.status(Parameters(StatusArgs::default())).await,
        t.list(Parameters(ListArgs::default())).await,
        t.publish(Parameters(html("x", "<p>"))).await,
    ] {
        let e = err(r);
        assert_eq!(e["error"]["code"], "daemon_unreachable", "{e}");
        assert_eq!(e["error"]["log"], log.to_string_lossy().as_ref());
    }
}

/// Registers a live session whose working directory is `cwd` and returns tools
/// that publish as it.
async fn session_tools(ts: &TestServer, cwd: &str) -> (ClaxTools, clax_core::model::Session) {
    let res = ts
        .post_json(
            "/api/sessions",
            serde_json::json!({"harness": "claude", "cwd": cwd}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let session: clax_core::model::Session = serde_json::from_value(v["session"].clone()).unwrap();
    let tools = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(session.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(session.clone()),
        ts.home.log_path(),
    );
    (tools, session)
}

#[tokio::test]
async fn relative_paths_need_a_session_working_directory() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    // daemon.json-style relative names must never resolve against the daemon's cwd.
    let e = err(t
        .publish(Parameters(PublishArgs {
            file_path: Some("page.html".into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("file paths must be absolute: there is no session working directory"),
        "{e}"
    );
    let mut files = BTreeMap::new();
    files.insert(
        "data.json".to_string(),
        Some(FileArg {
            path: Some("daemon.json".into()),
            ..Default::default()
        }),
    );
    let e = err(t
        .publish(Parameters(PublishArgs {
            files: Some(files),
            ..html("A", "<p>")
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args");
    let id = ok(t.publish(Parameters(html("A", "<p>"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = err(t
        .asset_upload(Parameters(AssetUploadArgs {
            url_or_id: id,
            file_path: Some("a.png".into()),
            file_paths: None,
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args");

    // With a session, relative paths resolve against its working directory.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("page.html"), "<p>relative").unwrap();
    let (st, _) = session_tools(&ts, &dir.path().to_string_lossy()).await;
    let p = ok(st
        .publish(Parameters(PublishArgs {
            file_path: Some("page.html".into()),
            title: Some("Relative".into()),
            ..Default::default()
        }))
        .await);
    let r = ok(st
        .read(Parameters(ReadArgs {
            url_or_id: p["artifact_id"].as_str().unwrap().to_string(),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content"], "<p>relative");
}

#[tokio::test]
async fn list_scope_mine_filters_by_owner_session() {
    let ts = TestServer::spawn().await;
    let anon = tools_for(&ts);
    let (st, session) = session_tools(&ts, "/tmp").await;
    ok(anon.publish(Parameters(html("theirs", "<p>"))).await);
    let mine_id = ok(st.publish(Parameters(html("mine", "<p>"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let titles = |v: &Value| -> Vec<String> {
        v["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["title"].as_str().unwrap().to_string())
            .collect()
    };
    let mine = ok(st
        .list(Parameters(ListArgs {
            scope: Some(ListScope::Mine),
            ..Default::default()
        }))
        .await);
    assert_eq!(titles(&mine), ["mine"]);
    assert_eq!(mine["artifacts"][0]["id"], mine_id.as_str());
    assert_eq!(
        mine["artifacts"][0]["owner_session_id"],
        session.id.as_str()
    );
    let all = ok(st
        .list(Parameters(ListArgs {
            scope: Some(ListScope::All),
            ..Default::default()
        }))
        .await);
    assert_eq!(titles(&all), ["mine", "theirs"]);
    assert_eq!(
        titles(&ok(st.list(Parameters(ListArgs::default())).await)).len(),
        2
    );
    // Without a session, `mine` is empty.
    let none = ok(anon
        .list(Parameters(ListArgs {
            scope: Some(ListScope::Mine),
            ..Default::default()
        }))
        .await);
    assert_eq!(titles(&none), Vec::<String>::new());
}

#[tokio::test]
async fn read_accepts_every_url_form_and_takes_the_version_from_it() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let port = ts.addr.port();
    let id = ok(t.publish(Parameters(html("V", "<p>one"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(t.publish(Parameters(PublishArgs {
        id: Some(id.clone()),
        ..html("V", "<p>two")
    }))
    .await);
    for (url, want_version, want) in [
        (format!("http://localhost:{port}/a/{id}"), 2, "<p>two"),
        (format!("http://localhost:{port}/a/{id}/v/1"), 1, "<p>one"),
        (format!("http://127.0.0.1:{port}/c/{id}/v/1/"), 1, "<p>one"),
        (
            format!("http://{id}.localhost:{port}/_clax/v/1/index.html"),
            1,
            "<p>one",
        ),
    ] {
        let r = ok(t
            .read(Parameters(ReadArgs {
                url_or_id: url.clone(),
                ..Default::default()
            }))
            .await);
        assert_eq!(r["version"], want_version, "{url}");
        assert_eq!(r["content"], want, "{url}");
    }
    // An explicit `version` wins over the URL's.
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: format!("http://localhost:{port}/a/{id}/v/1"),
            version: Some(2),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content"], "<p>two");
}

#[tokio::test]
async fn a_non_json_success_body_is_bad_response() {
    // A plain HTTP server that answers every request 200 with a non-JSON body.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
                    )
                    .await;
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let t = ClaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None),
        format!("http://localhost:{port}"),
        None,
        dir.path().join("daemon.log"),
    );
    let e = err(t.status(Parameters(StatusArgs::default())).await);
    assert_eq!(e["error"]["code"], "bad_response", "{e}");
}

#[tokio::test]
async fn a_new_artifact_takes_its_title_from_the_page_when_none_is_given() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let p = ok(t
        .publish(Parameters(PublishArgs {
            html: Some("<html><head><TITLE lang=en>\n Q3 &amp; Q4 </TITLE></head></html>".into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(p["title"], "Q3 & Q4");

    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.html");
    std::fs::write(&page, "<title>From disk</title><p>").unwrap();
    let p = ok(t
        .publish(Parameters(PublishArgs {
            file_path: Some(page.to_string_lossy().into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(p["title"], "From disk");

    // An explicit title wins over the page's.
    let p = ok(t
        .publish(Parameters(html("Given", "<title>Page</title>")))
        .await);
    assert_eq!(p["title"], "Given");

    // An update without a title keeps the artifact's.
    let id = p["artifact_id"].as_str().unwrap().to_string();
    let p = ok(t
        .publish(Parameters(PublishArgs {
            id: Some(id),
            html: Some("<title>Other</title>".into()),
            ..Default::default()
        }))
        .await);
    assert_eq!(p["title"], "Given");
}

#[tokio::test]
async fn a_new_artifact_without_any_title_is_invalid_args_naming_both_remedies() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    for page in ["<p>no title", "<title>  </title><p>"] {
        let e = err(t
            .publish(Parameters(PublishArgs {
                html: Some(page.into()),
                ..Default::default()
            }))
            .await);
        assert_eq!(e["error"]["code"], "invalid_args", "{e}");
        let msg = e["error"]["message"].as_str().unwrap();
        assert!(msg.contains("`title`") && msg.contains("<title>"), "{msg}");
    }
    let list = ok(t.list(Parameters(ListArgs::default())).await);
    assert!(list["artifacts"].as_array().unwrap().is_empty());
}

/// Tools over a managed client that registers `registration` with the test
/// daemon, as the stdio shim does.
fn managed_tools(
    ts: &TestServer,
    registration: clax_core::RegisterSession,
) -> (ClaxTools, DaemonClient) {
    let endpoint = clax_mcp::client::Endpoint {
        base: ts.base.clone(),
        browser_base: format!("http://localhost:{}", ts.addr.port()),
        token: ts.token.clone(),
    };
    let find: clax_mcp::client::Refresh = std::sync::Arc::new(move || Ok(endpoint.clone()));
    let client = DaemonClient::managed(find.clone(), find, registration);
    let tools = ClaxTools::new(client.clone(), String::new(), None, ts.home.log_path());
    (tools, client)
}

async fn end_session(ts: &TestServer, id: &str) {
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{id}", ts.base)))
        .json(&serde_json::json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

async fn live_sessions(ts: &TestServer) -> Vec<Value> {
    let v: Value = ts
        .get_authed("/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    v["sessions"].as_array().unwrap().clone()
}

fn claude_registration() -> clax_core::RegisterSession {
    clax_core::RegisterSession {
        harness: "claude".into(),
        harness_session_id: Some("cc-1".into()),
        cwd: "/work".into(),
        pid: Some(4_000_000_000),
        parent_pid: Some(4_000_000_001),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_publish_after_the_session_ended_registers_a_new_session() {
    let ts = TestServer::spawn().await;
    let (t, client) = managed_tools(&ts, claude_registration());
    ok(t.publish(Parameters(html("one", "<p>1"))).await);
    let first = client.session().unwrap();
    end_session(&ts, &first.id).await;

    let p = ok(t.publish(Parameters(html("two", "<p>2"))).await);
    let second = client.session().unwrap();
    assert_ne!(second.id, first.id);
    assert!(second.ended_at.is_none());
    let live = live_sessions(&ts).await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["id"], second.id.as_str());
    assert_eq!(live[0]["harness_session_id"], "cc-1");
    let got: Value = ts
        .get_authed(&format!(
            "/api/artifacts/{}",
            p["artifact_id"].as_str().unwrap()
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["artifact"]["owner_session_id"], second.id.as_str());
}

#[tokio::test]
async fn a_heartbeat_on_an_ended_session_registers_a_new_session() {
    let ts = TestServer::spawn().await;
    let (_t, client) = managed_tools(&ts, claude_registration());
    client.ensure_session().await.unwrap();
    let first = client.session().unwrap();
    end_session(&ts, &first.id).await;

    let beat = client.heartbeat().await.unwrap();
    assert_ne!(beat.id, first.id);
    assert!(beat.ended_at.is_none());
    assert_eq!(client.session().unwrap().id, beat.id);
    let live = live_sessions(&ts).await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["id"], beat.id.as_str());
}

#[tokio::test]
async fn a_working_directory_filled_in_later_is_used_after_the_next_heartbeat() {
    let ts = TestServer::spawn().await;
    let (t, client) = managed_tools(
        &ts,
        clax_core::RegisterSession {
            harness: "codex".into(),
            harness_session_id: None,
            cwd: String::new(),
            pid: Some(4_000_000_000),
            parent_pid: Some(4_000_000_001),
            ..Default::default()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("page.html"), "<title>Later</title>").unwrap();
    let relative = || PublishArgs {
        file_path: Some("page.html".into()),
        ..Default::default()
    };
    let e = err(t.publish(Parameters(relative())).await);
    assert_eq!(e["error"]["code"], "invalid_args");
    let msg = e["error"]["message"].as_str().unwrap();
    assert!(msg.contains("has no working directory yet"), "{msg}");

    // The Codex SessionStart hook fills the row's cwd.
    let res = ts
        .post_json(
            "/api/sessions/join",
            serde_json::json!({
                "harness": "codex",
                "parent_pid": 4_000_000_001u32,
                "harness_session_id": "codex-1",
                "cwd": dir.path().to_string_lossy(),
            }),
        )
        .await;
    assert_eq!(res.status(), 200);
    client.heartbeat().await.unwrap();
    let p = ok(t.publish(Parameters(relative())).await);
    assert_eq!(p["title"], "Later");
}

#[tokio::test]
async fn status_reports_the_plugin_version_and_skew_when_known() {
    let ts = TestServer::spawn().await;
    let s = ok(tools_for(&ts)
        .status(Parameters(StatusArgs::default()))
        .await);
    assert!(
        s.get("plugin_version").is_none() && s.get("skew").is_none(),
        "{s}"
    );
    assert_eq!(s["binary"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(!s["binary"]["path"].as_str().unwrap().is_empty(), "{s}");
    assert!(s.get("upgrade_held").is_none(), "{s}");
    let old = tools_for(&ts).with_plugin_version(Some("0.1.0".into()));
    let s = ok(old.status(Parameters(StatusArgs::default())).await);
    assert_eq!(s["plugin_version"], "0.1.0");
    assert_eq!(s["skew"], true);
    let same = tools_for(&ts).with_plugin_version(Some(env!("CARGO_PKG_VERSION").into()));
    let s = ok(same.status(Parameters(StatusArgs::default())).await);
    assert_eq!(s["plugin_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(s["skew"], false);
}

#[tokio::test]
async fn status_reports_a_failed_upgrade_that_holds_the_daemon_back() {
    let ts = TestServer::spawn().await;
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = asked.clone();
    let probe: clax_mcp::tools::UpgradeHoldProbe = std::sync::Arc::new(move |v: &str| {
        seen.lock().unwrap().push(v.to_string());
        Some(serde_json::json!({"version": "9.9.9", "reason": "it crashed"}))
    });
    let s = ok(tools_for(&ts)
        .with_upgrade_hold(probe)
        .status(Parameters(StatusArgs::default()))
        .await);
    assert_eq!(s["upgrade_held"]["reason"], "it crashed", "{s}");
    assert_eq!(
        *asked.lock().unwrap(),
        vec![s["version"].as_str().unwrap().to_string()]
    );
    let none: clax_mcp::tools::UpgradeHoldProbe = std::sync::Arc::new(|_: &str| None);
    let s = ok(tools_for(&ts)
        .with_upgrade_hold(none)
        .status(Parameters(StatusArgs::default()))
        .await);
    assert!(s.get("upgrade_held").is_none(), "{s}");
}

#[tokio::test]
async fn a_failed_reconnect_reaches_the_caller_with_its_reason() {
    let ts = TestServer::spawn().await;
    let endpoint = clax_mcp::client::Endpoint {
        base: ts.base.clone(),
        browser_base: format!("http://localhost:{}", ts.addr.port()),
        token: ts.token.clone(),
    };
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = calls.clone();
    // The first refresh finds the daemon; later ones fail as a rolled-back
    // upgrade does.
    let find: clax_mcp::client::Refresh = std::sync::Arc::new(move || {
        if c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(endpoint.clone())
        } else {
            Err(anyhow::anyhow!(
                "upgrading the clax daemon to v9: rolled back"
            ))
        }
    });
    let client = DaemonClient::managed(find.clone(), find, claude_registration());
    let t = ClaxTools::new(client.clone(), String::new(), None, ts.home.log_path());
    ok(t.publish(Parameters(html("one", "<p>1"))).await);
    end_session(&ts, &client.session().unwrap().id).await;
    // The session is gone, so the request is refused and refreshed; the
    // refresh fails, and its reason reaches the caller.
    let e = err(t.publish(Parameters(html("two", "<p>2"))).await);
    assert!(e.to_string().contains("rolled back"), "{e}");
}

#[tokio::test]
async fn draft_shows_people_work_in_progress_until_done_or_a_publish() {
    use clax_mcp::tools::DraftArgs;
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-12T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let (t, _session) = session_tools(&ts, "/w").await;
    let p = ok(t
        .publish(Parameters(html("Plan", "<h1>Plan</h1><p>One</p>")))
        .await);
    let id = p["artifact_id"].as_str().unwrap().to_string();
    let draft = |page: &str| DraftArgs {
        url_or_id: id.clone(),
        html: Some(page.to_string()),
        note: Some("rewording".into()),
        ..Default::default()
    };
    let d = ok(t.draft(Parameters(draft("<h1>Plan</h1><p>Two</p>"))).await);
    assert_eq!(d["drafting"], true);
    assert_eq!(d["base_version"], 1);
    assert_eq!(d["paths"], serde_json::json!(["index.html"]));
    assert_eq!(d["note"], "rewording");
    assert_eq!(d["expires_in_s"], 120);
    let listed: Value = ts
        .get_authed(&format!("/api/artifacts/{id}/drafts"))
        .await
        .json()
        .await
        .unwrap();
    let l = &listed["drafts"][0];
    let token = l["token"].as_str().unwrap();
    assert!(
        !d.to_string().contains(token),
        "the agent never holds the URL: {d}"
    );
    let page = ts
        .site_get(
            &id,
            &format!("/_clax/draft/{}/{token}/", l["agent"].as_str().unwrap()),
        )
        .await
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("<p data-clax-draft-changed>Two</p>"),
        "{page}"
    );
    // Two a second; the third is refused until the second is over.
    ok(t.draft(Parameters(draft("<h1>Plan</h1><p>Three</p>")))
        .await);
    let e = err(t.draft(Parameters(draft("<h1>Plan</h1><p>Four</p>"))).await);
    assert_eq!(e["error"]["code"], "rate_limited", "{e}");
    assert_eq!(e["error"]["retry_after_ms"], 1000, "{e}");
    // `done` drops it; nothing to draft is refused; a stale base conflicts.
    let gone = ok(t
        .draft(Parameters(DraftArgs {
            url_or_id: id.clone(),
            done: Some(true),
            ..Default::default()
        }))
        .await);
    assert_eq!(
        (&gone["drafting"], &gone["ended"]),
        (&Value::Bool(false), &Value::Bool(true))
    );
    let e = err(t
        .draft(Parameters(DraftArgs {
            url_or_id: id.clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args", "{e}");
    clock.advance(1);
    let e = err(t
        .draft(Parameters(DraftArgs {
            if_version: Some(5),
            ..draft("<p>x</p>")
        }))
        .await);
    assert_eq!(e["error"]["code"], "conflict", "{e}");
    assert_eq!(e["error"]["current_version"]["n"], 1, "{e}");
    // A publish of the artifact ends the draft.
    ok(t.draft(Parameters(draft("<h1>Plan</h1><p>Five</p>"))).await);
    ok(t.publish(Parameters(PublishArgs {
        id: Some(id.clone()),
        ..html("Plan", "<h1>Plan</h1><p>Five</p>")
    }))
    .await);
    let listed: Value = ts
        .get_authed(&format!("/api/artifacts/{id}/drafts"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["drafts"], serde_json::json!([]));
}

/// A host whose tokens are numbered, keeping whom each was handed to.
#[derive(Default)]
struct Remotes {
    minted: std::sync::Mutex<Vec<clax_http::git_remotes::Holder>>,
}

impl clax_http::git_remotes::GitRemotes for Remotes {
    fn mint<'a>(
        &'a self,
        artifact: &'a clax_core::ArtifactId,
        scope: clax_http::git_remotes::Scope,
        _ttl_s: u32,
        _origin: Option<&'a str>,
        holder: &'a clax_http::git_remotes::Holder,
    ) -> clax_http::git_remotes::RemoteFuture<'a, clax_http::git_remotes::Minted> {
        Box::pin(async move {
            let n = {
                let mut m = self.minted.lock().unwrap();
                m.push(holder.clone());
                m.len()
            };
            let remote = format!("http://clax.test/git/a-{}.git", artifact.as_str());
            let token = format!("cxm_{}_{n}", scope.as_str());
            Ok(clax_http::git_remotes::Minted {
                id: format!("cxt_{n}"),
                clone_command: format!(
                    "git -c http.extraHeader=\"Authorization: Bearer {token}\" clone {remote}"
                ),
                jj_clone_command: format!("jj git clone {remote}"),
                remote,
                token,
                expires_at: "2026-10-13T13:00:00.000Z".into(),
            })
        })
    }

    fn held(&self) -> clax_http::git_remotes::RemoteFuture<'_, Vec<clax_http::git_remotes::Held>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn revoke<'a>(
        &'a self,
        _artifact: &'a clax_core::ArtifactId,
        _id: &'a str,
    ) -> clax_http::git_remotes::RemoteFuture<'a, bool> {
        Box::pin(async { Ok(true) })
    }

    fn revoke_held<'a>(
        &'a self,
        _person: Option<&'a str>,
        _session: Option<&'a str>,
    ) -> clax_http::git_remotes::RemoteFuture<'a, usize> {
        Box::pin(async { Ok(0) })
    }
}

#[tokio::test]
async fn git_remote_hands_the_session_a_token_with_its_commands_and_says_where_there_is_none() {
    use clax_mcp::tools::{GitRemoteArgs, GitScope};
    let remotes = std::sync::Arc::new(Remotes::default());
    let r = remotes.clone();
    let ts = TestServer::spawn_with(move |s| s.git_remotes.set(r)).await;
    let (t, session) = session_tools(&ts, "/w").await;
    let p = ok(t.publish(Parameters(html("Plan", "<h1>Plan</h1>"))).await);
    let id = p["artifact_id"].as_str().unwrap().to_string();
    let g = ok(t
        .git_remote(Parameters(GitRemoteArgs {
            url_or_id: id.clone(),
            scope: GitScope::Write,
        }))
        .await);
    let remote = format!("http://clax.test/git/a-{id}.git");
    assert_eq!(g["artifact_id"], id.as_str());
    assert_eq!(g["scope"], "write");
    assert_eq!(g["remote"], remote.as_str());
    assert_eq!(g["token"], "cxm_write_1");
    assert_eq!(g["expires_at"], "2026-10-13T13:00:00.000Z");
    assert!(g["clone_command"].as_str().unwrap().ends_with(&remote));
    assert!(
        g["jj_clone_command"]
            .as_str()
            .unwrap()
            .contains("jj git clone")
    );
    assert_eq!(
        g["existing_checkout"]["add_remote_command"],
        format!("git remote add clax {remote}")
    );
    assert!(
        g["existing_checkout"]["push_command"]
            .as_str()
            .unwrap()
            .ends_with("push clax HEAD:main")
    );
    assert!(g["url"].as_str().unwrap().contains(&id));
    assert_eq!(
        remotes.minted.lock().unwrap()[0],
        clax_http::git_remotes::Holder {
            person: None,
            session: Some(session.id.clone()),
            link: false
        },
        "the token is the session's, so its pushes are"
    );

    // Where Clax keeps no repositories.
    let plain = TestServer::spawn().await;
    let (t, _) = session_tools(&plain, "/w").await;
    let p = ok(t.publish(Parameters(html("Plan", "<h1>Plan</h1>"))).await);
    let e = err(t
        .git_remote(Parameters(GitRemoteArgs {
            url_or_id: p["artifact_id"].as_str().unwrap().to_string(),
            scope: GitScope::Read,
        }))
        .await);
    assert_eq!(e["error"]["code"], "not_available", "{e}");
}

#[tokio::test]
async fn fork_makes_a_new_artifact_from_the_version_a_url_names() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let port = ts.addr.port();
    let id = ok(t.publish(Parameters(html("Plan", "<p>one"))).await)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(t.publish(Parameters(PublishArgs {
        id: Some(id.clone()),
        ..html("Plan", "<p>two")
    }))
    .await);
    let f = ok(t
        .fork(Parameters(ForkArgs {
            url_or_id: format!("http://localhost:{port}/a/{id}/v/1"),
            version: None,
        }))
        .await);
    let fid = f["artifact_id"].as_str().unwrap().to_string();
    assert_ne!(fid, id);
    assert_eq!(f["url"], format!("http://localhost:{port}/a/{fid}"));
    assert_eq!(f["version"], 1);
    assert_eq!(f["title"], "Plan");
    assert_eq!(
        f["forked_from"],
        serde_json::json!({"artifact_id": id, "version": 1, "title": "Plan"})
    );
    let r = ok(t
        .read(Parameters(ReadArgs {
            url_or_id: fid.clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(r["content"], "<p>one");
    // An explicit version wins; the current one by default.
    let f = ok(t
        .fork(Parameters(ForkArgs {
            url_or_id: id.clone(),
            version: None,
        }))
        .await);
    assert_eq!(f["forked_from"]["version"], 2);
    let e = err(t
        .fork(Parameters(ForkArgs {
            url_or_id: id.clone(),
            version: Some(9),
        }))
        .await);
    assert_eq!(e["error"]["code"], "unknown_version", "{e}");
    let e = err(t
        .fork(Parameters(ForkArgs {
            url_or_id: "zzzzzzzzzzzz".into(),
            version: None,
        }))
        .await);
    assert_eq!(e["error"]["code"], "not_found", "{e}");
}

/// Makes tag `name` as the owner does.
async fn owner_tag(ts: &TestServer, name: &str) {
    let res = ts
        .post_json("/api/tags", serde_json::json!({"name": name}))
        .await;
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn tags_lists_publish_applies_tag_changes_and_list_filters() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    // None yet: a publish naming a tag publishes nothing.
    let e = err(t
        .publish(Parameters(PublishArgs {
            tags: Some(vec!["draft".into()]),
            ..html("Plan", "<p>p</p>")
        }))
        .await);
    assert_eq!(e["error"]["code"], "unknown_tag", "{e}");
    let msg = e["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("no tags yet") && msg.contains("nothing was published"),
        "{msg}"
    );
    assert_eq!(
        ok(t.list(Parameters(ListArgs::default())).await)["artifacts"],
        serde_json::json!([])
    );
    owner_tag(&ts, "draft").await;
    owner_tag(&ts, "Final").await;
    let listed = ok(t.tags(Parameters(TagsArgs::default())).await);
    assert_eq!(
        listed["tags"],
        serde_json::json!([
            {"name": "draft", "colour": "gray", "description": null, "count": 0},
            {"name": "Final", "colour": "gray", "description": null, "count": 0},
        ])
    );
    let e = err(t
        .publish(Parameters(PublishArgs {
            tags: Some(vec!["draft".into(), "urgent".into()]),
            ..html("Plan", "<p>p</p>")
        }))
        .await);
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("draft, Final"),
        "{e}"
    );
    // A publish applies existing tags, by name in any case.
    let p = ok(t
        .publish(Parameters(PublishArgs {
            tags: Some(vec!["DRAFT".into()]),
            ..html("Plan", "<p>p</p>")
        }))
        .await);
    assert_eq!(p["tags"], serde_json::json!(["draft"]), "{p}");
    let id = p["artifact_id"].as_str().unwrap().to_string();
    ok(t.publish(Parameters(html("Other", "<p>o</p>"))).await);
    // `tag` adds and removes.
    let r = ok(t
        .tag(Parameters(TagArgs {
            url_or_id: id.clone(),
            add: Some(vec!["final".into()]),
            remove: Some(vec!["draft".into()]),
        }))
        .await);
    assert_eq!(
        (&r["tags"], &r["changed"]),
        (&serde_json::json!(["Final"]), &serde_json::json!(true))
    );
    let e = err(t
        .tag(Parameters(TagArgs {
            url_or_id: id.clone(),
            add: Some(vec!["urgent".into()]),
            remove: None,
        }))
        .await);
    assert_eq!(e["error"]["code"], "unknown_tag", "{e}");
    let e = err(t
        .tag(Parameters(TagArgs {
            url_or_id: id.clone(),
            ..Default::default()
        }))
        .await);
    assert_eq!(e["error"]["code"], "invalid_args", "{e}");
    // `list` shows the tags and filters on them, every one named.
    let all = ok(t.list(Parameters(ListArgs::default())).await);
    let plan = all["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == serde_json::json!(id))
        .unwrap()
        .clone();
    assert_eq!(plan["tags"], serde_json::json!(["Final"]));
    let only = ok(t
        .list(Parameters(ListArgs {
            tags: Some(vec!["final".into()]),
            ..Default::default()
        }))
        .await);
    let ids: Vec<&Value> = only["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["id"])
        .collect();
    assert_eq!(ids, vec![&serde_json::json!(id)]);
    let none = ok(t
        .list(Parameters(ListArgs {
            tags: Some(vec!["final".into(), "draft".into()]),
            ..Default::default()
        }))
        .await);
    assert_eq!(none["artifacts"], serde_json::json!([]));
    assert_eq!(
        ok(t.tags(Parameters(TagsArgs::default())).await)["tags"][1]["count"],
        1
    );
}

#[tokio::test]
async fn a_publish_whose_tags_are_refused_lands_and_reports_tags_error() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let max = clax_core::tags::MAX_ARTIFACT_TAGS;
    let names: Vec<String> = (0..=max).map(|i| format!("t{i}")).collect();
    for n in &names {
        owner_tag(&ts, n).await;
    }
    let p = ok(t.publish(Parameters(html("Full", "<p>1</p>"))).await);
    let id = p["artifact_id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/artifacts/{id}/tags", ts.base)),
        )
        .json(&serde_json::json!({"tags": names[..max].to_vec()}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // One more is past the bound: the version lands, the tags do not.
    let r = ok(t
        .publish(Parameters(PublishArgs {
            id: Some(id.clone()),
            tags: Some(vec![names[max].clone()]),
            ..html("Full", "<p>2</p>")
        }))
        .await);
    assert_eq!(r["version"], 2, "{r}");
    assert_eq!(r["tags_error"]["code"], "too_many_tags", "{r}");
    assert_eq!(r["tags"].as_array().unwrap().len(), max, "{r}");
}
