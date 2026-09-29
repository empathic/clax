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
