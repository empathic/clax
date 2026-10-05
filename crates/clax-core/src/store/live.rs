//! Live pages (spec 2026-10-05-chrome-overlay-design §5): artifacts of kind
//! `live`, keyed by origin and path, whose versions are snapshots.

use super::Store;
use crate::live::{KIND_LIVE, PageKey, placeholder_html};
use crate::model::{Artifact, CONTRACT_VERSION, Version};
use crate::publish::{Encoding, FileInput, INDEX, PublishRequest};
use crate::{ArtifactId, CoreError, Result};
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// A live page's key and its artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LivePage {
    pub artifact_id: String,
    pub origin: String,
    pub path: String,
}

/// What [`Store::ensure_live_page`] found or made.
#[derive(Clone, Debug)]
pub struct EnsuredPage {
    pub artifact: Artifact,
    /// The version a thread made now belongs on.
    pub version: Version,
    /// The page did not exist before this call.
    pub created: bool,
    /// This call wrote `version`.
    pub new_version: bool,
    /// Sessions a scope watch made watchers of the page in this call (Task 3).
    pub scoped_sessions: Vec<String>,
}

/// How many times [`Store::store_snapshot`] tries to write a version while
/// other snapshots of the page take the version number it read.
pub const SNAPSHOT_ATTEMPTS: u32 = 32;

fn row_to_page(r: &Row<'_>) -> rusqlite::Result<LivePage> {
    Ok(LivePage {
        artifact_id: r.get(0)?,
        origin: r.get(1)?,
        path: r.get(2)?,
    })
}

fn page_by_key(c: &Connection, key: &PageKey) -> Result<Option<LivePage>> {
    Ok(c.query_row(
        "SELECT p.artifact_id, p.origin, p.path FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
         WHERE p.origin = ?1 AND p.path = ?2 AND a.deleted_at IS NULL",
        params![key.origin, key.path],
        row_to_page,
    )
    .optional()?)
}

impl Store {
    /// The live page `key` names, if it exists (including one whose first
    /// version is still being written).
    pub fn find_live_page(&self, key: &PageKey) -> Result<Option<LivePage>> {
        self.with_read(|c| page_by_key(c, key))
    }

