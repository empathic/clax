mod common;
use common::TestServer;
use futures::StreamExt;
use std::time::Duration;

const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Appends the next body chunk to `buf`, failing the test if none arrives within
/// `READ_TIMEOUT` or the stream ends.
async fn read_chunk(
    stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    buf: &mut String,
) {
    let chunk = tokio::time::timeout(READ_TIMEOUT, stream.next())
        .await
        .expect("SSE chunk within 20 s")
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
    tokio::time::timeout(Duration::from_secs(20), async {
        while !buf.contains(": keep-alive\n\n") {
            read_chunk(&mut stream, &mut buf).await;
        }
    })
    .await
    .expect("keep-alive comment within 20 s");
}

#[tokio::test]
async fn lagged_subscriber_receives_resync_with_dropped_count() {
    let ts = TestServer::spawn().await;
    let (mut stream, mut buf) = open_stream(&ts, "").await;
    // Current-thread runtime: the server cannot drain the receiver while this loop
    // runs, so the sends beyond the bus capacity overflow it.
    let capacity = clax_core::EVENT_BUS_CAPACITY as u32;
    let sent = capacity + 44;
    for n in 0..sent {
        ts.events.publish(clax_core::Event::Version {
            artifact_id: "7q3k9mzx2b4t".into(),
            n,
            by_page: false,
            title: None,
            at: None,
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

/// One SSE block of a raw stream: (id, name, data), keep-alives skipped.
async fn next_block(
    stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    buf: &mut String,
) -> (Option<String>, String, String) {
    loop {
        if let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf.drain(..end + 2);
            if block.starts_with(':') {
                continue;
            }
            let field = |k: &str| {
                block
                    .lines()
                    .find_map(|l| l.strip_prefix(k))
                    .map(str::to_string)
            };
            return (
                field("id: "),
                field("event: ").unwrap_or_else(|| "message".into()),
                field("data: ").unwrap_or_default(),
            );
        }
        read_chunk(stream, buf).await;
    }
}

async fn raw_stream(
    ts: &TestServer,
    query: &str,
    last_event_id: Option<&str>,
) -> (
    impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin + use<>,
    String,
) {
    let mut req = ts.client.get(format!("{}/api/events{query}", ts.base));
    if let Some(id) = last_event_id {
        req = req.header("last-event-id", id);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    (res.bytes_stream(), String::new())
}

async fn new_version(ts: &TestServer, aid: &str, n: u32) {
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            serde_json::json!({"if_version": n - 1, "files": {"index.html": {"content": format!("v{n}")}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn a_stream_resumes_after_its_last_event_id_without_a_refetch() {
    let ts = TestServer::spawn().await;
    let aid = ts.publish("A", &[("index.html", "a")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let q = format!("?artifact={aid}&types=version");
    let (mut s, mut buf) = raw_stream(&ts, &q, None).await;
    let (ready_id, name, data) = next_block(&mut s, &mut buf).await;
    assert_eq!(name, "ready");
    assert_eq!(data, r#"{"resumed":false}"#);
    let ready_id = ready_id.expect("ready carries an ID");
    new_version(&ts, &aid, 2).await;
    let (first, name, _) = next_block(&mut s, &mut buf).await;
    assert_eq!(name, "version");
    let first = first.expect("events carry IDs");
    let (epoch, _) = first.split_once('-').unwrap();
    assert_eq!(ready_id.split_once('-').unwrap().0, epoch);
    drop(s);
    // Missed while away: version 3, and a thread the filters drop.
    ts.thread(&aid, 2, "missed").await;
    new_version(&ts, &aid, 3).await;

    // From the header (EventSource's own reconnect).
    let (mut s, mut buf) = raw_stream(&ts, &q, Some(&first)).await;
    let (id, name, data) = next_block(&mut s, &mut buf).await;
    assert_eq!(
        (name.as_str(), data.as_str()),
        ("ready", r#"{"resumed":true}"#)
    );
    assert_eq!(id.as_deref(), Some(first.as_str()));
    let (third, name, data) = next_block(&mut s, &mut buf).await;
    assert_eq!(name, "version");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&data).unwrap()["n"],
        3
    );
    drop(s);

    // From the query (a client reopening with other topics): caught up, nothing replayed.
    let third = third.unwrap();
    let (mut s, mut buf) = raw_stream(&ts, &format!("{q}&last_event_id={third}"), None).await;
    let (_, name, data) = next_block(&mut s, &mut buf).await;
    assert_eq!(
        (name.as_str(), data.as_str()),
        ("ready", r#"{"resumed":true}"#)
    );
    new_version(&ts, &aid, 4).await;
    let (_, _, data) = next_block(&mut s, &mut buf).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&data).unwrap()["n"],
        4
    );
    drop(s);

    // Another daemon run's ID, or a malformed one, resumes nothing.
    for bad in ["0000000000000000-1", "garbage"] {
        let (mut s, mut buf) = raw_stream(&ts, &q, Some(bad)).await;
        let (_, name, data) = next_block(&mut s, &mut buf).await;
        assert_eq!(
            (name.as_str(), data.as_str()),
            ("ready", r#"{"resumed":false}"#),
            "{bad}"
        );
        new_version(&ts, &aid, if bad == "garbage" { 6 } else { 5 }).await;
        let (_, _, data) = next_block(&mut s, &mut buf).await;
        let n = serde_json::from_str::<serde_json::Value>(&data).unwrap()["n"].clone();
        assert_eq!(
            n,
            if bad == "garbage" { 6 } else { 5 },
            "only live events after a refused resume"
        );
    }
}

#[tokio::test]
async fn the_artifact_filter_takes_a_comma_list() {
    let ts = TestServer::spawn().await;
    let mut ids = Vec::new();
    for t in ["A", "B", "C"] {
        ids.push(
            ts.publish(t, &[("index.html", "x")]).await["artifact"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let (mut s, mut buf) = open_stream(
        &ts,
        &format!("?artifact={},{}&types=version", ids[0], ids[2]),
    )
    .await;
    for id in &ids {
        new_version(&ts, id, 2).await;
    }
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (_, data) = next_event(&mut s, &mut buf).await;
        seen.push(
            serde_json::from_str::<serde_json::Value>(&data).unwrap()["artifact_id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(seen, [ids[0].clone(), ids[2].clone()]);
}

#[tokio::test]
async fn the_token_route_sets_the_events_cookie_for_the_shell_only() {
    let ts = TestServer::spawn().await;
    let port = ts.base.rsplit(':').next().unwrap().to_string();
    let plain = ts.get("/api/token").await;
    assert_eq!(plain.status(), 200);
    assert!(
        plain.headers().get("set-cookie").is_none(),
        "a script gets no cookie"
    );
    let shell = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    let sets: Vec<String> = shell
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(sets.len(), 2, "{sets:?}");
    for (set, path) in sets.iter().zip(["/api/events", "/api/stream"]) {
        assert!(set.starts_with(&format!("clax_events_{port}=")), "{set}");
        assert!(
            set.contains(&format!("; Path={path}; HttpOnly; SameSite=Strict")),
            "{set}"
        );
        assert!(!set.contains(&ts.token), "the cookie never holds the token");
    }
    for site in ["cross-site", "same-site"] {
        let other = ts
            .client
            .get(format!("{}/api/token", ts.base))
            .header("sec-fetch-site", site)
            .send()
            .await
            .unwrap();
        assert!(other.headers().get("set-cookie").is_none(), "{site}");
    }
}
