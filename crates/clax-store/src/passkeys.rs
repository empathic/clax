//! The owner's passkeys for browser confirmations (`clax_http::confirm`):
//! when the owner requires them, confirming a decision also takes a
//! WebAuthn assertion with user verification ([`clax_core::webauthn`]).
//!
//! - [`Store::passkeys`], [`Store::add_passkey`], [`Store::remove_passkey`]
//!   and [`Store::use_passkey`] (which checks an assertion against the
//!   stored key and counter, and keeps the new counter, in one write).
//! - A **challenge** ([`Store::new_passkey_challenge`]) is 32 random bytes,
//!   base64url, bound to its purpose (`confirm:<confirmation ID>`,
//!   `register`, `disable`, `remove:<credential ID>`), good for
//!   [`CHALLENGE_TTL_SECS`] and taken once ([`Store::take_passkey_challenge`]).
//! - [`Store::confirm_prefs`]: whether passkeys are required, and whether
//!   the owner put off the first-use offer. Required takes effect only
//!   while a passkey is registered ([`Store::passkeys_required`]).

use super::Store;
use chrono::Duration;
use clax_core::webauthn::{Refusal, Registered};
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// How long a challenge may be answered, from when it was issued.
pub const CHALLENGE_TTL_SECS: i64 = 600;

/// A registered passkey: never its key. `added_by` says how it was
/// added (`{via, agents_live}`: from which browser, and the owner's agents
/// live then), so one the owner did not add shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Passkey {
    pub credential_id: String,
    pub alg: i64,
    pub created_at: String,
    pub added_by: serde_json::Value,
}

/// The owner's confirmation preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConfirmPrefs {
    /// Confirming takes a passkey's assertion (once one is registered).
    pub require_passkey: bool,
    /// The first confirmation card's offer to set up a passkey was put off.
    pub offer_dismissed: bool,
}

