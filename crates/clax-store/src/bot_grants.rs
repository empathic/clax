//! ClaxBot's capability grants (migration 68, spec
//! 2026-11-19-claxbot-browser-design §4.2, §4.3, §11.2, §18.4).
//!
//! A run starts with no capability. Every capability it holds traces to
//! rows of `bot_cap_grants`, each shaped as the zero-capability design's
//! grant (`2026-11-17-zero-capability-gnap-design.md`, "Terms"): `{id,
//! granter, grantee, access, parent?, conditions, status, version, event,
//! issued_at}`, its `access` an array of RFC 9635 access objects of
//! `clax-web` or `clax-browser` ([`Access`]). Nothing is implied, and a
//! grant conveys `conv(g) ∩ caps(g.granter)`:
//!
//! - **An admin's grant** gives an action to one person (`person:<key>`)
//!   or to everyone, the selector [`EVERYONE`] (`people_of:instance`),
//!   which conveys nothing itself and issues each person a child grant
//!   (`person:<key>`, parent the selector). Its granter is the admin who
//!   issued it, by person key ([`GRANTER_MIGRATION`] and
//!   [`GRANTER_CONFIG`] stand for the instance's own acts), and it
//!   conveys while that person is the owner or an admin: an admin who
//!   loses the role stops conveying, without a row changing.
//! - **A person's grant to their own runs** is the selector
//!   `runs_of:<key>` (granter the person, no parent): what reaches their
//!   runs, conveying what it lists of what the person holds. An admin's
//!   grant issues it, in the same transaction, to each person it covers
//!   who holds no live one; a person who joins later gets the selector's
//!   child and it when their row is made ([`Store::issue_joined_grants_in`]).
//! - **A page's share** is a child of the sharer's grant (§18.4): granter
//!   and grantee `run:<ID>`, its ID the capability's, written in the
//!   share's Spend transaction.
//! - **A revoke ends the grant and its live descendants**, transitively,
//!   the grant with the reason given ([`RevokeReason`]) and each
//!   descendant `parent_ended`. A person's runs grant is no child of an
//!   admin's, so one admin grant of two revoked leaves the person holding
//!   the action from the other.
//!
//! Each row issued is recorded as `grant.issue {grant_id, granter,
//! grantee, access, parent?, event, expires_at?}`, each revoked as
//! `grant.revoke {grant_id, by, reason}`; migration 68's rows are written
//! by the migration alone.
//!
//! Whether the admin's switch offers an action, and whether the host can
//! perform it, are not grants: the caller intersects them
//! (`clax_bot::capability::Capabilities::now`).

use super::Store;
use super::roles::{OWNER, role_in, set_by};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The access type of retrieval (spec §4.2).
pub const TYPE_WEB: &str = "clax-web";
/// The access type of a running browser.
pub const TYPE_BROWSER: &str = "clax-browser";

/// Every capability a switch and a grant name, as `<type>:<action>`, in
/// the order they are shown.
pub const CAPABILITIES: [&str; 5] = [
    "clax-web:search",
    "clax-web:fetch",
    "clax-browser:open-url",
    "clax-browser:open-artifact",
    "clax-browser:open-content",
];

/// The retrieval capabilities.
pub const WEB_CAPABILITIES: [&str; 2] = ["clax-web:search", "clax-web:fetch"];

/// The running browser's capabilities, one per source kind.
pub const BROWSER_CAPABILITIES: [&str; 3] = [
    "clax-browser:open-url",
    "clax-browser:open-artifact",
    "clax-browser:open-content",
];

/// A browser session's actions (spec §4.2, §18.3), which every grant of
/// an `open-*` action lists beside it.
pub const SESSION_ACTIONS: [&str; 5] = ["read", "act", "navigate", "record", "close"];

/// The grantee of a grant to everyone on the instance: a selector grant.
pub const EVERYONE: &str = "people_of:instance";

/// The granter of a grant migration 68 wrote from main's web switches.
pub const GRANTER_MIGRATION: &str = "migration";

/// The granter of a grant to everyone a daemon wrote at its start
/// because its config.toml (or the environment) turns a web switch on
/// ([`Store::grant_web_from_config`]): shown on the admin page as from
/// config.toml.
pub const GRANTER_CONFIG: &str = "config";

/// `cap` (`<type>:<action>`) as its type and action.
#[must_use]
pub fn split_capability(cap: &str) -> Option<(&str, &str)> {
    cap.split_once(':')
}

/// One RFC 9635 §8 access object of `clax-web` or `clax-browser`: its
/// type, actions and, when it has any, locations (origins, hosts or
/// artifact IDs, spec §4.2). One without locations applies everywhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    #[serde(rename = "type")]
    pub kind: String,
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<String>>,
}

impl Access {
    /// Whether it allows `action` of `kind` at `location`: a call that
    /// goes nowhere (`None`) is not held to its locations.
    #[must_use]
    pub fn allows(&self, kind: &str, action: &str, location: Option<&str>) -> bool {
        self.kind == kind
            && self.actions.iter().any(|a| a == action)
            && match (&self.locations, location) {
                (Some(ls), Some(l)) => ls.iter().any(|x| x == l),
                _ => true,
            }
    }
}

/// Whether `access` allows `action` of `kind` at `location`.
#[must_use]
pub fn allows(access: &[Access], kind: &str, action: &str, location: Option<&str>) -> bool {
    access.iter().any(|a| a.allows(kind, action, location))
}

/// The access array a grant of capability `cap` carries: its action, and
/// for an `open-*` action the session's actions beside it (spec §4.2).
///
/// # Errors
/// `unknown_capability`.
pub fn access_of(cap: &str) -> Result<Vec<Access>> {
    if !CAPABILITIES.contains(&cap) {
        return Err(CoreError::invalid(
            "unknown_capability",
            format!("no capability is named '{cap}'"),
        ));
    }
    let (kind, action) = split_capability(cap).expect("a capability is <type>:<action>");
    let mut actions = vec![action.to_string()];
    if kind == TYPE_BROWSER {
        actions.extend(SESSION_ACTIONS.iter().map(|a| (*a).to_string()));
    }
    Ok(vec![Access {
        kind: kind.to_string(),
        actions,
        locations: None,
    }])
}

/// Who a grant is to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grantee {
    /// Every person of the instance, present and joining later: a
    /// selector grant ([`EVERYONE`]).
    Everyone,
    /// One person, by person key.
    Person(String),
    /// A person's own runs, by the person's key: a selector grant.
    RunsOf(String),
    /// One run, by its ID: a page's share (§18.4).
    Run(String),
    /// One grant of the grant flow, by its ID: an exported page (§18.5).
    Grant(String),
}

impl Grantee {
    /// The grantee as `bot_cap_grants.grantee` keeps it.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Grantee::Everyone => EVERYONE.to_string(),
            Grantee::Person(k) => format!("person:{k}"),
            Grantee::RunsOf(k) => format!("runs_of:{k}"),
            Grantee::Run(id) => format!("run:{id}"),
            Grantee::Grant(id) => format!("grant:{id}"),
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
                format!(
                    "'{text}' is not {EVERYONE}, person:<key>, runs_of:<key>, run:<ID> or grant:<ID>"
                ),
            )
        };
        if text == EVERYONE {
            return Ok(Grantee::Everyone);
        }
        let (kind, key) = text.split_once(':').ok_or_else(bad)?;
        if key.is_empty() {
            return Err(bad());
        }
        let key = key.to_string();
        match kind {
            "person" => Ok(Grantee::Person(key)),
            "runs_of" => Ok(Grantee::RunsOf(key)),
            "run" => Ok(Grantee::Run(key)),
            "grant" => Ok(Grantee::Grant(key)),
            _ => Err(bad()),
        }
    }
}

/// Why a grant was revoked: the zero-capability design's reasons
/// ("Journal records", `grant.revoke`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeReason {
    /// Its granter revoked it.
    Granter,
    /// Its grantee's controller (the person) revoked it.
    Controller,
    /// An admin who is not its granter revoked it.
    Owner,
    /// An ancestor of it ended.
    ParentEnded,
}

impl RevokeReason {
    /// `granter`, `controller`, `owner` or `parent_ended`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RevokeReason::Granter => "granter",
            RevokeReason::Controller => "controller",
            RevokeReason::Owner => "owner",
            RevokeReason::ParentEnded => "parent_ended",
        }
    }
}

