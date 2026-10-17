//! The files the `clax` binary carries: the web UI (`web/dist`, [`web_ui`]),
//! the Chrome extension (`web/dist-extension`, [`extension_files`]), the
//! plugin trees and their marketplaces ([`plugin_files`]) and the plugins'
//! launcher ([`LAUNCHER`]).
//!
//! It depends on no workspace crate, so nothing but an edit of these files
//! rebuilds it, and only the binary (`clax-cli`) links it: the daemon and
//! the commands read the files through `clax_core::embedded`, to which the
//! binary adapts these functions. A release build embeds every file; a
//! debug build reads them from the checkout on each call, unless it embeds
//! them too (`clax_embedded`, as Bazel's does). `build.rs` names every
//! embedded directory, so a new file rebuilds the crate.

mod extension;
mod plugins;
mod web;

pub use extension::extension_files;
pub use plugins::plugin_files;
pub use web::{web_ui, web_ui_dir, web_ui_paths};

use std::borrow::Cow;

/// One embedded file: its bytes and their SHA-256.
pub struct File {
    pub data: Cow<'static, [u8]>,
    pub sha256: [u8; 32],
}

/// The plugins' launcher (`scripts/ensure-clax.sh`): each plugin carries a
/// copy, and `clax doctor --agent` checks the installed ones against it.
pub const LAUNCHER: &str = include_str!("../../../scripts/ensure-clax.sh");
