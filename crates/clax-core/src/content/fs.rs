//! [`FsContent`]: artifact content in files under the Clax home.
//!
//! The layout, under `artifacts/<artifact ID>/`:
//!
//! - `versions/<n>/index.html`: a version's `index.html`;
//! - `versions/<n>/files/<path>`: each other file of it;
//! - `versions/.tmp-<ULID>/`: a version being written, renamed to
//!   `versions/<n>` once whole (a leftover one is an interrupted write);
//! - `assets/<asset ID>.<ext>`: an asset, written as `<…>.<ext>.tmp` and
//!   renamed into place;
//! - `clips/<thread ID>.png`: a thread's clip image, written as
//!   `<…>.png.tmp` and renamed into place.
//!
//! Deleting an artifact removes its whole directory.
//!
//! A thread ID, asset ID or asset extension is a file name, never a path: one
//! outside `[A-Za-z0-9_-]` (or empty) names no file, so nothing outside the
//! artifact's `assets/` and `clips/` is read, written or removed.

use super::{AssetKey, ContentStore, VersionWrite, missing, present, superseded};
use crate::publish::{INDEX, check_path};
use crate::{ArtifactId, Home, Result, new_ulid};
use std::path::{Path, PathBuf};

/// Artifact content in the home's `artifacts/` directory.
#[derive(Clone, Debug)]
pub struct FsContent {
    home: Home,
}

impl FsContent {
    pub fn new(home: Home) -> FsContent {
        FsContent { home }
    }
}

/// Where version `n` of `id` keeps the file `path`: `index.html` in the
/// version's directory, any other file under its `files/`.
pub fn file_path(home: &Home, id: &ArtifactId, n: u32, path: &str) -> PathBuf {
    file_in(&home.version_dir(id, n), path)
}

/// Where the asset `key` keeps its bytes.
pub fn asset_path(home: &Home, key: &AssetKey) -> PathBuf {
    home.assets_dir(&key.artifact)
        .join(format!("{}.{}", key.id, key.ext))
}

/// Whether `s` can stand in a file name as a thread ID, asset ID or asset
/// extension: non-empty, ASCII letters, digits, `_` and `-` only.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `key` names an asset file ([`is_name`]).
fn is_asset_name(key: &AssetKey) -> bool {
    is_name(&key.id) && is_name(&key.ext)
}

/// The `invalid_path` refusal of a name [`is_name`] does not accept.
fn bad_name(what: &str, name: &str) -> crate::CoreError {
    crate::CoreError::invalid("invalid_path", format!("'{name}' is not a valid {what}"))
}

/// `path` in the version directory `dir`.
fn file_in(dir: &Path, path: &str) -> PathBuf {
    if path == INDEX {
        dir.join(INDEX)
    } else {
        dir.join("files").join(path)
    }
}

