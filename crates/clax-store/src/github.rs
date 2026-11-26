//! GitHub sign-in on a hosted daemon: a GitHub account, by its numeric
//! user ID (stable across renames), signs a browser in as the viewer row
//! it is linked to (`github_identities`, migration 31).
//!
//! - **The owner**: the account the daemon names as the owner's (the
//!   server matches its login), or the account already linked to the
//!   owner's row, signs in as the owner. The first such sign-in links the
//!   account to the owner's row (made with setup when there is none yet)
//!   and names the owner after it; once linked, no other account signs in
//!   as the owner, whatever its login, so a login given up and taken by
//!   someone else does not inherit the instance.
//! - **A member**: any other account. Its first sign-in makes a connected
//!   person, named after the account (made unique by the people's naming
//!   rules), whose rights are [`member_rights`]; later sign-ins sign in as
//!   that person and refresh the account's login, name and avatar.
//! - **Blocked**: revoking the person ([`Store::revoke_person`]) blocks
//!   the account: it may not sign in, and does not sign up again, until
//!   the person is reinstated ([`Store::reinstate_person`]).
//!
//! A member's account is capped ([`MemberCaps`], set on the store with
//! [`Store::set_member_caps`]): the live artifacts they created. The owner
//! is never capped.

use super::Store;
use super::hosted::{SignInVia, SignedIn};
use clax_core::audit::AuditCtx;
use clax_core::rights::Rights;
use clax_core::{CoreError, Result, new_public_id, new_ulid};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

pub use clax_core::config::DEFAULT_MAX_ARTIFACTS;
/// What one GitHub member may keep: at most `max_artifacts` live artifacts
/// they created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberCaps {
    pub max_artifacts: u64,
}

impl Default for MemberCaps {
    fn default() -> Self {
        MemberCaps {
            max_artifacts: DEFAULT_MAX_ARTIFACTS,
        }
    }
}

/// A GitHub account as its `/user` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubUser {
    /// The numeric user ID: the account's stable key.
    pub id: i64,
    pub login: String,
    /// The profile's name, when set.
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

impl GitHubUser {
    /// The name a person made for this account starts from: the profile's
    /// name, else the login.
    fn display(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.login)
    }
}

/// The GitHub account a login named when access was given by that login
/// (a share, a queue invite): GitHub's own answer for it (`GET
/// /users/<login>`), which the server asks before the store keeps
/// anything. A pending grant binds to this account, by `id`, at its
/// sign-in, whatever its login is by then, and never to a later holder of
/// the login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubAccount {
    /// The numeric user ID.
    pub id: i64,
    /// The login as GitHub spells it (any case; an `@` is dropped).
    pub login: String,
}

/// The SQL condition a pending grant's row (with columns `github_id` and
/// `github_login`) meets to bind to the account whose ID is `?1` and
/// lowercased login `?2` at its sign-in: one given to the account's ID,
/// or one given by login alone before migration 77 (`github_id` NULL,
/// legacy) to the login it signs in with.
pub(super) const BINDS_TO: &str = "(github_id = ?1 OR (github_id IS NULL AND github_login = ?2))";

/// The GitHub account linked to a viewer row, as People shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GitHubLink {
    pub id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
    /// When the account was blocked, if it is.
    pub blocked_at: Option<String>,
}

/// Who a GitHub sign-in signed in as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubOutcome {
    /// The owner.
    Owner,
    /// The person the account was already linked to.
    Returning,
    /// A person made at this sign-in.
    New,
}

