//! Clax's core inside the Clax app's process (the in-process core design,
//! spec 2026-10-19 §2): what `clax serve` runs, without a listener, behind
//! four C functions (`include/clax_embed.h`, [`ffi`]) that take request
//! frames ([`frame`]) and post responses and events to Dart ports.
//!
//! The C functions' safe halves are here: [`ffi_open`], [`ffi_call`],
//! [`ffi_subscribe`] and [`ffi_close`], over a process-wide registry of
//! open cores by handle. They post through a [`Post`]: Dart's
//! `NativeApi.postCObject` in the app ([`dart::DartPost`]), a collector in
//! tests.
//!
//! A call's bytes are copied as few times as the ABI allows: the request
//! frame once out of the buffer Dart lends for the call (the body is a
//! slice of that copy), and the response once, chunk by chunk, into the
//! buffer that is posted (the tag and the frame's head first, sized from
//! the body's size hint, and grown only for a body whose size is not known
//! ahead); Dart then copies that buffer as it posts it, and Rust frees it
//! when the post returns.

mod core;
pub mod dart;
mod events;
pub mod ffi;
pub mod frame;
mod log;
mod web;

pub use crate::core::{
    Embed, OpenError, OpenEvent, OpenOptions, RESPONSE_LIMIT, admit, error_frame, error_frame_after,
};

use futures::FutureExt;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Where the core posts a port's messages.
pub trait Post: Send + Sync + 'static {
    /// Posts `bytes` to `port` (the receiver copies them); false when the
    /// port is closed.
    fn post(&self, port: i64, bytes: &[u8]) -> bool;
}

/// One handle in the registry: its core, and where its ports are posted.
#[derive(Clone)]
struct Registered {
    /// `None` for an open that failed before a core existed (`no_origins`,
    /// `internal`): calls answer 503 `core_failed`, and a close posts
    /// `closed` to [`Registered::port`] itself.
    embed: Option<Embed>,
    post: Arc<dyn Post>,
    /// The port given at open.
    port: i64,
    /// Where the core's open events go, for a core: an open that
    /// re-attaches to it moves them ([`reattach`]).
    events: Option<Arc<Mutex<OpenPort>>>,
    /// The calls in flight by port and tag, until each has its answer, for
    /// a cancel frame to abort ([`ffi_call`]).
    calls: Arc<Mutex<HashMap<(i64, i64), tokio::task::AbortHandle>>>,
}

/// Where a core's open events are posted, and the latest one posted.
struct OpenPort {
    port: i64,
    last: Option<Vec<u8>>,
}

/// The open cores by handle.
static CORES: LazyLock<Mutex<HashMap<u64, Registered>>> = LazyLock::new(Default::default);
/// The next handle; 0 is never one.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn cores() -> std::sync::MutexGuard<'static, HashMap<u64, Registered>> {
    lock(&CORES)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn registered(core: u64) -> Option<Registered> {
    cores().get(&core).cloned()
}

/// Registers a handle and returns it.
fn register(
    embed: Option<Embed>,
    post: Arc<dyn Post>,
    port: i64,
    events: Option<Arc<Mutex<OpenPort>>>,
) -> u64 {
    let handle = NEXT.fetch_add(1, Ordering::SeqCst);
    cores().insert(
        handle,
        Registered {
            embed,
            post,
            port,
            events,
            calls: Arc::default(),
        },
    );
    handle
}

/// Opens, one at a time: an open's [`Embed::open`] and its registration
/// happen under it, so an open of a home this process holds finds that
/// home's core registered ([`reattach`]), never a home held with no
/// handle yet.
static OPENING: Mutex<()> = Mutex::new(());

/// The message a re-attach posts to a core's open port to learn whether
/// its client lives: no open event (spec §2.2), and the client ignores it.
const PROBE: &[u8] = br#"{"type":"probe"}"#;

/// What [`reattach`] found.
enum Reattach {
    /// The core's handle: its open events now go to the new port.
    To(u64),
    /// The core's client lives (its open port takes a post).
    InUse,
    /// No registered core holds the home: its core is closing.
    Closing,
}

