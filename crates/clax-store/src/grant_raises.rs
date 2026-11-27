//! Grants on a token: one granter's grant of zero-default rights
//! ([`clax_core::rights::RightType::zero_by_default`], `clax-queues` today)
//! to a grant (an agent token, or a connected machine), one per (grant,
//! granter; `None` the owner), kept in `grant_elevations`. It is a raise
//! (spec §7.2, [`super::elevations`]) whose target is the grant, not one
//! session: it reaches every session of the grant, its requests that name
//! no session included, and outlives them; it ends with the grant.
//!
//! A session's rights of the zero-default types are exactly the union of
//! the grants that reach it, each read at every check within its holders
//! ([`clax_core::rights::granted_within`]): what its granter may give now
//! (a queue's members hold every queue action there; the owner holds them
//! at the queues the owner made), and what the grant's own person may give
//! there, which a grant never exceeds. A granter who left a queue, or was
//! revoked as a person, gives nothing from the next check on; their rows
//! are deleted too when they are revoked, released or folded
//! ([`drop_granted_in`]). After every change that can take a capability
//! away, the state held under it is revisited
//! ([`Store::revisit_queue_rights_in`]): a live claim whose `claim` is gone
//! ends, and a queue watch whose `read` is gone stops.
//!
//! A token made to ask for a report queue alone (a device sign-in that
//! asked for `scope: ["queue.request"]`, [`crate::queue_requests`]) holds
//! one grant of this kind and nothing else: `clax-queue-requests`
//! `request`, from the person who approved the sign-in, with an end of its
//! own (`grant_elevations.expires_at`, [`issue_request_grant_in`]). Its
//! narrowing is `[]`, so its person's rights give it nothing; every other
//! grant's person gives it `request` with the rest of what an agent of
//! theirs holds ([`super::people::grant_caller_in`]). Its rights are read
//! by the same evaluation as every other token's.
//!
//! Each change is recorded as `grant.elevate {grant_id, access, granter,
//! reason?, expires_at?}`.

use super::Store;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_core::rights::{RightType, Rights, granted_within};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// One granter's grant on a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRaise {
    /// The grant (token) it is on.
    pub grant_id: String,
    /// The granting person's public ID; `None` for the owner.
    pub granter: Option<String>,
    /// The access the granter gave, as given.
    pub access: Rights,
    pub created_at: String,
    /// A new value at each change of it (a ULID), for `if_version`.
    pub version: String,
}

/// `clax-queue-requests` `request`: asking one's person for a report
/// queue, which every agent of a person holds and a queue-request token
/// holds alone.
///
/// # Errors
/// None in practice: the value is fixed.
pub fn queue_request_access() -> Result<Rights> {
    Rights::parse(&serde_json::json!([
        {"type": RightType::QueueRequests.as_str(), "actions": ["request"]}
    ]))
}

/// The end of grant `gid`'s queue-request grant, on `c`, when `gid` is a
/// queue-request token (its one grant on it has an end of its own,
/// [`issue_request_grant_in`]); `None` for any other grant, whose person
/// gives it its rights.
///
/// # Errors
/// Database errors.
pub(crate) fn request_grant_in(c: &Connection, gid: &str) -> Result<Option<String>> {
    Ok(c.prepare_cached(
        "SELECT expires_at FROM grant_elevations WHERE grant_id = ?1 AND expires_at IS NOT NULL
         ORDER BY expires_at LIMIT 1",
    )?
    .query_row(params![gid], |r| r.get(0))
    .optional()?)
}

