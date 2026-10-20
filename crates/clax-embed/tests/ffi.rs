//! The C functions' safe halves (`clax_embed::ffi_*`), which the four
//! exports of `src/ffi.rs` call after making slices of their pointers, with
//! a collector in place of Dart's `NativeApi.postCObject`; and the two
//! exports that take no pointer, called directly.

use clax_embed::frame::{self, As, RequestHead, ResponseHead};
use clax_embed::{Post, ffi_call, ffi_close, ffi_open, ffi_subscribe};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Every message posted, by port, in order.
#[derive(Default)]
struct Collector {
    posted: Mutex<Vec<(i64, Vec<u8>)>>,
    arrived: Condvar,
}

impl Post for Collector {
    fn post(&self, port: i64, bytes: &[u8]) -> bool {
        self.posted.lock().unwrap().push((port, bytes.to_vec()));
        self.arrived.notify_all();
        true
    }
}

impl Collector {
    /// The first message on `port` that `want` accepts, waiting for it.
    fn take(&self, port: i64, want: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let mut posted = self.posted.lock().unwrap();
        loop {
            if let Some(i) = posted.iter().position(|(p, b)| *p == port && want(b)) {
                return posted.remove(i).1;
            }
            posted = self.arrived.wait(posted).unwrap();
        }
    }

    /// The next open event on `port` of `kind`, skipping the others.
    fn event(&self, port: i64, kind: &str) -> Value {
        let b = self.take(port, |b| {
            serde_json::from_slice::<Value>(b).is_ok_and(|v| v["type"] == kind)
        });
        serde_json::from_slice(&b).unwrap()
    }
}

/// A [`Collector`] whose first post to one port panics.
struct PanicsOnce {
    posts: Collector,
    port: i64,
    panicked: AtomicBool,
}

impl PanicsOnce {
    fn on(port: i64) -> Arc<PanicsOnce> {
        Arc::new(PanicsOnce {
            posts: Collector::default(),
            port,
            panicked: AtomicBool::new(false),
        })
    }
}

impl Post for PanicsOnce {
    fn post(&self, port: i64, bytes: &[u8]) -> bool {
        if port == self.port && !self.panicked.swap(true, Ordering::SeqCst) {
            panic!("a post to port {port} panics once");
        }
        self.posts.post(port, bytes)
    }
}

/// The response frame posted to `port` under `tag`.
fn answer(posts: &Collector, port: i64, tag: i64) -> (ResponseHead, Value) {
    let answer = posts.take(port, |b| b.len() >= 8 && b[..8] == tag.to_le_bytes());
    let (head, body) = frame::split(&answer[8..]).unwrap();
    (
        serde_json::from_slice(head).unwrap(),
        serde_json::from_slice(body).unwrap(),
    )
}

/// A home with the origins the app writes before it opens the core.
fn home(dir: &std::path::Path) -> std::path::PathBuf {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("embed.json"),
        json!({"shell": "clax://app", "content": "clax://{id}.c"}).to_string(),
    )
    .unwrap();
    home
}

fn request(method: &str, path: &str) -> Vec<u8> {
    frame::encode(
        &RequestHead {
            method: method.into(),
            path: path.into(),
            headers: vec![],
            as_: As::Owner,
            host: None,
        },
        b"",
    )
}

#[test]
fn open_call_subscribe_and_close_post_to_their_ports() {
    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let posts = Arc::new(Collector::default());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    assert_ne!(core, 0);
    posts.event(1, "ready");

    assert_eq!(ffi_call(core, &request("GET", "/api/artifacts"), 2, 7), 0);
    let answer = posts.take(2, |b| b.len() >= 8 && b[..8] == 7i64.to_le_bytes());
    let (head, body) = frame::split(&answer[8..]).unwrap();
    let head: ResponseHead = serde_json::from_slice(head).unwrap();
    assert_eq!(head.status, 200);
    let v: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(v["artifacts"], json!([]), "{v}");

    assert_eq!(ffi_subscribe(core, 3), 0);
    posts.take(3, |b| b.starts_with(b"event: ready\n"));

    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
    posts.take(3, |b| {
        b == b"event: end\ndata: {\"reason\":\"core_closed\"}\n\n"
    });
    assert_eq!(ffi_close(core), 1, "a closed core's handle is unknown");
}

#[test]
fn a_header_that_does_not_parse_is_answered_400() {
    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let posts = Arc::new(Collector::default());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    posts.event(1, "ready");
    let bad = [b"CLX1".as_slice(), &2u32.to_le_bytes(), b"{]"].concat();
    assert_eq!(ffi_call(core, &bad, 2, -1), 0);
    let answer = posts.take(2, |b| b.len() >= 8 && b[..8] == (-1i64).to_le_bytes());
    let (head, body) = frame::split(&answer[8..]).unwrap();
    let head: ResponseHead = serde_json::from_slice(head).unwrap();
    assert_eq!(head.status, 400);
    let v: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(v["error"]["code"], "bad_frame", "{v}");
    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
}

