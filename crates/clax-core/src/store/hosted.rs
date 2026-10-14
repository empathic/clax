//! Hosted mode's sign-in: the first person's setup, browser sessions,
//! invites redeemed in the browser, one-use sign-in links, guests, agent
//! tokens minted in the browser, and pages shared by link.
//!
//! A hosted daemon serves the public internet, so a browser is never "this
//! machine" and the `clax_viewer` cookie names no one. A browser signs in
//! as a viewer row instead, through a session whose cookie only the daemon
//! can mint:
//!
//! - **The owner**: the first person, made once with the setup secret
//!   ([`Store::hosted_setup`]). Their sessions name the owner's row, so
//!   their browser is a browser of the owner's.
//! - **A person**: a connected person (multi-person M1) whose invite was
//!   redeemed in the browser ([`Store::redeem_invite_in_browser`]) rather
//!   than by `clax connect`. Their rights are the person's rights.
//! - **A guest**: a viewer who gave a name on a page shared by link
//!   ([`Store::create_guest`]). A guest is no person: they are
//!   `*unconnected`, hold no grant, invite no one, mint no link or token,
//!   and act only within an artifact's [`LinkAccess`], which stops at
//!   commenting.
//!
//! A signed-in person or the owner may sign in another device with a
//! one-use link good for [`SIGNIN_TTL_SECS`] ([`Store::create_signin_link`]),
//! and mint agent tokens for remote agents ([`Store::mint_agent_token`]): a
//! *bearer grant*, approved at once for its person as an invite's grant is,
//! whose token is the bearer `cxa_…` (TLS carries it; no key signs it).
//! Its rights are the person's within the grant's narrowing, as for every
//! grant.
//!
//! Codes and tokens are [`Secret`]s handed out once; only their SHA-256 is
//! stored and looked up. Every time is the store's clock.

use super::Store;
use super::people::{
    Grant, checked_client_name, grant_access, grant_in, grant_record, new_grant_id,
};
use super::viewers::{check_viewer_name, checked_name, row_to_viewer};
use crate::audit::{Actor, AuditCtx, AuditKind, AuditRecord};
use crate::model::Viewer;
use crate::names::CollisionKeys;
use crate::rights::Rights;
use crate::secret::{self, Secret, sha256_hex};
use crate::{CoreError, Result, new_public_id, new_ulid};
use base64::Engine as _;
use chrono::Duration;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

/// How long a browser session lasts from sign-in.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
/// How long a sign-in link may be redeemed.
pub const SIGNIN_TTL_SECS: i64 = 10 * 60;
/// Prefix of a browser session's cookie value.
pub const SESSION_PREFIX: &str = "cxs_";
/// Prefix of a sign-in link's code.
pub const SIGNIN_PREFIX: &str = "cxl_";
/// Prefix of an agent token (a bearer grant's access token).
pub const AGENT_TOKEN_PREFIX: &str = "cxa_";
/// Longest agent token label, in characters.
pub const MAX_LABEL_CHARS: usize = super::people::MAX_CLIENT_NAME_CHARS;

/// What anyone with an artifact's link may do there. A guest is capped at
/// [`LinkAccess::Commenter`] whatever else the artifact allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkAccess {
    /// Read the artifact, its versions and threads.
    Viewer,
    /// [`LinkAccess::Viewer`], and start threads, reply and send threads
    /// to agents (once named, as a guest).
    Commenter,
}

impl LinkAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkAccess::Viewer => "viewer",
            LinkAccess::Commenter => "commenter",
        }
    }

    /// `viewer` or `commenter`; anything else is `None`.
    pub fn parse(s: &str) -> Option<LinkAccess> {
        match s {
            "viewer" => Some(LinkAccess::Viewer),
            "commenter" => Some(LinkAccess::Commenter),
            _ => None,
        }
    }

    /// The rights this access holds at artifact `aid`, as access objects
    /// (multi-person §7.1) located at that artifact alone: `viewer` reads
    /// the artifact and its comments and watches them; `commenter` also
    /// writes, replies and sends. Neither resolves, deletes, publishes or
    /// reaches the db, and neither asks.
    ///
    /// # Panics
    /// When `aid` is not an artifact ID (a location must be one).
    pub fn rights(self, aid: &str) -> Rights {
        let comments: &[&str] = match self {
            LinkAccess::Viewer => &["read", "watch"],
            LinkAccess::Commenter => &["read", "watch", "write", "reply", "send"],
        };
        Rights::parse(&serde_json::json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": [aid]},
            {"type": "clax-comments", "actions": comments, "locations": [aid]},
        ]))
        .expect("link rights are well formed")
    }
}

/// Who a browser session signs in as.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionViewer {
    /// The viewer row: the owner's, a person's or a guest's.
    pub viewer: Viewer,
    /// The owner's row.
    pub owner: bool,
    /// A guest ([`Store::create_guest`]).
    pub guest: bool,
}

impl SessionViewer {
    /// The owner or a connected, unrevoked person: a member of the site.
    pub fn member(&self) -> bool {
        self.owner || (self.viewer.person && self.viewer.person_revoked_at.is_none())
    }
}

/// A new browser session: who it signs in as, and its cookie value, shown
/// once.
#[derive(Debug)]
pub struct SignedIn {
    pub who: SessionViewer,
    pub session: Secret,
    pub expires_at: String,
}

/// What an invite says before it is redeemed (the join page).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InvitePeek {
    /// The name the person joins under; the joiner cannot change it.
    pub name: Option<String>,
    pub expires_at: String,
}