/// The handle of the core this process holds `home` with, when its
/// client is gone, its open events moved to `port`, which hears the
/// latest one at once (`ready`, or where opening has got to). A core
/// whose open failed or that is closing is not re-attached to; nor is one
/// whose client still lives: a post to its open port fails only once the
/// isolate that opened it is gone ([`PROBE`]).
fn reattach(home: &std::path::Path, post: &dyn Post, port: i64) -> Reattach {
    let key = crate::core::home_key(home);
    let mut cores = cores();
    let found = cores.iter_mut().find(|(_, r)| {
        r.embed
            .as_ref()
            .is_some_and(|e| e.home() == key && e.holds_home() && !e.closing())
    });
    let Some((handle, r)) = found else {
        return Reattach::Closing;
    };
    let Some(events) = r.events.as_ref() else {
        return Reattach::Closing;
    };
    let mut to = lock(events);
    if post_bytes(&*r.post, to.port, PROBE) {
        return Reattach::InUse;
    }
    to.port = port;
    r.port = port;
    if let Some(last) = &to.last {
        post_bytes(post, port, last);
    }
    Reattach::To(*handle)
}

/// `f`'s answer, or `on_panic`'s when `f` panics (and `last` when that
/// panics too): no panic leaves a C function, where it would abort the
/// app's process.
fn guarded<T>(what: &str, f: impl FnOnce() -> T, on_panic: impl FnOnce() -> T, last: T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            tracing::error!(what, "a C function's safe half panicked");
            catch_unwind(AssertUnwindSafe(on_panic)).unwrap_or(last)
        }
    }
}

/// Posts `bytes`; false when the port is closed or the post panicked.
fn post_bytes(post: &dyn Post, port: i64, bytes: &[u8]) -> bool {
    catch_unwind(AssertUnwindSafe(|| post.post(port, bytes))).unwrap_or(false)
}

fn post_json<T: serde::Serialize>(post: &dyn Post, port: i64, v: &T) -> bool {
    post_bytes(
        post,
        port,
        &serde_json::to_vec(v).expect("an event serializes"),
    )
}

/// The `failed` event of an open that panicked or could not start.
fn internal(message: String) -> OpenEvent {
    OpenEvent::Failed {
        code: "internal".into(),
        message,
        backup: None,
    }
}

/// The origins the app wrote to `<home>/embed.json` before it opened the
/// core (spec §4.1): `{"shell": …, "content": …}`; why not, as the
/// message of the `no_origins` its open fails with. The content template
/// must hold exactly one `{id}`, as the whole first label of its host
/// ([`content_template_ok`]): any other gives artifacts a shared origin,
/// which their isolation (spec §4.3) rests on not doing.
fn origins(home: &std::path::Path) -> Result<clax_http::state::EmbedOrigins, String> {
    #[derive(serde::Deserialize)]
    struct File {
        shell: String,
        content: String,
    }
    let unreadable = || format!("{} has no readable embed.json", home.display());
    let text = std::fs::read(home.join("embed.json")).map_err(|_| unreadable())?;
    let f: File = serde_json::from_slice(&text).map_err(|_| unreadable())?;
    if !content_template_ok(&f.content) {
        return Err(format!(
            "{}'s content template {:?} does not have exactly one {{id}}, as the first label of its host",
            home.join("embed.json").display(),
            f.content
        ));
    }
    Ok(clax_http::state::EmbedOrigins {
        shell: f.shell,
        content: f.content,
    })
}

/// Whether `template` is `<scheme>://{id}.<rest of the host>[:<port>]`,
/// with no other `{id}` and nothing after the host and port.
fn content_template_ok(template: &str) -> bool {
    let Some((_, authority)) = template.split_once("://") else {
        return false;
    };
    let host = authority.split(':').next().unwrap_or_default();
    template.matches("{id}").count() == 1
        && !authority.contains(['/', '?', '#', '@'])
        && host.split('.').next() == Some("{id}")
}

