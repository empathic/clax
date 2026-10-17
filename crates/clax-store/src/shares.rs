//! One artifact shared with one person (migration 32), and an artifact
//! shared by link listed in the public gallery.
//!
//! - **A share** gives one person [`ShareAccess`] at one artifact. It names
//!   the person by their viewer row, or by a GitHub login
//!   ([`ShareTo::GitHub`]). A login that a GitHub account already signed in
//!   with names that account's person at once; any other waits, and the
//!   account's first sign-in with that login binds it to the person made
//!   then ([`bind_github_shares`]). From then on the share follows the
//!   person, not the login, so a login given up and taken by someone else
//!   does not inherit it.
//! - **Resolution** is [`share_rights`]: the rights every share of a person
//!   holds, which the person's rights take in ([`Store::person_rights`]),
//!   so their browser, their listings and their agents all see them. It is
//!   the one place a person's per-artifact access is resolved.
//! - **General access** is the artifact's [`LinkAccess`] (anyone with the
//!   link), and `link_listed`: an artifact shared by link and listed is
//!   public, shown in [`Store::public_artifacts`].
//!
//! Who may share is the server's rule (the owner, and the artifact's
//! maker); the store records what it is told under the caller's audit
//! context.

use super::Store;
use super::hosted::LinkAccess;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::rights::Rights;
use crate::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};

/// Longest GitHub login, in characters (GitHub's own limit).
pub const MAX_GITHUB_LOGIN: usize = 39;
/// Most shares one artifact holds.
pub const MAX_SHARES_PER_ARTIFACT: usize = 200;
/// Most artifacts [`Store::public_artifacts`] lists.
pub const MAX_PUBLIC_LISTED: usize = 200;

/// What a share lets its person do at its artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareAccess {
    /// [`LinkAccess::Viewer`]'s rights.
    Viewer,
    /// [`LinkAccess::Commenter`]'s rights.
    Commenter,
    /// Read and publish new versions, every comment action (resolving
    /// included), and the page's data at the `interact` level: contents
    /// alone. Never delete, share, change who else has access, or change
    /// the pinned flag or capabilities (the server's admin rule).
    Editor,
}

impl ShareAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareAccess::Viewer => "viewer",
            ShareAccess::Commenter => "commenter",
            ShareAccess::Editor => "editor",
        }
    }

    /// `viewer`, `commenter` or `editor`; anything else is `None`.
    pub fn parse(s: &str) -> Option<ShareAccess> {
        match s {
            "viewer" => Some(ShareAccess::Viewer),
            "commenter" => Some(ShareAccess::Commenter),
            "editor" => Some(ShareAccess::Editor),
            _ => None,
        }
    }

    /// The access objects this access holds at `locations` (artifact IDs).
    fn objects(self, locations: &[String]) -> Vec<Value> {
        let comments: &[&str] = match self {
            ShareAccess::Viewer => &["read", "watch"],
            ShareAccess::Commenter => &["read", "watch", "write", "reply", "send"],
            ShareAccess::Editor => crate::rights::COMMENT_ACTIONS,
        };
        let artifacts: &[&str] = match self {
            ShareAccess::Editor => &["read", "update"],
            _ => &["read"],
        };
        let mut out = vec![
            json!({"type": "clax-artifacts", "actions": artifacts, "locations": locations}),
            json!({"type": "clax-comments", "actions": comments, "locations": locations}),
        ];
        if self == ShareAccess::Editor {
            out.push(json!({"type": "clax-db", "actions": ["read", "write"], "level": "interact", "locations": locations}));
        }
        out
    }

    /// The rights this access holds at artifact `aid` alone.
    ///
    /// # Errors
    /// `invalid_rights` when `aid` is not an artifact ID.
    pub fn rights(self, aid: &str) -> Result<Rights> {
        Rights::parse_internal(&Value::Array(self.objects(&[aid.to_string()])))
    }
}

/// Whom a share names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareTo {
    /// A GitHub login, checked and lowercased by [`github_login`].
    GitHub(String),
    /// A person, by public ID.
    Person(String),
}

