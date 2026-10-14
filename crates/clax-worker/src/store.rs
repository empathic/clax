//! A [`PageStore`] in a SQLite-backed Durable Object's own SQL storage
//! (`ctx.storage.sql`), committing through `ctx.storage.transactionSync`.
//!
//! The storage keeps two tables:
//!
//! - `_clax_vfs_files(file, size)`: each database file's size in bytes;
//! - `_clax_vfs_chunks(file, idx, data)`: chunk `idx` of the file, as a
//!   blob of [`clax_do_vfs::CHUNK_SIZE`] bytes.
//!
//! Every call is synchronous: `sql.exec` and `transactionSync` run within
//! the caller's turn, so the Rust SQLite connection on top stays synchronous.
//! The Durable Object's output gate holds every response and outgoing
//! message until the storage has made the turn's writes durable.

use std::rc::Rc;

use clax_do_vfs::{Chunks, PageStore, StoreError};
use js_sys::{Function, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use worker::{SqlStorage, SqlStorageValue, Storage};

/// Database files in one Durable Object's storage.
pub struct DoPageStore {
    storage: Storage,
    sql: SqlStorage,
}

fn err(e: impl std::fmt::Display) -> StoreError {
    e.to_string()
}

fn js_err(e: JsValue) -> StoreError {
    e.as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{e:?}"))
}

/// Runs a statement to completion, discarding any rows.
fn run(sql: &SqlStorage, query: &str, args: Vec<JsValue>) -> worker::Result<()> {
    for row in sql.exec_raw(query, args)?.raw() {
        row?;
    }
    Ok(())
}

impl DoPageStore {
    /// The store in `storage`, creating its tables if they are absent.
    pub fn new(storage: Storage) -> worker::Result<Self> {
        let sql = storage.sql();
        run(
            &sql,
            "CREATE TABLE IF NOT EXISTS _clax_vfs_files (
                 file TEXT PRIMARY KEY, size INTEGER NOT NULL)",
            vec![],
        )?;
        run(
            &sql,
            "CREATE TABLE IF NOT EXISTS _clax_vfs_chunks (
                 file TEXT NOT NULL, idx INTEGER NOT NULL, data BLOB NOT NULL,
                 PRIMARY KEY (file, idx)) WITHOUT ROWID",
            vec![],
        )?;
        Ok(DoPageStore { storage, sql })
    }

    /// Chunk rows the store holds for `file`.
    pub fn chunk_rows(&self, file: &str) -> worker::Result<i64> {
        let rows = self
            .sql
            .exec(
                "SELECT count(*) FROM _clax_vfs_chunks WHERE file = ?",
                vec![file.into()],
            )?
            .raw()
            .next()
            .transpose()?;
        Ok(match rows.as_deref() {
            Some([SqlStorageValue::Integer(n)]) => *n,
            _ => 0,
        })
    }

    /// Bytes of the Durable Object's whole SQLite storage.
    pub fn database_size(&self) -> usize {
        self.sql.database_size()
    }

    /// Runs `f` inside `ctx.storage.transactionSync`: if it fails, the
    /// storage rolls back every write it made.
    fn transaction_sync(
        &self,
        f: impl FnOnce() -> Result<(), JsValue> + 'static,
    ) -> Result<(), StoreError> {
        let raw: &JsValue = self.storage.as_raw();
        let run_sync: Function = Reflect::get(raw, &"transactionSync".into())
            .map_err(js_err)?
            .dyn_into()
            .map_err(|_| "ctx.storage.transactionSync is not a function".to_owned())?;
        let mut f = Some(f);
        let callback = Closure::<dyn FnMut() -> Result<(), JsValue>>::new(move || {
            f.take().map_or(Ok(()), |f| f())
        });
        run_sync
            .call1(raw, callback.as_ref())
            .map(drop)
            .map_err(js_err)
    }
}

impl PageStore for DoPageStore {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        let row = self
            .sql
            .exec(
                "SELECT size FROM _clax_vfs_files WHERE file = ?",
                vec![file.into()],
            )
            .map_err(err)?
            .raw()
            .next()
            .transpose()
            .map_err(err)?;
        match row.as_deref() {
            None => Ok(None),
            Some([SqlStorageValue::Integer(n)]) => Ok(Some(*n as u64)),
            Some(other) => Err(format!("{file}: unexpected size row {other:?}")),
        }
    }

    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        let row = self
            .sql
            .exec(
                "SELECT data FROM _clax_vfs_chunks WHERE file = ? AND idx = ?",
                vec![file.into(), (index as i64).into()],
            )
            .map_err(err)?
            .raw()
            .next()
            .transpose()
            .map_err(err)?;
        match row {
            None => Ok(None),
            Some(mut v) => match v.pop() {
                Some(SqlStorageValue::Blob(b)) => Ok(Some(b)),
                other => Err(format!("{file}#{index}: unexpected chunk {other:?}")),
            },
        }
    }

    fn commit(
        &self,
        file: &str,
        size: u64,
        chunks: &Rc<Chunks>,
        keep_below: u64,
    ) -> Result<(), StoreError> {
        // The closure must be 'static, so it holds a clone of the `Rc`,
        // dropped when `transaction_sync` drops the closure. Each chunk is
        // copied into a JS array only for its own statement, so at most one
        // chunk is held twice at a time.
        let file = JsValue::from(file);
        let chunks = Rc::clone(chunks);
        let sql = self.sql.clone();
        self.transaction_sync(move || {
            let e = |e: worker::Error| JsValue::from(js_sys::Error::new(&e.to_string()));
            for (idx, data) in chunks.iter() {
                run(
                    &sql,
                    "INSERT INTO _clax_vfs_chunks (file, idx, data) VALUES (?, ?, ?)
                     ON CONFLICT (file, idx) DO UPDATE SET data = excluded.data",
                    vec![
                        file.clone(),
                        JsValue::from(*idx as f64),
                        Uint8Array::from(data.as_slice()).into(),
                    ],
                )
                .map_err(e)?;
            }
            run(
                &sql,
                "DELETE FROM _clax_vfs_chunks WHERE file = ? AND idx >= ?",
                vec![file.clone(), JsValue::from(keep_below as f64)],
            )
            .map_err(e)?;
            run(
                &sql,
                "INSERT INTO _clax_vfs_files (file, size) VALUES (?, ?)
                 ON CONFLICT (file) DO UPDATE SET size = excluded.size",
                vec![file, JsValue::from(size as f64)],
            )
            .map_err(e)
        })
    }

    fn delete(&self, file: &str) -> Result<(), StoreError> {
        let file = JsValue::from(file);
        let sql = self.sql.clone();
        self.transaction_sync(move || {
            let e = |e: worker::Error| JsValue::from(js_sys::Error::new(&e.to_string()));
            run(
                &sql,
                "DELETE FROM _clax_vfs_chunks WHERE file = ?",
                vec![file.clone()],
            )
            .map_err(e)?;
            run(
                &sql,
                "DELETE FROM _clax_vfs_files WHERE file = ?",
                vec![file],
            )
            .map_err(e)
        })
    }
}