/// [`ffi::clax_embed_open`]'s safe half: opens the core on the home at
/// `home` (UTF-8), posting its [`OpenEvent`]s to `port` as JSON, and
/// returns its handle. A home this process holds already is not opened
/// again: when the client that opened it is gone (an isolate that
/// restarted without closing it) the open re-attaches to its core
/// ([`reattach`]) and returns that core's handle. 0, with `failed`
/// posted, when `home` is not UTF-8 (`open_failed`), when that home's
/// core has a live client (`home_in_use`, the home open in this app), or
/// when it is closing (`home_in_use`). Opens are taken one at a time
/// ([`OPENING`]), so two at once get one handle and one refusal. An open that fails before a core exists (no readable
/// `embed.json`, or one whose content template [`origins`] refuses:
/// `no_origins`, naming the template; a runtime that does not start or a panic:
/// `internal`) posts `failed` and still returns a handle, which
/// [`ffi_close`] closes as any other.
pub fn ffi_open(home: &[u8], post: Arc<dyn Post>, port: i64) -> u64 {
    let refused = |code: &str, message: &str| {
        let failed = OpenEvent::Failed {
            code: code.into(),
            message: message.into(),
            backup: None,
        };
        let _ = catch_unwind(AssertUnwindSafe(|| post_json(&*post, port, &failed)));
        0
    };
    let Ok(home) = std::str::from_utf8(home) else {
        return refused("open_failed", "the home's path is not UTF-8");
    };
    let home = std::path::PathBuf::from(home);
    let failed = |post: &Arc<dyn Post>, event: OpenEvent| {
        post_json(&**post, port, &event);
        register(None, post.clone(), port, None)
    };
    guarded(
        "clax_embed_open",
        || {
            let _opening = lock(&OPENING);
            let origins = match origins(&home) {
                Ok(origins) => origins,
                Err(message) => {
                    return failed(
                        &post,
                        OpenEvent::Failed {
                            code: "no_origins".into(),
                            message,
                            backup: None,
                        },
                    );
                }
            };
            let events = Arc::new(Mutex::new(OpenPort { port, last: None }));
            let (to, poster) = (events.clone(), post.clone());
            let opened = Embed::open(
                OpenOptions {
                    log: Some(log::path(&home)),
                    home: home.clone(),
                    origins,
                },
                Box::new(move |e| {
                    let bytes = serde_json::to_vec(&e).expect("an event serializes");
                    let mut to = to.lock().unwrap_or_else(|e| e.into_inner());
                    post_bytes(&*poster, to.port, &bytes);
                    to.last = Some(bytes);
                }),
            );
            match opened {
                Ok(embed) => register(Some(embed), post.clone(), port, Some(events)),
                Err(OpenError::HomeInUse) => match reattach(&home, &*post, port) {
                    Reattach::To(handle) => handle,
                    Reattach::InUse => refused(
                        "home_in_use",
                        "this home is open in this app already, by another of its clients",
                    ),
                    Reattach::Closing => {
                        refused("home_in_use", "this app is closing its core on this home")
                    }
                },
                Err(OpenError::NotUtf8) => refused("open_failed", "the home's path is not UTF-8"),
                Err(OpenError::Runtime(e)) => failed(
                    &post,
                    internal(format!("the core's runtime did not start: {e}")),
                ),
            }
        },
        || failed(&post, internal("the core panicked while it opened".into())),
        0,
    )
}

