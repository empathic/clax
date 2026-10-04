mod common;
use common::TestServer;
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;

/// One SSE event: name, data, and the `id:` field when it has one.
#[derive(Debug)]
struct Ev {
    name: String,
    data: Value,
    id: Option<String>,
}

struct Reader {
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    buf: String,
}

impl Reader {
    async fn next(&mut self) -> Ev {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block = self.buf[..end].to_string();
                self.buf.drain(..end + 2);
                if block.starts_with(':') {
                    continue;
                }
                let field = |k: &str| {
                    block
                        .lines()
                        .find_map(|l| l.strip_prefix(k))
                        .map(str::to_string)
                };
                return Ev {
                    name: field("event: ").unwrap_or_else(|| "message".into()),
                    data: serde_json::from_str(&field("data: ").unwrap_or("null".into())).unwrap(),
                    id: field("id: "),
                };
            }
            let chunk = tokio::time::timeout(Duration::from_secs(20), self.stream.next())
                .await
                .expect("SSE chunk within 20 s")
                .expect("stream still open")
                .unwrap();
            self.buf.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    }

    /// Whether another event arrives within `ms`.
    async fn quiet_for(&mut self, ms: u64) -> bool {
        tokio::time::timeout(Duration::from_millis(ms), self.next())
            .await
            .is_err()
    }
}

