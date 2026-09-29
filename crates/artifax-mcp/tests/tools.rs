use artifax_mcp::tools::{
    FileArg, FileEncoding, ListArgs, PublishArgs, ReadArgs, StatusArgs, TargetArgs,
};
use artifax_mcp::{ArtifaxTools, DaemonClient};
use artifax_server::testing::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::Value;
use std::collections::BTreeMap;

fn tools_for(ts: &TestServer) -> ArtifaxTools {
    ArtifaxTools::new(
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
            id: Some(id),
            if_version: Some(7),
            ..html("A", "<p>2")
        }))
        .await);
    assert_eq!(e["error"]["code"], "conflict");
    assert_eq!(e["error"]["current"], 1);
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

    let l = ok(t.list(Parameters(ListArgs { limit: Some(1) })).await);
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
        .asset_upload(Parameters(artifax_mcp::tools::AssetUploadArgs {
            url_or_id: id,
            file_path: Some(a.to_string_lossy().into()),
            file_paths: Some(vec![b.to_string_lossy().into()]),
        }))
        .await);
    let assets = r["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0]["content_type"], "image/png");
    assert_eq!(assets[0]["size"], 3);
    assert_eq!(assets[1]["content_type"], "text/plain");
    let url = assets[0]["url"].as_str().unwrap();
    let blob_path = url.split_once(&format!(":{}", ts.addr.port())).unwrap().1;
    assert!(blob_path.starts_with("/_blob/"), "{url}");
    let res = ts.get(blob_path).await;
    assert_eq!(res.bytes().await.unwrap().as_ref(), [1u8, 2, 3]);
}

#[tokio::test]
async fn status_reports_the_daemon() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let s = ok(t.status(Parameters(StatusArgs {})).await);
    assert_eq!(
        s["daemon_url"],
        format!("http://localhost:{}", ts.addr.port())
    );
    assert_eq!(s["version"], "test");
    assert!(s["session"].is_null());
    assert!(s["harness"].is_null());
    assert_eq!(s["watches"], serde_json::json!([]));
}

#[tokio::test]
async fn unreachable_daemon_is_an_error_result_naming_the_log() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("logs").join("daemon.log");
    let t = ArtifaxTools::new(
        DaemonClient::new(format!("http://127.0.0.1:{port}"), "t".into(), None),
        format!("http://localhost:{port}"),
        None,
        log.clone(),
    );
    for r in [
        t.status(Parameters(StatusArgs {})).await,
        t.list(Parameters(ListArgs::default())).await,
        t.publish(Parameters(html("x", "<p>"))).await,
    ] {
        let e = err(r);
        assert_eq!(e["error"]["code"], "daemon_unreachable", "{e}");
        assert_eq!(e["error"]["log"], log.to_string_lossy().as_ref());
    }
}
