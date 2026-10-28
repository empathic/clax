//! An instance's SQLite files in PostgreSQL: [`PgPages`], the
//! [`PageStore`] its database and its artifacts' app data stores open on
//! through `clax-pagevfs`.
//!
//! A file is a row of `clax_fleet_files` (its size) and its chunks' rows of
//! `clax_fleet_chunks`. A commit is one transaction, fenced by the
//! instance's lease ([`crate::pg::Fence`]): the chunks written (one
//! statement per [`WRITE_BATCH`] chunks, over arrays), the size when it
//! changed, and the deletion of the chunks past the end when the file
//! shrank. Every statement is typed (`query_typed`), one round trip each.
//!
//! Reads ahead on sequential misses: a chunk not cached right after the
//! last one missed is read with the [`READ_AHEAD`] chunks after it, any
//! other miss alone. What is read is kept, up to [`CACHE_CHUNKS`] chunks for
//! the instance, the oldest dropped first. The process owns the instance
//! while it holds the lease, so what it read stays true; a commit drops the
//! chunks it wrote from the cache rather than keep a second copy of what
//! SQLite's own page cache (`clax_pagevfs::Limits`) holds. This cache only
//! spares a cold start and a scan a round trip per page.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use clax_pagevfs::{CHUNK_SIZE, Commit, PageStore, StoreError};

use crate::pg::{Fence, Pool, Type, err};

/// Chunks read after the one asked for, on a sequential miss.
pub const READ_AHEAD: i64 = 63;

/// The most chunks the read cache keeps for one instance: 16 MiB.
pub const CACHE_CHUNKS: usize = 4096;

/// The most chunks one statement of a commit writes: 1 MiB, which bounds
/// the encoded statement a large commit sends at once.
pub const WRITE_BATCH: usize = 256;

/// A cached chunk: its bytes, or `None` for one the store has not.
type Cached = Option<Arc<Vec<u8>>>;

/// The read cache: chunks by (file number, index), in the order they came.
#[derive(Default)]
struct Cache {
    /// Each file name's number, so keys are copied without allocating.
    files: HashMap<String, u32>,
    chunks: HashMap<(u32, u64), Cached>,
    order: VecDeque<(u32, u64)>,
    /// The last chunk missed, by file: the next miss right after it reads
    /// ahead.
    last_miss: HashMap<u32, u64>,
}

impl Cache {
    fn file(&mut self, name: &str) -> u32 {
        if let Some(n) = self.files.get(name) {
            return *n;
        }
        let n = self.files.len() as u32;
        self.files.insert(name.to_string(), n);
        n
    }

    fn insert(&mut self, key: (u32, u64), chunk: Cached) {
        if self.chunks.insert(key, chunk).is_none() {
            self.order.push_back(key);
        }
        while self.chunks.len() > CACHE_CHUNKS {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            self.chunks.remove(&old);
        }
    }

    /// Forgets the chunks of `file` that `keep` turns down.
    fn forget(&mut self, file: u32, keep: impl Fn(u64) -> bool) {
        self.chunks.retain(|(f, i), _| *f != file || keep(*i));
        let chunks = &self.chunks;
        self.order.retain(|k| chunks.contains_key(k));
    }
}

/// One instance's files in PostgreSQL (see the [module docs](self)).
pub struct PgPages {
    pool: Arc<Pool>,
    fence: Arc<Fence>,
    cache: Mutex<Cache>,
}