/// One row of `bot_cap_grants`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GrantRow {
    pub id: String,
    pub access: Vec<Access>,
    /// A person key, `run:<ID>`, [`GRANTER_MIGRATION`] or
    /// [`GRANTER_CONFIG`].
    pub granter: String,
    /// [`Grantee::as_text`].
    pub grantee: String,
    pub parent: Option<String>,
    /// `{}`, or `{"expires_at": <RFC 3339>}`.
    pub conditions: Value,
    /// `issued` or `revoked`.
    pub status: String,
    pub version: i64,
    /// `admin`, `bot`, `share`, `migration` or `config`.
    pub event: String,
    pub issued_at: String,
    pub issued_by: String,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
    pub revoke_reason: Option<String>,
}

impl GrantRow {
    /// Whether it is issued (not revoked); its conditions are
    /// [`GrantRow::holds_at`]'s.
    #[must_use]
    pub fn live(&self) -> bool {
        self.status == "issued"
    }

    /// When it expires, if it does.
    #[must_use]
    pub fn expires_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.conditions
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
    }

    /// Whether it is issued and its conditions hold at `now`.
    #[must_use]
    pub fn holds_at(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.live() && self.expires_at().is_none_or(|t| now < t)
    }

    /// Whether it lists capability `cap` (`<type>:<action>`).
    #[must_use]
    pub fn lists(&self, cap: &str) -> bool {
        split_capability(cap).is_some_and(|(k, a)| allows(&self.access, k, a, None))
    }

    /// The capability ([`CAPABILITIES`]) it is a grant of: the first it
    /// lists. A share lists none.
    #[must_use]
    pub fn capability(&self) -> Option<&'static str> {
        CAPABILITIES.into_iter().find(|c| self.lists(c))
    }
}

/// Which grants [`Store::cap_grants`] lists.
#[derive(Clone, Debug, Default)]
pub struct GrantFilter {
    /// Only grants that list this capability.
    pub capability: Option<String>,
    /// Only grants to this grantee (its text, [`Grantee::as_text`]).
    pub grantee: Option<String>,
    /// Only grants issued (not revoked).
    pub live: bool,
}

/// What a person holds of one capability ([`Store::person_caps`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PersonCap {
    /// A live admin grant to the person that conveys it now (one to them,
    /// else a selector's child), if any.
    pub granted_by: Option<String>,
    /// The person's live grant of it to their own runs, if any.
    pub runs_grant: Option<String>,
}

impl PersonCap {
    /// Whether the capability reaches the person's runs.
    #[must_use]
    pub fn reaches_runs(&self) -> bool {
        self.granted_by.is_some() && self.runs_grant.is_some()
    }
}

const GRANT_COLS: &str = "id, access_json, granter, grantee, parent, conditions_json, status,
    version, event, issued_at, issued_by, revoked_at, revoked_by, revoke_reason";

fn row_to_grant(r: &Row<'_>) -> rusqlite::Result<GrantRow> {
    let json_col = |i: usize| -> rusqlite::Result<Value> {
        let t: String = r.get(i)?;
        serde_json::from_str(&t).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let access = serde_json::from_value(json_col(1)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(GrantRow {
        id: r.get(0)?,
        access,
        granter: r.get(2)?,
        grantee: r.get(3)?,
        parent: r.get(4)?,
        conditions: json_col(5)?,
        status: r.get(6)?,
        version: r.get(7)?,
        event: r.get(8)?,
        issued_at: r.get(9)?,
        issued_by: r.get(10)?,
        revoked_at: r.get(11)?,
        revoked_by: r.get(12)?,
        revoke_reason: r.get(13)?,
    })
}

pub(crate) fn grant_in(c: &Connection, id: &str) -> Result<Option<GrantRow>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {GRANT_COLS} FROM bot_cap_grants WHERE id = ?1"
    ))?
    .query_row(params![id], row_to_grant)
    .optional()?)
}

/// The issued grants to `grantee` on `c`, oldest first.
fn issued_to(c: &Connection, grantee: &str) -> Result<Vec<GrantRow>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {GRANT_COLS} FROM bot_cap_grants
          WHERE grantee = ?1 AND status = 'issued'
          ORDER BY issued_at, id"
    ))?
    .query_map(params![grantee], row_to_grant)?
    .collect::<rusqlite::Result<_>>()?)
}

/// Whether grant `g`'s parents, up to its root, are each issued and hold
/// at `now`, on `c`.
fn chain_holds(c: &Connection, g: &GrantRow, now: chrono::DateTime<chrono::Utc>) -> Result<bool> {
    let mut seen = BTreeSet::new();
    let mut at = g.clone();
    loop {
        if !at.holds_at(now) || !seen.insert(at.id.clone()) {
            return Ok(false);
        }
        match &at.parent {
            None => return Ok(true),
            Some(p) => match grant_in(c, p)? {
                Some(parent) => at = parent,
                None => return Ok(false),
            },
        }
    }
}

/// Whether `granter` conveys as an admin's grant does now, on `c`: the
/// instance's own acts always, a person while they are the owner or an
/// admin (spec §4.3).
fn has_authority(c: &Connection, granter: &str) -> Result<bool> {
    if granter == GRANTER_MIGRATION || granter == GRANTER_CONFIG {
        return Ok(true);
    }
    Ok(role_in(c, granter)?.is_admin())
}

/// The issued grant to `grantee` that lists `cap` and has no parent, on
/// `c`, if any.
fn root_grant_to(c: &Connection, cap: &str, grantee: &str) -> Result<Option<GrantRow>> {
    Ok(issued_to(c, grantee)?
        .into_iter()
        .find(|g| g.parent.is_none() && g.lists(cap)))
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

/// A grant row to insert ([`insert_grant`]).
struct NewGrant<'a> {
    id: Option<String>,
    access: Vec<Access>,
    granter: &'a str,
    grantee: &'a str,
    parent: Option<&'a str>,
    conditions: Value,
    event: &'a str,
}

/// Inserts `g` on `c`, issued at `at` by `by`, and returns its row.
fn insert_grant(c: &Connection, g: NewGrant<'_>, at: &str, by: &str) -> Result<GrantRow> {
    let id = g.id.unwrap_or_else(clax_core::new_ulid);
    let access = serde_json::to_string(&g.access).expect("access encodes");
    c.execute(
        "INSERT INTO bot_cap_grants (id, access_json, granter, grantee, parent, conditions_json,
            status, version, event, issued_at, issued_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'issued', 1, ?7, ?8, ?9)",
        params![
            id,
            access,
            g.granter,
            g.grantee,
            g.parent,
            g.conditions.to_string(),
            g.event,
            at,
            by
        ],
    )?;
    Ok(GrantRow {
        id,
        access: g.access,
        granter: g.granter.to_string(),
        grantee: g.grantee.to_string(),
        parent: g.parent.map(str::to_string),
        conditions: g.conditions,
        status: "issued".into(),
        version: 1,
        event: g.event.to_string(),
        issued_at: at.to_string(),
        issued_by: by.to_string(),
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
    })
}

/// The `grant.issue` record of `g`.
fn issue_record(g: &GrantRow, at: String) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::GrantIssue, at)
        .with("grant_id", g.id.as_str())
        .with("granter", g.granter.as_str())
        .with("grantee", g.grantee.as_str())
        .with(
            "access",
            serde_json::to_value(&g.access).expect("access encodes"),
        )
        .with("event", g.event.as_str());
    if let Some(p) = &g.parent {
        rec = rec.with("parent", p.as_str());
    }
    if let Some(t) = g.conditions.get("expires_at") {
        rec = rec.with("expires_at", t.clone());
    }
    rec
}

/// The `grant.revoke` record of `g`, revoked by `by` for `reason`.
fn revoke_record(g: &GrantRow, by: &str, reason: RevokeReason, at: String) -> AuditRecord {
    AuditRecord::new(AuditKind::GrantRevoke, at)
        .with("grant_id", g.id.as_str())
        .with("by", by)
        .with("reason", reason.as_str())
}

