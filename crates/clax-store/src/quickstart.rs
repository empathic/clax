//! The Clax Quickstart in each person's gallery (spec
//! 2026-10-26-quickstart-onboarding-design §9): seeded once per person
//! (migration 57's `quickstarts` row, written in version 1's transaction,
//! is both the flag and the record of what Clax wrote), and moved to each
//! bundled [`Release`] while the person has not touched it.
//!
//! Untouched means: the row has no `stopped_at`, its artifact is live, the
//! artifact's current and newest version are both the one Clax last wrote,
//! and the artifact has no comment thread of any status. Any other state
//! stops updates for good the first time the rule runs on it. Renames,
//! pins, tags and the page's documents are not versions, so they do not
//! stop updates, and an update leaves them as they are.

use super::Store;
use super::roles::person_key;
use clax_core::audit::AuditCtx;
use clax_core::model::{Artifact, Version};
use clax_core::publish::validate;
use clax_core::quickstart::Release;
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Transaction, params};

/// A person's `quickstarts` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quickstart {
    /// The person's public ID, or `*owner`.
    pub person: String,
    /// The artifact seeded; `None` when the gallery was not seeded (a home
    /// that already held artifacts).
    pub artifact_id: Option<ArtifactId>,
    /// The release Clax last wrote to it.
    pub release: Option<String>,
    /// The version Clax last wrote.
    pub version_n: Option<u32>,
    pub seeded_at: String,
    /// When updates stopped for good; `None` while they go on.
    pub stopped_at: Option<String>,
}

/// What [`Store::seed_quickstart`] did.
#[derive(Debug)]
pub enum Seeded {
    /// It made the person's quickstart: the pinned artifact and its version 1.
    Made {
        artifact: Artifact,
        version: Version,
    },
    /// The person has a row already (seeded, skipped or deleted): nothing
    /// was made.
    Already,
    /// The owner's home already held an artifact: the row is written with
    /// no artifact, so it is never seeded.
    Skipped,
}

/// What [`Store::update_quickstart`] did.
#[derive(Debug)]
pub enum Updated {
    /// It published the release as the artifact's next version.
    Made {
        artifact: Artifact,
        version: Version,
    },
    /// The quickstart already holds the release.
    Current,
    /// There is nothing to update: the person touched or deleted it (now
    /// or before), it was never made, or the person has no row.
    Stopped,
}

/// Where a row stands under the untouched rule.
enum Rule {
    /// The person has no row.
    NoRow,
    /// Updates have stopped (just now, or before).
    Stopped,
    /// Clax may write the next version.
    Untouched {
        artifact_id: ArtifactId,
        release: String,
        version_n: u32,
    },
}

impl Store {
    /// Seeds `release` into the gallery of `person` (`None`: the owner) as
    /// the artifact [`clax_core::quickstart::TITLE`], theirs, pinned, with
    /// comments on, through the publish checks
    /// ([`clax_core::publish::validate`]); its row is written in version
    /// 1's transaction under `ctx`. A person is seeded once: a second seed,
    /// or one racing it, makes nothing. The owner's gallery is seeded only
    /// in a home that has never held an artifact with a version (deleted
    /// ones and drafts count); otherwise the row is written with no
    /// artifact and [`Seeded::Skipped`] returned.
    ///
    /// # Errors
    /// The release's request ([`Release::request`]), the publish checks, or
    /// the create's.
    pub async fn seed_quickstart(
        &self,
        ctx: &AuditCtx,
        person: Option<&str>,
        release: &Release,
    ) -> Result<Seeded> {
        let key = person_key(person);
        let now = self.now();
        let decided = self.with_tx(|tx| {
            if row_exists(tx, &key)? {
                return Ok(Some(Seeded::Already));
            }
            if person.is_none() && held_an_artifact(tx)? {
                tx.execute(
                    "INSERT INTO quickstarts (person, seeded_at) VALUES (?1, ?2)",
                    params![key, now],
                )?;
                return Ok(Some(Seeded::Skipped));
            }
            Ok(None)
        })?;
        if let Some(decided) = decided {
            return Ok(decided);
        }
        let mut p = validate(release.request(&[], true)?)?;
        p.person = person.map(str::to_string);
        let id = ArtifactId::generate();
        let made = self
            .create_artifact_then_async(ctx, &id, p, None, |tx, n| {
                tx.execute(
                    "UPDATE artifacts SET pinned = 1 WHERE id = ?1",
                    params![id.as_str()],
                )?;
                let wrote = tx.execute(
                    "INSERT INTO quickstarts (person, artifact_id, release, version_n, seeded_at)
                     VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (person) DO NOTHING",
                    params![key, id.as_str(), release.id, n, now],
                )?;
                if wrote == 0 {
                    // Another seed wrote the row first: this artifact rolls back.
                    return Err(CoreError::Conflict { current: 0 });
                }
                Ok(())
            })
            .await;
        match made {
            Ok((artifact, version, ())) => Ok(Seeded::Made { artifact, version }),
            Err(CoreError::Conflict { .. }) => Ok(Seeded::Already),
            Err(e) => Err(e),
        }
    }