/// Makes grant `gid` a queue-request token, in `tx`: `granter` (`None`:
/// the owner), who approved its sign-in, grants it `clax-queue-requests`
/// `request` until `expires_at`, recorded as `grant.elevate` with
/// `expires_at`. Its narrowing is set to `[]`, so nothing else reaches it.
///
/// # Errors
/// Database errors.
pub(crate) fn issue_request_grant_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    gid: &str,
    granter: Option<&str>,
    expires_at: &str,
) -> Result<()> {
    let access = queue_request_access()?;
    tx.execute(
        "UPDATE grants SET narrow_json = '[]' WHERE id = ?1",
        params![gid],
    )?;
    tx.execute(
        "INSERT INTO grant_elevations (grant_id, granter, access_json, created_at, version, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            gid,
            granter,
            access.to_json().to_string(),
            st.now(),
            clax_core::new_ulid(),
            expires_at
        ],
    )?;
    let rec = super::people::grant_record(AuditKind::GrantElevate, &st.now(), gid)
        .with("access", access.to_json())
        .with("granter", granter)
        .with("expires_at", expires_at);
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

fn read_rights(json: &str) -> Result<Rights> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| CoreError::invalid("invalid_rights", "stored rights do not parse"))?;
    Rights::parse_internal(&v)
}

/// Grant `gid`'s grants on `c`, the owner's first, then oldest first.
pub(super) fn of_grant(c: &Connection, gid: &str) -> Result<Vec<GrantRaise>> {
    let mut stmt = c.prepare_cached(
        "SELECT granter, access_json, created_at, version FROM grant_elevations
         WHERE grant_id = ?1 ORDER BY granter IS NOT NULL, created_at, granter",
    )?;
    let rows = stmt
        .query_map(params![gid], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(granter, json, created_at, version)| {
            Ok(GrantRaise {
                grant_id: gid.to_string(),
                granter,
                access: read_rights(&json)?,
                created_at,
                version,
            })
        })
        .collect()
}

/// Whether `person` (a public ID) is a connected person who is not
/// revoked: one who may still give what they hold.
fn standing(c: &Connection, person: &str) -> Result<bool> {
    Ok(c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM viewers WHERE public_id = ?1 AND person = 1
           AND owner = 0 AND person_revoked_at IS NULL)",
    )?
    .query_row(params![person], |r| r.get(0))?)
}

/// What `person` (`None`: the owner) holds of the zero-default types now,
/// on `c`: every `clax-queues` action at each live queue they made or are
/// a member of (the owner's membership is kept under
/// [`super::queues::OWNER_MEMBER`]); nothing for a person who is not
/// connected or is revoked. Overseeing every queue in a browser (the owner
/// and the instance's administrators) is no membership and gives a token
/// nothing: their agents work the queues they are members of.
pub(crate) fn holds_in(c: &Connection, person: Option<&str>) -> Result<Rights> {
    if let Some(p) = person
        && !standing(c, p)?
    {
        return Ok(Rights::default());
    }
    let ids: Vec<String> = c
        .prepare_cached(
            "SELECT id FROM queues WHERE deleted_at IS NULL AND (person IS ?1
               OR id IN (SELECT queue_id FROM queue_members WHERE person = ?2))
             ORDER BY id",
        )?
        .query_map(params![person, super::queues::member_key(person)], |r| {
            r.get(0)
        })?
        .collect::<rusqlite::Result<_>>()?;
    // Anyone who may answer a queue request may let a token ask them.
    Ok(Rights::every_action_at(RightType::Queues, ids).union(&queue_request_access()?))
}