/// An agent token, without its token: a bearer grant.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentToken {
    /// The grant's ID.
    pub id: String,
    pub label: String,
    /// `approved` while it may be used, else `revoked` or `expired`.
    pub status: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub ended_at: Option<String>,
}

impl AgentToken {
    fn of(g: &Grant) -> AgentToken {
        AgentToken {
            id: g.id.clone(),
            label: g.client_name.clone(),
            status: g.status.clone(),
            created_at: g.created_at.clone(),
            last_used_at: g.last_used_at.clone(),
            ended_at: g.ended_at.clone(),
        }
    }
}

/// How a browser signed in, as its `session.start` record names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInVia {
    /// The first person's setup ([`Store::hosted_setup`]).
    Setup,
    /// An invite redeemed in the browser.
    Invite,
    /// A one-use sign-in link.
    Link,
    /// A guest naming themselves.
    Guest,
}

impl SignInVia {
    pub fn as_str(self) -> &'static str {
        match self {
            SignInVia::Setup => "setup",
            SignInVia::Invite => "invite",
            SignInVia::Link => "link",
            SignInVia::Guest => "guest",
        }
    }
}

/// The ID the audit journal names a browser session by: `browser-` and the
/// first 20 hex digits of its cookie's SHA-256 (stable for the session,
/// telling nothing of its cookie).
pub fn browser_session_id(cookie_sha256_hex: &str) -> String {
    format!(
        "browser-{}",
        &cookie_sha256_hex[..cookie_sha256_hex.len().min(20)]
    )
}

/// The audit actor a browser signed in as `who` acts as.
fn actor_of(who: &SessionViewer) -> Actor {
    if who.owner {
        Actor::Owner {
            public_id: who.viewer.public_id.clone(),
        }
    } else {
        Actor::Viewer {
            public_id: who.viewer.public_id.clone(),
            display_name: who.viewer.display_name.clone(),
            person: who.viewer.person && who.viewer.person_revoked_at.is_none(),
        }
    }
}

/// A live invite's ID, viewer row, name and expiry.
type LiveInvite = (String, String, Option<String>, String);

const VIEWER_COLS: &str =
    "v.id, v.public_id, v.display_name, v.created_at, v.person, v.person_revoked_at";

fn not_found() -> CoreError {
    CoreError::NotFound
}

/// The viewer row `id` (the owner's included), with its owner and guest
/// flags.
fn session_viewer(c: &rusqlite::Connection, id: &str) -> Result<Option<SessionViewer>> {
    Ok(c.query_row(
        &format!("SELECT {VIEWER_COLS}, v.owner, v.guest FROM viewers v WHERE v.id = ?1"),
        params![id],
        |r| {
            Ok(SessionViewer {
                viewer: row_to_viewer(r)?,
                owner: r.get(6)?,
                guest: r.get(7)?,
            })
        },
    )
    .optional()?)
}

