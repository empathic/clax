//! Forks (spec 2026-10-11-clax-dev-hosted-design H11): a new artifact of
//! the forking person's whose version 1 is a chosen version of another
//! artifact.
//!
//! - The fork's version 1 holds the source version's files, each with the
//!   authors it had there (spec 2026-10-08-multi-agent-multi-person §8.2),
//!   and its title, description, icon and capabilities are the source's,
//!   with their authors. Its creator, and version 1's publisher, is the
//!   forking person (`artifacts.person`, `versions.person`), and version 1
//!   is the forking session's when an agent forks.
//! - Its content comes from [`ContentStore::fork`]: a backend that keeps
//!   history gives the fork the source's history behind version 1 (git:
//!   `Clax-Forked-From`), the others copy the files. It keeps the source's
//!   `legacy_paths`, and refuses a file its site would serve itself
//!   ([`clax_core::publish::check_path_for`]).
//! - The source's assets that the version's files name by ID (a run of
//!   letters and digits that is the ID, [`super::assets::named_ids`]) are
//!   copied, bytes and row, under the same IDs, each with its uploader and
//!   page authors: a site serves only its own artifact's assets, so the
//!   fork's pages keep loading them from the fork's host, and the source's
//!   deletion of one, or of itself, leaves the fork's. Each copy is
//!   recorded as an `asset.upload` with `forked_from {artifact_id,
//!   asset_id}`. An asset named only at run time (built in script, or kept
//!   in a document) is not copied, as documents are not. A version naming
//!   more than [`MAX_FORK_ASSETS`] assets, or more than
//!   [`MAX_FORK_ASSET_BYTES`] of them, is refused `too_many_assets` before
//!   anything is copied. Threads, documents, shares and link access are not
//!   copied.
//! - `artifacts.forked_from_id` and `forked_from_version` record where it
//!   came from ([`clax_core::model::ForkedFrom`]); version 1's note says it,
//!   by ID (`Forked from <artifact ID>@v<n>`), as the git commit does, so
//!   it names nothing the fork's readers may not see.
//! - The transaction records `artifact.create` with `forked_from
//!   {artifact_id, n}` and `version.publish` with `source {artifact_id,
//!   n}` under the caller's context.
//!
//! The rights a fork needs (read on the source, create) are the caller's
//! to check.
//!
//! [`ContentStore::fork`]: clax_core::content::ContentStore::fork

use super::Store;
use super::artifacts::{
    SELECT, SELECT_VERSION, create_record, owners_alone, row_to_artifact, row_to_version, tokens,
    version_record,
};
use super::assets::{MAX_FORK_ASSET_BYTES, MAX_FORK_ASSETS};
use super::changelog::fill_addresses;
use clax_core::audit::{AuditCtx, content_manifest_sha256, sha256_hex};
use clax_core::live::KIND_LIVE;
use clax_core::model::ARTIFACT_TEXT_FIELDS as TEXT_FIELDS;
use clax_core::model::{Artifact, FileMeta, Version};
use clax_core::visibility::OWNER;
use clax_core::visibility::{Author, file_authors};
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

/// Version 1's note of a fork of version `n` of `from`.
pub fn fork_note(from: &str, n: u32) -> String {
    format!("Forked from {from}@v{n}")
}

/// Each author list of `map` as stored: `None` when every list names the
/// owner alone and the publisher (`creator`) is the owner, which is what a
/// missing map means; else the map, as JSON.
fn stored_authors(
    map: &BTreeMap<String, Vec<Option<String>>>,
    creator: Option<&str>,
) -> Option<String> {
    let creator_owner = matches!(Author::of_token(creator), Author::Owner);
    (!(creator_owner && owners_alone(map)))
        .then(|| serde_json::to_string(map).expect("serialisable map"))
}

impl Store {
    /// Forks version `n` (`None`: the current version) of artifact `from`
    /// into a new artifact (see the [module docs](self)) published as
    /// `person` (a public ID; `None`: `session_id`'s person, else the
    /// owner) from `session_id`.
    ///
    /// # Errors
    /// `NotFound` when `from` is not a live artifact; `invalid_argument`
    /// for a live page, and `unknown_version` when it has no version `n`;
    /// the content store's and the database's.
    pub fn fork_artifact(
        &self,
        ctx: &AuditCtx,
        from: &ArtifactId,
        n: Option<u32>,
        person: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        self.block_on(self.fork_artifact_async(ctx, from, n, person, session_id))
    }