/// The person grant `gid` speaks for (`Some(None)`: the owner's), while it
/// is approved; `None` otherwise.
fn grant_person(c: &Connection, gid: &str) -> Result<Option<Option<String>>> {
    let row: Option<(String, bool, String)> = c
        .prepare_cached(
            "SELECT g.status, COALESCE(v.owner, 0), v.public_id
               FROM grants g JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
        )?
        .query_row(params![gid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    Ok(row
        .filter(|r| r.0 == "approved")
        .map(|(_, owner, p)| (!owner).then_some(p)))
}

/// What each of grant `gid`'s grants gives now, on `c`: its access within
/// its granter's holdings and the grant's person's ([`holds_in`]). Empty
/// for a grant that is not approved.
pub(crate) fn given_in(c: &Connection, gid: &str) -> Result<Vec<Rights>> {
    let all = of_grant(c, gid)?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let Some(person) = grant_person(c, gid)? else {
        return Ok(Vec::new());
    };
    let theirs = holds_in(c, person.as_deref())?;
    let mut out = Vec::with_capacity(all.len());
    for g in &all {
        let granter = holds_in(c, g.granter.as_deref())?;
        out.push(granted_within(&g.access, &[&granter, &theirs]));
    }
    Ok(out)
}

/// 400 `invalid_rights` for a grant on a token that gives nothing, or
/// names a type that is not zero by default (those are a person's rights,
/// narrowed and raised per agent).
fn check_shape(access: &Rights) -> Result<()> {
    if access.is_empty() {
        return Err(CoreError::invalid(
            "invalid_rights",
            "name the access to grant; DELETE removes a grant",
        ));
    }
    if !access.only_zero_default() || access.allows_somewhere(RightType::QueueRequests, "request") {
        return Err(CoreError::invalid(
            "invalid_rights",
            "a token is granted clax-queues access alone: other rights are its person's, narrowed and raised per agent",
        ));
    }
    Ok(())
}

/// 403 `beyond_your_rights` unless `granter` (`None`: the owner) holds
/// every action `access` names at every queue it names by ID (`*` stands
/// for what they hold, read at each check).
fn check_holds(c: &Connection, granter: Option<&str>, access: &Rights) -> Result<()> {
    let holds = holds_in(c, granter)?;
    if !access.named_only().is_narrower_or_equal(&holds) {
        return Err(CoreError::invalid(
            "beyond_your_rights",
            "you grant a token only what you hold yourself: queue actions on the queues you are a member of",
        ));
    }
    Ok(())
}

/// Replaces (or with `None` deletes) `granter`'s grant on `gid` in `tx`;
/// whether it changed.
fn put_in(
    tx: &Transaction<'_>,
    gid: &str,
    granter: Option<&str>,
    access: Option<&Rights>,
    now: &str,
) -> Result<bool> {
    let json = access.map(|a| a.to_json().to_string());
    let was: Option<String> = tx
        .query_row(
            "SELECT access_json FROM grant_elevations WHERE grant_id = ?1 AND granter IS ?2",
            params![gid, granter],
            |r| r.get(0),
        )
        .optional()?;
    if was == json {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM grant_elevations WHERE grant_id = ?1 AND granter IS ?2",
        params![gid, granter],
    )?;
    if let Some(j) = &json {
        tx.execute(
            "INSERT INTO grant_elevations (grant_id, granter, access_json, created_at, version)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![gid, granter, j, now, clax_core::new_ulid()],
        )?;
    }
    Ok(true)
}

/// `grant.elevate {grant_id, access, granter}` (and `reason` for a removal
/// by someone other than the granter, or an end the granter did not ask
/// for).
fn record(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    (gid, granter): (&str, Option<&str>),
    access: Option<&Rights>,
    reason: Option<&str>,
) -> Result<()> {
    let mut rec = super::people::grant_record(AuditKind::GrantElevate, &st.now(), gid)
        .with("access", access.map(Rights::to_json))
        .with("granter", granter);
    if let Some(r) = reason {
        rec = rec.with("reason", r);
    }
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Deletes every grant on a token that `granter` gave, recording each
/// with `reason` (as [`super::elevations::drop_granted_in`] does for
/// session raises), then revisits what they held up.
pub(super) fn drop_granted_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    granter: &str,
    reason: &str,
) -> Result<()> {
    let gids: Vec<String> = {
        let mut stmt =
            tx.prepare("DELETE FROM grant_elevations WHERE granter = ?1 RETURNING grant_id")?;
        stmt.query_map(params![granter], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for gid in &gids {
        record(st, tx, ctx, (gid, Some(granter)), None, Some(reason))?;
    }
    if !gids.is_empty() {
        st.visibility_changed();
    }
    st.revisit_queue_rights_in(tx, ctx)?;
    Ok(())
}

impl Store {
    /// Grant `gid`'s grants, the owner's first, then oldest first. Its
    /// rows stay after the grant ends, giving nothing.
    ///
    /// # Errors
    /// The store's.
    pub fn grant_raises(&self, gid: &str) -> Result<Vec<GrantRaise>> {
        self.with_read(|c| of_grant(c, gid))
    }

    /// The live grants (approved tokens) `granter` (`None`: the owner)
    /// gave a grant on, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn grant_raises_by(&self, granter: Option<&str>) -> Result<Vec<GrantRaise>> {
        self.with_read(|c| {
            let rows: Vec<(String, String, String, String)> = c
                .prepare(
                    "SELECT e.grant_id, e.access_json, e.created_at, e.version
                       FROM grant_elevations e JOIN grants g ON g.id = e.grant_id
                     WHERE e.granter IS ?1 AND g.status = 'approved'
                     ORDER BY e.created_at, e.grant_id",
                )?
                .query_map(params![granter], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows.into_iter()
                .map(|(gid, json, created_at, version)| {
                    Ok(GrantRaise {
                        grant_id: gid,
                        granter: granter.map(str::to_string),
                        access: read_rights(&json)?,
                        created_at,
                        version,
                    })
                })
                .collect()
        })
    }

    /// What `person` (`None`: the owner) may grant a token now: every
    /// `clax-queues` action at each queue they are a member of.
    ///
    /// # Errors
    /// The store's.
    pub fn grant_holds(&self, person: Option<&str>) -> Result<Rights> {
        self.with_read(|c| holds_in(c, person))
    }

    /// What each of grant `gid`'s grants gives now, and their union: the
    /// token's `clax-queues` rights, whatever session it acts from.
    ///
    /// # Errors
    /// The store's.
    pub fn granted_on(&self, gid: &str) -> Result<Rights> {
        self.with_read(|c| {
            Ok(given_in(c, gid)?
                .iter()
                .fold(Rights::default(), |r, g| r.union(g)))
        })
    }

    /// Sets (or with `None` removes) `granter`'s (`None`: the owner's)
    /// grant on approved grant `gid`, only over the grant the caller built
    /// on when `if_version` is given (`Some(None)`: none). From the grant's
    /// next request on, every session of it may also do what the grant
    /// gives, within what `granter` and the grant's person may give then.
    /// `reason` names a removal by someone other than the granter.
    /// Recorded as `grant.elevate`; a change bumps the visibility
    /// generation and revisits the state the grant's sessions hold under
    /// it ([`Store::revisit_queue_rights_in`]).
    ///
    /// # Errors
    /// `NotFound` for a grant that is not approved, or a queue-request
    /// token; `invalid_rights` for an
    /// empty access or one naming a type that is not zero by default;
    /// `beyond_your_rights` for an action at a queue the granter does not
    /// hold; `raise_changed` when `if_version` is not the grant's version;
    /// `not_raised` for a removal of nothing.
    pub fn raise_grant(
        &self,
        ctx: &AuditCtx,
        gid: &str,
        granter: Option<&str>,
        access: Option<Rights>,
        if_version: Option<Option<&str>>,
        reason: Option<&str>,
    ) -> Result<Vec<GrantRaise>> {
        self.with_tx(|tx| {
            grant_person(tx, gid)?.ok_or(CoreError::NotFound)?;
            // A queue-request token holds its one grant to ask and takes
            // no other.
            if request_grant_in(tx, gid)?.is_some() {
                return Err(CoreError::NotFound);
            }
            if let Some(expected) = if_version {
                let current: Option<String> = tx
                    .query_row(
                        "SELECT version FROM grant_elevations WHERE grant_id = ?1 AND granter IS ?2",
                        params![gid, granter],
                        |r| r.get(0),
                    )
                    .optional()?;
                if current.as_deref() != expected {
                    return Err(super::elevations::raise_changed());
                }
            }
            if let Some(a) = &access {
                check_shape(a)?;
                check_holds(tx, granter, a)?;
            }
            let now = self.now();
            let changed = put_in(tx, gid, granter, access.as_ref(), &now)?;
            if access.is_none() && !changed {
                return Err(super::elevations::not_raised());
            }
            if changed {
                record(self, tx, ctx, (gid, granter), access.as_ref(), reason)?;
                self.visibility_changed();
                self.revisit_queue_rights_in(tx, ctx)?;
            }
            of_grant(tx, gid)
        })
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::RegisterSession;
    use crate::people::tests::{KEY_A, KEY_B, approved, code, store};
    use crate::queues::NewQueue;
    use crate::test_util::DAEMON;
    use clax_core::queue::{ReportersSee, Role};
    use serde_json::json;

    fn q(v: serde_json::Value) -> Rights {
        Rights::parse(&v).unwrap()
    }

    fn queue(st: &Store, maker: Option<&str>, name: &str) -> String {
        st.create_queue(
            DAEMON,
            maker,
            NewQueue {
                name: name.into(),
                origins: vec!["https://app.example.com".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
    }

    fn may(st: &Store, gid: &str, action: &str, qid: &str) -> bool {
        st.grant_caller(gid, None).unwrap().rights.allows(
            RightType::Queues,
            action,
            Some(qid),
            &|_| false,
        )
    }

    /// A token holds no queue right until one is granted; a grant is
    /// capped by its granter's and its person's membership at every check;
    /// two granters' grants unite, and either alone still proves what it
    /// gives.
    #[test]
    fn a_tokens_queue_rights_are_the_union_of_its_live_grants() {
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (mia_g, _, _) = approved(&st, "Mia", KEY_B);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let mia = st.grant(&mia_g).unwrap().person.unwrap();
        let q1 = queue(&st, None, "One");
        for p in [&sam, &mia] {
            st.set_member(DAEMON, &q1, p, Some(Role::Triage)).unwrap();
        }
        // Default deny, members or not.
        assert!(!may(&st, &sam_g, "read", &q1));
        // Mia grants Sam's token read; the owner grants it resolve.
        let read = q(json!([{"type": "clax-queues", "actions": ["read"], "locations": [q1]}]));
        let resolve =
            q(json!([{"type": "clax-queues", "actions": ["resolve"], "locations": ["*"]}]));
        st.raise_grant(DAEMON, &sam_g, Some(&mia), Some(read.clone()), None, None)
            .unwrap();
        st.raise_grant(DAEMON, &sam_g, None, Some(resolve.clone()), None, None)
            .unwrap();
        assert!(may(&st, &sam_g, "read", &q1));
        assert!(may(&st, &sam_g, "resolve", &q1));
        assert!(!may(&st, &sam_g, "claim", &q1));
        assert!(!may(&st, &mia_g, "read", &q1), "a grant is on one token");
        // Removing the owner's leaves Mia's read; removing both, nothing.
        st.raise_grant(DAEMON, &sam_g, None, None, None, None)
            .unwrap();
        assert!(may(&st, &sam_g, "read", &q1));
        assert!(!may(&st, &sam_g, "resolve", &q1));
        st.raise_grant(DAEMON, &sam_g, Some(&mia), None, None, None)
            .unwrap();
        assert!(!may(&st, &sam_g, "read", &q1));
        assert_eq!(
            code(st.raise_grant(DAEMON, &sam_g, Some(&mia), None, None, None)),
            "not_raised"
        );
        // Beyond the granter: a queue Mia is not in, or other rights.
        let q2 = queue(&st, None, "Two");
        let at_q2 = q(json!([{"type": "clax-queues", "actions": ["read"], "locations": [q2]}]));
        assert_eq!(
            code(st.raise_grant(DAEMON, &sam_g, Some(&mia), Some(at_q2.clone()), None, None)),
            "beyond_your_rights"
        );
        assert_eq!(
            code(st.raise_grant(
                DAEMON,
                &sam_g,
                None,
                Some(Rights::preset("clax:reader").unwrap()),
                None,
                None
            )),
            "invalid_rights"
        );
        // The owner made q2 and grants it, but Sam is no member: nothing.
        st.raise_grant(DAEMON, &sam_g, None, Some(at_q2), None, None)
            .unwrap();
        assert!(!may(&st, &sam_g, "read", &q2), "never beyond its person");
        // Mia's `*` follows her membership: she leaves q1, it gives
        // nothing; she is revoked, the row goes.
        st.raise_grant(DAEMON, &sam_g, Some(&mia), Some(resolve), None, None)
            .unwrap();
        assert!(may(&st, &sam_g, "resolve", &q1));
        st.set_member(DAEMON, &q1, &mia, None).unwrap();
        assert!(!may(&st, &sam_g, "resolve", &q1));
        st.set_member(DAEMON, &q1, &mia, Some(Role::Triage))
            .unwrap();
        assert!(may(&st, &sam_g, "resolve", &q1));
        st.revoke_person(DAEMON, &mia).unwrap();
        assert!(!may(&st, &sam_g, "resolve", &q1));
        assert!(st.grant_raises_by(Some(&mia)).unwrap().is_empty());
        // A version guards a change.
        st.raise_grant(DAEMON, &sam_g, None, None, None, None)
            .unwrap();
        let v = st
            .raise_grant(DAEMON, &sam_g, None, Some(read.clone()), Some(None), None)
            .unwrap()[0]
            .version
            .clone();
        assert_eq!(
            code(st.raise_grant(DAEMON, &sam_g, None, Some(read.clone()), Some(None), None)),
            "raise_changed"
        );
        st.raise_grant(DAEMON, &sam_g, None, None, Some(Some(&v)), None)
            .unwrap();
        // A session of the token holds them too.
        st.raise_grant(DAEMON, &sam_g, None, Some(read), None, None)
            .unwrap();
        let sid = st
            .register_session(
                DAEMON,
                RegisterSession {
                    harness: "codex".into(),
                    cwd: "/w".into(),
                    grant_id: Some(sam_g.clone()),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        assert!(st.grant_caller(&sam_g, Some(&sid)).unwrap().rights.allows(
            RightType::Queues,
            "read",
            Some(&q1),
            &|_| false
        ));
        // Revoking the token: it gives nothing.
        st.revoke_grant(DAEMON, &sam_g, crate::people::EndedBy::Person)
            .unwrap();
        assert!(st.granted_on(&sam_g).unwrap().is_empty());
    }

    /// Migration 74 makes `grant_elevations` empty: a token that existed
    /// before it, its person a member of a queue, holds no queue right
    /// after it until one is granted.
    #[test]
    fn migration_74_leaves_every_existing_token_with_no_queue_access() {
        let (dir, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let q1 = queue(&st, None, "One");
        st.set_member(DAEMON, &q1, &sam, Some(Role::Triage))
            .unwrap();
        drop(st);
        let home = clax_core::Home::at(dir.path().join("ax"));
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.execute_batch(&format!(
                "DROP TABLE grant_elevations; PRAGMA user_version = {};",
                crate::migrations::UNTITLED_MIGRATION
            ))
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let version: u32 = st
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, crate::migrations::GRANT_RAISES_MIGRATION);
        assert!(st.grant_raises(&sam_g).unwrap().is_empty());
        assert!(
            !may(&st, &sam_g, "read", &q1),
            "an existing token starts at none"
        );
        // Its person grants it, and then it reads.
        let read = q(json!([{"type": "clax-queues", "actions": ["read"], "locations": [q1]}]));
        st.raise_grant(DAEMON, &sam_g, Some(&sam), Some(read), None, None)
            .unwrap();
        assert!(may(&st, &sam_g, "read", &q1));
    }

    fn may_request(st: &Store, gid: &str) -> bool {
        st.grant_caller(gid, None).unwrap().rights.allows(
            RightType::QueueRequests,
            "request",
            None,
            &|_| false,
        )
    }

    /// Every agent of a person may ask them for a queue, by its person's
    /// grant; a queue-request token holds that one right, from its
    /// approver's grant on it, and nothing of its person's, and takes no
    /// other grant.
    #[test]
    fn a_queue_request_token_holds_its_one_grant_and_nothing_else() {
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (ask_g, _, _) = approved(&st, "Mia", KEY_B);
        let mia = st.grant(&ask_g).unwrap().person.unwrap();
        assert!(may_request(&st, &sam_g));
        assert!(may_request(&st, &ask_g));
        st.with_tx(|tx| {
            issue_request_grant_in(
                &st,
                tx,
                DAEMON,
                &ask_g,
                Some(&mia),
                "2999-01-01T00:00:00.000Z",
            )
        })
        .unwrap();
        let rights = st.grant_caller(&ask_g, None).unwrap().rights;
        assert!(may_request(&st, &ask_g));
        assert!(!rights.allows_somewhere(RightType::Artifacts, "read"));
        assert!(!rights.allows_somewhere(RightType::Comments, "read"));
        assert_eq!(
            st.with_read(|c| request_grant_in(c, &ask_g))
                .unwrap()
                .as_deref(),
            Some("2999-01-01T00:00:00.000Z")
        );
        assert_eq!(st.with_read(|c| request_grant_in(c, &sam_g)).unwrap(), None);
        let q1 = queue(&st, None, "One");
        st.set_member(DAEMON, &q1, &mia, Some(Role::Triage))
            .unwrap();
        let read = q(json!([{"type": "clax-queues", "actions": ["read"], "locations": [q1]}]));
        assert!(matches!(
            st.raise_grant(DAEMON, &ask_g, Some(&mia), Some(read), None, None),
            Err(CoreError::NotFound)
        ));
        // Nobody grants `request` by hand: it comes with a person's grant.
        let ask = queue_request_access().unwrap();
        assert_eq!(
            code(st.raise_grant(DAEMON, &sam_g, None, Some(ask), None, None)),
            "invalid_rights"
        );
    }

    /// Migration 74 makes a token of migration 70's `queue.request` scope a
    /// queue-request token: its approver's grant to ask, ending when its
    /// scope did, and the scope columns cleared.
    #[test]
    fn migration_74_makes_a_scoped_token_a_queue_request_token() {
        let (dir, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        drop(st);
        let home = clax_core::Home::at(dir.path().join("ax"));
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.execute_batch(&format!(
                "DROP TABLE grant_elevations;
                 UPDATE grants SET scope = 'queue.request',
                   scope_expires_at = '2999-01-01T00:00:00.000Z', narrow_json = '[]'
                 WHERE id = '{sam_g}';
                 PRAGMA user_version = {};",
                crate::migrations::UNTITLED_MIGRATION
            ))
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let raises = st.grant_raises(&sam_g).unwrap();
        assert_eq!(raises.len(), 1);
        assert_eq!(raises[0].granter.as_deref(), Some(sam.as_str()));
        assert_eq!(raises[0].access, queue_request_access().unwrap());
        assert_eq!(
            st.with_read(|c| request_grant_in(c, &sam_g))
                .unwrap()
                .as_deref(),
            Some("2999-01-01T00:00:00.000Z")
        );
        let scope: Option<String> = st
            .with_read(|c| {
                Ok(
                    c.query_row("SELECT scope FROM grants WHERE id = ?1", [&sam_g], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(scope, None);
        assert!(may_request(&st, &sam_g));
        assert!(
            !st.grant_caller(&sam_g, None)
                .unwrap()
                .rights
                .allows_somewhere(RightType::Artifacts, "read")
        );
    }
}
