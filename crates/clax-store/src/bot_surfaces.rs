//! Surfaces a host registers (migration 65, spec
//! 2026-11-10-claxbot-surfaces-design, "Registration and authority" and
//! "A host's surface on a daemon").
//!
//! A **definition** is a surface's canonical manifest, kept once for each
//! hash (`bot_surface_defs`). The hash is the SHA-256, in lowercase hex,
//! of the canonical manifest's bytes, and it is the caller's to compute:
//! `clax_surface::canonical_manifest` gives the bytes, which are also the
//! text to store. The store depends on no crate that reads a definition,
//! so it parses none and does not check that a hash is its text's: it
//! holds the text under the hash it is given, and refuses only a hash
//! that is not 64 lowercase hex characters. A run pins the definitions it
//! was made with by hash, so a definition is never changed, and it is
//! kept while a registration, a pending call or a run names it
//! (`prune_surfaces_in`).
//!
//! A **prompt file** is kept the same way (`bot_surface_prompts`): its
//! text once for each SHA-256 of its bytes, which the store checks
//! ([`Store::put_surface_prompt`]). A registration names its prompt file
//! by that hash and a run's binding names the one the run was started
//! with (`crate::bot_runs::StoredBinding::prompt`), so a run keeps the
//! prompt file it was started with whatever its registration has later.
//!
//! A **registration** (`bot_registrations`) is a definition, an optional
//! prompt file and settings, given by a person under a surface's name.
//! Many registrations of one name may be live at once (a host each); a
//! registration is live until it is replaced
//! ([`Store::replace_registration`], or a later registration that names
//! it in [`NewRegistration::replaces`]). The store keeps no lease: when a
//! registration that no longer serves is to end is its caller's rule, from
//! [`Registration::last_served_at`].
//!
//! Every function here takes the time as `now`; none reads the store's
//! clock.

use super::Store;
use chrono::{DateTime, Utc};
use clax_core::audit::sha256_hex;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::time::stamp;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

/// A surface's definition as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SurfaceDef {
    /// The SHA-256 of the canonical manifest's bytes, in lowercase hex.
    pub hash: String,
    /// The canonical manifest, as it was first stored under `hash`.
    pub manifest: String,
    /// When it was last kept: first stored, or given again later
    /// ([`Store::put_surface_def`]). A definition nothing names is kept
    /// [`BOT_DEF_KEEP`] from then. The column is `created_at`.
    pub kept_at: String,
}

/// One registration of a surface.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Registration {
    /// A ULID.
    pub id: String,
    /// The surface's name (`shell`).
    pub name: String,
    /// The hash of its definition ([`SurfaceDef::hash`]).
    pub def_hash: String,
    /// Its prompt file's text, when it has one: the text kept under
    /// [`Registration::prompt_hash`].
    pub prompt: Option<String>,
    /// The SHA-256 of its prompt file's bytes, in lowercase hex, when it
    /// has one ([`Store::surface_prompt`]).
    pub prompt_hash: Option<String>,
    /// Its settings, as the registrant gave them: the store reads none.
    /// `Null` when the row's are no JSON ([`Registration::settings_unread`]).
    pub settings: Value,
    /// Why the settings its row keeps do not read as JSON, when they do
    /// not (a damaged row): the registration is read all the same, and
    /// whoever serves it refuses to, never reading `settings` as none.
    #[serde(skip)]
    pub settings_unread: Option<String>,
    /// The person key of who registered it.
    pub registered_by: String,
    pub created_at: String,
    /// When it last held or ended a `serve` ([`Store::touch_registration`]).
    pub last_served_at: Option<String>,
    /// When it stopped being live; `None` while it is.
    pub replaced_at: Option<String>,
}

impl Registration {
    /// Whether it has not been replaced.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.replaced_at.is_none()
    }
}

/// A registration to make.
#[derive(Clone, Debug)]
pub struct NewRegistration {
    pub name: String,
    /// The hash of a definition already stored ([`Store::put_surface_def`]).
    pub def_hash: String,
    /// Its prompt file's text, when it has one: kept under its SHA-256
    /// in the registration's transaction.
    pub prompt: Option<String>,
    pub settings: Value,
    /// The person key of who registers it.
    pub registered_by: String,
    /// An earlier registration this one takes the place of, replaced in
    /// the same transaction.
    pub replaces: Option<String>,
}

const REG_COLS: &str = "id, name, def_hash,
    (SELECT text FROM bot_surface_prompts WHERE hash = bot_registrations.prompt_hash),
    settings_json, registered_by, created_at, last_served_at, replaced_at, prompt_hash";

fn row_to_registration(r: &Row<'_>) -> rusqlite::Result<Registration> {
    let settings: String = r.get(4)?;
    let (settings, settings_unread) = match serde_json::from_str(&settings) {
        Ok(settings) => (settings, None),
        Err(e) => (Value::Null, Some(e.to_string())),
    };
    Ok(Registration {
        id: r.get(0)?,
        name: r.get(1)?,
        def_hash: r.get(2)?,
        prompt: r.get(3)?,
        settings,
        settings_unread,
        registered_by: r.get(5)?,
        created_at: r.get(6)?,
        last_served_at: r.get(7)?,
        replaced_at: r.get(8)?,
        prompt_hash: r.get(9)?,
    })
}

/// How long a registration that started no run and has nothing waiting
/// is kept after it was last served (or made, when it never was): its
/// host may still go on as it until then.
pub const BOT_REGISTRATION_KEEP: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 3600);

/// How long a definition or a prompt file that nothing names is kept:
/// longer than the moment between its being stored and its
/// registration, call or run being written.
pub const BOT_DEF_KEEP: std::time::Duration = std::time::Duration::from_secs(3600);

/// The most rows of each kind (registrations, definitions, prompt files)
/// one prune pass removes ([`Store::prune_ended_runs`]).
pub const PRUNE_SURFACE_ROWS: usize = 128;

