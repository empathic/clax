//! Apps pushed from the Clax app to a hosted Clax, and what the hosted
//! Clax counts of them.
//!
//! - Each public version published from the app carries one bundle
//!   ([`AppBundle`]): a `.clax` file kept as the artifact's asset
//!   `bundle-<sha256>.clax` in the content store, never one of the version's
//!   served files, and a row of `app_bundles` naming its lineage, its
//!   release counter (`bundle_version`) and the hash of the files it holds,
//!   which must equal the version's `content_sha256`.
//! - Every bundle of one artifact has its first bundle's lineage, and each
//!   new one a `bundle_version` greater than any before it.
//! - [`Store::record_install`] and [`Store::install_count`] count distinct
//!   installs of an app; [`Store::fork_count`] its live forks.
//! - [`Store::apps_of`] lists a person's apps library, and
//!   [`Store::person_card`] names an app's owner and publisher.

use super::Store;
use super::shares::ShareAccess;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord, sha256_hex};
use clax_core::content::AssetKey;
use clax_core::model::FileMeta;
use clax_core::publish::MAX_BODY_BYTES;
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// The largest bundle a version may carry, in bytes (32 MiB, inclusive).
pub const MAX_BUNDLE_BYTES: u64 = 33_554_432;

/// The extension of a bundle's asset key in the content store.
pub const BUNDLE_EXT: &str = "clax";

/// The bundle a public version published from the Clax app carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppBundle {
    pub artifact_id: String,
    /// The artifact version that carries it.
    pub version: u32,
    /// The app's lineage, the same for every bundle of the artifact.
    pub lineage: String,
    /// The lineage's release counter, greater than every earlier bundle's.
    pub bundle_version: u64,
    /// The hash of the files the bundle holds: the version's `content_sha256`.
    pub content_sha256: String,
    /// The SHA-256 of the bundle's bytes, lowercase hex.
    pub sha256: String,
    /// The bundle's size in bytes.
    pub size: u64,
    /// The publishing account's public ID; `None` for the owner.
    pub person: Option<String>,
    pub created_at: String,
}

const COLS: &str = "artifact_id, version, lineage, bundle_version, content_sha256, sha256, size,
    person, created_at";

fn row_bundle(r: &Row<'_>) -> rusqlite::Result<AppBundle> {
    Ok(AppBundle {
        artifact_id: r.get(0)?,
        version: r.get(1)?,
        lineage: r.get(2)?,
        bundle_version: r.get::<_, i64>(3)? as u64,
        content_sha256: r.get(4)?,
        sha256: r.get(5)?,
        size: r.get::<_, i64>(6)? as u64,
        person: r.get(7)?,
        created_at: r.get(8)?,
    })
}

/// The content store's key of the bundle of `aid` whose bytes hash to
/// `sha256`.
fn bundle_key(aid: &ArtifactId, sha256: &str) -> AssetKey {
    AssetKey {
        artifact: aid.clone(),
        id: format!("bundle-{sha256}"),
        ext: BUNDLE_EXT.to_string(),
    }
}

fn bundle_in(c: &Connection, aid: &str, version: u32) -> Result<Option<AppBundle>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM app_bundles WHERE artifact_id = ?1 AND version = ?2"),
        params![aid, version],
        row_bundle,
    )
    .optional()?)
}