    /// Moves the quickstart of `person` (`None`: the owner) to `release`
    /// while it is untouched (the module's rule): its next version holds
    /// exactly the release's files, labelled `Quickstart <release>`, with
    /// the release's capabilities, keeping the title, pin and tags. A
    /// touched or deleted quickstart has updates stopped for good
    /// (`stopped_at`), and a thread made or version published while this
    /// one is written stops them too.
    ///
    /// # Errors
    /// The release's request, the publish checks or the publish's; a
    /// `Conflict` when another writer moved an untouched quickstart to
    /// another release meanwhile.
    pub async fn update_quickstart(
        &self,
        ctx: &AuditCtx,
        person: Option<&str>,
        release: &Release,
    ) -> Result<Updated> {
        let key = person_key(person);
        let (artifact_id, version_n) = match self.apply_rule(&key)? {
            Rule::NoRow | Rule::Stopped => return Ok(Updated::Stopped),
            Rule::Untouched { release: r, .. } if r == release.id => {
                return Ok(Updated::Current);
            }
            Rule::Untouched {
                artifact_id,
                version_n,
                ..
            } => (artifact_id, version_n),
        };
        let previous: Vec<String> = self
            .get_version(&artifact_id, version_n)?
            .map(|v| v.files.into_keys().collect())
            .unwrap_or_default();
        let mut p = validate(release.request(&previous, false)?)?;
        p.person = person.map(str::to_string);
        p.if_version = Some(version_n);
        let written = self
            .publish_version_then_async(ctx, &artifact_id, p, None, |tx, n| {
                let threads: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM threads WHERE artifact_id = ?1)",
                    params![artifact_id.as_str()],
                    |r| r.get(0),
                )?;
                let wrote = if threads {
                    0
                } else {
                    tx.execute(
                        "UPDATE quickstarts SET release = ?2, version_n = ?3
                         WHERE person = ?1 AND stopped_at IS NULL AND version_n = ?4",
                        params![key, release.id, n, version_n],
                    )?
                };
                if wrote == 0 {
                    return Err(CoreError::Conflict { current: version_n });
                }
                Ok(())
            })
            .await;
        match written {
            Ok((artifact, version, ())) => Ok(Updated::Made { artifact, version }),
            Err(e @ CoreError::Conflict { .. }) => match self.apply_rule(&key)? {
                Rule::NoRow | Rule::Stopped => Ok(Updated::Stopped),
                Rule::Untouched { release: r, .. } if r == release.id => Ok(Updated::Current),
                Rule::Untouched { .. } => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    /// The `quickstarts` row of `person` (`None`: the owner), if any.
    ///
    /// # Errors
    /// `Corrupt` when its `artifact_id` is not an artifact ID.
    pub fn quickstart_of(&self, person: Option<&str>) -> Result<Option<Quickstart>> {
        let key = person_key(person);
        let row = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id, release, version_n, seeded_at, stopped_at
                 FROM quickstarts WHERE person = ?1",
                params![key],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                    ))
                },
            )
            .optional()?)
        })?;
        let Some((artifact_id, release, version_n, seeded_at, stopped_at)) = row else {
            return Ok(None);
        };
        let artifact_id = artifact_id
            .map(|id| {
                ArtifactId::parse(&id).map_err(|_| CoreError::Corrupt {
                    artifact_id: id.clone(),
                    column: "quickstarts.artifact_id",
                    version: None,
                })
            })
            .transpose()?;
        Ok(Some(Quickstart {
            person: key,
            artifact_id,
            release,
            version_n,
            seeded_at,
            stopped_at,
        }))
    }

    /// Runs the untouched rule on the row of `key` in one write
    /// transaction, setting `stopped_at` when it stops updates.
    fn apply_rule(&self, key: &str) -> Result<Rule> {
        let now = self.now();
        self.with_tx(|tx| {
            let rule = rule_in(tx, key)?;
            if let Rule::Stopped = rule {
                tx.execute(
                    "UPDATE quickstarts SET stopped_at = ?2 WHERE person = ?1 AND stopped_at IS NULL",
                    params![key, now],
                )?;
            }
            Ok(rule)
        })
    }
}

