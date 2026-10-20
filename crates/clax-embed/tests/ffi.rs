//! The C functions' safe halves (`clax_embed::ffi_*`), which the four
//! exports of `src/ffi.rs` call after making slices of their pointers, with
//! a collector in place of Dart's `NativeApi.postCObject`; and the two
//! exports that take no pointer, called directly.

use clax_embed::frame::{self, As, RequestHead, ResponseHead};
use clax_embed::{Post, ffi_call, ffi_close, ffi_open, ffi_subscribe};
use serde_json::{Value, json};
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
fn a_path_that_is_not_utf8_or_a_home_open_already_answers_0() {
    let posts = Arc::new(Collector::default());
    assert_eq!(ffi_open(&[0xff, 0xfe], posts.clone(), 1), 0);

    let dir = tempfile::tempdir().unwrap();
    let home = home(dir.path());
    let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
    assert_ne!(core, 0);
    assert_eq!(
        ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 4),
        0
    );
    assert_eq!(ffi_close(core), 0);
    posts.event(1, "closed");
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
