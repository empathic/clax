//! Harness sessions: registration, joining by parent PID, liveness, and reaping.

use super::Store;
use crate::audit::{Actor, AgentActor, AuditCtx, AuditKind, AuditRecord, SystemReason};
use crate::model::Session;
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
}

const SELECT: &str = "SELECT id, harness, harness_session_id, cwd, pid, parent_pid, started_at,
    last_seen_at, ended_at, agent_handle, grant_id, person, name, name_source FROM sessions";

fn row_to_session(r: &Row<'_>) -> rusqlite::Result<Session> {
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
/// §5.2), read on `c`; `None` when no session has that ID.
pub(super) fn agent_of(c: &Connection, id: &str) -> Result<Option<AgentActor>> {
    Ok(c.prepare_cached(
        "SELECT harness, harness_session_id, agent_handle, transcript_path
         FROM sessions WHERE id = ?1",
    )?
    .query_row(params![id], |r| {
        Ok(AgentActor {
            session_id: Some(id.to_string()),
            harness: Some(r.get(0)?),
            harness_session_id: r.get(1)?,
            agent_handle: Some(r.get(2)?),
            transcript_path: r.get(3)?,
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
        let rec = session_record(kind, &self.now(), id, harness, f);
        self.record_audit(tx, &ctx, rec)?;
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

impl Store {
    /// A session unseen for this long, with no live process, is ended by the reaper;
    /// registration without a harness session ID only adopts rows seen within it.
    pub const SESSION_IDLE_SECS: u64 = 300;

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
    pub fn register_session(&self, ctx: &AuditCtx, mut r: RegisterSession) -> Result<Session> {
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
            if let Some(id) = existing {
                let before = facts(tx, &id)?;
                tx.execute(
                    "UPDATE sessions SET pid = COALESCE(?2, pid), parent_pid = COALESCE(?3, parent_pid),
                        harness_session_id = COALESCE(harness_session_id, ?4), last_seen_at = ?5,
                        cwd = CASE WHEN cwd = '' THEN ?6 ELSE cwd END,
                        transcript_path = COALESCE(?7, transcript_path)
                     WHERE id = ?1",
                    params![id, r.pid, r.parent_pid, r.harness_session_id, now, r.cwd, r.transcript_path],
                )?;
                if facts(tx, &id)? != before {
                    self.record_session(tx, ctx, AuditKind::SessionJoin, &id, &r.harness)?;
                }
                return fetch(tx, &id);
            }
            let id = new_ulid();
            tx.execute(
                "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                    started_at, last_seen_at, agent_handle, transcript_path,
                    grant_id, person)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11)",
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
                    person
                ],
            )?;
            self.record_session(tx, ctx, AuditKind::SessionStart, &id, &r.harness)?;
            fetch(tx, &id)
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
                    if facts(tx, &id)? != before {
                        self.record_session(tx, ctx, AuditKind::SessionJoin, &id, harness)?;
                    }
                    id
                }
                None => {
                    let id = new_ulid();
                    tx.execute(
                        "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, parent_pid,
                            started_at, last_seen_at, agent_handle, transcript_path, grant_id, person)
                         VALUES (?1, ?2, ?3, ?6, NULL, ?4, ?5, ?5, ?7, ?8, ?9, ?10)",
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
                            person
                        ],
                    )?;
                    self.record_session(tx, ctx, AuditKind::SessionStart, &id, harness)?;
                    id
                }
            };
            fetch(tx, &id)
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
        self.with_tx(|tx| {
            let now = self.now();
            let (touched, withdrawn_questions) = self
                .end_in(tx, ctx, id, &now, EndReason::Exit, None)?
                .unwrap_or_default();
            Ok(EndedSession {
                session: fetch(tx, id).map_err(not_found)?,
                touched,
                withdrawn_questions,
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
    /// path. Ended sessions count: the agent still acted. `None` when no
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
    /// credential's.
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

    /// Ends live sessions not seen for `idle` whose `pid` is unknown or no
    /// longer alive, releasing their watches and undelivered feedback as
    /// [`Store::end_session_touched`] does, and withdrawing their open
    /// questions. Returns the sessions ended, the feedback released, and the
    /// questions withdrawn. Each ends `idle`, its direct watches saved in
    /// `ended_watches_json` first, and records `session.end` (reason `ttl`)
    /// as `system:ttl`, naming the session's agent in `for_actor`, then
    /// `question.withdraw` (reason `session_end`) for each of its questions
    /// withdrawn, likewise.
    pub fn reap_sessions(&self, idle: Duration, pid_alive: &dyn Fn(u32) -> bool) -> Result<Reaped> {
        let cutoff = super::stamp(self.now_utc() - idle);
        let stale = self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, pid FROM sessions WHERE ended_at IS NULL AND last_seen_at < ?1",
            )?;
            Ok(stmt
                .query_map(params![cutoff], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<u32>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        // Checked outside the transaction; the update below re-checks that
        // each session is still live and idle.
        let dead: Vec<String> = stale
            .into_iter()
            .filter(|(_, pid)| !pid.is_some_and(pid_alive))
            .map(|(id, _)| id)
            .collect();
        if dead.is_empty() {
            return Ok(Reaped::default());
        }
        self.with_tx(|tx| {
            let now = self.now();
            let mut reaped = Reaped::default();
            let ttl = AuditCtx::system(SystemReason::Ttl);
            for id in dead {
                let Some((touched, withdrawn)) =
                    self.end_in(tx, &ttl, &id, &now, EndReason::Idle, Some(&cutoff))?
                else {
                    continue;
                };
                reaped.touched.merge(touched);
                reaped.withdrawn_questions.extend(withdrawn);
                reaped.ended.push(id);
            }
            Ok(reaped)
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

impl Store {
    /// Ends live session `id` for `reason` in `tx` under `ctx`: records
    /// `ended_at` and the reason, releases its watches and undelivered
    /// feedback, and withdraws its open questions. An `idle` end first
    /// saves the session's direct watches to `ended_watches_json`. With
    /// `unseen_before`, ends it only when it was last seen before then.
    /// `None` when it was not ended here. An end records `session.end`
    /// ([`EndReason::audit_reason`]), then `question.withdraw` (reason
    /// `session_end`) for each question withdrawn, under `ctx`; a system
    /// actor names the session's agent in `for_actor`.
    pub(super) fn end_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        now: &str,
        reason: EndReason,
        unseen_before: Option<&str>,
    ) -> Result<Option<(crate::feedback::Touched, Vec<String>)>> {
        let n = tx.execute(
            "UPDATE sessions SET ended_at = ?2, ended_reason = ?3
             WHERE id = ?1 AND ended_at IS NULL AND (?4 IS NULL OR last_seen_at < ?4)",
            params![id, now, reason.as_str(), unseen_before],
        )?;
        if n == 0 {
            return Ok(None);
        }
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
        Ok(Some((
            super::feedback::release_session(tx, id, now)?,
            self.withdraw_session_questions(tx, ctx, id, now)?,
        )))
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
        let s = st
            .register_session(DAEMON, reg(None, Some(10), Some(5)))
            .unwrap();
        let ev = recorded(&st, 0);
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
            st.revoke_grant(&gid, crate::store::people::EndedBy::Owner)
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
}
