//! Clax's core inside the Clax app's process (the in-process core design,
//! spec 2026-10-19 §2): what `clax serve` runs, without a listener, behind
//! four C functions that take request frames and post responses and
//! events to Dart ports.

mod core;
pub mod frame;
mod log;
mod web;

pub use crate::core::{
    Embed, OpenError, OpenEvent, OpenOptions, RESPONSE_LIMIT, ReadError, admit, error_frame,
    read_limited,
};
