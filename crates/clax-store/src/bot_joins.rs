//! Joins (migration 65's `bot_joins`; spec
//! 2026-11-10-claxbot-surfaces-design, "A run", "A host's surface on a
//! daemon"): a surface's standing request that its operations be bound
//! into the runs another surface starts on a scope of its own.
//!
//! A join is one row for each (surface, joined surface, artifact): the
//! surface a host registered (`shell`, by its name), the surface whose
//! runs it joins (`clax`, Clax's own, whose runs are on an artifact), the
//! artifact, the registration that set it and that serves the runs it
//! binds, and the allow patterns that narrow the operations it binds
//! (`None` for a join that gives none, which binds the definition's reads
//! that do not wait alone: a write, an effect or a read that waits only
//! when a join's own patterns name it; `clax_bot::joins` decides).
//!
//! - **Replaced whole.** A surface's joins are set together
//!   ([`Store::replace_joins`]): what it had is removed and the new set
//!   written in one transaction, so a surface never has half of two sets.
//! - **Kept until replaced.** A join outlasts its host being away, and
//!   the registration that set it is not pruned while it holds one
//!   (`crate::bot_surfaces::prune_surfaces_in`). A registration that
//!   replaces another of its name ([`crate::bot_surfaces::NewRegistration::replaces`])
//!   takes the other's joins in the same transaction ([`repoint_joins_in`]).
//! - **Gone with the artifact.** Deleting an artifact removes its joins
//!   (a trigger of the migration on the soft delete, the foreign key on a
//!   row delete).
//!
//! The store keeps what it is given and checks what it can of the rows:
//! that the registration exists and is of the surface, and that each
//! artifact exists, is not deleted, and was made by the person the
//! surface's registrant acts for. Which operations a join binds into a
//! run, and whether its host is connected when the run is made, are its
//! caller's to decide when the run is made.

use super::Store;
use clax_core::time::stamp;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// The surface whose runs every join so far joins: Clax's own, whose
/// runs are on an artifact.
pub const JOINED_CLAX: &str = crate::bot_runs::CLAX_SURFACE;

/// One join as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Join {
    /// The joining surface's name (`shell`).
    pub surface: String,
    /// The surface whose runs it joins ([`JOINED_CLAX`]).
    pub joined: String,
    /// The artifact whose runs it joins.
    pub artifact_id: String,
    /// The registration that set it, which serves the runs it binds.
    pub registration_id: String,
    /// The allow patterns that narrow the operations it binds
    /// (`shell.windows`, `shell.window.*`), as given; `None` for a join
    /// that gives none, which binds the definition's reads that do not
    /// wait (`@read`, not `@waits`) alone.
    pub allow: Option<Vec<String>>,
    pub created_at: String,
}

/// A join to set ([`Store::replace_joins`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJoin {
    /// The surface whose runs it joins.
    pub joined: String,
    pub artifact_id: String,
    /// As [`Join::allow`].
    pub allow: Option<Vec<String>>,
}

/// The column of [`JOIN_COLS`] that holds a join's allow patterns.
const ALLOW_COL: usize = 4;

/// One row of [`JOIN_COLS`] as a join. Allow patterns kept as what is no
/// list of strings (a damaged row, or a later build's shape) are an
/// error, never read as no `allow`, which would name every read of the
/// definition: [`kept_joins`] leaves such a join out.
fn row_to_join(r: &rusqlite::Row<'_>) -> rusqlite::Result<Join> {
    let allow: Option<String> = r.get(ALLOW_COL)?;
    let allow = allow
        .map(|a| serde_json::from_str::<Vec<String>>(&a))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                ALLOW_COL,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?;
    Ok(Join {
        surface: r.get(0)?,
        joined: r.get(1)?,
        artifact_id: r.get(2)?,
        registration_id: r.get(3)?,
        allow,
        created_at: r.get(5)?,
    })
}

