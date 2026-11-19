//! ClaxBot's capability grants (migration 67, spec
//! 2026-11-19-claxbot-browser-design §4.3, §11.2).
//!
//! A run starts with no capability. Every capability it holds traces to
//! rows of `bot_cap_grants`, and nothing is implied:
//!
//! - **The admin's grants** say who holds a capability: an instance-wide
//!   grant to [`EVERYONE`], or one to a person (`person:<key>`). Their
//!   granter is the instance ([`GRANTER_INSTANCE`]) or, for the grants
//!   migration 67 wrote from main's web switches, [`GRANTER_MIGRATION`].
//! - **A person's grant to their own runs** (`runs_of:<key>`) is what
//!   reaches a run. It is the child of an admin grant the person holds,
//!   and its granter is the person. Issuing an admin grant issues it, in
//!   the same transaction, to every person the admin grant covers who has
//!   no live one; a person who joins later gets it when their row is made
//!   ([`Store::issue_joined_grants_in`]).
//! - **A revoke ends the grant and its live descendants**, transitively.
//!   A person revoking their own grant ends only theirs.
//!
//! Each row issued or revoked is recorded as `bot.grant {grant,
//! capability, granter, grantee, parent, op}`; migration 67's rows are
//! written by the migration alone, with `issued_by` `migration`.
//!
//! Whether the admin's switch offers a capability, and whether the host
//! can perform it, are not grants: the caller intersects them
//! (`clax_bot::capability::Capabilities::now`).

use super::Store;
use super::roles::{OWNER, set_by};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// Every capability a grant may name, in the order they are shown.
pub const CAPABILITIES: [&str; 5] = [
    "web.search",
    "web.fetch",
    "browser.open.url",
    "browser.open.artifact",
    "browser.open.content",
];

/// The retrieval capabilities.
pub const WEB_CAPABILITIES: [&str; 2] = ["web.search", "web.fetch"];

/// The running browser's capabilities, one per source kind.
pub const BROWSER_CAPABILITIES: [&str; 3] = [
    "browser.open.url",
    "browser.open.artifact",
    "browser.open.content",
];

/// The grantee of an instance-wide grant.
pub const EVERYONE: &str = "everyone";

/// The granter of a grant an admin issues.
pub const GRANTER_INSTANCE: &str = "instance";

/// The granter of a grant migration 67 wrote from main's web switches.
pub const GRANTER_MIGRATION: &str = "migration";

/// Who a grant is to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grantee {
    /// Every person of the instance, present and joining later.
    Everyone,
    /// One person, by person key.
    Person(String),
    /// A person's own runs, by the person's key.
    RunsOf(String),
}

impl Grantee {
    /// The grantee as `bot_cap_grants.grantee` keeps it: `everyone`,
    /// `person:<key>` or `runs_of:<key>`.
    pub fn as_text(&self) -> String {
        match self {
            Grantee::Everyone => EVERYONE.to_string(),
            Grantee::Person(k) => format!("person:{k}"),
            Grantee::RunsOf(k) => format!("runs_of:{k}"),
        }
    }

    /// The grantee `text` names.
    ///
    /// # Errors
    /// `invalid_grantee`.
    pub fn parse(text: &str) -> Result<Grantee> {
        let bad = || {
            CoreError::invalid(
                "invalid_grantee",
                format!("'{text}' is not everyone, person:<key> or runs_of:<key>"),
            )
        };
        if text == EVERYONE {
            return Ok(Grantee::Everyone);
        }
        let (kind, key) = text.split_once(':').ok_or_else(bad)?;
        if key.is_empty() {
            return Err(bad());
        }
        match kind {
            "person" => Ok(Grantee::Person(key.to_string())),
            "runs_of" => Ok(Grantee::RunsOf(key.to_string())),
            _ => Err(bad()),
        }
    }
}

/// One row of `bot_cap_grants`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GrantRow {
    pub id: String,
    pub capability: String,
    pub granter: String,
    /// `everyone`, `person:<key>` or `runs_of:<key>` ([`Grantee`]).
    pub grantee: String,
    pub parent: Option<String>,
    /// `issued` or `revoked`.
    pub status: String,
    pub issued_at: String,
    pub issued_by: String,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
}

