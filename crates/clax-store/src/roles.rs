//! Roles (migration 50, spec 2026-10-21-claxbot-design §5): who may run
//! the admin pages. The owner is always an admin and has no row; the
//! owner grants `admin` to a connected person and removes it
//! (`person_roles`). Everyone else is a member.
//!
//! People are named here, and in every ClaxBot table, by a **person key**:
//! a connected person's public ID, or [`OWNER`] (`*owner`) for the owner,
//! whatever the owner's public ID is now.
//!
//! The store checks only what a change is to (a live person, never the
//! owner's role); whether the caller may make it is the routes' to decide,
//! with [`Role::may_grant_admin`] and [`Role::is_admin`].

use super::Store;
use clax_core::audit::{Actor, AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub use clax_core::visibility::OWNER;

/// A person's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The instance owner: always an admin, never demoted.
    Owner,
    /// A person the owner made an admin.
    Admin,
    /// Everyone else.
    Member,
}

impl Role {
    /// The role's name: `owner`, `admin` or `member`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Whether the role may use the admin pages: set flags, credits and
    /// lifts, see all usage and audit, edit cost lines, the provider and
    /// the instance key.
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }

    /// Whether the role may grant or remove `admin`: the owner's alone.
    pub fn may_grant_admin(self) -> bool {
        self == Role::Owner
    }
}

/// 403-class `owner_only` unless `caller` may grant or remove `admin`.
///
/// # Errors
/// `owner_only` for every caller but the owner.
pub fn check_may_grant_admin(caller: Role) -> Result<()> {
    if caller.may_grant_admin() {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "owner_only",
            "only the owner grants or removes admin",
        ))
    }
}

/// The person key a public ID gives (`None`: the owner).
pub fn person_key(public_id: Option<&str>) -> String {
    public_id.unwrap_or(OWNER).to_string()
}

/// The person key of whoever `actor` acts as: the owner and the owner's
/// agents are [`OWNER`]; a viewer, a person's agent and ClaxBot acting
/// for someone are that person; Clax itself, a grant's client and an
/// anonymous viewer are none.
pub fn actor_person(actor: &Actor) -> Option<String> {
    match actor {
        Actor::Owner { .. } => Some(OWNER.into()),
        Actor::Viewer { public_id, .. } => Some(public_id.clone()),
        Actor::Agent(a) => Some(person_key(a.person.as_deref())),
        Actor::Bot { on_behalf_of } => Some(on_behalf_of.clone()),
        Actor::Client { .. } | Actor::Anonymous | Actor::System { .. } => None,
    }
}

/// Who `ctx` names in a `set_by` column: its person key, else `*system`.
pub(crate) fn set_by(ctx: &AuditCtx) -> String {
    actor_person(&ctx.actor).unwrap_or_else(|| "*system".into())
}

/// `NotFound` unless `person` is the owner or a connected person who is
/// not revoked, on `c`.
pub(crate) fn check_live_person(c: &Connection, person: &str) -> Result<()> {
    if person == OWNER {
        return Ok(());
    }
    let live: Option<i64> = c
        .prepare_cached(
            "SELECT 1 FROM viewers WHERE public_id = ?1 AND person = 1 AND owner = 0
               AND person_revoked_at IS NULL",
        )?
        .query_row(params![person], |r| r.get(0))
        .optional()?;
    live.map(|_| ()).ok_or(CoreError::NotFound)
}

/// `person`'s role on `c`: a revoked or unknown person is a member.
pub(crate) fn role_in(c: &Connection, person: &str) -> Result<Role> {
    if person == OWNER {
        return Ok(Role::Owner);
    }
    if check_live_person(c, person).is_err() {
        return Ok(Role::Member);
    }
    let admin: Option<String> = c
        .prepare_cached("SELECT role FROM person_roles WHERE person = ?1")?
        .query_row(params![person], |r| r.get(0))
        .optional()?;
    Ok(if admin.is_some() {
        Role::Admin
    } else {
        Role::Member
    })
}

