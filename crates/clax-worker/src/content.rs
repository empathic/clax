//! Where an instance keeps artifact content: the seam between the Durable
//! Object and a [`ContentStore`].
//!
//! [`content_store`] picks the backend by the `CLAX_CONTENT` variable:
//!
//! - `storage` (the default): [`DoContent`], every version file and asset in
//!   the object's own SQL storage, beside the database, so content outlives
//!   the isolate;
//! - `memory`: clax-core's [`MemContent`], gone with the isolate (the
//!   database still lists the versions, whose files then 404).
//!
//! A git backend (one repository per artifact, spec
//! docs/superpowers/specs/2026-10-11-clax-dev-hosted-design.md) implements
//! the same trait and takes its place here.
//!
//! Both finish every call when first polled: the store's synchronous paths
//! drive content futures with `clax_core::content::wait`, which on wasm32
//! accepts no other kind.

use std::rc::Rc;
use std::sync::Arc;

use clax_core::content::memory::MemContent;
use clax_core::content::{AssetKey, ContentStore, VersionWrite};
use clax_core::{ArtifactId, CoreError, Result};
use js_sys::Uint8Array;
use wasm_bindgen::JsValue;
use worker::send::SendWrapper;
use worker::{SqlStorage, SqlStorageValue, Storage};

use crate::store::{run, transaction_sync};

/// The bytes of one stored row: SQL storage holds at most 2 MB a value.
const PART: usize = 1 << 20;

/// The content store `CLAX_CONTENT` names (see the module docs).
///
/// # Errors
/// An unknown name, or the storage's tables cannot be made.
pub fn content_store(state: &worker::State, name: &str) -> worker::Result<Arc<dyn ContentStore>> {
    match name {
        "" | "storage" => Ok(Arc::new(DoContent::new(state.storage())?)),
        "memory" => Ok(Arc::new(MemContent::new())),
        other => Err(worker::Error::RustError(format!(
            "CLAX_CONTENT is \"{other}\": use \"storage\" or \"memory\""
        ))),
    }
}

/// Artifact content in a Durable Object's SQL storage:
///
/// - `_clax_content_versions(artifact, n)`: each version present;
/// - `_clax_content_files(artifact, n, path, part, data)`: a version
///   file's bytes, in [`PART`]-byte parts;
/// - `_clax_content_assets(artifact, id, ext, part, data)`: an asset's.
///
/// Each write runs in one `transactionSync`, so it is all or nothing.
pub struct DoContent {
    inner: SendWrapper<Rc<Inner>>,
}

struct Inner {
    storage: Storage,
    sql: SqlStorage,
}

fn err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Io(std::io::Error::other(e.to_string()))
}

fn js(e: worker::Error) -> JsValue {
    JsValue::from(js_sys::Error::new(&e.to_string()))
}

fn int(n: impl Into<f64>) -> JsValue {
    JsValue::from(n.into())
}

impl DoContent {
    /// The store in `storage`, making its tables when absent.
    ///
    /// # Errors
    /// The storage's.
    pub fn new(storage: Storage) -> worker::Result<DoContent> {
        let sql = storage.sql();
        for ddl in [
            "CREATE TABLE IF NOT EXISTS _clax_content_versions (
                 artifact TEXT NOT NULL, n INTEGER NOT NULL,
                 PRIMARY KEY (artifact, n)) WITHOUT ROWID",
            "CREATE TABLE IF NOT EXISTS _clax_content_files (
                 artifact TEXT NOT NULL, n INTEGER NOT NULL, path TEXT NOT NULL,
                 part INTEGER NOT NULL, data BLOB NOT NULL,
                 PRIMARY KEY (artifact, n, path, part)) WITHOUT ROWID",
            "CREATE TABLE IF NOT EXISTS _clax_content_assets (
                 artifact TEXT NOT NULL, id TEXT NOT NULL, ext TEXT NOT NULL,
                 part INTEGER NOT NULL, data BLOB NOT NULL,
                 PRIMARY KEY (artifact, id, ext, part)) WITHOUT ROWID",
        ] {
            run(&sql, ddl, vec![])?;
        }
        Ok(DoContent {
            inner: SendWrapper::new(Rc::new(Inner { storage, sql })),
        })
    }

    /// The parts `query` selects (one `data` column, in order), joined;
    /// `None` when there are none.
    fn read_parts(&self, query: &str, args: Vec<JsValue>) -> Result<Option<Vec<u8>>> {
        let mut out: Option<Vec<u8>> = None;
        for row in self.inner.sql.exec_raw(query, args).map_err(err)?.raw() {
            match row.map_err(err)?.pop() {
                Some(SqlStorageValue::Blob(b)) => out.get_or_insert_with(Vec::new).extend(b),
                other => return Err(err(format!("unexpected content row {other:?}"))),
            }
        }
        Ok(out)
    }

    fn version_present(&self, id: &ArtifactId, n: u32) -> Result<bool> {
        let row = self
            .inner
            .sql
            .exec_raw(
                "SELECT 1 FROM _clax_content_versions WHERE artifact = ? AND n = ?",
                vec![id.as_str().into(), int(n)],
            )
            .map_err(err)?
            .raw()
            .next()
            .transpose()
            .map_err(err)?;
        Ok(row.is_some())
    }

    /// Runs `f` on the storage's SQL inside `transactionSync`.
    fn write(&self, f: impl FnOnce(&SqlStorage) -> worker::Result<()> + 'static) -> Result<()> {
        let sql = self.inner.sql.clone();
        transaction_sync(&self.inner.storage, move || f(&sql).map_err(js)).map_err(err)
    }
}

