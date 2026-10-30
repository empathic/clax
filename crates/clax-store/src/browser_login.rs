//! How a browser on a local daemon becomes the owner's: a one-use login
//! code, made by a holder of the daemon token, opens an owner browser
//! session for the browser that redeems it.
//!
//! - A **login code** ([`Store::create_login_code`]) is [`LOGIN_CODE_PREFIX`]
//!   and 32 random bytes, good for [`LOGIN_CODE_TTL_SECS`] and redeemable
//!   once ([`Store::redeem_login_code`]).
//! - An **owner browser session** ([`Store::open_owner_session`]) is
//!   [`OWNER_SESSION_PREFIX`] and 32 random bytes, the value of the owner
//!   and events cookies, good for [`OWNER_SESSION_TTL_SECS`] from when it
//!   opened unless ended first ([`Store::end_owner_session`]). It names no
//!   viewer row: it is the owner's whatever row the owner is.
//!
//! A code minted while an agent session was live
//! ([`Store::agent_session_live`]) says so, and so does the session it
//! opens ([`LiveOwnerSession::minted_while_agent`]): an agent holding the
//! token could have minted it for a browser it drives, so such a session
//! confirms no decision (`clax_http::confirm`).
//!
//! Only each secret's SHA-256 is stored. A hosted daemon writes none of
//! these: its browsers sign in through [`crate::hosted`].

use super::Store;
use chrono::Duration;
use clax_core::Result;
use clax_core::secret::{self, Secret, sha256_hex};
use rusqlite::{OptionalExtension, params};

/// How long a login code may be redeemed, from when it was made.
pub const LOGIN_CODE_TTL_SECS: i64 = 120;
/// How long an owner browser session lasts, from when it opened.
pub const OWNER_SESSION_TTL_SECS: i64 = 400 * 24 * 3600;
/// Prefix of a login code.
pub const LOGIN_CODE_PREFIX: &str = "cxi_";
/// Prefix of an owner browser session's cookie value.
pub const OWNER_SESSION_PREFIX: &str = "cxb_";

/// A live owner browser session, as the daemon keeps it in memory: its
/// cookie's SHA-256 (lowercase hex), when it expires, and whether the
/// login code that opened it was minted while an agent session was live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveOwnerSession {
    pub sha256: String,
    pub expires_at: String,
    pub minted_while_agent: bool,
}

/// A login code spent ([`Store::redeem_login_code`]): whether it was
/// minted while an agent session was live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Redeemed {
    pub minted_while_agent: bool,
}

impl Store {
    fn after(&self, secs: i64) -> String {
        clax_core::time::stamp(self.now_utc() + Duration::seconds(secs))
    }

