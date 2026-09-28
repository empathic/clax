mod common;
use common::TestServer;
use futures::StreamExt;

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
        let chunk = stream.next().await.unwrap().unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

#[tokio::test]
async fn publish_emits_version_event_filtered_by_artifact() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let b = ts.publish("B", &[("index.html", "b")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let bid = b["artifact"]["id"].as_str().unwrap().to_string();
    let res = ts.get(&format!("/api/events?artifact={aid}")).await;
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut stream = res.bytes_stream();
    let mut buf = String::new();
    let (ev, _) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "ready");
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
