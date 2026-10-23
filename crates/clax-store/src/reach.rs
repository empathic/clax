//! Whose artifact is whose: the reach rule's one question. An artifact is
//! [`Reach::Within`] a person's reach when the owner made it (so everyone
//! reaches the owner's artifacts) or the person made it themselves,
//! [`Reach::Beyond`] it when another person made it (the owner, `person:
//! None`, reaches only what the owner made), and
//! [`Reach::Gone`] for anyone when it does not exist, is deleted, or was
//! made by a person who is revoked: a revoked maker's pages are taken down
//! until they are reinstated.

use super::Store;
use clax_core::rights::Rights;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Where an artifact lies relative to a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// No live artifact by that ID, or its maker is revoked.
    Gone,
    /// The owner's own, or the person's own.
    Within,
    /// Someone else's.
    Beyond,
}

impl Store {
    /// Where artifact `aid` lies relative to `person` (a viewer's public
    /// ID; `None` is the owner).
    pub fn reach(&self, aid: &str, person: Option<&str>) -> Result<Reach> {
        self.with_read(|c| {
            let row: Option<(Option<String>, bool)> = c
                .prepare_cached(
                    "SELECT a.person, m.person_revoked_at IS NOT NULL FROM artifacts a
                       LEFT JOIN viewers m ON m.public_id = a.person
                      WHERE a.id = ?1 AND a.deleted_at IS NULL",
                )?
                .query_row(params![aid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            Ok(match row {
                None | Some((_, true)) => Reach::Gone,
                Some((None, false)) => Reach::Within,
                Some((Some(maker), false)) if Some(maker.as_str()) == person => Reach::Within,
                Some((Some(_), false)) => Reach::Beyond,
            })
        })
    }

    /// Every artifact a person made, deleted ones included: all that may
    /// lie beyond the owner's reach.
    pub fn artifacts_made_by_people(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached("SELECT id FROM artifacts WHERE person IS NOT NULL")?;
            Ok(st
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?)
        })
    }

    /// The rights the shares of viewer `public_id` alone hold (the owner's
    /// public ID included), at live artifacts whose maker is not revoked;
    /// none when it has none. Not the stored rights [`Store::person_rights`]
    /// adds.
    ///
    /// # Errors
    /// `NotFound` for an unknown public ID.
    pub fn share_rights_of(&self, public_id: &str) -> Result<Rights> {
        self.with_read(|c| {
            let id: String = c
                .query_row(
                    "SELECT id FROM viewers WHERE public_id = ?1",
                    params![public_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            Ok(super::shares::share_rights(c, &id)?.unwrap_or_default())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GitHubUser;
    use crate::hosted::LinkAccess;
    use crate::shares::{ShareAccess, ShareTo};
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::rights::RightType;

    fn gh(id: i64, login: &str) -> GitHubUser {
        GitHubUser {
            id,
            login: login.into(),
            name: None,
            avatar_url: None,
        }
    }

    /// A hosted store with Mia and Sam, an artifact of the owner's and one
    /// of Mia's: `(dir, store, mia, sam, owners, mias)`.
    fn world() -> (tempfile::TempDir, Store, String, String, String, String) {
        let (d, st) = store();
        st.hosted_setup(DAEMON, "Ada").unwrap();
        let mia = st
            .github_sign_in(DAEMON, &gh(7, "mia-k"), false)
            .unwrap()
            .0
            .who
            .viewer
            .public_id;
        let sam = st
            .github_sign_in(DAEMON, &gh(8, "sam-lee"), false)
            .unwrap()
            .0
            .who
            .viewer
            .public_id;
        let owners = artifact(&st, None);
        let (made, _) = st
            .fork_artifact(DAEMON, &owners, None, Some(&mia), None)
            .unwrap();
        (d, st, mia, sam, owners.to_string(), made.id)
    }

    fn reads(st: &Store, pid: &str, aid: &str) -> bool {
        st.share_rights_of(pid)
            .unwrap()
            .allows(RightType::Artifacts, "read", Some(aid), &|_| false)
    }

    #[test]
    fn reach_is_within_for_the_owners_and_ones_own_and_beyond_for_others() {
        let (_d, st, mia, sam, owners, mias) = world();
        assert_eq!(st.reach(&owners, None).unwrap(), Reach::Within);
        assert_eq!(st.reach(&owners, Some(&mia)).unwrap(), Reach::Within);
        assert_eq!(st.reach(&mias, Some(&mia)).unwrap(), Reach::Within);
        assert_eq!(st.reach(&mias, None).unwrap(), Reach::Beyond);
        assert_eq!(st.reach(&mias, Some(&sam)).unwrap(), Reach::Beyond);
        assert_eq!(st.reach("nope", None).unwrap(), Reach::Gone);
    }

    #[test]
    fn a_revoked_makers_artifacts_are_gone_to_everyone_until_reinstated() {
        let (_d, st, mia, sam, _owners, mias) = world();
        st.set_general_access(DAEMON, &mias, Some(LinkAccess::Viewer), Some(true))
            .unwrap();
        st.share_artifact(
            DAEMON,
            &mias,
            &ShareTo::Person(sam.clone()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        assert!(reads(&st, &sam, &mias));
        assert!(st.link_access(&mias).unwrap().is_some());
        assert!(
            st.public_artifacts()
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mias)
        );

        st.revoke_person(DAEMON, &mia).unwrap();
        assert_eq!(st.reach(&mias, Some(&mia)).unwrap(), Reach::Gone);
        assert_eq!(st.reach(&mias, None).unwrap(), Reach::Gone);
        assert_eq!(st.link_access(&mias).unwrap(), None);
        assert!(!reads(&st, &sam, &mias));
        assert!(
            st.public_artifacts()
                .unwrap()
                .iter()
                .all(|(i, _)| *i != mias)
        );
    }

    #[test]
    fn share_rights_of_holds_shares_alone() {
        let (_d, st, _mia, sam, owners, mias) = world();
        assert!(!reads(&st, &sam, &mias));
        st.share_artifact(
            DAEMON,
            &mias,
            &ShareTo::Person(sam.clone()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        assert!(reads(&st, &sam, &mias));
        assert!(!reads(&st, &sam, &owners));
    }
}
