//! Draft artifacts (spec 2026-10-12-flutter-app-round2-design §5.1): an
//! agent's work in progress, published and rendered like any artifact but
//! hidden from every listing (`hidden = 1`, migration 25).
//!
//! A draft of an app (`draft_of`) starts with a copy of the app's documents
//! that the owner reads, private per-viewer subtrees left out, so a page
//! that writes while it renders writes into the copy only. Its reads of
//! other apps use the grants of `draft_of` ([`Store::may_read`]). Deleting
//! a draft removes its rows and files for good; [`Store::sweep_drafts`]
//! deletes the ones older than [`DRAFT_MAX_AGE`].

use super::Store;
use super::artifacts::delete_record;
use super::docs::caps_in;
use crate::audit::AuditCtx;
use crate::db::{Caller, Level, Op, Rules};
use crate::model::{Artifact, CONTRACT_VERSION, Version};
use crate::publish::ValidatedPublish;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

/// Most documents a draft's copy takes from its app.
pub const DRAFT_DOCS_MAX: usize = 2000;
/// Most bytes of stored document JSON a draft's copy takes from its app.
pub const DRAFT_DOCS_MAX_BYTES: usize = 8 * 1024 * 1024;
/// How old a draft gets before the daemon's sweep deletes it.
pub const DRAFT_MAX_AGE: Duration = Duration::from_secs(3600);

/// A draft as `GET /api/drafts` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DraftRow {
    pub id: String,
    /// The app the draft changes; `None` for a new app's draft.
    pub of: Option<String>,
    pub created_at: String,
}

/// A new draft: its artifact and version 1, and what its document copy took.
#[derive(Clone, Debug)]
pub struct NewDraft {
    pub artifact: Artifact,
    pub version: Version,
    pub of: Option<String>,
    pub docs_copied: usize,
    /// Whether the copy stopped at [`DRAFT_DOCS_MAX`] or
    /// [`DRAFT_DOCS_MAX_BYTES`] with documents left.
    pub docs_truncated: bool,
}

/// The hidden artifacts' IDs: loaded on open and added to by
/// [`Store::create_draft`]. Event routing asks it, so an event of a draft
/// never reaches a listing's subscribers without a database read. A
/// deleted draft's ID stays for the life of the store, so events retained
/// from before the delete (an event stream's replay) stay hidden too.
#[derive(Default)]
pub(super) struct HiddenIds(std::sync::RwLock<HashSet<String>>);

impl HiddenIds {
    pub(super) fn load(c: &Connection) -> Result<HiddenIds> {
        let mut stmt = c.prepare("SELECT id FROM artifacts WHERE hidden = 1")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(HiddenIds(std::sync::RwLock::new(ids)))
    }

    fn insert(&self, id: &str) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.to_string());
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
    }
}

/// Whether the artifact `id` is hidden (a draft), read from the row.
pub(crate) fn is_hidden_in(c: &Connection, id: &str) -> Result<bool> {
    Ok(
        c.prepare_cached("SELECT hidden FROM artifacts WHERE id = ?1")?
            .query_row(params![id], |r| r.get::<_, i64>(0))
            .optional()?
            .is_some_and(|h| h != 0),
    )
}

/// The app `id` is a draft of, when `id` is a draft of an app.
pub(crate) fn draft_of_in(c: &Connection, id: &str) -> Result<Option<String>> {
    Ok(
        c.prepare_cached("SELECT draft_of FROM artifacts WHERE id = ?1 AND hidden = 1")?
            .query_row(params![id], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten(),
    )
}

/// Copies `of`'s documents that the owner reads (private per-viewer
/// subtrees left out) into `draft`, in path order, each at version 1, until
/// [`DRAFT_DOCS_MAX`] documents or [`DRAFT_DOCS_MAX_BYTES`] of JSON;
/// returns how many it copied and whether any were left.
fn copy_docs(c: &Connection, of: &ArtifactId, draft: &ArtifactId) -> Result<(usize, bool)> {
    let rules = Rules::from_capabilities(&caps_in(c, of)?).unwrap_or_default();
    let owner = Caller {
        level: Level::Owner,
        viewer: None,
    };
    let mut read =
        c.prepare("SELECT path, collection, json FROM docs WHERE artifact_id = ?1 ORDER BY path")?;
    let mut write = c.prepare(
        "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)",
    )?;
    let now = Store::now();
    let (mut copied, mut bytes) = (0usize, 0usize);
    let mut rows = read.query(params![of.as_str()])?;
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        if !rules.allows(&path, Op::Read, &owner) {
            continue;
        }
        let json: String = r.get(2)?;
        if copied == DRAFT_DOCS_MAX || bytes + json.len() > DRAFT_DOCS_MAX_BYTES {
            return Ok((copied, true));
        }
        let collection: String = r.get(1)?;
        write.execute(params![draft.as_str(), path, collection, json, now])?;
        copied += 1;
        bytes += json.len();
    }
    if copied > 0 {
        c.execute(
            "UPDATE artifacts SET doc_seq = 1 WHERE id = ?1",
            params![draft.as_str()],
        )?;
    }
    Ok((copied, false))
}

