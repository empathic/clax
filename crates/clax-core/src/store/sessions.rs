//! Harness sessions: registration, joining by parent PID, liveness, and reaping.

use super::Store;
use crate::audit::{Actor, AgentActor, AuditCtx, AuditKind, AuditRecord, SystemReason};
use crate::model::Session;
use crate::secret::Secret;
use crate::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What a harness shim or hook reports when it starts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegisterSession {
    pub harness: String,
    #[serde(default)]
    pub harness_session_id: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub parent_pid: Option<u32>,
    /// The harness's transcript file, where the harness's extension API
    /// names one (Pi's session file). Not sent when absent, so an older
    /// daemon is not asked to read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The grant the caller authenticated with; `None` for the daemon token.
    /// Set by the daemon from the credential, never read from a body.
    #[serde(skip)]
    pub grant_id: Option<String>,
    /// The repository's directory name, for the agent's name (spec §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The repository's branch, for the agent's name (spec §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// What a harness hook reports when it joins its session
/// ([`Store::join_session`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinSession {
    pub harness: String,
    pub parent_pid: u32,
    pub harness_session_id: String,
    /// Fills an empty `cwd`, and is the `cwd` of a row made here.
    pub cwd: Option<String>,
    /// The hook's ancestors, nearest first, tried after `parent_pid`.
    /// The harness's transcript file, from the hook input; replaces the
    /// recorded one.
    pub transcript_path: Option<String>,
    pub ancestor_pids: Vec<u32>,
}

/// What a new session got back from its harness session's last end, an
/// `idle` one (spec 2026-10-08 §6.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The ended session it follows.
    pub from: String,
    pub ended_at: String,
    /// The artifacts whose watches are back.
    pub artifacts: Vec<String>,
    /// The open threads that end released.
    pub threads: Vec<String>,
    /// The note the session's first tool result carries.
    pub note: String,
}

/// What a registration or join answered (spec 2026-10-08 §6.7, §6.8).
#[derive(Debug)]
pub struct Registered {
    pub session: Session,
    /// What a new row got back from an earlier `idle` end.
    pub restored: Option<Restored>,
    /// A grant's new session's secret, issued once when the call made the
    /// row ([`crate::secret::SESSION_PREFIX`]); `None` for a refresh and for
    /// the daemon token's sessions, which get none.
    pub secret: Option<Secret>,
}

/// Which of its grant's sessions a registration or join proved with the
/// session's secret (spec 2026-10-08 §6.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proved<'a> {
    /// No proof is asked: the daemon token's sessions, and the store's own
    /// callers outside a request.
    Unchecked,
    /// The request proved this session (`None`: no session). A grant's
    /// live row is refreshed only when it is this one.
    Session(Option<&'a str>),
}

/// 401 `invalid_session_proof`: the request names, or matches, one of its
/// grant's sessions without that session's secret (spec 2026-10-08 §6.8).
pub fn invalid_session_proof() -> CoreError {
    CoreError::invalid(
        "invalid_session_proof",
        "this request does not prove its agent session: send that session's X-Clax-Session-Secret",
    )
}

/// What a reaper pass ended, which feedback it released, and which open
/// questions of the ended sessions it withdrew.
#[derive(Debug, Default)]
pub struct Reaped {
    pub ended: Vec<String>,
    pub touched: crate::feedback::Touched,
    pub withdrawn_questions: Vec<String>,
}

/// What [`Store::end_session_touched`] ended and released.
#[derive(Debug)]
pub struct EndedSession {
    pub session: Session,
    /// Feedback released (see [`Store::end_session_touched`]).
    pub touched: crate::feedback::Touched,
    /// IDs of the session's open questions, now withdrawn. Empty when the
    /// session had already ended.
    pub withdrawn_questions: Vec<String>,
    /// IDs of its open `ask` approval requests, now withdrawn.
    pub withdrawn_requests: Vec<String>,
    /// IDs of the threads it owned, now released.
    pub released_threads: Vec<String>,
}

pub(super) const SELECT: &str =
    "SELECT id, harness, harness_session_id, cwd, pid, parent_pid, started_at,
    last_seen_at, ended_at, agent_handle, grant_id, person, name, name_source FROM sessions";

pub(super) fn row_to_session(r: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get("id")?,
        harness: r.get("harness")?,
        harness_session_id: r.get("harness_session_id")?,
        cwd: r.get("cwd")?,
        pid: r.get("pid")?,
        parent_pid: r.get("parent_pid")?,
        started_at: r.get("started_at")?,
        last_seen_at: r.get("last_seen_at")?,
        ended_at: r.get("ended_at")?,
        agent_handle: r.get("agent_handle")?,
        grant_id: r.get("grant_id")?,
        person: r.get("person")?,
        name: r.get("name")?,
        name_source: r.get("name_source")?,
    })
}

fn fetch(tx: &Transaction<'_>, id: &str) -> Result<Session> {
    Ok(tx.query_row(
        &format!("{SELECT} WHERE id = ?1"),
        params![id],
        row_to_session,
    )?)
}

/// Session `id` as the agent actor of the events it makes (audit spec
/// §5.2), read on `c`, with its grant and person when it is a person's
/// agent; `None` when no session has that ID.
pub(super) fn agent_of(c: &Connection, id: &str) -> Result<Option<AgentActor>> {
    Ok(c.prepare_cached(
        "SELECT harness, harness_session_id, agent_handle, transcript_path, grant_id, person
         FROM sessions WHERE id = ?1",
    )?
    .query_row(params![id], |r| {
        Ok(AgentActor {
            session_id: Some(id.to_string()),
            harness: Some(r.get(0)?),
            harness_session_id: r.get(1)?,
            agent_handle: Some(r.get(2)?),
            transcript_path: r.get(3)?,
            grant_id: r.get(4)?,
            person: r.get(5)?,
            channel: None,
        })
    })
    .optional()?)
}

/// What a `session.start` or `session.join` records of a session (spec
/// §6.8), and what a refresh must change for a join to be recorded.
#[derive(PartialEq)]
pub(super) struct Facts {
    harness_session_id: Option<String>,
    cwd: String,
    transcript_path: Option<String>,
    pid: Option<u32>,
}

pub(super) fn facts(c: &Connection, id: &str) -> Result<Facts> {
    Ok(c.prepare_cached(
        "SELECT harness_session_id, cwd, transcript_path, pid FROM sessions WHERE id = ?1",
    )?
    .query_row(params![id], |r| {
        Ok(Facts {
            harness_session_id: r.get(0)?,
            cwd: r.get(1)?,
            transcript_path: r.get(2)?,
            pid: r.get(3)?,
        })
    })?)
}

/// The `session.start` or `session.join` (`kind`) record of session `id`
/// (audit spec §6.8): its harness and facts.
pub(super) fn session_record(
    kind: AuditKind,
    at: &str,
    id: &str,
    harness: &str,
    f: Facts,
) -> AuditRecord {
    let mut rec = AuditRecord::new(kind, at)
        .with("harness", harness)
        .with("harness_session_id", f.harness_session_id)
        .with("cwd", f.cwd)
        .with("transcript_path", f.transcript_path)
        .with("pid", f.pid);
    rec.ids.session = Some(id.to_string());
    rec
}

/// The `session.end` record of session `id`, for `reason`.
pub(super) fn session_end_record(at: &str, id: &str, reason: &str) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::SessionEnd, at).with("reason", reason);
    rec.ids.session = Some(id.to_string());
    rec
}

impl Store {
    /// Records `kind` (`session.start` or `session.join`) of session `id`
    /// in `tx`: its facts, made by the session's own agent through `ctx`'s
    /// channel, git state and tool call.
    fn record_session(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        kind: AuditKind,
        id: &str,
        harness: &str,
    ) -> Result<()> {
        let f = facts(tx, id)?;
        let agent = agent_of(tx, id)?.ok_or(CoreError::NotFound)?;
        let ctx = AuditCtx {
            actor: Actor::Agent(agent),
            ..ctx.clone()
        };
        let mut rec = session_record(kind, &self.now(), id, harness, f);
        if kind == AuditKind::SessionStart && agent_grant(&ctx).is_some() {
            // The secret itself, and its hash, are never recorded.
            rec = rec.with("secret_issued", true);
        }
        self.record_audit(tx, &ctx, rec)?;
        if kind == AuditKind::SessionStart {
            self.agents_changed(tx, id)?;
        }
        Ok(())
    }

    /// Records `session.end` of session `id` in `tx` under `ctx`, for
    /// `reason`; a system actor names the session's agent in `for_actor`.
    fn record_session_end(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        reason: &str,
    ) -> Result<()> {
        let rec = session_end_record(&self.now(), id, reason);
        let rec = with_for_actor(tx, ctx, rec, id, AgentActor::default)?;
        self.record_audit(tx, ctx, rec)?;
        Ok(())
    }
}

/// `rec` with `for_actor`, session `sid`'s agent (else `fallback()`), when
/// `ctx`'s actor is Clax itself: a change Clax makes on an agent's behalf
/// names that agent.
pub(super) fn with_for_actor(
    c: &Connection,
    ctx: &AuditCtx,
    rec: AuditRecord,
    sid: &str,
    fallback: impl FnOnce() -> AgentActor,
) -> Result<AuditRecord> {
    if !matches!(ctx.actor, Actor::System { .. }) {
        return Ok(rec);
    }
    let who = agent_of(c, sid)?.unwrap_or_else(fallback);
    Ok(rec.with(
        "for_actor",
        serde_json::to_value(Actor::Agent(who)).expect("serialisable actor"),
    ))
}

/// The ID of a live row of credential `grant_id` (`None`: the daemon
/// token's) matching `where_clause`, newest first. `where_clause` numbers its
/// parameters from `?1`; the grant is bound after them.
fn find_live(
    tx: &Transaction<'_>,
    grant_id: Option<&str>,
    where_clause: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Option<String>> {
    let mut all = args.to_vec();
    all.push(&grant_id);
    Ok(tx
        .query_row(
            &format!(
                "SELECT id FROM sessions WHERE ended_at IS NULL AND {where_clause} AND grant_id IS ?{}
                 ORDER BY started_at DESC, id DESC LIMIT 1",
                all.len()
            ),
            all.as_slice(),
            |r| r.get(0),
        )
        .optional()?)
}

/// The grant of `ctx`'s agent actor, if it is a grant's agent.
fn agent_grant(ctx: &AuditCtx) -> Option<&str> {
    match &ctx.actor {
        Actor::Agent(a) => a.grant_id.as_deref(),
        _ => None,
    }
}

/// The live row a registration or join of credential `grant` refreshes,
/// given `matched`, the row its harness session ID or parent PID matched
/// (spec 2026-10-08 §6.7, §6.8). For a grant's checked call, a matched row
/// must be the session `proved` names, and with no match the proved
/// session (live, of the same harness) is the one refreshed.
///
/// # Errors
/// [`invalid_session_proof`] when a grant's call matches a live row it did
/// not prove.
fn proved_row(
    tx: &Transaction<'_>,
    grant: Option<&str>,
    harness: &str,
    matched: Option<String>,
    proved: Proved<'_>,
) -> Result<Option<String>> {
    let (Some(g), Proved::Session(p)) = (grant, proved) else {
        return Ok(matched);
    };
    match (matched, p) {
        (Some(m), Some(p)) if m == p => Ok(Some(m)),
        (Some(_), _) => Err(invalid_session_proof()),
        (None, Some(p)) => find_live(tx, Some(g), "id = ?1 AND harness = ?2", params![p, harness]),
        (None, None) => Ok(None),
    }
}

/// A new secret for a row of credential `grant`: one for a grant's
/// session, none for the token's (spec 2026-10-08 §6.8).
fn issue_secret(grant: Option<&str>) -> Option<Secret> {
    grant.map(|_| crate::secret::token(crate::secret::SESSION_PREFIX))
}

/// The person of a session registering under `grant_id`: the grant's
/// person's public ID, `None` for the owner's machines and the daemon token.
///
/// # Errors
/// `grant_revoked` unless the grant is approved.
fn person_of_grant(tx: &Transaction<'_>, grant_id: Option<&str>) -> Result<Option<String>> {
    let Some(gid) = grant_id else {
        return Ok(None);
    };
    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT g.status, CASE WHEN v.owner = 1 THEN NULL ELSE v.public_id END
             FROM grants g JOIN viewers v ON v.id = g.viewer_id WHERE g.id = ?1",
            params![gid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        Some((status, person)) if status == "approved" => Ok(person),
        _ => Err(CoreError::invalid(
            "grant_revoked",
            "this grant is not live",
        )),
    }
}

/// Who gave a session its name (spec §11): derived on registration, set
/// by the agent, or given by its person or the owner (which the agent can
/// no longer change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameSource {
    Derived,
    Agent,
    Person,
}

impl NameSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NameSource::Derived => "derived",
            NameSource::Agent => "agent",
            NameSource::Person => "person",
        }
    }
}

/// The display names whose token form no agent's name may be (spec §11):
/// the owner's and every connected person's.
pub(super) fn people_names(c: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = c.prepare_cached(
        "SELECT display_name FROM viewers WHERE display_name IS NOT NULL
           AND (owner = 1 OR (person = 1 AND person_revoked_at IS NULL))",
    )?;
    Ok(stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

/// Whether a live session other than `except` has `name`, ignoring case.
pub(super) fn live_name_taken(
    c: &rusqlite::Connection,
    name: &str,
    except: Option<&str>,
) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE ended_at IS NULL AND name = ?1 COLLATE NOCASE
           AND id IS NOT ?2)",
        params![name, except],
        |r| r.get(0),
    )?)
}

/// Whether a live session's name looks like the display name whose keys
/// are `key` ([`crate::names::CollisionKeys`], spec §9.6, §11).
pub(super) fn live_name_looks_like(
    c: &rusqlite::Connection,
    key: &crate::names::CollisionKeys,
) -> Result<bool> {
    let mut stmt =
        c.prepare_cached("SELECT name FROM sessions WHERE ended_at IS NULL AND name IS NOT NULL")?;
    for n in stmt.query_map([], |r| r.get::<_, String>(0))? {
        if crate::names::CollisionKeys::of(&n?).meets(key) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The name a session of `harness` registering from `repo` and `branch`
/// is given (spec §11, R3): the first of [`crate::names::candidates`] that
/// is neither reserved nor a live session's other than `except`'s.
fn derived_name(
    c: &rusqlite::Connection,
    repo: Option<&str>,
    branch: Option<&str>,
    harness: &str,
    except: Option<&str>,
) -> Result<String> {
    let base = crate::names::derive(repo, branch, harness);
    let people = people_names(c)?;
    for name in crate::names::candidates(&base, harness) {
        if !crate::names::is_reserved(&name, &people) && !live_name_taken(c, &name, except)? {
            return Ok(name);
        }
    }
    unreachable!("the numbered candidates never end")
}

/// `name_taken` when the unique index on live names refused a write.
fn taken_on_conflict(e: rusqlite::Error) -> CoreError {
    match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            CoreError::invalid("name_taken", "another live agent has that name")
        }
        e => e.into(),
    }
}