    /// [`Store::fork_artifact`], awaiting its content store.
    pub async fn fork_artifact_async(
        &self,
        ctx: &AuditCtx,
        from: &ArtifactId,
        n: Option<u32>,
        person: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(Artifact, Version)> {
        let creator = match person {
            Some(person) => Some(person.to_string()),
            None => match session_id {
                Some(sid) => self.get_session(sid)?.and_then(|s| s.person),
                None => None,
            },
        };
        self.check_quota(creator.as_deref())?;
        let id = ArtifactId::generate();
        // The source's lock keeps its version from going while it is
        // copied (a delete takes it too).
        let _locks = self.lock_contents(&[from, &id]).await?;
        let src = self.get_artifact(from)?.ok_or(CoreError::NotFound)?;
        if src.kind == KIND_LIVE {
            return Err(CoreError::invalid(
                "invalid_argument",
                "a live page cannot be forked: its versions are snapshots of a page elsewhere",
            ));
        }
        let n = n.unwrap_or(src.current_version);
        let v = self.get_version(from, n)?.ok_or_else(|| {
            CoreError::invalid(
                "unknown_version",
                format!("{} has no version {n}", from.as_str()),
            )
        })?;
        // The fork's site reserves what the source's does (`legacy_paths`,
        // inherited below): a path it would serve itself is refused.
        for path in v.files.keys() {
            clax_core::publish::check_path_for(path, src.legacy_paths)?;
        }
        let mut files: BTreeMap<String, FileMeta> = BTreeMap::new();
        for (path, meta) in &v.files {
            let mut meta = meta.clone();
            if meta.sha256.is_none() {
                // Written before file hashes: hash the stored bytes.
                let bytes = self
                    .content
                    .read_file(from, n, path)
                    .await?
                    .ok_or_else(|| clax_core::content::missing(from, n, path))?;
                meta.sha256 = Some(sha256_hex(&bytes));
            }
            files.insert(path.clone(), meta);
        }
        let note = fork_note(from.as_str(), n);
        let meta = self.publisher_meta(person, session_id, None)?;
        let paths: Vec<&str> = files.keys().map(String::as_str).collect();
        let named = self.fork_assets(from, n, &paths).await?;
        self.content.fork(from, n, &paths, &id, &meta).await?;
        let named: BTreeSet<&str> = named.iter().map(String::as_str).collect();
        let copies = match self.copy_asset_bytes(from, &named, &id).await {
            Ok(c) => c,
            Err(e) => {
                let _ = self.content.delete_artifact(&id).await;
                return Err(e);
            }
        };
        let now = self.now();
        let files_json = serde_json::to_string(&files).expect("serialisable map");
        let content_sha256 = content_manifest_sha256(&files);
        #[cfg(test)]
        self.point("artifact.fork.insert");
        let result = self.with_tx(|tx| {
            let creator: Option<String> = match person {
                Some(p) => Some(p.to_string()),
                None => tx
                    .query_row(
                        "SELECT person FROM sessions WHERE id = ?1",
                        params![session_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten(),
            }
            .filter(|p| p != OWNER);
            self.check_quota_in(tx, creator.as_deref())?;
            // Each file keeps the authors it had in the source version;
            // each text field the source's authors of it.
            let file_map: BTreeMap<String, Vec<Option<String>>> = files
                .keys()
                .map(|path| {
                    let authors = tokens(file_authors(&v.authors, path));
                    (path.clone(), authors.into_iter().map(Some).collect())
                })
                .collect();
            let source_creator = src.authors.creator.clone().unwrap_or(OWNER.to_string());
            let text_map: BTreeMap<String, Vec<Option<String>>> = TEXT_FIELDS
                .iter()
                .map(|f| {
                    let authors = src
                        .authors
                        .text
                        .as_ref()
                        .and_then(|t| t.get(*f))
                        .filter(|a| !a.is_empty())
                        .cloned()
                        .unwrap_or_else(|| vec![Some(source_creator.clone())]);
                    ((*f).to_string(), authors)
                })
                .collect();
            tx.execute(
                "INSERT INTO artifacts (id, title, description, icon, created_at, updated_at,
                    current_version, pinned, capabilities_json, contract_version, owner_session_id,
                    person, text_authors_json, forked_from_id, forked_from_version, legacy_paths)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, 0, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    id.as_str(),
                    src.title,
                    src.description,
                    src.icon,
                    now,
                    src.capabilities.to_string(),
                    src.contract_version,
                    session_id,
                    creator,
                    stored_authors(&text_map, creator.as_deref()),
                    from.as_str(),
                    n,
                    src.legacy_paths,
                ],
            )?;
            tx.execute(
                "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json, note,
                    content_sha256, person, by_page, file_authors_json)
                 VALUES (?1, 1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
                params![
                    id.as_str(),
                    now,
                    session_id,
                    files_json,
                    note,
                    content_sha256,
                    creator,
                    stored_authors(&file_map, creator.as_deref()),
                ],
            )?;
            super::inbox::note_version(tx, id.as_str(), 1, session_id, Some(&note), &now)?;
            let a = tx.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id.as_str()],
                row_to_artifact,
            )??;
            let mut fv = tx.query_row(
                &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = 1"),
                params![id.as_str()],
                row_to_version,
            )??;
            fill_addresses(tx, &id, std::slice::from_mut(&mut fv))?;
            let source = serde_json::json!({"artifact_id": from.as_str(), "n": n});
            let mut create = create_record(&a, &fv.created_at).with("forked_from", source.clone());
            create.ids.artifact2 = Some(from.as_str().to_string());
            self.record_audit(tx, ctx, create)?;
            let mut publish =
                version_record(&a, &fv, &[], Some(false), None).with("source", source);
            publish.ids.artifact2 = Some(from.as_str().to_string());
            self.record_audit(tx, ctx, publish)?;
            self.copy_assets_in(tx, ctx, &copies, &id, &now)?;
            Ok((a, fv))
        });
        if result.is_err()
            && let Err(e) = self.content.delete_artifact(&id).await
        {
            // Left unrecorded: never served, and listed by `clax doctor`.
            tracing::warn!(
                artifact = id.as_str(),
                error = %e,
                "could not remove an unrecorded fork's content"
            );
        }
        result
    }
}