impl Store {
    /// Whether `id` is, or was while this store was open, a hidden
    /// artifact (a draft), from memory.
    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.contains(id)
    }

    /// Creates a draft of the app `of` (or of a new app, `None`) from `p`:
    /// a hidden artifact whose version 1 holds `p`'s files and whose
    /// documents, in the same transaction, are the copy of `of`'s (see
    /// [`copy_docs`]). It has no owner session. Recorded under `ctx` as any
    /// artifact's first version is.
    ///
    /// # Errors
    /// `NotFound` when `of` is missing or deleted; `invalid_args` when `of`
    /// is itself a draft or `p` addresses threads; a publish's errors.
    pub fn create_draft(
        &self,
        ctx: &AuditCtx,
        of: Option<&ArtifactId>,
        p: ValidatedPublish,
    ) -> Result<NewDraft> {
        if !p.addresses.is_empty() {
            return Err(CoreError::invalid(
                "invalid_args",
                "a draft has no threads to address",
            ));
        }
        if let Some(of) = of {
            self.with_read(|c| {
                caps_in(c, of)?;
                if is_hidden_in(c, of.as_str())? {
                    return Err(CoreError::invalid(
                        "invalid_args",
                        "a draft is made of an app, not of another draft",
                    ));
                }
                Ok(())
            })?;
        }
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
                    pinned, capabilities_json, contract_version, owner_session_id, draft_of, hidden)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, ?6, ?7, NULL, ?8, 1)",
                params![
                    id.as_str(),
                    title,
                    p.description,
                    p.icon,
                    now,
                    caps.to_string(),
                    CONTRACT_VERSION,
                    of.map(ArtifactId::as_str)
                ],
            )?;
            Ok(())
        })?;
        self.hidden.insert(id.as_str());
        let written =
            self.write_version_then(ctx, &id, 0, &p, &BTreeMap::new(), None, |tx, _| match of {
                Some(of) => copy_docs(tx, of, &id),
                None => Ok((0, false)),
            });
        match written {
            Ok((artifact, version, (docs_copied, docs_truncated))) => Ok(NewDraft {
                artifact,
                version,
                of: of.map(|o| o.as_str().to_string()),
                docs_copied,
                docs_truncated,
            }),
            Err(e) => {
                // Version 1 never landed: the zero-version row goes too.
                if let Err(cleanup) = self.with_tx(|tx| {
                    tx.execute(
                        "DELETE FROM artifacts WHERE id = ?1 AND current_version = 0",
                        params![id.as_str()],
                    )?;
                    Ok(())
                }) {
                    tracing::warn!(draft = id.as_str(), error = %cleanup, "could not remove a failed draft's row");
                }
                Err(e)
            }
        }
    }

    /// Every draft, oldest first.
    pub fn list_drafts(&self) -> Result<Vec<DraftRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, draft_of, created_at FROM artifacts WHERE hidden = 1
                 ORDER BY created_at, id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(DraftRow {
                    id: r.get(0)?,
                    of: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Deletes the draft `id` for good: its row, versions, assets,
    /// documents and leases, then its directory. Recorded as
    /// `artifact.delete` under `ctx`. `false` when `id` is no draft (an
    /// ordinary artifact is left alone).
    ///
    /// # Errors
    /// Database errors, and I/O errors other than a missing directory.
    pub fn delete_draft(&self, ctx: &AuditCtx, id: &ArtifactId) -> Result<bool> {
        let gone = self.with_tx(|tx| {
            let Some((title, current)) = tx
                .query_row(
                    "SELECT title, current_version FROM artifacts WHERE id = ?1 AND hidden = 1",
                    params![id.as_str()],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)),
                )
                .optional()?
            else {
                return Ok(false);
            };
            for sql in [
                "DELETE FROM docs WHERE artifact_id = ?1",
                "DELETE FROM leases WHERE artifact_id = ?1",
                "DELETE FROM assets WHERE artifact_id = ?1",
                "DELETE FROM watches WHERE artifact_id = ?1",
                "DELETE FROM viewer_seen WHERE artifact_id = ?1",
                "DELETE FROM version_threads WHERE artifact_id = ?1",
                "DELETE FROM versions WHERE artifact_id = ?1",
                "DELETE FROM db_read_grants WHERE reader_id = ?1 OR source_id = ?1",
                "DELETE FROM artifacts WHERE id = ?1",
            ] {
                tx.execute(sql, params![id.as_str()])?;
            }
            if current > 0 {
                self.record_audit(
                    tx,
                    ctx,
                    delete_record(&Store::now(), id.as_str(), &title, current),
                )?;
            }
            Ok(true)
        })?;
        if !gone {
            return Ok(false);
        }
        match std::fs::remove_dir_all(self.home.artifact_dir(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(true)
    }

    /// Deletes ([`Store::delete_draft`]) every draft created before `now -
    /// max_age`; the IDs it deleted.
    ///
    /// # Errors
    /// As [`Store::delete_draft`]; drafts deleted before an error stay deleted.
    pub fn sweep_drafts(
        &self,
        ctx: &AuditCtx,
        now: chrono::DateTime<chrono::Utc>,
        max_age: Duration,
    ) -> Result<Vec<String>> {
        let age = chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::MAX);
        let cutoff = now
            .checked_sub_signed(age)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let old: Vec<String> = self
            .list_drafts()?
            .into_iter()
            .filter(|d| d.created_at < cutoff)
            .map(|d| d.id)
            .collect();
        let mut gone = Vec::with_capacity(old.len());
        for id in old {
            if self.delete_draft(ctx, &ArtifactId::parse(&id)?)? {
                gone.push(id);
            }
        }
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::validate;
    use crate::store::docs::{BatchOp, BatchWrite, Pin};
    use crate::store::test_util::{DAEMON, artifact_with_caps, session, store};
    use serde_json::{Value, json};

    const OWNER: Caller = Caller {
        level: Level::Owner,
        viewer: None,
    };

    fn publish(title: &str, caps: Option<Value>) -> ValidatedPublish {
        let mut req = json!({
            "title": title,
            "files": {"index.html": {"content": "<main>draft</main>", "encoding": "utf8"}}
        });
        if let Some(c) = caps {
            req["capabilities"] = c;
        }
        validate(serde_json::from_value(req).unwrap()).unwrap()
    }

    fn count(st: &Store, sql: &str, id: &str) -> i64 {
        st.with_read(|c| Ok(c.query_row(sql, params![id], |r| r.get(0))?))
            .unwrap()
    }

    fn set(st: &Store, id: &ArtifactId, path: &str, data: Value, caller: &Caller) {
        let lww = Pin {
            if_version: None,
            lww: true,
        };
        st.doc_set(DAEMON, id, path, data, lww, caller).unwrap();
    }

    #[test]
    fn a_draft_copies_the_owners_documents_at_version_one_and_stays_unlisted() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"db": {}}));
        set(&st, &app, "notes/n1", json!({"t": 1}), &OWNER);
        set(&st, &app, "notes/n1", json!({"t": 2}), &OWNER);
        set(&st, &app, "runs/r1", json!({}), &OWNER);
        let me = "u_00000000000000000000aa";
        let viewer = Caller {
            level: Level::Interact,
            viewer: Some(me.into()),
        };
        set(&st, &app, &format!("data/users/{me}/x"), json!({}), &viewer);

        let d = st
            .create_draft(
                DAEMON,
                Some(&app),
                publish("Draft", Some(json!({"db": {}}))),
            )
            .unwrap();
        assert_eq!((d.docs_copied, d.docs_truncated), (2, false));
        assert_eq!(d.of.as_deref(), Some(app.as_str()));
        assert_eq!(d.version.n, 1);
        let did = ArtifactId::parse(&d.artifact.id).unwrap();
        let n1 = st.doc_get(&did, "notes/n1", &OWNER).unwrap().unwrap();
        assert_eq!((n1.data, n1.version), (json!({"t": 2}), 1));
        assert!(st.is_hidden(did.as_str()) && !st.is_hidden(app.as_str()));
        assert!(st.get_artifact(&did).unwrap().is_some(), "a draft renders");
        let listed: Vec<String> = st
            .list_artifacts()
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(listed, vec![app.to_string()]);
        let next = st
            .doc_set(
                DAEMON,
                &did,
                "notes/n2",
                json!({}),
                Pin {
                    if_version: None,
                    lww: true,
                },
                &OWNER,
            )
            .unwrap();
        assert_eq!(
            next.doc.unwrap().version,
            2,
            "the sequence starts past the copy"
        );
        assert_eq!(
            st.list_drafts().unwrap(),
            vec![DraftRow {
                id: d.artifact.id.clone(),
                of: Some(app.to_string()),
                created_at: d.artifact.created_at.clone(),
            }]
        );
        let reopened = Store::open(st.home()).unwrap();
        assert!(
            reopened.is_hidden(did.as_str()),
            "the hidden set survives a restart"
        );
    }

    #[test]
    fn a_new_apps_draft_copies_nothing_and_drafts_of_drafts_are_refused() {
        let (_d, st) = store();
        let d = st.create_draft(DAEMON, None, publish("New", None)).unwrap();
        assert_eq!((d.docs_copied, d.docs_truncated, d.of), (0, false, None));
        let did = ArtifactId::parse(&d.artifact.id).unwrap();
        assert!(matches!(
            st.create_draft(DAEMON, Some(&did), publish("x", None)),
            Err(CoreError::Invalid {
                code: "invalid_args",
                ..
            })
        ));
        let ghost = ArtifactId::parse("zzzzzzzzzzzz").unwrap();
        assert!(matches!(
            st.create_draft(DAEMON, Some(&ghost), publish("x", None)),
            Err(CoreError::NotFound)
        ));
        assert_eq!(
            st.list_drafts().unwrap().len(),
            1,
            "a refused draft leaves no row"
        );
    }

    #[test]
    fn the_copy_stops_at_the_byte_cap() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"db": {}}));
        let big = "x".repeat(200 * 1024);
        let writes: Vec<BatchWrite> = (0..45)
            .map(|i| BatchWrite {
                path: format!("blobs/b{i:02}"),
                op: BatchOp::Set(json!({ "s": big })),
                if_version: None,
            })
            .collect();
        st.doc_batch(DAEMON, &app, writes, true, &OWNER).unwrap();
        let d = st
            .create_draft(
                DAEMON,
                Some(&app),
                publish("Draft", Some(json!({"db": {}}))),
            )
            .unwrap();
        let per = json!({ "s": big }).to_string().len();
        assert_eq!(d.docs_copied, DRAFT_DOCS_MAX_BYTES / per);
        assert!(d.docs_truncated);
    }

    #[test]
    fn delete_removes_every_row_and_the_files_and_spares_apps() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"db": {}}));
        set(&st, &app, "notes/n1", json!({}), &OWNER);
        let d = st
            .create_draft(
                DAEMON,
                Some(&app),
                publish("Draft", Some(json!({"db": {}}))),
            )
            .unwrap();
        let did = ArtifactId::parse(&d.artifact.id).unwrap();
        let dir = st.home().artifact_dir(&did);
        assert!(dir.exists());
        assert!(
            !st.delete_draft(DAEMON, &app).unwrap(),
            "an app is no draft"
        );
        assert!(st.get_artifact(&app).unwrap().is_some());
        assert!(st.delete_draft(DAEMON, &did).unwrap());
        assert!(!st.delete_draft(DAEMON, &did).unwrap());
        for sql in [
            "SELECT COUNT(*) FROM artifacts WHERE id = ?1",
            "SELECT COUNT(*) FROM versions WHERE artifact_id = ?1",
            "SELECT COUNT(*) FROM docs WHERE artifact_id = ?1",
        ] {
            assert_eq!(count(&st, sql, did.as_str()), 0, "{sql}");
        }
        assert_eq!(
            count(
                &st,
                "SELECT COUNT(*) FROM docs WHERE artifact_id = ?1",
                app.as_str()
            ),
            1
        );
        assert!(!dir.exists());
        assert!(
            st.is_hidden(did.as_str()),
            "its retained events stay hidden"
        );
    }

    #[test]
    fn sweep_deletes_drafts_older_than_the_age_and_keeps_newer_ones() {
        let (_d, st) = store();
        let old = st.create_draft(DAEMON, None, publish("Old", None)).unwrap();
        let new = st.create_draft(DAEMON, None, publish("New", None)).unwrap();
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET created_at = '2026-10-12T10:00:00.000Z' WHERE id = ?1",
                params![old.artifact.id],
            )?;
            tx.execute(
                "UPDATE artifacts SET created_at = '2026-10-12T10:30:00.000Z' WHERE id = ?1",
                params![new.artifact.id],
            )?;
            Ok(())
        })
        .unwrap();
        let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc();
        assert!(
            st.sweep_drafts(DAEMON, at("2026-10-12T11:00:00Z"), DRAFT_MAX_AGE)
                .unwrap()
                .is_empty(),
            "an hour old exactly is kept"
        );
        assert_eq!(
            st.sweep_drafts(DAEMON, at("2026-10-12T11:01:00Z"), DRAFT_MAX_AGE)
                .unwrap(),
            vec![old.artifact.id.clone()]
        );
        let left: Vec<String> = st
            .list_drafts()
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(left, vec![new.artifact.id]);
    }

    #[test]
    fn a_draft_reads_with_its_apps_declaration_and_grants() {
        use crate::store::grants::ReadAccess;
        let (_d, st) = store();
        let b = artifact_with_caps(&st, json!({"db": {}}));
        let a = artifact_with_caps(&st, json!({"db": {"read": [b.as_str()]}}));
        let caps = json!({"db": {"read": [b.as_str()]}});
        let d = st
            .create_draft(DAEMON, Some(&a), publish("Draft", Some(caps.clone())))
            .unwrap();
        let did = ArtifactId::parse(&d.artifact.id).unwrap();
        assert_eq!(st.may_read(&did, &b).unwrap(), ReadAccess::NotGranted);
        st.grant_read(DAEMON, &a, &b).unwrap();
        assert_eq!(st.may_read(&did, &b).unwrap(), ReadAccess::Granted);
        let fresh = st
            .create_draft(DAEMON, None, publish("New", Some(caps)))
            .unwrap();
        let fresh = ArtifactId::parse(&fresh.artifact.id).unwrap();
        assert_eq!(
            st.may_read(&fresh, &b).unwrap(),
            ReadAccess::NotGranted,
            "a new app's draft has no app's grants"
        );
    }

    #[test]
    fn a_draft_makes_no_inbox_item_and_takes_no_watch() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let d = st.create_draft(DAEMON, None, publish("New", None)).unwrap();
        let did = ArtifactId::parse(&d.artifact.id).unwrap();
        // What a session's first version of an artifact makes, refused here.
        let made = st
            .with_tx(|tx| super::super::inbox::note_version(tx, did.as_str(), 1, Some(&sid), None))
            .unwrap();
        assert_eq!(made, None);
        assert_eq!(
            count(
                &st,
                "SELECT COUNT(*) FROM inbox_items WHERE artifact_id = ?1",
                did.as_str()
            ),
            0
        );
        for r in [
            st.watch(DAEMON, &sid, &did, true).map(drop),
            st.ensure_watch(DAEMON, &sid, &did).map(drop),
        ] {
            assert!(
                matches!(r, Err(CoreError::Invalid { code: "draft", .. })),
                "{r:?}"
            );
        }
    }
}