/// Whether `key` has a `quickstarts` row.
fn row_exists(tx: &Transaction<'_>, key: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM quickstarts WHERE person = ?1)",
        params![key],
        |r| r.get(0),
    )?)
}

/// Whether the home has ever held an artifact with a version, deleted ones
/// and drafts included. A zero-version row is a create under way (another
/// seed's among them) or one a dead process left, never a page the owner had.
fn held_an_artifact(tx: &Transaction<'_>) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM artifacts WHERE current_version > 0)",
        [],
        |r| r.get(0),
    )?)
}

/// Where the row of `key` stands under the module's untouched rule.
fn rule_in(tx: &Transaction<'_>, key: &str) -> Result<Rule> {
    let row: Option<(Option<String>, Option<String>, Option<u32>, Option<String>)> = tx
        .query_row(
            "SELECT artifact_id, release, version_n, stopped_at FROM quickstarts WHERE person = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((artifact_id, release, version_n, stopped_at)) = row else {
        return Ok(Rule::NoRow);
    };
    let (None, Some(id), Some(release), Some(version_n)) =
        (stopped_at, artifact_id, release, version_n)
    else {
        return Ok(Rule::Stopped);
    };
    let untouched: Option<bool> = tx
        .query_row(
            "SELECT a.current_version = ?2
                AND (SELECT MAX(v.n) FROM versions v WHERE v.artifact_id = a.id) = ?2
                AND NOT EXISTS (SELECT 1 FROM threads t WHERE t.artifact_id = a.id)
             FROM artifacts a WHERE a.id = ?1 AND a.deleted_at IS NULL",
            params![id, version_n],
            |r| r.get(0),
        )
        .optional()?;
    match (untouched, ArtifactId::parse(&id)) {
        (Some(true), Ok(artifact_id)) => Ok(Rule::Untouched {
            artifact_id,
            release,
            version_n,
        }),
        _ => Ok(Rule::Stopped),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, anchor, store};
    use crate::{MetaPatch, NewThread};
    use clax_core::embedded::MemTree;
    use clax_core::quickstart::Release;
    use clax_core::{ArtifactId, publish::validate};

    /// The release of the three files `clax_testkit::quickstart_tree` builds
    /// with `mark`, and `extra` (paths under `quickstart/`).
    fn release_with(mark: &str, extra: &[(&str, &str)]) -> Release {
        let mut files = vec![
            (
                "quickstart/index.html".to_string(),
                format!("<!doctype html><title>Clax Quickstart</title><p>{mark}</p>"),
            ),
            (
                "quickstart/clax.json".to_string(),
                r#"{"capabilities":{"db":{}}}"#.to_string(),
            ),
            ("quickstart/style.css".to_string(), mark.to_string()),
        ];
        files.extend(extra.iter().map(|(p, c)| (p.to_string(), c.to_string())));
        Release::from_tree(&MemTree(
            files
                .into_iter()
                .map(|(p, c)| (p, c.into_bytes()))
                .collect(),
        ))
        .unwrap()
    }

    fn release(mark: &str) -> Release {
        release_with(mark, &[])
    }

    fn id_of(a: &clax_core::model::Artifact) -> ArtifactId {
        ArtifactId::parse(&a.id).unwrap()
    }

    fn person_of(st: &Store, id: &ArtifactId) -> Option<String> {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT person FROM artifacts WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    fn made(s: Seeded) -> (clax_core::model::Artifact, clax_core::model::Version) {
        match s {
            Seeded::Made { artifact, version } => (artifact, version),
            other => panic!("expected Made, got {other:?}"),
        }
    }

    async fn seeded(st: &Store, a: &Release) -> ArtifactId {
        let (artifact, _) = made(st.seed_quickstart(DAEMON, None, a).await.unwrap());
        id_of(&artifact)
    }

    fn thread_on(st: &Store, id: &ArtifactId) -> String {
        st.create_thread(
            DAEMON,
            id,
            NewThread {
                version_n: 1,
                anchor: anchor(),
                author_name: "Alex".into(),
                author_public_id: None,
                body: "This step could say more".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn files_of(v: &clax_core::model::Version) -> Vec<&str> {
        v.files.keys().map(String::as_str).collect()
    }

    #[tokio::test]
    async fn a_seed_makes_one_pinned_owner_artifact() {
        let (_d, st) = store();
        let a = release("a");
        let (artifact, version) = made(st.seed_quickstart(DAEMON, None, &a).await.unwrap());
        let listed = st.list_artifacts().unwrap();
        assert_eq!(listed.len(), 1);
        let id = id_of(&listed[0]);
        assert_eq!(listed[0].id, artifact.id);
        assert_eq!(listed[0].title, "Clax Quickstart");
        assert!(listed[0].pinned);
        assert!(artifact.pinned);
        assert_eq!(person_of(&st, &id), None);
        assert!(st.comments_on(&id).unwrap());
        assert_eq!(version.n, 1);
        assert_eq!(version.label, Some(format!("Quickstart {}", a.id)));
        assert_eq!(files_of(&version), ["clax.json", "index.html", "style.css"]);
        assert_eq!(
            st.version_capabilities(id.as_str(), 1).unwrap(),
            serde_json::json!({"db": {}})
        );
        let q = st.quickstart_of(None).unwrap().unwrap();
        assert_eq!(q.person, crate::roles::OWNER);
        assert_eq!(q.artifact_id.as_ref(), Some(&id));
        assert_eq!(q.release.as_deref(), Some(a.id.as_str()));
        assert_eq!(q.version_n, Some(1));
        assert_eq!(q.stopped_at, None);
    }

    #[tokio::test]
    async fn a_persons_seed_is_theirs() {
        let (_d, st) = store();
        let a = release("a");
        let (artifact, _) = made(st.seed_quickstart(DAEMON, Some("p7k2"), &a).await.unwrap());
        assert_eq!(person_of(&st, &id_of(&artifact)), Some("p7k2".to_string()));
        let q = st.quickstart_of(Some("p7k2")).unwrap().unwrap();
        assert_eq!(q.person, "p7k2");
        assert_eq!(q.artifact_id, Some(id_of(&artifact)));
        assert!(st.quickstart_of(None).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_second_seed_for_one_person_makes_nothing() {
        let (_d, st) = store();
        let a = release("a");
        made(st.seed_quickstart(DAEMON, None, &a).await.unwrap());
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &a).await.unwrap(),
            Seeded::Already
        ));
        assert_eq!(st.list_artifacts().unwrap().len(), 1);

        let (_d, st) = store();
        let (one, two) = tokio::join!(
            st.seed_quickstart(DAEMON, None, &a),
            st.seed_quickstart(DAEMON, None, &a)
        );
        let outcomes = [one.unwrap(), two.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|s| matches!(s, Seeded::Made { .. }))
                .count(),
            1,
            "{outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|s| matches!(s, Seeded::Already))
                .count(),
            1,
            "{outcomes:?}"
        );
        assert_eq!(st.list_artifacts().unwrap().len(), 1);

        // Another seed lands between this one's check and its version 1:
        // the row it wrote rolls this one's artifact back.
        let (_d, st) = store();
        let other = a.clone();
        st.at_point("artifact.create.insert", move |st| {
            made(
                st.block_on(st.seed_quickstart(DAEMON, None, &other))
                    .unwrap(),
            );
        });
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &a).await.unwrap(),
            Seeded::Already
        ));
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
        assert!(st.zero_version_artifacts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_home_that_held_an_artifact_is_not_seeded() {
        let (_d, st) = store();
        let a = release("a");
        let other = st.insert_artifact_for_test("Earlier page", "2026-10-01T00:00:00Z");
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &a).await.unwrap(),
            Seeded::Skipped
        ));
        let listed = st.list_artifacts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(id_of(&listed[0]), other);
        let q = st.quickstart_of(None).unwrap().unwrap();
        assert_eq!(q.artifact_id, None);
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &a).await.unwrap(),
            Seeded::Already
        ));
        assert_eq!(st.list_artifacts().unwrap().len(), 1);

        // A person's gallery is theirs alone: the owner's pages do not count.
        made(st.seed_quickstart(DAEMON, Some("p7k2"), &a).await.unwrap());
        assert_eq!(st.list_artifacts().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_deleted_artifact_counts_as_held() {
        let (_d, st) = store();
        let a = release("a");
        let other = st.insert_artifact_for_test("Earlier page", "2026-10-01T00:00:00Z");
        st.delete_artifact(DAEMON, &other).unwrap();
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &a).await.unwrap(),
            Seeded::Skipped
        ));
        assert!(st.list_artifacts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_untouched_quickstart_moves_to_a_new_release() {
        let (_d, st) = store();
        let a = release_with("a", &[("quickstart/old.js", "x")]);
        let b = release("b");
        let id = seeded(&st, &a).await;
        let Updated::Made { artifact, version } =
            st.update_quickstart(DAEMON, None, &b).await.unwrap()
        else {
            panic!("expected Made");
        };
        assert_eq!(artifact.current_version, 2);
        assert_eq!(version.n, 2);
        assert_eq!(version.label, Some(format!("Quickstart {}", b.id)));
        assert_eq!(files_of(&version), ["clax.json", "index.html", "style.css"]);
        assert_eq!(
            crate::test_util::content_file(&st, &id, 2, "style.css").as_deref(),
            Some(b"b".as_slice())
        );
        let q = st.quickstart_of(None).unwrap().unwrap();
        assert_eq!(q.release.as_deref(), Some(b.id.as_str()));
        assert_eq!(q.version_n, Some(2));
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &b).await.unwrap(),
            Updated::Current
        ));
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 2);
    }

    #[tokio::test]
    async fn one_thread_stops_updates_for_good() {
        let (_d, st) = store();
        let id = seeded(&st, &release("a")).await;
        let tid = thread_on(&st, &id);
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert!(
            st.quickstart_of(None)
                .unwrap()
                .unwrap()
                .stopped_at
                .is_some()
        );
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 1);

        st.resolve_thread(DAEMON, &tid, "Alex").unwrap();
        st.delete_thread(DAEMON, &tid).unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("c"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 1);
    }

    #[tokio::test]
    async fn a_version_by_anyone_else_stops_updates() {
        let (_d, st) = store();
        let id = seeded(&st, &release("a")).await;
        let mut req = release("mine").request(&[], false).unwrap();
        req.if_version = Some(1);
        st.publish_version(DAEMON, &id, validate(req).unwrap(), None)
            .unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 2);

        st.restore_version(DAEMON, &id, 1, 2, None).unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("c"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 3);
    }

    #[tokio::test]
    async fn a_restore_alone_stops_updates() {
        let (_d, st) = store();
        let id = seeded(&st, &release("a")).await;
        st.restore_version(DAEMON, &id, 1, 1, None).unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 2);
    }

    #[tokio::test]
    async fn a_deleted_quickstart_stops_and_is_never_seeded_again() {
        let (_d, st) = store();
        let id = seeded(&st, &release("a")).await;
        st.delete_artifact(DAEMON, &id).unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert!(
            st.quickstart_of(None)
                .unwrap()
                .unwrap()
                .stopped_at
                .is_some()
        );
        assert!(matches!(
            st.seed_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Seeded::Already
        ));
        assert!(st.list_artifacts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_update_keeps_the_persons_title_and_pin() {
        let (_d, st) = store();
        let id = seeded(&st, &release("a")).await;
        st.update_meta(
            DAEMON,
            &id,
            MetaPatch {
                title: Some("Mine".into()),
                ..Default::default()
            },
        )
        .unwrap();
        st.set_pinned(DAEMON, &id, false).unwrap();
        assert!(matches!(
            st.update_quickstart(DAEMON, None, &release("b"))
                .await
                .unwrap(),
            Updated::Made { .. }
        ));
        let a = st.get_artifact(&id).unwrap().unwrap();
        assert_eq!(a.current_version, 2);
        assert_eq!(a.title, "Mine");
        assert!(!a.pinned);
    }

    #[tokio::test]
    async fn a_person_never_seeded_has_nothing_to_update() {
        let (_d, st) = store();
        assert!(matches!(
            st.update_quickstart(DAEMON, Some("p7k2"), &release("b"))
                .await
                .unwrap(),
            Updated::Stopped
        ));
        assert!(st.quickstart_of(Some("p7k2")).unwrap().is_none());
        // A later seed still makes it.
        made(
            st.seed_quickstart(DAEMON, Some("p7k2"), &release("b"))
                .await
                .unwrap(),
        );
    }
}
