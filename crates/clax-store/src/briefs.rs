//! Artifacts a person started from a brief in their browser (`briefs`,
//! migration 40, `POST /api/artifacts:new`): who started each, for the
//! line under its title, and the `Idempotency-Key` the request named, so a
//! retried request answers the artifact the first one made. As a publish's
//! key ([`crate::remotes`]), a request holds its key's lock
//! ([`Store::lock_brief_key`]) from its look-up to its record, and a key
//! whose artifact is gone is taken over by the next request naming it.

use super::Store;
use clax_core::Result;
use clax_core::content::ArtifactLock;
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

/// The starter of an artifact the owner started.
pub const OWNER_STARTER: &str = "*owner";

/// The longest `Idempotency-Key` a brief takes.
pub const MAX_BRIEF_KEY: usize = 128;

impl Store {
    /// The artifact `starter` (a public ID, or [`OWNER_STARTER`]) started
    /// with idempotency key `key`, when one was recorded.
    ///
    /// # Errors
    /// The store's.
    pub fn brief_by_key(&self, starter: &str, key: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id FROM briefs WHERE starter = ?1 AND key = ?2",
                params![starter, key],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Records that `starter` started artifact `aid` from a brief, under
    /// `key` when the request named one. A key whose recorded artifact is
    /// gone (deleted, or never made) is taken over: its old row keeps its
    /// starter and loses the key. A key `starter` recorded for a live
    /// artifact keeps it: `false` then, and nothing is written.
    ///
    /// # Errors
    /// The store's.
    pub fn record_brief(&self, aid: &str, starter: &str, key: Option<&str>) -> Result<bool> {
        self.with_tx(|tx| {
            if key.is_some() {
                tx.execute(
                    "UPDATE briefs SET key = NULL
                      WHERE starter = ?1 AND key = ?2
                        AND NOT EXISTS (SELECT 1 FROM artifacts
                                         WHERE id = briefs.artifact_id AND deleted_at IS NULL)",
                    params![starter, key],
                )?;
            }
            let n = tx.execute(
                "INSERT OR IGNORE INTO briefs (artifact_id, starter, key, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![aid, starter, key, self.now()],
            )?;
            Ok(n == 1)
        })
    }

    /// Waits for, then holds, the lock of `starter`'s brief idempotency key
    /// `key`. A keyed `POST /api/artifacts:new` holds it from its look-up
    /// of the key ([`Store::brief_by_key`]) until the key is recorded
    /// ([`Store::record_brief`]), so the same request sent again meanwhile
    /// waits, then answers the artifact the first made. It serialises the
    /// requests of one daemon process only.
    pub async fn lock_brief_key(&self, starter: &str, key: &str) -> ArtifactLock<'_> {
        let _lane = super::exec::WriteLane::waiting();
        self.brief_key_locks
            .lock_key(&format!("{starter}\n{key}"))
            .await
    }

    /// For each of `aids` started from a brief, its starter's display name
    /// now (`None`: they have none), by artifact ID; artifacts not started
    /// from a brief are left out.
    ///
    /// # Errors
    /// The store's.
    pub fn brief_starters(&self, aids: &[&str]) -> Result<HashMap<String, Option<String>>> {
        let mut out = HashMap::new();
        if aids.is_empty() {
            return Ok(out);
        }
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT b.starter,
                        CASE WHEN b.starter = ?2
                             THEN (SELECT display_name FROM viewers WHERE owner = 1 LIMIT 1)
                             ELSE (SELECT display_name FROM viewers WHERE public_id = b.starter) END
                   FROM briefs b WHERE b.artifact_id = ?1",
            )?;
            for aid in aids {
                if let Some((_, name)) = st
                    .query_row(params![aid, OWNER_STARTER], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                    })
                    .optional()?
                {
                    out.insert((*aid).to_string(), name);
                }
            }
            Ok(())
        })?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-19T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock).unwrap();
        (dir, st)
    }

    /// A new artifact titled `t`: its ID.
    fn make(st: &Store, t: &str) -> String {
        let req = serde_json::from_value(serde_json::json!({
            "title": t,
            "files": {"index.html": {"content": t, "encoding": "utf8"}}
        }))
        .unwrap();
        let p = clax_core::publish::validate(req).unwrap();
        st.create_artifact(&AuditCtx::DAEMON, p, None).unwrap().0.id
    }

    #[test]
    fn a_key_keeps_its_first_artifact_per_starter() {
        let (_d, st) = store();
        let [a1, a2, a3, a4, a5] = ["1", "2", "3", "4", "5"].map(|t| make(&st, t));
        assert!(st.record_brief(&a1, "p_rae", Some("k1")).unwrap());
        assert!(!st.record_brief(&a2, "p_rae", Some("k1")).unwrap());
        assert!(st.record_brief(&a3, "p_sam", Some("k1")).unwrap());
        assert!(st.record_brief(&a4, OWNER_STARTER, None).unwrap());
        assert!(st.record_brief(&a5, OWNER_STARTER, None).unwrap());
        assert_eq!(st.brief_by_key("p_rae", "k1").unwrap(), Some(a1.clone()));
        assert_eq!(st.brief_by_key("p_sam", "k1").unwrap(), Some(a3));
        assert_eq!(st.brief_by_key("p_rae", "k2").unwrap(), None);
        let names = st.brief_starters(&[&a1, &a4, "zz"]).unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.contains_key(&a1) && names.contains_key(&a4));
    }

    #[test]
    fn a_key_whose_artifact_is_gone_is_taken_over() {
        let (_d, st) = store();
        let a = make(&st, "A");
        assert!(st.record_brief(&a, "p_rae", Some("k1")).unwrap());
        // Once it is deleted, the next artifact takes the key over, and the
        // deleted one still names its starter.
        st.delete_artifact(
            &AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(&a).unwrap(),
        )
        .unwrap();
        let c = make(&st, "C");
        assert!(st.record_brief(&c, "p_rae", Some("k1")).unwrap());
        assert_eq!(st.brief_by_key("p_rae", "k1").unwrap(), Some(c.clone()));
        assert_eq!(st.brief_starters(&[&a, &c]).unwrap().len(), 2);
        // A key recorded for an artifact that never existed is taken over too.
        assert!(st.record_brief("zz", "p_sam", Some("k2")).unwrap());
        let d = make(&st, "D");
        assert!(st.record_brief(&d, "p_sam", Some("k2")).unwrap());
        assert_eq!(st.brief_by_key("p_sam", "k2").unwrap(), Some(d));
    }

    #[test]
    fn a_brief_key_is_held_by_one_request_at_a_time() {
        let (_d, st) = store();
        let held = clax_core::content::ready(st.lock_brief_key("p_rae", "k")).unwrap();
        assert!(clax_core::content::ready(st.lock_brief_key("p_rae", "k")).is_none());
        assert!(clax_core::content::ready(st.lock_brief_key("p_sam", "k")).is_some());
        drop(held);
        assert!(clax_core::content::ready(st.lock_brief_key("p_rae", "k")).is_some());
    }
}
