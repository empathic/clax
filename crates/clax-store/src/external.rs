//! External artifacts (spec 2026-10-27-clax-sdk-design §6): artifacts of a
//! deployer's app, named by the app's own opaque `external_id`, whose
//! content Clax never stores. Each is an `html` artifact whose one version
//! is a page linking to the record's URL
//! ([`clax_core::queue::external_placeholder_html`]), with an
//! `external_artifacts` row holding the ID, the URL, the app's metadata and
//! revision. The same `external_id` from any app, queue or person is the
//! same artifact (D17). Its reads serve `kind: "external"`.

use super::Store;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::publish::{Encoding, FileInput, INDEX, PublishRequest};
use clax_core::queue::{
    MAX_ARTIFACT_METADATA_BYTES, check_external_id, check_revision, external_placeholder_html,
    metadata_json, metadata_sha256,
};
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::BTreeMap;

/// An external artifact's own fields.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalArtifact {
    pub artifact_id: String,
    pub external_id: String,
    pub title: String,
    pub url: String,
    /// The app's metadata; `Null` when it wrote none.
    pub metadata: serde_json::Value,
    pub metadata_version: u64,
    pub revision: Option<String>,
}

impl ExternalArtifact {
    /// As a report carries it.
    pub fn view(&self) -> clax_core::queue::ReportArtifact {
        clax_core::queue::ReportArtifact {
            id: self.artifact_id.clone(),
            external_id: self.external_id.clone(),
            title: self.title.clone(),
            url: self.url.clone(),
            metadata: self.metadata.clone(),
            metadata_version: self.metadata_version,
            revision: self.revision.clone(),
        }
    }
}

/// An external artifact to find or make ([`Store::ensure_external_async`]).
#[derive(Clone, Debug)]
pub struct NewExternal {
    /// Whose external IDs `external_id` is among: `""` for the whole
    /// instance, or one account's public ID on a hosted Clax, where each
    /// account is its own deployer (spec §6.1).
    pub namespace: String,
    pub external_id: String,
    pub title: String,
    /// The record's page in the app (`http` or `https`).
    pub url: String,
    pub revision: Option<String>,
}

/// The columns [`row_to_external`] reads; callers append `WHERE ...`.
pub(crate) const EXTERNAL_SELECT: &str =
    "SELECT x.artifact_id, x.external_id, a.title, x.url, x.metadata_json, x.metadata_version,
        x.revision FROM external_artifacts x JOIN artifacts a ON a.id = x.artifact_id";

pub(crate) fn row_to_external(r: &Row<'_>) -> rusqlite::Result<ExternalArtifact> {
    let meta: Option<String> = r.get(4)?;
    Ok(ExternalArtifact {
        artifact_id: r.get(0)?,
        external_id: r.get(1)?,
        title: r.get(2)?,
        url: r.get(3)?,
        metadata: meta
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or(serde_json::Value::Null),
        metadata_version: r.get(5)?,
        revision: r.get(6)?,
    })
}

/// The live external artifact named `external_id` in `namespace`, on `c`.
pub(crate) fn by_external_id_in(
    c: &Connection,
    namespace: &str,
    external_id: &str,
) -> Result<Option<ExternalArtifact>> {
    Ok(c.prepare_cached(&format!(
        "{EXTERNAL_SELECT} WHERE x.namespace = ?1 AND x.external_id = ?2 AND a.deleted_at IS NULL"
    ))?
    .query_row(params![namespace, external_id], row_to_external)
    .optional()?)
}

/// The live external artifact `aid`, on `c`.
pub(crate) fn by_artifact_in(c: &Connection, aid: &str) -> Result<Option<ExternalArtifact>> {
    Ok(c.prepare_cached(&format!(
        "{EXTERNAL_SELECT} WHERE x.artifact_id = ?1 AND a.deleted_at IS NULL"
    ))?
    .query_row(params![aid], row_to_external)
    .optional()?)
}

/// The SHA-256 the journal records of stored metadata `json` (`None`:
/// none written, recorded as the hash of `null`).
fn sha_of(json: Option<&str>) -> String {
    metadata_sha256(json.unwrap_or("null"))
}

