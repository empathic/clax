//! An instance's artifact content in PostgreSQL: [`PgContent`], its
//! [`ContentStore`].
//!
//! A version is a row of `clax_fleet_versions` (with its step mark) and a
//! row of `clax_fleet_version_files` per path, naming the SHA-256 of its
//! bytes. The bytes are kept once per artifact in `clax_fleet_blobs`, so a
//! carried file, the common case, writes no bytes: the new version's row
//! names the same blob. A blob no version of its artifact names any more
//! is deleted by the same transaction that left it so. Every change of an
//! artifact runs under the store's content lock for that artifact, in the
//! one process that owns the instance, so no write of the artifact races
//! that deletion. Assets and clips are rows of `clax_fleet_assets`, a clip
//! as the asset [`clax_core::content::clip_key`] names.
//!
//! Each method is one transaction, and each write is fenced by the
//! instance's lease ([`crate::pg::Fence`]). The methods block the calling
//! thread on the database (their futures do their work when first polled,
//! as the filesystem's do), so the store's synchronous methods drive them.

use std::sync::Arc;

use clax_core::content::{AssetKey, ContentStore, VersionWrite, missing, present, superseded};
use clax_core::{ArtifactId, CoreError, Result};
use postgres::Transaction;
use sha2::{Digest, Sha256};

use crate::pg::{Fence, PgError, Pool, err};

/// One instance's artifact content in PostgreSQL (see the
/// [module docs](self)).
pub struct PgContent {
    pool: Arc<Pool>,
    fence: Arc<Fence>,
}

impl PgContent {
    pub fn new(pool: Arc<Pool>, fence: Arc<Fence>) -> PgContent {
        PgContent { pool, fence }
    }

    /// Runs `f` in one transaction, fenced when `write`, and maps its
    /// errors: a [`Refused`] is the store's own error, anything else an
    /// I/O error naming the database.
    fn tx<R: Send + 'static>(
        &self,
        write: bool,
        f: impl FnOnce(&mut Transaction<'_>, &Fence) -> std::result::Result<R, Refused> + Send + 'static,
    ) -> Result<R> {
        let fence = self.fence.clone();
        let out = self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            if write {
                fence.check(&mut tx)?;
            }
            match f(&mut tx, &fence) {
                Ok(r) => {
                    tx.commit().map_err(err)?;
                    Ok(Ok(r))
                }
                Err(Refused::Store(e)) => Ok(Err(e)),
                Err(Refused::Pg(e)) => Err(e),
            }
        });
        match out {
            Ok(r) => r,
            Err(e) => Err(CoreError::Io(std::io::Error::other(format!(
                "PostgreSQL content store: {e}"
            )))),
        }
    }
}

/// Why a transaction's job stopped: a rule of the content store, which the
/// store answers as it is, or the database.
enum Refused {
    Store(CoreError),
    Pg(PgError),
}

impl From<postgres::Error> for Refused {
    fn from(e: postgres::Error) -> Refused {
        Refused::Pg(err(e))
    }
}

