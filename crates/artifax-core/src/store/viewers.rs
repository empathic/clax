//! Browser viewers, keyed by the `artifax_viewer` cookie (a ULID).

use super::Store;
use crate::ids::is_ulid;
use crate::model::Viewer;
use crate::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Longest accepted display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

impl Store {
    /// Creates viewer `id` when missing. `display_name`: `None` keeps the
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
                "INSERT INTO viewers (id, display_name, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET display_name = CASE WHEN ?4 THEN excluded.display_name ELSE display_name END",
                params![id, stored, Store::now(), name.is_some()],
            )?;
            Ok(tx.query_row("SELECT id, display_name, created_at FROM viewers WHERE id = ?1", params![id], |r| {
                Ok(Viewer { id: r.get(0)?, display_name: r.get(1)?, created_at: r.get(2)? })
            })?)
        })
    }

    pub fn get_viewer(&self, id: &str) -> Result<Option<Viewer>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT id, display_name, created_at FROM viewers WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Viewer {
                        id: r.get(0)?,
                        display_name: r.get(1)?,
                        created_at: r.get(2)?,
                    })
                },
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
