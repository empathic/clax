//! The web UI the server's routes render pages from (`web/dist`, as
//! `build.rs` names it), embedded
//! in the Worker: the artifact page and the gallery the router answers, as
//! the daemon serves them from its own tree. Static files under `/_clax/`
//! come from the static assets binding instead (`static_asset`).

use clax_core::embedded::{EmbeddedFile, FileTree};

#[derive(rust_embed::RustEmbed)]
#[folder = "$CLAX_WEB_DIST_DIR"]
struct Dist;

/// `web/dist` as the tree [`clax_http::state::AppState::web`] holds.
pub struct WebUi;

impl FileTree for WebUi {
    fn get(&self, path: &str) -> Option<EmbeddedFile> {
        Dist::get(path).map(|f| EmbeddedFile {
            sha256: f.metadata.sha256_hash(),
            data: f.data,
        })
    }

    fn paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Dist::iter().map(|p| p.into_owned()).collect();
        out.sort();
        out
    }
}
