//! Agents' elevations (spec 2026-10-08-multi-agent-multi-person §7.2, §7.4,
//! §14, O11): one per (agent session, granter). The owner's may name any
//! location; an artifact creator's names artifacts they created, and is
//! read at every check as its access at the artifacts its granter created
//! that still exist, within the granter's own rights there
//! ([`crate::rights::creator_elevation`]). An elevation ends with its
//! session, when its granter is revoked as a person, and (for its
//! locations) when a creator's artifact is deleted; each end is recorded as
//! `agent.elevate` with the `reason`.

use super::Store;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::rights::{Rights, creator_elevation};
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// One granter's elevation of an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elevation {
    /// The granting person's public ID; `None` for the owner.
    pub granter: Option<String>,
    /// The access the granter gave, as given.
    pub access: Rights,
    pub created_at: String,
}

fn read_rights(json: &str) -> Result<Rights> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| CoreError::invalid("invalid_rights", "stored rights do not parse"))?;
    Rights::parse_internal(&v)
}

/// Session `sid`'s elevations on `c`, the owner's first, then oldest first.
pub(super) fn of_session(c: &Connection, sid: &str) -> Result<Vec<Elevation>> {
    let mut stmt = c.prepare_cached(
        "SELECT granter, access_json, created_at FROM session_elevations
         WHERE session_id = ?1 ORDER BY granter IS NOT NULL, created_at, granter",
    )?;
    let rows = stmt
        .query_map(params![sid], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(granter, json, created_at)| {
            Ok(Elevation {
                granter,
                access: read_rights(&json)?,
                created_at,
            })
        })
        .collect()
}

/// Whether `granter` (a public ID) created live artifact `aid`.
fn created_by(c: &Connection, aid: &str, granter: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND person = ?2 AND deleted_at IS NULL)",
        params![aid, granter],
        |r| r.get(0),
    )?)
}

/// The rights of granter `granter` (a public ID) while they are a
/// connected, unrevoked person; `None` once they are not.
fn granter_rights(c: &Connection, granter: &str) -> Result<Option<Rights>> {
    let row: Option<String> = c
        .query_row(
            "SELECT id FROM viewers WHERE public_id = ?1 AND person = 1 AND owner = 0
               AND person_revoked_at IS NULL",
            params![granter],
            |r| r.get(0),
        )
        .optional()?;
    row.map(|id| super::people::rights_of_row(c, &id))
        .transpose()
}

/// What elevation `e` gives now (spec §7.2): the owner's as given; a
/// creator's at the artifacts they created that still exist, within their
/// rights there, and nothing once they are revoked.
fn gives(c: &Connection, e: &Elevation) -> Result<Rights> {
    let Some(g) = &e.granter else {
        return Ok(e.access.clone());
    };
    let Some(rights) = granter_rights(c, g)? else {
        return Ok(Rights::default());
    };
    // A creator's elevation names artifact IDs only (checked when given);
    // any other location gives nothing.
    let mut created = std::collections::HashSet::new();
    for id in e.access.artifact_ids().unwrap_or_default() {
        if created_by(c, &id, g)? {
            created.insert(id);
        }
    }
    Ok(creator_elevation(&e.access, &rights, &|aid| {
        created.contains(aid)
    }))
}

/// The union of what session `sid`'s elevations give now; `None` when it
/// has none.
pub(super) fn given_in(c: &Connection, sid: &str) -> Result<Option<Rights>> {
    let all = of_session(c, sid)?;
    if all.is_empty() {
        return Ok(None);
    }
    let mut out = Rights::default();
    for e in &all {
        out = out.union(&gives(c, e)?);
    }
    Ok(Some(out))
}

