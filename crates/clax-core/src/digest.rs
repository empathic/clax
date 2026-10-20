//! The digest of an app's files that the render gate compares (spec
//! 2026-10-19-in-process-core-design §8.2): the content hash
//! `apps/clax_app/lib/bundle/content_hash.dart` computes (`contentHash`),
//! over every file but `icon.svg`. The app records a draft's clean render
//! under it, and a publish step naming that draft must publish files with
//! the same digest.

use std::collections::BTreeMap;

use crate::audit::{content_manifest_sha256, sha256_hex};
use crate::model::FileMeta;

/// The one file the digest leaves out: the icon changes no render.
pub const ICON: &str = "icon.svg";

/// `sha256:` and the hex SHA-256 of one line `<path>\0<sha256 hex of the
/// bytes>\0<size>\n` per file but `icon.svg`, in byte order of path: the
/// digest `content_hash.dart`'s `contentHash` computes for the same files.
pub fn files_digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let metas: BTreeMap<String, FileMeta> = files
        .into_iter()
        .map(|(path, bytes)| {
            (
                path.to_string(),
                FileMeta {
                    content_type: String::new(),
                    size: bytes.len() as u64,
                    sha256: Some(sha256_hex(bytes)),
                },
            )
        })
        .collect();
    manifest_digest(&metas).expect("every file is hashed")
}

/// [`files_digest`] of files known by their hashes and sizes; `None` when
/// one of them (but `icon.svg`) has no hash.
pub fn manifest_digest(files: &BTreeMap<String, FileMeta>) -> Option<String> {
    if !files.contains_key(ICON) {
        return content_manifest_sha256(files);
    }
    let mut rest = files.clone();
    rest.remove(ICON);
    content_manifest_sha256(&rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector `apps/clax_app/test/bundle/files_digest_test.dart`
    /// checks `contentHash` against.
    const FILES: [(&str, &[u8]); 4] = [
        ("index.html", b"<!doctype html><title>Budget</title>"),
        ("app.js", b"let n = 1;\n"),
        ("data/caf\u{e9}.json", br#"{"n":1}"#),
        (
            "icon.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"/>",
        ),
    ];
    const DIGEST: &str = "sha256:f7822984f37e5debcd4ae329c8f7569f2518ded0ee8b8ca3d88266e381ff1ab2";

    #[test]
    fn the_digest_is_content_hash_dart_s_without_the_icon() {
        assert_eq!(files_digest(FILES), DIGEST);
        assert_eq!(files_digest(FILES.into_iter().take(3)), DIGEST);
        assert_ne!(files_digest(FILES.into_iter().skip(1)), DIGEST);
    }

    #[test]
    fn a_file_without_its_hash_has_no_digest() {
        let mut files = BTreeMap::from([(
            "index.html".to_string(),
            FileMeta {
                content_type: "text/html".into(),
                size: 1,
                sha256: None,
            },
        )]);
        assert_eq!(manifest_digest(&files), None);
        files.clear();
        assert_eq!(manifest_digest(&files), Some(files_digest([])));
    }
}
