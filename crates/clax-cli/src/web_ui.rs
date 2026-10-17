//! The web UI the daemon serves (`ServeConfig::web`): `clax-embedded`'s, as
//! a [`FileTree`].

use clax_core::embedded::{DiskTree, EmbeddedFile, FileTree};
use std::sync::Arc;

/// [`clax_embedded::web_ui`] as a tree.
struct Embedded;

impl FileTree for Embedded {
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

/// The build's web UI: the checkout's `web/dist` read from disk where the
/// build reads it there (a debug build that does not embed it), so a
/// rebuild under a running daemon is served at once; else the embed.
pub fn built_in() -> Arc<dyn FileTree> {
    match clax_embedded::web_ui_dir() {
        Some(dir) => Arc::new(DiskTree::new(dir)),
        None => Arc::new(Embedded),
    }
}
