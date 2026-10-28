//! The Clax Quickstart page the web build ships under `quickstart/` (spec
//! 2026-10-26-quickstart-onboarding-design §9): read from the web UI's
//! tree as one [`Release`] named by its files, and turned into the publish
//! request that creates or updates the gallery's copy.

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::embedded::FileTree;
use crate::error::CoreError;
use crate::publish::{Encoding, FileInput, PublishRequest};

/// Where the web UI's tree keeps the page.
pub const PREFIX: &str = "quickstart/";

/// The title of the page's artifact.
pub const TITLE: &str = "Start here: Clax in five minutes";

/// The page's entry file, relative to [`PREFIX`].
const PAGE: &str = "index.html";

/// The page's manifest, relative to [`PREFIX`].
const MANIFEST: &str = "clax.json";

/// One build of the quickstart page: its files (paths relative to
/// [`PREFIX`]) and the ID their bytes and names give.
#[derive(Clone, Debug)]
pub struct Release {
    /// The first 12 lowercase hex digits of the SHA-256 over every path and
    /// its bytes, in path order.
    pub id: String,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Release {
    /// The release `web` carries under [`PREFIX`], or none when it has no
    /// `quickstart/index.html` (a build made without the page).
    pub fn from_tree(web: &dyn FileTree) -> Option<Release> {
        let mut files = BTreeMap::new();
        for path in web.paths() {
            let Some(rel) = path.strip_prefix(PREFIX) else {
                continue;
            };
            if let Some(f) = web.get(&path) {
                files.insert(rel.to_string(), f.data.into_owned());
            }
        }
        if !files.contains_key(PAGE) {
            return None;
        }
        let mut hash = Sha256::new();
        for (path, bytes) in &files {
            hash.update((path.len() as u64).to_be_bytes());
            hash.update(path.as_bytes());
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
        let id = hash.finalize()[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Some(Release { id, files })
    }

    /// The label of the version this release publishes.
    pub fn label(&self) -> String {
        format!("Quickstart {}", self.id)
    }

    /// The `capabilities` of the release's `clax.json`, or none when it
    /// has no manifest or the manifest declares none.
    ///
    /// # Errors
    /// `Invalid { code: "invalid_manifest" }` when `clax.json` is not a JSON
    /// object.
    pub fn capabilities(&self) -> crate::Result<Option<Value>> {
        let Some(bytes) = self.files.get(MANIFEST) else {
            return Ok(None);
        };
        match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(mut m)) => Ok(m.remove("capabilities")),
            Ok(_) => Err(CoreError::invalid(
                "invalid_manifest",
                "clax.json must be a JSON object",
            )),
            Err(e) => Err(CoreError::invalid(
                "invalid_manifest",
                format!("clax.json is not valid JSON: {e}"),
            )),
        }
    }

    /// The publish request that makes the artifact hold exactly this
    /// release: every file as base64, a removal for each path of
    /// `previous` the release lacks, this release's label and
    /// capabilities, and [`TITLE`] when `create` is set.
    ///
    /// # Errors
    /// As [`Release::capabilities`].
    pub fn request(&self, previous: &[String], create: bool) -> crate::Result<PublishRequest> {
        let mut files: BTreeMap<String, Option<FileInput>> = previous
            .iter()
            .filter(|p| !self.files.contains_key(p.as_str()))
            .map(|p| (p.clone(), None))
            .collect();
        for (path, bytes) in &self.files {
            files.insert(
                path.clone(),
                Some(FileInput {
                    content: base64::engine::general_purpose::STANDARD.encode(bytes),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            );
        }
        Ok(PublishRequest {
            title: create.then(|| TITLE.to_string()),
            label: Some(self.label()),
            capabilities: self.capabilities()?,
            files,
            ..PublishRequest::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::{DiskTree, MemTree};
    use serde_json::json;

    fn tree(files: &[(&str, &str)]) -> MemTree {
        MemTree(
            files
                .iter()
                .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
                .collect(),
        )
    }

    fn base() -> Vec<(&'static str, &'static str)> {
        vec![
            ("quickstart/index.html", "<p>a</p>"),
            ("quickstart/clax.json", r#"{"capabilities":{"db":{}}}"#),
            ("quickstart/style.css", "p{}"),
        ]
    }

    fn id(files: &[(&str, &str)]) -> String {
        Release::from_tree(&tree(files)).unwrap().id
    }

    #[test]
    fn a_release_is_named_by_its_files() {
        let a = id(&base());
        assert_eq!(a, id(&base()));
        assert_eq!(a.len(), 12);
        assert!(a.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

        let mut changed = base();
        changed[2].1 = "p{ }";
        assert_ne!(a, id(&changed));

        let mut renamed = base();
        renamed[2].0 = "quickstart/other.css";
        assert_ne!(a, id(&renamed));

        let mut outside = base();
        outside.push(("assets/app.js", "x"));
        outside.push(("index.html", "<p>shell</p>"));
        assert_eq!(a, id(&outside));
    }

    #[test]
    fn a_tree_without_the_page_has_no_release() {
        assert!(Release::from_tree(&tree(&[("quickstart/style.css", "p{}")])).is_none());
        assert!(Release::from_tree(&tree(&[("index.html", "<p>")])).is_none());
    }

    #[test]
    fn a_release_request_removes_what_it_no_longer_ships() {
        let r = Release::from_tree(&tree(&base())).unwrap();
        let req = r
            .request(&["old.js".to_string(), "index.html".to_string()], false)
            .unwrap();
        assert!(req.files["old.js"].is_none());
        assert!(req.files["index.html"].is_some());
        assert_eq!(req.title, None);
        assert_eq!(req.label, Some(r.label()));
        assert_eq!(req.capabilities, Some(json!({"db": {}})));
        let create = r.request(&[], true).unwrap();
        assert_eq!(create.title.as_deref(), Some(TITLE));
    }

    #[test]
    fn a_malformed_manifest_is_invalid() {
        let mut files = base();
        files[1].1 = "{";
        let r = Release::from_tree(&tree(&files)).unwrap();
        let e = r.capabilities().unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "invalid_manifest",
                ..
            }
        ));
    }

    #[test]
    fn the_shipped_request_validates() {
        let dist = clax_fake_exe::repo().join("web/dist");
        if !dist.join("quickstart").is_dir() {
            eprintln!("skipped: {} has no quickstart/", dist.display());
            return;
        }
        let release = Release::from_tree(&DiskTree::new(dist)).unwrap();
        crate::publish::validate(release.request(&[], true).unwrap()).unwrap();
    }
}
