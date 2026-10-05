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

/// What [`Store::claim_for_owner`] did with a browser's viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// The cookie names no viewer, or the owner's own.
    Nothing,
    /// There was no owner yet: this viewer became the owner, keeping its
    /// public ID, name and history.
    Adopted,
    /// The viewer was folded into the existing owner and removed; its public
    /// ID (given here) names no one from now on.
    Merged(String),
}

fn owner_row(tx: &rusqlite::Connection) -> rusqlite::Result<Option<Viewer>> {
    tx.query_row(
        &format!("{VIEWER_SELECT} WHERE owner = 1"),
        [],
        row_to_viewer,
    )
    .optional()
}

impl Store {
    /// The owner identity: the one viewer that every owner credential (the
    /// bearer token, the owner and events cookies of the owner's browsers)
    /// acts as. Made on first use, with a new public ID and no name; its
    /// `id` is never handed to anyone as a cookie.
    pub fn owner_viewer(&self) -> Result<Viewer> {
        if let Some(v) = self.with_read(|c| Ok(owner_row(c)?))? {
            return Ok(v);
        }
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, owner)
                 SELECT ?1, ?2, NULL, ?3, 1 WHERE NOT EXISTS (SELECT 1 FROM viewers WHERE owner = 1)",
                params![crate::new_ulid(), new_public_id(), Store::now()],
            )?;
            Ok(owner_row(tx)?.expect("the owner row was just made"))
        })
    }

    /// Whether `id` (a cookie value) names the owner's viewer row.
    pub fn is_owner_viewer(&self, id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT owner FROM viewers WHERE id = ?1",
                params![id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }

    /// Claims the viewer behind `cookie` for the owner, on a request that
    /// carries both that cookie and an owner credential (so the cookie's
    /// browser is the owner's). With no owner yet the viewer becomes it
    /// ([`Claim::Adopted`]). Otherwise its history moves to the owner and the
    /// row goes ([`Claim::Merged`]): its comments' author, its mentions, the
    /// threads it resolved, its seen marks (the higher of the two, pruned to
    /// [`crate::changelog::MAX_SEEN_PER_VIEWER`]) and its looked-at marks
    /// (the later), and its name when the owner has none. Page data that
    /// holds the old public ID is not rewritten.
    pub fn claim_for_owner(&self, cookie: &str) -> Result<Claim> {
        self.with_tx(|tx| {
            let Some((legacy, is_owner)) = tx
                .query_row(
                    "SELECT id, public_id, display_name, created_at, owner FROM viewers WHERE id = ?1",
                    params![cookie],
                    |r| Ok((row_to_viewer(r)?, r.get::<_, bool>(4)?)),
                )
                .optional()?
            else {
                return Ok(Claim::Nothing);
            };
            if is_owner {
                return Ok(Claim::Nothing);
            }
            let Some(owner) = owner_row(tx)? else {
                tx.execute("UPDATE viewers SET owner = 1 WHERE id = ?1", params![legacy.id])?;
                return Ok(Claim::Adopted);
            };
            let (old, new) = (legacy.public_id.as_str(), owner.public_id.as_str());
            tx.execute(
                "UPDATE comments SET author_public_id = ?2 WHERE author_public_id = ?1",
                params![old, new],
            )?;
            tx.execute(
                "UPDATE OR IGNORE mentions SET public_id = ?2 WHERE public_id = ?1",
                params![old, new],
            )?;
            tx.execute("DELETE FROM mentions WHERE public_id = ?1", params![old])?;
            tx.execute(
                "UPDATE threads SET resolved_by = 'viewer:' || ?2 WHERE resolved_by = 'viewer:' || ?1",
                params![old, new],
            )?;
            tx.execute(
                "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at)
                 SELECT ?2, artifact_id, seen_n, updated_at FROM viewer_seen WHERE viewer_id = ?1 AND true
                 ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET
                   seen_n = MAX(seen_n, excluded.seen_n), updated_at = MAX(updated_at, excluded.updated_at)",
                params![legacy.id, owner.id],
            )?;
            tx.execute("DELETE FROM viewer_seen WHERE viewer_id = ?1", params![legacy.id])?;
            tx.execute(
                "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
                   (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
                params![owner.id, crate::changelog::MAX_SEEN_PER_VIEWER as i64],
            )?;
            tx.execute(
                "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at)
                 SELECT ?2, thread_id, looked_at FROM viewer_threads WHERE viewer_id = ?1 AND true
                 ON CONFLICT (viewer_id, thread_id) DO UPDATE SET looked_at = MAX(looked_at, excluded.looked_at)",
                params![legacy.id, owner.id],
            )?;
            tx.execute("DELETE FROM viewer_threads WHERE viewer_id = ?1", params![legacy.id])?;
            if owner.display_name.is_none() && legacy.display_name.is_some() {
                tx.execute(
                    "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                    params![owner.id, legacy.display_name],
                )?;
            }
            tx.execute("DELETE FROM viewers WHERE id = ?1", params![legacy.id])?;
            Ok(Claim::Merged(legacy.public_id))
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
    use super::{Claim, MAX_NAME_CHARS, MAX_SEARCH_SCAN};
    use crate::store::test_util::store;
    use crate::{CoreError, new_ulid};

    #[test]
    fn the_owner_is_one_viewer_made_on_first_use() {
        let (_d, st) = store();
        let a = st.owner_viewer().unwrap();
        let b = st.owner_viewer().unwrap();
        assert_eq!(a, b);
        assert!(st.is_owner_viewer(&a.id).unwrap());
        let other = st.upsert_viewer(&new_ulid(), Some("Sam")).unwrap();
        assert!(!st.is_owner_viewer(&other.id).unwrap());
        assert!(!st.is_owner_viewer("nope").unwrap());
        st.upsert_viewer(&a.id, Some("Alex")).unwrap();
        assert_eq!(
            st.owner_viewer().unwrap().display_name.as_deref(),
            Some("Alex")
        );
    }

    #[test]
    fn the_first_claimed_viewer_becomes_the_owner() {
        let (_d, st) = store();
        let chrome = st.upsert_viewer(&new_ulid(), Some("Alex")).unwrap();
        assert_eq!(st.claim_for_owner(&chrome.id).unwrap(), Claim::Adopted);
        let owner = st.owner_viewer().unwrap();
        assert_eq!(
            (owner.public_id.as_str(), owner.display_name.as_deref()),
            (chrome.public_id.as_str(), Some("Alex")),
            "the adopted viewer keeps its public ID and name"
        );
        assert_eq!(st.claim_for_owner(&chrome.id).unwrap(), Claim::Nothing);
        assert_eq!(st.claim_for_owner(&new_ulid()).unwrap(), Claim::Nothing);
    }

    #[test]
    fn a_later_claimed_viewer_merges_into_the_owner() {
        use crate::store::test_util::{anchor, artifact};
        use crate::store::threads::NewThread;
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let owner = st.owner_viewer().unwrap();
        let safari = st.upsert_viewer(&new_ulid(), Some("Alex S")).unwrap();
        let t = st
            .create_thread(
                &aid,
                NewThread {
                    author_public_id: Some(safari.public_id.clone()),
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Alex S".into(),
                    body: "hi".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        st.resolve_thread(&t.id, &format!("viewer:{}", safari.public_id))
            .unwrap();
        st.mark_seen(&safari.id, &aid, 1).unwrap();
        st.mark_looked(&safari.id, &aid, std::slice::from_ref(&t.id))
            .unwrap();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO mentions (comment_id, public_id) SELECT id, ?1 FROM comments",
                rusqlite::params![safari.public_id],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            st.claim_for_owner(&safari.id).unwrap(),
            Claim::Merged(safari.public_id.clone())
        );
        assert_eq!(st.get_viewer(&safari.id).unwrap(), None, "the row is gone");
        let now = st.owner_viewer().unwrap();
        assert_eq!(now.public_id, owner.public_id);
        assert_eq!(
            now.display_name.as_deref(),
            Some("Alex S"),
            "an unnamed owner takes the name"
        );
        assert_eq!(st.seen(&owner.id, &aid).unwrap(), Some(1));
        let (author, resolved_by, mention, looked): (String, String, String, i64) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT author_public_id FROM comments), (SELECT resolved_by FROM threads),
                            (SELECT public_id FROM mentions),
                            (SELECT COUNT(*) FROM viewer_threads WHERE viewer_id = ?1)",
                    rusqlite::params![owner.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(author, owner.public_id);
        assert_eq!(resolved_by, format!("viewer:{}", owner.public_id));
        assert_eq!(mention, owner.public_id);
        assert_eq!(looked, 1);
        // A named owner keeps its name.
        let laptop = st.upsert_viewer(&new_ulid(), Some("Other")).unwrap();
        st.claim_for_owner(&laptop.id).unwrap();
        assert_eq!(
            st.owner_viewer().unwrap().display_name.as_deref(),
            Some("Alex S")
        );
    }

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