impl Store {
    /// A session unseen for this long, with no live process, is ended by the reaper;
    /// registration without a harness session ID only adopts rows seen within it.
    pub const SESSION_IDLE_SECS: u64 = 300;
    /// A remote session (one with a `grant_id`) unseen for this long is
    /// ended by the reaper; its process is never probed (spec 2026-10-08
    /// §14), so a laptop that sleeps briefly keeps its sessions.
    pub const REMOTE_SESSION_IDLE_SECS: u64 = 1800;
    /// How long after an `idle` end a registration with the same harness
    /// session ID and credential gets the session's watches back (§6.7).
    pub const RESTORE_WINDOW_SECS: u64 = 24 * 3600;

    /// Registers a session, reusing a live row where one already stands for it.
    /// An empty `harness_session_id` counts as none. Every match is among
    /// the rows of the caller's credential (`r.grant_id`, `None` for the
    /// daemon token): a grant never reaches another credential's session
    /// (spec M4). A grant's new row carries the grant and its person.
    ///
    /// With a `harness_session_id`: a live row with the same `(harness,
    /// harness_session_id)` is refreshed and returned; failing that, a live row
    /// with the same `(harness, parent_pid)` and no harness session ID is
    /// adopted and given the ID. Without one: a live row with the same
    /// `(harness, parent_pid)` seen within [`Store::SESSION_IDLE_SECS`] is
    /// refreshed and returned, whether or not it has an ID yet (two shims, or a
    /// restarted shim, under one harness process share a row). Otherwise a new
    /// row is inserted. A refresh overwrites `pid` and `parent_pid` only when
    /// the caller supplies them, and fills an empty `cwd` (a hook-only row).
    /// A given `transcript_path` replaces the recorded one.
    ///
    /// A new row records `session.start`; a refresh that changes the
    /// session's harness session ID, `cwd`, `pid` or transcript path records
    /// `session.join`;
    /// a refresh that changes none of them records nothing. Either is the
    /// session's own agent's, through `ctx`'s channel, git state and call.
    ///
    /// # Errors
    /// `grant_revoked` when `r.grant_id` names no approved grant.
    pub fn register_session(&self, ctx: &AuditCtx, r: RegisterSession) -> Result<Session> {
        self.register_session_restoring(ctx, r).map(|(s, _)| s)
    }

    /// [`Store::register_session`], also answering what a new row got back
    /// from an earlier `idle` end ([`Restored`], spec 2026-10-08 §6.7).
    ///
    /// # Errors
    /// As [`Store::register_session`].
    pub fn register_session_restoring(
        &self,
        ctx: &AuditCtx,
        r: RegisterSession,
    ) -> Result<(Session, Option<Restored>)> {
        self.register_session_proved(ctx, r, Proved::Unchecked)
            .map(|x| (x.session, x.restored))
    }

