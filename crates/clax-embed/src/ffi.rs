//! The four C functions of `include/clax_embed.h` (spec 2026-10-19 §2.2),
//! and the only `unsafe` in the crate: each makes a slice of the bytes its
//! caller lends it, then calls its safe half in `lib.rs`. Handles are
//! numbers in a registry, never pointers.

use crate::dart::{ClaxPost, DartPost};
use std::sync::Arc;

/// Opens the core on the home at `home[..home_len]` (UTF-8); see the header.
/// A NULL `home` or `post` answers 0.
///
/// # Safety
/// `home`, when not NULL, points to `home_len` (at most `isize::MAX`)
/// readable bytes that stay valid and unchanged until this returns.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clax_embed_open(
    home: *const u8,
    home_len: usize,
    post: ClaxPost,
    port: i64,
) -> u64 {
    let Some(post) = post else {
        return 0;
    };
    if home.is_null() {
        return 0;
    }
    // SAFETY: home is not NULL (u8 needs no alignment), and the caller lends home_len readable bytes until this returns.
    let home = unsafe { std::slice::from_raw_parts(home, home_len) };
    crate::ffi_open(home, Arc::new(DartPost(post)), port)
}

/// Sends one request frame `req[..req_len]`; see the header. A NULL `req`
/// answers 2.
///
/// # Safety
/// `req`, when not NULL, points to `req_len` (at most `isize::MAX`)
/// readable bytes that stay valid and unchanged until this returns.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clax_embed_call(
    core: u64,
    req: *const u8,
    req_len: usize,
    port: i64,
    tag: i64,
) -> i32 {
    if req.is_null() {
        return 2;
    }
    // SAFETY: req is not NULL (u8 needs no alignment), and the caller lends req_len readable bytes until this returns.
    let req = unsafe { std::slice::from_raw_parts(req, req_len) };
    crate::ffi_call(core, req, port, tag)
}

/// Posts the core's events to `port`; see the header.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_embed_subscribe(core: u64, port: i64) -> i32 {
    crate::ffi_subscribe(core, port)
}

/// Stops the core; see the header.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_embed_close(core: u64) -> i32 {
    crate::ffi_close(core)
}