impl Store {
    /// `person`'s role (a person key). A revoked or unknown person is a
    /// member, whatever row is left.
    ///
    /// # Errors
    /// The store's.
    pub fn role_of(&self, person: &str) -> Result<Role> {
        self.with_read(|c| role_in(c, person))
    }

    /// The people the owner made admins, by person key, in key order
    /// (revoked people left out). The owner is an admin too, and is not
    /// listed.
    ///
    /// # Errors
    /// The store's.
    pub fn admins(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT r.person FROM person_roles r JOIN viewers v ON v.public_id = r.person
                  WHERE v.person = 1 AND v.owner = 0 AND v.person_revoked_at IS NULL
                  ORDER BY r.person",
            )?;
            let rows = q.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
        })
    }

    /// Makes `person` an admin (`admin`) or a member (`!admin`), recorded
    /// as `admin.role {person, role: "admin", op: grant|remove}` under
    /// `ctx` when it changes anything. Returns whether it did. Only the
    /// owner may ask ([`check_may_grant_admin`]); the caller checks.
    ///
    /// # Errors
    /// `owner_role` for the owner, who is always an admin; `NotFound` for
    /// no live connected person; the store's.
    pub fn set_admin(&self, ctx: &AuditCtx, person: &str, admin: bool) -> Result<bool> {
        if person == OWNER {
            return Err(CoreError::invalid(
                "owner_role",
                "the owner is always an admin and cannot be demoted",
            ));
        }
        self.with_tx(|tx| {
            check_live_person(tx, person)?;
            let now = self.now();
            let changed = if admin {
                tx.execute(
                    "INSERT OR IGNORE INTO person_roles (person, role, set_by, set_at)
                     VALUES (?1, 'admin', ?2, ?3)",
                    params![person, set_by(ctx), now],
                )?
            } else {
                tx.execute(
                    "DELETE FROM person_roles WHERE person = ?1",
                    params![person],
                )?
            };
            if changed == 0 {
                return Ok(false);
            }
            let rec = AuditRecord::new(AuditKind::AdminRole, now)
                .with("person", person)
                .with("role", "admin")
                .with("op", if admin { "grant" } else { "remove" });
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }
}

/// One person as the admin pages list them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AdminPerson {
    /// The person key.
    pub person: String,
    pub name: Option<String>,
    pub role: Role,
    pub flags: super::flags::Flags,
    /// The key their runs use.
    pub key_source: super::credits::KeySource,
    pub week: super::credits::WeekUsage,
    /// Tokens of every kind this week.
    pub tokens_week: i64,
}

