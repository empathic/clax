//! Artifact assets as objects in a bucket: R2 through the `ASSETS`
//! binding in a Worker, in both `REPO_STORE` modes (`js::R2Objects`); an
//! in-memory bucket in native tests ([`MemObjects`]).
//!
//! Asset `id` with extension `ext` of artifact `a` is the object
//! `assets/<a>/<id>.<ext>`, `id` and `ext` percent-encoded (every byte but
//! ASCII letters, digits, `-` and `_`), so distinct keys name distinct
//! objects and an artifact's assets are exactly the objects under
//! `assets/<a>/`. Assets are unversioned: a put replaces the object.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;

use clax_content_git::AssetStore;
use clax_core::ArtifactId;
use clax_core::content::AssetKey;
use clax_git::{GitError, MaybeSend};

/// One page of a listing: its keys, and the cursor of the next page when
/// there is one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub keys: Vec<String>,
    pub cursor: Option<String>,
}

/// The most keys one [`Objects::delete`] call takes (R2's bound).
pub const DELETE_BATCH: usize = 1000;

/// The bucket calls the asset store makes. Errors are the bucket's
/// messages.
pub trait Objects: Send + Sync {
    /// Stores `bytes` at `key`, replacing what was there.
    fn put(&self, key: &str, bytes: &[u8]) -> impl Future<Output = Result<(), String>> + MaybeSend;
    /// The bytes at `key`, or `None`.
    fn get(&self, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + MaybeSend;
    /// Removes `keys` (at most [`DELETE_BATCH`]); a missing key is no error.
    fn delete(&self, keys: &[String]) -> impl Future<Output = Result<(), String>> + MaybeSend;
    /// The keys under `prefix`, a page at a time from `cursor`.
    fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<Page, String>> + MaybeSend;
}

fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `assets/<artifact>/`: the prefix of every object of `artifact`.
pub fn artifact_prefix(artifact: &ArtifactId) -> String {
    format!("assets/{}/", artifact.as_str())
}

/// The object an asset is kept at; see the [module docs](self).
pub fn object_key(key: &AssetKey) -> String {
    format!(
        "{}{}.{}",
        artifact_prefix(&key.artifact),
        pct(&key.id),
        pct(&key.ext)
    )
}

fn unavailable(what: &str, e: String) -> clax_core::CoreError {
    clax_content_git::core_error(GitError::RepoUnavailable(format!(
        "The asset bucket failed to {what}: {e}"
    )))
}

/// Artifact assets in a bucket.
pub struct BucketAssets<O> {
    pub objects: O,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<O: Objects> AssetStore for BucketAssets<O> {
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> clax_core::Result<()> {
        self.objects
            .put(&object_key(key), bytes)
            .await
            .map_err(|e| unavailable("store an asset", e))
    }

    async fn read_asset(&self, key: &AssetKey) -> clax_core::Result<Option<Vec<u8>>> {
        self.objects
            .get(&object_key(key))
            .await
            .map_err(|e| unavailable("read an asset", e))
    }

    async fn delete_asset(&self, key: &AssetKey) -> clax_core::Result<()> {
        self.objects
            .delete(&[object_key(key)])
            .await
            .map_err(|e| unavailable("delete an asset", e))
    }

    /// Lists every key under the artifact's prefix first, then deletes
    /// them in batches, so the listing's cursor never runs over keys that
    /// are being deleted.
    async fn delete_assets(&self, id: &ArtifactId) -> clax_core::Result<()> {
        let prefix = artifact_prefix(id);
        let mut keys = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .objects
                .list(&prefix, cursor.as_deref())
                .await
                .map_err(|e| unavailable("list assets", e))?;
            keys.extend(page.keys);
            match page.cursor {
                Some(c) if Some(&c) != cursor.as_ref() => cursor = Some(c),
                _ => break,
            }
        }
        for batch in keys.chunks(DELETE_BATCH) {
            self.objects
                .delete(batch)
                .await
                .map_err(|e| unavailable("delete assets", e))?;
        }
        Ok(())
    }
}

/// A bucket in memory, listing `page` keys at a time.
pub struct MemObjects {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    page: usize,
}

impl MemObjects {
    pub fn new(page: usize) -> MemObjects {
        MemObjects {
            objects: Mutex::default(),
            page: page.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.objects.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every key held, in order.
    pub fn keys(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }
}

impl Objects for MemObjects {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        self.lock().insert(key.to_string(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.lock().get(key).cloned())
    }

    async fn delete(&self, keys: &[String]) -> Result<(), String> {
        if keys.len() > DELETE_BATCH {
            return Err(format!("more than {DELETE_BATCH} keys in one delete"));
        }
        let mut m = self.lock();
        for k in keys {
            m.remove(k);
        }
        Ok(())
    }

    /// The cursor is the last key of the page.
    async fn list(&self, prefix: &str, cursor: Option<&str>) -> Result<Page, String> {
        let m = self.lock();
        let keys: Vec<String> = m
            .keys()
            .filter(|k| k.starts_with(prefix) && cursor.is_none_or(|c| k.as_str() > c))
            .take(self.page + 1)
            .cloned()
            .collect();
        let more = keys.len() > self.page;
        let keys: Vec<String> = keys.into_iter().take(self.page).collect();
        Ok(Page {
            cursor: more.then(|| keys.last().cloned()).flatten(),
            keys,
        })
    }
}
