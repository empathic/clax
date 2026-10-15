//! Artifact metadata, creation, versions, and file lookup.

use super::Store;
use super::changelog::{fill_addresses, link_version};
use super::live::{LivePage, live_page_of_conn};
use crate::audit::{AuditCtx, AuditKind, AuditRecord, content_manifest_sha256, sha256_hex};
use crate::content::{NewFile, VersionMeta, VersionWrite};
use crate::live::KIND_LIVE;
use crate::model::{
    Artifact, ArtifactAuthors, CONTRACT_VERSION, FileMeta, ForkedFrom, Version, VersionAuthors,
};
use crate::publish::{FileChange, INDEX, ValidatedPublish};
use crate::store::approvals::OWNER;
use crate::visibility::{Author, file_authors};
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
    /// A full-set capabilities declaration that replaces the stored one.
    pub capabilities: Option<serde_json::Value>,
    /// Who sets the text fields (spec §8.2); `None`: the owner.
    pub by: Option<Author>,
    /// A page sets them: its page's authors are added.
    pub via_page: bool,
}

/// The `artifact.create` record of `a` (spec §6.1), at `at`.
pub(super) fn create_record(a: &Artifact, at: &str) -> AuditRecord {
    let mut r = AuditRecord::new(AuditKind::ArtifactCreate, at)
        .with("title", a.title.as_str())
        .with("kind", a.kind.as_str())
        .with("icon", a.icon.clone())
        .with("capabilities", a.capabilities.clone())
        .with("contract_version", a.contract_version.as_str());
    r.ids.artifact = Some(a.id.clone());
    r
}

/// The record of version `v` of `a` (spec §6.1, §6.3): `version.publish`,
/// or `live.snapshot` with the page's origin and path when `page` is the
/// live page it was written to. Each file names its `sha256:` hash, size and
/// type; `carried` are the paths taken unchanged from the previous version;
/// `by_page` whether the page wrote it (`None`: not known, as in the
/// backfill).
pub(super) fn version_record(
    a: &Artifact,
    v: &Version,
    carried: &[String],
    by_page: Option<bool>,
    page: Option<&LivePage>,
) -> AuditRecord {
    let files: serde_json::Map<String, serde_json::Value> = v
        .files
        .iter()
        .map(|(path, m)| {
            (
                path.clone(),
                serde_json::json!({
                    "sha256": m.sha256.as_ref().map(|h| format!("sha256:{h}")),
                    "size": m.size,
                    "content_type": m.content_type,
                }),
            )
        })
        .collect();
    let kind = if page.is_some() {
        AuditKind::LiveSnapshot
    } else {
        AuditKind::VersionPublish
    };
    let mut r = AuditRecord::new(kind, v.created_at.as_str())
        .with("n", v.n)
        .with("label", v.label.clone())
        .with("note", v.note.clone())
        .with("title", a.title.as_str())
        .with("files", files)
        .with("content_sha256", v.content_sha256.clone())
        .with("carried", carried.to_vec())
        .with("addresses", v.addresses.clone())
        .with("by_page", by_page);
    if let Some(p) = page {
        r = r
            .with("origin", p.origin.as_str())
            .with("path", p.path.as_str());
        r.ids.origin = Some(p.origin.clone());
    }
    r.ids.artifact = Some(a.id.clone());
    r
}

