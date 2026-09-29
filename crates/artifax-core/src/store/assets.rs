//! Per-artifact asset store: uploaded images, media, fonts, and data files served at /_blob/<id>.

use super::Store;
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
    })
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

const SELECT: &str = "SELECT id, artifact_id, content_type, size, ext, created_at FROM assets";

impl Store {
    /// Stores `bytes` as a new asset of artifact `id`.
    ///
    /// The bytes are written to `<id>.<ext>.tmp` and renamed into place before the row is
    /// inserted, so a visible row always has its file; if the insert fails the file is
    /// removed (should that removal fail, the orphaned file is unreferenced and harmless).
    /// The size cap ([`MAX_ASSET_BYTES`]) is inclusive.
    ///
    /// # Errors
    /// `Invalid { code: "unsupported_type" }` for a type failing [`is_supported`],
    /// `Invalid { code: "asset_too_large" }` when `bytes` exceeds the cap,
    /// `NotFound` when the artifact does not exist, and I/O or database errors otherwise.
    pub fn add_asset(&self, id: &ArtifactId, content_type: &str, bytes: &[u8]) -> Result<Asset> {
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
            created_at: Store::now(),
        };
        let dir = self.home.assets_dir(id);
        std::fs::create_dir_all(&dir)?;
        let final_path = dir.join(format!("{}.{}", asset.id, asset.ext));
        let tmp_path = dir.join(format!("{}.{}.tmp", asset.id, asset.ext));
        std::fs::write(&tmp_path, bytes)?;
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        let inserted = self.with_conn(|c| {
            c.execute("INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![asset.id, asset.artifact_id, asset.content_type, asset.size as i64, asset.ext, asset.created_at])?;
            Ok(())
        });
        if let Err(e) = inserted {
            let _ = std::fs::remove_file(&final_path);
            return Err(e);
        }
        Ok(asset)
    }

    /// Looks up an asset by ID and returns it with the path of its bytes on disk.
    ///
    /// Returns `Ok(None)` when no such asset exists; the path is not checked for existence.
    ///
    /// # Errors
    /// Database errors only.
    pub fn get_asset(&self, asset_id: &str) -> Result<Option<(Asset, PathBuf)>> {
        let a = self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![asset_id],
                row_to_asset,
            )
            .optional()?)
        })?;
        Ok(a.map(|a| {
            let id = ArtifactId::parse(&a.artifact_id).expect("stored id is valid");
            let path = self
                .home
                .assets_dir(&id)
                .join(format!("{}.{}", a.id, a.ext));
            (a, path)
        }))
    }

    /// Lists the assets of artifact `id`, ordered by `created_at` then ID.
    ///
    /// Returns an empty list for an unknown artifact.
    ///
    /// # Errors
    /// Database errors only.
    pub fn list_assets(&self, id: &ArtifactId) -> Result<Vec<Asset>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE artifact_id = ?1 ORDER BY created_at, id"
            ))?;
            Ok(stmt
                .query_map(params![id.as_str()], row_to_asset)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Deletes an asset's file, then its row.
    ///
    /// A file that is already missing is tolerated, so a retry after a file error still
    /// removes the row.
    ///
    /// # Errors
    /// `NotFound` when no such asset exists, and I/O or database errors otherwise.
    pub fn delete_asset(&self, asset_id: &str) -> Result<()> {
        let Some((_, path)) = self.get_asset(asset_id)? else {
            return Err(CoreError::NotFound);
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.with_conn(|c| {
            c.execute("DELETE FROM assets WHERE id = ?1", params![asset_id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{CoreError, Home, Store};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    #[test]
    fn add_get_list_delete() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store.add_asset(&id, "image/png", &[1, 2, 3]).unwrap();
        assert_eq!(a.ext, "png");
        assert_eq!(a.size, 3);
        let (got, path) = store.get_asset(&a.id).unwrap().unwrap();
        assert_eq!(got, a);
        assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3]);
        assert_eq!(store.list_assets(&id).unwrap().len(), 1);
        store.delete_asset(&a.id).unwrap();
        assert!(store.get_asset(&a.id).unwrap().is_none());
        assert!(!path.exists());
        assert!(matches!(
            store.delete_asset(&a.id),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn rejects_unsupported_types_and_oversize() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        assert!(matches!(
            store
                .add_asset(&id, "application/x-msdownload", &[0])
                .unwrap_err(),
            CoreError::Invalid {
                code: "unsupported_type",
                ..
            }
        ));
        let big = vec![0u8; super::MAX_ASSET_BYTES as usize + 1];
        assert!(matches!(
            store.add_asset(&id, "image/png", &big).unwrap_err(),
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
            store.add_asset(&id, "image/png", &[0]).unwrap_err(),
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
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let bytes = vec![0u8; super::MAX_ASSET_BYTES as usize];
        store.add_asset(&id, "image/png", &bytes).unwrap();
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
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        store
            .with_conn(|c| Ok(c.execute_batch("DROP TABLE assets")?))
            .unwrap();
        assert!(matches!(
            store.add_asset(&id, "image/png", &[1]).unwrap_err(),
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
        let a = store.add_asset(&id, "image/png", &[1]).unwrap();
        let (_, path) = store.get_asset(&a.id).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        store.delete_asset(&a.id).unwrap();
        assert!(store.get_asset(&a.id).unwrap().is_none());
    }
}
