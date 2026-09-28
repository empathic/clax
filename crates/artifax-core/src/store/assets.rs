//! Per-artifact asset store: uploaded images, media, fonts, and data files served at /_blob/<id>.

use super::Store;
use crate::model::Asset;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Row, params};
use std::path::PathBuf;

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
        std::fs::write(dir.join(format!("{}.{}", asset.id, asset.ext)), bytes)?;
        self.with_conn(|c| {
            c.execute("INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![asset.id, asset.artifact_id, asset.content_type, asset.size as i64, asset.ext, asset.created_at])?;
            Ok(())
        })?;
        Ok(asset)
    }

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

    pub fn list_assets(&self, id: &ArtifactId) -> Result<Vec<Asset>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE artifact_id = ?1 ORDER BY created_at"
            ))?;
            Ok(stmt
                .query_map(params![id.as_str()], row_to_asset)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn delete_asset(&self, asset_id: &str) -> Result<()> {
        let Some((_, path)) = self.get_asset(asset_id)? else {
            return Err(CoreError::NotFound);
        };
        self.with_conn(|c| {
            c.execute("DELETE FROM assets WHERE id = ?1", params![asset_id])?;
            Ok(())
        })?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
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
}