impl Store {
    /// The IDs of the assets of `from` that version `n`'s files `paths`
    /// name (see the [module docs](self)): what the fork copies.
    ///
    /// # Errors
    /// `too_many_assets` when they are more than [`MAX_FORK_ASSETS`] or
    /// their bytes more than [`MAX_FORK_ASSET_BYTES`]; content errors.
    async fn fork_assets(
        &self,
        from: &ArtifactId,
        n: u32,
        paths: &[&str],
    ) -> Result<BTreeSet<String>> {
        let assets = self.list_assets(from)?;
        if assets.is_empty() {
            return Ok(BTreeSet::new());
        }
        let ids: BTreeSet<&str> = assets.iter().map(|a| a.id.as_str()).collect();
        let mut named = BTreeSet::new();
        for path in paths {
            if named.len() == ids.len() {
                break;
            }
            let bytes = self
                .content
                .read_file(from, n, path)
                .await?
                .ok_or_else(|| clax_core::content::missing(from, n, path))?;
            super::assets::named_ids(&bytes, &ids, &mut named);
        }
        let bytes: u64 = assets
            .iter()
            .filter(|a| named.contains(a.id.as_str()))
            .map(|a| a.size)
            .sum();
        if named.len() > MAX_FORK_ASSETS || bytes > MAX_FORK_ASSET_BYTES {
            return Err(CoreError::invalid(
                "too_many_assets",
                format!(
                    "a fork copies at most {MAX_FORK_ASSETS} assets of at most \
                     {MAX_FORK_ASSET_BYTES} bytes in all; version {n}'s files name {} \
                     assets of {bytes} bytes",
                    named.len()
                ),
            ));
        }
        Ok(named.into_iter().map(str::to_string).collect())
    }
}

/// The most artifacts [`Store::repo_history`] follows up a lineage.
const MAX_LINEAGE: usize = 1000;

