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
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let mut stream = res.bytes_stream();
    use futures::StreamExt;
    assert!(
        stream.next().await.is_some(),
        "stream still open after the API timeout would have fired"
    );
}

#[tokio::test]
async fn store_calls_run_off_the_runtime_worker() {
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
