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
/// the shares waiting for the account's login to that row
/// ([`super::shares::bind_github_shares`]). Returns how many it bound.
fn link(st: &Store, tx: &Transaction<'_>, user: &GitHubUser, viewer_id: &str) -> Result<usize> {
    let now = st.now();
    tx.execute(
        "INSERT INTO github_identities (github_id, viewer_id, login, name, avatar_url, created_at, signed_in_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT (github_id) DO UPDATE SET viewer_id = excluded.viewer_id,
             login = excluded.login, name = excluded.name, avatar_url = excluded.avatar_url,
             signed_in_at = excluded.signed_in_at",
        params![user.id, viewer_id, user.login, user.name, user.avatar_url, now],
    )?;
    super::shares::bind_github_shares(tx, &user.login, viewer_id)
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
                self.with_tx(|tx| link(self, tx, user, &id).map(drop))?;
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
                link(self, tx, user, &owner.id)?;
                self.signed_in(tx, ctx, &owner.id, SignInVia::GitHub, &extra)
            })?;
            return Ok((signed, GitHubOutcome::Owner));
        }
        self.with_tx(|tx| {
            let now = self.now();
            if let Some((viewer_id, _, is_blocked)) = linked(tx, user.id)? {
                if is_blocked {
                    return Err(blocked());
                }
                if link(self, tx, user, &viewer_id)? > 0 {
                    // Shares given to the login before it signed in.
                    self.visibility_changed();
                }
                let signed = self.signed_in(tx, ctx, &viewer_id, SignInVia::GitHub, &extra)?;
                return Ok((signed, GitHubOutcome::Returning));
            }
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
                    now,
                    super::people::write_rights(&member_rights())
                ],
            )?;
            // A new person: not `*unconnected` (multi-person M14, M17).
            self.visibility_changed();
            link(self, tx, user, &id)?;
            let signed = self.signed_in(tx, ctx, &id, SignInVia::GitHub, &extra)?;
            Ok((signed, GitHubOutcome::New))
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

    /// Refuses one more artifact for person `person` (a public ID) when it
    /// would take their account past its cap. The quickstart seeded for
    /// them is not counted. Only a GitHub member is capped, and only while the store has caps. A read before the work
    /// of making the artifact starts; the transaction that inserts it
    /// checks again ([`Store::check_quota_in`]).
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
