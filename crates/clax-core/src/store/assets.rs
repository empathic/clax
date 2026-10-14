//! Per-artifact asset store: uploaded images, media, fonts, and data files served at /_blob/<id>.

use super::Store;
use crate::audit::{AuditCtx, AuditKind, AuditRecord, sha256_hex};
use crate::content::AssetKey;
use crate::model::Asset;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Row, params};
use std::path::PathBuf;

/// Largest accepted asset in bytes. The cap is inclusive: an asset of exactly this size is accepted.
pub const MAX_ASSET_BYTES: u64 = 20 * 1024 * 1024;

fn row_to_asset(r: &Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: r.get("id")?,
        artifact_id: r.get("artifact_id")?,
        content_type: r.get("content_type")?,
        size: r.get::<_, i64>("size")? as u64,
        ext: r.get("ext")?,
        created_at: r.get("created_at")?,
        person: r.get("person")?,
        page_authors: super::threads::json_list(r.get("page_authors")?),
    })
}

/// The entries of `dir`, or none when it does not exist.
pub(super) fn read_dir_or_empty(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => Ok(rd
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?),
        // Unsupported: a target with no filesystem (wasm32) has no files.
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(e.into()),
    }
}

/// Whether `content_type` (parameters after `;` ignored) is an accepted asset type.
pub fn is_supported(content_type: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    ct.starts_with("image/")
        || ct.starts_with("video/")
        || ct.starts_with("font/")
        || matches!(
            ct,
            "application/pdf"
                | "text/css"
                | "text/javascript"
                | "text/csv"
                | "text/markdown"
                | "application/json"
                | "text/plain"
        )
}

fn ext_for(content_type: &str) -> String {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    match ct {
        "text/plain" => "txt".into(),
        "image/jpeg" => "jpg".into(),
        "text/csv" => "csv".into(),
        "application/json" => "json".into(),
        "image/svg+xml" => "svg".into(),
        "text/css" => "css".into(),
        "font/woff2" => "woff2".into(),
        "application/pdf" => "pdf".into(),
        "text/javascript" => "js".into(),
        "text/markdown" => "md".into(),
        _ => mime_guess::get_mime_extensions_str(ct)
            .and_then(|e| e.first())
            .map(|s| s.to_string())
            .unwrap_or("bin".into()),
    }
}

const SELECT: &str = "SELECT id, artifact_id, content_type, size, ext, created_at, person,
    (SELECT json_group_array(pa.author) FROM page_authors pa
     WHERE pa.kind = 'asset' AND pa.id = assets.id) AS page_authors
    FROM assets";

/// An asset row with where its bytes belong on disk, as listed by
/// [`Store::list_all_asset_rows`].
#[derive(Clone, Debug, PartialEq)]
pub struct AssetRow {
    pub asset: Asset,
    /// Where the home's files keep its bytes ([`crate::content::fs`]); not
    /// checked for existence, and meaningful only for a store whose content
    /// is the home's files.
    pub path: PathBuf,
    /// Whether the owning artifact is soft-deleted.
    pub artifact_deleted: bool,
}

/// The `asset.upload` record of `asset` (audit spec §6.1), at its
/// creation; `sha256` is the hex hash of its bytes (`None` when they cannot
/// be read: the backfill of a missing blob).
pub(super) fn upload_record(asset: &Asset, sha256: Option<&str>) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::AssetUpload, asset.created_at.as_str())
        .with("asset_id", asset.id.as_str())
        .with("path", format!("/_blob/{}", asset.id))
        .with("sha256", sha256.map(|h| format!("sha256:{h}")))
        .with("size", asset.size)
        .with("content_type", asset.content_type.as_str());
    rec.ids.artifact = Some(asset.artifact_id.clone());
    rec
}

