//! Artifact metadata: read, patch, pin, delete. Creation and versions live in
//! `publish.rs` because they need validated input.

use super::Store;
use crate::model::{Artifact, CONTRACT_VERSION, FileMeta, Version};
use crate::publish::{FileChange, INDEX, ValidatedPublish};
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Row, params};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
                &format!("{SELECT} WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0"),
                params![id.as_str()],
                row_to_artifact,
            )
            .optional()?)
        })
    }

    pub fn list_artifacts(&self) -> Result<Vec<Artifact>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE deleted_at IS NULL AND current_version > 0 ORDER BY pinned DESC, updated_at DESC"
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

fn row_to_version(r: &Row<'_>) -> rusqlite::Result<Version> {
    let files: String = r.get("files_json")?;
    Ok(Version {
        artifact_id: r.get("artifact_id")?,
        n: r.get("n")?,
        label: r.get("label")?,
        created_at: r.get("created_at")?,
        session_id: r.get("session_id")?,
        files: serde_json::from_str(&files).unwrap_or_default(),
    })
}

const SELECT_VERSION: &str =
    "SELECT artifact_id, n, label, created_at, session_id, files_json FROM versions";

impl Store {
    /// Creates an artifact and writes its version 1. `p.files` are all stored;
    /// nothing is carried forward. The artifact is invisible until version 1 is
    /// recorded.
    pub fn create_artifact(&self, p: ValidatedPublish) -> Result<(Artifact, Version)> {
        let id = ArtifactId::generate();
        let now = Store::now();
        let title = p
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Untitled".to_string());
        let caps = p.capabilities.clone().unwrap_or(serde_json::json!({}));
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO artifacts (id, title, description, icon, created_at, updated_at, current_version,
                    pinned, capabilities_json, contract_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, ?6, ?7)",
                params![id.as_str(), title, p.description, p.icon, now, caps.to_string(), CONTRACT_VERSION],
            )?;
            Ok(())
        })?;
        self.write_version(&id, 1, &p, &BTreeMap::new())
    }

    /// Publishes the next version of an existing artifact. `p.if_version` must
    /// equal the current version (`Conflict` otherwise, `if_version_required`
    /// when absent). Files of the previous version not named in `p.files` are
    /// carried forward; `Remove` entries drop a path; `index.html` is always
    /// taken from `p`.
    pub fn publish_version(
        &self,
        id: &ArtifactId,
        p: ValidatedPublish,
    ) -> Result<(Artifact, Version)> {
        let current = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
        let Some(expected) = p.if_version else {
            return Err(CoreError::invalid(
                "if_version_required",
                "if_version is required when updating an artifact",
            ));
        };
        if expected != current.current_version {
            return Err(CoreError::Conflict {
                current: current.current_version,
            });
        }
        let prev = self
            .get_version(id, current.current_version)?
            .map(|v| v.files)
            .unwrap_or_default();
        self.write_version(id, current.current_version + 1, &p, &prev)
    }

    /// Writes files for version `n`, carrying forward `prev` entries not named in `p.files`,
    /// then records the version and bumps the artifact in one transaction.
    fn write_version(
        &self,
        id: &ArtifactId,
        n: u32,
        p: &ValidatedPublish,
        prev: &BTreeMap<String, FileMeta>,
    ) -> Result<(Artifact, Version)> {
        let vdir = self.home.version_dir(id, n);
        let files_dir = vdir.join("files");
        std::fs::create_dir_all(&files_dir)?;
        let write = |path: &str, bytes: &[u8]| -> Result<()> {
            let dest = if path == INDEX {
                vdir.join(INDEX)
            } else {
                files_dir.join(path)
            };
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(dest, bytes)?;
            Ok(())
        };
        let mut files: BTreeMap<String, FileMeta> = BTreeMap::new();
        for (path, meta) in prev {
            if path == INDEX || p.files.contains_key(path) {
                continue;
            }
            let src = self.home.version_dir(id, n - 1).join("files").join(path);
            let bytes = std::fs::read(&src)?;
            write(path, &bytes)?;
            files.insert(path.clone(), meta.clone());
        }
        for (path, change) in &p.files {
            if let FileChange::Put(f) = change {
                write(path, &f.bytes)?;
                files.insert(
                    path.clone(),
                    FileMeta {
                        content_type: f.content_type.clone(),
                        size: f.bytes.len() as u64,
                    },
                );
            }
        }
        let now = Store::now();
        let files_json = serde_json::to_string(&files).expect("serialisable map");
        let result = self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![id.as_str(), n, p.label, now, files_json],
            )?;
            tx.execute(
                "UPDATE artifacts SET current_version = ?2, updated_at = ?3,
                    title = COALESCE(?4, title), description = COALESCE(?5, description), icon = COALESCE(?6, icon),
                    capabilities_json = COALESCE(?7, capabilities_json)
                 WHERE id = ?1",
                params![
                    id.as_str(),
                    n,
                    now,
                    p.title,
                    p.description,
                    p.icon,
                    p.capabilities.as_ref().map(|c| c.to_string())
                ],
            )?;
            let a = tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )?;
            let v = tx.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                params![id.as_str(), n],
                row_to_version,
            )?;
            Ok((a, v))
        });
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&vdir);
        }
        result
    }

    pub fn get_version(&self, id: &ArtifactId, n: u32) -> Result<Option<Version>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                params![id.as_str(), n],
                row_to_version,
            )
            .optional()?)
        })
    }

    pub fn list_versions(&self, id: &ArtifactId) -> Result<Vec<Version>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT_VERSION} WHERE artifact_id = ?1 ORDER BY n"
            ))?;
            Ok(stmt
                .query_map(params![id.as_str()], row_to_version)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The on-disk path and metadata of `path` in version `n`, only for files
    /// recorded in that version.
    pub fn file_path(
        &self,
        id: &ArtifactId,
        n: u32,
        path: &str,
    ) -> Result<Option<(PathBuf, FileMeta)>> {
        let Some(v) = self.get_version(id, n)? else {
            return Ok(None);
        };
        let Some(meta) = v.files.get(path) else {
            return Ok(None);
        };
        let vdir = self.home.version_dir(id, n);
        let p = if path == INDEX {
            vdir.join(INDEX)
        } else {
            vdir.join("files").join(path)
        };
        Ok(Some((p, meta.clone())))
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

    use crate::publish::{Encoding, FileInput, PublishRequest, validate};
    use std::collections::BTreeMap;

    fn publish(
        files: &[(&str, Option<&str>)],
        if_version: Option<u32>,
    ) -> crate::publish::ValidatedPublish {
        let files = files
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.map(|s| FileInput {
                        content: s.to_string(),
                        encoding: Encoding::Utf8,
                        content_type: None,
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        validate(PublishRequest {
            title: Some("T".into()),
            description: None,
            icon: None,
            label: None,
            if_version,
            capabilities: None,
            files,
        })
        .unwrap()
    }

    #[test]
    fn create_writes_version_1_and_files() {
        let (_d, store) = store();
        let (a, v) = store
            .create_artifact(publish(
                &[("index.html", Some("<p>hi")), ("app.js", Some("1"))],
                None,
            ))
            .unwrap();
        assert_eq!(a.current_version, 1);
        assert_eq!(v.n, 1);
        assert_eq!(v.files["index.html"].size, 5);
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let vdir = store.home().version_dir(&id, 1);
        assert_eq!(
            std::fs::read_to_string(vdir.join("index.html")).unwrap(),
            "<p>hi"
        );
        assert_eq!(
            std::fs::read_to_string(vdir.join("files/app.js")).unwrap(),
            "1"
        );
        let (p, meta) = store.file_path(&id, 1, "app.js").unwrap().unwrap();
        assert!(p.ends_with("files/app.js"));
        assert_eq!(meta.content_type, "text/javascript");
        assert!(store.file_path(&id, 1, "nope.js").unwrap().is_none());
    }

    #[test]
    fn publish_version_carries_files_forward_and_honours_removals() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(publish(
                &[
                    ("index.html", Some("v1")),
                    ("a.js", Some("a")),
                    ("b.css", Some("b")),
                ],
                None,
            ))
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let (a2, v2) = store
            .publish_version(
                &id,
                publish(
                    &[
                        ("index.html", Some("v2")),
                        ("b.css", None),
                        ("c.txt", Some("c")),
                    ],
                    Some(1),
                ),
            )
            .unwrap();
        assert_eq!(a2.current_version, 2);
        assert_eq!(
            v2.files.keys().cloned().collect::<Vec<_>>(),
            vec!["a.js", "c.txt", "index.html"]
        );
        let vdir = store.home().version_dir(&id, 2);
        assert_eq!(
            std::fs::read_to_string(vdir.join("files/a.js")).unwrap(),
            "a"
        );
        assert!(!vdir.join("files/b.css").exists());
        assert_eq!(store.list_versions(&id).unwrap().len(), 2);
        assert_eq!(store.get_version(&id, 1).unwrap().unwrap().files.len(), 3);
    }

    #[test]
    fn stale_if_version_conflicts_and_writes_nothing() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(publish(&[("index.html", Some("v1"))], None))
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store
            .publish_version(&id, publish(&[("index.html", Some("v2"))], Some(7)))
            .unwrap_err();
        assert!(matches!(e, crate::CoreError::Conflict { current: 1 }));
        let e = store
            .publish_version(&id, publish(&[("index.html", Some("v2"))], None))
            .unwrap_err();
        assert!(matches!(
            e,
            crate::CoreError::Invalid {
                code: "if_version_required",
                ..
            }
        ));
        assert!(!store.home().version_dir(&id, 2).exists());
        assert_eq!(store.get_artifact(&id).unwrap().unwrap().current_version, 1);
    }

    #[test]
    fn zero_version_rows_are_hidden() {
        let (_d, store) = store();
        let id = crate::ArtifactId::generate();
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                     VALUES (?1, 'x', 'a', 'a', 0, 1)",
                    rusqlite::params![id.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.get_artifact(&id).unwrap().is_none());
        assert!(store.list_artifacts().unwrap().is_empty());
    }
}