/// The `artifact.delete` record of `aid` (spec §6.1): its title and
/// current version when deleted, at `at`.
pub(super) fn delete_record(at: &str, aid: &str, title: &str, current: u32) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ArtifactDelete, at)
        .with("title", title)
        .with("current_version", current);
    rec.ids.artifact = Some(aid.to_string());
    rec
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
pub(super) fn row_to_artifact(r: &Row<'_>) -> rusqlite::Result<Result<Artifact>> {
    let id: String = r.get("id")?;
    let capabilities = match json_column(r, "capabilities_json", &id, None)? {
        Ok(c) => c,
        Err(e) => return Ok(Err(e)),
    };
    let text = match r.get::<_, Option<String>>("text_authors_json")? {
        None => None,
        Some(_) => match json_column(r, "text_authors_json", &id, None)? {
            Ok(t) => Some(t),
            Err(e) => return Ok(Err(e)),
        },
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
        kind: r.get("kind")?,
        authors: ArtifactAuthors {
            creator: r.get("person")?,
            text,
        },
        forked_from: match (
            r.get::<_, Option<String>>("forked_from_id")?,
            r.get::<_, Option<u32>>("forked_from_version")?,
        ) {
            (Some(artifact_id), Some(version)) => Some(ForkedFrom {
                artifact_id,
                version,
            }),
            _ => None,
        },
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

pub(super) const SELECT: &str =
    "SELECT id, title, description, icon, created_at, updated_at, current_version,
    owner_session_id, pinned, capabilities_json, contract_version, kind, person,
    text_authors_json, forked_from_id, forked_from_version FROM artifacts";

/// The live artifact `id` (not deleted, at least one version) on `c`, or `None`.
pub(crate) fn live_artifact_in(c: &rusqlite::Connection, id: &str) -> Result<Option<Artifact>> {
    c.prepare_cached(&format!(
        "{SELECT} WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0"
    ))?
    .query_row(params![id], row_to_artifact)
    .optional()?
    .transpose()
}

impl Store {
    /// The live artifact `id` (not deleted, at least one version), or `None`.
    ///
    /// # Errors
    /// `Corrupt` when its `capabilities_json` is malformed.
    pub fn get_artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>> {
        self.with_read(|c| live_artifact_in(c, id.as_str()))
    }

    /// Live artifacts (not deleted, at least one version): pinned first, then
    /// most recently updated, ties broken by ID. Rows with a malformed JSON
    /// column are logged and left out (see [`Store::corrupt_rows`]).
    pub fn list_artifacts(&self) -> Result<Vec<Artifact>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE deleted_at IS NULL AND current_version > 0 ORDER BY pinned DESC, updated_at DESC, id"
            ))?;
            let rows = stmt.query_map([], row_to_artifact)?;
            skip_corrupt(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Pins or unpins the artifact, recorded as `artifact.update` under `ctx`.
    pub fn set_pinned(&self, ctx: &AuditCtx, id: &ArtifactId, pinned: bool) -> Result<Artifact> {
        self.update_meta(
            ctx,
            id,
            MetaPatch {
                pinned: Some(pinned),
                ..Default::default()
            },
        )
    }

    /// Overwrites each field that is `Some` in `patch` (a `capabilities` value
    /// replaces the whole declaration; `{}` clears it) and leaves the others
    /// untouched; any other field therefore cannot be cleared through this
    /// call. The caller validates `capabilities`. Does not
    /// bump `updated_at` (metadata edits are not new content). Recorded,
    /// in the same transaction, as `artifact.update` under `ctx`, naming in
    /// `fields` each field the patch sets, even to the value it had (an
    /// explicit request); a patch that sets nothing records nothing.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist or is deleted.
    pub fn update_meta(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        patch: MetaPatch,
    ) -> Result<Artifact> {
        self.with_tx(|tx| {
            let Some(before) = tx
                .query_row(
                    &format!("{SELECT} WHERE id = ?1 AND deleted_at IS NULL"),
                    params![id.as_str()],
                    row_to_artifact,
                )
                .optional()?
                .transpose()?
            else {
                return Err(CoreError::NotFound);
            };
            // The fields it sets are the patcher's now, and a page's
            // patch also its page's authors' (spec §8.2).
            let set: Vec<&str> = TEXT_FIELDS
                .into_iter()
                .zip([
                    patch.title.is_some(),
                    patch.description.is_some(),
                    patch.icon.is_some(),
                    patch.capabilities.is_some(),
                ])
                .filter_map(|(f, g)| g.then_some(f))
                .collect();
            let mut by = tokens([patch.by.clone().unwrap_or(Author::Owner)]);
            if patch.via_page {
                for t in page_authors_in(tx, id.as_str())? {
                    if !by.contains(&t) {
                        by.push(t);
                    }
                }
            }
            let text = if set.is_empty() {
                before
                    .authors
                    .text
                    .as_ref()
                    .map(|t| serde_json::to_string(t).expect("serialisable map"))
            } else {
                text_authors(
                    before.authors.text.clone(),
                    before.authors.creator.as_deref(),
                    &set,
                    &by,
                )
            };
            tx.execute(
                "UPDATE artifacts SET
                    title = COALESCE(?2, title),
                    description = COALESCE(?3, description),
                    icon = COALESCE(?4, icon),
                    pinned = COALESCE(?5, pinned),
                    capabilities_json = COALESCE(?6, capabilities_json),
                    text_authors_json = ?7
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![
                    id.as_str(),
                    patch.title,
                    patch.description,
                    patch.icon,
                    patch.pinned.map(|b| b as i64),
                    patch.capabilities.as_ref().map(|c| c.to_string()),
                    text
                ],
            )?;
            let mut fields = serde_json::Map::new();
            let mut set = |k: &str, v: Option<serde_json::Value>| {
                if let Some(v) = v {
                    fields.insert(k.to_string(), v);
                }
            };
            set("title", patch.title.clone().map(Into::into));
            set("description", patch.description.clone().map(Into::into));
            set("icon", patch.icon.clone().map(Into::into));
            set("pinned", patch.pinned.map(Into::into));
            set("capabilities", patch.capabilities.clone());
            if !fields.is_empty() {
                let mut rec =
                    AuditRecord::new(AuditKind::ArtifactUpdate, self.now()).with("fields", fields);
                rec.ids.artifact = Some(id.as_str().to_string());
                self.record_audit(tx, ctx, rec)?;
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
    /// The mark is recorded, in its transaction, as `artifact.delete` under
    /// `ctx`, with the title and current version it had.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist or is already deleted.
    pub fn delete_artifact(&self, ctx: &AuditCtx, id: &ArtifactId) -> Result<()> {
        self.block_on(self.delete_artifact_async(ctx, id))
    }

    /// [`Store::delete_artifact`], awaiting its content store: the mark's
    /// transaction commits, then the artifact's content is removed (a
    /// failure there is logged, and leaves only unreferenced content).
    pub async fn delete_artifact_async(&self, ctx: &AuditCtx, id: &ArtifactId) -> Result<()> {
        let _lock = self.lock_content(id).await?;
        self.with_tx(|tx| {
            let now = self.now();
            let Some((title, current)) = tx
                .query_row(
                    "UPDATE artifacts SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL
                     RETURNING title, current_version",
                    params![id.as_str(), now],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)),
                )
                .optional()?
            else {
                return Err(CoreError::NotFound);
            };
            self.record_audit(tx, ctx, delete_record(&now, id.as_str(), &title, current))?;
            super::approvals::withdraw_artifact_in(self, tx, ctx, id.as_str(), &now)?;
            tx.execute(
                "DELETE FROM docs WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM doc_authors WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM leases WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM viewer_seen WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM version_threads WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM viewer_threads WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM mentions WHERE comment_id IN (SELECT c.id FROM comments c
                    JOIN threads t ON t.id = c.thread_id WHERE t.artifact_id = ?1)",
                params![id.as_str()],
            )?;
            // A batch's threads may have moved to another live page since:
            // they keep their send record, and so does the batch.
            tx.execute(
                "DELETE FROM batch_threads WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM send_batches WHERE artifact_id = ?1
                    AND NOT EXISTS(SELECT 1 FROM batch_threads WHERE batch_id = send_batches.id)",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM live_pages WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM live_merged_pages WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            // Pages a join merged into this one are released: listed and
            // deletable again, never left hidden (spec §7.2).
            tx.execute(
                "UPDATE live_merged_pages SET merged_into = NULL WHERE merged_into = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM live_pending WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM live_picks WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            Ok(())
        })?;
        if let Err(e) = self.content.delete_artifact(id).await {
            tracing::warn!(
                artifact = id.as_str(),
                error = %e,
                "could not remove deleted artifact's files"
            );
        }
        Ok(())
    }

    /// Every row whose `artifacts.capabilities_json` or `versions.files_json`
    /// does not parse as JSON, including deleted artifacts, ordered by artifact
    /// ID then version. Reads the raw text, independent of the typed readers.
    pub fn corrupt_rows(&self) -> Result<Vec<CorruptRow>> {
        self.with_read(|c| {
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
        let currents: Vec<(String, u32)> = self.with_read(|c| {
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
        self.with_read(|c| {
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
        self.block_on(self.delete_zero_version_artifacts_async())
    }

    /// [`Store::delete_zero_version_artifacts`], awaiting its content
    /// store. Each artifact's rows go under its content lock, so a version
    /// 1 being written meanwhile is either recorded first (and the
    /// artifact kept) or never recorded.
    pub async fn delete_zero_version_artifacts_async(&self) -> Result<usize> {
        let ids = self.zero_version_artifacts()?;
        let mut n = 0;
        for id in ids {
            let aid = ArtifactId::parse(&id).ok();
            let _lock = match &aid {
                Some(aid) => Some(self.lock_content(aid).await?),
                None => None,
            };
            let removed = self.with_tx(|tx| {
                let zero = "SELECT id FROM artifacts WHERE id = ?1 AND current_version = 0 AND deleted_at IS NULL";
                for table in ["assets", "versions", "docs", "leases"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE artifact_id IN ({zero})"),
                        params![id],
                    )?;
                }
                tx.execute(
                    &format!("DELETE FROM live_pages WHERE artifact_id IN ({zero})"),
                    params![id],
                )?;
                Ok(tx.execute(
                    "DELETE FROM artifacts WHERE id = ?1 AND current_version = 0 AND deleted_at IS NULL",
                    params![id],
                )?)
            })?;
            if removed > 0 {
                n += 1;
                if let Some(aid) = &aid {
                    self.content.delete_artifact(aid).await?;
                }
            }
        }
        Ok(n)
    }

    /// Deletes the [`Store::corrupt_rows`] that belong to soft-deleted
    /// artifacts: a corrupt version row goes alone, a corrupt artifact row goes
    /// with its versions, assets, watches, documents, leases, threads, and the
    /// threads' comments and feedback. Rows of live artifacts are never touched.
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
                            "DELETE FROM version_threads WHERE artifact_id = ?1",
                            "DELETE FROM version_threads WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM live_pending WHERE artifact_id = ?1
                                OR thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM live_picks WHERE artifact_id = ?1",
                            "DELETE FROM viewer_seen WHERE artifact_id = ?1",
                            "DELETE FROM viewer_threads WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM mentions WHERE comment_id IN (SELECT c.id FROM comments c JOIN threads t ON t.id = c.thread_id WHERE t.artifact_id = ?1)",
                            "DELETE FROM feedback WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            // A thread moved off the artifact keeps its
                            // send record, and so does its batch.
                            "DELETE FROM batch_threads WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM send_batches WHERE artifact_id = ?1
                                AND NOT EXISTS(SELECT 1 FROM batch_threads WHERE batch_id = send_batches.id)",
                            "DELETE FROM page_authors WHERE kind = 'comment' AND id IN
                                (SELECT c.id FROM comments c JOIN threads t ON t.id = c.thread_id WHERE t.artifact_id = ?1)",
                            "DELETE FROM page_authors WHERE kind = 'asset' AND id IN
                                (SELECT id FROM assets WHERE artifact_id = ?1)",
                            "DELETE FROM comments WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM thread_moves WHERE thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
                            "DELETE FROM threads WHERE artifact_id = ?1",
                            "DELETE FROM watches WHERE artifact_id = ?1",
                            "DELETE FROM docs WHERE artifact_id = ?1",
                            "DELETE FROM doc_authors WHERE artifact_id = ?1",
                            "DELETE FROM leases WHERE artifact_id = ?1",
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
        self.with_tx(|c| {
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

/// Whether paths `a` and `b` cannot both be in a version: equal under ASCII
/// case folding, or one a directory of the other ([`check_collisions`]).
pub(super) fn collide(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || b.starts_with(&format!("{a}/")) || a.starts_with(&format!("{b}/"))
}

/// How a new version's content comes to be.
#[derive(Clone, Copy, Debug)]
pub(super) enum VersionStep<'a> {
    /// Written from the publish, through [`crate::content::ContentStore::write_version`].
    Write,
    /// Pushed commit `.0`, made the version as it is
    /// ([`crate::content::ContentStore::tag_pushed`]).
    Adopt(&'a str),
}

/// The paths of `prev` a new version made from `p` carries: every one `p`
/// does not name, but `index.html`, which a publish always gives, unless
/// `adopt` (a pushed commit, which carries it like any other file); and
/// none withheld from `p`'s publisher that collides with a path it puts.
fn carried_of<'a>(
    prev: &'a BTreeMap<String, FileMeta>,
    p: &ValidatedPublish,
    adopt: bool,
) -> Vec<&'a String> {
    prev.keys()
        .filter(|path| (adopt || *path != INDEX) && !p.files.contains_key(*path))
        .filter(|path| !(p.hidden.contains(*path) && put_paths(p).any(|q| collide(path, q))))
        .collect()
}

/// Paths that `p` writes (its `Put` entries).
fn put_paths(p: &ValidatedPublish) -> impl Iterator<Item = &String> {
    p.files
        .iter()
        .filter(|(_, c)| matches!(c, FileChange::Put(_)))
        .map(|(k, _)| k)
}

/// Reads a version row; the inner result is `Corrupt` when `files_json` is malformed.
pub(super) fn row_to_version(r: &Row<'_>) -> rusqlite::Result<Result<Version>> {
    let artifact_id: String = r.get("artifact_id")?;
    let n: u32 = r.get("n")?;
    let files = match json_column(r, "files_json", &artifact_id, Some(n))? {
        Ok(f) => f,
        Err(e) => return Ok(Err(e)),
    };
    let file_authors = match r.get::<_, Option<String>>("file_authors_json")? {
        None => None,
        Some(_) => match json_column(r, "file_authors_json", &artifact_id, Some(n))? {
            Ok(f) => Some(f),
            Err(e) => return Ok(Err(e)),
        },
    };
    Ok(Ok(Version {
        artifact_id,
        n,
        label: r.get("label")?,
        created_at: r.get("created_at")?,
        session_id: r.get("session_id")?,
        files,
        note: r.get("note")?,
        addresses: Vec::new(),
        agent: r.get("agent")?,
        agent_harness: r.get("agent_harness")?,
        content_sha256: r.get("content_sha256")?,
        authors: VersionAuthors {
            person: r.get("person")?,
            by_page: r.get::<_, i64>("by_page")? != 0,
            files: file_authors,
        },
    }))
}

pub(super) const SELECT_VERSION: &str =
    "SELECT artifact_id, n, label, created_at, session_id, files_json, note, content_sha256,
        person, by_page, file_authors_json,
        (SELECT agent_handle FROM sessions s WHERE s.id = versions.session_id) AS agent,
        (SELECT harness FROM sessions s WHERE s.id = versions.session_id) AS agent_harness
     FROM versions";

/// The text fields of an artifact that carry authors of their own (spec
/// §8.2): `artifacts.text_authors_json`'s keys.
pub const TEXT_FIELDS: [&str; 4] = ["title", "description", "icon", "capabilities"];

/// Author tokens, each once in first-seen order.
pub(super) fn tokens(authors: impl IntoIterator<Item = Author>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in authors {
        if let Some(t) = a.token()
            && !out.iter().any(|o| o == t)
        {
            out.push(t.to_string());
        }
    }
    out
}

/// Whether every list of `map` names the owner alone (stored as NULL).
pub(super) fn owners_alone(map: &BTreeMap<String, Vec<Option<String>>>) -> bool {
    map.values()
        .all(|a| a.iter().all(|t| t.is_none() || t.as_deref() == Some(OWNER)))
}

/// The authors of every file of `v` (spec §8.2, M11): what a page running
/// `v` holds, so what its writes carry.
fn every_file_author(v: &Version) -> Vec<String> {
    tokens(
        v.files
            .keys()
            .flat_map(|path| file_authors(&v.authors, path)),
    )
}

/// The page authors of artifact `aid` now (spec §8.2, M11): every author
/// of every file of its current version, as tokens (`*owner` for the
/// owner), the owner's alone when it has no version. A page's write
/// (a comment, a document, an upload, its own publish) carries all of them.
pub(crate) fn page_authors_in(c: &rusqlite::Connection, aid: &str) -> Result<Vec<String>> {
    let v = c
        .prepare_cached(&format!(
            "{SELECT_VERSION} WHERE artifact_id = ?1
               AND n = (SELECT current_version FROM artifacts WHERE id = ?1)"
        ))?
        .query_row(params![aid], row_to_version)
        .optional()?
        .transpose()?;
    Ok(match v {
        Some(v) => every_file_author(&v),
        None => vec![OWNER.to_string()],
    })
}

/// The authorship of version `expected + 1` of `aid`, published by `p`
/// from `session_id` with `files`, and the authors of what it writes: its
/// publisher (`p.person`, else the session's person) and, for a page's
/// publish, every author of the previous version's files (the page that
/// published it). A file `p` writes has those authors; a file carried
/// forward keeps the previous version's authors of it. `files` is `None`
/// when every file is the owner's alone.
fn version_authors(
    tx: &rusqlite::Transaction<'_>,
    aid: &str,
    expected: u32,
    p: &ValidatedPublish,
    session_id: Option<&str>,
    files: &BTreeMap<String, FileMeta>,
) -> Result<(VersionAuthors, Vec<String>)> {
    let person: Option<String> = match &p.person {
        Some(person) => Some(person.clone()),
        None => tx
            .query_row(
                "SELECT person FROM sessions WHERE id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten(),
    };
    let prev = if expected == 0 {
        None
    } else {
        tx.query_row(
            &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
            params![aid, expected],
            row_to_version,
        )
        .optional()?
        .transpose()?
    };
    let mut written = tokens([Author::of_token(person.as_deref())]);
    if p.by_page {
        for t in prev.as_ref().map(every_file_author).unwrap_or_default() {
            if !written.contains(&t) {
                written.push(t);
            }
        }
    }
    let mut map: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
    for path in files.keys() {
        let carried = !matches!(p.files.get(path), Some(FileChange::Put(_)));
        let authors = match (&prev, carried) {
            (Some(v), true) => tokens(file_authors(&v.authors, path)),
            _ => written.clone(),
        };
        map.insert(path.clone(), authors.into_iter().map(Some).collect());
    }
    let alone = owners_alone(&map);
    Ok((
        VersionAuthors {
            person,
            by_page: p.by_page,
            files: (!alone).then_some(map),
        },
        written,
    ))
}

/// `current`'s text authors with each field of [`TEXT_FIELDS`] in `set`
/// now `authors`'s, as stored: `None` when every field is the owner's
/// alone and the creator (`creator`) is the owner.
fn text_authors(
    current: Option<BTreeMap<String, Vec<Option<String>>>>,
    creator: Option<&str>,
    set: &[&str],
    authors: &[String],
) -> Option<String> {
    let mut map = current.unwrap_or_default();
    for f in set {
        map.insert(
            (*f).to_string(),
            authors.iter().cloned().map(Some).collect(),
        );
    }
    let creator_owner = matches!(Author::of_token(creator), Author::Owner);
    if creator_owner && owners_alone(&map) {
        None
    } else {
        // A field it does not list is the creator's: list every field, so
        // the stored map stands alone.
        for f in TEXT_FIELDS {
            map.entry(f.to_string())
                .or_insert_with(|| vec![Some(creator.unwrap_or(OWNER).to_string())]);
        }
        Some(serde_json::to_string(&map).expect("serialisable map"))
    }
}

/// The text fields `p` sets (all of them for version 1, which makes them).
fn fields_set(p: &ValidatedPublish, first: bool) -> Vec<&'static str> {
    if first {
        return TEXT_FIELDS.to_vec();
    }
    let given = [
        p.title.is_some(),
        p.description.is_some(),
        p.icon.is_some(),
        p.capabilities.is_some(),
    ];
    TEXT_FIELDS
        .into_iter()
        .zip(given)
        .filter_map(|(f, g)| g.then_some(f))
        .collect()
}

impl Store {
    /// Creates an artifact and writes its version 1. `p.files` are all stored;
    /// nothing is carried forward. The artifact is invisible until version 1 is
    /// recorded. `session_id` becomes the artifact's `owner_session_id` and the
    /// version's `session_id`. Version 1's transaction records, under `ctx`,
    /// `artifact.create` and then `version.publish`.
    pub fn create_artifact(
        &self,
        ctx: &AuditCtx,
        p: ValidatedPublish,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        self.block_on(self.create_artifact_async(ctx, p, session_id))
    }

    /// [`Store::create_artifact`], awaiting its content store.
    pub async fn create_artifact_async(
        &self,
        ctx: &AuditCtx,
        p: ValidatedPublish,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        let id = ArtifactId::generate();
        let now = self.now();
        let title = p
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Untitled".to_string());
        let caps = p.capabilities.clone().unwrap_or(serde_json::json!({}));
        // Reject before inserting, so a failed publish leaves no zero-version row.
        check_collisions(put_paths(&p))?;
        if let Some(tid) = p.addresses.first() {
            return Err(CoreError::invalid(
                "unknown_thread",
                format!("{tid} is not a thread of the new artifact"),
            ));
        }
        let creator = match &p.person {
            Some(person) => Some(person.clone()),
            None => match session_id {
                Some(sid) => self.get_session(sid)?.and_then(|s| s.person),
                None => None,
            },
        };
        self.check_quota(creator.as_deref())?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO artifacts (id, title, description, icon, created_at, updated_at, current_version,
                    pinned, capabilities_json, contract_version, owner_session_id, person)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, ?6, ?7, ?8,
                         COALESCE(?9, (SELECT person FROM sessions WHERE id = ?8)))",
                params![id.as_str(), title, p.description, p.icon, now, caps.to_string(), CONTRACT_VERSION, session_id, p.person],
            )?;
            Ok(())
        })?;
        self.write_version_then_async(ctx, &id, 0, &p, &BTreeMap::new(), session_id, |_, _| Ok(()))
            .await
            .map(|(a, v, ())| (a, v))
    }

    /// Publishes the next version of an existing artifact. `p.if_version` must
    /// equal the current version (`Conflict` otherwise, `if_version_required`
    /// when absent). Files of the previous version not named in `p.files` are
    /// carried forward; `Remove` entries drop a path; `index.html` is always
    /// taken from `p`. `Corrupt` when the artifact or its current version has a
    /// malformed JSON column. `session_id` is recorded on the new version,
    /// and the version as `version.publish` under `ctx`.
    pub fn publish_version(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        p: ValidatedPublish,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        self.block_on(self.publish_version_async(ctx, id, p, session_id))
    }

    /// [`Store::publish_version`], awaiting its content store.
    pub async fn publish_version_async(
        &self,
        ctx: &AuditCtx,
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
        self.write_version_then_async(
            ctx,
            id,
            current.current_version,
            &p,
            &prev,
            session_id,
            |_, _| Ok(()),
        )
        .await
        .map(|(a, v, ())| (a, v))
    }

    /// Writes version `expected + 1`, carrying forward `prev` entries not
    /// named in `p.files`, then, in one transaction, re-checks that the
    /// artifact is still at `expected` (`Conflict` otherwise), records the
    /// version and bumps the artifact.
    ///
    /// The content is written first, through the content store and under
    /// the artifact's content lock, once the artifact is seen at `expected`;
    /// the transaction follows, and when it fails the content written is
    /// removed (see the content module's Ordering). A failed content write
    /// while the artifact has moved past `expected` is `Conflict`.
    ///
    /// Each written file's SHA-256 is recorded in its metadata; a carried
    /// file keeps `prev`'s hash (or, when `prev` has none, its stored bytes
    /// are hashed). The version's `content_sha256` is the hash of that
    /// manifest ([`content_manifest_sha256`]).
    ///
    /// The transaction records the version under `ctx`: `version.publish`,
    /// or `live.snapshot` on a live page; version 1 of an artifact that is
    /// not a live page (whose creation is recorded when the page is made)
    /// is preceded by `artifact.create`.
    ///
    /// `then` runs in the version's transaction once the version row and
    /// its links are written, with the new version's number; an error from
    /// `then` rolls the version back.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn write_version_then<T>(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        expected: u32,
        p: &ValidatedPublish,
        prev: &BTreeMap<String, FileMeta>,
        session_id: Option<&str>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, u32) -> Result<T>,
    ) -> Result<(Artifact, Version, T)> {
        self.block_on(self.write_version_then_async(ctx, id, expected, p, prev, session_id, then))
    }

    /// [`Store::write_version_then`], awaiting its content store.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn write_version_then_async<T>(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        expected: u32,
        p: &ValidatedPublish,
        prev: &BTreeMap<String, FileMeta>,
        session_id: Option<&str>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, u32) -> Result<T>,
    ) -> Result<(Artifact, Version, T)> {
        // Refused before waiting for the lock.
        check_collisions(carried_of(prev, p, false).into_iter().chain(put_paths(p)))?;
        let _lock = self.lock_content(id).await?;
        if expected > 0 {
            // Commits pushed since `expected` become versions first, so a
            // push never blocks a publish and a publish that did not see
            // them is a conflict (the content module's Pushes). One whose
            // pusher may not publish leaves main where no write follows.
            match self
                .record_pushes_locked(&AuditCtx::DAEMON, id, &super::pushes::Pusher::default())
                .await
            {
                Ok(r) => {
                    if let Some(commit) = r.blocked
                        && self.current_version_of(id)? == Some(expected)
                    {
                        return Err(super::pushes::push_refused(&commit));
                    }
                }
                Err(e) => {
                    tracing::warn!(artifact = id.as_str(), error = %e, "could not record pushed commits")
                }
            }
        }
        self.record_version_locked(
            ctx,
            id,
            expected,
            p,
            prev,
            session_id,
            VersionStep::Write,
            then,
        )
        .await
    }

    /// The rest of [`Store::write_version_then_async`], under the content
    /// lock: `step` says how version `expected + 1`'s content comes to be,
    /// written from `p` or a pushed commit adopted as it is. An adopted
    /// commit's `index.html` is carried like any other file.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_version_locked<T>(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        expected: u32,
        p: &ValidatedPublish,
        prev: &BTreeMap<String, FileMeta>,
        session_id: Option<&str>,
        step: VersionStep<'_>,
        then: impl FnOnce(&rusqlite::Transaction<'_>, u32) -> Result<T>,
    ) -> Result<(Artifact, Version, T)> {
        let n = expected + 1;
        let adopt = matches!(step, VersionStep::Adopt(_));
        let carried = carried_of(prev, p, adopt);
        let carried_paths: Vec<String> = carried.iter().map(|p| (*p).clone()).collect();
        check_collisions(carried.iter().copied().chain(put_paths(p)))?;
        if let Some(current) = self.current_version_of(id)?
            && current != expected
        {
            return Err(CoreError::Conflict { current });
        }
        let mut files: BTreeMap<String, FileMeta> = BTreeMap::new();
        for path in &carried {
            let mut meta = prev[*path].clone();
            if meta.sha256.is_none() {
                // Written before file hashes: hash the stored bytes.
                let bytes = self
                    .content
                    .read_file(id, expected, path)
                    .await?
                    .ok_or_else(|| crate::content::missing(id, expected, path))?;
                meta.sha256 = Some(sha256_hex(&bytes));
            }
            files.insert((*path).clone(), meta);
        }
        let mut put = Vec::new();
        for (path, change) in &p.files {
            if let FileChange::Put(f) = change {
                put.push(NewFile {
                    path,
                    bytes: &f.bytes,
                    content_type: &f.content_type,
                });
                files.insert(
                    path.clone(),
                    FileMeta {
                        content_type: f.content_type.clone(),
                        size: f.bytes.len() as u64,
                        sha256: Some(sha256_hex(&f.bytes)),
                    },
                );
            }
        }
        if expected > 0 && matches!(step, VersionStep::Write) {
            // Every agent's repository token blind to the publisher goes
            // before the commit exists (an adopted commit's were revoked
            // before it was recorded; a new artifact has none): refused
            // when one cannot be.
            let person = match &p.person {
                Some(person) => Some(person.clone()),
                None => match session_id {
                    Some(sid) => self.get_session(sid)?.and_then(|s| s.person),
                    None => None,
                },
            };
            self.revoke_blind_tokens(id, &[Author::of_token(person.as_deref())])
                .await
                .map_err(|e| {
                    CoreError::invalid(
                        "repo_unavailable",
                        format!(
                            "a repository token that may not see this version could not be revoked, so it is not published: {e}"
                        ),
                    )
                })?;
        }
        // Whether this call made `n` (an adopted commit may already be it).
        let written = match step {
            VersionStep::Write => {
                let write = VersionWrite {
                    carry: carried.iter().map(|p| p.as_str()).collect(),
                    put,
                    meta: self.version_meta(p, session_id)?,
                };
                self.content
                    .write_version(id, n, &write)
                    .await
                    .map(|()| true)
            }
            VersionStep::Adopt(commit) => self.content.tag_pushed(id, n, commit).await,
        };
        let made = match written {
            Ok(made) => made,
            Err(e) => {
                // Another process may have written `n` and recorded it.
                return Err(match self.current_version_of(id)? {
                    Some(current) if current != expected => CoreError::Conflict { current },
                    _ => e,
                });
            }
        };
        let now = self.now();
        let files_json = serde_json::to_string(&files).expect("serialisable map");
        let content_sha256 = content_manifest_sha256(&files);
        let result = self.with_tx(|tx| {
            let current: u32 = tx.query_row(
                "SELECT current_version FROM artifacts WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )?;
            if current != expected {
                return Err(CoreError::Conflict { current });
            }
            // The publisher's person; a page's own version also keeps the
            // previous version's publisher, fixed now; and each file's
            // authors (spec §5.1, M11).
            let (authors, written) =
                version_authors(tx, id.as_str(), expected, p, session_id, &files)?;
            tx.execute(
                "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json, note,
                    content_sha256, person, by_page, file_authors_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id.as_str(),
                    n,
                    p.label,
                    now,
                    session_id,
                    files_json,
                    p.note,
                    content_sha256,
                    authors.person,
                    p.by_page,
                    authors
                        .files
                        .as_ref()
                        .map(|f| serde_json::to_string(f).expect("serialisable map"))
                ],
            )?;
            link_version(tx, id.as_str(), n, p, &self.now())?;
            let extra = then(tx, n)?;
            // Each text field it sets is its writers' now; the rest keep
            // their authors (spec §8.2).
            let before = tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )??;
            let text = text_authors(
                before.authors.text,
                before.authors.creator.as_deref(),
                &fields_set(p, expected == 0),
                &written,
            );
            tx.execute(
                "UPDATE artifacts SET current_version = ?2, updated_at = ?3,
                    title = COALESCE(?4, title), description = COALESCE(?5, description), icon = COALESCE(?6, icon),
                    capabilities_json = COALESCE(?7, capabilities_json), text_authors_json = ?8
                 WHERE id = ?1",
                params![
                    id.as_str(),
                    n,
                    now,
                    p.title,
                    p.description,
                    p.icon,
                    p.capabilities.as_ref().map(|c| c.to_string()),
                    text
                ],
            )?;
            super::inbox::note_version(
                tx,
                id.as_str(),
                n,
                session_id,
                p.note.as_deref(),
                &self.now(),
            )?;
            let a = tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )??;
            let mut v = tx.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                params![id.as_str(), n],
                row_to_version,
            )??;
            fill_addresses(tx, id, std::slice::from_mut(&mut v))?;
            let page = if a.kind == KIND_LIVE {
                live_page_of_conn(tx, id.as_str())?
            } else {
                None
            };
            if expected == 0 && a.kind != KIND_LIVE {
                self.record_audit(tx, ctx, create_record(&a, &v.created_at))?;
            }
            let rec = version_record(&a, &v, &carried_paths, Some(p.by_page), page.as_ref());
            self.record_audit(tx, ctx, rec)?;
            Ok((a, v, extra))
        });
        // No other writer can hold `n`: this one made it under the lock.
        // An adopted commit that already was `n` may be another process's
        // recorded version, so it stays.
        if result.is_err()
            && made
            && let Err(e) = match step {
                VersionStep::Write => self.content.remove_version(id, n).await,
                VersionStep::Adopt(commit) => self.content.untag_pushed(id, n, commit).await,
            }
        {
            // Left unrecorded: never served, and listed by `clax doctor`.
            tracing::warn!(
                artifact = id.as_str(),
                n,
                error = %e,
                "could not remove an unrecorded version's files"
            );
        }
        result
    }

    /// Who publishes `p` from `session_id`, for the content store: the
    /// person [`version_authors`] records (`p.person`, else the session's,
    /// else the owner's public ID) with their display name, the session's
    /// agent name (its name, else its harness), and the note.
    fn version_meta(&self, p: &ValidatedPublish, session_id: Option<&str>) -> Result<VersionMeta> {
        self.publisher_meta(p.person.as_deref(), session_id, p.note.as_deref())
    }

    /// [`Store::version_meta`] of a version published as `person` (a
    /// public ID; `None`: the session's person, else the owner) from
    /// `session_id`, with `note`.
    pub(super) fn publisher_meta(
        &self,
        person: Option<&str>,
        session_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<VersionMeta> {
        self.with_read(|c| {
            let session: Option<(Option<String>, Option<String>, String)> = match session_id {
                Some(sid) => c
                    .query_row(
                        "SELECT person, name, harness FROM sessions WHERE id = ?1",
                        params![sid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?,
                None => None,
            };
            let person = person
                .map(str::to_string)
                .or_else(|| session.as_ref().and_then(|s| s.0.clone()))
                .filter(|person| person != OWNER);
            let viewer: Option<(String, Option<String>)> = match &person {
                Some(public_id) => c
                    .query_row(
                        "SELECT public_id, display_name FROM viewers WHERE public_id = ?1",
                        params![public_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .or_else(|| Some((public_id.clone(), None))),
                None => c
                    .query_row(
                        "SELECT public_id, display_name FROM viewers WHERE owner = 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?,
            };
            let (person, person_name) = viewer.map_or((None, None), |(id, name)| (Some(id), name));
            Ok(VersionMeta {
                person,
                person_name: person_name.filter(|n| !n.trim().is_empty()),
                agent: session.map(|(_, name, harness)| name.unwrap_or(harness)),
                note: note.filter(|n| !n.trim().is_empty()).map(str::to_string),
            })
        })
    }

    /// The artifact's `current_version`, deleted or not; `None` when there
    /// is no such row.
    pub(super) fn current_version_of(&self, id: &ArtifactId) -> Result<Option<u32>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT current_version FROM artifacts WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Version `n` of artifact `id`, or `None`.
    ///
    /// # Errors
    /// `Corrupt` when its `files_json` is malformed.
    pub fn get_version(&self, id: &ArtifactId, n: u32) -> Result<Option<Version>> {
        self.with_read(|c| {
            let Some(mut v) = c
                .query_row(
                    &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"),
                    params![id.as_str(), n],
                    row_to_version,
                )
                .optional()?
                .transpose()?
            else {
                return Ok(None);
            };
            fill_addresses(c, id, std::slice::from_mut(&mut v))?;
            Ok(Some(v))
        })
    }

    /// Every version of the artifact, oldest first (ascending `n`). Versions with
    /// a malformed `files_json` are logged and left out.
    pub fn list_versions(&self, id: &ArtifactId) -> Result<Vec<Version>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT_VERSION} WHERE artifact_id = ?1 ORDER BY n"
            ))?;
            let mut versions = skip_corrupt(
                stmt.query_map(params![id.as_str()], row_to_version)?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )?;
            fill_addresses(c, id, &mut versions)?;
            Ok(versions)
        })
    }

    /// The metadata of `path` in version `n`, only for files recorded in that
    /// version. `Corrupt` when that version's `files_json` is malformed.
    pub fn file_meta(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<FileMeta>> {
        let Some(mut v) = self.get_version(id, n)? else {
            return Ok(None);
        };
        Ok(v.files.remove(path))
    }

    /// The bytes and metadata of `path` in version `n`, read from the
    /// content store, only for files recorded in that version; `None` also
    /// when the recorded file's bytes are missing. `Corrupt` when that
    /// version's `files_json` is malformed.
    pub fn read_file(
        &self,
        id: &ArtifactId,
        n: u32,
        path: &str,
    ) -> Result<Option<(Vec<u8>, FileMeta)>> {
        self.block_on(self.read_file_async(id, n, path))
    }

    /// [`Store::read_file`], awaiting its content store.
    pub async fn read_file_async(
        &self,
        id: &ArtifactId,
        n: u32,
        path: &str,
    ) -> Result<Option<(Vec<u8>, FileMeta)>> {
        let Some(meta) = self.file_meta(id, n, path)? else {
            return Ok(None);
        };
        Ok(self
            .content
            .read_file(id, n, path)
            .await?
            .map(|bytes| (bytes, meta)))
    }
}