impl Store {
    /// Stores `bytes` as a new asset of artifact `id`.
    ///
    /// The bytes are written to the content store (whole, before they are visible there)
    /// before the row is inserted, so a visible row always has its bytes; if the insert
    /// fails the bytes are deleted (should that fail, they are unreferenced and harmless).
    /// The size cap ([`MAX_ASSET_BYTES`]) is inclusive. The row's transaction
    /// records `asset.upload` under `ctx`, with the bytes' SHA-256.
    ///
    /// # Errors
    /// `Invalid { code: "unsupported_type" }` for a type failing [`is_supported`],
    /// `Invalid { code: "asset_too_large" }` when `bytes` exceeds the cap,
    /// `NotFound` when the artifact does not exist, and I/O or database errors otherwise.
    pub fn add_asset(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<Asset> {
        self.add_asset_by(ctx, id, content_type, bytes, None, false)
    }

    /// [`Store::add_asset`] for an uploader: `person` is their person's
    /// public ID, `None` for the owner; `via_page` when a page uploads it,
    /// which records every author of the page with it (spec §8.2, M11).
    pub fn add_asset_by(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        content_type: &str,
        bytes: &[u8],
        person: Option<&str>,
        via_page: bool,
    ) -> Result<Asset> {
        self.block_on(self.add_asset_async(ctx, id, content_type, bytes, person, via_page))
    }

    /// [`Store::add_asset_by`], awaiting its content store.
    pub async fn add_asset_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        content_type: &str,
        bytes: &[u8],
        person: Option<&str>,
        via_page: bool,
    ) -> Result<Asset> {
        if !is_supported(content_type) {
            return Err(CoreError::invalid(
                "unsupported_type",
                format!("'{content_type}' is not an accepted asset type"),
            ));
        }
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(CoreError::invalid(
                "asset_too_large",
                format!("asset exceeds {MAX_ASSET_BYTES} bytes"),
            ));
        }
        self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
        let asset = Asset {
            id: new_ulid(),
            artifact_id: id.as_str().to_string(),
            content_type: content_type.to_string(),
            size: bytes.len() as u64,
            ext: ext_for(content_type),
            created_at: self.now(),
            person: person.map(str::to_string),
            page_authors: Vec::new(),
        };
        let key = AssetKey::of(&asset)?;
        self.content.put_asset(&key, bytes).await?;
        let rec = upload_record(&asset, Some(&sha256_hex(bytes)));
        let inserted = self.with_tx(|c| {
            c.execute("INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at, person) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![asset.id, asset.artifact_id, asset.content_type, asset.size as i64, asset.ext, asset.created_at, person])?;
            let page = if via_page {
                let a = super::artifacts::page_authors_in(c, &asset.artifact_id)?;
                super::threads::add_page_authors(c, "asset", &asset.id, &a)?;
                a
            } else {
                Vec::new()
            };
            self.record_audit(c, ctx, rec)?;
            Ok(page)
        });
        match inserted {
            Ok(page) => Ok(Asset {
                page_authors: page,
                ..asset
            }),
            Err(e) => {
                let _ = self.content.delete_asset(&key).await;
                Err(e)
            }
        }
    }