/// Inserts `bytes` as the parts of one file, through `insert` (whose last
/// two parameters are the part's index and its bytes).
fn put_parts(sql: &SqlStorage, insert: &str, key: &[JsValue], bytes: &[u8]) -> worker::Result<()> {
    // An empty file still has its one, empty, part.
    let parts: Vec<&[u8]> = if bytes.is_empty() {
        vec![&[]]
    } else {
        bytes.chunks(PART).collect()
    };
    for (i, part) in parts.into_iter().enumerate() {
        let mut args = key.to_vec();
        args.push(int(i as f64));
        args.push(Uint8Array::from(part).into());
        run(sql, insert, args)?;
    }
    Ok(())
}

const INSERT_FILE: &str =
    "INSERT INTO _clax_content_files (artifact, n, path, part, data) VALUES (?, ?, ?, ?, ?)";

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl ContentStore for DoContent {
    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        self.read_parts(
            "SELECT data FROM _clax_content_files WHERE artifact = ? AND n = ? AND path = ? ORDER BY part",
            vec![id.as_str().into(), int(n), path.into()],
        )
    }

    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        if self.version_present(id, n)? {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("version {n} of {id} is already present"),
            )));
        }
        for path in &write.carry {
            if self.read_file(id, n - 1, path).await?.is_none() {
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{path} is not in version {} of {id}", n - 1),
                )));
            }
        }
        let aid = id.as_str().to_string();
        let carry: Vec<String> = write.carry.iter().map(|p| p.to_string()).collect();
        let put: Vec<(String, Vec<u8>)> = write
            .put
            .iter()
            .map(|f| (f.path.to_string(), f.bytes.to_vec()))
            .collect();
        self.write(move |sql| {
            run(
                sql,
                "INSERT INTO _clax_content_versions (artifact, n) VALUES (?, ?)",
                vec![aid.as_str().into(), int(n)],
            )?;
            for path in &carry {
                run(
                    sql,
                    "INSERT INTO _clax_content_files (artifact, n, path, part, data)
                     SELECT artifact, ?, path, part, data FROM _clax_content_files
                     WHERE artifact = ? AND n = ? AND path = ?",
                    vec![
                        int(n),
                        aid.as_str().into(),
                        int(n - 1),
                        path.as_str().into(),
                    ],
                )?;
            }
            for (path, bytes) in &put {
                put_parts(
                    sql,
                    INSERT_FILE,
                    &[aid.as_str().into(), int(n), path.as_str().into()],
                    bytes,
                )?;
            }
            Ok(())
        })
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        let aid = id.as_str().to_string();
        self.write(move |sql| {
            for table in ["_clax_content_files", "_clax_content_versions"] {
                run(
                    sql,
                    &format!("DELETE FROM {table} WHERE artifact = ? AND n = ?"),
                    vec![aid.as_str().into(), int(n)],
                )?;
            }
            Ok(())
        })
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        let aid = id.as_str().to_string();
        self.write(move |sql| {
            for table in [
                "_clax_content_files",
                "_clax_content_versions",
                "_clax_content_assets",
            ] {
                run(
                    sql,
                    &format!("DELETE FROM {table} WHERE artifact = ?"),
                    vec![aid.as_str().into()],
                )?;
            }
            Ok(())
        })
    }

    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        let k = [
            key.artifact.as_str().to_string(),
            key.id.clone(),
            key.ext.clone(),
        ];
        let bytes = bytes.to_vec();
        self.write(move |sql| {
            let args: Vec<JsValue> = k.iter().map(|s| JsValue::from(s.as_str())).collect();
            run(
                sql,
                "DELETE FROM _clax_content_assets WHERE artifact = ? AND id = ? AND ext = ?",
                args.clone(),
            )?;
            put_parts(
                sql,
                "INSERT INTO _clax_content_assets (artifact, id, ext, part, data) VALUES (?, ?, ?, ?, ?)",
                &args,
                &bytes,
            )
        })
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        self.read_parts(
            "SELECT data FROM _clax_content_assets WHERE artifact = ? AND id = ? AND ext = ? ORDER BY part",
            vec![
                key.artifact.as_str().into(),
                key.id.as_str().into(),
                key.ext.as_str().into(),
            ],
        )
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        let args = [
            key.artifact.as_str().to_string(),
            key.id.clone(),
            key.ext.clone(),
        ];
        self.write(move |sql| {
            run(
                sql,
                "DELETE FROM _clax_content_assets WHERE artifact = ? AND id = ? AND ext = ?",
                args.iter().map(|s| JsValue::from(s.as_str())).collect(),
            )
        })
    }
}