/// What a new bundle for version `version` of `aid` must agree with: the
/// live artifact, the version and its files' hash, the version's size
/// limit with the bundle's `size` added, the first bundle's lineage, the
/// highest release counter. `Ok(Some)` is the row a bundle of the same
/// bytes already has; `Ok(None)` lets the new one in.
#[allow(clippy::too_many_arguments)]
fn check_bundle(
    c: &Connection,
    aid: &str,
    version: u32,
    lineage: &str,
    bundle_version: u64,
    content_sha256: &str,
    sha256: &str,
    size: u64,
) -> Result<Option<AppBundle>> {
    super::artifacts::live_artifact_in(c, aid)?.ok_or(CoreError::NotFound)?;
    let row: Option<(Option<String>, String)> = c
        .query_row(
            "SELECT content_sha256, files_json FROM versions WHERE artifact_id = ?1 AND n = ?2",
            params![aid, version],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (files, files_json) = row.ok_or(CoreError::NotFound)?;
    if files.as_deref() != Some(content_sha256) {
        return Err(CoreError::invalid(
            "bundle_mismatch",
            format!("version {version}'s files are not the bundle's"),
        ));
    }
    if let Some(held) = bundle_in(c, aid, version)? {
        if held.sha256 == sha256 {
            return Ok(Some(held));
        }
        return Err(CoreError::invalid(
            "bundle_mismatch",
            format!("version {version} carries another bundle"),
        ));
    }
    let metas: BTreeMap<String, FileMeta> =
        serde_json::from_str(&files_json).map_err(|_| CoreError::Corrupt {
            artifact_id: aid.to_string(),
            column: "versions.files_json",
            version: Some(version),
        })?;
    let total = metas.values().map(|m| m.size).sum::<u64>() + size;
    if total > MAX_BODY_BYTES {
        return Err(CoreError::invalid(
            "bundle_too_large",
            format!(
                "version {version}'s files and its bundle may be at most {MAX_BODY_BYTES} bytes"
            ),
        ));
    }
    let first: Option<String> = c
        .query_row(
            "SELECT lineage FROM app_bundles WHERE artifact_id = ?1 ORDER BY version LIMIT 1",
            params![aid],
            |r| r.get(0),
        )
        .optional()?;
    if first.is_some_and(|l| l != lineage) {
        return Err(CoreError::invalid(
            "bundle_lineage",
            "the bundle is of another lineage than the app's earlier bundles",
        ));
    }
    let top: Option<i64> = c.query_row(
        "SELECT MAX(bundle_version) FROM app_bundles WHERE artifact_id = ?1",
        params![aid],
        |r| r.get(0),
    )?;
    if let Some(top) = top
        && bundle_version <= top as u64
    {
        return Err(CoreError::invalid(
            "bundle_version_order",
            format!("bundle version {bundle_version} is not above the last, {top}"),
        ));
    }
    Ok(None)
}

impl Store {
    /// Stores `bytes` as the bundle version `version` of `aid` carries,
    /// published by `person` (a public ID; `None` for the owner), with
    /// lineage `lineage`, release counter `bundle_version`, and holding the
    /// files whose hash is `content_sha256`. The bytes go to the content
    /// store, under the artifact's content lock, before the row, and are
    /// removed again when the row's transaction fails. The transaction
    /// records `artifact.update` (field `app_bundle`) under `ctx`.
    ///
    /// The same bytes again for a version that has them, with the version's
    /// `content_sha256`, answer the row it has, unchanged, whatever
    /// `lineage`, `bundle_version` and `person` say (a retry whose answer
    /// was lost); a `content_sha256` that is not the version's is refused
    /// first.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted, or has no
    /// version `version`; `Invalid` with code:
    /// - `invalid_args`: `bundle_version` is over `i64::MAX`, the most
    ///   the database keeps;
    /// - `bundle_too_large`: `bytes` is over [`MAX_BUNDLE_BYTES`], or the
    ///   version's files and `bytes` together are over
    ///   [`MAX_BODY_BYTES`], the version's size limit;
    /// - `bundle_mismatch`: `content_sha256` is not the version's
    ///   `content_sha256`, or the version carries other bytes;
    /// - `bundle_lineage`: `lineage` is not the artifact's first bundle's;
    /// - `bundle_version_order`: `bundle_version` is not above every earlier
    ///   bundle's of the artifact.
    #[allow(clippy::too_many_arguments)]
    pub async fn put_app_bundle_async(
        &self,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        version: u32,
        lineage: &str,
        bundle_version: u64,
        content_sha256: &str,
        bytes: &[u8],
        person: Option<&str>,
    ) -> Result<AppBundle> {
        if bytes.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(CoreError::invalid(
                "bundle_too_large",
                format!("a bundle may be at most {MAX_BUNDLE_BYTES} bytes"),
            ));
        }
        if i64::try_from(bundle_version).is_err() {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("a bundle version may be at most {}", i64::MAX),
            ));
        }
        let sha256 = sha256_hex(bytes);
        let id = aid.as_str();
        let check = |c: &Connection| {
            check_bundle(
                c,
                id,
                version,
                lineage,
                bundle_version,
                content_sha256,
                &sha256,
                bytes.len() as u64,
            )
        };
        let _lock = self.lock_content(aid).await?;
        if let Some(held) = self.with_read(check)? {
            return Ok(held);
        }
        let key = bundle_key(aid, &sha256);
        self.content.put_asset(&key, bytes).await?;
        let inserted = self.with_tx(|tx| {
            if let Some(held) = check(tx)? {
                return Ok(held);
            }
            let b = AppBundle {
                artifact_id: id.to_string(),
                version,
                lineage: lineage.to_string(),
                bundle_version,
                content_sha256: content_sha256.to_string(),
                sha256: sha256.clone(),
                size: bytes.len() as u64,
                person: person.map(str::to_string),
                created_at: self.now(),
            };
            tx.execute(
                &format!(
                    "INSERT INTO app_bundles ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                ),
                params![
                    b.artifact_id,
                    b.version,
                    b.lineage,
                    b.bundle_version as i64,
                    b.content_sha256,
                    b.sha256,
                    b.size as i64,
                    b.person,
                    b.created_at
                ],
            )?;
            let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, b.created_at.as_str()).with(
                "fields",
                serde_json::json!({"app_bundle": {
                    "version": version,
                    "lineage": lineage,
                    "bundle_version": bundle_version,
                    "sha256": format!("sha256:{sha256}"),
                    "size": b.size,
                }}),
            );
            rec.ids.artifact = Some(b.artifact_id.clone());
            self.record_audit(tx, ctx, rec)?;
            Ok(b)
        });
        if inserted.is_err() {
            // Bytes no row names; another version's bundle of the same
            // bytes keeps them.
            let named = self.with_read(|c| {
                Ok(c.query_row(
                    "SELECT EXISTS (SELECT 1 FROM app_bundles WHERE artifact_id = ?1 AND sha256 = ?2)",
                    params![id, sha256],
                    |r| r.get::<_, bool>(0),
                )?)
            });
            if matches!(named, Ok(false)) {
                let _ = self.content.delete_asset(&key).await;
            }
        }
        inserted
    }

    /// [`Store::put_app_bundle_async`], blocking on its content store.
    #[allow(clippy::too_many_arguments)]
    pub fn put_app_bundle(
        &self,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        version: u32,
        lineage: &str,
        bundle_version: u64,
        content_sha256: &str,
        bytes: &[u8],
        person: Option<&str>,
    ) -> Result<AppBundle> {
        self.block_on(self.put_app_bundle_async(
            ctx,
            aid,
            version,
            lineage,
            bundle_version,
            content_sha256,
            bytes,
            person,
        ))
    }

    /// The bundle version `version` of `aid` carries; `None` when it
    /// carries none (a version published from the web), or the artifact is
    /// deleted.
    pub fn app_bundle(&self, aid: &ArtifactId, version: u32) -> Result<Option<AppBundle>> {
        self.with_read(|c| bundle_in(c, aid.as_str(), version))
    }

    /// The bundle of `aid`'s highest version that carries one; `None` when
    /// none does.
    pub fn latest_app_bundle(&self, aid: &ArtifactId) -> Result<Option<AppBundle>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {COLS} FROM app_bundles WHERE artifact_id = ?1
                     ORDER BY version DESC LIMIT 1"
                ),
                params![aid.as_str()],
                row_bundle,
            )
            .optional()?)
        })
    }

    /// The bytes of bundle `b`.
    ///
    /// # Errors
    /// `Corrupt` when its artifact ID is not an ID; `NotFound` when the
    /// content store has no bytes for it.
    pub async fn read_app_bundle_async(&self, b: &AppBundle) -> Result<Vec<u8>> {
        let aid = ArtifactId::parse(&b.artifact_id).map_err(|_| CoreError::Corrupt {
            artifact_id: b.artifact_id.clone(),
            column: "app_bundles.artifact_id",
            version: Some(b.version),
        })?;
        self.content
            .read_asset(&bundle_key(&aid, &b.sha256))
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// [`Store::read_app_bundle_async`], blocking on its content store.
    pub fn read_app_bundle(&self, b: &AppBundle) -> Result<Vec<u8>> {
        self.block_on(self.read_app_bundle_async(b))
    }

    /// Records one install of `aid` by the device whose opaque hash is
    /// `install_sha256`; the same device again counts once.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn record_install(&self, aid: &ArtifactId, install_sha256: &str) -> Result<()> {
        self.with_tx(|tx| {
            super::artifacts::live_artifact_in(tx, aid.as_str())?.ok_or(CoreError::NotFound)?;
            tx.execute(
                "INSERT INTO app_installs (artifact_id, install_sha256, created_at)
                 VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                params![aid.as_str(), install_sha256, self.now()],
            )?;
            Ok(())
        })
    }

    /// How many distinct devices installed `aid`.
    pub fn install_count(&self, aid: &ArtifactId) -> Result<u64> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM app_installs WHERE artifact_id = ?1",
                params![aid.as_str()],
                |r| r.get::<_, i64>(0),
            )? as u64)
        })
    }

    /// How many live (not deleted) artifacts are forks of `aid`.
    pub fn fork_count(&self, aid: &ArtifactId) -> Result<u64> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM artifacts WHERE forked_from_id = ?1 AND deleted_at IS NULL",
                params![aid.as_str()],
                |r| r.get::<_, i64>(0),
            )? as u64)
        })
    }

    /// How an app's page names `person` (a public ID; `None` for the
    /// owner): their display name, else their GitHub login, else
    /// "Someone"; their GitHub login when one is linked and not blocked;
    /// and their public ID (the owner's own, when the owner has a viewer).
    /// A public ID no viewer has is "Someone" with no login, keeping that
    /// ID.
    pub fn person_card(&self, person: Option<&str>) -> Result<PersonCard> {
        self.with_read(|c| {
            let row: Option<(Option<String>, Option<String>, Option<String>)> = match person {
                Some(pid) => c
                    .query_row(
                        "SELECT v.display_name, gi.login, v.public_id FROM viewers v
                           LEFT JOIN github_identities gi
                             ON gi.viewer_id = v.id AND gi.blocked_at IS NULL
                          WHERE v.public_id = ?1",
                        params![pid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?,
                None => c
                    .query_row(
                        "SELECT v.display_name, gi.login, v.public_id FROM viewers v
                           LEFT JOIN github_identities gi
                             ON gi.viewer_id = v.id AND gi.blocked_at IS NULL
                          WHERE v.owner = 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?,
            };
            let (name, login, public_id) = row.unwrap_or((None, None, None));
            let public_id = public_id.or_else(|| person.map(str::to_string));
            let name = name
                .filter(|n| !n.trim().is_empty())
                .or_else(|| login.clone())
                .unwrap_or_else(|| "Someone".to_string());
            Ok(PersonCard {
                name,
                login,
                public_id,
            })
        })
    }

    /// The live artifacts carrying at least one bundle that `person` (a
    /// public ID; `None` for the owner) made, then those shared with them
    /// by a share naming them (not by link), each once, its title order:
    /// what their apps library lists. An artifact they made is listed as
    /// made, whatever its shares.
    pub fn apps_of(&self, person: Option<&str>) -> Result<Vec<AppListing>> {
        self.with_read(|c| {
            let mut out: Vec<AppListing> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let latest =
                "(SELECT MAX(b.bundle_version) FROM app_bundles b WHERE b.artifact_id = a.id)";
            let mut made = c.prepare(&format!(
                "SELECT a.id, a.title, a.current_version, {latest} FROM artifacts a
                  WHERE a.person IS ?1 AND a.deleted_at IS NULL
                    AND EXISTS (SELECT 1 FROM app_bundles b WHERE b.artifact_id = a.id)
                  ORDER BY lower(a.title), a.id"
            ))?;
            let rows = made.query_map(params![person], |r| {
                Ok(AppListing {
                    artifact_id: r.get(0)?,
                    title: r.get(1)?,
                    version: r.get(2)?,
                    bundle_version: r.get::<_, i64>(3)? as u64,
                    share: None,
                })
            })?;
            for l in rows {
                let l = l?;
                seen.insert(l.artifact_id.clone());
                out.push(l);
            }
            let Some(pid) = person else {
                return Ok(out);
            };
            let mut shared = c.prepare(&format!(
                "SELECT a.id, a.title, a.current_version, {latest}, s.access
                   FROM artifact_shares s
                   JOIN viewers v ON v.id = s.viewer_id
                   JOIN artifacts a ON a.id = s.artifact_id
                  WHERE v.public_id = ?1 AND a.deleted_at IS NULL
                    AND EXISTS (SELECT 1 FROM app_bundles b WHERE b.artifact_id = a.id)
                  ORDER BY lower(a.title), a.id"
            ))?;
            let rows = shared.query_map(params![pid], |r| {
                let access: String = r.get(4)?;
                Ok(AppListing {
                    artifact_id: r.get(0)?,
                    title: r.get(1)?,
                    version: r.get(2)?,
                    bundle_version: r.get::<_, i64>(3)? as u64,
                    share: ShareAccess::parse(&access),
                })
            })?;
            for l in rows {
                let l = l?;
                if seen.insert(l.artifact_id.clone()) {
                    out.push(l);
                }
            }
            Ok(out)
        })
    }
}

/// How an app's page names a person ([`Store::person_card`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PersonCard {
    pub name: String,
    pub login: Option<String>,
    /// The person's public ID: what tells them apart from another person
    /// with the same name; `None` for an owner with no viewer.
    pub public_id: Option<String>,
}

/// One app of a person's apps library ([`Store::apps_of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppListing {
    pub artifact_id: String,
    pub title: String,
    /// The artifact's current version.
    pub version: u32,
    /// The highest release counter of its bundles.
    pub bundle_version: u64,
    /// The share naming the person; `None` for an artifact they made.
    pub share: Option<ShareAccess>,
}

