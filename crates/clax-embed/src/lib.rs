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

mod core;
pub mod dart;
mod events;
pub mod ffi;
pub mod frame;
mod log;
mod web;

pub use crate::core::{
    Embed, OpenError, OpenEvent, OpenOptions, RESPONSE_LIMIT, ReadError, admit, error_frame,
    read_limited,
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
}

/// The open cores by handle.
static CORES: LazyLock<Mutex<HashMap<u64, Registered>>> = LazyLock::new(Default::default);
/// The next handle; 0 is never one.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn cores() -> std::sync::MutexGuard<'static, HashMap<u64, Registered>> {
    CORES.lock().unwrap_or_else(|e| e.into_inner())
}

fn registered(core: u64) -> Option<Registered> {
    cores().get(&core).cloned()
}

/// Registers a handle and returns it.
fn register(embed: Option<Embed>, post: Arc<dyn Post>, port: i64) -> u64 {
    let handle = NEXT.fetch_add(1, Ordering::SeqCst);
    cores().insert(handle, Registered { embed, post, port });
    handle
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
    post_bytes(post, port, &serde_json::to_vec(v).expect("an event serializes"))
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
/// core (spec §4.1): `{"shell": …, "content": …}`.
fn origins(home: &std::path::Path) -> Option<clax_http::state::EmbedOrigins> {
    #[derive(serde::Deserialize)]
    struct File {
        shell: String,
        content: String,
    }
    let text = std::fs::read(home.join("embed.json")).ok()?;
    let f: File = serde_json::from_slice(&text).ok()?;
    Some(clax_http::state::EmbedOrigins {
        shell: f.shell,
        content: f.content,
    })
}

/// [`ffi::clax_embed_open`]'s safe half: opens the core on the home at
/// `home` (UTF-8), posting its [`OpenEvent`]s to `port` as JSON, and
/// returns its handle; 0 when `home` is not UTF-8 or this process has that
/// home open. An open that fails before a core exists (no readable
/// `embed.json`: `no_origins`; a runtime that does not start or a panic:
/// `internal`) posts `failed` and still returns a handle, which
/// [`ffi_close`] closes as any other.
pub fn ffi_open(home: &[u8], post: Arc<dyn Post>, port: i64) -> u64 {
    let Ok(home) = std::str::from_utf8(home) else {
        return 0;
    };
    let home = std::path::PathBuf::from(home);
    let failed = |post: &Arc<dyn Post>, event: OpenEvent| {
        post_json(&**post, port, &event);
        register(None, post.clone(), port)
    };
    guarded(
        "clax_embed_open",
        || {
            let Some(origins) = origins(&home) else {
                let message = format!("{} has no readable embed.json", home.display());
                return failed(
                    &post,
                    OpenEvent::Failed {
                        code: "no_origins".into(),
                        message,
                        backup: None,
                    },
                );
            };
            let events = post.clone();
            let opened = Embed::open(
                OpenOptions {
                    home: home.clone(),
                    origins,
                    log: None,
                },
                Box::new(move |e| {
                    post_json(&*events, port, &e);
                }),
            );
            match opened {
                Ok(embed) => register(Some(embed), post.clone(), port),
                Err(OpenError::HomeInUse | OpenError::NotUtf8) => 0,
                Err(OpenError::Runtime(e)) => {
                    failed(&post, internal(format!("the core's runtime did not start: {e}")))
                }
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
pub fn ffi_call(core: u64, req: &[u8], port: i64, tag: i64) -> i32 {
    let Some(r) = registered(core) else {
        return 1;
    };
    if frame::split(req).is_err() {
        return 2;
    }
    let answer = |bytes: Vec<u8>| {
        if !post_bytes(&*r.post, port, &frame::tagged(tag, bytes)) {
            tracing::debug!(port, "a call's port closed before its answer");
        }
    };
    guarded(
        "clax_embed_call",
        || {
            let Some(embed) = &r.embed else {
                answer(error_frame(503, "core_failed", "the core did not open"));
                return 0;
            };
            let req = req.to_vec();
            let call = embed.clone();
            let post = r.post.clone();
            embed.spawn(async move {
                let answered = AssertUnwindSafe(call.call(&req)).catch_unwind().await;
                let bytes = match answered {
                    Ok(Ok(a)) => a,
                    Ok(Err(e)) => error_frame(400, "bad_frame", &format!("{e:?}")),
                    Err(_) => error_frame(500, "internal", "the core panicked while it answered"),
                };
                if !post_bytes(&*post, port, &frame::tagged(tag, bytes)) {
                    tracing::debug!(port, "a call's port closed before its answer");
                }
            });
            0
        },
        || {
            answer(error_frame(500, "internal", "the core panicked while it took the call"));
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
        || ended(),
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
