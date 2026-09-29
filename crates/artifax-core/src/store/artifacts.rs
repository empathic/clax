//! Artifact metadata, creation, versions, and file lookup.

use super::Store;
use crate::model::{Artifact, CONTRACT_VERSION, FileMeta, Version};
use crate::publish::{FileChange, INDEX, ValidatedPublish};
use crate::{ArtifactId, CoreError, Result, new_ulid};
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

/// A stored JSON column that does not parse, as found by [`Store::corrupt_rows`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorruptRow {
    pub artifact_id: String,
    /// The version, for `versions.files_json`; `None` for artifact columns.
    pub version: Option<u32>,
    pub column: &'static str,
}

/// Parses the JSON text column `column`; malformed JSON yields `Corrupt` naming
/// the row, carried inside `Ok` so callers can skip or report it.
fn json_column<T: serde::de::DeserializeOwned>(
    r: &Row<'_>,
    column: &'static str,
    artifact_id: &str,
    version: Option<u32>,
) -> rusqlite::Result<Result<T>> {
    let text: String = r.get(column)?;
    Ok(serde_json::from_str(&text).map_err(|_| CoreError::Corrupt {
        artifact_id: artifact_id.to_string(),
        column,
        version,
    }))
}

/// Reads an artifact row; the inner result is `Corrupt` when a JSON column is malformed.
fn row_to_artifact(r: &Row<'_>) -> rusqlite::Result<Result<Artifact>> {
    let id: String = r.get("id")?;
    let capabilities = match json_column(r, "capabilities_json", &id, None)? {
        Ok(c) => c,
        Err(e) => return Ok(Err(e)),
    };
    Ok(Ok(Artifact {
        id,
        title: r.get("title")?,
        description: r.get("description")?,
        icon: r.get("icon")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        current_version: r.get("current_version")?,
        pinned: r.get::<_, i64>("pinned")? != 0,
        capabilities,
        contract_version: r.get("contract_version")?,
        owner_session_id: r.get("owner_session_id")?,
    }))
}