#[cfg(test)]
mod tests {
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::CoreError;
    use clax_core::publish::MAX_BODY_BYTES;
    use rusqlite::params;

    #[test]
    fn a_bundle_counts_against_its_versions_size_limit() {
        let (_d, st) = store();
        let a = artifact(&st, None);
        let v = st.get_version(&a, 1).unwrap().unwrap();
        let c = v.content_sha256.unwrap();
        // The version's files claim all but five bytes of the limit.
        let mut files = v.files;
        files.get_mut("index.html").unwrap().size = MAX_BODY_BYTES - 5;
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE versions SET files_json = ?2 WHERE artifact_id = ?1 AND n = 1",
                params![a.as_str(), serde_json::to_string(&files).unwrap()],
            )?;
            Ok(())
        })
        .unwrap();
        let err = st
            .put_app_bundle(DAEMON, &a, 1, "lin", 1, &c, b"sixsix", None)
            .unwrap_err();
        assert!(
            matches!(&err, CoreError::Invalid { code, .. } if *code == "bundle_too_large"),
            "{err:?}"
        );
        assert!(st.app_bundle(&a, 1).unwrap().is_none());
        let b = st
            .put_app_bundle(DAEMON, &a, 1, "lin", 1, &c, b"five5", None)
            .unwrap();
        assert_eq!(b.size, 5);
    }
}