/// The words of `name` as names are compared: its [`CollisionKeys`]
/// folded skeleton, split at whatever is not a letter or a digit.
fn name_words(name: &str) -> Vec<String> {
    let key: String = unicode_security::skeleton(&crate::names::token_form(name)).collect();
    key.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Words no guest's name may hold: they would claim to be the owner or
/// Clax itself (`Ada (the owner)`).
const GUEST_RESERVED: [&str; 2] = ["owner", "clax"];

/// Refuses `name` for a guest: it must be a valid, non-empty display name
/// that looks like ([`CollisionKeys`]) neither a member's (the owner's or
/// a connected person's) nor a live agent's ([`check_viewer_name`]) nor
/// another guest's, holds no member's name among its words (`Ada (the
/// owner)`, `Sam's agent`), and claims to be neither the owner nor Clax.
/// Returns it trimmed.
fn check_guest_name(tx: &Transaction<'_>, id: &str, name: &str) -> Result<String> {
    let Some(name) = checked_name(name)? else {
        return Err(CoreError::invalid("invalid_name", "a guest needs a name"));
    };
    check_viewer_name(tx, id, name)?;
    let words = name_words(name);
    if words.iter().any(|w| GUEST_RESERVED.contains(&w.as_str())) {
        return Err(CoreError::invalid(
            "name_taken",
            format!("a guest's name cannot claim to be the owner or Clax: '{name}'"),
        ));
    }
    let mut members = tx.prepare_cached(
        "SELECT display_name FROM viewers WHERE display_name IS NOT NULL
           AND (owner = 1 OR (person = 1 AND person_revoked_at IS NULL))",
    )?;
    for member in members.query_map([], |r| r.get::<_, String>(0))? {
        let m = name_words(&member?);
        if !m.is_empty() && words.windows(m.len()).any(|w| w == m.as_slice()) {
            return Err(CoreError::invalid(
                "name_taken",
                format!("the name '{name}' holds a member's name"),
            ));
        }
    }
    let key = CollisionKeys::of(name);
    let mut stmt = tx.prepare_cached(
        "SELECT display_name FROM viewers WHERE guest = 1 AND display_name IS NOT NULL AND id != ?1",
    )?;
    for other in stmt.query_map(params![id], |r| r.get::<_, String>(0))? {
        if CollisionKeys::of(&other?).meets(&key) {
            return Err(CoreError::invalid(
                "name_taken",
                format!("the name '{name}' is taken"),
            ));
        }
    }
    Ok(name.to_string())
}

impl Store {
    fn later(&self, secs: i64) -> String {
        super::stamp(self.now_utc() + Duration::seconds(secs))
    }

    /// Opens a session for viewer `viewer_id` in `tx`: its cookie value and
    /// expiry.
    fn open_session(&self, tx: &Transaction<'_>, viewer_id: &str) -> Result<(Secret, String)> {
        let token = secret::token(SESSION_PREFIX);
        let expires = self.later(SESSION_TTL_SECS);
        tx.execute(
            "INSERT INTO browser_sessions (sha256, viewer_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![token.sha256_hex(), viewer_id, self.now(), expires],
        )?;
        Ok((token, expires))
    }

    /// Signs viewer `viewer_id` in (a new session), recording
    /// `session.start` of the browser session (harness `browser`, `how` it
    /// signed in, and `extra` fields) as that viewer, through `ctx`'s
    /// channel.
    fn signed_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        viewer_id: &str,
        how: SignInVia,
        extra: &[(&str, serde_json::Value)],
    ) -> Result<SignedIn> {
        let who = session_viewer(tx, viewer_id)?.ok_or_else(not_found)?;
        let (session, expires_at) = self.open_session(tx, viewer_id)?;
        let mut rec = AuditRecord::new(AuditKind::SessionStart, self.now())
            .with("harness", "browser")
            .with("how", how.as_str())
            .with("expires_at", expires_at.clone());
        for (k, v) in extra {
            rec = rec.with(k, v.clone());
        }
        rec.ids.session = Some(browser_session_id(&session.sha256_hex()));
        let ctx = AuditCtx {
            actor: actor_of(&who),
            ..ctx.clone()
        };
        self.record_audit(tx, &ctx, rec)?;
        Ok(SignedIn {
            who,
            session,
            expires_at,
        })
    }

    /// Whether setup is still open: no first person has been made.
    pub fn hosted_needs_setup(&self) -> Result<bool> {
        self.with_read(|c| {
            Ok(
                !c.query_row("SELECT EXISTS (SELECT 1 FROM hosted_setup)", [], |r| {
                    r.get::<_, bool>(0)
                })?,
            )
        })
    }

    /// Makes the first person, once: the owner's row (made, or claimed for
    /// a browser, when the CLI made it first), named `name`, with a session.
    /// The caller has checked the setup secret. It also revokes the owner's
    /// approval of `*unconnected` (multi-person M14), so a guest's comments
    /// reach the owner's agents only once the owner approves them: on the
    /// internet, anyone with a shared page's link may become a guest. The
    /// owner may turn it back on in People.
    ///
    /// # Errors
    /// `NotFound` once setup is done; `invalid_name` for an empty or
    /// invalid name, `name_taken` for a live agent's.
    pub fn hosted_setup(&self, ctx: &AuditCtx, name: &str) -> Result<SignedIn> {
        if checked_name(name)?.is_none() {
            return Err(CoreError::invalid(
                "invalid_name",
                "the first person needs a name",
            ));
        }
        if !self.hosted_needs_setup()? {
            return Err(not_found());
        }
        let owner = self.set_owner_name(name, true)?;
        let signed = self.with_tx(|tx| {
            if tx.execute(
                "INSERT OR IGNORE INTO hosted_setup (id, viewer_id, created_at) VALUES (1, ?1, ?2)",
                params![owner.id, self.now()],
            )? == 0
            {
                return Err(not_found());
            }
            // In the setup's transaction: setup is never done while the
            // owner's agents still hear guests.
            let ids: Vec<String> = tx
                .prepare(
                    "SELECT id FROM approvals
                     WHERE approver IS NULL AND author = ?1 AND revoked_at IS NULL",
                )?
                .query_map(params![super::approvals::UNCONNECTED], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let now = self.now();
            for id in ids {
                super::approvals::revoke_in(self, tx, &AuditCtx::DAEMON, &id, &now)?;
            }
            self.signed_in(tx, ctx, &owner.id, SignInVia::Setup, &[])
        })?;
        Ok(signed)
    }

    /// Who the session whose cookie value is `token` signs in as, while it
    /// is live: not ended, not expired, and not a revoked person's (a
    /// revoked person is signed out everywhere).
    pub fn browser_session(&self, token: &str) -> Result<Option<SessionViewer>> {
        if !token.starts_with(SESSION_PREFIX) {
            return Ok(None);
        }
        let hash = sha256_hex(token);
        self.with_read(|c| {
            let viewer: Option<String> = c
                .query_row(
                    "SELECT viewer_id FROM browser_sessions
                     WHERE sha256 = ?1 AND ended_at IS NULL AND expires_at > ?2",
                    params![hash, self.now()],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = viewer else {
                return Ok(None);
            };
            Ok(session_viewer(c, &id)?.filter(|s| s.viewer.person_revoked_at.is_none()))
        })
    }

    /// Ends the session whose cookie value is `token`, recording
    /// `session.end` with `reason` (`sign_out`, or `replaced` when the
    /// browser signed in anew) as its viewer through `ctx`'s channel.
    /// Answers whether a live one ended.
    pub fn end_browser_session(&self, ctx: &AuditCtx, token: &str, reason: &str) -> Result<bool> {
        let hash = sha256_hex(token);
        self.with_tx(|tx| {
            let viewer: Option<String> = tx
                .query_row(
                    "SELECT viewer_id FROM browser_sessions WHERE sha256 = ?1 AND ended_at IS NULL",
                    params![hash],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(viewer) = viewer else {
                return Ok(false);
            };
            tx.execute(
                "UPDATE browser_sessions SET ended_at = ?2 WHERE sha256 = ?1",
                params![hash, self.now()],
            )?;
            let actor = match session_viewer(tx, &viewer)? {
                Some(who) => actor_of(&who),
                None => Actor::Anonymous,
            };
            let mut rec =
                AuditRecord::new(AuditKind::SessionEnd, self.now()).with("reason", reason);
            rec.ids.session = Some(browser_session_id(&hash));
            let ctx = AuditCtx {
                actor,
                ..ctx.clone()
            };
            self.record_audit(tx, &ctx, rec)?;
            Ok(true)
        })
    }

    /// A one-use link's code, good for [`SIGNIN_TTL_SECS`], that signs
    /// another browser in as member `viewer_id`, and its expiry.
    ///
    /// # Errors
    /// `forbidden` for a guest or anyone else not a member; `NotFound` for
    /// no such viewer.
    pub fn create_signin_link(&self, viewer_id: &str) -> Result<(Secret, String)> {
        self.with_tx(|tx| {
            let who = session_viewer(tx, viewer_id)?.ok_or_else(not_found)?;
            if !who.member() {
                return Err(CoreError::invalid(
                    "forbidden",
                    "only a member of this Clax makes sign-in links",
                ));
            }
            let code = secret::token(SIGNIN_PREFIX);
            let expires = self.later(SIGNIN_TTL_SECS);
            tx.execute(
                "INSERT INTO signin_links (sha256, viewer_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![code.sha256_hex(), viewer_id, self.now(), expires],
            )?;
            Ok((code, expires))
        })
    }

    /// Redeems a sign-in link's `code` once: a new session for its member.
    /// A code that was presented is spent, live or not.
    ///
    /// # Errors
    /// `NotFound` for an unknown, used or expired code, or one whose member
    /// is no longer one (one answer for all).
    pub fn redeem_signin_link(&self, ctx: &AuditCtx, code: &str) -> Result<SignedIn> {
        let hash = sha256_hex(code);
        self.with_tx(|tx| {
            let row: Option<(String, String)> = tx
                .query_row(
                    "SELECT viewer_id, expires_at FROM signin_links WHERE sha256 = ?1 AND used_at IS NULL",
                    params![hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((viewer_id, expires)) = row else {
                return Ok(Err(not_found()));
            };
            let now = self.now();
            tx.execute(
                "UPDATE signin_links SET used_at = ?2 WHERE sha256 = ?1",
                params![hash, now],
            )?;
            if expires <= now {
                return Ok(Err(not_found()));
            }
            match session_viewer(tx, &viewer_id)? {
                Some(who) if who.member() => self
                    .signed_in(tx, ctx, &viewer_id, SignInVia::Link, &[])
                    .map(Ok),
                _ => Ok(Err(not_found())),
            }
        })?
    }

    /// The live invite (unused, unrevoked, unexpired) whose code is `code`,
    /// as typed: its ID, viewer row and expiry. The owner's own invites
    /// (for `clax connect` from another machine of theirs) are not redeemed
    /// in a browser.
    fn live_browser_invite(
        c: &rusqlite::Connection,
        code: &str,
        now: &str,
    ) -> Result<Option<LiveInvite>> {
        let Some(canon) = secret::canonical_invite(code) else {
            return Ok(None);
        };
        Ok(c.query_row(
            "SELECT i.id, i.viewer_id, v.display_name, i.expires_at
               FROM invites i JOIN viewers v ON v.id = i.viewer_id
              WHERE i.code_sha256 = ?1 AND i.used_at IS NULL AND i.revoked_at IS NULL
                AND i.expires_at > ?2 AND v.owner = 0",
            params![sha256_hex(&canon), now],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?)
    }

    /// What invite `code` says, while it may be redeemed in a browser.
    pub fn peek_invite(&self, code: &str) -> Result<Option<InvitePeek>> {
        self.with_read(|c| {
            Ok(Self::live_browser_invite(c, code, &self.now())?
                .map(|(_, _, name, expires_at)| InvitePeek { name, expires_at }))
        })
    }

    /// Redeems invite `code` in a browser, once: its viewer becomes (or
    /// stays) a connected person, under the name the invite gave, and gets
    /// a session. No grant is made: the person mints agent tokens later.
    ///
    /// # Errors
    /// `invalid_invite` for an expired, used, revoked or unknown code, or
    /// the owner's own invite (one message for all).
    pub fn redeem_invite_in_browser(&self, ctx: &AuditCtx, code: &str) -> Result<SignedIn> {
        self.with_tx(|tx| {
            let now = self.now();
            let Some((invite_id, viewer_id, _, _)) = Self::live_browser_invite(tx, code, &now)?
            else {
                return Err(CoreError::invalid(
                    "invalid_invite",
                    "that invite is not valid: ask for a new one",
                ));
            };
            tx.execute(
                "UPDATE invites SET used_at = ?2 WHERE id = ?1",
                params![invite_id, now],
            )?;
            if tx.execute(
                "UPDATE viewers SET person = 1, person_revoked_at = NULL
                 WHERE id = ?1 AND (person = 0 OR person_revoked_at IS NOT NULL)",
                params![viewer_id],
            )? > 0
            {
                // No longer `*unconnected` (multi-person M14, M17).
                self.visibility_changed();
            }
            self.signed_in(
                tx,
                ctx,
                &viewer_id,
                SignInVia::Invite,
                &[("invite_id", invite_id.clone().into())],
            )
        })
    }

    /// Makes a guest named `name` and signs them in.
    ///
    /// # Errors
    /// `invalid_name` or `name_taken` ([`check_guest_name`]).
    pub fn create_guest(&self, ctx: &AuditCtx, name: &str) -> Result<SignedIn> {
        self.with_tx(|tx| {
            let id = new_ulid();
            let name = check_guest_name(tx, &id, name)?;
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, guest)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                params![id, new_public_id(), name, self.now()],
            )?;
            self.signed_in(tx, ctx, &id, SignInVia::Guest, &[])
        })
    }

    /// Renames guest `viewer_id`.
    ///
    /// # Errors
    /// `invalid_name` or `name_taken` ([`check_guest_name`]); `NotFound`
    /// when `viewer_id` names no guest.
    pub fn rename_guest(&self, viewer_id: &str, name: &str) -> Result<Viewer> {
        self.with_tx(|tx| {
            let name = check_guest_name(tx, viewer_id, name)?;
            if tx.execute(
                "UPDATE viewers SET display_name = ?2 WHERE id = ?1 AND guest = 1",
                params![viewer_id, name],
            )? == 0
            {
                return Err(not_found());
            }
            Ok(session_viewer(tx, viewer_id)?.ok_or_else(not_found)?.viewer)
        })
    }

    /// Mints an agent token for member `viewer_id`, labelled `label` (what
    /// the person calls the agent's machine), recording `grant.approve` and
    /// `grant.token` under `ctx`: a bearer grant, approved at once, whose
    /// rights are the person's. Its key and its manage and continuation
    /// tokens are random and handed to no one, so nothing signs for it;
    /// it lapses unused for 30 days, as every grant does.
    ///
    /// # Errors
    /// `forbidden` for a guest or anyone else not a member; `invalid_args`
    /// for a label over [`MAX_LABEL_CHARS`] characters or with control
    /// characters; `NotFound` for no such viewer.
    pub fn mint_agent_token(
        &self,
        ctx: &AuditCtx,
        viewer_id: &str,
        label: &str,
        peer_ip: &str,
    ) -> Result<(AgentToken, Secret)> {
        let label = match label.trim() {
            "" => "Agent".to_string(),
            l => checked_client_name(l)?,
        };
        let token = secret::token(AGENT_TOKEN_PREFIX);
        let key =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret::random_bytes::<32>());
        self.with_tx(|tx| {
            let who = session_viewer(tx, viewer_id)?.ok_or_else(not_found)?;
            if !who.member() {
                return Err(CoreError::invalid(
                    "forbidden",
                    "only a member of this Clax mints agent tokens",
                ));
            }
            let now = self.now();
            let gid = new_grant_id();
            tx.execute(
                "INSERT INTO grants (id, viewer_id, status, via, client_key, client_name, peer_ip,
                    continue_sha256, manage_sha256, created_at, decided_at, bearer)
                 VALUES (?1, ?2, 'approved', 'invite', ?3, ?4, ?5, ?6, ?7, ?8, ?8, 1)",
                params![
                    gid,
                    viewer_id,
                    key,
                    label,
                    peer_ip,
                    secret::token(secret::CONTINUE_PREFIX).sha256_hex(),
                    secret::token(secret::MANAGE_PREFIX).sha256_hex(),
                    now
                ],
            )?;
            tx.execute(
                "INSERT INTO grant_tokens (sha256, grant_id, created_at) VALUES (?1, ?2, ?3)",
                params![token.sha256_hex(), gid, now],
            )?;
            let grant = grant_in(tx, &gid)?.expect("the grant was written");
            let access = grant_access(tx, viewer_id, None)?;
            let rec = grant_record(AuditKind::GrantApprove, &now, &gid)
                .with("person", grant.person.clone())
                .with("access", access.to_json())
                .with("code_checked", false);
            self.record_audit(tx, ctx, rec)?;
            let rec = grant_record(AuditKind::GrantToken, &now, &gid).with("op", "issue");
            self.record_audit(tx, ctx, rec)?;
            Ok((AgentToken::of(&grant), token.clone()))
        })
    }

    /// Member `viewer_id`'s agent tokens, newest first, live and ended.
    pub fn agent_tokens(&self, viewer_id: &str) -> Result<Vec<AgentToken>> {
        let ids: Vec<String> = self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM grants WHERE viewer_id = ?1 AND bearer = 1
                 ORDER BY created_at DESC, id DESC",
            )?;
            Ok(stmt
                .query_map(params![viewer_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })?;
        self.with_read(|c| {
            ids.iter()
                .filter_map(|gid| grant_in(c, gid).transpose())
                .map(|g| g.map(|g| AgentToken::of(&g)))
                .collect()
        })
    }

    /// Whether bearer grant `gid` is one of viewer `viewer_id`'s.
    pub fn agent_token_of(&self, viewer_id: &str, gid: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM grants WHERE id = ?1 AND viewer_id = ?2 AND bearer = 1)",
                params![gid, viewer_id],
                |r| r.get(0),
            )?)
        })
    }

    /// The grant a bearer agent token `token` holds, while the token and
    /// its grant are live: what the remote MCP endpoint authenticates by,
    /// before [`Store::grant_caller`] gives its rights.
    pub fn bearer_grant(&self, token: &str) -> Result<Option<String>> {
        if !token.starts_with(AGENT_TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = sha256_hex(token);
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT g.id FROM grant_tokens t JOIN grants g ON g.id = t.grant_id
                 WHERE t.sha256 = ?1 AND t.revoked_at IS NULL AND g.status = 'approved'
                   AND g.bearer = 1",
                params![hash],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// What anyone with artifact `aid`'s link may do there; `None` when it
    /// is not shared by link, or does not exist.
    pub fn link_access(&self, aid: &str) -> Result<Option<LinkAccess>> {
        self.with_read(|c| {
            let v: Option<Option<String>> = c
                .query_row(
                    "SELECT link_access FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                    params![aid],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.flatten().as_deref().and_then(LinkAccess::parse))
        })
    }

    /// Shares artifact `aid` by link with `access`, or stops (`None`),
    /// recording `artifact.update` of its `link_access` under `ctx`.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist.
    pub fn set_link_access(
        &self,
        ctx: &AuditCtx,
        aid: &str,
        access: Option<LinkAccess>,
    ) -> Result<()> {
        self.with_tx(|tx| {
            let value = access.map(LinkAccess::as_str);
            if tx.execute(
                "UPDATE artifacts SET link_access = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![aid, value],
            )? == 0
            {
                return Err(not_found());
            }
            let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, self.now())
                .with("fields", serde_json::json!({ "link_access": value }));
            rec.ids.artifact = Some(aid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Every artifact shared by link, with its access.
    pub fn shared_by_link(&self) -> Result<Vec<(String, LinkAccess)>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, link_access FROM artifacts
                 WHERE link_access IS NOT NULL AND deleted_at IS NULL ORDER BY id",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(id, a)| LinkAccess::parse(&a).map(|a| (id, a)))
                .collect())
        })
    }

    /// Whether viewer `viewer_id` is a guest.
    pub fn is_guest(&self, viewer_id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT guest FROM viewers WHERE id = ?1",
                params![viewer_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Home;
    use crate::rights::RightType;
    use crate::store::people::{EndedBy, InvitePerson};
    use crate::working::ManualClock;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        (dir, st, clock)
    }

    fn code_of(err: CoreError) -> String {
        match err {
            CoreError::Invalid { code, .. } => code.to_string(),
            CoreError::NotFound => "not_found".into(),
            e => format!("{e:?}"),
        }
    }

    #[test]
    fn setup_makes_the_owner_once() {
        let (_d, st, _c) = store();
        assert!(st.hosted_needs_setup().unwrap());
        assert_eq!(
            code_of(st.hosted_setup(&AuditCtx::DAEMON, "  ").unwrap_err()),
            "invalid_name"
        );
        let s = st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        assert!(s.who.owner && s.who.member() && !s.who.guest);
        assert_eq!(s.who.viewer.display_name.as_deref(), Some("Ada"));
        assert!(s.session.expose().starts_with(SESSION_PREFIX));
        assert_eq!(s.expires_at, "2026-11-11T10:00:00.000Z");
        assert!(!st.hosted_needs_setup().unwrap());
        assert_eq!(
            code_of(st.hosted_setup(&AuditCtx::DAEMON, "Eve").unwrap_err()),
            "not_found"
        );
        let who = st.browser_session(s.session.expose()).unwrap().unwrap();
        assert!(who.owner);
        assert_eq!(st.owner().unwrap().unwrap().id, who.viewer.id);
        // Guests are `*unconnected`: their words wait for the owner's approval.
        assert!(
            st.live_approvals(None)
                .unwrap()
                .iter()
                .all(|a| a.author != crate::store::approvals::UNCONNECTED)
        );
    }

    #[test]
    fn a_session_lasts_thirty_days_and_ends_on_sign_out() {
        let (_d, st, clock) = store();
        let s = st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let t = s.session.expose().to_string();
        assert!(st.browser_session(&t).unwrap().is_some());
        assert!(st.browser_session("cxs_nothing").unwrap().is_none());
        assert!(
            st.browser_session(&t.replacen("cxs_", "cxa_", 1))
                .unwrap()
                .is_none()
        );
        clock.advance(30 * 24 * 3600 - 1);
        assert!(st.browser_session(&t).unwrap().is_some());
        clock.advance(1);
        assert!(st.browser_session(&t).unwrap().is_none());
        let s2 = st
            .redeem_signin_link(
                &AuditCtx::DAEMON,
                st.create_signin_link(&s.who.viewer.id).unwrap().0.expose(),
            )
            .unwrap();
        let t2 = s2.session.expose().to_string();
        assert!(
            st.end_browser_session(&AuditCtx::DAEMON, &t2, "sign_out")
                .unwrap()
        );
        assert!(
            !st.end_browser_session(&AuditCtx::DAEMON, &t2, "sign_out")
                .unwrap()
        );
        assert!(st.browser_session(&t2).unwrap().is_none());
    }

    #[test]
    fn a_signin_link_is_used_once_within_ten_minutes() {
        let (_d, st, clock) = store();
        let owner = st
            .hosted_setup(&AuditCtx::DAEMON, "Ada")
            .unwrap()
            .who
            .viewer;
        let (code, expires) = st.create_signin_link(&owner.id).unwrap();
        assert!(code.expose().starts_with(SIGNIN_PREFIX));
        assert_eq!(expires, "2026-10-12T10:10:00.000Z");
        let s = st
            .redeem_signin_link(&AuditCtx::DAEMON, code.expose())
            .unwrap();
        assert!(s.who.owner);
        assert_eq!(
            code_of(
                st.redeem_signin_link(&AuditCtx::DAEMON, code.expose())
                    .unwrap_err()
            ),
            "not_found"
        );
        let (late, _) = st.create_signin_link(&owner.id).unwrap();
        clock.advance(10 * 60);
        assert_eq!(
            code_of(
                st.redeem_signin_link(&AuditCtx::DAEMON, late.expose())
                    .unwrap_err()
            ),
            "not_found"
        );
        // Presented once late, it is spent: the clock going back does not revive it.
        clock.advance(-5 * 60);
        assert_eq!(
            code_of(
                st.redeem_signin_link(&AuditCtx::DAEMON, late.expose())
                    .unwrap_err()
            ),
            "not_found"
        );
        assert_eq!(
            code_of(
                st.redeem_signin_link(&AuditCtx::DAEMON, "cxl_unknown")
                    .unwrap_err()
            ),
            "not_found"
        );
    }

    #[test]
    fn an_invite_redeemed_in_the_browser_makes_a_person_who_stays_one() {
        let (_d, st, clock) = store();
        st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let (_, code) = st
            .create_invite(&AuditCtx::DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let peek = st.peek_invite(code.expose()).unwrap().unwrap();
        assert_eq!(peek.name.as_deref(), Some("Sam"));
        // Typed in lower case, with or without dashes, it is the same code.
        let lower = code.expose().to_lowercase();
        assert!(st.peek_invite(&lower).unwrap().is_some());
        let s = st
            .redeem_invite_in_browser(&AuditCtx::DAEMON, code.expose())
            .unwrap();
        assert!(s.who.member() && !s.who.owner && !s.who.guest && s.who.viewer.person);
        assert_eq!(s.who.viewer.display_name.as_deref(), Some("Sam"));
        assert!(st.peek_invite(code.expose()).unwrap().is_none());
        assert_eq!(
            code_of(
                st.redeem_invite_in_browser(&AuditCtx::DAEMON, code.expose())
                    .unwrap_err()
            ),
            "invalid_invite"
        );
        // Past the invite's day, the sweep leaves a person who redeemed one a person.
        clock.advance(2 * 24 * 3600);
        st.expire_grants().unwrap();
        let who = st.browser_session(s.session.expose()).unwrap().unwrap();
        assert!(who.member());
        // The owner's own invite is for `clax connect`, never a browser.
        let (_, mine) = st
            .create_invite(&AuditCtx::DAEMON, InvitePerson::Owner, None)
            .unwrap();
        assert!(st.peek_invite(mine.expose()).unwrap().is_none());
        assert_eq!(
            code_of(
                st.redeem_invite_in_browser(&AuditCtx::DAEMON, mine.expose())
                    .unwrap_err()
            ),
            "invalid_invite"
        );
        // An expired invite is refused.
        let (_, old) = st
            .create_invite(&AuditCtx::DAEMON, InvitePerson::New("Wes".into()), None)
            .unwrap();
        clock.advance(24 * 3600);
        assert_eq!(
            code_of(
                st.redeem_invite_in_browser(&AuditCtx::DAEMON, old.expose())
                    .unwrap_err()
            ),
            "invalid_invite"
        );
    }

    #[test]
    fn a_revoked_person_is_signed_out() {
        let (_d, st, _c) = store();
        st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let (_, code) = st
            .create_invite(&AuditCtx::DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let s = st
            .redeem_invite_in_browser(&AuditCtx::DAEMON, code.expose())
            .unwrap();
        st.revoke_person(&AuditCtx::DAEMON, &s.who.viewer.public_id)
            .unwrap();
        assert!(st.browser_session(s.session.expose()).unwrap().is_none());
        assert_eq!(
            code_of(st.create_signin_link(&s.who.viewer.id).unwrap_err()),
            "forbidden"
        );
    }

    #[test]
    fn guests_take_free_names_and_are_never_members() {
        let (_d, st, _c) = store();
        st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let g = st.create_guest(&AuditCtx::DAEMON, "Mia").unwrap();
        assert!(g.who.guest && !g.who.member() && !g.who.viewer.person);
        assert!(st.is_guest(&g.who.viewer.id).unwrap());
        for taken in ["mia", "ADA", " Ada "] {
            assert_eq!(
                code_of(st.create_guest(&AuditCtx::DAEMON, taken).unwrap_err()),
                "name_taken",
                "{taken}"
            );
        }
        assert_eq!(
            code_of(st.create_guest(&AuditCtx::DAEMON, "").unwrap_err()),
            "invalid_name"
        );
        assert_eq!(
            code_of(st.create_signin_link(&g.who.viewer.id).unwrap_err()),
            "forbidden"
        );
        assert_eq!(
            code_of(
                st.mint_agent_token(&AuditCtx::DAEMON, &g.who.viewer.id, "x", "203.0.113.9")
                    .unwrap_err()
            ),
            "forbidden"
        );
        let v = st.rename_guest(&g.who.viewer.id, "Mia R").unwrap();
        assert_eq!(v.display_name.as_deref(), Some("Mia R"));
        assert_eq!(
            code_of(st.rename_guest(&g.who.viewer.id, "Ada").unwrap_err()),
            "name_taken"
        );
        let owner = st.owner().unwrap().unwrap();
        assert_eq!(
            code_of(st.rename_guest(&owner.id, "Zed").unwrap_err()),
            "not_found"
        );
    }

    #[test]
    fn a_guest_may_not_take_a_name_that_reads_as_a_members() {
        let (_d, st, _c) = store();
        st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let (_, code) = st
            .create_invite(&AuditCtx::DAEMON, InvitePerson::New("Sam Lee".into()), None)
            .unwrap();
        st.redeem_invite_in_browser(&AuditCtx::DAEMON, code.expose())
            .unwrap();
        for taken in [
            "\u{410}da",   // Cyrillic А
            "ＡＤＡ",      // fullwidth
            "A\u{200B}da", // a zero-width space
            "Ada\u{3164}", // a Hangul filler
            "A\u{0301}da", // a combining mark
            "Ada (the owner)",
            "ada, owner",
            "The owner",
            "Clax",
            "Sam Lee's agent",
            "SAM  LEE",
            "S\u{430}m Lee", // Cyrillic а
        ] {
            assert_eq!(
                code_of(st.create_guest(&AuditCtx::DAEMON, taken).unwrap_err()),
                "name_taken",
                "{taken}"
            );
        }
        for bad in ["\u{202E}adA", "Ada\u{2066}"] {
            assert_eq!(
                code_of(st.create_guest(&AuditCtx::DAEMON, bad).unwrap_err()),
                "invalid_name",
                "{bad}"
            );
        }
        // Names that only share letters are free: Adam, Sam, Lee.
        for free in ["Adam", "Sam", "Lee", "Samuel Leeds"] {
            assert!(st.create_guest(&AuditCtx::DAEMON, free).is_ok(), "{free}");
        }
    }

    #[test]
    fn an_agent_token_is_a_bearer_grant_with_the_persons_rights_until_revoked() {
        let (_d, st, _c) = store();
        st.hosted_setup(&AuditCtx::DAEMON, "Ada").unwrap();
        let (_, code) = st
            .create_invite(
                &AuditCtx::DAEMON,
                InvitePerson::New("Sam".into()),
                Some(Rights::preset(crate::rights::PRESET_READER).unwrap()),
            )
            .unwrap();
        let sam = st
            .redeem_invite_in_browser(&AuditCtx::DAEMON, code.expose())
            .unwrap()
            .who
            .viewer;
        let (t, token) = st
            .mint_agent_token(&AuditCtx::DAEMON, &sam.id, "  sam-laptop ", "203.0.113.9")
            .unwrap();
        assert!(token.expose().starts_with(AGENT_TOKEN_PREFIX));
        assert_eq!(
            (t.label.as_str(), t.status.as_str()),
            ("sam-laptop", "approved")
        );
        assert_eq!(
            st.bearer_grant(token.expose()).unwrap().as_deref(),
            Some(t.id.as_str())
        );
        assert_eq!(st.bearer_grant("cxa_nothing").unwrap(), None);
        assert!(st.agent_token_of(&sam.id, &t.id).unwrap());
        let gc = st.grant_caller(&t.id, None).unwrap();
        assert_eq!(gc.person.as_deref(), Some(sam.public_id.as_str()));
        let none = |_: &str| false;
        assert!(
            gc.rights
                .allows(RightType::Artifacts, "read", Some("7q3k9mzx2b4t"), &none)
        );
        assert!(
            !gc.rights
                .allows(RightType::Artifacts, "create", None, &none)
        );
        // A bearer grant's token is never in the signed credentials' index.
        assert!(
            st.credential_index()
                .unwrap()
                .iter()
                .all(|c| c.grant_id != t.id)
        );
        assert_eq!(st.agent_tokens(&sam.id).unwrap().len(), 1);
        let (_, unlabelled) = st
            .mint_agent_token(&AuditCtx::DAEMON, &sam.id, "", "203.0.113.9")
            .unwrap();
        assert!(
            st.agent_tokens(&sam.id)
                .unwrap()
                .iter()
                .any(|t| t.label == "Agent")
        );
        assert!(st.bearer_grant(unlabelled.expose()).unwrap().is_some());
        assert_eq!(
            code_of(
                st.mint_agent_token(&AuditCtx::DAEMON, &sam.id, &"x".repeat(65), "203.0.113.9")
                    .unwrap_err()
            ),
            "invalid_args"
        );
        st.revoke_grant(&AuditCtx::DAEMON, &t.id, EndedBy::Person)
            .unwrap();
        assert_eq!(st.bearer_grant(token.expose()).unwrap(), None);
        let listed = st.agent_tokens(&sam.id).unwrap();
        assert!(listed.iter().any(|x| x.id == t.id && x.status == "revoked"));
    }

    #[test]
    fn link_access_maps_onto_rights_at_one_artifact() {
        let aid = "7q3k9mzx2b4t";
        let none = |_: &str| false;
        let v = LinkAccess::Viewer.rights(aid);
        let c = LinkAccess::Commenter.rights(aid);
        for r in [&v, &c] {
            assert!(r.allows(RightType::Artifacts, "read", Some(aid), &none));
            assert!(r.allows(RightType::Comments, "read", Some(aid), &none));
            assert!(!r.allows(RightType::Artifacts, "read", Some("8q3k9mzx2b4t"), &none));
            for (k, a) in [
                (RightType::Artifacts, "update"),
                (RightType::Artifacts, "delete"),
                (RightType::Comments, "resolve"),
                (RightType::Db, "read"),
            ] {
                assert!(!r.allows(k, a, Some(aid), &none), "{a}");
            }
        }
        for a in ["write", "reply", "send"] {
            assert!(!v.allows(RightType::Comments, a, Some(aid), &none), "{a}");
            assert!(c.allows(RightType::Comments, a, Some(aid), &none), "{a}");
        }
        assert_eq!(LinkAccess::parse("editor"), None);
    }
}
