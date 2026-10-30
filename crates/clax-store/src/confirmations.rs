//! Browser confirmations of the decisions that matter: while an agent
//! session is live, a decision made with the bare daemon token waits for a
//! click in the owner's browser (`clax_http::confirm`).
//!
//! - A **confirmation** ([`Store::open_confirmation`]) is bound to one
//!   decision (`decision_json`, the decision as `clax_http::confirm`
//!   writes it) and is `open` until the owner approves or declines it
//!   ([`Store::answer_confirmation`]), or [`CONFIRMATION_TTL_SECS`] pass
//!   from when it was made, after which it reads as `expired`.
//! - An approved confirmation lets one call of its own decision through
//!   ([`Store::use_confirmation`]), once: it is then spent (`used_at`).
//! - [`Store::agent_session_live`] says whether any agent session has not
//!   ended: what makes a token's decision wait at all.
//!
//! Times are the store's clock's, so tests move them with a manual clock.

use super::Store;
use chrono::Duration;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;

/// How long a confirmation may be answered and used, from when it was made.
pub const CONFIRMATION_TTL_SECS: i64 = 600;

/// A confirmation as the store keeps it. `status` is `open`, `approved`,
/// `declined` or `expired`; an open one past [`CONFIRMATION_TTL_SECS`]
/// reads as `expired`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Confirmation {
    pub id: String,
    pub decision: serde_json::Value,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub answered_at: Option<String>,
    pub used_at: Option<String>,
}

/// What presenting a confirmation with a decision came to
/// ([`Store::use_confirmation`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Use {
    /// Approved for this decision: spent now, the decision goes ahead.
    Approved,
    /// Not answered yet.
    Pending(Confirmation),
    /// The owner declined it.
    Declined,
    /// Past its time, unanswered or unused.
    Expired,
    /// Spent before.
    Used,
    /// Bound to another decision: it lets this one through never.
    Mismatch,
    /// No confirmation has this ID.
    Unknown,
}

const SELECT: &str =
    "SELECT id, decision_json, status, created_at, answered_at, used_at FROM confirmations";

impl Store {
    /// Whether any agent session has not ended.
    ///
    /// # Errors
    /// The store's.
    pub fn agent_session_live(&self) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM sessions WHERE ended_at IS NULL)",
                [],
                |r| r.get(0),
            )?)
        })
    }

    fn confirmation_of(&self, r: &Row<'_>) -> rusqlite::Result<Confirmation> {
        let created_at: String = r.get(3)?;
        let decision: String = r.get(1)?;
        let mut status: String = r.get(2)?;
        let expires_at = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|t| {
                clax_core::time::stamp(
                    t.with_timezone(&chrono::Utc) + Duration::seconds(CONFIRMATION_TTL_SECS),
                )
            })
            .unwrap_or_default();
        let used_at: Option<String> = r.get(5)?;
        // Unanswered or approved but unused, past its time: expired.
        if (status == "open" || (status == "approved" && used_at.is_none()))
            && expires_at <= self.now()
        {
            status = "expired".into();
        }
        Ok(Confirmation {
            id: r.get(0)?,
            decision: serde_json::from_str(&decision).unwrap_or(serde_json::Value::Null),
            status,
            created_at,
            expires_at,
            answered_at: r.get(4)?,
            used_at,
        })
    }

    /// The confirmation open for `decision` (its canonical JSON), or a new
    /// one. Confirmations past their time are marked `expired` first.
    ///
    /// # Errors
    /// The store's.
    pub fn open_confirmation(&self, decision: &serde_json::Value) -> Result<Confirmation> {
        let json = decision.to_string();
        let id = new_id();
        self.with_tx(|tx| {
            self.expire_confirmations(tx)?;
            let open: Option<String> = tx
                .query_row(
                    "SELECT id FROM confirmations WHERE decision_json = ?1 AND status = 'open'",
                    params![json],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match open {
                Some(id) => id,
                None => {
                    tx.execute(
                        "INSERT INTO confirmations (id, decision_json, status, created_at)
                         VALUES (?1, ?2, 'open', ?3)",
                        params![id, json, self.now()],
                    )?;
                    id.clone()
                }
            };
            tx.query_row(&format!("{SELECT} WHERE id = ?1"), params![id], |r| {
                self.confirmation_of(r)
            })
            .map_err(Into::into)
        })
    }

    /// Marks every confirmation past its time and not spent `expired`.
    fn expire_confirmations(&self, tx: &rusqlite::Transaction<'_>) -> Result<()> {
        let cutoff =
            clax_core::time::stamp(self.now_utc() - Duration::seconds(CONFIRMATION_TTL_SECS));
        tx.execute(
            "UPDATE confirmations SET status = 'expired'
             WHERE created_at <= ?1 AND used_at IS NULL AND status IN ('open', 'approved')",
            params![cutoff],
        )?;
        Ok(())
    }

    /// Confirmation `id`, as it reads now.
    ///
    /// # Errors
    /// The store's.
    pub fn confirmation(&self, id: &str) -> Result<Option<Confirmation>> {
        self.with_read(|c| {
            Ok(
                c.query_row(&format!("{SELECT} WHERE id = ?1"), params![id], |r| {
                    self.confirmation_of(r)
                })
                .optional()?,
            )
        })
    }

    /// The confirmations still open, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn open_confirmations(&self) -> Result<Vec<Confirmation>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE status = 'open' ORDER BY created_at, id"
            ))?;
            let rows = stmt
                .query_map([], |r| self.confirmation_of(r))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().filter(|c| c.status == "open").collect())
        })
    }

    /// Approves (`approve`) or declines open confirmation `id`: it as it
    /// is after.
    ///
    /// # Errors
    /// `NotFound` for no such confirmation; `confirmation_expired` for one
    /// past its time; `confirmation_answered` for one answered before.
    pub fn answer_confirmation(&self, id: &str, approve: bool) -> Result<Confirmation> {
        self.with_tx(|tx| {
            self.expire_confirmations(tx)?;
            let c = tx
                .query_row(&format!("{SELECT} WHERE id = ?1"), params![id], |r| {
                    self.confirmation_of(r)
                })
                .optional()?
                .ok_or(CoreError::NotFound)?;
            match c.status.as_str() {
                "open" => {}
                "expired" => {
                    return Err(CoreError::invalid(
                        "confirmation_expired",
                        "this confirmation has expired: make the call again for a new one",
                    ));
                }
                _ => {
                    return Err(CoreError::invalid(
                        "confirmation_answered",
                        "this confirmation was answered already",
                    ));
                }
            }
            let status = if approve { "approved" } else { "declined" };
            tx.execute(
                "UPDATE confirmations SET status = ?2, answered_at = ?3 WHERE id = ?1",
                params![id, status, self.now()],
            )?;
            tx.query_row(&format!("{SELECT} WHERE id = ?1"), params![id], |r| {
                self.confirmation_of(r)
            })
            .map_err(Into::into)
        })
    }

    /// Presents confirmation `id` for `decision` (its canonical JSON): an
    /// approved one bound to that very decision is spent and lets it
    /// through ([`Use::Approved`]), once; anything else says why not.
    ///
    /// # Errors
    /// The store's.
    pub fn use_confirmation(&self, id: &str, decision: &serde_json::Value) -> Result<Use> {
        self.with_tx(|tx| {
            let Some(c) = tx
                .query_row(&format!("{SELECT} WHERE id = ?1"), params![id], |r| {
                    self.confirmation_of(r)
                })
                .optional()?
            else {
                return Ok(Use::Unknown);
            };
            if &c.decision != decision {
                return Ok(Use::Mismatch);
            }
            if c.used_at.is_some() {
                return Ok(Use::Used);
            }
            Ok(match c.status.as_str() {
                "approved" => {
                    tx.execute(
                        "UPDATE confirmations SET used_at = ?2 WHERE id = ?1",
                        params![id, self.now()],
                    )?;
                    Use::Approved
                }
                "declined" => Use::Declined,
                "expired" => Use::Expired,
                _ => Use::Pending(c),
            })
        })
    }
}