#[cfg(test)]
mod tests {
    use crate::store::artifacts::MetaPatch;
    use crate::store::test_util::DAEMON;
    use crate::{Home, Store};

    use crate::store::test_util::store;

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
        store.set_pinned(DAEMON, &a, true).unwrap();
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
                .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
                .unwrap();
            crate::ArtifactId::parse(&a.id).unwrap()
        };
        let good = mk();
        let bad = mk();
        let old_bad = mk();
        store
            .publish_version(
                DAEMON,
                &old_bad,
                publish(&[("index.html", Some("v2"))], Some(1)),
                None,
            )
            .unwrap();
        store
            .with_write(|c| {
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
            store.file_meta(&old_bad, 1, "index.html").unwrap_err(),
            crate::CoreError::Corrupt { .. }
        ));
        assert!(matches!(
            store
                .publish_version(
                    DAEMON,
                    &bad,
                    publish(&[("index.html", Some("v2"))], Some(1)),
                    None
                )
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
        store.delete_artifact(DAEMON, &gone).unwrap();
        assert!(store.get_artifact(&gone).unwrap().is_none());

        if nix::unistd::geteuid().is_root() {
            eprintln!("skipping unremovable-directory case: root ignores directory modes");
            return;
        }

        let stuck = store.insert_artifact_for_test("B", "2026-01-01T00:00:00.000Z");
        let locked = store.home().artifact_dir(&stuck).join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("f"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = store.delete_artifact(DAEMON, &stuck);
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
                DAEMON,
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
        crate::content::wait(store.content().put_clip(&id, "t1", b"png")).unwrap();
        store.delete_artifact(DAEMON, &id).unwrap();
        assert!(store.get_artifact(&id).unwrap().is_none());
        assert!(store.list_artifacts().unwrap().is_empty());
        assert_eq!(
            crate::store::test_util::content_clip(&store, &id, "t1"),
            None
        );
        assert!(matches!(
            store.delete_artifact(DAEMON, &id),
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
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        let expected = crate::store::migrations::MIGRATIONS.len() as u32;
        assert_eq!(version, expected);

        // Opening a second time should not re-run migrations or error
        let store2 = Store::open(&home).unwrap();
        let version2: u32 = store2
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version2, expected);
    }

    use crate::publish::{Encoding, FileInput, PublishRequest, validate};
    use rusqlite::params;
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
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn create_writes_version_1_and_files() {
        let (_d, store) = store();
        let (a, v) = store
            .create_artifact(
                DAEMON,
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
        let file = |p| crate::store::test_util::content_file(&store, &id, 1, p);
        assert_eq!(file("index.html").unwrap(), b"<p>hi");
        assert_eq!(file("app.js").unwrap(), b"1");
        let (bytes, meta) = store.read_file(&id, 1, "app.js").unwrap().unwrap();
        assert_eq!(bytes, b"1");
        assert_eq!(meta.content_type, "text/javascript");
        assert_eq!(store.file_meta(&id, 1, "app.js").unwrap(), Some(meta));
        assert!(store.read_file(&id, 1, "nope.js").unwrap().is_none());
        assert!(store.file_meta(&id, 1, "nope.js").unwrap().is_none());
    }

    #[test]
    fn publish_version_carries_files_forward_and_honours_removals() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(
                DAEMON,
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
                DAEMON,
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
        let file = |p| crate::store::test_util::content_file(&store, &id, 2, p);
        assert_eq!(file("a.js").unwrap(), b"a");
        assert_eq!(file("b.css"), None);
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

    const X_SHA256: &str = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";

    #[test]
    fn content_sha256_matches_manifest_rule() {
        let (_d, store) = store();
        let (a, v) = store
            .create_artifact(
                DAEMON,
                publish(
                    &[("index.html", Some("<p>hi</p>")), ("app.js", Some("x"))],
                    None,
                ),
                None,
            )
            .unwrap();
        // "app.js\0<sha256(x)>\01\nindex.html\0<sha256(<p>hi</p>)>\09\n"
        let expected = "sha256:33754657b6a0ce355999e0dc8adbb0d2c6c2780035a2c1d9e43d707782c3ad9a";
        assert_eq!(v.content_sha256.as_deref(), Some(expected));
        assert_eq!(v.files["app.js"].sha256.as_deref(), Some(X_SHA256));
        assert_eq!(
            v.files["index.html"].sha256.as_deref(),
            Some("0a4735281db700223af63abc387c351f64ea6961a1ef955631df08d96169e772")
        );
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let stored = store.get_version(&id, 1).unwrap().unwrap();
        assert_eq!(stored.content_sha256.as_deref(), Some(expected));
        assert_eq!(stored.files, v.files);
    }

    #[test]
    fn carried_files_keep_their_hash() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(
                DAEMON,
                publish(&[("index.html", Some("v1")), ("app.js", Some("x"))], None),
                None,
            )
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        // The carried file's hash comes from version 1's record, not from
        // re-reading its bytes.
        let c = store.content();
        crate::content::wait(c.remove_version(&id, 1)).unwrap();
        let changed = crate::content::VersionWrite {
            carry: Vec::new(),
            put: vec![
                crate::content::NewFile {
                    path: "index.html",
                    bytes: b"v1",
                    content_type: "text/html",
                },
                crate::content::NewFile {
                    path: "app.js",
                    bytes: b"changed in the content store",
                    content_type: "text/javascript",
                },
            ],
            meta: crate::content::VersionMeta::default(),
        };
        crate::content::wait(c.write_version(&id, 1, &changed)).unwrap();
        let (_, v2) = store
            .publish_version(
                DAEMON,
                &id,
                publish(&[("index.html", Some("v2"))], Some(1)),
                None,
            )
            .unwrap();
        assert_eq!(v2.files["app.js"].sha256.as_deref(), Some(X_SHA256));
        assert_eq!(
            v2.content_sha256,
            crate::audit::content_manifest_sha256(&v2.files)
        );
        assert!(v2.content_sha256.is_some());

        // A carried file whose version predates file hashes is hashed from
        // its stored bytes.
        store
            .with_write(|c| {
                c.execute(
                    "UPDATE versions SET files_json = ?2, content_sha256 = NULL
                     WHERE artifact_id = ?1 AND n = 2",
                    params![
                        id.as_str(),
                        r#"{"app.js":{"content_type":"text/javascript","size":28},"index.html":{"content_type":"text/html","size":2}}"#
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let (_, v3) = store
            .publish_version(
                DAEMON,
                &id,
                publish(&[("index.html", Some("v3"))], Some(2)),
                None,
            )
            .unwrap();
        assert_eq!(
            v3.files["app.js"].sha256.as_deref(),
            Some(crate::audit::sha256_hex(b"changed in the content store").as_str())
        );
        assert_eq!(
            v3.content_sha256,
            crate::audit::content_manifest_sha256(&v3.files)
        );
    }

    #[test]
    fn stale_if_version_conflicts_and_writes_nothing() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store
            .publish_version(
                DAEMON,
                &id,
                publish(&[("index.html", Some("v2"))], Some(7)),
                None,
            )
            .unwrap_err();
        assert!(matches!(e, crate::CoreError::Conflict { current: 1 }));
        let e = store
            .publish_version(
                DAEMON,
                &id,
                publish(&[("index.html", Some("v2"))], None),
                None,
            )
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
            .with_write(|c| {
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
    fn a_put_over_a_hidden_path_drops_it_where_a_visible_one_is_refused() {
        let (_d, store) = store();
        let (a, _) = store
            .create_artifact(
                DAEMON,
                publish(
                    &[
                        ("index.html", Some("v1")),
                        ("Name.js", Some("1")),
                        ("dir/b.css", Some("2")),
                        ("keep.css", Some("3")),
                    ],
                    None,
                ),
                None,
            )
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let puts: &[(&str, Option<&str>)] = &[
            ("index.html", Some("v2")),
            ("name.js", Some("4")),
            ("dir", Some("5")),
        ];
        // Visible, each collision is refused, and nothing is written.
        let e = store
            .publish_version(DAEMON, &id, publish(puts, Some(1)), None)
            .unwrap_err();
        assert!(
            matches!(
                e,
                crate::CoreError::Invalid {
                    code: "invalid_path",
                    ..
                }
            ),
            "{e:?}"
        );
        // Hidden from the publisher, the colliding paths are dropped as a
        // remove would drop them; the rest are carried.
        let mut p = publish(puts, Some(1));
        p.hidden = vec!["Name.js".into(), "dir/b.css".into(), "keep.css".into()];
        let (_, v) = store.publish_version(DAEMON, &id, p, None).unwrap();
        let files: Vec<&String> = v.files.keys().collect();
        assert_eq!(files, ["dir", "index.html", "keep.css", "name.js"]);
    }

    #[test]
    fn colliding_paths_are_rejected() {
        let (_d, store) = store();
        for (a, b) in [("a", "a/b.js"), ("App.js", "app.js")] {
            let e = store
                .create_artifact(
                    DAEMON,
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
            .with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(rows, 0, "a rejected create leaves no artifact row");

        let (a, _) = store
            .create_artifact(
                DAEMON,
                publish(&[("index.html", Some("v1")), ("a", Some("1"))], None),
                None,
            )
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store
            .publish_version(
                DAEMON,
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
                DAEMON,
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
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let handles: Vec<_> = ["left", "right"]
            .into_iter()
            .map(|body| {
                let store = store.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    let r = store.publish_version(
                        DAEMON,
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
        // Only the winner's version is recorded.
        let published: Vec<serde_json::Value> = store
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "version.publish")
            .map(|e| serde_json::from_str::<serde_json::Value>(&e.body).unwrap()["n"].clone())
            .collect();
        assert_eq!(published, [1, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_keep_running_while_publishes_of_one_artifact_wait_for_its_content_lock() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};
        // The pool's workers: one connection has none to hold.
        let (_d, st) = crate::store::test_util::pool_store();
        let st = Arc::new(st);
        let (a, _) = st
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        // Another process holds the write lock: the first publish waits on
        // it holding the artifact's content lock, the rest for that lock.
        let other = rusqlite::Connection::open(st.home().db_path()).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let n = super::super::exec::reader_count() + 2;
        let started = Arc::new(AtomicUsize::new(0));
        let publishes: Vec<_> = (0..n)
            .map(|i| {
                let (st, started, id) = (st.clone(), started.clone(), id.clone());
                tokio::spawn(async move {
                    st.call(move |s| {
                        started.fetch_add(1, Ordering::SeqCst);
                        let body = format!("v2 from {i}");
                        s.publish_version(
                            DAEMON,
                            &id,
                            publish(&[("index.html", Some(&body))], Some(1)),
                            None,
                        )
                    })
                    .await
                })
            })
            .collect();
        // More publishes than base workers all start: those waiting for the
        // content lock do not hold the workers reads need.
        let deadline = Instant::now() + Duration::from_secs(2);
        while started.load(Ordering::SeqCst) < n {
            assert!(
                Instant::now() < deadline,
                "publishes waiting for the content lock hold every worker"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let read =
            tokio::time::timeout(Duration::from_secs(1), st.call(|s| s.integrity_check())).await;
        assert_eq!(
            read.expect("the read ran behind the waiting publishes")
                .unwrap(),
            "ok"
        );
        other.execute_batch("COMMIT").unwrap();
        let mut wins = 0;
        for p in publishes {
            match p.await.unwrap() {
                Ok(_) => wins += 1,
                Err(e) => assert!(matches!(e, crate::CoreError::Conflict { .. }), "{e:?}"),
            }
        }
        assert_eq!(wins, 1);
    }

    #[test]
    fn a_stray_version_directory_refuses_the_publish_and_records_nothing() {
        // The home's files: a stray directory is theirs.
        let (_d, store) = crate::store::test_util::pool_store();
        let (a, _) = store
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let seq = store.events_after(0, 100).unwrap().last().unwrap().seq;
        // A directory in version 2's place makes the content write, before
        // the transaction, fail.
        let vdir = store.home().version_dir(&id, 2);
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("stray"), "x").unwrap();
        assert!(
            store
                .publish_version(
                    DAEMON,
                    &id,
                    publish(&[("index.html", Some("v2"))], Some(1)),
                    None
                )
                .is_err()
        );
        assert_eq!(store.events_after(seq, 100).unwrap(), vec![]);
        assert_eq!(store.get_artifact(&id).unwrap().unwrap().current_version, 1);
        assert!(vdir.join("stray").exists(), "the stray is left for doctor");
    }

    #[test]
    fn a_transaction_failing_after_the_content_write_removes_the_content() {
        let (_d, store) = store();
        let (a, v1) = store
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let seq = store.events_after(0, 100).unwrap().last().unwrap().seq;
        let p = publish(&[("index.html", Some("v2"))], Some(1));
        let e = store
            .write_version_then(DAEMON, &id, 1, &p, &v1.files, None, |_, n| {
                // The content is written by the time the transaction runs.
                assert!(crate::store::test_util::content_version_exists(
                    &store, &id, n
                ));
                Err::<(), _>(crate::CoreError::invalid("refused", "refused"))
            })
            .unwrap_err();
        assert!(matches!(
            e,
            crate::CoreError::Invalid {
                code: "refused",
                ..
            }
        ));
        assert!(!crate::store::test_util::content_version_exists(
            &store, &id, 2
        ));
        assert_eq!(store.events_after(seq, 100).unwrap(), vec![]);
        // Nothing left blocks the next publish of version 2.
        store.publish_version(DAEMON, &id, p, None).unwrap();
        assert_eq!(
            store.read_file(&id, 2, "index.html").unwrap().unwrap().0,
            b"v2"
        );
    }

    #[test]
    fn a_store_keeps_its_content_in_the_content_store_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let content = std::sync::Arc::new(crate::content::memory::MemContent::new());
        let store = Store::open(&home).unwrap().with_content(content.clone());
        let (a, _) = store
            .create_artifact(
                DAEMON,
                publish(&[("index.html", Some("v1")), ("app.js", Some("one"))], None),
                None,
            )
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let (_, v2) = crate::content::wait(store.publish_version_async(
            DAEMON,
            &id,
            publish(&[("index.html", Some("v2"))], Some(1)),
            None,
        ))
        .unwrap();
        assert_eq!(
            v2.files.keys().collect::<Vec<_>>(),
            ["app.js", "index.html"]
        );
        assert_eq!(
            store.read_file(&id, 2, "app.js").unwrap().unwrap().0,
            b"one"
        );
        assert_eq!(
            store.read_file(&id, 2, "index.html").unwrap().unwrap().0,
            b"v2"
        );
        assert_eq!(
            store.read_file(&id, 1, "index.html").unwrap().unwrap().0,
            b"v1"
        );
        let asset = store.add_asset(DAEMON, &id, "image/png", b"png").unwrap();
        assert_eq!(store.read_asset(&asset.id).unwrap().unwrap().1, b"png");
        assert_eq!(content.versions(), [(a.id.clone(), 1), (a.id.clone(), 2)]);
        assert_eq!(content.asset_count(), 1);
        assert!(!home.artifact_dir(&id).exists(), "nothing under the home");
        store.delete_asset(DAEMON, &asset.id).unwrap();
        assert_eq!(content.asset_count(), 0);
        store.delete_artifact(DAEMON, &id).unwrap();
        assert!(content.versions().is_empty());
    }

    /// A content store in memory that keeps the metadata of each write.
    #[derive(Default)]
    struct Recording {
        inner: crate::content::memory::MemContent,
        metas: std::sync::Mutex<Vec<(u32, crate::content::VersionMeta)>>,
    }

    #[async_trait::async_trait]
    impl crate::content::ContentStore for Recording {
        async fn read_file(
            &self,
            id: &crate::ArtifactId,
            n: u32,
            path: &str,
        ) -> crate::Result<Option<Vec<u8>>> {
            self.inner.read_file(id, n, path).await
        }
        async fn write_version(
            &self,
            id: &crate::ArtifactId,
            n: u32,
            write: &crate::content::VersionWrite<'_>,
        ) -> crate::Result<()> {
            self.metas.lock().unwrap().push((n, write.meta.clone()));
            self.inner.write_version(id, n, write).await
        }
        async fn remove_version(&self, id: &crate::ArtifactId, n: u32) -> crate::Result<()> {
            self.inner.remove_version(id, n).await
        }
        async fn delete_artifact(&self, id: &crate::ArtifactId) -> crate::Result<()> {
            self.inner.delete_artifact(id).await
        }
        async fn put_asset(
            &self,
            key: &crate::content::AssetKey,
            bytes: &[u8],
        ) -> crate::Result<()> {
            self.inner.put_asset(key, bytes).await
        }
        async fn read_asset(
            &self,
            key: &crate::content::AssetKey,
        ) -> crate::Result<Option<Vec<u8>>> {
            self.inner.read_asset(key).await
        }
        async fn delete_asset(&self, key: &crate::content::AssetKey) -> crate::Result<()> {
            self.inner.delete_asset(key).await
        }
    }

    #[test]
    fn a_version_write_names_its_publisher_agent_and_note() {
        use crate::content::VersionMeta;
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let content = std::sync::Arc::new(Recording::default());
        let store = Store::open(&home).unwrap().with_content(content.clone());
        // The owner, before they have an identity: no person.
        let (a, _) = store
            .create_artifact(DAEMON, publish(&[("index.html", Some("v1"))], None), None)
            .unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let owner = store.set_owner_name("Alex", false).unwrap();
        let session = store
            .register_session(
                DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    cwd: "/w/web".into(),
                    repo: Some("web".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut p = publish(&[("index.html", Some("v2"))], Some(1));
        p.note = Some("Tighter layout".into());
        store
            .publish_version(DAEMON, &id, p, Some(&session.id))
            .unwrap();
        let rae = store
            .upsert_viewer(&crate::new_ulid(), Some("Rae"))
            .unwrap();
        let mut p = publish(&[("index.html", Some("v3"))], Some(2));
        p.person = Some(rae.public_id.clone());
        store.publish_version(DAEMON, &id, p, None).unwrap();
        assert_eq!(
            *content.metas.lock().unwrap(),
            [
                (1, VersionMeta::default()),
                (
                    2,
                    VersionMeta {
                        person: Some(owner.public_id.clone()),
                        person_name: Some("Alex".into()),
                        agent: session.name.clone(),
                        note: Some("Tighter layout".into()),
                    }
                ),
                (
                    3,
                    VersionMeta {
                        person: Some(rae.public_id.clone()),
                        person_name: Some("Rae".into()),
                        agent: None,
                        note: None,
                    }
                ),
            ]
        );
        assert!(session.name.is_some(), "a registered agent is named");
    }

    fn one_version(store: &Store) -> crate::ArtifactId {
        let p = crate::publish::validate(
            serde_json::from_value(serde_json::json!({
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap();
        crate::ArtifactId::parse(&store.create_artifact(DAEMON, p, None).unwrap().0.id).unwrap()
    }

    #[test]
    fn corrupt_deleted_artifact_with_threads_and_watches_is_cleared() {
        let (_d, store) = store();
        let aid = one_version(&store);
        let session = crate::store::test_util::session(&store, "claude", "h1");
        store.watch(DAEMON, &session, &aid, true).unwrap();
        let t = store
            .create_thread(
                DAEMON,
                &aid,
                crate::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: crate::store::test_util::anchor(),
                    author_name: "Alex".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        store
            .with_write(|c| {
                c.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![crate::new_ulid(), t.id, t.comments[0].id, session, store.now()],
                )?;
                Ok(())
            })
            .unwrap();
        store.delete_artifact(DAEMON, &aid).unwrap();
        store
            .with_write(|c| {
                c.execute(
                    "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
                    [aid.as_str()],
                )?;
                c.execute(
                    "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                     VALUES (?1, 't/1', 't', '{}', 1, 'x')",
                    [aid.as_str()],
                )?;
                c.execute(
                    "INSERT INTO leases (artifact_id, path, holder, expires_at) VALUES (?1, 't/1', 'h', 'x')",
                    [aid.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.delete_corrupt_deleted_rows().unwrap(), 1);
        for table in [
            "feedback",
            "comments",
            "threads",
            "watches",
            "docs",
            "leases",
            "artifacts",
        ] {
            let n: i64 = store
                .with_read(|c| {
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
            .with_write(|c| {
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
        store.delete_artifact(DAEMON, &gone_caps).unwrap();
        store.delete_artifact(DAEMON, &gone_files).unwrap();
        store
            .with_write(|c| {
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
        store.delete_artifact(DAEMON, &gone).unwrap();
        store
            .with_write(|c| {
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
