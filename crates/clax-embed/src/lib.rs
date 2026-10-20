//! Clax's core inside the Clax app's process (the in-process core design,
//! spec 2026-10-19 §2): what `clax serve` runs, without a listener, behind
//! four C functions that take request frames and post responses and
//! events to Dart ports.

pub mod frame;
pub mod log;
pub mod web;