/// The bytes at `p`, or `None` when there is no file there.
fn read_if_present(p: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if is_absent(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Whether `e` says the path is not there: missing, or a directory on the
/// way is a file.
fn is_absent(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn remove_dir_if_present(p: &Path) -> Result<()> {
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// A version directory being written; removed on drop unless renamed away.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl FsContent {
    /// A fresh staging directory for a version of `id`.
    fn stage(&self, id: &ArtifactId) -> Result<Staging> {
        let versions = self.home.artifact_dir(id).join("versions");
        std::fs::create_dir_all(&versions)?;
        let staging = Staging(versions.join(format!(".tmp-{}", new_ulid())));
        std::fs::create_dir_all(staging.0.join("files"))?;
        Ok(staging)
    }

    /// Whether version `n` of `id` is present: its directory has its
    /// `index.html`, as every version Clax writes does.
    fn has_version(&self, id: &ArtifactId, n: u32) -> bool {
        self.home.version_dir(id, n).join(INDEX).exists()
    }

    /// The newest version of `id` above `n`, when one is present.
    fn above(&self, id: &ArtifactId, n: u32) -> Result<Option<u32>> {
        let versions = self.home.artifact_dir(id).join("versions");
        let entries = match std::fs::read_dir(&versions) {
            Ok(entries) => entries,
            Err(e) if is_absent(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut newest = None;
        for entry in entries {
            let name = entry?.file_name();
            let Some(m) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if m > n && newest.is_none_or(|k| m > k) && self.has_version(id, m) {
                newest = Some(m);
            }
        }
        Ok(newest)
    }

    /// Fails when version `n` of `id`, or a version above it, is present.
    fn check_new(&self, id: &ArtifactId, n: u32) -> Result<()> {
        if self.has_version(id, n) {
            return Err(present(id, n));
        }
        match self.above(id, n)? {
            Some(m) => Err(superseded(id, n, m)),
            None => Ok(()),
        }
    }

    /// Renames `staging` to version `n` of `id`. A version directory with
    /// anything in it is never replaced.
    fn place(&self, staging: Staging, id: &ArtifactId, n: u32) -> Result<()> {
        let vdir = self.home.version_dir(id, n);
        match std::fs::rename(&staging.0, &vdir) {
            Ok(()) => Ok(()),
            Err(_) if vdir.exists() => Err(present(id, n)),
            Err(e) => Err(e.into()),
        }
    }
}

/// Copies `from` to `to`, making `to`'s directories.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).map(|_| ())
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl ContentStore for FsContent {
    /// A path that is not a safe relative one ([`check_path`]) names no
    /// file: nothing outside the version's directory is read.
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        if check_path(path).is_err() {
            return Ok(None);
        }
        read_if_present(&file_path(&self.home, id, n, path))
    }

    /// Refuses (`invalid_path`) a version naming a path that is not a safe
    /// relative one ([`check_path`]) before anything is written.
    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        for path in write
            .carry
            .iter()
            .copied()
            .chain(write.put.iter().map(|f| f.path))
        {
            check_path(path)?;
        }
        self.check_new(id, n)?;
        let staging = self.stage(id)?;
        if !write.carry.is_empty() {
            let prev = self.home.version_dir(id, n - 1);
            for path in &write.carry {
                match copy_file(&file_in(&prev, path), &file_in(&staging.0, path)) {
                    Ok(()) => {}
                    Err(e) if is_absent(&e) => return Err(missing(id, n - 1, path)),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        for f in &write.put {
            let dest = file_in(&staging.0, f.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(dest, f.bytes)?;
        }
        self.place(staging, id, n)
    }

    async fn copy_version(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        to_n: u32,
    ) -> Result<()> {
        for path in paths {
            check_path(path)?;
        }
        self.check_new(to, to_n)?;
        let staging = self.stage(to)?;
        let src = self.home.version_dir(from, from_n);
        for path in paths {
            match copy_file(&file_in(&src, path), &file_in(&staging.0, path)) {
                Ok(()) => {}
                Err(e) if is_absent(&e) => return Err(missing(from, from_n, path)),
                Err(e) => return Err(e.into()),
            }
        }
        self.place(staging, to, to_n)
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        if self.has_version(id, n)
            && let Some(m) = self.above(id, n)?
        {
            return Err(superseded(id, n, m));
        }
        remove_dir_if_present(&self.home.version_dir(id, n))
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        remove_dir_if_present(&self.home.artifact_dir(id))
    }

    /// Refuses (`invalid_path`) a key whose ID or extension is not a name.
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        if !is_asset_name(key) {
            return Err(bad_name("asset key", &format!("{}.{}", key.id, key.ext)));
        }
        std::fs::create_dir_all(self.home.assets_dir(&key.artifact))?;
        let final_path = asset_path(&self.home, key);
        let mut tmp = final_path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, bytes)?;
        if let Err(e) = std::fs::rename(&tmp, &final_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        if !is_asset_name(key) {
            return Ok(None);
        }
        read_if_present(&asset_path(&self.home, key))
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        if !is_asset_name(key) {
            return Ok(());
        }
        remove_file_if_present(&asset_path(&self.home, key))
    }

    /// Refuses (`invalid_path`) a thread ID that is not a name.
    async fn put_clip(&self, id: &ArtifactId, thread: &str, bytes: &[u8]) -> Result<()> {
        if !is_name(thread) {
            return Err(bad_name("thread ID", thread));
        }
        std::fs::create_dir_all(self.home.clips_dir(id))?;
        let path = self.home.clip_path(id, thread);
        let tmp = path.with_extension("png.tmp");
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    async fn read_clip(&self, id: &ArtifactId, thread: &str) -> Result<Option<Vec<u8>>> {
        if !is_name(thread) {
            return Ok(None);
        }
        read_if_present(&self.home.clip_path(id, thread))
    }

    async fn delete_clip(&self, id: &ArtifactId, thread: &str) -> Result<()> {
        if !is_name(thread) {
            return Ok(());
        }
        remove_file_if_present(&self.home.clip_path(id, thread))
    }

    /// The file when it is there and `path` is a safe relative one.
    fn local_file(&self, id: &ArtifactId, n: u32, path: &str) -> Option<PathBuf> {
        check_path(path).ok()?;
        Some(file_path(&self.home, id, n, path)).filter(|p| p.is_file())
    }

    /// The clip's file when it is there and `thread` is a name.
    fn local_clip(&self, id: &ArtifactId, thread: &str) -> Option<PathBuf> {
        if !is_name(thread) {
            return None;
        }
        Some(self.home.clip_path(id, thread)).filter(|p| p.is_file())
    }
}

fn remove_file_if_present(p: &Path) -> Result<()> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{NewFile, wait};

    fn content() -> (tempfile::TempDir, Home, FsContent) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        (dir, home.clone(), FsContent::new(home))
    }

    #[test]
    fn the_contract_holds() {
        let (_d, _h, c) = content();
        wait(crate::content::contract::run(&c));
    }

    #[test]
    fn versions_and_assets_keep_the_home_layout() {
        let (_d, home, c) = content();
        let id = ArtifactId::generate();
        let put = |path, bytes| NewFile {
            path,
            bytes,
            content_type: "text/plain",
        };
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![put("index.html", b"<p>1"), put("js/app.js", b"go()")],
            ..Default::default()
        };
        wait(c.write_version(&id, 1, &w)).unwrap();
        let v1 = home.artifact_dir(&id).join("versions/1");
        assert_eq!(std::fs::read(v1.join("index.html")).unwrap(), b"<p>1");
        assert_eq!(std::fs::read(v1.join("files/js/app.js")).unwrap(), b"go()");
        let key = AssetKey {
            artifact: id.clone(),
            id: new_ulid(),
            ext: "png".into(),
        };
        wait(c.put_asset(&key, b"png")).unwrap();
        let assets: Vec<String> = std::fs::read_dir(home.artifact_dir(&id).join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(assets, [format!("{}.png", key.id)]);
        let versions: Vec<String> = std::fs::read_dir(home.artifact_dir(&id).join("versions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(versions, ["1"], "no staging directory is left");
    }

    #[test]
    fn paths_that_leave_the_version_read_nothing_and_write_nothing() {
        let (_d, home, c) = content();
        let id = ArtifactId::generate();
        let put = |path, bytes| NewFile {
            path,
            bytes,
            content_type: "text/plain",
        };
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![put("index.html", b"<p>1")],
            ..Default::default()
        };
        wait(c.write_version(&id, 1, &w)).unwrap();
        // `files/../index.html` is the version's own index.html on disk.
        for bad in ["../index.html", "/etc/hosts", "a//b", "a/./b", ""] {
            assert_eq!(wait(c.read_file(&id, 1, bad)).unwrap(), None, "{bad}");
            assert_eq!(c.local_file(&id, 1, bad), None, "{bad}");
        }
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![put("index.html", b"<p>2"), put("../../escape", b"x")],
            ..Default::default()
        };
        let e = wait(c.write_version(&id, 2, &w)).unwrap_err();
        assert!(e.to_string().contains("not a safe relative path"), "{e}");
        let w = VersionWrite {
            carry: vec!["../index.html"],
            put: vec![put("index.html", b"<p>2")],
            ..Default::default()
        };
        assert!(wait(c.write_version(&id, 2, &w)).is_err());
        let e = wait(c.copy_version(&id, 1, &["../index.html"], &id, 2)).unwrap_err();
        assert!(e.to_string().contains("not a safe relative path"), "{e}");
        assert!(!home.artifact_dir(&id).join("escape").exists());
        assert!(!c.has_version(&id, 2));
    }

    #[test]
    fn thread_and_asset_names_that_leave_their_directory_name_nothing() {
        let (_d, home, c) = content();
        let id = ArtifactId::generate();
        let other = ArtifactId::generate();
        // A file one level up from `clips/` and `assets/`, and one in
        // another artifact's clips: what `../` names would reach.
        std::fs::create_dir_all(home.clips_dir(&other)).unwrap();
        std::fs::create_dir_all(home.artifact_dir(&id)).unwrap();
        std::fs::write(home.artifact_dir(&id).join("x.png"), b"up").unwrap();
        std::fs::write(home.clips_dir(&other).join("t.png"), b"other").unwrap();
        let escapes = [
            "../x".to_string(),
            format!("../../{}/clips/t", other.as_str()),
            "a/b".into(),
            "".into(),
            "a.b".into(),
        ];
        for bad in &escapes {
            assert_eq!(wait(c.read_clip(&id, bad)).unwrap(), None, "{bad}");
            assert_eq!(c.local_clip(&id, bad), None, "{bad}");
            wait(c.delete_clip(&id, bad)).unwrap();
            let e = wait(c.put_clip(&id, bad, b"png")).unwrap_err();
            assert!(e.to_string().contains("not a valid thread ID"), "{e}");
        }
        for (aid, ext) in [("../x", "png"), ("x", "png/../../x"), ("x", "")] {
            let key = AssetKey {
                artifact: id.clone(),
                id: aid.into(),
                ext: ext.into(),
            };
            assert_eq!(wait(c.read_asset(&key)).unwrap(), None, "{aid}.{ext}");
            wait(c.delete_asset(&key)).unwrap();
            assert!(wait(c.put_asset(&key, b"a")).is_err(), "{aid}.{ext}");
        }
        assert_eq!(
            std::fs::read(home.artifact_dir(&id).join("x.png")).unwrap(),
            b"up"
        );
        assert_eq!(
            std::fs::read(home.clips_dir(&other).join("t.png")).unwrap(),
            b"other"
        );
    }

    #[test]
    fn local_paths_name_only_files_that_are_there() {
        let (_d, _home, c) = content();
        let id = ArtifactId::generate();
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![NewFile {
                path: "index.html",
                bytes: b"<p>1",
                content_type: "text/html",
            }],
            ..Default::default()
        };
        wait(c.write_version(&id, 1, &w)).unwrap();
        assert!(c.local_file(&id, 1, "index.html").is_some());
        assert_eq!(c.local_file(&id, 1, "gone.css"), None);
        assert_eq!(c.local_file(&id, 2, "index.html"), None);
        assert_eq!(c.local_clip(&id, "01M4X6ZZJ3CCAQC3SVGKDD8NTX"), None);
        wait(c.put_clip(&id, "01M4X6ZZJ3CCAQC3SVGKDD8NTX", b"png")).unwrap();
        assert!(c.local_clip(&id, "01M4X6ZZJ3CCAQC3SVGKDD8NTX").is_some());
    }

    #[test]
    fn a_version_directory_with_files_is_never_replaced() {
        let (_d, home, c) = content();
        let id = ArtifactId::generate();
        let stray = home.version_dir(&id, 1);
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("stray"), "x").unwrap();
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![NewFile {
                path: "index.html",
                bytes: b"<p>1",
                content_type: "text/html",
            }],
            ..Default::default()
        };
        let e = wait(c.write_version(&id, 1, &w)).unwrap_err();
        assert!(
            matches!(&e, crate::CoreError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists),
            "{e:?}"
        );
        assert!(stray.join("stray").exists());
        assert!(!stray.join("index.html").exists());
    }
}
