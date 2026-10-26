//! A hosted daemon's device sign-in: another machine's Clax signs in as a
//! person here (RFC 8628's shape) and receives an agent token of theirs.
//!
//! 1. The machine asks for a code ([`Store::start_device`]), naming itself.
//!    It gets a device code, which it keeps and polls with, and a user code
//!    (`WXYZ-4821`), which it shows the person beside the approval page's
//!    address. Both last [`DEVICE_TTL_SECS`].
//! 2. A signed-in member opens the approval page, which names the machine
//!    and the address it asked from ([`Store::peek_device`]), and approves
//!    ([`Store::approve_device`]) or denies ([`Store::deny_device`]).
//!    Approving makes a bearer grant of the member's, labelled with the
//!    machine's name, at once: an agent token on their People page, as one
//!    they minted there, revocable there, whose rights are theirs.
//! 3. The machine's next poll ([`Store::poll_device`]) issues that grant's
//!    token, once. A poll sooner than [`DEVICE_INTERVAL_SECS`] after the
//!    last is `slow_down`; a denied, expired, unknown or spent code is
//!    refused, one answer per case.
//!
//! Codes are [`Secret`]s handed out once; only their SHA-256 is stored and
//! looked up. A user code is looked up by its canonical form, so it may be
//! typed in lowercase or without its dash. Every time is the store's clock.

use super::Store;
use super::hosted::{AgentToken, SessionViewer, agent_label, session_viewer};
use chrono::Duration;
use clax_core::audit::AuditCtx;
use clax_core::secret::{self, Secret, sha256_hex};
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// How long a device sign-in's codes may be used.
pub const DEVICE_TTL_SECS: i64 = 10 * 60;
/// The least time between two polls of one device code.
pub const DEVICE_INTERVAL_SECS: i64 = 5;
/// Prefix of a device code.
pub const DEVICE_PREFIX: &str = "cxd_";
/// The most device sign-ins waiting at once; past it, a new one is refused
/// `limit_reached` until some expire.
pub const MAX_PENDING_DEVICES: i64 = 1000;

/// The kind a device sign-in names when the Clax app makes it: its token
/// is marked as the person's own device ([`Store::is_person_device_grant`]).
pub const CLAX_APP_KIND: &str = "clax-app";

/// A device sign-in just started: its codes, shown to the machine once.
#[derive(Debug)]
pub struct DeviceStart {
    pub device_code: Secret,
    pub user_code: Secret,
    pub expires_at: String,
}

/// A waiting device sign-in as its approval page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceRequest {
    /// What the machine calls itself (untrusted).
    pub client_name: String,
    /// The address it asked from.
    pub peer_ip: String,
    /// The kind it named ([`CLAX_APP_KIND`] for the Clax app, whose token
    /// may manage sharing), if any (untrusted).
    pub client_kind: Option<String>,
    /// The loopback page on the machine to send its person to once the
    /// sign-in is decided (already vetted by the caller), if any.
    pub return_to: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

/// What a machine's poll found.
#[derive(Debug)]
pub enum DevicePoll {
    /// Not decided yet.
    Pending,
    /// Polled sooner than [`DEVICE_INTERVAL_SECS`] after the last poll.
    SlowDown,
    /// The person denied it.
    Denied,
    /// Unknown, expired, or its token was issued already.
    Expired,
    /// Approved: the agent token, issued this once, and whose it is.
    Issued {
        token: Secret,
        grant_id: String,
        person: String,
        person_name: Option<String>,
    },
}

impl Store {
    fn device_later(&self, secs: i64) -> String {
        clax_core::time::stamp(self.now_utc() + Duration::seconds(secs))
    }

    /// Starts a device sign-in for the machine named `client_name`, asking
    /// from `peer_ip`. Expired sign-ins are dropped first.
    ///
    /// # Errors
    /// `invalid_args` for a name over the label limit or with control
    /// characters; `limit_reached` with [`MAX_PENDING_DEVICES`] waiting.
    pub fn start_device(&self, client_name: &str, peer_ip: &str) -> Result<DeviceStart> {
        self.start_device_kind(client_name, peer_ip, None, None)
    }

