//! The Chrome extension's release build (`web/dist-extension`).

use rust_embed::RustEmbed;

/// `CLAX_EXTENSION_DIST_DIR`, set by `build.rs`: the checkout's
/// `web/dist-extension`, or Bazel's build of it.
#[derive(RustEmbed)]
#[folder = "$CLAX_EXTENSION_DIST_DIR"]
#[exclude = ".gitkeep"]
#[exclude = "key/*"]
#[exclude = "**/.DS_Store"]
struct Dist;

/// Every file of the extension: (path, contents), sorted. Empty when the
/// crate was built before `web/dist-extension` was.
pub fn extension_files() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Dist::iter()
        .map(|p| {
            let f = Dist::get(&p).expect("an embedded file lists itself");
            (p.to_string(), f.data.into_owned())
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_build_carries_no_placeholder_or_key() {
        assert!(
            Dist::iter().all(|p| p != ".gitkeep" && !p.starts_with("key/")),
            "{:?}",
            Dist::iter().collect::<Vec<_>>()
        );
    }

    /// `build.rs` names every directory a file is embedded from, so a
    /// rebuilt extension rebuilds the crate.
    #[test]
    fn the_build_script_watches_every_embedded_directory() {
        let watched: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/plugin-dirs.txt"))
            .lines()
            .collect();
        assert!(watched.contains(&"web/dist-extension"), "{watched:?}");
        for p in Dist::iter() {
            for d in std::path::Path::new(p.as_ref()).ancestors().skip(1) {
                let d = std::path::Path::new("web/dist-extension").join(d);
                let d = d.to_str().unwrap().trim_end_matches('/');
                assert!(watched.contains(&d), "{d} (for {p}) is not watched");
            }
        }
    }
}
