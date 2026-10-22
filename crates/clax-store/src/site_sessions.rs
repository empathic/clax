//! Site sessions (migration 44): what lets a browser read a private
//! artifact on its own site host (`<id>.<site domain>`).
//!
//! - **A site code** is minted for someone who may read the artifact
//!   ([`Store::site_code_mint`]): by the shell for its frame, or by the
//!   public host's `/site-signin` for a top-level visit. It is good for
//!   [`SITE_CODE_TTL_SECS`] and for one redemption, on that artifact's host
//!   alone ([`Store::site_code_redeem`]), which makes a site session, and
//!   only beside the state it was minted for: a value the redeeming
//!   browser holds in a cookie of the site host, so a code minted for one
//!   browser signs no other in (login CSRF).
//! - **A site session** names its [`Holder`] for one artifact. It lasts
//!   [`SITE_SESSION_TTL_SECS`] from its last use, each use renewing it
//!   ([`Store::site_session`]), and ends with the browser session it was
//!   made from (`browser_sessions`), when it was made from one. Whether the
//!   holder may still read the artifact is not the store's to say: the
//!   caller checks it on every request.
//! - Codes and session IDs are 32 random bytes in base64url; only their
//!   SHA-256 is kept.

use super::Store;
use clax_core::secret::{self, sha256_hex};
use clax_core::{ArtifactId, Result};
use rusqlite::{OptionalExtension, params};

/// How long a site code may be redeemed.
pub const SITE_CODE_TTL_SECS: i64 = 60;
/// How long a site session lasts unused.
pub const SITE_SESSION_TTL_SECS: i64 = 3600;

/// The `remote_id` of a `push_approvals` row that approves opening its
/// artifact's site to the world rather than a push: an agent's request to
/// open one waits on the person's answer to its question, which is spent
/// once used, as a push's is ([`Store::create_push_approval`]).
pub const OPEN_TO_WORLD: &str = "site:world";

/// Who a site code or session reads as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// The owner (the owner's browser, or the daemon token).
    Owner,
    /// A connected person, by public ID.
    Person(String),
    /// A guest, by public ID: reads while the artifact is shared by link.
    Guest(String),
    /// Someone with no session who opened the artifact by its link: reads
    /// while it is shared by link.
    Link,
}

impl Holder {
    fn parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Holder::Owner => ("owner", None),
            Holder::Person(p) => ("person", Some(p)),
            Holder::Guest(g) => ("guest", Some(g)),
            Holder::Link => ("link", None),
        }
    }

    fn from_parts(kind: &str, holder: Option<String>) -> Option<Holder> {
        match (kind, holder) {
            ("owner", _) => Some(Holder::Owner),
            ("person", Some(p)) => Some(Holder::Person(p)),
            ("guest", Some(g)) => Some(Holder::Guest(g)),
            ("link", _) => Some(Holder::Link),
            _ => None,
        }
    }
}

/// Whether a code or session bound to `browser_session` (a browser
/// session's cookie SHA-256) may still be used: unbound, or its browser
/// session is live.
fn bound_session_live(st: &Store, browser_session: Option<&str>) -> Result<bool> {
    match browser_session {
        None => Ok(true),
        Some(b) => Ok(st.browser_session_of_hash(b)?.is_some()),
    }
}

/// A site code's row: its artifact, holder kind and holder, browser
/// session, whether it is partitioned, its state's SHA-256 and its expiry.
type CodeRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
    String,
    String,
);

