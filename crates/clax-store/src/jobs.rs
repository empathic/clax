//! The Clax app's jobs (spec 2026-10-19-in-process-core-design §8.1): each
//! make or change the app's queue runs, its committed steps, the threads it
//! answers and the questions it asked (`jobs`, `job_steps`, `job_threads`,
//! `job_questions`), and the agent's remembered read consents
//! (`agent_read_consents`), all from the jobs migration
//! ([`JOBS_MIGRATION`](crate::migrations::JOBS_MIGRATION)).
//!
//! These rows are the truth about every job. Every write answers the job's
//! row after it, its `rev` one above the row before, so a client applying
//! rows only when their `rev` is above the one it holds applies each once.
//! The store knows nothing of who may write a job: the queue's owner is
//! the core's, in memory, and no rule here reads a clock.

use super::Store;
use super::questions::Reason;
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, RwLock};

/// The state of a job, as `jobs.state` keeps it (its `CHECK` lists the
/// same eight). Which state a job may move to from which is [`may_move`]'s
/// table and nothing else's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Created, not yet started.
    Queued,
    Running,
    /// Holds a question it asked that is not yet closed.
    Waiting,
    Paused,
    Stopped,
    Failed,
    /// Final.
    Cancelled,
    /// Final.
    Done,
}

impl JobState {
    /// The text `jobs.state` keeps.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Waiting => "waiting",
            JobState::Paused => "paused",
            JobState::Stopped => "stopped",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Done => "done",
        }
    }

    fn parse(s: &str) -> Option<JobState> {
        Some(match s {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "waiting" => JobState::Waiting,
            "paused" => JobState::Paused,
            "stopped" => JobState::Stopped,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            "done" => JobState::Done,
            _ => return None,
        })
    }

    /// Whether no write moves the job out of it.
    pub fn is_final(self) -> bool {
        matches!(self, JobState::Cancelled | JobState::Done)
    }
}

impl rusqlite::types::ToSql for JobState {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for JobState {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = v.as_str()?;
        JobState::parse(s)
            .ok_or_else(|| rusqlite::types::FromSqlError::Other(format!("no job state {s}").into()))
    }
}

/// What moves a job, for [`may_move`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Move {
    /// A patch naming this state ([`Store::patch_job`]), or a cancel by
    /// the store when the job's artifact goes.
    To(JobState),
    /// The job records a question it asked ([`Store::ask_job_question`]).
    Ask,
    /// A question the job asked closed.
    Close,
    /// The job commits a step, ending it when `done`
    /// ([`Store::commit_step`]).
    Step { done: bool },
}

/// The state a job in `from` is in after `by`, given whether it then
/// `waits` on a question it asked that is not closed; `None` when `by` may
/// not happen to a job in `from`. The one table of the jobs' moves:
///
/// - `cancelled` and `done` are final: nothing moves a job out of them.
/// - `waiting` is entered only by asking, and only from `running`; a job
///   in another state that asks keeps its state.
/// - A job waits exactly while it holds an unclosed question and would
///   otherwise run: a move to `running` lands in `waiting` while it does,
///   and only a question's close takes a waiting job back to `running`,
///   once it waits on none.
/// - A patch moves a job to `running`, `paused`, `stopped`, `failed` or
///   `cancelled`, or names the state it is in; never to `queued` (only
///   creation makes it), `waiting` (only asking) or `done` (only a step).
/// - A step moves nothing, unless it is the job's last (`done`).
pub(crate) fn may_move(from: JobState, by: Move, waits: bool) -> Option<JobState> {
    use JobState::*;
    let run = if waits { Waiting } else { Running };
    match (from, by) {
        (Cancelled | Done, _) => None,
        (from, Move::To(to)) if to == from => Some(from),
        (_, Move::To(Running)) => Some(run),
        (_, Move::To(to @ (Paused | Stopped | Failed | Cancelled))) => Some(to),
        (_, Move::To(Queued | Waiting | Done)) => None,
        (Running, Move::Ask) => Some(run),
        (from, Move::Ask) => Some(from),
        (Waiting, Move::Close) => Some(run),
        (from, Move::Close) => Some(from),
        (_, Move::Step { done: true }) => Some(Done),
        (from, Move::Step { done: false }) => Some(from),
    }
}

/// A committed change of a job, as its listener hears it
/// ([`Store::set_job_listener`]): what `Event::Job` carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobChange {
    pub id: String,
    pub client: String,
    pub state: JobState,
    pub step: Option<String>,
    pub rev: i64,
}

/// The installed job listener, cloned out of its lock to be called.
type JobListener = Arc<dyn Fn(Vec<JobChange>) + Send + Sync>;

/// The jobs a write transaction changed, and the listener told of them
/// once it commits ([`Store::with_tx`]). The write connection's update
/// hook records the `rowid` of every `jobs` row written, by whatever
/// statement, trigger or foreign key action; a write that deletes a job
/// it changed leaves its last row here first ([`JobChanges::going`]).
#[derive(Default)]
pub(crate) struct JobChanges {
    /// The rows of `jobs` written in the transaction under way, in order.
    pub(crate) rowids: Arc<Mutex<Vec<i64>>>,
    /// Rows of jobs the transaction changed and then deletes, by `rowid`.
    gone: Mutex<Vec<(i64, JobChange)>>,
    listener: RwLock<Option<JobListener>>,
}

impl JobChanges {
    /// Forgets what an earlier transaction recorded.
    pub(crate) fn clear(&self) {
        self.rowids.lock().unwrap().clear();
        self.gone.lock().unwrap().clear();
    }

    /// Keeps `change`, job `rowid`'s row, for when the transaction deletes
    /// the job after changing it.
    fn going(&self, rowid: i64, change: JobChange) {
        self.gone.lock().unwrap().push((rowid, change));
    }

    /// The changes recorded, read on `c` in the transaction before it
    /// commits: each job once, as the transaction leaves it, or as it was
    /// before its delete.
    pub(crate) fn collect(&self, c: &Connection) -> clax_core::Result<Vec<JobChange>> {
        let rowids = std::mem::take(&mut *self.rowids.lock().unwrap());
        let mut gone = std::mem::take(&mut *self.gone.lock().unwrap());
        let mut out = Vec::with_capacity(rowids.len());
        for rowid in rowids {
            let row = c
                .prepare_cached("SELECT id, client, state, step, rev FROM jobs WHERE rowid = ?1")?
                .query_row(params![rowid], |r| {
                    Ok(JobChange {
                        id: r.get(0)?,
                        client: r.get(1)?,
                        state: r.get(2)?,
                        step: r.get(3)?,
                        rev: r.get(4)?,
                    })
                })
                .optional()?;
            let row = row.or_else(|| {
                let i = gone.iter().rposition(|(r, _)| *r == rowid)?;
                Some(gone.swap_remove(i).1)
            });
            out.extend(row);
        }
        Ok(out)
    }

    /// Tells the listener of `changes`, outside the write turn.
    pub(crate) fn tell(&self, changes: Vec<JobChange>) {
        if changes.is_empty() {
            return;
        }
        let listen = self.listener.read().unwrap().clone();
        if let Some(listen) = listen {
            listen(changes);
        }
    }
}

/// A job's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JobRow {
    pub id: String,
    /// Who runs it (`app` for the app's queue).
    pub client: String,
    /// `make` or `change`; opaque to the core.
    pub kind: String,
    pub artifact_id: Option<String>,
    pub state: JobState,
    /// The key of the step committed last.
    pub step: Option<String>,
    pub title: Option<String>,
    pub input: serde_json::Value,
    pub plan: Option<serde_json::Value>,
    pub carry: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    /// 1 when created, one more at every write of the job.
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// A job with everything `GET /api/jobs` answers for it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JobFull {
    pub job: JobRow,
    /// In the order they were committed.
    pub steps: Vec<StepRow>,
    /// By thread ID.
    pub threads: Vec<JobThread>,
    /// By question ID.
    pub questions: Vec<JobQuestion>,
}

/// A thread a job answers, or stopped at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobThread {
    pub thread_id: String,
    /// The newest comment of the thread the job answers.
    pub comment_id: String,
    pub stopped: bool,
}

/// A question a job asked. A question belongs to one job at most.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobQuestion {
    pub question_id: String,
    pub answer: JobAnswer,
}

/// What became of a question a job asked (`job_questions.outcome` and
/// `answers_json`). Serialised as `{"outcome": "pending"}`, or
/// `"answered"` with `answers`, `"declined"`, `"withdrawn"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "answers", rename_all = "lowercase")]
pub enum JobAnswer {
    /// The question is not closed.
    Pending,
    /// The person answered: the answers, as the question keeps them.
    Answered(serde_json::Value),
    Declined,
    /// Withdrawn: by the session that asked, or by Clax.
    Withdrawn,
}

impl JobAnswer {
    /// The `outcome` and `answers_json` columns that keep it.
    fn columns(&self) -> (Option<&'static str>, Option<String>) {
        match self {
            JobAnswer::Pending => (None, None),
            JobAnswer::Answered(a) => (Some("answered"), Some(text(a))),
            JobAnswer::Declined => (Some("declined"), None),
            JobAnswer::Withdrawn => (Some("withdrawn"), None),
        }
    }