/// Keeps the readable rows, logging and dropping the corrupt ones.
fn skip_corrupt<T>(rows: Vec<Result<T>>) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            Ok(v) => out.push(v),
            Err(CoreError::Corrupt {
                artifact_id,
                column,
                version,
            }) => {
                tracing::warn!(artifact_id, column, version, "skipping corrupt row");
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

const SELECT: &str = "SELECT id, title, description, icon, created_at, updated_at, current_version,
    owner_session_id, pinned, capabilities_json, contract_version FROM artifacts";

impl Store {
    /// The live artifact `id` (not deleted, at least one version), or `None`.
    ///
    /// # Errors
    /// `Corrupt` when its `capabilities_json` is malformed.
    pub fn get_artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("{SELECT} WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0"),
                params![id.as_str()],
                row_to_artifact,
            )
            .optional()?
            .transpose()
        })
    }

    /// Live artifacts (not deleted, at least one version): pinned first, then
    /// most recently updated, ties broken by ID. Rows with a malformed JSON
    /// column are logged and left out (see [`Store::corrupt_rows`]).
    pub fn list_artifacts(&self) -> Result<Vec<Artifact>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE deleted_at IS NULL AND current_version > 0 ORDER BY pinned DESC, updated_at DESC, id"
            ))?;
            let rows = stmt.query_map([], row_to_artifact)?;
            skip_corrupt(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

    /// Overwrites each field that is `Some` in `patch` and leaves the others
    /// untouched; a field therefore cannot be cleared through this call. Does not
    /// bump `updated_at` (metadata edits are not new content).
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist or is deleted.
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
            tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )?
        })
    }

    /// Marks the artifact deleted, then removes its directory. Once the row is
    /// marked the delete has happened: a missing directory is fine, and any other
    /// removal failure is logged and leaves orphaned files rather than an error.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist or is already deleted.
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
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                artifact = id.as_str(),
                dir = %dir.display(),
                error = %e,
                "could not remove deleted artifact's files"
            ),
        }
        Ok(())
    }

    /// Every row whose `artifacts.capabilities_json` or `versions.files_json`
    /// does not parse as JSON, including deleted artifacts, ordered by artifact
    /// ID then version. Reads the raw text, independent of the typed readers.
    pub fn corrupt_rows(&self) -> Result<Vec<CorruptRow>> {
        self.with_conn(|c| {
            let mut out = Vec::new();
            let mut stmt = c.prepare("SELECT id, capabilities_json FROM artifacts ORDER BY id")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (artifact_id, text) = row?;
                if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                    out.push(CorruptRow {
                        artifact_id,
                        version: None,
                        column: "capabilities_json",
                    });
                }
            }
            let mut stmt = c.prepare(
                "SELECT artifact_id, n, files_json FROM versions ORDER BY artifact_id, n",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (artifact_id, n, text) = row?;
                if serde_json::from_str::<BTreeMap<String, FileMeta>>(&text).is_err() {
                    out.push(CorruptRow {
                        artifact_id,
                        version: Some(n),
                        column: "files_json",
                    });
                }
            }
            out.sort_by(|a, b| (&a.artifact_id, a.version).cmp(&(&b.artifact_id, b.version)));
            Ok(out)
        })
    }

    /// Directories under any `artifacts/<id>/versions/` that no version row
    /// accounts for: `.tmp-*` staging directories, and numbered directories above
    /// the artifact's `current_version`. Sorted.
    ///
    /// # Errors
    /// Database errors, and I/O errors other than a missing directory.
    pub fn stray_version_dirs(&self) -> Result<Vec<PathBuf>> {
        let currents: Vec<(String, u32)> = self.with_conn(|c| {
            let mut stmt = c.prepare("SELECT id, current_version FROM artifacts ORDER BY id")?;
            Ok(stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut out = Vec::new();
        for (id, current) in currents {
            let Ok(id) = ArtifactId::parse(&id) else {
                continue;
            };
            let versions = self.home.artifact_dir(&id).join("versions");
            for path in super::assets::read_dir_or_empty(&versions)? {
                if !path.is_dir() {
                    continue;
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let stray =
                    name.starts_with(".tmp-") || name.parse::<u32>().is_ok_and(|n| n > current);
                if stray {
                    out.push(path);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// IDs of live-looking artifact rows that never recorded a version (a
    /// creation that died before version 1), sorted.
    ///
    /// # Errors
    /// Database errors only.
    pub fn zero_version_artifacts(&self) -> Result<Vec<String>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM artifacts WHERE current_version = 0 AND deleted_at IS NULL ORDER BY id",
            )?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Deletes the rows (and any files) of [`Store::zero_version_artifacts`].
    /// Returns how many artifacts were removed.
    ///
    /// # Errors
    /// Database errors, and I/O errors other than a missing directory.
    pub fn delete_zero_version_artifacts(&self) -> Result<usize> {
        let ids = self.zero_version_artifacts()?;
        let mut n = 0;
        for id in ids {
            let removed = self.with_tx(|tx| {
                let zero = "SELECT id FROM artifacts WHERE id = ?1 AND current_version = 0 AND deleted_at IS NULL";
                for table in ["assets", "versions"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE artifact_id IN ({zero})"),
                        params![id],
                    )?;
                }
                Ok(tx.execute(
                    "DELETE FROM artifacts WHERE id = ?1 AND current_version = 0 AND deleted_at IS NULL",
                    params![id],
                )?)
            })?;
            if removed > 0 {
                n += 1;
                if let Ok(aid) = ArtifactId::parse(&id) {
                    match std::fs::remove_dir_all(self.home.artifact_dir(&aid)) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        Ok(n)
    }

    /// Deletes the [`Store::corrupt_rows`] that belong to soft-deleted
    /// artifacts: a corrupt version row goes alone, a corrupt artifact row goes
    /// with its versions, assets, watches, threads, and the threads' comments
    /// and feedback. Rows of live artifacts are never touched.
    /// Returns how many corrupt rows were cleared.
    ///
    /// # Errors
    /// Database errors only.
    pub fn delete_corrupt_deleted_rows(&self) -> Result<usize> {
        let corrupt = self.corrupt_rows()?;
        self.with_tx(|tx| {
            let mut n = 0;
            for row in corrupt {
                // A missing artifact row means an earlier entry already removed it.
                let deleted: Option<bool> = tx
                    .query_row(
                        "SELECT deleted_at IS NOT NULL FROM artifacts WHERE id = ?1",
                        params![row.artifact_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if deleted != Some(true) {
                    continue;
                }
                match row.version {
                    Some(v) => {
                        tx.execute(
                            "DELETE FROM versions WHERE artifact_id = ?1 AND n = ?2",
                            params![row.artifact_id, v],
                        )?;
                    }
                    None => {
                        for sql in [
                            "DELETE FROM feedback WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM comments WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM threads WHERE artifact_id = ?1",
                            "DELETE FROM watches WHERE artifact_id = ?1",
                        ] {
                            tx.execute(sql, params![row.artifact_id])?;
                        }
                        for table in ["assets", "versions"] {
                            tx.execute(
                                &format!("DELETE FROM {table} WHERE artifact_id = ?1"),
                                params![row.artifact_id],
                            )?;
                        }
                        tx.execute(
                            "DELETE FROM artifacts WHERE id = ?1",
                            params![row.artifact_id],
                        )?;
                    }
                }
                n += 1;
            }
            Ok(n)
        })
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

/// Removes its directory on drop; a no-op once the directory has been renamed away.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Rejects path sets where one path is a `/`-prefix of another (a file and a
/// directory of the same name) or two paths are equal under ASCII case folding.
fn check_collisions<'a>(paths: impl Iterator<Item = &'a String>) -> Result<()> {
    let folded: Vec<String> = paths.map(|p| p.to_ascii_lowercase()).collect();
    let set: std::collections::HashSet<&str> = folded.iter().map(String::as_str).collect();
    if set.len() != folded.len() {
        return Err(CoreError::invalid(
            "invalid_path",
            "two paths differ only by letter case",
        ));
    }
    for p in &folded {
        for (i, _) in p.match_indices('/') {
            if set.contains(&p[..i]) {
                return Err(CoreError::invalid(
                    "invalid_path",
                    format!("'{}' is both a file and a directory", &p[..i]),
                ));
            }
        }
    }
    Ok(())
}

/// Paths that `p` writes (its `Put` entries).
fn put_paths(p: &ValidatedPublish) -> impl Iterator<Item = &String> {
    p.files
        .iter()
        .filter(|(_, c)| matches!(c, FileChange::Put(_)))
        .map(|(k, _)| k)
}

/// Reads a version row; the inner result is `Corrupt` when `files_json` is malformed.
fn row_to_version(r: &Row<'_>) -> rusqlite::Result<Result<Version>> {
    let artifact_id: String = r.get("artifact_id")?;
    let n: u32 = r.get("n")?;
    let files = match json_column(r, "files_json", &artifact_id, Some(n))? {
        Ok(f) => f,
        Err(e) => return Ok(Err(e)),
    };
    Ok(Ok(Version {
        artifact_id,
        n,
        label: r.get("label")?,
        created_at: r.get("created_at")?,
        session_id: r.get("session_id")?,
        files,
    }))
}

const SELECT_VERSION: &str =
    "SELECT artifact_id, n, label, created_at, session_id, files_json FROM versions";

impl Store {
    /// Creates an artifact and writes its version 1. `p.files` are all stored;
    /// nothing is carried forward. The artifact is invisible until version 1 is
    /// recorded. `session_id` becomes the artifact's `owner_session_id` and the
    /// version's `session_id`.
    pub fn create_artifact(
        &self,
        p: ValidatedPublish,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        let id = ArtifactId::generate();
        let now = Store::now();
        let title = p
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Untitled".to_string());
        let caps = p.capabilities.clone().unwrap_or(serde_json::json!({}));
        // Reject before inserting, so a failed publish leaves no zero-version row.
        check_collisions(put_paths(&p))?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO artifacts (id, title, description, icon, created_at, updated_at, current_version,
                    pinned, capabilities_json, contract_version, owner_session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, ?6, ?7, ?8)",
                params![id.as_str(), title, p.description, p.icon, now, caps.to_string(), CONTRACT_VERSION, session_id],
            )?;
            Ok(())
        })?;
        self.write_version(&id, 0, &p, &BTreeMap::new(), session_id)
    }

    /// Publishes the next version of an existing artifact. `p.if_version` must
    /// equal the current version (`Conflict` otherwise, `if_version_required`
    /// when absent). Files of the previous version not named in `p.files` are
    /// carried forward; `Remove` entries drop a path; `index.html` is always
    /// taken from `p`. `Corrupt` when the artifact or its current version has a
    /// malformed JSON column. `session_id` is recorded on the new version.
    pub fn publish_version(
        &self,
        id: &ArtifactId,
        p: ValidatedPublish,
        session_id: Option<&str>,
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
        self.write_version(id, current.current_version, &p, &prev, session_id)
    }

    /// Stages the files of version `expected + 1` in a private directory,
    /// carrying forward `prev` entries not named in `p.files`, then, in one
    /// transaction, re-checks that the artifact is still at `expected`
    /// (`Conflict` otherwise), records the version, bumps the artifact and
    /// renames the staging directory into place before committing.
    fn write_version(
        &self,
        id: &ArtifactId,
        expected: u32,
        p: &ValidatedPublish,
        prev: &BTreeMap<String, FileMeta>,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        let n = expected + 1;
        let carried: Vec<&String> = prev
            .keys()
            .filter(|path| *path != INDEX && !p.files.contains_key(*path))
            .collect();
        check_collisions(carried.iter().copied().chain(put_paths(p)))?;

        let versions_dir = self.home.artifact_dir(id).join("versions");
        std::fs::create_dir_all(&versions_dir)?;
        let staging = Staging(versions_dir.join(format!(".tmp-{}", new_ulid())));
        let files_dir = staging.0.join("files");
        std::fs::create_dir_all(&files_dir)?;
        let write = |path: &str, bytes: &[u8]| -> Result<()> {
            let dest = if path == INDEX {
                staging.0.join(INDEX)
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
        for path in carried {
            let src = self.home.version_dir(id, expected).join("files").join(path);
            let dest = files_dir.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dest)?;
            files.insert(path.clone(), prev[path].clone());
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
        let vdir = self.home.version_dir(id, n);
        let renamed = std::cell::Cell::new(false);
        let result = self.with_tx(|tx| {
            let current: u32 = tx.query_row(
                "SELECT current_version FROM artifacts WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )?;
            if current != expected {
                return Err(CoreError::Conflict { current });
            }
            tx.execute(
                "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id.as_str(), n, p.label, now, session_id, files_json],
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
            )??;
            let v = tx.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                params![id.as_str(), n],
                row_to_version,
            )??;
            std::fs::rename(&staging.0, &vdir)?;
            renamed.set(true);
            Ok((a, v))
        });
        if result.is_err() && renamed.get() {
            // Commit failed after the rename; no other writer can hold `n`
            // because the transaction verified `expected` under the lock.
            let _ = std::fs::remove_dir_all(&vdir);
        }
        result
    }

    /// Version `n` of artifact `id`, or `None`.
    ///
    /// # Errors
    /// `Corrupt` when its `files_json` is malformed.
    pub fn get_version(&self, id: &ArtifactId, n: u32) -> Result<Option<Version>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                params![id.as_str(), n],
                row_to_version,
            )
            .optional()?
            .transpose()
        })
    }

    /// Every version of the artifact, oldest first (ascending `n`). Versions with
    /// a malformed `files_json` are logged and left out.
    pub fn list_versions(&self, id: &ArtifactId) -> Result<Vec<Version>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT_VERSION} WHERE artifact_id = ?1 ORDER BY n"
            ))?;
            skip_corrupt(
                stmt.query_map(params![id.as_str()], row_to_version)?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )
        })
    }

    /// The on-disk path and metadata of `path` in version `n`, only for files
    /// recorded in that version. `Corrupt` when that version's `files_json` is
    /// malformed.
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
    fn list_breaks_updated_at_ties_by_id() {
        let (_d, store) = store();
        let mut ids: Vec<String> = (0..6)
            .map(|i| {
                store
                    .insert_artifact_for_test(&format!("T{i}"), "2026-01-01T00:00:00.000Z")
                    .as_str()
                    .to_string()
            })
            .collect();
        ids.sort();
        let listed: Vec<String> = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert_eq!(listed, ids);
    }

    #[test]
    fn corrupt_rows_are_skipped_in_lists_and_named_on_lookup() {
        let (_d, store) = store();
        let mk = || {
            let (a, _) = store
                .create_artifact(publish(&[("index.html", Some("v1"))], None), None)
                .unwrap();
            crate::ArtifactId::parse(&a.id).unwrap()
        };
        let good = mk();
        let bad = mk();
        let old_bad = mk();
        store
            .publish_version(
                &old_bad,
                publish(&[("index.html", Some("v2"))], Some(1)),
                None,
            )
            .unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
                    rusqlite::params![bad.as_str()],
                )?;
                c.execute(
                    "UPDATE versions SET files_json = '{' WHERE artifact_id = ?1 AND n = 1",
                    rusqlite::params![old_bad.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let mut listed: Vec<String> = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        listed.sort();
        let mut expected = vec![good.as_str().to_string(), old_bad.as_str().to_string()];
        expected.sort();
        assert_eq!(listed, expected);
        match store.get_artifact(&bad).unwrap_err() {
            crate::CoreError::Corrupt {
                artifact_id,
                column,
                version,
            } => {
                assert_eq!(artifact_id, bad.as_str());
                assert_eq!(column, "capabilities_json");
                assert_eq!(version, None);
            }
            e => panic!("{e:?}"),
        }

        let versions: Vec<u32> = store
            .list_versions(&old_bad)
            .unwrap()
            .iter()
            .map(|v| v.n)
            .collect();
        assert_eq!(versions, [2]);
        assert!(matches!(
            store.get_version(&old_bad, 1).unwrap_err(),
            crate::CoreError::Corrupt {
                version: Some(1),
                column: "files_json",
                ..
            }
        ));
        assert!(matches!(
            store.file_path(&old_bad, 1, "index.html").unwrap_err(),
            crate::CoreError::Corrupt { .. }
        ));
        assert!(matches!(
            store
                .publish_version(&bad, publish(&[("index.html", Some("v2"))], Some(1)), None)
                .unwrap_err(),
            crate::CoreError::Corrupt { .. }
        ));

        let rows = store.corrupt_rows().unwrap();
        let mut want = vec![
            super::CorruptRow {
                artifact_id: bad.as_str().to_string(),
                version: None,
                column: "capabilities_json",
            },
            super::CorruptRow {
                artifact_id: old_bad.as_str().to_string(),
                version: Some(1),
                column: "files_json",
            },
        ];
        want.sort_by(|a, b| a.artifact_id.cmp(&b.artifact_id));
        assert_eq!(rows, want);
    }

    #[test]
    fn delete_succeeds_when_the_directory_is_missing_or_cannot_be_removed() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, store) = store();
        let gone = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        store.delete_artifact(&gone).unwrap();
        assert!(store.get_artifact(&gone).unwrap().is_none());

        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping unremovable-directory case: root ignores directory modes");
            return;
        }

        let stuck = store.insert_artifact_for_test("B", "2026-01-01T00:00:00.000Z");
        let locked = store.home().artifact_dir(&stuck).join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("f"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = store.delete_artifact(&stuck);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        result.unwrap();
        assert!(store.get_artifact(&stuck).unwrap().is_none());
        assert!(locked.join("f").exists(), "removal really failed");
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

        // After first open, user_version is the number of migrations
        let version: u32 = store
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        let expected = crate::store::migrations::MIGRATIONS.len() as u32;
        assert_eq!(version, expected);

        // Opening a second time should not re-run migrations or error
        let store2 = Store::open(&home).unwrap();
        let version2: u32 = store2
            .with_conn(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version2, expected);
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
            .create_artifact(
                publish(
                    &[("index.html", Some("<p>hi")), ("app.js", Some("1"))],
                    None,
                ),
                None,
            )
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
            .create_artifact(
                publish(
                    &[
                        ("index.html", Some("v1")),
                        ("a.js", Some("a")),
                        ("b.css", Some("b")),
                    ],
                    None,
                ),
                None,
            )
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
                None,
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
        assert_eq!(
            store
                .list_versions(&id)
                .unwrap()
                .iter()
                .map(|v| v.n)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(store.get_version(&id, 1).unwrap().unwrap().files.len(), 3);
    }

    #[test]
    fn stale_if_version_conflicts_and_writes_nothing() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store
            .publish_version(&id, publish(&[("index.html", Some("v2"))], Some(7)), None)
            .unwrap_err();
        assert!(matches!(e, crate::CoreError::Conflict { current: 1 }));
        let e = store
            .publish_version(&id, publish(&[("index.html", Some("v2"))], None), None)
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

    #[test]
    fn colliding_paths_are_rejected() {
        let (_d, store) = store();
        for (a, b) in [("a", "a/b.js"), ("App.js", "app.js")] {
            let e = store
                .create_artifact(
                    publish(
                        &[("index.html", Some("x")), (a, Some("1")), (b, Some("2"))],
                        None,
                    ),
                    None,
                )
                .unwrap_err();
            assert!(matches!(
                e,
                crate::CoreError::Invalid {
                    code: "invalid_path",
                    ..
                }
            ));
        }
        assert!(store.list_artifacts().unwrap().is_empty());
        let rows: i64 = store
            .with_conn(|c| Ok(c.query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(rows, 0, "a rejected create leaves no artifact row");

        let (a, _) = store
            .create_artifact(
                publish(&[("index.html", Some("v1")), ("a", Some("1"))], None),
                None,
            )
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store
            .publish_version(
                &id,
                publish(
                    &[("index.html", Some("v2")), ("a/b.js", Some("2"))],
                    Some(1),
                ),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            e,
            crate::CoreError::Invalid {
                code: "invalid_path",
                ..
            }
        ));
        let e = store
            .publish_version(
                &id,
                publish(&[("index.html", Some("v2")), ("A", Some("2"))], Some(1)),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            e,
            crate::CoreError::Invalid {
                code: "invalid_path",
                ..
            }
        ));
        assert!(!store.home().version_dir(&id, 2).exists());
    }

    #[test]
    fn concurrent_publishes_with_same_if_version_yield_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = std::sync::Arc::new(Store::open(&home).unwrap());
        let (a, _) = store
            .create_artifact(publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let handles: Vec<_> = ["left", "right"]
            .into_iter()
            .map(|body| {
                let store = store.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    let r = store.publish_version(
                        &id,
                        publish(&[("index.html", Some(body))], Some(1)),
                        None,
                    );
                    (body, r)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();
        assert_eq!(wins.len(), 1);
        for (_, r) in &results {
            if let Err(e) = r {
                assert!(matches!(e, crate::CoreError::Conflict { .. }));
            }
        }
        let winner = wins[0].0;
        let vdir = store.home().version_dir(&id, 2);
        assert_eq!(
            std::fs::read_to_string(vdir.join("index.html")).unwrap(),
            winner
        );
        assert_eq!(store.get_artifact(&id).unwrap().unwrap().current_version, 2);
        let leftovers: Vec<_> = std::fs::read_dir(vdir.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    fn one_version(store: &Store) -> crate::ArtifactId {
        let p = crate::publish::validate(
            serde_json::from_value(serde_json::json!({
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap();
        crate::ArtifactId::parse(&store.create_artifact(p, None).unwrap().0.id).unwrap()
    }

    #[test]
    fn corrupt_deleted_artifact_with_threads_and_watches_is_cleared() {
        let (_d, store) = store();
        let aid = one_version(&store);
        let session = crate::store::test_util::session(&store, "claude", "h1");
        store.watch(&session, &aid, true).unwrap();
        let t = store
            .create_thread(
                &aid,
                crate::NewThread {
                    version_n: 1,
                    anchor: crate::store::test_util::anchor(),
                    author_name: "Alex".into(),
                    body: "x".into(),
                    clip: None,
                },
            )
            .unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![crate::new_ulid(), t.id, t.comments[0].id, session, Store::now()],
                )?;
                Ok(())
            })
            .unwrap();
        store.delete_artifact(&aid).unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
                    [aid.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.delete_corrupt_deleted_rows().unwrap(), 1);
        for table in ["feedback", "comments", "threads", "watches", "artifacts"] {
            let n: i64 = store
                .with_conn(|c| {
                    Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn stray_version_dirs_finds_staging_and_future_versions_only() {
        let (_d, store) = store();
        let id = one_version(&store);
        assert!(store.stray_version_dirs().unwrap().is_empty());
        let versions = store.home().artifact_dir(&id).join("versions");
        std::fs::create_dir_all(versions.join(".tmp-x")).unwrap();
        std::fs::create_dir_all(versions.join("99")).unwrap();
        std::fs::create_dir_all(versions.join("notes")).unwrap();
        assert_eq!(
            store.stray_version_dirs().unwrap(),
            vec![versions.join(".tmp-x"), versions.join("99")]
        );
    }

    #[test]
    fn zero_version_artifacts_are_listed_and_deleted_but_deleted_ones_are_not() {
        let (_d, store) = store();
        let live = one_version(&store);
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                     VALUES ('zzzzzzzzzzzz', 'z', 'x', 'x', 0, '1')",
                    [],
                )?;
                c.execute(
                    "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version, deleted_at)
                     VALUES ('yyyyyyyyyyyy', 'y', 'x', 'x', 0, '1', 'x')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store.zero_version_artifacts().unwrap(),
            vec!["zzzzzzzzzzzz"]
        );
        assert_eq!(store.delete_zero_version_artifacts().unwrap(), 1);
        assert!(store.zero_version_artifacts().unwrap().is_empty());
        assert!(store.get_artifact(&live).unwrap().is_some());
        assert_eq!(store.delete_zero_version_artifacts().unwrap(), 0);
    }

    #[test]
    fn only_corrupt_rows_of_deleted_artifacts_are_deleted() {
        let (_d, store) = store();
        let live = one_version(&store);
        let gone_caps = one_version(&store);
        let gone_files = one_version(&store);
        store.delete_artifact(&gone_caps).unwrap();
        store.delete_artifact(&gone_files).unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE artifacts SET capabilities_json = 'nope' WHERE id IN (?1, ?2)",
                    [live.as_str(), gone_caps.as_str()],
                )?;
                c.execute(
                    "UPDATE versions SET files_json = 'nope' WHERE artifact_id = ?1",
                    [gone_files.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.corrupt_rows().unwrap().len(), 3);
        assert_eq!(store.delete_corrupt_deleted_rows().unwrap(), 2);
        let left = store.corrupt_rows().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].artifact_id, live.as_str());
        assert_eq!(store.delete_corrupt_deleted_rows().unwrap(), 0);
    }

    #[test]
    fn corrupt_artifact_and_version_rows_of_one_deleted_artifact_clear_together() {
        let (_d, store) = store();
        let gone = one_version(&store);
        store.delete_artifact(&gone).unwrap();
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
                    [gone.as_str()],
                )?;
                c.execute(
                    "UPDATE versions SET files_json = 'nope' WHERE artifact_id = ?1",
                    [gone.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.corrupt_rows().unwrap().len(), 2);
        store.delete_corrupt_deleted_rows().unwrap();
        assert!(store.corrupt_rows().unwrap().is_empty());
    }
}
