//! The audit journal's backfill (spec 2026-10-06-toolpath-audit-design L12,
//! §13): the history stored before the audit existed, recorded as events of
//! the same kinds, built by the same builders, as live recording, so the
//! journal and every export cover the install from its first day.
//!
//! **Once per home.** The install row `backfill` marks it done. It is
//! written when the last event is recorded (or at once, when there is no
//! history), and never inferred from the schema version, so a home reaching
//! the audit schema under any numbering gets exactly one backfill. A home
//! that already has live events backfills only what happened before the
//! first of them.
//!
//! **Three phases**, all inside [`Store::open`](super::Store::open), on the
//! write connection, before the store exists, so no change is recorded until
//! the history before it is, and `seq` follows history:
//!
//! 1. *Planning.* One `IMMEDIATE` transaction stages one row per event to
//!    make in `audit_backfill`, numbered in history order: by `at`, then by
//!    kind (a session starts before its publish, a thread opens before its
//!    first comment, a delete comes last), then by the source row's natural
//!    ID. SQLite sorts; memory holds nothing of it.
//! 2. *Hashing files* and 3. *recording*: short `IMMEDIATE` transactions of
//!    at most [`Limits::rows`] rows or [`Limits::bytes`] of hashed files
//!    each record their rows' events and delete those rows, so an
//!    interrupted backfill resumes where it stopped and never records an
//!    event twice. The last one drops the staging table and writes the
//!    marker. Files are hashed in chunks. Two processes opening one database
//!    share the work: each batch takes the write lock and reads what is
//!    still staged.
//!
//! Progress (phase, rows done of rows staged, bytes hashed) goes to a
//! callback, which the daemon publishes for whoever started it, and to the
//! log at least every [`LOG_EVERY`] and after each batch that hashed
//! [`LOG_BYTES`] or more.
//!
//! **What a backfilled event is.** Its row has `backfilled = 1`; its actor
//! is Clax itself (`system:backfill`), through the `daemon` channel, with no
//! git context and no tool call; who made the change, where the history
//! keeps it, is named in `body.for_actor` (an agent by its session, a viewer
//! or the owner by public ID). Fields the history holds only as they are
//! now, or that are derived, are listed in `body.inferred`; fields it
//! cannot give at all are `null` (`version.publish.by_page`, a `live.join`'s
//! `with` and rules). Not reconstructed, because the history keeps no trace
//! of them: earlier resolve and reopen cycles, rows since hard-deleted,
//! releases, rules, splits (apart from the answers they left), working
//! records and document writes.
//!
//! **A row that cannot be converted** (a stored value of the wrong type, a
//! JSON column that does not parse where it matters) records
//! `backfill.skip {table, row_id, reason}` in its place, and the backfill
//! goes on: a first start never stops at one bad row. Errors of the
//! database or the disk fail the open, and the next start resumes.
//!
//! Versions are hashed from their stored files (§5.3), and each file's hash
//! and the version's `content_sha256` are written back to the `versions`
//! row. A file that cannot be read is recorded with `sha256: null` and
//! `missing: true`, one whose length differs from its recorded size with
//! `sha256: null` and `size_mismatch: true`; either way the version keeps
//! no content hash. Asset blobs are hashed the same way.

use super::Store;
use super::artifacts::{create_record, delete_record, version_file_path, version_record};
use super::audit::insert_backfilled;
use super::live::{LivePage, page_record};
use super::sessions::{agent_of, facts, session_end_record, session_record};
use super::threads::{addressed_record, comment_record, open_record, resolve_record};
use super::watches::{Cause, artifact_watch_record_at, scope_watch_record_at, with_cause};
use crate::audit::{Actor, AgentActor, AuditKind, AuditRecord, content_manifest_sha256};
use crate::model::{Asset, Comment, FileMeta, Version};
use crate::{ArtifactId, CoreError, Home, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

/// The staging table's name.
const STAGING: &str = "audit_backfill";

/// The `install` row that marks the backfill done (its value: when).
const MARKER: &str = "backfill";

/// How many staged rows one backfill transaction takes at most, and how
/// many bytes of files it hashes before it commits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub rows: u32,
    pub bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            rows: 500,
            bytes: 64 << 20,
        }
    }
}

/// Progress is logged at least this often while the backfill runs...
const LOG_EVERY: Duration = Duration::from_secs(2);
/// ... and after each batch that hashed this many bytes.
const LOG_BYTES: u64 = 16 << 20;
/// Progress is reported to the callback at most this often while hashing.
const REPORT_EVERY: Duration = Duration::from_millis(250);

/// What the backfill is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Listing the history to record.
    Planning,
    /// Hashing a batch's version and asset files.
    Hashing,
    /// Recording a batch's events.
    Recording,
    /// Finished: every staged row is recorded.
    Done,
}

impl Phase {
    /// How the log and the spawner name it.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Planning => "planning",
            Phase::Hashing => "hashing files",
            Phase::Recording => "recording",
            Phase::Done => "done",
        }
    }
}

/// Where a running backfill is: its phase, staged rows done of `total`, and
/// file bytes hashed so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    pub bytes: u64,
}

/// Reports progress to the callback (throttled while hashing) and the log.
struct Reporter<'a> {
    report: &'a mut dyn FnMut(&Progress),
    at: Progress,
    started: Instant,
    logged: Instant,
    reported: Instant,
    batch_bytes: u64,
}

impl Reporter<'_> {
    fn set(&mut self, phase: Phase) {
        self.at.phase = phase;
        (self.report)(&self.at);
        self.reported = Instant::now();
    }

    fn hashed(&mut self, n: u64) {
        self.at.bytes += n;
        self.batch_bytes += n;
        if self.reported.elapsed() >= REPORT_EVERY {
            self.set(Phase::Hashing);
        }
        if self.logged.elapsed() >= LOG_EVERY {
            self.log();
        }
    }

    fn batch_done(&mut self, rows: u64) {
        self.at.done += rows;
        self.set(Phase::Recording);
        if self.logged.elapsed() >= LOG_EVERY || self.batch_bytes >= LOG_BYTES {
            self.log();
        }
        self.batch_bytes = 0;
    }

    fn log(&mut self) {
        tracing::info!(
            phase = self.at.phase.label(),
            done = self.at.done,
            staged = self.at.total,
            bytes_hashed = self.at.bytes,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "audit backfill: {} ({} of {} rows, {} MiB hashed)",
            self.at.phase.label(),
            self.at.done,
            self.at.total,
            self.at.bytes >> 20
        );
        self.logged = Instant::now();
    }
}

/// Each event's sources, as `(at, rank, nid, src, k1, k2, k3)`: when it
/// happened, its rank among events at the same instant, the source row's
/// natural ID, and the keys [`build`] reads it back by.
const SOURCES: &[&str] = &[
    "SELECT started_at, 10, id, 'session.start', id, NULL, NULL FROM sessions",
    "SELECT created_at, 20, id, 'artifact.create', id, NULL, NULL FROM artifacts",
    "SELECT created_at, 25, id, 'asset.upload', id, NULL, NULL FROM assets",
    "SELECT created_at, 30, artifact_id, 'live.page', artifact_id, NULL, NULL FROM live_pages",
    "SELECT a.created_at, 30, m.artifact_id, 'live.page.merged', m.artifact_id, NULL, NULL
        FROM live_merged_pages m JOIN artifacts a ON a.id = m.artifact_id",
    "SELECT created_at, 40, artifact_id || '/' || printf('%010d', n), 'version', artifact_id, n, NULL
        FROM versions",
    "SELECT created_at, 50, id, 'thread.open', id, NULL, NULL FROM threads",
    "SELECT created_at, 60, id, 'comment.add', id, NULL, NULL FROM comments",
    "SELECT joined_at, 68, site || '/' || joined_at, 'live.join', site, joined_at, NULL
        FROM live_sites WHERE origin <> site GROUP BY site, joined_at",
    "SELECT created_at, 70, id, 'thread.move', id, NULL, NULL FROM thread_moves",
    "SELECT merged_at, 75, artifact_id, 'live.page_merge', artifact_id, NULL, NULL
        FROM live_merged_pages",
    "SELECT created_at, 80, id, 'send.batch', id, NULL, NULL FROM send_batches",
    // Rows of no batch, or of a batch deleted with its artifact: one send
    // per thread and instant.
    "SELECT created_at, 80, thread_id || '/' || created_at, 'send.group', thread_id, created_at, NULL
        FROM feedback WHERE batch_id IS NULL OR batch_id NOT IN (SELECT id FROM send_batches)
        GROUP BY thread_id, created_at",
    "SELECT delivered_at, 90, id, 'feedback.delivered', id, NULL, NULL
        FROM feedback WHERE delivered_at IS NOT NULL",
    "SELECT resolved_at, 100, id, 'thread.resolve', id, NULL, NULL
        FROM threads WHERE status = 'resolved' AND resolved_at IS NOT NULL",
    "SELECT created_at, 110, thread_id || '/' || artifact_id || '/' || printf('%010d', version_n),
        'thread.addressed', artifact_id, version_n, thread_id
        FROM version_threads WHERE source = 'resolve'",
    "SELECT created_at, 118, a || '/' || b, 'live.join_answer', a, b, NULL FROM live_site_answers",
    "SELECT created_at, 120, session_id || '/' || artifact_id, 'watch.artifact', session_id, artifact_id, NULL
        FROM watches",
    "SELECT created_at, 120, session_id || '/' || origin || path, 'watch.scope', session_id, origin, path
        FROM live_watches",
    "SELECT deleted_at, 130, id, 'artifact.delete', id, NULL, NULL
        FROM artifacts WHERE deleted_at IS NOT NULL",
    "SELECT ended_at, 140, id, 'session.end', id, NULL, NULL FROM sessions WHERE ended_at IS NOT NULL",
];

/// The sources of agent questions, when the database has them.
const QUESTION_SOURCES: &[&str] = &[
    "SELECT created_at, 65, id, 'question.ask', id, NULL, NULL FROM questions",
    "SELECT closed_at, 115, id, 'question.close', id, NULL, NULL
        FROM questions WHERE status <> 'open' AND closed_at IS NOT NULL",
];

/// The table each source reads, for `backfill.skip`.
fn table_of(src: &str) -> &'static str {
    match src {
        "session.start" | "session.end" => "sessions",
        "artifact.create" | "artifact.delete" => "artifacts",
        "asset.upload" => "assets",
        "live.page" => "live_pages",
        "live.page.merged" | "live.page_merge" => "live_merged_pages",
        "version" => "versions",
        "thread.open" | "thread.resolve" => "threads",
        "comment.add" => "comments",
        "live.join" => "live_sites",
        "thread.move" => "thread_moves",
        "send.batch" => "send_batches",
        "send.group" | "feedback.delivered" => "feedback",
        "thread.addressed" => "version_threads",
        "live.join_answer" => "live_site_answers",
        "watch.artifact" => "watches",
        "watch.scope" => "live_watches",
        "question.ask" | "question.close" => "questions",
        _ => "unknown",
    }
}

fn table_exists(c: &Connection, name: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        params![name],
        |r| r.get(0),
    )?)
}

fn marked(c: &Connection) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM install WHERE k = ?1)",
        params![MARKER],
        |r| r.get(0),
    )?)
}

fn mark(c: &Connection) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO install (k, v) VALUES (?1, ?2)",
        params![MARKER, Store::now()],
    )?;
    Ok(())
}

/// Stages the backfill in `tx`: one row per event to make, numbered
/// (`ord`) in history order, before the first live event when there is
/// one. With no history, marks the backfill done and leaves no table.
/// Returns how many rows it staged.
fn stage(tx: &Transaction<'_>) -> Result<u64> {
    let mut sources: Vec<&str> = SOURCES.to_vec();
    if table_exists(tx, "questions")? {
        sources.extend_from_slice(QUESTION_SOURCES);
    }
    let cutoff: Option<String> = tx.query_row(
        "SELECT MIN(at) FROM audit_events WHERE backfilled = 0",
        [],
        |r| r.get(0),
    )?;
    tx.execute_batch(&format!(
        "CREATE TABLE {STAGING} (
            ord INTEGER PRIMARY KEY,
            at TEXT NOT NULL,
            src TEXT NOT NULL,
            k1 TEXT, -- the keys are TEXT, so a version number reads back as text
            k2 TEXT,
            k3 TEXT
        )"
    ))?;
    let staged = tx.execute(
        &format!(
            "INSERT INTO {STAGING} (ord, at, src, k1, k2, k3)
             WITH h(at, rank, nid, src, k1, k2, k3) AS ({})
             SELECT row_number() OVER (ORDER BY at, rank, nid), at, src, k1, k2, k3
             FROM h WHERE at IS NOT NULL AND (?1 IS NULL OR at < ?1)",
            sources.join(" UNION ALL ")
        ),
        params![cutoff],
    )?;
    if staged == 0 {
        tx.execute_batch(&format!("DROP TABLE {STAGING}"))?;
        mark(tx)?;
    }
    Ok(staged as u64)
}