/// One share, as the sharing dialog lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Share {
    pub id: String,
    pub artifact_id: String,
    pub access: ShareAccess,
    /// The login it was given to (lowercased), if it was given by login.
    pub github_login: Option<String>,
    /// The person's public ID once the share names one; `None` while it
    /// waits for its login's first sign-in.
    pub person: Option<String>,
    /// The person's name, once it names one.
    pub name: Option<String>,
    pub created_at: String,
    /// Who shared it (a public ID).
    pub created_by: Option<String>,
}

/// A person the sharing dialog may suggest: their public ID, their name and
/// their GitHub login, if they have one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ShareCandidate {
    pub person: String,
    pub name: Option<String>,
    pub github_login: Option<String>,
}

/// `s` as a GitHub login: an optional leading `@` dropped, then 1 to
/// [`MAX_GITHUB_LOGIN`] ASCII letters, digits and hyphens, not starting with
/// a hyphen; lowercased (GitHub's logins ignore case).
///
/// # Errors
/// `invalid_login` otherwise.
pub fn github_login(s: &str) -> Result<String> {
    let l = s.trim();
    let l = l.strip_prefix('@').unwrap_or(l);
    let ok = !l.is_empty()
        && l.len() <= MAX_GITHUB_LOGIN
        && !l.starts_with('-')
        && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(l.to_ascii_lowercase())
    } else {
        Err(CoreError::invalid(
            "invalid_login",
            "a GitHub login is 1 to 39 letters, digits or hyphens, not starting with a hyphen",
        ))
    }
}

/// The rights every share of viewer row `viewer_id` holds, at live
/// artifacts; `None` when it has none. The one place a person's
/// per-artifact access is resolved.
pub(super) fn share_rights(c: &Connection, viewer_id: &str) -> Result<Option<Rights>> {
    let mut stmt = c.prepare_cached(
        "SELECT s.access, s.artifact_id FROM artifact_shares s
           JOIN artifacts a ON a.id = s.artifact_id AND a.deleted_at IS NULL
          WHERE s.viewer_id = ?1",
    )?;
    let mut by: std::collections::BTreeMap<ShareAccess, Vec<String>> = Default::default();
    for row in stmt.query_map(params![viewer_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (access, aid) = row?;
        if let Some(a) = ShareAccess::parse(&access) {
            by.entry(a).or_default().push(aid);
        }
    }
    if by.is_empty() {
        return Ok(None);
    }
    let objects: Vec<Value> = by.iter().flat_map(|(a, ls)| a.objects(ls)).collect();
    Ok(Some(Rights::parse_internal(&Value::Array(objects))?))
}

/// Binds the shares waiting for GitHub login `login` to viewer row
/// `viewer_id`, at that account's sign-in. A waiting share of an artifact
/// the person already holds a share of is dropped when it would give less,
/// and replaces it otherwise.
pub(super) fn bind_github_shares(
    tx: &Transaction<'_>,
    login: &str,
    viewer_id: &str,
) -> Result<usize> {
    let login = login.to_ascii_lowercase();
    let waiting: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, artifact_id, access FROM artifact_shares
              WHERE viewer_id IS NULL AND github_login = ?1",
        )?;
        stmt.query_map(params![login], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, aid, access) in &waiting {
        let held: Option<(String, String)> = tx
            .query_row(
                "SELECT id, access FROM artifact_shares WHERE artifact_id = ?1 AND viewer_id = ?2",
                params![aid, viewer_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((held_id, held_access)) = held {
            let more = ShareAccess::parse(access) > ShareAccess::parse(&held_access);
            if more {
                tx.execute(
                    "UPDATE artifact_shares SET access = ?2 WHERE id = ?1",
                    params![held_id, access],
                )?;
            }
            tx.execute("DELETE FROM artifact_shares WHERE id = ?1", params![id])?;
        } else {
            tx.execute(
                "UPDATE artifact_shares SET viewer_id = ?2 WHERE id = ?1",
                params![id, viewer_id],
            )?;
        }
    }
    Ok(waiting.len())
}

const SHARE_SELECT: &str = "SELECT s.id, s.artifact_id, s.access, s.github_login, v.public_id,
        v.display_name, s.created_at, s.created_by
    FROM artifact_shares s LEFT JOIN viewers v ON v.id = s.viewer_id";

fn row_to_share(r: &rusqlite::Row<'_>) -> rusqlite::Result<Share> {
    let access: String = r.get(2)?;
    Ok(Share {
        id: r.get(0)?,
        artifact_id: r.get(1)?,
        access: ShareAccess::parse(&access).unwrap_or(ShareAccess::Viewer),
        github_login: r.get(3)?,
        person: r.get(4)?,
        name: r.get(5)?,
        created_at: r.get(6)?,
        created_by: r.get(7)?,
    })
}

fn live_artifact(tx: &Connection, aid: &str) -> Result<()> {
    let live: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL)",
        params![aid],
        |r| r.get(0),
    )?;
    if live {
        Ok(())
    } else {
        Err(CoreError::NotFound)
    }
}