    /// [`Store::register_session_restoring`] for a request that proved
    /// `proved` (spec 2026-10-08 §6.8): a grant's registration refreshes a
    /// live row only when it proved that session, and a new grant row gets
    /// a secret, answered once ([`Registered::secret`]) and kept only as
    /// its SHA-256.
    ///
    /// # Errors
    /// As [`Store::register_session`], and [`invalid_session_proof`] when a
    /// grant's registration matches a live row it did not prove.
    pub fn register_session_proved(
        &self,
        ctx: &AuditCtx,
        mut r: RegisterSession,
        proved: Proved<'_>,
    ) -> Result<Registered> {
        if r.harness_session_id.as_deref() == Some("") {
            r.harness_session_id = None;
        }
        self.with_tx(|tx| {
            let grant = r.grant_id.as_deref();
            let person = person_of_grant(tx, grant)?;
            let now = self.now();
            let recent = super::stamp(
                self.now_utc() - Duration::from_secs(Self::SESSION_IDLE_SECS),
            );
            let existing = match (&r.harness_session_id, r.parent_pid) {
                (Some(hid), parent) => {
                    match find_live(
                        tx,
                        grant,
                        "harness = ?1 AND harness_session_id = ?2",
                        params![r.harness, hid],
                    )? {
                        Some(id) => Some(id),
                        None => match parent {
                            Some(p) => find_live(
                                tx,
                                grant,
                                "harness = ?1 AND parent_pid = ?2 AND harness_session_id IS NULL",
                                params![r.harness, p],
                            )?,
                            None => None,
                        },
                    }
                }
                (None, Some(p)) => find_live(
                    tx,
                    grant,
                    "harness = ?1 AND parent_pid = ?2 AND last_seen_at >= ?3",
                    params![r.harness, p, recent],
                )?,
                (None, None) => None,
            };
            let existing = proved_row(tx, grant, &r.harness, existing, proved)?;
            if let Some(id) = existing {
                let before = fetch(tx, &id)?;
                let facts_before = facts(tx, &id)?;
                tx.execute(
                    "UPDATE sessions SET pid = COALESCE(?2, pid), parent_pid = COALESCE(?3, parent_pid),
                        harness_session_id = COALESCE(harness_session_id, ?4), last_seen_at = ?5,
                        cwd = CASE WHEN cwd = '' THEN ?6 ELSE cwd END,
                        transcript_path = COALESCE(?7, transcript_path)
                     WHERE id = ?1",
                    params![id, r.pid, r.parent_pid, r.harness_session_id, now, r.cwd, r.transcript_path],
                )?;
                if facts(tx, &id)? != facts_before {
                    self.record_session(tx, ctx, AuditKind::SessionJoin, &id, &r.harness)?;
                }
                // A row without a name, or a hook's row (no process yet)
                // named from its harness alone, takes the name its shim's
                // repository gives.
                let hook_row = before.pid.is_none()
                    && before.name_source.as_deref() == Some(NameSource::Derived.as_str())
                    && r.repo.is_some();
                if before.name.is_none() || hook_row {
                    let name = derived_name(
                        tx,
                        r.repo.as_deref(),
                        r.branch.as_deref(),
                        &r.harness,
                        Some(&id),
                    )?;
                    tx.execute(
                        "UPDATE sessions SET name = ?2, name_source = 'derived' WHERE id = ?1",
                        params![id, name],
                    )?;
                }
                // A row adopted by its parent PID takes the harness
                // session ID now: what its last idle end saved comes back.
                let restored = if facts_before.harness_session_id.is_none()
                    && r.harness_session_id.is_some()
                {
                    self.restore_in(tx, ctx, &id)?
                } else {
                    None
                };
                return Ok(Registered {
                    session: fetch(tx, &id)?,
                    restored,
                    secret: None,
                });
            }
            let id = new_ulid();
            let name = derived_name(
                tx,
                r.repo.as_deref(),
                r.branch.as_deref(),
                &r.harness,
                None,
            )?;
            let secret = issue_secret(grant);
            tx.execute(
                "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                    started_at, last_seen_at, agent_handle, transcript_path,
                    grant_id, person, name, name_source, secret_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11, ?12, 'derived', ?13)",
                params![
                    id,
                    r.harness,
                    r.harness_session_id,
                    r.cwd,
                    r.pid,
                    r.parent_pid,
                    now,
                    crate::new_agent_handle(),
                    r.transcript_path,
                    grant,
                    person,
                    name,
                    secret.as_ref().map(Secret::sha256_hex)
                ],
            )?;
            self.record_session(tx, ctx, AuditKind::SessionStart, &id, &r.harness)?;
            let restored = self.restore_in(tx, ctx, &id)?;
            Ok(Registered {
                session: fetch(tx, &id)?,
                restored,
                secret,
            })
        })
    }

    /// Called from a harness hook that knows the harness session ID and the
    /// harness process (`parent_pid`). Returns the live row for that
    /// `(harness, harness_session_id)`; else gives the ID to a live row for
    /// `(harness, parent_pid)` that has none; else inserts a hook-only row (no
    /// `pid`, `cwd` as given or empty) for the shim to adopt when it registers.
    /// A given `cwd` also fills an empty one on an existing row. Hook-only rows
    /// do not heartbeat: unless a shim adopts one, it is reaped after the idle
    /// window ([`Store::SESSION_IDLE_SECS`]).
    /// `ancestor_pids` (nearest first) are tried after `parent_pid` when no row matches.
    /// A given `transcript_path` replaces the recorded one.
    ///
    /// A new row records `session.start`; a join that changes the session's
    /// harness session ID, `cwd` or transcript path records `session.join`;
    /// one that changes none of them records nothing. Either is the
    /// session's own agent's, through `ctx`'s channel, git state and call.
    /// Every candidate is among the rows of the caller's credential
    /// (`grant_id`, `None` for the daemon token), and a row made here
    /// carries it (spec M4).
    ///
    /// # Errors
    /// `grant_revoked` when `grant_id` names no approved grant.
    pub fn join_session(
        &self,
        ctx: &AuditCtx,
        j: JoinSession,
        grant_id: Option<&str>,
    ) -> Result<Session> {
        self.join_session_restoring(ctx, j, grant_id)
            .map(|(s, _)| s)
    }

    /// [`Store::join_session`], also answering what a new row got back from
    /// an earlier `idle` end ([`Restored`], spec 2026-10-08 §6.7).
    ///
    /// # Errors
    /// As [`Store::join_session`].
    pub fn join_session_restoring(
        &self,
        ctx: &AuditCtx,
        j: JoinSession,
        grant_id: Option<&str>,
    ) -> Result<(Session, Option<Restored>)> {
        self.join_session_proved(ctx, j, grant_id, Proved::Unchecked)
            .map(|x| (x.session, x.restored))
    }

    /// [`Store::join_session_restoring`] for a request that proved `proved`
    /// (spec 2026-10-08 §6.8): a grant's join takes a live row only when it
    /// proved that session, and a row it makes gets a secret, answered once
    /// ([`Registered::secret`]).
    ///
    /// # Errors
    /// As [`Store::join_session`], and [`invalid_session_proof`] when a
    /// grant's join matches a live row it did not prove.
    pub fn join_session_proved(
        &self,
        ctx: &AuditCtx,
        j: JoinSession,
        grant_id: Option<&str>,
        proved: Proved<'_>,
    ) -> Result<Registered> {
        let JoinSession {
            harness,
            parent_pid,
            harness_session_id,
            cwd,
            ancestor_pids,
            transcript_path,
        } = j;
        let (harness, harness_session_id) = (harness.as_str(), harness_session_id.as_str());
        let cwd = cwd.as_deref();
        let transcript_path = transcript_path.as_deref();
        self.with_tx(|tx| {
            let person = person_of_grant(tx, grant_id)?;
            let now = self.now();
            let existing = match find_live(
                tx,
                grant_id,
                "harness = ?1 AND harness_session_id = ?2",
                params![harness, harness_session_id],
            )? {
                Some(id) => Some(id),
                None => {
                    // The hook's parent, then its ancestors nearest first: a
                    // hook run through a wrapper shell is a grandchild of the
                    // harness the shim registered under.
                    let mut found = None;
                    for pid in std::iter::once(&parent_pid).chain(&ancestor_pids) {
                        found = find_live(
                            tx,
                            grant_id,
                            "harness = ?1 AND parent_pid = ?2 AND harness_session_id IS NULL",
                            params![harness, pid],
                        )?;
                        if found.is_some() {
                            break;
                        }
                    }
                    found
                }
            };
            let existing = proved_row(tx, grant_id, harness, existing, proved)?;
            let mut restored = None;
            let mut secret = None;
            let id = match existing {
                Some(id) => {
                    let before = facts(tx, &id)?;
                    tx.execute(
                        "UPDATE sessions SET harness_session_id = COALESCE(harness_session_id, ?2),
                            parent_pid = COALESCE(parent_pid, ?3), last_seen_at = ?4,
                            cwd = CASE WHEN cwd = '' THEN COALESCE(?5, cwd) ELSE cwd END,
                            transcript_path = COALESCE(?6, transcript_path)
                         WHERE id = ?1",
                        params![id, harness_session_id, parent_pid, now, cwd, transcript_path],
                    )?;
                    let after = facts(tx, &id)?;
                    if after != before {
                        self.record_session(tx, ctx, AuditKind::SessionJoin, &id, harness)?;
                    }
                    // A row the shim made without the harness session ID
                    // (Codex's) gets it here: what its last idle end saved
                    // comes back now.
                    if before.harness_session_id.is_none() {
                        restored = self.restore_in(tx, ctx, &id)?;
                    }
                    id
                }
                None => {
                    let id = new_ulid();
                    let name = derived_name(tx, None, None, harness, None)?;
                    secret = issue_secret(grant_id);
                    tx.execute(
                        "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                            started_at, last_seen_at, agent_handle, transcript_path, grant_id, person,
                            name, name_source, secret_sha256)
                         VALUES (?1, ?2, ?3, ?6, NULL, ?4, ?5, ?5, ?7, ?8, ?9, ?10, ?11, 'derived', ?12)",
                        params![
                            id,
                            harness,
                            harness_session_id,
                            parent_pid,
                            now,
                            cwd.unwrap_or(""),
                            crate::new_agent_handle(),
                            transcript_path,
                            grant_id,
                            person,
                            name,
                            secret.as_ref().map(Secret::sha256_hex)
                        ],
                    )?;
                    self.record_session(tx, ctx, AuditKind::SessionStart, &id, harness)?;
                    restored = self.restore_in(tx, ctx, &id)?;
                    id
                }
            };
            Ok(Registered {
                session: fetch(tx, &id)?,
                restored,
                secret,
            })
        })
    }

    /// Whether `secret` is the secret of live session `sid` of grant
    /// `grant_id` (spec 2026-10-08 §6.8), compared by SHA-256 in constant
    /// time. `false` for another grant's session, one that has ended (its
    /// secret is cleared), and one that has none.
    ///
    /// # Errors
    /// The store's.
    pub fn session_secret_matches(
        &self,
        sid: &str,
        grant_id: &str,
        secret: &Secret,
    ) -> Result<bool> {
        self.with_read(|c| {
            let stored: Option<Option<String>> = c
                .prepare_cached(
                    "SELECT secret_sha256 FROM sessions
                     WHERE id = ?1 AND grant_id = ?2 AND ended_at IS NULL",
                )?
                .query_row(params![sid, grant_id], |r| r.get(0))
                .optional()?;
            Ok(stored.flatten().is_some_and(|h| {
                crate::secret::constant_time_eq(h.as_bytes(), secret.sha256_hex().as_bytes())
            }))
        })
    }

    /// Records that the session is alive. An ended session is returned as is:
    /// it is never revived.
    ///
    /// # Errors
    /// `NotFound` when no such session exists.
    pub fn heartbeat(&self, id: &str) -> Result<Session> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sessions SET last_seen_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![id, self.now()],
            )?;
            fetch(tx, id).map_err(not_found)
        })
    }

    /// Names live session `sid` `name` (spec §11, M22), recording
    /// `agent.name` under `ctx` when the name or its source changes, given
    /// by `source`:
    /// the agent itself ([`NameSource::Agent`]) or its person or the owner
    /// ([`NameSource::Person`], which the agent can no longer change).
    /// `name` is compared and stored lowercase.
    ///
    /// # Errors
    /// `NotFound` for no such session; `unknown_session` for an ended one;
    /// `name_locked` when the agent renames a session its person named;
    /// `invalid_name` for a name outside `[a-z0-9][a-z0-9._-]{0,31}`, one
    /// ending in `.`, `_` or `-`, or a reserved word; `name_taken` for another live session's name
    /// (ignoring case) or the token form of the owner's or a connected
    /// person's display name.
    pub fn set_session_name(
        &self,
        ctx: &AuditCtx,
        sid: &str,
        name: &str,
        source: NameSource,
    ) -> Result<Session> {
        let invalid = || {
            CoreError::invalid(
                "invalid_name",
                "an agent name is 1 to 32 of a-z, 0-9, '.', '_' and '-', starting and ending with a letter or digit, and not a reserved word",
            )
        };
        if !name.is_ascii() {
            return Err(invalid());
        }
        let name = name.to_ascii_lowercase();
        // A chosen name also ends in a letter or digit, as derived ones do:
        // a trailing `.` would read as a sentence's end after `@name`.
        if !crate::names::is_valid(&name)
            || name.ends_with(['.', '_', '-'])
            || crate::names::RESERVED.contains(&name.as_str())
        {
            return Err(invalid());
        }
        self.with_tx(|tx| {
            let s = fetch(tx, sid).map_err(not_found)?;
            if s.ended_at.is_some() {
                return Err(CoreError::invalid(
                    "unknown_session",
                    format!("session {sid} has ended"),
                ));
            }
            if source == NameSource::Agent
                && s.name_source.as_deref() == Some(NameSource::Person.as_str())
            {
                return Err(CoreError::invalid(
                    "name_locked",
                    "this agent's person named it; only they can rename it",
                ));
            }
            let taken = || CoreError::invalid("name_taken", format!("the name '{name}' is taken"));
            if crate::names::is_reserved(&name, &people_names(tx)?)
                || live_name_taken(tx, &name, Some(sid))?
            {
                return Err(taken());
            }
            tx.execute(
                "UPDATE sessions SET name = ?2, name_source = ?3 WHERE id = ?1",
                params![sid, name, source.as_str()],
            )
            .map_err(taken_on_conflict)?;
            if s.name.as_deref() != Some(name.as_str())
                || s.name_source.as_deref() != Some(source.as_str())
            {
                let mut rec = AuditRecord::new(AuditKind::AgentName, self.now())
                    .with("name", name.as_str())
                    .with("source", source.as_str())
                    .with("previous", s.name.clone());
                rec.ids.session = Some(sid.to_string());
                self.record_audit(tx, ctx, rec)?;
                self.agents_changed(tx, sid)?;
            }
            fetch(tx, sid)
        })
    }

    /// The live session whose agent handle is `handle`.
    pub fn live_session_by_handle(&self, handle: &str) -> Result<Option<Session>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE agent_handle = ?1 AND ended_at IS NULL"),
                params![handle],
                row_to_session,
            )
            .optional()?)
        })
    }

    /// Every live session's ID and name, for `@name` (spec §11).
    pub fn live_agent_names(&self) -> Result<Vec<(String, String)>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, name FROM sessions WHERE ended_at IS NULL AND name IS NOT NULL
                 ORDER BY started_at, id",
            )?;
            Ok(stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Ends the session, drops its watches, releases its undelivered
    /// feedback, and withdraws its open questions (see
    /// [`Store::end_session_touched`]). Ending an ended session keeps its
    /// first `ended_at`.
    ///
    /// # Errors
    /// `NotFound` when no such session exists.
    pub fn end_session(&self, ctx: &AuditCtx, id: &str) -> Result<Session> {
        self.end_session_touched(ctx, id).map(|e| e.session)
    }

    /// [`Store::end_session`], also returning the feedback it released (rows
    /// deleted because another live session is a target of the same comment,
    /// or untargeted for the next session that publishes or watches) and the
    /// IDs of the open questions it withdrew. Ending a live session records
    /// `session.end` (reason `explicit`) under `ctx`, then a
    /// `question.withdraw` (reason `session_end`) for each question
    /// withdrawn; ending an ended one records nothing. The session's `ended_reason` is `exit`.
    pub fn end_session_touched(&self, ctx: &AuditCtx, id: &str) -> Result<EndedSession> {
        self.end_session_for(ctx, id, EndReason::Exit)
    }

    /// Ends live session `id` because its person or the owner ended it
    /// from their browser (spec 2026-10-08-multi-agent-multi-person §14,
    /// `ended_reason` `person`), with the cleanup every end does
    /// ([`Store::end_session_touched`]); `session.end` is recorded with
    /// reason `person` under `ctx`. An ended session is left as it was.
    ///
    /// # Errors
    /// `NotFound` when no such session exists.
    pub fn end_session_by_person(&self, ctx: &AuditCtx, id: &str) -> Result<EndedSession> {
        self.end_session_for(ctx, id, EndReason::Person)
    }

    fn end_session_for(&self, ctx: &AuditCtx, id: &str, reason: EndReason) -> Result<EndedSession> {
        self.with_tx(|tx| {
            let now = self.now();
            let c = self
                .end_in(tx, ctx, id, &now, reason, None)?
                .unwrap_or_default();
            Ok(EndedSession {
                session: fetch(tx, id).map_err(not_found)?,
                touched: c.touched,
                withdrawn_questions: c.withdrawn_questions,
                withdrawn_requests: c.withdrawn_requests,
                released_threads: c.released_threads,
            })
        })
    }

    /// Records the `CODEX_HOME` the session's Codex runs with, for `codex queue`.
    pub fn set_codex_home(&self, session_id: &str, codex_home: &str) -> Result<()> {
        self.with_tx(|c| {
            c.execute(
                "INSERT INTO session_env (session_id, codex_home) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET codex_home = excluded.codex_home",
                params![session_id, codex_home],
            )?;
            Ok(())
        })
    }

    /// The `CODEX_HOME` recorded for the session, if any.
    pub fn codex_home(&self, session_id: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT codex_home FROM session_env WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// Records the outcome of the latest `codex queue` run for the session:
    /// `Some(reason)` stores the failure with the current time, `None` (a
    /// success) clears it.
    pub fn set_push_error(&self, session_id: &str, error: Option<&str>) -> Result<()> {
        let at = error.map(|_| self.now());
        self.with_tx(|c| {
            c.execute(
                "INSERT INTO session_env (session_id, push_error, push_error_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET push_error = excluded.push_error,
                    push_error_at = excluded.push_error_at",
                params![session_id, error, at],
            )?;
            Ok(())
        })
    }

    /// The last `codex queue` failure recorded for the session and when, if
    /// the latest run failed.
    pub fn push_error(&self, session_id: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT push_error, push_error_at FROM session_env
                 WHERE session_id = ?1 AND push_error IS NOT NULL",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1"),
                params![id],
                row_to_session,
            )
            .optional()?)
        })
    }

    /// Session `id` as the agent actor of the events it makes (audit spec
    /// §5.2): its harness, harness session ID, agent handle and transcript
    /// path, and the grant and person of a person's agent. Ended sessions
    /// count: the agent still acted. `None` when no
    /// session has that ID.
    pub fn session_actor(&self, id: &str) -> Result<Option<AgentActor>> {
        self.with_read(|c| agent_of(c, id))
    }

    /// The sessions of credential `grant_id` (`None`: the daemon token's),
    /// newest first; only those not ended when `live_only`.
    pub fn list_sessions_for(
        &self,
        grant_id: Option<&str>,
        live_only: bool,
    ) -> Result<Vec<Session>> {
        self.with_read(|c| {
            let live = if live_only {
                "AND ended_at IS NULL"
            } else {
                ""
            };
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE grant_id IS ?1 {live} ORDER BY started_at DESC, id DESC"
            ))?;
            Ok(stmt
                .query_map(params![grant_id], row_to_session)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Session `sid` when it is one of credential `grant_id`'s (`None`: the
    /// daemon token's); `None` for a missing session or another
    /// credential's. Ownership only: an ended session is returned too, so a
    /// caller that needs a live one checks `ended_at` itself.
    pub fn session_of(&self, sid: &str, grant_id: Option<&str>) -> Result<Option<Session>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1 AND grant_id IS ?2"),
                params![sid, grant_id],
                row_to_session,
            )
            .optional()?)
        })
    }

    /// Sessions, newest first; only those not ended when `live_only`.
    pub fn list_sessions(&self, live_only: bool) -> Result<Vec<Session>> {
        self.with_read(|c| {
            let filter = if live_only {
                "WHERE ended_at IS NULL"
            } else {
                ""
            };
            let mut stmt = c.prepare(&format!(
                "{SELECT} {filter} ORDER BY started_at DESC, id DESC"
            ))?;
            Ok(stmt
                .query_map([], row_to_session)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Ends live sessions not seen for long enough (spec 2026-10-08 §14):
    /// a local one (no `grant_id`) unseen for `idle` whose `pid` is unknown
    /// or no longer alive; a remote one (with a `grant_id`) unseen for
    /// [`Store::REMOTE_SESSION_IDLE_SECS`] of the daemon's uptime (counted
    /// from no earlier than [`Store::resumed_at`]: it could not be heard
    /// while the daemon was down or its machine slept), whose process is
    /// never probed: it runs on another machine. Each ends as
    /// [`Store::end_session_touched`] ends one, for `idle`: its direct
    /// watches saved in `ended_watches_json` first, `session.end` (reason
    /// `ttl`) recorded as `system:ttl`, naming the session's agent in
    /// `for_actor`, and the rest of the cleanup likewise. Returns the
    /// sessions ended, the feedback and threads released, and the questions
    /// withdrawn.
    pub fn reap_sessions(&self, idle: Duration, pid_alive: &dyn Fn(u32) -> bool) -> Result<Reaped> {
        let now = self.now_utc();
        let local = super::stamp(now - idle);
        let remote = super::stamp(now - Duration::from_secs(Self::REMOTE_SESSION_IDLE_SECS));
        // A remote session could not reach the daemon while it was down or
        // its machine slept: its idle time counts from the resume.
        let resumed = self.resumed_at();
        let stale = self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, pid, grant_id IS NOT NULL FROM sessions WHERE ended_at IS NULL
                   AND ((grant_id IS NULL AND last_seen_at < ?1)
                     OR (grant_id IS NOT NULL AND max(last_seen_at, ?3) < ?2))",
            )?;
            Ok(stmt
                .query_map(params![local, remote, resumed], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<u32>>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        // Checked outside the transaction; the update below re-checks that
        // each session is still live and idle.
        let dead: Vec<(String, bool)> = stale
            .into_iter()
            .filter(|(_, pid, remote)| *remote || !pid.is_some_and(pid_alive))
            .map(|(id, _, remote)| (id, remote))
            .collect();
        if dead.is_empty() {
            return Ok(Reaped::default());
        }
        self.with_tx(|tx| {
            let now = self.now();
            let mut reaped = Reaped::default();
            let ttl = AuditCtx::system(SystemReason::Ttl);
            for (id, is_remote) in dead {
                let cutoff = if is_remote { &remote } else { &local };
                let Some(c) = self.end_in(tx, &ttl, &id, &now, EndReason::Idle, Some(cutoff))?
                else {
                    continue;
                };
                reaped.touched.merge(c.touched);
                reaped.withdrawn_questions.extend(c.withdrawn_questions);
                reaped.ended.push(id);
            }
            Ok(reaped)
        })
    }
}

impl Store {
    /// Gives new session `id` back what its harness session's last end
    /// saved (spec 2026-10-08 §6.7): when the newest ended row with the
    /// same harness, harness session ID and credential (`grant_id`, `NULL`
    /// for the token) ended `idle` within [`Store::RESTORE_WINDOW_SECS`],
    /// the direct watches in its `ended_watches_json` on artifacts still
    /// there are made again (each `watch.start` under `ctx`); any other end
    /// restores nothing. Rights and visibility still apply at every take.
    fn restore_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
    ) -> Result<Option<Restored>> {
        let prev: Option<(String, Option<String>, String, Option<String>)> = tx
            .query_row(
                "SELECT o.id, o.ended_reason, o.ended_at, o.ended_watches_json
                 FROM sessions me JOIN sessions o
                   ON o.harness = me.harness AND o.harness_session_id = me.harness_session_id
                  AND o.grant_id IS me.grant_id AND o.id <> me.id AND o.ended_at IS NOT NULL
                 WHERE me.id = ?1
                 ORDER BY o.ended_at DESC, o.id DESC LIMIT 1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((from, reason, ended_at, saved)) = prev else {
            return Ok(None);
        };
        let window = super::stamp(self.now_utc() - Duration::from_secs(Self::RESTORE_WINDOW_SECS));
        if reason.as_deref() != Some(EndReason::Idle.as_str()) || ended_at < window {
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct Saved {
            artifact_id: String,
            replies_armed: bool,
        }
        // Written by the end above; a value that does not parse restores
        // nothing rather than failing the registration.
        let saved: Vec<Saved> = saved
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let now = self.now();
        // A grant's session watches only where its rights let it now (its
        // person's, its grant's and its own narrowing): a watch PUT would
        // be refused elsewhere.
        let (grant, person): (Option<String>, Option<String>) = tx.query_row(
            "SELECT grant_id, person FROM sessions WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let rights = match &grant {
            Some(g) => Some(super::people::grant_caller_in(tx, g, Some(id))?.rights),
            None => None,
        };
        let mut artifacts = Vec::new();
        for w in saved {
            if !super::threads::artifact_live(tx, &w.artifact_id)? {
                continue;
            }
            if let Some(r) = &rights {
                let creator: Option<String> = tx.query_row(
                    "SELECT person FROM artifacts WHERE id = ?1",
                    params![w.artifact_id],
                    |r| r.get(0),
                )?;
                let own = |x: &str| x == w.artifact_id && creator == person;
                if !r.allows(
                    crate::rights::RightType::Comments,
                    "watch",
                    Some(&w.artifact_id),
                    &own,
                ) {
                    continue;
                }
            }
            let n = tx.execute(
                "INSERT OR IGNORE INTO watches (session_id, artifact_id, replies_armed, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, w.artifact_id, w.replies_armed, now],
            )?;
            if n > 0 {
                let rec = super::watches::artifact_watch_record(
                    tx,
                    AuditKind::WatchStart,
                    &now,
                    id,
                    &w.artifact_id,
                    w.replies_armed,
                    "direct",
                )?;
                self.record_audit(tx, ctx, rec)?;
                artifacts.push(w.artifact_id);
            }
        }
        let detail: Option<String> = tx
            .query_row(
                "SELECT detail_json FROM inbox_items WHERE kind = 'ended' AND session_id = ?1 LIMIT 1",
                params![from],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let threads: Vec<String> = detail
            .as_deref()
            .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
            .and_then(|d| {
                d["threads"].as_array().map(|ts| {
                    ts.iter()
                        .filter_map(|t| t["thread_id"].as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default();
        if !artifacts.is_empty() {
            // Their Send menus offer the agent again.
            self.agents_changed(tx, id)?;
        }
        let window = if grant.is_some() {
            Self::REMOTE_SESSION_IDLE_SECS
        } else {
            Self::SESSION_IDLE_SECS
        };
        let note = restore_note(window / 60, &ended_at, &threads);
        Ok(Some(Restored {
            from,
            ended_at,
            artifacts,
            threads,
            note,
        }))
    }
}

/// The note a restored session's first tool result carries (spec
/// 2026-10-08 §6.7), naming the window it went unseen for (`minutes`: 30
/// for another machine's agent, 5 for this machine's) and the threads its
/// end released.
pub fn restore_note(minutes: u64, ended_at: &str, threads: &[String]) -> String {
    let released = match threads.len() {
        0 => "no threads were released".to_string(),
        1 => format!("1 thread was released ({})", threads[0]),
        n => format!("{n} threads were released ({})", threads.join(", ")),
    };
    format!(
        "[clax] Your session ended after {minutes} minutes without contact ({ended_at}): {released} and your questions withdrawn; your watches were restored."
    )
}

/// One agent of the agents roster (spec 2026-10-08-multi-agent-multi-person
/// §13): a live session, or one that ended within
/// [`Store::ROSTER_ENDED_SECS`].
#[derive(Clone, Debug, PartialEq)]
pub struct RosterAgent {
    pub session: Session,
    /// What it waits on: its open questions and open approval requests
    /// (counted as the inbox's agent strip counts them), and when the
    /// oldest was made; `None` when it waits on nothing.
    pub blocked_since: Option<String>,
    pub questions: u32,
    pub approvals: u32,
    /// Whether it waits on its person: an open question or an open `ask`
    /// request (`state: waiting`).
    pub asking: bool,
    /// When the oldest open question or `ask` request was made.
    pub asking_since: Option<String>,
    /// The open threads it owns.
    pub threads_owned: u32,
}

impl Store {
    /// How long an ended agent stays in the roster (spec §13, §14 item 7).
    pub const ROSTER_ENDED_SECS: u64 = 3600;

    /// The agents roster (spec 2026-10-08 §13): every live session and
    /// every one that ended within [`Store::ROSTER_ENDED_SECS`], live ones
    /// first, newest first, each with what it waits on and the open
    /// threads it owns. Callers scope it to a reader.
    pub fn roster(&self) -> Result<Vec<RosterAgent>> {
        use std::collections::HashMap;
        let since = super::stamp(self.now_utc() - Duration::from_secs(Self::ROSTER_ENDED_SECS));
        self.with_read(|c| {
            let sessions: Vec<Session> = {
                let mut stmt = c.prepare_cached(&format!(
                    "{SELECT} WHERE ended_at IS NULL OR ended_at >= ?1
                     ORDER BY ended_at IS NOT NULL, started_at DESC, id DESC"
                ))?;
                stmt.query_map(params![since], row_to_session)?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut owned: HashMap<String, u32> = HashMap::new();
            {
                let mut stmt = c.prepare_cached(
                    "SELECT target_session_id, count(*) FROM threads
                     WHERE target_session_id IS NOT NULL AND status = 'open'
                     GROUP BY target_session_id",
                )?;
                for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                    let (sid, n) = row?;
                    owned.insert(sid, n);
                }
            }
            let mut out = Vec::with_capacity(sessions.len());
            for s in sessions {
                let q: (u32, Option<String>) = c
                    .prepare_cached(
                        "SELECT count(*), min(created_at) FROM questions INDEXED BY questions_by_session
                         WHERE session_id = ?1 AND status = 'open'",
                    )?
                    .query_row(params![s.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let a: (u32, Option<String>, u32, Option<String>) = c
                    .prepare_cached(
                        "SELECT count(*), min(created_at),
                                count(*) FILTER (WHERE cause = 'ask'),
                                min(created_at) FILTER (WHERE cause = 'ask')
                         FROM approval_requests INDEXED BY approval_requests_by_session
                         WHERE session_id = ?1 AND status = 'open'",
                    )?
                    .query_row(params![s.id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?;
                let earliest = |x: Option<String>, y: Option<String>| match (x, y) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (x, y) => x.or(y),
                };
                out.push(RosterAgent {
                    threads_owned: owned.get(&s.id).copied().unwrap_or(0),
                    blocked_since: earliest(q.1.clone(), a.1),
                    questions: q.0,
                    approvals: a.0,
                    asking: q.0 > 0 || a.2 > 0,
                    asking_since: earliest(q.1, a.3),
                    session: s,
                });
            }
            Ok(out)
        })
    }
}

/// Why a session ended (`sessions.ended_reason`, spec §14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Its end hook or its shim's exit.
    Exit,
    /// The reaper.
    Idle,
    /// Its grant or token was revoked, or its grant expired.
    Revoked,
    /// Its person or the owner ended it.
    Person,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Exit => "exit",
            EndReason::Idle => "idle",
            EndReason::Revoked => "revoked",
            EndReason::Person => "person",
        }
    }
}

impl EndReason {
    /// The reason `session.end` records (audit spec §6.8): `explicit` for
    /// an exit, `ttl` for the reaper's end, else this reason's own name.
    fn audit_reason(self) -> &'static str {
        match self {
            EndReason::Exit => "explicit",
            EndReason::Idle => "ttl",
            EndReason::Revoked | EndReason::Person => self.as_str(),
        }
    }
}

/// What ending one session cleaned up (spec 2026-10-08-multi-agent-multi-
/// person §14).
#[derive(Debug, Default)]
pub(crate) struct Cleanup {
    /// Feedback released, and every thread released (in `threads`).
    pub touched: crate::feedback::Touched,
    pub withdrawn_questions: Vec<String>,
    pub withdrawn_requests: Vec<String>,
    /// Every thread it owned, now released.
    pub released_threads: Vec<String>,
}

impl Store {
    /// Ends live session `id` for `reason` in `tx` under `ctx` (spec
    /// 2026-10-08-multi-agent-multi-person §14), all in `tx`: records
    /// `ended_at` and the reason (an `idle` end first saves the session's
    /// direct watches to `ended_watches_json`); withdraws its open `ask`
    /// approval requests (routed ones stay); releases every thread it owns
    /// (`thread.release`, reason `grant_revoked` for a revoked end, else
    /// `session_end`); clears its narrowing and elevation (`agent.narrow`,
    /// `agent.elevate` with `access` null, bumping the visibility
    /// generation); ends its watches and releases its undelivered feedback;
    /// withdraws its open questions; and, when it owned open threads or had
    /// open questions or `ask` requests, makes one `ended` inbox item for its
    /// person (M25). With `unseen_before`, ends it only when it was last
    /// seen before then. `None` when it was not ended here. Everything is
    /// recorded under `ctx`, `session.end` ([`EndReason::audit_reason`])
    /// first; a system actor names the session's agent in `for_actor`.
    pub(super) fn end_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        now: &str,
        reason: EndReason,
        unseen_before: Option<&str>,
    ) -> Result<Option<Cleanup>> {
        let n = tx.execute(
            "UPDATE sessions SET ended_at = ?2, ended_reason = ?3, secret_sha256 = NULL
             WHERE id = ?1 AND ended_at IS NULL AND (?4 IS NULL OR last_seen_at < ?4)",
            params![id, now, reason.as_str(), unseen_before],
        )?;
        if n == 0 {
            return Ok(None);
        }
        // Before its watches go: their artifacts' Send menus drop it.
        self.agents_changed(tx, id)?;
        if reason == EndReason::Idle {
            tx.execute(
                "UPDATE sessions SET ended_watches_json = (
                    SELECT json_group_array(json_object('artifact_id', artifact_id,
                                                        'replies_armed', json(CASE WHEN replies_armed THEN 'true' ELSE 'false' END)))
                    FROM (SELECT artifact_id, replies_armed FROM watches
                          WHERE session_id = ?1 AND source = 'direct' ORDER BY created_at, artifact_id))
                 WHERE id = ?1",
                params![id],
            )?;
        }
        self.record_session_end(tx, ctx, id, reason.audit_reason())?;
        // What it asked its person for goes with it; routed requests stay.
        let withdrawn_requests = super::approvals::withdraw_session_in(self, tx, ctx, id, now)?;
        let released = self.release_owned_in(tx, ctx, id, reason)?;
        self.clear_rights_in(tx, ctx, id)?;
        let mut touched = super::feedback::release_session(tx, id, now)?;
        let withdrawn_questions = self.withdraw_session_questions(tx, ctx, id, now)?;
        let mut note = super::inbox::EndedNote {
            threads: Vec::new(),
            questions: withdrawn_questions.len(),
            requests: withdrawn_requests.len(),
        };
        // A person never hears of a live page (L10): their note leaves out
        // its threads.
        let person = super::inbox::person_of_session(tx, id)?;
        for (aid, tid, open) in &released {
            touched.threads.insert((aid.clone(), tid.clone()));
            if *open && (person.is_none() || !super::inbox::is_live_page(tx, aid)?) {
                note.threads.push((aid.clone(), tid.clone()));
            }
        }
        let (name, harness): (Option<String>, String) = tx.query_row(
            "SELECT name, harness FROM sessions WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let name = name.unwrap_or(harness);
        super::inbox::note_ended(tx, id, &name, reason.as_str(), &note, now)?;
        Ok(Some(Cleanup {
            touched,
            withdrawn_questions,
            withdrawn_requests,
            released_threads: released.into_iter().map(|(_, t, _)| t).collect(),
        }))
    }

    /// Releases every thread session `id` owns, as its end (`reason`) does:
    /// the owner is cleared and `thread.release {thread_id, session,
    /// reason}` recorded for each (`grant_revoked` for a revoked end, else
    /// `session_end`). Returns `(artifact ID, thread ID, open)` of each.
    fn release_owned_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        reason: EndReason,
    ) -> Result<Vec<(String, String, bool)>> {
        let owned: Vec<(String, String, bool)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT artifact_id, id, status = 'open' FROM threads
                 WHERE target_session_id = ?1 ORDER BY created_at, id",
            )?;
            stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        if owned.is_empty() {
            return Ok(owned);
        }
        tx.execute(
            "UPDATE threads SET target_session_id = NULL WHERE target_session_id = ?1",
            params![id],
        )?;
        let why = match reason {
            EndReason::Revoked => "grant_revoked",
            _ => "session_end",
        };
        let now = self.now();
        for (aid, tid, _) in &owned {
            let rec = super::threads::thread_record(AuditKind::ThreadRelease, &now, aid, tid)
                .with("thread_id", tid.as_str())
                .with("session", id)
                .with("reason", why);
            let mut rec = with_for_actor(tx, ctx, rec, id, AgentActor::default)?;
            rec.ids.session = Some(id.to_string());
            self.record_audit(tx, ctx, rec)?;
        }
        Ok(owned)
    }

    /// Clears session `id`'s narrowing and elevation, as its end does: each
    /// one set is cleared, recorded (`agent.narrow` or `agent.elevate`
    /// `{session, access: null}`) and marks the write as changing what
    /// agents may see (M17).
    fn clear_rights_in(&self, tx: &Transaction<'_>, ctx: &AuditCtx, id: &str) -> Result<()> {
        let (narrow, elevate): (Option<String>, Option<String>) = tx.query_row(
            "SELECT narrow_json, elevate_json FROM sessions WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if narrow.is_none() && elevate.is_none() {
            return Ok(());
        }
        tx.execute(
            "UPDATE sessions SET narrow_json = NULL, elevate_json = NULL WHERE id = ?1",
            params![id],
        )?;
        self.visibility_changed();
        let now = self.now();
        for (kind, was) in [
            (AuditKind::AgentNarrow, narrow),
            (AuditKind::AgentElevate, elevate),
        ] {
            if was.is_none() {
                continue;
            }
            let rec = AuditRecord::new(kind, &now)
                .with("session", id)
                .with("access", serde_json::Value::Null)
                .with("reason", "session_end");
            let mut rec = with_for_actor(tx, ctx, rec, id, AgentActor::default)?;
            rec.ids.session = Some(id.to_string());
            self.record_audit(tx, ctx, rec)?;
        }
        Ok(())
    }
}

fn not_found(e: CoreError) -> CoreError {
    match e {
        CoreError::Db(rusqlite::Error::QueryReturnedNoRows) => CoreError::NotFound,
        e => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Home;
    use crate::store::test_util::DAEMON;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    fn join(
        harness: &str,
        parent_pid: u32,
        harness_session_id: &str,
        cwd: Option<&str>,
        ancestor_pids: &[u32],
    ) -> JoinSession {
        JoinSession {
            harness: harness.into(),
            parent_pid,
            harness_session_id: harness_session_id.into(),
            cwd: cwd.map(String::from),
            ancestor_pids: ancestor_pids.to_vec(),
            transcript_path: None,
        }
    }

    fn reg(
        harness_session_id: Option<&str>,
        pid: Option<u32>,
        parent: Option<u32>,
    ) -> RegisterSession {
        RegisterSession {
            harness: "claude".into(),
            harness_session_id: harness_session_id.map(String::from),
            cwd: "/work".into(),
            pid,
            parent_pid: parent,
            ..Default::default()
        }
    }

    #[test]
    fn session_actor_names_the_harness_session_and_transcript() {
        let (_d, st) = store();
        let s = st
            .register_session(DAEMON, reg(Some("hs-1"), None, None))
            .unwrap();
        st.writer
            .run(|c| {
                c.execute(
                    "UPDATE sessions SET transcript_path = '/t/hs-1.jsonl' WHERE id = ?1",
                    params![s.id],
                )?;
                Ok(())
            })
            .unwrap();
        st.end_session(DAEMON, &s.id).unwrap();
        assert_eq!(
            st.session_actor(&s.id).unwrap(),
            Some(crate::audit::AgentActor {
                session_id: Some(s.id.clone()),
                harness: Some("claude".into()),
                harness_session_id: Some("hs-1".into()),
                agent_handle: Some(s.agent_handle.clone()),
                transcript_path: Some("/t/hs-1.jsonl".into()),
                ..Default::default()
            })
        );
        assert_eq!(st.session_actor(&crate::new_ulid()).unwrap(), None);
    }

    #[test]
    fn phase_1_database_upgrades_to_2() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = rusqlite::Connection::open(home.db_path()).unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[0])
                .unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
        }
        let store = Store::open(&home).unwrap();
        let version: u32 = store
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, super::super::migrations::MIGRATIONS.len() as u32);
        assert!(store.list_sessions(false).unwrap().is_empty());
    }

    #[test]
    fn a_phase_3_database_upgrades_with_docs_and_leases() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        // Version 6 is the last phase 3 schema.
        let before = 6;
        {
            let c = rusqlite::Connection::open(home.db_path()).unwrap();
            for sql in &super::super::migrations::MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
        }
        let store = Store::open(&home).unwrap();
        let (version, docs, leases): (u32, i64, i64) = store
            .with_read(|c| {
                Ok((
                    c.query_row("PRAGMA user_version", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM leases", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(
            (version, docs, leases),
            (super::super::migrations::MIGRATIONS.len() as u32, 0, 0)
        );
    }

    #[test]
    fn phase_2_database_upgrades_to_3() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = rusqlite::Connection::open(home.db_path()).unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[0])
                .unwrap();
            c.execute_batch(super::super::migrations::MIGRATIONS[1])
                .unwrap();
            c.pragma_update(None, "user_version", 2).unwrap();
        }
        let store = Store::open(&home).unwrap();
        let version: u32 = store
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, super::super::migrations::MIGRATIONS.len() as u32);
        for table in [
            "watches",
            "threads",
            "comments",
            "feedback",
            "viewers",
            "session_env",
        ] {
            let n: i64 = store
                .with_read(|c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        [table],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn register_with_harness_id_is_idempotent() {
        let (_d, store) = store();
        let a = store
            .register_session(DAEMON, reg(Some("h1"), Some(10), Some(5)))
            .unwrap();
        let b = store
            .register_session(DAEMON, reg(Some("h1"), Some(11), None))
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.pid, Some(11));
        assert_eq!(b.parent_pid, Some(5));
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn register_after_end_starts_a_new_session() {
        let (_d, store) = store();
        let a = store
            .register_session(DAEMON, reg(Some("h1"), None, None))
            .unwrap();
        store.end_session(DAEMON, &a.id).unwrap();
        let b = store
            .register_session(DAEMON, reg(Some("h1"), None, None))
            .unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn shim_first_then_hook_joins() {
        let (_d, store) = store();
        let shim = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.harness_session_id, None);
        let joined = store
            .join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        assert_eq!(joined.id, shim.id);
        assert_eq!(joined.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(joined.pid, Some(10));
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn hook_joins_a_shim_row_registered_under_an_ancestor() {
        let (_d, store) = store();
        let shim = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        let other = store
            .register_session(DAEMON, reg(None, Some(11), Some(7)))
            .unwrap();
        // Hook's parent is a wrapper shell (4); the harness (5) is next.
        let joined = store
            .join_session(DAEMON, join("claude", 4, "h1", None, &[5, 7]), None)
            .unwrap();
        assert_eq!(joined.id, shim.id, "nearest matching ancestor wins");
        assert_eq!(joined.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(joined.parent_pid, Some(5));
        assert_ne!(joined.id, other.id);
        assert_eq!(store.list_sessions(true).unwrap().len(), 2);
    }

    #[test]
    fn hook_first_then_shim_adopts() {
        let (_d, store) = store();
        let hook = store
            .join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        assert_eq!(hook.pid, None);
        let shim = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.id, hook.id);
        assert_eq!(shim.pid, Some(10));
        assert_eq!(shim.harness_session_id.as_deref(), Some("h1"));
        assert_eq!(shim.cwd, "/work");
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    #[test]
    fn hook_cwd_is_used_for_new_rows_and_fills_empty_ones() {
        let (_d, store) = store();
        let hook = store
            .join_session(DAEMON, join("claude", 5, "h1", Some("/hook"), &[]), None)
            .unwrap();
        assert_eq!(hook.cwd, "/hook");
        let shim = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.cwd, "/hook", "an existing cwd is kept");
        let empty = store
            .join_session(DAEMON, join("codex", 8, "c1", None, &[]), None)
            .unwrap();
        assert_eq!(empty.cwd, "");
        let filled = store
            .join_session(DAEMON, join("codex", 8, "c1", Some("/late"), &[]), None)
            .unwrap();
        assert_eq!(filled.id, empty.id);
        assert_eq!(filled.cwd, "/late");
    }

    #[test]
    fn registrations_without_id_under_one_parent_share_a_row() {
        let (_d, store) = store();
        let a = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        let b = store
            .register_session(DAEMON, reg(None, Some(11), Some(5)))
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.pid, Some(11));
        assert_eq!(store.list_sessions(true).unwrap().len(), 1);
        let joined = store
            .join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        assert_eq!(joined.id, a.id);
    }

    #[test]
    fn stale_row_is_not_adopted_by_parent() {
        let (_d, store) = store();
        let old = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        store
            .with_write(|c| {
                c.execute(
                    "UPDATE sessions SET last_seen_at = ?2 WHERE id = ?1",
                    params![
                        old.id,
                        (chrono::Utc::now() - Duration::from_secs(600))
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let new = store
            .register_session(DAEMON, reg(None, Some(12), Some(5)))
            .unwrap();
        assert_ne!(new.id, old.id);
    }

    #[test]
    fn hook_join_is_idempotent_and_other_harnesses_stay_apart() {
        let (_d, store) = store();
        let a = store
            .join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        let b = store
            .join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        assert_eq!(a.id, b.id);
        let other = store
            .join_session(DAEMON, join("codex", 5, "h1", None, &[]), None)
            .unwrap();
        assert_ne!(a.id, other.id);
    }

    #[test]
    fn shim_with_harness_id_adopts_hookless_row_by_parent() {
        let (_d, store) = store();
        let shim = store
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        let again = store
            .register_session(DAEMON, reg(Some("h1"), Some(10), Some(5)))
            .unwrap();
        assert_eq!(shim.id, again.id);
        assert_eq!(again.harness_session_id.as_deref(), Some("h1"));
    }

    #[test]
    fn heartbeat_and_end() {
        let (_d, store) = store();
        let s = store
            .register_session(DAEMON, reg(None, Some(10), None))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let beat = store.heartbeat(&s.id).unwrap();
        assert!(beat.last_seen_at > s.last_seen_at);
        let ended = store.end_session(DAEMON, &s.id).unwrap();
        assert!(ended.ended_at.is_some());
        let again = store.end_session(DAEMON, &s.id).unwrap();
        assert_eq!(again.ended_at, ended.ended_at);
        assert_eq!(
            store.heartbeat(&s.id).unwrap().last_seen_at,
            ended.last_seen_at
        );
        assert!(matches!(store.heartbeat("nope"), Err(CoreError::NotFound)));
        assert!(store.list_sessions(true).unwrap().is_empty());
        assert_eq!(store.list_sessions(false).unwrap().len(), 1);
    }

    /// The kinds and bodies of the events after `seq`.
    fn recorded(st: &Store, seq: i64) -> Vec<(String, serde_json::Value, serde_json::Value)> {
        st.events_after(seq, 100)
            .unwrap()
            .into_iter()
            .map(|e| {
                (
                    e.kind,
                    serde_json::from_str(&e.actor).unwrap(),
                    serde_json::from_str(&e.body).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn session_changes_record_start_join_and_end_and_refreshes_record_nothing() {
        let (_d, st) = store();
        // After what the store records as it opens (migration 25's approval).
        let opened = st.newest_seq().unwrap();
        let s = st
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        let ev = recorded(&st, opened);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "session.start");
        // The session's own agent, through the caller's channel.
        assert_eq!(ev[0].1["type"], "agent");
        assert_eq!(ev[0].1["session_id"], s.id.as_str());
        assert_eq!(ev[0].2["via"], "daemon");
        assert_eq!(
            (&ev[0].2["harness"], &ev[0].2["cwd"], &ev[0].2["pid"]),
            (
                &serde_json::json!("claude"),
                &serde_json::json!("/work"),
                &serde_json::json!(10)
            )
        );
        assert_eq!(ev[0].2["harness_session_id"], serde_json::Value::Null);
        let seq = st.newest_seq().unwrap();
        // A refresh within the idle window changes nothing recorded.
        st.register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        st.heartbeat(&s.id).unwrap();
        assert_eq!(st.newest_seq().unwrap(), seq);
        // The hook joins the harness session ID, then its transcript.
        st.join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        st.join_session(DAEMON, join("claude", 5, "h1", None, &[]), None)
            .unwrap();
        st.join_session(
            DAEMON,
            JoinSession {
                transcript_path: Some("/t/h1.jsonl".into()),
                ..join("claude", 5, "h1", None, &[])
            },
            None,
        )
        .unwrap();
        let ev = recorded(&st, seq);
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(kinds, ["session.join", "session.join"]);
        assert_eq!(ev[0].2["harness_session_id"], "h1");
        assert_eq!(ev[0].2["transcript_path"], serde_json::Value::Null);
        assert_eq!(ev[1].2["transcript_path"], "/t/h1.jsonl");
        assert_eq!(ev[1].1["transcript_path"], "/t/h1.jsonl");
        // The shim restarted under another pid: a join.
        let seq = st.newest_seq().unwrap();
        st.register_session(DAEMON, reg(Some("h1"), Some(11), Some(5)))
            .unwrap();
        assert_eq!(recorded(&st, seq)[0].2["pid"], 11);
        let seq = st.newest_seq().unwrap();
        st.end_session(DAEMON, &s.id).unwrap();
        st.end_session(DAEMON, &s.id).unwrap();
        let ev = recorded(&st, seq);
        assert_eq!(ev.len(), 1, "ending an ended session records nothing");
        assert_eq!(
            (ev[0].0.as_str(), &ev[0].2["reason"]),
            ("session.end", &serde_json::json!("explicit"))
        );
        assert_eq!(ev[0].2["for_actor"]["session_id"], s.id.as_str());
        // A hook runs first; the shim adopts its row with its pid and Pi's
        // kind of transcript path: a join.
        let hook = st
            .join_session(DAEMON, join("codex", 77, "cx-9", Some("/w"), &[]), None)
            .unwrap();
        let seq = st.newest_seq().unwrap();
        let adopted = st
            .register_session(
                DAEMON,
                RegisterSession {
                    harness: "codex".into(),
                    harness_session_id: Some("cx-9".into()),
                    cwd: "/w".into(),
                    pid: Some(78),
                    parent_pid: Some(77),
                    transcript_path: Some("/t/cx-9.jsonl".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(adopted.id, hook.id);
        let ev = recorded(&st, seq);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "session.join");
        assert_eq!(
            (&ev[0].2["pid"], &ev[0].2["transcript_path"]),
            (&serde_json::json!(78), &serde_json::json!("/t/cx-9.jsonl"))
        );
        // Reaped: Clax ends it for the idle agent.
        let idle = st
            .register_session(DAEMON, reg(Some("idle"), None, None))
            .unwrap();
        st.writer
            .run(|c| {
                c.execute(
                    "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
                    params![idle.id],
                )?;
                Ok(())
            })
            .unwrap();
        let seq = st.newest_seq().unwrap();
        let reaped = st
            .reap_sessions(Duration::from_secs(300), &|_| false)
            .unwrap();
        assert_eq!(reaped.ended, std::slice::from_ref(&idle.id));
        let ev = recorded(&st, seq);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "session.end");
        assert_eq!(
            ev[0].1,
            serde_json::json!({"type": "system", "reason": "ttl"})
        );
        assert_eq!(ev[0].2["reason"], "ttl");
        assert_eq!(ev[0].2["for_actor"]["session_id"], idle.id.as_str());
        assert_eq!(ev[0].2["for_actor"]["harness_session_id"], "idle");
    }

    #[test]
    fn reaper_ends_only_idle_sessions_with_dead_or_unknown_pids() {
        let (_d, store) = store();
        let dead = store
            .register_session(DAEMON, reg(Some("dead"), Some(1001), None))
            .unwrap();
        let alive = store
            .register_session(DAEMON, reg(Some("alive"), Some(1002), None))
            .unwrap();
        let hook_only = store
            .join_session(DAEMON, join("claude", 7, "hook", None, &[]), None)
            .unwrap();
        // Everything is fresh: nothing is idle yet.
        let pid_alive = |pid: u32| pid == 1002;
        assert_eq!(
            store
                .reap_sessions(Duration::from_secs(300), &pid_alive)
                .unwrap()
                .ended
                .len(),
            0
        );
        std::thread::sleep(Duration::from_millis(300));
        let fresh = store
            .register_session(DAEMON, reg(Some("fresh"), Some(1003), None))
            .unwrap();
        // Wide enough that `fresh` stays unreaped on a loaded test run.
        let reaped = store
            .reap_sessions(Duration::from_millis(200), &pid_alive)
            .unwrap();
        assert_eq!(reaped.ended.len(), 2);
        let ended = |id: &str| store.get_session(id).unwrap().unwrap().ended_at.is_some();
        assert!(ended(&dead.id));
        assert!(ended(&hook_only.id));
        assert!(!ended(&alive.id));
        assert!(!ended(&fresh.id));
    }
    #[test]
    fn reaping_releases_watches_and_rows() {
        use crate::store::test_util::{anchor, artifact};
        use crate::store::threads::NewThread;
        let (_d, store) = store();
        let s = store
            .register_session(DAEMON, reg(Some("dead"), Some(1001), None))
            .unwrap();
        let aid = artifact(&store, Some(&s.id));
        store.ensure_watch(DAEMON, &s.id, &aid).unwrap();
        let t = store
            .create_thread(
                DAEMON,
                &aid,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "A".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        store.send_to_agent(DAEMON, &t.id).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let reaped = store
            .reap_sessions(Duration::from_millis(10), &|_| false)
            .unwrap();
        assert_eq!(reaped.ended, vec![s.id.clone()]);
        assert!(reaped.touched.threads.iter().any(|(_, tid)| *tid == t.id));
        assert!(store.list_watches(&s.id).unwrap().is_empty());
        assert_eq!(
            store.feedback_rows(&t.id).unwrap()[0].target_session_id,
            None
        );
    }

    #[test]
    fn push_errors_are_recorded_and_cleared_without_touching_codex_home() {
        let (_d, store) = store();
        let s = store
            .join_session(DAEMON, join("codex", 5, "cx-1", Some("/w"), &[]), None)
            .unwrap();
        assert_eq!(store.push_error(&s.id).unwrap(), None);
        store
            .set_push_error(&s.id, Some("codex queue exited with code 1"))
            .unwrap();
        let (e, at) = store.push_error(&s.id).unwrap().unwrap();
        assert_eq!(e, "codex queue exited with code 1");
        assert!(!at.is_empty());
        store.set_codex_home(&s.id, "/tmp/cxh").unwrap();
        assert!(
            store.push_error(&s.id).unwrap().is_some(),
            "codex_home keeps it"
        );
        store.set_push_error(&s.id, None).unwrap();
        assert_eq!(store.push_error(&s.id).unwrap(), None);
        assert_eq!(
            store.codex_home(&s.id).unwrap().as_deref(),
            Some("/tmp/cxh")
        );
    }

    #[test]
    fn codex_home_is_stored_per_session() {
        let (_d, store) = store();
        let s = store
            .join_session(DAEMON, join("codex", 5, "cx-1", Some("/w"), &[]), None)
            .unwrap();
        assert_eq!(store.codex_home(&s.id).unwrap(), None);
        store.set_codex_home(&s.id, "/tmp/cxh").unwrap();
        store.set_codex_home(&s.id, "/tmp/cxh2").unwrap();
        assert_eq!(
            store.codex_home(&s.id).unwrap().as_deref(),
            Some("/tmp/cxh2")
        );
    }

    mod names {
        use super::*;
        use crate::store::people::InvitePerson;
        use crate::store::people::tests::{KEY_A, approved, code, store};

        fn reg_repo(harness: &str, repo: &str, branch: &str) -> RegisterSession {
            RegisterSession {
                harness: harness.into(),
                cwd: "/w".into(),
                repo: Some(repo.into()),
                branch: Some(branch.into()),
                ..Default::default()
            }
        }

        fn assert_invalid(r: Result<Session>, want: &str) {
            assert_eq!(code(r), want);
        }

        #[test]
        fn collisions_suffix_derived_names_and_refuse_chosen_ones() {
            let (_d, _c, st) = store();
            let a = st
                .register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            let b = st
                .register_session(DAEMON, reg_repo("codex", "web", "main"))
                .unwrap();
            let c = st
                .register_session(DAEMON, reg_repo("codex", "web", "main"))
                .unwrap();
            assert_eq!(
                (a.name.as_deref(), b.name.as_deref(), c.name.as_deref()),
                (Some("web"), Some("web-codex"), Some("web-2"))
            );
            assert_eq!(a.name_source.as_deref(), Some("derived"));
            assert_invalid(
                st.set_session_name(DAEMON, &b.id, "WEB", NameSource::Agent),
                "name_taken",
            );
            let renamed = st
                .set_session_name(DAEMON, &b.id, "chart-axis", NameSource::Person)
                .unwrap();
            assert_eq!(
                (renamed.name.as_deref(), renamed.name_source.as_deref()),
                (Some("chart-axis"), Some("person"))
            );
            assert_invalid(
                st.set_session_name(DAEMON, &b.id, "other", NameSource::Agent),
                "name_locked",
            );
            st.set_session_name(DAEMON, &b.id, "other", NameSource::Person)
                .unwrap();
            st.end_session(DAEMON, &a.id).unwrap();
            let c = st
                .set_session_name(DAEMON, &c.id, "Web", NameSource::Agent)
                .unwrap();
            assert_eq!(
                c.name.as_deref(),
                Some("web"),
                "stored lowercase; freed on end"
            );
            let d = st
                .register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            assert_eq!(d.name.as_deref(), Some("web-claude"));
        }

        #[test]
        fn the_spec_reserves_clax_so_a_clax_repository_takes_the_harness_suffix() {
            let (_d, _c, st) = store();
            let a = st
                .register_session(DAEMON, reg_repo("claude", "clax", "main"))
                .unwrap();
            let b = st
                .register_session(DAEMON, reg_repo("codex", "clax", "main"))
                .unwrap();
            assert_eq!(
                (a.name.as_deref(), b.name.as_deref()),
                (Some("clax-claude"), Some("clax-codex"))
            );
            let m = st
                .register_session(DAEMON, reg_repo("claude", "clax", "multi-person"))
                .unwrap();
            assert_eq!(m.name.as_deref(), Some("clax-multi-person"));
            for word in crate::names::RESERVED {
                assert_invalid(
                    st.set_session_name(DAEMON, &a.id, word, NameSource::Agent),
                    "invalid_name",
                );
                assert_invalid(
                    st.set_session_name(DAEMON, &a.id, &word.to_uppercase(), NameSource::Person),
                    "invalid_name",
                );
            }
        }

        #[test]
        fn chosen_names_must_have_the_m22_form() {
            let (_d, _c, st) = store();
            let a = st
                .register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            for bad in [
                "",
                " web",
                "web app",
                "-web",
                ".web",
                "wéb",
                "\u{212A}elvin",
                "ｗｅｂ",
                "wеb",
                "web\u{200B}",
                "web\n",
                "a\u{0}",
                "web.",
                "web_",
                "web-",
                "a/b",
                "@web",
                &"x".repeat(33),
                &"x".repeat(100_000),
            ] {
                assert_invalid(
                    st.set_session_name(DAEMON, &a.id, bad, NameSource::Agent),
                    "invalid_name",
                );
            }
            assert_eq!(
                st.get_session(&a.id).unwrap().unwrap().name.as_deref(),
                Some("web"),
                "a refused name changes nothing"
            );
            let ok = st
                .set_session_name(DAEMON, &a.id, &"x".repeat(32), NameSource::Agent)
                .unwrap();
            assert_eq!(ok.name_source.as_deref(), Some("agent"));
            assert!(matches!(
                st.set_session_name(DAEMON, "nope", "x", NameSource::Agent),
                Err(CoreError::NotFound)
            ));
            st.end_session(DAEMON, &a.id).unwrap();
            assert_invalid(
                st.set_session_name(DAEMON, &a.id, "y", NameSource::Agent),
                "unknown_session",
            );
        }

        #[test]
        fn a_derived_name_skips_names_of_people() {
            let (_d, _c, st) = store();
            approved(&st, "Web", KEY_A);
            st.set_owner_name("Clax Main", false).unwrap();
            let a = st
                .register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            assert_eq!(a.name.as_deref(), Some("web-claude"));
            let b = st
                .register_session(DAEMON, reg_repo("pi", "clax", "main"))
                .unwrap();
            assert_eq!(b.name.as_deref(), Some("clax-pi"));
        }

        #[test]
        fn names_without_a_repository_are_the_harness_and_re_registration_keeps_them() {
            let (_d, _c, st) = store();
            let a = st
                .register_session(DAEMON, reg(Some("h1"), Some(10), Some(5)))
                .unwrap();
            let b = st
                .register_session(DAEMON, reg(Some("h2"), Some(11), Some(6)))
                .unwrap();
            assert_eq!(
                (a.name.as_deref(), b.name.as_deref()),
                (Some("claude"), Some("claude-2"))
            );
            let again = st
                .register_session(
                    DAEMON,
                    RegisterSession {
                        repo: Some("web".into()),
                        ..reg(Some("h1"), Some(10), Some(5))
                    },
                )
                .unwrap();
            assert_eq!(
                (again.id.as_str(), again.name.as_deref()),
                (a.id.as_str(), Some("claude"))
            );
        }

        #[test]
        fn a_hook_row_is_named_by_the_shim_that_adopts_it() {
            let (_d, _c, st) = store();
            let hook = st
                .join_session(
                    DAEMON,
                    JoinSession {
                        harness: "codex".into(),
                        parent_pid: 5,
                        harness_session_id: "cx".into(),
                        cwd: None,
                        ancestor_pids: vec![],
                        transcript_path: None,
                    },
                    None,
                )
                .unwrap();
            assert_eq!(hook.name.as_deref(), Some("codex"));
            let shim = st
                .register_session(
                    DAEMON,
                    RegisterSession {
                        harness: "codex".into(),
                        cwd: "/w".into(),
                        pid: Some(10),
                        parent_pid: Some(5),
                        repo: Some("web".into()),
                        branch: Some("fix/axis".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(
                (shim.id.as_str(), shim.name.as_deref()),
                (hook.id.as_str(), Some("web-axis"))
            );
        }

        #[test]
        fn people_and_agents_cannot_shadow_each_other() {
            let (_d, _c, st) = store();
            // A live agent "sam": a new person "Sam" is name_taken.
            let s = st
                .register_session(DAEMON, reg_repo("claude", "sam", "main"))
                .unwrap();
            assert_eq!(s.name.as_deref(), Some("sam"));
            assert_eq!(
                code(st.create_invite(DAEMON, InvitePerson::New("Sam".into()), None)),
                "name_taken"
            );
            assert_eq!(
                code(st.create_invite(DAEMON, InvitePerson::New("  SÁM ".into()), None)),
                "name_taken"
            );
            // Once it ends, the name is free for a person.
            st.end_session(DAEMON, &s.id).unwrap();
            let (sam_grant, _, _) = approved(&st, "Sam", KEY_A);
            // A person "Mia Kovač": no agent takes "mia-kovac".
            st.create_invite(DAEMON, InvitePerson::New("Mia Kovač".into()), None)
                .unwrap();
            let a = st
                .register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            assert_invalid(
                st.set_session_name(DAEMON, &a.id, "mia-kovac", NameSource::Agent),
                "name_taken",
            );
            assert_invalid(
                st.set_session_name(DAEMON, &a.id, "SAM", NameSource::Person),
                "name_taken",
            );
            let m = st
                .register_session(DAEMON, reg_repo("claude", "Mia-Kovac", "main"))
                .unwrap();
            assert_eq!(m.name.as_deref(), Some("mia-kovac-claude"), "{m:?}");
            // A LAN viewer cannot take a person's or the owner's name, in any
            // case or with any diacritics.
            let lan = crate::new_ulid();
            st.upsert_viewer(&lan, Some("Lan")).unwrap();
            for taken in [
                "sam",
                "Mia Kovač",
                "MIA  KOVAC",
                "mia kovac",
                "Ｓａｍ",
                "Sa\u{0301}m",
            ] {
                assert_eq!(
                    code(st.upsert_viewer(&lan, Some(taken))),
                    "name_taken",
                    "{taken:?}"
                );
            }
            st.set_owner_name("Alex", false).unwrap();
            assert_eq!(code(st.upsert_viewer(&lan, Some("ALEX"))), "name_taken");
            assert_eq!(
                st.get_viewer(&lan)
                    .unwrap()
                    .unwrap()
                    .display_name
                    .as_deref(),
                Some("Lan")
            );
            st.upsert_viewer(&lan, Some("Mia")).unwrap();
            st.upsert_viewer(&lan, Some("")).unwrap();
            // A connected person renaming themselves cannot take a live
            // agent's name.
            let sam = st.grant(&sam_grant).unwrap().person.unwrap();
            let sam_cookie: String = st
                .with_read(|c| {
                    Ok(
                        c.query_row("SELECT id FROM viewers WHERE public_id = ?1", [&sam], |r| {
                            r.get(0)
                        })?,
                    )
                })
                .unwrap();
            assert_eq!(
                code(st.upsert_viewer(&sam_cookie, Some("WEB"))),
                "name_taken"
            );
            st.upsert_viewer(&sam_cookie, Some("Sam K")).unwrap();
            // A live agent's name is protected from LAN viewers too.
            assert_eq!(code(st.upsert_viewer(&lan, Some("web"))), "name_taken");
            assert_eq!(
                code(st.upsert_viewer(&lan, Some("W\u{0435}b"))),
                "name_taken"
            );
        }

        #[test]
        fn the_owner_cannot_take_a_live_agents_or_a_persons_name() {
            let (_d, _c, st) = store();
            st.register_session(DAEMON, reg_repo("claude", "web", "main"))
                .unwrap();
            approved(&st, "Sam", KEY_A);
            for taken in ["web", "WEB", "Sam", "sám"] {
                assert_eq!(
                    code(st.set_owner_name(taken, true)),
                    "name_taken",
                    "{taken}"
                );
            }
            assert_eq!(
                st.set_owner_name("Alex", true)
                    .unwrap()
                    .display_name
                    .as_deref(),
                Some("Alex")
            );
            st.set_owner_name("Alex", true).unwrap();
            st.set_owner_name("", true).unwrap();
        }

        #[test]
        fn a_new_person_cannot_take_an_uninvited_viewers_name() {
            let (_d, clock, st) = store();
            let jun = crate::new_ulid();
            st.upsert_viewer(&jun, Some("Jun")).unwrap();
            let e = st.create_invite(DAEMON, InvitePerson::New("JUN".into()), None);
            assert_eq!(code(e), "name_taken");
            let msg = match st.create_invite(DAEMON, InvitePerson::New("jun".into()), None) {
                Err(CoreError::Invalid { message, .. }) => message,
                other => panic!("{other:?}"),
            };
            assert!(msg.contains("invite that viewer instead"), "{msg}");
            // Inviting that viewer is the way.
            let public = st.get_viewer(&jun).unwrap().unwrap().public_id;
            st.create_invite(DAEMON, InvitePerson::Viewer(public), None)
                .unwrap();
            // A revoked person's name is free again.
            let (mia, _, _) = approved(&st, "Mia", KEY_A);
            let mia = st.grant(&mia).unwrap().person.unwrap();
            st.revoke_person(DAEMON, &mia).unwrap();
            st.create_invite(DAEMON, InvitePerson::New("Mia".into()), None)
                .unwrap();
            // A lapsed invite's name is free again.
            st.create_invite(DAEMON, InvitePerson::New("Ola".into()), None)
                .unwrap();
            clock.advance(crate::store::people::INVITE_TTL_SECS + 1);
            st.expire_grants().unwrap();
            st.create_invite(DAEMON, InvitePerson::New("Ola".into()), None)
                .unwrap();
        }
    }

    mod binding {
        use super::*;
        use crate::store::people::tests::{KEY_A, KEY_B, approved, store};

        fn reg_pid(harness: &str, pid: u32, grant: Option<&str>) -> RegisterSession {
            RegisterSession {
                harness: harness.into(),
                parent_pid: Some(pid),
                cwd: "/w".into(),
                grant_id: grant.map(String::from),
                ..Default::default()
            }
        }

        #[test]
        fn remote_registration_never_matches_an_owner_session_with_the_same_pid() {
            let (_d, _c, st) = store();
            let owner = st
                .register_session(
                    DAEMON,
                    RegisterSession {
                        cwd: "/o".into(),
                        ..reg_pid("claude", 4242, None)
                    },
                )
                .unwrap();
            let (gid, _, _) = approved(&st, "Sam", KEY_A);
            let sam = st
                .register_session(
                    DAEMON,
                    RegisterSession {
                        cwd: "/s".into(),
                        ..reg_pid("claude", 4242, Some(&gid))
                    },
                )
                .unwrap();
            assert_ne!(sam.id, owner.id);
            let o = st.get_session(&owner.id).unwrap().unwrap();
            assert_eq!(
                (o.cwd.as_str(), o.grant_id.as_deref()),
                ("/o", None),
                "the owner's row is untouched"
            );
            let sam_pid = st.grant(&gid).unwrap().person;
            assert!(sam_pid.is_some());
            assert_eq!(sam.person, sam_pid);
            assert_eq!(sam.grant_id.as_deref(), Some(gid.as_str()));
            // And the other way round: the token never reaches Sam's row.
            let again = st
                .register_session(DAEMON, reg_pid("claude", 4242, None))
                .unwrap();
            assert_eq!(again.id, owner.id);
        }

        #[test]
        fn a_registration_body_never_names_a_grant() {
            let r: RegisterSession = serde_json::from_value(serde_json::json!({
                "harness": "claude", "cwd": "/w", "grant_id": "g_x", "repo": "web"
            }))
            .unwrap();
            assert_eq!((r.grant_id, r.repo.as_deref()), (None, Some("web")));
            let sent = serde_json::to_value(reg(None, Some(1), Some(2))).unwrap();
            assert!(sent.get("grant_id").is_none() && sent.get("repo").is_none());
        }

        #[test]
        fn harness_session_ids_are_unique_per_credential() {
            let (_d, _c, st) = store();
            let (sam, _, _) = approved(&st, "Sam", KEY_A);
            let (mia, _, _) = approved(&st, "Mia", KEY_B);
            let with = |g: Option<&str>| RegisterSession {
                harness_session_id: Some("hs-1".into()),
                ..reg_pid("claude", 1, g)
            };
            let ids: Vec<String> = [None, Some(sam.as_str()), Some(mia.as_str())]
                .into_iter()
                .map(|g| st.register_session(DAEMON, with(g)).unwrap().id)
                .collect();
            assert_eq!(
                ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
                3
            );
            assert_eq!(
                st.register_session(DAEMON, with(Some(&sam))).unwrap().id,
                ids[1]
            );
        }

        #[test]
        fn join_is_scoped_to_the_grant() {
            let (_d, _c, st) = store();
            let owner = st
                .register_session(DAEMON, reg_pid("codex", 777, None))
                .unwrap();
            let (gid, _, _) = approved(&st, "Sam", KEY_A);
            let j = st
                .join_session(DAEMON, join("codex", 777, "hs-1", None, &[777]), Some(&gid))
                .unwrap();
            assert_ne!(j.id, owner.id);
            assert_eq!(j.grant_id.as_deref(), Some(gid.as_str()));
            assert!(
                st.get_session(&owner.id)
                    .unwrap()
                    .unwrap()
                    .harness_session_id
                    .is_none()
            );
            assert!(st.session_of(&owner.id, Some(&gid)).unwrap().is_none());
            assert!(st.session_of(&j.id, None).unwrap().is_none());
            assert_eq!(st.session_of(&j.id, Some(&gid)).unwrap().unwrap().id, j.id);
            assert_eq!(
                st.session_of(&owner.id, None).unwrap().unwrap().id,
                owner.id
            );
            let mine = st.list_sessions_for(Some(&gid), true).unwrap();
            assert_eq!(mine.iter().map(|s| &s.id).collect::<Vec<_>>(), [&j.id]);
            let token = st.list_sessions_for(None, true).unwrap();
            assert_eq!(token.iter().map(|s| &s.id).collect::<Vec<_>>(), [&owner.id]);
            // The owner's join reaches the owner's row.
            let o = st
                .join_session(DAEMON, join("codex", 777, "hs-2", None, &[]), None)
                .unwrap();
            assert_eq!(o.id, owner.id);
        }

        #[test]
        fn a_revoked_grant_cannot_register_or_join() {
            let (_d, _c, st) = store();
            let (gid, _, _) = approved(&st, "Sam", KEY_A);
            st.revoke_grant(DAEMON, &gid, crate::store::people::EndedBy::Owner)
                .unwrap();
            let refused = |r: Result<Session>| {
                matches!(
                    r,
                    Err(CoreError::Invalid {
                        code: "grant_revoked",
                        ..
                    })
                )
            };
            assert!(refused(
                st.register_session(DAEMON, reg_pid("claude", 1, Some(&gid)))
            ));
            assert!(refused(st.join_session(
                DAEMON,
                join("claude", 1, "h", None, &[]),
                Some(&gid)
            )));
            assert!(refused(st.join_session(
                DAEMON,
                join("claude", 1, "h", None, &[]),
                Some("g_unknown")
            )));
        }

        #[test]
        fn ends_record_their_reason_and_idle_ends_keep_direct_watches() {
            let (_d, clock, st) = store();
            let gone = st
                .register_session(DAEMON, reg(Some("exit"), None, None))
                .unwrap();
            st.end_session(DAEMON, &gone.id).unwrap();
            let idle = st
                .register_session(DAEMON, reg(Some("idle"), None, None))
                .unwrap();
            let aid = crate::store::test_util::artifact(&st, Some(&idle.id));
            st.watch(DAEMON, &idle.id, &aid, false).unwrap();
            clock.advance(Store::SESSION_IDLE_SECS as i64 + 1);
            let reaped = st
                .reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                .unwrap();
            assert_eq!(reaped.ended, vec![idle.id.clone()]);
            let row = |id: &str| -> (Option<String>, Option<String>) {
                st.with_read(|c| {
                    Ok(c.query_row(
                        "SELECT ended_reason, ended_watches_json FROM sessions WHERE id = ?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?)
                })
                .unwrap()
            };
            assert_eq!(row(&gone.id), (Some("exit".into()), None));
            let (reason, watches) = row(&idle.id);
            assert_eq!(reason.as_deref(), Some("idle"));
            let watches: serde_json::Value = serde_json::from_str(&watches.unwrap()).unwrap();
            assert_eq!(
                watches,
                serde_json::json!([{"artifact_id": aid.as_str(), "replies_armed": false}])
            );
        }
    }

    /// Ending a session (spec 2026-10-08-multi-agent-multi-person §14, M25).
    mod cleanup {
        use super::*;
        use crate::store::approvals::{AskApproval, Route};
        use crate::store::inbox::{InboxQuery, Kind};
        use crate::store::people::EndedBy;
        use crate::store::people::tests::{KEY_A, KEY_B, approved, store};
        use crate::store::questions::{NewQuestion, Source};
        use crate::store::test_util::{anchor, artifact};
        use crate::store::threads::NewThread;
        use crate::working::ManualClock;
        use std::sync::Arc;

        /// Sam's agent, holding everything an end cleans up: a thread it
        /// owns, one of Mia's it asked Sam about, an open question and an
        /// elevation.
        struct World {
            _d: tempfile::TempDir,
            clock: Arc<ManualClock>,
            st: Store,
            grant: String,
            sam: String,
            sid: String,
            aid: crate::ArtifactId,
            owned: String,
            asked: String,
        }

        fn thread_by(st: &Store, aid: &crate::ArtifactId, by: &str) -> String {
            st.create_thread(
                DAEMON,
                aid,
                NewThread {
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "x".into(),
                    author_public_id: Some(by.to_string()),
                    body: "b".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap()
            .id
        }

        fn reg_in(grant: &str, hsid: &str) -> RegisterSession {
            RegisterSession {
                harness: "claude".into(),
                harness_session_id: Some(hsid.into()),
                cwd: "/w".into(),
                grant_id: Some(grant.into()),
                ..Default::default()
            }
        }

        /// Sam's agent with nothing to clean up yet.
        fn bare() -> World {
            let (d, clock, st) = store();
            st.owner_viewer(true).unwrap();
            let (grant, _, _) = approved(&st, "Sam", KEY_A);
            let sam = st.viewer_of_grant(&grant).unwrap().public_id;
            let sid = st
                .register_session(DAEMON, reg_in(&grant, "hs-1"))
                .unwrap()
                .id;
            let aid = artifact(&st, None);
            st.ensure_watch(DAEMON, &sid, &aid).unwrap();
            World {
                _d: d,
                clock,
                st,
                grant,
                sam,
                sid,
                aid,
                owned: String::new(),
                asked: String::new(),
            }
        }

        fn world() -> World {
            let mut w = bare();
            let (mia_grant, _, _) = approved(&w.st, "Mia", KEY_B);
            let mia = w.st.viewer_of_grant(&mia_grant).unwrap().public_id;
            w.owned = thread_by(&w.st, &w.aid, &w.sam);
            w.st.send_routed(
                DAEMON,
                &w.owned,
                crate::SendTarget::Agent(&w.sid),
                Route::Send,
                false,
            )
            .unwrap();
            w.asked = thread_by(&w.st, &w.aid, &mia);
            w.st.ask_approval(
                DAEMON,
                &AskApproval {
                    session_id: w.sid.clone(),
                    artifact_id: None,
                    thread_id: Some(w.asked.clone()),
                    author: None,
                },
            )
            .unwrap();
            w.st.create_question(
                DAEMON,
                NewQuestion {
                    session_id: w.sid.clone(),
                    artifact_id: None,
                    source: Source::Ask,
                    tool_use_id: None,
                    questions: serde_json::from_value(serde_json::json!([{
                        "question": "Which?", "header": "Pick",
                        "options": [{"label": "A"}, {"label": "B"}]}]))
                    .unwrap(),
                    released: false,
                },
            )
            .unwrap();
            w.st.with_tx(|tx| {
                tx.execute(
                    "UPDATE sessions SET elevate_json = '[]' WHERE id = ?1",
                    params![w.sid],
                )?;
                Ok(())
            })
            .unwrap();
            w
        }

        impl World {
            fn ended_items(&self) -> Vec<crate::store::inbox::ItemRow> {
                self.st
                    .inbox_list(&InboxQuery {
                        recipient: Some(self.sam.clone()),
                        kinds: vec![Kind::Ended],
                        ..Default::default()
                    })
                    .unwrap()
                    .0
            }

            fn kinds_since(&self, seq: i64) -> Vec<(String, serde_json::Value)> {
                self.st
                    .events_after(seq, 10_000)
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.kind, serde_json::from_str(&r.body).unwrap()))
                    .collect()
            }

            fn last_seq(&self) -> i64 {
                self.st
                    .events_after(0, 100_000)
                    .unwrap()
                    .last()
                    .map_or(0, |r| r.seq)
            }

            fn owner_of(&self, tid: &str) -> Option<String> {
                self.st
                    .with_read(|c| {
                        Ok(c.query_row(
                            "SELECT target_session_id FROM threads WHERE id = ?1",
                            [tid],
                            |r| r.get(0),
                        )?)
                    })
                    .unwrap()
            }

            fn open_requests(&self) -> usize {
                self.st
                    .approval_requests(Some(&self.sam), true, 500)
                    .unwrap()
                    .0
                    .len()
            }
        }

        #[test]
        fn ending_a_session_cleans_up_and_leaves_one_note() {
            let w = world();
            assert_eq!(w.owner_of(&w.owned).as_deref(), Some(w.sid.as_str()));
            assert_eq!(w.open_requests(), 1);
            let (seq, generation) = (w.last_seq(), w.st.visibility_generation());
            let e = w.st.end_session_touched(DAEMON, &w.sid).unwrap();
            assert_eq!(e.released_threads, vec![w.owned.clone()]);
            assert_eq!(e.withdrawn_requests.len(), 1);
            assert_eq!(e.withdrawn_questions.len(), 1);
            assert!(
                e.touched
                    .threads
                    .contains(&(w.aid.to_string(), w.owned.clone()))
            );
            assert_eq!(w.owner_of(&w.owned), None, "the thread is released");
            assert_eq!(w.open_requests(), 0, "the ask request is withdrawn");
            let (narrow, elevate): (Option<String>, Option<String>) =
                w.st.with_read(|c| {
                    Ok(c.query_row(
                        "SELECT narrow_json, elevate_json FROM sessions WHERE id = ?1",
                        [&w.sid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?)
                })
                .unwrap();
            assert_eq!((narrow, elevate), (None, None), "its rights are cleared");
            assert!(
                w.st.visibility_generation() > generation,
                "clearing an elevation changes what agents may see"
            );
            // Every effect is recorded, in the end's transaction.
            let recs = w.kinds_since(seq);
            let kinds: Vec<&str> = recs.iter().map(|(k, _)| k.as_str()).collect();
            for k in [
                "session.end",
                "approval.withdraw",
                "thread.release",
                "agent.elevate",
                "question.withdraw",
            ] {
                assert_eq!(
                    kinds.iter().filter(|x| **x == k).count(),
                    1,
                    "{k} in {kinds:?}"
                );
            }
            let get = |k: &str| &recs.iter().find(|(x, _)| x == k).unwrap().1;
            assert_eq!(get("approval.withdraw")["reason"], "session_end");
            assert_eq!(get("thread.release")["reason"], "session_end");
            assert_eq!(get("thread.release")["thread_id"], w.owned.as_str());
            assert_eq!(get("thread.release")["session"], w.sid.as_str());
            assert_eq!(get("agent.elevate")["access"], serde_json::Value::Null);
            // One note, for Sam, saying what it cleaned up.
            let items = w.ended_items();
            assert_eq!(items.len(), 1);
            let d = items[0].detail.clone().unwrap();
            assert_eq!(items[0].session_id.as_deref(), Some(w.sid.as_str()));
            assert_eq!(d["reason"], "exit");
            assert_eq!(d["questions"], 1);
            assert_eq!(d["requests"], 1);
            assert_eq!(d["threads"][0]["thread_id"], w.owned.as_str());
            assert_eq!(
                d["text"],
                "claude ended · released 1 thread · withdrew 1 question and 1 approval request"
            );
            // Ending it again changes and notes nothing.
            let seq = w.last_seq();
            let again = w.st.end_session_touched(DAEMON, &w.sid).unwrap();
            assert!(again.released_threads.is_empty() && again.withdrawn_requests.is_empty());
            assert!(w.kinds_since(seq).is_empty());
            assert_eq!(w.ended_items().len(), 1);
        }

        #[test]
        fn an_idle_session_ends_without_a_note() {
            let w = bare();
            w.clock.advance(Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
            let r =
                w.st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                    .unwrap();
            assert_eq!(r.ended, vec![w.sid.clone()]);
            assert!(w.ended_items().is_empty(), "nothing to clean up, no note");
            // A resolved thread it owned is released, but tells nothing.
            let w = bare();
            let t = thread_by(&w.st, &w.aid, &w.sam);
            w.st.send_routed(
                DAEMON,
                &t,
                crate::SendTarget::Agent(&w.sid),
                Route::Send,
                false,
            )
            .unwrap();
            w.st.resolve_thread(DAEMON, &t, "owner").unwrap();
            let e = w.st.end_session_touched(DAEMON, &w.sid).unwrap();
            assert_eq!(e.released_threads, vec![t.clone()]);
            assert_eq!(w.owner_of(&t), None);
            assert!(w.ended_items().is_empty());
        }

        #[test]
        fn remote_sessions_are_never_pid_probed_and_idle_thirty_minutes() {
            let w = bare();
            let local =
                w.st.register_session(
                    DAEMON,
                    RegisterSession {
                        harness: "claude".into(),
                        cwd: "/w".into(),
                        pid: Some(4242),
                        ..Default::default()
                    },
                )
                .unwrap()
                .id;
            // A remote row that holds a process ID (the daemon drops one
            // from a grant's registration; this one is written directly).
            w.st.with_tx(|tx| {
                tx.execute(
                    "UPDATE sessions SET pid = 777 WHERE id = ?1",
                    params![w.sid],
                )?;
                Ok(())
            })
            .unwrap();
            let probed = std::sync::Mutex::new(Vec::new());
            let alive = |p: u32| {
                probed.lock().unwrap().push(p);
                true
            };
            let idle = Duration::from_secs(Store::SESSION_IDLE_SECS);
            w.clock.advance(29 * 60);
            let r = w.st.reap_sessions(idle, &alive).unwrap();
            assert!(r.ended.is_empty(), "29 minutes: still live, {:?}", r.ended);
            assert_eq!(
                *probed.lock().unwrap(),
                vec![4242],
                "only the local process is probed"
            );
            w.clock.advance(2 * 60);
            // Its process "alive" changes nothing: it is never probed.
            let dead = |p: u32| {
                probed.lock().unwrap().push(p);
                p == 777
            };
            probed.lock().unwrap().clear();
            let r = w.st.reap_sessions(idle, &dead).unwrap();
            assert!(r.ended.contains(&w.sid), "31 minutes: ended");
            assert!(r.ended.contains(&local));
            assert_eq!(
                *probed.lock().unwrap(),
                vec![4242],
                "a grant's session is never probed"
            );
            let reason: String =
                w.st.with_read(|c| {
                    Ok(c.query_row(
                        "SELECT ended_reason FROM sessions WHERE id = ?1",
                        [&w.sid],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(reason, "idle");
        }

        #[test]
        fn revoking_a_grant_ends_its_sessions_with_their_notes() {
            let w = world();
            let quiet =
                w.st.register_session(DAEMON, reg_in(&w.grant, "hs-2"))
                    .unwrap()
                    .id;
            let seq = w.last_seq();
            let r = w.st.revoke_grant(DAEMON, &w.grant, EndedBy::Owner).unwrap();
            assert_eq!(r.sessions_ended.len(), 2);
            assert!(r.sessions_ended.contains(&quiet));
            assert_eq!(w.owner_of(&w.owned), None);
            let recs = w.kinds_since(seq);
            let release = recs.iter().find(|(k, _)| k == "thread.release").unwrap();
            assert_eq!(release.1["reason"], "grant_revoked");
            let items = w.ended_items();
            assert_eq!(items.len(), 1, "the quiet session makes no note");
            assert_eq!(items[0].session_id.as_deref(), Some(w.sid.as_str()));
            assert_eq!(items[0].detail.as_ref().unwrap()["reason"], "revoked");
        }

        #[test]
        fn reregistering_after_an_idle_end_restores_watches_and_says_what_ended() {
            let w = world();
            let other = artifact(&w.st, None);
            w.st.watch(DAEMON, &w.sid, &other, false).unwrap();
            w.clock.advance(Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
            let r =
                w.st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                    .unwrap();
            assert_eq!(r.ended, vec![w.sid.clone()]);
            // Another grant with the same harness session ID gets nothing.
            let (mia_grant, _, _) = approved(&w.st, "Ola", [9; 32]);
            let (_, none) =
                w.st.register_session_restoring(DAEMON, reg_in(&mia_grant, "hs-1"))
                    .unwrap();
            assert_eq!(none, None);
            // The same grant and ID within a day gets its watches back, armed
            // as they were, and a note naming the released thread.
            w.clock.advance(3600);
            let (s, restored) =
                w.st.register_session_restoring(DAEMON, reg_in(&w.grant, "hs-1"))
                    .unwrap();
            assert_ne!(s.id, w.sid);
            let restored = restored.expect("restored");
            assert_eq!(restored.from, w.sid);
            assert_eq!(restored.threads, vec![w.owned.clone()]);
            let mut back: Vec<(String, bool)> =
                w.st.list_watches(&s.id)
                    .unwrap()
                    .into_iter()
                    .map(|x| (x.artifact_id, x.replies_armed))
                    .collect();
            back.sort();
            let mut want = vec![(w.aid.to_string(), true), (other.to_string(), false)];
            want.sort();
            assert_eq!(back, want);
            assert!(
                restored
                    .note
                    .starts_with("[clax] Your session ended after 30 minutes without contact (")
            );
            assert!(
                restored
                    .note
                    .contains(&format!("1 thread was released ({})", w.owned)),
                "{}",
                restored.note
            );
            assert!(restored.note.ends_with("your watches were restored."));
        }

        #[test]
        fn only_a_recent_idle_end_restores_anything() {
            // exit, revoked and person ends restore nothing.
            for reason in [EndReason::Exit, EndReason::Revoked, EndReason::Person] {
                let w = bare();
                w.st.with_tx(|tx| {
                    w.st.end_in(tx, DAEMON, &w.sid, &w.st.now(), reason, None)?;
                    Ok(())
                })
                .unwrap();
                let (_, r) =
                    w.st.register_session_restoring(DAEMON, reg_in(&w.grant, "hs-1"))
                        .unwrap();
                assert_eq!(r, None, "{reason:?}");
            }
            // An idle end more than a day old restores nothing.
            let w = bare();
            w.clock.advance(Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
            w.st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                .unwrap();
            w.clock.advance(Store::RESTORE_WINDOW_SECS as i64 + 1);
            let (s, r) =
                w.st.register_session_restoring(DAEMON, reg_in(&w.grant, "hs-1"))
                    .unwrap();
            assert_eq!(r, None);
            assert!(w.st.list_watches(&s.id).unwrap().is_empty());
            // Within the day, a hook's join that makes the row restores too.
            let w = bare();
            w.clock.advance(Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
            w.st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                .unwrap();
            let (s, r) =
                w.st.join_session_restoring(
                    DAEMON,
                    JoinSession {
                        harness: "claude".into(),
                        parent_pid: 1,
                        harness_session_id: "hs-1".into(),
                        cwd: None,
                        transcript_path: None,
                        ancestor_pids: vec![],
                    },
                    Some(&w.grant),
                )
                .unwrap();
            let r = r.expect("restored");
            assert_eq!(r.artifacts, vec![w.aid.to_string()]);
            assert!(r.note.contains("no threads were released"), "{}", r.note);
            assert_eq!(w.st.list_watches(&s.id).unwrap().len(), 1);
        }

        #[test]
        fn daemon_downtime_does_not_count_against_remote_sessions() {
            let (d, clock, st) = store();
            let (gid, _, _) = approved(&st, "Sam", KEY_A);
            let sid = st
                .register_session(DAEMON, reg_in(&gid, "hs-1"))
                .unwrap()
                .id;
            drop(st);
            // The daemon is down for an hour, then starts again.
            clock.advance(3600);
            let st = Store::open_with_clock(&crate::Home::at(d.path().join("ax")), clock.clone())
                .unwrap();
            let idle = Duration::from_secs(Store::SESSION_IDLE_SECS);
            assert!(st.reap_sessions(idle, &|_| false).unwrap().ended.is_empty());
            clock.advance(29 * 60);
            assert!(
                st.reap_sessions(idle, &|_| false).unwrap().ended.is_empty(),
                "29 minutes of uptime"
            );
            // Its machine sleeps an hour; on wake the daemon marks the resume.
            clock.advance(3600);
            st.mark_resumed();
            assert!(st.reap_sessions(idle, &|_| false).unwrap().ended.is_empty());
            clock.advance(31 * 60);
            assert_eq!(
                st.reap_sessions(idle, &|_| false).unwrap().ended,
                vec![sid],
                "a whole window of uptime unseen"
            );
        }

        #[test]
        fn a_restore_brings_back_only_watches_the_rights_allow_and_pings_them() {
            let w = bare();
            let other = artifact(&w.st, None);
            w.st.watch(DAEMON, &w.sid, &other, true).unwrap();
            w.clock.advance(Store::REMOTE_SESSION_IDLE_SECS as i64 + 1);
            w.st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                .unwrap();
            let pinged = Arc::new(std::sync::Mutex::new(Vec::new()));
            let p = pinged.clone();
            w.st.set_agents_listener(move |a| p.lock().unwrap().push(a));
            // Sam narrows his machine to reading the first artifact only.
            let narrow = crate::rights::Rights::parse(&serde_json::json!([
                {"type": "clax-artifacts", "actions": ["read"]},
                {"type": "clax-comments", "actions": ["read", "watch"], "locations": [w.aid.as_str()]}
            ]))
            .unwrap();
            w.st.narrow_grant(DAEMON, &w.grant, Some(narrow)).unwrap();
            pinged.lock().unwrap().clear();
            let (s, r) =
                w.st.register_session_restoring(DAEMON, reg_in(&w.grant, "hs-1"))
                    .unwrap();
            assert_eq!(r.unwrap().artifacts, vec![w.aid.to_string()]);
            let back: Vec<String> =
                w.st.list_watches(&s.id)
                    .unwrap()
                    .into_iter()
                    .map(|x| x.artifact_id)
                    .collect();
            assert_eq!(
                back,
                vec![w.aid.to_string()],
                "not the one it may no longer watch"
            );
            assert!(
                pinged
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|a| a.contains(&w.aid.to_string())),
                "the restored watch's Send menu hears of it: {:?}",
                pinged.lock().unwrap()
            );
        }

        #[test]
        fn a_codex_session_restores_when_its_hook_names_it() {
            // Codex's shim registers with no harness session ID; its hook
            // joins by parent PID with the ID.
            let (_d, clock, st) = store();
            let shim = |pid: u32| RegisterSession {
                harness: "codex".into(),
                cwd: "/w".into(),
                parent_pid: Some(pid),
                ..Default::default()
            };
            let hook = |pid: u32| JoinSession {
                harness: "codex".into(),
                parent_pid: pid,
                harness_session_id: "cx-1".into(),
                cwd: None,
                transcript_path: None,
                ancestor_pids: vec![],
            };
            let first = st.register_session(DAEMON, shim(10)).unwrap().id;
            st.join_session(DAEMON, hook(10), None).unwrap();
            let aid = artifact(&st, None);
            st.watch(DAEMON, &first, &aid, true).unwrap();
            clock.advance(Store::SESSION_IDLE_SECS as i64 + 1);
            st.reap_sessions(Duration::from_secs(Store::SESSION_IDLE_SECS), &|_| false)
                .unwrap();
            // Resumed: a new shim process, then its hook.
            let (again, none) = st.register_session_restoring(DAEMON, shim(11)).unwrap();
            assert_eq!(none, None, "no harness session ID yet");
            let (joined, r) = st.join_session_restoring(DAEMON, hook(11), None).unwrap();
            assert_eq!(joined.id, again.id);
            let r = r.expect("restored at the join");
            assert_eq!(r.artifacts, vec![aid.to_string()]);
            assert!(
                r.note
                    .starts_with("[clax] Your session ended after 5 minutes without contact"),
                "a local session names the local window: {}",
                r.note
            );
        }

        #[test]
        fn a_named_send_to_another_agent_displaces_the_old_owner() {
            let w = world();
            let seq = w.last_seq();
            // Sam's second agent, then a send naming it.
            let second =
                w.st.register_session(DAEMON, reg_in(&w.grant, "hs-2"))
                    .unwrap()
                    .id;
            w.st.ensure_watch(DAEMON, &second, &w.aid).unwrap();
            // The first agent asks about the owned thread too.
            let mia_comment = |tid: &str| {
                let (g, _, _) = approved(&w.st, "Ola", [7; 32]);
                let ola = w.st.viewer_of_grant(&g).unwrap().public_id;
                w.st.add_comment(
                    DAEMON,
                    tid,
                    crate::store::threads::NewComment {
                        author_public_id: Some(ola),
                        author_kind: crate::store::threads::AUTHOR_VIEWER,
                        author_name: "Ola".into(),
                        via_session_id: None,
                        body: "more".into(),
                        via_page: false,
                    },
                )
                .unwrap();
            };
            mia_comment(&w.owned);
            w.st.ask_approval(
                DAEMON,
                &AskApproval {
                    session_id: w.sid.clone(),
                    artifact_id: None,
                    thread_id: Some(w.owned.clone()),
                    author: None,
                },
            )
            .unwrap();
            let undelivered = |sid: &str| -> i64 {
                w.st.with_read(|c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM feedback WHERE thread_id = ?1 AND target_session_id = ?2
                           AND delivered_at IS NULL",
                        [&w.owned, sid],
                        |r| r.get(0),
                    )?)
                })
                .unwrap()
            };
            assert!(undelivered(&w.sid) > 0);
            let pings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let p = pings.clone();
            w.st.set_agents_listener(move |_| {
                p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            let (_, touched, _) =
                w.st.send_routed(
                    DAEMON,
                    &w.owned,
                    crate::SendTarget::Agent(&second),
                    Route::Send,
                    false,
                )
                .unwrap();
            assert_eq!(w.owner_of(&w.owned).as_deref(), Some(second.as_str()));
            assert_eq!(undelivered(&w.sid), 0, "the old owner's rows are gone");
            assert_eq!(
                pings.load(std::sync::atomic::Ordering::Relaxed),
                1,
                "the owner changed: the roster's threads_owned did"
            );
            assert!(touched.targets.contains(&w.sid));
            let recs = w.kinds_since(seq);
            assert!(
                recs.iter()
                    .any(|(k, b)| k == "approval.withdraw" && b["reason"] == "thread_taken"),
                "{recs:?}"
            );
        }
    }
}