impl PgPages {
    pub fn new(pool: Arc<Pool>, fence: Arc<Fence>) -> PgPages {
        PgPages {
            pool,
            fence,
            cache: Mutex::default(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn instance(&self) -> String {
        self.fence.instance.clone()
    }

    /// The chunks the read cache holds, for tests.
    pub fn cached_chunks(&self) -> usize {
        self.cache().chunks.len()
    }
}

impl PageStore for PgPages {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        let (instance, file) = (self.instance(), file.to_string());
        self.pool.run(move |c| {
            let row = c
                .query_typed_opt(
                    "SELECT size FROM clax_fleet_files WHERE instance = $1 AND file = $2",
                    &[(&instance, Type::TEXT), (&file, Type::TEXT)],
                )
                .map_err(err)?;
            Ok(row.map(|r| r.get::<_, i64>(0) as u64))
        })
    }

    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        let ahead = {
            let mut cache = self.cache();
            let f = cache.file(file);
            if let Some(hit) = cache.chunks.get(&(f, index)) {
                return Ok(hit.as_ref().map(|b| b.as_ref().clone()));
            }
            let sequential = cache
                .last_miss
                .insert(f, index)
                .is_some_and(|last| last + 1 == index);
            if sequential { READ_AHEAD } else { 0 }
        };
        let (instance, name) = (self.instance(), file.to_string());
        let first = index as i64;
        let rows: Vec<(i64, Vec<u8>)> = self.pool.run(move |c| {
            let rows = c
                .query_typed(
                    "SELECT idx, bytes FROM clax_fleet_chunks
                     WHERE instance = $1 AND file = $2 AND idx BETWEEN $3 AND $3 + $4",
                    &[
                        (&instance, Type::TEXT),
                        (&name, Type::TEXT),
                        (&first, Type::INT8),
                        (&ahead, Type::INT8),
                    ],
                )
                .map_err(err)?;
            Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
        })?;
        let mut got: HashMap<u64, Arc<Vec<u8>>> = HashMap::new();
        for (i, bytes) in rows {
            if bytes.len() != CHUNK_SIZE {
                return Err(format!(
                    "{file}: chunk {i} of instance {} is {} bytes, not {CHUNK_SIZE}",
                    self.fence.instance,
                    bytes.len()
                ));
            }
            got.insert(i as u64, Arc::new(bytes));
        }
        let found = got.get(&index).map(|b| b.as_ref().clone());
        let mut cache = self.cache();
        let f = cache.file(file);
        // The chunk asked for is SQLite's now; the ones after it wait here.
        for i in index + 1..=index + ahead as u64 {
            cache.insert((f, i), got.remove(&i));
        }
        Ok(found)
    }

    fn commit(&self, file: &str, commit: &Commit) -> Result<(), StoreError> {
        let fence = self.fence.clone();
        let name = file.to_string();
        let chunks = commit.chunks.clone();
        let size = commit.size as i64;
        let resize = commit.resizes();
        let truncate = commit.truncates().then_some(commit.keep_below as i64);
        self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            fence.check(&mut tx)?;
            let all: Vec<(&u64, &Vec<u8>)> = chunks.iter().collect();
            for batch in all.chunks(WRITE_BATCH) {
                let indexes: Vec<i64> = batch.iter().map(|(i, _)| **i as i64).collect();
                let bytes: Vec<&[u8]> = batch.iter().map(|(_, b)| b.as_slice()).collect();
                tx.execute_typed(
                    "INSERT INTO clax_fleet_chunks (instance, file, idx, bytes)
                     SELECT $1, $2, i, b FROM unnest($3::bigint[], $4::bytea[]) AS u(i, b)
                     ON CONFLICT (instance, file, idx) DO UPDATE SET bytes = EXCLUDED.bytes",
                    &[
                        (&fence.instance, Type::TEXT),
                        (&name, Type::TEXT),
                        (&indexes, Type::INT8_ARRAY),
                        (&bytes, Type::BYTEA_ARRAY),
                    ],
                )
                .map_err(err)?;
            }
            if resize {
                tx.execute_typed(
                    "INSERT INTO clax_fleet_files (instance, file, size) VALUES ($1, $2, $3)
                     ON CONFLICT (instance, file) DO UPDATE SET size = EXCLUDED.size",
                    &[
                        (&fence.instance, Type::TEXT),
                        (&name, Type::TEXT),
                        (&size, Type::INT8),
                    ],
                )
                .map_err(err)?;
            }
            if let Some(keep) = truncate {
                tx.execute_typed(
                    "DELETE FROM clax_fleet_chunks
                     WHERE instance = $1 AND file = $2 AND idx >= $3",
                    &[
                        (&fence.instance, Type::TEXT),
                        (&name, Type::TEXT),
                        (&keep, Type::INT8),
                    ],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)
        })?;
        let mut cache = self.cache();
        let f = cache.file(file);
        let (written, keep) = (&commit.chunks, commit.keep_below);
        cache.forget(f, |i| i < keep && !written.contains_key(&i));
        Ok(())
    }

    fn delete(&self, file: &str) -> Result<(), StoreError> {
        let fence = self.fence.clone();
        let name = file.to_string();
        self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            fence.check(&mut tx)?;
            for table in ["clax_fleet_chunks", "clax_fleet_files"] {
                tx.execute_typed(
                    &format!("DELETE FROM {table} WHERE instance = $1 AND file = $2"),
                    &[(&fence.instance, Type::TEXT), (&name, Type::TEXT)],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)
        })?;
        let mut cache = self.cache();
        let f = cache.file(file);
        cache.forget(f, |_| false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk() -> Cached {
        Some(Arc::new(vec![0; CHUNK_SIZE]))
    }

    #[test]
    fn the_cache_keeps_its_bound_dropping_the_oldest() {
        let mut c = Cache::default();
        let f = c.file("db");
        for i in 0..(CACHE_CHUNKS as u64 + 100) {
            c.insert((f, i), chunk());
        }
        assert_eq!(c.chunks.len(), CACHE_CHUNKS);
        assert!(!c.chunks.contains_key(&(f, 0)));
        assert!(c.chunks.contains_key(&(f, CACHE_CHUNKS as u64 + 99)));
        // Inserting a key again does not count it twice.
        c.insert((f, CACHE_CHUNKS as u64 + 99), chunk());
        assert_eq!(c.order.len(), CACHE_CHUNKS);
    }

    #[test]
    fn forgetting_keeps_other_files_and_the_order_in_step() {
        let mut c = Cache::default();
        let (a, b) = (c.file("db"), c.file("app/x"));
        assert_eq!(c.file("db"), a);
        for i in 0..10 {
            c.insert((a, i), chunk());
            c.insert((b, i), chunk());
        }
        c.forget(a, |i| i < 5);
        assert_eq!(c.chunks.len(), 15);
        assert_eq!(c.order.len(), 15);
        c.forget(b, |_| false);
        assert_eq!(c.chunks.len(), 5);
    }
}