/// What creator `granter` (`None`: the owner) may give at artifact `aid`:
/// every action there within their rights there; `None` when they did not
/// create it or it is deleted.
fn holds_in(c: &Connection, granter: Option<&str>, aid: &str) -> Result<Option<Rights>> {
    let everything = Rights::parse(&serde_json::json!([
        {"type": "clax-artifacts", "actions": ["read", "update", "delete"], "locations": [aid]},
        {"type": "clax-comments", "actions": crate::rights::COMMENT_ACTIONS, "locations": [aid]},
        {"type": "clax-db", "actions": ["read", "write"], "level": "admin", "locations": [aid]}
    ]))?;
    let Some(g) = granter else {
        let mine: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND person IS NULL AND deleted_at IS NULL)",
            params![aid],
            |r| r.get(0),
        )?;
        return Ok(mine.then_some(everything));
    };
    if !created_by(c, aid, g)? {
        return Ok(None);
    }
    let Some(rights) = granter_rights(c, g)? else {
        return Ok(None);
    };
    Ok(Some(creator_elevation(&everything, &rights, &|_| true)))
}

/// 403 `not_creator` (spec §7.4).
fn not_creator() -> CoreError {
    CoreError::invalid(
        "not_creator",
        "you raise agents only on artifacts you created: name each by its ID (not \"*\" or \"own\")",
    )
}

/// Checks a creator's elevation (spec §7.4): every location an artifact
/// `granter` created that is not deleted (`not_creator`), and every action
/// one they hold there (`beyond_your_rights`).
fn check_creator(c: &Connection, granter: &str, access: &Rights) -> Result<()> {
    let ids = access.artifact_ids().ok_or_else(not_creator)?;
    for id in &ids {
        if !created_by(c, id, granter)? {
            return Err(not_creator());
        }
    }
    let rights = granter_rights(c, granter)?.ok_or_else(not_creator)?;
    if !access.is_narrower_or_equal(&creator_elevation(access, &rights, &|_| true)) {
        return Err(CoreError::invalid(
            "beyond_your_rights",
            "a raise gives only what you may do on that artifact yourself",
        ));
    }
    Ok(())
}

/// Replaces (or with `None` deletes) `granter`'s elevation of `sid` on
/// `tx`; whether it changed.
fn put_in(
    tx: &Transaction<'_>,
    sid: &str,
    granter: Option<&str>,
    access: Option<&Rights>,
    now: &str,
) -> Result<bool> {
    let json = access.map(|a| a.to_json().to_string());
    let was: Option<String> = tx
        .query_row(
            "SELECT access_json FROM session_elevations WHERE session_id = ?1 AND granter IS ?2",
            params![sid, granter],
            |r| r.get(0),
        )
        .optional()?;
    if was == json {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM session_elevations WHERE session_id = ?1 AND granter IS ?2",
        params![sid, granter],
    )?;
    if let Some(j) = &json {
        tx.execute(
            "INSERT INTO session_elevations (session_id, granter, access_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![sid, granter, j, now],
        )?;
    }
    Ok(true)
}