    /// [`Store::start_device`] for a machine that names its kind too:
    /// approving a sign-in of kind [`CLAX_APP_KIND`] marks its token as the
    /// person's own device. Any other kind is kept but marks nothing.
    ///
    /// # Errors
    /// [`Store::start_device`]'s, and `invalid_args` for a kind over 64
    /// characters or with any but ASCII letters, digits and `-`.
    ///
    /// `return_to` is kept as given for [`Store::peek_device`] to serve:
    /// the caller has checked that it names a page on the machine.
    pub fn start_device_kind(
        &self,
        client_name: &str,
        peer_ip: &str,
        kind: Option<&str>,
        return_to: Option<&str>,
    ) -> Result<DeviceStart> {
        if let Some(k) = kind
            && (k.is_empty()
                || k.len() > 64
                || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        {
            return Err(CoreError::invalid(
                "invalid_args",
                "a client kind is 1 to 64 ASCII letters, digits and dashes",
            ));
        }
        let name = agent_label(client_name)?;
        self.with_tx(|tx| {
            let now = self.now();
            tx.execute(
                "DELETE FROM device_codes WHERE expires_at <= ?1",
                params![now],
            )?;
            let waiting: i64 = tx.query_row(
                "SELECT COUNT(*) FROM device_codes WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )?;
            if waiting >= MAX_PENDING_DEVICES {
                return Err(CoreError::invalid(
                    "limit_reached",
                    "too many sign-ins are waiting: try again in a few minutes",
                ));
            }
            let device_code = secret::token(DEVICE_PREFIX);
            // A user code is short: draw again in the rare case one is taken.
            let mut user_code = secret::user_code();
            let mut user_hash = user_sha256(user_code.expose()).expect("a fresh user code");
            for _ in 0..8 {
                let taken: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM device_codes WHERE user_sha256 = ?1)",
                    params![user_hash],
                    |r| r.get(0),
                )?;
                if !taken {
                    break;
                }
                user_code = secret::user_code();
                user_hash = user_sha256(user_code.expose()).expect("a fresh user code");
            }
            let expires_at = self.device_later(DEVICE_TTL_SECS);
            tx.execute(
                "INSERT INTO device_codes (id, device_sha256, user_sha256, client_name, peer_ip,
                    created_at, expires_at, client_kind, return_to)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new_ulid(),
                    device_code.sha256_hex(),
                    user_hash,
                    name,
                    peer_ip,
                    now,
                    expires_at,
                    kind,
                    return_to
                ],
            )?;
            Ok(DeviceStart {
                device_code,
                user_code,
                expires_at,
            })
        })
    }

    /// The waiting sign-in whose user code is `user_code`, as typed; `None`
    /// when it is unknown, decided or expired.
    pub fn peek_device(&self, user_code: &str) -> Result<Option<DeviceRequest>> {
        let Some(hash) = user_sha256(user_code) else {
            return Ok(None);
        };
        let now = self.now();
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT client_name, peer_ip, client_kind, return_to, created_at, expires_at
                 FROM device_codes
                 WHERE user_sha256 = ?1 AND status = 'pending' AND expires_at > ?2",
                params![hash, now],
                |r| {
                    Ok(DeviceRequest {
                        client_name: r.get(0)?,
                        peer_ip: r.get(1)?,
                        client_kind: r.get(2)?,
                        return_to: r.get(3)?,
                        created_at: r.get(4)?,
                        expires_at: r.get(5)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Approves the waiting sign-in whose user code is `user_code` as
    /// member `viewer_id`, recording `grant.approve` under `ctx`: a bearer
    /// grant of theirs labelled with the machine's name, whose token the
    /// machine's next poll receives, marked as the person's own device when
    /// the sign-in's kind is [`CLAX_APP_KIND`].
    ///
    /// # Errors
    /// `NotFound` for a code that is unknown, decided or expired;
    /// `forbidden` for anyone not a member.
    pub fn approve_device(
        &self,
        ctx: &AuditCtx,
        viewer_id: &str,
        user_code: &str,
    ) -> Result<AgentToken> {
        let hash = user_sha256(user_code).ok_or(CoreError::NotFound)?;
        self.with_tx(|tx| {
            let now = self.now();
            let row: Option<(String, String, String, Option<String>)> = tx
                .query_row(
                    "SELECT id, client_name, peer_ip, client_kind FROM device_codes
                     WHERE user_sha256 = ?1 AND status = 'pending' AND expires_at > ?2",
                    params![hash, now],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let (id, name, peer_ip, kind) = row.ok_or(CoreError::NotFound)?;
            let grant = self.bearer_grant_in(tx, ctx, viewer_id, &name, &peer_ip)?;
            let device = kind.as_deref() == Some(CLAX_APP_KIND);
            if device {
                tx.execute(
                    "UPDATE grants SET person_device = 1 WHERE id = ?1",
                    params![grant.id],
                )?;
            }
            tx.execute(
                "UPDATE device_codes SET status = 'approved', viewer_id = ?2, grant_id = ?3,
                    decided_at = ?4 WHERE id = ?1",
                params![id, viewer_id, grant.id, now],
            )?;
            Ok(AgentToken::of(&grant).device(device))
        })
    }

    /// Denies the waiting sign-in whose user code is `user_code`, as
    /// member `viewer_id`.
    ///
    /// # Errors
    /// `NotFound` for a code that is unknown, decided or expired.
    pub fn deny_device(&self, viewer_id: &str, user_code: &str) -> Result<()> {
        let hash = user_sha256(user_code).ok_or(CoreError::NotFound)?;
        self.with_tx(|tx| {
            let now = self.now();
            let n = tx.execute(
                "UPDATE device_codes SET status = 'denied', viewer_id = ?2, decided_at = ?3
                 WHERE user_sha256 = ?1 AND status = 'pending' AND expires_at > ?3",
                params![hash, viewer_id, now],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(())
        })
    }

    /// A machine's poll with `device_code`: issues an approved sign-in's
    /// token once, recording `grant.token` under `ctx`.
    ///
    /// # Errors
    /// The store's.
    pub fn poll_device(&self, ctx: &AuditCtx, device_code: &str) -> Result<DevicePoll> {
        if !device_code.starts_with(DEVICE_PREFIX) {
            return Ok(DevicePoll::Expired);
        }
        let hash = sha256_hex(device_code);
        self.with_tx(|tx| {
            let now_utc = self.now_utc();
            let now = self.now();
            type Row = (
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
            );
            let row: Option<Row> = tx
                .query_row(
                    "SELECT id, status, expires_at, polled_at, viewer_id, grant_id
                     FROM device_codes WHERE device_sha256 = ?1",
                    params![hash],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, status, expires, polled, viewer, gid)) = row else {
                return Ok(DevicePoll::Expired);
            };
            if expires <= now || status == "issued" {
                return Ok(DevicePoll::Expired);
            }
            let too_soon = polled
                .as_deref()
                .and_then(|p| chrono::DateTime::parse_from_rfc3339(p).ok())
                .is_some_and(|p| {
                    now_utc - p.with_timezone(&chrono::Utc)
                        < Duration::seconds(DEVICE_INTERVAL_SECS)
                });
            tx.execute(
                "UPDATE device_codes SET polled_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            if too_soon {
                return Ok(DevicePoll::SlowDown);
            }
            match (status.as_str(), viewer, gid) {
                ("denied", _, _) => Ok(DevicePoll::Denied),
                ("approved", Some(viewer), Some(gid)) => {
                    let live: Option<String> = tx
                        .query_row(
                            "SELECT v.public_id FROM grants g JOIN viewers v ON v.id = g.viewer_id
                             WHERE g.id = ?1 AND g.status = 'approved'",
                            params![gid],
                            |r| r.get(0),
                        )
                        .optional()?;
                    tx.execute(
                        "UPDATE device_codes SET status = 'issued' WHERE id = ?1",
                        params![id],
                    )?;
                    // Revoked on People before the machine came back.
                    let Some(person) = live else {
                        return Ok(DevicePoll::Denied);
                    };
                    let person_name: Option<String> = tx.query_row(
                        "SELECT display_name FROM viewers WHERE id = ?1",
                        params![viewer],
                        |r| r.get(0),
                    )?;
                    let token = self.issue_bearer_token_in(tx, ctx, &gid)?;
                    Ok(DevicePoll::Issued {
                        token,
                        grant_id: gid,
                        person,
                        person_name,
                    })
                }
                _ => Ok(DevicePoll::Pending),
            }
        })
    }
}

impl Store {
    /// Whether agent token `grant_id` was issued to another machine's Clax
    /// by a device sign-in (and not minted on People for an agent).
    pub fn is_device_grant(&self, grant_id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM device_codes WHERE grant_id = ?1)",
                params![grant_id],
                |r| r.get(0),
            )?)
        })
    }

    /// Whether agent token `grant_id` was issued to the person's own
    /// device: a device sign-in that named itself [`CLAX_APP_KIND`].
    pub fn is_person_device_grant(&self, grant_id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM grants WHERE id = ?1 AND person_device = 1)",
                params![grant_id],
                |r| r.get(0),
            )?)
        })
    }
}