/// The person keys whose web switch was off on main
/// (`bot_person_prefs.web` 0), on `c`: whom migration 68 and
/// [`Store::grant_web_from_config`] give no grant of web to their runs.
fn web_off_people_in(c: &Connection) -> Result<BTreeSet<String>> {
    let mut q = c.prepare_cached("SELECT person FROM bot_person_prefs WHERE web = 0")?;
    let rows = q.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Writes, on `c`, a grant of `cap` from `granter` to everyone: the
/// selector, its child to each of `people`, and the runs grant of each
/// who holds no live one and is not in `skip_runs`, issued at `at` by
/// `by` with `event` (the children `admin` unless `event` is the
/// migration's or the config's). Returns every row written, the selector
/// first.
#[allow(clippy::too_many_arguments)]
fn write_everyone(
    c: &Connection,
    cap: &str,
    granter: &str,
    people: &[String],
    skip_runs: &BTreeSet<String>,
    event: &str,
    at: &str,
    by: &str,
) -> Result<Vec<GrantRow>> {
    let access = access_of(cap)?;
    let child_event = if event == "admin" { "admin" } else { event };
    let runs_event = if event == "admin" { "bot" } else { event };
    let root = insert_grant(
        c,
        NewGrant {
            id: None,
            access: access.clone(),
            granter,
            grantee: EVERYONE,
            parent: None,
            conditions: json!({}),
            event,
        },
        at,
        by,
    )?;
    let mut out = vec![root.clone()];
    for person in people {
        let to = Grantee::Person(person.clone()).as_text();
        out.push(insert_grant(
            c,
            NewGrant {
                id: None,
                access: access.clone(),
                granter,
                grantee: &to,
                parent: Some(&root.id),
                conditions: json!({}),
                event: child_event,
            },
            at,
            by,
        )?);
        if skip_runs.contains(person) {
            continue;
        }
        out.extend(issue_runs_if_none(c, cap, person, runs_event, at, by)?);
    }
    Ok(out)
}

/// The runs grant of `cap` to `person`'s runs, written on `c` unless one
/// is live, issued at `at` by `by` with `event`.
fn issue_runs_if_none(
    c: &Connection,
    cap: &str,
    person: &str,
    event: &str,
    at: &str,
    by: &str,
) -> Result<Option<GrantRow>> {
    let runs = Grantee::RunsOf(person.to_string()).as_text();
    if issued_to(c, &runs)?.iter().any(|g| g.lists(cap)) {
        return Ok(None);
    }
    Ok(Some(insert_grant(
        c,
        NewGrant {
            id: None,
            access: access_of(cap)?,
            granter: person,
            grantee: &runs,
            parent: None,
            conditions: json!({}),
            event,
        },
        at,
        by,
    )?))
}

/// Migration 68's Rust step, on `tx`: main's web switches as explicit
/// grants, and each run's capabilities from its shapes.
///
/// For each of `clax-web:search` and `clax-web:fetch` whose
/// `web.<op>.enabled` setting is stored on, one selector grant to
/// everyone (granter [`GRANTER_MIGRATION`], event `migration`), its child
/// to the owner and every person ([`person_keys_in`]), and the runs grant
/// of each except a person whose `bot_person_prefs.web` is 0. Nothing for
/// a capability whose switch is off or unset, and nothing of
/// `clax-browser`. Then `bot_runs.caps` from `bot_runs.web` (`search`,
/// `fetch`) ([`crate::bot_runs::caps_of_web`]).
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
    let off_people = web_off_people_in(tx)?;
    let people = person_keys_in(tx)?;
    for (cap, key) in [
        ("clax-web:search", "web.search.enabled"),
        ("clax-web:fetch", "web.fetch.enabled"),
    ] {
        if !on(key)? {
            continue;
        }
        write_everyone(
            tx,
            cap,
            GRANTER_MIGRATION,
            &people,
            &off_people,
            "migration",
            &now,
            GRANTER_MIGRATION,
        )?;
    }
    let runs: Vec<(String, String)> = tx
        .prepare("SELECT id, web FROM bot_runs")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut set = tx.prepare("UPDATE bot_runs SET caps = ?2 WHERE id = ?1")?;
    for (id, web) in runs {
        let caps = crate::bot_runs::caps_of_web(&web);
        if !caps.is_empty() {
            set.execute(params![id, caps])?;
        }
    }
    Ok(())
}