impl GrantRow {
    /// Whether the grant is live (not revoked).
    pub fn live(&self) -> bool {
        self.status == "issued"
    }
}

/// Which grants [`Store::cap_grants`] lists.
#[derive(Clone, Debug, Default)]
pub struct GrantFilter {
    /// Only grants of this capability.
    pub capability: Option<String>,
    /// Only grants to this grantee (its text, [`Grantee::as_text`]).
    pub grantee: Option<String>,
    /// Only live grants.
    pub live: bool,
}

/// What a person holds of one capability ([`Store::person_caps`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PersonCap {
    /// The live admin grant that gives the person the capability (one to
    /// them, else the instance-wide one), if any.
    pub granted_by: Option<String>,
    /// The person's live grant of it to their own runs, if any.
    pub runs_grant: Option<String>,
}

impl PersonCap {
    /// Whether the capability reaches the person's runs.
    pub fn reaches_runs(&self) -> bool {
        self.granted_by.is_some() && self.runs_grant.is_some()
    }
}

const GRANT_COLS: &str = "id, capability, granter, grantee, parent, status, issued_at, issued_by,
    revoked_at, revoked_by";

fn row_to_grant(r: &Row<'_>) -> rusqlite::Result<GrantRow> {
    Ok(GrantRow {
        id: r.get(0)?,
        capability: r.get(1)?,
        granter: r.get(2)?,
        grantee: r.get(3)?,
        parent: r.get(4)?,
        status: r.get(5)?,
        issued_at: r.get(6)?,
        issued_by: r.get(7)?,
        revoked_at: r.get(8)?,
        revoked_by: r.get(9)?,
    })
}

fn check_capability(capability: &str) -> Result<()> {
    if CAPABILITIES.contains(&capability) {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "unknown_capability",
            format!("no capability is named '{capability}'"),
        ))
    }
}

fn grant_in(c: &Connection, id: &str) -> Result<Option<GrantRow>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {GRANT_COLS} FROM bot_cap_grants WHERE id = ?1"
    ))?
    .query_row(params![id], row_to_grant)
    .optional()?)
}

/// The live grant of `capability` to `grantee` on `c`, if any.
fn live_grant_to(c: &Connection, capability: &str, grantee: &str) -> Result<Option<GrantRow>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {GRANT_COLS} FROM bot_cap_grants
              WHERE grantee = ?1 AND capability = ?2 AND status = 'issued'
              ORDER BY issued_at, id LIMIT 1"
    ))?
    .query_row(params![grantee, capability], row_to_grant)
    .optional()?)
}

/// Every person key of the instance on `c`: the owner, then each
/// connected person, revoked ones included (a reinstated person is the
/// person they were), by public ID.
pub(crate) fn person_keys_in(c: &Connection) -> Result<Vec<String>> {
    let mut keys = vec![OWNER.to_string()];
    let mut q = c.prepare_cached(
        "SELECT public_id FROM viewers
          WHERE person = 1 AND owner = 0 AND public_id IS NOT NULL
          ORDER BY public_id",
    )?;
    let rows = q.query_map([], |r| r.get::<_, String>(0))?;
    for k in rows {
        keys.push(k?);
    }
    Ok(keys)
}

/// Inserts a grant row on `c` and returns it.
#[allow(clippy::too_many_arguments)]
fn insert_grant(
    c: &Connection,
    capability: &str,
    granter: &str,
    grantee: &str,
    parent: Option<&str>,
    issued_at: &str,
    issued_by: &str,
) -> Result<GrantRow> {
    let id = clax_core::new_ulid();
    c.execute(
        "INSERT INTO bot_cap_grants (id, capability, granter, grantee, parent, status, issued_at,
            issued_by)
         VALUES (?1, ?2, ?3, ?4, ?5, 'issued', ?6, ?7)",
        params![
            id, capability, granter, grantee, parent, issued_at, issued_by
        ],
    )?;
    Ok(GrantRow {
        id,
        capability: capability.to_string(),
        granter: granter.to_string(),
        grantee: grantee.to_string(),
        parent: parent.map(str::to_string),
        status: "issued".into(),
        issued_at: issued_at.to_string(),
        issued_by: issued_by.to_string(),
        revoked_at: None,
        revoked_by: None,
    })
}