    /// The live page whose artifact is `id`, or `None` for any other artifact.
    pub fn live_page_of(&self, id: &ArtifactId) -> Result<Option<LivePage>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id, origin, path FROM live_pages WHERE artifact_id = ?1",
                params![id.as_str()],
                row_to_page,
            )
            .optional()?)
        })
    }

    /// Every live page whose artifact is not deleted.
    pub fn live_pages(&self) -> Result<Vec<LivePage>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT p.artifact_id, p.origin, p.path FROM live_pages p
                 JOIN artifacts a ON a.id = p.artifact_id WHERE a.deleted_at IS NULL",
            )?;
            let pages = st
                .query_map([], row_to_page)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(pages)
        })
    }

    /// The artifact IDs of every live page that is not deleted.
    pub fn live_page_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .live_pages()?
            .into_iter()
            .map(|p| p.artifact_id)
            .collect())
    }

    /// Finds or creates the live page `key` and gives it the version a new
    /// thread belongs on: a new page's version 1 is `snapshot`, or the
    /// placeholder without one; an existing page takes `snapshot` as a new
    /// version when it differs from the current one ([`Store::store_snapshot`]).
    /// Concurrent first calls for one key settle on one page.
    pub fn ensure_live_page(
        &self,
        key: &PageKey,
        title: &str,
        snapshot: Option<&[u8]>,
    ) -> Result<EnsuredPage> {
        let (id, created) = self.with_tx(|tx| {
            if let Some(p) = page_by_key(tx, key)? {
                return Ok((ArtifactId::parse(&p.artifact_id)?, false));
            }
            let id = ArtifactId::generate();
            let now = Store::now();
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, pinned,
                    capabilities_json, contract_version, kind)
                 VALUES (?1, ?2, ?3, ?3, 0, 0, '{}', ?4, 'live')",
                params![id.as_str(), title, now, CONTRACT_VERSION],
            )?;
            tx.execute(
                "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id.as_str(), key.origin, key.path, now],
            )?;
            Ok((id, true))
        })?;
        let current: u32 = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT current_version FROM artifacts WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })?;
        let (version, new_version) = if current == 0 {
            let html = snapshot
                .map(<[u8]>::to_vec)
                .unwrap_or_else(|| placeholder_html(key).into_bytes());
            match self.write_snapshot(&id, 0, title, &html) {
                Ok(v) => (v, true),
                // Another first call wrote version 1 meanwhile: build on it.
                Err(CoreError::Conflict { .. }) => match snapshot {
                    Some(s) => self.store_snapshot(&id, title, s, false)?,
                    None => (self.get_version(&id, 1)?.ok_or(CoreError::NotFound)?, false),
                },
                Err(e) => return Err(e),
            }
        } else if let Some(s) = snapshot {
            self.store_snapshot(&id, title, s, false)?
        } else {
            (
                self.get_version(&id, current)?.ok_or(CoreError::NotFound)?,
                false,
            )
        };
        let artifact = self.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        Ok(EnsuredPage {
            artifact,
            version,
            created,
            new_version,
            scoped_sessions: Vec::new(),
        })
    }

    /// Stores `html` as the next version of the live page `id`, titled
    /// `title`, unless it is byte-identical to the current version's
    /// `index.html` and `force` is false; then the current version is
    /// returned. The flag says whether a version was written. When another
    /// snapshot of the page lands first, this one is compared with it and
    /// written after it, so concurrent snapshots each get a version.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `not_live` for an
    /// artifact that is not a live page; `Conflict` only when other
    /// snapshots keep landing first [`SNAPSHOT_ATTEMPTS`] times in a row.
    pub fn store_snapshot(
        &self,
        id: &ArtifactId,
        title: &str,
        html: &[u8],
        force: bool,
    ) -> Result<(Version, bool)> {
        let mut attempt = 1;
        loop {
            let a = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
            if a.kind != KIND_LIVE {
                return Err(CoreError::invalid(
                    "not_live",
                    format!("{id} is not a live page"),
                ));
            }
            if !force {
                let current =
                    std::fs::read(self.home.version_dir(id, a.current_version).join(INDEX)).ok();
                if current.as_deref() == Some(html) {
                    return Ok((
                        self.get_version(id, a.current_version)?
                            .ok_or(CoreError::NotFound)?,
                        false,
                    ));
                }
            }
            match self.write_snapshot(id, a.current_version, title, html) {
                Ok(v) => return Ok((v, true)),
                // Another snapshot took this version number: build on it.
                Err(CoreError::Conflict { .. }) if attempt < SNAPSHOT_ATTEMPTS => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// Records that the agent of `harness` addressed thread `tid` of the
    /// live page `id`, to be linked to the page's next snapshot (spec L11).
    /// `source` is `explicit` (a reply with `addressed`) or `resolve` (an
    /// agent resolve). A `resolve` is recorded only when the thread has no
    /// version link and no pending address yet; an `explicit` one replaces a
    /// pending `resolve`. Returns whether a row was written.
    ///
    /// # Errors
    /// Database errors only.
    pub fn mark_pending(
        &self,
        id: &ArtifactId,
        tid: &str,
        source: &str,
        harness: &str,
    ) -> Result<bool> {
        self.with_tx(|tx| {
            if source == "resolve" {
                let linked: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM version_threads WHERE thread_id = ?1)
                        OR EXISTS(SELECT 1 FROM live_pending WHERE thread_id = ?1)",
                    params![tid],
                    |r| r.get(0),
                )?;
                if linked {
                    return Ok(false);
                }
            }
            tx.execute(
                "INSERT INTO live_pending (artifact_id, thread_id, source, harness, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(artifact_id, thread_id) DO UPDATE SET source = excluded.source,
                    harness = excluded.harness, created_at = excluded.created_at",
                params![id.as_str(), tid, source, harness, Store::now()],
            )?;
            Ok(true)
        })
    }

    /// Links every pending address of the live page `id` to its version `n`
    /// (each with its `source`) and clears them; returns the linked thread
    /// IDs, oldest address first.
    ///
    /// # Errors
    /// Database errors only.
    pub fn link_pending(&self, id: &ArtifactId, n: u32) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let ids: Vec<String> = {
                let mut st = tx.prepare(
                    "SELECT thread_id FROM live_pending WHERE artifact_id = ?1
                     ORDER BY created_at, thread_id",
                )?;
                st.query_map(params![id.as_str()], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 SELECT artifact_id, ?2, thread_id, source, ?3 FROM live_pending WHERE artifact_id = ?1
                 ORDER BY created_at, thread_id",
                params![id.as_str(), n, Store::now()],
            )?;
            tx.execute(
                "DELETE FROM live_pending WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            Ok(ids)
        })
    }

    /// The pending address of thread `tid`: the addressing agent's harness
    /// and when it addressed the thread.
    ///
    /// # Errors
    /// Database errors only.
    pub fn pending_address(&self, tid: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT harness, created_at FROM live_pending WHERE thread_id = ?1",
                params![tid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// Whether the live page `id` has addresses waiting for a snapshot.
    ///
    /// # Errors
    /// Database errors only.
    pub fn has_pending(&self, id: &ArtifactId) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_pending WHERE artifact_id = ?1)",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })
    }

    /// Writes `html` as version `expected + 1` (its only file), noted `snapshot`.
    fn write_snapshot(
        &self,
        id: &ArtifactId,
        expected: u32,
        title: &str,
        html: &[u8],
    ) -> Result<Version> {
        let p = crate::publish::validate(PublishRequest {
            title: Some(title.to_string()),
            note: Some("snapshot".to_string()),
            if_version: Some(expected),
            files: BTreeMap::from([(
                INDEX.to_string(),
                Some(FileInput {
                    content: base64::engine::general_purpose::STANDARD.encode(html),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            )]),
            ..Default::default()
        })?;
        let (_, v) = self.write_version(id, expected, &p, &BTreeMap::new(), None)?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use crate::live::{KIND_LIVE, PageKey};
    use crate::{ArtifactId, Home, Store};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }
    fn key(path: &str) -> PageKey {
        PageKey {
            origin: "http://localhost:5173".into(),
            path: path.into(),
        }
    }
    fn index(st: &Store, id: &ArtifactId, n: u32) -> String {
        std::fs::read_to_string(st.home().version_dir(id, n).join("index.html")).unwrap()
    }

    #[test]
    fn live_page_ids_name_the_pages_that_are_not_deleted() {
        let (_d, st) = store();
        let a = st.ensure_live_page(&key("/a"), "a", None).unwrap();
        let b = st.ensure_live_page(&key("/b"), "b", None).unwrap();
        let mut ids = st.live_page_ids().unwrap();
        ids.sort();
        let mut want = vec![a.artifact.id.clone(), b.artifact.id.clone()];
        want.sort();
        assert_eq!(ids, want);
        st.delete_artifact(&ArtifactId::parse(&a.artifact.id).unwrap())
            .unwrap();
        assert_eq!(st.live_page_ids().unwrap(), vec![b.artifact.id.clone()]);
        let pages = st.live_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "/b");
    }

    #[test]
    fn a_page_created_without_a_snapshot_gets_a_placeholder_version() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(&key("/"), "localhost:5173/", None)
            .unwrap();
        assert!(e.created && e.new_version);
        assert_eq!(e.artifact.kind, KIND_LIVE);
        assert_eq!(e.version.n, 1);
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        assert!(index(&st, &id, 1).contains("No snapshot yet"));
        assert_eq!(
            st.find_live_page(&key("/")).unwrap().unwrap().artifact_id,
            e.artifact.id
        );
        assert!(
            st.list_artifacts()
                .unwrap()
                .iter()
                .any(|a| a.id == e.artifact.id)
        );
    }

    #[test]
    fn a_first_comment_makes_its_snapshot_version_one_and_identical_snapshots_reuse_it() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>a"))
            .unwrap();
        assert_eq!((e.version.n, e.created, e.new_version), (1, true, true));
        let again = st
            .ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>a"))
            .unwrap();
        assert_eq!(
            (again.version.n, again.created, again.new_version),
            (1, false, false)
        );
        let changed = st
            .ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>b"))
            .unwrap();
        assert_eq!((changed.version.n, changed.new_version), (2, true));
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let (v, new) = st
            .store_snapshot(&id, "Settings", b"<!doctype html><p>b", true)
            .unwrap();
        assert_eq!(
            (v.n, new),
            (3, true),
            "force makes a version even when identical"
        );
        assert_eq!(index(&st, &id, 3), "<!doctype html><p>b");
    }

    #[test]
    fn concurrent_snapshots_of_one_page_each_get_a_version() {
        let (_d, st) = store();
        let e = st.ensure_live_page(&key("/"), "x", None).unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        const N: usize = 8;
        let start = std::sync::Barrier::new(N);
        let mut got: Vec<u32> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..N)
                .map(|i| {
                    let (st, id, start) = (&st, &id, &start);
                    s.spawn(move || {
                        start.wait();
                        let html = format!("<!doctype html><p>{i}");
                        let (v, new) = st.store_snapshot(id, "x", html.as_bytes(), false).unwrap();
                        assert!(new);
                        assert_eq!(index(st, id, v.n), html);
                        v.n
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        got.sort_unstable();
        assert_eq!(got, (2..2 + N as u32).collect::<Vec<_>>());
    }

    #[test]
    fn deleting_a_live_page_frees_its_key() {
        let (_d, st) = store();
        let e = st.ensure_live_page(&key("/"), "x", None).unwrap();
        st.delete_artifact(&ArtifactId::parse(&e.artifact.id).unwrap())
            .unwrap();
        assert!(st.find_live_page(&key("/")).unwrap().is_none());
        let again = st.ensure_live_page(&key("/"), "x", None).unwrap();
        assert_ne!(again.artifact.id, e.artifact.id);
    }

    #[test]
    fn snapshots_are_refused_on_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let err = st.store_snapshot(&id, "T", b"<p>", false).unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "not_live"));
    }

    #[test]
    fn a_route_is_refused_on_threads_of_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let mut anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "body", "file": "index.html"
        }))
        .unwrap();
        anchor.route = Some("?a=1".into());
        let err = st
            .create_thread(
                &id,
                crate::store::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor,
                    author_name: "A".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "invalid_anchor"));
    }
    /// A thread on version 1 of a new live page `/p`.
    fn live_thread(st: &Store) -> (ArtifactId, String) {
        let e = st.ensure_live_page(&key("/p"), "p", None).unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "body", "file": "index.html"
        }))
        .unwrap();
        let t = st
            .create_thread(
                &id,
                crate::store::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor,
                    author_name: "A".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        (id, t.id)
    }

    fn link_source(st: &Store, tid: &str) -> Option<String> {
        use rusqlite::OptionalExtension as _;
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT source FROM version_threads WHERE thread_id = ?1",
                rusqlite::params![tid],
                |r| r.get(0),
            )
            .optional()?)
        })
        .unwrap()
    }

    #[test]
    fn a_pending_address_links_to_the_next_snapshot_with_its_source() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(!st.has_pending(&id).unwrap());
        assert!(st.mark_pending(&id, &tid, "explicit", "claude").unwrap());
        assert!(st.has_pending(&id).unwrap());
        let (harness, at) = st.pending_address(&tid).unwrap().unwrap();
        assert_eq!(harness, "claude");
        assert!(!at.is_empty());
        assert!(
            !st.mark_pending(&id, &tid, "resolve", "codex").unwrap(),
            "a resolve leaves an explicit address alone"
        );
        assert_eq!(st.pending_address(&tid).unwrap().unwrap().0, "claude");
        let (v, _) = st.store_snapshot(&id, "p", b"<p>2", true).unwrap();
        assert_eq!(st.link_pending(&id, v.n).unwrap(), vec![tid.clone()]);
        assert_eq!(st.addressed_in(&tid).unwrap(), vec![v.n]);
        assert_eq!(link_source(&st, &tid).as_deref(), Some("explicit"));
        assert!(st.pending_address(&tid).unwrap().is_none());
        assert!(!st.has_pending(&id).unwrap());
        assert!(st.link_pending(&id, v.n).unwrap().is_empty());
        assert!(
            !st.mark_pending(&id, &tid, "resolve", "claude").unwrap(),
            "a resolve of a linked thread records nothing"
        );
    }

    #[test]
    fn an_explicit_address_replaces_a_pending_resolve() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(st.mark_pending(&id, &tid, "resolve", "codex").unwrap());
        assert!(st.mark_pending(&id, &tid, "explicit", "claude").unwrap());
        assert_eq!(st.pending_address(&tid).unwrap().unwrap().0, "claude");
        st.link_pending(&id, 1).unwrap();
        assert_eq!(link_source(&st, &tid).as_deref(), Some("explicit"));
    }

    #[test]
    fn a_resolve_alone_links_as_a_resolve() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(st.mark_pending(&id, &tid, "resolve", "codex").unwrap());
        st.link_pending(&id, 1).unwrap();
        assert_eq!(link_source(&st, &tid).as_deref(), Some("resolve"));
    }

    #[test]
    fn deleting_a_thread_deletes_its_pending_address() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        st.mark_pending(&id, &tid, "explicit", "claude").unwrap();
        st.delete_thread(&tid).unwrap();
        assert!(st.pending_address(&tid).unwrap().is_none());
        assert!(!st.has_pending(&id).unwrap());
    }
}