/// [`ffi::clax_embed_call`]'s safe half: 0 when the core took the frame
/// (its response, after the tag, is posted to `port`: a header that does
/// not parse answers 400 `bad_frame`, a handle whose open failed 503
/// `core_failed`, a panic 500 `internal`), 1 for an unknown handle, 2 for
/// bytes that are not a frame.
///
/// A cancel frame (`{"cancel": <tag>}`, spec §2.3) is answered 204 at
/// once, under its own tag, after it aborts the call sent to `port` with
/// that tag if the call is still running: the call's task is dropped at
/// its next await, so a write that has not committed does not happen, and
/// the call is never answered. A cancel of a call already answered, or
/// never sent, does nothing more.
pub fn ffi_call(core: u64, req: &[u8], port: i64, tag: i64) -> i32 {
    let Some(r) = registered(core) else {
        return 1;
    };
    let Ok((head, _)) = frame::split(req) else {
        return 2;
    };
    let key = (port, tag);
    let tag = frame::tag(tag);
    if let Some(target) = frame::cancel_of(head) {
        if let Some(call) = lock(&r.calls).remove(&(port, target)) {
            call.abort();
        }
        let done = frame::encode_after(
            &tag,
            &frame::ResponseHead {
                status: 204,
                headers: vec![],
            },
            &[],
        );
        if !post_bytes(&*r.post, port, &done) {
            tracing::debug!(port, "a cancel's port closed before its answer");
        }
        return 0;
    }
    let answer = |status, code, message: &str| {
        if !post_bytes(
            &*r.post,
            port,
            &error_frame_after(&tag, status, code, message),
        ) {
            tracing::debug!(port, "a call's port closed before its answer");
        }
    };
    guarded(
        "clax_embed_call",
        || {
            let Some(embed) = &r.embed else {
                answer(503, "core_failed", "the core did not open");
                return 0;
            };
            // Dart lends the frame only for this call: its one copy.
            let req = bytes::Bytes::copy_from_slice(req);
            let call = embed.clone();
            let post = r.post.clone();
            // Counted until its answer is posted, so `closed` follows it.
            let counted = embed.count_call();
            let calls = r.calls.clone();
            // Locked until the call is in `calls`, so its task, which
            // takes it out once it has its answer, cannot do so first.
            let mut in_flight = lock(&r.calls);
            let task = embed.spawn(async move {
                let answered = AssertUnwindSafe(call.call_owned(req, tag.to_vec()))
                    .catch_unwind()
                    .await;
                // Answered: a cancel from here on finds nothing to abort.
                lock(&calls).remove(&key);
                let bytes = match answered {
                    Ok(Ok(a)) => a,
                    Ok(Err(e)) => error_frame_after(&tag, 400, "bad_frame", &format!("{e:?}")),
                    Err(_) => error_frame_after(
                        &tag,
                        500,
                        "internal",
                        "the core panicked while it answered",
                    ),
                };
                if !post_bytes(&*post, port, &bytes) {
                    tracing::debug!(port, "a call's port closed before its answer");
                }
                drop(counted);
            });
            in_flight.insert(key, task.abort_handle());
            0
        },
        || {
            answer(500, "internal", "the core panicked while it took the call");
            0
        },
        0,
    )
}

/// [`ffi::clax_embed_subscribe`]'s safe half: posts each SSE block of the
/// core's event stream to `port` until a post fails (the port closed) or
/// the subscription ends, which posts an `event: end` block last
/// ([`Embed::subscribe`]; at once for a handle whose open failed); 0, or 1
/// for an unknown handle.
pub fn ffi_subscribe(core: u64, port: i64) -> i32 {
    let Some(r) = registered(core) else {
        return 1;
    };
    let ended = || {
        post_bytes(&*r.post, port, &events::end("core_failed"));
        0
    };
    guarded(
        "clax_embed_subscribe",
        || {
            let Some(embed) = &r.embed else {
                return ended();
            };
            let post = r.post.clone();
            embed.subscribe(Box::new(move |block| post_bytes(&*post, port, &block)));
            0
        },
        ended,
        0,
    )
}