impl Store {
    /// The person agent token `grant_id` speaks for on the routes a
    /// person's own device reaches (their artifacts' sharing, their apps):
    /// `Some` only while it is an approved bearer grant marked as their
    /// device ([`Store::is_person_device_grant`]) and they are still a
    /// member here.
    pub fn person_device_viewer(&self, grant_id: &str) -> Result<Option<SessionViewer>> {
        self.with_read(|c| {
            let viewer: Option<String> = c
                .query_row(
                    "SELECT viewer_id FROM grants
                     WHERE id = ?1 AND person_device = 1 AND bearer = 1 AND status = 'approved'",
                    params![grant_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = viewer else {
                return Ok(None);
            };
            Ok(session_viewer(c, &id)?.filter(SessionViewer::member))
        })
    }
}

/// The SHA-256 of a typed user code's canonical form; `None` for anything
/// that is not one.
fn user_sha256(code: &str) -> Option<String> {
    secret::canonical_user_code(code).map(|c| sha256_hex(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        (dir, st, clock)
    }

    fn code_of<T: std::fmt::Debug>(r: Result<T>) -> String {
        match r.unwrap_err() {
            CoreError::Invalid { code, .. } => code.to_string(),
            CoreError::NotFound => "not_found".into(),
            e => format!("{e:?}"),
        }
    }

    fn owner(st: &Store) -> String {
        st.hosted_setup(&AuditCtx::DAEMON, "Ada")
            .unwrap()
            .who
            .viewer
            .id
    }

    #[test]
    fn an_approved_code_issues_the_persons_agent_token_once() {
        let (_d, st, clock) = store();
        let ada = owner(&st);
        let start = st.start_device("alex-laptop", "203.0.113.9").unwrap();
        assert!(start.device_code.expose().starts_with(DEVICE_PREFIX));
        assert_eq!(start.expires_at, "2026-10-12T10:10:00.000Z");
        let typed = start.user_code.expose().replace('-', "").to_lowercase();
        let peek = st.peek_device(&typed).unwrap().unwrap();
        assert_eq!(peek.client_name, "alex-laptop");
        assert_eq!(peek.peer_ip, "203.0.113.9");
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, start.device_code.expose())
                .unwrap(),
            DevicePoll::Pending
        ));
        let t = st
            .approve_device(&AuditCtx::DAEMON, &ada, start.user_code.expose())
            .unwrap();
        assert_eq!(t.label, "alex-laptop");
        // Listed on People at once, before the machine polls.
        assert_eq!(st.agent_tokens(&ada).unwrap()[0].id, t.id);
        assert!(st.peek_device(start.user_code.expose()).unwrap().is_none());
        clock.advance(DEVICE_INTERVAL_SECS);
        let DevicePoll::Issued {
            token,
            grant_id,
            person_name,
            ..
        } = st
            .poll_device(&AuditCtx::DAEMON, start.device_code.expose())
            .unwrap()
        else {
            panic!("not issued");
        };
        assert_eq!(grant_id, t.id);
        assert_eq!(person_name.as_deref(), Some("Ada"));
        assert_eq!(st.bearer_grant(token.expose()).unwrap(), Some(t.id));
        clock.advance(DEVICE_INTERVAL_SECS);
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, start.device_code.expose())
                .unwrap(),
            DevicePoll::Expired
        ));
    }

    #[test]
    fn codes_expire_and_polls_slow_down() {
        let (_d, st, clock) = store();
        let ada = owner(&st);
        let start = st.start_device("box", "::1").unwrap();
        let dc = start.device_code.expose();
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, dc).unwrap(),
            DevicePoll::Pending
        ));
        clock.advance(DEVICE_INTERVAL_SECS - 1);
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, dc).unwrap(),
            DevicePoll::SlowDown
        ));
        // A slow-down poll counts as a poll: the wait starts again.
        clock.advance(DEVICE_INTERVAL_SECS);
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, dc).unwrap(),
            DevicePoll::Pending
        ));
        clock.advance(DEVICE_TTL_SECS);
        assert!(st.peek_device(start.user_code.expose()).unwrap().is_none());
        assert_eq!(
            code_of(st.approve_device(&AuditCtx::DAEMON, &ada, start.user_code.expose())),
            "not_found"
        );
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, dc).unwrap(),
            DevicePoll::Expired
        ));
        assert!(st.agent_tokens(&ada).unwrap().is_empty());
    }

    #[test]
    fn a_denied_code_is_refused_and_unknown_codes_find_nothing() {
        let (_d, st, clock) = store();
        let ada = owner(&st);
        let start = st.start_device("box", "::1").unwrap();
        st.deny_device(&ada, start.user_code.expose()).unwrap();
        assert_eq!(
            code_of(st.deny_device(&ada, start.user_code.expose())),
            "not_found"
        );
        assert_eq!(
            code_of(st.approve_device(&AuditCtx::DAEMON, &ada, start.user_code.expose())),
            "not_found"
        );
        clock.advance(DEVICE_INTERVAL_SECS);
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, start.device_code.expose())
                .unwrap(),
            DevicePoll::Denied
        ));
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, "cxd_nothing").unwrap(),
            DevicePoll::Expired
        ));
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, "cxa_wrongprefix")
                .unwrap(),
            DevicePoll::Expired
        ));
        assert!(st.peek_device("not a code").unwrap().is_none());
        assert!(st.peek_device("ABCD-0000").unwrap().is_none());
    }

    #[test]
    fn only_a_member_approves_and_a_token_revoked_before_the_poll_is_denied() {
        let (_d, st, clock) = store();
        let ada = owner(&st);
        let guest = st.create_guest(&AuditCtx::DAEMON, "Gus").unwrap();
        let start = st.start_device("box", "::1").unwrap();
        assert_eq!(
            code_of(st.approve_device(
                &AuditCtx::DAEMON,
                &guest.who.viewer.id,
                start.user_code.expose()
            )),
            "forbidden"
        );
        // The refusal changed nothing: the owner still may.
        let t = st
            .approve_device(&AuditCtx::DAEMON, &ada, start.user_code.expose())
            .unwrap();
        st.revoke_grant(&AuditCtx::DAEMON, &t.id, crate::people::EndedBy::Person)
            .unwrap();
        clock.advance(DEVICE_INTERVAL_SECS);
        assert!(matches!(
            st.poll_device(&AuditCtx::DAEMON, start.device_code.expose())
                .unwrap(),
            DevicePoll::Denied
        ));
    }

    /// Approves a sign-in of `kind` and returns its grant's ID.
    fn approved(st: &Store, ada: &str, kind: Option<&str>) -> String {
        let s = st.start_device_kind("phone", "::1", kind, None).unwrap();
        let peek = st.peek_device(s.user_code.expose()).unwrap().unwrap();
        assert_eq!(peek.client_kind.as_deref(), kind);
        st.approve_device(&AuditCtx::DAEMON, ada, s.user_code.expose())
            .unwrap()
            .id
    }

    #[test]
    fn a_sign_ins_return_to_is_kept_for_its_peek() {
        let (_d, st, _c) = store();
        let to = "http://localhost:7480/a/x?publish=1";
        let s = st.start_device_kind("box", "::1", None, Some(to)).unwrap();
        let peek = st.peek_device(s.user_code.expose()).unwrap().unwrap();
        assert_eq!(peek.return_to.as_deref(), Some(to));
        let plain = st.start_device("box", "::1").unwrap();
        let peek = st.peek_device(plain.user_code.expose()).unwrap().unwrap();
        assert_eq!(peek.return_to, None);
    }

    #[test]
    fn device_kind_clax_app_marks_grant() {
        let (_d, st, _c) = store();
        let ada = owner(&st);
        let app = approved(&st, &ada, Some(CLAX_APP_KIND));
        let other = approved(&st, &ada, Some("clax-cli"));
        let none = approved(&st, &ada, None);
        let plain = st.start_device("box", "::1").unwrap();
        let plain = st
            .approve_device(&AuditCtx::DAEMON, &ada, plain.user_code.expose())
            .unwrap()
            .id;
        assert!(st.is_person_device_grant(&app).unwrap());
        for g in [&other, &none, &plain] {
            assert!(!st.is_person_device_grant(g).unwrap(), "{g}");
            assert!(st.is_device_grant(g).unwrap(), "{g}");
        }
        assert!(!st.is_person_device_grant("no-such-grant").unwrap());
        // The device speaks for its person while it is live; People lists
        // it apart.
        let who = st.person_device_viewer(&app).unwrap().unwrap();
        assert_eq!(who.viewer.id, ada);
        for g in [&other, &none, &plain] {
            assert!(st.person_device_viewer(g).unwrap().is_none(), "{g}");
        }
        let kinds: Vec<(String, &str)> = st
            .agent_tokens(&ada)
            .unwrap()
            .into_iter()
            .map(|t| (t.id, t.kind))
            .collect();
        assert!(kinds.contains(&(app.clone(), "device")), "{kinds:?}");
        assert_eq!(kinds.iter().filter(|(_, k)| *k == "agent").count(), 3);
        st.revoke_grant(&AuditCtx::DAEMON, &app, crate::people::EndedBy::Person)
            .unwrap();
        assert!(st.person_device_viewer(&app).unwrap().is_none());
        let kinds: Vec<Option<String>> = st
            .with_read(|c| {
                let mut s = c.prepare("SELECT client_kind FROM device_codes ORDER BY rowid")?;
                Ok(s.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(
            kinds,
            [Some("clax-app".into()), Some("clax-cli".into()), None, None]
        );
        for bad in ["", "clax app", "clax-app\n", &"k".repeat(65)] {
            assert_eq!(
                code_of(st.start_device_kind("phone", "::1", Some(bad), None)),
                "invalid_args",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_machine_name_is_checked_as_a_token_label() {
        let (_d, st, _c) = store();
        assert_eq!(code_of(st.start_device("a\u{7}b", "::1")), "invalid_args");
        let s = st.start_device("  ", "::1").unwrap();
        assert_eq!(
            st.peek_device(s.user_code.expose())
                .unwrap()
                .unwrap()
                .client_name,
            "Agent"
        );
    }
}
