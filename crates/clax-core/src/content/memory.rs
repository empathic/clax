//! [`MemContent`]: artifact content in memory, for tests and for hosts
//! that keep nothing on disk.

use super::{AssetKey, ContentStore, VersionWrite, missing, present};
use crate::{ArtifactId, Result};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// A version's files by path.
type Files = BTreeMap<String, Arc<Vec<u8>>>;

#[derive(Default)]
struct Inner {
    versions: HashMap<(String, u32), Files>,
    assets: HashMap<AssetKey, Arc<Vec<u8>>>,
}

/// Artifact content in memory. A carried or copied file shares its bytes
/// with the version it comes from.
#[derive(Default)]
pub struct MemContent {
    inner: Mutex<Inner>,
}

impl MemContent {
    pub fn new() -> MemContent {
        MemContent::default()
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The versions held, as `(artifact ID, n)`, sorted.
    pub fn versions(&self) -> Vec<(String, u32)> {
        let mut v: Vec<_> = self.inner().versions.keys().cloned().collect();
        v.sort();
        v
    }

    /// The number of assets held.
    pub fn asset_count(&self) -> usize {
        self.inner().assets.len()
    }
}

fn key(id: &ArtifactId, n: u32) -> (String, u32) {
    (id.as_str().to_string(), n)
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl ContentStore for MemContent {
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .inner()
            .versions
            .get(&key(id, n))
            .and_then(|f| f.get(path))
            .map(|b| b.as_ref().clone()))
    }

    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        let mut g = self.inner();
        if g.versions.contains_key(&key(id, n)) {
            return Err(present(id, n));
        }
        let mut files = Files::new();
        if !write.carry.is_empty() {
            let prev = g.versions.get(&key(id, n - 1));
            for path in &write.carry {
                let bytes = prev
                    .and_then(|f| f.get(*path))
                    .ok_or_else(|| missing(id, n - 1, path))?;
                files.insert(path.to_string(), bytes.clone());
            }
        }
        for f in &write.put {
            files.insert(f.path.to_string(), Arc::new(f.bytes.to_vec()));
        }
        g.versions.insert(key(id, n), files);
        Ok(())
    }

    async fn copy_version(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        to_n: u32,
    ) -> Result<()> {
        let mut g = self.inner();
        if g.versions.contains_key(&key(to, to_n)) {
            return Err(present(to, to_n));
        }
        let src = g.versions.get(&key(from, from_n));
        let mut files = Files::new();
        for path in paths {
            let bytes = src
                .and_then(|f| f.get(*path))
                .ok_or_else(|| missing(from, from_n, path))?;
            files.insert(path.to_string(), bytes.clone());
        }
        g.versions.insert(key(to, to_n), files);
        Ok(())
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        self.inner().versions.remove(&key(id, n));
        Ok(())
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        let mut g = self.inner();
        g.versions.retain(|(a, _), _| a != id.as_str());
        g.assets.retain(|k, _| k.artifact != *id);
        Ok(())
    }

    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        self.inner()
            .assets
            .insert(key.clone(), Arc::new(bytes.to_vec()));
        Ok(())
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        Ok(self.inner().assets.get(key).map(|b| b.as_ref().clone()))
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        self.inner().assets.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_holds() {
        let c = MemContent::new();
        crate::content::wait(crate::content::contract::run(&c));
    }

    /// [`MemContent`] with the trait's own `copy_version`.
    struct DefaultCopy(MemContent);

    #[async_trait::async_trait]
    impl ContentStore for DefaultCopy {
        async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
            self.0.read_file(id, n, path).await
        }
        async fn write_version(
            &self,
            id: &ArtifactId,
            n: u32,
            write: &VersionWrite<'_>,
        ) -> Result<()> {
            self.0.write_version(id, n, write).await
        }
        async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
            self.0.remove_version(id, n).await
        }
        async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
            self.0.delete_artifact(id).await
        }
        async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
            self.0.put_asset(key, bytes).await
        }
        async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
            self.0.read_asset(key).await
        }
        async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
            self.0.delete_asset(key).await
        }
    }

    #[test]
    fn the_default_copy_keeps_the_contract() {
        let c = DefaultCopy(MemContent::new());
        crate::content::wait(crate::content::contract::run(&c));
    }
}