impl Store {
    /// The registered passkeys, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn passkeys(&self) -> Result<Vec<Passkey>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT credential_id, alg, created_at, added_by_json FROM passkeys
                 ORDER BY created_at, credential_id",
            )?;
            Ok(stmt
                .query_map([], |r| {
                    let by: String = r.get(3)?;
                    Ok(Passkey {
                        credential_id: r.get(0)?,
                        alg: r.get(1)?,
                        created_at: r.get(2)?,
                        added_by: serde_json::from_str(&by).unwrap_or_default(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Registers the checked passkey `r`, added as `added_by` says.
    ///
    /// # Errors
    /// `passkey_exists` for a credential registered already; the store's.
    pub fn add_passkey(&self, r: &Registered, added_by: &serde_json::Value) -> Result<Passkey> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "INSERT OR IGNORE INTO passkeys
                   (credential_id, public_key, alg, sign_count, created_at, added_by_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    r.credential_id,
                    r.public_key,
                    r.alg,
                    r.sign_count,
                    self.now(),
                    added_by.to_string()
                ],
            )?;
            if n == 0 {
                return Err(CoreError::invalid(
                    "passkey_exists",
                    "this passkey is registered already",
                ));
            }
            Ok(Passkey {
                credential_id: r.credential_id.clone(),
                alg: r.alg,
                created_at: self.now(),
                added_by: added_by.clone(),
            })
        })
    }

    /// Removes passkey `id`: true when one was registered.
    ///
    /// # Errors
    /// The store's.
    pub fn remove_passkey(&self, id: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute("DELETE FROM passkeys WHERE credential_id = ?1", params![id])? > 0)
        })
    }

    /// Checks an assertion by passkey `id` with `check`, which is handed
    /// its public key, algorithm and stored counter and answers the new
    /// counter; keeps that counter. All in one write, so two assertions
    /// never both pass one counter.
    ///
    /// # Errors
    /// `NotFound` for no such passkey; `passkey_refused` with `check`'s
    /// reason; the store's.
    pub fn use_passkey(
        &self,
        id: &str,
        check: impl FnOnce(&[u8], i64, u32) -> std::result::Result<u32, Refusal>,
    ) -> Result<()> {
        self.with_tx(|tx| {
            let (key, alg, count): (Vec<u8>, i64, u32) = tx
                .query_row(
                    "SELECT public_key, alg, sign_count FROM passkeys WHERE credential_id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            let next = check(&key, alg, count)
                .map_err(|why| CoreError::invalid("passkey_refused", why))?;
            tx.execute(
                "UPDATE passkeys SET sign_count = ?2 WHERE credential_id = ?1",
                params![id, next],
            )?;
            Ok(())
        })
    }

    /// A new challenge for `purpose`. Challenges past their time are
    /// dropped first.
    ///
    /// # Errors
    /// The store's.
    pub fn new_passkey_challenge(&self, purpose: &str) -> Result<String> {
        let challenge = clax_core::secret::base64url(&clax_core::secret::random_bytes::<32>());
        self.with_tx(|tx| {
            let cutoff =
                clax_core::time::stamp(self.now_utc() - Duration::seconds(CHALLENGE_TTL_SECS));
            tx.execute(
                "DELETE FROM passkey_challenges WHERE created_at <= ?1",
                params![cutoff],
            )?;
            tx.execute(
                "INSERT INTO passkey_challenges (challenge, purpose, created_at) VALUES (?1, ?2, ?3)",
                params![challenge, purpose, self.now()],
            )?;
            Ok(())
        })?;
        Ok(challenge)
    }

    /// Takes challenge `challenge` for `purpose`: true when it was issued
    /// for that purpose and is within its time. A challenge presented is
    /// spent whatever the answer, so none is answered twice.
    ///
    /// # Errors
    /// The store's.
    pub fn take_passkey_challenge(&self, challenge: &str, purpose: &str) -> Result<bool> {
        self.with_tx(|tx| {
            let found: Option<(String, String)> = tx
                .query_row(
                    "SELECT purpose, created_at FROM passkey_challenges WHERE challenge = ?1",
                    params![challenge],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            tx.execute(
                "DELETE FROM passkey_challenges WHERE challenge = ?1",
                params![challenge],
            )?;
            let cutoff =
                clax_core::time::stamp(self.now_utc() - Duration::seconds(CHALLENGE_TTL_SECS));
            Ok(found.is_some_and(|(p, at)| p == purpose && at > cutoff))
        })
    }

    /// The owner's confirmation preferences.
    ///
    /// # Errors
    /// The store's.
    pub fn confirm_prefs(&self) -> Result<ConfirmPrefs> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT require_passkey, offer_dismissed FROM confirm_prefs WHERE id = 1",
                [],
                |r| {
                    Ok(ConfirmPrefs {
                        require_passkey: r.get(0)?,
                        offer_dismissed: r.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
        })
    }

    /// Sets the owner's confirmation preferences to `p`.
    ///
    /// # Errors
    /// The store's.
    pub fn set_confirm_prefs(&self, p: ConfirmPrefs) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO confirm_prefs (id, require_passkey, offer_dismissed) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET require_passkey = ?1, offer_dismissed = ?2",
                params![p.require_passkey, p.offer_dismissed],
            )?;
            Ok(())
        })
    }

    /// Removes every passkey and turns the requirement off: the offline
    /// recovery (`clax passkeys reset`) for an owner locked out by a
    /// passkey they cannot use. The passkeys removed.
    ///
    /// # Errors
    /// The store's.
    pub fn reset_passkeys(&self) -> Result<Vec<Passkey>> {
        let removed = self.passkeys()?;
        self.with_tx(|tx| {
            tx.execute("DELETE FROM passkeys", [])?;
            tx.execute("DELETE FROM passkey_challenges", [])?;
            tx.execute("UPDATE confirm_prefs SET require_passkey = 0", [])?;
            Ok(())
        })?;
        Ok(removed)
    }

    /// Whether confirming takes a passkey's assertion now: required, and a
    /// passkey registered.
    ///
    /// # Errors
    /// The store's.
    pub fn passkeys_required(&self) -> Result<bool> {
        Ok(self.confirm_prefs()?.require_passkey && !self.passkeys()?.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn store(clock: Arc<ManualClock>) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        let st = Store::open_with_clock(&home, clock).unwrap();
        (dir, st)
    }

    #[test]
    fn a_challenge_is_taken_once_for_its_own_purpose_within_its_time() {
        let clock = Arc::new(ManualClock::at("2026-10-30T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let c = st.new_passkey_challenge("confirm:cf_a").unwrap();
        assert!(
            !st.take_passkey_challenge(&c, "confirm:cf_b").unwrap(),
            "another purpose"
        );
        assert!(
            !st.take_passkey_challenge(&c, "confirm:cf_a").unwrap(),
            "spent by the try"
        );
        let c = st.new_passkey_challenge("register").unwrap();
        assert!(st.take_passkey_challenge(&c, "register").unwrap());
        assert!(!st.take_passkey_challenge(&c, "register").unwrap(), "once");
        let late = st.new_passkey_challenge("register").unwrap();
        clock.advance(CHALLENGE_TTL_SECS);
        assert!(
            !st.take_passkey_challenge(&late, "register").unwrap(),
            "expired"
        );
    }

    #[test]
    fn requiring_passkeys_takes_effect_only_with_one_registered() {
        let clock = Arc::new(ManualClock::at("2026-10-30T10:00:00Z"));
        let (_d, st) = store(clock);
        assert_eq!(st.confirm_prefs().unwrap(), ConfirmPrefs::default());
        st.set_confirm_prefs(ConfirmPrefs {
            require_passkey: true,
            offer_dismissed: true,
        })
        .unwrap();
        assert!(!st.passkeys_required().unwrap());
        let r = Registered {
            credential_id: "c1".into(),
            public_key: vec![1, 2],
            alg: clax_core::webauthn::ES256,
            sign_count: 4,
        };
        let by = serde_json::json!({"via": "a browser session", "agents_live": []});
        st.add_passkey(&r, &by).unwrap();
        assert!(st.add_passkey(&r, &by).is_err(), "once");
        assert_eq!(st.passkeys().unwrap()[0].added_by, by);
        assert!(st.passkeys_required().unwrap());
        st.use_passkey("c1", |key, alg, n| {
            assert_eq!((key, alg, n), (&[1u8, 2][..], -7, 4));
            Ok(5)
        })
        .unwrap();
        let refused = st.use_passkey("c1", |_, _, n| {
            assert_eq!(n, 5);
            Err("no")
        });
        assert!(refused.unwrap_err().to_string().contains("no"));
        assert!(st.remove_passkey("c1").unwrap());
        assert!(!st.passkeys_required().unwrap());
    }
}