/// The `external.create` record of `x` (spec §6.6).
fn create_record(at: &str, x: &ExternalArtifact) -> AuditRecord {
    let json = serde_json::to_string(&x.metadata).expect("JSON values serialise");
    let mut r = AuditRecord::new(AuditKind::ExternalCreate, at)
        .with("external_id", x.external_id.as_str())
        .with("title", x.title.as_str())
        .with("url", x.url.as_str())
        .with("revision", x.revision.clone())
        .with("metadata", x.metadata.clone())
        .with("metadata_sha256", metadata_sha256(&json));
    r.ids.artifact = Some(x.artifact_id.clone());
    r
}

impl Store {
    /// The live external artifact named `external_id` in `namespace`
    /// ([`NewExternal::namespace`]), when there is one.
    ///
    /// # Errors
    /// Database errors.
    pub fn external_artifact(
        &self,
        namespace: &str,
        external_id: &str,
    ) -> Result<Option<ExternalArtifact>> {
        self.with_read(|c| by_external_id_in(c, namespace, external_id))
    }

    /// The external fields of artifact `aid`, when it is a live external
    /// artifact.
    ///
    /// # Errors
    /// Database errors.
    pub fn external_of(&self, aid: &ArtifactId) -> Result<Option<ExternalArtifact>> {
        self.with_read(|c| by_artifact_in(c, aid.as_str()))
    }