/// The `bot.grant` record of `g` for `op` (`issue` or `revoke`).
fn grant_record(g: &GrantRow, op: &str, at: String) -> AuditRecord {
    let rec = AuditRecord::new(AuditKind::BotGrant, at)
        .with("grant", g.id.as_str())
        .with("capability", g.capability.as_str())
        .with("granter", g.granter.as_str())
        .with("grantee", g.grantee.as_str())
        .with("op", op);
    match &g.parent {
        Some(p) => rec.with("parent", p.as_str()),
        None => rec,
    }
}

/// Migration 67's Rust step, on `tx`: main's web switches as explicit
/// grants, and each run's capabilities from its shapes.
///
/// For each of `web.search` and `web.fetch` whose `web.<op>.enabled`
/// setting is stored on, one instance-wide grant (granter
/// [`GRANTER_MIGRATION`]) and its child to the runs of the owner and of
/// every person ([`person_keys_in`]), except a person whose
/// `bot_person_prefs.web` is 0. Nothing for a capability whose switch is
/// off or unset, and nothing for any `browser.open` kind. Then
/// `bot_runs.caps` from `bot_runs.web` (`search`, `fetch`) and
/// `bot_runs.browser` (`test`, `browse`) ([`crate::bot_runs::caps_of_shapes`]).
pub(crate) fn grants_from_main(tx: &Transaction<'_>) -> Result<()> {
    let on = |key: &str| -> Result<bool> {
        let v: Option<String> = tx
            .query_row(
                "SELECT value_json FROM bot_settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(
            v.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
                .and_then(|v| v.as_bool())
                == Some(true),
        )
    };
    let now: String = tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |r| {
        r.get(0)
    })?;
    let mut off_people = std::collections::BTreeSet::new();
    {
        let mut q = tx.prepare("SELECT person FROM bot_person_prefs WHERE web = 0")?;
        for p in q.query_map([], |r| r.get::<_, String>(0))? {
            off_people.insert(p?);
        }
    }
    let people = person_keys_in(tx)?;
    for (cap, key) in [
        ("web.search", "web.search.enabled"),
        ("web.fetch", "web.fetch.enabled"),
    ] {
        if !on(key)? {
            continue;
        }
        let root = insert_grant(
            tx,
            cap,
            GRANTER_MIGRATION,
            EVERYONE,
            None,
            &now,
            GRANTER_MIGRATION,
        )?;
        for person in people.iter().filter(|p| !off_people.contains(*p)) {
            insert_grant(
                tx,
                cap,
                person,
                &Grantee::RunsOf(person.clone()).as_text(),
                Some(&root.id),
                &now,
                GRANTER_MIGRATION,
            )?;
        }
    }
    let runs: Vec<(String, String, String)> = tx
        .prepare("SELECT id, web, browser FROM bot_runs")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut set = tx.prepare("UPDATE bot_runs SET caps = ?2 WHERE id = ?1")?;
    for (id, web, browser) in runs {
        let caps = crate::bot_runs::caps_of_shapes(&web, &browser);
        if !caps.is_empty() {
            set.execute(params![id, caps])?;
        }
    }
    Ok(())
}