/// `agent.elevate {session, access, granter}` (and `reason` for an end the
/// granter did not ask for).
fn record(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    (sid, granter): (&str, Option<&str>),
    access: Option<&Rights>,
    reason: Option<&str>,
) -> Result<()> {
    let now = st.now();
    let mut rec = AuditRecord::new(AuditKind::AgentElevate, &now)
        .with("session", sid)
        .with("access", access.map(Rights::to_json))
        .with("granter", granter);
    if let Some(r) = reason {
        rec = rec.with("reason", r);
    }
    rec.ids.session = Some(sid.to_string());
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Deletes every elevation `granter` gave (their revocation as a person),
/// recording each with `reason` `person_revoked`. Bumps the visibility
/// generation when there was any.
pub(super) fn drop_granted_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    granter: &str,
) -> Result<()> {
    let sids: Vec<String> = {
        let mut stmt =
            tx.prepare("DELETE FROM session_elevations WHERE granter = ?1 RETURNING session_id")?;
        stmt.query_map(params![granter], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for sid in &sids {
        record(
            st,
            tx,
            ctx,
            (sid, Some(granter)),
            None,
            Some("person_revoked"),
        )?;
    }
    if !sids.is_empty() {
        st.visibility_changed();
    }
    Ok(())
}

/// Removes deleted artifact `aid` from its creator's elevations (an
/// elevation left with no location is deleted), recording each change with
/// `reason` `artifact_deleted`. An owner's artifact has no creator
/// elevations; the owner's own elevations are left as they are.
pub(super) fn drop_artifact_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    aid: &str,
) -> Result<()> {
    let now = st.now();
    let creator: Option<String> = tx
        .query_row(
            "SELECT person FROM artifacts WHERE id = ?1",
            params![aid],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(creator) = creator else {
        return Ok(());
    };
    let rows: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare("SELECT session_id, access_json FROM session_elevations WHERE granter = ?1")?;
        stmt.query_map(params![creator], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut changed = false;
    for (sid, json) in rows {
        let access = read_rights(&json)?;
        let left = access.at_artifacts(&|x| x != aid);
        if left == access {
            continue;
        }
        let left = (!left.is_empty()).then_some(left);
        put_in(tx, &sid, Some(&creator), left.as_ref(), &now)?;
        record(
            st,
            tx,
            ctx,
            (&sid, Some(&creator)),
            left.as_ref(),
            Some("artifact_deleted"),
        )?;
        changed = true;
    }
    if changed {
        st.visibility_changed();
    }
    Ok(())
}

/// Deletes session `sid`'s elevations, as its end does; returns them for
/// the end to record.
pub(super) fn drop_session_in(tx: &Transaction<'_>, sid: &str) -> Result<Vec<Elevation>> {
    let all = of_session(tx, sid)?;
    tx.execute(
        "DELETE FROM session_elevations WHERE session_id = ?1",
        params![sid],
    )?;
    Ok(all)
}

/// A live agent `granter` raised, with the elevation they gave it.
#[derive(Clone, Debug)]
pub struct Raise {
    pub session: crate::model::Session,
    pub access: Rights,
}

impl Store {
    /// Sets (or with `None` removes) `granter`'s elevation of live agent
    /// `sid` of a connected machine (spec §7.4, O11): `granter` `None` is
    /// the owner, whose elevation may name any location; a person's names
    /// artifacts they created only, with actions they hold there. Recorded
    /// as `agent.elevate {session, access, granter}` under `ctx`; a change
    /// bumps the visibility generation (M17).
    ///
    /// # Errors
    /// `NotFound` for a session that is not live; `token_agent` for an
    /// agent of this machine; `not_creator` for a person's elevation at
    /// `*`, `own`, `clax-ask` or an artifact they did not create (or that
    /// is deleted); `beyond_your_rights` for one giving more than they may
    /// do there.
    pub fn elevate_agent(
        &self,
        ctx: &AuditCtx,
        sid: &str,
        granter: Option<&str>,
        access: Option<Rights>,
    ) -> Result<super::people::AgentRights> {
        self.with_tx(|tx| {
            super::people::live_grant_of(tx, sid)?;
            if let (Some(g), Some(a)) = (granter, &access) {
                check_creator(tx, g, a)?;
            }
            let now = self.now();
            let changed = put_in(tx, sid, granter, access.as_ref(), &now)?;
            record(self, tx, ctx, (sid, granter), access.as_ref(), None)?;
            if changed {
                self.visibility_changed();
            }
            super::people::agent_rights_in(tx, sid)?.ok_or(CoreError::NotFound)
        })
    }

    /// The live agents `granter` (`None`: the owner) raised, oldest raise
    /// first, each with the elevation they gave it.
    pub fn raises_by(&self, granter: Option<&str>) -> Result<Vec<Raise>> {
        self.with_read(|c| {
            let rows: Vec<(String, String)> = {
                let mut stmt = c.prepare(
                    "SELECT e.session_id, e.access_json FROM session_elevations e
                       JOIN sessions s ON s.id = e.session_id
                     WHERE e.granter IS ?1 AND s.ended_at IS NULL
                     ORDER BY e.created_at, e.session_id",
                )?;
                stmt.query_map(params![granter], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut out = Vec::with_capacity(rows.len());
            for (sid, json) in rows {
                let session = c.query_row(
                    &format!("{} WHERE id = ?1", super::sessions::SELECT),
                    params![sid],
                    super::sessions::row_to_session,
                )?;
                out.push(Raise {
                    session,
                    access: read_rights(&json)?,
                });
            }
            Ok(out)
        })
    }

    /// What `granter` (`None`: the owner) may give an agent at artifact
    /// `aid` (spec §7.4): every artifact, comment and db action there,
    /// within their rights there; `None` when they did not create it (the
    /// owner: when it is not live).
    pub fn creator_holds(&self, granter: Option<&str>, aid: &str) -> Result<Option<Rights>> {
        self.with_read(|c| holds_in(c, granter, aid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rights::{PRESET_READER, RightType};
    use crate::store::people::tests::{KEY_A, KEY_B, approved, code, store};
    use crate::store::test_util::{DAEMON, artifact};
    use crate::{CoreError, RegisterSession};
    use serde_json::json;

    fn reg(gid: &str) -> RegisterSession {
        RegisterSession {
            harness: "codex".into(),
            cwd: "/w".into(),
            grant_id: Some(gid.into()),
            ..Default::default()
        }
    }

    fn access(v: serde_json::Value) -> Rights {
        Rights::parse(&v).unwrap()
    }

    fn update_at(aid: &str) -> Rights {
        access(json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [aid]}]))
    }

    /// Whether grant `gid`'s session `sid` may update `aid` now.
    fn may_update(st: &Store, gid: &str, sid: &str, aid: &str) -> bool {
        st.grant_caller(gid, Some(sid)).unwrap().rights.allows(
            RightType::Artifacts,
            "update",
            Some(aid),
            &|_| false,
        )
    }

    /// Spec §7.2, §7.4, §14 (O11): Sam raises Mia's agent on the artifact he
    /// created, within his rights there; it reaches nothing else, stays
    /// apart from the owner's elevation, and ends when Sam is revoked or
    /// the artifact is deleted.
    #[test]
    fn a_creator_raises_another_persons_agent_on_their_artifact_only() {
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (mia_g, _, _) = approved(&st, "Mia", KEY_B);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let notes = artifact(&st, None);
        let qr = artifact(&st, None);
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET person = ?2 WHERE id = ?1",
                params![notes.as_str(), sam],
            )?;
            Ok(())
        })
        .unwrap();
        let agent = st.register_session(DAEMON, reg(&mia_g)).unwrap().id;
        let (notes, qr) = (notes.as_str(), qr.as_str());
        assert!(!may_update(&st, &mia_g, &agent, notes));
        // Not at `*`, `own`, the owner's artifact, or by `clax-ask`.
        let g0 = st.visibility_generation();
        for bad in [
            json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["*"]}]),
            json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["own"]}]),
            json!([{"type": "clax-artifacts", "actions": ["update"], "locations": [notes, qr]}]),
            json!([{"type": "clax-ask", "actions": ["ask"]}]),
        ] {
            assert_eq!(
                code(st.elevate_agent(DAEMON, &agent, Some(&sam), Some(access(bad.clone())))),
                "not_creator",
                "{bad}"
            );
        }
        assert_eq!(st.visibility_generation(), g0, "nothing changed");
        // At his own artifact: Mia's agent may publish there, nowhere else.
        let a = st
            .elevate_agent(DAEMON, &agent, Some(&sam), Some(update_at(notes)))
            .unwrap();
        assert_eq!(a.elevation_by(Some(&sam)), Some(&update_at(notes)));
        assert_eq!(a.elevation_by(None), None);
        assert!(st.visibility_generation() > g0);
        assert!(may_update(&st, &mia_g, &agent, notes));
        assert!(!may_update(&st, &mia_g, &agent, qr));
        // The owner's elevation is apart from Sam's: removing it leaves his.
        st.elevate_agent(DAEMON, &agent, None, Some(update_at(qr)))
            .unwrap();
        assert!(may_update(&st, &mia_g, &agent, qr));
        let a = st.elevate_agent(DAEMON, &agent, None, None).unwrap();
        assert_eq!(a.elevations.len(), 1);
        assert!(may_update(&st, &mia_g, &agent, notes));
        assert!(!may_update(&st, &mia_g, &agent, qr));
        assert_eq!(st.raises_by(Some(&sam)).unwrap().len(), 1);
        assert!(st.raises_by(None).unwrap().is_empty());
        // Sam made a reader: his raise gives nothing he may no longer do,
        // and he cannot give it again.
        let reader = Rights::preset(PRESET_READER).unwrap();
        st.set_person_rights(DAEMON, &sam, Some(reader)).unwrap();
        assert!(!may_update(&st, &mia_g, &agent, notes));
        assert_eq!(
            code(st.elevate_agent(DAEMON, &agent, Some(&sam), Some(update_at(notes)))),
            "beyond_your_rights"
        );
        st.set_person_rights(DAEMON, &sam, None).unwrap();
        assert!(may_update(&st, &mia_g, &agent, notes));
        // The artifact deleted: the raise goes with it.
        st.delete_artifact(DAEMON, &crate::ArtifactId::parse(notes).unwrap())
            .unwrap();
        assert!(
            st.agent_rights(&agent)
                .unwrap()
                .unwrap()
                .elevations
                .is_empty()
        );
        assert_eq!(
            code(st.elevate_agent(DAEMON, &agent, Some(&sam), Some(update_at(notes)))),
            "not_creator"
        );
    }

    #[test]
    fn revoking_the_creator_drops_every_raise_they_gave() {
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (mia_g, _, _) = approved(&st, "Mia", KEY_B);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let notes = artifact(&st, None);
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET person = ?2 WHERE id = ?1",
                params![notes.as_str(), sam],
            )?;
            Ok(())
        })
        .unwrap();
        let agent = st.register_session(DAEMON, reg(&mia_g)).unwrap().id;
        st.elevate_agent(DAEMON, &agent, Some(&sam), Some(update_at(notes.as_str())))
            .unwrap();
        let g0 = st.visibility_generation();
        st.revoke_person(DAEMON, &sam).unwrap();
        assert!(st.visibility_generation() > g0);
        let a = st.agent_rights(&agent).unwrap().unwrap();
        assert!(a.elevations.is_empty(), "dropped");
        assert!(!may_update(&st, &mia_g, &agent, notes.as_str()));
        assert!(matches!(
            st.elevate_agent(DAEMON, "nope", None, None),
            Err(CoreError::NotFound)
        ));
    }

    /// Spec §7.2: what a creator's elevation gives is decided at every
    /// check, not only when it is given: a stored raise naming an artifact
    /// its granter did not create, or by a granter since revoked, gives
    /// nothing, whatever wrote it.
    #[test]
    fn a_stored_raise_gives_only_what_its_granter_may_give_now() {
        let (_d, _c, st) = store();
        let (sam_g, _, _) = approved(&st, "Sam", KEY_A);
        let (mia_g, _, _) = approved(&st, "Mia", KEY_B);
        let sam = st.grant(&sam_g).unwrap().person.unwrap();
        let notes = artifact(&st, None);
        let qr = artifact(&st, None);
        let agent = st.register_session(DAEMON, reg(&mia_g)).unwrap().id;
        let raw = |sid: &str, json: String| {
            st.with_tx(|tx| {
                tx.execute(
                    "INSERT INTO session_elevations (session_id, granter, access_json, created_at)
                     VALUES (?1, ?2, ?3, 'x')",
                    params![sid, sam, json],
                )?;
                Ok(())
            })
            .unwrap();
        };
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET person = ?2 WHERE id = ?1",
                params![notes.as_str(), sam],
            )?;
            Ok(())
        })
        .unwrap();
        // Naming the owner's artifact beside his own: only his own counts.
        let both = access(json!([{"type": "clax-artifacts", "actions": ["update"],
            "locations": [notes.as_str(), qr.as_str()]}]));
        raw(&agent, both.to_json().to_string());
        assert!(may_update(&st, &mia_g, &agent, notes.as_str()));
        assert!(!may_update(&st, &mia_g, &agent, qr.as_str()));
        // Revoked, he gives nothing, even through a raise still stored.
        st.revoke_person(DAEMON, &sam).unwrap();
        raw(&agent, both.to_json().to_string());
        assert!(!may_update(&st, &mia_g, &agent, notes.as_str()));
    }
}