    /// The external artifact `x.external_id` names in `x.namespace`, made when there is
    /// none: an artifact of `person` (`None`: the owner) titled `x.title`,
    /// whose version 1 links to `x.url`, and its row, written with version
    /// 1 and recorded as `external.create` under `ctx` beside the
    /// artifact's own `artifact.create` and `version.publish`. A found
    /// artifact keeps its title, URL and revision: `revision` is written
    /// only when this call makes the artifact. Two callers making one ID at
    /// once settle on one artifact. Answers the artifact and whether this
    /// call made it.
    ///
    /// # Errors
    /// `invalid_external_id`, `invalid_revision`, `invalid_url` or
    /// `unsupported_url` for bad input; the quota's errors.
    pub async fn ensure_external_async(
        &self,
        ctx: &AuditCtx,
        person: Option<&str>,
        x: NewExternal,
    ) -> Result<(ExternalArtifact, bool)> {
        check_external_id(&x.external_id)?;
        clax_core::live::parse_page_url(&x.url)?;
        if let Some(r) = &x.revision {
            check_revision(r)?;
        }
        let title = match x.title.trim() {
            "" => x.external_id.clone(),
            t => t.to_string(),
        };
        // A concurrent maker's row wins the unique ID: this one's artifact
        // is then removed, and the found one answered.
        for _ in 0..2 {
            if let Some(found) = self.external_artifact(&x.namespace, &x.external_id)? {
                return Ok((found, false));
            }
            let html = external_placeholder_html(&title, &x.url);
            let mut p = clax_core::publish::validate(PublishRequest {
                title: Some(title.clone()),
                note: Some("external".to_string()),
                files: BTreeMap::from([(
                    INDEX.to_string(),
                    Some(FileInput {
                        content: html,
                        encoding: Encoding::Utf8,
                        content_type: None,
                    }),
                )]),
                ..Default::default()
            })?;
            p.person = person.map(str::to_string);
            let id = ArtifactId::generate();
            let made = self
                .create_artifact_then_async(ctx, &id, p, None, |tx, _| {
                    tx.execute(
                        "INSERT INTO external_artifacts (artifact_id, namespace, external_id, url,
                            metadata_json, metadata_version, revision) VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5)",
                        params![id.as_str(), x.namespace, x.external_id, x.url, x.revision],
                    )?;
                    let made = by_artifact_in(tx, id.as_str())?.ok_or(CoreError::NotFound)?;
                    self.record_audit(tx, ctx, create_record(&self.now(), &made))?;
                    Ok(made)
                })
                .await;
            match made {
                Ok((_, _, made)) => return Ok((made, true)),
                Err(CoreError::Db(rusqlite::Error::SqliteFailure(e, _)))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation => {}
                Err(e) => return Err(e),
            }
        }
        self.external_artifact(&x.namespace, &x.external_id)?
            .map(|f| (f, false))
            .ok_or(CoreError::NotFound)
    }

    /// Writes `value` as external artifact `aid`'s metadata when its
    /// version is `if_version`, and answers the version it has after: one
    /// more, or the same when `value` equals what it holds (then nothing is
    /// written or recorded). Recorded as `external.update {metadata,
    /// metadata_sha256, previous_sha256}` under `ctx` (spec §6.6).
    ///
    /// # Errors
    /// `NotFound` when `aid` is no live external artifact;
    /// `metadata_too_large` over [`MAX_ARTIFACT_METADATA_BYTES`];
    /// [`CoreError::DocConflict`] (path `metadata`) when `if_version` is
    /// not its version.
    pub fn set_artifact_metadata(
        &self,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        value: serde_json::Value,
        if_version: u64,
    ) -> Result<u64> {
        let json = metadata_json(&value, MAX_ARTIFACT_METADATA_BYTES)?;
        self.with_tx(|tx| self.set_artifact_metadata_in(tx, ctx, aid, &json, value, if_version))
    }

    /// [`Store::set_artifact_metadata`] inside the caller's transaction,
    /// `json` being `value` as [`metadata_json`] stores it.
    pub(crate) fn set_artifact_metadata_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        json: &str,
        value: serde_json::Value,
        if_version: u64,
    ) -> Result<u64> {
        {
            let row: Option<(Option<String>, u64)> = tx
                .query_row(
                    "SELECT x.metadata_json, x.metadata_version FROM external_artifacts x
                     JOIN artifacts a ON a.id = x.artifact_id
                     WHERE x.artifact_id = ?1 AND a.deleted_at IS NULL",
                    params![aid.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((before, version)) = row else {
                return Err(CoreError::NotFound);
            };
            if version != if_version {
                return Err(CoreError::DocConflict {
                    path: "metadata".into(),
                    current: Some(version),
                });
            }
            if before.as_deref().unwrap_or("null") == json {
                return Ok(version);
            }
            tx.execute(
                "UPDATE external_artifacts SET metadata_json = ?2, metadata_version = ?3
                 WHERE artifact_id = ?1",
                params![aid.as_str(), json, version + 1],
            )?;
            let mut r = AuditRecord::new(AuditKind::ExternalUpdate, self.now())
                .with("metadata", value)
                .with("metadata_sha256", metadata_sha256(json))
                .with("previous_sha256", sha_of(before.as_deref()));
            r.ids.artifact = Some(aid.as_str().to_string());
            self.record_audit(tx, ctx, r)?;
            Ok(version + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NewExternal;
    use crate::test_util::{DAEMON, store};
    use clax_core::ArtifactId;
    use serde_json::json;

    fn new(id: &str) -> NewExternal {
        NewExternal {
            namespace: String::new(),
            external_id: id.into(),
            title: "Invoice 4821".into(),
            url: "https://app.example.com/invoices/4821".into(),
            revision: Some("r1".into()),
        }
    }

    #[test]
    fn one_external_id_is_one_artifact_served_as_external() {
        let (_d, st) = store();
        let (a, made) = st
            .block_on(st.ensure_external_async(DAEMON, None, new("invoice:4821")))
            .unwrap();
        assert!(made);
        let (b, made) = st
            .block_on(st.ensure_external_async(DAEMON, Some("u_x"), new("invoice:4821")))
            .unwrap();
        assert!(!made);
        assert_eq!(a.artifact_id, b.artifact_id);
        // A later caller's revision does not overwrite the maker's.
        let (c, _) = st
            .block_on(st.ensure_external_async(
                DAEMON,
                None,
                NewExternal {
                    revision: Some("r2".into()),
                    ..new("invoice:4821")
                },
            ))
            .unwrap();
        assert_eq!(c.revision.as_deref(), Some("r1"));
        assert_eq!(
            st.external_artifact("", "invoice:4821")
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some("r1")
        );
        let art = st
            .get_artifact(&ArtifactId::parse(&a.artifact_id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(art.kind, clax_core::live::KIND_EXTERNAL);
        assert_eq!(art.current_version, 1);
        assert!(
            st.external_artifact("", "invoice:4822").unwrap().is_none(),
            "IDs compare byte for byte"
        );
    }

    #[test]
    fn one_external_id_in_two_namespaces_is_two_artifacts() {
        let (_d, st) = store();
        let in_ns = |ns: &str| NewExternal {
            namespace: ns.into(),
            ..new("invoice:4821")
        };
        let (a, _) = st
            .block_on(st.ensure_external_async(DAEMON, Some("u_a"), in_ns("u_a")))
            .unwrap();
        let (b, made) = st
            .block_on(st.ensure_external_async(DAEMON, Some("u_b"), in_ns("u_b")))
            .unwrap();
        assert!(made);
        assert_ne!(a.artifact_id, b.artifact_id);
        let aid = ArtifactId::parse(&a.artifact_id).unwrap();
        st.set_artifact_metadata(DAEMON, &aid, json!({"plan": "team"}), 1)
            .unwrap();
        let theirs = st
            .external_artifact("u_b", "invoice:4821")
            .unwrap()
            .unwrap();
        assert_eq!(theirs.metadata, serde_json::Value::Null);
        assert!(st.external_artifact("", "invoice:4821").unwrap().is_none());
    }

    #[test]
    fn a_deleted_external_artifact_frees_its_id() {
        let (_d, st) = store();
        let (a, _) = st
            .block_on(st.ensure_external_async(DAEMON, None, new("x")))
            .unwrap();
        let aid = ArtifactId::parse(&a.artifact_id).unwrap();
        st.set_artifact_metadata(DAEMON, &aid, json!({"plan": "team"}), 1)
            .unwrap();
        st.delete_artifact(DAEMON, &aid).unwrap();
        let rows: Vec<(String, Option<String>)> = st
            .with_read(|c| {
                Ok(c.prepare(
                    "SELECT body, artifact_id FROM audit_events WHERE kind = 'external.delete'",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        let body: serde_json::Value = serde_json::from_str(&rows[0].0).unwrap();
        assert_eq!(body["external_id"], "x");
        assert_eq!(
            body["metadata_sha256"],
            json!(clax_core::queue::metadata_sha256("{\"plan\":\"team\"}"))
        );
        assert_eq!(rows[0].1.as_deref(), Some(a.artifact_id.as_str()));
        let (b, made) = st
            .block_on(st.ensure_external_async(DAEMON, None, new("x")))
            .unwrap();
        assert!(made);
        assert_ne!(a.artifact_id, b.artifact_id);
    }

    #[test]
    fn artifact_metadata_takes_its_version() {
        let (_d, st) = store();
        let (a, _) = st
            .block_on(st.ensure_external_async(DAEMON, None, new("x")))
            .unwrap();
        let aid = ArtifactId::parse(&a.artifact_id).unwrap();
        assert_eq!(a.metadata_version, 1);
        let v = st
            .set_artifact_metadata(DAEMON, &aid, json!({"plan": "team"}), 1)
            .unwrap();
        assert_eq!(v, 2);
        let err = st
            .set_artifact_metadata(DAEMON, &aid, json!({"plan": "free"}), 1)
            .unwrap_err();
        assert!(
            matches!(
                err,
                clax_core::CoreError::DocConflict {
                    current: Some(2),
                    ..
                }
            ),
            "{err:?}"
        );
        // The same value again changes nothing.
        assert_eq!(
            st.set_artifact_metadata(DAEMON, &aid, json!({"plan": "team"}), 2)
                .unwrap(),
            2
        );
        let big = json!("x".repeat(clax_core::queue::MAX_ARTIFACT_METADATA_BYTES));
        assert!(st.set_artifact_metadata(DAEMON, &aid, big, 2).is_err());
        assert_eq!(
            st.external_of(&aid).unwrap().unwrap().metadata,
            json!({"plan": "team"})
        );
    }
}