/// What [`prune_surfaces_in`] removed.
#[derive(Debug, Default)]
pub(crate) struct SurfacesPruned {
    pub registrations: usize,
    pub defs: usize,
    pub prompts: usize,
    /// Rows of some kind were left that nothing needs: more than
    /// [`PRUNE_SURFACE_ROWS`] were due.
    pub more: bool,
}

/// Removes, on `tx`, the registrations, definitions and prompt files
/// nothing needs, [`PRUNE_SURFACE_ROWS`] of each kind at the most, the
/// oldest first:
///
/// - a registration last served (or, never served, made) before
///   `registrations_before`, that started no run the store still has,
///   and has no pending call, no run's end and no join kept for it. One
///   that started a run is kept for as long as the run is, so the run's
///   `started_by` still says what surface it was; one that set joins,
///   for as long as they are kept (`crate::bot_joins`);
/// - a definition last kept before `defs_before` ([`SurfaceDef::kept_at`])
///   that no registration, no pending call and no run's bindings name (a
///   host's run's, or a run of Clax's own that a join bound it into: any
///   run whose bindings are of version 2);
/// - a prompt file last kept before `defs_before` that no registration
///   and no such run's bindings name.
///
/// What the runs' bindings name is read once, in one pass over those
/// runs and only when a definition or a prompt file is a candidate: each
/// binding's `hash` and `prompt`, and for a run whose bindings do not
/// read as JSON (a damaged row), any candidate whose hash its text holds
/// is kept. A definition or prompt file a run names is passed over, so
/// it never fills the bound and holds back one nothing names.
pub(crate) fn prune_surfaces_in(
    tx: &Connection,
    registrations_before: &str,
    defs_before: &str,
) -> Result<SurfacesPruned> {
    let limit = i64::try_from(PRUNE_SURFACE_ROWS).unwrap_or(i64::MAX);
    let mut pruned = SurfacesPruned::default();
    let registrations: Vec<String> = tx
        .prepare(
            "SELECT id FROM bot_registrations
              WHERE COALESCE(last_served_at, created_at) < ?1
                AND id NOT IN (SELECT started_by FROM bot_runs WHERE started_by IS NOT NULL)
                AND id NOT IN (SELECT registration_id FROM bot_pending_calls)
                AND id NOT IN (SELECT registration_id FROM bot_run_ends)
                AND id NOT IN (SELECT registration_id FROM bot_joins)
              ORDER BY COALESCE(last_served_at, created_at), rowid LIMIT ?2 + 1",
        )?
        .query_map(params![registrations_before, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    pruned.more |= registrations.len() > PRUNE_SURFACE_ROWS;
    for id in registrations.iter().take(PRUNE_SURFACE_ROWS) {
        pruned.registrations +=
            tx.execute("DELETE FROM bot_registrations WHERE id = ?1", params![id])?;
    }
    let defs: Vec<String> = tx
        .prepare(
            "SELECT hash FROM bot_surface_defs
              WHERE created_at < ?1
                AND hash NOT IN (SELECT def_hash FROM bot_registrations)
                AND hash NOT IN (SELECT def_hash FROM bot_pending_calls)
              ORDER BY created_at, hash",
        )?
        .query_map(params![defs_before], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let prompts: Vec<String> = tx
        .prepare(
            "SELECT hash FROM bot_surface_prompts
              WHERE kept_at < ?1
                AND hash NOT IN
                    (SELECT prompt_hash FROM bot_registrations WHERE prompt_hash IS NOT NULL)
              ORDER BY kept_at, hash",
        )?
        .query_map(params![defs_before], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if defs.is_empty() && prompts.is_empty() {
        return Ok(pruned);
    }
    let named = RunsName::read(tx)?;
    let mut prune = |candidates: Vec<String>, member: &HashSet<String>, delete: &str| {
        let unnamed: Vec<String> = candidates
            .into_iter()
            .filter(|h| !member.contains(h) && !named.unread.iter().any(|t| t.contains(h.as_str())))
            .collect();
        pruned.more |= unnamed.len() > PRUNE_SURFACE_ROWS;
        let mut n = 0;
        for hash in unnamed.iter().take(PRUNE_SURFACE_ROWS) {
            n += tx.execute(delete, params![hash])?;
        }
        Ok::<_, CoreError>(n)
    };
    let defs = prune(
        defs,
        &named.defs,
        "DELETE FROM bot_surface_defs WHERE hash = ?1",
    )?;
    let prompts = prune(
        prompts,
        &named.prompts,
        "DELETE FROM bot_surface_prompts WHERE hash = ?1",
    )?;
    pruned.defs = defs;
    pruned.prompts = prompts;
    Ok(pruned)
}

/// What the bindings of the runs a surface's rows are kept for name: a
/// host's runs (`started_by`) and the runs of Clax's own whose bindings
/// are of version 2.
struct RunsName {
    /// Each binding's `hash`.
    defs: HashSet<String>,
    /// Each binding's `prompt`.
    prompts: HashSet<String>,
    /// The text of each such run's bindings that does not read as JSON.
    unread: Vec<String>,
}

impl RunsName {
    fn read(c: &Connection) -> Result<RunsName> {
        const RUNS: &str = "(started_by IS NOT NULL OR bindings_json LIKE '{\"v\":2,%')";
        let mut named = RunsName {
            defs: HashSet::new(),
            prompts: HashSet::new(),
            unread: Vec::new(),
        };
        let mut q = c.prepare(&format!(
            "SELECT DISTINCT
                    CASE WHEN json_type(b.value, '$.hash') = 'text'
                         THEN json_extract(b.value, '$.hash') END,
                    CASE WHEN json_type(b.value, '$.prompt') = 'text'
                         THEN json_extract(b.value, '$.prompt') END
               FROM bot_runs r, json_each(r.bindings_json, '$.bindings') b
              WHERE {RUNS} AND json_valid(r.bindings_json) AND b.type = 'object'"
        ))?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })?;
        for row in rows {
            let (def, prompt) = row?;
            named.defs.extend(def);
            named.prompts.extend(prompt);
        }
        named.unread = c
            .prepare(&format!(
                "SELECT bindings_json FROM bot_runs
                  WHERE {RUNS} AND bindings_json IS NOT NULL AND NOT json_valid(bindings_json)"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(named)
    }
}

/// Keeps `text` under `hash` on `c`, last kept at `at`: the text as it
/// was first stored, and the later of the two times.
fn keep_prompt_in(c: &Connection, hash: &str, text: &str, at: &str) -> Result<bool> {
    let new = c.execute(
        "INSERT OR IGNORE INTO bot_surface_prompts (hash, text, kept_at) VALUES (?1, ?2, ?3)",
        params![hash, text, at],
    )? > 0;
    if !new {
        c.execute(
            "UPDATE bot_surface_prompts SET kept_at = ?2 WHERE hash = ?1 AND kept_at < ?2",
            params![hash, at],
        )?;
    }
    Ok(new)
}

/// Registration `id` on `c`, or `None`.
pub(crate) fn registration_in(c: &Connection, id: &str) -> Result<Option<Registration>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {REG_COLS} FROM bot_registrations WHERE id = ?1"
    ))?
    .query_row(params![id], row_to_registration)
    .optional()?)
}