/// Opens `/api/stream` with `build` shaping the request; returns the reader
/// and the `ready` data.
async fn open_with(
    ts: &TestServer,
    build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> (Reader, Value) {
    let res = build(ts.client.get(format!("{}/api/stream", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut r = Reader {
        stream: Box::pin(res.bytes_stream()),
        buf: String::new(),
    };
    let ready = r.next().await;
    assert_eq!(ready.name, "ready");
    assert!(ready.id.is_none());
    (r, ready.data)
}

async fn open(ts: &TestServer) -> (Reader, String) {
    let (r, ready) = open_with(ts, |b| b).await;
    (r, ready["stream"].as_str().unwrap().to_string())
}

async fn update(
    ts: &TestServer,
    sid: &str,
    body: Value,
    build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> (u16, Value) {
    let res = build(
        ts.client
            .post(format!("{}/api/stream/{sid}", ts.base))
            .json(&body),
    )
    .send()
    .await
    .unwrap();
    (
        res.status().as_u16(),
        res.json().await.unwrap_or(Value::Null),
    )
}

async fn subscribe(ts: &TestServer, sid: &str, topics: &[&str]) -> Value {
    let (s, v) = update(ts, sid, json!({"subscribe": topics}), |b| b).await;
    assert_eq!(s, 200, "{v}");
    v
}

fn aid_of(p: &Value) -> String {
    p["artifact"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn one_stream_carries_each_subscribed_topic_as_small_deltas() {
    let ts = TestServer::spawn().await;
    let a = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let b = aid_of(&ts.publish("B", &[("index.html", "b")]).await);
    let (mut r, sid) = open(&ts).await;
    let v = subscribe(&ts, &sid, &["gallery", &format!("artifact:{a}")]).await;
    assert_eq!(v["topics"], json!(["gallery", format!("artifact:{a}")]));
    // A version on B reaches the gallery only; one on A reaches both topics.
    ts.post_json(
        &format!("/api/artifacts/{b}/versions"),
        json!({"if_version": 1, "title": "B2", "files": {"index.html": {"content": "b2"}}}),
    )
    .await;
    let e = r.next().await;
    assert_eq!(e.name, "version");
    assert_eq!(
        (
            e.data["topic"].as_str(),
            e.data["artifact_id"].as_str(),
            e.data["n"].as_u64(),
            e.data["title"].as_str()
        ),
        (Some("gallery"), Some(b.as_str()), Some(2), Some("B2"))
    );
    let id = e.id.expect("events carry an id");
    assert!(id.starts_with(&format!("{sid}:")), "{id}");
    let t = ts.thread(&a, 1, "please fix the heading").await;
    let mut got = [r.next().await, r.next().await];
    got.sort_by_key(|e| e.data["topic"].as_str().unwrap().to_string());
    let (art, gal) = (&got[0], &got[1]);
    assert_eq!((art.name.as_str(), gal.name.as_str()), ("thread", "thread"));
    assert_eq!(art.id, gal.id, "one write, one sequence number");
    assert_eq!(art.data["thread"]["id"], t["id"]);
    assert_eq!(art.data["thread"]["comment_count"], 1);
    assert_eq!(
        art.data["thread"]["last_comment"]["body"],
        "please fix the heading"
    );
    assert!(art.data["thread"].get("comments").is_none());
    assert_eq!(gal.data["thread_id"], t["id"]);
    assert_eq!(gal.data["comments"], 1);
    assert!(
        !gal.data.to_string().contains("please fix"),
        "gallery topics carry no thread bodies: {}",
        gal.data
    );
    // Unsubscribed: nothing more from the artifact topic.
    let (s, v) = update(
        &ts,
        &sid,
        json!({"unsubscribe": [format!("artifact:{a}")]}),
        |b| b,
    )
    .await;
    assert_eq!((s, v["topics"].clone()), (200, json!(["gallery"])));
    ts.events.publish(clax_core::Event::FeedbackState {
        artifact_id: a.clone(),
        thread_id: "t".into(),
        state: clax_core::FeedbackPhase::Sent,
        tier: None,
        since: "s".into(),
        resends: 0,
        exhausted: false,
    });
    assert!(r.quiet_for(300).await);
}

#[tokio::test]
async fn presence_and_working_topics_carry_their_lists() {
    let ts = TestServer::spawn().await;
    let a = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let v = ts.viewer(Some("Mia")).await;
    let (mut r, sid) = open(&ts).await;
    subscribe(
        &ts,
        &sid,
        &[&format!("presence:{a}"), &format!("working:{a}")],
    )
    .await;
    let res = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"artifact_id": a, "state": "here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let e = r.next().await;
    assert_eq!(e.name, "presence");
    assert_eq!(e.data["people"][0]["public_id"], v.public_id.as_str());
    assert_eq!(e.data["gone"], json!([]));
    assert!(!e.data.to_string().contains(&v.cookie), "never the cookie");
    let sess = ts.register_session("claude", "h1").await;
    let sid_s = sess["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid_s}/working/{a}", ts.base)),
        )
        .json(&json!({"message": "tidying"}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let e = r.next().await;
    assert_eq!(e.name, "working");
    assert_eq!(e.data["working"][0]["message"], "tidying");
    assert!(!e.data.to_string().contains(sid_s), "never a session ID");
}

#[tokio::test]
async fn subscriptions_are_checked_once_at_subscribe_time() {
    let ts = TestServer::spawn().await;
    let plain = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let (_r, sid) = open(&ts).await;
    let (s, v) = update(
        &ts,
        &sid,
        json!({"subscribe": ["artifact:zzzzzzzzzzzz"]}),
        |b| b,
    )
    .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = update(&ts, &sid, json!({"subscribe": ["nope"]}), |b| b).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_topic"))
    );
    let (s, v) = update(
        &ts,
        &sid,
        json!({"subscribe": [format!("docs:{plain}")]}),
        |b| b,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_declared"))
    );
    // Another caller cannot change this stream, and an unknown stream is 404.
    let other = ts.viewer(None).await;
    let (s, v) = update(&ts, &sid, json!({"subscribe": ["gallery"]}), |b| {
        b.header("cookie", format!("clax_viewer={}", other.cookie))
    })
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("unknown_stream"))
    );
    let (s, _) = update(
        &ts,
        &"0".repeat(32),
        json!({"subscribe": ["gallery"]}),
        |b| b,
    )
    .await;
    assert_eq!(s, 404);
    // A page on an artifact origin cannot drive it.
    let (s, v) = update(&ts, &sid, json!({"subscribe": ["gallery"]}), |b| {
        b.header("origin", format!("http://{plain}.localhost:1"))
    })
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden_origin"))
    );
    let many: Vec<String> = (0..257).map(|_| "gallery".to_string()).collect();
    let (s, _) = update(&ts, &sid, json!({"subscribe": many}), |b| b).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn the_events_cookie_stands_for_the_token_on_the_stream_and_its_subscriptions() {
    let ts = TestServer::spawn().await;
    let plain = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let shell = ts
        .client
        .get(format!("{}/api/token", ts.base))
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    let cookie = shell
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|c| c.contains("Path=/api/stream"))
        .expect("a cookie for /api/stream");
    let pair = cookie.split(';').next().unwrap().to_string();
    let with = |p: String| move |b: reqwest::RequestBuilder| b.header("cookie", p);
    // The token's level: a `docs` topic of an artifact that declares no `db`.
    let (_r, ready) = open_with(&ts, with(pair.clone())).await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let docs = json!({"subscribe": [format!("docs:{plain}")]});
    let (s, v) = update(&ts, &sid, docs.clone(), with(pair.clone())).await;
    assert_eq!(s, 200, "{v}");
    // A wrong value is no token, and is another caller than the stream's.
    let wrong = format!("{pair}0");
    let (_r2, ready) = open_with(&ts, with(wrong.clone())).await;
    let sid2 = ready["stream"].as_str().unwrap().to_string();
    let (s, v) = update(&ts, &sid2, docs.clone(), with(wrong.clone())).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("not_declared"))
    );
    let (s, v) = update(&ts, &sid, json!({"subscribe": ["gallery"]}), with(wrong)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("unknown_stream"))
    );
}