    /// A new login code, good for [`LOGIN_CODE_TTL_SECS`], marked as
    /// minted while an agent session was live when one was. Codes past
    /// their time or spent are dropped first.
    ///
    /// # Errors
    /// The store's.
    pub fn create_login_code(&self) -> Result<Secret> {
        let code = secret::token(LOGIN_CODE_PREFIX);
        self.with_tx(|tx| {
            let now = self.now();
            tx.execute(
                "DELETE FROM login_codes WHERE expires_at <= ?1 OR used_at IS NOT NULL",
                params![now],
            )?;
            let agent = crate::confirmations::agent_live_in(tx, self.now_utc())?;
            tx.execute(
                "INSERT INTO login_codes (sha256, created_at, expires_at, minted_while_agent)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    code.sha256_hex(),
                    now,
                    self.after(LOGIN_CODE_TTL_SECS),
                    agent
                ],
            )?;
            Ok(())
        })?;
        Ok(code)
    }

    /// Spends login code `code`: [`Redeemed`] when it was live (made,
    /// unspent, and within [`LOGIN_CODE_TTL_SECS`]); `None` for one spent
    /// before, past its time, or never made (one answer for all). A live
    /// code presented is spent whatever happens next.
    ///
    /// # Errors
    /// The store's.
    pub fn redeem_login_code(&self, code: &str) -> Result<Option<Redeemed>> {
        let hash = sha256_hex(code);
        self.with_tx(|tx| {
            let now = self.now();
            let found: Option<(String, bool)> = tx
                .query_row(
                    "SELECT expires_at, minted_while_agent FROM login_codes
                     WHERE sha256 = ?1 AND used_at IS NULL",
                    params![hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((expires, minted_while_agent)) = found else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE login_codes SET used_at = ?2 WHERE sha256 = ?1",
                params![hash, now],
            )?;
            Ok((expires > now).then_some(Redeemed { minted_while_agent }))
        })
    }

    /// Opens an owner browser session, marked `minted_while_agent` when
    /// the login code that opens it was: its cookie value, and its row's
    /// [`LiveOwnerSession`]. Sessions ended or past their time are dropped
    /// first.
    ///
    /// # Errors
    /// The store's.
    pub fn open_owner_session(
        &self,
        minted_while_agent: bool,
    ) -> Result<(Secret, LiveOwnerSession)> {
        let session = secret::token(OWNER_SESSION_PREFIX);
        let live = LiveOwnerSession {
            sha256: session.sha256_hex(),
            expires_at: self.after(OWNER_SESSION_TTL_SECS),
            minted_while_agent,
        };
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM owner_browser_sessions WHERE expires_at <= ?1 OR ended_at IS NOT NULL",
                params![self.now()],
            )?;
            tx.execute(
                "INSERT INTO owner_browser_sessions (sha256, created_at, expires_at, minted_while_agent)
                 VALUES (?1, ?2, ?3, ?4)",
                params![live.sha256, self.now(), live.expires_at, minted_while_agent],
            )?;
            Ok(())
        })?;
        Ok((session, live))
    }

    /// Every owner browser session neither ended nor expired.
    ///
    /// # Errors
    /// The store's.
    pub fn live_owner_sessions(&self) -> Result<Vec<LiveOwnerSession>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT sha256, expires_at, minted_while_agent FROM owner_browser_sessions
                 WHERE ended_at IS NULL AND expires_at > ?1",
            )?;
            let rows = stmt
                .query_map(params![self.now()], |r| {
                    Ok(LiveOwnerSession {
                        sha256: r.get(0)?,
                        expires_at: r.get(1)?,
                        minted_while_agent: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Ends the owner browser session whose cookie's SHA-256 is `sha256`:
    /// its cookies stand for no one from then on. True when a live one
    /// ended.
    ///
    /// # Errors
    /// The store's.
    pub fn end_owner_session(&self, sha256: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE owner_browser_sessions SET ended_at = ?2 WHERE sha256 = ?1 AND ended_at IS NULL",
                params![sha256, self.now()],
            )? > 0)
        })
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
    fn a_login_code_is_spent_once_and_only_within_its_time() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let code = st.create_login_code().unwrap();
        assert!(code.expose().starts_with(LOGIN_CODE_PREFIX));
        assert_eq!(
            st.redeem_login_code(code.expose()).unwrap(),
            Some(Redeemed {
                minted_while_agent: false
            })
        );
        assert!(
            st.redeem_login_code(code.expose()).unwrap().is_none(),
            "spent"
        );
        assert!(st.redeem_login_code("cxi_never-made").unwrap().is_none());
        let late = st.create_login_code().unwrap();
        clock.advance(LOGIN_CODE_TTL_SECS);
        assert!(
            st.redeem_login_code(late.expose()).unwrap().is_none(),
            "expired"
        );
        // Only the hash is kept.
        let kept: Vec<String> = st
            .with_read(|c| {
                Ok(c.prepare("SELECT sha256 FROM login_codes")?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert!(kept.iter().all(|h| !h.starts_with(LOGIN_CODE_PREFIX)));
    }

    #[test]
    fn a_code_minted_while_an_agent_session_is_live_says_so() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        // Each code is redeemed at once: a code lives two minutes.
        let minted = |st: &Store| {
            let c = st.create_login_code().unwrap();
            st.redeem_login_code(c.expose())
                .unwrap()
                .unwrap()
                .minted_while_agent
        };
        assert!(!st.agent_session_live().unwrap());
        assert!(!minted(&st));
        let session = st
            .register_session(
                crate::test_util::DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    cwd: "/w".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(st.agent_session_live().unwrap());
        assert!(minted(&st));
        st.end_session(crate::test_util::DAEMON, &session.id)
            .unwrap();
        // Ended moments ago, it still counts until ten minutes pass.
        assert!(st.agent_session_live().unwrap());
        assert!(minted(&st));
        clock.advance(crate::confirmations::CONFIRMATION_TTL_SECS);
        assert!(!st.agent_session_live().unwrap());
        assert!(!minted(&st));
    }

    #[test]
    fn an_owner_session_lives_until_it_ends_or_expires() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let (cookie, live) = st.open_owner_session(false).unwrap();
        assert!(cookie.expose().starts_with(OWNER_SESSION_PREFIX));
        assert_eq!(live.sha256, cookie.sha256_hex());
        assert_eq!(st.live_owner_sessions().unwrap(), vec![live.clone()]);
        let (_, other) = st.open_owner_session(true).unwrap();
        assert!(other.minted_while_agent);
        assert!(st.end_owner_session(&live.sha256).unwrap());
        assert!(!st.end_owner_session(&live.sha256).unwrap());
        assert_eq!(st.live_owner_sessions().unwrap(), vec![other]);
        clock.advance(OWNER_SESSION_TTL_SECS);
        assert!(st.live_owner_sessions().unwrap().is_empty());
    }
}