/// What `person` holds of each capability on `c`, at `now` ([`PersonCap`]).
fn person_caps_in(
    c: &Connection,
    person: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<BTreeMap<String, PersonCap>> {
    let mine = issued_to(c, &Grantee::Person(person.to_string()).as_text())?;
    let runs = issued_to(c, &Grantee::RunsOf(person.to_string()).as_text())?;
    let mut conveying = Vec::new();
    for g in mine {
        if chain_holds(c, &g, now)? && has_authority(c, &g.granter)? {
            conveying.push(g);
        }
    }
    // A grant to them before a selector's child.
    conveying.sort_by_key(|g| g.parent.is_some());
    let mut out = BTreeMap::new();
    for cap in CAPABILITIES {
        let granted_by = conveying
            .iter()
            .find(|g| g.lists(cap))
            .map(|g| g.id.clone());
        let runs_grant = runs
            .iter()
            .find(|g| g.lists(cap) && g.holds_at(now) && g.granter == person)
            .map(|g| g.id.clone());
        out.insert(
            cap.to_string(),
            PersonCap {
                granted_by,
                runs_grant,
            },
        );
    }
    Ok(out)
}

impl Store {
    /// The store's clock, as a time.
    fn grants_now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now_utc()
    }

    /// Issues a grant of `capability` from `granter` to `grantee` (its
    /// text, [`Grantee::as_text`]), as `ctx`, and returns it.
    ///
    /// - To [`EVERYONE`] or a person: `granter` is the admin who issues it
    ///   (a person key; [`GRANTER_MIGRATION`] or [`GRANTER_CONFIG`] for the
    ///   instance's own acts). To everyone it is a selector, which issues
    ///   each person ([`person_keys_in`]) a child; either way, in the same
    ///   transaction, each person it covers who holds no live grant of the
    ///   capability to their runs gets one (granter the person, event
    ///   `bot`): those who revoked an earlier one's included.
    /// - To a person's runs: `granter` is that person, and the person
    ///   holds the capability ([`Store::person_caps`]).
    ///
    /// Each row issued is recorded as `grant.issue`. Whether `ctx`'s actor
    /// may ask (an admin, or the person) is the caller's to check.
    ///
    /// # Errors
    /// `unknown_capability`; `invalid_grantee` (a run or a grant of the
    /// grant flow: a share's and an export's are their own); `not_held`
    /// when the granter does not hold the capability to grant it (an
    /// admin's grant from a person who is not an admin, a person granting
    /// to another person's runs or what they do not hold); `grant_exists`
    /// when the grantee already holds a live grant of it from an admin
    /// (or to their runs); the store's.
    pub fn issue_cap_grant(
        &self,
        ctx: &AuditCtx,
        capability: &str,
        granter: &str,
        grantee: &str,
    ) -> Result<GrantRow> {
        let access = access_of(capability)?;
        let to = Grantee::parse(grantee)?;
        let not_held = |why: &str| {
            CoreError::invalid(
                "not_held",
                format!("{granter} does not hold {capability} to grant it: {why}"),
            )
        };
        let exists = || {
            CoreError::invalid(
                "grant_exists",
                format!("{grantee} already holds a live grant of {capability}"),
            )
        };
        let now = self.now();
        let by = set_by(ctx);
        let at = self.grants_now();
        self.with_tx(|tx| {
            let written = match &to {
                Grantee::Everyone | Grantee::Person(_) => {
                    if !has_authority(tx, granter)? {
                        return Err(not_held("only an admin grants to people"));
                    }
                    if root_grant_to(tx, capability, grantee)?.is_some() {
                        return Err(exists());
                    }
                    match &to {
                        Grantee::Everyone => write_everyone(
                            tx,
                            capability,
                            granter,
                            &person_keys_in(tx)?,
                            &BTreeSet::new(),
                            "admin",
                            &now,
                            &by,
                        )?,
                        Grantee::Person(k) => {
                            let g = insert_grant(
                                tx,
                                NewGrant {
                                    id: None,
                                    access,
                                    granter,
                                    grantee,
                                    parent: None,
                                    conditions: json!({}),
                                    event: "admin",
                                },
                                &now,
                                &by,
                            )?;
                            let mut out = vec![g];
                            out.extend(issue_runs_if_none(tx, capability, k, "bot", &now, &by)?);
                            out
                        }
                        _ => unreachable!("matched above"),
                    }
                }
                Grantee::RunsOf(k) => {
                    if granter != k {
                        return Err(not_held("a person grants only to their own runs"));
                    }
                    if person_caps_in(tx, k, at)?[capability].granted_by.is_none() {
                        return Err(not_held("no admin's grant gives it to them"));
                    }
                    match issue_runs_if_none(tx, capability, k, "bot", &now, &by)? {
                        Some(g) => vec![g],
                        None => return Err(exists()),
                    }
                }
                Grantee::Run(_) | Grantee::Grant(_) => {
                    return Err(CoreError::invalid(
                        "invalid_grantee",
                        "a run's and a grant holder's grants are its shares and exports",
                    ));
                }
            };
            for g in &written {
                self.record_audit(tx, ctx, issue_record(g, now.clone()))?;
            }
            Ok(written.into_iter().next().expect("one row at least"))
        })
    }

    /// Revokes grant `id` for `reason` and every issued grant under it,
    /// transitively (`parent_ended`), as `ctx`, each recorded as
    /// `grant.revoke`. Returns the IDs it ended, `id` first; none when
    /// `id` was already revoked. Whether `ctx`'s actor may revoke it is
    /// the caller's to check.
    ///
    /// # Errors
    /// `NotFound` when no grant has that ID; the store's.
    pub fn revoke_cap_grant(
        &self,
        ctx: &AuditCtx,
        id: &str,
        reason: RevokeReason,
    ) -> Result<Vec<String>> {
        self.with_tx(|tx| self.revoke_grant_in(tx, ctx, id, reason))
    }

    /// [`Store::revoke_cap_grant`] on `tx`.
    pub(crate) fn revoke_grant_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        reason: RevokeReason,
    ) -> Result<Vec<String>> {
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
        for (i, g) in ended.iter().enumerate() {
            let why = if i == 0 {
                reason
            } else {
                RevokeReason::ParentEnded
            };
            tx.execute(
                "UPDATE bot_cap_grants SET status = 'revoked', revoked_at = ?2, revoked_by = ?3,
                        revoke_reason = ?4
                  WHERE id = ?1",
                params![g.id, now, by, why.as_str()],
            )?;
            self.record_audit(tx, ctx, revoke_record(g, &by, why, now.clone()))?;
        }
        Ok(ended.into_iter().map(|g| g.id).collect())
    }

    /// The grants to everyone of each of `clax-web:search` and
    /// `clax-web:fetch` that the daemon's configuration turns on (its
    /// switch an override from config.toml or the environment,
    /// [`Store::set_bot_overrides`]) and that no grant of it has ever been
    /// issued, to anyone, live or revoked: what migration 68 writes for a
    /// switch stored on, which it cannot see for one set only in the
    /// configuration. An instance whose admin has granted the capability to
    /// some people keeps the grants the admin made. As migration 68 does,
    /// it issues the selector's child to the owner and every person
    /// ([`person_keys_in`]), and the runs grant of each but a person whose
    /// `bot_person_prefs.web` is 0. Issued as `ctx` in one transaction,
    /// granter [`GRANTER_CONFIG`], event `config`, each row recorded as
    /// `grant.issue`. So it is written at most once per instance and
    /// capability: a restart after the admin revokes it writes nothing.
    /// Nothing of `clax-browser` is ever granted here. Returns the
    /// selectors issued.
    ///
    /// # Errors
    /// The store's.
    pub fn grant_web_from_config(&self, ctx: &AuditCtx) -> Result<Vec<String>> {
        use crate::bot_settings::{Setting, SettingSource};
        let eff = self.effective_bot_settings()?;
        let caps: Vec<&str> = [
            ("clax-web:search", Setting::WebSearchEnabled),
            ("clax-web:fetch", Setting::WebFetchEnabled),
        ]
        .into_iter()
        .filter(|(_, switch)| {
            eff.get(switch).is_some_and(|e| {
                matches!(e.source, SettingSource::Config(_)) && e.value.as_bool() == Some(true)
            })
        })
        .map(|(cap, _)| cap)
        .collect();
        if caps.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.now();
        let by = set_by(ctx);
        self.with_tx(|tx| {
            let off = web_off_people_in(tx)?;
            let people = person_keys_in(tx)?;
            let all: Vec<GrantRow> = tx
                .prepare(&format!("SELECT {GRANT_COLS} FROM bot_cap_grants"))?
                .query_map([], row_to_grant)?
                .collect::<rusqlite::Result<_>>()?;
            let mut issued = Vec::new();
            for cap in caps {
                if all.iter().any(|g| g.lists(cap)) {
                    continue;
                }
                let rows =
                    write_everyone(tx, cap, GRANTER_CONFIG, &people, &off, "config", &now, &by)?;
                for g in &rows {
                    self.record_audit(tx, ctx, issue_record(g, now.clone()))?;
                }
                issued.push(rows[0].id.clone());
            }
            Ok(issued)
        })
    }

    /// Every person key of the instance: the owner, then each connected
    /// person, revoked ones included, by public ID (whom a grant to
    /// everyone covers).
    ///
    /// # Errors
    /// The store's.
    pub fn person_keys(&self) -> Result<Vec<String>> {
        self.with_read(person_keys_in)
    }

    /// The grant `id`, if any.
    ///
    /// # Errors
    /// The store's.
    pub fn cap_grant(&self, id: &str) -> Result<Option<GrantRow>> {
        self.with_read(|c| grant_in(c, id))
    }

    /// The grants `filter` selects, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn cap_grants(&self, filter: &GrantFilter) -> Result<Vec<GrantRow>> {
        let rows: Vec<GrantRow> = self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {GRANT_COLS} FROM bot_cap_grants
                      WHERE (?1 IS NULL OR grantee = ?1)
                        AND (?2 = 0 OR status = 'issued')
                      ORDER BY issued_at, id"
            ))?
            .query_map(params![filter.grantee, filter.live], row_to_grant)?
            .collect::<rusqlite::Result<_>>()?)
        })?;
        Ok(rows
            .into_iter()
            .filter(|g| filter.capability.as_deref().is_none_or(|c| g.lists(c)))
            .collect())
    }

    /// Whether grant `g` conveys now: it and every parent are issued and
    /// hold, and, for an admin's grant (to a person or everyone), its
    /// granter is the owner or an admin (spec §4.3).
    ///
    /// # Errors
    /// The store's.
    pub fn grant_conveys(&self, g: &GrantRow) -> Result<bool> {
        let now = self.grants_now();
        self.with_read(|c| {
            let admin = matches!(
                Grantee::parse(&g.grantee),
                Ok(Grantee::Everyone | Grantee::Person(_))
            );
            Ok(chain_holds(c, g, now)? && (!admin || has_authority(c, &g.granter)?))
        })
    }

    /// What `person` holds of each capability ([`CAPABILITIES`], every one
    /// a key): a live admin grant to them that conveys it now (its granter
    /// still an admin), and their live grant of it to their runs.
    ///
    /// # Errors
    /// The store's.
    pub fn person_caps(&self, person: &str) -> Result<BTreeMap<String, PersonCap>> {
        let now = self.grants_now();
        self.with_read(|c| person_caps_in(c, person, now))
    }

    /// Issues to `person`, who joined just now, the child of each issued
    /// selector to everyone they have no child of, and the runs grant of
    /// each such capability they hold no live one of, on `tx`, as `ctx`: a
    /// person row made or made a person again ([`Store::create_invite`], a
    /// browser's invite, a GitHub sign-up) calls it in its own transaction.
    /// A child they revoked stays revoked.
    ///
    /// # Errors
    /// The store's.
    pub(crate) fn issue_joined_grants_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        person: &str,
    ) -> Result<()> {
        let to = Grantee::Person(person.to_string()).as_text();
        let roots: Vec<GrantRow> = tx
            .prepare_cached(&format!(
                "SELECT {GRANT_COLS} FROM bot_cap_grants g
                  WHERE grantee = ?1 AND status = 'issued'
                    AND NOT EXISTS (SELECT 1 FROM bot_cap_grants c
                                     WHERE c.parent = g.id AND c.grantee = ?2)
                  ORDER BY issued_at, id"
            ))?
            .query_map(params![EVERYONE, to], row_to_grant)?
            .collect::<rusqlite::Result<_>>()?;
        let now = self.now();
        let by = set_by(ctx);
        for root in roots {
            let child = insert_grant(
                tx,
                NewGrant {
                    id: None,
                    access: root.access.clone(),
                    granter: &root.granter,
                    grantee: &to,
                    parent: Some(&root.id),
                    conditions: json!({}),
                    event: "admin",
                },
                &now,
                &by,
            )?;
            self.record_audit(tx, ctx, issue_record(&child, now.clone()))?;
            if let Some(cap) = root.capability()
                && let Some(runs) = issue_runs_if_none(tx, cap, person, "bot", &now, &by)?
            {
                self.record_audit(tx, ctx, issue_record(&runs, now.clone()))?;
            }
        }
        Ok(())
    }
}