#[tokio::test]
async fn doc_events_follow_the_level_fixed_when_the_stream_opened() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "T", "capabilities": {"db": {}}, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    let aid = aid_of(&res.json().await.unwrap());
    let a = ts.viewer(Some("A")).await;
    let b = ts.viewer(Some("B")).await;
    let as_viewer = |c: String| {
        move |r: reqwest::RequestBuilder| r.header("cookie", format!("clax_viewer={c}"))
    };
    let (mut ra, ready) = open_with(&ts, as_viewer(a.cookie.clone())).await;
    let sa = ready["stream"].as_str().unwrap().to_string();
    let (mut rb, ready) = open_with(&ts, as_viewer(b.cookie.clone())).await;
    let sb = ready["stream"].as_str().unwrap().to_string();
    let topic = [format!("docs:{aid}")];
    assert_eq!(
        update(
            &ts,
            &sa,
            json!({"subscribe": topic}),
            as_viewer(a.cookie.clone())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        update(
            &ts,
            &sb,
            json!({"subscribe": topic}),
            as_viewer(b.cookie.clone())
        )
        .await
        .0,
        200
    );
    let private = format!("data/users/{}/pick", a.public_id);
    for path in [private.as_str(), "shared/s"] {
        let res = ts
            .client
            .put(format!("{}/api/artifacts/{aid}/docs/{path}", ts.base))
            .header("cookie", format!("clax_viewer={}", a.cookie))
            .json(&json!({"data": {"n": 1}, "lww": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
    let e = ra.next().await;
    assert_eq!(
        (e.name.as_str(), e.data["path"].as_str()),
        ("doc", Some(private.as_str()))
    );
    assert!(e.data.get("data").is_none(), "never a body");
    assert_eq!(ra.next().await.data["path"], "shared/s");
    assert_eq!(
        rb.next().await.data["path"],
        "shared/s",
        "B never sees A's private path"
    );
    assert!(rb.quiet_for(200).await);
}

#[tokio::test]
async fn a_reconnect_with_last_event_id_resumes_the_same_stream() {
    let ts = TestServer::spawn().await;
    let a = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let (mut r, sid) = open(&ts).await;
    subscribe(&ts, &sid, &[&format!("artifact:{a}")]).await;
    let url = format!("/api/artifacts/{a}/versions");
    let publish = |n: u32| {
        ts.post_json(
            &url,
            json!({"if_version": n - 1, "files": {"index.html": {"content": format!("v{n}")}}}),
        )
    };
    assert_eq!(publish(2).await.status(), 201);
    let last = r.next().await.id.unwrap();
    drop(r);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(publish(3).await.status(), 201);
    let (mut r, ready) = open_with(&ts, |b| b.header("last-event-id", &last)).await;
    assert_eq!(ready["stream"], sid.as_str());
    assert_eq!(ready["resumed"], true);
    assert_eq!(ready["topics"], json!([format!("artifact:{a}")]));
    let e = r.next().await;
    assert_eq!(
        (e.name.as_str(), e.data["n"].as_u64()),
        ("version", Some(3)),
        "the missed event"
    );
    assert_eq!(publish(4).await.status(), 201);
    assert_eq!(r.next().await.data["n"], 4, "and live events after it");
    // Another caller naming the stream gets a fresh one.
    let other = ts.viewer(None).await;
    let (_r2, ready) = open_with(&ts, |b| {
        b.header("last-event-id", &last)
            .header("cookie", format!("clax_viewer={}", other.cookie))
    })
    .await;
    assert_ne!(ready["stream"], sid.as_str());
    assert_eq!(ready["resumed"], false);
}

#[tokio::test]
async fn a_client_that_falls_behind_gets_resync_and_never_a_growing_backlog() {
    let ts = TestServer::spawn().await;
    let (mut r, sid) = open(&ts).await;
    subscribe(&ts, &sid, &["gallery"]).await;
    // Current-thread runtime: the connection cannot drain while this loop
    // runs, so its queue fills.
    let sent = clax_server::stream::QUEUE as u32 * 4;
    for n in 0..sent {
        ts.events.publish(clax_core::Event::Version {
            artifact_id: "7q3k9mzx2b4t".into(),
            n,
            by_page: false,
            title: None,
            at: None,
        });
    }
    let e = r.next().await;
    assert_eq!(e.name, "resync");
    assert_eq!(e.data, json!({"topic": "gallery", "reason": "behind"}));
    assert!(e.id.is_none());
    // The backlog was dropped: the next event is a new one.
    ts.events.publish(clax_core::Event::Version {
        artifact_id: "7q3k9mzx2b4t".into(),
        n: 9999,
        by_page: false,
        title: None,
        at: None,
    });
    assert_eq!(r.next().await.data["n"], 9999);
}

#[tokio::test]
async fn the_old_event_stream_still_works_alongside() {
    let ts = TestServer::spawn().await;
    let (mut r, sid) = open(&ts).await;
    subscribe(&ts, &sid, &["gallery"]).await;
    let mut old = ts.events("").await;
    let a = aid_of(&ts.publish("A", &[("index.html", "a")]).await);
    let (name, data) = old.next().await;
    assert_eq!(
        (name.as_str(), data["artifact_id"].as_str()),
        ("version", Some(a.as_str()))
    );
    assert!(data.get("title").is_none(), "/api/events keeps its shape");
    assert_eq!(r.next().await.data["artifact_id"], a.as_str());
}