/// Records the history before the audit once per home (see the module
/// documentation), with the default [`Limits`], reporting progress to
/// `report`. Returns at once when it is done.
pub(super) fn run(
    conn: &mut Connection,
    home: &Home,
    report: &mut dyn FnMut(&Progress),
) -> Result<()> {
    run_with(conn, home, Limits::default(), None, report).map(drop)
}

/// [`run`] in transactions of at most `limits`, stopping after
/// `max_batches` batches when given. Whether the backfill is done.
pub(crate) fn run_with(
    conn: &mut Connection,
    home: &Home,
    limits: Limits,
    max_batches: Option<usize>,
    report: &mut dyn FnMut(&Progress),
) -> Result<bool> {
    if marked(conn)? {
        return Ok(true);
    }
    let now = Instant::now();
    let mut r = Reporter {
        report,
        at: Progress {
            phase: Phase::Planning,
            done: 0,
            total: 0,
            bytes: 0,
        },
        started: now,
        logged: now,
        reported: now,
        batch_bytes: 0,
    };
    r.set(Phase::Planning);
    {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if marked(&tx)? {
            tx.commit()?;
            return Ok(true);
        }
        let total = if table_exists(&tx, STAGING)? {
            tx.query_row(&format!("SELECT COUNT(*) FROM {STAGING}"), [], |r| r.get(0))?
        } else {
            stage(&tx)?
        };
        tx.commit()?;
        r.at.total = total;
    }
    if r.at.total == 0 {
        return Ok(true);
    }
    tracing::info!(
        staged = r.at.total,
        "audit backfill: first start with the audit log: recording the existing history ({} rows)",
        r.at.total
    );
    let (mut events, mut skipped, mut batches) = (0u64, 0u64, 0usize);
    loop {
        if max_batches.is_some_and(|m| batches >= m) {
            return Ok(false);
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Another process may have finished it while this one waited.
        if !table_exists(&tx, STAGING)? {
            tx.commit()?;
            break;
        }
        let b = batch(&tx, home, limits, &mut r)?;
        let left: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {STAGING})"),
            [],
            |r| r.get(0),
        )?;
        if !left {
            tx.execute_batch(&format!("DROP TABLE {STAGING}"))?;
            mark(&tx)?;
        }
        tx.commit()?;
        batches += 1;
        events += b.events;
        skipped += b.skipped;
        r.batch_done(b.rows);
        if !left {
            break;
        }
    }
    r.set(Phase::Done);
    tracing::info!(
        rows = r.at.done,
        events,
        skipped,
        bytes_hashed = r.at.bytes,
        ms = r.started.elapsed().as_millis() as u64,
        "audit backfill: done"
    );
    Ok(true)
}

/// One staged row. `at` is `None` when the source's time is not text (a
/// value of another type in a time column): the row is skipped.
struct Staged {
    ord: i64,
    at: Option<String>,
    src: String,
    k1: Option<String>,
    k2: Option<String>,
    k3: Option<String>,
}

/// What one batch did.
struct Batch {
    rows: u64,
    events: u64,
    skipped: u64,
}

/// The `backfill.skip` reason for `e`: a fixed phrase naming the fault,
/// and at most a column and a type, never a value from the row (the error's
/// own message can quote one).
fn skip_reason(e: &CoreError) -> String {
    match e {
        CoreError::Invalid {
            code: "bad_time", ..
        } => "its time is not text".into(),
        CoreError::Invalid { code, .. } => format!("invalid ({code})"),
        CoreError::Corrupt { column, .. } => format!("corrupt {column}"),
        CoreError::NotFound => "a row it names is missing".into(),
        CoreError::Db(rusqlite::Error::InvalidColumnType(_, name, ty)) => {
            format!("column {name} is {ty}")
        }
        CoreError::Db(rusqlite::Error::FromSqlConversionFailure(i, ty, _)) => {
            format!("column {i} of type {ty} does not convert")
        }
        CoreError::Db(rusqlite::Error::SqliteFailure(f, _)) => {
            format!("database error ({:?})", f.code)
        }
        CoreError::Db(_) => "database error".into(),
        _ => "conversion failed".into(),
    }
}

/// Whether `e` is the row's fault (it is skipped) rather than the
/// database's or the disk's (the open fails, and the next one resumes).
fn row_fault(e: &CoreError) -> bool {
    use rusqlite::ErrorCode as C;
    match e {
        CoreError::Io(_) => false,
        CoreError::Db(rusqlite::Error::SqliteFailure(f, _)) => !matches!(
            f.code,
            C::DiskFull
                | C::SystemIoFailure
                | C::DatabaseBusy
                | C::DatabaseLocked
                | C::ReadOnly
                | C::CannotOpen
                | C::OutOfMemory
                | C::DatabaseCorrupt
                | C::NotADatabase
                | C::FileLockingProtocolFailed
                | C::OperationInterrupted
        ),
        _ => true,
    }
}