/// A page's share, as its grant row (spec §18.4): written in the share's
/// Spend transaction ([`crate::bot_web::Spend::grant`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareGrant {
    /// The new capability's ID: its macaroon is the row's token.
    pub id: String,
    /// The run that shares it, its granter and grantee.
    pub run: String,
    /// The sharer's grant: the parent page's share, or for a share of a
    /// session's root the person's runs grant of the session's open action.
    pub parent: String,
    /// The session actions it conveys.
    pub actions: Vec<String>,
    /// The origins a `navigate` of it may reach, when it is bounded.
    pub origins: Option<Vec<String>>,
    /// When it expires (RFC 3339), if it does.
    pub expires_at: Option<String>,
}

/// What a call's Spend writes of the grants: a share's row, or the
/// revoke of a share and every grant under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantWrite {
    Share(ShareGrant),
    Revoke(String),
}

/// Whether `g` is a share's or an export's row.
fn is_share(g: &GrantRow) -> bool {
    g.event == "share"
}

impl Store {
    /// Writes `w` on `tx` as `ctx`: a share's row, recorded as
    /// `grant.issue` (its parent must be issued), or the revoke of a share
    /// of `run` and every grant under it, reason `granter`.
    ///
    /// # Errors
    /// `not_held` for a share whose parent is not issued, or a revoke of
    /// a grant that is not a share of `run`; the store's.
    pub(crate) fn write_grant_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        run: &str,
        w: &GrantWrite,
    ) -> Result<()> {
        let holder = Grantee::Run(run.to_string()).as_text();
        let not_held = |why: &str| CoreError::invalid("not_held", why.to_string());
        match w {
            GrantWrite::Share(sh) => {
                if sh.run != run {
                    return Err(not_held("a run shares only within its own holder"));
                }
                if !grant_in(tx, &sh.parent)?.is_some_and(|p| p.live()) {
                    return Err(not_held("the sharer's grant is not issued"));
                }
                let mut conditions = json!({});
                if let Some(t) = &sh.expires_at {
                    conditions["expires_at"] = json!(t);
                }
                let now = self.now();
                let g = insert_grant(
                    tx,
                    NewGrant {
                        id: Some(sh.id.clone()),
                        access: vec![Access {
                            kind: TYPE_BROWSER.to_string(),
                            actions: sh.actions.clone(),
                            locations: sh.origins.clone(),
                        }],
                        granter: &holder,
                        grantee: &holder,
                        parent: Some(&sh.parent),
                        conditions,
                        event: "share",
                    },
                    &now,
                    &set_by(ctx),
                )?;
                self.record_audit(tx, ctx, issue_record(&g, now))?;
            }
            GrantWrite::Revoke(id) => {
                let ok = grant_in(tx, id)?.is_some_and(|g| is_share(&g) && g.grantee == holder);
                if !ok {
                    return Err(not_held("only a share of the run itself is revoked here"));
                }
                self.revoke_grant_in(tx, ctx, id, RevokeReason::Granter)?;
            }
        }
        Ok(())
    }

    /// Whether the share `id` conveys `action` of `clax-browser` (none for
    /// a call that needs no action) at `location` to run `run` at `now`
    /// (spec §18.4): its row is a share to the run, it and every grant of
    /// its chain are issued and hold, and each lists the action.
    ///
    /// # Errors
    /// The store's.
    pub fn share_conveys(
        &self,
        run: &str,
        id: &str,
        action: Option<&str>,
        location: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        self.conveys_to(&Grantee::Run(run.to_string()), id, action, location, now)
    }

    /// Whether the export `id` conveys `action` of `clax-browser` at
    /// `location` to machine grant `grant` at `now` (spec §18.5): as
    /// [`Store::share_conveys`] for a share, its grantee the grant.
    ///
    /// # Errors
    /// The store's.
    pub fn export_conveys(
        &self,
        grant: &str,
        id: &str,
        action: &str,
        location: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        self.conveys_to(
            &Grantee::Grant(grant.to_string()),
            id,
            Some(action),
            location,
            now,
        )
    }

    fn conveys_to(
        &self,
        grantee: &Grantee,
        id: &str,
        action: Option<&str>,
        location: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let holder = grantee.as_text();
        self.with_read(|c| {
            let Some(g) = grant_in(c, id)? else {
                return Ok(false);
            };
            if !is_share(&g) || g.grantee != holder || !chain_holds(c, &g, now)? {
                return Ok(false);
            }
            let Some(action) = action else {
                return Ok(true);
            };
            let mut at = Some(g);
            while let Some(row) = at {
                if !allows(&row.access, TYPE_BROWSER, action, location) {
                    return Ok(false);
                }
                at = match &row.parent {
                    Some(p) => grant_in(c, p)?,
                    None => None,
                };
            }
            Ok(true)
        })
    }

    /// The shares of run `run` whose grant and every grant of its chain are
    /// issued, their expiry not read (spec §18.7).
    ///
    /// # Errors
    /// The store's.
    pub fn live_shares(&self, run: &str) -> Result<BTreeSet<String>> {
        let holder = Grantee::Run(run.to_string()).as_text();
        self.with_read(|c| {
            let mut out = BTreeSet::new();
            for g in issued_to(c, &holder)? {
                if is_share(&g) && chain_issued(c, &g)? {
                    out.insert(g.id);
                }
            }
            Ok(out)
        })
    }

    /// Every issued share and export, oldest first (the admin's page).
    ///
    /// # Errors
    /// The store's.
    pub fn live_share_grants(&self) -> Result<Vec<GrantRow>> {
        Ok(self
            .cap_grants(&GrantFilter {
                live: true,
                ..GrantFilter::default()
            })?
            .into_iter()
            .filter(is_share)
            .collect())
    }
}

/// A page of a run's state the person exports to one of their machine
/// grants (spec §18.5, option A), as its grant row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportGrant {
    /// The new capability's ID: its macaroon is the row's token.
    pub id: String,
    /// The person who exports it, its granter.
    pub person: String,
    /// The machine grant it is to.
    pub grant: String,
    /// The page's grant: its share, or the person's runs grant of the
    /// session's open action.
    pub parent: String,
    /// The session actions it conveys.
    pub actions: Vec<String>,
    /// The origins a `navigate` of it may reach, when it is bounded.
    pub origins: Option<Vec<String>>,
    /// When it expires (RFC 3339), if it does.
    pub expires_at: Option<String>,
    /// Its capability without the signature (the browser's
    /// `Cap::unsigned`): what [`Store::export_token`] releases to the
    /// person, signed again with the instance key.
    pub token: String,
}

impl Store {
    /// Issues export `e` as `ctx`, recorded as `grant.issue` (event
    /// `share`): granter the person, grantee `grant:<ID>`, parent the
    /// page's grant. Whether the machine grant is the person's is the
    /// caller's to check.
    ///
    /// # Errors
    /// `not_held` when the page's grant is not issued, or is not the
    /// person's (a share of a run of theirs, or their own runs grant);
    /// the store's.
    pub fn issue_export(&self, ctx: &AuditCtx, e: &ExportGrant) -> Result<GrantRow> {
        let now = self.now();
        self.with_tx(|tx| {
            let parent = grant_in(tx, &e.parent)?
                .filter(GrantRow::live)
                .ok_or_else(|| CoreError::invalid("not_held", "the page's grant is not issued"))?;
            let runs = Grantee::RunsOf(e.person.clone()).as_text();
            let theirs = chain_issued(tx, &parent)? && root_of(tx, &parent)?.grantee == runs;
            if !theirs {
                return Err(CoreError::invalid(
                    "not_held",
                    "only a page of the person's own runs is exported",
                ));
            }
            let mut conditions = json!({});
            if let Some(t) = &e.expires_at {
                conditions["expires_at"] = json!(t);
            }
            let to = Grantee::Grant(e.grant.clone()).as_text();
            let g = insert_grant(
                tx,
                NewGrant {
                    id: Some(e.id.clone()),
                    access: vec![Access {
                        kind: TYPE_BROWSER.to_string(),
                        actions: e.actions.clone(),
                        locations: e.origins.clone(),
                    }],
                    granter: &e.person,
                    grantee: &to,
                    parent: Some(&e.parent),
                    conditions,
                    event: "share",
                },
                &now,
                &set_by(ctx),
            )?;
            tx.execute(
                "UPDATE bot_cap_grants SET token_json = ?2 WHERE id = ?1",
                params![g.id, e.token],
            )?;
            self.record_audit(tx, ctx, issue_record(&g, now.clone()))?;
            Ok(g)
        })
    }