/// What an artifact's repository history carries, as far as the store
/// knows its authors ([`Store::repo_history`]).
#[derive(Debug, Default)]
pub struct RepoHistory {
    /// Every version whose commit the history holds: for a fork, its
    /// source's versions up to the forked one (and so on up the lineage),
    /// then its own.
    pub versions: Vec<Version>,
    /// The pushers of commits left out of a version
    /// (`version.push_refused`) of the artifact or of an artifact up its
    /// lineage (`None`: the owner), as a public ID: those commits stay on
    /// `main`, where a clone reaches them.
    pub refused_pushers: Vec<Option<String>>,
}

impl Store {
    /// What `id`'s repository history carries (see [`RepoHistory`]), its
    /// lineage followed through `forked_from_id` and
    /// `forked_from_version` whether or not those artifacts are deleted
    /// since. An ancestor's refused pushers are all listed, whether the
    /// refused commit came before or after the forked version. `None` when
    /// an artifact of the lineage is gone from the store, or the lineage is
    /// longer than [`MAX_LINEAGE`] or loops: the history's authors cannot
    /// all be named.
    ///
    /// # Errors
    /// The database's.
    pub fn repo_history(&self, id: &ArtifactId) -> Result<Option<RepoHistory>> {
        let mut out = RepoHistory::default();
        let mut seen: Vec<String> = Vec::new();
        let mut at: (String, Option<u32>) = (id.as_str().to_string(), None);
        loop {
            if seen.len() >= MAX_LINEAGE || seen.contains(&at.0) {
                return Ok(None);
            }
            let row: Option<(Option<String>, Option<u32>)> = self.with_read(|c| {
                Ok(c.query_row(
                    "SELECT forked_from_id, forked_from_version FROM artifacts WHERE id = ?1",
                    params![at.0],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
            })?;
            let Some((from, from_n)) = row else {
                return Ok(None);
            };
            let Ok(aid) = ArtifactId::parse(&at.0) else {
                return Ok(None);
            };
            let mut versions: Vec<Version> = self
                .list_versions(&aid)?
                .into_iter()
                .filter(|v| at.1.is_none_or(|upto| v.n <= upto))
                .collect();
            versions.append(&mut out.versions);
            out.versions = versions;
            let refused: Vec<Option<String>> = self.with_read(|c| {
                let mut stmt = c.prepare(
                    "SELECT json_extract(body, '$.person') FROM audit_events
                     WHERE artifact_id = ?1 AND kind = ?2",
                )?;
                let rows = stmt.query_map(
                    params![
                        at.0,
                        clax_core::audit::AuditKind::VersionPushRefused.as_str()
                    ],
                    |r| r.get(0),
                )?;
                Ok(rows.collect::<rusqlite::Result<Vec<Option<String>>>>()?)
            })?;
            for p in refused {
                if !out.refused_pushers.contains(&p) {
                    out.refused_pushers.push(p);
                }
            }
            seen.push(at.0.clone());
            match (from, from_n) {
                (Some(f), Some(n)) => at = (f, Some(n)),
                _ => return Ok(Some(out)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use crate::test_util::{DAEMON, anchor, content_file, store};
    use crate::threads::NewThread;
    use clax_core::ArtifactId;
    use rusqlite::params;

    /// A connected person's public ID, as the store records one.
    const MIA: &str = "p_mia0000000000";

    /// The audit events of artifact `aid`: kind and body.
    fn audit_of(st: &Store, aid: &str) -> Vec<(String, serde_json::Value)> {
        st.events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.ids.artifact.as_deref() == Some(aid))
            .map(|e| (e.kind.to_string(), serde_json::from_str(&e.body).unwrap()))
            .collect()
    }

    fn publish(st: &Store, id: Option<&ArtifactId>, files: &[(&str, &str)]) -> ArtifactId {
        let mut f = serde_json::Map::new();
        for (path, body) in files {
            f.insert(
                (*path).to_string(),
                serde_json::json!({"content": body, "encoding": "utf8"}),
            );
        }
        let mut req = serde_json::json!({"files": f});
        match id {
            None => {
                req["title"] = "Quarterly Review".into();
                req["description"] = "The numbers".into();
                let p = clax_core::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
                let (a, _) = st.create_artifact(DAEMON, p, None).unwrap();
                ArtifactId::parse(&a.id).unwrap()
            }
            Some(id) => {
                let cur = st.get_artifact(id).unwrap().unwrap().current_version;
                req["if_version"] = cur.into();
                let p = clax_core::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
                st.publish_version(DAEMON, id, p, None).unwrap();
                id.clone()
            }
        }
    }

    #[test]
    fn a_fork_keeps_its_sources_reserved_paths() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        assert!(!a.legacy_paths);
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET legacy_paths = 1 WHERE id = ?1",
                params![src.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        assert!(a.legacy_paths, "a legacy artifact's fork keeps /_blob/");
        // A new artifact owns `_blob/`; once its site reserves it, no fork
        // copies a file there.
        let new = publish(&st, None, &[("index.html", "<p>"), ("_blob/a.txt", "a")]);
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET legacy_paths = 1 WHERE id = ?1",
                params![new.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let e = st
            .fork_artifact(DAEMON, &new, None, None, None)
            .unwrap_err();
        assert!(e.to_string().contains("reserved"), "{e}");
    }

    #[test]
    fn a_fork_is_a_new_artifact_whose_version_one_is_the_chosen_version() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one"), ("app.js", "a")]);
        publish(&st, Some(&src), &[("index.html", "<p>two")]);
        let (a, v) = st.fork_artifact(DAEMON, &src, Some(1), None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        assert_ne!(fork, src);
        assert_eq!(a.title, "Quarterly Review");
        assert_eq!(a.description.as_deref(), Some("The numbers"));
        assert_eq!(a.current_version, 1);
        assert_eq!(
            a.forked_from,
            Some(clax_core::model::ForkedFrom {
                artifact_id: src.as_str().to_string(),
                version: 1
            })
        );
        assert_eq!(v.n, 1);
        assert_eq!(
            v.note.as_deref(),
            Some(format!("Forked from {src}@v1").as_str())
        );
        assert_eq!(v.files.keys().collect::<Vec<_>>(), ["app.js", "index.html"]);
        assert_eq!(
            content_file(&st, &fork, 1, "index.html").unwrap(),
            b"<p>one"
        );
        assert_eq!(content_file(&st, &fork, 1, "app.js").unwrap(), b"a");
        let src_v1 = st.get_version(&src, 1).unwrap().unwrap();
        assert_eq!(v.content_sha256, src_v1.content_sha256);
        // The source is as it was, and its lineage is its own (none).
        let s = st.get_artifact(&src).unwrap().unwrap();
        assert_eq!((s.current_version, s.forked_from), (2, None));
        // The fork lists and goes on as any artifact does.
        assert!(st.list_artifacts().unwrap().iter().any(|x| x.id == a.id));
        publish(&st, Some(&fork), &[("index.html", "<p>mine")]);
        assert_eq!(content_file(&st, &fork, 2, "app.js").unwrap(), b"a");
        assert_eq!(content_file(&st, &src, 2, "index.html").unwrap(), b"<p>two");
    }

    #[test]
    fn a_fork_defaults_to_the_current_version_and_copies_no_threads() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        publish(&st, Some(&src), &[("index.html", "<p>two")]);
        st.create_thread(
            DAEMON,
            &src,
            NewThread {
                author_public_id: None,
                version_n: 2,
                anchor: anchor(),
                author_name: "Alex".into(),
                body: "A thought.".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap();
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        assert_eq!(a.forked_from.unwrap().version, 2);
        assert_eq!(
            content_file(&st, &fork, 1, "index.html").unwrap(),
            b"<p>two"
        );
        assert!(st.list_threads(&fork, true, None, 50).unwrap().0.is_empty());
        assert_eq!(st.list_threads(&src, true, None, 50).unwrap().0.len(), 1);
    }

    #[test]
    fn a_fork_by_a_person_is_theirs_and_keeps_the_authors_of_what_it_copies() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        let mia = MIA.to_string();
        let (a, v) = st
            .fork_artifact(DAEMON, &src, None, Some(&mia), None)
            .unwrap();
        assert_eq!(a.authors.creator.as_deref(), Some(mia.as_str()));
        assert!(st.artifact_created_by(&a.id, Some(&mia)).unwrap());
        assert!(!st.artifact_created_by(src.as_str(), Some(&mia)).unwrap());
        // Mia published version 1; its file and the title stay the owner's.
        assert_eq!(v.authors.person.as_deref(), Some(mia.as_str()));
        assert_eq!(
            clax_core::visibility::file_authors(&v.authors, "index.html"),
            [clax_core::visibility::Author::Owner]
        );
        let text = a.authors.text.unwrap();
        assert_eq!(text["title"], [Some(super::OWNER.to_string())]);
    }

    #[test]
    fn a_fork_of_a_missing_artifact_or_version_makes_nothing() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        let before = st.list_artifacts().unwrap().len();
        assert!(matches!(
            st.fork_artifact(DAEMON, &ArtifactId::generate(), None, None, None),
            Err(clax_core::CoreError::NotFound)
        ));
        let e = st
            .fork_artifact(DAEMON, &src, Some(7), None, None)
            .unwrap_err();
        assert!(
            matches!(
                e,
                clax_core::CoreError::Invalid {
                    code: "unknown_version",
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(st.list_artifacts().unwrap().len(), before);
    }

    #[test]
    fn a_fork_is_recorded_as_a_creation_and_a_publish_naming_its_source() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let rows = audit_of(&st, &a.id);
        let kinds: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["artifact.create", "version.publish"]);
        let source = serde_json::json!({"artifact_id": src.as_str(), "n": 1});
        assert_eq!(rows[0].1["forked_from"], source);
        assert_eq!(rows[1].1["source"], source);
    }

    /// Forks that race each other and publishes of their source each copy
    /// one whole version of it: the files of the version they name, and
    /// the source's lock keeps a publish from landing mid-copy.
    #[test]
    fn forks_racing_each_other_and_publishes_copy_whole_versions() {
        let (_d, st) = crate::test_util::pool_store();
        let st = std::sync::Arc::new(st);
        let src = publish(&st, None, &[("index.html", "v1"), ("app.js", "v1")]);
        let forks = std::thread::scope(|s| {
            let publisher = s.spawn(|| {
                for n in 2..=6 {
                    let body = format!("v{n}");
                    publish(&st, Some(&src), &[("index.html", &body), ("app.js", &body)]);
                }
            });
            let forkers: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        (0..3)
                            .map(|_| st.fork_artifact(DAEMON, &src, None, None, None).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            publisher.join().unwrap();
            forkers
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let ids: std::collections::BTreeSet<&str> =
            forks.iter().map(|(a, _)| a.id.as_str()).collect();
        assert_eq!(ids.len(), 12, "every fork is its own artifact");
        for (a, _) in &forks {
            let fork = ArtifactId::parse(&a.id).unwrap();
            let n = a.forked_from.as_ref().unwrap().version;
            let want = format!("v{n}");
            for path in ["index.html", "app.js"] {
                assert_eq!(
                    content_file(&st, &fork, 1, path).unwrap(),
                    want.as_bytes(),
                    "{path} of a fork of version {n}"
                );
            }
        }
        assert_eq!(st.get_artifact(&src).unwrap().unwrap().current_version, 6);
    }

    /// A fork's repository history is its lineage's up to each forked
    /// version, then its own, whether the source is deleted since; with an
    /// ancestor's row gone it cannot be named.
    #[test]
    fn a_forks_repository_history_carries_its_lineage_up_to_each_forked_version() {
        let (_d, st) = store();
        let as_person = |id: &ArtifactId, person: &str, body: &str| {
            let cur = st.get_artifact(id).unwrap().unwrap().current_version;
            let req = serde_json::json!({"if_version": cur, "files": {"index.html": {"content": body, "encoding": "utf8"}}});
            let mut p = clax_core::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
            p.person = Some(person.to_string());
            st.publish_version(DAEMON, id, p, None).unwrap();
        };
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        as_person(&src, "p_rae0000000000", "<p>two");
        let mia = MIA.to_string();
        let (a, _) = st
            .fork_artifact(DAEMON, &src, Some(2), Some(&mia), None)
            .unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        // After the forked version: not in the fork's history.
        as_person(&src, "p_sam0000000000", "<p>three");
        let (b, _) = st.fork_artifact(DAEMON, &fork, None, None, None).unwrap();
        let again = ArtifactId::parse(&b.id).unwrap();
        as_person(&again, "p_kai0000000000", "<p>mine");
        let seen = |st: &Store| -> Vec<(String, u32, Option<String>)> {
            st.repo_history(&again)
                .unwrap()
                .unwrap()
                .versions
                .into_iter()
                .map(|v| (v.artifact_id, v.n, v.authors.person))
                .collect()
        };
        let want = vec![
            (src.to_string(), 1, None),
            (src.to_string(), 2, Some("p_rae0000000000".to_string())),
            (fork.to_string(), 1, Some(mia.clone())),
            (again.to_string(), 1, None),
            (again.to_string(), 2, Some("p_kai0000000000".to_string())),
        ];
        assert_eq!(seen(&st), want);
        assert!(
            st.repo_history(&again)
                .unwrap()
                .unwrap()
                .refused_pushers
                .is_empty()
        );
        // A commit pushed to the source and left out of its versions.
        st.with_tx(|tx| {
            let mut rec = clax_core::audit::AuditRecord::new(
                clax_core::audit::AuditKind::VersionPushRefused,
                st.now(),
            )
            .with("commit", "c0ffee")
            .with("person", "p_lee0000000000")
            .with("reason", "no_right");
            rec.ids.artifact = Some(src.as_str().to_string());
            st.record_audit(tx, DAEMON, rec)
        })
        .unwrap();
        assert_eq!(
            st.repo_history(&again).unwrap().unwrap().refused_pushers,
            [Some("p_lee0000000000".to_string())]
        );
        st.delete_artifact(DAEMON, &src).unwrap();
        assert_eq!(
            seen(&st),
            want,
            "a deleted source's history stays in the fork's"
        );
        st.with_tx(|tx| {
            tx.execute(
                "DELETE FROM versions WHERE artifact_id = ?1",
                [src.as_str()],
            )?;
            tx.execute("DELETE FROM artifacts WHERE id = ?1", [src.as_str()])?;
            Ok(())
        })
        .unwrap();
        assert!(
            st.repo_history(&again).unwrap().is_none(),
            "an ancestor gone: authors unknown"
        );
        assert!(st.repo_history(&ArtifactId::generate()).unwrap().is_none());
    }

    /// A fork after its source is deleted finds no such artifact.
    #[test]
    fn a_deleted_artifact_is_not_forked() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one")]);
        st.delete_artifact(DAEMON, &src).unwrap();
        assert!(matches!(
            st.fork_artifact(DAEMON, &src, None, None, None),
            Err(clax_core::CoreError::NotFound)
        ));
    }

    /// A fork copies the source's assets its version names, under the same
    /// IDs, bytes and authors; the source's deletion of one, or of itself,
    /// leaves the fork's. An asset no file names stays the source's alone.
    #[test]
    fn a_fork_copies_the_assets_its_version_names_under_the_same_ids() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>")]);
        let named = st
            .add_asset_by(DAEMON, &src, "image/png", b"png!", Some(MIA), false)
            .unwrap();
        let in_js = st.add_asset(DAEMON, &src, "text/csv", b"a,b").unwrap();
        let unnamed = st.add_asset(DAEMON, &src, "image/png", b"no").unwrap();
        // A longer word holding an ID is not a reference to it.
        let html = format!(
            "<img src=\"_clax/blob/{}\"><p>x{}y</p>",
            named.id, unnamed.id
        );
        let js = format!("fetch('/_clax/blob/' + '{}')", in_js.id);
        publish(&st, Some(&src), &[("index.html", &html), ("app.js", &js)]);
        let (a, v) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        let src_v = st.get_version(&src, 2).unwrap().unwrap();
        assert_eq!(v.content_sha256, src_v.content_sha256, "files unrewritten");
        let ids: Vec<String> = st
            .list_assets(&fork)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        let mut want = vec![named.id.clone(), in_js.id.clone()];
        want.sort();
        assert_eq!(ids, want);
        let (copy, bytes) = st.read_asset(&fork, &named.id).unwrap().unwrap();
        assert_eq!(bytes, b"png!");
        assert_eq!(
            (copy.content_type.as_str(), copy.person.as_deref()),
            ("image/png", Some(MIA))
        );
        assert!(st.get_asset(&fork, &unnamed.id).unwrap().is_none());
        // The address naming only the ID means the upload.
        assert_eq!(
            st.find_asset(&named.id).unwrap().unwrap().artifact_id,
            src.as_str()
        );
        let uploads: Vec<serde_json::Value> = audit_of(&st, &a.id)
            .into_iter()
            .filter(|(k, _)| k == "asset.upload")
            .map(|(_, b)| b["forked_from"].clone())
            .collect();
        assert_eq!(uploads.len(), 2);
        assert!(
            uploads
                .contains(&serde_json::json!({"artifact_id": src.as_str(), "asset_id": named.id}))
        );
        // The source's deletions leave the fork's copies.
        st.delete_asset(DAEMON, &src, &named.id).unwrap();
        assert_eq!(st.read_asset(&fork, &named.id).unwrap().unwrap().1, b"png!");
        st.delete_artifact(DAEMON, &src).unwrap();
        st.delete_assets_of_deleted_artifacts().unwrap();
        assert_eq!(st.read_asset(&fork, &in_js.id).unwrap().unwrap().1, b"a,b");
        assert_eq!(
            st.find_asset(&in_js.id).unwrap().unwrap().artifact_id,
            a.id,
            "with the upload's artifact gone, the copy is what the ID means"
        );
        // And the fork's deletion of its copy is its own.
        st.delete_asset(DAEMON, &fork, &in_js.id).unwrap();
        assert!(st.find_asset(&in_js.id).unwrap().is_none());
    }

    /// A fork whose version names more than [`MAX_FORK_ASSETS`] assets, or
    /// more than [`MAX_FORK_ASSET_BYTES`] of them, is refused
    /// `too_many_assets` and makes nothing: no artifact, no asset.
    #[test]
    fn a_fork_naming_too_many_assets_or_bytes_is_refused_before_any_copy() {
        use crate::assets::{MAX_FORK_ASSET_BYTES, MAX_FORK_ASSETS};
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>")]);
        let ids: Vec<String> = (0..=MAX_FORK_ASSETS)
            .map(|_| st.add_asset(DAEMON, &src, "image/png", b"x").unwrap().id)
            .collect();
        let made = |st: &Store| {
            st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM artifacts), (SELECT COUNT(*) FROM assets)",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )?)
            })
            .unwrap()
        };
        let refused = |st: &Store, html: &str| {
            publish(st, Some(&src), &[("index.html", html)]);
            let before = made(st);
            match st.fork_artifact(DAEMON, &src, None, None, None) {
                Err(clax_core::CoreError::Invalid { code, message }) => {
                    assert_eq!(code, "too_many_assets", "{message}");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(made(st), before, "nothing made");
        };
        refused(&st, &ids.join(" "));
        // At the count, the fork copies them all.
        publish(&st, Some(&src), &[("index.html", &ids[1..].join(" "))]);
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        assert_eq!(st.list_assets(&fork).unwrap().len(), MAX_FORK_ASSETS);
        // Two assets whose bytes together pass the cap.
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE assets SET size = ?2 WHERE artifact_id = ?1 AND id IN (?3, ?4)",
                params![
                    src.as_str(),
                    (MAX_FORK_ASSET_BYTES / 2 + 1) as i64,
                    ids[0],
                    ids[1]
                ],
            )?;
            Ok(())
        })
        .unwrap();
        refused(&st, &format!("{} {}", ids[0], ids[1]));
    }

    /// A fork of a version naming none of its source's assets copies none.
    #[test]
    fn a_fork_naming_no_asset_copies_none() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>")]);
        st.add_asset(DAEMON, &src, "image/png", b"x").unwrap();
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        assert!(st.list_assets(&fork).unwrap().is_empty());
    }

    #[test]
    fn a_fork_through_the_default_content_store_copies_the_files() {
        let (_d, st) = store();
        let src = publish(&st, None, &[("index.html", "<p>one"), ("a/b.css", "b")]);
        let (a, _) = st.fork_artifact(DAEMON, &src, None, None, None).unwrap();
        let fork = ArtifactId::parse(&a.id).unwrap();
        let bytes = clax_core::content::wait(st.content().read_file(&fork, 1, "a/b.css")).unwrap();
        assert_eq!(bytes.as_deref(), Some(&b"b"[..]));
    }
}
