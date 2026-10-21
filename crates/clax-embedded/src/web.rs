//! The web UI (`web/dist`): the shell the daemon serves and the bridge
//! pages load.

use crate::File;

/// `CLAX_WEB_DIST_DIR`, set by `build.rs`: the checkout's `web/dist`, or
/// Bazel's build of it.
#[derive(rust_embed::RustEmbed)]
#[folder = "$CLAX_WEB_DIST_DIR"]
struct Assets;

/// The file at `path` (relative to `web/dist`), or none.
pub fn web_ui(path: &str) -> Option<File> {
    Assets::get(path).map(|f| File {
        sha256: f.metadata.sha256_hash(),
        data: f.data,
    })
}

/// Every file's path, sorted.
pub fn web_ui_paths() -> Vec<String> {
    let mut out: Vec<String> = Assets::iter().map(|p| p.into_owned()).collect();
    out.sort();
    out
}

/// The directory a build that reads its web UI from disk reads (a debug
/// build that does not embed it): the binary serves it as a directory, so
/// a rebuild under a running daemon is served at once. None when the web
/// UI is embedded. A build that embeds names no directory: Bazel's is the
/// build action's sandbox.
pub fn web_ui_dir() -> Option<&'static str> {
    #[cfg(all(debug_assertions, not(clax_embedded)))]
    let dir = Some(env!("CLAX_WEB_DIST_DIR"));
    #[cfg(any(not(debug_assertions), clax_embedded))]
    let dir = None;
    dir
}

#[cfg(test)]
mod watched {
    /// `build.rs` names every directory a web UI file comes from, so a new
    /// file in the web build rebuilds the crate.
    #[test]
    fn the_build_script_watches_every_web_ui_directory() {
        let watched: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/plugin-dirs.txt"))
            .lines()
            .collect();
        for p in super::web_ui_paths() {
            for d in std::path::Path::new(&p).ancestors().skip(1) {
                let d = std::path::Path::new("web/dist").join(d);
                let d = d.to_str().unwrap().trim_end_matches('/');
                assert!(watched.contains(&d), "{d} (for {p}) is not watched");
            }
        }
    }
}

/// Only a build that embeds its web UI has something to check: a debug
/// build that reads it from disk compiles no tests here.
#[cfg(all(test, any(not(debug_assertions), clax_embedded)))]
mod tests {
    use super::*;

    /// A build that embeds its web UI carries the bridge with its hash, and
    /// the app's transport, which an embedded core's shell documents load.
    #[test]
    fn an_embedded_build_carries_the_bridge() {
        use sha2::Digest;
        let f = web_ui("_clax/bridge.js").expect("the bridge is built");
        assert_eq!(f.sha256, <[u8; 32]>::from(sha2::Sha256::digest(&f.data)));
        assert!(web_ui_paths().iter().any(|p| p == "artifact.html"));
        assert!(web_ui_paths().iter().any(|p| p == "_clax/transport.js"));
        assert_eq!(web_ui_dir(), None);
    }

    /// No embedded file carries its file's times, which a rebuild of the
    /// web UI changes without changing its content: the crate compiles to
    /// the same metadata from the same inputs, so its `.rmeta` and its
    /// `.rlib`, two separate compiles under Bazel's pipelining, agree.
    #[test]
    fn embedded_files_carry_no_file_times() {
        for path in Assets::iter() {
            let meta = Assets::get(&path).expect("listed").metadata;
            assert_eq!(meta.last_modified(), Some(0), "{path}");
            assert_eq!(meta.created(), Some(0), "{path}");
        }
    }
}