    /// The capability of export `id`, unsigned, when it is an issued export
    /// `person` made and its chain is issued; else `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn export_token(&self, person: &str, id: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            let Some(g) = grant_in(c, id)? else {
                return Ok(None);
            };
            if !is_share(&g)
                || g.granter != person
                || !g.grantee.starts_with("grant:")
                || !chain_issued(c, &g)?
            {
                return Ok(None);
            }
            Ok(c.query_row(
                "SELECT token_json FROM bot_cap_grants WHERE id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )?)
        })
    }

    /// The capability, unsigned, of every export whose grant and every
    /// grant of its chain are issued, its expiry not read (spec §18.7).
    ///
    /// # Errors
    /// The store's.
    pub fn live_export_tokens(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let rows: Vec<(String, String)> = c
                .prepare_cached(
                    "SELECT id, token_json FROM bot_cap_grants
                      WHERE status = 'issued' AND event = 'share'
                        AND grantee LIKE 'grant:%' AND token_json IS NOT NULL
                      ORDER BY issued_at, id",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut out = Vec::new();
            for (id, token) in rows {
                if let Some(g) = grant_in(c, &id)?
                    && chain_issued(c, &g)?
                {
                    out.push(token);
                }
            }
            Ok(out)
        })
    }

    /// The issued exports `person` made, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn exports_of(&self, person: &str) -> Result<Vec<GrantRow>> {
        Ok(self
            .live_share_grants()?
            .into_iter()
            .filter(|g| g.granter == person && g.grantee.starts_with("grant:"))
            .collect())
    }
}

/// The root of `g`'s chain, on `c`: the grant with no parent it is under.
fn root_of(c: &Connection, g: &GrantRow) -> Result<GrantRow> {
    let mut seen = BTreeSet::new();
    let mut at = g.clone();
    while let Some(p) = at.parent.clone() {
        if !seen.insert(p.clone()) {
            break;
        }
        match grant_in(c, &p)? {
            Some(parent) => at = parent,
            None => break,
        }
    }
    Ok(at)
}

