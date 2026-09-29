mod common;
use common::TestServer;
use futures::StreamExt;
use std::time::Duration;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Appends the next body chunk to `buf`, failing the test if none arrives within
/// `READ_TIMEOUT` or the stream ends.
async fn read_chunk(
    stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    buf: &mut String,
) {
    let chunk = tokio::time::timeout(READ_TIMEOUT, stream.next())
        .await
        .expect("SSE chunk within 5 s")
        .expect("stream still open")
        .unwrap();
    buf.push_str(std::str::from_utf8(&chunk).unwrap());
}

/// Returns the next non-comment event as (name, data).
async fn next_event(
    stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    buf: &mut String,
) -> (String, String) {
    loop {
        if let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf.drain(..end + 2);
            if block.starts_with(':') {
                continue;
            }
            let ev = block
                .lines()
                .find_map(|l| l.strip_prefix("event: "))
                .unwrap_or("message")
                .to_string();
            let data = block
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .unwrap_or("")
                .to_string();
            return (ev, data);
        }
        read_chunk(stream, buf).await;
    }
}

async fn open_stream(
    ts: &TestServer,
    query: &str,
) -> (
    impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin + use<>,
    String,
) {
    let res = ts.get(&format!("/api/events{query}")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut stream = res.bytes_stream();
    let mut buf = String::new();
    let (ev, _) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "ready");
    (stream, buf)
}

#[tokio::test]
async fn publish_emits_version_event_filtered_by_artifact() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let b = ts.publish("B", &[("index.html", "b")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let bid = b["artifact"]["id"].as_str().unwrap().to_string();
    let (mut stream, mut buf) = open_stream(&ts, &format!("?artifact={aid}")).await;
    ts.post_json(
        &format!("/api/artifacts/{bid}/versions"),
        serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "b2"}}}),
    )
    .await;
    ts.post_json(
        &format!("/api/artifacts/{aid}/versions"),
        serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "a2"}}}),
    )
    .await;
    let (ev, data) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "version");
    let v: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(v["artifact_id"], aid);
    assert_eq!(v["n"], 2);
}

#[tokio::test]
async fn unfiltered_stream_carries_every_artifact() {
    let ts = TestServer::spawn().await;
    let (mut stream, mut buf) = open_stream(&ts, "").await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let b = ts.publish("B", &[("index.html", "b")]).await;
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (ev, data) = next_event(&mut stream, &mut buf).await;
        assert_eq!(ev, "version");
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["n"], 1);
        seen.push(v["artifact_id"].as_str().unwrap().to_string());
    }
    let expected: Vec<String> = [a, b]
        .iter()
        .map(|p| p["artifact"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn delete_emits_artifact_deleted() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let (mut stream, mut buf) = open_stream(&ts, &format!("?artifact={aid}")).await;
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let (ev, data) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "artifact_deleted");
    let v: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"type": "artifact_deleted", "artifact_id": aid})
    );
}

#[tokio::test]
async fn idle_stream_sends_keep_alive_comments() {
    let ts = TestServer::spawn_with(|s| s.sse_keep_alive = Duration::from_millis(200)).await;
    let (mut stream, mut buf) = open_stream(&ts, "").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !buf.contains(": keep-alive\n\n") {
            read_chunk(&mut stream, &mut buf).await;
        }
    })
    .await
    .expect("keep-alive comment within 2 s");
}

#[tokio::test]
async fn lagged_subscriber_receives_resync_with_dropped_count() {
    let ts = TestServer::spawn().await;
    let (mut stream, mut buf) = open_stream(&ts, "").await;
    // Current-thread runtime: the server cannot drain the receiver while this loop
    // runs, so the sends beyond the bus capacity overflow it.
    let capacity = artifax_core::EVENT_BUS_CAPACITY as u32;
    let sent = capacity + 44;
    for n in 0..sent {
        ts.events.publish(artifax_core::Event::Version {
            artifact_id: "7q3k9mzx2b4t".into(),
            n,
            by_page: false,
        });
    }
    let dropped = sent - capacity;
    let (ev, data) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "resync");
    let v: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(v, serde_json::json!({ "dropped": dropped }));
    let (ev, data) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "version");
    let v: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(
        v["n"], dropped,
        "stream resumes at the oldest retained event"
    );
}

#[tokio::test]
async fn events_are_not_served_on_an_artifact_origin() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let res = ts
        .client
        .get(format!("{}/api/events", ts.base))
        .header("host", format!("{aid}.localhost"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_found");
}