/// The rights a GitHub sign-up gets: create artifacts, and every action
/// on the artifacts they created (their comments and their db included),
/// and `clax-ask`, so their agents may ask them: a question reaches only
/// its asking agent's own person, as every agent's does. Nothing at
/// anyone else's artifact: what they see beyond their own is what a page's
/// link, or the owner, gives them.
pub fn member_rights() -> Rights {
    Rights::parse(&serde_json::json!([
        {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
        {"type": "clax-artifacts", "actions": ["read", "update", "delete"], "locations": ["own"]},
        {"type": "clax-comments", "actions": clax_core::rights::COMMENT_ACTIONS, "locations": ["own"]},
        {"type": "clax-db", "actions": ["read", "write"], "level": "admin", "locations": ["own"]},
        {"type": "clax-ask", "actions": clax_core::rights::ASK_ACTIONS},
    ]))
    .expect("the GitHub member rights are well formed")
}

fn blocked() -> CoreError {
    CoreError::invalid("github_blocked", "this GitHub account may not sign in here")
}

/// The account linked to `github_id`: its viewer row, whether that row is
/// the owner's, and whether it is blocked (or its person revoked).
fn linked(tx: &Transaction<'_>, github_id: i64) -> Result<Option<(String, bool, bool)>> {
    Ok(tx
        .query_row(
            "SELECT gi.viewer_id, COALESCE(v.owner, 0),
                    gi.blocked_at IS NOT NULL OR v.person_revoked_at IS NOT NULL
               FROM github_identities gi LEFT JOIN viewers v ON v.id = gi.viewer_id
              WHERE gi.github_id = ?1",
            params![github_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// Links `user` to viewer row `viewer_id` (moving the account's link, and
/// refreshing its login, name and avatar), noting the sign-in, and binds
/// the shares waiting for the account ([`BINDS_TO`]) to that row
/// ([`super::shares::bind_github_shares`]) and its queue invites to its
/// person, the owner's as the owner ([`Store::bind_queue_invites`],
/// recorded under `ctx`). Returns how many shares it bound.
fn link(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    user: &GitHubUser,
    viewer_id: &str,
) -> Result<usize> {
    let now = st.now();
    tx.execute(
        "INSERT INTO github_identities (github_id, viewer_id, login, name, avatar_url, created_at, signed_in_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT (github_id) DO UPDATE SET viewer_id = excluded.viewer_id,
             login = excluded.login, name = excluded.name, avatar_url = excluded.avatar_url,
             signed_in_at = excluded.signed_in_at",
        params![user.id, viewer_id, user.login, user.name, user.avatar_url, now],
    )?;
    let shares = super::shares::bind_github_shares(tx, user, viewer_id)?;
    let (owner, public_id): (bool, String) = tx.query_row(
        "SELECT owner, public_id FROM viewers WHERE id = ?1",
        params![viewer_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let member = if owner {
        super::queues::OWNER_MEMBER
    } else {
        public_id.as_str()
    };
    st.bind_queue_invites(tx, ctx, user, member)?;
    Ok(shares)
}

/// Makes a person for `user` in `tx`, with `rights`, named after the
/// account (the first of [`name_candidates`] free), and links the account
/// to it ([`link`]). Returns its viewer row's ID.
///
/// # Errors
/// `name_taken` when no name is free.
fn new_person(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    user: &GitHubUser,
    rights: &Rights,
) -> Result<String> {
    let mut name = None;
    for candidate in name_candidates(user) {
        match super::people::check_person_name(tx, &candidate, None) {
            Ok(n) => {
                name = Some(n);
                break;
            }
            Err(e) if matches!(code_of(&e), Some("name_taken" | "invalid_name")) => {}
            Err(e) => return Err(e),
        }
    }
    let Some(name) = name else {
        return Err(CoreError::invalid(
            "name_taken",
            format!("no name for {} is free here", user.login),
        ));
    };
    let id = new_ulid();
    tx.execute(
        "INSERT INTO viewers (id, public_id, display_name, created_at, person, rights_json)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)",
        params![
            id,
            new_public_id(),
            name,
            st.now(),
            super::people::write_rights(rights)
        ],
    )?;
    // A new person: not `*unconnected` (multi-person M14, M17).
    st.visibility_changed();
    link(st, tx, ctx, user, &id)?;
    Ok(id)
}

/// Whether a pending queue invite or artifact share would bind to GitHub
/// account `user` ([`BINDS_TO`]): what lets a new account sign up on a
/// local Clax.
fn invited(tx: &Transaction<'_>, user: &GitHubUser) -> Result<bool> {
    Ok(tx.query_row(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM queue_member_invites WHERE {BINDS_TO})
                 OR EXISTS (SELECT 1 FROM artifact_shares WHERE viewer_id IS NULL AND {BINDS_TO})"
        ),
        params![user.id, user.login.to_ascii_lowercase()],
        |r| r.get(0),
    )?)
}

/// The names a new person for `user` may take, in order: the profile's
/// name, the login, both, and the login numbered.
fn name_candidates(user: &GitHubUser) -> Vec<String> {
    let first = user.display().to_string();
    let mut out = vec![first.clone()];
    if first != user.login {
        out.push(user.login.clone());
        out.push(format!("{first} ({})", user.login));
    }
    out.extend((2..=9).map(|n| format!("{} {n}", user.login)));
    out
}

impl Store {
    /// Caps every GitHub member's account at `caps`; `None` (a local
    /// daemon's, and the default) caps no one.
    pub fn set_member_caps(&self, caps: Option<MemberCaps>) {
        *self.member_caps.write().unwrap_or_else(|e| e.into_inner()) = caps;
    }

    fn member_caps(&self) -> Option<MemberCaps> {
        *self.member_caps.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether GitHub account `github_id` is linked to a viewer row.
    pub fn github_known(&self, github_id: i64) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM github_identities WHERE github_id = ?1)",
                params![github_id],
                |r| r.get(0),
            )?)
        })
    }

    /// Signs a browser in as GitHub account `user` (see the module docs):
    /// as the owner when `owner_login` (the daemon names the account the
    /// owner's) or when the account is linked to the owner's row; else as
    /// its person, made now at its first sign-in. Records `session.start`
    /// with `how: github` and the account's login.
    ///
    /// # Errors
    /// `github_blocked` (and nothing is changed) for a blocked account or
    /// a revoked person's; `github_not_owner` when `owner_login` but
    /// another account is linked to the owner's row; `name_taken` when no
    /// name for a new person is free.
    pub fn github_sign_in(
        &self,
        ctx: &AuditCtx,
        user: &GitHubUser,
        owner_login: bool,
    ) -> Result<(SignedIn, GitHubOutcome)> {
        let extra = [("github_login", serde_json::json!(user.login))];
        let as_owner = owner_login
            || self.with_tx(|tx| Ok(linked(tx, user.id)?.is_some_and(|(_, owner, _)| owner)))?;
        if as_owner {
            if self.hosted_needs_setup()? {
                let signed = self
                    .hosted_setup_via(ctx, user.display(), SignInVia::GitHub, &extra)
                    .or_else(|e| match code_of(&e) {
                        Some("name_taken" | "invalid_name") => {
                            self.hosted_setup_via(ctx, &user.login, SignInVia::GitHub, &extra)
                        }
                        _ => Err(e),
                    })?;
                let id = signed.who.viewer.id.clone();
                self.with_tx(|tx| link(self, tx, ctx, user, &id).map(drop))?;
                return Ok((signed, GitHubOutcome::Owner));
            }
            // The owner's name follows the account's, when it is free.
            let _ = self.set_owner_name(user.display(), true);
            let owner = self.owner_viewer(true)?;
            let signed = self.with_tx(|tx| {
                let other: Option<i64> = tx
                    .query_row(
                        "SELECT github_id FROM github_identities WHERE viewer_id = ?1",
                        params![owner.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if other.is_some_and(|o| o != user.id) {
                    return Err(CoreError::invalid(
                        "github_not_owner",
                        "another GitHub account is this Clax's owner's",
                    ));
                }
                link(self, tx, ctx, user, &owner.id)?;
                self.signed_in(tx, ctx, &owner.id, SignInVia::GitHub, &extra)
            })?;
            return Ok((signed, GitHubOutcome::Owner));
        }
        self.with_tx(|tx| {
            if let Some((viewer_id, _, is_blocked)) = linked(tx, user.id)? {
                if is_blocked {
                    return Err(blocked());
                }
                if link(self, tx, ctx, user, &viewer_id)? > 0 {
                    // Shares given to the login before it signed in.
                    self.visibility_changed();
                }
                let signed = self.signed_in(tx, ctx, &viewer_id, SignInVia::GitHub, &extra)?;
                return Ok((signed, GitHubOutcome::Returning));
            }
            let id = new_person(self, tx, ctx, user, &member_rights())?;
            let signed = self.signed_in(tx, ctx, &id, SignInVia::GitHub, &extra)?;
            Ok((signed, GitHubOutcome::New))
        })
    }

    /// Signs a local Clax's browser in as GitHub account `user`'s person
    /// (`[github]`; the hosted sign-in is [`Store::github_sign_in`]):
    /// the person the account is linked to, its login, name and avatar
    /// refreshed, or, at its first sign-in, a person made now with no
    /// rights of their own ([`Rights::default`]: what they reach is what is
    /// shared with them and the queues they are members of), only when a
    /// pending queue invite or artifact share was given to the account
    /// (by its ID, or, for one given before migration 77, its login:
    /// [`BINDS_TO`]). Either way the shares and queue invites waiting for
    /// the account bind to that person, as on a hosted Clax. Never the owner: no login makes the
    /// account the owner's here. Returns the person's public ID, a new
    /// `clax_viewer` cookie naming them (kept as its hash) and how they
    /// were found.
    ///
    /// # Errors
    /// (nothing changed) `github_owner_account` for an account linked to
    /// the owner's row; `github_blocked` for a blocked account or a revoked
    /// person's; `github_not_invited` for a new account no pending invite or
    /// share binds to; `name_taken` when no name for a new person is free.
    pub fn github_sign_in_local(
        &self,
        ctx: &AuditCtx,
        user: &GitHubUser,
    ) -> Result<(String, clax_core::secret::Secret, GitHubOutcome)> {
        self.with_tx(|tx| {
            let (viewer_id, outcome) = match linked(tx, user.id)? {
                Some((_, true, _)) => {
                    return Err(CoreError::invalid(
                        "github_owner_account",
                        "this GitHub account is linked to the owner, who does not sign in with GitHub on a local Clax",
                    ));
                }
                Some((_, _, true)) => return Err(blocked()),
                Some((viewer_id, false, false)) => {
                    if link(self, tx, ctx, user, &viewer_id)? > 0 {
                        self.visibility_changed();
                    }
                    (viewer_id, GitHubOutcome::Returning)
                }
                None => {
                    if !invited(tx, user)? {
                        return Err(CoreError::invalid(
                            "github_not_invited",
                            format!(
                                "no queue invite or share here names {}: only an added GitHub login signs up on a local Clax",
                                user.login
                            ),
                        ));
                    }
                    let id = new_person(self, tx, ctx, user, &Rights::default())?;
                    (id, GitHubOutcome::New)
                }
            };
            let public_id: String = tx.query_row(
                "SELECT public_id FROM viewers WHERE id = ?1",
                params![viewer_id],
                |r| r.get(0),
            )?;
            let cookie = super::viewers::issue_cookie_in(self, tx, &viewer_id)?;
            Ok((public_id, cookie, outcome))
        })
    }

    /// The GitHub account linked to each person, by their public ID.
    pub fn github_links(&self) -> Result<std::collections::HashMap<String, GitHubLink>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT v.public_id, gi.github_id, gi.login, gi.avatar_url, gi.blocked_at
                   FROM github_identities gi JOIN viewers v ON v.id = gi.viewer_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    GitHubLink {
                        id: r.get(1)?,
                        login: r.get(2)?,
                        avatar_url: r.get(3)?,
                        blocked_at: r.get(4)?,
                    },
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The GitHub account linked to the person whose public ID is
    /// `public_id` (the owner's included), as its last sign-in left it;
    /// `None` when none is.
    ///
    /// # Errors
    /// The database's.
    pub fn github_user_of(&self, public_id: &str) -> Result<Option<GitHubUser>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT gi.github_id, gi.login, gi.name, gi.avatar_url
                   FROM github_identities gi JOIN viewers v ON v.id = gi.viewer_id
                  WHERE v.public_id = ?1",
                params![public_id],
                |r| {
                    Ok(GitHubUser {
                        id: r.get(0)?,
                        login: r.get(1)?,
                        name: r.get(2)?,
                        avatar_url: r.get(3)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Refuses one more artifact for person `person` (a public ID) when it
    /// would take their account past its cap. The quickstart seeded for
    /// them is not counted. Only a GitHub member is capped, and only while the
    /// store has caps. A read before the work of making the artifact starts;
    /// the transaction that inserts it checks again ([`Store::check_quota_in`]).
    ///
    /// # Errors
    /// `quota_exceeded`, naming the cap.
    pub(super) fn check_quota(&self, person: Option<&str>) -> Result<()> {
        if self.member_caps().is_none() || person.is_none() {
            return Ok(());
        }
        self.with_read(|c| self.check_quota_in(c, person))
    }

    /// [`Store::check_quota`] on `c`: in the transaction that inserts the
    /// artifact, so the count and the insert are one write and concurrent
    /// creates never take an account past its cap.
    ///
    /// # Errors
    /// `quota_exceeded`, naming the cap.
    pub(super) fn check_quota_in(
        &self,
        c: &rusqlite::Connection,
        person: Option<&str>,
    ) -> Result<()> {
        let (Some(caps), Some(person)) = (self.member_caps(), person) else {
            return Ok(());
        };
        let capped: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM github_identities gi JOIN viewers v ON v.id = gi.viewer_id
                             WHERE v.public_id = ?1 AND v.owner = 0)",
            params![person],
            |r| r.get(0),
        )?;
        if !capped {
            return Ok(());
        }
        let made: i64 = c.query_row(
            "SELECT COUNT(*) FROM artifacts WHERE person = ?1 AND deleted_at IS NULL
               AND current_version > 0
               AND id NOT IN (SELECT artifact_id FROM quickstarts
                               WHERE person = ?1 AND artifact_id IS NOT NULL)",
            params![person],
            |r| r.get(0),
        )?;
        if made as u64 >= caps.max_artifacts {
            return Err(CoreError::invalid(
                "quota_exceeded",
                format!(
                    "an account here keeps at most {} artifacts: delete one first",
                    caps.max_artifacts
                ),
            ));
        }
        Ok(())
    }
}

/// An `Invalid` error's code.
fn code_of(e: &CoreError) -> Option<&'static str> {
    match e {
        CoreError::Invalid { code, .. } => Some(code),
        _ => None,
    }
}

/// Blocks the GitHub account of viewer row `viewer_id`, in `tx` (its
/// person is being revoked).
pub(super) fn block_in(tx: &Transaction<'_>, viewer_id: &str, now: &str) -> Result<()> {
    tx.execute(
        "UPDATE github_identities SET blocked_at = ?2 WHERE viewer_id = ?1 AND blocked_at IS NULL",
        params![viewer_id, now],
    )?;
    Ok(())
}

/// Unblocks the GitHub account of viewer row `viewer_id`, in `tx` (its
/// person is being reinstated).
pub(super) fn unblock_in(tx: &Transaction<'_>, viewer_id: &str) -> Result<()> {
    tx.execute(
        "UPDATE github_identities SET blocked_at = NULL WHERE viewer_id = ?1",
        params![viewer_id],
    )?;
    Ok(())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::publish::{PublishRequest, ValidatedPublish, validate};

    /// A hosted store whose members keep at most one artifact, and a
    /// member, Sam: their public ID.
    fn capped() -> (tempfile::TempDir, Store, String) {
        let (d, st) = store();
        st.hosted_setup(DAEMON, "Ada").unwrap();
        st.set_member_caps(Some(MemberCaps { max_artifacts: 1 }));
        let sam = GitHubUser {
            id: 7,
            login: "sam-lee".into(),
            name: None,
            avatar_url: None,
        };
        let (signed, _) = st.github_sign_in(DAEMON, &sam, false).unwrap();
        (d, st, signed.who.viewer.public_id)
    }

    /// A new artifact's publish, made by `person`.
    fn by(person: &str) -> ValidatedPublish {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Notes",
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let mut p = validate(req).unwrap();
        p.person = Some(person.to_string());
        p
    }

    fn live_made_by(st: &Store, person: &str) -> i64 {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM artifacts WHERE person = ?1 AND deleted_at IS NULL
               AND current_version > 0",
                params![person],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    fn quota_code<T: std::fmt::Debug>(r: Result<T>) -> String {
        match r {
            Err(CoreError::Invalid { code, .. }) => code.to_string(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn a_row_a_crash_left_without_a_version_counts_toward_no_quota_and_is_swept_once_old() {
        let (_d, st, sam) = capped();
        // Rows a crash left between an artifact's row and its first version:
        // one old, one made a moment ago (a create may still be under way).
        let old = clax_core::time::stamp(st.now_utc() - chrono::Duration::seconds(600));
        let young = st.now();
        st.with_write(|c| {
            for (id, at) in [("zzzzzzzzzzzz", &old), ("yyyyyyyyyyyy", &young)] {
                c.execute(
                    "INSERT INTO artifacts (id, title, created_at, updated_at, current_version,
                        contract_version, person) VALUES (?1, 'x', ?2, ?2, 0, '1', ?3)",
                    params![id, at, sam],
                )?;
            }
            Ok(())
        })
        .unwrap();
        // Neither takes Sam's one artifact.
        st.create_artifact(DAEMON, by(&sam), None).unwrap();
        assert_eq!(live_made_by(&st, &sam), 1);
        let swept = st
            .block_on(st.sweep_zero_version_artifacts(chrono::Duration::seconds(60)))
            .unwrap();
        assert_eq!(swept, 1);
        assert_eq!(st.zero_version_artifacts().unwrap(), ["yyyyyyyyyyyy"]);
    }

    #[test]
    fn a_create_racing_another_never_takes_a_member_past_the_cap() {
        let (_d, st, sam) = capped();
        let other = by(&sam);
        // Another create by Sam lands between this one's start and its insert.
        st.at_point("artifact.create.insert", move |st| {
            st.create_artifact(DAEMON, other, None).unwrap();
        });
        assert_eq!(
            quota_code(st.create_artifact(DAEMON, by(&sam), None)),
            "quota_exceeded"
        );
        assert_eq!(live_made_by(&st, &sam), 1);
    }

    #[test]
    fn a_fork_racing_a_create_never_takes_a_member_past_the_cap() {
        let (_d, st, sam) = capped();
        let source = artifact(&st, None);
        let other = by(&sam);
        st.at_point("artifact.fork.insert", move |st| {
            st.create_artifact(DAEMON, other, None).unwrap();
        });
        assert_eq!(
            quota_code(st.fork_artifact(DAEMON, &source, None, Some(&sam), None)),
            "quota_exceeded"
        );
        assert_eq!(live_made_by(&st, &sam), 1);
    }

    #[test]
    fn a_seeded_quickstart_is_made_past_a_zero_cap_and_counts_toward_none() {
        let (_d, st, sam) = capped();
        st.set_member_caps(Some(MemberCaps { max_artifacts: 0 }));
        let release = clax_core::quickstart::Release::from_tree(&clax_core::embedded::MemTree(
            [
                (
                    "quickstart/index.html".to_string(),
                    b"<!doctype html><title>Clax Quickstart</title><p>a</p>".to_vec(),
                ),
                (
                    "quickstart/clax.json".to_string(),
                    br#"{"capabilities":{"db":{}}}"#.to_vec(),
                ),
            ]
            .into_iter()
            .collect(),
        ))
        .unwrap();
        let seeded = st
            .block_on(st.seed_quickstart(DAEMON, Some(&sam), &release))
            .unwrap();
        assert!(matches!(seeded, crate::quickstart::Seeded::Made { .. }));
        assert_eq!(
            quota_code(st.create_artifact(DAEMON, by(&sam), None)),
            "quota_exceeded"
        );
        // At a cap of two, the member still makes two of their own.
        st.set_member_caps(Some(MemberCaps { max_artifacts: 2 }));
        st.create_artifact(DAEMON, by(&sam), None).unwrap();
        st.create_artifact(DAEMON, by(&sam), None).unwrap();
        assert_eq!(
            quota_code(st.create_artifact(DAEMON, by(&sam), None)),
            "quota_exceeded"
        );
        assert_eq!(live_made_by(&st, &sam), 3);
    }
}