fn audit_share(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    aid: &str,
    share: Value,
) -> Result<()> {
    let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, st.now())
        .with("fields", json!({ "share": share }));
    rec.ids.artifact = Some(aid.to_string());
    st.record_audit(tx, ctx, rec).map(drop)
}

impl Store {
    /// Artifact `aid`'s shares, oldest first.
    pub fn artifact_shares(&self, aid: &str) -> Result<Vec<Share>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "{SHARE_SELECT} WHERE s.artifact_id = ?1 ORDER BY s.created_at, s.id"
            ))?;
            Ok(stmt
                .query_map(params![aid], row_to_share)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Shares artifact `aid` with `to` at `access`, or changes the access of
    /// the share it already has, recording `artifact.update` of its
    /// `share` under `ctx`. `by` is the sharer's public ID. A login a
    /// GitHub account signed in with names that account's person now.
    ///
    /// # Errors
    /// `NotFound` for no such live artifact; `unknown_person` for a public
    /// ID that names no unrevoked person; `already_owner` for the owner,
    /// who has every right; `limit_reached` past
    /// [`MAX_SHARES_PER_ARTIFACT`].
    pub fn share_artifact(
        &self,
        ctx: &AuditCtx,
        aid: &str,
        to: &ShareTo,
        access: ShareAccess,
        by: Option<&str>,
    ) -> Result<Share> {
        self.with_tx(|tx| {
            live_artifact(tx, aid)?;
            let (viewer, login): (Option<String>, Option<String>) = match to {
                ShareTo::Person(pid) => {
                    let row: Option<(String, bool, bool, bool)> = tx
                        .query_row(
                            "SELECT id, owner, person, person_revoked_at IS NOT NULL
                               FROM viewers WHERE public_id = ?1",
                            params![pid],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                        )
                        .optional()?;
                    match row {
                        Some((_, true, _, _)) => return Err(already_owner()),
                        Some((id, false, true, false)) => (Some(id), None),
                        _ => {
                            return Err(CoreError::invalid(
                                "unknown_person",
                                "that names no person here",
                            ));
                        }
                    }
                }
                ShareTo::GitHub(login) => {
                    let login = github_login(login)?;
                    let known: Option<(String, bool)> = tx
                        .query_row(
                            "SELECT gi.viewer_id, COALESCE(v.owner, 0) FROM github_identities gi
                               LEFT JOIN viewers v ON v.id = gi.viewer_id
                              WHERE lower(gi.login) = ?1 ORDER BY gi.signed_in_at DESC LIMIT 1",
                            params![login],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    match known {
                        Some((_, true)) => return Err(already_owner()),
                        Some((id, false)) => (Some(id), Some(login)),
                        None => (None, Some(login)),
                    }
                }
            };
            let existing: Option<String> = match &viewer {
                Some(v) => tx
                    .query_row(
                        "SELECT id FROM artifact_shares WHERE artifact_id = ?1 AND viewer_id = ?2",
                        params![aid, v],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => tx
                    .query_row(
                        "SELECT id FROM artifact_shares
                          WHERE artifact_id = ?1 AND viewer_id IS NULL AND github_login = ?2",
                        params![aid, login],
                        |r| r.get(0),
                    )
                    .optional()?,
            };
            let id = match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE artifact_shares SET access = ?2,
                                github_login = COALESCE(?3, github_login) WHERE id = ?1",
                        params![id, access.as_str(), login],
                    )?;
                    id
                }
                None => {
                    let n: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM artifact_shares WHERE artifact_id = ?1",
                        params![aid],
                        |r| r.get(0),
                    )?;
                    if n as usize >= MAX_SHARES_PER_ARTIFACT {
                        return Err(CoreError::invalid(
                            "limit_reached",
                            format!("an artifact is shared with at most {MAX_SHARES_PER_ARTIFACT} people"),
                        ));
                    }
                    let id = new_ulid();
                    tx.execute(
                        "INSERT INTO artifact_shares (id, artifact_id, viewer_id, github_login, access,
                             created_at, created_by)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![id, aid, viewer, login, access.as_str(), self.now(), by],
                    )?;
                    id
                }
            };
            let share = tx.query_row(
                &format!("{SHARE_SELECT} WHERE s.id = ?1"),
                params![id],
                row_to_share,
            )?;
            audit_share(
                self,
                tx,
                ctx,
                aid,
                json!({"op": "set", "id": share.id, "person": share.person,
                       "github_login": share.github_login, "access": access.as_str()}),
            )?;
            self.visibility_changed();
            Ok(share)
        })
    }

    /// Removes share `share_id` of artifact `aid`, recording
    /// `artifact.update` of its `share` under `ctx`; returns it.
    ///
    /// # Errors
    /// `NotFound` when the artifact holds no such share.
    pub fn unshare_artifact(&self, ctx: &AuditCtx, aid: &str, share_id: &str) -> Result<Share> {
        self.with_tx(|tx| {
            let share = tx
                .query_row(
                    &format!("{SHARE_SELECT} WHERE s.id = ?1 AND s.artifact_id = ?2"),
                    params![share_id, aid],
                    row_to_share,
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            tx.execute(
                "DELETE FROM artifact_shares WHERE id = ?1",
                params![share_id],
            )?;
            audit_share(
                self,
                tx,
                ctx,
                aid,
                json!({"op": "remove", "id": share.id, "person": share.person,
                       "github_login": share.github_login}),
            )?;
            self.visibility_changed();
            Ok(share)
        })
    }

    /// Whether artifact `aid` is listed in the public gallery: shared by
    /// link, live and listed.
    pub fn link_listed(&self, aid: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL
                                 AND link_access IS NOT NULL AND link_listed = 1)",
                params![aid],
                |r| r.get(0),
            )?)
        })
    }

    /// Sets artifact `aid`'s general access: shared by link with `access`
    /// (`None`: not, which also unlists it), and, when `listed` is given,
    /// whether it is listed in the public gallery (`None` keeps it).
    /// Records `artifact.update` of its `link_access` and `link_listed`
    /// under `ctx`. Returns the access and listing now held.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist; `invalid_args` for
    /// listing an artifact not shared by link.
    pub fn set_general_access(
        &self,
        ctx: &AuditCtx,
        aid: &str,
        access: Option<LinkAccess>,
        listed: Option<bool>,
    ) -> Result<(Option<LinkAccess>, bool)> {
        if access.is_none() && listed == Some(true) {
            return Err(CoreError::invalid(
                "invalid_args",
                "only an artifact shared by link is listed in public",
            ));
        }
        self.with_tx(|tx| {
            let value = access.map(LinkAccess::as_str);
            let n = tx.execute(
                "UPDATE artifacts SET link_access = ?2,
                        link_listed = CASE WHEN ?2 IS NULL THEN 0 ELSE COALESCE(?3, link_listed) END
                  WHERE id = ?1 AND deleted_at IS NULL",
                params![aid, value, listed],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            let now_listed: bool = tx.query_row(
                "SELECT link_listed FROM artifacts WHERE id = ?1",
                params![aid],
                |r| r.get(0),
            )?;
            let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, self.now()).with(
                "fields",
                json!({ "link_access": value, "link_listed": now_listed }),
            );
            rec.ids.artifact = Some(aid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok((access, now_listed))
        })
    }

    /// The public gallery: live artifacts shared by link and listed, as
    /// `(ID, title)`, most recently updated first, at most
    /// [`MAX_PUBLIC_LISTED`].
    pub fn public_artifacts(&self) -> Result<Vec<(String, String)>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, title FROM artifacts
                  WHERE link_listed = 1 AND link_access IS NOT NULL AND deleted_at IS NULL
                  ORDER BY updated_at DESC, id LIMIT ?1",
            )?;
            Ok(stmt
                .query_map(params![MAX_PUBLIC_LISTED as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Every unrevoked person who is not the owner, with their GitHub
    /// login when they have one, by name: whom the owner's sharing dialog
    /// suggests.
    pub fn share_candidates(&self) -> Result<Vec<ShareCandidate>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT v.public_id, v.display_name, gi.login FROM viewers v
                   LEFT JOIN github_identities gi ON gi.viewer_id = v.id
                  WHERE v.person = 1 AND v.owner = 0 AND v.person_revoked_at IS NULL
                    AND gi.blocked_at IS NULL
                  ORDER BY lower(COALESCE(v.display_name, gi.login, '')), v.public_id",
            )?;
            Ok(stmt
                .query_map([], |r| {
                    Ok(ShareCandidate {
                        person: r.get(0)?,
                        name: r.get(1)?,
                        github_login: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?)
        })
    }
}

fn already_owner() -> CoreError {
    CoreError::invalid(
        "already_owner",
        "that is this Clax's owner, who can already do everything",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rights::RightType;
    use crate::store::github::GitHubUser;
    use crate::store::people::InvitePerson;
    use crate::store::test_util::{DAEMON, artifact, store};

    fn code_of<T: std::fmt::Debug>(r: Result<T>) -> String {
        match r {
            Err(CoreError::Invalid { code, .. }) => code.to_string(),
            Err(CoreError::NotFound) => "not_found".into(),
            other => format!("{other:?}"),
        }
    }

    fn gh(id: i64, login: &str) -> GitHubUser {
        GitHubUser {
            id,
            login: login.into(),
            name: None,
            avatar_url: None,
        }
    }

    /// A hosted store with its owner, and a GitHub member, Sam.
    fn hosted() -> (tempfile::TempDir, Store, String) {
        let (d, st) = store();
        st.hosted_setup(DAEMON, "Ada").unwrap();
        let (signed, _) = st.github_sign_in(DAEMON, &gh(7, "sam-lee"), false).unwrap();
        (d, st, signed.who.viewer.public_id)
    }

    fn may(st: &Store, pid: &str, kind: RightType, action: &str, aid: &str) -> bool {
        let own = |x: &str| st.artifact_created_by(x, Some(pid)).unwrap_or(false);
        st.person_rights(pid)
            .unwrap()
            .allows(kind, action, Some(aid), &own)
    }

    #[test]
    fn each_access_holds_its_rights_at_its_artifact_alone() {
        let (_d, st, sam) = hosted();
        let (a, other) = (artifact(&st, None), artifact(&st, None));
        let (a, other) = (a.as_str(), other.as_str());
        let r = |k, act| may(&st, &sam, k, act, a);
        assert!(!r(RightType::Artifacts, "read"), "nothing before a share");
        let to = ShareTo::Person(sam.clone());
        st.share_artifact(DAEMON, a, &to, ShareAccess::Viewer, None)
            .unwrap();
        assert!(r(RightType::Artifacts, "read"));
        assert!(r(RightType::Comments, "read"));
        assert!(
            !r(RightType::Comments, "write"),
            "a viewer does not comment"
        );
        assert!(!r(RightType::Artifacts, "update"));
        st.share_artifact(DAEMON, a, &to, ShareAccess::Commenter, None)
            .unwrap();
        assert!(r(RightType::Comments, "write") && r(RightType::Comments, "reply"));
        assert!(
            !r(RightType::Comments, "resolve"),
            "a commenter does not resolve"
        );
        assert!(
            !r(RightType::Artifacts, "update"),
            "a commenter does not publish"
        );
        assert!(!r(RightType::Db, "write"));
        st.share_artifact(DAEMON, a, &to, ShareAccess::Editor, None)
            .unwrap();
        assert!(r(RightType::Artifacts, "update"), "an editor publishes");
        assert!(r(RightType::Comments, "resolve"));
        assert!(
            !r(RightType::Artifacts, "delete"),
            "an editor does not delete"
        );
        let own = |_: &str| false;
        let rights = st.person_rights(&sam).unwrap();
        assert_eq!(
            rights.db_level(a, "write", &own),
            Some(crate::db::Level::Interact)
        );
        assert!(!may(&st, &sam, RightType::Artifacts, "read", other));
        assert_eq!(
            st.artifact_shares(a).unwrap().len(),
            1,
            "one share, changed"
        );
        // People shows the rights set on the person, not their shares.
        let row = st
            .people()
            .unwrap()
            .into_iter()
            .find(|p| p.viewer.public_id == sam);
        assert_eq!(row.unwrap().rights, crate::store::github::member_rights());
        let s = st.artifact_shares(a).unwrap().remove(0);
        st.unshare_artifact(DAEMON, a, &s.id).unwrap();
        assert!(
            !r(RightType::Artifacts, "read"),
            "removed, it holds nothing"
        );
        assert_eq!(code_of(st.unshare_artifact(DAEMON, a, &s.id)), "not_found");
    }

    #[test]
    fn a_login_share_waits_for_its_first_sign_in_then_follows_the_person() {
        let (_d, st, _sam) = hosted();
        let a = artifact(&st, None);
        let a = a.as_str();
        let s = st
            .share_artifact(
                DAEMON,
                a,
                &ShareTo::GitHub("@Octo-Cat".into()),
                ShareAccess::Commenter,
                Some("u_x"),
            )
            .unwrap();
        assert_eq!(s.github_login.as_deref(), Some("octo-cat"));
        assert_eq!(
            (s.person.as_deref(), s.created_by.as_deref()),
            (None, Some("u_x"))
        );
        let (signed, _) = st
            .github_sign_in(DAEMON, &gh(42, "octo-cat"), false)
            .unwrap();
        let octo = signed.who.viewer.public_id;
        assert!(may(&st, &octo, RightType::Comments, "write", a));
        let s = st.artifact_shares(a).unwrap().remove(0);
        assert_eq!(s.person.as_deref(), Some(octo.as_str()));
        // A login already signed in names its person at once; the same
        // person again changes the one share.
        let b = artifact(&st, None);
        let b = b.as_str();
        st.share_artifact(
            DAEMON,
            b,
            &ShareTo::GitHub("octo-cat".into()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        st.share_artifact(
            DAEMON,
            b,
            &ShareTo::Person(octo.clone()),
            ShareAccess::Editor,
            None,
        )
        .unwrap();
        assert_eq!(st.artifact_shares(b).unwrap().len(), 1);
        assert!(may(&st, &octo, RightType::Artifacts, "update", b));
        // The account renames; someone else takes the login and signs up:
        // they inherit nothing.
        st.github_sign_in(DAEMON, &gh(42, "octo-renamed"), false)
            .unwrap();
        let (other, _) = st
            .github_sign_in(DAEMON, &gh(99, "octo-cat"), false)
            .unwrap();
        let other = other.who.viewer.public_id;
        assert!(!may(&st, &other, RightType::Artifacts, "read", a));
        assert!(!may(&st, &other, RightType::Artifacts, "read", b));
        assert!(may(&st, &octo, RightType::Artifacts, "read", a));
    }

    #[test]
    fn a_pending_share_and_a_persons_share_merge_at_sign_in_keeping_the_more() {
        let (_d, st, _sam) = hosted();
        let a = artifact(&st, None);
        let a = a.as_str();
        let (_, code) = st
            .create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
            .unwrap();
        let mia = st
            .redeem_invite_in_browser(DAEMON, code.expose())
            .unwrap()
            .who
            .viewer;
        st.share_artifact(
            DAEMON,
            a,
            &ShareTo::Person(mia.public_id.clone()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        st.share_artifact(
            DAEMON,
            a,
            &ShareTo::GitHub("mia".into()),
            ShareAccess::Editor,
            None,
        )
        .unwrap();
        assert_eq!(st.artifact_shares(a).unwrap().len(), 2);
        st.with_tx(|tx| bind_github_shares(tx, "Mia", &mia.id))
            .unwrap();
        let shares = st.artifact_shares(a).unwrap();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].access, ShareAccess::Editor);
    }

    #[test]
    fn shares_refuse_the_owner_strangers_bad_logins_and_dead_artifacts() {
        let (_d, st, sam) = hosted();
        let a = artifact(&st, None);
        let a = a.as_str();
        let owner = st.owner().unwrap().unwrap().public_id;
        let share = |to: ShareTo| st.share_artifact(DAEMON, a, &to, ShareAccess::Viewer, None);
        assert_eq!(code_of(share(ShareTo::Person(owner))), "already_owner");
        assert_eq!(
            code_of(share(ShareTo::Person("u_nobody".into()))),
            "unknown_person"
        );
        for bad in ["", "@", "-lead", "has space", "a_b", "é", &"x".repeat(40)] {
            assert_eq!(
                code_of(share(ShareTo::GitHub(bad.into()))),
                "invalid_login",
                "{bad}"
            );
        }
        assert_eq!(github_login(&"x".repeat(39)).unwrap().len(), 39);
        assert_eq!(
            code_of(st.share_artifact(
                DAEMON,
                "7q3k9mzx2b4t",
                &ShareTo::Person(sam.clone()),
                ShareAccess::Viewer,
                None
            )),
            "not_found"
        );
        st.share_artifact(
            DAEMON,
            a,
            &ShareTo::Person(sam.clone()),
            ShareAccess::Editor,
            None,
        )
        .unwrap();
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET deleted_at = 'x' WHERE id = ?1",
                params![a],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            !may(&st, &sam, RightType::Artifacts, "read", a),
            "a deleted artifact's share holds nothing"
        );
    }

    #[test]
    fn an_agent_token_of_the_person_holds_their_shares() {
        let (_d, st, sam) = hosted();
        let a = artifact(&st, None);
        let sam_row = st.viewer_by_public_id(&sam).unwrap().unwrap();
        let (t, _) = st
            .mint_agent_token(DAEMON, &sam_row.id, "laptop", "203.0.113.9")
            .unwrap();
        let none = |_: &str| false;
        let reads = || {
            st.grant_caller(&t.id, None).unwrap().rights.allows(
                RightType::Artifacts,
                "update",
                Some(a.as_str()),
                &none,
            )
        };
        assert!(!reads());
        st.share_artifact(
            DAEMON,
            a.as_str(),
            &ShareTo::Person(sam.clone()),
            ShareAccess::Editor,
            None,
        )
        .unwrap();
        assert!(reads(), "the share reaches the person's agents");
    }

    #[test]
    fn the_public_gallery_lists_only_listed_pages_shared_by_link() {
        let (_d, st, _sam) = hosted();
        let (a, b, c) = (
            artifact(&st, None),
            artifact(&st, None),
            artifact(&st, None),
        );
        let (a, b, c) = (a.as_str(), b.as_str(), c.as_str());
        assert_eq!(
            code_of(st.set_general_access(DAEMON, a, None, Some(true))),
            "invalid_args"
        );
        assert_eq!(
            st.set_general_access(DAEMON, a, Some(LinkAccess::Viewer), Some(true))
                .unwrap(),
            (Some(LinkAccess::Viewer), true)
        );
        st.set_general_access(DAEMON, b, Some(LinkAccess::Commenter), None)
            .unwrap();
        st.share_artifact(
            DAEMON,
            c,
            &ShareTo::GitHub("someone".into()),
            ShareAccess::Editor,
            None,
        )
        .unwrap();
        let ids = |st: &Store| {
            st.public_artifacts()
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&st), vec![a.to_string()]);
        assert!(st.link_listed(a).unwrap() && !st.link_listed(b).unwrap());
        // Changing the link's access keeps the listing; stopping unlists.
        assert_eq!(
            st.set_general_access(DAEMON, a, Some(LinkAccess::Commenter), None)
                .unwrap(),
            (Some(LinkAccess::Commenter), true)
        );
        st.set_general_access(DAEMON, a, None, None).unwrap();
        assert!(ids(&st).is_empty());
        st.set_general_access(DAEMON, a, Some(LinkAccess::Viewer), None)
            .unwrap();
        assert!(ids(&st).is_empty(), "sharing by link again does not relist");
        // Stopping through the older link setter unlists too.
        st.set_general_access(DAEMON, b, Some(LinkAccess::Viewer), Some(true))
            .unwrap();
        st.set_link_access(DAEMON, b, None).unwrap();
        assert!(!st.link_listed(b).unwrap());
        st.set_link_access(DAEMON, b, Some(LinkAccess::Viewer))
            .unwrap();
        assert!(
            ids(&st).is_empty(),
            "a link stopped then given again is not listed"
        );
    }
}
