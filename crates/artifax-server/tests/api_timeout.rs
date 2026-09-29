mod common;
use common::TestServer;

#[tokio::test]
async fn slow_api_requests_time_out_with_json_408() {
    let ts = TestServer::spawn_with(|state| {
        state.request_timeout = std::time::Duration::from_millis(200);
    })
    .await;
    let res = ts.get("/api/_test/sleep/1000").await;
    assert_eq!(res.status(), 408);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "timeout");
}

#[tokio::test]
async fn events_stream_is_exempt_from_the_timeout() {
    let ts = TestServer::spawn_with(|state| {
        state.request_timeout = std::time::Duration::from_millis(200);
    })
    .await;
    let res = ts.get("/api/events").await;
    assert_eq!(res.status(), 200);
    let mut stream = res.bytes_stream();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    ts.publish("late", &[("index.html", "<p>")]).await;
    assert!(
        saw_event(&mut stream, "event: version").await,
        "version event must arrive on the still-open stream"
    );
}

/// Reads the stream until `needle` appears, within 2 s.
async fn saw_event(
    stream: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    needle: &str,
) -> bool {
    use futures::StreamExt;
    let mut seen = String::new();
    let read = async {
        while let Some(Ok(chunk)) = stream.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains(needle) {
                return true;
            }
        }
        false
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), read)
        .await
        .unwrap_or(false)
}

#[tokio::test]
async fn write_side_effects_survive_a_handler_timeout() {
    let ts = TestServer::spawn_with(|state| {
        state.request_timeout = std::time::Duration::from_millis(100);
    })
    .await;
    let events = ts.get("/api/events").await;
    let mut stream = events.bytes_stream();
    assert!(saw_event(&mut stream, "event: ready").await);
    let res = ts
        .client
        .post(format!("{}/api/_test/slow_publish/300", ts.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 408);
    assert!(
        saw_event(&mut stream, "event: version").await,
        "version event still published after the 408"
    );
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_publishes_all_succeed() {
    let ts = TestServer::spawn().await;
    let futs: Vec<_> = (0..16)
        .map(|i| {
            let ts = &ts;
            async move { ts.publish(&format!("A{i}"), &[("index.html", "<p>")]).await }
        })
        .collect();
    let all = futures::future::join_all(futs).await;
    assert_eq!(all.len(), 16);
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 16);
}

/// Test builds delay a request by this many milliseconds, inside its route
/// group's timeout.
const DELAY: &str = "x-artifax-test-delay-ms";

#[tokio::test]
async fn publish_routes_run_under_the_publish_timeout() {
    let ts = TestServer::spawn_with(|state| {
        state.request_timeout = std::time::Duration::from_millis(100);
        state.publish_timeout = std::time::Duration::from_secs(2);
    })
    .await;
    // The delay counts against the fast group's timeout.
    let res = ts
        .client
        .get(format!("{}/api/artifacts", ts.base))
        .header(DELAY, "500")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 408);

    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts", ts.base)))
        .header(DELAY, "500")
        .json(&serde_json::json!({
            "title": "slow",
            "files": {"index.html": {"content": "<p>1"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let created: serde_json::Value = res.json().await.unwrap();
    let id = created["artifact"]["id"].as_str().unwrap();

    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/versions", ts.base)),
        )
        .header(DELAY, "500")
        .json(&serde_json::json!({
            "if_version": 1,
            "files": {"index.html": {"content": "<p>2"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let part = reqwest::multipart::Part::bytes(vec![1u8, 2, 3])
        .file_name("a.png")
        .mime_str("image/png")
        .unwrap();
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{id}/assets", ts.base)),
        )
        .header(DELAY, "500")
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
}

#[tokio::test]
async fn mcp_is_outside_the_request_timeout() {
    // Shorter than the delay under both groups' timeouts, so /mcp under either fails.
    let ts = TestServer::spawn_with(|state| {
        state.request_timeout = std::time::Duration::from_millis(100);
        state.publish_timeout = std::time::Duration::from_millis(200);
    })
    .await;
    let res = ts
        .authed(ts.client.post(format!("{}/mcp", ts.base)))
        .header("accept", "application/json, text/event-stream")
        .header(DELAY, "300")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "api-timeout-test", "version": "0"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let text = res.text().await.unwrap();
    assert!(text.contains("\"serverInfo\""), "{text}");
}