/// Whether `hash` is 64 lowercase hex characters.
fn is_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The registrations `filter` selects by its one argument, oldest first,
/// the replaced ones among them only with `all`.
fn registrations_where(
    c: &Connection,
    filter: &str,
    arg: &str,
    all: bool,
) -> Result<Vec<Registration>> {
    // Oldest first: rows made at one instant in the order they were
    // made (`rowid`, which only grows past every row there is), not in
    // the order of their random IDs.
    let mut q = c.prepare_cached(&format!(
        "SELECT {REG_COLS} FROM bot_registrations
          WHERE {filter} AND (?2 OR replaced_at IS NULL) ORDER BY created_at, rowid"
    ))?;
    let rows = q.query_map(params![arg, all], row_to_registration)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl Store {
    /// Keeps `manifest`, a surface's canonical manifest, under `hash`: the
    /// SHA-256 of its bytes in lowercase hex, which the caller computed
    /// (`clax_surface::canonical_manifest`, then SHA-256). Returns whether
    /// it was stored now; `false` when `hash` already held a definition,
    /// which is kept as it was first stored whatever `manifest` is, and
    /// then counts as last kept at `now` when that is later
    /// ([`SurfaceDef::kept_at`]), as a prompt file given again does
    /// ([`Store::put_surface_prompt`]): whoever is about to write a
    /// registration or a call that names a definition gives it first, so
    /// a prune between the two leaves it.
    ///
    /// # Errors
    /// `surface_hash_invalid` for a hash that is not 64 lowercase hex
    /// characters; the store's.
    pub fn put_surface_def(&self, hash: &str, manifest: &str, now: DateTime<Utc>) -> Result<bool> {
        if !is_hash(hash) {
            return Err(CoreError::invalid(
                "surface_hash_invalid",
                format!(
                    "a definition's hash is 64 lowercase hex characters (a SHA-256), not {hash:?}"
                ),
            ));
        }
        let at = stamp(now);
        self.with_tx(|tx| {
            let new = tx.execute(
                "INSERT OR IGNORE INTO bot_surface_defs (hash, manifest, created_at)
                 VALUES (?1, ?2, ?3)",
                params![hash, manifest, at],
            )? > 0;
            if !new {
                tx.execute(
                    "UPDATE bot_surface_defs SET created_at = ?2 WHERE hash = ?1 AND created_at < ?2",
                    params![hash, at],
                )?;
            }
            Ok(new)
        })
    }

    /// The definition kept under `hash`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn surface_def(&self, hash: &str) -> Result<Option<SurfaceDef>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT hash, manifest, created_at FROM bot_surface_defs WHERE hash = ?1",
            )?
            .query_row(params![hash], |r| {
                Ok(SurfaceDef {
                    hash: r.get(0)?,
                    manifest: r.get(1)?,
                    kept_at: r.get(2)?,
                })
            })
            .optional()?)
        })
    }

    /// Keeps `text`, a prompt file, under `hash`: the SHA-256 of its
    /// bytes in lowercase hex. Returns whether it was stored now; `false`
    /// when `hash` already held it, which then counts as last kept at
    /// `now` when that is later: a prompt file nothing names is kept
    /// [`BOT_DEF_KEEP`] from when it was last given, so whoever is about
    /// to write a run that names it gives it again first.
    ///
    /// # Errors
    /// `surface_hash_invalid` for a hash that is not `text`'s SHA-256 in
    /// lowercase hex; the store's.
    pub fn put_surface_prompt(&self, hash: &str, text: &str, now: DateTime<Utc>) -> Result<bool> {
        if sha256_hex(text.as_bytes()) != hash {
            return Err(CoreError::invalid(
                "surface_hash_invalid",
                format!(
                    "a prompt file is kept under the SHA-256 of its bytes in lowercase hex, \
                     which {hash:?} is not"
                ),
            ));
        }
        let at = stamp(now);
        self.with_tx(|tx| keep_prompt_in(tx, hash, text, &at))
    }

    /// The prompt file's text kept under `hash`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn surface_prompt(&self, hash: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(
                c.prepare_cached("SELECT text FROM bot_surface_prompts WHERE hash = ?1")?
                    .query_row(params![hash], |r| r.get(0))
                    .optional()?,
            )
        })
    }

    /// Makes a live registration from `new` under an ID the store draws
    /// (a ULID), recording `bot.surface.registered {registration, name,
    /// def_hash, registered_by, prompt_sha256?, replaces?}` under `ctx`.
    /// Its prompt file's text, when it has one, is kept under its
    /// SHA-256 ([`Store::surface_prompt`]) in the same transaction. When
    /// `new.replaces` names a live registration, that one is replaced at
    /// `now` in the same transaction; one already replaced stays as it is.
    /// Either way the joins the one it names set are the new one's from
    /// then on (`crate::bot_joins`).
    ///
    /// # Errors
    /// `surface_def_unknown` when no definition is kept under
    /// `new.def_hash`; `registration_invalid` for an empty name or person,
    /// and when `new.replaces` names a registration of another surface's
    /// name (whose joins are that surface's); `NotFound` when
    /// `new.replaces` names no registration; the store's.
    pub fn register_surface(
        &self,
        ctx: &AuditCtx,
        new: NewRegistration,
        now: DateTime<Utc>,
    ) -> Result<Registration> {
        let id = clax_core::new_ulid();
        Ok(self
            .issue_registration(ctx, &id, new, now)?
            .expect("a new ULID is no registration's"))
    }

    /// [`Store::register_surface`] under `id`, an ID its caller drew:
    /// the registration, or `None`, changing and recording nothing, when
    /// the store holds a registration under `id`, live or replaced. A
    /// registration's ID and name are kept for as long as its row is:
    /// while anything names it, and [`BOT_REGISTRATION_KEEP`] after it
    /// last served otherwise (`prune_surfaces_in`). Once it is pruned,
    /// [`Store::registration`] says nothing of the ID and it may be
    /// issued again, under any name, so a caller draws a fresh ID (a
    /// ULID) for each registration it makes.
    ///
    /// # Errors
    /// As [`Store::register_surface`]; `registration_invalid` for an
    /// empty ID too.
    pub fn issue_registration(
        &self,
        ctx: &AuditCtx,
        id: &str,
        new: NewRegistration,
        now: DateTime<Utc>,
    ) -> Result<Option<Registration>> {
        if id.is_empty() || new.name.is_empty() || new.registered_by.is_empty() {
            return Err(CoreError::invalid(
                "registration_invalid",
                "a registration has an ID, a surface's name and the person who registers it",
            ));
        }
        let at = stamp(now);
        self.with_tx(|tx| {
            if registration_in(tx, id)?.is_some() {
                return Ok(None);
            }
            let known: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM bot_surface_defs WHERE hash = ?1",
                    params![new.def_hash],
                    |r| r.get(0),
                )
                .optional()?;
            if known.is_none() {
                return Err(CoreError::invalid(
                    "surface_def_unknown",
                    format!(
                        "no definition is kept under {}; store it before registering it",
                        new.def_hash
                    ),
                ));
            }
            if let Some(old) = &new.replaces {
                let replaced = registration_in(tx, old)?.ok_or(CoreError::NotFound)?;
                if replaced.name != new.name {
                    return Err(CoreError::invalid(
                        "registration_invalid",
                        format!(
                            "registration {old} is of the surface {:?}, which a registration \
                             of {:?} cannot replace",
                            replaced.name, new.name
                        ),
                    ));
                }
                tx.execute(
                    "UPDATE bot_registrations SET replaced_at = ?2
                      WHERE id = ?1 AND replaced_at IS NULL",
                    params![old, at],
                )?;
            }
            let prompt_hash = new.prompt.as_deref().map(|p| sha256_hex(p.as_bytes()));
            if let (Some(hash), Some(text)) = (&prompt_hash, &new.prompt) {
                keep_prompt_in(tx, hash, text, &at)?;
            }
            tx.execute(
                "INSERT INTO bot_registrations (id, name, def_hash, prompt_hash, settings_json,
                    registered_by, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    new.name,
                    new.def_hash,
                    prompt_hash,
                    new.settings.to_string(),
                    new.registered_by,
                    at
                ],
            )?;
            if let Some(old) = &new.replaces {
                crate::bot_joins::repoint_joins_in(tx, old, id)?;
            }
            let mut rec = AuditRecord::new(AuditKind::BotSurfaceRegistered, at.clone())
                .with("registration", id)
                .with("name", new.name.as_str())
                .with("def_hash", new.def_hash.as_str())
                .with("registered_by", new.registered_by.as_str());
            if let Some(hash) = prompt_hash {
                rec = rec.with("prompt_sha256", hash);
            }
            if let Some(old) = &new.replaces {
                rec = rec.with("replaces", old.as_str());
            }
            self.record_audit(tx, ctx, rec)?;
            registration_in(tx, id)
        })
    }

    /// Records, at `now`, what registration `id` was gone on with: the
    /// definition kept under `def_hash`, the prompt file `prompt` and
    /// the settings `settings`. When any differs from what its row
    /// holds, the row is given them (the prompt file's text kept under
    /// its SHA-256 in the same transaction) and the change is recorded as
    /// `bot.surface.registered` with `changed: true`, the definition's
    /// hash and the prompt file's SHA-256 (`prompt_sha256`, when it has
    /// one), under `ctx`. Returns whether anything changed.
    ///
    /// # Errors
    /// `NotFound` for no such registration; `surface_def_unknown` when
    /// no definition is kept under `def_hash`; the store's.
    pub fn change_registration(
        &self,
        ctx: &AuditCtx,
        id: &str,
        def_hash: &str,
        prompt: Option<&str>,
        settings: &Value,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let at = stamp(now);
        self.with_tx(|tx| {
            let row = registration_in(tx, id)?.ok_or(CoreError::NotFound)?;
            let prompt_hash = prompt.map(|p| sha256_hex(p.as_bytes()));
            if row.def_hash == def_hash
                && row.prompt_hash == prompt_hash
                && row.settings == *settings
            {
                return Ok(false);
            }
            let known: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM bot_surface_defs WHERE hash = ?1",
                    params![def_hash],
                    |r| r.get(0),
                )
                .optional()?;
            if known.is_none() {
                return Err(CoreError::invalid(
                    "surface_def_unknown",
                    format!(
                        "no definition is kept under {def_hash}; store it before registering it"
                    ),
                ));
            }
            if let (Some(hash), Some(text)) = (&prompt_hash, prompt) {
                keep_prompt_in(tx, hash, text, &at)?;
            }
            tx.execute(
                "UPDATE bot_registrations SET def_hash = ?2, prompt_hash = ?3, settings_json = ?4
                  WHERE id = ?1",
                params![id, def_hash, prompt_hash, settings.to_string()],
            )?;
            let mut rec = AuditRecord::new(AuditKind::BotSurfaceRegistered, at.clone())
                .with("registration", id)
                .with("name", row.name.as_str())
                .with("def_hash", def_hash)
                .with("registered_by", row.registered_by.as_str())
                .with("changed", true);
            if let Some(hash) = prompt_hash {
                rec = rec.with("prompt_sha256", hash);
            }
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }

    /// Registration `id`, live or replaced, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn registration(&self, id: &str) -> Result<Option<Registration>> {
        self.with_read(|c| registration_in(c, id))
    }

    /// The registrations of the surface named `name`, oldest first: the
    /// live ones, and with `all` the replaced ones too.
    ///
    /// # Errors
    /// The store's.
    pub fn registrations_named(&self, name: &str, all: bool) -> Result<Vec<Registration>> {
        self.with_read(|c| registrations_where(c, "name = ?1", name, all))
    }

    /// The registrations `person` (a person key) made, oldest first: the
    /// live ones, and with `all` the replaced ones too.
    ///
    /// # Errors
    /// The store's.
    pub fn registrations_by(&self, person: &str, all: bool) -> Result<Vec<Registration>> {
        self.with_read(|c| registrations_where(c, "registered_by = ?1", person, all))
    }

    /// Records that registration `id` served at `now`
    /// ([`Registration::last_served_at`]), unless it served later already.
    /// Returns whether `id` is a live registration; a replaced or unknown
    /// one is not touched.
    ///
    /// # Errors
    /// The store's.
    pub fn touch_registration(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let at = stamp(now);
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE bot_registrations
                    SET last_served_at = max(COALESCE(last_served_at, ''), ?2)
                  WHERE id = ?1 AND replaced_at IS NULL",
                params![id, at],
            )? > 0)
        })
    }

    /// Ends registration `id` at `now` with none in its place: it is no
    /// longer live, and the calls waiting on it fail when next expired
    /// ([`Store::expire_calls`]). Returns whether it was live; a replaced
    /// one keeps the time it was first replaced at.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn replace_registration(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let at = stamp(now);
        self.with_tx(|tx| {
            registration_in(tx, id)?.ok_or(CoreError::NotFound)?;
            Ok(tx.execute(
                "UPDATE bot_registrations SET replaced_at = ?2
                  WHERE id = ?1 AND replaced_at IS NULL",
                params![id, at],
            )? > 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, store};
    use serde_json::json;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const OTHER: &str = "60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752";

    fn at(time: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(time)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    fn new(name: &str, person: &str) -> NewRegistration {
        NewRegistration {
            name: name.into(),
            def_hash: HASH.into(),
            prompt: None,
            settings: json!({}),
            registered_by: person.into(),
            replaces: None,
        }
    }

    /// Registrations made at one instant are listed oldest first, in the
    /// order they were made, whatever their IDs: by name and by person.
    #[test]
    fn registrations_made_at_one_instant_are_listed_in_the_order_made() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let ids = ["c", "a", "d", "b"];
        for id in ids {
            st.issue_registration(DAEMON, id, new("desk", "*owner"), t0)
                .unwrap()
                .unwrap();
        }
        let named: Vec<String> = st
            .registrations_named("desk", true)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(named, ids);
        let by: Vec<String> = st
            .registrations_by("*owner", false)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(by, ids);
    }

    #[test]
    fn a_definition_is_kept_once_under_its_hash_and_read_back() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        assert_eq!(st.surface_def(HASH).unwrap(), None);
        assert!(st.put_surface_def(HASH, r#"{"name":"shell"}"#, t0).unwrap());
        let kept = SurfaceDef {
            hash: HASH.into(),
            manifest: r#"{"name":"shell"}"#.into(),
            kept_at: "2026-11-11T09:00:00.000Z".into(),
        };
        assert_eq!(st.surface_def(HASH).unwrap(), Some(kept.clone()));
        // The same hash again stores nothing, whatever the text, and
        // counts as kept at the later of the two times.
        assert!(
            !st.put_surface_def(HASH, r#"{"name":"shell"}"#, at("2026-11-12T09:00:00Z"))
                .unwrap()
        );
        assert!(!st.put_surface_def(HASH, "other text", t0).unwrap());
        let again = SurfaceDef {
            kept_at: "2026-11-12T09:00:00.000Z".into(),
            ..kept
        };
        assert_eq!(st.surface_def(HASH).unwrap(), Some(again));
        // Text the store does not parse is kept as given.
        assert!(st.put_surface_def(OTHER, "not a manifest", t0).unwrap());
        assert_eq!(
            st.surface_def(OTHER).unwrap().unwrap().manifest,
            "not a manifest"
        );
    }

    /// A definition given again counts as kept from then: a prune that
    /// lands between the hub's keeping a definition no registration
    /// names any more and the registration's naming it again leaves it,
    /// so the registration goes on with it.
    #[test]
    fn a_definition_given_again_outlives_a_prune_before_the_registration_names_it() {
        use clax_core::working::{Clock, ManualClock};
        use std::sync::Arc;
        let clock = Arc::new(ManualClock::at("2026-11-11T09:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        st.put_surface_def(HASH, "{}", clock.now()).unwrap();
        st.put_surface_def(OTHER, "{}", clock.now()).unwrap();
        let reg = st
            .register_surface(DAEMON, new("shell", "*owner"), clock.now())
            .unwrap();
        // Two hours on, the registration goes on with the other
        // definition; five minutes after, back to the first.
        clock.advance(2 * 3600);
        assert!(
            st.change_registration(DAEMON, &reg.id, OTHER, None, &json!({}), clock.now())
                .unwrap()
        );
        clock.advance(300);
        assert!(!st.put_surface_def(HASH, "{}", clock.now()).unwrap());
        st.prune_ended_runs().unwrap();
        assert!(
            st.surface_def(HASH).unwrap().is_some(),
            "the definition just given again is kept"
        );
        assert!(
            st.change_registration(DAEMON, &reg.id, HASH, None, &json!({}), clock.now())
                .unwrap()
        );
    }

    /// A prune pass removes [`PRUNE_SURFACE_ROWS`] definitions nothing
    /// names at the most, the oldest first, and says it left more. The
    /// definitions runs' bindings name are passed over, older or not, so
    /// they never hold back one nothing names; a run whose bindings do
    /// not read keeps any definition whose hash its text holds.
    #[test]
    fn a_prune_removes_a_bounded_number_of_definitions_and_passes_over_those_runs_name() {
        use clax_core::working::{Clock, ManualClock};
        use std::sync::Arc;
        let clock = Arc::new(ManualClock::at("2026-11-11T09:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let hash = |i: usize| format!("{i:064x}");
        // Named by runs, and stored first.
        let named = PRUNE_SURFACE_ROWS + 5;
        for i in 0..named + 1 {
            st.put_surface_def(&hash(i), "{}", clock.now()).unwrap();
        }
        clock.advance(1);
        let unnamed = PRUNE_SURFACE_ROWS + 3;
        for i in 0..unnamed {
            st.put_surface_def(&hash(1000 + i), "{}", clock.now())
                .unwrap();
        }
        let bindings: Vec<String> = (0..named)
            .map(|i| {
                format!(
                    r#"{{"surface":"s{i}","hash":"{}","scope":null,"operations":[]}}"#,
                    hash(i)
                )
            })
            .collect();
        st.with_write(|c| {
            for (id, text) in [
                (
                    "01JC0",
                    format!(r#"{{"v":2,"bindings":[{}]}}"#, bindings.join(",")),
                ),
                (
                    "01JC1",
                    format!(r#"{{"v":2,"bindings":[{{"hash":"{}""#, hash(named)),
                ),
            ] {
                c.execute(
                    "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json,
                            status, model, key_source, base_version, created_at, updated_at,
                            bindings_json, kind)
                     VALUES (?1, 'a', 'u', 'other', '[]', 'finished', 'm', 'instance', 1,
                             '2026-11-11T09:00:00.000Z', '2026-11-11T09:00:00.000Z', ?2, 'x.y')",
                    params![id, text],
                )?;
            }
            Ok(())
        })
        .unwrap();
        clock.advance(BOT_DEF_KEEP.as_secs() as i64 + 1);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.surface_defs, p.more), (PRUNE_SURFACE_ROWS, true));
        let p = st.prune_ended_runs().unwrap();
        assert_eq!(
            (p.surface_defs, p.more),
            (unnamed - PRUNE_SURFACE_ROWS, false)
        );
        for i in 0..unnamed {
            assert_eq!(st.surface_def(&hash(1000 + i)).unwrap(), None);
        }
        for i in 0..named + 1 {
            assert!(st.surface_def(&hash(i)).unwrap().is_some(), "{i}");
        }
    }

    #[test]
    fn a_prompt_file_is_kept_once_under_its_sha256_and_read_back() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        let text = "You are the shell.";
        let hash = sha256_hex(text.as_bytes());
        assert_eq!(st.surface_prompt(&hash).unwrap(), None);
        assert!(st.put_surface_prompt(&hash, text, t0).unwrap());
        assert_eq!(st.surface_prompt(&hash).unwrap().as_deref(), Some(text));
        let kept_at = || -> String {
            st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT kept_at FROM bot_surface_prompts WHERE hash = ?1",
                    params![hash],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
        };
        assert_eq!(kept_at(), "2026-11-11T09:00:00.000Z");
        // Given again it is not stored anew: it was last kept at the
        // later time, and an earlier one does not move that back.
        assert!(
            !st.put_surface_prompt(&hash, text, at("2026-11-11T10:00:00Z"))
                .unwrap()
        );
        assert!(!st.put_surface_prompt(&hash, text, t0).unwrap());
        assert_eq!(kept_at(), "2026-11-11T10:00:00.000Z");
        assert_eq!(st.surface_prompt(&hash).unwrap().as_deref(), Some(text));
        // A hash that is not the text's is refused, and nothing is kept
        // under it.
        for wrong in [HASH, "", &hash.to_uppercase(), &hash[..63]] {
            assert_eq!(
                code(st.put_surface_prompt(wrong, text, t0)),
                "surface_hash_invalid",
                "{wrong:?}"
            );
            assert_eq!(st.surface_prompt(wrong).unwrap(), None);
        }
        // A registration's prompt file is kept by registering it, and
        // by going on with it changed.
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let desk = "You are a desk.";
        let a = st
            .register_surface(
                DAEMON,
                NewRegistration {
                    prompt: Some(desk.into()),
                    ..new("shell", "*owner")
                },
                t0,
            )
            .unwrap();
        let desk_hash = sha256_hex(desk.as_bytes());
        assert_eq!(a.prompt_hash.as_deref(), Some(desk_hash.as_str()));
        assert_eq!(
            st.surface_prompt(&desk_hash).unwrap().as_deref(),
            Some(desk)
        );
        let lamp = "You are a lamp.";
        st.change_registration(DAEMON, &a.id, HASH, Some(lamp), &json!({}), t0)
            .unwrap();
        let lamp_hash = sha256_hex(lamp.as_bytes());
        assert_eq!(
            st.surface_prompt(&lamp_hash).unwrap().as_deref(),
            Some(lamp)
        );
        let row = st.registration(&a.id).unwrap().unwrap();
        assert_eq!(
            (row.prompt.as_deref(), row.prompt_hash.as_deref()),
            (Some(lamp), Some(lamp_hash.as_str()))
        );
        // The one it had is still kept: a run started with it names it.
        assert_eq!(
            st.surface_prompt(&desk_hash).unwrap().as_deref(),
            Some(desk)
        );
    }

    #[test]
    fn a_hash_that_is_not_64_lowercase_hex_characters_is_refused() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        let upper = HASH.to_uppercase();
        let long = format!("{HASH}0");
        let other = format!("{}g", &HASH[..63]);
        for hash in ["", "abc", &HASH[..63], &long, &upper, &other] {
            assert_eq!(
                code(st.put_surface_def(hash, "{}", t0)),
                "surface_hash_invalid",
                "{hash:?}"
            );
            assert_eq!(st.surface_def(hash).unwrap(), None);
        }
    }

    #[test]
    fn a_registration_is_made_read_and_listed_by_name_and_by_person() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        assert_eq!(
            code(st.register_surface(DAEMON, new("shell", "*owner"), t0)),
            "surface_def_unknown"
        );
        st.put_surface_def(HASH, "{}", t0).unwrap();
        assert_eq!(
            code(st.register_surface(DAEMON, new("", "*owner"), t0)),
            "registration_invalid"
        );
        assert_eq!(
            code(st.register_surface(DAEMON, new("shell", ""), t0)),
            "registration_invalid"
        );
        let a = st
            .register_surface(
                DAEMON,
                NewRegistration {
                    prompt: Some("You are the shell.".into()),
                    settings: json!({"allow": ["*"], "call_timeout_s": 30}),
                    ..new("shell", "*owner")
                },
                t0,
            )
            .unwrap();
        assert_eq!(
            a,
            Registration {
                id: a.id.clone(),
                name: "shell".into(),
                def_hash: HASH.into(),
                prompt: Some("You are the shell.".into()),
                prompt_hash: Some(sha256_hex(b"You are the shell.")),
                settings: json!({"allow": ["*"], "call_timeout_s": 30}),
                settings_unread: None,
                registered_by: "*owner".into(),
                created_at: "2026-11-11T09:00:00.000Z".into(),
                last_served_at: None,
                replaced_at: None,
            }
        );
        assert!(a.is_live());
        assert_eq!(st.registration(&a.id).unwrap(), Some(a.clone()));
        assert_eq!(st.registration("nope").unwrap(), None);
        // A second live registration of the same name, and one of another
        // name by another person.
        let b = st
            .register_surface(DAEMON, new("shell", "u_mia"), at("2026-11-11T09:01:00Z"))
            .unwrap();
        let c = st
            .register_surface(DAEMON, new("review", "u_mia"), at("2026-11-11T09:02:00Z"))
            .unwrap();
        let ids = |regs: Vec<Registration>| regs.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(
            ids(st.registrations_named("shell", false).unwrap()),
            [a.id.clone(), b.id.clone()]
        );
        assert_eq!(
            ids(st.registrations_named("review", false).unwrap()),
            std::slice::from_ref(&c.id)
        );
        assert!(st.registrations_named("files", true).unwrap().is_empty());
        assert_eq!(
            ids(st.registrations_by("u_mia", false).unwrap()),
            [b.id.clone(), c.id.clone()]
        );
        assert_eq!(
            ids(st.registrations_by("*owner", false).unwrap()),
            std::slice::from_ref(&a.id)
        );
    }

    #[test]
    fn a_registration_id_its_caller_drew_is_issued_once_and_keeps_its_name() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let issued = st
            .issue_registration(DAEMON, id, new("shell", "*owner"), t0)
            .unwrap()
            .expect("issued");
        assert_eq!((issued.id.as_str(), issued.name.as_str()), (id, "shell"));
        // Issued again, under any name, nothing changes or is recorded.
        let later = at("2026-11-12T09:00:00Z");
        for name in ["shell", "review"] {
            assert_eq!(
                st.issue_registration(DAEMON, id, new(name, "u_mia"), later)
                    .unwrap(),
                None
            );
        }
        assert_eq!(st.registration(id).unwrap(), Some(issued));
        // Replaced, the ID is still the surface's and is not issued anew.
        st.replace_registration(id, later).unwrap();
        assert_eq!(
            st.issue_registration(DAEMON, id, new("review", "*owner"), later)
                .unwrap(),
            None
        );
        assert_eq!(st.registration(id).unwrap().unwrap().name, "shell");
        let recorded: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM audit_events WHERE kind = 'bot.surface.registered'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(recorded, 1);
        assert_eq!(
            code(st.issue_registration(DAEMON, "", new("shell", "*owner"), t0)),
            "registration_invalid"
        );
    }

    #[test]
    fn going_on_with_a_registration_changed_brings_its_row_up_to_date_and_is_recorded() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let first = NewRegistration {
            prompt: Some("You are the shell.".into()),
            settings: json!({"allow": ["*"]}),
            ..new("shell", "*owner")
        };
        let a = st.register_surface(DAEMON, first, t0).unwrap();
        let recorded = || -> Vec<Value> {
            st.with_read(|c| {
                let mut q = c.prepare(
                    "SELECT body FROM audit_events WHERE kind = 'bot.surface.registered'
                      ORDER BY seq",
                )?;
                let rows = q.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows
                    .map(|r| r.map(|body| serde_json::from_str(&body).unwrap()))
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap()
        };
        // The first registration says its prompt file's hash.
        let prompt_hash = sha256_hex(b"You are the shell.");
        assert_eq!(recorded()[0]["prompt_sha256"], prompt_hash.as_str());
        assert_eq!(recorded()[0].get("changed"), None);
        // Gone on with as it is: nothing changes and nothing is recorded.
        let t1 = at("2026-11-11T09:05:00Z");
        let same = st.change_registration(
            DAEMON,
            &a.id,
            HASH,
            Some("You are the shell."),
            &json!({"allow": ["*"]}),
            t1,
        );
        assert!(!same.unwrap());
        assert_eq!(recorded().len(), 1);
        // With another definition, prompt file or settings: the row has
        // them, and a record says so.
        assert_eq!(
            code(st.change_registration(DAEMON, &a.id, OTHER, None, &json!({}), t1)),
            "surface_def_unknown"
        );
        st.put_surface_def(OTHER, "{}", t1).unwrap();
        for (hash, prompt, settings) in [
            (OTHER, Some("You are the shell."), json!({"allow": ["*"]})),
            (OTHER, Some("You are a desk."), json!({"allow": ["*"]})),
            (OTHER, None, json!({"allow": ["*"]})),
            (OTHER, None, json!({})),
        ] {
            assert!(
                st.change_registration(DAEMON, &a.id, hash, prompt, &settings, t1)
                    .unwrap()
            );
            let row = st.registration(&a.id).unwrap().unwrap();
            assert_eq!(
                (row.def_hash.as_str(), row.prompt.as_deref(), &row.settings),
                (hash, prompt, &settings)
            );
            assert_eq!(
                (row.name.as_str(), row.created_at.as_str()),
                ("shell", a.created_at.as_str())
            );
        }
        let records = recorded();
        assert_eq!(records.len(), 5);
        for record in &records[1..] {
            assert_eq!(record["changed"], true);
            assert_eq!(record["registration"], a.id.as_str());
            assert_eq!(record["def_hash"], OTHER);
        }
        assert_eq!(
            records[2]["prompt_sha256"],
            sha256_hex(b"You are a desk.").as_str()
        );
        assert_eq!(records[3].get("prompt_sha256"), None);
        assert!(matches!(
            st.change_registration(DAEMON, "nope", HASH, None, &json!({}), t1),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn registering_is_recorded() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let a = st
            .register_surface(DAEMON, new("shell", "*owner"), t0)
            .unwrap();
        let b = st
            .register_surface(
                DAEMON,
                NewRegistration {
                    replaces: Some(a.id.clone()),
                    ..new("shell", "*owner")
                },
                at("2026-11-11T09:05:00Z"),
            )
            .unwrap();
        let recorded: Vec<(String, Value)> = st
            .with_read(|c| {
                let mut q = c.prepare(
                    "SELECT at, body FROM audit_events WHERE kind = 'bot.surface.registered'
                      ORDER BY seq",
                )?;
                let rows = q.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?;
                Ok(rows
                    .map(|r| r.map(|(at, body)| (at, serde_json::from_str(&body).unwrap())))
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        let fields = |v: &Value| {
            (
                v["registration"].clone(),
                v["name"].clone(),
                v["def_hash"].clone(),
                v["registered_by"].clone(),
                v.get("replaces").cloned(),
            )
        };
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].0, "2026-11-11T09:00:00.000Z");
        assert_eq!(
            fields(&recorded[0].1),
            (
                json!(a.id),
                json!("shell"),
                json!(HASH),
                json!("*owner"),
                None
            )
        );
        assert_eq!(
            fields(&recorded[1].1),
            (
                json!(b.id),
                json!("shell"),
                json!(HASH),
                json!("*owner"),
                Some(json!(a.id))
            )
        );
    }

    #[test]
    fn a_registration_is_touched_and_replaced() {
        let (_d, st) = store();
        let t0 = at("2026-11-11T09:00:00Z");
        st.put_surface_def(HASH, "{}", t0).unwrap();
        let a = st
            .register_surface(DAEMON, new("shell", "*owner"), t0)
            .unwrap();
        assert!(
            st.touch_registration(&a.id, at("2026-11-11T09:00:10Z"))
                .unwrap()
        );
        let served = |id: &str| st.registration(id).unwrap().unwrap().last_served_at;
        assert_eq!(served(&a.id).as_deref(), Some("2026-11-11T09:00:10.000Z"));
        // An earlier time does not move it back; a later one moves it on.
        assert!(
            st.touch_registration(&a.id, at("2026-11-11T09:00:05Z"))
                .unwrap()
        );
        assert_eq!(served(&a.id).as_deref(), Some("2026-11-11T09:00:10.000Z"));
        assert!(
            st.touch_registration(&a.id, at("2026-11-11T09:00:25Z"))
                .unwrap()
        );
        assert_eq!(served(&a.id).as_deref(), Some("2026-11-11T09:00:25.000Z"));
        assert!(!st.touch_registration("nope", t0).unwrap());

        // A later registration takes its place.
        let b = st
            .register_surface(
                DAEMON,
                NewRegistration {
                    replaces: Some(a.id.clone()),
                    ..new("shell", "*owner")
                },
                at("2026-11-11T09:05:00Z"),
            )
            .unwrap();
        let old = st.registration(&a.id).unwrap().unwrap();
        assert_eq!(old.replaced_at.as_deref(), Some("2026-11-11T09:05:00.000Z"));
        assert!(!old.is_live());
        assert_eq!(
            st.registrations_named("shell", false).unwrap(),
            std::slice::from_ref(&b)
        );
        assert_eq!(
            st.registrations_named("shell", true).unwrap(),
            [old.clone(), b.clone()]
        );
        assert_eq!(st.registrations_by("*owner", true).unwrap().len(), 2);
        // A replaced registration is not touched, and replacing it again
        // changes nothing.
        assert!(
            !st.touch_registration(&a.id, at("2026-11-11T09:06:00Z"))
                .unwrap()
        );
        assert!(
            !st.replace_registration(&a.id, at("2026-11-11T09:07:00Z"))
                .unwrap()
        );
        assert_eq!(st.registration(&a.id).unwrap(), Some(old));
        // One ended with none in its place.
        assert!(
            st.replace_registration(&b.id, at("2026-11-11T09:08:00Z"))
                .unwrap()
        );
        assert!(st.registrations_named("shell", false).unwrap().is_empty());
        assert!(matches!(
            st.replace_registration("nope", t0),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.register_surface(
                DAEMON,
                NewRegistration {
                    replaces: Some("nope".into()),
                    ..new("shell", "*owner")
                },
                t0
            ),
            Err(CoreError::NotFound)
        ));
    }
}
