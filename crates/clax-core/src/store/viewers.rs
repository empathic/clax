//! Browser viewers, keyed by the `clax_viewer` cookie (a ULID, the
//! viewer's credential) and named to others by a public ID.

use super::Store;
use crate::ids::{is_public_id, is_ulid, new_public_id};
use crate::model::Viewer;
use crate::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Longest accepted display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

/// Most named viewers one [`Store::search_viewers`] call reads. The match runs
/// in Rust (SQLite folds ASCII case only), so a search costs time linear in the
/// names it reads; past this many it stops, and names later in name order are
/// not found by that search.
pub const MAX_SEARCH_SCAN: usize = 10_000;

const VIEWER_SELECT: &str = "SELECT id, public_id, display_name, created_at FROM viewers";

fn row_to_viewer(r: &rusqlite::Row<'_>) -> rusqlite::Result<Viewer> {
    Ok(Viewer {
        id: r.get(0)?,
        public_id: r.get(1)?,
        display_name: r.get(2)?,
        created_at: r.get(3)?,
    })
}

impl Store {
    /// Creates viewer `id` when missing, with a new public ID
    /// ([`crate::new_public_id`]) that it keeps for good. `display_name`: `None` keeps the
    /// current name, `Some("")` (after trimming) clears it, any other value
    /// replaces it.
    ///
    /// # Errors
    /// `invalid_viewer` when `id` is not a ULID; `invalid_name` for a name with
    /// control characters or longer than [`MAX_NAME_CHARS`].
    pub fn upsert_viewer(&self, id: &str, display_name: Option<&str>) -> Result<Viewer> {
        if !is_ulid(id) {
            return Err(CoreError::invalid("invalid_viewer", "viewer IDs are ULIDs"));
        }
        let name = display_name.map(str::trim);
        if name
            .is_some_and(|n| n.chars().any(char::is_control) || n.chars().count() > MAX_NAME_CHARS)
        {
            return Err(CoreError::invalid(
                "invalid_name",
                format!(
                    "a display name is at most {MAX_NAME_CHARS} characters with no control characters"
                ),
            ));
        }
        let stored = name.filter(|n| !n.is_empty());
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET display_name = CASE WHEN ?5 THEN excluded.display_name ELSE display_name END",
                params![id, new_public_id(), stored, Store::now(), name.is_some()],
            )?;
            Ok(tx.query_row(&format!("{VIEWER_SELECT} WHERE id = ?1"), params![id], row_to_viewer)?)
        })
    }

    /// The viewer whose cookie is `id`.
    pub fn get_viewer(&self, id: &str) -> Result<Option<Viewer>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE id = ?1"),
                params![id],
                row_to_viewer,
            )
            .optional()?)
        })
    }

    /// The viewer whose public ID is `public_id`; `None` for anything else,
    /// including a cookie value.
    pub fn viewer_by_public_id(&self, public_id: &str) -> Result<Option<Viewer>> {
        if !is_public_id(public_id) {
            return Ok(None);
        }
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE public_id = ?1"),
                params![public_id],
                row_to_viewer,
            )
            .optional()?)
        })
    }
}