    /// Looks up an asset by ID. Returns `Ok(None)` when no such asset exists.
    ///
    /// # Errors
    /// Database errors only.
    pub fn get_asset(&self, asset_id: &str) -> Result<Option<Asset>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![asset_id],
                row_to_asset,
            )
            .optional()?)
        })
    }

    /// The asset `asset_id` with its bytes, read from the content store;
    /// `None` when there is no such asset or its bytes are missing.
    ///
    /// # Errors
    /// `Corrupt` for a row whose artifact ID is not valid, and database or
    /// content errors.
    pub fn read_asset(&self, asset_id: &str) -> Result<Option<(Asset, Vec<u8>)>> {
        self.block_on(self.read_asset_async(asset_id))
    }

    /// [`Store::read_asset`], awaiting its content store.
    pub async fn read_asset_async(&self, asset_id: &str) -> Result<Option<(Asset, Vec<u8>)>> {
        let Some(asset) = self.get_asset(asset_id)? else {
            return Ok(None);
        };
        Ok(self
            .asset_bytes_async(&asset)
            .await?
            .map(|bytes| (asset, bytes)))
    }

    /// The bytes of `asset`, a row read with [`Store::get_asset`], from the
    /// content store; `None` when they are missing. For a caller that
    /// checks the row before reading its bytes.
    ///
    /// # Errors
    /// `Corrupt` for a row whose artifact ID is not valid, and content
    /// errors.
    pub fn asset_bytes(&self, asset: &Asset) -> Result<Option<Vec<u8>>> {
        self.block_on(self.asset_bytes_async(asset))
    }

    /// [`Store::asset_bytes`], awaiting its content store.
    pub async fn asset_bytes_async(&self, asset: &Asset) -> Result<Option<Vec<u8>>> {
        self.content.read_asset(&AssetKey::of(asset)?).await
    }

    /// Lists the assets of artifact `id`, ordered by `created_at` then ID.
    ///
    /// Returns an empty list for an unknown artifact.
    ///
    /// # Errors
    /// Database errors only.
    pub fn list_assets(&self, id: &ArtifactId) -> Result<Vec<Asset>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE artifact_id = ?1 ORDER BY created_at, id"
            ))?;
            Ok(stmt
                .query_map(params![id.as_str()], row_to_asset)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Deletes an asset's bytes from the content store, then its row.
    /// Removing the row records `asset.delete` under `ctx` in its transaction.
    ///
    /// Bytes that are already missing are tolerated, so a retry after a content error
    /// still removes the row.
    ///
    /// # Errors
    /// `NotFound` when no such asset exists, and I/O or database errors otherwise.
    pub fn delete_asset(&self, ctx: &AuditCtx, asset_id: &str) -> Result<()> {
        self.block_on(self.delete_asset_async(ctx, asset_id))
    }

    /// [`Store::delete_asset`], awaiting its content store.
    pub async fn delete_asset_async(&self, ctx: &AuditCtx, asset_id: &str) -> Result<()> {
        let Some(asset) = self.get_asset(asset_id)? else {
            return Err(CoreError::NotFound);
        };
        self.content.delete_asset(&AssetKey::of(&asset)?).await?;
        self.with_tx(|c| {
            c.execute(
                "DELETE FROM page_authors WHERE kind = 'asset' AND id = ?1",
                params![asset_id],
            )?;
            let n = c.execute("DELETE FROM assets WHERE id = ?1", params![asset_id])?;
            if n > 0 {
                let mut rec = AuditRecord::new(AuditKind::AssetDelete, self.now())
                    .with("asset_id", asset.id.as_str())
                    .with("path", format!("/_blob/{}", asset.id))
                    .with("size", asset.size)
                    .with("content_type", asset.content_type.as_str());
                rec.ids.artifact = Some(asset.artifact_id.clone());
                self.record_audit(c, ctx, rec)?;
            }
            Ok(())
        })
    }

    /// Every asset row of every artifact, deleted ones included, ordered by ID.
    /// Rows whose artifact ID is not a valid ID are logged and left out.
    ///
    /// # Errors
    /// Database errors only.
    pub fn list_all_asset_rows(&self) -> Result<Vec<AssetRow>> {
        let rows = self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT a.id, a.artifact_id, a.content_type, a.size, a.ext, a.created_at, a.person,
                        NULL AS page_authors, r.deleted_at IS NOT NULL AS artifact_deleted
                 FROM assets a JOIN artifacts r ON r.id = a.artifact_id ORDER BY a.id",
            )?;
            Ok(stmt
                .query_map([], |r| {
                    Ok((row_to_asset(r)?, r.get::<_, bool>("artifact_deleted")?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|(asset, artifact_deleted)| {
                let Ok(id) = ArtifactId::parse(&asset.artifact_id) else {
                    tracing::warn!(
                        asset = asset.id,
                        artifact_id = asset.artifact_id,
                        "skipping asset row with an invalid artifact ID"
                    );
                    return None;
                };
                let path = crate::content::fs::asset_path(
                    &self.home,
                    &AssetKey {
                        artifact: id,
                        id: asset.id.clone(),
                        ext: asset.ext.clone(),
                    },
                );
                Some(AssetRow {
                    asset,
                    path,
                    artifact_deleted,
                })
            })
            .collect())
    }

    /// Leftover `*.tmp` files in any artifact's assets directory (an upload that
    /// died between write and rename), sorted.
    ///
    /// # Errors
    /// I/O errors other than a missing directory.
    pub fn stray_asset_temp_files(&self) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for artifact in read_dir_or_empty(&self.home.root().join("artifacts"))? {
            for entry in read_dir_or_empty(&artifact.join("assets"))? {
                if entry.extension().is_some_and(|e| e == "tmp") {
                    out.push(entry);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Removes the asset rows, and their bytes, of soft-deleted artifacts.
    /// Returns how many rows were removed. Missing bytes are tolerated.
    ///
    /// # Errors
    /// I/O or database errors.
    pub fn delete_assets_of_deleted_artifacts(&self) -> Result<usize> {
        self.block_on(self.delete_assets_of_deleted_artifacts_async())
    }

    /// [`Store::delete_assets_of_deleted_artifacts`], awaiting its content
    /// store.
    pub async fn delete_assets_of_deleted_artifacts_async(&self) -> Result<usize> {
        let mut n = 0;
        for row in self.list_all_asset_rows()? {
            if !row.artifact_deleted {
                continue;
            }
            self.content
                .delete_asset(&AssetKey::of(&row.asset)?)
                .await?;
            self.with_tx(|c| {
                c.execute(
                    "DELETE FROM page_authors WHERE kind = 'asset' AND id = ?1",
                    params![row.asset.id],
                )?;
                c.execute("DELETE FROM assets WHERE id = ?1", params![row.asset.id])?;
                Ok(())
            })?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::test_util::DAEMON;
    use crate::{CoreError, Store};

    use crate::store::test_util::store;

    use crate::store::test_util::pool_store;

    /// Where the home's files keep `a`'s bytes.
    fn path_of(store: &Store, a: &crate::model::Asset) -> std::path::PathBuf {
        crate::content::fs::asset_path(store.home(), &crate::content::AssetKey::of(a).unwrap())
    }

    /// `a`'s bytes in the store's content store.
    fn bytes_of(store: &Store, a: &crate::model::Asset) -> Option<Vec<u8>> {
        let key = crate::content::AssetKey::of(a).unwrap();
        crate::content::wait(store.content().read_asset(&key)).unwrap()
    }

    #[test]
    fn add_get_list_delete() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store
            .add_asset(DAEMON, &id, "image/png", &[1, 2, 3])
            .unwrap();
        assert_eq!(a.ext, "png");
        assert_eq!(a.size, 3);
        let got = store.get_asset(&a.id).unwrap().unwrap();
        assert_eq!(got, a);
        assert_eq!(bytes_of(&store, &a).unwrap(), vec![1, 2, 3]);
        assert_eq!(
            store.read_asset(&a.id).unwrap().unwrap(),
            (a.clone(), vec![1, 2, 3])
        );
        assert_eq!(store.list_assets(&id).unwrap().len(), 1);
        store.delete_asset(DAEMON, &a.id).unwrap();
        assert!(store.get_asset(&a.id).unwrap().is_none());
        assert_eq!(bytes_of(&store, &a), None);
        assert!(matches!(
            store.delete_asset(DAEMON, &a.id),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn rejects_unsupported_types_and_oversize() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        assert!(matches!(
            store
                .add_asset(DAEMON, &id, "application/x-msdownload", &[0])
                .unwrap_err(),
            CoreError::Invalid {
                code: "unsupported_type",
                ..
            }
        ));
        let big = vec![0u8; super::MAX_ASSET_BYTES as usize + 1];
        assert!(matches!(
            store.add_asset(DAEMON, &id, "image/png", &big).unwrap_err(),
            CoreError::Invalid {
                code: "asset_too_large",
                ..
            }
        ));
    }

    #[test]
    fn unknown_artifact_is_not_found() {
        let (_d, store) = store();
        let id = crate::ArtifactId::generate();
        assert!(matches!(
            store.add_asset(DAEMON, &id, "image/png", &[0]).unwrap_err(),
            CoreError::NotFound
        ));
    }

    #[test]
    fn extensions_are_sane_for_common_types() {
        for (ct, ext) in [
            ("text/plain", "txt"),
            ("image/jpeg", "jpg"),
            ("text/csv", "csv"),
            ("application/json", "json"),
            ("image/png", "png"),
            ("image/svg+xml", "svg"),
            ("application/pdf", "pdf"),
            ("text/css", "css"),
            ("font/woff2", "woff2"),
        ] {
            assert_eq!(super::ext_for(ct), ext, "{ct}");
        }
    }

    #[test]
    fn exact_cap_is_accepted_and_no_temp_file_remains() {
        // The home's files, which an upload writes through a temporary one.
        let (_d, store) = pool_store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let bytes = vec![0u8; super::MAX_ASSET_BYTES as usize];
        store.add_asset(DAEMON, &id, "image/png", &bytes).unwrap();
        let dir = store.home().assets_dir(&id);
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
    }

    #[test]
    fn failed_insert_leaves_no_file_behind() {
        let (_d, store) = pool_store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        store
            .with_write(|c| Ok(c.execute_batch("DROP TABLE assets")?))
            .unwrap();
        assert!(matches!(
            store.add_asset(DAEMON, &id, "image/png", &[1]).unwrap_err(),
            CoreError::Db(_)
        ));
        let dir = store.home().assets_dir(&id);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn delete_removes_row_even_if_file_is_already_gone() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store.add_asset(DAEMON, &id, "image/png", &[1]).unwrap();
        let key = crate::content::AssetKey::of(&a).unwrap();
        crate::content::wait(store.content().delete_asset(&key)).unwrap();
        assert!(store.read_asset(&a.id).unwrap().is_none());
        store.delete_asset(DAEMON, &a.id).unwrap();
        assert!(store.get_asset(&a.id).unwrap().is_none());
    }

    #[test]
    fn all_rows_report_owner_state_and_path() {
        // `AssetRow::path` names the home's files.
        let (_d, store) = pool_store();
        let live = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let gone = store.insert_artifact_for_test("B", "2026-01-01T00:00:00.000Z");
        let a = store.add_asset(DAEMON, &live, "image/png", &[1]).unwrap();
        let b = store
            .add_asset(DAEMON, &gone, "image/png", &[1, 2])
            .unwrap();
        store.delete_artifact(DAEMON, &gone).unwrap();
        let rows = store.list_all_asset_rows().unwrap();
        assert_eq!(rows.len(), 2);
        let ra = rows.iter().find(|r| r.asset.id == a.id).unwrap();
        let rb = rows.iter().find(|r| r.asset.id == b.id).unwrap();
        assert!(!ra.artifact_deleted && ra.path.exists());
        assert!(rb.artifact_deleted && !rb.path.exists());
    }

    #[test]
    fn stray_temp_files_are_listed() {
        let (_d, store) = pool_store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        assert!(store.stray_asset_temp_files().unwrap().is_empty());
        let a = store.add_asset(DAEMON, &id, "image/png", &[1]).unwrap();
        let tmp = store.home().assets_dir(&id).join("X.png.tmp");
        std::fs::write(&tmp, b"x").unwrap();
        assert_eq!(store.stray_asset_temp_files().unwrap(), vec![tmp]);
        assert!(path_of(&store, &a).exists());
    }

    #[test]
    fn deleted_artifacts_lose_their_asset_rows_and_files() {
        let (_d, store) = store();
        let live = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let gone = store.insert_artifact_for_test("B", "2026-01-01T00:00:00.000Z");
        let keep = store.add_asset(DAEMON, &live, "image/png", &[1]).unwrap();
        let drop_row = store.add_asset(DAEMON, &gone, "image/png", &[2]).unwrap();
        store.delete_artifact(DAEMON, &gone).unwrap();
        // Simulate a content removal that failed: the bytes are still there.
        let key = crate::content::AssetKey::of(&drop_row).unwrap();
        crate::content::wait(store.content().put_asset(&key, b"x")).unwrap();
        assert_eq!(store.delete_assets_of_deleted_artifacts().unwrap(), 1);
        assert_eq!(bytes_of(&store, &drop_row), None);
        assert!(store.get_asset(&drop_row.id).unwrap().is_none());
        assert!(store.get_asset(&keep.id).unwrap().is_some());
        assert_eq!(store.delete_assets_of_deleted_artifacts().unwrap(), 0);
    }
}
