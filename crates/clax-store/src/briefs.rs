//! Artifacts a person started from a brief in their browser (`briefs`,
//! migration 40, `POST /api/artifacts:new`): who started each, for the
//! line under its title, and the `Idempotency-Key` the request named, so a
//! retried request answers the artifact the first one made.

use super::Store;
use clax_core::Result;
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
    /// `key` when the request named one. A key `starter` recorded already
    /// keeps its artifact: `false` then, and nothing is written.
    ///
    /// # Errors
    /// The store's.
    pub fn record_brief(&self, aid: &str, starter: &str, key: Option<&str>) -> Result<bool> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "INSERT OR IGNORE INTO briefs (artifact_id, starter, key, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![aid, starter, key, self.now()],
            )?;
            Ok(n == 1)
        })
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
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    #[test]
    fn a_key_keeps_its_first_artifact_per_starter() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-19T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock).unwrap();
        assert!(st.record_brief("a1", "p_rae", Some("k1")).unwrap());
        assert!(!st.record_brief("a2", "p_rae", Some("k1")).unwrap());
        assert!(st.record_brief("a3", "p_sam", Some("k1")).unwrap());
        assert!(st.record_brief("a4", OWNER_STARTER, None).unwrap());
        assert!(st.record_brief("a5", OWNER_STARTER, None).unwrap());
        assert_eq!(
            st.brief_by_key("p_rae", "k1").unwrap().as_deref(),
            Some("a1")
        );
        assert_eq!(
            st.brief_by_key("p_sam", "k1").unwrap().as_deref(),
            Some("a3")
        );
        assert_eq!(st.brief_by_key("p_rae", "k2").unwrap(), None);
        let names = st.brief_starters(&["a1", "a4", "zz"]).unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.contains_key("a1") && names.contains_key("a4"));
    }
}
