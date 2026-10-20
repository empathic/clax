//! The embedded core answering request frames through the router, by who
//! each frame speaks for (spec 2026-10-19 §2.3, §2.5).

use clax_embed::frame::{self, As, RequestHead, ResponseHead};
use clax_embed::{Embed, OpenError, OpenEvent, OpenOptions};
use serde_json::{Value, json};
use std::sync::mpsc;

fn options(dir: &std::path::Path) -> OpenOptions {
    OpenOptions {
        home: dir.join("home"),
        origins: clax_http::state::EmbedOrigins {
            shell: "clax://app".into(),
            content: "clax://{id}.c".into(),
        },
        log: None,
    }
}

/// Opens a core on `dir`'s home, with every open event sent to the receiver.
fn start(dir: &std::path::Path) -> (Embed, mpsc::Receiver<OpenEvent>) {
    let (tx, rx) = mpsc::channel();
    let embed = Embed::open(
        options(dir),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .expect("the home opens");
    (embed, rx)
}

/// The next event that is not an `Opening`.
fn settled(rx: &mpsc::Receiver<OpenEvent>) -> OpenEvent {
    loop {
        match rx.recv().expect("an open event") {
            OpenEvent::Opening { .. } => continue,
            e => return e,
        }
    }
}

/// Opens a core on `dir`'s home and waits until it is ready.
fn open(dir: &std::path::Path) -> (Embed, mpsc::Receiver<OpenEvent>) {
    let (embed, rx) = start(dir);
    assert_eq!(settled(&rx), OpenEvent::Ready);
    (embed, rx)
}

fn head(method: &str, path: &str, as_: As) -> RequestHead {
    RequestHead {
        method: method.into(),
        path: path.into(),
        headers: vec![],
        as_,
        host: None,
    }
}

/// Splits a response frame.
fn decode_response(bytes: &[u8]) -> (ResponseHead, Vec<u8>) {
    let (h, body) = frame::split(bytes).expect("a response frame");
    (serde_json::from_slice(h).unwrap(), body.to_vec())
}

async fn call(embed: &Embed, head: RequestHead, body: &[u8]) -> (ResponseHead, Vec<u8>) {
    let res = embed
        .call(&frame::encode(&head, body))
        .await
        .expect("a frame");
    decode_response(&res)
}

async fn call_json(embed: &Embed, head: RequestHead, body: Option<Value>) -> (u16, Value) {
    let mut head = head;
    let bytes = match body {
        Some(b) => {
            head.headers
                .push(("content-type".into(), "application/json".into()));
            serde_json::to_vec(&b).unwrap()
        }
        None => vec![],
    };
    let (h, body) = call(embed, head, &bytes).await;
    let v = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (h.status, v)
}

fn header<'a>(h: &'a ResponseHead, name: &str) -> Option<&'a str> {
    h.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Publishes an app as the owner (`session`: the agent session it is
/// attributed to); returns its ID.
async fn publish(embed: &Embed, html: &str, session: Option<&str>) -> String {
    let mut h = head("POST", "/api/artifacts", As::Owner);
    if let Some(s) = session {
        h.headers.push(("x-clax-session".into(), s.into()));
    }
    let (status, v) = call_json(
        embed,
        h,
        Some(json!({"title": "t", "files": {"index.html": {"content": html, "encoding": "utf8"}}})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    v["artifact"]["id"].as_str().unwrap().to_string()
}

fn not_in_app(status: u16, v: &Value) {
    assert_eq!(status, 404, "{v}");
    assert_eq!(v["error"]["code"], "not_in_app", "{v}");
}

#[tokio::test]
async fn an_owner_call_lists_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(&embed, head("GET", "/api/artifacts", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["artifacts"], json!([]), "{v}");
    embed.close();
}

#[tokio::test]
async fn an_owner_call_creates_and_a_content_frame_reads_its_page() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let id = publish(&embed, "<title>t</title>hi", None).await;
    let mut h = head("GET", "/v/1/", As::Content);
    h.host = Some(id.clone());
    let (res, body) = call(&embed, h, b"").await;
    assert_eq!(res.status, 200);
    assert!(String::from_utf8_lossy(&body).contains("hi"));
    let csp = header(&res, "content-security-policy").expect("a content policy");
    assert!(csp.contains("frame-ancestors clax://app"), "{csp}");
    embed.close();
}

#[tokio::test]
async fn a_content_frame_reaches_no_api() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let id = publish(&embed, "<title>t</title>hi", None).await;
    let mut h = head("GET", "/api/artifacts", As::Content);
    h.host = Some(id);
    let (status, v) = call_json(&embed, h, None).await;
    assert_eq!(status, 404, "{v}");
    let mut h = head("POST", "/v/1/", As::Content);
    h.host = Some("7q3k9mzx2b4t".into());
    let (status, v) = call_json(&embed, h, None).await;
    not_in_app(status, &v);
    let mut h = head("GET", "/v/1/", As::Content);
    h.host = Some("not-an-id".into());
    let (status, v) = call_json(&embed, h, None).await;
    not_in_app(status, &v);
    embed.close();
}

#[tokio::test]
async fn a_shell_page_frame_never_reaches_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(&embed, head("GET", "/api/artifacts", As::ShellPage), None).await;
    not_in_app(status, &v);
    let (status, v) = call_json(&embed, head("POST", "/a/x", As::ShellPage), None).await;
    not_in_app(status, &v);
    embed.close();
}