    /// The outcome of a question whose `questions.status` is `status`,
    /// with `answers_json` `answers`: [`JobAnswer::Pending`] while it is
    /// open (or handed to the terminal, which only a hook question is).
    pub(crate) fn of(status: &str, answers: Option<String>) -> clax_core::Result<JobAnswer> {
        Ok(match (status, answers) {
            ("answered", Some(a)) => JobAnswer::Answered(json(a)?),
            ("declined", _) => JobAnswer::Declined,
            ("withdrawn", _) => JobAnswer::Withdrawn,
            ("open" | "released", _) => JobAnswer::Pending,
            (s, _) => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("no question status {s} with those answers").into(),
                )
                .into());
            }
        })
    }

    /// Read back from its columns.
    fn read(outcome: Option<String>, answers: Option<String>) -> rusqlite::Result<JobAnswer> {
        Ok(match (outcome.as_deref(), answers) {
            (None, None) => JobAnswer::Pending,
            (Some("answered"), Some(a)) => JobAnswer::Answered(json(a)?),
            (Some("declined"), None) => JobAnswer::Declined,
            (Some("withdrawn"), None) => JobAnswer::Withdrawn,
            (o, _) => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("no job question outcome {o:?} with those answers").into(),
                ));
            }
        })
    }
}

/// A job to create.
#[derive(Clone, Debug, PartialEq)]
pub struct NewJob {
    pub id: String,
    pub client: String,
    pub kind: String,
    pub artifact_id: Option<String>,
    pub title: Option<String>,
    pub input: serde_json::Value,
    pub plan: Option<serde_json::Value>,
}

/// A change to a job's row, applied when the job is at `if_rev`. A `None`
/// leaves a field as it is; `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobPatch {
    pub if_rev: i64,
    /// The state to move to, by [`may_move`]'s table.
    pub state: Option<JobState>,
    pub step: Option<Option<String>>,
    pub title: Option<String>,
    pub plan: Option<Option<serde_json::Value>>,
    pub carry: Option<Option<serde_json::Value>>,
    pub error: Option<Option<serde_json::Value>>,
}

/// A committed step of a job.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepRow {
    pub key: String,
    pub result: serde_json::Value,
    /// The version a publish step made.
    pub version_n: Option<i64>,
    pub created_at: String,
}

/// Why a write of a job was refused.
#[derive(Debug)]
pub enum JobError {
    /// The job is no longer at the `rev` the write named, or is in a final
    /// state ([`JobState::is_final`]), which no write changes; its row now.
    Moved(Box<JobRow>),
    /// A job of that ID exists with another body.
    Conflict,
    /// No job of that ID.
    NotFound,
    /// The store failed, or the write named something invalid
    /// ([`CoreError::Invalid`]: a move [`may_move`] refuses, an artifact
    /// or a thread).
    Store(CoreError),
}

impl From<CoreError> for JobError {
    fn from(e: CoreError) -> Self {
        JobError::Store(e)
    }
}

