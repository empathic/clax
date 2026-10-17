//! Trees of files a process serves or installs: the web UI the daemon
//! serves, and later the extension and plugin trees the commands write.
//! A tree is either embedded in the binary or read from a directory
//! ([`DiskTree`]); the daemon and the commands read through [`FileTree`]
//! and never name the crate that embeds them.

use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// One file of a tree: its bytes and their SHA-256.
#[derive(Clone, Debug)]
pub struct EmbeddedFile {
    pub data: Cow<'static, [u8]>,
    pub sha256: [u8; 32],
}

/// A tree of files addressed by `/`-separated paths relative to its root.
pub trait FileTree: Send + Sync {
    /// The file at `path`, or none when the tree has no such file.
    fn get(&self, path: &str) -> Option<EmbeddedFile>;
    /// Every file's path, sorted.
    fn paths(&self) -> Vec<String>;
    /// The SHA-256 of the file at `path`. A tree that can answer without
    /// reading the bytes (an embed, or a directory whose file has not
    /// changed since it was last hashed) does.
    fn sha256(&self, path: &str) -> Option<[u8; 32]> {
        self.get(path).map(|f| f.sha256)
    }
}

/// When a file was last written and its size: a hash taken at one stamp
/// holds while the stamp is unchanged.
type Stamp = (Option<SystemTime>, u64);

/// A tree read from a directory on every call, so a rebuild of the
/// directory under a running process is served at once (a debug build's
/// `web/dist`, `clax serve --web-dist`, a test's scratch copy). Paths with
/// any component other than a plain name (`..`, a root) name no file.
pub struct DiskTree {
    root: PathBuf,
    hashes: Mutex<HashMap<String, (Stamp, [u8; 32])>>,
}

impl DiskTree {
    pub fn new(root: impl Into<PathBuf>) -> DiskTree {
        DiskTree {
            root: root.into(),
            hashes: Mutex::new(HashMap::new()),
        }
    }

    /// The directory the tree reads.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file `path` names under the root, when it is a plain relative
    /// path.
    fn file(&self, path: &str) -> Option<PathBuf> {
        let rel = Path::new(path);
        let plain = rel.components().next().is_some()
            && rel.components().all(|c| matches!(c, Component::Normal(_)));
        plain.then(|| self.root.join(rel))
    }
}

impl FileTree for DiskTree {
    fn get(&self, path: &str) -> Option<EmbeddedFile> {
        let data = std::fs::read(self.file(path)?).ok()?;
        let sha256 = Sha256::digest(&data).into();
        Some(EmbeddedFile {
            data: Cow::Owned(data),
            sha256,
        })
    }

    fn paths(&self) -> Vec<String> {
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let rel = format!("{prefix}{name}");
                match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path(), &format!("{rel}/"), out),
                    Ok(_) => out.push(rel),
                    Err(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, "", &mut out);
        out.sort();
        out
    }

    /// Rehashes the file only when its modification time or size changed
    /// since it was last hashed.
    fn sha256(&self, path: &str) -> Option<[u8; 32]> {
        let file = self.file(path)?;
        let m = std::fs::metadata(&file).ok()?;
        let stamp = (m.modified().ok(), m.len());
        let mut hashes = self.hashes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, h)) = hashes.get(path)
            && *s == stamp
        {
            return Some(*h);
        }
        let h = self.get(path)?.sha256;
        hashes.insert(path.to_string(), (stamp, h));
        Some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disk_tree_serves_plain_paths_under_its_root() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("_clax/bridge")).unwrap();
        std::fs::write(d.path().join("index.html"), "<p>i</p>").unwrap();
        std::fs::write(d.path().join("_clax/bridge/a.js"), "a").unwrap();
        let t = DiskTree::new(d.path());
        assert_eq!(&*t.get("index.html").unwrap().data, b"<p>i</p>");
        assert_eq!(t.paths(), ["_clax/bridge/a.js", "index.html"]);
        for bad in ["", "../index.html", "/etc/passwd", "_clax/../index.html"] {
            assert!(t.get(bad).is_none(), "{bad}");
        }
        assert!(t.get("missing.html").is_none());
    }

    #[test]
    fn a_disk_trees_hash_follows_the_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("b.js");
        std::fs::write(&p, "one").unwrap();
        let t = DiskTree::new(d.path());
        let a = t.sha256("b.js").unwrap();
        assert_eq!(a, <[u8; 32]>::from(Sha256::digest(b"one")));
        assert_eq!(t.sha256("b.js"), Some(a), "unchanged: the kept hash");
        std::fs::write(&p, "two, longer").unwrap();
        assert_eq!(
            t.sha256("b.js"),
            Some(<[u8; 32]>::from(Sha256::digest(b"two, longer")))
        );
        assert!(t.sha256("gone.js").is_none());
    }
}
