//! The web UI the core serves: the one `clax-embedded` carries.

use clax_core::embedded::{EmbeddedFile, FileTree};
use std::sync::Arc;

/// [`clax_embedded::web_ui`] as a tree (`clax-cli`'s own `WebUi` in
/// `crates/clax-cli/src/bundle.rs`; `clax-embedded` depends on no crate of
/// the workspace, so it cannot implement [`FileTree`] itself).
struct WebUi;

impl FileTree for WebUi {
    fn get(&self, path: &str) -> Option<EmbeddedFile> {
        clax_embedded::web_ui(path).map(|f| EmbeddedFile {
            data: f.data,
            sha256: f.sha256,
        })
    }

    fn paths(&self) -> Vec<String> {
        clax_embedded::web_ui_paths()
    }
}

/// The embedded web UI, always: an app has no checkout to read it from.
pub fn web_ui() -> Arc<dyn FileTree> {
    Arc::new(WebUi)
}