/// [`ffi::clax_embed_close`]'s safe half: forgets the handle and stops the
/// core ([`Embed::close`]; for a handle whose open failed, posts `closed`
/// to its open port itself); 0, or 1 for an unknown handle. The handle is
/// forgotten before anything that could panic, so a panic answers 0.
pub fn ffi_close(core: u64) -> i32 {
    let Some(r) = cores().remove(&core) else {
        return 1;
    };
    guarded(
        "clax_embed_close",
        || {
            match &r.embed {
                Some(embed) => embed.close(),
                None => {
                    post_json(&*r.post, r.port, &OpenEvent::Closed);
                }
            }
            0
        },
        || 0,
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, OnceLock};

    /// Every message posted to a port that is open, in order, and how many
    /// calls the core counted in flight as each answer (port 2) was
    /// posted. A post to a port in `closed` (its isolate is gone) fails.
    #[derive(Default)]
    struct Ordered {
        core: OnceLock<Embed>,
        posted: Mutex<Vec<(i64, Vec<u8>, usize)>>,
        arrived: Condvar,
        closed: Mutex<std::collections::HashSet<i64>>,
    }

    impl Post for Ordered {
        fn post(&self, port: i64, bytes: &[u8]) -> bool {
            if self.closed.lock().unwrap().contains(&port) {
                return false;
            }
            let counted = match (port, self.core.get()) {
                (2, Some(e)) => e.in_flight(),
                _ => 0,
            };
            let mut posted = self.posted.lock().unwrap();
            posted.push((port, bytes.to_vec(), counted));
            self.arrived.notify_all();
            true
        }
    }

    impl Ordered {
        /// Closes `port`, as an isolate that ends closes its ports.
        fn close(&self, port: i64) {
            self.closed.lock().unwrap().insert(port);
        }

        /// The messages posted to `port`, as text.
        fn on(&self, port: i64) -> Vec<String> {
            self.posted
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _, _)| *p == port)
                .map(|(_, b, _)| String::from_utf8_lossy(b).into_owned())
                .collect()
        }

        /// Waits for a message on `port` that contains `text`; its index.
        fn wait(&self, port: i64, text: &str) -> usize {
            let mut posted = self.posted.lock().unwrap();
            loop {
                if let Some(i) = posted
                    .iter()
                    .position(|(p, b, _)| *p == port && String::from_utf8_lossy(b).contains(text))
                {
                    return i;
                }
                posted = self.arrived.wait(posted).unwrap();
            }
        }
    }

    fn home(dir: &std::path::Path) -> std::path::PathBuf {
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("embed.json"),
            r#"{"shell": "clax://app", "content": "clax://{id}.c"}"#,
        )
        .unwrap();
        home
    }

    fn request(path: &str) -> Vec<u8> {
        frame::encode(
            &frame::RequestHead {
                method: "GET".into(),
                path: path.into(),
                headers: vec![],
                as_: frame::As::Owner,
                host: None,
            },
            b"",
        )
    }

    #[test]
    fn a_call_is_counted_until_its_answer_is_posted_so_closed_comes_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let posts = Arc::new(Ordered::default());
        let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
        let embed = registered(core).and_then(|r| r.embed).unwrap();
        assert!(posts.core.set(embed).is_ok());
        posts.wait(1, "\"ready\"");
        assert_eq!(ffi_call(core, &request("/api/artifacts"), 2, 7), 0);
        assert_eq!(ffi_close(core), 0);
        let closed = posts.wait(1, "\"closed\"");
        // Answered 200, or 503 core_closed when the close began first.
        let answered = posts.wait(2, "");
        let counted = posts.posted.lock().unwrap()[answered].2;
        assert!(
            counted >= 1,
            "the call was not counted while its answer was posted"
        );
        assert!(answered < closed, "closed came before the answer");
    }

    #[test]
    fn an_open_of_a_home_this_process_holds_reattaches_to_its_core() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let posts = Arc::new(Ordered::default());
        let core = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 1);
        posts.wait(1, "\"ready\"");
        // An isolate that restarted without closing opens the home again:
        // the old isolate, and so its port, is gone.
        posts.close(1);
        let again = ffi_open(home.to_str().unwrap().as_bytes(), posts.clone(), 4);
        assert_eq!(again, core, "the live core's handle");
        posts.wait(4, "\"ready\"");
        assert_eq!(ffi_call(again, &request("/api/artifacts"), 5, 1), 0);
        posts.wait(5, "artifacts");
        assert_eq!(ffi_close(again), 0);
        let closed = posts.wait(4, "\"closed\"");
        let on_old = posts
            .posted
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, b, _)| *p == 1 && String::from_utf8_lossy(b).contains("closed"))
            .count();
        assert_eq!(on_old, 0, "the old port hears nothing after the new open");
        assert!(closed > 0);
    }

    #[test]
    fn an_open_that_answers_0_still_posts_failed() {
        let posts = Arc::new(Ordered::default());
        assert_eq!(ffi_open(b"\xff\xfe", posts.clone(), 6), 0);
        posts.wait(6, "\"failed\"");
    }

    /// The `failed` event posted to `port`, as JSON.
    fn failed_on(posts: &Ordered, port: i64) -> serde_json::Value {
        let i = posts.wait(port, "\"failed\"");
        serde_json::from_slice(&posts.posted.lock().unwrap()[i].1).unwrap()
    }

    #[test]
    fn a_reopen_after_a_failed_open_is_what_a_later_open_reattaches_to() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let path = home.to_str().unwrap().as_bytes();
        let posts = Arc::new(Ordered::default());
        // Another process holds the home: the open fails home_in_use, and
        // its client keeps the failed core's handle without closing it.
        let held = std::fs::File::create(home.join("core.lock")).unwrap();
        let held = clax_core::daemon::FileLock::try_exclusive(held)
            .unwrap()
            .expect("the lock is free");
        let failed = ffi_open(path, posts.clone(), 10);
        assert_ne!(failed, 0);
        assert_eq!(failed_on(&posts, 10)["code"], "home_in_use");
        drop(held);
        // The client tries again, and the core opens.
        let live = ffi_open(path, posts.clone(), 11);
        assert_ne!(live, 0);
        assert_ne!(live, failed);
        posts.wait(11, "\"ready\"");
        // Its isolate restarts: the new one re-attaches to the live core,
        // never to the failed one.
        posts.close(10);
        posts.close(11);
        let again = ffi_open(path, posts.clone(), 12);
        assert_eq!(again, live, "the live core's handle");
        posts.wait(12, "\"ready\"");
        assert_eq!(ffi_call(again, &request("/api/artifacts"), 13, 1), 0);
        posts.wait(13, "artifacts");
        assert_eq!(ffi_close(again), 0);
        posts.wait(12, "\"closed\"");
        assert_eq!(ffi_close(failed), 0);
    }

    #[test]
    fn a_reopen_while_the_first_client_lives_is_refused_home_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let path = home.to_str().unwrap().as_bytes();
        let posts = Arc::new(Ordered::default());
        let core = ffi_open(path, posts.clone(), 1);
        posts.wait(1, "\"ready\"");
        // A second client in a live isolate opens the home too.
        assert_eq!(ffi_open(path, posts.clone(), 4), 0);
        let refused = failed_on(&posts, 4);
        assert_eq!(refused["code"], "home_in_use", "{refused}");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("open in this app"),
            "{refused}"
        );
        // The first client keeps its core: its events and its close.
        assert_eq!(ffi_close(core), 0);
        posts.wait(1, "\"closed\"");
        assert_eq!(posts.on(4).len(), 1, "the refused port hears only failed");
        // What the first client heard besides its events is the probe,
        // which names no event.
        for m in posts.on(1) {
            let v: serde_json::Value = serde_json::from_str(&m).unwrap();
            assert!(
                ["opening", "ready", "closed", "probe"].contains(&v["type"].as_str().unwrap()),
                "{m}"
            );
        }
    }

    #[test]
    fn a_reattach_while_the_core_opens_hears_the_rest_of_the_open() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let path = home.to_str().unwrap().as_bytes();
        let posts = Arc::new(Ordered::default());
        let hold = Arc::new(tokio::sync::Semaphore::new(0));
        crate::core::HOLD_OPENING.set(Some(hold.clone()));
        let core = ffi_open(path, posts.clone(), 1);
        assert_ne!(core, 0);
        // The core holds the home and waits; its client's isolate restarts.
        posts.close(1);
        let again = ffi_open(path, posts.clone(), 4);
        assert_eq!(again, core, "the opening core's handle");
        hold.add_permits(1);
        posts.wait(4, "\"ready\"");
        assert_eq!(ffi_close(again), 0);
        posts.wait(4, "\"closed\"");
    }

    #[test]
    fn two_opens_at_once_answer_one_handle_and_one_refusal_home_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let posts = Arc::new(Ordered::default());
        let start = Arc::new(std::sync::Barrier::new(2));
        let opens: Vec<_> = [20, 21]
            .into_iter()
            .map(|port| {
                let (home, posts, start) = (home.clone(), posts.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    (port, ffi_open(home.to_str().unwrap().as_bytes(), posts, port))
                })
            })
            .collect();
        let opened: Vec<(i64, u64)> = opens.into_iter().map(|t| t.join().unwrap()).collect();
        let live: Vec<_> = opened.iter().filter(|(_, h)| *h != 0).collect();
        assert_eq!(live.len(), 1, "{opened:?}");
        let (port, handle) = *live[0];
        let other = if port == 20 { 21 } else { 20 };
        let refused = failed_on(&posts, other);
        assert_eq!(refused["code"], "home_in_use", "{refused}");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("open in this app"),
            "{refused}"
        );
        posts.wait(port, "\"ready\"");
        assert_eq!(ffi_close(handle), 0);
        posts.wait(port, "\"closed\"");
    }
}