#[test]
fn bad_handles_and_frames_are_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let posts = Arc::new(Collector::default());
    assert_eq!(ffi_call(999, &request("GET", "/api/artifacts"), 2, 1), 1);
    assert_eq!(ffi_subscribe(999, 3), 1);
    assert_eq!(ffi_close(999), 1);
    assert_eq!(clax_embed::ffi::clax_embed_subscribe(999, 3), 1);
    assert_eq!(clax_embed::ffi::clax_embed_close(999), 1);

    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    assert_eq!(ffi_call(core, b"CLX2", 2, 1), 2);
    assert_eq!(ffi_call(core, b"CLX1\x09\0\0\0{}", 2, 1), 2);
    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
}

#[test]
fn a_path_that_is_not_utf8_answers_0_and_a_home_open_already_its_handle() {
    let posts = Arc::new(Collector::default());
    assert_eq!(ffi_open(&[0xff, 0xfe], posts.clone(), 1), 0);
    assert_eq!(posts.event(1, "failed")["code"], "open_failed");

    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 2);
    assert_ne!(core, 0);
    posts.event(2, "ready");
    assert_eq!(
        ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 4),
        core,
        "an open of a home this process holds re-attaches"
    );
    posts.event(4, "ready");
    assert_eq!(ffi_close(core), 0);
    posts.event(4, "closed");
}

#[test]
fn a_home_without_its_origins_fails_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let posts = Arc::new(Collector::default());
    let core = ffi_open(dir.path().to_str().unwrap().as_bytes(), posts.clone(), 1);
    assert_ne!(core, 0);
    let failed = posts.event(1, "failed");
    assert_eq!(failed["code"], "no_origins", "{failed}");

    std::fs::write(dir.path().join("embed.json"), "{\"shell\": 1}").unwrap();
    let core = ffi_open(dir.path().to_str().unwrap().as_bytes(), posts.clone(), 5);
    assert_ne!(core, 0);
    let failed = posts.event(5, "failed");
    assert_eq!(failed["code"], "no_origins", "{failed}");
}

#[test]
fn a_failed_open_is_a_handle_that_answers_and_closes() {
    let dir = tempfile::tempdir().unwrap();
    let posts = Arc::new(Collector::default());
    let core = ffi_open(dir.path().to_str().unwrap().as_bytes(), posts.clone(), 1);
    assert_ne!(core, 0);
    posts.event(1, "failed");

    assert_eq!(ffi_call(core, &request("GET", "/api/artifacts"), 2, 9), 0);
    let (head, v) = answer(&posts, 2, 9);
    assert_eq!(head.status, 503, "{v}");
    assert_eq!(v["error"]["code"], "core_failed", "{v}");
    assert_eq!(ffi_subscribe(core, 3), 0);
    let end = posts.take(3, |_| true);
    assert_eq!(
        String::from_utf8(end).unwrap(),
        "event: end\ndata: {\"reason\":\"core_failed\"}\n\n"
    );

    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
    assert_eq!(ffi_close(core), 1, "a closed handle is unknown");
}

#[test]
fn a_post_that_panics_does_not_unwind_out_of_the_c_functions() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_str().unwrap().as_bytes();

    // The open port's `failed` panics; the handle still closes.
    let posts = PanicsOnce::on(1);
    let core = ffi_open(home, posts.clone(), 1);
    assert_ne!(core, 0);
    assert_eq!(ffi_close(core), 0);
    posts.posts.event(1, "closed");

    // A call's answer panics; the next call is answered.
    let posts = PanicsOnce::on(2);
    let core = ffi_open(home, posts.clone(), 1);
    posts.posts.event(1, "failed");
    assert_eq!(ffi_call(core, &request("GET", "/api/artifacts"), 2, 5), 0);
    assert_eq!(ffi_call(core, &request("GET", "/api/artifacts"), 2, 6), 0);
    let (head, _) = answer(&posts.posts, 2, 6);
    assert_eq!(head.status, 503);
    assert_eq!(ffi_close(core), 0);
}

/// A request frame with `headers` and `body`, as the owner.
fn request_with(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    frame::encode(
        &RequestHead {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            as_: As::Owner,
            host: None,
        },
        body,
    )
}