impl Store {
    /// A new site code for `holder` on artifact `aid`, bound to the browser
    /// session whose cookie's SHA-256 is `browser_session` (when made from
    /// one), whose session cookie is to be `partitioned`, redeemed only
    /// beside `state` (the redeeming browser's state cookie). Expired codes
    /// and sessions are swept as it is made.
    ///
    /// # Errors
    /// The database's.
    pub fn site_code_mint(
        &self,
        aid: &ArtifactId,
        holder: &Holder,
        browser_session: Option<&str>,
        partitioned: bool,
        state: &str,
    ) -> Result<String> {
        let code = secret::token("");
        let (kind, who) = holder.parts();
        let now = self.now();
        let expires =
            clax_core::time::stamp(self.now_utc() + chrono::Duration::seconds(SITE_CODE_TTL_SECS));
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM site_codes WHERE expires_at <= ?1",
                params![now],
            )?;
            tx.execute(
                "DELETE FROM site_sessions WHERE expires_at <= ?1",
                params![now],
            )?;
            tx.execute(
                "INSERT INTO site_codes (code_hash, artifact_id, holder_kind, holder,
                    browser_session, partitioned, state_hash, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    code.sha256_hex(),
                    aid.as_str(),
                    kind,
                    who,
                    browser_session,
                    partitioned,
                    sha256_hex(state),
                    expires
                ],
            )?;
            Ok(())
        })?;
        Ok(code.expose().to_string())
    }

    /// Redeems site code `code` on artifact `aid`'s host: the new site
    /// session's ID, its holder, and whether its cookie is partitioned.
    /// `None` for a code that is unknown, used, expired, another
    /// artifact's, presented without the state it was minted for (`state`,
    /// the request's state cookie), or bound to a browser session that has
    /// ended. A code is gone once presented, whatever the answer.
    ///
    /// # Errors
    /// The database's.
    pub fn site_code_redeem(
        &self,
        code: &str,
        aid: &ArtifactId,
        state: Option<&str>,
    ) -> Result<Option<(String, Holder, bool)>> {
        let hash = sha256_hex(code);
        let now = self.now();
        let row = self.with_tx(|tx| {
            let row: Option<CodeRow> = tx
                .query_row(
                    "SELECT artifact_id, holder_kind, holder, browser_session, partitioned,
                        state_hash, expires_at
                     FROM site_codes WHERE code_hash = ?1",
                    params![hash],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    },
                )
                .optional()?;
            tx.execute("DELETE FROM site_codes WHERE code_hash = ?1", params![hash])?;
            Ok(row)
        })?;
        let Some((artifact, kind, who, browser, partitioned, state_hash, expires)) = row else {
            return Ok(None);
        };
        let held = state.is_some_and(|s| {
            secret::constant_time_eq(sha256_hex(s).as_bytes(), state_hash.as_bytes())
        });
        if artifact != aid.as_str() || expires <= now || !held {
            return Ok(None);
        }
        let Some(holder) = Holder::from_parts(&kind, who.clone()) else {
            return Ok(None);
        };
        if !bound_session_live(self, browser.as_deref())? {
            return Ok(None);
        }
        let id = secret::token("");
        let expires = clax_core::time::stamp(
            self.now_utc() + chrono::Duration::seconds(SITE_SESSION_TTL_SECS),
        );
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO site_sessions (id_hash, artifact_id, holder_kind, holder,
                    browser_session, expires_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id.sha256_hex(),
                    aid.as_str(),
                    kind,
                    who,
                    browser,
                    expires,
                    now
                ],
            )?;
            Ok(())
        })?;
        Ok(Some((id.expose().to_string(), holder, partitioned)))
    }

    /// The holder of site session `id` on artifact `aid`'s host, renewing
    /// it for [`SITE_SESSION_TTL_SECS`] from now; `None` when it is unknown,
    /// expired, another artifact's, or its browser session has ended.
    ///
    /// # Errors
    /// The database's.
    pub fn site_session(&self, id: &str, aid: &ArtifactId) -> Result<Option<Holder>> {
        let hash = sha256_hex(id);
        let now = self.now();
        let row: Option<(String, Option<String>, Option<String>)> = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT holder_kind, holder, browser_session FROM site_sessions
                 WHERE id_hash = ?1 AND artifact_id = ?2 AND expires_at > ?3",
                params![hash, aid.as_str(), now],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })?;
        let Some((kind, who, browser)) = row else {
            return Ok(None);
        };
        if !bound_session_live(self, browser.as_deref())? {
            return Ok(None);
        }
        let expires = clax_core::time::stamp(
            self.now_utc() + chrono::Duration::seconds(SITE_SESSION_TTL_SECS),
        );
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE site_sessions SET expires_at = ?2 WHERE id_hash = ?1",
                params![hash, expires],
            )?;
            Ok(())
        })?;
        Ok(Holder::from_parts(&kind, who))
    }

    /// Whether artifact `aid`'s site is open to the world
    /// (`artifacts.site_access` is `world`); false for one that does not
    /// exist.
    ///
    /// # Errors
    /// The database's.
    pub fn site_open_to_world(&self, aid: &ArtifactId) -> Result<bool> {
        self.with_read(|c| {
            let v: Option<String> = c
                .query_row(
                    "SELECT site_access FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                    params![aid.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.as_deref() == Some("world"))
        })
    }

    /// Opens artifact `aid`'s site to the world (`world`) or closes it.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist; the database's.
    pub fn set_site_open_to_world(&self, aid: &ArtifactId, world: bool) -> Result<()> {
        let access = if world { "world" } else { "private" };
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE artifacts SET site_access = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![aid.as_str(), access],
            )?;
            if n == 0 {
                return Err(clax_core::CoreError::NotFound);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, store_with_clock};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn artifact(st: &Store) -> ArtifactId {
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": "Doc",
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let (a, _) = st.create_artifact(DAEMON, p, None).unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    #[test]
    fn a_code_is_redeemed_once_on_its_artifact_within_sixty_seconds() {
        let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let (a, b) = (artifact(&st), artifact(&st));
        let code = st
            .site_code_mint(&a, &Holder::Owner, None, true, "st")
            .unwrap();
        assert_eq!(code.len(), 43);
        assert!(
            st.site_code_redeem(&code, &b, Some("st"))
                .unwrap()
                .is_none(),
            "B's host"
        );
        // Presented once, on another host, it is gone.
        assert!(
            st.site_code_redeem(&code, &a, Some("st"))
                .unwrap()
                .is_none()
        );
        let code = st
            .site_code_mint(&a, &Holder::Link, None, false, "st")
            .unwrap();
        let (id, holder, partitioned) =
            st.site_code_redeem(&code, &a, Some("st")).unwrap().unwrap();
        assert_eq!((holder, partitioned), (Holder::Link, false));
        assert_eq!(st.site_session(&id, &a).unwrap(), Some(Holder::Link));
        assert_eq!(st.site_session(&id, &b).unwrap(), None);
        assert!(
            st.site_code_redeem(&code, &a, Some("st"))
                .unwrap()
                .is_none(),
            "used"
        );
        let late = st
            .site_code_mint(&a, &Holder::Person("p1".into()), None, false, "st")
            .unwrap();
        clock.advance(SITE_CODE_TTL_SECS);
        assert!(
            st.site_code_redeem(&late, &a, Some("st"))
                .unwrap()
                .is_none(),
            "expired"
        );
    }

    #[test]
    fn a_code_redeems_only_beside_the_state_it_was_minted_for() {
        let (_d, st) = crate::test_util::store();
        let a = artifact(&st);
        for other in [None, Some("another"), Some("")] {
            let code = st
                .site_code_mint(&a, &Holder::Owner, None, false, "st")
                .unwrap();
            assert!(
                st.site_code_redeem(&code, &a, other).unwrap().is_none(),
                "{other:?}"
            );
            assert!(
                st.site_code_redeem(&code, &a, Some("st"))
                    .unwrap()
                    .is_none(),
                "gone once presented"
            );
        }
        let code = st
            .site_code_mint(&a, &Holder::Owner, None, false, "st")
            .unwrap();
        assert!(
            st.site_code_redeem(&code, &a, Some("st"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_session_lasts_an_hour_from_its_last_use() {
        let clock = Arc::new(ManualClock::at("2026-10-22T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let a = artifact(&st);
        let code = st
            .site_code_mint(&a, &Holder::Guest("g1".into()), None, false, "st")
            .unwrap();
        let (id, _, _) = st.site_code_redeem(&code, &a, Some("st")).unwrap().unwrap();
        clock.advance(SITE_SESSION_TTL_SECS - 1);
        assert_eq!(
            st.site_session(&id, &a).unwrap(),
            Some(Holder::Guest("g1".into()))
        );
        clock.advance(SITE_SESSION_TTL_SECS - 1);
        assert!(st.site_session(&id, &a).unwrap().is_some(), "renewed");
        clock.advance(SITE_SESSION_TTL_SECS);
        assert!(st.site_session(&id, &a).unwrap().is_none(), "unused");
    }

    #[test]
    fn a_session_ends_with_its_browser_session() {
        let (_d, st) = crate::test_util::store();
        let a = artifact(&st);
        let signed = st.hosted_setup(DAEMON, "Owner").unwrap();
        let browser = signed.session.sha256_hex();
        let code = st
            .site_code_mint(&a, &Holder::Owner, Some(&browser), true, "st")
            .unwrap();
        let (id, _, _) = st.site_code_redeem(&code, &a, Some("st")).unwrap().unwrap();
        assert_eq!(st.site_session(&id, &a).unwrap(), Some(Holder::Owner));
        let unused = st
            .site_code_mint(&a, &Holder::Owner, Some(&browser), true, "st")
            .unwrap();
        st.end_browser_session(DAEMON, signed.session.expose(), "sign_out")
            .unwrap();
        assert!(st.site_session(&id, &a).unwrap().is_none());
        assert!(
            st.site_code_redeem(&unused, &a, Some("st"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_site_is_private_until_opened_to_the_world() {
        let (_d, st) = crate::test_util::store();
        let a = artifact(&st);
        assert!(!st.site_open_to_world(&a).unwrap());
        st.set_site_open_to_world(&a, true).unwrap();
        assert!(st.site_open_to_world(&a).unwrap());
        st.set_site_open_to_world(&a, false).unwrap();
        assert!(!st.site_open_to_world(&a).unwrap());
    }
}
