//! Connected people, invites, grants and their tokens (spec §6.2–§6.4).
//!
//! A person is a viewer row with `person = 1` (M1). A grant is one machine's
//! credential (M3), made from an invite (approved at once, bound to the
//! invite's person) or from a request the owner approves with the
//! requester's user code (always as a new person). Codes and tokens are
//! [`Secret`]s handed out once; only their SHA-256 is stored, and hashes are
//! compared in constant time. Every time is the store's clock.

use super::Store;
use super::sessions::EndReason;
use super::viewers::{checked_name, row_to_viewer};
use crate::audit::{Actor, AuditCtx, AuditKind, AuditRecord, SystemReason};
use crate::model::Viewer;
use crate::names::CollisionKeys;
use crate::rights::Rights;
use crate::secret::{self, Secret, constant_time_eq, sha256_hex};
use crate::{CoreError, Result, new_public_id, new_ulid};
use base64::Engine as _;
use chrono::Duration;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

/// How long an invite may be redeemed.
pub const INVITE_TTL_SECS: i64 = 24 * 3600;
/// Most invites live (unused, unrevoked, unexpired) at once.
pub const MAX_LIVE_INVITES: usize = 20;
/// How long a request waits for the owner before it expires.
pub const PENDING_TTL_SECS: i64 = 10 * 60;
/// Seconds after a grant's tokens are issued within which `clax connect`
/// may record the person's answer about the owner (M15).
pub const OWNER_ANSWER_SECS: i64 = 10 * 60;
/// Most requests from one peer address within [`PENDING_TTL_SECS`],
/// whatever became of them.
pub const MAX_PENDING_PER_IP: usize = 3;
/// Most requests in all within [`PENDING_TTL_SECS`], whatever became of
/// them.
pub const MAX_PENDING: usize = 20;
/// The continuation's `wait`: a poll sooner than this after the last is
/// `too_fast`.
pub const CONTINUE_WAIT_SECS: i64 = 5;
/// A grant unused this long expires with its tokens.
pub const GRANT_IDLE_SECS: i64 = 30 * 24 * 3600;
/// `last_used_at` is written at most this often.
pub const USE_NOTE_SECS: i64 = 3600;
/// The wrong user code that denies a request.
pub const MAX_WRONG_CODES: i64 = 3;
/// How long a browser link may be redeemed (spec §6.5).
pub const BROWSER_LINK_TTL_SECS: i64 = 10 * 60;
/// Longest client name, in characters.
pub const MAX_CLIENT_NAME_CHARS: usize = 64;
/// How long a request that never had tokens issued (denied, expired or
/// cancelled) is kept before [`Store::expire_grants`] deletes it.
pub const UNISSUED_RETENTION_SECS: i64 = 24 * 3600;

/// Who an invite is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvitePerson {
    /// A new person with this display name.
    New(String),
    /// An existing LAN viewer, by public ID, which becomes a person.
    Viewer(String),
    /// The owner, for another machine of theirs.
    Owner,
}

/// An invite, without its code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Invite {
    pub id: String,
    /// The person's public ID.
    pub person: String,
    pub person_name: Option<String>,
    /// The invite is for the owner's own machine.
    pub owner: bool,
    pub created_at: String,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub grant_id: Option<String>,
    pub revoked_at: Option<String>,
}

/// A grant request (spec §6.3), its signature already verified.
#[derive(Clone, Debug)]
pub struct NewGrant {
    /// The client's Ed25519 public key.
    pub key: [u8; 32],
    /// What the client calls itself (untrusted).
    pub client_name: String,
    pub peer_ip: String,
    /// The access asked for; it can only narrow the person's rights.
    pub access: Rights,
    /// An invite code, as typed (a secret: `Debug` redacts it).
    pub invite: Option<Secret>,
}

/// How a grant ended (`grants.ended_by`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndedBy {
    Owner,
    Person,
    Expiry,
}

impl EndedBy {
    fn as_str(self) -> &'static str {
        match self {
            EndedBy::Owner => "owner",
            EndedBy::Person => "person",
            EndedBy::Expiry => "expiry",
        }
    }
}

/// A grant, without its hashes.
#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    /// `g_` and 22 base32 characters; public.
    pub id: String,
    /// `pending`, `approved`, `denied`, `revoked` or `expired`.
    pub status: String,
    /// `invite` or `request`.
    pub via: String,
    /// The client's public key, base64url.
    pub client_key: String,
    pub client_name: String,
    pub peer_ip: String,
    /// The person's public ID (the owner's for the owner's machines); `None`
    /// while pending.
    pub person: Option<String>,
    pub person_name: Option<String>,
    /// The grant is for one of the owner's own machines.
    pub owner: bool,
    /// The narrowing of every agent of this grant: while pending, the
    /// access asked for.
    pub narrow: Option<Rights>,
    pub wrong_codes: i64,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub last_used_at: Option<String>,
    pub ended_at: Option<String>,
    pub ended_by: Option<String>,
}

/// What a grant request made.
#[derive(Debug)]
pub enum GrantStart {
    /// An invite's grant: approved, with its tokens and the access it
    /// holds (the person's rights within its narrowing). `browser_code` is
    /// `None` for the owner's machines (the browser link is refused for the
    /// owner row, §6.5).
    Approved {
        grant: Grant,
        access: Rights,
        token: Secret,
        manage: Secret,
        continue_token: Secret,
        browser_code: Option<Secret>,
    },
    /// A request, waiting for the owner.
    Pending {
        grant: Grant,
        continue_token: Secret,
        user_code: Secret,
    },
}

/// What a continuation poll found.
#[derive(Debug)]
pub enum ContinueOutcome {
    /// Still waiting for the owner.
    Pending { grant: Grant },
    /// Approved: the tokens, issued once, a new continuation token, and the
    /// access the grant holds.
    Approved {
        grant: Grant,
        access: Rights,
        token: Secret,
        manage: Secret,
        continue_token: Secret,
        browser_code: Option<Secret>,
    },
}

/// What ending one grant ended.
#[derive(Debug, Default)]
pub struct RevokedGrant {
    pub grant_id: String,
    /// The grant ended now; `false` when it had ended already and nothing
    /// changed.
    pub ended: bool,
    /// Its sessions that were live, now ended (`revoked`).
    pub sessions_ended: Vec<String>,
    /// Feedback their ends released.
    pub touched: crate::feedback::Touched,
    /// Their open questions, now withdrawn.
    pub withdrawn_questions: Vec<String>,
}

/// Which of a grant's tokens a [`Credential`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CredentialKind {
    /// An access token (`cxg_`), carried as `Clax-Grant`.
    Access,
    /// The manage token (`cxm_`), carried as `GNAP` to revoke.
    Manage,
    /// The current continuation token (`cxc_`), carried as `GNAP` to poll
    /// or disconnect.
    Continue,
}

/// One token the daemon knows, by hash (spec §6.4, §6.6): what it is, its
/// grant, the key its requests must be signed with, and whether its grant
/// has ended (`grant_revoked`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub sha256: [u8; 32],
    pub kind: CredentialKind,
    pub grant_id: String,
    pub key: [u8; 32],
    /// The token was revoked, or its grant revoked or expired after
    /// approval.
    pub revoked: bool,
}

/// A continuation token of a request that never had tokens issued
/// ([`Store::unissued_continuation`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unissued {
    /// Pending, or approved with its tokens not yet collected: its calls
    /// are signed with this key.
    Live([u8; 32]),
    /// Denied, expired or cancelled: it answers `denied`.
    Ended,
}

/// Who a verified request of grant `grant_id` speaks for (spec §6.1,
/// §7.2): its person and the rights it acts with.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantCaller {
    pub grant_id: String,
    /// The person's public ID; `None` for a grant of the owner's row.
    pub person: Option<String>,
    /// The session the request names (`X-Clax-Session`), one of the
    /// grant's.
    pub session: Option<String>,
    /// `person ∩ grant narrowing`, and with a session `∩ its narrowing ∪
    /// its elevation` (the elevation only while the session is live, M2).
    pub rights: Rights,
}

/// What revoking a person ended.
#[derive(Debug, Default)]
pub struct RevokedPerson {
    pub grants: Vec<RevokedGrant>,
    /// Their invites, unused until now and now revoked.
    pub invites: Vec<String>,
}

const GRANT_SELECT: &str = "SELECT g.id, g.status, g.via, g.client_key, g.client_name, g.peer_ip,
        v.public_id, v.display_name, COALESCE(v.owner, 0), g.narrow_json, g.wrong_codes,
        g.created_at, g.decided_at, g.last_used_at, g.ended_at, g.ended_by
    FROM grants g LEFT JOIN viewers v ON v.id = g.viewer_id";

fn row_to_grant(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Grant, Option<String>)> {
    Ok((
        Grant {
            id: r.get(0)?,
            status: r.get(1)?,
            via: r.get(2)?,
            client_key: r.get(3)?,
            client_name: r.get(4)?,
            peer_ip: r.get(5)?,
            person: r.get(6)?,
            person_name: r.get(7)?,
            owner: r.get(8)?,
            narrow: None,
            wrong_codes: r.get(10)?,
            created_at: r.get(11)?,
            decided_at: r.get(12)?,
            last_used_at: r.get(13)?,
            ended_at: r.get(14)?,
            ended_by: r.get(15)?,
        },
        r.get(9)?,
    ))
}

fn with_narrow((mut g, narrow): (Grant, Option<String>)) -> Result<Grant> {
    g.narrow = narrow.as_deref().map(read_rights).transpose()?;
    Ok(g)
}

fn grant_in(c: &rusqlite::Connection, gid: &str) -> Result<Option<Grant>> {
    c.query_row(
        &format!("{GRANT_SELECT} WHERE g.id = ?1"),
        params![gid],
        row_to_grant,
    )
    .optional()?
    .map(with_narrow)
    .transpose()
}

/// Rights the store wrote.
fn read_rights(json: &str) -> Result<Rights> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| CoreError::invalid("invalid_rights", "stored rights do not parse"))?;
    Rights::parse_internal(&v)
}

fn write_rights(r: &Rights) -> String {
    r.to_json().to_string()
}

const INVITE_SELECT: &str = "SELECT i.id, v.public_id, v.display_name, v.owner, i.created_at,
        i.expires_at, i.used_at, i.grant_id, i.revoked_at
    FROM invites i JOIN viewers v ON v.id = i.viewer_id";

fn row_to_invite(r: &rusqlite::Row<'_>) -> rusqlite::Result<Invite> {
    Ok(Invite {
        id: r.get(0)?,
        person: r.get(1)?,
        person_name: r.get(2)?,
        owner: r.get(3)?,
        created_at: r.get(4)?,
        expires_at: r.get(5)?,
        used_at: r.get(6)?,
        grant_id: r.get(7)?,
        revoked_at: r.get(8)?,
    })
}

/// A new grant ID: `g_` and 22 lowercase Crockford base32 characters.
fn new_grant_id() -> String {
    let bytes = secret::random_bytes::<14>();
    let mut n = u128::from_le_bytes({
        let mut b = [0u8; 16];
        b[..14].copy_from_slice(&bytes);
        b
    });
    let mut out = String::from("g_");
    for _ in 0..22 {
        out.push(crate::ids::ALPHABET[(n & 31) as usize] as char);
        n >>= 5;
    }
    out
}

/// Checks `name` for a person (spec §11, §9.6): a valid, non-empty display
/// name that looks like ([`crate::names::CollisionKeys`]) neither the
/// owner's, nor another connected person's, nor that of a viewer never
/// invited and not revoked (other than viewer `except`; a revoked person's
/// or a lapsed invite's name is free again), nor a live agent's name.
/// Returns it trimmed.
fn check_person_name(tx: &Transaction<'_>, name: &str, except: Option<&str>) -> Result<String> {
    let Some(name) = checked_name(name)? else {
        return Err(CoreError::invalid("invalid_name", "a person needs a name"));
    };
    let form = CollisionKeys::of(name);
    let taken = || CoreError::invalid("name_taken", format!("the name '{name}' is taken"));
    let mut stmt = tx.prepare(
        "SELECT display_name FROM viewers WHERE display_name IS NOT NULL
           AND (owner = 1 OR (person = 1 AND person_revoked_at IS NULL)) AND id IS NOT ?1",
    )?;
    let names = stmt.query_map(params![except], |r| r.get::<_, String>(0))?;
    for other in names {
        if CollisionKeys::of(&other?).meets(&form) {
            return Err(taken());
        }
    }
    let mut stmt = tx.prepare(
        "SELECT v.display_name FROM viewers v WHERE v.display_name IS NOT NULL
           AND v.owner = 0 AND v.person = 0 AND v.person_revoked_at IS NULL AND v.id IS NOT ?1
           AND NOT EXISTS (SELECT 1 FROM invites WHERE viewer_id = v.id)",
    )?;
    let viewers = stmt.query_map(params![except], |r| r.get::<_, String>(0))?;
    for other in viewers {
        if CollisionKeys::of(&other?).meets(&form) {
            return Err(CoreError::invalid(
                "name_taken",
                format!(
                    "a viewer on your LAN is named '{name}': invite that viewer instead, or choose another name"
                ),
            ));
        }
    }
    if super::sessions::live_name_looks_like(tx, &form)? {
        return Err(taken());
    }
    Ok(name.to_string())
}

