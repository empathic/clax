//! Artifact metadata: read, patch, pin, delete. Creation and versions live in
//! `publish.rs` because they need validated input.

use super::Store;
use crate::model::{Artifact, CONTRACT_VERSION};
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Row, params};

#[derive(Debug, Default, Clone)]
pub struct MetaPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub pinned: Option<bool>,
}

pub(crate) fn row_to_artifact(r: &Row<'_>) -> rusqlite::Result<Artifact> {
    let caps: String = r.get("capabilities_json")?;
    Ok(Artifact {
        id: r.get("id")?,
        title: r.get("title")?,
        description: r.get("description")?,
        icon: r.get("icon")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        current_version: r.get("current_version")?,
        pinned: r.get::<_, i64>("pinned")? != 0,
        capabilities: serde_json::from_str(&caps).unwrap_or(serde_json::json!({})),
        contract_version: r.get("contract_version")?,
        owner_session_id: r.get("owner_session_id")?,
    })
}

const SELECT: &str = "SELECT id, title, description, icon, created_at, updated_at, current_version,
    owner_session_id, pinned, capabilities_json, contract_version FROM artifacts";

impl Store {
    pub fn get_artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1 AND deleted_at IS NULL"),
                params![id.as_str()],
                row_to_artifact,
            )
            .optional()?)
        })
    }

    pub fn list_artifacts(&self) -> Result<Vec<Artifact>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE deleted_at IS NULL ORDER BY pinned DESC, updated_at DESC"
            ))?;
            let rows = stmt.query_map([], row_to_artifact)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn set_pinned(&self, id: &ArtifactId, pinned: bool) -> Result<Artifact> {
        self.update_meta(
            id,
            MetaPatch {
                pinned: Some(pinned),
                ..Default::default()
            },
        )
    }

    pub fn update_meta(&self, id: &ArtifactId, patch: MetaPatch) -> Result<Artifact> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE artifacts SET
                    title = COALESCE(?2, title),
                    description = COALESCE(?3, description),
                    icon = COALESCE(?4, icon),
                    pinned = COALESCE(?5, pinned)
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![
                    id.as_str(),
                    patch.title,
                    patch.description,
                    patch.icon,
                    patch.pinned.map(|b| b as i64)
                ],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )?)
        })
    }

    pub fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE artifacts SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id.as_str(), Store::now()],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(())
        })?;
        let dir = self.home.artifact_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn insert_artifact_for_test(&self, title: &str, at: &str) -> ArtifactId {
        let id = ArtifactId::generate();
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES (?1, ?2, ?3, ?3, 1, ?4)",
                params![id.as_str(), title, at, CONTRACT_VERSION],
            )?;
            Ok(())
        })
        .unwrap();
        id
    }
}

#[cfg(test)]
mod tests {
    use crate::store::artifacts::MetaPatch;
    use crate::{Home, Store};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = Store::open(&home).unwrap();
        (dir, store)
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        Store::open(&home).unwrap();
        Store::open(&home).unwrap();
        assert!(home.db_path().exists());
    }

    #[test]
    fn list_orders_pinned_first_then_recent() {
        let (_d, store) = store();
        let a = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let b = store.insert_artifact_for_test("B", "2026-01-02T00:00:00.000Z");
        let c = store.insert_artifact_for_test("C", "2026-01-03T00:00:00.000Z");
        store.set_pinned(&a, true).unwrap();
        let titles: Vec<String> = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .map(|x| x.title)
            .collect();
        assert_eq!(titles, vec!["A", "C", "B"]);
        let _ = (b, c);
    }

    #[test]
    fn update_meta_patches_only_given_fields() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store
            .update_meta(
                &id,
                MetaPatch {
                    description: Some("d".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(a.title, "A");
        assert_eq!(a.description.as_deref(), Some("d"));
        assert!(!a.pinned);
    }

    #[test]
    fn delete_hides_from_get_and_list_and_removes_dir() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let dir = store.home().artifact_dir(&id);
        std::fs::create_dir_all(&dir).unwrap();
        store.delete_artifact(&id).unwrap();
        assert!(store.get_artifact(&id).unwrap().is_none());
        assert!(store.list_artifacts().unwrap().is_empty());
        assert!(!dir.exists());
        assert!(matches!(
            store.delete_artifact(&id),
            Err(crate::CoreError::NotFound)
        ));
    }

    #[test]
    fn migrations_run_in_transaction_and_version_bumps() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = Store::open(&home).unwrap();

        // After first open, user_version should be 1
        let version: u32 = store
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, 1);

        // Opening a second time should not re-run migrations or error
        let store2 = Store::open(&home).unwrap();
        let version2: u32 = store2
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version2, 1);
    }
}