/// Sends `req` on `port` under `tag`; its status and JSON body.
fn send(posts: &Collector, core: u64, port: i64, tag: i64, req: &[u8]) -> (u16, Value) {
    assert_eq!(ffi_call(core, req, port, tag), 0);
    let (head, v) = answer(posts, port, tag);
    (head.status, v)
}

/// A cancel frame for the call sent under `tag` (spec §2.3).
fn cancel(tag: i64) -> Vec<u8> {
    frame::encode(&json!({ "cancel": tag }), b"")
}

#[test]
fn a_cancelled_feedback_poll_takes_no_batch() {
    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let posts = Arc::new(Collector::default());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    posts.event(1, "ready");
    let json_body = [("content-type", "application/json")];
    let (status, v) = send(
        &posts,
        core,
        2,
        1,
        &request_with(
            "POST",
            "/api/sessions",
            &json_body,
            json!({"harness": "app", "harness_session_id": "install-1", "cwd": "/w"})
                .to_string()
                .as_bytes(),
        ),
    );
    assert_eq!(status, 201, "{v}");
    let sid = v["session"]["id"].as_str().unwrap().to_string();
    let (status, v) = send(
        &posts,
        core,
        2,
        2,
        &request_with(
            "POST",
            "/api/artifacts",
            &[json_body[0], ("x-clax-session", &sid)],
            json!({"title": "t", "files": {"index.html": {"content": "<title>t</title>runs", "encoding": "utf8"}}})
                .to_string()
                .as_bytes(),
        ),
    );
    assert_eq!(status, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();

    // A poll that would wait far longer than the test, cancelled at once
    // by a frame on the same port: the cancel is answered 204 under its
    // own tag, and the poll never.
    let poll = format!("/api/sessions/{sid}/feedback?tier=wait&wait=600");
    assert_eq!(ffi_call(core, &request("GET", &poll), 3, 10), 0);
    assert_eq!(ffi_call(core, &cancel(10), 3, 11), 0);
    let cancelled = posts.take(3, |b| b.len() >= 8 && b[..8] == 11i64.to_le_bytes());
    let (head, body) = frame::split(&cancelled[8..]).unwrap();
    let head: ResponseHead = serde_json::from_slice(head).unwrap();
    assert_eq!(head.status, 204);
    assert!(body.is_empty());

    // Feedback arrives after the cancel: a thread, sent to the agent.
    let boundary = "clax-test-boundary";
    let anchor = json!({"kind": "element", "selector": "body", "quote": "runs",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null});
    let mut form = String::new();
    for (name, value) in [
        ("anchor", anchor.to_string()),
        ("body", "make it blue".to_string()),
        ("version", "1".to_string()),
    ] {
        form.push_str(&format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    form.push_str(&format!("--{boundary}--\r\n"));
    let ct = format!("multipart/form-data; boundary={boundary}");
    let (status, v) = send(
        &posts,
        core,
        2,
        3,
        &request_with(
            "POST",
            &format!("/api/artifacts/{aid}/threads"),
            &[("content-type", &ct)],
            form.as_bytes(),
        ),
    );
    assert_eq!(status, 201, "{v}");
    let tid = v["thread"]["id"].as_str().unwrap().to_string();
    let (status, v) = send(
        &posts,
        core,
        2,
        4,
        &request("POST", &format!("/api/artifacts/{aid}/threads/{tid}/send")),
    );
    assert_eq!(status, 200, "{v}");

    // The next poll takes the batch the cancelled one would have.
    let (status, v) = send(
        &posts,
        core,
        4,
        12,
        &request("GET", &format!("/api/sessions/{sid}/feedback?tier=wait&wait=5")),
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["feedback"][0]["body"], "make it blue", "{v}");
    assert!(
        !posts
            .posted
            .lock()
            .unwrap()
            .iter()
            .any(|(p, b)| *p == 3 && b.len() >= 8 && b[..8] == 10i64.to_le_bytes()),
        "the cancelled poll was answered"
    );
    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
}

#[test]
fn a_cancel_of_a_call_already_answered_or_never_sent_is_answered_204() {
    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let posts = Arc::new(Collector::default());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    posts.event(1, "ready");
    let (status, _) = send(&posts, core, 2, 1, &request("GET", "/api/artifacts"));
    assert_eq!(status, 200);
    for (tag, target) in [(2, 1), (3, 99)] {
        assert_eq!(ffi_call(core, &cancel(target), 2, tag), 0);
        let a = posts.take(2, |b| b.len() >= 8 && b[..8] == tag.to_le_bytes());
        let (head, _) = frame::split(&a[8..]).unwrap();
        let head: ResponseHead = serde_json::from_slice(head).unwrap();
        assert_eq!(head.status, 204);
    }
    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
}