/// A confirmation's ID: `cf_` and 32 lowercase hex digits (16 random bytes).
fn new_id() -> String {
    let b = clax_core::secret::random_bytes::<16>();
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("cf_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::working::ManualClock;
    use serde_json::json;
    use std::sync::Arc;

    fn store(clock: Arc<ManualClock>) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        let st = Store::open_with_clock(&home, clock).unwrap();
        (dir, st)
    }

    #[test]
    fn an_approved_confirmation_lets_its_own_decision_through_once() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (_d, st) = store(clock);
        let push = json!({"kind": "push", "artifact_id": "a", "remote": "r"});
        let other = json!({"kind": "push", "artifact_id": "b", "remote": "r"});
        let c = st.open_confirmation(&push).unwrap();
        assert!(c.id.starts_with("cf_"));
        assert_eq!(c.status, "open");
        // The same decision asks with the same confirmation while it is open.
        assert_eq!(st.open_confirmation(&push).unwrap().id, c.id);
        assert!(matches!(
            st.use_confirmation(&c.id, &push).unwrap(),
            Use::Pending(_)
        ));
        assert_eq!(st.open_confirmations().unwrap(), vec![c.clone()]);
        st.answer_confirmation(&c.id, true).unwrap();
        assert!(st.open_confirmations().unwrap().is_empty());
        assert_eq!(st.use_confirmation(&c.id, &other).unwrap(), Use::Mismatch);
        assert_eq!(st.use_confirmation(&c.id, &push).unwrap(), Use::Approved);
        assert_eq!(st.use_confirmation(&c.id, &push).unwrap(), Use::Used);
        assert_eq!(st.use_confirmation("cf_none", &push).unwrap(), Use::Unknown);
        let err = st.answer_confirmation(&c.id, false).unwrap_err();
        assert!(err.to_string().contains("answered"), "{err}");
    }

    #[test]
    fn a_confirmation_expires_after_ten_minutes_by_the_stores_clock() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let d = json!({"kind": "answer", "question_id": "q"});
        let declined = st.open_confirmation(&json!({"kind": "x"})).unwrap();
        st.answer_confirmation(&declined.id, false).unwrap();
        assert_eq!(
            st.use_confirmation(&declined.id, &json!({"kind": "x"}))
                .unwrap(),
            Use::Declined
        );
        let c = st.open_confirmation(&d).unwrap();
        clock.advance(CONFIRMATION_TTL_SECS - 1);
        assert_eq!(st.confirmation(&c.id).unwrap().unwrap().status, "open");
        clock.advance(1);
        assert_eq!(st.confirmation(&c.id).unwrap().unwrap().status, "expired");
        assert_eq!(st.use_confirmation(&c.id, &d).unwrap(), Use::Expired);
        let err = st.answer_confirmation(&c.id, true).unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
        // An approved one left unused expires too, and a new call asks anew.
        let again = st.open_confirmation(&d).unwrap();
        assert_ne!(again.id, c.id);
        st.answer_confirmation(&again.id, true).unwrap();
        clock.advance(CONFIRMATION_TTL_SECS);
        assert_eq!(st.use_confirmation(&again.id, &d).unwrap(), Use::Expired);
    }
}
