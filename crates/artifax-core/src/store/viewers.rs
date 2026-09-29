//! Browser viewers, keyed by the `artifax_viewer` cookie (a ULID, the
//! viewer's credential) and named to others by a public ID.

use super::Store;
use crate::ids::{is_public_id, is_ulid, new_public_id};
use crate::model::Viewer;
use crate::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Longest accepted display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

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
        self.with_conn(|c| {
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
        self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE public_id = ?1"),
                params![public_id],
                row_to_viewer,
            )
            .optional()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::MAX_NAME_CHARS;
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
}