impl From<rusqlite::Error> for JobError {
    fn from(e: rusqlite::Error) -> Self {
        JobError::Store(e.into())
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Moved(j) => write!(f, "job {} moved to rev {}", j.id, j.rev),
            JobError::Conflict => f.write_str("a job of that ID exists with another body"),
            JobError::NotFound => f.write_str("no such job"),
            JobError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for JobError {}

type JobResult<T> = std::result::Result<T, JobError>;

const JOB_COLUMNS: &str =
    "id, client, kind, artifact_id, state, step, title, input_json, plan_json,
     carry_json, error_json, rev, created_at, updated_at";

/// JSON text read back as a value.
fn json(text: String) -> rusqlite::Result<serde_json::Value> {
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// A value as the JSON text a column keeps.
fn text(v: &serde_json::Value) -> String {
    v.to_string()
}

fn job_row(c: &Connection, id: &str) -> clax_core::Result<Option<JobRow>> {
    Ok(
        c.prepare_cached(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"))?
            .query_row(params![id], read_job)
            .optional()?,
    )
}

fn read_job(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    read_job_at(r, 0)
}

/// A job's row from [`JOB_COLUMNS`] starting at column `at`.
fn read_job_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<JobRow> {
    let json_at = |i: usize| -> rusqlite::Result<Option<serde_json::Value>> {
        r.get::<_, Option<String>>(at + i)?.map(json).transpose()
    };
    Ok(JobRow {
        id: r.get(at)?,
        client: r.get(at + 1)?,
        kind: r.get(at + 2)?,
        artifact_id: r.get(at + 3)?,
        state: r.get(at + 4)?,
        step: r.get(at + 5)?,
        title: r.get(at + 6)?,
        input: json(r.get(at + 7)?)?,
        plan: json_at(8)?,
        carry: json_at(9)?,
        error: json_at(10)?,
        rev: r.get(at + 11)?,
        created_at: r.get(at + 12)?,
        updated_at: r.get(at + 13)?,
    })
}

/// Job `id`'s row, or [`JobError::NotFound`].
fn existing(c: &Connection, id: &str) -> JobResult<JobRow> {
    job_row(c, id)?.ok_or(JobError::NotFound)
}

/// Whether job `id` holds a question it asked that is not closed.
fn waits(c: &Connection, id: &str) -> clax_core::Result<bool> {
    Ok(c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM job_questions WHERE job_id = ?1 AND outcome IS NULL)",
    )?
    .query_row(params![id], |r| r.get(0))?)
}

/// The state job `row` is in after `by` ([`may_move`]), read on `c` as
/// the transaction has it so far.
///
/// # Errors
/// [`JobError::Moved`] with `row` when it is final; [`CoreError::Invalid`]
/// `invalid_move` for another move the table refuses.
fn moving(c: &Connection, row: &JobRow, by: Move) -> JobResult<JobState> {
    if let Some(to) = may_move(row.state, by, waits(c, &row.id)?) {
        return Ok(to);
    }
    if row.state.is_final() {
        return Err(JobError::Moved(Box::new(row.clone())));
    }
    Err(CoreError::invalid(
        "invalid_move",
        format!("a job in {} cannot move by {by:?}", row.state.as_str()),
    )
    .into())
}

/// Job `id`'s row when a write may change it: [`JobError::NotFound`]
/// when there is none, [`JobError::Moved`] with it when it is final.
fn open_row(c: &Connection, id: &str) -> JobResult<JobRow> {
    let row = existing(c, id)?;
    if row.state.is_final() {
        return Err(JobError::Moved(Box::new(row)));
    }
    Ok(row)
}

/// Moves job `id` one `rev` on, at `now`, and answers its row after it.
fn bump(c: &Connection, id: &str, now: &str) -> JobResult<JobRow> {
    c.execute(
        "UPDATE jobs SET rev = rev + 1, updated_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    existing(c, id)
}

fn full(c: &Connection, job: JobRow) -> clax_core::Result<JobFull> {
    let steps = c
        .prepare_cached(
            "SELECT key, result_json, version_n, created_at FROM job_steps
             WHERE job_id = ?1 ORDER BY rowid",
        )?
        .query_map(params![job.id], read_step)?
        .collect::<rusqlite::Result<_>>()?;
    let threads = c
        .prepare_cached(
            "SELECT thread_id, comment_id, stopped FROM job_threads
             WHERE job_id = ?1 ORDER BY thread_id",
        )?
        .query_map(params![job.id], |r| {
            Ok(JobThread {
                thread_id: r.get(0)?,
                comment_id: r.get(1)?,
                stopped: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let questions = c
        .prepare_cached(
            "SELECT question_id, outcome, answers_json FROM job_questions
             WHERE job_id = ?1 ORDER BY question_id",
        )?
        .query_map(params![job.id], |r| {
            Ok(JobQuestion {
                question_id: r.get(0)?,
                answer: JobAnswer::read(r.get(1)?, r.get(2)?)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(JobFull {
        job,
        steps,
        threads,
        questions,
    })
}

fn read_step(r: &rusqlite::Row<'_>) -> rusqlite::Result<StepRow> {
    Ok(StepRow {
        key: r.get(0)?,
        result: json(r.get(1)?)?,
        version_n: r.get(2)?,
        created_at: r.get(3)?,
    })
}

fn step_row(c: &Connection, id: &str, key: &str) -> clax_core::Result<Option<StepRow>> {
    Ok(c.prepare_cached(
        "SELECT key, result_json, version_n, created_at FROM job_steps
         WHERE job_id = ?1 AND key = ?2",
    )?
    .query_row(params![id, key], read_step)
    .optional()?)
}

impl Store {
    /// Runs `f` in a write transaction ([`Store::with_tx`]), committed
    /// when `f` succeeds and rolled back on every error `f` answers, a
    /// refusal as much as a failure, which is then answered as `f` gave it.
    fn job_tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> JobResult<T>) -> JobResult<T> {
        let mut refused = None;
        let out = self.with_tx(|tx| {
            f(tx).map_err(|e| match e {
                JobError::Store(e) => e,
                other => {
                    refused = Some(other);
                    // Stands in for the refusal, kept above, to roll back.
                    CoreError::TaskFailed
                }
            })
        });
        match (out, refused) {
            (Ok(v), _) => Ok(v),
            (Err(_), Some(refusal)) => Err(refusal),
            (Err(e), None) => Err(JobError::Store(e)),
        }
    }

    /// Installs `f`, called after each committed write transaction that
    /// changed any job, with each changed job once, as the transaction
    /// left it, outside the write turn and after the other listeners. A
    /// rolled-back transaction is never heard of. Replaces any earlier
    /// listener.
    pub fn set_job_listener(&self, f: impl Fn(Vec<JobChange>) + Send + Sync + 'static) {
        *self.jobs.listener.write().unwrap() = Some(Arc::new(f));
    }

    /// Creates job `j` in `queued` at `rev` 1. Creating it again with the
    /// body its row holds (client, kind, artifact, title, input and plan)
    /// answers the row; with another body, [`JobError::Conflict`].
    ///
    /// # Errors
    /// [`JobError::Conflict`]; [`CoreError::Invalid`] `invalid_artifact`
    /// when `artifact_id` names no artifact that is not deleted.
    pub fn create_job(&self, j: NewJob) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            if let Some(row) = job_row(tx, &j.id)? {
                let same = (&row.client, &row.kind, &row.artifact_id, &row.title)
                    == (&j.client, &j.kind, &j.artifact_id, &j.title)
                    && row.input == j.input
                    && row.plan == j.plan;
                return if same {
                    Ok(row)
                } else {
                    Err(JobError::Conflict)
                };
            }
            if let Some(aid) = &j.artifact_id {
                let there: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL)",
                    params![aid],
                    |r| r.get(0),
                )?;
                if !there {
                    return Err(CoreError::invalid(
                        "invalid_artifact",
                        format!("no artifact {aid}"),
                    )
                    .into());
                }
            }
            let now = self.now();
            tx.execute(
                "INSERT INTO jobs (id, client, kind, artifact_id, state, title, input_json,
                                   plan_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?8, ?8)",
                params![
                    j.id,
                    j.client,
                    j.kind,
                    j.artifact_id,
                    j.title,
                    text(&j.input),
                    j.plan.as_ref().map(text),
                    now
                ],
            )?;
            existing(tx, &j.id)
        })
    }

    /// `client`'s jobs that are neither `cancelled` nor `done`, oldest
    /// first, each with its steps, threads and questions.
    ///
    /// # Errors
    /// Storage errors only.
    pub fn open_jobs(&self, client: &str) -> clax_core::Result<Vec<JobFull>> {
        self.with_read(|c| {
            let jobs: Vec<JobRow> = c
                .prepare_cached(&format!(
                    "SELECT {JOB_COLUMNS} FROM jobs
                     WHERE state NOT IN ('cancelled', 'done') AND client = ?1
                     ORDER BY created_at, rowid"
                ))?
                .query_map(params![client], read_job)?
                .collect::<rusqlite::Result<_>>()?;
            jobs.into_iter().map(|j| full(c, j)).collect()
        })
    }

    /// Job `id` with its steps, threads and questions; `None` when there is
    /// none.
    ///
    /// # Errors
    /// Storage errors only.
    pub fn job(&self, id: &str) -> clax_core::Result<Option<JobFull>> {
        self.with_read(|c| job_row(c, id)?.map(|j| full(c, j)).transpose())
    }

    /// Applies `p` to job `id` when it is at `p.if_rev`, its state by
    /// [`may_move`]'s table: a move to `running` lands in `waiting` while
    /// the job holds a question that is not closed.
    ///
    /// # Errors
    /// [`JobError::Moved`] with the current row when it is not at
    /// `p.if_rev` or is final; [`JobError::NotFound`];
    /// [`CoreError::Invalid`] `invalid_move` for a move to `queued`,
    /// `waiting` or `done`.
    pub fn patch_job(&self, id: &str, p: JobPatch) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
            if row.rev != p.if_rev {
                return Err(JobError::Moved(Box::new(row)));
            }
            let state = moving(tx, &row, Move::To(p.state.unwrap_or(row.state)))?;
            let step = p.step.unwrap_or(row.step);
            let title = p.title.or(row.title);
            let plan = p.plan.unwrap_or(row.plan);
            let carry = p.carry.unwrap_or(row.carry);
            let error = p.error.unwrap_or(row.error);
            tx.execute(
                "UPDATE jobs SET state = ?2, step = ?3, title = ?4, plan_json = ?5,
                                 carry_json = ?6, error_json = ?7
                 WHERE id = ?1",
                params![
                    id,
                    state,
                    step,
                    title,
                    plan.as_ref().map(text),
                    carry.as_ref().map(text),
                    error.as_ref().map(text)
                ],
            )?;
            if state.is_final() {
                self.ending(tx, id)?;
            }
            bump(tx, id, &self.now())
        })
    }

    /// Records that job `id` answers thread `thread` up to comment
    /// `comment`, or stopped at it (`stopped`), replacing what it recorded
    /// for that thread before.
    ///
    /// # Errors
    /// [`JobError::NotFound`]; [`JobError::Moved`] when the job is final;
    /// [`CoreError::Invalid`] `invalid_thread` when `thread` names no
    /// thread.
    pub fn set_job_thread(
        &self,
        id: &str,
        thread: &str,
        comment: &str,
        stopped: bool,
    ) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            open_row(tx, id)?;
            let there: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM threads WHERE id = ?1)",
                params![thread],
                |r| r.get(0),
            )?;
            if !there {
                return Err(
                    CoreError::invalid("invalid_thread", format!("no thread {thread}")).into(),
                );
            }
            tx.execute(
                "INSERT INTO job_threads (job_id, thread_id, comment_id, stopped)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (job_id, thread_id)
                 DO UPDATE SET comment_id = excluded.comment_id, stopped = excluded.stopped",
                params![id, thread, comment, stopped],
            )?;
            bump(tx, id, &self.now())
        })
    }

    /// Forgets thread `thread` of job `id`. The row is unchanged when the
    /// job recorded nothing for it.
    ///
    /// # Errors
    /// [`JobError::NotFound`]; [`JobError::Moved`] when the job is final.
    pub fn remove_job_thread(&self, id: &str, thread: &str) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            let row = open_row(tx, id)?;
            let n = tx.execute(
                "DELETE FROM job_threads WHERE job_id = ?1 AND thread_id = ?2",
                params![id, thread],
            )?;
            if n == 0 {
                return Ok(row);
            }
            bump(tx, id, &self.now())
        })
    }

    /// Records a question the job asked and, when it is `running` and the
    /// question is not closed, moves it to `waiting` ([`may_move`]), in
    /// one transaction that reads the question: one already closed has
    /// its outcome stored at once, and the job keeps its state. Recording
    /// a question the job already asked changes nothing.
    ///
    /// # Errors
    /// [`JobError::NotFound`]; [`JobError::Moved`] when the job is final;
    /// [`CoreError::Invalid`] `invalid_question` when `question` names no
    /// question of the `ask` tool (a mirrored AskUserQuestion is the
    /// hook's, withdrawn in bulk without its close), or one another job
    /// asked.
    pub fn ask_job_question(&self, id: &str, question: &str) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
            let asked: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM job_questions WHERE job_id = ?1 AND question_id = ?2)",
                params![id, question],
                |r| r.get(0),
            )?;
            if asked {
                return Ok(row);
            }
            // The question as this transaction sees it: a close that
            // landed before this call is stored now, not waited for.
            let asked: Option<(String, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT q.status, q.answers_json,
                            (SELECT job_id FROM job_questions WHERE question_id = q.id)
                     FROM questions q WHERE q.id = ?1 AND q.source = 'ask'",
                    params![question],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let outcome = match asked {
                Some((status, answers, None)) => JobAnswer::of(&status, answers)?,
                Some((_, _, Some(job))) => {
                    return Err(CoreError::invalid(
                        "invalid_question",
                        format!("question {question} is job {job}'s"),
                    )
                    .into());
                }
                None => {
                    return Err(CoreError::invalid(
                        "invalid_question",
                        format!("no question {question} of the ask tool"),
                    )
                    .into());
                }
            };
            let (outcome, answers) = outcome.columns();
            tx.execute(
                "INSERT INTO job_questions (job_id, question_id, outcome, answers_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, question, outcome, answers],
            )?;
            let state = moving(tx, &row, Move::Ask)?;
            tx.execute(
                "UPDATE jobs SET state = ?2 WHERE id = ?1",
                params![id, state],
            )?;
            bump(tx, id, &self.now())
        })
    }

    /// Commits step `key` of job `id` with `result`, setting the job's
    /// `step` to it, and its state to `done` when `done`, in one
    /// transaction. A key the job already has returns its step as recorded
    /// with `replayed = true`, and changes nothing, whatever the job's
    /// state.
    ///
    /// # Errors
    /// [`JobError::NotFound`]; [`JobError::Moved`] when the job is final.
    pub fn commit_step(
        &self,
        id: &str,
        key: &str,
        result: serde_json::Value,
        done: bool,
    ) -> JobResult<(StepRow, JobRow, bool)> {
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
            if let Some(step) = step_row(tx, id, key)? {
                return Ok((step, row, true));
            }
            let state = moving(tx, &row, Move::Step { done })?;
            let now = self.now();
            tx.execute(
                "INSERT INTO job_steps (job_id, key, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, key, text(&result), now],
            )?;
            tx.execute(
                "UPDATE jobs SET step = ?2, state = ?3 WHERE id = ?1",
                params![id, key, state],
            )?;
            if state.is_final() {
                self.ending(tx, id)?;
            }
            let row = bump(tx, id, &now)?;
            let step = step_row(tx, id, key)?.ok_or(JobError::NotFound)?;
            Ok((step, row, false))
        })
    }

    /// The agent's remembered read consents, as `(reader, source)`, in
    /// order.
    ///
    /// # Errors
    /// Storage errors only.
    pub fn agent_read_consents(&self) -> clax_core::Result<Vec<(String, String)>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT reader, source FROM agent_read_consents ORDER BY reader, source",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Remembers that the agent may read `source` for `reader`; keeps the
    /// first time when it already does.
    ///
    /// # Errors
    /// Storage errors only.
    pub fn put_agent_read_consent(&self, reader: &str, source: &str) -> clax_core::Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO agent_read_consents (reader, source, created_at)
                 VALUES (?1, ?2, ?3)",
                params![reader, source, self.now()],
            )?;
            Ok(())
        })
    }

    /// Forgets the consent of `reader` to `source`; whether there was one.
    ///
    /// # Errors
    /// Storage errors only.
    pub fn delete_agent_read_consent(&self, reader: &str, source: &str) -> clax_core::Result<bool> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "DELETE FROM agent_read_consents WHERE reader = ?1 AND source = ?2",
                params![reader, source],
            )?;
            Ok(n > 0)
        })
    }

    /// Withdraws each question job `id` asked that is not closed, inside
    /// the caller's transaction, which has just moved the job to a final
    /// state (`cancelled` or `done`): no worker reads its outcome again, so
    /// the person is no longer asked. Each goes by
    /// [`Store::withdraw_question_in`] (reason `job_end`, as
    /// `system:daemon`), which stores its outcome and moves the job no
    /// further. Every final move calls it: a patch to `cancelled`
    /// ([`Store::patch_job`]), a `done` step ([`Store::commit_step`]) and an
    /// artifact's delete ([`Store::artifact_going_tx`]).
    fn ending(&self, tx: &Transaction<'_>, id: &str) -> clax_core::Result<()> {
        let open: Vec<String> = tx
            .prepare_cached(
                "SELECT question_id FROM job_questions
                 WHERE job_id = ?1 AND outcome IS NULL ORDER BY question_id",
            )?
            .query_map(params![id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for q in open {
            self.withdraw_question_in(tx, &AuditCtx::DAEMON, &q, Reason::JobEnd)?;
        }
        Ok(())
    }

    /// Stores `outcome`, how question `question` closed, in the job that
    /// asked it, and by [`may_move`] moves that job from `waiting` back to
    /// `running` once it waits on no other question, inside the caller's
    /// transaction; the job's row after it, or `None` when no job asked it,
    /// its outcome is already stored, or the job is final (which only
    /// [`Store::ending`] closes a question of, the outcome stored and the
    /// job left as it is). Called by
    /// `Store::close_in`, the one write that closes a question, at every
    /// close that ends it: an answer, a decline or a withdrawal.
    pub(crate) fn close_job_question_tx(
        &self,
        tx: &Transaction<'_>,
        question: &str,
        outcome: &JobAnswer,
    ) -> clax_core::Result<Option<JobRow>> {
        let id: Option<String> = tx
            .query_row(
                "SELECT job_id FROM job_questions WHERE question_id = ?1 AND outcome IS NULL",
                params![question],
                |r| r.get(0),
            )
            .optional()?;
        let Some(row) = id.map(|id| job_row(tx, &id)).transpose()?.flatten() else {
            return Ok(None);
        };
        let (outcome, answers) = outcome.columns();
        tx.execute(
            "UPDATE job_questions SET outcome = ?2, answers_json = ?3 WHERE question_id = ?1",
            params![question, outcome, answers],
        )?;
        if row.state.is_final() {
            return Ok(None);
        }
        let moved = moving(tx, &row, Move::Close).and_then(|state| {
            tx.execute(
                "UPDATE jobs SET state = ?2 WHERE id = ?1",
                params![row.id, state],
            )?;
            bump(tx, &row.id, &self.now())
        });
        match moved {
            Ok(row) => Ok(Some(row)),
            Err(JobError::Store(e)) => Err(e),
            // No move refuses a job that is not final.
            Err(_) => Ok(None),
        }
    }
}

impl Store {
    /// Cancels the open jobs that name artifact `aid`, inside the caller's
    /// transaction, which is deleting it: each moves to `cancelled`
    /// ([`may_move`]) with the error `{"code": "artifact_deleted"}` and
    /// its `rev` one on, so a worker holding it hears of it (and so does
    /// the job listener, even when the transaction then deletes the job's
    /// row). Called by every delete of an artifact, before it.
    pub(crate) fn artifact_going_tx(
        &self,
        tx: &Transaction<'_>,
        aid: &str,
    ) -> clax_core::Result<()> {
        let open: Vec<(i64, JobRow)> = tx
            .prepare_cached(&format!(
                "SELECT rowid, {JOB_COLUMNS} FROM jobs
                 WHERE artifact_id = ?1 AND state NOT IN ('cancelled', 'done')"
            ))?
            .query_map(params![aid], |r| {
                let row = read_job_at(r, 1)?;
                Ok((r.get(0)?, row))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let now = self.now();
        for (rowid, row) in open {
            let Some(state) = may_move(row.state, Move::To(JobState::Cancelled), false) else {
                continue;
            };
            tx.execute(
                "UPDATE jobs SET state = ?2, error_json = ?3, rev = rev + 1, updated_at = ?4
                 WHERE id = ?1",
                params![row.id, state, r#"{"code":"artifact_deleted"}"#, now],
            )?;
            self.ending(tx, &row.id)?;
            self.jobs.going(
                rowid,
                JobChange {
                    id: row.id,
                    client: row.client,
                    state,
                    step: row.step,
                    rev: row.rev + 1,
                },
            );
        }
        Ok(())
    }

    /// Forgets thread `tid` in every job that answers it, inside the
    /// caller's transaction, which is deleting it: each such job's `rev`
    /// moves one on. Called by every delete of a thread, before it.
    pub(crate) fn thread_going_tx(&self, tx: &Transaction<'_>, tid: &str) -> clax_core::Result<()> {
        self.threads_going(tx, "thread_id = ?1", tid)
    }

    /// [`Store::thread_going_tx`] for every thread of artifact `aid`.
    pub(crate) fn artifact_threads_going_tx(
        &self,
        tx: &Transaction<'_>,
        aid: &str,
    ) -> clax_core::Result<()> {
        self.threads_going(
            tx,
            "thread_id IN (SELECT id FROM threads WHERE artifact_id = ?1)",
            aid,
        )
    }

    /// The `job_threads` rows `which` selects (with `?1` bound to `key`)
    /// go, and each job that had one moves one `rev` on.
    fn threads_going(&self, tx: &Transaction<'_>, which: &str, key: &str) -> clax_core::Result<()> {
        tx.execute(
            &format!(
                "UPDATE jobs SET rev = rev + 1, updated_at = ?2
                 WHERE id IN (SELECT job_id FROM job_threads WHERE {which})"
            ),
            params![key, self.now()],
        )?;
        tx.execute(
            &format!("DELETE FROM job_threads WHERE {which}"),
            params![key],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::{Close, NewQuestion, Source};
    use crate::test_util::{DAEMON, artifact, session, store};
    use clax_core::questions::Answer;
    use serde_json::json;

    fn new_job(id: &str) -> NewJob {
        NewJob {
            id: id.into(),
            client: "app".into(),
            kind: "make".into(),
            artifact_id: None,
            title: Some("Budget".into()),
            input: json!({"request": "a budget"}),
            plan: Some(json!({"steps": ["draft"]})),
        }
    }

    fn patch(rev: i64) -> JobPatch {
        JobPatch {
            if_rev: rev,
            ..JobPatch::default()
        }
    }

    /// Creates job `id` and moves it to `running`, at `rev` 2.
    fn running(st: &Store, id: &str) -> JobRow {
        st.create_job(new_job(id)).unwrap();
        st.patch_job(
            id,
            JobPatch {
                state: Some(JobState::Running),
                ..patch(1)
            },
        )
        .unwrap()
    }

    /// Patches job `id`'s state to `to` at its current `rev`.
    fn move_to(st: &Store, id: &str, to: JobState) -> JobResult<JobRow> {
        let rev = st.job(id).unwrap().unwrap().job.rev;
        st.patch_job(
            id,
            JobPatch {
                state: Some(to),
                ..patch(rev)
            },
        )
    }

    fn thread_on(st: &Store, aid: &clax_core::ArtifactId) -> String {
        st.create_thread(
            DAEMON,
            aid,
            crate::threads::NewThread {
                author_public_id: None,
                version_n: 1,
                anchor: crate::test_util::anchor(),
                body: "make it green".into(),
                author_name: "Alex".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn question(st: &Store) -> String {
        question_from(st, Source::Ask)
    }

    fn question_from(st: &Store, source: Source) -> String {
        question_in(st, "h1", source)
    }

    /// A question asked by the session with harness session ID `hsid`.
    fn question_in(st: &Store, hsid: &str, source: Source) -> String {
        let sid = session(st, "app", hsid);
        let qs = serde_json::from_value(json!([{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]))
        .unwrap();
        st.create_question(
            DAEMON,
            NewQuestion {
                session_id: sid,
                artifact_id: None,
                source,
                tool_use_id: (source == Source::Hook).then(|| "toolu_1".into()),
                questions: qs,
                released: false,
                thread_id: None,
                in_terminal: false,
            },
        )
        .unwrap()
        .0
        .id
    }

    #[test]
    fn creating_a_job_twice_with_one_body_returns_it() {
        let (_d, st) = store();
        let a = st.create_job(new_job("j1")).unwrap();
        assert_eq!(
            (a.state, a.rev, a.step.as_deref()),
            (JobState::Queued, 1, None)
        );
        assert_eq!(a.input, json!({"request": "a budget"}));
        let b = st.create_job(new_job("j1")).unwrap();
        assert_eq!(a, b);
        assert_eq!(st.open_jobs("app").unwrap().len(), 1);
    }

    #[test]
    fn creating_with_another_body_conflicts() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let other = NewJob {
            input: json!({"request": "a calendar"}),
            ..new_job("j1")
        };
        assert!(matches!(st.create_job(other), Err(JobError::Conflict)));
        let other = NewJob {
            kind: "change".into(),
            ..new_job("j1")
        };
        assert!(matches!(st.create_job(other), Err(JobError::Conflict)));
        assert_eq!(
            st.job("j1").unwrap().unwrap().job.input,
            json!({"request": "a budget"})
        );
    }

    #[test]
    fn a_job_names_a_real_artifact() {
        let (_d, st) = store();
        let missing = NewJob {
            artifact_id: Some("7q3k9mzx2b4t".into()),
            ..new_job("j1")
        };
        assert!(matches!(
            st.create_job(missing),
            Err(JobError::Store(CoreError::Invalid {
                code: "invalid_artifact",
                ..
            }))
        ));
        let aid = artifact(&st, None);
        let change = NewJob {
            kind: "change".into(),
            artifact_id: Some(aid.as_str().into()),
            ..new_job("j1")
        };
        let j = st.create_job(change).unwrap();
        assert_eq!(j.artifact_id.as_deref(), Some(aid.as_str()));
    }

    #[test]
    fn a_stale_rev_is_refused_with_the_current_job() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let running = st
            .patch_job(
                "j1",
                JobPatch {
                    state: Some(JobState::Running),
                    ..patch(1)
                },
            )
            .unwrap();
        assert_eq!((running.state, running.rev), (JobState::Running, 2));
        let stale = st.patch_job(
            "j1",
            JobPatch {
                state: Some(JobState::Paused),
                ..patch(1)
            },
        );
        let Err(JobError::Moved(now)) = stale else {
            panic!("expected Moved, got {stale:?}")
        };
        assert_eq!(*now, running);
        assert_eq!(st.job("j1").unwrap().unwrap().job, running);
        assert!(matches!(
            st.patch_job("nope", patch(1)),
            Err(JobError::NotFound)
        ));
        assert!(serde_json::from_value::<JobState>(json!("sleeping")).is_err());
        assert_eq!(
            serde_json::to_value(JobState::Cancelled).unwrap(),
            json!("cancelled")
        );
    }

    #[test]
    fn a_patch_sets_and_clears_fields() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let j = st
            .patch_job(
                "j1",
                JobPatch {
                    step: Some(Some("turn:1".into())),
                    title: Some("Monthly budget".into()),
                    plan: Some(None),
                    carry: Some(Some(json!({"notes": "half"}))),
                    error: Some(Some(json!({"code": "x"}))),
                    ..patch(1)
                },
            )
            .unwrap();
        assert_eq!(j.step.as_deref(), Some("turn:1"));
        assert_eq!(j.title.as_deref(), Some("Monthly budget"));
        assert_eq!(j.plan, None);
        assert_eq!(j.carry, Some(json!({"notes": "half"})));
        assert_eq!(j.error, Some(json!({"code": "x"})));
        let j = st
            .patch_job(
                "j1",
                JobPatch {
                    error: Some(None),
                    ..patch(2)
                },
            )
            .unwrap();
        assert_eq!(
            (j.error, j.carry, j.rev),
            (None, Some(json!({"notes": "half"})), 3)
        );
    }

    #[test]
    fn every_write_answers_the_row_after_it() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        let q = question(&st);
        let mut revs = vec![st.create_job(new_job("j1")).unwrap().rev];
        let mut wrote = |row: JobRow| {
            assert_eq!(st.job("j1").unwrap().unwrap().job, row, "the row after it");
            revs.push(row.rev);
        };
        wrote(
            st.patch_job(
                "j1",
                JobPatch {
                    state: Some(JobState::Running),
                    ..patch(1)
                },
            )
            .unwrap(),
        );
        wrote(st.set_job_thread("j1", &t, "c1", false).unwrap());
        wrote(st.set_job_thread("j1", &t, "c2", true).unwrap());
        wrote(st.ask_job_question("j1", &q).unwrap());
        wrote(
            st.commit_step("j1", "turn:1", json!({"calls": []}), false)
                .unwrap()
                .1,
        );
        wrote(st.remove_job_thread("j1", &t).unwrap());
        assert_eq!(revs, (1..=7).collect::<Vec<i64>>());
    }

    #[test]
    fn a_thread_is_recorded_replaced_and_removed() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        st.create_job(new_job("j1")).unwrap();
        st.set_job_thread("j1", &t, "c1", false).unwrap();
        st.set_job_thread("j1", &t, "c2", true).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(
            full.threads,
            vec![JobThread {
                thread_id: t.clone(),
                comment_id: "c2".into(),
                stopped: true
            }]
        );
        let removed = st.remove_job_thread("j1", &t).unwrap();
        assert!(st.job("j1").unwrap().unwrap().threads.is_empty());
        // Removing it again changes nothing.
        assert_eq!(st.remove_job_thread("j1", &t).unwrap(), removed);
        assert!(matches!(
            st.set_job_thread("j1", "tmissing", "c1", false),
            Err(JobError::Store(CoreError::Invalid {
                code: "invalid_thread",
                ..
            }))
        ));
        assert!(matches!(
            st.set_job_thread("nope", &t, "c1", false),
            Err(JobError::NotFound)
        ));
    }

    fn answer(st: &Store, q: &str) {
        st.close_question(
            DAEMON,
            q,
            Close::Answer {
                answers: vec![Answer {
                    selected: vec!["A".into()],
                    text: None,
                }],
                via: "shell",
            },
        )
        .unwrap();
    }

    #[test]
    fn asking_moves_the_job_to_waiting_and_the_answer_back_to_running() {
        let (_d, st) = store();
        let q = question(&st);
        st.create_job(new_job("j1")).unwrap();
        st.patch_job(
            "j1",
            JobPatch {
                state: Some(JobState::Running),
                ..patch(1)
            },
        )
        .unwrap();
        let asked = st.ask_job_question("j1", &q).unwrap();
        assert_eq!((asked.state, asked.rev), (JobState::Waiting, 3));
        // Asking it again changes nothing.
        assert_eq!(st.ask_job_question("j1", &q).unwrap(), asked);
        assert_eq!(
            st.job("j1").unwrap().unwrap().questions,
            vec![JobQuestion {
                question_id: q.clone(),
                answer: JobAnswer::Pending
            }]
        );
        answer(&st, &q);
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!((full.job.state, full.job.rev), (JobState::Running, 4));
        let JobAnswer::Answered(stored) = &full.questions[0].answer else {
            panic!("expected answers, got {:?}", full.questions[0].answer)
        };
        assert_eq!(stored[0]["selected"], json!(["A"]));
        assert_eq!(
            serde_json::to_value(&full.questions[0].answer).unwrap()["outcome"],
            json!("answered")
        );
    }

    #[test]
    fn a_declined_question_moves_the_job_back_too() {
        let (_d, st) = store();
        let q = question(&st);
        running(&st, "j1");
        st.ask_job_question("j1", &q).unwrap();
        st.close_question(DAEMON, &q, Close::Decline).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!((full.job.state, full.job.rev), (JobState::Running, 4));
        assert_eq!(full.questions[0].answer, JobAnswer::Declined);
        assert_eq!(
            serde_json::to_value(&full.questions[0].answer).unwrap(),
            json!({"outcome": "declined"})
        );
    }

    #[test]
    fn an_answer_moves_only_a_waiting_job_and_only_once_it_waits_on_nothing() {
        let (_d, st) = store();
        let (q1, q2) = (question(&st), question(&st));
        running(&st, "j1");
        st.ask_job_question("j1", &q1).unwrap();
        st.ask_job_question("j1", &q2).unwrap();
        answer(&st, &q1);
        let j = st.job("j1").unwrap().unwrap().job;
        assert_eq!(
            (j.state, j.rev),
            (JobState::Waiting, 5),
            "still waits on q2"
        );
        let paused = st
            .patch_job(
                "j1",
                JobPatch {
                    state: Some(JobState::Paused),
                    ..patch(5)
                },
            )
            .unwrap();
        answer(&st, &q2);
        let j = st.job("j1").unwrap().unwrap().job;
        assert_eq!((j.state, j.rev), (JobState::Paused, paused.rev + 1));
        // A question no job asked moves nothing.
        let q3 = question(&st);
        answer(&st, &q3);
        assert_eq!(st.job("j1").unwrap().unwrap().job, j);
    }

    #[test]
    fn a_step_committed_twice_is_replayed() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let (step, row, replayed) = st
            .commit_step("j1", "turn:1", json!({"calls": [1]}), false)
            .unwrap();
        assert!(!replayed);
        assert_eq!((step.key.as_str(), step.version_n), ("turn:1", None));
        assert_eq!((row.step.as_deref(), row.rev), (Some("turn:1"), 2));
        let (again, row2, replayed) = st
            .commit_step("j1", "turn:1", json!({"calls": [2]}), true)
            .unwrap();
        assert!(replayed);
        assert_eq!(again, step, "the step as recorded");
        assert_eq!(row2, row, "nothing changed");
        assert_eq!(st.job("j1").unwrap().unwrap().steps, vec![step]);
        assert!(matches!(
            st.commit_step("nope", "turn:1", json!({}), false),
            Err(JobError::NotFound)
        ));
    }

    #[test]
    fn a_done_step_ends_the_job_in_its_transaction() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let (_, row, _) = st
            .commit_step("j1", "reply:0", json!({"replied": true}), true)
            .unwrap();
        assert_eq!(
            (row.state, row.step.as_deref()),
            (JobState::Done, Some("reply:0"))
        );
        assert!(st.open_jobs("app").unwrap().is_empty());
        assert_eq!(st.job("j1").unwrap().unwrap().job, row);
    }

    #[test]
    fn open_jobs_lists_steps_threads_and_questions() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        let q = question(&st);
        st.create_job(new_job("j1")).unwrap();
        st.create_job(new_job("j2")).unwrap();
        st.create_job(NewJob {
            client: "cli".into(),
            ..new_job("j3")
        })
        .unwrap();
        st.create_job(new_job("j4")).unwrap();
        st.patch_job(
            "j4",
            JobPatch {
                state: Some(JobState::Cancelled),
                ..patch(1)
            },
        )
        .unwrap();
        move_to(&st, "j1", JobState::Running).unwrap();
        st.commit_step("j1", "turn:1", json!({"n": 1}), false)
            .unwrap();
        st.commit_step("j1", "turn:2", json!({"n": 2}), false)
            .unwrap();
        st.set_job_thread("j1", &t, "c1", false).unwrap();
        st.ask_job_question("j1", &q).unwrap();
        let open = st.open_jobs("app").unwrap();
        let ids: Vec<&str> = open.iter().map(|f| f.job.id.as_str()).collect();
        assert_eq!(ids, ["j1", "j2"]);
        let j1 = &open[0];
        let keys: Vec<&str> = j1.steps.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["turn:1", "turn:2"]);
        assert_eq!(j1.steps[1].result, json!({"n": 2}));
        assert_eq!(j1.threads.len(), 1);
        assert_eq!(j1.questions.len(), 1);
        assert_eq!(j1.job.state, JobState::Waiting);
        assert!(open[1].steps.is_empty() && open[1].threads.is_empty());
        assert_eq!(st.open_jobs("cli").unwrap().len(), 1);
        assert!(st.job("nope").unwrap().is_none());
    }

    /// Every write of a job, applied to job `id` at `rev`.
    fn every_write(st: &Store, id: &str, rev: i64, t: &str, q: &str) -> Vec<JobResult<JobRow>> {
        vec![
            st.patch_job(
                id,
                JobPatch {
                    state: Some(JobState::Running),
                    ..patch(rev)
                },
            ),
            st.patch_job(
                id,
                JobPatch {
                    title: Some("again".into()),
                    ..patch(rev)
                },
            ),
            st.set_job_thread(id, t, "c1", false),
            st.remove_job_thread(id, t),
            st.ask_job_question(id, q),
            st.commit_step(id, "turn:9", json!({}), false)
                .map(|(_, row, _)| row),
            st.commit_step(id, "turn:9", json!({}), true)
                .map(|(_, row, _)| row),
        ]
    }

    #[test]
    fn done_and_cancelled_jobs_refuse_every_write() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        let q = question(&st);
        running(&st, "j1");
        st.set_job_thread("j1", &t, "c0", false).unwrap();
        let (_, done, _) = st.commit_step("j1", "publish:1", json!({}), true).unwrap();
        running(&st, "j2");
        st.set_job_thread("j2", &t, "c0", false).unwrap();
        let cancelled = move_to(&st, "j2", JobState::Cancelled).unwrap();
        for row in [done, cancelled] {
            let before = st.job(&row.id).unwrap().unwrap();
            for (i, refused) in every_write(&st, &row.id, row.rev, &t, &q)
                .into_iter()
                .enumerate()
            {
                let Err(JobError::Moved(now)) = refused else {
                    panic!("{} write {i}: expected Moved, got {refused:?}", row.id)
                };
                assert_eq!(*now, row, "{} write {i}", row.id);
            }
            assert_eq!(st.job(&row.id).unwrap().unwrap(), before, "{}", row.id);
        }
        // The step that ended a job is still replayed.
        let (_, again, replayed) = st.commit_step("j1", "publish:1", json!({}), true).unwrap();
        assert!(replayed);
        assert_eq!(again.state, JobState::Done);
    }

    #[test]
    fn a_patch_never_moves_a_job_to_queued_waiting_or_done() {
        let (_d, st) = store();
        let before = running(&st, "j1");
        for to in [JobState::Queued, JobState::Waiting, JobState::Done] {
            let refused = move_to(&st, "j1", to);
            assert!(
                matches!(
                    refused,
                    Err(JobError::Store(CoreError::Invalid {
                        code: "invalid_move",
                        ..
                    }))
                ),
                "{to:?}: {refused:?}"
            );
            assert_eq!(st.job("j1").unwrap().unwrap().job, before, "{to:?}");
        }
        // A patch naming the state the job is in moves nothing.
        assert_eq!(
            move_to(&st, "j1", JobState::Running).unwrap().state,
            JobState::Running
        );
        st.create_job(new_job("j2")).unwrap();
        assert_eq!(
            move_to(&st, "j2", JobState::Queued).unwrap().state,
            JobState::Queued
        );
    }

    #[test]
    fn only_a_running_job_that_asks_moves_to_waiting() {
        let (_d, st) = store();
        let (q1, q2) = (question(&st), question(&st));
        st.create_job(new_job("j1")).unwrap();
        let asked = st.ask_job_question("j1", &q1).unwrap();
        assert_eq!((asked.state, asked.rev), (JobState::Queued, 2));
        // Started with a question open, it waits.
        let started = move_to(&st, "j1", JobState::Running).unwrap();
        assert_eq!(started.state, JobState::Waiting);
        answer(&st, &q1);
        assert_eq!(st.job("j1").unwrap().unwrap().job.state, JobState::Running);
        let paused = move_to(&st, "j1", JobState::Paused).unwrap();
        let asked = st.ask_job_question("j1", &q2).unwrap();
        assert_eq!((asked.state, asked.rev), (JobState::Paused, paused.rev + 1));
    }

    #[test]
    fn a_job_resumed_while_its_question_is_open_waits() {
        let (_d, st) = store();
        let q = question(&st);
        running(&st, "j1");
        st.ask_job_question("j1", &q).unwrap();
        // Running again while it waits keeps it waiting.
        let still = move_to(&st, "j1", JobState::Running).unwrap();
        assert_eq!(still.state, JobState::Waiting);
        let paused = move_to(&st, "j1", JobState::Paused).unwrap();
        assert_eq!(paused.state, JobState::Paused);
        let resumed = move_to(&st, "j1", JobState::Running).unwrap();
        assert_eq!(resumed.state, JobState::Waiting);
        answer(&st, &q);
        let j = st.job("j1").unwrap().unwrap().job;
        assert_eq!((j.state, j.rev), (JobState::Running, resumed.rev + 1));
        // A step on a paused job keeps it paused.
        let paused = move_to(&st, "j1", JobState::Paused).unwrap();
        let (_, row, _) = st.commit_step("j1", "turn:1", json!({}), false).unwrap();
        assert_eq!((row.state, row.rev), (JobState::Paused, paused.rev + 1));
    }

    #[test]
    fn a_withdrawn_question_moves_the_job_back_with_its_outcome() {
        let (_d, st) = store();
        let q = question(&st);
        running(&st, "j1");
        st.ask_job_question("j1", &q).unwrap();
        st.close_question(DAEMON, &q, Close::Withdraw).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!((full.job.state, full.job.rev), (JobState::Running, 4));
        assert_eq!(full.questions[0].answer, JobAnswer::Withdrawn);
    }

    /// The session that asked question `q`.
    fn asker(st: &Store, q: &str) -> String {
        st.question(q).unwrap().unwrap().session_id
    }

    #[test]
    fn a_sessions_end_withdraws_its_question_and_moves_the_job_back() {
        let (_d, st) = store();
        let q1 = question_in(&st, "h1", Source::Ask);
        let q2 = question_in(&st, "h2", Source::Ask);
        running(&st, "j1");
        let waiting = st.ask_job_question("j1", &q1).unwrap();
        st.end_session(DAEMON, &asker(&st, &q1)).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(
            (full.job.state, full.job.rev),
            (JobState::Running, waiting.rev + 1)
        );
        assert_eq!(full.questions[0].answer, JobAnswer::Withdrawn);
        // The reaper's end does the same.
        let waiting = st.ask_job_question("j1", &q2).unwrap();
        assert_eq!(waiting.state, JobState::Waiting);
        let s2 = asker(&st, &q2);
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
                [&s2],
            )?)
        })
        .unwrap();
        let reaped = st
            .reap_sessions(std::time::Duration::from_secs(60), &|_| false)
            .unwrap();
        assert_eq!(reaped.withdrawn_questions, vec![q2.clone()]);
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(
            (full.job.state, full.job.rev),
            (JobState::Running, waiting.rev + 1)
        );
        assert_eq!(full.questions[1].answer, JobAnswer::Withdrawn);
    }

    /// How many questions are open in the owner's inbox.
    fn open_questions(st: &Store) -> u32 {
        st.list_questions(None, crate::questions::ListStatus::Open, 50)
            .unwrap()
            .1
    }

    #[test]
    fn a_jobs_final_move_withdraws_its_open_questions() {
        let (_d, st) = store();
        // Cancelled by a patch.
        let q1 = question(&st);
        running(&st, "j1");
        st.ask_job_question("j1", &q1).unwrap();
        assert_eq!(open_questions(&st), 1);
        let cancelled = move_to(&st, "j1", JobState::Cancelled).unwrap();
        assert_eq!(open_questions(&st), 0, "a cancel withdraws");
        // Done by its last step.
        let q2 = question(&st);
        running(&st, "j2");
        st.ask_job_question("j2", &q2).unwrap();
        let (_, done, _) = st.commit_step("j2", "reply:0", json!({}), true).unwrap();
        assert_eq!(open_questions(&st), 0, "a done step withdraws");
        // Cancelled by its artifact's delete.
        let aid = artifact(&st, None);
        change_of(&st, "j3", aid.as_str());
        let q3 = question(&st);
        st.ask_job_question("j3", &q3).unwrap();
        assert_eq!(open_questions(&st), 1);
        st.delete_artifact(DAEMON, &aid).unwrap();
        assert_eq!(open_questions(&st), 0, "an artifact's delete withdraws");
        for (j, q) in [("j1", &q1), ("j2", &q2), ("j3", &q3)] {
            assert_eq!(
                st.question(q).unwrap().unwrap().status,
                crate::questions::Status::Withdrawn,
                "{j}"
            );
            let full = st.job(j).unwrap().unwrap();
            assert!(full.job.state.is_final(), "{j}");
            assert_eq!(full.questions[0].answer, JobAnswer::Withdrawn, "{j}");
        }
        // The withdrawal moves a final job no further.
        assert_eq!(st.job("j1").unwrap().unwrap().job, cancelled);
        assert_eq!(st.job("j2").unwrap().unwrap().job, done);
        // Stopped and failed are not final: the question stays open.
        let q4 = question(&st);
        running(&st, "j4");
        st.ask_job_question("j4", &q4).unwrap();
        move_to(&st, "j4", JobState::Stopped).unwrap();
        move_to(&st, "j4", JobState::Failed).unwrap();
        move_to(&st, "j4", JobState::Paused).unwrap();
        assert_eq!(open_questions(&st), 1);
        assert_eq!(
            st.job("j4").unwrap().unwrap().questions[0].answer,
            JobAnswer::Pending
        );
    }

    #[test]
    fn a_question_closed_before_it_is_recorded_is_stored_at_once() {
        let (_d, st) = store();
        let (q1, q2) = (question(&st), question(&st));
        let before = running(&st, "j1");
        answer(&st, &q1);
        let asked = st.ask_job_question("j1", &q1).unwrap();
        assert_eq!(
            (asked.state, asked.rev),
            (JobState::Running, before.rev + 1)
        );
        let full = st.job("j1").unwrap().unwrap();
        let JobAnswer::Answered(stored) = &full.questions[0].answer else {
            panic!("expected answers, got {:?}", full.questions[0].answer)
        };
        assert_eq!(stored[0]["selected"], json!(["A"]));
        // A paused job's withdrawn question is stored the same way.
        let paused = move_to(&st, "j1", JobState::Paused).unwrap();
        st.close_question(DAEMON, &q2, Close::Withdraw).unwrap();
        let asked = st.ask_job_question("j1", &q2).unwrap();
        assert_eq!((asked.state, asked.rev), (JobState::Paused, paused.rev + 1));
        assert_eq!(
            st.job("j1").unwrap().unwrap().questions[1].answer,
            JobAnswer::Withdrawn
        );
        // Resumed, it runs: it waits on nothing.
        assert_eq!(
            move_to(&st, "j1", JobState::Running).unwrap().state,
            JobState::Running
        );
    }

    #[test]
    fn a_job_asks_only_the_ask_tools_questions() {
        let (_d, st) = store();
        let q = question_from(&st, Source::Hook);
        let before = running(&st, "j1");
        let refused = st.ask_job_question("j1", &q);
        assert!(
            matches!(
                refused,
                Err(JobError::Store(CoreError::Invalid {
                    code: "invalid_question",
                    ..
                }))
            ),
            "{refused:?}"
        );
        assert_eq!(st.job("j1").unwrap().unwrap().job, before);
    }

    #[test]
    fn a_question_belongs_to_one_job_and_must_exist() {
        let (_d, st) = store();
        let q = question(&st);
        running(&st, "j1");
        let j2 = running(&st, "j2");
        st.ask_job_question("j1", &q).unwrap();
        for (job, q) in [("j2", q.as_str()), ("j2", "qmissing")] {
            let refused = st.ask_job_question(job, q);
            assert!(
                matches!(
                    refused,
                    Err(JobError::Store(CoreError::Invalid {
                        code: "invalid_question",
                        ..
                    }))
                ),
                "{q}: {refused:?}"
            );
        }
        let full = st.job("j2").unwrap().unwrap();
        assert_eq!(full.job, j2);
        assert!(full.questions.is_empty());
    }

    /// Installs a job listener on `st` that keeps what it hears.
    fn heard(st: &Store) -> std::sync::Arc<std::sync::Mutex<Vec<JobChange>>> {
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let keep = heard.clone();
        st.set_job_listener(move |changes| keep.lock().unwrap().extend(changes));
        heard
    }

    #[test]
    fn every_committed_job_change_is_heard_once_whatever_made_it() {
        let (_d, st) = store();
        let q = question(&st);
        let heard = heard(&st);
        let row = running(&st, "j1");
        let asked = st.ask_job_question("j1", &q).unwrap();
        let change = |r: &JobRow| JobChange {
            id: r.id.clone(),
            client: r.client.clone(),
            state: r.state,
            step: r.step.clone(),
            rev: r.rev,
        };
        assert_eq!(
            std::mem::take(&mut *heard.lock().unwrap()),
            vec![
                JobChange {
                    state: JobState::Queued,
                    rev: 1,
                    ..change(&row)
                },
                change(&row),
                change(&asked)
            ]
        );
        // The answer's transaction is the questions' own.
        answer(&st, &q);
        let after = st.job("j1").unwrap().unwrap().job;
        assert_eq!(after.state, JobState::Running);
        assert_eq!(*heard.lock().unwrap(), vec![change(&after)]);
        heard.lock().unwrap().clear();
        // A rolled-back write is not heard of.
        let refused: JobResult<()> = st.job_tx(|tx| {
            tx.execute("UPDATE jobs SET rev = rev + 1 WHERE id = 'j1'", [])?;
            Err(JobError::Conflict)
        });
        assert!(refused.is_err());
        assert!(heard.lock().unwrap().is_empty());
    }

    /// A change job naming `aid`, with a step.
    fn change_of(st: &Store, id: &str, aid: &str) -> JobRow {
        st.create_job(NewJob {
            kind: "change".into(),
            artifact_id: Some(aid.into()),
            ..new_job(id)
        })
        .unwrap();
        st.commit_step(id, "turn:1", json!({}), false).unwrap().1
    }

    fn rows_of(st: &Store, id: &str) -> i64 {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COUNT(*) FROM jobs WHERE id = ?1)
                      + (SELECT COUNT(*) FROM job_steps WHERE job_id = ?1)
                      + (SELECT COUNT(*) FROM job_threads WHERE job_id = ?1)
                      + (SELECT COUNT(*) FROM job_questions WHERE job_id = ?1)",
                params![id],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    #[test]
    fn a_deleted_artifact_cancels_its_open_jobs_and_a_purge_takes_them() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        let open = change_of(&st, "j1", aid.as_str());
        change_of(&st, "j2", aid.as_str());
        st.commit_step("j2", "reply", json!({}), true).unwrap();
        // A make elsewhere that answers a thread of the artifact.
        running(&st, "j3");
        let answering = st.set_job_thread("j3", &t, "c1", false).unwrap();
        let heard = heard(&st);
        st.delete_artifact(DAEMON, &aid).unwrap();
        let cancelled = st.job("j1").unwrap().unwrap().job;
        assert_eq!(
            (cancelled.state, cancelled.rev),
            (JobState::Cancelled, open.rev + 1)
        );
        assert_eq!(cancelled.error, Some(json!({"code": "artifact_deleted"})));
        assert_eq!(
            *heard.lock().unwrap(),
            vec![JobChange {
                id: "j1".into(),
                client: "app".into(),
                state: JobState::Cancelled,
                step: Some("turn:1".into()),
                rev: open.rev + 1,
            }]
        );
        heard.lock().unwrap().clear();
        st.with_write(|c| {
            c.execute(
                "UPDATE artifacts SET capabilities_json = 'nope' WHERE id = ?1",
                [aid.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(st.delete_corrupt_deleted_rows().unwrap(), 1);
        assert_eq!((rows_of(&st, "j1"), rows_of(&st, "j2")), (0, 0));
        let lost = st.job("j3").unwrap().unwrap();
        assert!(lost.threads.is_empty());
        assert_eq!(
            (lost.job.state, lost.job.rev),
            (JobState::Running, answering.rev + 1)
        );
        assert_eq!(
            heard
                .lock()
                .unwrap()
                .iter()
                .map(|c| (c.id.as_str(), c.rev))
                .collect::<Vec<_>>(),
            vec![("j3", answering.rev + 1)]
        );
    }

    #[test]
    fn deleting_a_thread_a_job_answers_moves_the_job() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let (t1, t2) = (thread_on(&st, &aid), thread_on(&st, &aid));
        running(&st, "j1");
        st.set_job_thread("j1", &t1, "c1", false).unwrap();
        let before = st.set_job_thread("j1", &t2, "c2", true).unwrap();
        let heard = heard(&st);
        st.delete_thread(DAEMON, &t1).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(full.job.rev, before.rev + 1);
        assert_eq!(
            full.threads
                .iter()
                .map(|t| t.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec![t2.as_str()]
        );
        assert_eq!(heard.lock().unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_draft_or_an_empty_artifact_takes_its_jobs_and_consents() {
        let (_d, st) = store();
        let req = serde_json::from_value(json!({
            "title": "Draft",
            "files": {"index.html": {"content": "<main>draft</main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let draft = st
            .create_draft(DAEMON, None, clax_core::publish::validate(req).unwrap())
            .unwrap();
        let did = clax_core::ArtifactId::parse(&draft.artifact.id).unwrap();
        let app = artifact(&st, None);
        change_of(&st, "j1", did.as_str());
        st.put_agent_read_consent(did.as_str(), app.as_str())
            .unwrap();
        st.put_agent_read_consent(app.as_str(), did.as_str())
            .unwrap();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO draft_renders (draft_id, digest, clean, created_at)
                 VALUES (?1, 'ab', 1, 'x')",
                [did.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let heard = heard(&st);
        assert!(st.delete_draft(DAEMON, &did).unwrap());
        assert_eq!(rows_of(&st, "j1"), 0);
        assert_eq!(
            heard
                .lock()
                .unwrap()
                .iter()
                .map(|c| (c.id.as_str(), c.state))
                .collect::<Vec<_>>(),
            vec![("j1", JobState::Cancelled)]
        );
        assert!(st.agent_read_consents().unwrap().is_empty());
        let renders: i64 = st
            .with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM draft_renders", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(renders, 0);
        // An artifact whose first version never landed goes with its jobs.
        st.with_write(|c| {
            c.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES ('zzzzzzzzzzzz', 'z', 'x', 'x', 0, '1')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        change_of(&st, "j2", "zzzzzzzzzzzz");
        assert_eq!(st.delete_zero_version_artifacts().unwrap(), 1);
        assert_eq!(rows_of(&st, "j2"), 0);
    }

    #[test]
    fn the_schema_refuses_values_no_job_can_have() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        st.create_job(new_job("j1")).unwrap();
        let a = aid.as_str();
        let bad = [
            "INSERT INTO jobs (id, client, kind, state, input_json, rev, created_at, updated_at)
             VALUES ('j2', 'app', 'make', 'queued', '{}', 0, 'x', 'x')"
                .into(),
            "UPDATE jobs SET rev = -1 WHERE id = 'j1'".into(),
            "UPDATE jobs SET input_json = 'not json' WHERE id = 'j1'".into(),
            "UPDATE jobs SET plan_json = '{' WHERE id = 'j1'".into(),
            "UPDATE jobs SET carry_json = 'x' WHERE id = 'j1'".into(),
            "UPDATE jobs SET error_json = 'x' WHERE id = 'j1'".into(),
            "INSERT INTO job_steps (job_id, key, result_json, created_at)
             VALUES ('j1', 'turn:1', 'x', 'x')"
                .into(),
            format!(
                "INSERT INTO job_threads (job_id, thread_id, comment_id, stopped)
                 VALUES ('j1', '{t}', 'c1', 2)"
            ),
            format!(
                "INSERT INTO draft_renders (draft_id, digest, clean, created_at)
                 VALUES ('{a}', 'ab', 5, 'x')"
            ),
            "INSERT INTO client_imports (key, done_at, summary_json) VALUES ('k', 'x', 'x')".into(),
        ];
        for sql in bad {
            let err = st.with_write(|c| Ok(c.execute(&sql, [])?)).expect_err(&sql);
            assert!(
                err.to_string().contains("CHECK constraint failed"),
                "{sql}: {err}"
            );
        }
        // Answers stored with an outcome that is no answer.
        let q = question(&st);
        let err = st
            .with_write(|c| {
                Ok(c.execute(
                    "INSERT INTO job_questions (job_id, question_id, outcome, answers_json)
                     VALUES ('j1', ?1, 'declined', '[]')",
                    [q.as_str()],
                )?)
            })
            .unwrap_err();
        assert!(err.to_string().contains("CHECK constraint failed"), "{err}");
    }

    #[test]
    fn a_refusal_after_a_write_rolls_the_write_back() {
        let (_d, st) = store();
        st.create_job(new_job("j1")).unwrap();
        let refused: JobResult<()> = st.job_tx(|tx| {
            tx.execute(
                "INSERT INTO job_steps (job_id, key, result_json, created_at)
                 VALUES ('j1', 'turn:1', '{}', 'x')",
                [],
            )?;
            Err(JobError::Conflict)
        });
        assert!(matches!(refused, Err(JobError::Conflict)));
        let full = st.job("j1").unwrap().unwrap();
        assert!(full.steps.is_empty(), "rolled back: {:?}", full.steps);
        // A refusal whose row it carries is handed back as it was.
        let row = full.job;
        let moved: JobResult<()> = st.job_tx(|tx| {
            tx.execute("UPDATE jobs SET title = 'changed' WHERE id = 'j1'", [])?;
            Err(JobError::Moved(Box::new(row.clone())))
        });
        let Err(JobError::Moved(back)) = moved else {
            panic!("expected Moved, got {moved:?}")
        };
        assert_eq!(*back, row);
        assert_eq!(st.job("j1").unwrap().unwrap().job, row);
    }

    #[test]
    fn consents_round_trip() {
        let (_d, st) = store();
        let (a, b) = (artifact(&st, None), artifact(&st, None));
        let (a, b) = (a.as_str().to_string(), b.as_str().to_string());
        assert!(st.agent_read_consents().unwrap().is_empty());
        st.put_agent_read_consent(&b, &a).unwrap();
        st.put_agent_read_consent(&b, &a).unwrap();
        st.put_agent_read_consent(&a, &b).unwrap();
        let mut both = vec![(a.clone(), b.clone()), (b.clone(), a.clone())];
        both.sort();
        assert_eq!(st.agent_read_consents().unwrap(), both);
        assert!(st.delete_agent_read_consent(&b, &a).unwrap());
        assert!(!st.delete_agent_read_consent(&b, &a).unwrap());
        assert_eq!(st.agent_read_consents().unwrap(), vec![(a, b)]);
    }
}
