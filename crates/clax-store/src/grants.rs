//! Cross-app read consent (spec 2026-10-12-flutter-app-design §3.8): the
//! owner's grants (`db_read_grants`, migration 41) and the check that joins
//! them with both artifacts' current declarations.
//!
//! A grant row says only that the owner let `reader_id` read `source_id`.
//! It never widens what a read may see: the reader still has to declare the
//! read, and the routes judge it at `view` with no viewer identity. Deleting
//! either artifact removes its rows (a trigger on the soft delete, and the
//! foreign keys on a row delete), and a publish or metadata change that drops
//! a source from the reader's `db.read` removes that pair's row, so declaring
//! the read again asks the owner again.

use super::Store;
use super::docs::caps_in;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::db::ReadDecl;
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// One owner grant: the reader may read `source_id`'s documents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Grant {
    pub source_id: String,
    pub granted_at: String,
}

/// Whether a reader may read a source's documents, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadAccess {
    /// The reader declares the read, and the source shares with it or the
    /// owner granted it.
    Granted,
    /// The reader's current declaration does not list the source in
    /// `db.read` (or is not a valid declaration).
    NotDeclared,
    /// The reader declares the read, but the source does not share with it
    /// and no grant exists.
    NotGranted,
}

/// The `artifact.update` record of a grant change on `reader`: the field
/// `db_read_grants` holds `{"grant": source}` or `{"revoke": source}`.
fn grant_record(now: String, reader: &ArtifactId, source: &ArtifactId, op: &str) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, now).with(
        "fields",
        serde_json::json!({"db_read_grants": {op: source.as_str()}}),
    );
    rec.ids.artifact = Some(reader.as_str().to_string());
    rec.ids.artifact2 = Some(source.as_str().to_string());
    rec
}

/// `invalid_args` when `reader` and `source` are one artifact; `NotFound`
/// when either is missing, deleted or has no version.
fn check_pair(c: &Connection, reader: &ArtifactId, source: &ArtifactId) -> Result<()> {
    if reader == source {
        return Err(CoreError::invalid(
            "invalid_args",
            "an artifact reads its own documents without a grant",
        ));
    }
    caps_in(c, reader)?;
    caps_in(c, source)?;
    Ok(())
}