/// The joins `rows` read ([`row_to_join`]), less each whose allow
/// patterns do not read, which is logged with its surface and artifact:
/// such a join binds nothing and is listed nowhere.
fn kept_joins(
    rows: impl Iterator<Item = rusqlite::Result<(String, String, rusqlite::Result<Join>)>>,
) -> Result<Vec<Join>> {
    let mut kept = Vec::new();
    for row in rows {
        let (surface, artifact, join) = row?;
        match join {
            Ok(join) => kept.push(join),
            Err(rusqlite::Error::FromSqlConversionFailure(ALLOW_COL, _, e)) => {
                tracing::warn!(
                    "clax-store: the join of {surface} on {artifact} keeps allow patterns that                      do not read ({e}); it binds nothing"
                );
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(kept)
}

/// A row of [`JOIN_COLS`]: its surface and artifact, and the join.
fn row_of_join(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, rusqlite::Result<Join>)> {
    Ok((r.get(0)?, r.get(2)?, row_to_join(r)))
}

const JOIN_COLS: &str = "surface, joined, artifact_id, registration_id, ops_json, created_at";

/// Points every join that registration `from` set at registration `to`
/// instead, on `tx`: what a registration that takes another's place
/// does, so the joins outlast the one that is replaced. How many.
///
/// # Errors
/// The store's.
pub(crate) fn repoint_joins_in(tx: &Connection, from: &str, to: &str) -> Result<usize> {
    Ok(tx.execute(
        "UPDATE bot_joins SET registration_id = ?2 WHERE registration_id = ?1",
        params![from, to],
    )?)
}

/// The code of a join refused for an artifact that does not exist or is
/// deleted.
pub const CODE_JOIN_ARTIFACT_UNKNOWN: &str = "join_artifact_unknown";
/// The code of a join refused for an artifact that the person the
/// surface's registrant acts for did not make.
pub const CODE_JOIN_ARTIFACT_REFUSED: &str = "join_artifact_refused";
/// The code of a set of joins that is not one: two joins of one
/// artifact, an empty name.
pub const CODE_JOIN_INVALID: &str = "join_invalid";

impl Store {
    /// Replaces every join of the surface `surface` with `joins`, set by
    /// registration `registration_id`, at `now`, in one transaction:
    /// the joins it had are removed, whichever registration of its name
    /// set them, and `joins` written. An empty `joins`
    /// leaves it none. Returns the joins it has now, by joined surface and
    /// then artifact.
    ///
    /// `maker` is the person whose artifacts the surface may join, as
    /// [`Store::artifact_created_by`] names one (`None` for the owner):
    /// each artifact must have been made by them.
    ///
    /// # Errors
    /// Nothing is changed when any is refused:
    /// - `NotFound` for no registration `registration_id`;
    /// - [`CODE_JOIN_INVALID`] for a registration of another surface, an
    ///   empty surface or joined surface name, or two joins of one
    ///   artifact for one joined surface;
    /// - [`CODE_JOIN_ARTIFACT_UNKNOWN`] for an artifact that does not
    ///   exist or is deleted, naming it;
    /// - [`CODE_JOIN_ARTIFACT_REFUSED`] for an artifact `maker` did not
    ///   make, naming it;
    /// - the store's.
    pub fn replace_joins(
        &self,
        surface: &str,
        registration_id: &str,
        maker: Option<&str>,
        joins: &[NewJoin],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Join>> {
        let invalid = |m: String| CoreError::invalid(CODE_JOIN_INVALID, m);
        if surface.is_empty() || joins.iter().any(|j| j.joined.is_empty()) {
            return Err(invalid(
                "a join names its surface and the surface it joins".into(),
            ));
        }
        for (n, j) in joins.iter().enumerate() {
            if joins[..n]
                .iter()
                .any(|o| o.joined == j.joined && o.artifact_id == j.artifact_id)
            {
                return Err(invalid(format!(
                    "the artifact {} is joined twice for the surface {}",
                    j.artifact_id, j.joined
                )));
            }
        }
        let at = stamp(now);
        self.with_tx(|tx| {
            let reg = crate::bot_surfaces::registration_in(tx, registration_id)?
                .ok_or(CoreError::NotFound)?;
            if reg.name != surface {
                return Err(invalid(format!(
                    "registration {registration_id} is of the surface {}, not {surface}",
                    reg.name
                )));
            }
            for j in joins {
                let made_by: Option<Option<String>> = tx
                    .query_row(
                        "SELECT person FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                        params![j.artifact_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                match made_by {
                    None => {
                        return Err(CoreError::invalid(
                            CODE_JOIN_ARTIFACT_UNKNOWN,
                            format!("no artifact {} is here", j.artifact_id),
                        ));
                    }
                    Some(person) if person.as_deref() != maker => {
                        return Err(CoreError::invalid(
                            CODE_JOIN_ARTIFACT_REFUSED,
                            format!(
                                "the artifact {} is another person's: a surface joins the runs \
                                 of its registrant's own artifacts alone",
                                j.artifact_id
                            ),
                        ));
                    }
                    Some(_) => {}
                }
            }
            tx.execute("DELETE FROM bot_joins WHERE surface = ?1", params![surface])?;
            for j in joins {
                let allow = j
                    .allow
                    .as_ref()
                    .map(|a| serde_json::to_string(a).expect("serialisable patterns"));
                tx.execute(
                    "INSERT INTO bot_joins (surface, joined, artifact_id, registration_id,
                        ops_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![surface, j.joined, j.artifact_id, registration_id, allow, at],
                )?;
            }
            joins_where(tx, "surface = ?1", surface)
        })
    }

    /// The joins of the runs whose `joined` surface is scoped to artifact
    /// `artifact_id`, in the order their surfaces' names sort.
    ///
    /// # Errors
    /// The store's.
    pub fn joins_on(&self, artifact_id: &str, joined: &str) -> Result<Vec<Join>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {JOIN_COLS} FROM bot_joins WHERE artifact_id = ?1 AND joined = ?2
                  ORDER BY surface"
            ))?;
            let rows = q.query_map(params![artifact_id, joined], row_of_join)?;
            kept_joins(rows)
        })
    }

    /// The joins of the surface `surface`, by joined surface and then
    /// artifact.
    ///
    /// # Errors
    /// The store's.
    pub fn joins_of(&self, surface: &str) -> Result<Vec<Join>> {
        self.with_read(|c| joins_where(c, "surface = ?1", surface))
    }
}

