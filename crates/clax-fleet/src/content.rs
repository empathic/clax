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
//! Each write is one transaction, fenced by the instance's lease
//! ([`crate::pg::Fence`]); a publish writes all its new blobs and file rows
//! in one statement each. A read is one typed query, outside any
//! transaction. Version files never change once written, so what is read
//! (and what a write puts) is kept in a cache of at most [`CACHE_BYTES`],
//! the oldest dropped first; a removal, a deletion or an asset written
//! again forgets its artifact's entries. Reads take no lock, so a read
//! keeps what it found only when nothing was forgotten since it missed:
//! a read in flight across a write never caches the bytes the write
//! replaced. The methods block the calling thread on the database (their
//! futures do their work when first polled, as the filesystem's do), so
//! the store's synchronous methods drive them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use clax_core::content::{AssetKey, ContentStore, VersionWrite, missing, present, superseded};
use clax_core::{ArtifactId, CoreError, Result};
use postgres::Transaction;
use sha2::{Digest, Sha256};

use crate::pg::{Fence, PgError, Pool, Type, err};

/// The most bytes the read cache keeps for one instance: 64 MiB.
pub const CACHE_BYTES: usize = 64 << 20;

/// The largest file the read cache keeps: 4 MiB.
pub const CACHE_FILE_BYTES: usize = 4 << 20;

/// What the read cache keys a file by: a version's file, or an asset.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Key {
    File(String, u32, String),
    Asset(AssetKey),
}

impl Key {
    fn artifact(&self) -> &str {
        match self {
            Key::File(a, _, _) => a,
            Key::Asset(k) => k.artifact.as_str(),
        }
    }
}

#[derive(Default)]
struct Cache {
    bytes: HashMap<Key, Arc<Vec<u8>>>,
    order: VecDeque<Key>,
    held: usize,
    /// Forgets so far: a read that missed puts what it read only if none
    /// came between ([`Cache::put_read`]).
    forgets: u64,
}

/// A read's miss: the forget count when it missed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Miss(u64);

impl Cache {
    /// The cached bytes of `k`, or the [`Miss`] to put a read of them with.
    fn get(&self, k: &Key) -> std::result::Result<Vec<u8>, Miss> {
        self.bytes
            .get(k)
            .map(|b| b.as_ref().clone())
            .ok_or(Miss(self.forgets))
    }

    /// Puts what a read that missed at `miss` found, unless something was
    /// forgotten since: that read may have found what a write replaced.
    fn put_read(&mut self, miss: Miss, k: Key, b: Arc<Vec<u8>>) {
        if miss.0 == self.forgets {
            self.put(k, b);
        }
    }

    fn put(&mut self, k: Key, b: Arc<Vec<u8>>) {
        if b.len() > CACHE_FILE_BYTES {
            return;
        }
        self.held += b.len();
        if let Some(old) = self.bytes.insert(k.clone(), b) {
            self.held -= old.len();
        } else {
            self.order.push_back(k);
        }
        while self.held > CACHE_BYTES {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(b) = self.bytes.remove(&old) {
                self.held -= b.len();
            }
        }
    }

    fn forget(&mut self, artifact: &str, which: impl Fn(&Key) -> bool) {
        self.forgets += 1;
        let mut freed = 0;
        self.bytes.retain(|k, b| {
            let gone = k.artifact() == artifact && which(k);
            if gone {
                freed += b.len();
            }
            !gone
        });
        self.held -= freed;
        let bytes = &self.bytes;
        self.order.retain(|k| bytes.contains_key(k));
    }
}

/// One instance's artifact content in PostgreSQL (see the
/// [module docs](self)).
pub struct PgContent {
    pool: Arc<Pool>,
    fence: Arc<Fence>,
    cache: Mutex<Cache>,
}