fn hash(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// Whether version `n` of `artifact` is present.
fn has_version(
    tx: &mut Transaction<'_>,
    instance: &str,
    artifact: &str,
    n: i32,
) -> std::result::Result<bool, Refused> {
    Ok(tx
        .query_opt(
            "SELECT 1 FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n = $3",
            &[&instance, &artifact, &n],
        )?
        .is_some())
}

/// The newest version of `artifact` above `n`, when one is present.
fn above(
    tx: &mut Transaction<'_>,
    instance: &str,
    artifact: &str,
    n: i32,
) -> std::result::Result<Option<u32>, Refused> {
    let row = tx.query_one(
        "SELECT max(n) FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n > $3",
        &[&instance, &artifact, &n],
    )?;
    Ok(row.get::<_, Option<i32>>(0).map(|m| m as u32))
}

/// Refuses a write of version `n` of `id` that is present, or below a
/// present version.
fn creatable(
    tx: &mut Transaction<'_>,
    instance: &str,
    id: &ArtifactId,
    n: u32,
) -> std::result::Result<(), Refused> {
    if has_version(tx, instance, id.as_str(), n as i32)? {
        return Err(Refused::Store(present(id, n)));
    }
    if let Some(m) = above(tx, instance, id.as_str(), n as i32)? {
        return Err(Refused::Store(superseded(id, n, m)));
    }
    Ok(())
}

/// Deletes the blobs of `artifact` no version names.
fn sweep(
    tx: &mut Transaction<'_>,
    instance: &str,
    artifact: &str,
) -> std::result::Result<(), Refused> {
    tx.execute(
        "DELETE FROM clax_fleet_blobs b
         WHERE b.instance = $1 AND b.artifact = $2 AND NOT EXISTS (
             SELECT 1 FROM clax_fleet_version_files f
             WHERE f.instance = b.instance AND f.artifact = b.artifact AND f.hash = b.hash)",
        &[&instance, &artifact],
    )?;
    Ok(())
}

/// Copies version `from_n` of `from`'s `paths` to the version row of `to`
/// at `to_n` (which the caller made), with their blobs.
fn copy_files(
    tx: &mut Transaction<'_>,
    instance: &str,
    (from, from_n): (&ArtifactId, u32),
    paths: &[String],
    (to, to_n): (&ArtifactId, u32),
) -> std::result::Result<(), Refused> {
    let rows = tx.query(
        "SELECT path, hash FROM clax_fleet_version_files
         WHERE instance = $1 AND artifact = $2 AND n = $3 AND path = ANY($4)",
        &[&instance, &from.as_str(), &(from_n as i32), &paths],
    )?;
    let found: std::collections::HashMap<String, Vec<u8>> =
        rows.into_iter().map(|r| (r.get(0), r.get(1))).collect();
    if let Some(gone) = paths.iter().find(|p| !found.contains_key(*p)) {
        return Err(Refused::Store(missing(from, from_n, gone)));
    }
    if from != to {
        let hashes: Vec<&Vec<u8>> = found.values().collect();
        tx.execute(
            "INSERT INTO clax_fleet_blobs (instance, artifact, hash, bytes)
             SELECT instance, $3, hash, bytes FROM clax_fleet_blobs
             WHERE instance = $1 AND artifact = $2 AND hash = ANY($4)
             ON CONFLICT DO NOTHING",
            &[&instance, &from.as_str(), &to.as_str(), &hashes],
        )?;
        // Every blob the copy names must now be the target's: a source
        // whose blobs are gone (deleted meanwhile) fails the copy.
        let distinct: std::collections::HashSet<&Vec<u8>> = hashes.iter().copied().collect();
        let held: i64 = tx
            .query_one(
                "SELECT count(*) FROM clax_fleet_blobs
                 WHERE instance = $1 AND artifact = $2 AND hash = ANY($3)",
                &[&instance, &to.as_str(), &hashes],
            )?
            .get(0);
        if held as usize != distinct.len() {
            return Err(Refused::Store(missing(from, from_n, &paths[0])));
        }
    }
    let (paths, hashes): (Vec<String>, Vec<Vec<u8>>) = found.into_iter().unzip();
    tx.execute(
        "INSERT INTO clax_fleet_version_files (instance, artifact, n, path, hash)
         SELECT $1, $2, $3, p, h FROM unnest($4::text[], $5::bytea[]) AS u(p, h)",
        &[&instance, &to.as_str(), &(to_n as i32), &paths, &hashes],
    )?;
    Ok(())
}

#[async_trait::async_trait]
impl ContentStore for PgContent {
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        let (id, path) = (id.clone(), path.to_string());
        self.tx(false, move |tx, fence| {
            let row = tx.query_opt(
                "SELECT b.bytes FROM clax_fleet_version_files f
                 JOIN clax_fleet_blobs b
                   ON b.instance = f.instance AND b.artifact = f.artifact AND b.hash = f.hash
                 WHERE f.instance = $1 AND f.artifact = $2 AND f.n = $3 AND f.path = $4",
                &[&fence.instance, &id.as_str(), &(n as i32), &path],
            )?;
            Ok(row.map(|r| r.get(0)))
        })
    }

    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        let id = id.clone();
        let carry: Vec<String> = write.carry.iter().map(|p| p.to_string()).collect();
        let put: Vec<(String, Vec<u8>)> = write
            .put
            .iter()
            .map(|f| (f.path.to_string(), f.bytes.to_vec()))
            .collect();
        let step = write.meta.step.clone();
        self.tx(true, move |tx, fence| {
            let instance = fence.instance.as_str();
            creatable(tx, instance, &id, n)?;
            tx.execute(
                "INSERT INTO clax_fleet_versions (instance, artifact, n, step) VALUES ($1, $2, $3, $4)",
                &[&instance, &id.as_str(), &(n as i32), &step],
            )?;
            if !carry.is_empty() {
                if n < 2 {
                    return Err(Refused::Store(missing(&id, 0, &carry[0])));
                }
                copy_files(tx, instance, (&id, n - 1), &carry, (&id, n))?;
            }
            for (path, bytes) in &put {
                let h = hash(bytes);
                tx.execute(
                    "INSERT INTO clax_fleet_blobs (instance, artifact, hash, bytes)
                     VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                    &[&instance, &id.as_str(), &h, bytes],
                )?;
                tx.execute(
                    "INSERT INTO clax_fleet_version_files (instance, artifact, n, path, hash)
                     VALUES ($1, $2, $3, $4, $5)",
                    &[&instance, &id.as_str(), &(n as i32), path, &h],
                )?;
            }
            Ok(())
        })
    }

    async fn copy_version(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        to_n: u32,
    ) -> Result<()> {
        let (from, to) = (from.clone(), to.clone());
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.tx(true, move |tx, fence| {
            let instance = fence.instance.as_str();
            creatable(tx, instance, &to, to_n)?;
            tx.execute(
                "INSERT INTO clax_fleet_versions (instance, artifact, n, step) VALUES ($1, $2, $3, NULL)",
                &[&instance, &to.as_str(), &(to_n as i32)],
            )?;
            copy_files(tx, instance, (&from, from_n), &paths, (&to, to_n))
        })
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        let id = id.clone();
        self.tx(true, move |tx, fence| {
            let instance = fence.instance.as_str();
            if !has_version(tx, instance, id.as_str(), n as i32)? {
                return Ok(());
            }
            if let Some(m) = above(tx, instance, id.as_str(), n as i32)? {
                return Err(Refused::Store(superseded(&id, n, m)));
            }
            tx.execute(
                "DELETE FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n = $3",
                &[&instance, &id.as_str(), &(n as i32)],
            )?;
            sweep(tx, instance, id.as_str())
        })
    }

    async fn unrecorded_step(&self, id: &ArtifactId, n: u32) -> Result<Option<String>> {
        let id = id.clone();
        self.tx(false, move |tx, fence| {
            let instance = fence.instance.as_str();
            if above(tx, instance, id.as_str(), n as i32)?.is_some() {
                return Ok(None);
            }
            let row = tx.query_opt(
                "SELECT step FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n = $3",
                &[&instance, &id.as_str(), &(n as i32)],
            )?;
            Ok(row.and_then(|r| r.get(0)))
        })
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        let id = id.clone();
        self.tx(true, move |tx, fence| {
            for table in [
                "clax_fleet_versions",
                "clax_fleet_blobs",
                "clax_fleet_assets",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE instance = $1 AND artifact = $2"),
                    &[&fence.instance, &id.as_str()],
                )?;
            }
            Ok(())
        })
    }

    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        let (key, bytes) = (key.clone(), bytes.to_vec());
        self.tx(true, move |tx, fence| {
            tx.execute(
                "INSERT INTO clax_fleet_assets (instance, artifact, id, ext, bytes)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (instance, artifact, id, ext) DO UPDATE SET bytes = EXCLUDED.bytes",
                &[
                    &fence.instance,
                    &key.artifact.as_str(),
                    &key.id,
                    &key.ext,
                    &bytes,
                ],
            )?;
            Ok(())
        })
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        let key = key.clone();
        self.tx(false, move |tx, fence| {
            let row = tx.query_opt(
                "SELECT bytes FROM clax_fleet_assets
                 WHERE instance = $1 AND artifact = $2 AND id = $3 AND ext = $4",
                &[&fence.instance, &key.artifact.as_str(), &key.id, &key.ext],
            )?;
            Ok(row.map(|r| r.get(0)))
        })
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        let key = key.clone();
        self.tx(true, move |tx, fence| {
            tx.execute(
                "DELETE FROM clax_fleet_assets
                 WHERE instance = $1 AND artifact = $2 AND id = $3 AND ext = $4",
                &[&fence.instance, &key.artifact.as_str(), &key.id, &key.ext],
            )?;
            Ok(())
        })
    }
}