#[tokio::test]
async fn the_event_stream_is_not_a_call() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(&embed, head("GET", "/api/stream", As::Owner), None).await;
    not_in_app(status, &v);
    let (status, v) = call_json(&embed, head("POST", "/api/admin/shutdown", As::Owner), None).await;
    not_in_app(status, &v);
    embed.close();
}

#[tokio::test]
async fn a_page_frame_carrying_a_cookie_gets_no_credential() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let mut h = head("GET", "/api/token", As::ShellPage);
    h.headers.push(("cookie".into(), "clax_owner=x".into()));
    h.headers
        .push(("authorization".into(), "Bearer forged".into()));
    let (status, v) = call_json(&embed, h, None).await;
    not_in_app(status, &v);
    let (status, v) = call_json(&embed, head("GET", "/api/token", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    embed.close();
}

/// A multipart body creating a thread on version 1, as a browser sends it.
fn thread_form(body: &str) -> (String, Vec<u8>) {
    let boundary = "clax-test-boundary";
    let anchor = json!({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null});
    let mut out = String::new();
    for (name, value) in [
        ("anchor", anchor.to_string()),
        ("body", body.to_string()),
        ("version", "1".to_string()),
    ] {
        out.push_str(&format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    (
        format!("multipart/form-data; boundary={boundary}"),
        out.into_bytes(),
    )
}

#[tokio::test]
async fn a_long_poll_answers_when_feedback_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(
        &embed,
        head("POST", "/api/sessions", As::Owner),
        Some(json!({"harness": "app", "harness_session_id": "install-1", "cwd": "/w"})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let sid = v["session"]["id"].as_str().unwrap().to_string();
    let aid = publish(&embed, "<title>t</title>runs", Some(&sid)).await;

    // The poll waits far longer than the test takes: it answers when the
    // feedback arrives, not when its wait ends.
    let poll = {
        let embed = embed.clone();
        let path = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
        tokio::spawn(async move { call_json(&embed, head("GET", &path, As::Owner), None).await })
    };
    let (ct, form) = thread_form("make it blue");
    let mut h = head("POST", &format!("/api/artifacts/{aid}/threads"), As::Owner);
    h.headers.push(("content-type".into(), ct));
    let (res, body) = call(&embed, h, &form).await;
    assert_eq!(res.status, 201, "{}", String::from_utf8_lossy(&body));
    let t: Value = serde_json::from_slice(&body).unwrap();
    let tid = t["thread"]["id"].as_str().unwrap();
    let (status, v) = call_json(
        &embed,
        head(
            "POST",
            &format!("/api/artifacts/{aid}/threads/{tid}/send"),
            As::Owner,
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = poll.await.unwrap();
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["feedback"][0]["body"], "make it blue", "{v}");
    embed.close();
}

#[tokio::test]
async fn calls_made_before_ready_wait_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, rx) = start(dir.path());
    let (status, v) = call_json(&embed, head("GET", "/api/artifacts", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(settled(&rx), OpenEvent::Ready);
    embed.close();
}

#[test]
fn a_second_open_of_one_home_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, rx) = open(dir.path());
    let again = Embed::open(options(dir.path()), Box::new(|_| {}));
    assert_eq!(again.err(), Some(OpenError::HomeInUse));
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[test]
fn dropping_an_embed_without_close_frees_its_home() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, dropped) = mpsc::channel();
    let guard = Dropped(tx);
    let (events, rx) = mpsc::channel();
    let embed = Embed::open(
        options(dir.path()),
        Box::new(move |e| {
            let _keep = &guard;
            let _ = events.send(e);
        }),
    )
    .expect("the home opens");
    assert_eq!(settled(&rx), OpenEvent::Ready);
    drop(embed);
    // The core's last share, which holds `on_event`, is gone.
    dropped.recv().unwrap();
    let (again, rx) = start(dir.path());
    assert_eq!(settled(&rx), OpenEvent::Ready);
    again.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[tokio::test]
async fn close_posts_closed_ends_calls_and_frees_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, rx) = open(dir.path());
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
    let (status, v) = call_json(&embed, head("GET", "/api/artifacts", As::Owner), None).await;
    assert_eq!(status, 503, "{v}");
    assert_eq!(v["error"]["code"], "core_closed", "{v}");
    drop(embed);
    let (again, rx) = open(dir.path());
    again.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[tokio::test]
async fn an_old_daemon_json_is_retired() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("daemon.json"), "{}").unwrap();
    let (embed, rx) = open(dir.path());
    assert!(!home.join("daemon.json").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("daemon.json.retired")).unwrap(),
        "{}"
    );
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused() {
    let body = || axum::body::Body::from(vec![7u8; 9]);
    assert!(clax_embed::read_limited(body(), 8).await.is_err());
    assert_eq!(clax_embed::read_limited(body(), 9).await.unwrap().len(), 9);
}