impl Store {
    /// Issues a grant of `capability` from `granter` to `grantee` (its
    /// text, [`Grantee::as_text`]) under `parent`, as `ctx`, and returns
    /// it.
    ///
    /// - To [`EVERYONE`] or a person: `granter` is [`GRANTER_INSTANCE`]
    ///   (or [`GRANTER_MIGRATION`]) and there is no parent. In the same
    ///   transaction it issues its child, a grant to the person's runs
    ///   with the person as granter, to every person it covers
    ///   ([`person_keys_in`] for everyone) who has no live grant of the
    ///   capability to their runs: those who revoked an earlier one's
    ///   included.
    /// - To a person's runs: `granter` is that person and `parent` a live
    ///   grant of the same capability to everyone or to them, which they
    ///   hold.
    ///
    /// Each row issued is recorded as `bot.grant` with `op: "issue"`.
    /// Whether `ctx`'s actor may issue it (an admin, or the person) is the
    /// caller's to check.
    ///
    /// # Errors
    /// `unknown_capability`; `invalid_grantee`; `not_held` when the
    /// granter does not hold the capability (a person granting to another
    /// person's runs, or under a parent that is not live, not theirs or of
    /// another capability; an admin grant from another granter or with a
    /// parent); `grant_exists` when the grantee already holds a live grant
    /// of it; the store's.
    pub fn issue_cap_grant(
        &self,
        ctx: &AuditCtx,
        capability: &str,
        granter: &str,
        grantee: &str,
        parent: Option<&str>,
    ) -> Result<GrantRow> {
        check_capability(capability)?;
        let to = Grantee::parse(grantee)?;
        let not_held = |why: &str| {
            CoreError::invalid(
                "not_held",
                format!("{granter} does not hold {capability} to grant it: {why}"),
            )
        };
        self.with_tx(|tx| {
            if live_grant_to(tx, capability, grantee)?.is_some() {
                return Err(CoreError::invalid(
                    "grant_exists",
                    format!("{grantee} already holds a live grant of {capability}"),
                ));
            }
            let covered = match &to {
                Grantee::Everyone | Grantee::Person(_) => {
                    if parent.is_some() {
                        return Err(not_held("an admin grant has no parent"));
                    }
                    if granter != GRANTER_INSTANCE && granter != GRANTER_MIGRATION {
                        return Err(not_held("only the instance grants to people"));
                    }
                    match &to {
                        Grantee::Person(k) => vec![k.clone()],
                        _ => person_keys_in(tx)?,
                    }
                }
                Grantee::RunsOf(k) => {
                    if granter != k {
                        return Err(not_held("a person grants only to their own runs"));
                    }
                    let p = match parent {
                        Some(p) => grant_in(tx, p)?,
                        None => None,
                    };
                    let holds = p.is_some_and(|p| {
                        p.live()
                            && p.capability == capability
                            && (p.grantee == EVERYONE
                                || p.grantee == Grantee::Person(k.clone()).as_text())
                    });
                    if !holds {
                        return Err(not_held("no live grant of it to them is its parent"));
                    }
                    Vec::new()
                }
            };
            let now = self.now();
            let by = set_by(ctx);
            let g = insert_grant(tx, capability, granter, grantee, parent, &now, &by)?;
            self.record_audit(tx, ctx, grant_record(&g, "issue", now.clone()))?;
            for person in covered {
                let runs = Grantee::RunsOf(person.clone()).as_text();
                if live_grant_to(tx, capability, &runs)?.is_some() {
                    continue;
                }
                let child = insert_grant(tx, capability, &person, &runs, Some(&g.id), &now, &by)?;
                self.record_audit(tx, ctx, grant_record(&child, "issue", now.clone()))?;
            }
            Ok(g)
        })
    }