/// The rights of the person behind viewer row `viewer_id`: everything for
/// the owner, else the stored rights or the collaborator preset.
pub(super) fn rights_of_row(c: &rusqlite::Connection, viewer_id: &str) -> Result<Rights> {
    let (owner, json): (bool, Option<String>) = c
        .query_row(
            "SELECT owner, rights_json FROM viewers WHERE id = ?1",
            params![viewer_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or(CoreError::NotFound)?;
    if owner {
        return Ok(Rights::full());
    }
    match json {
        Some(j) => read_rights(&j),
        None => Ok(Rights::preset(crate::rights::PRESET_COLLABORATOR).expect("a preset")),
    }
}

/// The access a grant of viewer row `viewer_id` holds: the person's rights
/// within `narrow`.
fn grant_access(
    c: &rusqlite::Connection,
    viewer_id: &str,
    narrow: Option<&Rights>,
) -> Result<Rights> {
    let rights = rights_of_row(c, viewer_id)?;
    Ok(match narrow {
        Some(n) => rights.intersect(n),
        None => rights,
    })
}

/// A grant's narrowing from the access its client asked for: none when the
/// access covers the person's rights.
fn narrowing(person: &Rights, access: &Rights) -> Option<Rights> {
    (!person.is_narrower_or_equal(access)).then(|| access.clone())
}

/// Checks a client's name: at most [`MAX_CLIENT_NAME_CHARS`] characters, no
/// control characters. Returns it trimmed.
fn checked_client_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.chars().any(char::is_control) || n.chars().count() > MAX_CLIENT_NAME_CHARS {
        return Err(CoreError::invalid(
            "invalid_args",
            format!(
                "a client name is at most {MAX_CLIENT_NAME_CHARS} characters with no control characters"
            ),
        ));
    }
    Ok(n.to_string())
}

/// The tokens of a grant being approved.
struct Issued {
    token: Secret,
    manage: Secret,
    continue_token: Secret,
    browser_code: Option<Secret>,
}

/// Mints the access, manage and continuation tokens (and, unless `owner`,
/// a browser code redeemable until `browser_expires`) of grant `gid`,
/// storing their hashes.
fn issue(
    tx: &Transaction<'_>,
    gid: &str,
    owner: bool,
    now: &str,
    browser_expires: &str,
) -> Result<Issued> {
    let issued = Issued {
        token: secret::token(secret::ACCESS_PREFIX),
        manage: secret::token(secret::MANAGE_PREFIX),
        continue_token: secret::token(secret::CONTINUE_PREFIX),
        browser_code: (!owner).then(secret::browser_code),
    };
    tx.execute(
        "INSERT INTO grant_tokens (sha256, grant_id, created_at) VALUES (?1, ?2, ?3)",
        params![issued.token.sha256_hex(), gid, now],
    )?;
    tx.execute(
        "UPDATE grants SET manage_sha256 = ?2, continue_sha256 = ?3,
            browser_sha256 = ?4, browser_expires_at = CASE WHEN ?4 IS NULL THEN NULL ELSE ?5 END
         WHERE id = ?1",
        params![
            gid,
            issued.manage.sha256_hex(),
            issued.continue_token.sha256_hex(),
            issued.browser_code.as_ref().map(Secret::sha256_hex),
            browser_expires
        ],
    )?;
    Ok(issued)
}

/// The context of a lapse: Clax itself, for reason `expiry`.
const EXPIRY: AuditCtx = AuditCtx::system(SystemReason::Expiry);

/// A grant event (spec 2026-10-08-multi-agent-multi-person §20.2) of
/// `kind` for grant `gid` at `at`.
fn grant_record(kind: AuditKind, at: &str, gid: &str) -> AuditRecord {
    AuditRecord::new(kind, at).with("grant_id", gid)
}

/// `ctx` with `actor` in place of its own.
fn acting(ctx: &AuditCtx, actor: Actor) -> AuditCtx {
    AuditCtx {
        actor,
        ..ctx.clone()
    }
}

/// The client asking for grant `gid`, as an audit actor: before anyone
/// approved it, it is known only by its grant and the name it gave.
fn client_actor(c: &rusqlite::Connection, gid: &str) -> Result<Actor> {
    let client_name: String = c.query_row(
        "SELECT client_name FROM grants WHERE id = ?1",
        params![gid],
        |r| r.get(0),
    )?;
    Ok(Actor::Client {
        grant_id: gid.to_string(),
        client_name,
    })
}

/// The person grant `gid` is bound to, as an audit actor: a viewer marked
/// `person`, or the owner for a grant of the owner's row.
fn person_actor(c: &rusqlite::Connection, gid: &str) -> Result<Actor> {
    let (public_id, display_name, owner): (String, Option<String>, bool) = c.query_row(
        "SELECT v.public_id, v.display_name, v.owner FROM grants g
         JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
        params![gid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(if owner {
        Actor::Owner { public_id }
    } else {
        Actor::Viewer {
            public_id,
            display_name,
            person: true,
        }
    })
}

/// Records that grant `gid`, `was` before, lapsed: a request still pending
/// is denied (`expired`), an approved grant ends (`by` `expiry`); both as
/// `system:expiry`.
fn record_lapse(
    st: &Store,
    tx: &Transaction<'_>,
    gid: &str,
    was: &str,
    sessions_ended: &[String],
    now: &str,
) -> Result<()> {
    let rec = if was == "pending" {
        grant_record(AuditKind::GrantDeny, now, gid).with("reason", "expired")
    } else {
        grant_record(AuditKind::GrantRevoke, now, gid)
            .with("by", EndedBy::Expiry.as_str())
            .with("sessions_ended", sessions_ended.to_vec())
    };
    st.record_audit(tx, &EXPIRY, rec)?;
    Ok(())
}

/// Who ends grant `gid` from its own machine: its person, or for a
/// request no one approved, its client.
fn requester_actor(c: &rusqlite::Connection, gid: &str) -> Result<Actor> {
    let bound: bool = c.query_row(
        "SELECT viewer_id IS NOT NULL FROM grants WHERE id = ?1",
        params![gid],
        |r| r.get(0),
    )?;
    if bound {
        person_actor(c, gid)
    } else {
        client_actor(c, gid)
    }
}

/// Records `grant.request` for grant `gid` that `g` asked for through
/// `via` (`flow`: `invite` or `request`), under `ctx` (the client's).
fn record_request(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    gid: &str,
    via: &str,
    g: &NewGrant,
    now: &str,
) -> Result<()> {
    let client_name: String = tx.query_row(
        "SELECT client_name FROM grants WHERE id = ?1",
        params![gid],
        |r| r.get(0),
    )?;
    let rec = grant_record(AuditKind::GrantRequest, now, gid)
        .with("flow", via)
        .with("client_name", client_name)
        .with("peer_ip", g.peer_ip.as_str())
        .with("key_sha256", crate::audit::sha256_hex(&g.key))
        .with("access", g.access.to_json());
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Records `invite.revoke` of `invite` under `ctx`.
fn record_invite_revoke(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    invite: &Invite,
    now: &str,
) -> Result<()> {
    let rec = AuditRecord::new(AuditKind::InviteRevoke, now)
        .with("invite_id", invite.id.as_str())
        .with("person", invite.person.as_str())
        .with("expires_at", invite.expires_at.as_str());
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Ends live grant `gid` (`by`, with status `expired` for an expiry, else
/// `revoked`), revokes its tokens and ends its live sessions (`revoked`),
/// recording under `ctx` `grant.revoke` after its sessions' ends (a lapse
/// is recorded by [`record_lapse`]). A grant that has already ended ends
/// nothing and records nothing.
fn end_grant(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    gid: &str,
    by: EndedBy,
    now: &str,
) -> Result<RevokedGrant> {
    let mut out = RevokedGrant {
        grant_id: gid.to_string(),
        ..Default::default()
    };
    let status = if by == EndedBy::Expiry {
        "expired"
    } else {
        "revoked"
    };
    let was: Option<String> = tx
        .query_row(
            "SELECT status FROM grants WHERE id = ?1",
            params![gid],
            |r| r.get(0),
        )
        .optional()?;
    let n = tx.execute(
        "UPDATE grants SET status = ?2, ended_at = ?3, ended_by = ?4
         WHERE id = ?1 AND status IN ('pending', 'approved')",
        params![gid, status, now, by.as_str()],
    )?;
    if n == 0 {
        return Ok(out);
    }
    out.ended = true;
    // Its agents' rights end with it (M17).
    st.visibility_changed();
    tx.execute(
        "UPDATE grant_tokens SET revoked_at = ?2 WHERE grant_id = ?1 AND revoked_at IS NULL",
        params![gid, now],
    )?;
    let live: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM sessions WHERE grant_id = ?1 AND ended_at IS NULL
             ORDER BY started_at, id",
        )?;
        stmt.query_map(params![gid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for sid in live {
        if let Some(c) = st.end_in(tx, ctx, &sid, now, EndReason::Revoked, None)? {
            out.touched.merge(c.touched);
            out.withdrawn_questions.extend(c.withdrawn_questions);
            out.sessions_ended.push(sid);
        }
    }
    if by == EndedBy::Expiry {
        record_lapse(
            st,
            tx,
            gid,
            was.as_deref().unwrap_or_default(),
            &out.sessions_ended,
            now,
        )?;
    } else {
        let rec = grant_record(AuditKind::GrantRevoke, now, gid)
            .with("by", by.as_str())
            .with("sessions_ended", out.sessions_ended.clone());
        st.record_audit(tx, ctx, rec)?;
    }
    Ok(out)
}

/// A grant's stored hash of `column`, for checking a presented token.
fn stored_hash(tx: &Transaction<'_>, gid: &str, column: &str) -> Result<Option<String>> {
    tx.query_row(
        &format!("SELECT {column} FROM grants WHERE id = ?1"),
        params![gid],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()?
    .ok_or(CoreError::NotFound)
}

/// Accepts a continuation call signed at `t` with `nonce` on grant `gid`
/// unless an earlier `t` than the last accepted call's, or the same `t` and
/// nonce: a replay of an older call is refused here whatever became of its
/// nonce, and one in the same second by the nonce caches (and the stored
/// nonce, for the last call).
fn accept_t(tx: &Transaction<'_>, gid: &str, t: i64, nonce: &str) -> Result<()> {
    let n = tx.execute(
        "UPDATE grants SET continue_t = ?2, continue_nonce = ?3 WHERE id = ?1
           AND (continue_t IS NULL OR continue_t < ?2
                OR (continue_t = ?2 AND continue_nonce IS NOT ?3))",
        params![gid, t, nonce],
    )?;
    if n == 0 {
        return Err(CoreError::invalid(
            "replayed_request",
            "a continuation call must not be signed before the last one, nor repeat it",
        ));
    }
    Ok(())
}

fn invalid_token() -> CoreError {
    CoreError::invalid("invalid_token", "that token does not belong to this grant")
}

/// Whether `presented` hashes to `stored`, compared in constant time.
fn matches(presented: &str, stored: Option<&str>) -> bool {
    stored.is_some_and(|s| constant_time_eq(sha256_hex(presented).as_bytes(), s.as_bytes()))
}

impl Store {
    fn ago(&self, secs: i64) -> String {
        super::stamp(self.now_utc() - Duration::seconds(secs))
    }

    fn browser_expiry(&self) -> String {
        super::stamp(self.now_utc() + Duration::seconds(BROWSER_LINK_TTL_SECS))
    }

    /// Redeems a browser link's `code` (spec §6.5): once, within
    /// [`BROWSER_LINK_TTL_SECS`] of its issue, while its grant is approved
    /// and its person unrevoked. Returns the person's viewer row, whose `id`
    /// becomes the browser's `clax_viewer` cookie; the code is cleared.
    ///
    /// # Errors
    /// `NotFound` for an unknown, used or expired code (the link's 404),
    /// and for a revoked grant or person.
    pub fn redeem_browser_code(&self, code: &str) -> Result<Viewer> {
        let hash = sha256_hex(code);
        self.with_tx(|tx| {
            let now = self.now();
            let row: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id, browser_sha256, browser_expires_at, viewer_id FROM grants
                     WHERE browser_sha256 = ?1",
                    params![hash],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            // The lookup by the hash of a 32-byte random code is the check:
            // the index compares hashes, not the code.
            let Some((gid, _, expires, viewer_id)) = row else {
                return Ok(Err(CoreError::NotFound));
            };
            // One use, live or not: a code that was presented is spent.
            tx.execute(
                "UPDATE grants SET browser_sha256 = NULL, browser_expires_at = NULL WHERE id = ?1",
                params![gid],
            )?;
            if expires <= now {
                return Ok(Err(CoreError::NotFound));
            }
            let viewer = tx
                .query_row(
                    "SELECT v.id, v.public_id, v.display_name, v.created_at, v.person, v.person_revoked_at
                     FROM viewers v JOIN grants g ON g.viewer_id = v.id
                     WHERE v.id = ?1 AND g.id = ?2 AND g.status = 'approved'
                       AND v.owner = 0 AND v.person = 1 AND v.person_revoked_at IS NULL",
                    params![viewer_id, gid],
                    row_to_viewer,
                )
                .optional()?;
            Ok(viewer.ok_or(CoreError::NotFound))
        })?
    }

    /// Makes an invite for `who` (spec §6.2), recording `invite.create`
    /// under `ctx`: a new person row for `New`,
    /// the viewer marked a person for `Viewer`, the owner row (made when
    /// missing) for `Owner`. Returns it with its code, shown once: 12
    /// Crockford base32 characters in three groups. It expires in 24 hours.
    /// `rights` (a person's; ignored for `Owner`, whose rights are all) are
    /// set in the same transaction, `None` keeping the person's.
    ///
    /// A viewer or new person made a person here goes back to a plain
    /// viewer when its invites are revoked or expire without its ever
    /// holding a grant ([`Store::revoke_invite`], [`Store::expire_grants`]).
    ///
    /// # Errors
    /// `limit_reached` with [`MAX_LIVE_INVITES`] live; `invalid_name` or
    /// `name_taken` for a new person's name (spec §11), and `name_taken`
    /// for a viewer whose name a connected person or the owner has;
    /// `unknown_viewer` for a public ID that names no viewer, or the owner.
    pub fn create_invite(
        &self,
        ctx: &AuditCtx,
        who: InvitePerson,
        rights: Option<Rights>,
    ) -> Result<(Invite, Secret)> {
        let code = secret::invite_code();
        let canon = secret::canonical_invite(code.expose()).expect("a minted code is canonical");
        self.with_tx(|tx| {
            let now = self.now();
            let live: i64 = tx.query_row(
                "SELECT COUNT(*) FROM invites
                 WHERE used_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
                params![now],
                |r| r.get(0),
            )?;
            if live as usize >= MAX_LIVE_INVITES {
                return Err(CoreError::invalid(
                    "limit_reached",
                    format!("at most {MAX_LIVE_INVITES} invites are live at once"),
                ));
            }
            let viewer_id = match &who {
                InvitePerson::New(name) => {
                    let name = check_person_name(tx, name, None)?;
                    let id = new_ulid();
                    tx.execute(
                        "INSERT INTO viewers (id, public_id, display_name, created_at, person)
                         VALUES (?1, ?2, ?3, ?4, 1)",
                        params![id, new_public_id(), name, now],
                    )?;
                    // A new person: not `*unconnected` (M14, M17).
                    self.visibility_changed();
                    id
                }
                InvitePerson::Viewer(public_id) => {
                    let row: Option<(String, Option<String>)> = tx
                        .query_row(
                            "SELECT id, display_name FROM viewers WHERE public_id = ?1 AND owner = 0",
                            params![public_id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let Some((id, name)) = row else {
                        return Err(CoreError::invalid(
                            "unknown_viewer",
                            "that public ID names no viewer",
                        ));
                    };
                    if let Some(name) = name {
                        check_person_name(tx, &name, Some(&id))?;
                    }
                    if tx.execute(
                        "UPDATE viewers SET person = 1 WHERE id = ?1 AND person = 0",
                        params![id],
                    )? > 0
                    {
                        // A viewer made a person: no longer `*unconnected`.
                        self.visibility_changed();
                    }
                    id
                }
                InvitePerson::Owner => {
                    if tx.execute(
                        "INSERT INTO viewers (id, public_id, display_name, created_at, owner, claimed)
                         SELECT ?1, ?2, NULL, ?3, 1, 0
                         WHERE NOT EXISTS (SELECT 1 FROM viewers WHERE owner = 1)",
                        params![new_ulid(), new_public_id(), now],
                    )? > 0
                    {
                        // The owner is connected from now on (§9.6).
                        self.visibility_changed();
                    }
                    tx.query_row("SELECT id FROM viewers WHERE owner = 1", [], |r| r.get(0))?
                }
            };
            if let (Some(r), false) = (&rights, who == InvitePerson::Owner)
                && tx.execute(
                    "UPDATE viewers SET rights_json = ?2 WHERE id = ?1 AND rights_json IS NOT ?2",
                    params![viewer_id, write_rights(r)],
                )? > 0
            {
                self.visibility_changed();
            }
            let id = new_ulid();
            tx.execute(
                "INSERT INTO invites (id, code_sha256, viewer_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    sha256_hex(&canon),
                    viewer_id,
                    now,
                    super::stamp(self.now_utc() + Duration::seconds(INVITE_TTL_SECS))
                ],
            )?;
            let invite = tx.query_row(
                &format!("{INVITE_SELECT} WHERE i.id = ?1"),
                params![id],
                row_to_invite,
            )?;
            let mut rec = AuditRecord::new(AuditKind::InviteCreate, now.as_str())
                .with("invite_id", invite.id.as_str())
                .with("person", invite.person.as_str())
                .with("expires_at", invite.expires_at.as_str());
            if let (Some(r), false) = (&rights, who == InvitePerson::Owner) {
                rec = rec.with("rights", r.to_json());
            }
            self.record_audit(tx, ctx, rec)?;
            Ok((invite, code.clone()))
        })
    }

    /// Every invite, newest first.
    pub fn list_invites(&self) -> Result<Vec<Invite>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{INVITE_SELECT} ORDER BY i.created_at DESC, i.id DESC"
            ))?;
            Ok(stmt
                .query_map([], row_to_invite)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Revokes invite `id` unless it was used or revoked already, recording
    /// `invite.revoke` under `ctx` when it did. A person
    /// it made who never held a grant and has no other live invite goes
    /// back to a plain viewer.
    ///
    /// # Errors
    /// `NotFound` when no such invite exists.
    pub fn revoke_invite(&self, ctx: &AuditCtx, id: &str) -> Result<Invite> {
        self.with_tx(|tx| {
            let now = self.now();
            let n = tx.execute(
                "UPDATE invites SET revoked_at = ?2
                 WHERE id = ?1 AND used_at IS NULL AND revoked_at IS NULL",
                params![id, now],
            )?;
            let viewer: Option<String> = tx
                .query_row(
                    "SELECT viewer_id FROM invites WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(v) = viewer
                && release_unconnected(self, tx, ctx, Some(&v), &now)? > 0
            {
                // Back to `*unconnected` (M14, M17).
                self.visibility_changed();
            }
            let invite = tx
                .query_row(
                    &format!("{INVITE_SELECT} WHERE i.id = ?1"),
                    params![id],
                    row_to_invite,
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            if n > 0 {
                record_invite_revoke(self, tx, ctx, &invite, &now)?;
            }
            Ok(invite)
        })
    }

    /// Takes a grant request (spec §6.3), recording `grant.request` (and, for
    /// an invite, `grant.approve` and `grant.token`) as the requesting
    /// client, through `ctx`'s channel. With an invite code, a live,
    /// unused invite binds the grant at once to its person (its use binds
    /// this key; the access asked for can only narrow the person's rights).
    /// Without one, a pending grant waits for the owner, who approves it
    /// with the user code returned here.
    ///
    /// # Errors
    /// `invalid_invite` for an expired, used, revoked or unknown code (one
    /// message for all four); `limit_reached` with [`MAX_PENDING_PER_IP`]
    /// requests from the peer in the last ten minutes, cancelled ones
    /// included, or [`MAX_PENDING`] in all;
    /// `key_in_use` when a live grant holds the key; `invalid_args` for a
    /// client name over [`MAX_CLIENT_NAME_CHARS`] characters or with control
    /// characters.
    pub fn request_grant(&self, ctx: &AuditCtx, g: NewGrant) -> Result<GrantStart> {
        let client_name = checked_client_name(&g.client_name)?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(g.key);
        let invite_hash = g
            .invite
            .as_ref()
            .map(|code| secret::canonical_invite(code.expose()).map(|c| sha256_hex(&c)));
        self.with_tx(|tx| {
            let now = self.now();
            // Unfinished requests past their time free their keys now, and
            // record their lapse; `expire_grants` reports them (`ended_at`
            // is set then).
            let stale: Vec<(String, String)> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT id, status FROM grants WHERE {UNFINISHED_STALE} ORDER BY id"
                ))?;
                stmt.query_map(params![self.ago(PENDING_TTL_SECS)], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            for (gid, was) in &stale {
                tx.execute(
                    "UPDATE grants SET status = 'expired', ended_by = 'expiry' WHERE id = ?1",
                    params![gid],
                )?;
                record_lapse(self, tx, gid, was, &[], &now)?;
            }
            let invite = match invite_hash {
                None => None,
                Some(hash) => {
                    let found: Option<(String, String)> = match hash {
                        Some(h) => tx
                            .query_row(
                                "SELECT id, viewer_id FROM invites WHERE code_sha256 = ?1
                                   AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?2",
                                params![h, now],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .optional()?,
                        None => None,
                    };
                    Some(found.ok_or_else(|| {
                        CoreError::invalid(
                            "invalid_invite",
                            "that invite is not valid: ask for a new one",
                        )
                    })?)
                }
            };
            let in_use: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM grants WHERE client_key = ?1
                                  AND status IN ('pending', 'approved'))",
                params![key],
                |r| r.get(0),
            )?;
            if in_use {
                return Err(CoreError::invalid(
                    "key_in_use",
                    "a live grant already holds this key",
                ));
            }
            let gid = new_grant_id();
            match invite {
                Some((invite_id, viewer_id)) => {
                    let owner: bool = tx.query_row(
                        "SELECT owner FROM viewers WHERE id = ?1",
                        params![viewer_id],
                        |r| r.get(0),
                    )?;
                    if !owner
                        && tx.execute(
                            "UPDATE viewers SET person = 1, person_revoked_at = NULL
                             WHERE id = ?1 AND (person = 0 OR person_revoked_at IS NOT NULL)",
                            params![viewer_id],
                        )? > 0
                    {
                        // Connected again (M17).
                        self.visibility_changed();
                    }
                    let narrow = narrowing(&rights_of_row(tx, &viewer_id)?, &g.access);
                    tx.execute(
                        "INSERT INTO grants (id, viewer_id, status, via, client_key, client_name, peer_ip,
                            continue_sha256, narrow_json, created_at, decided_at)
                         VALUES (?1, ?2, 'approved', 'invite', ?3, ?4, ?5, '', ?6, ?7, ?7)",
                        params![
                            gid,
                            viewer_id,
                            key,
                            client_name,
                            g.peer_ip,
                            narrow.as_ref().map(write_rights),
                            now
                        ],
                    )?;
                    let issued = issue(tx, &gid, owner, &now, &self.browser_expiry())?;
                    tx.execute(
                        "UPDATE invites SET used_at = ?2, grant_id = ?3 WHERE id = ?1",
                        params![invite_id, now, gid],
                    )?;
                    let grant = grant_in(tx, &gid)?.expect("the grant was written");
                    let access = grant_access(tx, &viewer_id, grant.narrow.as_ref())?;
                    let client = acting(ctx, client_actor(tx, &gid)?);
                    record_request(self, tx, &client, &gid, "invite", &g, &now)?;
                    let rec = grant_record(AuditKind::GrantApprove, &now, &gid)
                        .with("person", grant.person.clone())
                        .with("access", access.to_json())
                        .with("code_checked", false);
                    self.record_audit(tx, &client, rec)?;
                    let rec = grant_record(AuditKind::GrantToken, &now, &gid).with("op", "issue");
                    self.record_audit(tx, &client, rec)?;
                    // The owner hears that a machine connected (spec §3.1).
                    super::inbox::note_connection(
                        tx,
                        &gid,
                        &client_name,
                        &g.peer_ip,
                        grant.person_name.as_deref(),
                        &now,
                    )?;
                    Ok(GrantStart::Approved {
                        access,
                        grant,
                        token: issued.token,
                        manage: issued.manage,
                        continue_token: issued.continue_token,
                        browser_code: issued.browser_code,
                    })
                }
                None => {
                    // Every request of the last PENDING_TTL_SECS counts,
                    // whatever became of it: a requester that cancels its
                    // request and asks again stays within the caps.
                    let (from_peer, all): (i64, i64) = tx.query_row(
                        "SELECT COALESCE(SUM(peer_ip = ?1), 0), COUNT(*) FROM grants
                         WHERE via = 'request' AND created_at >= ?2",
                        params![g.peer_ip, self.ago(PENDING_TTL_SECS)],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    if from_peer as usize >= MAX_PENDING_PER_IP || all as usize >= MAX_PENDING {
                        return Err(CoreError::invalid(
                            "limit_reached",
                            "too many requests are waiting for the owner: try again later",
                        ));
                    }
                    let user_code = secret::user_code();
                    let continue_token = secret::token(secret::CONTINUE_PREFIX);
                    let canon = secret::canonical_user_code(user_code.expose())
                        .expect("a minted code is canonical");
                    tx.execute(
                        "INSERT INTO grants (id, status, via, client_key, client_name, peer_ip,
                            user_code_sha256, continue_sha256, narrow_json, created_at)
                         VALUES (?1, 'pending', 'request', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            gid,
                            key,
                            client_name,
                            g.peer_ip,
                            sha256_hex(&canon),
                            continue_token.sha256_hex(),
                            write_rights(&g.access),
                            now
                        ],
                    )?;
                    let client = acting(ctx, client_actor(tx, &gid)?);
                    record_request(self, tx, &client, &gid, "request", &g, &now)?;
                    // A request waits for the owner in their inbox (spec §6.3).
                    super::inbox::note_connection(tx, &gid, &client_name, &g.peer_ip, None, &now)?;
                    Ok(GrantStart::Pending {
                        grant: grant_in(tx, &gid)?.expect("the grant was written"),
                        continue_token,
                        user_code,
                    })
                }
            }
        })
    }

    /// Approves pending request `gid` under `ctx` (recording
    /// `grant.approve`, or `grant.deny` for the wrong code that denies it)
    /// as a new person named `name` with
    /// `rights` (`None`: the collaborator preset), when `user_code` is the
    /// request's (spec §6.3). A request never becomes an existing person or
    /// the owner. The tokens are issued to the client's next continuation
    /// poll.
    ///
    /// # Errors
    /// `wrong_code` for any other code, which changes nothing but the count:
    /// the [`MAX_WRONG_CODES`]th denies the request. `not_pending` for a
    /// request that was decided, ended or has expired; `invalid_name` or
    /// `name_taken` for the name (spec §11); `NotFound` for an unknown grant.
    pub fn approve_grant(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        user_code: &str,
        name: &str,
        rights: Option<Rights>,
    ) -> Result<Grant> {
        let presented = secret::canonical_user_code(user_code);
        // The outer result rolls the transaction back; the inner one is
        // the outcome of a transaction that commits (a wrong code counts).
        self.with_tx(|tx| {
            let now = self.now();
            let row: Option<(String, String, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT status, created_at, user_code_sha256, narrow_json FROM grants WHERE id = ?1",
                    params![gid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let (status, created_at, code_hash, asked) = row.ok_or(CoreError::NotFound)?;
            if status != "pending" || created_at < self.ago(PENDING_TTL_SECS) {
                return Err(CoreError::invalid(
                    "not_pending",
                    "this request is no longer pending",
                ));
            }
            if !presented
                .as_deref()
                .is_some_and(|p| matches(p, code_hash.as_deref()))
            {
                let wrong: i64 = tx.query_row(
                    "UPDATE grants SET wrong_codes = wrong_codes + 1 WHERE id = ?1 RETURNING wrong_codes",
                    params![gid],
                    |r| r.get(0),
                )?;
                let denied = wrong >= MAX_WRONG_CODES;
                if denied {
                    tx.execute(
                        "UPDATE grants SET status = 'denied', decided_at = ?2, user_code_sha256 = NULL
                         WHERE id = ?1",
                        params![gid, now],
                    )?;
                    let rec =
                        grant_record(AuditKind::GrantDeny, &now, gid).with("reason", "wrong_code");
                    self.record_audit(tx, ctx, rec)?;
                    super::inbox::connection_decided(tx, gid, &now)?;
                }
                return Ok(Err(CoreError::invalid(
                    "wrong_code",
                    if denied {
                        "that is not the request's code; the request is now denied"
                    } else {
                        "that is not the request's code"
                    },
                )));
            }
            let name = check_person_name(tx, name, None)?;
            self.visibility_changed();
            let viewer_id = new_ulid();
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, person, rights_json)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![
                    viewer_id,
                    new_public_id(),
                    name,
                    now,
                    rights.as_ref().map(write_rights)
                ],
            )?;
            let asked = asked.as_deref().map(read_rights).transpose()?;
            let narrow = match asked {
                Some(a) => narrowing(&rights_of_row(tx, &viewer_id)?, &a),
                None => None,
            };
            tx.execute(
                "UPDATE grants SET viewer_id = ?2, status = 'approved', decided_at = ?3,
                    user_code_sha256 = NULL, narrow_json = ?4, last_used_at = NULL
                 WHERE id = ?1",
                params![gid, viewer_id, now, narrow.as_ref().map(write_rights)],
            )?;
            let grant = grant_in(tx, gid)?.expect("the grant exists");
            let rec = grant_record(AuditKind::GrantApprove, &now, gid)
                .with("person", grant.person.clone())
                .with(
                    "access",
                    grant_access(tx, &viewer_id, grant.narrow.as_ref())?.to_json(),
                )
                .with("code_checked", true);
            self.record_audit(tx, ctx, rec)?;
            super::inbox::connection_decided(tx, gid, &now)?;
            Ok(Ok(grant))
        })?
    }

    /// Denies pending request `gid`, recording `grant.deny` under `ctx`.
    ///
    /// # Errors
    /// `not_pending` unless it is pending; `NotFound` for an unknown grant.
    pub fn deny_grant(&self, ctx: &AuditCtx, gid: &str) -> Result<Grant> {
        self.with_tx(|tx| {
            let now = self.now();
            let n = tx.execute(
                "UPDATE grants SET status = 'denied', decided_at = ?2, user_code_sha256 = NULL
                 WHERE id = ?1 AND status = 'pending'",
                params![gid, now],
            )?;
            if n > 0 {
                let rec = grant_record(AuditKind::GrantDeny, &now, gid).with("reason", "owner");
                self.record_audit(tx, ctx, rec)?;
                super::inbox::connection_decided(tx, gid, &now)?;
            }
            let g = grant_in(tx, gid)?.ok_or(CoreError::NotFound)?;
            if n == 0 {
                return Err(CoreError::invalid(
                    "not_pending",
                    "this request is no longer pending",
                ));
            }
            Ok(g)
        })
    }

    /// A client's continuation poll of grant `gid` with `continue_token`
    /// (spec §6.3), through `ctx`'s channel: an issue records `grant.token`
    /// as the client, a lapse `grant.deny` or `grant.revoke` as
    /// `system:expiry`. Pending: [`ContinueOutcome::Pending`], at most once per
    /// [`CONTINUE_WAIT_SECS`]. Approved and not yet issued: the tokens and
    /// a new continuation token, once.
    ///
    /// # Errors
    /// `invalid_token` for another token; `too_fast` for a poll sooner than
    /// the wait after the last (or the request); `denied` for a grant
    /// denied, revoked or expired (a pending one past [`PENDING_TTL_SECS`]
    /// expires here); `already_issued` for an approved grant whose tokens
    /// were handed out; `replayed_request` when `t`, the call's signed time,
    /// is before the last accepted continuation call's, or equals it with
    /// the same `nonce` (tested after `too_fast`); `NotFound` for an unknown
    /// grant.
    pub fn continue_grant(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        continue_token: &str,
        t: i64,
        nonce: &str,
    ) -> Result<ContinueOutcome> {
        let denied = || CoreError::invalid("denied", "this request was denied or has expired");
        // As in `approve_grant`: the inner error commits (an expiry).
        self.with_tx(|tx| {
            let now = self.now();
            if !matches(continue_token, stored_hash(tx, gid, "continue_sha256")?.as_deref()) {
                return Err(invalid_token());
            }
            let (status, created_at, last, manage, decided): (
                String,
                String,
                Option<String>,
                Option<String>,
                String,
            ) = tx.query_row(
                "SELECT status, created_at, last_used_at, manage_sha256, COALESCE(decided_at, '')
                 FROM grants WHERE id = ?1",
                params![gid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
            match status.as_str() {
                "pending" => {
                    if created_at < self.ago(PENDING_TTL_SECS) {
                        // Reported, and given `ended_at`, by `expire_grants`.
                        tx.execute(
                            "UPDATE grants SET status = 'expired', ended_by = 'expiry' WHERE id = ?1",
                            params![gid],
                        )?;
                        record_lapse(self, tx, gid, "pending", &[], &now)?;
                        return Ok(Err(denied()));
                    }
                    if last.unwrap_or(created_at) > self.ago(CONTINUE_WAIT_SECS) {
                        return Err(CoreError::invalid(
                            "too_fast",
                            format!("poll at most once every {CONTINUE_WAIT_SECS} seconds"),
                        ));
                    }
                    accept_t(tx, gid, t, nonce)?;
                    tx.execute(
                        "UPDATE grants SET last_used_at = ?2 WHERE id = ?1",
                        params![gid, now],
                    )?;
                    Ok(Ok(ContinueOutcome::Pending {
                        grant: grant_in(tx, gid)?.expect("the grant exists"),
                    }))
                }
                "approved" if manage.is_none() && decided < self.ago(PENDING_TTL_SECS) => {
                    // Reported, and given `ended_at`, by `expire_grants`.
                    tx.execute(
                        "UPDATE grants SET status = 'expired', ended_by = 'expiry' WHERE id = ?1",
                        params![gid],
                    )?;
                    record_lapse(self, tx, gid, "approved", &[], &now)?;
                    Ok(Err(denied()))
                }
                "approved" if manage.is_none() => {
                    accept_t(tx, gid, t, nonce)?;
                    let owner = grant_in(tx, gid)?.expect("the grant exists").owner;
                    let issued = issue(tx, gid, owner, &now, &self.browser_expiry())?;
                    tx.execute(
                        "UPDATE grants SET last_used_at = ?2 WHERE id = ?1",
                        params![gid, now],
                    )?;
                    let rec = grant_record(AuditKind::GrantToken, &now, gid).with("op", "issue");
                    self.record_audit(tx, &acting(ctx, client_actor(tx, gid)?), rec)?;
                    let grant = grant_in(tx, gid)?.expect("the grant exists");
                    let viewer_id: String = tx.query_row(
                        "SELECT viewer_id FROM grants WHERE id = ?1",
                        params![gid],
                        |r| r.get(0),
                    )?;
                    Ok(Ok(ContinueOutcome::Approved {
                        access: grant_access(tx, &viewer_id, grant.narrow.as_ref())?,
                        grant,
                        token: issued.token,
                        manage: issued.manage,
                        continue_token: issued.continue_token,
                        browser_code: issued.browser_code,
                    }))
                }
                "approved" => Err(CoreError::invalid(
                    "already_issued",
                    "this grant's tokens were already issued",
                )),
                _ => Err(denied()),
            }
        })?
    }

    /// Revokes grant `gid` through its manage token (spec §6.4): the access
    /// token goes, and with it the whole grant, as [`Store::revoke_grant`]
    /// by the person, recorded as the person through `ctx`'s channel.
    ///
    /// # Errors
    /// `invalid_token` unless `manage_token` is the grant's; `NotFound` for
    /// an unknown grant.
    pub fn revoke_token(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        manage_token: &str,
    ) -> Result<RevokedGrant> {
        self.with_tx(|tx| {
            if !matches(
                manage_token,
                stored_hash(tx, gid, "manage_sha256")?.as_deref(),
            ) {
                return Err(invalid_token());
            }
            let by = acting(ctx, requester_actor(tx, gid)?);
            end_grant(self, tx, &by, gid, EndedBy::Person, &self.now())
        })
    }

    /// Ends grant `gid` through its continuation (`DELETE <continue uri>`,
    /// the person's `clax connect --off`), pending or approved, as
    /// [`Store::revoke_grant`] by the person, recorded as the person (the
    /// client, for a request no one approved) through `ctx`'s channel.
    ///
    /// # Errors
    /// `invalid_token` unless `continue_token` is the grant's current one;
    /// `replayed_request` when `t`, the call's signed time, is before the
    /// last accepted continuation call's, or equals it with the same
    /// `nonce`; `NotFound` for an unknown grant.
    pub fn disconnect_grant(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        continue_token: &str,
        t: i64,
        nonce: &str,
    ) -> Result<RevokedGrant> {
        self.with_tx(|tx| {
            if !matches(
                continue_token,
                stored_hash(tx, gid, "continue_sha256")?.as_deref(),
            ) {
                return Err(invalid_token());
            }
            accept_t(tx, gid, t, nonce)?;
            let by = acting(ctx, requester_actor(tx, gid)?);
            end_grant(self, tx, &by, gid, EndedBy::Person, &self.now())
        })
    }

    /// Records the answer `clax connect` gives for grant `gid`'s person,
    /// through the grant's current continuation token (spec §3.1, M15):
    /// "Let <owner>'s comments and pages reach your agents everywhere?". The
    /// person approves the owner's content everywhere (`created_via`
    /// `connect`), recorded as the person through `ctx`'s channel. It is the
    /// one approval made without a browser (§21 item 3), so it is bounded
    /// to the connect: once per grant, within [`OWNER_ANSWER_SECS`] of the
    /// tokens' issue, and never once the person has made or revoked any
    /// approval of the owner. `None` for a grant of the owner's own row,
    /// whose agents are the owner's.
    ///
    /// # Errors
    /// `invalid_token` unless `continue_token` is the grant's current one;
    /// `not_issued` unless the grant is approved and its tokens issued;
    /// `replayed_request` as [`Store::disconnect_grant`] (tested before the
    /// rest, so a replay is told as one); `answer_closed` past
    /// [`OWNER_ANSWER_SECS`] of the issue; `owner_answered` when the person
    /// has an approval of the owner, live or revoked; `NotFound` for an
    /// unknown grant; [`Store::approve`]'s errors.
    pub fn approve_owner_on_connect(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        continue_token: &str,
        t: i64,
        nonce: &str,
    ) -> Result<Option<(super::approvals::Approval, crate::feedback::Touched)>> {
        let owner = self.owner_viewer(false)?;
        let owner_pid = owner.public_id.clone();
        let found = self.with_tx(|tx| {
            if !matches(
                continue_token,
                stored_hash(tx, gid, "continue_sha256")?.as_deref(),
            ) {
                return Err(invalid_token());
            }
            let issued: bool = tx.query_row(
                "SELECT status = 'approved' AND manage_sha256 IS NOT NULL FROM grants WHERE id = ?1",
                params![gid],
                |r| r.get(0),
            )?;
            if !issued {
                return Err(CoreError::invalid(
                    "not_issued",
                    "this grant has no live tokens: connect first",
                ));
            }
            accept_t(tx, gid, t, nonce)?;
            let (person, owner): (String, bool) = tx.query_row(
                "SELECT v.public_id, v.owner FROM grants g
                 JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
                params![gid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if owner {
                return Ok(None);
            }
            let issued_at: Option<String> = tx.query_row(
                "SELECT MIN(created_at) FROM grant_tokens WHERE grant_id = ?1",
                params![gid],
                |r| r.get(0),
            )?;
            if issued_at.is_none_or(|at| at < self.ago(OWNER_ANSWER_SECS)) {
                return Err(CoreError::invalid(
                    "answer_closed",
                    "the question about the owner is answered while connecting: allow the owner's content in People",
                ));
            }
            let answered: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM approvals WHERE approver = ?1 AND author = ?2)",
                params![person, owner_pid],
                |r| r.get(0),
            )?;
            if answered {
                return Err(CoreError::invalid(
                    "owner_answered",
                    "this person has already decided about the owner's content: change it in People",
                ));
            }
            Ok(Some((person, person_actor(tx, gid)?)))
        })?;
        let Some((person, actor)) = found else {
            return Ok(None);
        };
        self.approve_touched(
            &acting(ctx, actor),
            Some(&person),
            &owner.public_id,
            super::approvals::Scope::Everywhere,
            super::approvals::Via::Connect,
        )
        .map(Some)
    }

    /// Ends grant `gid` (`by`): its tokens are revoked and its live sessions
    /// end (`revoked`), releasing their feedback and withdrawing their
    /// questions; the key may connect again. Recorded under `ctx`
    /// (`grant.revoke`). A grant that has ended already ends nothing more.
    ///
    /// # Errors
    /// `NotFound` for an unknown grant.
    pub fn revoke_grant(&self, ctx: &AuditCtx, gid: &str, by: EndedBy) -> Result<RevokedGrant> {
        self.with_tx(|tx| {
            grant_in(tx, gid)?.ok_or(CoreError::NotFound)?;
            end_grant(self, tx, ctx, gid, by, &self.now())
        })
    }

    /// Revokes person `public_id` (spec §6.4): their live grants end (by the
    /// owner), their unused invites are revoked, and `person_revoked_at` is
    /// set. Their viewer row and authorship stay. Recorded under `ctx`: each
    /// grant's `grant.revoke`, each invite's `invite.revoke`, then
    /// `person.revoke` (once).
    ///
    /// # Errors
    /// `not_a_person` for the owner or a viewer who is not a person;
    /// `NotFound` for an unknown public ID.
    pub fn revoke_person(&self, ctx: &AuditCtx, public_id: &str) -> Result<RevokedPerson> {
        self.with_tx(|tx| {
            let now = self.now();
            let (id, person, owner): (String, bool, bool) = tx
                .query_row(
                    "SELECT id, person, owner FROM viewers WHERE public_id = ?1",
                    params![public_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            if !person || owner {
                return Err(CoreError::invalid(
                    "not_a_person",
                    "only a connected person can be revoked",
                ));
            }
            let mut out = RevokedPerson::default();
            let grants: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT id FROM grants WHERE viewer_id = ?1 AND status IN ('pending', 'approved')
                     ORDER BY created_at, id",
                )?;
                stmt.query_map(params![id], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for gid in grants {
                out.grants
                    .push(end_grant(self, tx, ctx, &gid, EndedBy::Owner, &now)?);
            }
            {
                let mut stmt = tx.prepare(
                    "UPDATE invites SET revoked_at = ?2
                     WHERE viewer_id = ?1 AND used_at IS NULL AND revoked_at IS NULL RETURNING id",
                )?;
                out.invites = stmt
                    .query_map(params![id, now], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
            }
            for invite in &out.invites {
                let i = tx.query_row(
                    &format!("{INVITE_SELECT} WHERE i.id = ?1"),
                    params![invite],
                    row_to_invite,
                )?;
                record_invite_revoke(self, tx, ctx, &i, &now)?;
            }
            let n = tx.execute(
                "UPDATE viewers SET person_revoked_at = ?2 WHERE id = ?1 AND person_revoked_at IS NULL",
                params![id, now],
            )?;
            if n > 0 {
                super::approvals::withdraw_person_in(self, tx, ctx, public_id, &now)?;
                super::elevations::drop_granted_in(self, tx, ctx, public_id, "person_revoked")?;
                let rec = AuditRecord::new(AuditKind::PersonRevoke, now.as_str())
                    .with("person", public_id);
                self.record_audit(tx, ctx, rec)?;
                self.visibility_changed();
            }
            Ok(out)
        })
    }

    /// Sets (or with `None` clears) the narrowing of every agent of live
    /// grant `gid`, recording `grant.narrow` under `ctx`.
    ///
    /// # Errors
    /// `NotFound` unless `gid` names a pending or approved grant;
    /// `not_narrower` for a narrowing of a grant with a person that allows
    /// anything the person's rights do not.
    pub fn narrow_grant(&self, ctx: &AuditCtx, gid: &str, narrow: Option<Rights>) -> Result<Grant> {
        self.with_tx(|tx| {
            if let Some(viewer_id) = tx
                .query_row(
                    "SELECT viewer_id FROM grants WHERE id = ?1 AND viewer_id IS NOT NULL",
                    params![gid],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                check_narrower(narrow.as_ref(), &rights_of_row(tx, &viewer_id)?)?;
            }
            let json = narrow.as_ref().map(write_rights);
            let was: Option<Option<String>> = tx
                .query_row(
                    "SELECT narrow_json FROM grants WHERE id = ?1 AND status IN ('pending', 'approved')",
                    params![gid],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(was) = was else {
                return Err(CoreError::NotFound);
            };
            tx.execute(
                "UPDATE grants SET narrow_json = ?2 WHERE id = ?1",
                params![gid, json],
            )?;
            let rec = grant_record(AuditKind::GrantNarrow, &self.now(), gid)
                .with("narrow", narrow.as_ref().map(Rights::to_json));
            self.record_audit(tx, ctx, rec)?;
            if was != json {
                self.visibility_changed();
            }
            Ok(grant_in(tx, gid)?.expect("the grant exists"))
        })
    }

    /// Grant `gid`.
    ///
    /// # Errors
    /// `NotFound` for an unknown grant.
    pub fn grant(&self, gid: &str) -> Result<Grant> {
        self.with_read(|c| grant_in(c, gid)?.ok_or(CoreError::NotFound))
    }

    /// Every grant, newest first.
    pub fn list_grants(&self) -> Result<Vec<Grant>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{GRANT_SELECT} ORDER BY g.created_at DESC, g.id DESC"
            ))?;
            let rows = stmt
                .query_map([], row_to_grant)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(with_narrow).collect()
        })
    }

    /// The viewer row grant `gid` is bound to (the owner's for the owner's
    /// machines).
    ///
    /// # Errors
    /// `NotFound` for an unknown or pending grant.
    pub fn viewer_of_grant(&self, gid: &str) -> Result<Viewer> {
        self.with_read(|c| {
            c.query_row(
                "SELECT v.id, v.public_id, v.display_name, v.created_at, v.person, v.person_revoked_at
                 FROM grants g JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
                params![gid],
                row_to_viewer,
            )
            .optional()?
            .ok_or(CoreError::NotFound)
        })
    }

    /// Every live access token's SHA-256 with its grant: the daemon's
    /// in-memory index, loaded on start and after every change.
    pub fn token_index(&self) -> Result<Vec<([u8; 32], String)>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT t.sha256, t.grant_id FROM grant_tokens t JOIN grants g ON g.id = t.grant_id
                 WHERE t.revoked_at IS NULL AND g.status = 'approved'",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(hex, gid)| Some((unhex32(&hex)?, gid)))
                .collect())
        })
    }

    /// Every token of every grant whose tokens were issued, by hash, for
    /// the daemon's in-memory index (spec §6.4): access tokens, manage
    /// tokens and current continuation tokens, each with its grant's key. A
    /// revoked token, and every token of a grant that has ended, is marked
    /// `revoked`. A request that never had tokens issued is not in it: its
    /// continuation is checked against the store
    /// ([`Store::unissued_continuation`]).
    pub fn credential_index(&self) -> Result<Vec<Credential>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT t.sha256, 'access', g.id, g.client_key,
                        t.revoked_at IS NOT NULL OR g.status <> 'approved'
                   FROM grant_tokens t JOIN grants g ON g.id = t.grant_id
                 UNION ALL
                 SELECT manage_sha256, 'manage', id, client_key, status <> 'approved'
                   FROM grants WHERE manage_sha256 IS NOT NULL
                 UNION ALL
                 SELECT continue_sha256, 'continue', id, client_key, status <> 'approved'
                   FROM grants WHERE manage_sha256 IS NOT NULL",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(hash, kind, grant_id, key, revoked)| {
                    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(key)
                        .ok()?
                        .try_into()
                        .ok()?;
                    Some(Credential {
                        sha256: unhex32(&hash)?,
                        kind: match kind.as_str() {
                            "access" => CredentialKind::Access,
                            "manage" => CredentialKind::Manage,
                            _ => CredentialKind::Continue,
                        },
                        grant_id,
                        key,
                        revoked,
                    })
                })
                .collect())
        })
    }

    /// What `continue_token` is for grant `gid` when the grant never had
    /// tokens issued and the token is its continuation token (compared in
    /// constant time): a live request (pending, or approved with its tokens
    /// not yet collected) with the client key its continuation calls are
    /// signed with, or a request that ended. `None` otherwise.
    pub fn unissued_continuation(
        &self,
        gid: &str,
        continue_token: &str,
    ) -> Result<Option<Unissued>> {
        self.with_read(|c| {
            let row: Option<(String, String, String)> = c
                .query_row(
                    "SELECT client_key, continue_sha256, status FROM grants
                     WHERE id = ?1 AND manage_sha256 IS NULL",
                    params![gid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(row.and_then(|(key, hash, status)| {
                if !matches(continue_token, Some(&hash)) {
                    return None;
                }
                if !matches!(status.as_str(), "pending" | "approved") {
                    return Some(Unissued::Ended);
                }
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(key)
                    .ok()?
                    .try_into()
                    .ok()
                    .map(Unissued::Live)
            }))
        })
    }

    /// Who a verified request of grant `gid` naming `session` speaks for
    /// (spec §6.1, §6.7, §7.2). With no session, its rights are narrowed by
    /// every live narrowing of the grant's sessions (§6.8). It does not
    /// check the session's secret: [`Store::session_secret_matches`] does.
    /// A session that has ended is `invalid_session_proof`: its rights are
    /// never read after its end cleared its narrowing.
    ///
    /// # Errors
    /// `grant_revoked` when `gid` is not an approved grant (it ended after
    /// the request was authenticated); `foreign_session` when `session` is
    /// not one of the grant's.
    pub fn grant_caller(&self, gid: &str, session: Option<&str>) -> Result<GrantCaller> {
        self.with_read(|c| grant_caller_in(c, gid, session))
    }

    /// Whether artifact `aid` was created by `person` (a public ID; `None`
    /// for the owner): the `own` location of rights (spec §7.1).
    pub fn artifact_created_by(&self, aid: &str, person: Option<&str>) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND person IS ?2)",
                params![aid, person],
                |r| r.get(0),
            )?)
        })
    }

    /// Which of `ids` name a grant row (of any status).
    pub fn existing_grant_ids(&self, ids: &[String]) -> Result<std::collections::HashSet<String>> {
        self.with_read(|c| {
            let mut stmt = c.prepare("SELECT 1 FROM grants WHERE id = ?1")?;
            let mut out = std::collections::HashSet::new();
            for id in ids {
                if stmt.exists(params![id])? {
                    out.insert(id.clone());
                }
            }
            Ok(out)
        })
    }

    /// The IDs of every grant that has ended: denied, revoked or expired.
    pub fn ended_grant_ids(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM grants WHERE status IN ('denied', 'revoked', 'expired')",
            )?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Whether access token `token` is live: unrevoked, of an approved grant.
    pub fn token_live(&self, token: &str) -> Result<bool> {
        let hash = sha256_hex(token);
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM grant_tokens t JOIN grants g ON g.id = t.grant_id
                   WHERE t.sha256 = ?1 AND t.revoked_at IS NULL AND g.status = 'approved')",
                params![hash],
                |r| r.get(0),
            )?)
        })
    }

    /// Records that grant `gid` was used, writing `last_used_at` at most
    /// once per [`USE_NOTE_SECS`].
    pub fn note_grant_use(&self, gid: &str) -> Result<()> {
        let cutoff = self.ago(USE_NOTE_SECS);
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE grants SET last_used_at = ?2
                 WHERE id = ?1 AND status = 'approved' AND (last_used_at IS NULL OR last_used_at < ?3)",
                params![gid, self.now(), cutoff],
            )?;
            Ok(())
        })
    }

    /// The rights of person `public_id` (spec §7): everything for the owner,
    /// else what was set, by default the collaborator preset. A revoked
    /// person keeps theirs, as history.
    ///
    /// # Errors
    /// `not_a_person` for a viewer who is neither a person nor the owner;
    /// `NotFound` for an unknown public ID.
    pub fn person_rights(&self, public_id: &str) -> Result<Rights> {
        self.with_read(|c| {
            let (id, person, owner): (String, bool, bool) = c
                .query_row(
                    "SELECT id, person, owner FROM viewers WHERE public_id = ?1",
                    params![public_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            if !person && !owner {
                return Err(CoreError::invalid(
                    "not_a_person",
                    "only a connected person or the owner has rights",
                ));
            }
            rights_of_row(c, &id)
        })
    }

    /// Sets person `public_id`'s rights; `None` restores the default.
    ///
    /// # Errors
    /// `not_a_person` for the owner or a viewer who is not a person;
    /// `NotFound` for an unknown public ID.
    pub fn set_person_rights(
        &self,
        ctx: &AuditCtx,
        public_id: &str,
        rights: Option<Rights>,
    ) -> Result<()> {
        self.with_tx(|tx| {
            let (person, owner): (bool, bool) = tx
                .query_row(
                    "SELECT person, owner FROM viewers WHERE public_id = ?1",
                    params![public_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            if !person || owner {
                return Err(CoreError::invalid(
                    "not_a_person",
                    "only a connected person has rights to set",
                ));
            }
            let changed = tx.execute(
                "UPDATE viewers SET rights_json = ?2 WHERE public_id = ?1 AND rights_json IS NOT ?2",
                params![public_id, rights.as_ref().map(write_rights)],
            )? > 0;
            let rec = AuditRecord::new(AuditKind::PersonRights, self.now())
                .with("person", public_id)
                .with("rights", rights.as_ref().map(Rights::to_json));
            self.record_audit(tx, ctx, rec)?;
            if changed {
                self.visibility_changed();
            }
            Ok(())
        })
    }

    /// Expires unfinished requests (pending, or approved with their tokens
    /// never collected) older than [`PENDING_TTL_SECS`], freeing their keys,
    /// and approved grants unused for [`GRANT_IDLE_SECS`] (counted from
    /// approval when never used), each with its tokens and live sessions.
    /// Also reports requests that [`Store::request_grant`] or
    /// [`Store::continue_grant`] expired since the last call, and returns
    /// to plain viewers the people whose invites all lapsed without a
    /// grant, and deletes the requests that never had tokens issued
    /// (denied, expired or cancelled) once [`UNISSUED_RETENTION_SECS`] old.
    /// Returns what each expiry ended.
    pub fn expire_grants(&self) -> Result<Vec<RevokedGrant>> {
        let pending_cutoff = self.ago(PENDING_TTL_SECS);
        let retention_cutoff = self.ago(UNISSUED_RETENTION_SECS);
        let idle_cutoff = self.ago(GRANT_IDLE_SECS);
        self.with_tx(|tx| {
            let now = self.now();
            let mut out: Vec<RevokedGrant> = {
                let mut stmt = tx.prepare(
                    "UPDATE grants SET ended_at = ?1 WHERE status = 'expired' AND ended_at IS NULL
                     RETURNING id",
                )?;
                stmt.query_map(params![now], |r| r.get(0))?
                    .map(|gid| {
                        gid.map(|grant_id| RevokedGrant {
                            grant_id,
                            ended: true,
                            ..Default::default()
                        })
                    })
                    .collect::<rusqlite::Result<_>>()?
            };
            let due: Vec<String> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT id FROM grants WHERE {UNFINISHED_STALE}
                     UNION ALL
                     SELECT id FROM grants WHERE status = 'approved' AND manage_sha256 IS NOT NULL
                       AND COALESCE(last_used_at, decided_at, created_at) < ?2"
                ))?;
                stmt.query_map(params![pending_cutoff, idle_cutoff], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for gid in &due {
                out.push(end_grant(self, tx, &EXPIRY, gid, EndedBy::Expiry, &now)?);
            }
            out.sort_by(|a, b| a.grant_id.cmp(&b.grant_id));
            // Requests that never had tokens issued leave no trace in the
            // index or the sessions; past their retention, and once
            // reported, they go.
            tx.execute(
                "DELETE FROM grants WHERE manage_sha256 IS NULL AND created_at < ?1
                   AND (status = 'denied'
                        OR (status IN ('expired', 'revoked') AND ended_at IS NOT NULL))",
                params![retention_cutoff],
            )?;
            if release_unconnected(self, tx, &EXPIRY, None, &now)? > 0 {
                self.visibility_changed();
            }
            Ok(out)
        })
    }
}

/// A connected person (or a revoked one, kept as history) with their
/// rights (spec §7.3, §7.4).
#[derive(Clone, Debug)]
pub struct PersonRow {
    pub viewer: Viewer,
    pub rights: Rights,
}

/// A live agent with the narrowing its person set and the elevations its
/// granters gave it (spec §7.2, §7.4, O11).
#[derive(Clone, Debug)]
pub struct AgentRights {
    pub session: crate::model::Session,
    /// Its person's narrowing of it alone.
    pub narrow: Option<Rights>,
    /// Its elevations until its session ends, one per granter, the owner's
    /// first, each as given.
    pub elevations: Vec<super::elevations::Elevation>,
    /// What it may do (`(person ∩ grant.narrow ∩ narrow) ∪ elevations`);
    /// `None` for an agent of this machine, which acts with the daemon token
    /// that rights do not limit.
    pub rights: Option<Rights>,
}

impl AgentRights {
    /// The elevation `granter` (`None`: the owner) gave it, as given.
    pub fn elevation_by(&self, granter: Option<&str>) -> Option<&Rights> {
        self.elevations
            .iter()
            .find(|e| e.granter.as_deref() == granter)
            .map(|e| &e.access)
    }
}

/// The access an agent of grant `gid` has before its own narrowing and
/// elevation: its person's rights within the grant's narrowing.
fn agent_base(c: &rusqlite::Connection, gid: &str) -> Result<Rights> {
    let (viewer_id, narrow): (String, Option<String>) = c
        .query_row(
            "SELECT viewer_id, narrow_json FROM grants WHERE id = ?1 AND viewer_id IS NOT NULL",
            params![gid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or(CoreError::NotFound)?;
    grant_access(
        c,
        &viewer_id,
        narrow.as_deref().map(read_rights).transpose()?.as_ref(),
    )
}

/// 400 `not_narrower`: `narrow` allows something `base` does not.
fn check_narrower(narrow: Option<&Rights>, base: &Rights) -> Result<()> {
    match narrow {
        Some(n) if !n.is_narrower_or_equal(base) => Err(CoreError::invalid(
            "not_narrower",
            "a narrowing only takes rights away: this one allows something the agent's person may not do",
        )),
        _ => Ok(()),
    }
}

/// Live session `sid` on `c` with its narrowing and elevations.
pub(super) fn agent_rights_in(c: &rusqlite::Connection, sid: &str) -> Result<Option<AgentRights>> {
    let row = c
        .query_row(
            &format!(
                "{} WHERE id = ?1 AND ended_at IS NULL",
                super::sessions::SELECT
                    .replace(" FROM sessions", ", narrow_json AS narrow_j FROM sessions")
            ),
            params![sid],
            |r| {
                Ok((
                    super::sessions::row_to_session(r)?,
                    r.get::<_, Option<String>>("narrow_j")?,
                ))
            },
        )
        .optional()?;
    let Some((session, narrow)) = row else {
        return Ok(None);
    };
    let rights = match &session.grant_id {
        Some(gid) => match grant_caller_in(c, gid, Some(sid)) {
            Ok(gc) => Some(gc.rights),
            // A grant that ended has no agents that act.
            Err(CoreError::Invalid { .. }) => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    Ok(Some(AgentRights {
        session,
        narrow: narrow.as_deref().map(read_rights).transpose()?,
        elevations: super::elevations::of_session(c, sid)?,
        rights,
    }))
}

/// 400 `token_agent`: rights neither narrow nor elevate an agent of this
/// machine, which acts with the daemon token.
fn token_agent() -> CoreError {
    CoreError::invalid(
        "token_agent",
        "this machine's agents act with the daemon token, which rights do not limit: narrowing and elevation apply to agents of connected machines",
    )
}

/// The live session `sid`'s grant on `tx`: `NotFound` for one that is not
/// live, [`token_agent`] for one of this machine.
pub(super) fn live_grant_of(tx: &Transaction<'_>, sid: &str) -> Result<String> {
    let gid: Option<Option<String>> = tx
        .query_row(
            "SELECT grant_id FROM sessions WHERE id = ?1 AND ended_at IS NULL",
            params![sid],
            |r| r.get(0),
        )
        .optional()?;
    gid.ok_or(CoreError::NotFound)?.ok_or_else(token_agent)
}

impl Store {
    /// Every connected person, and every revoked one, oldest first, with
    /// their rights.
    pub fn people(&self) -> Result<Vec<PersonRow>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, public_id, display_name, created_at, person, person_revoked_at
                 FROM viewers WHERE person = 1 AND owner = 0 ORDER BY created_at, rowid",
            )?;
            let viewers = stmt
                .query_map([], row_to_viewer)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            viewers
                .into_iter()
                .map(|v| {
                    let rights = rights_of_row(c, &v.id)?;
                    Ok(PersonRow { viewer: v, rights })
                })
                .collect()
        })
    }

    /// Every live agent with its narrowing and elevations, oldest first.
    pub fn live_agents(&self) -> Result<Vec<AgentRights>> {
        self.with_read(|c| {
            let ids: Vec<String> = {
                let mut stmt = c.prepare(
                    "SELECT id FROM sessions WHERE ended_at IS NULL ORDER BY started_at, id",
                )?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(a) = agent_rights_in(c, &id)? {
                    out.push(a);
                }
            }
            Ok(out)
        })
    }

    /// Live session `sid` with its narrowing and elevations; `None` once it
    /// has ended or for no such session.
    pub fn agent_rights(&self, sid: &str) -> Result<Option<AgentRights>> {
        self.with_read(|c| agent_rights_in(c, sid))
    }

    /// Sets (or with `None` clears) the narrowing of live agent `sid` of
    /// another machine (spec §7.4), recorded as `agent.narrow {session,
    /// access}` under `ctx`. From its next request on, it may do only
    /// what `(person ∩ grant narrowing ∩ narrow) ∪ elevations` allows
    /// (M17: a change bumps the visibility generation).
    ///
    /// # Errors
    /// `NotFound` for a session that is not live; `token_agent` for an
    /// agent of this machine; `not_narrower` for a narrowing that allows
    /// anything its person's rights within its grant's narrowing do not.
    pub fn narrow_agent(
        &self,
        ctx: &AuditCtx,
        sid: &str,
        value: Option<Rights>,
    ) -> Result<AgentRights> {
        self.with_tx(|tx| {
            let gid = live_grant_of(tx, sid)?;
            check_narrower(value.as_ref(), &agent_base(tx, &gid)?)?;
            let json = value.as_ref().map(write_rights);
            let changed = tx.execute(
                "UPDATE sessions SET narrow_json = ?2 WHERE id = ?1 AND narrow_json IS NOT ?2",
                params![sid, json],
            )? > 0;
            let mut rec = AuditRecord::new(AuditKind::AgentNarrow, self.now())
                .with("session", sid)
                .with("access", value.as_ref().map(Rights::to_json));
            rec.ids.session = Some(sid.to_string());
            self.record_audit(tx, ctx, rec)?;
            if changed {
                self.visibility_changed();
            }
            agent_rights_in(tx, sid)?.ok_or(CoreError::NotFound)
        })
    }
}

/// Grants whose request is unfinished past its time (`?1`: the cutoff,
/// [`PENDING_TTL_SECS`] ago): pending, or approved with their tokens never
/// collected (a lost continuation response).
const UNFINISHED_STALE: &str = "((status = 'pending' AND created_at < ?1)
    OR (status = 'approved' AND manage_sha256 IS NULL AND decided_at < ?1))";

/// Returns to plain viewers (and to the default rights) the people, of
/// `viewer_id` or of every row when `None`, who never held a grant, are
/// unrevoked, and have no live invite: an invite's person whose invites
/// were revoked or expired unredeemed. Every elevation one of them gave is
/// dropped (`person_released`, under `ctx`). Returns how many it returned.
fn release_unconnected(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    viewer_id: Option<&str>,
    now: &str,
) -> Result<usize> {
    let released: Vec<String> = {
        let mut stmt = tx.prepare(
            "UPDATE viewers SET person = 0, rights_json = NULL
             WHERE person = 1 AND owner = 0 AND person_revoked_at IS NULL AND (?1 IS NULL OR id = ?1)
               AND NOT EXISTS (SELECT 1 FROM grants g WHERE g.viewer_id = viewers.id)
               AND NOT EXISTS (SELECT 1 FROM invites i WHERE i.viewer_id = viewers.id
                                 AND i.used_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > ?2)
             RETURNING public_id",
        )?;
        stmt.query_map(params![viewer_id, now], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for p in &released {
        super::elevations::drop_granted_in(st, tx, ctx, p, "person_released")?;
    }
    Ok(released.len())
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The intersection of every narrowing of grant `gid`'s live sessions
/// (spec §6.8); `None` when none is narrowed.
fn sessionless_narrow(c: &rusqlite::Connection, gid: &str) -> Result<Option<Rights>> {
    let mut stmt = c.prepare_cached(
        "SELECT narrow_json FROM sessions
         WHERE grant_id = ?1 AND ended_at IS NULL AND narrow_json IS NOT NULL
         ORDER BY id",
    )?;
    let narrowings = stmt
        .query_map(params![gid], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut all: Option<Rights> = None;
    for n in narrowings {
        let n = read_rights(&n)?;
        all = Some(match all {
            Some(a) => a.intersect(&n),
            None => n,
        });
    }
    Ok(all)
}

/// [`Store::grant_caller`], read on `c`.
pub(crate) fn grant_caller_in(
    c: &rusqlite::Connection,
    gid: &str,
    session: Option<&str>,
) -> Result<GrantCaller> {
    let row: Option<(String, String, bool, String, Option<String>)> = c
        .query_row(
            "SELECT g.status, g.viewer_id, COALESCE(v.owner, 0), v.public_id, g.narrow_json
                 FROM grants g JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
            params![gid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((_, viewer_id, owner, public_id, narrow)) = row.filter(|r| r.0 == "approved") else {
        return Err(CoreError::invalid(
            "grant_revoked",
            "this machine's grant was revoked: connect again",
        ));
    };
    let person_rights = rights_of_row(c, &viewer_id)?;
    let grant_narrow = narrow.as_deref().map(read_rights).transpose()?;
    let (session_narrow, elevate) = match session {
        // A request naming no session is bound by every live narrowing of
        // its grant's sessions (spec §6.8), so leaving one's session out
        // never widens an agent.
        None => (sessionless_narrow(c, gid)?, None),
        Some(sid) => {
            let row: Option<(Option<String>, Option<String>)> = c
                .query_row(
                    "SELECT narrow_json, ended_at FROM sessions
                         WHERE id = ?1 AND grant_id = ?2",
                    params![sid, gid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (narrow, ended) = row.ok_or_else(|| {
                CoreError::invalid(
                    "foreign_session",
                    "that session is not one of this credential's",
                )
            })?;
            // An ended session's narrowing is cleared, so its rights would
            // be wider than a request naming no session: it acts as nothing
            // (spec §6.8, its secret proves nothing once it ended).
            if ended.is_some() {
                return Err(super::sessions::invalid_session_proof());
            }
            (
                narrow.as_deref().map(read_rights).transpose()?,
                super::elevations::given_in(c, sid)?,
            )
        }
    };
    Ok(GrantCaller {
        grant_id: gid.to_string(),
        person: (!owner).then_some(public_id),
        session: session.map(str::to_string),
        rights: crate::rights::effective(
            &person_rights,
            grant_narrow.as_ref(),
            session_narrow.as_ref(),
            elevate.as_ref(),
        ),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::test_util::DAEMON;
    use crate::working::ManualClock;
    use crate::{Home, RegisterSession};
    use std::sync::Arc;

    pub(crate) const KEY_A: [u8; 32] = [1; 32];

    /// A continuation call's signed time: later than every one before.
    /// A continuation call's nonce: new each time.
    pub(crate) fn n() -> String {
        crate::sig::new_nonce()
    }

    pub(crate) fn t() -> i64 {
        static N: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
    pub(crate) const KEY_B: [u8; 32] = [2; 32];
    const T0: &str = "2026-10-01T09:00:00Z";

    pub(crate) fn store() -> (tempfile::TempDir, Arc<ManualClock>, Store) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(T0));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap();
        (dir, clock, st)
    }

    pub(crate) fn grant(key: [u8; 32], invite: Option<&str>) -> NewGrant {
        NewGrant {
            key,
            client_name: "sam-mbp".into(),
            peer_ip: "192.168.1.20".into(),
            access: Rights::preset(crate::rights::PRESET_COLLABORATOR).unwrap(),
            invite: invite.map(|c| Secret::new(c.into())),
        }
    }

    fn from_ip(key: [u8; 32], ip: &str) -> NewGrant {
        NewGrant {
            peer_ip: ip.into(),
            ..grant(key, None)
        }
    }

    pub(crate) fn code(e: Result<impl std::fmt::Debug>) -> &'static str {
        match e {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("expected an Invalid error, got {other:?}"),
        }
    }

    /// Sam, connected by invite with `key`: the grant ID, access token and
    /// manage token.
    pub(crate) fn approved(st: &Store, name: &str, key: [u8; 32]) -> (String, Secret, Secret) {
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New(name.into()), None)
            .unwrap();
        match st
            .request_grant(DAEMON, grant(key, Some(c.expose())))
            .unwrap()
        {
            GrantStart::Approved {
                grant,
                token,
                manage,
                ..
            } => (grant.id, token, manage),
            other => panic!("{other:?}"),
        }
    }

    pub(crate) fn pending(st: &Store, key: [u8; 32]) -> (Grant, Secret, Secret) {
        match st.request_grant(DAEMON, grant(key, None)).unwrap() {
            GrantStart::Pending {
                grant,
                continue_token,
                user_code,
            } => (grant, continue_token, user_code),
            other => panic!("{other:?}"),
        }
    }

    fn reg(grant_id: Option<&str>) -> RegisterSession {
        RegisterSession {
            harness: "claude".into(),
            cwd: "/w".into(),
            grant_id: grant_id.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn invite_is_single_use_and_binds_the_first_key() {
        let (_d, _c, st) = store();
        let (inv, code_) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        assert_eq!(inv.person_name.as_deref(), Some("Sam"));
        let GrantStart::Approved {
            grant: g,
            token,
            manage,
            continue_token,
            browser_code,
            ..
        } = st
            .request_grant(DAEMON, grant(KEY_A, Some(code_.expose())))
            .unwrap()
        else {
            panic!()
        };
        assert!(token.expose().starts_with("cxg_") && manage.expose().starts_with("cxm_"));
        assert!(continue_token.expose().starts_with("cxc_"));
        assert!(browser_code.is_some());
        assert_eq!((g.status.as_str(), g.via.as_str()), ("approved", "invite"));
        assert_eq!(g.person.as_deref(), Some(inv.person.as_str()));
        assert!(st.token_live(token.expose()).unwrap());
        assert_eq!(
            code(st.request_grant(DAEMON, grant(KEY_B, Some(code_.expose())))),
            "invalid_invite"
        );
        let used = &st.list_invites().unwrap()[0];
        assert_eq!(used.grant_id.as_deref(), Some(g.id.as_str()));
        assert!(used.used_at.is_some());
        // The code is accepted however it is typed.
        let (_, c2) = st
            .create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
            .unwrap();
        let typed = c2.expose().replace('-', "").to_lowercase();
        assert!(matches!(
            st.request_grant(DAEMON, grant(KEY_B, Some(&typed)))
                .unwrap(),
            GrantStart::Approved { .. }
        ));
    }

    #[test]
    fn expired_revoked_and_unknown_invites_are_one_error() {
        let (_d, clock, st) = store();
        let (_, expired) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        clock.advance(INVITE_TTL_SECS + 1);
        let (inv, revoked) = st
            .create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
            .unwrap();
        st.revoke_invite(DAEMON, &inv.id).unwrap();
        for c in [
            expired.expose(),
            revoked.expose(),
            "AAAA-AAAA-AAAA",
            "nonsense",
        ] {
            assert_eq!(
                code(st.request_grant(DAEMON, grant(KEY_A, Some(c)))),
                "invalid_invite",
                "{c}"
            );
        }
    }

    #[test]
    fn invites_are_limited_and_name_their_person() {
        let (_d, _c, st) = store();
        let lan = st.upsert_viewer(&crate::new_ulid(), Some("Lan")).unwrap();
        let (inv, _) = st
            .create_invite(DAEMON, InvitePerson::Viewer(lan.public_id.clone()), None)
            .unwrap();
        assert_eq!(inv.person, lan.public_id);
        assert!(st.get_viewer(&lan.id).unwrap().unwrap().person);
        let (me, _) = st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        assert!(me.owner);
        assert_eq!(
            code(st.create_invite(DAEMON, InvitePerson::Viewer(me.person.clone()), None)),
            "unknown_viewer"
        );
        assert_eq!(
            code(st.create_invite(DAEMON, InvitePerson::New("lan".into()), None)),
            "name_taken"
        );
        for i in 2..MAX_LIVE_INVITES {
            st.create_invite(DAEMON, InvitePerson::New(format!("P{i}")), None)
                .unwrap();
        }
        assert_eq!(
            code(st.create_invite(DAEMON, InvitePerson::New("Late".into()), None)),
            "limit_reached"
        );
    }

    #[test]
    fn an_owner_invite_connects_the_owner_without_a_browser_link() {
        let (_d, _c, st) = store();
        let (_, c) = st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        let GrantStart::Approved {
            grant: g,
            browser_code,
            ..
        } = st
            .request_grant(DAEMON, grant(KEY_A, Some(c.expose())))
            .unwrap()
        else {
            panic!()
        };
        assert!(g.owner && browser_code.is_none());
        assert_eq!(
            st.person_rights(g.person.as_deref().unwrap()).unwrap(),
            Rights::full()
        );
        let s = st.register_session(DAEMON, reg(Some(&g.id))).unwrap();
        assert_eq!(
            (s.grant_id.as_deref(), s.person),
            (Some(g.id.as_str()), None)
        );
    }

    #[test]
    fn connecting_records_the_persons_approval_of_the_owner_once() {
        let (_d, _c, st) = store();
        let owner = st.owner_viewer(true).unwrap();
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let GrantStart::Approved {
            grant: g,
            continue_token,
            ..
        } = st
            .request_grant(DAEMON, grant(KEY_A, Some(c.expose())))
            .unwrap()
        else {
            panic!()
        };
        let sam = g.person.clone().unwrap();
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, "cxc_wrong", t(), &n())),
            "invalid_token"
        );
        let (ta, na) = (t(), n());
        let (a, _) = st
            .approve_owner_on_connect(DAEMON, &g.id, continue_token.expose(), ta, &na)
            .unwrap()
            .unwrap();
        // The same call again is a replay, told as one before anything else.
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, continue_token.expose(), ta, &na)),
            "replayed_request"
        );
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, continue_token.expose(), ta - 1, &n())),
            "replayed_request"
        );
        assert_eq!(
            (
                a.approver.as_deref(),
                a.author.as_str(),
                a.created_via.as_str()
            ),
            (Some(sam.as_str()), owner.public_id.as_str(), "connect")
        );
        assert_eq!(a.scope, super::super::approvals::ScopeKind::Everywhere);
        // Asked again: answered once already.
        let (t1, n1) = (t(), n());
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, continue_token.expose(), t1, &n1)),
            "owner_answered"
        );
        assert_eq!(st.live_approvals(Some(&sam)).unwrap().len(), 1);
        // The owner's own machine needs none.
        let (_, mine) = st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        let GrantStart::Approved {
            grant: og,
            continue_token: oc,
            ..
        } = st
            .request_grant(DAEMON, grant(KEY_B, Some(mine.expose())))
            .unwrap()
        else {
            panic!()
        };
        assert!(
            st.approve_owner_on_connect(DAEMON, &og.id, oc.expose(), t(), &n())
                .unwrap()
                .is_none()
        );
        // A request no one approved yet, and an ended grant, have nothing to answer for.
        let (p, pc, _) = pending(&st, [3; 32]);
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &p.id, pc.expose(), t(), &n())),
            "not_issued"
        );
        st.disconnect_grant(DAEMON, &g.id, continue_token.expose(), t(), &n())
            .unwrap();
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, continue_token.expose(), t(), &n())),
            "not_issued"
        );
    }

    /// The connect-time answer is the one approval made without a browser
    /// (§21 item 3): never after the person decided about the owner in
    /// People, never later than ten minutes after the tokens' issue.
    #[test]
    fn the_answer_about_the_owner_never_overrides_the_person() {
        use super::super::approvals::{Scope, Via};
        let (_d, clock, st) = store();
        let owner = st.owner_viewer(true).unwrap();
        let connect = |key: [u8; 32], name: &str| {
            let (_, c) = st
                .create_invite(DAEMON, InvitePerson::New(name.into()), None)
                .unwrap();
            match st
                .request_grant(DAEMON, grant(key, Some(c.expose())))
                .unwrap()
            {
                GrantStart::Approved {
                    grant,
                    continue_token,
                    ..
                } => (grant, continue_token),
                other => panic!("{other:?}"),
            }
        };
        // Sam allowed the owner in People, then revoked it: the revocation
        // stands.
        let (g, cont) = connect(KEY_A, "Sam");
        let sam = g.person.clone().unwrap();
        let a = st
            .approve(
                DAEMON,
                Some(&sam),
                &owner.public_id,
                Scope::Everywhere,
                Via::Shell,
            )
            .unwrap();
        st.revoke_approval(DAEMON, &a.id, Some(&sam)).unwrap();
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, cont.expose(), t(), &n())),
            "owner_answered"
        );
        assert!(st.live_approvals(Some(&sam)).unwrap().is_empty());
        // Mia answers eleven minutes after connecting: too late.
        let (g, cont) = connect(KEY_B, "Mia");
        clock.advance(OWNER_ANSWER_SECS + 1);
        assert_eq!(
            code(st.approve_owner_on_connect(DAEMON, &g.id, cont.expose(), t(), &n())),
            "answer_closed"
        );
        assert!(st.live_approvals(g.person.as_deref()).unwrap().is_empty());
    }

    #[test]
    fn a_request_only_makes_a_new_person() {
        let (_d, clock, st) = store();
        let (g, cont, user_code) = pending(&st, KEY_A);
        assert_eq!((g.status.as_str(), g.person.as_ref()), ("pending", None));
        let approved = st
            .approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)
            .unwrap();
        let v = st.viewer_of_grant(&approved.id).unwrap();
        assert!(v.person && v.display_name.as_deref() == Some("Sam"));
        assert_ne!(
            Some(v.public_id.clone()),
            st.owner().unwrap().map(|o| o.public_id)
        );
        // The tokens go to the client's next poll, once.
        clock.advance(CONTINUE_WAIT_SECS);
        let ContinueOutcome::Approved {
            token,
            browser_code,
            ..
        } = st
            .continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
            .unwrap()
        else {
            panic!()
        };
        assert!(st.token_live(token.expose()).unwrap() && browser_code.is_some());
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "invalid_token",
            "rotated"
        );
        // Name rules: another person's name, in any case, is taken.
        let (g2, _, code2) = pending(&st, KEY_B);
        assert_eq!(
            code(st.approve_grant(DAEMON, &g2.id, code2.expose(), "sam", None)),
            "name_taken"
        );
        assert_eq!(
            code(st.approve_grant(DAEMON, &g2.id, code2.expose(), " ", None)),
            "invalid_name"
        );
        assert_eq!(st.grant(&g2.id).unwrap().status, "pending");
    }

    #[test]
    fn wrong_code_approves_nothing_and_the_third_denies() {
        let (_d, clock, st) = store();
        let (g, cont, user_code) = pending(&st, KEY_A);
        let wrong = if user_code.expose() == "AAAA-0000" {
            "AAAA-0001"
        } else {
            "AAAA-0000"
        };
        for _ in 0..2 {
            assert_eq!(
                code(st.approve_grant(DAEMON, &g.id, wrong, "Sam", None)),
                "wrong_code"
            );
            assert_eq!(st.grant(&g.id).unwrap().status, "pending");
        }
        assert_eq!(
            code(st.approve_grant(DAEMON, &g.id, "not a code", "Sam", None)),
            "wrong_code"
        );
        let g = st.grant(&g.id).unwrap();
        assert_eq!((g.status.as_str(), g.wrong_codes), ("denied", 3));
        assert_eq!(
            code(st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)),
            "not_pending"
        );
        clock.advance(CONTINUE_WAIT_SECS);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "denied"
        );
        // A lowercase code without the dash is the same code.
        let (g, _, user_code) = pending(&st, KEY_B);
        let typed = user_code.expose().replace('-', "").to_lowercase();
        assert!(st.approve_grant(DAEMON, &g.id, &typed, "Sam", None).is_ok());
    }

    #[test]
    fn pending_limits_per_ip_and_overall() {
        let (_d, clock, st) = store();
        for i in 0..MAX_PENDING_PER_IP as u8 {
            st.request_grant(DAEMON, from_ip([10 + i; 32], "10.0.0.1"))
                .unwrap();
        }
        assert_eq!(
            code(st.request_grant(DAEMON, from_ip([99; 32], "10.0.0.1"))),
            "limit_reached"
        );
        for i in MAX_PENDING_PER_IP as u8..MAX_PENDING as u8 {
            st.request_grant(DAEMON, from_ip([10 + i; 32], &format!("10.0.1.{i}")))
                .unwrap();
        }
        assert_eq!(
            code(st.request_grant(DAEMON, from_ip([99; 32], "10.0.2.1"))),
            "limit_reached"
        );
        // A live key cannot ask again; pending requests expire after ten minutes.
        assert_eq!(
            code(st.request_grant(DAEMON, from_ip([10; 32], "10.0.2.1"))),
            "key_in_use"
        );
        clock.advance(PENDING_TTL_SECS + 1);
        st.request_grant(DAEMON, from_ip([10; 32], "10.0.0.1"))
            .unwrap();
    }

    /// The events recorded after `seq`: kind, actor and body.
    fn recorded(st: &Store, seq: i64) -> Vec<(String, serde_json::Value, serde_json::Value)> {
        st.events_after(seq, 1000)
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r.kind,
                    serde_json::from_str(&r.actor).unwrap(),
                    serde_json::from_str(&r.body).unwrap(),
                )
            })
            .collect()
    }

    fn owner_ctx(st: &Store) -> AuditCtx {
        AuditCtx {
            actor: Actor::Owner {
                public_id: st.owner_viewer(false).unwrap().public_id,
            },
            via: crate::audit::Via::Shell,
            git: crate::gitctx::GitField::Absent,
            call: None,
        }
    }

    #[test]
    fn the_grant_flow_records_who_did_what_without_secrets() {
        let (_d, clock, st) = store();
        let owner = owner_ctx(&st);
        // An invite, redeemed: the request, its approval and the tokens'
        // issue are the client's.
        let seq = st.newest_seq().unwrap();
        let (invite, code) = st
            .create_invite(&owner, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let GrantStart::Approved { grant: sam, .. } = st
            .request_grant(DAEMON, grant(KEY_A, Some(code.expose())))
            .unwrap()
        else {
            panic!("an invite approves at once")
        };
        let ev = recorded(&st, seq);
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "invite.create",
                "grant.request",
                "grant.approve",
                "grant.token"
            ]
        );
        assert_eq!(ev[0].1["type"], "owner");
        assert_eq!(ev[0].2["invite_id"], invite.id.as_str());
        let client = serde_json::json!({"type": "client", "grant_id": sam.id,
                                        "client_name": "sam-mbp"});
        for e in &ev[1..] {
            assert_eq!(e.1, client, "{}", e.0);
            assert_eq!(e.2["grant_id"], sam.id.as_str());
        }
        assert_eq!(ev[1].2["flow"], "invite");
        assert_eq!(ev[1].2["peer_ip"], "192.168.1.20");
        assert_eq!(
            ev[1].2["key_sha256"],
            crate::audit::sha256_hex(&KEY_A).as_str()
        );
        assert_eq!(ev[2].2["person"], sam.person.clone().unwrap().as_str());
        assert_eq!(ev[2].2["code_checked"], false);
        assert_eq!(ev[3].2["op"], "issue");
        let all = serde_json::to_string(&ev).unwrap();
        assert!(
            !all.contains(code.expose()),
            "the invite code is never recorded"
        );
        // A request: the owner approves it with its code; the client
        // collects the tokens.
        let seq = st.newest_seq().unwrap();
        let (g, cont, user_code) = pending(&st, KEY_B);
        st.approve_grant(&owner, &g.id, user_code.expose(), "Mia", None)
            .unwrap();
        clock.advance(CONTINUE_WAIT_SECS);
        let issued = st
            .continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
            .unwrap();
        let ev = recorded(&st, seq);
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(kinds, ["grant.request", "grant.approve", "grant.token"]);
        assert_eq!(ev[0].1["type"], "client");
        assert_eq!(ev[0].2["flow"], "request");
        assert_eq!(ev[1].1, serde_json::to_value(&owner.actor).unwrap());
        assert_eq!(ev[1].2["code_checked"], true);
        assert_eq!(ev[2].1["type"], "client");
        let ContinueOutcome::Approved { token, manage, .. } = issued else {
            panic!("approved")
        };
        let all = serde_json::to_string(&ev).unwrap();
        for secret in [&user_code, &cont, &token, &manage] {
            assert!(!all.contains(secret.expose()), "no secret is recorded");
        }
        // The person disconnects their machine: the person ended it.
        let seq = st.newest_seq().unwrap();
        st.revoke_token(DAEMON, &g.id, manage.expose()).unwrap();
        let ev = recorded(&st, seq);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "grant.revoke");
        assert_eq!(ev[0].1["type"], "viewer");
        assert_eq!(ev[0].1["person"], true);
        assert_eq!(ev[0].1["display_name"], "Mia");
        assert_eq!(ev[0].2["by"], "person");
        // A request denied; another left to lapse.
        let (denied, _, _) = pending(&st, [3; 32]);
        let (lapsed, _, _) = pending(&st, [4; 32]);
        let seq = st.newest_seq().unwrap();
        st.deny_grant(&owner, &denied.id).unwrap();
        clock.advance(PENDING_TTL_SECS + 1);
        st.expire_grants().unwrap();
        let ev = recorded(&st, seq);
        assert_eq!(
            ev.iter()
                .map(|e| (
                    e.0.as_str(),
                    e.2["grant_id"].as_str().unwrap(),
                    &e.2["reason"]
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "grant.deny",
                    denied.id.as_str(),
                    &serde_json::json!("owner")
                ),
                (
                    "grant.deny",
                    lapsed.id.as_str(),
                    &serde_json::json!("expired")
                ),
            ]
        );
        assert_eq!(
            ev[1].1,
            serde_json::json!({"type": "system", "reason": "expiry"})
        );
        // Revoking Sam ends their grant and records the person's revocation.
        let seq = st.newest_seq().unwrap();
        let sam_id = sam.person.clone().unwrap();
        st.revoke_person(&owner, &sam_id).unwrap();
        let ev = recorded(&st, seq);
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(kinds, ["grant.revoke", "person.revoke"]);
        assert_eq!(ev[0].2["by"], "owner");
        assert_eq!(ev[1].2["person"], sam_id.as_str());
        assert!(ev.iter().all(|e| e.1["type"] == "owner"));
        // Nothing changes, nothing is recorded.
        let seq = st.newest_seq().unwrap();
        st.revoke_person(&owner, &sam_id).unwrap();
        st.revoke_invite(&owner, &invite.id).unwrap();
        assert!(recorded(&st, seq).is_empty());
    }

    #[test]
    fn continuation_is_paced_and_expiry_denies() {
        let (_d, clock, st) = store();
        let (g, cont, _) = pending(&st, KEY_A);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "too_fast"
        );
        clock.advance(CONTINUE_WAIT_SECS);
        assert!(matches!(
            st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
                .unwrap(),
            ContinueOutcome::Pending { .. }
        ));
        clock.advance(CONTINUE_WAIT_SECS - 1);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "too_fast"
        );
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, "cxc_wrong", t(), &n())),
            "invalid_token"
        );
        clock.advance(PENDING_TTL_SECS);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "denied"
        );
        assert_eq!(st.grant(&g.id).unwrap().status, "expired");
    }

    #[test]
    fn revoking_the_token_revokes_the_grant() {
        let (_d, _c, st) = store();
        let (gid, t1, manage) = approved(&st, "Sam", KEY_A);
        let s = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        assert_eq!(
            code(st.revoke_token(DAEMON, &gid, t1.expose())),
            "invalid_token"
        );
        let r = st.revoke_token(DAEMON, &gid, manage.expose()).unwrap();
        assert!(!st.token_live(t1.expose()).unwrap());
        assert!(st.token_index().unwrap().is_empty());
        let g = st.grant(&gid).unwrap();
        assert_eq!(
            (g.status.as_str(), g.ended_by.as_deref()),
            ("revoked", Some("person"))
        );
        assert_eq!(r.sessions_ended, vec![s.id.clone()]);
        let ended: Option<String> = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT ended_reason FROM sessions WHERE id = ?1",
                    [&s.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(ended.as_deref(), Some("revoked"));
        assert_eq!(
            code(st.register_session(DAEMON, reg(Some(&gid)))),
            "grant_revoked"
        );
        assert!(
            st.request_grant(DAEMON, grant(KEY_A, None)).is_ok(),
            "the key may connect again"
        );
    }

    #[test]
    fn disconnecting_through_the_continuation_ends_the_grant() {
        let (_d, _c, st) = store();
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let GrantStart::Approved {
            grant: g,
            continue_token,
            token,
            ..
        } = st
            .request_grant(DAEMON, grant(KEY_A, Some(c.expose())))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            code(st.disconnect_grant(DAEMON, &g.id, token.expose(), t(), &n())),
            "invalid_token"
        );
        st.disconnect_grant(DAEMON, &g.id, continue_token.expose(), t(), &n())
            .unwrap();
        assert!(!st.token_live(token.expose()).unwrap());
    }

    #[test]
    fn grants_expire_after_thirty_days_unused_and_tokens_never_alone() {
        let (_d, clock, st) = store();
        let (gid, token, _) = approved(&st, "Sam", KEY_A);
        let (used, used_token, _) = approved(&st, "Mia", KEY_B);
        let s = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let (stale, _, _) = pending(&st, [7; 32]);
        clock.advance(PENDING_TTL_SECS + 1);
        let ended = st.expire_grants().unwrap();
        assert_eq!(
            ended
                .iter()
                .map(|e| e.grant_id.as_str())
                .collect::<Vec<_>>(),
            [stale.id.as_str()]
        );
        clock.advance(29 * 24 * 3600);
        st.note_grant_use(&used).unwrap();
        assert!(st.expire_grants().unwrap().is_empty());
        assert!(st.token_live(token.expose()).unwrap(), "29 days: live");
        clock.advance(2 * 24 * 3600);
        let ended = st.expire_grants().unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(
            (ended[0].grant_id.as_str(), ended[0].sessions_ended.clone()),
            (gid.as_str(), vec![s.id.clone()])
        );
        let g = st.grant(&gid).unwrap();
        assert_eq!(
            (g.status.as_str(), g.ended_by.as_deref()),
            ("expired", Some("expiry"))
        );
        assert!(!st.token_live(token.expose()).unwrap());
        assert!(
            st.token_live(used_token.expose()).unwrap(),
            "used two days ago"
        );
    }

    #[test]
    fn grant_use_is_noted_at_most_hourly() {
        let (_d, clock, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        st.note_grant_use(&gid).unwrap();
        let first = st.grant(&gid).unwrap().last_used_at;
        assert!(first.is_some());
        clock.advance(USE_NOTE_SECS - 1);
        st.note_grant_use(&gid).unwrap();
        assert_eq!(st.grant(&gid).unwrap().last_used_at, first);
        clock.advance(2);
        st.note_grant_use(&gid).unwrap();
        assert_ne!(st.grant(&gid).unwrap().last_used_at, first);
    }

    #[test]
    fn revoking_a_grant_ends_its_sessions_and_revoking_a_person_keeps_history() {
        let (_d, _c, st) = store();
        let (g1, t1, _) = approved(&st, "Sam", KEY_A);
        let s1 = st.register_session(DAEMON, reg(Some(&g1))).unwrap();
        let r = st.revoke_grant(DAEMON, &g1, EndedBy::Owner).unwrap();
        assert_eq!(r.sessions_ended, vec![s1.id.clone()]);
        assert!(st.get_session(&s1.id).unwrap().unwrap().ended_at.is_some());
        assert!(!st.token_live(t1.expose()).unwrap());
        assert!(
            st.revoke_grant(DAEMON, &g1, EndedBy::Owner)
                .unwrap()
                .sessions_ended
                .is_empty()
        );

        let (g2, t2, _) = approved(&st, "Mia", KEY_B);
        let mia = st.grant(&g2).unwrap().person.unwrap();
        let s2 = st.register_session(DAEMON, reg(Some(&g2))).unwrap();
        let (left, _) = st
            .create_invite(DAEMON, InvitePerson::Viewer(mia.clone()), None)
            .unwrap();
        let r = st.revoke_person(DAEMON, &mia).unwrap();
        assert_eq!(r.grants.len(), 1);
        assert_eq!(r.grants[0].sessions_ended, vec![s2.id.clone()]);
        assert_eq!(r.invites, vec![left.id]);
        assert!(!st.token_live(t2.expose()).unwrap());
        let v = st.viewer_by_public_id(&mia).unwrap().unwrap();
        assert!(
            v.person && v.person_revoked_at.is_some(),
            "the row stays as history"
        );
        // A revoked person's name is free again.
        st.create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
            .unwrap();
        let owner = st.owner_viewer(false).unwrap();
        assert_eq!(
            code(st.revoke_person(DAEMON, &owner.public_id)),
            "not_a_person"
        );
    }

    #[test]
    fn rights_default_to_collaborator_and_requests_narrow() {
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&gid).unwrap().person.unwrap();
        let collaborator = Rights::preset(crate::rights::PRESET_COLLABORATOR).unwrap();
        assert_eq!(st.person_rights(&sam).unwrap(), collaborator);
        assert_eq!(
            st.grant(&gid).unwrap().narrow,
            None,
            "asking for all narrows nothing"
        );
        let reader = Rights::preset(crate::rights::PRESET_READER).unwrap();
        st.set_person_rights(DAEMON, &sam, Some(reader.clone()))
            .unwrap();
        assert_eq!(st.person_rights(&sam).unwrap(), reader);
        st.set_person_rights(DAEMON, &sam, None).unwrap();
        assert_eq!(st.person_rights(&sam).unwrap(), collaborator);
        assert_eq!(
            st.narrow_grant(DAEMON, &gid, Some(reader.clone()))
                .unwrap()
                .narrow,
            Some(reader.clone())
        );

        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
            .unwrap();
        let GrantStart::Approved { grant: g, .. } = st
            .request_grant(
                DAEMON,
                NewGrant {
                    access: reader.clone(),
                    ..grant(KEY_B, Some(c.expose()))
                },
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(g.narrow, Some(reader));
    }

    #[test]
    fn a_grant_caller_has_the_persons_rights_within_every_narrowing_and_a_live_elevation() {
        use crate::rights::{PRESET_COLLABORATOR, PRESET_FULL, PRESET_READER, RightType};
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&gid).unwrap().person.unwrap();
        let none = |_: &str| false;
        let c = st.grant_caller(&gid, None).unwrap();
        assert_eq!(c.person.as_deref(), Some(sam.as_str()));
        assert_eq!(c.session, None);
        assert_eq!(c.rights, Rights::preset(PRESET_COLLABORATOR).unwrap());
        // The grant's narrowing applies.
        let reader = Rights::preset(PRESET_READER).unwrap();
        st.narrow_grant(DAEMON, &gid, Some(reader.clone())).unwrap();
        let c = st.grant_caller(&gid, None).unwrap();
        assert!(
            !c.rights
                .allows(RightType::Comments, "write", Some("x"), &none)
        );
        // A session's elevation applies while it is live, and its narrowing
        // always.
        let s1 = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let full = Rights::preset(PRESET_FULL).unwrap();
        st.with_tx(|tx| {
            tx.execute(
                "INSERT INTO session_elevations (session_id, granter, access_json, created_at)
                 VALUES (?1, NULL, ?2, 'x')",
                params![s1.id, write_rights(&full)],
            )?;
            Ok(())
        })
        .unwrap();
        let c = st.grant_caller(&gid, Some(&s1.id)).unwrap();
        assert_eq!(c.session.as_deref(), Some(s1.id.as_str()));
        // The machine's read-only narrowing caps the raise (spec §7.2):
        // what it turned off stays off; what it leaves on, the raise gives
        // everywhere.
        assert!(
            !c.rights
                .allows(RightType::Artifacts, "delete", Some("x"), &none)
        );
        st.narrow_grant(DAEMON, &gid, None).unwrap();
        let c = st.grant_caller(&gid, Some(&s1.id)).unwrap();
        assert!(
            c.rights
                .allows(RightType::Artifacts, "delete", Some("x"), &none)
        );
        // An ended session acts as nothing: neither its elevation nor its
        // cleared narrowing (spec §6.8), whatever proved it a moment before.
        st.end_session(DAEMON, &s1.id).unwrap();
        assert_eq!(
            code(st.grant_caller(&gid, Some(&s1.id))),
            "invalid_session_proof"
        );
        // Another grant's or the token's session is foreign.
        let (g2, _, _) = approved(&st, "Mia", KEY_B);
        let s2 = st.register_session(DAEMON, reg(Some(&g2))).unwrap();
        let s0 = st.register_session(DAEMON, reg(None)).unwrap();
        for sid in [&s2.id, &s0.id, &"nope".to_string()] {
            assert_eq!(code(st.grant_caller(&gid, Some(sid))), "foreign_session");
        }
        // A revoked grant speaks for no one.
        st.revoke_grant(DAEMON, &gid, EndedBy::Owner).unwrap();
        assert_eq!(code(st.grant_caller(&gid, None)), "grant_revoked");
        assert_eq!(code(st.grant_caller("g_unknown", None)), "grant_revoked");
        // A grant of the owner's row is the owner's agents: no person.
        let (_, c) = st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        let GrantStart::Approved { grant: g, .. } = st
            .request_grant(DAEMON, grant([3; 32], Some(c.expose())))
            .unwrap()
        else {
            panic!()
        };
        let c = st.grant_caller(&g.id, None).unwrap();
        assert_eq!(c.person, None);
        // Everything, within the collaborator access the machine asked for.
        let collaborator = Rights::preset(PRESET_COLLABORATOR).unwrap();
        assert!(
            c.rights.is_narrower_or_equal(&collaborator)
                && collaborator.is_narrower_or_equal(&c.rights)
        );
    }

    #[test]
    fn an_agent_is_narrowed_by_its_person_and_elevated_by_the_owner_until_it_ends() {
        use crate::rights::{PRESET_FULL, PRESET_READER, RightType};
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        let none = |_: &str| false;
        let s1 = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let reader = Rights::preset(PRESET_READER).unwrap();
        let full = Rights::preset(PRESET_FULL).unwrap();
        // Wider than Sam's rights: refused, nothing written, no bump.
        let g0 = st.visibility_generation();
        assert_eq!(
            code(st.narrow_agent(DAEMON, &s1.id, Some(full.clone()))),
            "not_narrower"
        );
        assert_eq!(
            code(st.narrow_grant(DAEMON, &gid, Some(full.clone()))),
            "not_narrower"
        );
        assert_eq!(st.visibility_generation(), g0);
        assert!(st.agent_rights(&s1.id).unwrap().unwrap().narrow.is_none());
        // Narrowed to reading: one bump, and its rights follow.
        let a = st
            .narrow_agent(DAEMON, &s1.id, Some(reader.clone()))
            .unwrap();
        assert_eq!(st.visibility_generation(), g0 + 1);
        assert_eq!(a.narrow.as_ref(), Some(&reader));
        assert!(
            !a.rights
                .unwrap()
                .allows(RightType::Db, "write", Some("x"), &none)
        );
        // The same narrowing again changes nothing: no bump.
        st.narrow_agent(DAEMON, &s1.id, Some(reader.clone()))
            .unwrap();
        assert_eq!(st.visibility_generation(), g0 + 1);
        // The owner's elevation goes beyond Sam's rights, for this agent,
        // within what its narrowing leaves on (spec §7.2): read-only, it
        // deletes nothing; with the narrowing gone, it deletes anywhere.
        let a = st
            .elevate_agent(DAEMON, &s1.id, None, Some(full.clone()))
            .unwrap();
        assert_eq!(st.visibility_generation(), g0 + 2);
        assert!(
            !a.rights
                .unwrap()
                .allows(RightType::Artifacts, "delete", Some("x"), &none)
        );
        let a = st.narrow_agent(DAEMON, &s1.id, None).unwrap();
        assert_eq!(st.visibility_generation(), g0 + 3);
        assert!(
            a.rights
                .unwrap()
                .allows(RightType::Artifacts, "delete", Some("x"), &none)
        );
        let s2 = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let other = st.agent_rights(&s2.id).unwrap().unwrap();
        assert!(
            !other
                .rights
                .unwrap()
                .allows(RightType::Artifacts, "delete", Some("x"), &none)
        );
        // Ended by its person: both go, and nothing applies to an ended one.
        let ended = st.end_session_by_person(DAEMON, &s1.id).unwrap();
        assert!(ended.session.ended_at.is_some());
        assert!(st.agent_rights(&s1.id).unwrap().is_none());
        assert!(matches!(
            st.narrow_agent(DAEMON, &s1.id, None),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.elevate_agent(DAEMON, &s1.id, None, None),
            Err(CoreError::NotFound)
        ));
        let reason: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT ended_reason FROM sessions WHERE id = ?1",
                    params![s1.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(reason, "person");
        // An agent of this machine: rights do not limit the daemon token.
        let s0 = st.register_session(DAEMON, reg(None)).unwrap();
        assert_eq!(
            code(st.narrow_agent(DAEMON, &s0.id, Some(reader))),
            "token_agent"
        );
        assert_eq!(
            code(st.elevate_agent(DAEMON, &s0.id, None, Some(full))),
            "token_agent"
        );
    }

    #[test]
    fn people_lists_every_person_with_their_rights_revoked_ones_kept() {
        use crate::rights::PRESET_READER;
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (mia_g, _, _) = approved(&st, "Mia", KEY_B);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let mia = st.grant(&mia_g).unwrap().person.unwrap();
        let reader = Rights::preset(PRESET_READER).unwrap();
        st.set_person_rights(DAEMON, &mia, Some(reader.clone()))
            .unwrap();
        st.revoke_person(DAEMON, &sam).unwrap();
        let people = st.people().unwrap();
        let names: Vec<_> = people
            .iter()
            .map(|p| {
                (
                    p.viewer.display_name.clone().unwrap(),
                    p.viewer.person_revoked_at.is_some(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [("Sam".to_string(), true), ("Mia".to_string(), false)]
        );
        assert_eq!(people[1].rights, reader);
    }

    #[test]
    fn own_artifacts_are_those_the_person_created() {
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&gid).unwrap().person.unwrap();
        let s = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let p = |t: &str| {
            crate::publish::validate(
                serde_json::from_value(serde_json::json!({
                    "title": t, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
                }))
                .unwrap(),
            )
            .unwrap()
        };
        let (mine, _) = st.create_artifact(DAEMON, p("mine"), Some(&s.id)).unwrap();
        let (owners, _) = st.create_artifact(DAEMON, p("owner's"), None).unwrap();
        assert!(
            st.artifact_created_by(mine.id.as_str(), Some(&sam))
                .unwrap()
        );
        assert!(!st.artifact_created_by(mine.id.as_str(), None).unwrap());
        assert!(st.artifact_created_by(owners.id.as_str(), None).unwrap());
        assert!(
            !st.artifact_created_by(owners.id.as_str(), Some(&sam))
                .unwrap()
        );
        assert!(!st.artifact_created_by("nope", None).unwrap());
    }

    #[test]
    fn name_tokens_fold_case_diacritics_and_spaces() {
        let (_d, _c, st) = store();
        st.create_invite(DAEMON, InvitePerson::New("Zoë Ångström".into()), None)
            .unwrap();
        assert_eq!(
            code(st.create_invite(DAEMON, InvitePerson::New("zoe   angstrom".into()), None)),
            "name_taken"
        );
        let long = "x".repeat(MAX_CLIENT_NAME_CHARS + 1);
        assert_eq!(
            code(st.request_grant(
                DAEMON,
                NewGrant {
                    client_name: long,
                    ..grant(KEY_A, None)
                }
            )),
            "invalid_args"
        );
    }

    #[test]
    fn a_persons_agent_writes_as_the_person_and_page_versions_keep_the_publisher() {
        use crate::publish::{PublishRequest, validate};
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&gid).unwrap().person.unwrap();
        let s = st.register_session(DAEMON, reg(Some(&gid))).unwrap();
        let req = |v: Option<u32>| -> PublishRequest {
            serde_json::from_value(serde_json::json!({
                "if_version": v,
                "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}
            }))
            .unwrap()
        };
        let (a, _) = st
            .create_artifact(DAEMON, validate(req(None)).unwrap(), Some(&s.id))
            .unwrap();
        let aid = crate::ArtifactId::parse(&a.id).unwrap();
        let mut by_page = validate(req(Some(1))).unwrap();
        by_page.by_page = true;
        st.publish_version(DAEMON, &aid, by_page, None).unwrap();
        st.publish_version(DAEMON, &aid, validate(req(Some(2))).unwrap(), None)
            .unwrap();
        let asset = st
            .add_asset_by(DAEMON, &aid, "image/png", &[1], Some(&sam), false)
            .unwrap();
        let rows = st
            .with_read(|c| {
                let artifact: Option<String> =
                    c.query_row("SELECT person FROM artifacts WHERE id = ?1", [&a.id], |r| {
                        r.get(0)
                    })?;
                let asset: Option<String> = c.query_row(
                    "SELECT person FROM assets WHERE id = ?1",
                    [&asset.id],
                    |r| r.get(0),
                )?;
                let mut stmt = c.prepare(
                    "SELECT person, by_page FROM versions WHERE artifact_id = ?1 ORDER BY n",
                )?;
                let versions = stmt
                    .query_map([&a.id], |r| {
                        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, bool>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok((artifact, asset, versions))
            })
            .unwrap();
        let sam = Some(sam);
        assert_eq!(rows.0, sam);
        assert_eq!(rows.1, sam);
        assert_eq!(
            rows.2,
            vec![(sam.clone(), false), (None, true), (None, false)]
        );
        // The page's own version keeps its page's author, Sam.
        let aid = crate::ArtifactId::parse(&a.id).unwrap();
        let v2 = st.get_version(&aid, 2).unwrap().unwrap();
        assert_eq!(
            crate::visibility::authors_of_version(&v2),
            vec![
                crate::visibility::Author::Owner,
                crate::visibility::Author::Person(sam.clone().unwrap())
            ]
        );
    }

    #[test]
    fn claiming_a_browser_for_the_owner_keeps_the_owners_grants_and_skips_people() {
        let (_d, _c, st) = store();
        let (_, c) = st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        let GrantStart::Approved { grant: g, .. } = st
            .request_grant(DAEMON, grant(KEY_A, Some(c.expose())))
            .unwrap()
        else {
            panic!()
        };
        let cookie = crate::new_ulid();
        st.mint_viewer(&cookie, true).unwrap();
        assert!(matches!(
            st.claim_for_owner(crate::audit::Via::Shell, &cookie)
                .unwrap(),
            super::super::viewers::Claim::Adopted { retired: Some(_) }
        ));
        let owner = st.owner().unwrap().unwrap();
        let bound = st.viewer_of_grant(&g.id).unwrap();
        assert_eq!(
            bound.public_id, owner.public_id,
            "the grant follows the owner row"
        );
        // A person's locally minted browser is never claimed for the owner.
        let person = crate::new_ulid();
        let v = st.mint_viewer(&person, true).unwrap();
        st.create_invite(DAEMON, InvitePerson::Viewer(v.public_id), None)
            .unwrap();
        assert_eq!(
            st.claim_for_owner(crate::audit::Via::Shell, &person)
                .unwrap(),
            super::super::viewers::Claim::Nothing
        );
    }

    fn invited_with_link(st: &Store, name: &str, key: [u8; 32]) -> (Grant, Secret) {
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New(name.into()), None)
            .unwrap();
        match st
            .request_grant(DAEMON, grant(key, Some(c.expose())))
            .unwrap()
        {
            GrantStart::Approved {
                grant,
                browser_code: Some(b),
                ..
            } => (grant, b),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_browser_link_is_redeemed_once_for_its_person() {
        let (_d, _c, st) = store();
        let (g, link) = invited_with_link(&st, "Sam", KEY_A);
        assert!(matches!(
            st.redeem_browser_code("not-the-code"),
            Err(CoreError::NotFound)
        ));
        let v = st.redeem_browser_code(link.expose()).unwrap();
        assert_eq!(Some(v.public_id.clone()), g.person);
        assert!(v.person && !v.id.is_empty());
        assert!(
            matches!(
                st.redeem_browser_code(link.expose()),
                Err(CoreError::NotFound)
            ),
            "one use"
        );
    }

    #[test]
    fn a_browser_link_expires_after_ten_minutes() {
        let (_d, clock, st) = store();
        let (_, link) = invited_with_link(&st, "Sam", KEY_A);
        clock.advance(BROWSER_LINK_TTL_SECS);
        assert!(matches!(
            st.redeem_browser_code(link.expose()),
            Err(CoreError::NotFound)
        ));
        let (_, early) = invited_with_link(&st, "Mia", KEY_B);
        clock.advance(BROWSER_LINK_TTL_SECS - 1);
        assert!(st.redeem_browser_code(early.expose()).is_ok());
    }

    #[test]
    fn a_browser_link_survives_a_restart_and_follows_the_request_flow() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let clock = Arc::new(ManualClock::at(T0));
        let link = {
            let st = Store::open_with_clock(&home, clock.clone()).unwrap();
            let (g, cont, user_code) = pending(&st, KEY_A);
            st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)
                .unwrap();
            clock.advance(CONTINUE_WAIT_SECS);
            match st
                .continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
                .unwrap()
            {
                ContinueOutcome::Approved {
                    browser_code: Some(b),
                    ..
                } => b,
                other => panic!("{other:?}"),
            }
        };
        let st = Store::open_with_clock(&home, clock).unwrap();
        assert_eq!(
            st.redeem_browser_code(link.expose())
                .unwrap()
                .display_name
                .as_deref(),
            Some("Sam")
        );
    }

    #[test]
    fn a_browser_link_dies_with_its_grant() {
        let (_d, _c, st) = store();
        let (g, link) = invited_with_link(&st, "Sam", KEY_A);
        st.revoke_grant(DAEMON, &g.id, EndedBy::Owner).unwrap();
        assert!(matches!(
            st.redeem_browser_code(link.expose()),
            Err(CoreError::NotFound)
        ));
    }

    /// Hashes of secrets are compared only through `constant_time_eq`: no
    /// line of the store's code compares a hash, a stored or presented
    /// value, an exposed secret or a `Secret` with `==`, `!=` or `.eq(`
    /// (timing is not observable in a unit test, so this guards the
    /// source; review guards the rest).
    #[test]
    fn secret_hashes_are_never_compared_with_eq() {
        let src = include_str!("people.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        let bad: Vec<&str> = code
            .lines()
            .filter(|l| ["==", "!=", ".eq(", ".ne("].iter().any(|op| l.contains(op)))
            .filter(|l| {
                [
                    "hash",
                    "stored",
                    "_sha256",
                    "presented",
                    "expose",
                    "Secret",
                    "code",
                ]
                .iter()
                .any(|w| l.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "{bad:#?}");
    }

    fn tokens_of(st: &Store, gid: &str) -> Vec<([u8; 32], String)> {
        st.token_index()
            .unwrap()
            .into_iter()
            .filter(|(_, g)| g == gid)
            .collect()
    }

    #[test]
    fn the_rotated_continuation_token_cannot_issue_again() {
        let (_d, clock, st) = store();
        let (g, cont, user_code) = pending(&st, KEY_A);
        st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)
            .unwrap();
        clock.advance(CONTINUE_WAIT_SECS);
        let ContinueOutcome::Approved {
            token,
            continue_token,
            ..
        } = st
            .continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
            .unwrap()
        else {
            panic!()
        };
        for _ in 0..2 {
            clock.advance(CONTINUE_WAIT_SECS);
            assert_eq!(
                code(st.continue_grant(DAEMON, &g.id, continue_token.expose(), t(), &n())),
                "already_issued"
            );
        }
        assert_eq!(
            tokens_of(&st, &g.id),
            vec![(secret::sha256(token.expose()), g.id.clone())]
        );
    }

    #[test]
    fn a_request_past_ten_minutes_cannot_be_approved() {
        let (_d, clock, st) = store();
        let (g, _, user_code) = pending(&st, KEY_A);
        let viewers = |st: &Store| -> i64 {
            st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))?))
                .unwrap()
        };
        let before = viewers(&st);
        clock.advance(PENDING_TTL_SECS + 1);
        assert_eq!(
            code(st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)),
            "not_pending"
        );
        assert_ne!(st.grant(&g.id).unwrap().status, "approved");
        assert_eq!(viewers(&st), before, "no person was made");
    }

    #[test]
    fn denying_ends_a_request_once_and_never_an_approved_grant() {
        let (_d, clock, st) = store();
        let (g, cont, user_code) = pending(&st, KEY_A);
        assert_eq!(st.deny_grant(DAEMON, &g.id).unwrap().status, "denied");
        assert_eq!(st.grant(&g.id).unwrap().status, "denied");
        clock.advance(CONTINUE_WAIT_SECS);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "denied"
        );
        assert_eq!(
            code(st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)),
            "not_pending"
        );
        assert_eq!(code(st.deny_grant(DAEMON, &g.id)), "not_pending");
        let (gid, token, _) = approved(&st, "Mia", KEY_B);
        assert_eq!(code(st.deny_grant(DAEMON, &gid)), "not_pending");
        assert!(st.token_live(token.expose()).unwrap());
        assert!(matches!(
            st.deny_grant(DAEMON, "g_unknown"),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn the_token_index_holds_each_live_token_once_and_revoking_marks_the_rows() {
        let (_d, _c, st) = store();
        let (gid, token, manage) = approved(&st, "Sam", KEY_A);
        assert_eq!(
            st.token_index().unwrap(),
            vec![(secret::sha256(token.expose()), gid.clone())]
        );
        st.revoke_token(DAEMON, &gid, manage.expose()).unwrap();
        let unrevoked: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM grant_tokens WHERE grant_id = ?1 AND revoked_at IS NULL",
                    [&gid],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(unrevoked, 0);
    }

    #[test]
    fn the_credential_index_holds_every_token_with_its_key_and_marks_ended_grants() {
        let (_d, _c, st) = store();
        let find = |st: &Store, token: &str| {
            let h = secret::sha256(token);
            st.credential_index()
                .unwrap()
                .into_iter()
                .find(|c| c.sha256 == h)
        };
        let (pending_grant, cont, code) = pending(&st, KEY_B);
        assert!(
            find(&st, cont.expose()).is_none(),
            "a request enters the index once its tokens are issued"
        );
        assert_eq!(
            st.unissued_continuation(&pending_grant.id, cont.expose())
                .unwrap(),
            Some(Unissued::Live(KEY_B))
        );
        assert_eq!(
            st.unissued_continuation(&pending_grant.id, "cxc_other")
                .unwrap(),
            None
        );
        st.approve_grant(DAEMON, &pending_grant.id, code.expose(), "Mia", None)
            .unwrap();
        let ContinueOutcome::Approved {
            token: mia_token,
            continue_token: mia_cont,
            ..
        } = st
            .continue_grant(DAEMON, &pending_grant.id, cont.expose(), t(), &n())
            .unwrap()
        else {
            panic!("approved")
        };
        assert!(find(&st, cont.expose()).is_none(), "the old one is gone");
        assert_eq!(
            find(&st, mia_cont.expose()).map(|c| (c.kind, c.grant_id, c.key, c.revoked)),
            Some((
                CredentialKind::Continue,
                pending_grant.id.clone(),
                KEY_B,
                false
            ))
        );
        assert_eq!(
            st.unissued_continuation(&pending_grant.id, mia_cont.expose())
                .unwrap(),
            None,
            "an issued grant is the index's"
        );
        assert_eq!(
            find(&st, mia_token.expose()).map(|c| (c.kind, c.key, c.revoked)),
            Some((CredentialKind::Access, KEY_B, false))
        );
        let (gid, token, manage) = approved(&st, "Sam", KEY_A);
        assert_eq!(
            find(&st, manage.expose()).map(|c| (c.kind, c.grant_id, c.revoked)),
            Some((CredentialKind::Manage, gid.clone(), false))
        );
        st.revoke_token(DAEMON, &gid, manage.expose()).unwrap();
        for t in [&token, &manage] {
            assert_eq!(find(&st, t.expose()).map(|c| c.revoked), Some(true));
        }
        assert_eq!(
            find(&st, mia_token.expose()).map(|c| c.revoked),
            Some(false),
            "another grant's token stays live"
        );
        // A denied request never enters the index; the store answers it.
        let (denied, dcont, _) = pending(&st, KEY_A);
        st.deny_grant(DAEMON, &denied.id).unwrap();
        assert!(find(&st, dcont.expose()).is_none());
        assert_eq!(
            st.unissued_continuation(&denied.id, dcont.expose())
                .unwrap(),
            Some(Unissued::Ended)
        );
    }

    #[test]
    fn cancelled_requests_count_against_the_caps_for_their_ten_minutes() {
        let (_d, clock, st) = store();
        for i in 0..MAX_PENDING_PER_IP as u8 {
            let GrantStart::Pending {
                grant,
                continue_token,
                ..
            } = st
                .request_grant(DAEMON, from_ip([10 + i; 32], "10.0.0.1"))
                .unwrap()
            else {
                panic!("pending")
            };
            let ended = st
                .disconnect_grant(DAEMON, &grant.id, continue_token.expose(), t(), &n())
                .unwrap();
            assert!(ended.ended);
        }
        assert_eq!(
            code(st.request_grant(DAEMON, from_ip([99; 32], "10.0.0.1"))),
            "limit_reached"
        );
        clock.advance(PENDING_TTL_SECS + 1);
        st.request_grant(DAEMON, from_ip([99; 32], "10.0.0.1"))
            .unwrap();
    }

    #[test]
    fn requests_never_issued_are_deleted_after_their_retention() {
        let (_d, clock, st) = store();
        let (denied, _, _) = pending(&st, KEY_A);
        st.deny_grant(DAEMON, &denied.id).unwrap();
        let (cancelled, cont, _) = pending(&st, KEY_B);
        st.disconnect_grant(DAEMON, &cancelled.id, cont.expose(), t(), &n())
            .unwrap();
        let (_, _, _) = pending(&st, [3; 32]);
        let (sam, _, _) = approved(&st, "Sam", [4; 32]);
        clock.advance(PENDING_TTL_SECS + 1);
        // The lapsed request is reported first, and kept.
        assert_eq!(st.expire_grants().unwrap().len(), 1);
        assert_eq!(st.list_grants().unwrap().len(), 4);
        clock.advance(UNISSUED_RETENTION_SECS);
        st.expire_grants().unwrap();
        let left: Vec<String> = st
            .list_grants()
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(left, vec![sam], "an issued grant stays as history");
        assert!(matches!(st.grant(&denied.id), Err(CoreError::NotFound)));
    }

    #[test]
    fn a_continuation_call_is_refused_before_the_last_or_repeating_it() {
        let (_d, clock, st) = store();
        let (g, cont, code_) = pending(&st, KEY_A);
        clock.advance(CONTINUE_WAIT_SECS);
        st.continue_grant(DAEMON, &g.id, cont.expose(), 100, "n1")
            .unwrap();
        st.approve_grant(DAEMON, &g.id, code_.expose(), "Sam", None)
            .unwrap();
        // The pending poll, replayed after the approval, takes nothing; nor
        // does an older one.
        for (t, nonce) in [(100, "n1"), (99, "n0")] {
            assert_eq!(
                code(st.continue_grant(DAEMON, &g.id, cont.expose(), t, nonce)),
                "replayed_request"
            );
        }
        // Another call in the same second is accepted.
        assert!(matches!(
            st.continue_grant(DAEMON, &g.id, cont.expose(), 100, "n2")
                .unwrap(),
            ContinueOutcome::Approved { .. }
        ));
        let (d, dcont, _) = pending(&st, KEY_B);
        clock.advance(CONTINUE_WAIT_SECS);
        st.continue_grant(DAEMON, &d.id, dcont.expose(), 50, "m1")
            .unwrap();
        assert_eq!(
            code(st.disconnect_grant(DAEMON, &d.id, dcont.expose(), 50, "m1")),
            "replayed_request"
        );
        assert_eq!(
            code(st.disconnect_grant(DAEMON, &d.id, dcont.expose(), 49, "m9")),
            "replayed_request"
        );
        // A poll and a disconnect in one second.
        assert!(
            st.disconnect_grant(DAEMON, &d.id, dcont.expose(), 50, "m2")
                .unwrap()
                .ended
        );
        assert_eq!(
            st.unissued_continuation(&d.id, dcont.expose()).unwrap(),
            Some(Unissued::Ended)
        );
    }

    #[test]
    fn ending_an_ended_grant_ends_nothing() {
        let (_d, _c, st) = store();
        let (gid, _, _) = approved(&st, "Sam", KEY_A);
        assert!(st.revoke_grant(DAEMON, &gid, EndedBy::Owner).unwrap().ended);
        let again = st.revoke_grant(DAEMON, &gid, EndedBy::Owner).unwrap();
        assert!(!again.ended && again.sessions_ended.is_empty());
    }

    #[test]
    fn issued_tokens_carry_the_access_the_grant_holds() {
        let (_d, clock, st) = store();
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        let reader = Rights::preset(crate::rights::PRESET_READER).unwrap();
        let GrantStart::Approved { access, .. } = st
            .request_grant(
                DAEMON,
                NewGrant {
                    access: reader.clone(),
                    ..grant(KEY_A, Some(c.expose()))
                },
            )
            .unwrap()
        else {
            panic!("approved")
        };
        assert!(access.is_narrower_or_equal(&reader) && reader.is_narrower_or_equal(&access));
        let (g, cont, code_) = pending(&st, KEY_B);
        st.approve_grant(DAEMON, &g.id, code_.expose(), "Mia", None)
            .unwrap();
        clock.advance(CONTINUE_WAIT_SECS);
        let ContinueOutcome::Approved { access, .. } = st
            .continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())
            .unwrap()
        else {
            panic!("approved")
        };
        let collaborator = Rights::preset(crate::rights::PRESET_COLLABORATOR).unwrap();
        assert!(
            access.is_narrower_or_equal(&collaborator)
                && collaborator.is_narrower_or_equal(&access)
        );
    }

    #[test]
    fn invite_codes_never_print() {
        let g = grant(KEY_A, Some("K7QF-M2XD-9PNA"));
        let shown = format!("{g:?}");
        assert!(
            !shown.contains("K7QF") && !shown.contains("9PNA"),
            "{shown}"
        );
    }

    #[test]
    fn an_invite_carries_its_persons_rights() {
        let (_d, _c, st) = store();
        let reader = Rights::preset(crate::rights::PRESET_READER).unwrap();
        let (inv, _) = st
            .create_invite(
                DAEMON,
                InvitePerson::New("Sam".into()),
                Some(reader.clone()),
            )
            .unwrap();
        assert_eq!(st.person_rights(&inv.person).unwrap(), reader);
        let (me, _) = st
            .create_invite(DAEMON, InvitePerson::Owner, Some(reader))
            .unwrap();
        assert_eq!(
            st.person_rights(&me.person).unwrap(),
            Rights::full(),
            "ignored for the owner"
        );
    }

    #[test]
    fn an_unredeemed_invites_person_goes_back_to_a_viewer() {
        let (_d, clock, st) = store();
        let lan = st.upsert_viewer(&crate::new_ulid(), Some("Lan")).unwrap();
        let (inv, _) = st
            .create_invite(DAEMON, InvitePerson::Viewer(lan.public_id.clone()), None)
            .unwrap();
        assert!(st.get_viewer(&lan.id).unwrap().unwrap().person);
        st.revoke_invite(DAEMON, &inv.id).unwrap();
        assert!(!st.get_viewer(&lan.id).unwrap().unwrap().person);
        assert_eq!(code(st.person_rights(&lan.public_id)), "not_a_person");
        // A new person whose invite expires frees the name; a person with a
        // grant stays one.
        st.create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        approved(&st, "Mia", KEY_A);
        clock.advance(INVITE_TTL_SECS + 1);
        st.expire_grants().unwrap();
        st.create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        assert_eq!(
            code(st.create_invite(DAEMON, InvitePerson::New("Mia".into()), None)),
            "name_taken"
        );
    }

    #[test]
    fn a_person_revoked_before_connecting_stays_revoked_when_the_invite_lapses() {
        let (_d, clock, st) = store();
        let (inv, _) = st
            .create_invite(DAEMON, InvitePerson::New("Ola".into()), None)
            .unwrap();
        let r = st.revoke_person(DAEMON, &inv.person).unwrap();
        assert_eq!(r.invites, vec![inv.id]);
        clock.advance(INVITE_TTL_SECS + 1);
        st.expire_grants().unwrap();
        let v = st.viewer_by_public_id(&inv.person).unwrap().unwrap();
        assert!(
            v.person && v.person_revoked_at.is_some(),
            "the revocation stays on record, not a plain viewer"
        );
    }

    #[test]
    fn requests_expired_on_the_way_are_reported_and_lost_approvals_free_the_key() {
        let (_d, clock, st) = store();
        let (stale, _, _) = pending(&st, KEY_A);
        clock.advance(PENDING_TTL_SECS + 1);
        // The same key asks again: the stale request expires on the way.
        let (fresh, cont, user_code) = pending(&st, KEY_A);
        assert_eq!(st.grant(&stale.id).unwrap().status, "expired");
        let ended = st.expire_grants().unwrap();
        assert_eq!(
            ended
                .iter()
                .map(|e| e.grant_id.as_str())
                .collect::<Vec<_>>(),
            [stale.id.as_str()]
        );
        assert!(st.grant(&stale.id).unwrap().ended_at.is_some());
        assert!(st.expire_grants().unwrap().is_empty(), "reported once");
        // Approved, but the client never collects its tokens.
        st.approve_grant(DAEMON, &fresh.id, user_code.expose(), "Sam", None)
            .unwrap();
        clock.advance(PENDING_TTL_SECS + 1);
        let ended = st.expire_grants().unwrap();
        assert_eq!(
            ended
                .iter()
                .map(|e| e.grant_id.as_str())
                .collect::<Vec<_>>(),
            [fresh.id.as_str()]
        );
        assert_eq!(
            code(st.continue_grant(DAEMON, &fresh.id, cont.expose(), t(), &n())),
            "denied"
        );
        assert!(matches!(
            st.request_grant(DAEMON, grant(KEY_A, None)).unwrap(),
            GrantStart::Pending { .. }
        ));
    }

    #[test]
    fn a_lost_approval_polled_late_is_denied_and_frees_the_key() {
        let (_d, clock, st) = store();
        let (g, cont, user_code) = pending(&st, KEY_A);
        st.approve_grant(DAEMON, &g.id, user_code.expose(), "Sam", None)
            .unwrap();
        clock.advance(PENDING_TTL_SECS + 1);
        assert_eq!(
            code(st.continue_grant(DAEMON, &g.id, cont.expose(), t(), &n())),
            "denied"
        );
        assert!(st.token_index().unwrap().is_empty());
        assert!(st.request_grant(DAEMON, grant(KEY_A, None)).is_ok());
        assert_eq!(
            st.expire_grants()
                .unwrap()
                .iter()
                .map(|e| e.grant_id.as_str())
                .collect::<Vec<_>>(),
            [g.id.as_str()]
        );
    }
}