/// Records the oldest staged rows, up to `limits`, and deletes them. A row
/// that cannot be converted records `backfill.skip` instead (its partial
/// writes rolled back).
fn batch(tx: &Transaction<'_>, home: &Home, limits: Limits, r: &mut Reporter) -> Result<Batch> {
    let staged: Vec<Staged> = {
        // Not cached: the last batch drops the table.
        let mut q = tx.prepare(&format!(
            "SELECT ord, at, src, k1, k2, k3 FROM {STAGING} ORDER BY ord LIMIT ?1"
        ))?;
        q.query_map(params![limits.rows], |r| {
            Ok(Staged {
                ord: r.get(0)?,
                at: match r.get::<_, rusqlite::types::Value>(1)? {
                    rusqlite::types::Value::Text(t) => Some(t),
                    _ => None,
                },
                src: r.get(2)?,
                k1: r.get(3)?,
                k2: r.get(4)?,
                k3: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut b = Batch {
        rows: 0,
        events: 0,
        skipped: 0,
    };
    let start = r.at.bytes;
    let mut last = None;
    for s in &staged {
        tx.execute_batch("SAVEPOINT backfill_row")?;
        let built = match &s.at {
            Some(at) => build(tx, home, at, s, r),
            None => Err(CoreError::invalid(
                "bad_time",
                format!("its time is not text ({})", s.src),
            )),
        };
        match built {
            Ok(recs) => {
                for rec in recs {
                    insert_backfilled(tx, rec)?;
                    b.events += 1;
                }
                tx.execute_batch("RELEASE backfill_row")?;
            }
            Err(e) if row_fault(&e) => {
                tx.execute_batch("ROLLBACK TO backfill_row; RELEASE backfill_row")?;
                let keys: Vec<&str> = [&s.k1, &s.k2, &s.k3]
                    .into_iter()
                    .filter_map(|k| k.as_deref())
                    .collect();
                tracing::warn!(
                    src = s.src,
                    row = keys.join("/"),
                    error = %e,
                    "audit backfill: a row could not be converted; skipped"
                );
                let at = s.at.clone().unwrap_or_else(Store::now);
                let rec = AuditRecord::new(AuditKind::BackfillSkip, at)
                    .with("table", table_of(&s.src))
                    .with("row_id", keys.join("/"))
                    .with("reason", skip_reason(&e));
                insert_backfilled(tx, rec)?;
                b.skipped += 1;
            }
            Err(e) => return Err(e),
        }
        b.rows += 1;
        last = Some(s.ord);
        if r.at.bytes - start >= limits.bytes {
            break;
        }
    }
    if let Some(last) = last {
        tx.execute(
            &format!("DELETE FROM {STAGING} WHERE ord <= ?1"),
            params![last],
        )?;
    }
    Ok(b)
}

fn key(k: &Option<String>) -> &str {
    k.as_deref().unwrap_or_default()
}

fn json_or_null(text: Option<String>) -> Value {
    text.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// `rec` naming `who` in `for_actor`, when known.
fn for_actor(rec: AuditRecord, who: Option<Actor>) -> AuditRecord {
    match who {
        Some(a) => rec.with(
            "for_actor",
            serde_json::to_value(a).expect("serialisable actor"),
        ),
        None => rec,
    }
}

/// `rec` listing `fields` in `inferred`: values the history holds only as
/// they are now, or that the backfill derived.
fn inferred(rec: AuditRecord, fields: &[&str]) -> AuditRecord {
    let mut list: Vec<String> = rec
        .body
        .get("inferred")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    list.extend(fields.iter().map(|f| f.to_string()));
    rec.with("inferred", list)
}

/// The agent of session `sid`, as live recording names it; just its
/// session ID when the session row is gone.
fn agent(c: &Connection, sid: Option<&str>) -> Result<Option<Actor>> {
    let Some(sid) = sid else { return Ok(None) };
    Ok(Some(Actor::Agent(agent_of(c, sid)?.unwrap_or_else(|| {
        AgentActor {
            session_id: Some(sid.to_string()),
            ..AgentActor::default()
        }
    }))))
}

/// The viewer with public ID `pid`: the owner when it is the owner's row.
fn person(c: &Connection, pid: Option<&str>) -> Result<Option<Actor>> {
    let Some(pid) = pid else { return Ok(None) };
    let row: Option<(Option<String>, bool)> = c
        .prepare_cached("SELECT display_name, owner FROM viewers WHERE public_id = ?1")?
        .query_row(params![pid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(Some(match row {
        Some((_, true)) => Actor::Owner {
            public_id: pid.to_string(),
        },
        Some((display_name, false)) => Actor::Viewer {
            public_id: pid.to_string(),
            display_name,
        },
        None => Actor::Viewer {
            public_id: pid.to_string(),
            display_name: None,
        },
    }))
}

/// A comment's author: its agent session, or the viewer by public ID.
fn author(
    c: &Connection,
    kind: &str,
    pid: Option<&str>,
    via_session: Option<&str>,
) -> Result<Option<Actor>> {
    match kind {
        "agent" => agent(c, via_session),
        _ => person(c, pid),
    }
}

/// The owner, by the owner's viewer row.
fn owner(c: &Connection) -> Result<Option<Actor>> {
    let pid: Option<String> = c
        .prepare_cached("SELECT public_id FROM viewers WHERE owner = 1")?
        .query_row([], |r| r.get(0))
        .optional()?;
    person(c, pid.as_deref())
}

fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// Whether `a` and `b` are the same change's times: within a second.
fn same_change(a: &str, b: &str) -> bool {
    match (parse_time(a), parse_time(b)) {
        (Some(x), Some(y)) => (x - y).num_milliseconds().abs() <= 1000,
        _ => a == b,
    }
}

/// Whether a version link rides in its version's event (spec §6, "no
/// duplicate facts"): an explicit or working link always does, as it is
/// made with the version; a resolve link does only on a live page (`live`),
/// where the version made it: the page's pending address, linked by its
/// next snapshot. Elsewhere a resolve link is the resolve's, as live
/// recording has it.
fn rides_version(source: &str, link_at: &str, version_at: &str, live: bool) -> bool {
    source != "resolve" || (live && same_change(link_at, version_at))
}

/// Whether artifact `aid` is a live page.
fn is_live(c: &Connection, aid: &str) -> Result<bool> {
    Ok(
        c.prepare_cached("SELECT kind = 'live' FROM artifacts WHERE id = ?1")?
            .query_row(params![aid], |r| r.get(0))
            .optional()?
            .unwrap_or(false),
    )
}

/// Whether a resolve link rides in its thread's `thread.resolve`: the
/// thread is resolved now, and the link was made with that resolve.
fn rides_resolve(link_at: &str, resolved_at: Option<&str>) -> bool {
    resolved_at.is_some_and(|r| same_change(link_at, r))
}

/// The events of staged row `s`. A row whose source is gone or yields no
/// event gives none.
fn build(
    tx: &Transaction<'_>,
    home: &Home,
    at: &str,
    s: &Staged,
    r: &mut Reporter,
) -> Result<Vec<AuditRecord>> {
    let (k1, k2, k3) = (key(&s.k1), key(&s.k2), key(&s.k3));
    let rec = match s.src.as_str() {
        "session.start" => session_start(tx, at, k1)?,
        "session.end" => session_end(tx, at, k1)?,
        "artifact.create" => artifact_create(tx, at, k1)?,
        "asset.upload" => asset_upload(tx, home, k1, r)?,
        "live.page" => live_page(tx, at, k1, false)?,
        "live.page.merged" => live_page(tx, at, k1, true)?,
        "version" => version(tx, home, at, k1, k2, r)?,
        "thread.open" => thread_open(tx, at, k1)?,
        "comment.add" => comment_add(tx, at, k1)?,
        "live.join" => live_join(tx, at, k1, k2)?,
        "thread.move" => thread_move(tx, at, k1)?,
        "live.page_merge" => page_merge(tx, at, k1)?,
        "send.batch" => send_batch(tx, at, k1)?,
        "send.group" => send_group(tx, at, k1, k2)?,
        "feedback.delivered" => delivered(tx, at, k1)?,
        "thread.resolve" => thread_resolve(tx, at, k1)?,
        "thread.addressed" => thread_addressed(tx, at, k1, k2, k3)?,
        "live.join_answer" => join_answer(tx, at, k1, k2)?,
        "watch.artifact" => watch_artifact(tx, at, k1, k2)?,
        "watch.scope" => watch_scope(tx, at, k1, k2, k3)?,
        "artifact.delete" => artifact_delete(tx, at, k1)?,
        "question.ask" => question_ask(tx, at, k1)?,
        "question.close" => question_close(tx, at, k1)?,
        other => {
            return Err(CoreError::invalid(
                "unknown_source",
                format!("unknown staged source {other}"),
            ));
        }
    };
    Ok(rec.into_iter().collect())
}

fn session_start(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let harness: Option<String> = c
        .prepare_cached("SELECT harness FROM sessions WHERE id = ?1")?
        .query_row(params![id], |r| r.get(0))
        .optional()?;
    let Some(harness) = harness else {
        return Ok(None);
    };
    let rec = session_record(AuditKind::SessionStart, at, id, &harness, facts(c, id)?);
    let rec = inferred(
        rec,
        &["harness_session_id", "cwd", "transcript_path", "pid"],
    );
    Ok(Some(for_actor(rec, agent(c, Some(id))?)))
}

fn session_end(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let seen: Option<String> = c
        .prepare_cached("SELECT last_seen_at FROM sessions WHERE id = ?1")?
        .query_row(params![id], |r| r.get(0))
        .optional()?;
    let Some(seen) = seen else { return Ok(None) };
    // Only the reaper ends a session this long after it was last seen.
    let idle = match (parse_time(at), parse_time(&seen)) {
        (Some(end), Some(seen)) => (end - seen).num_seconds() >= Store::SESSION_IDLE_SECS as i64,
        _ => false,
    };
    let rec = session_end_record(at, id, if idle { "ttl" } else { "explicit" });
    let rec = inferred(rec, &["reason"]);
    Ok(Some(for_actor(rec, agent(c, Some(id))?)))
}

fn artifact_of(c: &Connection, id: &str) -> Result<Option<crate::model::Artifact>> {
    let row = c
        .prepare_cached(&format!("{} WHERE id = ?1", super::artifacts::SELECT))?
        .query_row(params![id], super::artifacts::row_to_artifact)
        .optional()?;
    row.transpose()
}

fn artifact_create(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let Some(a) = artifact_of(c, id)? else {
        return Ok(None);
    };
    let rec = create_record(&a, at);
    Ok(Some(inferred(
        rec,
        &["title", "icon", "capabilities", "contract_version"],
    )))
}

/// Whether `path` is a relative path of plain components only.
fn plain(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

/// What hashing a stored file found.
enum Hashed {
    /// Its lowercase hex SHA-256.
    Sha(String),
    /// It could not be read.
    Missing,
    /// It holds this many bytes, not the size recorded.
    SizeMismatch,
}

/// Hashes the file at `p`, recorded as `size` bytes, in chunks, reporting
/// the bytes read.
fn hash_file(p: &Path, size: u64, r: &mut Reporter) -> Hashed {
    use sha2::{Digest, Sha256};
    let mut f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %p.display(), error = %e, "audit backfill: unreadable file");
            }
            return Hashed::Missing;
        }
    };
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 << 10];
    let mut read = 0u64;
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                read += n as u64;
                r.hashed(n as u64);
                h.update(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::warn!(path = %p.display(), error = %e, "audit backfill: unreadable file");
                return Hashed::Missing;
            }
        }
    }
    if read != size {
        tracing::warn!(
            path = %p.display(),
            recorded = size,
            read,
            "audit backfill: a file's size differs from the size recorded"
        );
        return Hashed::SizeMismatch;
    }
    Hashed::Sha(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The live page an artifact is, or was until it was merged away.
fn page_of(c: &Connection, aid: &str) -> Result<Option<LivePage>> {
    let row: Option<(String, String)> = c
        .prepare_cached(
            "SELECT origin, path FROM live_pages WHERE artifact_id = ?1
             UNION ALL SELECT origin, path FROM live_merged_pages WHERE artifact_id = ?1
             LIMIT 1",
        )?
        .query_row(params![aid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.map(|(origin, path)| LivePage {
        artifact_id: aid.to_string(),
        origin,
        path,
    }))
}

fn live_page(c: &Connection, at: &str, aid: &str, merged: bool) -> Result<Option<AuditRecord>> {
    let table = if merged {
        "live_merged_pages"
    } else {
        "live_pages"
    };
    let row: Option<(String, String)> = c
        .prepare_cached(&format!(
            "SELECT origin, path FROM {table} WHERE artifact_id = ?1"
        ))?
        .query_row(params![aid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((origin, path)) = row else {
        return Ok(None);
    };
    let rec = page_record(at, aid, &origin, &path);
    // A page created with its artifact: at its creation time.
    Ok(Some(if merged { inferred(rec, &["at"]) } else { rec }))
}

fn page_merge(c: &Connection, at: &str, aid: &str) -> Result<Option<AuditRecord>> {
    let row: Option<(String, String, Option<String>)> = c
        .prepare_cached(
            "SELECT origin, path, merged_into FROM live_merged_pages WHERE artifact_id = ?1",
        )?
        .query_row(params![aid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((origin, path, into)) = row else {
        return Ok(None);
    };
    let rec = super::joined::page_merge_record(at, aid, &origin, &path, into.as_deref());
    Ok(Some(inferred(rec, &["merged_into"])))
}

fn live_join(c: &Connection, at: &str, site: &str, joined_at: &str) -> Result<Option<AuditRecord>> {
    let joined: Vec<String> = c
        .prepare_cached(
            "SELECT origin FROM live_sites WHERE site = ?1 AND joined_at = ?2 AND origin <> site
             ORDER BY origin",
        )?
        .query_map(params![site, joined_at], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let Some(first) = joined.first() else {
        return Ok(None);
    };
    let rec = super::joined::join_record(at, first, None, site, &joined, None);
    Ok(Some(inferred(rec, &["origin", "joined"])))
}

fn join_answer(c: &Connection, at: &str, a: &str, b: &str) -> Result<Option<AuditRecord>> {
    let row: Option<(String, Option<String>)> = c
        .prepare_cached("SELECT answer, until FROM live_site_answers WHERE a = ?1 AND b = ?2")?
        .query_row(params![a, b], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((answer, until)) = row else {
        return Ok(None);
    };
    let rec = super::joined::answer_record(at, a, b, &answer, until.as_deref());
    Ok(Some(for_actor(rec, owner(c)?)))
}

fn asset_upload(
    c: &Connection,
    home: &Home,
    id: &str,
    r: &mut Reporter,
) -> Result<Option<AuditRecord>> {
    let row = c
        .prepare_cached(
            "SELECT artifact_id, content_type, size, ext, created_at FROM assets WHERE id = ?1",
        )?
        .query_row(params![id], |r| {
            Ok(Asset {
                id: id.to_string(),
                artifact_id: r.get(0)?,
                content_type: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                ext: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .optional()?;
    let Some(asset) = row else { return Ok(None) };
    r.set(Phase::Hashing);
    let blob = ArtifactId::parse(&asset.artifact_id)
        .ok()
        .filter(|_| plain(&asset.ext) && plain(&asset.id))
        .map(|aid| {
            home.assets_dir(&aid)
                .join(format!("{}.{}", asset.id, asset.ext))
        });
    let hashed = match blob {
        Some(p) => hash_file(&p, asset.size, r),
        None => Hashed::Missing,
    };
    let rec = match &hashed {
        Hashed::Sha(h) => super::assets::upload_record(&asset, Some(h)),
        Hashed::Missing => super::assets::upload_record(&asset, None).with("missing", true),
        Hashed::SizeMismatch => {
            super::assets::upload_record(&asset, None).with("size_mismatch", true)
        }
    };
    Ok(Some(rec))
}

/// `version.publish` (or `live.snapshot`, for a live page's version) of
/// version `n` of `aid`, hashing its files and writing the hashes back.
fn version(
    tx: &Transaction<'_>,
    home: &Home,
    at: &str,
    aid: &str,
    n: &str,
    r: &mut Reporter,
) -> Result<Option<AuditRecord>> {
    let Ok(n) = n.parse::<u32>() else {
        return Ok(None);
    };
    let Some(a) = artifact_of(tx, aid)? else {
        return Ok(None);
    };
    let live = a.kind == crate::live::KIND_LIVE;
    let row = tx
        .prepare_cached(
            "SELECT label, note, session_id, files_json, content_sha256
             FROM versions WHERE artifact_id = ?1 AND n = ?2",
        )?
        .query_row(params![aid, n], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .optional()?;
    let Some((label, note, session, files_json, stored_sha)) = row else {
        return Ok(None);
    };
    let parsed: Option<BTreeMap<String, FileMeta>> = serde_json::from_str(&files_json).ok();
    if parsed.is_none() {
        tracing::warn!(artifact = aid, n, "audit backfill: unreadable files_json");
    }
    let mut metas = parsed.clone().unwrap_or_default();
    let (mut missing, mut mismatched) = (Vec::new(), Vec::new());
    let id = ArtifactId::parse(aid).ok();
    for (path, meta) in metas.iter_mut() {
        if meta.sha256.is_some() {
            continue;
        }
        r.set(Phase::Hashing);
        let p = id
            .as_ref()
            .filter(|_| plain(path))
            .map(|id| version_file_path(home, id, n, path));
        match p.map_or(Hashed::Missing, |p| hash_file(&p, meta.size, r)) {
            Hashed::Sha(h) => meta.sha256 = Some(h),
            Hashed::Missing => missing.push(path.clone()),
            Hashed::SizeMismatch => mismatched.push(path.clone()),
        }
    }
    let content = stored_sha.clone().or_else(|| {
        parsed
            .is_some()
            .then(|| content_manifest_sha256(&metas))
            .flatten()
    });
    if let Some(before) = &parsed
        && (*before != metas || content != stored_sha)
    {
        tx.prepare_cached(
            "UPDATE versions SET files_json = ?3, content_sha256 = ?4 WHERE artifact_id = ?1 AND n = ?2",
        )?
        .execute(params![
            aid,
            n,
            serde_json::to_string(&metas).expect("serialisable files"),
            content
        ])?;
    }
    let prev: Option<BTreeMap<String, FileMeta>> = tx
        .prepare_cached(
            "SELECT files_json FROM versions WHERE artifact_id = ?1 AND n < ?2 ORDER BY n DESC LIMIT 1",
        )?
        .query_row(params![aid, n], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|j| serde_json::from_str(&j).ok());
    let carried: Vec<String> = metas
        .iter()
        .filter(|(path, m)| {
            m.sha256.is_some()
                && prev
                    .as_ref()
                    .and_then(|p| p.get(*path))
                    .is_some_and(|p| p.sha256 == m.sha256 && p.size == m.size)
        })
        .map(|(path, _)| path.clone())
        .collect();
    let addresses: Vec<String> = {
        let mut q = tx.prepare_cached(
            "SELECT thread_id, source, created_at FROM version_threads
             WHERE artifact_id = ?1 AND version_n = ?2 ORDER BY created_at, rowid",
        )?;
        let rows = q
            .query_map(params![aid, n], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .filter(|(_, src, link_at)| rides_version(src, link_at, at, live))
            .map(|(tid, ..)| tid)
            .collect()
    };
    let v = Version {
        artifact_id: aid.to_string(),
        n,
        label,
        created_at: at.to_string(),
        session_id: session.clone(),
        files: metas,
        note,
        addresses,
        agent: None,
        agent_harness: None,
        content_sha256: content,
    };
    let page = if live { page_of(tx, aid)? } else { None };
    let mut rec = version_record(&a, &v, &carried, None, page.as_ref());
    let mut fields = vec!["title", "carried", "addresses"];
    if live && page.is_none() {
        // A live page whose key is gone (its artifact deleted).
        rec.kind = AuditKind::LiveSnapshot;
        rec = rec.with("origin", Value::Null).with("path", Value::Null);
        fields.extend(["origin", "path"]);
    }
    if let Some(files) = rec.body.get_mut("files") {
        for p in &missing {
            files[p]["missing"] = true.into();
        }
        for p in &mismatched {
            files[p]["size_mismatch"] = true.into();
        }
    }
    if parsed.is_none() {
        rec = rec.with("files_unreadable", true);
        fields.push("files");
    }
    let rec = inferred(rec, &fields);
    Ok(Some(for_actor(rec, agent(tx, session.as_deref())?)))
}

/// The anchor as `thread.open` records it, from its stored JSON: as live
/// recording renders it when it parses, else the same keys picked from it.
fn anchor(json: &str) -> Value {
    if let Ok(a) = serde_json::from_str::<crate::anchor::Anchor>(json) {
        return super::threads::anchor_record(&a);
    }
    let raw: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let pick = |k: &str| raw.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "kind": pick("kind"),
        "selector": pick("selector"),
        "quote": pick("quote"),
        "prefix": pick("prefix"),
        "suffix": pick("suffix"),
        "html_hash": pick("html_hash"),
        "file": raw.get("file").cloned().unwrap_or_else(|| crate::publish::INDEX.into()),
        "route": pick("route"),
    })
}

fn thread_open(c: &Connection, at: &str, tid: &str) -> Result<Option<AuditRecord>> {
    let row = c
        .prepare_cached(
            "SELECT artifact_id, version_n, anchor_json, live_path, has_clip FROM threads WHERE id = ?1",
        )?
        .query_row(params![tid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })
        .optional()?;
    let Some((aid, version_n, anchor_json, live_path, has_clip)) = row else {
        return Ok(None);
    };
    let first = c
        .prepare_cached(
            "SELECT id, author_kind, author_public_id, via_session_id FROM comments
             WHERE thread_id = ?1 ORDER BY created_at, id LIMIT 1",
        )?
        .query_row(params![tid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .optional()?;
    let rec = open_record(
        at,
        &aid,
        tid,
        version_n,
        anchor(&anchor_json),
        live_path,
        has_clip,
        first.as_ref().map(|f| f.0.as_str()),
    );
    let rec = inferred(rec, &["version_n", "live_path", "has_clip"]);
    let who = match &first {
        Some((_, kind, pid, via)) => author(c, kind, pid.as_deref(), via.as_deref())?,
        None => None,
    };
    Ok(Some(for_actor(rec, who)))
}

fn comment_add(c: &Connection, at: &str, cid: &str) -> Result<Option<AuditRecord>> {
    let row = c
        .prepare_cached(
            "SELECT c.thread_id, t.artifact_id, c.author_kind, c.author_name, c.author_public_id,
                c.via_session_id, s.harness, c.via_page, c.body
             FROM comments c JOIN threads t ON t.id = c.thread_id
             LEFT JOIN sessions s ON s.id = c.via_session_id WHERE c.id = ?1",
        )?
        .query_row(params![cid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                Comment {
                    id: cid.to_string(),
                    thread_id: r.get(0)?,
                    author_kind: r.get(2)?,
                    author_name: r.get(3)?,
                    author_public_id: r.get(4)?,
                    via_harness: r.get(6)?,
                    via_page: r.get(7)?,
                    body: r.get(8)?,
                    created_at: at.to_string(),
                },
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .optional()?;
    let Some((_, aid, comment, via)) = row else {
        return Ok(None);
    };
    let who = author(
        c,
        &comment.author_kind,
        comment.author_public_id.as_deref(),
        via.as_deref(),
    )?;
    Ok(Some(for_actor(comment_record(&aid, &comment), who)))
}

fn thread_move(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let row = c
        .prepare_cached(
            "SELECT thread_id, from_artifact_id, from_url, to_artifact_id, to_url, kind, rule_id
             FROM thread_moves WHERE id = ?1",
        )?
        .query_row(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .optional()?;
    let Some((tid, from, from_url, to, to_url, kind, rule_id)) = row else {
        return Ok(None);
    };
    let origin = match page_of(c, &to)? {
        Some(p) => Some(p.origin),
        None => page_of(c, &from)?.map(|p| p.origin),
    };
    Ok(Some(super::site::move_record(&super::site::MoveFacts {
        at,
        move_id: id,
        thread_id: &tid,
        from: &from,
        from_url: &from_url,
        to: &to,
        to_url: &to_url,
        kind: &kind,
        rule_id: rule_id.as_deref(),
        origin: origin.as_deref(),
    })))
}

/// A send's target: the agent, when every row went to the one session
/// `thread_targets` all name, else `watchers`.
fn send_target(
    c: &Connection,
    row_targets: &[Option<String>],
    thread_targets: &[Option<String>],
) -> Result<Value> {
    let first = row_targets.first().cloned().flatten();
    let one = first.filter(|s| {
        row_targets.iter().all(|t| t.as_deref() == Some(s))
            && thread_targets.iter().all(|t| t.as_deref() == Some(s))
    });
    super::feedback::send_target_value(c, one.as_deref())
}

fn send_batch(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let aid: Option<String> = c
        .prepare_cached("SELECT artifact_id FROM send_batches WHERE id = ?1")?
        .query_row(params![id], |r| r.get(0))
        .optional()?;
    let Some(aid) = aid else { return Ok(None) };
    let rows: Vec<(String, Option<String>)> = c
        .prepare_cached(
            "SELECT id, target_session_id FROM feedback WHERE batch_id = ?1 ORDER BY created_at, id",
        )?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let threads: Vec<(String, Option<String>)> = c
        .prepare_cached(
            "SELECT bt.thread_id, t.target_session_id FROM batch_threads bt
             LEFT JOIN threads t ON t.id = bt.thread_id WHERE bt.batch_id = ?1 ORDER BY bt.rowid",
        )?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let targets: Vec<Option<String>> = rows.iter().map(|r| r.1.clone()).collect();
    let thread_targets: Vec<Option<String>> = threads.iter().map(|t| t.1.clone()).collect();
    let rec = super::feedback::send_record(
        at,
        &aid,
        None,
        send_target(c, &targets, &thread_targets)?,
        rows.into_iter().map(|r| r.0).collect(),
        Some(id),
        threads.into_iter().map(|t| t.0).collect(),
    );
    Ok(Some(inferred(rec, &["target"])))
}

fn send_group(
    c: &Connection,
    at: &str,
    tid: &str,
    created_at: &str,
) -> Result<Option<AuditRecord>> {
    let thread: Option<(String, Option<String>)> = c
        .prepare_cached("SELECT artifact_id, target_session_id FROM threads WHERE id = ?1")?
        .query_row(params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((aid, thread_target)) = thread else {
        return Ok(None);
    };
    let rows: Vec<(String, Option<String>)> = c
        .prepare_cached(
            "SELECT id, target_session_id FROM feedback
             WHERE thread_id = ?1 AND created_at = ?2
                AND (batch_id IS NULL OR batch_id NOT IN (SELECT id FROM send_batches))
             ORDER BY id",
        )?
        .query_map(params![tid, created_at], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let targets: Vec<Option<String>> = rows.iter().map(|r| r.1.clone()).collect();
    let rec = super::feedback::send_record(
        at,
        &aid,
        Some(tid),
        send_target(c, &targets, &[thread_target])?,
        rows.into_iter().map(|r| r.0).collect(),
        None,
        vec![tid.to_string()],
    );
    Ok(Some(inferred(rec, &["target"])))
}

fn delivered(c: &Connection, at: &str, fid: &str) -> Result<Option<AuditRecord>> {
    let row = c
        .prepare_cached(
            "SELECT f.thread_id, t.artifact_id, f.target_session_id, f.delivery_tier
             FROM feedback f JOIN threads t ON t.id = f.thread_id WHERE f.id = ?1",
        )?
        .query_row(params![fid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .optional()?;
    let Some((tid, aid, Some(sid), tier)) = row else {
        return Ok(None);
    };
    let rec = super::feedback::delivered_record(
        at,
        &aid,
        &tid,
        &sid,
        fid,
        tier.as_deref().unwrap_or_default(),
    );
    Ok(Some(for_actor(rec, agent(c, Some(&sid))?)))
}

fn thread_resolve(c: &Connection, at: &str, tid: &str) -> Result<Option<AuditRecord>> {
    let row: Option<(String, Option<String>)> = c
        .prepare_cached("SELECT artifact_id, resolved_by FROM threads WHERE id = ?1")?
        .query_row(params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((aid, by)) = row else {
        return Ok(None);
    };
    let links: Vec<(u32, String, Option<String>)> = c
        .prepare_cached(
            "SELECT vt.version_n, vt.created_at, v.created_at FROM version_threads vt
             LEFT JOIN versions v ON v.artifact_id = vt.artifact_id AND v.n = vt.version_n
             WHERE vt.thread_id = ?1 AND vt.source = 'resolve' ORDER BY vt.created_at, vt.rowid",
        )?
        .query_map(params![tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let live = is_live(c, &aid)?;
    let addressed = links
        .into_iter()
        .find(|(_, link_at, v_at)| {
            !rides_version(
                "resolve",
                link_at,
                v_at.as_deref().unwrap_or_default(),
                live,
            ) && rides_resolve(link_at, Some(at))
        })
        .map(|(n, ..)| n);
    let rec = resolve_record(at, &aid, tid, by.as_deref(), addressed);
    let rec = inferred(rec, &["resolved_by", "addressed_version"]);
    let who = match by.as_deref().and_then(|b| b.strip_prefix("viewer:")) {
        Some(pid) if pid != "anonymous" => person(c, Some(pid))?,
        _ => None,
    };
    Ok(Some(for_actor(rec, who)))
}

/// `thread.addressed` of a resolve link no other event carries: one made by
/// an agent's resolve of a thread already resolved, or by a resolve since
/// undone (the thread reopened).
fn thread_addressed(
    c: &Connection,
    at: &str,
    aid: &str,
    n: &str,
    tid: &str,
) -> Result<Option<AuditRecord>> {
    let Ok(n) = n.parse::<u32>() else {
        return Ok(None);
    };
    let row: Option<(Option<String>, Option<String>, Option<String>)> = c
        .prepare_cached(
            "SELECT v.created_at, t.status, t.resolved_at FROM version_threads vt
             LEFT JOIN versions v ON v.artifact_id = vt.artifact_id AND v.n = vt.version_n
             LEFT JOIN threads t ON t.id = vt.thread_id
             WHERE vt.artifact_id = ?1 AND vt.version_n = ?2 AND vt.thread_id = ?3",
        )?
        .query_row(params![aid, n, tid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((v_at, status, resolved_at)) = row else {
        return Ok(None);
    };
    let resolved_at = resolved_at.filter(|_| status.as_deref() == Some("resolved"));
    if rides_version(
        "resolve",
        at,
        v_at.as_deref().unwrap_or_default(),
        is_live(c, aid)?,
    ) || rides_resolve(at, resolved_at.as_deref())
    {
        return Ok(None);
    }
    let rec = addressed_record(at, aid, tid, n);
    Ok(Some(inferred(rec, &["source"])))
}

fn watch_artifact(c: &Connection, at: &str, sid: &str, aid: &str) -> Result<Option<AuditRecord>> {
    let row: Option<(bool, String)> = c
        .prepare_cached(
            "SELECT replies_armed, source FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
        )?
        .query_row(params![sid, aid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((armed, source)) = row else {
        return Ok(None);
    };
    let rec = artifact_watch_record_at(c, AuditKind::WatchStart, at, sid, aid, armed, &source)?;
    let rec = with_cause(rec, (source == "scope").then_some(Cause::Scope));
    let rec = inferred(rec, &["replies_armed", "source", "cause"]);
    Ok(Some(for_actor(rec, agent(c, Some(sid))?)))
}

fn watch_scope(
    c: &Connection,
    at: &str,
    sid: &str,
    origin: &str,
    path: &str,
) -> Result<Option<AuditRecord>> {
    let armed: Option<bool> = c
        .prepare_cached(
            "SELECT replies_armed FROM live_watches WHERE session_id = ?1 AND origin = ?2 AND path = ?3",
        )?
        .query_row(params![sid, origin, path], |r| r.get(0))
        .optional()?;
    let Some(armed) = armed else { return Ok(None) };
    let rec = scope_watch_record_at(AuditKind::WatchStart, at, sid, origin, path, armed);
    let rec = inferred(rec, &["replies_armed"]);
    Ok(Some(for_actor(rec, agent(c, Some(sid))?)))
}

fn artifact_delete(c: &Connection, at: &str, id: &str) -> Result<Option<AuditRecord>> {
    let row: Option<(String, u32)> = c
        .prepare_cached("SELECT title, current_version FROM artifacts WHERE id = ?1")?
        .query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((title, current)) = row else {
        return Ok(None);
    };
    Ok(Some(delete_record(at, id, &title, current)))
}

/// A question's row: its session, artifact, source, tool-use ID, questions,
/// status, answers and the channel it was answered through.
type QuestionRow = (
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn question(c: &Connection, qid: &str) -> Result<Option<QuestionRow>> {
    Ok(c.prepare_cached(
        "SELECT session_id, artifact_id, source, tool_use_id, questions_json, status,
                answers_json, answered_via FROM questions WHERE id = ?1",
    )?
    .query_row(params![qid], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ))
    })
    .optional()?)
}

fn question_ask(c: &Connection, at: &str, qid: &str) -> Result<Option<AuditRecord>> {
    let Some((sid, aid, source, tool_use_id, questions, ..)) = question(c, qid)? else {
        return Ok(None);
    };
    let mut rec = AuditRecord::new(AuditKind::QuestionAsk, at)
        .with("source", source)
        .with("tool_use_id", tool_use_id)
        .with("questions", json_or_null(Some(questions)));
    rec.ids.question = Some(qid.to_string());
    rec.ids.session = Some(sid.clone());
    rec.ids.artifact = aid;
    Ok(Some(for_actor(rec, agent(c, Some(&sid))?)))
}

fn question_close(c: &Connection, at: &str, qid: &str) -> Result<Option<AuditRecord>> {
    let Some((sid, aid, _, _, _, status, answers, via)) = question(c, qid)? else {
        return Ok(None);
    };
    let (kind, who) = match status.as_str() {
        "answered" => (AuditKind::QuestionAnswer, owner(c)?),
        "declined" => (AuditKind::QuestionDecline, None),
        "released" => (AuditKind::QuestionRelease, None),
        "withdrawn" => (AuditKind::QuestionWithdraw, agent(c, Some(&sid))?),
        _ => return Ok(None),
    };
    let mut rec = AuditRecord::new(kind, at);
    if kind == AuditKind::QuestionAnswer {
        rec = rec
            .with("answers", json_or_null(answers))
            .with("answered_via", via);
    }
    rec.ids.question = Some(qid.to_string());
    rec.ids.session = Some(sid);
    rec.ids.artifact = aid;
    Ok(Some(for_actor(rec, who)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::audit::sha256_hex;
    use crate::store::audit::AuditRow;
    use crate::store::migrations::{AUDIT_MIGRATION, MIGRATIONS};
    use crate::store::test_util::DAEMON;

    /// A reporter that drops what it is told, for driving a batch by hand.
    fn quiet(report: &mut dyn FnMut(&Progress)) -> Reporter<'_> {
        let now = Instant::now();
        Reporter {
            report,
            at: Progress {
                phase: Phase::Planning,
                done: 0,
                total: 0,
                bytes: 0,
            },
            started: now,
            logged: now,
            reported: now,
            batch_bytes: 0,
        }
    }

    /// `2026-01-01T00:mm:ss.000Z`, `s` seconds into the day.
    fn t(s: u32) -> String {
        format!(
            "2026-01-01T{:02}:{:02}:{:02}.000Z",
            s / 3600,
            s / 60 % 60,
            s % 60
        )
    }

    /// A home whose database is at the schema just before the audit
    /// migration, with `fill` run on it.
    fn old_home(dir: &Path, fill: impl FnOnce(&Connection, &Home)) -> Home {
        let home = Home::at(dir.join("ax"));
        home.ensure_dirs().unwrap();
        let c = Connection::open(home.db_path()).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let before = AUDIT_MIGRATION as usize - 1;
        for sql in &MIGRATIONS[..before] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", before as u32)
            .unwrap();
        c.execute_batch("BEGIN").unwrap();
        fill(&c, &home);
        c.execute_batch("COMMIT").unwrap();
        home
    }

    fn session(c: &Connection, id: &str, started: &str, seen: &str, ended: Option<&str>) {
        c.execute(
            "INSERT INTO sessions (id, harness, harness_session_id, cwd, pid, started_at, last_seen_at, ended_at, agent_handle)
             VALUES (?1, 'claude', ?2, '/w', 42, ?3, ?4, ?5, ?6)",
            params![id, format!("h-{id}"), started, seen, ended, format!("a_{id}")],
        )
        .unwrap();
    }

    fn artifact(c: &Connection, id: &str, at: &str, owner: Option<&str>) {
        c.execute(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, owner_session_id, contract_version)
             VALUES (?1, 'Quarterly Review', ?2, ?2, 1, ?3, '1')",
            params![id, at, owner],
        )
        .unwrap();
    }

    /// Version `n` of `aid` with `files` (path, bytes) written to its store,
    /// except those whose bytes are `None`, which are recorded but missing.
    fn version(
        c: &Connection,
        home: &Home,
        aid: &str,
        n: u32,
        at: &str,
        session: Option<&str>,
        files: &[(&str, Option<&[u8]>)],
    ) {
        let mut metas = BTreeMap::new();
        for (path, bytes) in files {
            let size = bytes.map_or(7, |b| b.len() as u64);
            metas.insert(
                path.to_string(),
                FileMeta {
                    content_type: "text/plain".into(),
                    size,
                    sha256: None,
                },
            );
            if let Some(b) = bytes {
                let p = version_file_path(home, &ArtifactId::parse(aid).unwrap(), n, path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, b).unwrap();
            }
        }
        c.execute(
            "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'a note')",
            params![
                aid,
                n,
                format!("v{n}"),
                at,
                session,
                serde_json::to_string(&metas).unwrap()
            ],
        )
        .unwrap();
        c.execute(
            "UPDATE artifacts SET current_version = ?2 WHERE id = ?1",
            params![aid, n],
        )
        .unwrap();
    }

    fn thread(c: &Connection, id: &str, aid: &str, at: &str) {
        c.execute(
            "INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at) VALUES (?1, ?2, 1, ?3, ?4)",
            params![
                id,
                aid,
                serde_json::to_string(&crate::store::test_util::anchor()).unwrap(),
                at
            ],
        )
        .unwrap();
    }

    fn comment(c: &Connection, id: &str, tid: &str, at: &str, agent: Option<&str>) {
        c.execute(
            "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                tid,
                if agent.is_some() { "agent" } else { "viewer" },
                if agent.is_some() { "Claude" } else { "Ana" },
                agent.is_none().then_some("u_ana"),
                agent,
                format!("body of {id}"),
                at
            ],
        )
        .unwrap();
    }

    fn events(st: &Store) -> Vec<AuditRow> {
        st.events_after(0, 100_000).unwrap()
    }

    fn body(e: &AuditRow) -> Value {
        serde_json::from_str(&e.body).unwrap()
    }

    fn kinds(rows: &[AuditRow]) -> Vec<String> {
        rows.iter().map(|e| e.kind.clone()).collect()
    }

    /// One artifact by session `s1`: two versions, a watch, a thread with a
    /// viewer comment and an agent reply, resolved by the agent (linking
    /// version 2), and the session ended by the reaper.
    fn history(c: &Connection, home: &Home, aid: &str) {
        c.execute(
            "INSERT INTO viewers (id, display_name, created_at, public_id) VALUES ('cookie', 'Ana', ?1, 'u_ana')",
            params![t(0)],
        )
        .unwrap();
        session(c, "s1", &t(1), &t(100), Some(&t(500)));
        artifact(c, aid, &t(2), Some("s1"));
        version(
            c,
            home,
            aid,
            1,
            &t(2),
            Some("s1"),
            &[("index.html", Some(b"<p>one</p>"))],
        );
        c.execute(
            "INSERT INTO watches (session_id, artifact_id, created_at) VALUES ('s1', ?1, ?2)",
            params![aid, "2026-01-01T00:00:02.500Z"],
        )
        .unwrap();
        thread(c, "t1", aid, &t(3));
        comment(c, "c1", "t1", &t(3), None);
        comment(c, "c2", "t1", &t(4), Some("s1"));
        version(
            c,
            home,
            aid,
            2,
            &t(5),
            Some("s1"),
            &[("index.html", Some(b"<p>two</p>"))],
        );
        c.execute(
            "UPDATE threads SET status = 'resolved', resolved_at = ?1, resolved_by = 'agent:claude' WHERE id = 't1'",
            params![t(6)],
        )
        .unwrap();
        c.execute(
            "INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
             VALUES (?1, 2, 't1', 'resolve', ?2)",
            params![aid, "2026-01-01T00:00:06.001Z"],
        )
        .unwrap();
    }

    #[test]
    fn backfill_orders_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(
            kinds(&rows),
            [
                "session.start",
                "artifact.create",
                "version.publish",
                "watch.start",
                "thread.open",
                "comment.add",
                "comment.add",
                "version.publish",
                "thread.resolve",
                "session.end",
            ]
        );
        assert!(
            rows.windows(2)
                .all(|w| w[0].seq < w[1].seq && w[0].at <= w[1].at)
        );
        let b: Vec<Value> = rows.iter().map(body).collect();
        assert_eq!(b[4]["first_comment_id"], "c1");
        assert_eq!(b[4]["anchor"]["quote"], "Quarterly goals");
        assert_eq!(rows[5].ids.thread.as_deref(), Some("t1"));
        assert_eq!(b[6]["comment_id"], "c2");
        assert_eq!(b[6]["via_harness"], "claude");
        // The resolve's link rides in the resolve, not in version 2.
        assert_eq!(b[7]["addresses"], json!([]));
        assert_eq!(b[8]["addressed_version"], 2);
        assert_eq!(b[8]["resolved_by"], "agent:claude");
        // Ended 400 s after it was last seen: the reaper.
        assert_eq!(b[9]["reason"], "ttl");
        assert_eq!(b[0]["harness_session_id"], "h-s1");
        assert_eq!(b[0]["pid"], 42);
    }

    #[test]
    fn backfill_hashes_version_files() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let a = aid.as_str().to_string();
        let home = old_home(dir.path(), |c, h| {
            artifact(c, &a, &t(1), None);
            version(
                c,
                h,
                &a,
                1,
                &t(1),
                None,
                &[
                    ("index.html", Some(b"<p>page</p>")),
                    ("js/app.js", Some(b"let a = 1;")),
                ],
            );
            version(
                c,
                h,
                &a,
                2,
                &t(2),
                None,
                &[
                    ("index.html", Some(b"<p>page</p>")),
                    ("js/app.js", Some(b"let a = 2;")),
                    ("gone.css", None),
                ],
            );
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(
            kinds(&rows),
            ["artifact.create", "version.publish", "version.publish"]
        );
        let (v1, v2) = (body(&rows[1]), body(&rows[2]));
        let sha = |b: &[u8]| format!("sha256:{}", sha256_hex(b));
        assert_eq!(v1["files"]["index.html"]["sha256"], sha(b"<p>page</p>"));
        assert_eq!(
            v1["files"]["js/app.js"],
            json!({
                "sha256": sha(b"let a = 1;"), "size": 10, "content_type": "text/plain"
            })
        );
        assert_eq!(
            v2["files"]["gone.css"],
            json!({
                "sha256": null, "size": 7, "content_type": "text/plain", "missing": true
            })
        );
        assert_eq!(v2["carried"], json!(["index.html"]));
        assert_eq!(v1["carried"], json!([]));
        assert_eq!(v2["content_sha256"], Value::Null);
        // The hashes are written back to the versions.
        let stored = st.get_version(&aid, 1).unwrap().unwrap();
        assert_eq!(
            stored.files["js/app.js"].sha256.as_deref(),
            Some(sha256_hex(b"let a = 1;").as_str())
        );
        let expected = content_manifest_sha256(&stored.files).unwrap();
        assert_eq!(stored.content_sha256.as_deref(), Some(expected.as_str()));
        assert_eq!(v1["content_sha256"], expected);
        let v2_row = st.get_version(&aid, 2).unwrap().unwrap();
        assert_eq!(v2_row.content_sha256, None);
        assert_eq!(v2_row.files["gone.css"].sha256, None);
        assert!(v2_row.files["js/app.js"].sha256.is_some());
    }

    #[test]
    fn backfill_marks_rows() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(rows.len(), 10);
        for e in &rows {
            assert!(e.backfilled, "{e:?}");
            assert_eq!(
                serde_json::from_str::<Value>(&e.actor).unwrap(),
                json!({"type": "system", "reason": "backfill"})
            );
            let b = body(e);
            assert_eq!((&b["v"], &b["via"]), (&json!(1), &json!("daemon")));
            for absent in ["git", "git_capture", "call"] {
                assert!(b.get(absent).is_none(), "{absent} in {b}");
            }
            assert_eq!(e.ids.call, None);
        }
        // Who acted, where the history keeps it.
        let agent = json!({"type": "agent", "session_id": "s1", "harness": "claude",
            "harness_session_id": "h-s1", "agent_handle": "a_s1"});
        assert_eq!(body(&rows[2])["for_actor"], agent);
        assert_eq!(body(&rows[6])["for_actor"], agent);
        assert_eq!(
            body(&rows[5])["for_actor"],
            json!({"type": "viewer", "public_id": "u_ana", "display_name": "Ana"})
        );
        assert!(body(&rows[1]).get("for_actor").is_none());
        // A change made afterwards is live, and follows the history.
        let newest = rows.last().unwrap().seq;
        st.set_pinned(DAEMON, &aid, true).unwrap();
        let live = st.events_after(newest, 10).unwrap();
        assert_eq!(kinds(&live), ["artifact.update"]);
        assert!(!live[0].backfilled);
        // Opening again adds nothing, and nothing stays staged.
        drop(st);
        let st = Store::open(&home).unwrap();
        assert_eq!(events(&st).len(), 11);
        let staged = st.with_read(|c| table_exists(c, STAGING)).unwrap();
        assert!(!staged);
    }

    /// Kind, time, actor, IDs and body of each event, in `seq` order.
    fn shape(rows: &[AuditRow]) -> Vec<(String, String, String, crate::audit::AuditIds, String)> {
        rows.iter()
            .map(|e| {
                (
                    e.kind.clone(),
                    e.at.clone(),
                    e.actor.clone(),
                    e.ids.clone(),
                    e.body.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn an_interrupted_backfill_resumes_without_duplicates() {
        let aid = ArtifactId::generate();
        let whole = tempfile::tempdir().unwrap();
        let home = old_home(whole.path(), |c, h| history(c, h, aid.as_str()));
        let reference = shape(&events(&Store::open(&home).unwrap()));

        let cut = tempfile::tempdir().unwrap();
        let home = old_home(cut.path(), |c, h| history(c, h, aid.as_str()));
        {
            let mut conn = super::super::exec::open_writer(&home.db_path()).unwrap();
            super::super::migrate(&mut conn).unwrap();
            let small = Limits {
                rows: 3,
                bytes: 1 << 20,
            };
            // Two batches commit; the third fails before it commits.
            assert!(!run_with(&mut conn, &home, small, Some(2), &mut |_| {}).unwrap());
            let tx = conn.transaction().unwrap();
            batch(&tx, &home, small, &mut quiet(&mut |_| {})).unwrap();
            drop(tx);
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM audit_events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 6);
        }
        let st = Store::open(&home).unwrap();
        assert_eq!(shape(&events(&st)), reference);
    }

    #[test]
    fn concurrent_opens_backfill_once() {
        let aid = ArtifactId::generate();
        let whole = tempfile::tempdir().unwrap();
        let home = old_home(whole.path(), |c, h| history(c, h, aid.as_str()));
        let reference = shape(&events(&Store::open(&home).unwrap()));
        let dir = tempfile::tempdir().unwrap();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let opens: Vec<_> = (0..4)
            .map(|_| {
                let (home, barrier) = (home.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open(&home).map(drop)
                })
            })
            .collect();
        for o in opens {
            o.join().unwrap().unwrap();
        }
        assert_eq!(shape(&events(&Store::open(&home).unwrap())), reference);
    }

    #[test]
    fn backfill_records_live_pages_sends_moves_and_questions() {
        let dir = tempfile::tempdir().unwrap();
        let (page, other) = (ArtifactId::generate(), ArtifactId::generate());
        let (p, o) = (page.as_str().to_string(), other.as_str().to_string());
        let home = old_home(dir.path(), |c, h| {
            c.execute_batch(
                "INSERT INTO viewers (id, display_name, created_at, public_id, owner)
                 VALUES ('ck', NULL, 'x', 'u_owner', 1)",
            )
            .unwrap();
            session(c, "s1", &t(1), &t(2), Some(&t(30)));
            artifact(c, &p, &t(2), None);
            c.execute(
                "UPDATE artifacts SET kind = 'live' WHERE id = ?1",
                params![p],
            )
            .unwrap();
            c.execute(
                "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, 'http://localhost:3000', '/a', ?2)",
                params![p, t(2)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO live_watches (session_id, origin, path, created_at) VALUES ('s1', 'http://localhost:3000', '/', ?1)",
                params![t(2)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO watches (session_id, artifact_id, created_at, source) VALUES ('s1', ?1, ?2, 'scope')",
                params![p, t(2)],
            )
            .unwrap();
            version(
                c,
                h,
                &p,
                1,
                &t(3),
                None,
                &[("index.html", Some(b"<p>live</p>"))],
            );
            thread(c, "t1", &p, &t(4));
            comment(c, "c1", "t1", &t(4), None);
            thread(c, "t2", &p, &t(4));
            comment(c, "c2", "t2", &t(4), None);
            // t1 sent to s1 alone; t2 in a batch to the watchers.
            c.execute(
                "UPDATE threads SET target_session_id = 's1', sent_to_agent = 1 WHERE id = 't1'",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, delivered_at, delivery_tier)
                 VALUES ('f1', 't1', 'c1', 's1', ?1, ?2, 'stop_hook')",
                params![t(5), t(6)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO send_batches (id, artifact_id, note, sent_by, size, created_at) VALUES ('b1', ?1, NULL, 'Ana', 1, ?2)",
                params![p, t(7)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO batch_threads (batch_id, thread_id) VALUES ('b1', 't2')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, batch_id)
                 VALUES ('f2', 't2', 'c2', 's1', ?1, 'b1')",
                params![t(7)],
            )
            .unwrap();
            // t2 resolved by the owner, pending until the snapshot at 9 s.
            c.execute(
                "UPDATE threads SET status = 'resolved', resolved_at = ?1, resolved_by = 'viewer:u_owner' WHERE id = 't2'",
                params![t(8)],
            )
            .unwrap();
            version(
                c,
                h,
                &p,
                2,
                &t(9),
                None,
                &[("index.html", Some(b"<p>live 2</p>"))],
            );
            c.execute(
                "INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 VALUES (?1, 2, 't2', 'resolve', ?2)",
                params![p, t(9)],
            )
            .unwrap();
            // t1 resolved at 10 s, linked again by an agent's resolve at 12 s.
            c.execute(
                "UPDATE threads SET status = 'resolved', resolved_at = ?1, resolved_by = 'agent:claude' WHERE id = 't1'",
                params![t(10)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 VALUES (?1, 2, 't1', 'resolve', ?2)",
                params![p, t(12)],
            )
            .unwrap();
            // A move to another page, and that page's deletion.
            artifact(c, &o, &t(13), None);
            c.execute(
                "INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id, to_url, moved_by, kind, created_at)
                 VALUES ('m1', 't1', ?1, 'http://localhost:3000/a', ?2, 'http://localhost:3000/b', 'owner', 'move', ?3)",
                params![p, o, t(14)],
            )
            .unwrap();
            c.execute(
                "UPDATE artifacts SET deleted_at = ?2 WHERE id = ?1",
                params![o, t(15)],
            )
            .unwrap();
            // A question (migration 20), answered.
            c.execute(
                "INSERT INTO questions (id, session_id, source, questions_json, status, answers_json, answered_via, created_at, closed_at)
                 VALUES ('q1', 's1', 'ask', '[{\"q\":\"Ship?\"}]', 'answered', '[\"yes\"]', 'shell', ?1, ?2)",
                params![t(16), t(17)],
            )
            .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(
            kinds(&rows),
            [
                "session.start",
                "artifact.create",
                "live.page",
                "watch.start",
                "watch.start",
                "live.snapshot",
                "thread.open",
                "thread.open",
                "comment.add",
                "comment.add",
                "thread.send",
                "feedback.delivered",
                "thread.send",
                "thread.resolve",
                "live.snapshot",
                "thread.resolve",
                "thread.addressed",
                "artifact.create",
                "thread.move",
                "artifact.delete",
                "question.ask",
                "question.answer",
                "session.end",
            ]
        );
        let b: Vec<Value> = rows.iter().map(body).collect();
        let by_kind = |i: usize, k: &str| {
            assert_eq!(rows[i].kind, k);
            &b[i]
        };
        assert_eq!(rows[2].ids.origin.as_deref(), Some("http://localhost:3000"));
        // The page watch the scope made, then the scope watch (by natural ID).
        let watches: Vec<&Value> = vec![by_kind(3, "watch.start"), by_kind(4, "watch.start")];
        assert!(
            watches
                .iter()
                .any(|w| w["target"] == "scope" && w["path"] == "/")
        );
        assert!(
            watches
                .iter()
                .any(|w| w["target"] == "page" && w["cause"] == "scope" && w["source"] == "scope")
        );
        assert_eq!(by_kind(5, "live.snapshot")["path"], "/a");
        assert_eq!(
            by_kind(10, "thread.send")["target"],
            json!({"session_id": "s1", "agent_handle": "a_s1"})
        );
        assert_eq!(b[10]["feedback_ids"], json!(["f1"]));
        assert_eq!(b[10]["thread_ids"], json!(["t1"]));
        assert_eq!(by_kind(11, "feedback.delivered")["tier"], "stop_hook");
        assert_eq!(rows[11].ids.session.as_deref(), Some("s1"));
        assert_eq!(by_kind(12, "thread.send")["target"], "watchers");
        assert_eq!(b[12]["batch_id"], "b1");
        assert_eq!(b[12]["feedback_ids"], json!(["f2"]));
        // The owner's resolve linked nothing; the snapshot carried the link.
        assert_eq!(
            by_kind(13, "thread.resolve")["addressed_version"],
            Value::Null
        );
        assert_eq!(
            b[13]["for_actor"],
            json!({"type": "owner", "public_id": "u_owner"})
        );
        assert_eq!(by_kind(14, "live.snapshot")["addresses"], json!(["t2"]));
        // An agent's resolve of a resolved thread: its own event.
        assert_eq!(
            by_kind(15, "thread.resolve")["addressed_version"],
            Value::Null
        );
        assert_eq!(by_kind(16, "thread.addressed")["version_n"], 2);
        assert_eq!(b[16]["source"], "resolve");
        assert_eq!(by_kind(18, "thread.move")["move_kind"], "move");
        assert_eq!(rows[18].ids.artifact2.as_deref(), Some(o.as_str()));
        assert_eq!(rows[18].ids.thread.as_deref(), Some("t1"));
        assert_eq!(
            rows[18].ids.origin.as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(by_kind(19, "artifact.delete")["current_version"], 1);
        assert_eq!(
            by_kind(20, "question.ask")["questions"],
            json!([{"q": "Ship?"}])
        );
        assert_eq!(rows[20].ids.question.as_deref(), Some("q1"));
        assert_eq!(by_kind(21, "question.answer")["answers"], json!(["yes"]));
        assert_eq!(b[21]["for_actor"]["type"], "owner");
        // Ended within the idle window: the agent side ended it.
        assert_eq!(by_kind(22, "session.end")["reason"], "explicit");
    }

    /// The perf daemon's seeded home (scripts/perf-daemon-budget.json
    /// `seed`), times `scale`: 50 sessions; 300 artifacts of one small
    /// version, each watched by its session, with 8 threads of a viewer
    /// comment and 3 replies, every other thread sent and delivered and
    /// every fourth resolved; and 3 large artifacts of 200 files (8 MiB
    /// each), which are not scaled.
    fn perf_seed(c: &Connection, home: &Home, scale: usize) {
        c.execute(
            "INSERT INTO viewers (id, display_name, created_at, public_id) VALUES ('cookie', 'Ana', ?1, 'u_ana')",
            params![t(0)],
        )
        .unwrap();
        // One millisecond apart from 2026-01-01.
        let ts = |i: usize| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(1_767_225_600_000 + i as i64)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        };
        let sessions = 50 * scale;
        for s in 0..sessions {
            session(c, &format!("s{s}"), &ts(s), &ts(s), None);
        }
        let mut clock = sessions;
        let mut tick = || {
            clock += 1;
            ts(clock)
        };
        for a in 0..300 * scale {
            let aid = ArtifactId::generate();
            let aid = aid.as_str();
            let sid = format!("s{}", a % sessions);
            let at = tick();
            artifact(c, aid, &at, Some(&sid));
            let page = format!(
                "<!doctype html><title>Seed {a}</title><main><h2>Quarterly goals</h2><p>{}</p></main>",
                "x".repeat(120)
            );
            version(
                c,
                home,
                aid,
                1,
                &at,
                Some(&sid),
                &[("index.html", Some(page.as_bytes()))],
            );
            c.execute(
                "INSERT INTO watches (session_id, artifact_id, created_at) VALUES (?1, ?2, ?3)",
                params![sid, aid, at],
            )
            .unwrap();
            for th in 0..8 {
                let tid = crate::new_ulid();
                thread(c, &tid, aid, &tick());
                let first = crate::new_ulid();
                comment(c, &first, &tid, &tick(), None);
                for _ in 0..3 {
                    comment(c, &crate::new_ulid(), &tid, &tick(), None);
                }
                if th % 2 == 0 {
                    let (sent, got) = (tick(), tick());
                    c.execute(
                        "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, delivered_at, delivery_tier)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'stop_hook')",
                        params![crate::new_ulid(), tid, first, sid, sent, got],
                    )
                    .unwrap();
                }
                if th % 4 == 0 {
                    c.execute(
                        "UPDATE threads SET status = 'resolved', resolved_at = ?2, resolved_by = 'agent:claude' WHERE id = ?1",
                        params![tid, tick()],
                    )
                    .unwrap();
                }
            }
        }
        for l in 0..3 {
            let aid = ArtifactId::generate();
            let at = tick();
            artifact(c, aid.as_str(), &at, None);
            let files: Vec<(String, Vec<u8>)> = (0..200)
                .map(|f| {
                    // Distinct bytes per file; hashing costs the same whatever
                    // they are.
                    let mut bytes = vec![(l * 200 + f) as u8; (8 << 20) / 200];
                    bytes[..8].copy_from_slice(&((l * 200 + f) as u64).to_le_bytes());
                    (format!("assets/f{l}-{f}.bin"), bytes)
                })
                .collect();
            let refs: Vec<(&str, Option<&[u8]>)> = files
                .iter()
                .map(|(p, b)| (p.as_str(), Some(b.as_slice())))
                .collect();
            version(c, home, aid.as_str(), 1, &at, None, &refs);
        }
    }

    /// The CPU time this thread has used.
    fn thread_cpu() -> std::time::Duration {
        use nix::time::{ClockId, clock_gettime};
        clock_gettime(ClockId::CLOCK_THREAD_CPUTIME_ID)
            .unwrap()
            .into()
    }

    /// The budget holds for the work the backfill does (its thread's CPU
    /// time, which the open runs on); wall time on a loaded machine is
    /// printed alongside.
    #[test]
    fn backfill_on_perf_seed_under_3s() {
        let dir = tempfile::tempdir().unwrap();
        let home = old_home(dir.path(), |c, h| perf_seed(c, h, 1));
        let (wall, cpu) = (std::time::Instant::now(), thread_cpu());
        let st = Store::open(&home).unwrap();
        let (wall, cpu) = (wall.elapsed(), thread_cpu() - cpu);
        let n = st.newest_seq().unwrap();
        eprintln!("backfill of the perf seed: {n} events, {cpu:?} CPU, {wall:?} wall");
        // 50 sessions, 303 artifacts and versions, 300 watches, 2400
        // threads, 9600 comments, 1200 sends and deliveries, 600 resolves.
        assert_eq!(n, 50 + 303 * 2 + 300 + 2400 + 9600 + 1200 * 2 + 600);
        assert!(cpu < std::time::Duration::from_secs(3), "{cpu:?}");
    }

    /// Measures the backfill of the perf seed times `CLAX_BACKFILL_SCALE`
    /// (default 10). Run in release, alone, under `/usr/bin/time -l`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn backfill_at_scale() {
        let scale = std::env::var("CLAX_BACKFILL_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let home = old_home(dir.path(), |c, h| perf_seed(c, h, scale));
        let db = std::fs::metadata(home.db_path()).unwrap().len();
        eprintln!(
            "seeded scale {scale} in {:?}: {} MiB database",
            started.elapsed(),
            db >> 20
        );
        let started = std::time::Instant::now();
        let st = Store::open(&home).unwrap();
        eprintln!(
            "backfill: {} events in {:?}",
            st.newest_seq().unwrap(),
            started.elapsed()
        );
    }

    /// A writer that counts what it is given and keeps none of it.
    struct Counting(u64);

    impl std::io::Write for Counting {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len() as u64;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Measures exporting the backfilled perf seed times
    /// `CLAX_BACKFILL_SCALE` (default 10): the whole install in the
    /// artifacts shape, then in the journal shape. With `CLAX_SCALE_HOME`
    /// set, the home there is seeded once and reused, so a second run
    /// measures opening and exporting alone; with `CLAX_SCALE_EXPORT=0` it
    /// opens without exporting, the baseline for peak memory. Run in
    /// release, alone, under `/usr/bin/time -l`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn export_at_scale() {
        use crate::toolpath::RenderEnv;
        use crate::toolpath::project::{Export, ExportEnv, Shape};
        let scale = std::env::var("CLAX_BACKFILL_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::env::var_os("CLAX_SCALE_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| tmp.path().to_path_buf());
        let home = if dir.join("ax").join("clax.db").exists() {
            Home::at(dir.join("ax"))
        } else {
            old_home(&dir, |c, h| perf_seed(c, h, scale))
        };
        let started = std::time::Instant::now();
        let st = Store::open(&home).unwrap();
        eprintln!(
            "opened: {} events in {:?}",
            st.newest_seq().unwrap(),
            started.elapsed()
        );
        if std::env::var("CLAX_SCALE_EXPORT").as_deref() == Ok("0") {
            return;
        }
        let env = ExportEnv {
            render: RenderEnv::export(st.install_id().unwrap(), "http://localhost:7480"),
            clax_version: "0.0.0".into(),
            clax_commit: "unknown".into(),
        };
        for shape in [Shape::Artifacts, Shape::Journal] {
            let req = Export {
                shape,
                ..Default::default()
            };
            let mut out = Counting(0);
            let started = std::time::Instant::now();
            st.export(&req, &env, &mut out).unwrap();
            eprintln!(
                "export {}: {} MiB in {:?}",
                shape.name(),
                out.0 >> 20,
                started.elapsed()
            );
        }
    }

    /// A reopened thread keeps the link its agent's resolve made to version
    /// 1; it is its own `thread.addressed`, not version 1's address.
    #[test]
    fn a_reopened_threads_resolve_link_is_its_own_event() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let a = aid.as_str().to_string();
        let home = old_home(dir.path(), |c, h| {
            artifact(c, &a, &t(1), None);
            version(
                c,
                h,
                &a,
                1,
                &t(1),
                None,
                &[("index.html", Some(b"<p>1</p>"))],
            );
            thread(c, "t1", &a, &t(2));
            comment(c, "c1", "t1", &t(2), None);
            // Resolved at 10 s linking version 1, then reopened (the
            // resolve's time is gone, its link is kept).
            c.execute(
                "INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 VALUES (?1, 1, 't1', 'resolve', ?2)",
                params![a, t(10)],
            )
            .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(
            kinds(&rows),
            [
                "artifact.create",
                "version.publish",
                "thread.open",
                "comment.add",
                "thread.addressed"
            ]
        );
        assert_eq!(body(&rows[1])["addresses"], json!([]));
        assert_eq!(rows[4].at, t(10));
        assert_eq!(body(&rows[4])["version_n"], 1);
        assert_eq!(body(&rows[4])["inferred"], json!(["source"]));
    }

    #[test]
    fn a_row_that_cannot_be_converted_is_skipped_and_the_rest_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| {
            history(c, h, aid.as_str());
            // A pid stored as text: the session cannot be read.
            session(c, "bad", &t(0), &t(0), None);
            c.execute("UPDATE sessions SET pid = 'not a pid' WHERE id = 'bad'", [])
                .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        assert_eq!(rows[0].kind, "backfill.skip");
        let b = body(&rows[0]);
        assert_eq!(
            (&b["table"], &b["row_id"]),
            (&json!("sessions"), &json!("bad"))
        );
        assert!(b["reason"].as_str().unwrap().contains("pid"), "{b}");
        assert_eq!(rows.len(), 11, "the rest is recorded");
        assert!(st.with_read(marked).unwrap());
    }

    /// A home at main's joined-sites schema (19): a page merged away into
    /// the site's page by a join, with its snapshot, the join's thread move
    /// and an answer; and an asset with its blob and one without.
    #[test]
    fn a_joined_sites_home_backfills_its_sites_and_merged_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (site, merged) = (ArtifactId::generate(), ArtifactId::generate());
        let (s, m) = (site.as_str().to_string(), merged.as_str().to_string());
        const A: &str = "http://localhost:7702";
        const B: &str = "http://localhost:7703";
        let home = old_home(dir.path(), |c, h| {
            c.execute_batch(
                "INSERT INTO viewers (id, display_name, created_at, public_id, owner)
                 VALUES ('ck', NULL, 'x', 'u_owner', 1)",
            )
            .unwrap();
            for (aid, origin) in [(&s, A), (&m, B)] {
                artifact(c, aid, &t(1), None);
                c.execute(
                    "UPDATE artifacts SET kind = 'live' WHERE id = ?1",
                    params![aid],
                )
                .unwrap();
                version(
                    c,
                    h,
                    aid,
                    1,
                    &t(1),
                    None,
                    &[("index.html", Some(origin.as_bytes()))],
                );
            }
            c.execute(
                "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, ?2, '/', ?3)",
                params![s, A, t(1)],
            )
            .unwrap();
            thread(c, "t1", &m, &t(2));
            comment(c, "c1", "t1", &t(2), None);
            c.execute_batch(&format!(
                "INSERT INTO live_sites (origin, site, joined_at, last_used_at) VALUES
                    ('{A}', '{A}', '{j0}', '{j0}'), ('{B}', '{A}', '{j}', '{j}');
                 UPDATE threads SET artifact_id = '{s}' WHERE id = 't1';
                 INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id,
                    to_url, moved_by, kind, created_at)
                    VALUES ('m1', 't1', '{m}', '{B}/', '{s}', '{A}/', 'owner', 'join', '{j}');
                 INSERT INTO live_merged_pages (artifact_id, origin, path, merged_into, merged_at)
                    VALUES ('{m}', '{B}', '/', '{s}', '{j}');
                 INSERT INTO live_site_answers (a, b, answer, until, created_at)
                    VALUES ('http://127.0.0.1:7704', '{A}', 'never', NULL, '{n}');",
                j0 = "2026-01-01T00:00:04.999Z",
                j = t(5),
                n = t(6),
            ))
            .unwrap();
            let assets = h.assets_dir(&ArtifactId::parse(&s).unwrap());
            std::fs::create_dir_all(&assets).unwrap();
            std::fs::write(assets.join("as1.png"), b"png!").unwrap();
            c.execute_batch(&format!(
                "INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at) VALUES
                    ('as1', '{s}', 'image/png', 4, 'png', '{a}'), ('as2', '{s}', 'image/png', 9, 'png', '{a}')",
                a = t(1),
            ))
            .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        let ks = kinds(&rows);
        assert_eq!(
            ks,
            [
                "artifact.create",
                "artifact.create",
                "asset.upload",
                "asset.upload",
                "live.page",
                "live.page",
                "live.snapshot",
                "live.snapshot",
                "thread.open",
                "comment.add",
                "live.join",
                "thread.move",
                "live.page_merge",
                "live.join_answer",
            ],
        );
        let of = |aid: &str, kind: &str| {
            rows.iter()
                .find(|e| e.kind == kind && e.ids.artifact.as_deref() == Some(aid))
                .map(body)
                .unwrap_or_else(|| panic!("no {kind} of {aid}"))
        };
        // The merged-away page is still a live page, of its own origin.
        assert_eq!(of(&m, "live.page")["origin"], B);
        assert_eq!(of(&m, "live.page")["inferred"], json!(["at"]));
        assert_eq!(of(&m, "live.snapshot")["origin"], B);
        assert_eq!(of(&s, "live.snapshot")["origin"], A);
        let merge = of(&m, "live.page_merge");
        assert_eq!(
            (&merge["origin"], &merge["merged_into"]),
            (&json!(B), &json!(s))
        );
        let join = rows.iter().find(|e| e.kind == "live.join").unwrap();
        assert_eq!(join.ids.origin.as_deref(), Some(A));
        let jb = body(join);
        assert_eq!(
            (&jb["origin"], &jb["joined"], &jb["with"]),
            (&json!(B), &json!([B]), &Value::Null)
        );
        assert_eq!(of(&m, "thread.move")["move_kind"], "join");
        let answer = rows
            .iter()
            .find(|e| e.kind == "live.join_answer")
            .map(body)
            .unwrap();
        assert_eq!(answer["answer"], "never");
        assert_eq!(
            answer["for_actor"],
            json!({"type": "owner", "public_id": "u_owner"})
        );
        let uploads: Vec<Value> = rows
            .iter()
            .filter(|e| e.kind == "asset.upload")
            .map(body)
            .collect();
        assert_eq!(
            uploads[0]["sha256"],
            format!("sha256:{}", sha256_hex(b"png!"))
        );
        assert_eq!(uploads[0]["path"], "/_blob/as1");
        assert_eq!(
            (&uploads[1]["sha256"], &uploads[1]["missing"]),
            (&Value::Null, &json!(true))
        );
    }

    /// The marker, not the schema, says whether the backfill ran: a home
    /// with live events and no marker backfills what came before them, once.
    #[test]
    fn the_marker_alone_decides_and_live_events_bound_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        {
            // Migrated with no backfill, as by an earlier build: one live
            // event after the history's first five.
            let mut conn = super::super::exec::open_writer(&home.db_path()).unwrap();
            super::super::migrate(&mut conn).unwrap();
            conn.execute(
                "INSERT INTO audit_events (at, kind, actor, body) VALUES (?1, 'artifact.update', '{}', '{}')",
                params!["2026-01-01T00:00:03.500Z"],
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        let back: Vec<&str> = rows
            .iter()
            .filter(|e| e.backfilled)
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(
            back,
            [
                "session.start",
                "artifact.create",
                "version.publish",
                "watch.start",
                "thread.open",
                "comment.add"
            ]
        );
        drop(st);
        let n = events(&Store::open(&home).unwrap()).len();
        assert_eq!(n, rows.len(), "once");
    }

    #[test]
    fn progress_names_each_phase_and_ends_with_every_row_done() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        let mut seen = Vec::new();
        Store::open_reporting(&home, &mut |p| seen.push(*p)).unwrap();
        assert_eq!(seen.first().unwrap().phase, Phase::Planning);
        assert!(seen.iter().any(|p| p.phase == Phase::Hashing));
        let last = seen.last().unwrap();
        // Eleven rows: the resolve link the resolve itself carries is
        // planned as a candidate `thread.addressed` and makes no event.
        assert_eq!((last.phase, last.done, last.total), (Phase::Done, 11, 11));
        assert!(last.bytes > 0);
        assert!(
            seen.windows(2)
                .all(|w| w[0].done <= w[1].done && w[0].bytes <= w[1].bytes)
        );
        // Once done, an open reports nothing.
        seen.clear();
        Store::open_reporting(&home, &mut |p| seen.push(*p)).unwrap();
        assert!(seen.is_empty());
    }

    /// Every ordinary table but the audit's (not the inbox's full-text
    /// index, a virtual table, nor its shadow tables), as rows of text, with
    /// `files_json`'s per-file hashes and `content_sha256` taken out.
    fn everything_else(home: &Home) -> BTreeMap<String, Vec<String>> {
        let c = Connection::open(home.db_path()).unwrap();
        let tables: Vec<String> = c
            .prepare(
                "SELECT name FROM pragma_table_list
                 WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite_%'
                 AND name NOT IN ('audit_events', 'install', 'audit_backfill')",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut out = BTreeMap::new();
        for t in tables {
            let mut q = c
                .prepare(&format!("SELECT * FROM {t} ORDER BY rowid"))
                .unwrap();
            let names: Vec<String> = q.column_names().iter().map(|s| s.to_string()).collect();
            let rows: Vec<String> = q
                .query_map([], |r| {
                    let mut cells = Vec::new();
                    for (i, name) in names.iter().enumerate() {
                        let v: rusqlite::types::Value = r.get(i)?;
                        let text = match (name.as_str(), v) {
                            ("content_sha256", _) => "-".to_string(),
                            ("files_json", rusqlite::types::Value::Text(j)) => {
                                let mut f: Value = serde_json::from_str(&j).unwrap();
                                for m in f.as_object_mut().unwrap().values_mut() {
                                    m.as_object_mut().unwrap().remove("sha256");
                                }
                                f.to_string()
                            }
                            (_, v) => format!("{v:?}"),
                        };
                        cells.push(text);
                    }
                    Ok(cells.join("|"))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            out.insert(t, rows);
        }
        out
    }

    #[test]
    fn the_backfill_changes_nothing_but_version_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| history(c, h, aid.as_str()));
        {
            let mut conn = super::super::exec::open_writer(&home.db_path()).unwrap();
            super::super::migrate(&mut conn).unwrap();
        }
        let before = everything_else(&home);
        drop(Store::open(&home).unwrap());
        assert_eq!(everything_else(&home), before);
    }

    /// The keys of each kind's bodies, apart from the backfill's own
    /// (`inferred`, `missing`, `size_mismatch`, `files_unreadable`,
    /// `for_actor`) and the envelope's agent fields.
    fn shapes(rows: &[AuditRow]) -> BTreeMap<String, Vec<std::collections::BTreeSet<String>>> {
        let own = [
            "inferred",
            "for_actor",
            "files_unreadable",
            "git",
            "git_capture",
            "call",
        ];
        let mut out: BTreeMap<String, Vec<std::collections::BTreeSet<String>>> = BTreeMap::new();
        for e in rows {
            let keys = body(e)
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| !own.contains(&k.as_str()))
                .cloned()
                .collect();
            out.entry(e.kind.clone()).or_default().push(keys);
        }
        out
    }

    /// A history made through the store's own API: sessions (one ended),
    /// an artifact with a watch, threads, an agent reply, a send, a
    /// delivery, a resolve, an asset, two live pages joined into one site
    /// and settled, a join answer, and a deleted artifact.
    pub(crate) fn live_history(st: &Store) {
        use crate::live::PageKey;
        use crate::store::feedback::TakeFeedback;
        use crate::store::test_util::{anchor, artifact as make, session as start};
        use crate::store::threads::{NewComment, NewThread};
        let sid = start(st, "claude", "h1");
        let aid = make(st, Some(&sid));
        st.watch(DAEMON, &sid, &aid, true).unwrap();
        let thread = |body: &str| {
            st.create_thread(
                DAEMON,
                &aid,
                NewThread {
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Ana".into(),
                    author_public_id: None,
                    body: body.into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap()
            .id
        };
        let t1 = thread("one");
        let t2 = thread("two");
        st.add_comment(
            DAEMON,
            &t1,
            NewComment {
                author_kind: crate::store::threads::AUTHOR_AGENT,
                author_name: "Claude".into(),
                author_public_id: None,
                via_session_id: Some(sid.clone()),
                body: "on it".into(),
                via_page: false,
            },
        )
        .unwrap();
        st.send_to_agent(DAEMON, &t2).unwrap();
        st.take_feedback(
            DAEMON,
            &TakeFeedback {
                session_id: sid.clone(),
                tier: crate::feedback::Tier::Piggyback,
                artifact_id: None,
                include_resends: false,
            },
            "http://localhost:1",
        )
        .unwrap();
        st.resolve_thread(DAEMON, &t1, "agent:claude").unwrap();
        st.add_asset(DAEMON, &aid, "image/png", b"\x89PNG\r\n\x1a\nfake")
            .unwrap();
        let page = |origin: &str| {
            st.ensure_live_page(
                DAEMON,
                &PageKey {
                    origin: origin.into(),
                    path: "/".into(),
                },
                "App",
                Some(origin.as_bytes()),
            )
            .unwrap()
        };
        page("http://localhost:7702");
        page("http://localhost:7703");
        st.join_origins(DAEMON, "http://localhost:7703", "http://localhost:7702")
            .unwrap();
        st.settle_joined_pages(DAEMON, "http://localhost:7702")
            .unwrap();
        st.answer_join(
            DAEMON,
            "http://127.0.0.1:1",
            "http://localhost:7702",
            "never",
        )
        .unwrap();
        let other = make(st, None);
        st.delete_artifact(DAEMON, &other).unwrap();
        let ended = start(st, "codex", "h2");
        st.end_session(DAEMON, &ended).unwrap();
    }

    /// A history made through the store's own API, then forgotten by the
    /// audit and backfilled: each kind's backfilled bodies have the keys its
    /// live bodies have.
    #[test]
    fn backfilled_and_live_events_of_a_kind_have_the_same_shape() {
        use crate::store::test_util::store;
        let (_d, st) = store();
        live_history(&st);
        let live = shapes(&events(&st));
        let home = st.home().clone();
        st.with_write(|c| {
            c.execute_batch("DELETE FROM audit_events; DELETE FROM install WHERE k = 'backfill';")?;
            Ok(())
        })
        .unwrap();
        drop(st);
        let back = shapes(&events(&Store::open(&home).unwrap()));
        let mut compared = Vec::new();
        for (kind, bodies) in &back {
            let Some(theirs) = live.get(kind) else {
                panic!("{kind} is backfilled but never recorded live here");
            };
            let all: std::collections::BTreeSet<String> =
                theirs.iter().flatten().cloned().collect();
            let common: std::collections::BTreeSet<String> =
                theirs.iter().skip(1).fold(theirs[0].clone(), |a, b| {
                    a.intersection(b).cloned().collect()
                });
            for keys in bodies {
                let keys: std::collections::BTreeSet<String> = keys
                    .iter()
                    .filter(|k| !["missing", "size_mismatch"].contains(&k.as_str()))
                    .cloned()
                    .collect();
                assert!(
                    keys.is_subset(&all),
                    "{kind}: {keys:?} has keys live {all:?} lacks"
                );
                assert!(
                    common.is_subset(&keys),
                    "{kind}: {keys:?} lacks keys of live {common:?}"
                );
            }
            compared.push(kind.clone());
        }
        for k in [
            "session.start",
            "session.end",
            "artifact.create",
            "artifact.delete",
            "asset.upload",
            "version.publish",
            "live.page",
            "live.snapshot",
            "thread.open",
            "comment.add",
            "thread.send",
            "feedback.delivered",
            "thread.resolve",
            "watch.start",
            "live.join",
            "live.page_merge",
            "live.join_answer",
        ] {
            assert!(
                compared.iter().any(|c| c == k),
                "{k} not compared: {compared:?}"
            );
        }
    }

    /// Measures a backfill dominated by hashing, as the owner's home is
    /// (few rows, hundreds of MiB of files): `CLAX_BACKFILL_VERSIONS`
    /// versions (default 150) of one file each, `CLAX_BACKFILL_MIB` MiB in
    /// all (default 500). Run in release, alone, under `/usr/bin/time -l`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn backfill_hash_heavy() {
        let var = |k: &str, d: usize| {
            std::env::var(k)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(d)
        };
        let (versions, mib) = (
            var("CLAX_BACKFILL_VERSIONS", 150),
            var("CLAX_BACKFILL_MIB", 500),
        );
        let dir = tempfile::tempdir().unwrap();
        let each = (mib << 20) / versions;
        let home = old_home(dir.path(), |c, h| {
            for i in 0..versions {
                let aid = ArtifactId::generate();
                artifact(c, aid.as_str(), &t(i as u32), None);
                let mut bytes = vec![b'x'; each];
                bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
                version(
                    c,
                    h,
                    aid.as_str(),
                    1,
                    &t(i as u32),
                    None,
                    &[("index.html", Some(&bytes))],
                );
            }
        });
        let started = std::time::Instant::now();
        let st = Store::open(&home).unwrap();
        eprintln!(
            "hash-heavy backfill: {versions} versions, {mib} MiB: {} events in {:?} wall",
            st.newest_seq().unwrap(),
            started.elapsed()
        );
    }

    /// An agent publishes and resolves within the second: on an html
    /// artifact the link is the resolve's, as live recording has it.
    #[test]
    fn a_resolve_link_made_just_after_a_publish_is_the_resolves_off_live_pages() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let a = aid.as_str().to_string();
        let home = old_home(dir.path(), |c, h| {
            artifact(c, &a, &t(1), None);
            thread(c, "t1", &a, &t(2));
            comment(c, "c1", "t1", &t(2), None);
            version(
                c,
                h,
                &a,
                2,
                "2026-01-01T00:00:05.000Z",
                None,
                &[("index.html", Some(b"2"))],
            );
            c.execute_batch(&format!(
                "UPDATE threads SET status = 'resolved', resolved_at = '2026-01-01T00:00:05.400Z',
                    resolved_by = 'agent:claude' WHERE id = 't1';
                 INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                    VALUES ('{a}', 2, 't1', 'resolve', '2026-01-01T00:00:05.401Z');"
            ))
            .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        let of = |k: &str| rows.iter().find(|e| e.kind == k).map(body).unwrap();
        assert_eq!(of("version.publish")["addresses"], json!([]));
        assert_eq!(of("thread.resolve")["addressed_version"], 2);
        assert!(rows.iter().all(|e| e.kind != "thread.addressed"));
    }

    #[test]
    fn a_row_whose_time_is_not_text_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let aid = ArtifactId::generate();
        let home = old_home(dir.path(), |c, h| {
            history(c, h, aid.as_str());
            session(c, "blob", &t(0), &t(0), None);
            c.execute(
                "UPDATE sessions SET started_at = x'00ff' WHERE id = 'blob'",
                [],
            )
            .unwrap();
        });
        let st = Store::open(&home).unwrap();
        let rows = events(&st);
        let skip: Vec<Value> = rows
            .iter()
            .filter(|e| e.kind == "backfill.skip")
            .map(body)
            .collect();
        assert_eq!(skip.len(), 1, "{rows:?}");
        assert_eq!(
            (&skip[0]["table"], &skip[0]["row_id"]),
            (&json!("sessions"), &json!("blob"))
        );
        assert!(skip[0]["reason"].as_str().unwrap().contains("not text"));
        assert_eq!(rows.len(), 11, "the rest is recorded");
    }

    #[test]
    fn a_fresh_database_stages_nothing() {
        let (_d, st) = crate::store::test_util::store();
        assert!(events(&st).is_empty());
        assert!(!st.with_read(|c| table_exists(c, STAGING)).unwrap());
        assert!(st.with_read(marked).unwrap());
    }
}