impl Store {
    /// The viewers with these public IDs, in the order given; unknown IDs,
    /// and anything that is not a public ID, are skipped.
    pub fn viewers_by_public_ids(&self, ids: &[String]) -> Result<Vec<Viewer>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!("{VIEWER_SELECT} WHERE public_id = ?1"))?;
            let mut out = Vec::new();
            for id in ids.iter().filter(|i| is_public_id(i)) {
                if let Some(v) = stmt.query_row(params![id], row_to_viewer).optional()? {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }

    /// Up to `limit` named viewers whose name contains `q` as literal text,
    /// ignoring case in every script (Unicode lowercase on both sides),
    /// ordered by name (then public ID). Reads at most [`MAX_SEARCH_SCAN`]
    /// named viewers, in that order: its cost is linear in the names read.
    pub fn search_viewers(&self, q: &str, limit: usize) -> Result<Vec<Viewer>> {
        let needle = q.to_lowercase();
        self.with_read(|c| {
            // SQLite's lower() folds ASCII only, so names are matched here,
            // streaming in name order and stopping at `limit` hits or
            // MAX_SEARCH_SCAN names read.
            let mut stmt = c.prepare(&format!(
                "{VIEWER_SELECT} WHERE display_name IS NOT NULL
                 ORDER BY display_name COLLATE NOCASE, public_id"
            ))?;
            let mut out = Vec::new();
            for v in stmt.query_map([], row_to_viewer)?.take(MAX_SEARCH_SCAN) {
                if out.len() >= limit {
                    break;
                }
                let v = v?;
                if v.display_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&needle))
                {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_NAME_CHARS, MAX_SEARCH_SCAN};
    use crate::store::test_util::store;
    use crate::{CoreError, new_ulid};

    #[test]
    fn upsert_creates_renames_and_clears() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert_eq!(v.display_name, None);
        assert_eq!(
            st.upsert_viewer(&id, Some("  Alex  "))
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alex")
        );
        assert_eq!(
            st.upsert_viewer(&id, None).unwrap().display_name.as_deref(),
            Some("Alex"),
            "None keeps the name"
        );
        assert_eq!(
            st.upsert_viewer(&id, Some("")).unwrap().display_name,
            None,
            "empty clears"
        );
        assert_eq!(
            st.get_viewer(&id).unwrap().unwrap().created_at,
            v.created_at
        );
    }

    #[test]
    fn a_viewer_has_a_stable_public_id_distinct_from_its_cookie() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert!(crate::is_public_id(&v.public_id), "{}", v.public_id);
        assert_ne!(v.public_id, id);
        assert_eq!(
            st.upsert_viewer(&id, Some("Alex")).unwrap().public_id,
            v.public_id
        );
        let other = st.upsert_viewer(&new_ulid(), None).unwrap();
        assert_ne!(other.public_id, v.public_id);
        let found = st.viewer_by_public_id(&v.public_id).unwrap().unwrap();
        assert_eq!(
            (found.id.as_str(), found.display_name.as_deref()),
            (id.as_str(), Some("Alex"))
        );
        assert_eq!(
            st.viewer_by_public_id("u_0123456789abcdef012345").unwrap(),
            None
        );
        assert_eq!(
            st.viewer_by_public_id(&id).unwrap(),
            None,
            "the cookie is not a public ID"
        );
        let json = serde_json::to_value(&v).unwrap();
        assert!(
            !json.to_string().contains(&id),
            "a serialised viewer never carries its cookie: {json}"
        );
        assert_eq!(json["public_id"], v.public_id.as_str());
    }

    #[test]
    fn bad_ids_and_names_are_refused() {
        let (_d, st) = store();
        assert!(matches!(
            st.upsert_viewer("not-a-ulid", None),
            Err(CoreError::Invalid {
                code: "invalid_viewer",
                ..
            })
        ));
        let id = new_ulid();
        for bad in ["a\nb".to_string(), "x".repeat(MAX_NAME_CHARS + 1)] {
            assert!(matches!(
                st.upsert_viewer(&id, Some(&bad)),
                Err(CoreError::Invalid {
                    code: "invalid_name",
                    ..
                })
            ));
        }
    }

    #[test]
    fn lookups_by_public_id_and_name_search() {
        let (_d, st) = store();
        let alex = st.upsert_viewer(&new_ulid(), Some("Alex Chen")).unwrap();
        let sam = st.upsert_viewer(&new_ulid(), Some("Sam")).unwrap();
        let anon = st.upsert_viewer(&new_ulid(), None).unwrap();
        let found: Vec<_> = st
            .viewers_by_public_ids(&[
                anon.public_id.clone(),
                "u_ffffffffffffffffffffff".into(),
                alex.public_id.clone(),
                "not a public ID".into(),
            ])
            .unwrap()
            .into_iter()
            .map(|v| v.public_id)
            .collect();
        assert_eq!(
            found,
            [anon.public_id.clone(), alex.public_id.clone()],
            "in the order asked, unknown and malformed IDs skipped"
        );
        let names: Vec<_> = st
            .search_viewers("A", 8)
            .unwrap()
            .into_iter()
            .map(|v| v.display_name.unwrap())
            .collect();
        assert_eq!(
            names,
            ["Alex Chen", "Sam"],
            "case-insensitive substring, by name, named only"
        );
        assert_eq!(
            st.search_viewers("chen", 8).unwrap()[0].public_id,
            alex.public_id
        );
        assert!(st.search_viewers("zz", 8).unwrap().is_empty());
        assert_eq!(st.search_viewers("a", 1).unwrap().len(), 1);
        let _ = sam;
    }

    #[test]
    fn search_folds_case_beyond_ascii_and_matches_wildcards_literally() {
        let (_d, st) = store();
        let umlaut = st.upsert_viewer(&new_ulid(), Some("Ärger")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("Arno")).unwrap();
        let pct = st.upsert_viewer(&new_ulid(), Some("100% sure")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("100 x sure")).unwrap();
        let under = st.upsert_viewer(&new_ulid(), Some("a_b")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("axb")).unwrap();
        let ids = |q: &str| -> Vec<String> {
            st.search_viewers(q, 8)
                .unwrap()
                .into_iter()
                .map(|v| v.public_id)
                .collect()
        };
        assert_eq!(ids("ärger"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("ÄRGER"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("0%"), std::slice::from_ref(&pct.public_id));
        assert_eq!(ids("a_b"), std::slice::from_ref(&under.public_id));
        assert!(ids("%").len() == 1 && ids("_").len() == 1);
    }
    #[test]
    fn search_scans_at_most_max_search_scan_names() {
        assert_eq!(MAX_SEARCH_SCAN, 10_000);
        let (_d, st) = store();
        // MAX_SEARCH_SCAN names sorting before one more, "zed", which lies past the scan.
        st.with_write(|c| {
            let tx = c.unchecked_transaction()?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO viewers (id, public_id, display_name, created_at)
                     VALUES (?1, 'u_' || lower(hex(randomblob(11))), ?2, '2026-01-01T00:00:00Z')",
                )?;
                for i in 0..=MAX_SEARCH_SCAN {
                    let name = if i == MAX_SEARCH_SCAN {
                        "zed".to_string()
                    } else {
                        format!("a{i:05}")
                    };
                    ins.execute(rusqlite::params![format!("v{i}"), name])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            st.search_viewers("a09999", 8).unwrap().len(),
            1,
            "the last scanned name is found"
        );
        assert!(
            st.search_viewers("zed", 8).unwrap().is_empty(),
            "a name past the scan is not reached"
        );
    }
}
