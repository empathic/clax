//! `clax-embedded`'s files as the trees the commands and the daemon read
//! (`clax_core::embedded`).

use clax_core::embedded::{Bundle, DiskTree, EmbeddedFile, FileTree, MemTree};
use std::sync::{Arc, LazyLock};

/// The files the commands read ([`clax_commands::Env::embedded`]).
pub struct Embedded;

/// The extension's and the plugins' files, read from the embed once.
static EXTENSION: LazyLock<MemTree> =
    LazyLock::new(|| MemTree(clax_embedded::extension_files().into_iter().collect()));
static PLUGINS: LazyLock<MemTree> =
    LazyLock::new(|| MemTree(clax_embedded::plugin_files().into_iter().collect()));

impl Bundle for Embedded {
    fn extension(&self) -> &dyn FileTree {
        &*EXTENSION
    }

    fn plugins(&self) -> &dyn FileTree {
        &*PLUGINS
    }

    fn launcher(&self) -> &'static str {
        clax_embedded::LAUNCHER
    }
}

/// [`clax_embedded::web_ui`] as a tree.
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

/// The web UI the daemon serves: the checkout's `web/dist` read from disk
/// where the build reads it there (a debug build that does not embed it),
/// so a rebuild under a running daemon is served at once; else the embed.
pub fn web_ui() -> Arc<dyn FileTree> {
    match clax_embedded::web_ui_dir() {
        Some(dir) => Arc::new(DiskTree::new(dir)),
        None => Arc::new(WebUi),
    }
}