impl PgContent {
    pub fn new(pool: Arc<Pool>, fence: Arc<Fence>) -> PgContent {
        PgContent {
            pool,
            fence,
            cache: Mutex::default(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` in one write transaction, fenced, and maps its errors: a
    /// [`Refused`] is the store's own error, anything else an I/O error
    /// naming the database.
    fn tx<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Transaction<'_>, &Fence) -> std::result::Result<R, Refused> + Send + 'static,
    ) -> Result<R> {
        let fence = self.fence.clone();
        let out = self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            fence.check(&mut tx)?;
            match f(&mut tx, &fence) {
                Ok(r) => {
                    tx.commit().map_err(err)?;
                    Ok(Ok(r))
                }
                Err(Refused::Store(e)) => Ok(Err(e)),
                Err(Refused::Pg(e)) => Err(e),
            }
        });
        out.unwrap_or_else(|e| Err(db_error(e)))
    }

    /// Runs `f`, one read, on a client of the pool.
    fn read<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut postgres::Client, &str) -> std::result::Result<R, postgres::Error>
        + Send
        + 'static,
    ) -> Result<R> {
        let instance = self.fence.instance.clone();
        self.pool
            .run(move |c| f(c, &instance).map_err(err))
            .map_err(db_error)
    }
}

fn db_error(e: PgError) -> CoreError {
    CoreError::Io(std::io::Error::other(format!(
        "PostgreSQL content store: {e}"
    )))
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

type Out<T> = std::result::Result<T, Refused>;

/// Whether version `n` of `artifact` is present.
fn has_version(tx: &mut Transaction<'_>, instance: &str, artifact: &str, n: i32) -> Out<bool> {
    Ok(tx
        .query_typed_opt(
            "SELECT 1 FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n = $3",
            &[
                (&instance, Type::TEXT),
                (&artifact, Type::TEXT),
                (&n, Type::INT4),
            ],
        )?
        .is_some())
}

/// The newest version of `artifact` above `n`, when one is present.
fn above(tx: &mut Transaction<'_>, instance: &str, artifact: &str, n: i32) -> Out<Option<u32>> {
    let row = tx.query_typed_one(
        "SELECT max(n) FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n > $3",
        &[
            (&instance, Type::TEXT),
            (&artifact, Type::TEXT),
            (&n, Type::INT4),
        ],
    )?;
    Ok(row.get::<_, Option<i32>>(0).map(|m| m as u32))
}

/// Refuses a write of version `n` of `id` that is present, or below a
/// present version.
fn creatable(tx: &mut Transaction<'_>, instance: &str, id: &ArtifactId, n: u32) -> Out<()> {
    if has_version(tx, instance, id.as_str(), n as i32)? {
        return Err(Refused::Store(present(id, n)));
    }
    if let Some(m) = above(tx, instance, id.as_str(), n as i32)? {
        return Err(Refused::Store(superseded(id, n, m)));
    }
    Ok(())
}

/// Makes version `n` of `id`'s row, with its step mark.
fn version_row(
    tx: &mut Transaction<'_>,
    instance: &str,
    id: &ArtifactId,
    n: u32,
    step: Option<&str>,
) -> Out<()> {
    tx.execute_typed(
        "INSERT INTO clax_fleet_versions (instance, artifact, n, step) VALUES ($1, $2, $3, $4)",
        &[
            (&instance, Type::TEXT),
            (&id.as_str(), Type::TEXT),
            (&(n as i32), Type::INT4),
            (&step, Type::TEXT),
        ],
    )?;
    Ok(())
}

/// Deletes the blobs of `artifact` no version names.
fn sweep(tx: &mut Transaction<'_>, instance: &str, artifact: &str) -> Out<()> {
    tx.execute_typed(
        "DELETE FROM clax_fleet_blobs b
         WHERE b.instance = $1 AND b.artifact = $2 AND NOT EXISTS (
             SELECT 1 FROM clax_fleet_version_files f
             WHERE f.instance = b.instance AND f.artifact = b.artifact AND f.hash = b.hash)",
        &[(&instance, Type::TEXT), (&artifact, Type::TEXT)],
    )?;
    Ok(())
}

/// Adds `(path, hash)` rows to version `n` of `id`.
fn file_rows(
    tx: &mut Transaction<'_>,
    instance: &str,
    (id, n): (&ArtifactId, u32),
    paths: &[String],
    hashes: &[Vec<u8>],
) -> Out<()> {
    tx.execute_typed(
        "INSERT INTO clax_fleet_version_files (instance, artifact, n, path, hash)
         SELECT $1, $2, $3, p, h FROM unnest($4::text[], $5::bytea[]) AS u(p, h)",
        &[
            (&instance, Type::TEXT),
            (&id.as_str(), Type::TEXT),
            (&(n as i32), Type::INT4),
            (&paths, Type::TEXT_ARRAY),
            (&hashes, Type::BYTEA_ARRAY),
        ],
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
) -> Out<()> {
    let rows = tx.query_typed(
        "SELECT path, hash FROM clax_fleet_version_files
         WHERE instance = $1 AND artifact = $2 AND n = $3 AND path = ANY($4)",
        &[
            (&instance, Type::TEXT),
            (&from.as_str(), Type::TEXT),
            (&(from_n as i32), Type::INT4),
            (&paths, Type::TEXT_ARRAY),
        ],
    )?;
    let found: HashMap<String, Vec<u8>> = rows.into_iter().map(|r| (r.get(0), r.get(1))).collect();
    if let Some(gone) = paths.iter().find(|p| !found.contains_key(*p)) {
        return Err(Refused::Store(missing(from, from_n, gone)));
    }
    if from != to {
        let hashes: Vec<&Vec<u8>> = found.values().collect();
        tx.execute_typed(
            "INSERT INTO clax_fleet_blobs (instance, artifact, hash, bytes)
             SELECT instance, $3, hash, bytes FROM clax_fleet_blobs
             WHERE instance = $1 AND artifact = $2 AND hash = ANY($4)
             ON CONFLICT DO NOTHING",
            &[
                (&instance, Type::TEXT),
                (&from.as_str(), Type::TEXT),
                (&to.as_str(), Type::TEXT),
                (&hashes, Type::BYTEA_ARRAY),
            ],
        )?;
        // Every blob the copy names must now be the target's: a source
        // whose blobs are gone (deleted meanwhile) fails the copy.
        let distinct: HashSet<&Vec<u8>> = hashes.iter().copied().collect();
        let held: i64 = tx
            .query_typed_one(
                "SELECT count(*) FROM clax_fleet_blobs
                 WHERE instance = $1 AND artifact = $2 AND hash = ANY($3)",
                &[
                    (&instance, Type::TEXT),
                    (&to.as_str(), Type::TEXT),
                    (&hashes, Type::BYTEA_ARRAY),
                ],
            )?
            .get(0);
        if held as usize != distinct.len() {
            return Err(Refused::Store(missing(from, from_n, &paths[0])));
        }
    }
    let (paths, hashes): (Vec<String>, Vec<Vec<u8>>) = found.into_iter().unzip();
    file_rows(tx, instance, (to, to_n), &paths, &hashes)
}

#[async_trait::async_trait]
impl ContentStore for PgContent {
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        let key = Key::File(id.as_str().to_string(), n, path.to_string());
        let miss = match self.cache().get(&key) {
            Ok(b) => return Ok(Some(b)),
            Err(miss) => miss,
        };
        let (artifact, path) = (id.as_str().to_string(), path.to_string());
        let got: Option<Vec<u8>> = self.read(move |c, instance| {
            let row = c.query_typed_opt(
                "SELECT b.bytes FROM clax_fleet_version_files f
                 JOIN clax_fleet_blobs b
                   ON b.instance = f.instance AND b.artifact = f.artifact AND b.hash = f.hash
                 WHERE f.instance = $1 AND f.artifact = $2 AND f.n = $3 AND f.path = $4",
                &[
                    (&instance, Type::TEXT),
                    (&artifact, Type::TEXT),
                    (&(n as i32), Type::INT4),
                    (&path, Type::TEXT),
                ],
            )?;
            Ok(row.map(|r| r.get(0)))
        })?;
        if let Some(b) = &got {
            self.cache().put_read(miss, key, Arc::new(b.clone()));
        }
        Ok(got)
    }

    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        let id = id.clone();
        let carry: Vec<String> = write.carry.iter().map(|p| p.to_string()).collect();
        // Hashed here, on the caller's thread; the job gets what it sends.
        let put: Vec<(String, Vec<u8>, Vec<u8>)> = write
            .put
            .iter()
            .map(|f| (f.path.to_string(), hash(f.bytes), f.bytes.to_vec()))
            .collect();
        let step = write.meta.step.clone();
        let cached: Vec<(Key, Arc<Vec<u8>>)> = put
            .iter()
            .map(|(p, _, b)| {
                (
                    Key::File(id.as_str().to_string(), n, p.clone()),
                    Arc::new(b.clone()),
                )
            })
            .filter(|(_, b)| b.len() <= CACHE_FILE_BYTES)
            .collect();
        let job_id = id.clone();
        self.tx(move |tx, fence| {
            let instance = fence.instance.as_str();
            let id = job_id;
            creatable(tx, instance, &id, n)?;
            version_row(tx, instance, &id, n, step.as_deref())?;
            if !carry.is_empty() {
                if n < 2 {
                    return Err(Refused::Store(missing(&id, 0, &carry[0])));
                }
                copy_files(tx, instance, (&id, n - 1), &carry, (&id, n))?;
            }
            if !put.is_empty() {
                let hashes: Vec<&Vec<u8>> = put.iter().map(|(_, h, _)| h).collect();
                let bytes: Vec<&Vec<u8>> = put.iter().map(|(_, _, b)| b).collect();
                tx.execute_typed(
                    "INSERT INTO clax_fleet_blobs (instance, artifact, hash, bytes)
                     SELECT $1, $2, h, b FROM unnest($3::bytea[], $4::bytea[]) AS u(h, b)
                     ON CONFLICT DO NOTHING",
                    &[
                        (&instance, Type::TEXT),
                        (&id.as_str(), Type::TEXT),
                        (&hashes, Type::BYTEA_ARRAY),
                        (&bytes, Type::BYTEA_ARRAY),
                    ],
                )?;
                let paths: Vec<String> = put.iter().map(|(p, _, _)| p.clone()).collect();
                let hashes: Vec<Vec<u8>> = put.iter().map(|(_, h, _)| h.clone()).collect();
                file_rows(tx, instance, (&id, n), &paths, &hashes)?;
            }
            Ok(())
        })?;
        let mut cache = self.cache();
        for (k, b) in cached {
            cache.put(k, b);
        }
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
        let (from, to) = (from.clone(), to.clone());
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.tx(move |tx, fence| {
            let instance = fence.instance.as_str();
            creatable(tx, instance, &to, to_n)?;
            version_row(tx, instance, &to, to_n, None)?;
            copy_files(tx, instance, (&from, from_n), &paths, (&to, to_n))
        })
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        let job_id = id.clone();
        self.tx(move |tx, fence| {
            let instance = fence.instance.as_str();
            let id = job_id;
            if !has_version(tx, instance, id.as_str(), n as i32)? {
                return Ok(());
            }
            if let Some(m) = above(tx, instance, id.as_str(), n as i32)? {
                return Err(Refused::Store(superseded(&id, n, m)));
            }
            tx.execute_typed(
                "DELETE FROM clax_fleet_versions WHERE instance = $1 AND artifact = $2 AND n = $3",
                &[
                    (&instance, Type::TEXT),
                    (&id.as_str(), Type::TEXT),
                    (&(n as i32), Type::INT4),
                ],
            )?;
            sweep(tx, instance, id.as_str())
        })?;
        self.cache()
            .forget(id.as_str(), |k| matches!(k, Key::File(_, m, _) if *m == n));
        Ok(())
    }

    async fn unrecorded_step(&self, id: &ArtifactId, n: u32) -> Result<Option<String>> {
        let artifact = id.as_str().to_string();
        self.read(move |c, instance| {
            let row = c.query_typed_opt(
                "SELECT step FROM clax_fleet_versions v
                 WHERE instance = $1 AND artifact = $2 AND n = $3 AND NOT EXISTS (
                     SELECT 1 FROM clax_fleet_versions a
                     WHERE a.instance = v.instance AND a.artifact = v.artifact AND a.n > v.n)",
                &[
                    (&instance, Type::TEXT),
                    (&artifact, Type::TEXT),
                    (&(n as i32), Type::INT4),
                ],
            )?;
            Ok(row.and_then(|r| r.get(0)))
        })
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        let job_id = id.clone();
        self.tx(move |tx, fence| {
            for table in [
                "clax_fleet_versions",
                "clax_fleet_blobs",
                "clax_fleet_assets",
            ] {
                tx.execute_typed(
                    &format!("DELETE FROM {table} WHERE instance = $1 AND artifact = $2"),
                    &[
                        (&fence.instance, Type::TEXT),
                        (&job_id.as_str(), Type::TEXT),
                    ],
                )?;
            }
            Ok(())
        })?;
        self.cache().forget(id.as_str(), |_| true);
        Ok(())
    }

    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        let (k, b) = (key.clone(), bytes.to_vec());
        self.tx(move |tx, fence| {
            tx.execute_typed(
                "INSERT INTO clax_fleet_assets (instance, artifact, id, ext, bytes)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (instance, artifact, id, ext) DO UPDATE SET bytes = EXCLUDED.bytes",
                &[
                    (&fence.instance, Type::TEXT),
                    (&k.artifact.as_str(), Type::TEXT),
                    (&k.id, Type::TEXT),
                    (&k.ext, Type::TEXT),
                    (&b, Type::BYTEA),
                ],
            )?;
            Ok(())
        })?;
        // A clip is written again over its old bytes: never serve those.
        let k = Key::Asset(key.clone());
        self.cache().forget(key.artifact.as_str(), |x| *x == k);
        Ok(())
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        let ck = Key::Asset(key.clone());
        let miss = match self.cache().get(&ck) {
            Ok(b) => return Ok(Some(b)),
            Err(miss) => miss,
        };
        let k = key.clone();
        let got: Option<Vec<u8>> = self.read(move |c, instance| {
            let row = c.query_typed_opt(
                "SELECT bytes FROM clax_fleet_assets
                 WHERE instance = $1 AND artifact = $2 AND id = $3 AND ext = $4",
                &[
                    (&instance, Type::TEXT),
                    (&k.artifact.as_str(), Type::TEXT),
                    (&k.id, Type::TEXT),
                    (&k.ext, Type::TEXT),
                ],
            )?;
            Ok(row.map(|r| r.get(0)))
        })?;
        if let Some(b) = &got {
            self.cache().put_read(miss, ck, Arc::new(b.clone()));
        }
        Ok(got)
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        let k = key.clone();
        self.tx(move |tx, fence| {
            tx.execute_typed(
                "DELETE FROM clax_fleet_assets
                 WHERE instance = $1 AND artifact = $2 AND id = $3 AND ext = $4",
                &[
                    (&fence.instance, Type::TEXT),
                    (&k.artifact.as_str(), Type::TEXT),
                    (&k.id, Type::TEXT),
                    (&k.ext, Type::TEXT),
                ],
            )?;
            Ok(())
        })?;
        let k = Key::Asset(key.clone());
        self.cache().forget(key.artifact.as_str(), |x| *x == k);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_keeps_its_byte_bound_and_forgets_by_artifact() {
        let mut c = Cache::default();
        let mb = || Arc::new(vec![0u8; 1 << 20]);
        for i in 0..80 {
            c.put(Key::File("a".into(), i, "f".into()), mb());
        }
        assert!(c.held <= CACHE_BYTES);
        assert_eq!(c.bytes.len(), 64);
        assert!(c.get(&Key::File("a".into(), 0, "f".into())).is_err());
        c.put(Key::File("b".into(), 1, "f".into()), mb());
        c.put(Key::File("a".into(), 79, "f".into()), mb());
        assert_eq!(c.held, 64 << 20, "a key put again is counted once");
        c.forget("a", |_| true);
        assert_eq!(c.bytes.len(), 1);
        assert_eq!(c.held, 1 << 20);
        assert_eq!(c.order.len(), 1);
        // Too large to keep.
        c.put(
            Key::File("c".into(), 1, "f".into()),
            Arc::new(vec![0; CACHE_FILE_BYTES + 1]),
        );
        assert_eq!(c.bytes.len(), 1);
    }

    /// A read that missed before a write's forget, and puts after it, keeps
    /// nothing: it may have read the bytes the write replaced. A read that
    /// missed after the forget is kept.
    #[test]
    fn a_read_across_a_forget_is_not_cached() {
        let mut c = Cache::default();
        let k = Key::File("a".into(), 1, "f".into());
        let stale = c.get(&k).unwrap_err();
        c.forget("b", |_| true);
        c.put_read(stale, k.clone(), Arc::new(b"old".to_vec()));
        assert!(c.get(&k).is_err());
        let fresh = c.get(&k).unwrap_err();
        c.put_read(fresh, k.clone(), Arc::new(b"new".to_vec()));
        assert_eq!(c.get(&k).unwrap(), b"new");
    }
}