/// Whether `g` and every parent up to its root are issued, on `c`, their
/// conditions not read.
fn chain_issued(c: &Connection, g: &GrantRow) -> Result<bool> {
    let mut seen = BTreeSet::new();
    let mut at = Some(g.clone());
    while let Some(row) = at {
        if !row.live() || !seen.insert(row.id.clone()) {
            return Ok(false);
        }
        at = match &row.parent {
            Some(p) => match grant_in(c, p)? {
                Some(parent) => Some(parent),
                None => return Ok(false),
            },
            None => None,
        };
    }
    Ok(true)
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::roles::tests::person;
    use crate::test_util::{DAEMON, store};

    fn admin_everyone(st: &Store, cap: &str) -> GrantRow {
        st.issue_cap_grant(DAEMON, cap, OWNER, EVERYONE).unwrap()
    }

    fn to_person(st: &Store, cap: &str, granter: &str, who: &str) -> GrantRow {
        st.issue_cap_grant(DAEMON, cap, granter, &Grantee::Person(who.into()).as_text())
            .unwrap()
    }

    fn runs(person: &str) -> String {
        Grantee::RunsOf(person.to_string()).as_text()
    }

    fn events(st: &Store, kind: &str) -> Vec<serde_json::Value> {
        st.events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
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
            Grantee::Run("r1".into()),
            Grantee::Grant("g1".into()),
        ] {
            assert_eq!(Grantee::parse(&g.as_text()).unwrap(), g);
        }
        for bad in [
            "",
            "person:",
            "runs_of",
            "team:x",
            "everyone",
            "people_of:x",
        ] {
            assert!(Grantee::parse(bad).is_err(), "{bad}");
        }
    }

    /// A grant's access is RFC 9635 objects: an open action carries the
    /// session's actions beside it, and an access without locations
    /// applies everywhere.
    #[test]
    fn a_grant_carries_its_capabilitys_access_objects() {
        let web = access_of("clax-web:fetch").unwrap();
        assert_eq!(
            serde_json::to_value(&web).unwrap(),
            json!([{"type": "clax-web", "actions": ["fetch"]}])
        );
        let url = access_of("clax-browser:open-url").unwrap();
        assert_eq!(
            url[0].actions,
            ["open-url", "read", "act", "navigate", "record", "close"]
        );
        assert!(allows(&url, TYPE_BROWSER, "act", Some("https://a.example")));
        assert!(!allows(&url, TYPE_BROWSER, "open-artifact", None));
        assert!(!allows(&url, TYPE_WEB, "fetch", None));
        let bounded = Access {
            kind: TYPE_BROWSER.into(),
            actions: vec!["navigate".into()],
            locations: Some(vec!["https://a.example".into()]),
        };
        assert!(bounded.allows(TYPE_BROWSER, "navigate", Some("https://a.example")));
        assert!(!bounded.allows(TYPE_BROWSER, "navigate", Some("https://b.example")));
        assert!(bounded.allows(TYPE_BROWSER, "navigate", None));
        assert_eq!(
            code(&access_of("clax-web:crawl").unwrap_err()),
            "unknown_capability"
        );
    }

    #[test]
    fn a_grant_to_everyone_is_a_selector_with_a_child_per_person_and_a_new_one() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "clax-browser:open-content");
        assert_eq!(root.grantee, EVERYONE);
        assert_eq!(
            root.granter, OWNER,
            "the admin who issued it, not the instance"
        );
        assert_eq!(root.event, "admin");
        for p in [OWNER, sam.as_str()] {
            let caps = st.person_caps(p).unwrap();
            let c = &caps["clax-browser:open-content"];
            let child = st
                .cap_grant(c.granted_by.as_ref().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(child.parent.as_deref(), Some(root.id.as_str()), "{p}");
            assert_eq!(child.grantee, Grantee::Person(p.into()).as_text());
            assert!(c.reaches_runs(), "{p}");
            assert!(!caps["clax-browser:open-url"].reaches_runs(), "{p}");
        }
        let mine = st
            .cap_grants(&GrantFilter {
                grantee: Some(runs(&sam)),
                ..GrantFilter::default()
            })
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].granter, sam, "the person grants to their runs");
        assert_eq!(
            mine[0].parent, None,
            "a runs grant is no child of the admin's"
        );
        assert_eq!(mine[0].event, "bot");
        let mia = person(&st, "Mia", 2);
        assert!(
            st.person_caps(&mia).unwrap()["clax-browser:open-content"].reaches_runs(),
            "a person who joins later gets the child and the runs grant"
        );
    }

    /// An admin's grant conveys while its granter is an admin: removing
    /// the role stops it at once, with no row changed, and the role coming
    /// back makes it convey again.
    #[test]
    fn an_admin_who_loses_the_role_stops_conveying() {
        let (_d, st) = store();
        let ana = person(&st, "Ana", 1);
        let sam = person(&st, "Sam", 2);
        st.set_admin(DAEMON, &ana, true).unwrap();
        let g = to_person(&st, "clax-web:search", &ana, &sam);
        assert_eq!(g.granter, ana);
        assert!(st.person_caps(&sam).unwrap()["clax-web:search"].reaches_runs());
        assert!(st.grant_conveys(&g).unwrap());
        st.set_admin(DAEMON, &ana, false).unwrap();
        let held = &st.person_caps(&sam).unwrap()["clax-web:search"];
        assert_eq!(held.granted_by, None, "no longer conveyed");
        assert!(held.runs_grant.is_some(), "the person's own grant stands");
        assert!(!st.grant_conveys(&g).unwrap());
        assert!(
            st.cap_grant(&g.id).unwrap().unwrap().live(),
            "no row changed"
        );
        let e = st
            .issue_cap_grant(DAEMON, "clax-web:fetch", &ana, EVERYONE)
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "a member grants nothing");
        st.set_admin(DAEMON, &ana, true).unwrap();
        assert!(st.person_caps(&sam).unwrap()["clax-web:search"].reaches_runs());
    }

    #[test]
    fn revoking_a_grant_ends_its_children_transitively() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "clax-web:fetch");
        admin_everyone(&st, "clax-web:search");
        let ended = st
            .revoke_cap_grant(DAEMON, &root.id, RevokeReason::Granter)
            .unwrap();
        assert_eq!(ended.len(), 3, "the selector and two children: {ended:?}");
        assert_eq!(ended[0], root.id);
        for p in [OWNER, sam.as_str()] {
            let caps = st.person_caps(p).unwrap();
            assert_eq!(caps["clax-web:fetch"].granted_by, None, "{p}");
            assert!(caps["clax-web:search"].reaches_runs(), "{p}");
        }
        let gone = st.cap_grant(&ended[1]).unwrap().unwrap();
        assert_eq!(gone.revoke_reason.as_deref(), Some("parent_ended"));
        assert_eq!(
            st.cap_grant(&root.id)
                .unwrap()
                .unwrap()
                .revoke_reason
                .as_deref(),
            Some("granter")
        );
        assert!(
            st.revoke_cap_grant(DAEMON, &root.id, RevokeReason::Granter)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            st.revoke_cap_grant(DAEMON, "nope", RevokeReason::Owner),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn a_person_revokes_and_reissues_only_their_own() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        admin_everyone(&st, "clax-browser:open-artifact");
        let cap = "clax-browser:open-artifact";
        let mine = st.person_caps(&sam).unwrap()[cap]
            .runs_grant
            .clone()
            .unwrap();
        assert_eq!(
            st.revoke_cap_grant(DAEMON, &mine, RevokeReason::Controller)
                .unwrap(),
            [mine.as_str()]
        );
        assert!(!st.person_caps(&sam).unwrap()[cap].reaches_runs());
        assert!(
            st.person_caps(OWNER).unwrap()[cap].reaches_runs(),
            "the owner's is their own"
        );
        let again = st.issue_cap_grant(DAEMON, cap, &sam, &runs(&sam)).unwrap();
        assert_eq!(again.granter, sam);
        assert!(st.person_caps(&sam).unwrap()[cap].reaches_runs());
        let e = st
            .issue_cap_grant(DAEMON, cap, &sam, &runs(&sam))
            .unwrap_err();
        assert_eq!(code(&e), "grant_exists");
        let e = st
            .issue_cap_grant(DAEMON, cap, &sam, &runs(OWNER))
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "not another person's runs");
    }

    #[test]
    fn a_person_cannot_issue_what_they_do_not_hold() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let cap = "clax-browser:open-url";
        let e = st
            .issue_cap_grant(DAEMON, cap, &sam, &runs(&sam))
            .unwrap_err();
        assert_eq!(code(&e), "not_held");
        let mia = person(&st, "Mia", 2);
        let to_mia = to_person(&st, cap, OWNER, &mia);
        assert!(st.person_caps(&mia).unwrap()[cap].reaches_runs());
        let e = st
            .issue_cap_grant(DAEMON, cap, &sam, &runs(&sam))
            .unwrap_err();
        assert_eq!(code(&e), "not_held", "another person's grant is not theirs");
        let e = st
            .issue_cap_grant(DAEMON, cap, OWNER, &Grantee::Person(mia.clone()).as_text())
            .unwrap_err();
        assert_eq!(code(&e), "grant_exists");
        st.revoke_cap_grant(DAEMON, &to_mia.id, RevokeReason::Granter)
            .unwrap();
        let held = &st.person_caps(&mia).unwrap()[cap];
        assert_eq!(held.granted_by, None);
        let e = st.issue_cap_grant(DAEMON, cap, &sam, EVERYONE).unwrap_err();
        assert_eq!(code(&e), "not_held", "only an admin grants to people");
        let e = st
            .issue_cap_grant(DAEMON, "clax-web:crawl", OWNER, EVERYONE)
            .unwrap_err();
        assert_eq!(code(&e), "unknown_capability");
        let e = st
            .issue_cap_grant(DAEMON, cap, OWNER, "run:r1")
            .unwrap_err();
        assert_eq!(code(&e), "invalid_grantee");
    }

    #[test]
    fn every_issue_and_revoke_is_one_grant_record() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let root = admin_everyone(&st, "clax-web:search");
        let issued = events(&st, "grant.issue");
        // The selector, a child and a runs grant for each of two people.
        assert_eq!(issued.len(), 5, "{issued:?}");
        assert_eq!(issued[0]["grant_id"], root.id.as_str());
        assert_eq!(issued[0]["grantee"], EVERYONE);
        assert_eq!(issued[0]["granter"], OWNER);
        assert_eq!(issued[0]["event"], "admin");
        assert_eq!(
            issued[0]["access"],
            json!([{"type": "clax-web", "actions": ["search"]}])
        );
        assert!(issued[0].get("parent").is_none());
        assert!(
            issued
                .iter()
                .any(|e| e["grantee"] == runs(&sam).as_str() && e["event"] == "bot")
        );
        assert!(
            issued
                .iter()
                .any(|e| e["parent"] == root.id.as_str() && e["event"] == "admin")
        );
        st.revoke_cap_grant(DAEMON, &root.id, RevokeReason::Owner)
            .unwrap();
        let revoked = events(&st, "grant.revoke");
        assert_eq!(revoked.len(), 3, "the selector and its two children");
        assert_eq!(revoked[0]["grant_id"], root.id.as_str());
        assert_eq!(revoked[0]["reason"], "owner");
        assert!(revoked[1..].iter().all(|e| e["reason"] == "parent_ended"));
        assert!(events(&st, "bot.grant").is_empty(), "no kind of its own");
    }

    /// A daemon whose web switches are on only in config.toml gets the
    /// grants to everyone migration 68 would have written, once: a grant
    /// the admin revokes is not written again, a stored switch and a switch
    /// off write none, and nothing of the browser is ever granted.
    #[test]
    fn a_config_web_switch_grants_everyone_once() {
        use crate::bot_settings::{Overrides, Setting};
        let (_d, st) = store();
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        st.set_bot_settings(DAEMON, &[(Setting::WebSearchEnabled, Some(json!(true)))])
            .unwrap();
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        let mut ov = Overrides::default();
        ov.set(Setting::WebFetchEnabled, &json!(true), "file")
            .unwrap();
        ov.set(Setting::WebSearchEnabled, &json!(false), "file")
            .unwrap();
        ov.set(Setting::BrowserOpenUrlEnabled, &json!(true), "file")
            .unwrap();
        st.set_bot_overrides(ov);
        let issued = st.grant_web_from_config(DAEMON).unwrap();
        assert_eq!(issued.len(), 1);
        let g = st
            .cap_grants(&GrantFilter {
                grantee: Some(EVERYONE.into()),
                ..GrantFilter::default()
            })
            .unwrap();
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(
            (
                g[0].capability(),
                g[0].granter.as_str(),
                g[0].event.as_str()
            ),
            (Some("clax-web:fetch"), GRANTER_CONFIG, "config")
        );
        assert!(st.person_caps(OWNER).unwrap()["clax-web:fetch"].reaches_runs());
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        st.revoke_cap_grant(DAEMON, &g[0].id, RevokeReason::Owner)
            .unwrap();
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        assert!(!st.person_caps(OWNER).unwrap()["clax-web:fetch"].reaches_runs());
        for cap in BROWSER_CAPABILITIES {
            assert!(
                st.cap_grants(&GrantFilter {
                    capability: Some(cap.into()),
                    ..GrantFilter::default()
                })
                .unwrap()
                .is_empty(),
                "{cap}"
            );
        }
    }

    fn config_turns_fetch_on(st: &Store) {
        use crate::bot_settings::{Overrides, Setting};
        let mut ov = Overrides::default();
        ov.set(Setting::WebFetchEnabled, &json!(true), "file")
            .unwrap();
        st.set_bot_overrides(ov);
    }

    /// The config grant skips a person whose web was off on main, as
    /// migration 68 does.
    #[test]
    fn a_config_web_grant_skips_a_person_whose_web_was_off() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let mia = person(&st, "Mia", 2);
        st.with_tx(|tx| {
            tx.execute(
                "INSERT INTO bot_person_prefs (person, web, set_at) VALUES (?1, 0, 'x')",
                params![mia],
            )?;
            tx.execute(
                "INSERT INTO bot_person_prefs (person, web, set_at) VALUES (?1, 1, 'x')",
                params![sam],
            )?;
            Ok(())
        })
        .unwrap();
        config_turns_fetch_on(&st);
        assert_eq!(st.grant_web_from_config(DAEMON).unwrap().len(), 1);
        for p in [OWNER, sam.as_str()] {
            assert!(
                st.person_caps(p).unwrap()["clax-web:fetch"].reaches_runs(),
                "{p}"
            );
        }
        let mias = st.person_caps(&mia).unwrap()["clax-web:fetch"].clone();
        assert!(mias.granted_by.is_some(), "the selector's child covers her");
        assert_eq!(mias.runs_grant, None, "her runs are not granted it");
        assert!(
            events(&st, "grant.issue")
                .iter()
                .all(|e| e["grantee"] != runs(&mia).as_str())
        );
    }

    /// Any grant of a web capability, to anyone and live or revoked,
    /// means the config writes none of it.
    #[test]
    fn a_config_web_grant_is_not_written_where_the_capability_was_ever_granted() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let to_sam = to_person(&st, "clax-web:fetch", OWNER, &sam);
        config_turns_fetch_on(&st);
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        st.revoke_cap_grant(DAEMON, &to_sam.id, RevokeReason::Granter)
            .unwrap();
        assert!(st.grant_web_from_config(DAEMON).unwrap().is_empty());
        assert!(!st.person_caps(OWNER).unwrap()["clax-web:fetch"].reaches_runs());
    }

    /// A person holding a capability from two admin grants keeps it in
    /// their runs when either is revoked, whichever was issued first: their
    /// runs grant is no child of either, and conveys what they hold.
    #[test]
    fn revoking_one_of_two_admin_grants_keeps_the_runs_grant() {
        let cap = "clax-browser:open-url";
        for everyone_first in [true, false] {
            let (_d, st) = store();
            let ben = person(&st, "Ben", 1);
            let (first, second) = if everyone_first {
                let e = admin_everyone(&st, cap);
                (e, to_person(&st, cap, OWNER, &ben))
            } else {
                let p = to_person(&st, cap, OWNER, &ben);
                (p, admin_everyone(&st, cap))
            };
            let before = st.person_caps(&ben).unwrap()[cap].clone();
            assert!(before.reaches_runs());
            let n = events(&st, "grant.issue").len();
            st.revoke_cap_grant(DAEMON, &first.id, RevokeReason::Granter)
                .unwrap();
            let after = st.person_caps(&ben).unwrap()[cap].clone();
            assert!(after.reaches_runs(), "everyone first: {everyone_first}");
            assert_eq!(after.runs_grant, before.runs_grant, "the same runs grant");
            let by = st
                .cap_grant(after.granted_by.as_ref().unwrap())
                .unwrap()
                .unwrap();
            assert!(
                by.id == second.id || by.parent.as_deref() == Some(second.id.as_str()),
                "conveyed by the other"
            );
            assert_eq!(events(&st, "grant.issue").len(), n, "nothing reissued");
            if everyone_first {
                assert_eq!(st.person_caps(OWNER).unwrap()[cap].granted_by, None);
            }
        }
    }

    /// A share is a child of the sharer's grant: it conveys only what it
    /// and every grant of its chain list, while each is issued and before
    /// its expiry, and a revoke of its parent ends it (spec §18.4).
    #[test]
    fn a_share_conveys_only_within_its_chain() {
        let (_d, st) = store();
        let cap = "clax-browser:open-artifact";
        admin_everyone(&st, cap);
        let runs_grant = st.person_caps(OWNER).unwrap()[cap]
            .runs_grant
            .clone()
            .unwrap();
        let share = |id: &str, parent: &str, actions: &[&str], expires: Option<&str>| {
            st.with_tx(|tx| {
                st.write_grant_in(
                    tx,
                    DAEMON,
                    "r1",
                    &GrantWrite::Share(ShareGrant {
                        id: id.into(),
                        run: "r1".into(),
                        parent: parent.into(),
                        actions: actions.iter().map(|a| (*a).to_string()).collect(),
                        origins: None,
                        expires_at: expires.map(str::to_string),
                    }),
                )
            })
        };
        share("s1", &runs_grant, &["read", "act"], None).unwrap();
        share("s2", "s1", &["read"], Some("2100-01-01T00:00:00.000Z")).unwrap();
        let now = st.now_utc();
        let conveys =
            |id: &str, action: &str| st.share_conveys("r1", id, Some(action), None, now).unwrap();
        assert!(conveys("s1", "act") && conveys("s2", "read"));
        assert!(!conveys("s2", "act"), "no wider than itself");
        assert!(
            !st.share_conveys("r2", "s1", Some("read"), None, now)
                .unwrap(),
            "another run's"
        );
        let later = chrono::DateTime::parse_from_rfc3339("2100-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(
            !st.share_conveys("r1", "s2", Some("read"), None, later)
                .unwrap(),
            "expired"
        );
        assert_eq!(
            st.live_shares("r1").unwrap(),
            ["s1".to_string(), "s2".to_string()].into()
        );
        assert_eq!(
            code(&share("s3", "nope", &["read"], None).unwrap_err()),
            "not_held"
        );
        st.with_tx(|tx| st.write_grant_in(tx, DAEMON, "r1", &GrantWrite::Revoke("s1".into())))
            .unwrap();
        assert!(!conveys("s2", "read"), "its parent revoked");
        assert!(st.live_shares("r1").unwrap().is_empty());
        let s2 = st.cap_grant("s2").unwrap().unwrap();
        assert_eq!(s2.revoke_reason.as_deref(), Some("parent_ended"));
        // A run revokes only its own share; the runs grant is not one.
        let e = st
            .with_tx(|tx| {
                st.write_grant_in(tx, DAEMON, "r1", &GrantWrite::Revoke(runs_grant.clone()))
            })
            .unwrap_err();
        assert_eq!(code(&e), "not_held");
        let issued = events(&st, "grant.issue");
        assert!(issued.iter().any(|e| e["grant_id"] == "s2"
            && e["event"] == "share"
            && e["expires_at"] == "2100-01-01T00:00:00.000Z"));
    }

    /// An export is a grant row from the person to their machine grant,
    /// under a page of their own runs, listed and revoked like a share.
    #[test]
    fn a_person_exports_a_page_of_their_own_runs() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let cap = "clax-browser:open-content";
        admin_everyone(&st, cap);
        let mine = st.person_caps(&sam).unwrap()[cap]
            .runs_grant
            .clone()
            .unwrap();
        let theirs = st.person_caps(OWNER).unwrap()[cap]
            .runs_grant
            .clone()
            .unwrap();
        let export = |id: &str, parent: &str| {
            st.issue_export(
                DAEMON,
                &ExportGrant {
                    id: id.into(),
                    person: sam.clone(),
                    grant: "g_sams".into(),
                    parent: parent.into(),
                    actions: vec!["read".into()],
                    origins: Some(vec!["https://example.com".into()]),
                    expires_at: Some("2100-01-01T00:00:00.000Z".into()),
                    token: format!("{{\"i\":\"{id}\"}}"),
                },
            )
        };
        let g = export("e1", &mine).unwrap();
        assert_eq!(
            (g.granter.as_str(), g.grantee.as_str(), g.event.as_str()),
            (sam.as_str(), "grant:g_sams", "share")
        );
        assert_eq!(code(&export("e2", &theirs).unwrap_err()), "not_held");
        assert_eq!(st.exports_of(&sam).unwrap().len(), 1);
        // Its token is released to the person who made it alone, and it
        // conveys its actions within its origins to its grant alone.
        let token = Some("{\"i\":\"e1\"}".to_string());
        assert_eq!(st.export_token(&sam, "e1").unwrap(), token);
        assert_eq!(st.export_token(OWNER, "e1").unwrap(), None);
        assert_eq!(st.export_token(&sam, &mine).unwrap(), None, "not an export");
        assert_eq!(
            st.live_export_tokens().unwrap(),
            vec![token.clone().unwrap()]
        );
        let now = st.now_utc();
        let ex = Some("https://example.com");
        assert!(st.export_conveys("g_sams", "e1", "read", ex, now).unwrap());
        assert!(
            st.export_conveys("g_sams", "e1", "read", None, now)
                .unwrap()
        );
        assert!(!st.export_conveys("g_sams", "e1", "act", ex, now).unwrap());
        assert!(
            !st.export_conveys("g_sams", "e1", "read", Some("https://other.example"), now)
                .unwrap()
        );
        assert!(!st.export_conveys("g_other", "e1", "read", ex, now).unwrap());
        let later = chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(
            !st.export_conveys("g_sams", "e1", "read", ex, later)
                .unwrap(),
            "expired"
        );
        st.revoke_cap_grant(DAEMON, "e1", RevokeReason::Granter)
            .unwrap();
        assert!(st.exports_of(&sam).unwrap().is_empty());
        assert_eq!(st.export_token(&sam, "e1").unwrap(), None);
        assert!(st.live_export_tokens().unwrap().is_empty());
        assert!(!st.export_conveys("g_sams", "e1", "read", ex, now).unwrap());
    }
}
