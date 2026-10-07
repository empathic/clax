//! Version notes' links to threads and viewers' seen marks (spec §5).

use super::Store;
use crate::changelog::{LinkSource, MAX_SEEN_PER_VIEWER};
use crate::model::Version;
use crate::publish::ValidatedPublish;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

fn insert_link(
    tx: &Transaction<'_>,
    aid: &str,
    n: u32,
    tid: &str,
    source: LinkSource,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![aid, n, tid, source.as_str(), Store::now()],
    )?;
    Ok(())
}

fn thread_on(tx: &Transaction<'_>, aid: &str, tid: &str) -> Result<bool> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM threads WHERE id = ?1 AND artifact_id = ?2",
            params![tid, aid],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Links version `n` to `p.addresses` (each must be a thread of `aid`, else
/// `unknown_thread`) and then to `p.working_threads` (skipping any that no
/// longer exist). Runs inside the version's transaction.
/// [`Store::link_on_resolve`] inside a transaction.
pub(crate) fn link_on_resolve_in(tx: &Transaction<'_>, thread_id: &str) -> Result<Option<u32>> {
    let linked: bool = tx
        .query_row(
            "SELECT 1 FROM version_threads WHERE thread_id = ?1 LIMIT 1",
            params![thread_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if linked {
        return Ok(None);
    }
    let (aid, n): (String, u32) = tx
        .query_row(
            "SELECT a.id, a.current_version FROM threads t JOIN artifacts a ON a.id = t.artifact_id WHERE t.id = ?1",
            params![thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or(CoreError::NotFound)?;
    insert_link(tx, &aid, n, thread_id, LinkSource::Resolve)?;
    Ok(Some(n))
}

pub(crate) fn link_version(
    tx: &Transaction<'_>,
    aid: &str,
    n: u32,
    p: &ValidatedPublish,
) -> Result<()> {
    for tid in &p.addresses {
        if !thread_on(tx, aid, tid)? {
            return Err(CoreError::invalid(
                "unknown_thread",
                format!("{tid} is not a thread of {aid}"),
            ));
        }
        insert_link(tx, aid, n, tid, LinkSource::Explicit)?;
    }
    for tid in &p.working_threads {
        if thread_on(tx, aid, tid)? {
            insert_link(tx, aid, n, tid, LinkSource::Working)?;
        }
    }
    Ok(())
}

/// Sets each version's `addresses`, in link order.
pub(crate) fn fill_addresses(
    c: &Connection,
    aid: &ArtifactId,
    versions: &mut [Version],
) -> Result<()> {
    let mut stmt = c.prepare(
        "SELECT version_n, thread_id FROM version_threads WHERE artifact_id = ?1 ORDER BY created_at, rowid",
    )?;
    let rows = stmt.query_map(params![aid.as_str()], |r| {
        Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (n, tid) = row?;
        if let Some(v) = versions.iter_mut().find(|v| v.n == n) {
            v.addresses.push(tid);
        }
    }
    Ok(())
}

impl Store {
    /// The versions `thread_id` is linked to, ascending.
    pub fn addressed_in(&self, thread_id: &str) -> Result<Vec<u32>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT version_n FROM version_threads WHERE thread_id = ?1 ORDER BY version_n",
            )?;
            Ok(stmt
                .query_map(params![thread_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<u32>>>()?)
        })
    }

    /// An agent resolved `thread_id`: links it to its artifact's current
    /// version when it has no link yet. The version linked, or `None`.
    pub fn link_on_resolve(&self, thread_id: &str) -> Result<Option<u32>> {
        self.with_tx(|tx| link_on_resolve_in(tx, thread_id))
    }

    /// The viewer's seen mark on `aid`.
    pub fn seen(&self, viewer_id: &str, aid: &ArtifactId) -> Result<Option<u32>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Raises the viewer's seen mark on `aid` to `n`, at most the latest
    /// version (never lowers it), and prunes the viewer's rows past
    /// [`MAX_SEEN_PER_VIEWER`], least recently updated first. The mark after
    /// the write.
    pub fn mark_seen(&self, viewer_id: &str, aid: &ArtifactId, n: u32) -> Result<u32> {
        let now = Store::now();
        self.with_tx(|tx| {
            let latest: Option<u32> = tx.query_row(
                "SELECT MAX(n) FROM versions WHERE artifact_id = ?1",
                params![aid.as_str()],
                |r| r.get(0),
            )?;
            let n = n.min(latest.unwrap_or(0));
            tx.execute(
                "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET seen_n = MAX(seen_n, excluded.seen_n), updated_at = excluded.updated_at",
                params![viewer_id, aid.as_str(), n, now],
            )?;
            super::inbox::read_by_seen(tx, viewer_id, aid.as_str(), n, &now)?;
            tx.execute(
                "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
                   (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
                params![viewer_id, MAX_SEEN_PER_VIEWER as i64],
            )?;
            Ok(tx.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::publish::{PublishRequest, validate};
    use crate::store::test_util::DAEMON;
    use crate::store::test_util::{anchor, artifact, store};
    use crate::{ArtifactId, NewThread, Store};

    fn thread(st: &Store, id: &ArtifactId) -> String {
        st.create_thread(
            id,
            NewThread {
                author_public_id: None,
                version_n: 1,
                anchor: anchor(),
                body: "@agent x".into(),
                author_name: "Alex".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn v2(
        st: &Store,
        id: &ArtifactId,
        note: Option<&str>,
        addresses: &[&str],
        working: &[String],
    ) -> crate::Result<crate::model::Version> {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": 1, "note": note, "addresses": addresses,
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let mut p = validate(req)?;
        p.working_threads = working.to_vec();
        st.publish_version(DAEMON, id, p, None).map(|(_, v)| v)
    }

    #[test]
    fn a_publish_stores_its_note_and_links_explicit_and_working_threads_once() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        let v = v2(
            &st,
            &id,
            Some("  Two\ncolumns  "),
            &[&t1],
            &[t1.clone(), t2.clone()],
        )
        .unwrap();
        assert_eq!(v.note.as_deref(), Some("Two columns"));
        assert_eq!(v.addresses, [t1.clone(), t2.clone()]);
        assert_eq!(
            st.list_versions(&id).unwrap()[1].addresses,
            [t1.clone(), t2.clone()]
        );
        assert_eq!(st.addressed_in(&t1).unwrap(), [2]);
        assert_eq!(
            st.get_thread(&t1).unwrap().unwrap().status,
            "open",
            "linking never resolves"
        );
    }

    #[test]
    fn an_unknown_address_fails_the_publish_and_a_vanished_working_thread_is_skipped() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let e = v2(&st, &id, None, &[&crate::new_ulid()], &[]).unwrap_err();
        assert!(
            matches!(e, crate::CoreError::Invalid { code, .. } if code == "unknown_thread"),
            "{e:?}"
        );
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 1);
        let v = v2(&st, &id, None, &[], &[crate::new_ulid()]).unwrap();
        assert!(v.addresses.is_empty());
    }

    #[test]
    fn a_long_note_is_cut_and_flagged_and_too_many_addresses_are_refused() {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "note": "n".repeat(400), "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        }))
        .unwrap();
        let p = validate(req).unwrap();
        assert!(p.note_truncated);
        assert_eq!(p.note.unwrap().chars().count(), 280);
        let many: Vec<String> = (0..51).map(|_| crate::new_ulid()).collect();
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "addresses": many, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        }))
        .unwrap();
        assert!(validate(req).is_err());
    }

    #[test]
    fn an_agent_resolve_links_only_a_thread_with_no_link() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        assert_eq!(st.link_on_resolve(&t1).unwrap(), None);
        assert_eq!(st.link_on_resolve(&t2).unwrap(), Some(2));
        assert_eq!(st.addressed_in(&t2).unwrap(), [2]);
    }

    #[test]
    fn deleting_a_thread_deletes_its_links() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let t1 = thread(&st, &id);
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        st.delete_thread(&t1).unwrap();
        assert!(st.list_versions(&id).unwrap()[1].addresses.is_empty());
    }

    #[test]
    fn seen_marks_only_move_forward_and_are_bounded_per_viewer() {
        let (_d, st) = store();
        let viewer = st.upsert_viewer(&crate::new_ulid(), None).unwrap().id;
        let id = artifact(&st, None);
        v2(&st, &id, None, &[], &[]).unwrap();
        assert_eq!(st.seen(&viewer, &id).unwrap(), None);
        assert_eq!(st.mark_seen(&viewer, &id, 2).unwrap(), 2);
        assert_eq!(st.mark_seen(&viewer, &id, 1).unwrap(), 2);
        let ids: Vec<ArtifactId> = (0..crate::changelog::MAX_SEEN_PER_VIEWER)
            .map(|_| artifact(&st, None))
            .collect();
        for a in &ids {
            st.mark_seen(&viewer, a, 1).unwrap();
        }
        assert_eq!(
            st.seen(&viewer, &id).unwrap(),
            None,
            "the least recently updated row was pruned"
        );
        assert_eq!(st.seen(&viewer, &ids[0]).unwrap(), Some(1));
    }

    #[test]
    fn a_seen_mark_never_passes_the_latest_version() {
        let (_d, st) = store();
        let viewer = st.upsert_viewer(&crate::new_ulid(), None).unwrap().id;
        let id = artifact(&st, None);
        assert_eq!(st.mark_seen(&viewer, &id, 999).unwrap(), 1);
        v2(&st, &id, None, &[], &[]).unwrap();
        assert_eq!(
            st.mark_seen(&viewer, &id, 2).unwrap(),
            2,
            "a later version is still news"
        );
    }
}
