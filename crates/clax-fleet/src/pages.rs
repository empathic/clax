//! An instance's SQLite files in PostgreSQL: [`PgPages`], the
//! [`PageStore`] its database and its artifacts' app data stores open on
//! through `clax-pagevfs`.
//!
//! A file is a row of `clax_fleet_files` (its size) and its chunks' rows of
//! `clax_fleet_chunks`. A commit is one transaction, fenced by the
//! instance's lease ([`crate::pg::Fence`]): the size, every chunk written
//! (one statement over arrays), and the deletion of the chunks past the
//! end.
//!
//! Reads ahead: a chunk not cached is read with the [`READ_AHEAD`] chunks
//! after it, and kept, up to [`CACHE_CHUNKS`] chunks for the instance (the
//! cache is emptied when full). The process owns the instance while it
//! holds the lease, so what it read stays true: its own commits update the
//! cache. SQLite's page cache (`clax_pagevfs::Limits`) stays the first
//! cache; this one spares a cold start a round trip per page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clax_pagevfs::{CHUNK_SIZE, Chunks, PageStore, StoreError};

use crate::pg::{Fence, Pool, err};

/// Chunks read after the one asked for, in the same query.
pub const READ_AHEAD: i64 = 63;

/// The most chunks the read cache keeps for one instance: 32 MiB.
pub const CACHE_CHUNKS: usize = 8192;

/// A cached chunk: its bytes, or `None` for one the store has not.
type Cached = Option<Arc<Vec<u8>>>;

/// One instance's files in PostgreSQL (see the [module docs](self)).
pub struct PgPages {
    pool: Arc<Pool>,
    fence: Arc<Fence>,
    cache: Mutex<HashMap<(String, u64), Cached>>,
}

impl PgPages {
    pub fn new(pool: Arc<Pool>, fence: Arc<Fence>) -> PgPages {
        PgPages {
            pool,
            fence,
            cache: Mutex::default(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<(String, u64), Cached>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn instance(&self) -> String {
        self.fence.instance.clone()
    }
}

impl PageStore for PgPages {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        let (instance, file) = (self.instance(), file.to_string());
        self.pool.run(move |c| {
            let row = c
                .query_opt(
                    "SELECT size FROM clax_fleet_files WHERE instance = $1 AND file = $2",
                    &[&instance, &file],
                )
                .map_err(err)?;
            Ok(row.map(|r| r.get::<_, i64>(0) as u64))
        })
    }

    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        if let Some(hit) = self.cache().get(&(file.to_string(), index)) {
            return Ok(hit.as_ref().map(|b| b.as_ref().clone()));
        }
        let (instance, name) = (self.instance(), file.to_string());
        let first = index as i64;
        let rows: Vec<(i64, Vec<u8>)> = self.pool.run(move |c| {
            let rows = c
                .query(
                    "SELECT idx, bytes FROM clax_fleet_chunks
                     WHERE instance = $1 AND file = $2 AND idx BETWEEN $3 AND $3 + $4",
                    &[&instance, &name, &first, &READ_AHEAD],
                )
                .map_err(err)?;
            Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
        })?;
        let mut cache = self.cache();
        if cache.len() + READ_AHEAD as usize + 1 > CACHE_CHUNKS {
            cache.clear();
        }
        let mut found = None;
        let mut got: HashMap<u64, Arc<Vec<u8>>> = HashMap::new();
        for (i, bytes) in rows {
            if bytes.len() != CHUNK_SIZE {
                return Err(format!(
                    "{file}: chunk {i} of instance {} is {} bytes, not {CHUNK_SIZE}",
                    self.fence.instance,
                    bytes.len()
                ));
            }
            let bytes = Arc::new(bytes);
            if i as u64 == index {
                found = Some(bytes.as_ref().clone());
            }
            got.insert(i as u64, bytes);
        }
        for i in index..=index + READ_AHEAD as u64 {
            cache.insert((file.to_string(), i), got.remove(&i));
        }
        Ok(found)
    }

    fn commit(
        &self,
        file: &str,
        size: u64,
        chunks: &Chunks,
        keep_below: u64,
    ) -> Result<(), StoreError> {
        let fence = self.fence.clone();
        let name = file.to_string();
        let indexes: Vec<i64> = chunks.keys().map(|i| *i as i64).collect();
        let bytes: Vec<Vec<u8>> = chunks.values().cloned().collect();
        let (size, keep) = (size as i64, keep_below as i64);
        self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            fence.check(&mut tx)?;
            tx.execute(
                "INSERT INTO clax_fleet_files (instance, file, size) VALUES ($1, $2, $3)
                 ON CONFLICT (instance, file) DO UPDATE SET size = EXCLUDED.size",
                &[&fence.instance, &name, &size],
            )
            .map_err(err)?;
            if !indexes.is_empty() {
                tx.execute(
                    "INSERT INTO clax_fleet_chunks (instance, file, idx, bytes)
                     SELECT $1, $2, i, b FROM unnest($3::bigint[], $4::bytea[]) AS u(i, b)
                     ON CONFLICT (instance, file, idx) DO UPDATE SET bytes = EXCLUDED.bytes",
                    &[&fence.instance, &name, &indexes, &bytes],
                )
                .map_err(err)?;
            }
            tx.execute(
                "DELETE FROM clax_fleet_chunks WHERE instance = $1 AND file = $2 AND idx >= $3",
                &[&fence.instance, &name, &keep],
            )
            .map_err(err)?;
            tx.commit().map_err(err)
        })?;
        let mut cache = self.cache();
        cache.retain(|(f, i), _| f != file || *i < keep_below);
        for (i, c) in chunks {
            cache.insert((file.to_string(), *i), Some(Arc::new(c.clone())));
        }
        Ok(())
    }

    fn delete(&self, file: &str) -> Result<(), StoreError> {
        let fence = self.fence.clone();
        let name = file.to_string();
        self.pool.run(move |c| {
            let mut tx = c.transaction().map_err(err)?;
            fence.check(&mut tx)?;
            for table in ["clax_fleet_chunks", "clax_fleet_files"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE instance = $1 AND file = $2"),
                    &[&fence.instance, &name],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)
        })?;
        self.cache().retain(|(f, _), _| f != file);
        Ok(())
    }
}
