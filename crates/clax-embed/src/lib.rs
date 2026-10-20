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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Where the core posts a port's messages.
pub trait Post: Send + Sync + 'static {
    /// Posts `bytes` to `port` (the receiver copies them); false when the
    /// port is closed.
    fn post(&self, port: i64, bytes: &[u8]) -> bool;
}

/// One open core in the registry: the core and where its ports are posted.
#[derive(Clone)]
struct Registered {
    embed: Embed,
    post: Arc<dyn Post>,
}

/// The open cores by handle.
static CORES: LazyLock<Mutex<HashMap<u64, Registered>>> = LazyLock::new(Default::default);
/// The next handle; 0 is never one.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn registered(core: u64) -> Option<Registered> {
    CORES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&core)
        .cloned()
}

fn post_json<T: serde::Serialize>(post: &dyn Post, port: i64, v: &T) -> bool {
    post.post(port, &serde_json::to_vec(v).expect("an event serializes"))
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
/// home open. A home without its `embed.json` gets a handle of no core and
/// `failed` with code `no_origins`.
pub fn ffi_open(home: &[u8], post: Arc<dyn Post>, port: i64) -> u64 {
    let Ok(home) = std::str::from_utf8(home) else {
        return 0;
    };
    let home = std::path::PathBuf::from(home);
    let handle = NEXT.fetch_add(1, Ordering::SeqCst);
    let Some(origins) = origins(&home) else {
        post_json(
            &*post,
            port,
            &OpenEvent::Failed {
                code: "no_origins".into(),
                message: format!("{} has no readable embed.json", home.display()),
                backup: None,
            },
        );
        return handle;
    };
    let events = post.clone();
    let opened = Embed::open(
        OpenOptions {
            home,
            origins,
            log: None,
        },
        Box::new(move |e| {
            post_json(&*events, port, &e);
        }),
    );
    match opened {
        Ok(embed) => {
            CORES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(handle, Registered { embed, post });
            handle
        }
        Err(_) => 0,
    }
}

/// [`ffi::clax_embed_call`]'s safe half: 0 when the core took the frame
/// (its response, after the tag, is posted to `port`; a header that does
/// not parse is answered 400 `bad_frame`), 1 for an unknown handle, 2 for
/// bytes that are not a frame.
pub fn ffi_call(core: u64, req: &[u8], port: i64, tag: i64) -> i32 {
    let Some(r) = registered(core) else {
        return 1;
    };
    if frame::split(req).is_err() {
        return 2;
    }
    let req = req.to_vec();
    let embed = r.embed.clone();
    r.embed.spawn(async move {
        let answer = match embed.call(&req).await {
            Ok(a) => a,
            Err(e) => error_frame(400, "bad_frame", &format!("{e:?}")),
        };
        if !r.post.post(port, &frame::tagged(tag, answer)) {
            tracing::debug!(port, "a call's port closed before its answer");
        }
    });
    0
}

/// [`ffi::clax_embed_subscribe`]'s safe half: posts each SSE block of the
/// core's event stream to `port` until a post fails (the port closed) or
/// the core closes; 0, or 1 for an unknown handle.
pub fn ffi_subscribe(core: u64, port: i64) -> i32 {
    let Some(r) = registered(core) else {
        return 1;
    };
    let post = r.post.clone();
    r.embed
        .subscribe(Box::new(move |block| post.post(port, &block)));
    0
}

/// [`ffi::clax_embed_close`]'s safe half: forgets the handle and stops the
/// core ([`Embed::close`]); 0, or 1 for an unknown handle.
pub fn ffi_close(core: u64) -> i32 {
    let removed = CORES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&core);
    match removed {
        Some(r) => {
            r.embed.close();
            0
        }
        None => 1,
    }
}