impl Store {
    /// The owner, then every connected person who is not revoked (oldest
    /// first), with their role, flags, key source and the credits of the
    /// week `at` falls in.
    ///
    /// # Errors
    /// The store's.
    pub fn people_for_admin(&self, at: chrono::DateTime<chrono::Utc>) -> Result<Vec<AdminPerson>> {
        let mut who: Vec<(String, Option<String>)> = vec![(
            OWNER.to_string(),
            self.owner()?.and_then(|v| v.display_name),
        )];
        for p in self.people()? {
            if p.viewer.person_revoked_at.is_none() {
                who.push((p.viewer.public_id, p.viewer.display_name));
            }
        }
        who.into_iter()
            .map(|(person, name)| {
                let tokens_week = self
                    .usage_rows(&person, at)?
                    .iter()
                    .filter(|r| r.unit.is_tokens())
                    .map(|r| r.qty)
                    .sum();
                Ok(AdminPerson {
                    role: self.role_of(&person)?,
                    flags: self.flags_of(&person)?,
                    key_source: self.key_source_of(&person)?,
                    week: self.week_usage(&person, at)?,
                    tokens_week,
                    name,
                    person,
                })
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::people::tests::approved;
    use crate::test_util::{DAEMON, store};

    /// A connected person named `name`; returns their public ID.
    pub(crate) fn person(st: &Store, name: &str, key: u8) -> String {
        let (gid, _, _) = approved(st, name, [key; 32]);
        st.viewer_of_grant(&gid).unwrap().public_id
    }

    fn owner_ctx() -> AuditCtx {
        AuditCtx {
            actor: Actor::Owner {
                public_id: "u_owner".into(),
            },
            ..DAEMON.clone()
        }
    }

    #[test]
    fn the_owner_grants_and_removes_admin_and_is_always_one() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        assert_eq!(st.role_of(OWNER).unwrap(), Role::Owner);
        assert_eq!(st.role_of(&sam).unwrap(), Role::Member);
        let ctx = owner_ctx();
        assert!(st.set_admin(&ctx, &sam, true).unwrap());
        assert!(!st.set_admin(&ctx, &sam, true).unwrap(), "no change");
        assert_eq!(st.role_of(&sam).unwrap(), Role::Admin);
        assert_eq!(st.admins().unwrap(), std::slice::from_ref(&sam));
        assert!(st.set_admin(&ctx, &sam, false).unwrap());
        assert_eq!(st.role_of(&sam).unwrap(), Role::Member);
        let events: Vec<String> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "admin.role")
            .map(|e| e.body)
            .collect();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("\"op\":\"grant\""), "{}", events[0]);
        assert!(events[1].contains("\"op\":\"remove\""), "{}", events[1]);

        let err = st.set_admin(&ctx, OWNER, false).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                code: "owner_role",
                ..
            }
        ));
        assert!(matches!(
            st.set_admin(&ctx, "u_nobody", true),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn the_admin_list_has_the_owner_first_and_each_live_person() {
        use crate::credits::{KeySource, Metered, Unit, token_line};
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let ana = person(&st, "Ana", 2);
        st.set_admin(&owner_ctx(), &sam, true).unwrap();
        st.revoke_person(DAEMON, &ana).unwrap();
        st.charge(
            &sam,
            None,
            &[Metered::new(
                token_line("claude-haiku-5-5", Unit::MtokOutput),
                1_200,
                KeySource::Instance,
            )],
        )
        .unwrap();
        let list = st.people_for_admin(st.now_utc()).unwrap();
        let keys: Vec<&str> = list.iter().map(|p| p.person.as_str()).collect();
        assert_eq!(keys, [OWNER, sam.as_str()]);
        assert_eq!(list[0].role, Role::Owner);
        assert!(list[0].flags.claxbot);
        assert_eq!(list[1].role, Role::Admin);
        assert_eq!(list[1].name.as_deref(), Some("Sam"));
        assert_eq!(list[1].tokens_week, 1_200);
        assert!(list[1].week.spent_milli > 0);
        assert_eq!(list[1].key_source, KeySource::Instance);
    }

    #[test]
    fn a_revoked_admin_is_a_member() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        st.set_admin(&owner_ctx(), &sam, true).unwrap();
        st.revoke_person(DAEMON, &sam).unwrap();
        assert_eq!(st.role_of(&sam).unwrap(), Role::Member);
        assert!(st.admins().unwrap().is_empty());
    }

    #[test]
    fn only_the_owner_may_grant_admin() {
        assert!(check_may_grant_admin(Role::Owner).is_ok());
        for r in [Role::Admin, Role::Member] {
            assert!(matches!(
                check_may_grant_admin(r),
                Err(CoreError::Invalid {
                    code: "owner_only",
                    ..
                })
            ));
        }
        assert!(Role::Owner.is_admin() && Role::Admin.is_admin() && !Role::Member.is_admin());
    }

    #[test]
    fn actors_name_their_person() {
        use clax_core::audit::{AgentActor, SystemReason};
        assert_eq!(
            actor_person(&Actor::Owner {
                public_id: "u_x".into()
            })
            .as_deref(),
            Some(OWNER)
        );
        assert_eq!(
            actor_person(&Actor::Agent(AgentActor::default())).as_deref(),
            Some(OWNER)
        );
        assert_eq!(
            actor_person(&Actor::Agent(AgentActor {
                person: Some("u_s".into()),
                ..Default::default()
            }))
            .as_deref(),
            Some("u_s")
        );
        assert_eq!(
            actor_person(&Actor::Bot {
                on_behalf_of: "u_s".into()
            })
            .as_deref(),
            Some("u_s")
        );
        assert_eq!(
            actor_person(&Actor::System {
                reason: SystemReason::Daemon
            }),
            None
        );
    }
}
