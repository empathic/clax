//! The instance's side of the artifact stores: the rows an artifact's
//! store takes over once (`app_data_moved` marks the artifacts that moved;
//! the instance's `docs`, `doc_authors` and `leases` tables hold the rest),
//! and the log of viewer folds every artifact store applies in turn
//! (`app_data_folds`).

use super::{AuditEnv, Exported, ExportedDoc, ExportedLease, Fold};
use crate::Store;
use clax_core::audit::AuditCtx;
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Where an artifact's app data stands, read before each call to its
/// store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppDataState {
    /// Its rows moved to its store.
    pub moved: bool,
    /// The newest viewer fold in the log (0 when there is none).
    pub folds: i64,
    /// The newest of its store's audit records the instance's history
    /// holds a copy of ([`Store::mirror_app_data_audit`]).
    pub audit_seq: i64,
}

/// Appends a fold of viewer `old` into `new` to the log, in the fold's
/// transaction `tx`, recorded under `ctx`.
pub(crate) fn log_fold(
    tx: &rusqlite::Transaction<'_>,
    ctx: &AuditCtx,
    old: &str,
    new: &str,
    into_owner: bool,
) -> Result<()> {
    let audit = serde_json::to_string(&AuditEnv::of(ctx)).expect("serialisable envelope");
    tx.execute(
        "INSERT INTO app_data_folds (old, new, into_owner, audit) VALUES (?1, ?2, ?3, ?4)",
        params![old, new, into_owner, audit],
    )?;
    Ok(())
}

impl Store {
    /// Whether live artifact `aid`'s rows moved to its store, and the
    /// newest fold in the log.
    ///
    /// # Errors
    /// `NotFound` for an artifact that is missing or deleted.
    pub fn app_data_state(&self, aid: &ArtifactId) -> Result<AppDataState> {
        self.with_read(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM app_data_moved WHERE artifact_id = ?1),
                        COALESCE((SELECT MAX(seq) FROM app_data_folds), 0),
                        COALESCE((SELECT audit_seq FROM app_data_moved WHERE artifact_id = ?1), 0)
                   FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                params![aid.as_str()],
                |r| {
                    Ok(AppDataState {
                        moved: r.get(0)?,
                        folds: r.get(1)?,
                        audit_seq: r.get(2)?,
                    })
                },
            )
            .optional()?
            .ok_or(CoreError::NotFound)
        })
    }

    /// Artifact `aid`'s documents, their authors, its leases and its
    /// document sequence as the instance holds them, with the newest fold
    /// they reflect, read in one snapshot.
    pub fn export_app_data(&self, aid: &ArtifactId) -> Result<Exported> {
        self.with_read(|c| {
            let id = aid.as_str();
            let docs = c
                .prepare(
                    "SELECT path, collection, json, version, updated_at FROM docs
                      WHERE artifact_id = ?1 ORDER BY path",
                )?
                .query_map(params![id], |r| {
                    Ok(ExportedDoc {
                        path: r.get(0)?,
                        collection: r.get(1)?,
                        json: r.get(2)?,
                        version: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let authors = c
                .prepare("SELECT path, author FROM doc_authors WHERE artifact_id = ?1 ORDER BY rowid")?
                .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let leases = c
                .prepare(
                    "SELECT path, holder, expires_at FROM leases WHERE artifact_id = ?1 ORDER BY path",
                )?
                .query_map(params![id], |r| {
                    Ok(ExportedLease {
                        path: r.get(0)?,
                        holder: r.get(1)?,
                        expires_at: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let doc_seq: i64 = c
                .query_row(
                    "SELECT doc_seq FROM artifacts WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let fold_seq: i64 = c.query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM app_data_folds",
                [],
                |r| r.get(0),
            )?;
            Ok(Exported {
                artifact_id: id.to_string(),
                docs,
                authors,
                leases,
                doc_seq,
                fold_seq,
            })
        })
    }

    /// Marks artifact `aid`'s rows moved to its store and deletes them from
    /// the instance, in one transaction: once its store holds them
    /// ([`super::AppData::import`]), the instance holds no app data of
    /// it.
    pub fn forget_app_data(&self, aid: &ArtifactId) -> Result<()> {
        self.with_tx(|tx| {
            let id = aid.as_str();
            tx.execute(
                "INSERT OR IGNORE INTO app_data_moved (artifact_id) VALUES (?1)",
                params![id],
            )?;
            for table in ["docs", "doc_authors", "leases"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE artifact_id = ?1"),
                    params![id],
                )?;
            }
            Ok(())
        })
    }

    /// The viewer folds in the log with a `seq` above `seq`, oldest first.
    pub fn app_data_folds_after(&self, seq: i64) -> Result<Vec<Fold>> {
        self.with_read(|c| {
            let rows: Vec<(i64, String, String, bool, String)> = c
                .prepare_cached(
                    "SELECT seq, old, new, into_owner, audit FROM app_data_folds
                      WHERE seq > ?1 ORDER BY seq",
                )?
                .query_map(params![seq], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows.into_iter()
                .map(|(seq, old, new, into_owner, audit)| {
                    let audit = serde_json::from_str(&audit).map_err(|_| CoreError::Corrupt {
                        artifact_id: String::new(),
                        column: "app_data_folds.audit",
                        version: None,
                    })?;
                    Ok(Fold {
                        seq,
                        old,
                        new,
                        into_owner,
                        audit,
                    })
                })
                .collect()
        })
    }

    /// The authors a page's write to artifact `aid`'s documents adds: every
    /// author of every file of its current version, as stored tokens.
    pub fn doc_page_authors(&self, aid: &ArtifactId) -> Result<Vec<String>> {
        self.with_read(|c| crate::artifacts::page_authors_in(c, aid.as_str()))
    }

    /// The live artifacts whose rows the instance still holds (documents,
    /// authors or leases) and has not moved to their stores, in ID order.
    pub fn app_data_pending(&self) -> Result<Vec<ArtifactId>> {
        self.with_read(|c| {
            let ids: Vec<String> = c
                .prepare(
                    "SELECT a.id FROM artifacts a
                      WHERE a.deleted_at IS NULL
                        AND NOT EXISTS (SELECT 1 FROM app_data_moved m WHERE m.artifact_id = a.id)
                        AND (EXISTS (SELECT 1 FROM docs d WHERE d.artifact_id = a.id)
                          OR EXISTS (SELECT 1 FROM doc_authors x WHERE x.artifact_id = a.id)
                          OR EXISTS (SELECT 1 FROM leases l WHERE l.artifact_id = a.id))
                      ORDER BY a.id",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids
                .iter()
                .filter_map(|id| ArtifactId::parse(id).ok())
                .collect())
        })
    }
}