impl Store {
    /// Inside a write that sets `reader`'s declaration to `caps`, removes
    /// the owner's grants to `reader` for every source `caps` does not list
    /// in `db.read` (all of them when `caps` does not parse). Each removal is
    /// recorded as a revoke under `audit`, as [`Store::revoke_read`] does.
    pub(super) fn drop_undeclared_grants(
        &self,
        tx: &rusqlite::Transaction<'_>,
        audit: &AuditCtx,
        reader: &ArtifactId,
        caps: &serde_json::Value,
    ) -> Result<()> {
        let reads = ReadDecl::from_capabilities(caps, reader.as_str())
            .map(|d| d.reads)
            .unwrap_or_default();
        let sources: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT source_id FROM db_read_grants WHERE reader_id = ?1")?;
            let rows = stmt.query_map(params![reader.as_str()], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for source in sources.iter().filter(|s| !reads.contains(s)) {
            tx.execute(
                "DELETE FROM db_read_grants WHERE reader_id = ?1 AND source_id = ?2",
                params![reader.as_str(), source],
            )?;
            let source = ArtifactId::parse(source)?;
            self.record_audit(
                tx,
                audit,
                grant_record(self.now(), reader, &source, "revoke"),
            )?;
        }
        Ok(())
    }

    /// Lets `reader` read `source`'s documents, once it declares the read.
    /// Granting again changes nothing. A new grant is recorded as
    /// `artifact.update` of `reader` under `audit`.
    ///
    /// # Errors
    /// `invalid_args` when `reader == source`; `NotFound` when either is
    /// missing or deleted.
    pub fn grant_read(
        &self,
        audit: &AuditCtx,
        reader: &ArtifactId,
        source: &ArtifactId,
    ) -> Result<()> {
        self.with_tx(|tx| {
            check_pair(tx, reader, source)?;
            let n = tx.execute(
                "INSERT OR IGNORE INTO db_read_grants (reader_id, source_id, granted_at) VALUES (?1, ?2, ?3)",
                params![reader.as_str(), source.as_str(), self.now()],
            )?;
            if n > 0 {
                self.record_audit(tx, audit, grant_record(self.now(), reader, source, "grant"))?;
            }
            Ok(())
        })
    }

    /// Removes the owner's grant letting `reader` read `source`; whether
    /// there was one. A share declared by `source` is not a grant and stays.
    /// A removal is recorded as `artifact.update` of `reader` under `audit`.
    ///
    /// # Errors
    /// `invalid_args` when `reader == source`; `NotFound` when either is
    /// missing or deleted.
    pub fn revoke_read(
        &self,
        audit: &AuditCtx,
        reader: &ArtifactId,
        source: &ArtifactId,
    ) -> Result<bool> {
        self.with_tx(|tx| {
            check_pair(tx, reader, source)?;
            let n = tx.execute(
                "DELETE FROM db_read_grants WHERE reader_id = ?1 AND source_id = ?2",
                params![reader.as_str(), source.as_str()],
            )?;
            if n > 0 {
                self.record_audit(
                    tx,
                    audit,
                    grant_record(self.now(), reader, source, "revoke"),
                )?;
            }
            Ok(n > 0)
        })
    }

    /// The owner's grants to `reader`, oldest first.
    ///
    /// # Errors
    /// `NotFound` when `reader` is missing or deleted.
    pub fn grants_of(&self, reader: &ArtifactId) -> Result<Vec<Grant>> {
        self.with_read(|c| {
            caps_in(c, reader)?;
            let mut stmt = c.prepare(
                "SELECT source_id, granted_at FROM db_read_grants WHERE reader_id = ?1
                 ORDER BY granted_at, source_id",
            )?;
            let rows = stmt.query_map(params![reader.as_str()], |r| {
                Ok(Grant {
                    source_id: r.get(0)?,
                    granted_at: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Whether `reader` may read `source`'s documents now: `Granted` iff the
    /// reader's current declaration lists `source` in `db.read` and either
    /// the source's current declaration lists `reader` in `db.share.read` or
    /// the owner granted it ([`Store::grant_read`]). A declaration that does
    /// not parse grants nothing: an invalid reader declaration is
    /// `NotDeclared`, an invalid source declaration shares with no one.
    /// `reader == source` is `NotDeclared`, since a declaration cannot name
    /// its own artifact.
    ///
    /// A draft (spec 2026-10-12-flutter-app-round2-design §5.1) is
    /// `NotGranted` for every other source, whatever grants or shares name
    /// it or its app: what a render reads reaches the agent, and an app's
    /// grant is not the owner's consent to that. A draft reads its own
    /// copied documents without a reader.
    ///
    /// # Errors
    /// `NotFound` when either artifact is missing or deleted.
    pub fn may_read(&self, reader: &ArtifactId, source: &ArtifactId) -> Result<ReadAccess> {
        self.with_read(|c| {
            let reader_caps = caps_in(c, reader)?;
            let source_caps = caps_in(c, source)?;
            if reader != source && super::drafts::is_hidden_in(c, reader.as_str())? {
                return Ok(ReadAccess::NotGranted);
            }
            let declared = ReadDecl::from_capabilities(&reader_caps, reader.as_str())
                .is_ok_and(|d| d.reads.iter().any(|s| s == source.as_str()));
            if !declared {
                return Ok(ReadAccess::NotDeclared);
            }
            let shared = ReadDecl::from_capabilities(&source_caps, source.as_str())
                .is_ok_and(|d| d.shares_read.iter().any(|r| r == reader.as_str()));
            let granted = shared
                || c.query_row(
                    "SELECT 1 FROM db_read_grants WHERE reader_id = ?1 AND source_id = ?2",
                    params![reader.as_str(), source.as_str()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            Ok(if granted {
                ReadAccess::Granted
            } else {
                ReadAccess::NotGranted
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetaPatch;
    use crate::docs::Pin;
    use crate::test_util::{DAEMON, artifact_with_caps, store};
    use clax_core::db::{Caller, Level};
    use serde_json::{Value, json};

    fn set_caps(st: &Store, id: &ArtifactId, caps: Value) {
        st.update_meta(
            DAEMON,
            id,
            MetaPatch {
                capabilities: Some(caps),
                ..MetaPatch::default()
            },
        )
        .unwrap();
    }

    fn grant_rows(st: &Store) -> i64 {
        st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM db_read_grants", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn may_read_needs_the_declaration_and_a_share_or_a_grant() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({"db": {}}));
        let b = artifact_with_caps(&st, json!({"db": {}}));
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotDeclared);

        set_caps(&st, &a, json!({"db": {"read": [b.as_str()]}}));
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotGranted);

        set_caps(&st, &b, json!({"db": {"share": {"read": [a.as_str()]}}}));
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::Granted);
        assert_eq!(
            st.may_read(&b, &a).unwrap(),
            ReadAccess::NotDeclared,
            "a share is one way"
        );

        set_caps(&st, &b, json!({"db": {}}));
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotGranted);

        st.grant_read(DAEMON, &a, &b).unwrap();
        st.grant_read(DAEMON, &a, &b).unwrap();
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::Granted);
        assert_eq!(grant_rows(&st), 1, "granting again changes nothing");
        assert_eq!(
            st.may_read(&b, &a).unwrap(),
            ReadAccess::NotDeclared,
            "a grant is one way"
        );

        assert!(st.revoke_read(DAEMON, &a, &b).unwrap());
        assert!(!st.revoke_read(DAEMON, &a, &b).unwrap());
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotGranted);

        st.grant_read(DAEMON, &a, &b).unwrap();
        set_caps(&st, &a, json!({"db": {}}));
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotDeclared);
        assert_eq!(grant_rows(&st), 0, "dropping the read drops its grant");
        set_caps(&st, &a, json!({"db": {"read": [b.as_str()]}}));
        assert_eq!(
            st.may_read(&a, &b).unwrap(),
            ReadAccess::NotGranted,
            "declaring the read again asks the owner again"
        );
    }

    #[test]
    fn a_publish_that_drops_a_source_drops_only_that_grant() {
        let (_d, st) = store();
        let b = artifact_with_caps(&st, json!({"db": {}}));
        let c = artifact_with_caps(&st, json!({"db": {}}));
        let a = artifact_with_caps(&st, json!({"db": {"read": [b.as_str(), c.as_str()]}}));
        st.grant_read(DAEMON, &a, &b).unwrap();
        st.grant_read(DAEMON, &a, &c).unwrap();
        st.grant_read(DAEMON, &c, &b).unwrap();
        let publish = |caps: Option<Value>| {
            let cur = st.get_artifact(&a).unwrap().unwrap().current_version;
            let mut req = json!({
                "if_version": cur,
                "files": {"index.html": {"content": "<main>2</main>", "encoding": "utf8"}}
            });
            if let Some(caps) = caps {
                req["capabilities"] = caps;
            }
            let req: clax_core::publish::PublishRequest = serde_json::from_value(req).unwrap();
            st.publish_version(DAEMON, &a, clax_core::publish::validate(req).unwrap(), None)
                .unwrap();
        };
        publish(None);
        assert_eq!(
            grant_rows(&st),
            3,
            "a publish keeping the declaration keeps the grants"
        );

        publish(Some(json!({"db": {"read": [c.as_str()]}})));
        let left: Vec<String> = st
            .grants_of(&a)
            .unwrap()
            .into_iter()
            .map(|g| g.source_id)
            .collect();
        assert_eq!(left, vec![c.to_string()]);
        assert_eq!(
            st.grants_of(&c).unwrap().len(),
            1,
            "another reader's grant stays"
        );
        assert_eq!(st.may_read(&a, &c).unwrap(), ReadAccess::Granted);

        let revokes: i64 = st
            .with_read(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM audit_events WHERE kind = 'artifact.update'
                     AND artifact_id = ?1 AND artifact2_id = ?2
                     AND json_extract(body, '$.fields.db_read_grants.revoke') = ?2",
                    params![a.as_str(), b.as_str()],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(revokes, 1, "the dropped grant is audited as a revoke");
    }

    #[test]
    fn may_read_fails_closed_on_declarations_that_do_not_parse() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({}));
        let b = artifact_with_caps(&st, json!({}));
        // Declarations written before validation knew `db.read`, straight to
        // the column, so the publish check cannot refuse them.
        let raw = |id: &ArtifactId, caps: Value| {
            st.with_tx(|tx| {
                tx.execute(
                    "UPDATE artifacts SET capabilities_json = ?2 WHERE id = ?1",
                    params![id.as_str(), caps.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        };
        raw(&a, json!({"db": {"read": [b.as_str(), a.as_str()]}}));
        st.grant_read(DAEMON, &a, &b).unwrap();
        assert_eq!(st.may_read(&a, &b).unwrap(), ReadAccess::NotDeclared);

        raw(&a, json!({"db": {"read": [b.as_str()]}}));
        raw(&b, json!({"db": {"share": {"read": [a.as_str(), 7]}}}));
        st.revoke_read(DAEMON, &a, &b).unwrap();
        assert_eq!(
            st.may_read(&a, &b).unwrap(),
            ReadAccess::NotGranted,
            "an invalid share shares with no one"
        );
    }

    #[test]
    fn grants_refuse_self_and_unknown_or_deleted_artifacts() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({}));
        let b = artifact_with_caps(&st, json!({}));
        let ghost = ArtifactId::parse("zzzzzzzzzzzz").unwrap();
        assert!(matches!(
            st.grant_read(DAEMON, &a, &a),
            Err(CoreError::Invalid {
                code: "invalid_args",
                ..
            })
        ));
        assert_eq!(st.may_read(&a, &a).unwrap(), ReadAccess::NotDeclared);
        for (r, s) in [(&a, &ghost), (&ghost, &a)] {
            assert!(matches!(
                st.grant_read(DAEMON, r, s),
                Err(CoreError::NotFound)
            ));
            assert!(matches!(
                st.revoke_read(DAEMON, r, s),
                Err(CoreError::NotFound)
            ));
            assert!(matches!(st.may_read(r, s), Err(CoreError::NotFound)));
        }
        assert!(matches!(st.grants_of(&ghost), Err(CoreError::NotFound)));
        st.delete_artifact(DAEMON, &b).unwrap();
        assert!(matches!(
            st.grant_read(DAEMON, &a, &b),
            Err(CoreError::NotFound)
        ));
        assert_eq!(grant_rows(&st), 0);
    }

    #[test]
    fn grants_go_with_a_deleted_artifact() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({}));
        let b = artifact_with_caps(&st, json!({}));
        let c = artifact_with_caps(&st, json!({}));
        st.grant_read(DAEMON, &a, &b).unwrap();
        st.grant_read(DAEMON, &a, &c).unwrap();
        st.grant_read(DAEMON, &c, &a).unwrap();
        let sources: Vec<String> = st
            .grants_of(&a)
            .unwrap()
            .into_iter()
            .map(|g| g.source_id)
            .collect();
        assert_eq!(sources.len(), 2);
        assert!(sources.contains(&b.to_string()) && sources.contains(&c.to_string()));

        st.delete_artifact(DAEMON, &b).unwrap();
        let left = st.grants_of(&a).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].source_id, c.as_str());

        st.delete_artifact(DAEMON, &a).unwrap();
        assert!(
            st.grants_of(&c).unwrap().is_empty(),
            "the reader's deletion too"
        );
        assert_eq!(grant_rows(&st), 0);
    }

    #[test]
    fn grant_changes_are_audited_on_the_reader() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({}));
        let b = artifact_with_caps(&st, json!({}));
        let audited = || -> Vec<(String, String, String)> {
            st.with_read(|c| {
                let mut s = c.prepare(
                    "SELECT artifact_id, artifact2_id, body FROM audit_events
                     WHERE kind = 'artifact.update' ORDER BY seq",
                )?;
                let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap()
        };
        st.grant_read(DAEMON, &a, &b).unwrap();
        st.grant_read(DAEMON, &a, &b).unwrap();
        st.revoke_read(DAEMON, &a, &b).unwrap();
        st.revoke_read(DAEMON, &a, &b).unwrap();
        let rows = audited();
        assert_eq!(rows.len(), 2, "only changes are recorded: {rows:?}");
        for ((aid, aid2, body), op) in rows.iter().zip(["grant", "revoke"]) {
            assert_eq!((aid.as_str(), aid2.as_str()), (a.as_str(), b.as_str()));
            let body: Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["fields"]["db_read_grants"][op], b.as_str(), "{body}");
        }
    }

    #[test]
    fn collections_count_documents_and_skip_private() {
        let (_d, st) = store();
        let id = artifact_with_caps(
            &st,
            json!({"db": {"rules": [{"path": "votes/{self}", "read": "view", "write": "interact"}]}}),
        );
        let owner = Caller::new(Level::Owner, None);
        let me = "u_00000000000000000000aa";
        let viewer = Caller::new(Level::Interact, Some(me.into()));
        let lww = Pin {
            if_version: None,
            lww: true,
        };
        for p in ["runs/r1", "runs/r2", "notes/n1"] {
            st.doc_set(DAEMON, &id, p, json!({}), lww, &owner).unwrap();
        }
        for p in [format!("data/users/{me}/x"), format!("votes/{me}/v/1")] {
            st.doc_set(DAEMON, &id, &p, json!({}), lww, &viewer)
                .unwrap();
        }
        assert_eq!(
            st.doc_collections(&id).unwrap(),
            vec![("notes".to_string(), 1), ("runs".to_string(), 2)]
        );
        st.delete_artifact(DAEMON, &id).unwrap();
        assert!(matches!(st.doc_collections(&id), Err(CoreError::NotFound)));
    }
}