#[tokio::test]
async fn a_frame_that_does_not_parse_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    assert_eq!(
        embed.call(b"CLX2").await.unwrap_err(),
        frame::FrameError::Magic
    );
    embed.close();
}

/// Subscribes to the core's events, sending each block's text to the
/// receiver.
fn subscribe(embed: &Embed) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    embed.subscribe(Box::new(move |b| {
        tx.send(String::from_utf8_lossy(&b).into_owned()).is_ok()
    }));
    rx
}

#[tokio::test]
async fn a_subscription_hears_ready_then_a_version() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let blocks = subscribe(&embed);
    let ready = blocks.recv().unwrap();
    assert!(ready.starts_with("event: ready\n"), "{ready}");
    let data: Value = serde_json::from_str(
        ready
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    let stream = data["stream"].as_str().unwrap();
    let (status, v) = call_json(
        &embed,
        head("POST", &format!("/api/stream/{stream}"), As::Owner),
        Some(json!({"subscribe": ["gallery"]})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let id = publish(&embed, "<title>t</title>hi", None).await;
    loop {
        let b = blocks.recv().unwrap();
        if b.contains("event: ") && b.contains(&id) {
            break;
        }
    }
    embed.close();
}

/// Sends on its channel when dropped.
struct Dropped(mpsc::Sender<()>);

impl Drop for Dropped {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[tokio::test]
async fn a_subscriber_that_returns_false_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (tx, dropped) = mpsc::channel();
    let guard = Dropped(tx);
    let (heard_tx, heard) = mpsc::channel();
    embed.subscribe(Box::new(move |b| {
        let _keep = &guard;
        let _ = heard_tx.send(b);
        false
    }));
    assert!(String::from_utf8_lossy(&heard.recv().unwrap()).starts_with("event: ready"));
    dropped.recv().unwrap();
    assert!(
        heard.try_recv().is_err(),
        "nothing after the subscriber said no"
    );
    embed.close();
}

/// The `end` block a subscription's last message is, for `reason`.
fn end(reason: &str) -> String {
    format!("event: end\ndata: {{\"reason\":\"{reason}\"}}\n\n")
}

#[tokio::test]
async fn a_closed_core_ends_its_subscriptions_with_an_end_block() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, rx) = open(dir.path());
    let (tx, dropped) = mpsc::channel();
    let guard = Dropped(tx);
    let (heard_tx, heard) = mpsc::channel();
    embed.subscribe(Box::new(move |b| {
        let _keep = &guard;
        let _ = heard_tx.send(String::from_utf8_lossy(&b).into_owned());
        true
    }));
    assert!(heard.recv().unwrap().starts_with("event: ready\n"));
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
    dropped.recv().unwrap();
    let last = heard.try_iter().last().expect("an end block");
    assert_eq!(last, end("core_closed"));
}

#[test]
fn a_subscription_to_a_core_that_did_not_open_hears_only_its_end() {
    let dir = tempfile::tempdir().unwrap();
    // The home is a file, so the core cannot open it.
    std::fs::write(dir.path().join("home"), "").unwrap();
    let (embed, rx) = start(dir.path());
    let failed = settled(&rx);
    assert!(
        matches!(&failed, OpenEvent::Failed { code, .. } if code == "open_failed"),
        "{failed:?}"
    );
    let blocks = subscribe(&embed);
    assert_eq!(blocks.recv().unwrap(), end("core_failed"));
    assert!(blocks.recv().is_err(), "nothing after the end");
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[tokio::test]
async fn a_subscriber_that_returns_false_hears_no_end() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, rx) = open(dir.path());
    let (tx, dropped) = mpsc::channel();
    let guard = Dropped(tx);
    let (heard_tx, heard) = mpsc::channel();
    embed.subscribe(Box::new(move |b| {
        let _keep = &guard;
        let _ = heard_tx.send(b);
        false
    }));
    dropped.recv().unwrap();
    assert_eq!(heard.try_iter().count(), 1, "the ready block only");
    embed.close();
    assert_eq!(settled(&rx), OpenEvent::Closed);
}

#[tokio::test]
async fn trimming_answers_on_an_embedded_core() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(&embed, head("POST", "/api/core/trim", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    assert!(v["trimmed"]["wal_frames"].is_i64(), "{v}");
    assert_eq!(
        v["trimmed"]["checkpointed"], v["trimmed"]["wal_frames"],
        "{v}"
    );
    embed.close();
}

#[tokio::test]
async fn a_key_put_reaches_the_sampler_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (embed, _rx) = open(dir.path());
    let (status, v) = call_json(&embed, head("GET", "/api/sample", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["available"], false, "{v}");
    let (status, v) = call_json(
        &embed,
        head("PUT", "/api/sample/key", As::Owner),
        Some(json!({"key": "sk-test"})),
    )
    .await;
    assert_eq!(status, 204, "{v}");
    let (status, v) = call_json(&embed, head("GET", "/api/sample", As::Owner), None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["available"], true, "{v}");
    assert_eq!(v["provider"], "anthropic", "{v}");
    embed.close();
}