    /// Revokes grant `id` and every live grant under it, transitively, as
    /// `ctx`, each recorded as `bot.grant` with `op: "revoke"`. Returns the
    /// IDs it ended, `id` first; none when `id` was already revoked.
    /// Whether `ctx`'s actor may revoke it is the caller's to check.
    ///
    /// # Errors
    /// `NotFound` when no grant has that ID; the store's.
    pub fn revoke_cap_grant(&self, ctx: &AuditCtx, id: &str) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let g = grant_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if !g.live() {
                return Ok(Vec::new());
            }
            let ended: Vec<GrantRow> = tx
                .prepare(&format!(
                    "WITH RECURSIVE under(id, depth) AS (
                        SELECT ?1, 0
                        UNION
                        SELECT g.id, under.depth + 1 FROM bot_cap_grants g
                          JOIN under ON g.parent = under.id
                         WHERE g.status = 'issued'
                     )
                     SELECT {GRANT_COLS} FROM bot_cap_grants
                      WHERE id IN (SELECT id FROM under) AND status = 'issued'
                      ORDER BY (SELECT MIN(depth) FROM under WHERE under.id = bot_cap_grants.id),
                               issued_at, id"
                ))?
                .query_map(params![id], row_to_grant)?
                .collect::<rusqlite::Result<_>>()?;
            let now = self.now();
            let by = set_by(ctx);
            for g in &ended {
                tx.execute(
                    "UPDATE bot_cap_grants SET status = 'revoked', revoked_at = ?2, revoked_by = ?3
                      WHERE id = ?1",
                    params![g.id, now, by],
                )?;
                self.record_audit(tx, ctx, grant_record(g, "revoke", now.clone()))?;
            }
            Ok(ended.into_iter().map(|g| g.id).collect())
        })
    }

    /// Every person key of the instance: the owner, then each connected
    /// person, revoked ones included, by public ID (whom an instance-wide
    /// grant covers).
    ///
    /// # Errors
    /// The store's.
    pub fn person_keys(&self) -> Result<Vec<String>> {
        self.with_read(person_keys_in)
    }

    /// The grants `filter` selects, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn cap_grants(&self, filter: &GrantFilter) -> Result<Vec<GrantRow>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {GRANT_COLS} FROM bot_cap_grants
                      WHERE (?1 IS NULL OR capability = ?1)
                        AND (?2 IS NULL OR grantee = ?2)
                        AND (?3 = 0 OR status = 'issued')
                      ORDER BY issued_at, id"
            ))?
            .query_map(
                params![filter.capability, filter.grantee, filter.live],
                row_to_grant,
            )?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What `person` holds of each capability ([`CAPABILITIES`], every one
    /// a key): the live admin grant that gives it to them, and their live
    /// grant of it to their runs.
    ///
    /// # Errors
    /// The store's.
    pub fn person_caps(&self, person: &str) -> Result<BTreeMap<String, PersonCap>> {
        let as_person = Grantee::Person(person.to_string()).as_text();
        let runs = Grantee::RunsOf(person.to_string()).as_text();
        self.with_read(|c| {
            let mut out = BTreeMap::new();
            for cap in CAPABILITIES {
                let runs_grant = live_grant_to(c, cap, &runs)?;
                let mine = live_grant_to(c, cap, &as_person)?;
                let all = live_grant_to(c, cap, EVERYONE)?;
                let parent = runs_grant.as_ref().and_then(|g| g.parent.clone());
                let held: Vec<String> = [mine, all].into_iter().flatten().map(|g| g.id).collect();
                // The one the runs grant is under, else theirs, else everyone's.
                let granted_by = held
                    .iter()
                    .find(|id| Some(*id) == parent.as_ref())
                    .or_else(|| held.first())
                    .cloned();
                out.insert(
                    cap.to_string(),
                    PersonCap {
                        granted_by,
                        runs_grant: runs_grant.map(|g| g.id),
                    },
                );
            }
            Ok(out)
        })
    }

    /// Issues to `person`, who joined just now, the child of each live
    /// instance-wide grant they have never held a child of, on `tx`, as
    /// `ctx`: a person row made or made a person again
    /// ([`Store::create_invite`], a browser's invite, a GitHub sign-up)
    /// calls it in its own transaction. A child they revoked stays revoked.
    ///
    /// # Errors
    /// The store's.
    pub(crate) fn issue_joined_grants_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        person: &str,
    ) -> Result<()> {
        let runs = Grantee::RunsOf(person.to_string()).as_text();
        let roots: Vec<GrantRow> = tx
            .prepare_cached(&format!(
                "SELECT {GRANT_COLS} FROM bot_cap_grants g
                  WHERE grantee = ?1 AND status = 'issued'
                    AND NOT EXISTS (SELECT 1 FROM bot_cap_grants c
                                     WHERE c.parent = g.id AND c.grantee = ?2)
                  ORDER BY issued_at, id"
            ))?
            .query_map(params![EVERYONE, runs], row_to_grant)?
            .collect::<rusqlite::Result<_>>()?;
        let now = self.now();
        let by = set_by(ctx);
        for root in roots {
            if live_grant_to(tx, &root.capability, &runs)?.is_some() {
                continue;
            }
            let child = insert_grant(
                tx,
                &root.capability,
                person,
                &runs,
                Some(&root.id),
                &now,
                &by,
            )?;
            self.record_audit(tx, ctx, grant_record(&child, "issue", now.clone()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::roles::tests::person;
    use crate::test_util::{DAEMON, store};

    fn admin_everyone(st: &Store, cap: &str) -> GrantRow {
        st.issue_cap_grant(DAEMON, cap, GRANTER_INSTANCE, EVERYONE, None)
            .unwrap()
    }

    fn runs(person: &str) -> String {
        Grantee::RunsOf(person.to_string()).as_text()
    }

    fn grant_events(st: &Store) -> Vec<serde_json::Value> {
        st.events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "bot.grant")
            .map(|e| serde_json::from_str(&e.body).unwrap())
            .collect()
    }

    fn code(e: &CoreError) -> &str {
        match e {
            CoreError::Invalid { code, .. } => code,
            other => panic!("not an invalid: {other:?}"),
        }
    }

    #[test]
    fn grantees_round_trip_through_their_text() {
        for g in [
            Grantee::Everyone,
            Grantee::Person("u_ana".into()),
            Grantee::RunsOf(OWNER.into()),
        ] {
            assert_eq!(Grantee::parse(&g.as_text()).unwrap(), g);
        }
        for bad in ["", "person:", "runs_of", "team:x", "Everyone"] {
            assert!(Grantee::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_instance_wide_grant_issues_a_child_to_each_person_and_to_a_new_one() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "browser.open.content");
        for p in [OWNER, sam.as_str()] {
            let caps = st.person_caps(p).unwrap();
            let c = &caps["browser.open.content"];
            assert_eq!(c.granted_by.as_deref(), Some(root.id.as_str()), "{p}");
            assert!(c.reaches_runs(), "{p}");
            assert!(!caps["browser.open.url"].reaches_runs(), "{p}");
        }
        let child = st
            .cap_grants(&GrantFilter {
                grantee: Some(runs(&sam)),
                ..GrantFilter::default()
            })
            .unwrap();
        assert_eq!(child.len(), 1);
        assert_eq!(child[0].granter, sam, "the person grants to their runs");
        assert_eq!(child[0].parent.as_deref(), Some(root.id.as_str()));

        let mia = person(&st, "Mia", 2);
        assert!(
            st.person_caps(&mia).unwrap()["browser.open.content"].reaches_runs(),
            "a person who joins later gets the child"
        );
    }

    #[test]
    fn revoking_a_grant_ends_its_children_transitively() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "web.fetch");
        let other = admin_everyone(&st, "web.search");
        let ended = st.revoke_cap_grant(DAEMON, &root.id).unwrap();
        assert_eq!(ended.len(), 3, "the root and two children: {ended:?}");
        assert_eq!(ended[0], root.id);
        for p in [OWNER, sam.as_str()] {
            let caps = st.person_caps(p).unwrap();
            assert_eq!(caps["web.fetch"], PersonCap::default(), "{p}");
            assert!(caps["web.search"].reaches_runs(), "{p}");
        }
        assert!(st.revoke_cap_grant(DAEMON, &root.id).unwrap().is_empty());
        assert!(
            st.cap_grants(&GrantFilter {
                capability: Some("web.fetch".into()),
                live: true,
                ..GrantFilter::default()
            })
            .unwrap()
            .is_empty()
        );
        assert!(other.live());
        assert!(matches!(
            st.revoke_cap_grant(DAEMON, "nope"),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn a_person_revokes_and_reissues_only_their_own() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "browser.open.artifact");
        let mine = st.person_caps(&sam).unwrap()["browser.open.artifact"]
            .runs_grant
            .clone()
            .unwrap();
        assert_eq!(st.revoke_cap_grant(DAEMON, &mine).unwrap(), [mine.as_str()]);
        assert!(!st.person_caps(&sam).unwrap()["browser.open.artifact"].reaches_runs());
        assert!(
            st.person_caps(OWNER).unwrap()["browser.open.artifact"].reaches_runs(),
            "the owner's is their own"
        );
        assert!(root.live());
        let again = st
            .issue_cap_grant(
                DAEMON,
                "browser.open.artifact",
                &sam,
                &runs(&sam),
                Some(&root.id),
            )
            .unwrap();
        assert_eq!(again.granter, sam);
        assert!(st.person_caps(&sam).unwrap()["browser.open.artifact"].reaches_runs());
        let e = st
            .issue_cap_grant(
                DAEMON,
                "browser.open.artifact",
                &sam,
                &runs(&sam),
                Some(&root.id),
            )
            .unwrap_err();
        assert_eq!(code(&e), "grant_exists");
        let mine = st.person_caps(OWNER).unwrap()["browser.open.artifact"]
            .runs_grant
            .clone()
            .unwrap();
        st.revoke_cap_grant(DAEMON, &mine).unwrap();
        let e = st
            .issue_cap_grant(
                DAEMON,
                "browser.open.artifact",
                &sam,
                &runs(OWNER),
                Some(&root.id),
            )
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "not another person's runs");
    }

    #[test]
    fn a_person_cannot_issue_what_they_do_not_hold() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let to_sam = |parent: Option<&str>| {
            st.issue_cap_grant(DAEMON, "browser.open.url", &sam, &runs(&sam), parent)
        };
        assert_eq!(code(&to_sam(None).unwrap_err()), "not_held");
        let search = admin_everyone(&st, "web.search");
        assert_eq!(
            code(&to_sam(Some(&search.id)).unwrap_err()),
            "not_held",
            "a parent of another capability"
        );
        let mia = person(&st, "Mia", 2);
        let to_mia = st
            .issue_cap_grant(
                DAEMON,
                "browser.open.url",
                GRANTER_INSTANCE,
                &Grantee::Person(mia.clone()).as_text(),
                None,
            )
            .unwrap();
        assert_eq!(
            code(&to_sam(Some(&to_mia.id)).unwrap_err()),
            "not_held",
            "another person's grant"
        );
        assert!(st.person_caps(&mia).unwrap()["browser.open.url"].reaches_runs());
        st.revoke_cap_grant(DAEMON, &to_mia.id).unwrap();
        let e = st
            .issue_cap_grant(
                DAEMON,
                "browser.open.url",
                &mia,
                &runs(&mia),
                Some(&to_mia.id),
            )
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "a revoked parent");
        let e = st
            .issue_cap_grant(DAEMON, "browser.open.url", &sam, EVERYONE, None)
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "only the instance grants to people");
        let e = st
            .issue_cap_grant(DAEMON, "web.crawl", GRANTER_INSTANCE, EVERYONE, None)
            .unwrap_err();
        assert_eq!(code(&e), "unknown_capability");
    }

    #[test]
    fn every_issue_and_revoke_is_one_bot_grant() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "web.search");
        let events = grant_events(&st);
        assert_eq!(events.len(), 3, "the root and a child each: {events:?}");
        assert_eq!(events[0]["op"], "issue");
        assert_eq!(events[0]["grant"], root.id.as_str());
        assert_eq!(events[0]["grantee"], EVERYONE);
        assert_eq!(events[0]["granter"], GRANTER_INSTANCE);
        assert_eq!(events[0]["capability"], "web.search");
        assert!(events[0].get("parent").is_none());
        assert!(
            events[1..]
                .iter()
                .all(|e| e["parent"] == root.id.as_str() && e["op"] == "issue")
        );
        assert!(events.iter().any(|e| e["grantee"] == runs(&sam).as_str()));
        st.revoke_cap_grant(DAEMON, &root.id).unwrap();
        let events = grant_events(&st);
        assert_eq!(events.len(), 6);
        assert!(events[3..].iter().all(|e| e["op"] == "revoke"));
        assert_eq!(events[3]["grant"], root.id.as_str());
    }
}