/// The joins `filter` selects by its one argument, by joined surface and
/// then artifact.
fn joins_where(c: &Connection, filter: &str, arg: &str) -> Result<Vec<Join>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {JOIN_COLS} FROM bot_joins WHERE {filter} ORDER BY joined, artifact_id"
    ))?;
    let rows = q.query_map(params![arg], row_of_join)?;
    kept_joins(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_surfaces::NewRegistration;
    use crate::test_util::{DAEMON, artifact, store, store_with_clock};
    use clax_core::working::ManualClock;
    use serde_json::{Value, json};
    use std::sync::Arc;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn register(st: &Store, name: &str, replaces: Option<&str>) -> String {
        st.put_surface_def(HASH, "{}", st.now_utc()).unwrap();
        st.register_surface(
            DAEMON,
            NewRegistration {
                name: name.into(),
                def_hash: HASH.into(),
                prompt: None,
                settings: json!({}),
                registered_by: crate::roles::OWNER.into(),
                replaces: replaces.map(str::to_string),
            },
            st.now_utc(),
        )
        .unwrap()
        .id
    }

    fn join(aid: &str, allow: Option<&[&str]>) -> NewJoin {
        NewJoin {
            joined: JOINED_CLAX.into(),
            artifact_id: aid.into(),
            allow: allow.map(|a| a.iter().map(|p| (*p).to_string()).collect()),
        }
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    fn artifacts_of(joins: &[Join]) -> Vec<&str> {
        joins.iter().map(|j| j.artifact_id.as_str()).collect()
    }

    #[test]
    fn a_surfaces_joins_are_replaced_whole_and_listed_by_artifact() {
        let (_d, st) = store();
        let shell = register(&st, "shell", None);
        let mail = register(&st, "mail", None);
        let (a, b, c) = (
            artifact(&st, None),
            artifact(&st, None),
            artifact(&st, None),
        );
        let (a, b, c) = (a.as_str(), b.as_str(), c.as_str());
        let now = st.now_utc();
        let set = st
            .replace_joins(
                "shell",
                &shell,
                None,
                &[join(a, None), join(b, Some(&["shell.windows"]))],
                now,
            )
            .unwrap();
        let mut both = vec![a, b];
        both.sort_unstable();
        assert_eq!(artifacts_of(&set), both);
        let on_b = st.joins_on(b, JOINED_CLAX).unwrap();
        assert_eq!(
            on_b,
            [Join {
                surface: "shell".into(),
                joined: JOINED_CLAX.into(),
                artifact_id: b.into(),
                registration_id: shell.clone(),
                allow: Some(vec!["shell.windows".into()]),
                created_at: stamp(now),
            }]
        );
        st.replace_joins("mail", &mail, None, &[join(b, None)], now)
            .unwrap();
        let surfaces: Vec<String> = st
            .joins_on(b, JOINED_CLAX)
            .unwrap()
            .into_iter()
            .map(|j| j.surface)
            .collect();
        assert_eq!(surfaces, ["mail", "shell"]);
        assert!(st.joins_on(b, "other").unwrap().is_empty());
        // A second set replaces the first whole, and touches no other
        // surface's.
        let set = st
            .replace_joins("shell", &shell, None, &[join(c, None)], now)
            .unwrap();
        assert_eq!(artifacts_of(&set), [c]);
        assert!(st.joins_on(a, JOINED_CLAX).unwrap().is_empty());
        assert_eq!(st.joins_on(b, JOINED_CLAX).unwrap()[0].surface, "mail");
        // An empty set leaves none.
        assert!(
            st.replace_joins("shell", &shell, None, &[], now)
                .unwrap()
                .is_empty()
        );
        assert!(st.joins_of("shell").unwrap().is_empty());
        assert_eq!(st.joins_of("mail").unwrap().len(), 1);
    }

    #[test]
    fn a_set_with_one_join_refused_changes_nothing() {
        let (_d, st) = store();
        let shell = register(&st, "shell", None);
        let mail = register(&st, "mail", None);
        let (a, b) = (artifact(&st, None), artifact(&st, None));
        let (a, b) = (a.as_str(), b.as_str());
        let now = st.now_utc();
        st.replace_joins("shell", &shell, None, &[join(a, None)], now)
            .unwrap();
        let kept = st.joins_of("shell").unwrap();
        let theirs = artifact(&st, None);
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET person = 'p_mia' WHERE id = ?1",
                params![theirs.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let gone = artifact(&st, None);
        st.delete_artifact(DAEMON, &gone).unwrap();
        for (set, why) in [
            (
                vec![join(b, None), join("nope", None)],
                CODE_JOIN_ARTIFACT_UNKNOWN,
            ),
            (
                vec![join(b, None), join(gone.as_str(), None)],
                CODE_JOIN_ARTIFACT_UNKNOWN,
            ),
            (
                vec![join(b, None), join(theirs.as_str(), None)],
                CODE_JOIN_ARTIFACT_REFUSED,
            ),
            (vec![join(b, None), join(b, Some(&[]))], CODE_JOIN_INVALID),
        ] {
            assert_eq!(
                code(st.replace_joins("shell", &shell, None, &set, now)),
                why
            );
            assert_eq!(st.joins_of("shell").unwrap(), kept);
        }
        // The other person may join their own artifact, and not the owner's.
        assert!(
            st.replace_joins(
                "shell",
                &shell,
                Some("p_mia"),
                &[join(theirs.as_str(), None)],
                now
            )
            .is_ok()
        );
        assert_eq!(
            code(st.replace_joins("shell", &shell, Some("p_mia"), &[join(a, None)], now)),
            CODE_JOIN_ARTIFACT_REFUSED
        );
        // A registration of another surface, and one that is not there.
        assert_eq!(
            code(st.replace_joins("shell", &mail, None, &[join(a, None)], now)),
            CODE_JOIN_INVALID
        );
        assert!(matches!(
            st.replace_joins("shell", "nope", None, &[join(a, None)], now),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn deleting_an_artifact_removes_its_joins() {
        let (_d, st) = store();
        let shell = register(&st, "shell", None);
        let (a, b) = (artifact(&st, None), artifact(&st, None));
        st.replace_joins(
            "shell",
            &shell,
            None,
            &[join(a.as_str(), None), join(b.as_str(), None)],
            st.now_utc(),
        )
        .unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert!(st.joins_on(a.as_str(), JOINED_CLAX).unwrap().is_empty());
        assert_eq!(artifacts_of(&st.joins_of("shell").unwrap()), [b.as_str()]);
    }

    /// Joins are kept until they are replaced: a registration that takes
    /// another's place takes its joins, and neither the one replaced nor
    /// one that set joins and was not served for a week is pruned with
    /// them.
    #[test]
    fn joins_outlast_the_registration_that_set_them_being_replaced_and_pruned() {
        let clock = Arc::new(ManualClock::at("2026-11-11T09:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let a = register(&st, "shell", None);
        let page = artifact(&st, None);
        st.replace_joins(
            "shell",
            &a,
            None,
            &[join(page.as_str(), None)],
            st.now_utc(),
        )
        .unwrap();
        let b = register(&st, "shell", Some(&a));
        let on = || st.joins_on(page.as_str(), JOINED_CLAX).unwrap();
        assert_eq!(on()[0].registration_id, b);
        // A week and more on, the replaced registration is pruned, and the
        // one that holds the joins is not.
        clock.advance(8 * 24 * 3600);
        st.prune_ended_runs().unwrap();
        assert_eq!(st.registration(&a).unwrap(), None);
        assert!(st.registration(&b).unwrap().is_some());
        assert_eq!(on()[0].registration_id, b);
        // With its joins gone it is pruned too.
        st.replace_joins("shell", &b, None, &[], st.now_utc())
            .unwrap();
        st.prune_ended_runs().unwrap();
        assert_eq!(st.registration(&b).unwrap(), None);
    }

    /// A join whose kept allow patterns do not read as a list of strings
    /// is read as no join, never as one with no `allow`: it binds nothing,
    /// and the joins beside it are read as kept.
    #[test]
    fn a_join_whose_allow_does_not_read_is_no_join() {
        let (_d, st) = store();
        let shell = register(&st, "shell", None);
        let (a, b) = (artifact(&st, None), artifact(&st, None));
        st.replace_joins(
            "shell",
            &shell,
            None,
            &[
                join(a.as_str(), Some(&["shell.windows"])),
                join(b.as_str(), Some(&["shell.windows"])),
            ],
            st.now_utc(),
        )
        .unwrap();
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE bot_joins SET ops_json = '{\"v\": 2}' WHERE artifact_id = ?1",
                params![a.as_str()],
            )?)
        })
        .unwrap();
        assert_eq!(st.joins_on(a.as_str(), JOINED_CLAX).unwrap(), vec![]);
        let kept = st.joins_on(b.as_str(), JOINED_CLAX).unwrap();
        assert_eq!(kept[0].allow, Some(vec!["shell.windows".to_string()]));
        assert_eq!(artifacts_of(&st.joins_of("shell").unwrap()), [b.as_str()]);
    }

    /// A run of Clax's own kind made with a joined surface's binding keeps
    /// the definition that binding names from being pruned, as a host's
    /// own run does; its `clax` binding must be on its artifact.
    #[test]
    fn a_joined_runs_bindings_keep_their_definition() {
        use crate::bot_runs::{Bindings, NewRun, RequestKind, StoredBinding};
        use crate::credits::KeySource;
        let clock = Arc::new(ManualClock::at("2026-11-11T09:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let reg = register(&st, "shell", None);
        let page = artifact(&st, None).to_string();
        let new = || NewRun {
            artifact_id: page.clone(),
            person: crate::roles::OWNER.into(),
            request: RequestKind::Thread,
            thread_ids: vec!["t1".into()],
            model: "claude-sonnet-5-5".into(),
            key_source: KeySource::Instance,
            base_version: 1,
            agent_grant: None,
            web: String::new(),
        };
        let bound = |artifact: &str| {
            Bindings::bound(vec![
                StoredBinding::bound(
                    "clax",
                    None,
                    None,
                    None,
                    json!({"artifact": artifact}),
                    vec!["files.read".into()],
                ),
                StoredBinding::bound(
                    "shell",
                    Some(HASH.into()),
                    Some(reg.clone()),
                    None,
                    Value::Null,
                    vec!["shell.windows".into()],
                ),
            ])
            .unwrap()
        };
        assert_eq!(
            code(st.create_run_exclusive_bound(DAEMON, new(), &bound("elsewhere"))),
            "run_invalid"
        );
        assert_eq!(
            code(st.create_run_exclusive_bound(DAEMON, new(), &Bindings::clax(&page, ""))),
            "run_invalid"
        );
        let run = st
            .create_run_exclusive_bound(DAEMON, new(), &bound(&page))
            .unwrap()
            .expect("made");
        assert_eq!(run.artifact(), Some(page.as_str()));
        let (_, made, _) = st.run_with_steps_as_made(&run.id).unwrap().unwrap();
        assert_eq!(made.bindings().unwrap(), bound(&page));
        assert_eq!(made.kind, "clax.thread");
        // A second run of its thread is the first.
        let again = st
            .create_run_exclusive_bound(DAEMON, new(), &bound(&page))
            .unwrap();
        assert_eq!(again.unwrap_err().id, run.id);
        // Its registration ended and pruned, the definition is kept.
        st.replace_registration(&reg, st.now_utc()).unwrap();
        clock.advance(8 * 24 * 3600);
        st.prune_ended_runs().unwrap();
        assert!(st.surface_def(HASH).unwrap().is_some());
    }
}
